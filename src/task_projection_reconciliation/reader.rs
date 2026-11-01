//! Bounded, read-only packet input. This reader does not replay a TaskLedger or
//! draw projection, scheduling, admission, or publication conclusions.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::{Path, PathBuf};

use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::ser::{SerializeMap, SerializeSeq};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value as SerdeValue;
use sha2::{Digest, Sha256};

pub(crate) const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
pub(crate) const MAX_INPUT_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_INPUT_FILES: usize = 256;
pub(crate) const MAX_ROWS: usize = 16_384;
const MAX_ISSUES: usize = 256;
const MAX_JSON_NODES: usize = MAX_ROWS * 16;

type PacketResult<T> = Result<T, PacketError>;

/// Reader-owned JSON values isolate exact numeric input from serde_json's
/// workspace-wide feature choices. This is a parsed input representation, not
/// another ledger model or a projection conclusion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum JsonValue {
    Null,
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<Self>),
    Object(BTreeMap<String, Self>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct JsonNumber(String);

type Value = JsonValue;
type Map<K, V> = BTreeMap<K, V>;

impl JsonValue {
    pub(crate) fn as_object(&self) -> Option<&BTreeMap<String, Self>> {
        match self {
            Self::Object(value) => Some(value),
            _ => None,
        }
    }
    pub(crate) fn as_array(&self) -> Option<&Vec<Self>> {
        match self {
            Self::Array(value) => Some(value),
            _ => None,
        }
    }
    pub(crate) fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }
    pub(crate) fn is_object(&self) -> bool {
        self.as_object().is_some()
    }
    pub(crate) fn is_array(&self) -> bool {
        self.as_array().is_some()
    }
    pub(crate) fn is_string(&self) -> bool {
        self.as_str().is_some()
    }
    pub(crate) fn get(&self, key: &str) -> Option<&Self> {
        self.as_object().and_then(|value| value.get(key))
    }
    #[cfg(test)]
    fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }
    #[cfg(test)]
    fn is_number(&self) -> bool {
        matches!(self, Self::Number(_))
    }
}

/// Stable diagnostics deliberately exclude operating-system or input prose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PacketError {
    InvalidRoot,
    UnsafePath,
    SymlinkInput,
    NotRegularFile,
    FileBudgetExceeded,
    ByteBudgetExceeded,
    InputIo,
    InvalidJson,
    WrongJsonShape,
    UnsupportedSchema,
}

impl fmt::Display for PacketError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::InvalidRoot => "packet root is not an available directory",
            Self::UnsafePath => "unsafe packet path",
            Self::SymlinkInput => "symlink packet input",
            Self::NotRegularFile => "packet input is not a regular file",
            Self::FileBudgetExceeded => "input file budget exceeded",
            Self::ByteBudgetExceeded => "input byte budget exceeded",
            Self::InputIo => "packet input I/O failed",
            Self::InvalidJson => "invalid strict JSON input",
            Self::WrongJsonShape => "wrong JSON shape",
            Self::UnsupportedSchema => "unsupported schema",
        })
    }
}

impl std::error::Error for PacketError {}

#[derive(Clone, Copy, Debug)]
struct Limits {
    file_bytes: usize,
    input_bytes: usize,
    input_files: usize,
    rows: usize,
    issues: usize,
}

impl Limits {
    const PRODUCTION: Self = Self {
        file_bytes: MAX_FILE_BYTES,
        input_bytes: MAX_INPUT_BYTES,
        input_files: MAX_INPUT_FILES,
        rows: MAX_ROWS,
        issues: MAX_ISSUES,
    };
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct Source {
    pub(crate) path: String,
    pub(crate) bytes: usize,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub(crate) struct Issue {
    pub(crate) code: &'static str,
    pub(crate) path: String,
    pub(crate) identity: String,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum JsonShape {
    Object,
    Array,
}

impl JsonShape {
    fn matches(self, value: &Value) -> bool {
        match self {
            Self::Object => value.is_object(),
            Self::Array => value.is_array(),
        }
    }
}

#[derive(Debug)]
enum CachedInput {
    Missing,
    Read(Vec<u8>),
    Failed(PacketError),
}

/// The root is resolved once. Each admitted unique read attempt is cached,
/// including missing and invalid inputs; revisits never consume more budget.
pub(crate) struct PacketReader {
    root: PathBuf,
    limits: Limits,
    raw: BTreeMap<String, CachedInput>,
    sources: BTreeMap<String, Source>,
    coverage: BTreeMap<String, String>,
    total: usize,
    issues: BTreeSet<Issue>,
    issues_overflow: bool,
    input_unavailable: bool,
}

impl PacketReader {
    pub(crate) fn new(root: &Path) -> PacketResult<Self> {
        let root = fs::canonicalize(root).map_err(|_| PacketError::InvalidRoot)?;
        if !root.is_dir() {
            return Err(PacketError::InvalidRoot);
        }
        Ok(Self {
            root,
            limits: Limits::PRODUCTION,
            raw: BTreeMap::new(),
            sources: BTreeMap::new(),
            coverage: BTreeMap::new(),
            total: 0,
            issues: BTreeSet::new(),
            issues_overflow: false,
            input_unavailable: false,
        })
    }

    #[cfg(test)]
    fn with_limits(root: &Path, limits: Limits) -> PacketResult<Self> {
        let mut reader = Self::new(root)?;
        reader.limits = limits;
        Ok(reader)
    }

    pub(crate) fn sources(&self) -> &BTreeMap<String, Source> {
        &self.sources
    }

    pub(crate) fn coverage(&self) -> &BTreeMap<String, String> {
        &self.coverage
    }

    pub(crate) fn issues(&self) -> &BTreeSet<Issue> {
        &self.issues
    }

    pub(crate) fn issues_overflow(&self) -> bool {
        self.issues_overflow
    }

    pub(crate) fn input_unavailable(&self) -> bool {
        self.input_unavailable
    }

    pub(crate) fn mark_unavailable(&mut self, code: &'static str, path: &str) {
        self.input_unavailable = true;
        self.issue(code, path, "");
    }

    pub(crate) fn total_bytes(&self) -> usize {
        self.total
    }

    pub(crate) fn attempts(&self) -> usize {
        self.raw.len()
    }

    fn issue(&mut self, code: &'static str, path: &str, identity: &str) {
        let row = Issue {
            code,
            path: label(path),
            identity: label(identity),
        };
        if self.issues.contains(&row) {
            return;
        }
        if self.issues.len() < self.limits.issues {
            self.issues.insert(row);
        } else {
            self.issues_overflow = true;
        }
    }

    fn invalid(&mut self, name: &str) {
        // Budget-denied names must not create an unbounded second inventory.
        if self.raw.contains_key(name) {
            self.coverage.insert(name.to_owned(), "invalid".to_owned());
        }
        self.input_unavailable = true;
        self.issue("invalid_projection", name, "");
    }

    /// Validate canonical POSIX spelling and reject every existing symlink
    /// below the already-resolved root. This does not charge a read attempt.
    pub(crate) fn confined_path(&self, name: &str) -> PacketResult<PathBuf> {
        if name.is_empty() || name.contains(['\\', '\0']) {
            return Err(PacketError::UnsafePath);
        }
        let mut target = self.root.clone();
        for part in name.split('/') {
            if part.is_empty() || part == "." || part == ".." {
                return Err(PacketError::UnsafePath);
            }
            // Also fail closed on platform-specific prefixes when compiled on
            // a non-POSIX host, rather than allowing PathBuf::push to replace root.
            if !matches!(
                Path::new(part).components().next(),
                Some(std::path::Component::Normal(_))
            ) {
                return Err(PacketError::UnsafePath);
            }
            target.push(part);
            match fs::symlink_metadata(&target) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(PacketError::SymlinkInput);
                }
                Ok(_) => {}
                Err(error)
                    if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {}
                Err(_) => return Err(PacketError::InputIo),
            }
        }
        Ok(target)
    }

    fn read_uncached(&mut self, name: &str) -> PacketResult<CachedInput> {
        let path = self.confined_path(name)?;
        let metadata = match fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error)
                if matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) =>
            {
                self.coverage
                    .insert(name.to_owned(), "unavailable".to_owned());
                return Ok(CachedInput::Missing);
            }
            Err(_) => return Err(PacketError::InputIo),
        };
        if !metadata.file_type().is_file() {
            return Err(PacketError::NotRegularFile);
        }
        let remaining = self.limits.input_bytes.saturating_sub(self.total);
        if remaining == 0 {
            return Err(PacketError::ByteBudgetExceeded);
        }
        let read_limit = self.limits.file_bytes.min(remaining) + 1;
        let file = File::open(path).map_err(|_| PacketError::InputIo)?;
        let mut data = Vec::new();
        let read_result = file.take(read_limit as u64).read_to_end(&mut data);
        // The one probe byte (and bytes obtained before an I/O failure) count.
        self.total += data.len();
        read_result.map_err(|_| PacketError::InputIo)?;
        if data.len() > self.limits.file_bytes || self.total > self.limits.input_bytes {
            return Err(PacketError::ByteBudgetExceeded);
        }
        self.sources.insert(
            name.to_owned(),
            Source {
                path: name.to_owned(),
                bytes: data.len(),
                sha256: format!("{:x}", Sha256::digest(&data)),
            },
        );
        self.coverage.insert(name.to_owned(), "read".to_owned());
        Ok(CachedInput::Read(data))
    }

    pub(crate) fn read(&mut self, name: &str, required: bool) -> PacketResult<Option<&[u8]>> {
        if !self.raw.contains_key(name) {
            if self.raw.len() >= self.limits.input_files {
                self.invalid(name);
                return Err(PacketError::FileBudgetExceeded);
            }
            // Reserve the attempt before path validation or I/O.
            self.raw.insert(name.to_owned(), CachedInput::Missing);
            let attempt = match self.read_uncached(name) {
                Ok(attempt) => attempt,
                Err(error) => {
                    self.invalid(name);
                    CachedInput::Failed(error)
                }
            };
            self.raw.insert(name.to_owned(), attempt);
        }
        if required && matches!(self.raw.get(name), Some(CachedInput::Missing)) {
            self.issue("missing_projection", name, "");
        }
        match self.raw.get(name) {
            Some(CachedInput::Read(data)) => Ok(Some(data)),
            Some(CachedInput::Failed(error)) => Err(*error),
            _ => Ok(None),
        }
    }

    pub(crate) fn load_json(&mut self, name: &str, required: bool) -> PacketResult<Option<Value>> {
        let result = self
            .read(name, required)
            .and_then(|data| data.map(strict_json).transpose());
        if result.is_err() {
            self.invalid(name);
        }
        result
    }

    pub(crate) fn load(
        &mut self,
        name: &str,
        shape: JsonShape,
        schema: Option<&str>,
        required: bool,
    ) -> PacketResult<Option<Value>> {
        let Some(value) = self.load_json(name, required)? else {
            return Ok(None);
        };
        let error = if !shape.matches(&value) {
            Some(PacketError::WrongJsonShape)
        } else if schema
            .is_some_and(|schema| value.get("schema").and_then(Value::as_str) != Some(schema))
        {
            Some(PacketError::UnsupportedSchema)
        } else {
            None
        };
        if let Some(error) = error {
            self.invalid(name);
            return Err(error);
        }
        Ok(Some(value))
    }

    pub(crate) fn rows<'a>(
        &mut self,
        document: Option<&'a Value>,
        path: &str,
        key: Option<&str>,
    ) -> Vec<&'a Map<String, Value>> {
        let Some(document) = document else {
            return Vec::new();
        };
        let value = match key {
            Some(key) => document.as_object().and_then(|object| object.get(key)),
            None => Some(document),
        };
        if let Some(rows) = value.and_then(Value::as_array)
            && rows.len() <= self.limits.rows
            && rows.iter().all(Value::is_object)
        {
            return rows.iter().filter_map(Value::as_object).collect();
        }
        self.input_unavailable = true;
        self.issue("invalid_projection_rows", path, key.unwrap_or("rows"));
        Vec::new()
    }

    pub(crate) fn strings<'a>(
        &mut self,
        document: &'a Value,
        path: &str,
        key: &str,
    ) -> Vec<&'a str> {
        if let Some(rows) = document
            .as_object()
            .and_then(|object| object.get(key))
            .and_then(Value::as_array)
            && rows.len() <= self.limits.rows
            && rows.iter().all(Value::is_string)
        {
            return rows.iter().filter_map(Value::as_str).collect();
        }
        self.input_unavailable = true;
        self.issue("invalid_projection_rows", path, key);
        Vec::new()
    }

    pub(crate) fn mapping<'a>(
        &mut self,
        document: &'a Value,
        path: &str,
        key: &str,
        identity: &str,
    ) -> Option<&'a Map<String, Value>> {
        if let Some(object) = document.as_object() {
            match object.get(key) {
                None | Some(Value::Null) => return None,
                Some(Value::Object(mapping)) => return Some(mapping),
                _ => {}
            }
        }
        self.input_unavailable = true;
        self.issue(
            "invalid_projection_object",
            path,
            if identity.is_empty() { key } else { identity },
        );
        None
    }

    pub(crate) fn lease_quantities(
        &mut self,
        lease: &Map<String, Value>,
        path: &str,
        identity: &str,
    ) -> Option<LeaseQuantities> {
        let cpu = lease.get("cpu").and_then(integer_u32);
        let memory_mb = lease.get("memory_mb").and_then(integer_u64);
        let disk_mb = lease.get("disk_mb").and_then(integer_u64);
        let timeout_sec = lease.get("timeout_sec").and_then(integer_u64);
        for (key, valid) in [
            ("cpu", cpu.is_some()),
            ("memory_mb", memory_mb.is_some()),
            ("disk_mb", disk_mb.is_some()),
            ("timeout_sec", timeout_sec.is_some()),
        ] {
            if !valid {
                self.input_unavailable = true;
                self.issue(
                    "invalid_lease_quantity",
                    path,
                    &format!("{}:{key}", label(identity)),
                );
            }
        }
        Some(LeaseQuantities {
            cpu: cpu?,
            memory_mb: memory_mb?,
            disk_mb: disk_mb?,
            timeout_sec: timeout_sec?,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LeaseQuantities {
    pub(crate) cpu: u32,
    pub(crate) memory_mb: u64,
    pub(crate) disk_mb: u64,
    pub(crate) timeout_sec: u64,
}

pub(crate) fn integer_u32(value: &Value) -> Option<u32> {
    integer_u128(value).and_then(|value| u32::try_from(value).ok())
}

pub(crate) fn integer_u64(value: &Value) -> Option<u64> {
    integer_u128(value).and_then(|value| u64::try_from(value).ok())
}

pub(crate) fn integer_u128(value: &Value) -> Option<u128> {
    match value {
        Value::Number(JsonNumber(lexeme)) if !lexeme.contains(['.', 'e', 'E']) => {
            lexeme.parse().ok()
        }
        _ => None,
    }
}

fn label(value: &str) -> String {
    let safe: String = value
        .chars()
        .map(|character| {
            // Rust's debug-printable boundary also rejects invisible format,
            // private-use and unassigned characters. It deliberately sanitizes
            // combining marks more tightly than Python's isprintable().
            let unicode_escape = character.escape_debug().take(3).eq(['\\', 'u', '{']);
            if character.is_control()
                || (character.is_whitespace() && character != ' ')
                || unicode_escape
            {
                '?'
            } else {
                character
            }
        })
        .collect();
    if safe.chars().count() <= 120 {
        return safe;
    }
    format!(
        "{}~{:.16}",
        safe.chars().take(96).collect::<String>(),
        format!("{:x}", Sha256::digest(value.as_bytes()))
    )
}

// A validation pass before constructing Value keeps duplicate detection from
// losing overwritten object members, including inside arrays and escaped keys.
struct UniqueJson;

impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(UniqueVisitor)
    }
}

struct UniqueVisitor;

impl<'de> Visitor<'de> for UniqueVisitor {
    type Value = UniqueJson;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("strict JSON with unique object keys")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
        if value.is_finite() {
            Ok(UniqueJson)
        } else {
            Err(E::custom("non-finite JSON number"))
        }
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        Ok(UniqueJson)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut count = 0;
        while sequence.next_element::<UniqueJson>()?.is_some() {
            if count >= MAX_ROWS {
                return Err(de::Error::custom("JSON container budget exceeded"));
            }
            count += 1;
        }
        Ok(UniqueJson)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut object: A) -> Result<Self::Value, A::Error> {
        let mut keys = BTreeSet::new();
        while let Some(key) = object.next_key::<String>()? {
            if keys.len() >= MAX_ROWS {
                return Err(de::Error::custom("JSON container budget exceeded"));
            }
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate JSON key"));
            }
            object.next_value::<UniqueJson>()?;
        }
        Ok(UniqueJson)
    }
}

pub(crate) fn strict_json(data: &[u8]) -> PacketResult<Value> {
    if data.len() > MAX_FILE_BYTES {
        return Err(PacketError::InvalidJson);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(data);
    UniqueJson::deserialize(&mut deserializer).map_err(|_| PacketError::InvalidJson)?;
    deserializer.end().map_err(|_| PacketError::InvalidJson)?;
    NativeDecoder {
        data,
        cursor: 0,
        nodes: 0,
    }
    .parse()
}

/// Independent bounded token decoding retains numeric lexemes. The preliminary
/// validation pass never supplies numeric values to this representation.
struct NativeDecoder<'a> {
    data: &'a [u8],
    cursor: usize,
    nodes: usize,
}

impl NativeDecoder<'_> {
    const MAX_DEPTH: usize = 128;

    fn parse(mut self) -> PacketResult<Value> {
        if self.data.len() > MAX_FILE_BYTES {
            return Err(PacketError::InvalidJson);
        }
        self.whitespace()?;
        let value = self.value(0)?;
        self.whitespace()?;
        if self.cursor != self.data.len() {
            return Err(PacketError::InvalidJson);
        }
        Ok(value)
    }

    fn peek(&self) -> Option<u8> {
        self.data.get(self.cursor).copied()
    }

    fn advance(&mut self, bytes: usize) -> PacketResult<()> {
        let cursor = self
            .cursor
            .checked_add(bytes)
            .ok_or(PacketError::InvalidJson)?;
        if cursor > self.data.len() {
            return Err(PacketError::InvalidJson);
        }
        self.cursor = cursor;
        Ok(())
    }

    fn eat(&mut self, byte: u8) -> PacketResult<bool> {
        if self.peek() != Some(byte) {
            return Ok(false);
        }
        self.advance(1)?;
        Ok(true)
    }

    fn expect(&mut self, byte: u8) -> PacketResult<()> {
        if self.eat(byte)? {
            Ok(())
        } else {
            Err(PacketError::InvalidJson)
        }
    }

    fn whitespace(&mut self) -> PacketResult<()> {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.advance(1)?;
        }
        Ok(())
    }

    fn literal(&mut self, literal: &[u8], value: Value) -> PacketResult<Value> {
        if !self
            .data
            .get(self.cursor..)
            .is_some_and(|remaining| remaining.starts_with(literal))
        {
            return Err(PacketError::InvalidJson);
        }
        self.advance(literal.len())?;
        Ok(value)
    }

    fn value(&mut self, depth: usize) -> PacketResult<Value> {
        if depth >= Self::MAX_DEPTH {
            return Err(PacketError::InvalidJson);
        }
        self.node()?;
        self.whitespace()?;
        match self.peek() {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::String),
            Some(b't') => self.literal(b"true", Value::Bool(true)),
            Some(b'f') => self.literal(b"false", Value::Bool(false)),
            Some(b'n') => self.literal(b"null", Value::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(PacketError::InvalidJson),
        }
    }

    fn node(&mut self) -> PacketResult<()> {
        if self.nodes >= MAX_JSON_NODES {
            return Err(PacketError::InvalidJson);
        }
        self.nodes += 1;
        Ok(())
    }

    fn string(&mut self) -> PacketResult<String> {
        let start = self.cursor;
        self.expect(b'"')?;
        let mut escaped = false;
        loop {
            let byte = self.peek().ok_or(PacketError::InvalidJson)?;
            self.advance(1)?;
            if !escaped && byte == b'"' {
                let token = self
                    .data
                    .get(start..self.cursor)
                    .ok_or(PacketError::InvalidJson)?;
                return serde_json::from_slice::<String>(token)
                    .map_err(|_| PacketError::InvalidJson);
            }
            escaped = !escaped && byte == b'\\';
        }
    }

    fn array(&mut self, depth: usize) -> PacketResult<Value> {
        self.expect(b'[')?;
        self.whitespace()?;
        let mut array = Vec::new();
        if self.eat(b']')? {
            return Ok(Value::Array(array));
        }
        loop {
            if array.len() >= MAX_ROWS {
                return Err(PacketError::InvalidJson);
            }
            array.push(self.value(depth + 1)?);
            self.whitespace()?;
            if self.eat(b']')? {
                return Ok(Value::Array(array));
            }
            self.expect(b',')?;
        }
    }

    fn object(&mut self, depth: usize) -> PacketResult<Value> {
        self.expect(b'{')?;
        self.whitespace()?;
        let mut object = BTreeMap::new();
        if self.eat(b'}')? {
            return Ok(Value::Object(object));
        }
        loop {
            if object.len() >= MAX_ROWS {
                return Err(PacketError::InvalidJson);
            }
            self.whitespace()?;
            self.node()?;
            let key = self.string()?;
            if object.contains_key(&key) {
                return Err(PacketError::InvalidJson);
            }
            self.whitespace()?;
            self.expect(b':')?;
            let value = self.value(depth + 1)?;
            object.insert(key, value);
            self.whitespace()?;
            if self.eat(b'}')? {
                return Ok(Value::Object(object));
            }
            self.expect(b',')?;
        }
    }

    fn digits(&mut self) -> PacketResult<usize> {
        let start = self.cursor;
        while matches!(self.peek(), Some(b'0'..=b'9')) {
            self.advance(1)?;
        }
        Ok(self.cursor - start)
    }

    fn number(&mut self) -> PacketResult<Value> {
        let start = self.cursor;
        self.eat(b'-')?;
        match self.peek() {
            Some(b'0') => self.advance(1)?,
            Some(b'1'..=b'9') => {
                self.digits()?;
            }
            _ => return Err(PacketError::InvalidJson),
        }
        if self.eat(b'.')? && self.digits()? == 0 {
            return Err(PacketError::InvalidJson);
        }
        if matches!(self.peek(), Some(b'e' | b'E')) {
            self.advance(1)?;
            if matches!(self.peek(), Some(b'+' | b'-')) {
                self.advance(1)?;
            }
            if self.digits()? == 0 {
                return Err(PacketError::InvalidJson);
            }
        }
        let token = self
            .data
            .get(start..self.cursor)
            .ok_or(PacketError::InvalidJson)?;
        let lexeme = std::str::from_utf8(token).map_err(|_| PacketError::InvalidJson)?;
        // This explicit finite-f64 envelope is wider than every u128 domain,
        // and rejects overflow even if future workspace feature choices change.
        if !lexeme.parse::<f64>().is_ok_and(f64::is_finite) {
            return Err(PacketError::InvalidJson);
        }
        Ok(Value::Number(JsonNumber(if lexeme == "-0" {
            "0".to_owned()
        } else {
            lexeme.to_owned()
        })))
    }
}

pub(crate) fn canonical_json(value: &Value) -> PacketResult<Vec<u8>> {
    fn write(value: &Value, output: &mut Vec<u8>) -> PacketResult<()> {
        match value {
            Value::Null => output.extend_from_slice(b"null"),
            Value::Bool(value) => output.extend_from_slice(if *value { b"true" } else { b"false" }),
            Value::Number(JsonNumber(lexeme)) => output.extend_from_slice(lexeme.as_bytes()),
            Value::String(value) => {
                output.extend(serde_json::to_vec(value).map_err(|_| PacketError::InvalidJson)?)
            }
            Value::Array(array) => {
                output.push(b'[');
                for (index, value) in array.iter().enumerate() {
                    if index != 0 {
                        output.push(b',');
                    }
                    write(value, output)?;
                }
                output.push(b']');
            }
            Value::Object(object) => {
                output.push(b'{');
                for (index, (key, value)) in object.iter().enumerate() {
                    if index != 0 {
                        output.push(b',');
                    }
                    output.extend(serde_json::to_vec(key).map_err(|_| PacketError::InvalidJson)?);
                    output.push(b':');
                    write(value, output)?;
                }
                output.push(b'}');
            }
        }
        Ok(())
    }
    let mut output = Vec::new();
    write(value, &mut output)?;
    output.push(b'\n');
    Ok(output)
}

/// Sorted compact UTF-8 JSON with one trailing LF; independent of serde_json's
/// map-storage feature selection. Number precision is preserved.
pub(crate) fn canonical(value: &SerdeValue) -> PacketResult<Vec<u8>> {
    let mut bytes =
        serde_json::to_vec(&SortedValue(value)).map_err(|_| PacketError::InvalidJson)?;
    bytes.push(b'\n');
    Ok(bytes)
}

struct SortedValue<'a>(&'a SerdeValue);

impl Serialize for SortedValue<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self.0 {
            SerdeValue::Object(object) => {
                let mut entries: Vec<_> = object.iter().collect();
                entries.sort_unstable_by_key(|(key, _)| *key);
                let mut map = serializer.serialize_map(Some(entries.len()))?;
                for (key, value) in entries {
                    map.serialize_entry(key, &SortedValue(value))?;
                }
                map.end()
            }
            SerdeValue::Array(array) => {
                let mut sequence = serializer.serialize_seq(Some(array.len()))?;
                for value in array {
                    sequence.serialize_element(&SortedValue(value))?;
                }
                sequence.end()
            }
            SerdeValue::Number(number)
                if number.to_string().contains(['.', 'e', 'E']) && number.as_f64().is_none() =>
            {
                Err(serde::ser::Error::custom("non-finite JSON number"))
            }
            value => value.serialize(serializer),
        }
    }
}

#[cfg(test)]
#[path = "reader/tests.rs"]
mod tests;
