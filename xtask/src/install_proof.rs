//! Release-only installation-proof producer (#815). Installs a release-shaped
//! artifact (archive + checksum) through the same resolver contract as the
//! Action's `install-mode=release` path and emits a typed, versioned receipt
//! asserting `source_build_used=false`.
//!
//! Trust rules implemented here (see `docs/RELEASE_RUNBOOK.md` "Install-proof
//! assertion and producer" and the resolver gates landed in `action.yml`):
//! - The staged archive and checksum are untrusted input. The harness enforces
//!   the Action resolver's gates itself: checksum receipt + digest match
//!   (`action.yml:487-497`), single-root `ub-review` archive layout
//!   (`action.yml:504-525`), installed binary identity via `--version`
//!   (`action.yml:554-557`), and the Linux x64 platform gate
//!   (`action.yml:486-489`).
//! - Environment/tool-absence probes run before the resolver is consulted and
//!   before any install, and every probe outcome is recorded in the receipt.
//!   A detected host `cargo`, `rustc`, or preinstalled `ub-review` aborts the
//!   run with no receipt: a passing receipt is impossible while a host
//!   toolchain that could have produced the binary is visible to the install.
//! - There is no source-fallback path. Resolver unavailability and every gate
//!   refusal fail closed with no receipt, mirroring the release-mode refusal
//!   at `action.yml:566-572`. This module contains no build step at all, so a
//!   source-fallback attempt has nothing to reach.
//! - [`InstallProofReceipt`] has private fields and no public constructor.
//!   The only code that can produce a success receipt is [`run_with_resolver`],
//!   and it hardcodes `source_build_used = false` after every gate passed. No
//!   CLI flag or API can stamp or override the assertion; a refused run writes
//!   no receipt file.
//! - The resolver transport is a seam ([`ReleaseResolver`]) like
//!   `release_publish.rs`'s publisher seam: the real transport
//!   ([`StagedResolver`]) reads a staged archive/checksum pair with no
//!   network and no credentials; tests inject recording fakes.
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};

use crate::release_bytes as rb;

/// Receipt schema printed by `prove-install`.
pub const RECEIPT_SCHEMA: &str = "ub-review.install_proof.v1";
/// Installed binary name required by the resolver contract.
pub const BINARY_NAME: &str = "ub-review";
/// Tools whose presence invalidates a release-only installation proof.
pub const FORBIDDEN_TOOLS: &[&str] = &["cargo", "rustc", BINARY_NAME];
const MAX_TAR_BYTES: usize = rb::MAX_ARCHIVE_BYTES + 4096;
/// Recorded/compared text bound, mirroring `release_publish.rs`'s `bounded`.
const BOUNDED_TEXT: usize = 400;

/// CLI inputs for `cargo xtask prove-install`. Everything here is a fact the
/// operator or workflow supplies; none of it can flip the receipt assertion.
#[derive(Clone)]
pub struct ProveInstallOptions {
    pub archive: PathBuf,
    pub checksum: PathBuf,
    pub release_tag: String,
    pub asset_name: String,
    pub install_dir: PathBuf,
    pub output: PathBuf,
    pub harness_revision: String,
    /// PATH entries scanned by the tool-absence probes. Defaults to the
    /// process PATH; production runs pass the sanitized installation
    /// environment's PATH. Recorded verbatim in the receipt either way.
    pub probe_path: Option<String>,
    /// Cargo home scanned for cargo-sourced tools in addition to PATH.
    /// Defaults to `$CARGO_HOME` (or `~/.cargo`) when unset.
    pub cargo_home: Option<PathBuf>,
    /// Receipt timestamp override for deterministic tests.
    pub now: Option<u64>,
}

impl ProveInstallOptions {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut archive: Option<PathBuf> = None;
        let mut checksum: Option<PathBuf> = None;
        let mut release_tag: Option<String> = None;
        let mut asset_name: Option<String> = None;
        let mut install_dir: Option<PathBuf> = None;
        let mut output: Option<PathBuf> = None;
        let mut harness_revision: Option<String> = None;
        let mut probe_path: Option<String> = None;
        let mut cargo_home: Option<PathBuf> = None;
        let mut now: Option<u64> = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--archive" => archive = Some(PathBuf::from(take_value(&mut args, "--archive")?)),
                "--checksum" => {
                    checksum = Some(PathBuf::from(take_value(&mut args, "--checksum")?))
                }
                "--release-tag" => release_tag = Some(take_value(&mut args, "--release-tag")?),
                "--asset" => asset_name = Some(take_value(&mut args, "--asset")?),
                "--install-dir" => {
                    install_dir = Some(PathBuf::from(take_value(&mut args, "--install-dir")?))
                }
                "--output" => output = Some(PathBuf::from(take_value(&mut args, "--output")?)),
                "--harness-revision" => {
                    harness_revision = Some(take_value(&mut args, "--harness-revision")?)
                }
                "--probe-path" => probe_path = Some(take_value(&mut args, "--probe-path")?),
                "--cargo-home" => {
                    cargo_home = Some(PathBuf::from(take_value(&mut args, "--cargo-home")?))
                }
                "--now" => now = Some(take_u64(&mut args, "--now")?),
                other => bail!("unexpected prove-install argument `{other}`"),
            }
        }
        let missing =
            |name: &'static str| -> anyhow::Error { anyhow::anyhow!("missing required {name}") };
        Ok(Self {
            archive: archive.ok_or_else(|| missing("--archive <path>"))?,
            checksum: checksum.ok_or_else(|| missing("--checksum <path>"))?,
            release_tag: release_tag.ok_or_else(|| missing("--release-tag <tag>"))?,
            asset_name: asset_name.ok_or_else(|| missing("--asset <name>"))?,
            install_dir: install_dir.ok_or_else(|| missing("--install-dir <dir>"))?,
            output: output.ok_or_else(|| missing("--output <path>"))?,
            harness_revision: harness_revision.ok_or_else(|| missing("--harness-revision <id>"))?,
            probe_path,
            cargo_home,
            now,
        })
    }
}

fn take_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    let value = args
        .next()
        .with_context(|| format!("{flag} requires a value"))?;
    if value.is_empty() {
        bail!("{flag} must not be empty");
    }
    Ok(value)
}

fn take_u64(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<u64> {
    let value = take_value(args, flag)?;
    value
        .parse::<u64>()
        .with_context(|| format!("{flag} must be a non-negative integer, got `{value}`"))
}

/// The release identity a resolver transport fetches.
pub struct ReleaseRequest {
    pub release_tag: String,
    pub asset_name: String,
    pub checksum_asset_name: String,
}

/// Exact artifact bytes plus the resolver path identity recorded in the
/// receipt (the Action records the release URL; the staged transport records
/// the staged file path).
pub struct ResolvedArtifact {
    pub source: String,
    pub archive: Vec<u8>,
    pub checksum: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResolverStop {
    Unavailable { detail: String },
}

impl std::fmt::Display for ResolverStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResolverStop::Unavailable { detail } => {
                write!(f, "release binary unavailable; {detail}")
            }
        }
    }
}

impl std::error::Error for ResolverStop {}

/// Resolver transport seam, mirroring `release_publish.rs`'s publisher seam.
/// The trait can only fetch bytes; it cannot build from source, stamp the
/// receipt, or bypass any gate — those live in this module's contract path.
pub trait ReleaseResolver {
    /// Transport identity recorded verbatim in the receipt.
    fn transport(&self) -> &'static str;
    /// Fetch the exact archive and checksum bytes for the release request.
    fn fetch(
        &mut self,
        request: &ReleaseRequest,
    ) -> std::result::Result<ResolvedArtifact, ResolverStop>;
}

/// Real transport for the local harness: reads a staged archive/checksum
/// pair. This is the no-network, no-credential analogue of the Action's
/// curl download; the resolver contract (checksum, layout, identity,
/// no-fallback) is enforced by this module, not by the transport.
pub struct StagedResolver {
    archive_path: PathBuf,
    checksum_path: PathBuf,
}

impl StagedResolver {
    pub fn new(archive_path: PathBuf, checksum_path: PathBuf) -> Self {
        Self {
            archive_path,
            checksum_path,
        }
    }
}

impl ReleaseResolver for StagedResolver {
    fn transport(&self) -> &'static str {
        "staged-files"
    }

    fn fetch(
        &mut self,
        _request: &ReleaseRequest,
    ) -> std::result::Result<ResolvedArtifact, ResolverStop> {
        let read = |path: &Path| -> std::result::Result<Vec<u8>, ResolverStop> {
            fs::read(path).map_err(|error| ResolverStop::Unavailable {
                detail: format!("{}: {error}", path.display()),
            })
        };
        Ok(ResolvedArtifact {
            source: self.archive_path.display().to_string(),
            archive: read(&self.archive_path)?,
            checksum: read(&self.checksum_path)?,
        })
    }
}

/// Runs the installed binary's `--version`, mirroring the Action's
/// `version_output="$(... 2>&1)"` capture.
pub trait VersionProbe {
    fn probe(&self, binary: &Path) -> std::io::Result<String>;
}

/// Real probe. The child runs with the scanned (tool-absent) PATH so the
/// installed binary cannot resolve host tooling during identity check.
pub struct CommandVersionProbe {
    pub path_env: String,
}

impl VersionProbe for CommandVersionProbe {
    fn probe(&self, binary: &Path) -> std::io::Result<String> {
        let output = Command::new(binary)
            .arg("--version")
            .env("PATH", &self.path_env)
            .output()?;
        if !output.status.success() {
            return Err(std::io::Error::other(format!(
                "exit status {:?}",
                output.status.code()
            )));
        }
        let mut text = String::from_utf8_lossy(&output.stdout).into_owned();
        text.push_str(&String::from_utf8_lossy(&output.stderr));
        // Command substitution strips trailing newlines; the remaining text
        // is exactly what the Action compares against the expected identity.
        Ok(bounded(text.trim_end()))
    }
}

fn bounded(text: &str) -> String {
    text.trim().chars().take(BOUNDED_TEXT).collect()
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ProbeOutcome {
    Absent,
    Present { path: String },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolProbe {
    pub tool: String,
    pub scope: String,
    pub outcome: ProbeOutcome,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeRecord {
    pub scanned_path: Vec<String>,
    pub process_path: Option<String>,
    pub cargo_home: Option<String>,
    pub probes: Vec<ToolProbe>,
}

/// Environment/tool-absence probes, run before the resolver is consulted.
/// Every outcome is recorded; [`detect_forbidden_tools`] converts any
/// detection into a pre-install refusal.
pub fn probe_environment(scanned_path: &str, cargo_home: Option<&Path>) -> ProbeRecord {
    let path_entries: Vec<String> = env_split_paths(scanned_path);
    let process_path = std::env::var("PATH").ok();
    let home = cargo_home_dir(cargo_home);
    let mut probes = Vec::new();
    for tool in FORBIDDEN_TOOLS {
        probes.push(ToolProbe {
            tool: (*tool).to_owned(),
            scope: "path".to_owned(),
            outcome: scan_tool(tool, &path_entries),
        });
    }
    if let Some(home) = &home {
        let bin = home.join("bin");
        for tool in FORBIDDEN_TOOLS {
            probes.push(ToolProbe {
                tool: (*tool).to_owned(),
                scope: "cargo-home".to_owned(),
                outcome: probe_one(&bin, tool),
            });
        }
    }
    ProbeRecord {
        scanned_path: path_entries,
        process_path,
        cargo_home: home.map(|dir| dir.display().to_string()),
        probes,
    }
}

fn env_split_paths(value: &str) -> Vec<String> {
    std::env::split_paths(value)
        .map(|entry| entry.to_string_lossy().into_owned())
        .collect()
}

fn cargo_home_dir(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(dir) = explicit {
        return Some(dir.to_path_buf());
    }
    if let Some(dir) = std::env::var_os("CARGO_HOME") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(PathBuf::from(home).join(".cargo"))
}

fn scan_tool(tool: &str, dirs: &[String]) -> ProbeOutcome {
    for dir in dirs {
        let outcome = probe_one(Path::new(dir), tool);
        if outcome != ProbeOutcome::Absent {
            return outcome;
        }
    }
    ProbeOutcome::Absent
}

fn probe_one(dir: &Path, tool: &str) -> ProbeOutcome {
    let candidate = dir.join(tool);
    if is_executable_candidate(&candidate) {
        ProbeOutcome::Present {
            path: candidate.display().to_string(),
        }
    } else {
        ProbeOutcome::Absent
    }
}

fn is_executable_candidate(path: &Path) -> bool {
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Refuses before any install when a forbidden host tool was detected.
pub fn detect_forbidden_tools(probes: &ProbeRecord) -> std::result::Result<(), InstallStop> {
    for probe in &probes.probes {
        if let ProbeOutcome::Present { path } = &probe.outcome {
            return Err(InstallStop::ToolPresent {
                tool: probe.tool.clone(),
                path: path.clone(),
                scope: probe.scope.clone(),
            });
        }
    }
    Ok(())
}

/// Every refusal of the resolver contract. Any stop means: no receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallStop {
    InvalidRequest {
        detail: String,
    },
    Unavailable {
        detail: String,
    },
    PlatformUnsupported {
        os: String,
        arch: String,
    },
    ChecksumReceipt {
        detail: String,
    },
    ChecksumMismatch {
        expected: String,
        actual: String,
    },
    ArchiveLayout {
        detail: String,
    },
    ExtractionFailed {
        detail: String,
    },
    BinaryMissing {
        detail: String,
    },
    BinaryNotExecutable {
        path: String,
    },
    VersionExecution {
        detail: String,
    },
    VersionIdentity {
        expected: String,
        reported: String,
    },
    ToolPresent {
        tool: String,
        path: String,
        scope: String,
    },
}

impl std::fmt::Display for InstallStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallStop::InvalidRequest { detail } => {
                write!(f, "invalid release request; {detail}")
            }
            InstallStop::Unavailable { detail } => {
                write!(f, "release binary unavailable; {detail}")
            }
            InstallStop::PlatformUnsupported { os, arch } => write!(
                f,
                "release binary unavailable; only Linux x64 runners are supported (host is {os}/{arch})"
            ),
            InstallStop::ChecksumReceipt { detail } => {
                write!(f, "release binary checksum receipt is {detail}")
            }
            InstallStop::ChecksumMismatch { expected, actual } => {
                write!(
                    f,
                    "release binary checksum mismatch (expected {expected}, got {actual})"
                )
            }
            InstallStop::ArchiveLayout { detail } => {
                write!(f, "release binary archive rejected; {detail}")
            }
            InstallStop::ExtractionFailed { detail } => {
                write!(f, "release binary archive extraction failed; {detail}")
            }
            InstallStop::BinaryMissing { detail } => write!(f, "{detail}"),
            InstallStop::BinaryNotExecutable { path } => {
                write!(f, "release binary is not executable: {path}")
            }
            InstallStop::VersionExecution { detail } => {
                write!(f, "release binary --version execution failed: {detail}")
            }
            InstallStop::VersionIdentity { expected, reported } => {
                write!(
                    f,
                    "release binary version mismatch (expected {expected}, got {reported})"
                )
            }
            InstallStop::ToolPresent { tool, path, scope } => write!(
                f,
                "forbidden host tool present: {tool} at {path} (scope {scope}); installation proof aborted before install"
            ),
        }
    }
}

impl std::error::Error for InstallStop {}

impl From<ResolverStop> for InstallStop {
    fn from(stop: ResolverStop) -> Self {
        match stop {
            ResolverStop::Unavailable { detail } => InstallStop::Unavailable { detail },
        }
    }
}

/// Identity of the binary the resolver contract installed, everything the
/// receipt's install/artifact records need.
pub struct InstalledBinary {
    pub platform_os: String,
    pub platform_arch: String,
    pub install_dir: PathBuf,
    pub binary_path: PathBuf,
    pub binary_sha256: String,
    pub version_expected: String,
    pub version_reported: String,
    pub archive_source: String,
    pub archive_sha256: String,
    pub archive_length: u64,
    pub checksum_sha256: String,
}

/// The Action resolver's `install-mode=release` contract
/// (`action.yml:448-572`), applied to fetched bytes. Gate order and refusal
/// semantics mirror the shell step: request validation, platform gate,
/// fetch, checksum receipt + digest match, single-root layout, extraction,
/// executable bit, `--version` identity. There is no source-fallback branch.
pub fn install_release_binary(
    resolver: &mut dyn ReleaseResolver,
    version_probe: &dyn VersionProbe,
    request: &ReleaseRequest,
    platform: (&str, &str),
    install_dir: &Path,
) -> std::result::Result<InstalledBinary, InstallStop> {
    let release_tag = normalize_release_tag(&request.release_tag);
    validate_release_request(&release_tag, &request.asset_name)?;
    if platform.0 != "linux" || platform.1 != "x86_64" {
        return Err(InstallStop::PlatformUnsupported {
            os: platform.0.to_owned(),
            arch: platform.1.to_owned(),
        });
    }
    let artifact = resolver.fetch(request)?;
    let expected_sha = parse_checksum_receipt(&artifact.checksum)?;
    let archive_sha256 = rb::sha256(&artifact.archive);
    if archive_sha256 != expected_sha {
        return Err(InstallStop::ChecksumMismatch {
            expected: expected_sha,
            actual: archive_sha256,
        });
    }
    let candidate_bytes = if is_tgz_asset(&request.asset_name) {
        decode_release_archive(&artifact.archive)?
    } else {
        // The Action's non-tar branch copies the checksum-verified archive
        // bytes to the binary name unchanged (`action.yml:532-536`).
        artifact.archive.clone()
    };
    let binary_path = write_installed_binary(install_dir, &candidate_bytes)?;
    let expected_version = release_tag.strip_prefix('v').unwrap_or(&release_tag);
    let reported =
        version_probe
            .probe(&binary_path)
            .map_err(|error| InstallStop::VersionExecution {
                detail: format!("{}: {error}", binary_path.display()),
            })?;
    let expected_identity = format!("{BINARY_NAME} {expected_version}");
    if reported != expected_identity {
        return Err(InstallStop::VersionIdentity {
            expected: expected_identity,
            reported,
        });
    }
    Ok(InstalledBinary {
        platform_os: platform.0.to_owned(),
        platform_arch: platform.1.to_owned(),
        install_dir: install_dir.to_path_buf(),
        binary_sha256: rb::sha256(&candidate_bytes),
        binary_path,
        version_expected: expected_identity,
        version_reported: reported,
        archive_source: artifact.source,
        archive_sha256,
        archive_length: artifact.archive.len() as u64,
        checksum_sha256: rb::sha256(&artifact.checksum),
    })
}

/// Mirrors the Action's `normalize_release_version` (`action.yml:412-419`).
fn normalize_release_tag(tag: &str) -> String {
    tag.strip_prefix("refs/tags/").unwrap_or(tag).to_owned()
}

/// Mirrors `validate_release_request` (`action.yml:423-435`) and the
/// empty-release-version unavailability (`action.yml:450-453`).
fn validate_release_request(
    release_tag: &str,
    asset_name: &str,
) -> std::result::Result<(), InstallStop> {
    if release_tag.is_empty() {
        return Err(InstallStop::Unavailable {
            detail: "no release-version or action ref is set".to_owned(),
        });
    }
    if !simple_token(release_tag) {
        return Err(InstallStop::InvalidRequest {
            detail: "release-version must be a simple tag name using letters, numbers, dot, underscore, or dash".to_owned(),
        });
    }
    if asset_name.is_empty()
        || asset_name.contains('/')
        || asset_name.contains('\\')
        || asset_name.contains("..")
    {
        return Err(InstallStop::InvalidRequest {
            detail: "release-asset must be a file name, not a path".to_owned(),
        });
    }
    if !simple_token(asset_name) {
        return Err(InstallStop::InvalidRequest {
            detail: "release-asset must use only letters, numbers, dot, underscore, or dash"
                .to_owned(),
        });
    }
    Ok(())
}

fn simple_token(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
}

fn is_tgz_asset(asset_name: &str) -> bool {
    asset_name.ends_with(".tar.gz") || asset_name.ends_with(".tgz")
}

/// Mirrors the Action's `awk 'NF >= 1 {print $1; exit}'` checksum-receipt
/// parse and its `^[A-Fa-f0-9]{64}$` shape check (`action.yml:480-491`).
/// The digest comparison itself stays exact-case like the Action's
/// `[[ "$actual_sha" != "$expected_sha" ]]`.
fn parse_checksum_receipt(checksum: &[u8]) -> std::result::Result<String, InstallStop> {
    let text = String::from_utf8_lossy(checksum);
    let field = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .and_then(|line| line.split_whitespace().next());
    let Some(field) = field else {
        return Err(InstallStop::ChecksumReceipt {
            detail: "empty".to_owned(),
        });
    };
    if field.len() != 64 || !field.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(InstallStop::ChecksumReceipt {
            detail: "malformed".to_owned(),
        });
    }
    Ok(field.to_owned())
}

/// Mirrors the Action's tar listing gate (`action.yml:504-525`): exactly one
/// root-level regular `ub-review` entry, zero root-level non-regular
/// `ub-review` entries, zero nested `ub-review` entries. Returns the root
/// entry's bytes — the candidate installed as the binary.
fn decode_release_archive(archive: &[u8]) -> std::result::Result<Vec<u8>, InstallStop> {
    let listing_failed = |detail: String| InstallStop::ArchiveLayout { detail };
    let mut tar = Vec::new();
    GzDecoder::new(archive)
        .take((MAX_TAR_BYTES + 1) as u64)
        .read_to_end(&mut tar)
        .map_err(|error| listing_failed(format!("listing failed: {error}")))?;
    if tar.len() > MAX_TAR_BYTES {
        return Err(listing_failed("exceeds the decompression bound".to_owned()));
    }
    if tar.len() < 1024 || tar.len() % 512 != 0 {
        return Err(listing_failed("truncated or malformed archive".to_owned()));
    }
    let mut offset = 0;
    let mut root_regular = 0;
    let mut root_nonregular = 0;
    let mut nested = 0;
    let mut candidate: Option<Vec<u8>> = None;
    while offset + 512 <= tar.len() {
        let header = &tar[offset..offset + 512];
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let name = cstring(&header[..100])
            .ok_or_else(|| listing_failed("unreadable entry name".to_owned()))?;
        let prefix = cstring(&header[345..500]).unwrap_or_default();
        let size = octal(&header[124..136])
            .ok_or_else(|| listing_failed("unreadable entry size".to_owned()))?;
        let data_start = offset + 512;
        let data_end = data_start + size as usize;
        if data_end > tar.len() {
            return Err(listing_failed("entry data exceeds the archive".to_owned()));
        }
        let mut full = prefix;
        if !full.is_empty() {
            full.push(b'/');
        }
        full.extend_from_slice(&name);
        let path = String::from_utf8_lossy(&full).into_owned();
        // `(\.\/)*` in the Action's awk pattern: leading "./" is not a path
        // depth; strip it repeatedly before classification.
        let normalized = path.trim_start_matches("./");
        if normalized == BINARY_NAME {
            if matches!(header[156], 0 | b'0') {
                root_regular += 1;
                candidate = Some(tar[data_start..data_end].to_vec());
            } else {
                root_nonregular += 1;
            }
        } else if normalized.split('/').any(|part| part == BINARY_NAME) {
            nested += 1;
        }
        offset = data_end + (512 - size as usize % 512) % 512;
    }
    if root_regular != 1 || root_nonregular != 0 || nested != 0 {
        return Err(listing_failed(format!(
            "release binary archive must contain exactly one root-level ub-review executable (root regular {root_regular}, root non-regular {root_nonregular}, nested {nested})"
        )));
    }
    candidate.ok_or_else(|| listing_failed("root entry content missing".to_owned()))
}

fn cstring(bytes: &[u8]) -> Option<Vec<u8>> {
    let end = bytes.iter().position(|byte| *byte == 0)?;
    Some(bytes[..end].to_vec())
}

fn octal(bytes: &[u8]) -> Option<u64> {
    let digits: Vec<u8> = bytes
        .iter()
        .copied()
        .take_while(|byte| *byte != 0 && *byte != b' ')
        .collect();
    if digits.is_empty() || !digits.iter().all(|byte| (b'0'..=b'7').contains(byte)) {
        return None;
    }
    Some(
        digits
            .iter()
            .fold(0u64, |acc, byte| acc * 8 + u64::from(*byte - b'0')),
    )
}

/// Writes the extracted candidate as the installed binary, mirroring the
/// Action's candidate checks and `chmod +x` (`action.yml:529-552`).
fn write_installed_binary(
    install_dir: &Path,
    candidate_bytes: &[u8],
) -> std::result::Result<PathBuf, InstallStop> {
    let binary_path = install_dir.join(BINARY_NAME);
    if binary_path.exists() {
        // The install directory must be fresh; writing through a
        // pre-existing entry (possibly a symlink) is a side effect the
        // proof cannot own.
        return Err(InstallStop::ExtractionFailed {
            detail: format!(
                "install directory must be fresh; {} already exists",
                binary_path.display()
            ),
        });
    }
    fs::create_dir_all(install_dir).map_err(|error| InstallStop::ExtractionFailed {
        detail: format!("create install dir {}: {error}", install_dir.display()),
    })?;
    fs::write(&binary_path, candidate_bytes).map_err(|error| InstallStop::ExtractionFailed {
        detail: format!("write {}: {error}", binary_path.display()),
    })?;
    let meta = fs::symlink_metadata(&binary_path).map_err(|error| InstallStop::BinaryMissing {
        detail: format!("{}: {error}", binary_path.display()),
    })?;
    if meta.file_type().is_symlink() || !meta.is_file() {
        return Err(InstallStop::BinaryMissing {
            detail: "release binary archive did not contain ub-review".to_owned(),
        });
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&binary_path, fs::Permissions::from_mode(0o755)).map_err(|_| {
            InstallStop::BinaryNotExecutable {
                path: binary_path.display().to_string(),
            }
        })?;
        let mode = fs::metadata(&binary_path)
            .map_err(|_| InstallStop::BinaryNotExecutable {
                path: binary_path.display().to_string(),
            })?
            .permissions()
            .mode();
        if mode & 0o111 == 0 {
            return Err(InstallStop::BinaryNotExecutable {
                path: binary_path.display().to_string(),
            });
        }
    }
    let installed = fs::read(&binary_path).map_err(|error| InstallStop::ExtractionFailed {
        detail: format!("re-read {}: {error}", binary_path.display()),
    })?;
    if installed != candidate_bytes {
        return Err(InstallStop::ExtractionFailed {
            detail: format!(
                "installed bytes at {} diverged from the extracted bytes",
                binary_path.display()
            ),
        });
    }
    Ok(binary_path)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlatformRecord {
    pub os: String,
    pub arch: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArtifactRecord {
    /// Resolver path identity: the release URL on the Action path, the
    /// staged archive path on the local harness path.
    pub source: String,
    pub transport: String,
    pub archive_sha256: String,
    pub archive_length: u64,
    pub checksum_sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallRecord {
    pub install_dir: String,
    pub binary_path: String,
    pub binary_sha256: String,
    pub version_expected: String,
    pub version_reported: String,
}

/// The typed, versioned installation receipt. Every field is private and
/// there is no public constructor: the receipt is produced by a harness run
/// ([`run_with_resolver`]) or parsed back from an emitted file
/// ([`from_json`]). No caller — CLI flag, API, or test — can construct a
/// receipt with `source_build_used` set to any value; the field is
/// hardcoded `false` on the sole success path, after every gate passed.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstallProofReceipt {
    schema: String,
    produced_at: u64,
    /// Recorded producer source revision (trusted fact, verbatim).
    harness_revision: String,
    platform: PlatformRecord,
    release_tag: String,
    expected_version: String,
    asset_name: String,
    checksum_asset_name: String,
    artifact: ArtifactRecord,
    probes: ProbeRecord,
    install: InstallRecord,
    source_build_used: bool,
}

impl InstallProofReceipt {
    pub fn schema(&self) -> &str {
        &self.schema
    }
    pub fn produced_at(&self) -> u64 {
        self.produced_at
    }
    pub fn harness_revision(&self) -> &str {
        &self.harness_revision
    }
    pub fn platform(&self) -> &PlatformRecord {
        &self.platform
    }
    pub fn release_tag(&self) -> &str {
        &self.release_tag
    }
    pub fn expected_version(&self) -> &str {
        &self.expected_version
    }
    pub fn asset_name(&self) -> &str {
        &self.asset_name
    }
    pub fn checksum_asset_name(&self) -> &str {
        &self.checksum_asset_name
    }
    pub fn artifact(&self) -> &ArtifactRecord {
        &self.artifact
    }
    pub fn probes(&self) -> &ProbeRecord {
        &self.probes
    }
    pub fn install(&self) -> &InstallRecord {
        &self.install
    }
    /// The runbook's install-proof assertion. Only ever `false`: the field
    /// is set on the single success path and no constructor, setter, or CLI
    /// flag exists for any other value.
    pub fn source_build_used(&self) -> bool {
        self.source_build_used
    }
    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string_pretty(self).context("serialize install-proof receipt")
    }
    /// Parses a receipt back from retained evidence, rejecting any other
    /// schema version.
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        let receipt: Self = serde_json::from_slice(bytes).context("parse install-proof receipt")?;
        if receipt.schema != RECEIPT_SCHEMA {
            bail!(
                "unsupported install-proof receipt schema `{}`; expected `{RECEIPT_SCHEMA}`",
                receipt.schema
            );
        }
        Ok(receipt)
    }
}

/// CLI entry for `cargo xtask prove-install`: real staged-file resolver and
/// real `--version` probe; tests inject fakes through [`run_with_resolver`].
pub fn run(root: &Path, options: ProveInstallOptions) -> Result<()> {
    let output = resolve_input(root, &options.output);
    let mut resolver = StagedResolver::new(
        resolve_input(root, &options.archive),
        resolve_input(root, &options.checksum),
    );
    let probe = CommandVersionProbe {
        path_env: scanned_path(&options),
    };
    let receipt = run_with_resolver(
        root,
        options,
        &mut resolver,
        &probe,
        (std::env::consts::OS, std::env::consts::ARCH),
    )?;
    println!(
        "{}",
        serde_json::json!({
            "schema": RECEIPT_SCHEMA,
            "output": output.display().to_string(),
            "release_tag": receipt.release_tag(),
            "artifact_archive_sha256": receipt.artifact().archive_sha256,
            "installed_binary_sha256": receipt.install().binary_sha256,
            "source_build_used": receipt.source_build_used(),
        })
    );
    Ok(())
}

fn resolve_input(root: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    }
}

fn scanned_path(options: &ProveInstallOptions) -> String {
    options
        .probe_path
        .clone()
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default())
}

fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

/// Decision entry point shared by the CLI and the recording-transport tests.
/// Probes run first; any forbidden-tool detection aborts before the resolver
/// is consulted. Any contract stop fails closed with no receipt file. On the
/// single success path the receipt is built with `source_build_used = false`
/// and written to the output path. `platform` is the gated host identity —
/// the real CLI passes the actual host; contract tests pass the platform
/// under test so the resolver-contract logic stays cross-platform testable
/// while the receipt records exactly what was gated.
pub fn run_with_resolver(
    root: &Path,
    options: ProveInstallOptions,
    resolver: &mut dyn ReleaseResolver,
    version_probe: &dyn VersionProbe,
    platform: (&str, &str),
) -> Result<InstallProofReceipt> {
    let install_dir = resolve_input(root, &options.install_dir);
    let output = resolve_input(root, &options.output);
    let scanned = scanned_path(&options);
    // Environment/tool-absence probes run before install and are recorded
    // in the receipt; a detection refuses before the resolver is consulted.
    let probes = probe_environment(&scanned, options.cargo_home.as_deref());
    detect_forbidden_tools(&probes)?;
    let request = ReleaseRequest {
        release_tag: options.release_tag.clone(),
        asset_name: options.asset_name.clone(),
        checksum_asset_name: format!("{}.sha256", options.asset_name),
    };
    let installed =
        install_release_binary(resolver, version_probe, &request, platform, &install_dir)
            .map_err(|stop| anyhow::anyhow!("{stop}"))?;
    let receipt = InstallProofReceipt {
        schema: RECEIPT_SCHEMA.to_owned(),
        produced_at: options.now.unwrap_or_else(now_epoch),
        harness_revision: options.harness_revision.clone(),
        platform: PlatformRecord {
            os: installed.platform_os,
            arch: installed.platform_arch,
        },
        release_tag: options.release_tag.clone(),
        expected_version: installed
            .version_expected
            .strip_prefix("ub-review ")
            .unwrap_or(&installed.version_expected)
            .to_owned(),
        asset_name: options.asset_name.clone(),
        checksum_asset_name: request.checksum_asset_name,
        artifact: ArtifactRecord {
            source: installed.archive_source,
            transport: resolver.transport().to_owned(),
            archive_sha256: installed.archive_sha256,
            archive_length: installed.archive_length,
            checksum_sha256: installed.checksum_sha256,
        },
        probes,
        install: InstallRecord {
            install_dir: installed.install_dir.display().to_string(),
            binary_path: installed.binary_path.display().to_string(),
            binary_sha256: installed.binary_sha256,
            version_expected: installed.version_expected,
            version_reported: installed.version_reported,
        },
        // The assertion exists exactly here: after the platform gate, the
        // tool-absence probes, the checksum gate, the layout gate, the
        // extraction checks, and the version-identity gate all passed, with
        // no source-fallback branch in this module to reach instead.
        source_build_used: false,
    };
    let json = receipt.to_json()?;
    if let Some(parent) = output.parent()
        && !parent.as_os_str().is_empty()
    {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&output, format!("{json}\n"))
        .with_context(|| format!("write receipt {}", output.display()))?;
    Ok(receipt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write as IoWrite;

    const FIXTURE_VERSION_TAG: &str = "v0.1.3";
    const FIXTURE_BINARY_CONTENT: &[u8] = b"ELF!";
    const FIXTURE_REVISION: &str = "install-proof-test-rev";

    fn unique_dir(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or_default();
        std::env::temp_dir().join(format!(
            "ub-review-prove-install-{tag}-{}-{nanos}",
            std::process::id()
        ))
    }

    fn seal_header(bytes: &mut [u8]) {
        bytes[148..156].fill(b' ');
        let sum: u64 = bytes[..512].iter().map(|byte| u64::from(*byte)).sum();
        bytes[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    }

    fn tar_entry(name: &[u8], kind: u8, content: &[u8]) -> Vec<u8> {
        let mut header = vec![0u8; 512];
        header[..name.len()].copy_from_slice(name);
        header[100..108].copy_from_slice(b"0000755\0");
        header[124..136].copy_from_slice(format!("{:011o}\0", content.len()).as_bytes());
        header[148..156].fill(b' ');
        header[156] = kind;
        header[257..263].copy_from_slice(b"ustar\0");
        header[263..265].copy_from_slice(b"00");
        seal_header(&mut header);
        let mut bytes = header;
        bytes.extend_from_slice(content);
        let padding = (512 - content.len() % 512) % 512;
        bytes.resize(bytes.len() + padding, 0);
        bytes
    }

    fn tar_archive(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut bytes = entries.concat();
        bytes.resize(bytes.len() + 1024, 0);
        bytes
    }

    fn gzip(tar: &[u8]) -> Result<Vec<u8>> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(tar)?;
        encoder.finish().map_err(Into::into)
    }

    fn fixture_archive(content: &[u8]) -> Result<Vec<u8>> {
        gzip(&tar_archive(&[tar_entry(b"ub-review", b'0', content)]))
    }

    fn checksum_bytes(archive: &[u8]) -> Vec<u8> {
        format!("{}  {}\n", rb::sha256(archive), rb::ASSET).into_bytes()
    }

    struct RecordingResolver {
        archive: Vec<u8>,
        checksum: Vec<u8>,
        unavailable: Option<String>,
        fetches: Vec<String>,
    }

    impl RecordingResolver {
        fn serving(archive: Vec<u8>, checksum: Vec<u8>) -> Self {
            Self {
                archive,
                checksum,
                unavailable: None,
                fetches: Vec::new(),
            }
        }
        fn unavailable(detail: &str) -> Self {
            Self {
                archive: Vec::new(),
                checksum: Vec::new(),
                unavailable: Some(detail.to_owned()),
                fetches: Vec::new(),
            }
        }
    }

    impl ReleaseResolver for RecordingResolver {
        fn transport(&self) -> &'static str {
            "recording"
        }
        fn fetch(
            &mut self,
            request: &ReleaseRequest,
        ) -> std::result::Result<ResolvedArtifact, ResolverStop> {
            self.fetches.push(request.release_tag.clone());
            if let Some(detail) = &self.unavailable {
                return Err(ResolverStop::Unavailable {
                    detail: detail.clone(),
                });
            }
            Ok(ResolvedArtifact {
                source: "recording://fixture".to_owned(),
                archive: self.archive.clone(),
                checksum: self.checksum.clone(),
            })
        }
    }

    struct RecordingProbe {
        output: std::result::Result<String, String>,
        calls: std::cell::RefCell<Vec<PathBuf>>,
    }

    impl RecordingProbe {
        fn reporting(text: &str) -> Self {
            Self {
                output: Ok(text.to_owned()),
                calls: std::cell::RefCell::new(Vec::new()),
            }
        }
        fn failing(detail: &str) -> Self {
            Self {
                output: Err(detail.to_owned()),
                calls: std::cell::RefCell::new(Vec::new()),
            }
        }
    }

    impl VersionProbe for RecordingProbe {
        fn probe(&self, binary: &Path) -> std::io::Result<String> {
            self.calls.borrow_mut().push(binary.to_path_buf());
            match &self.output {
                Ok(text) => Ok(text.clone()),
                Err(detail) => Err(std::io::Error::other(detail.clone())),
            }
        }
    }

    fn fixture_environment(tag: &str) -> Result<(PathBuf, ProveInstallOptions, Vec<u8>, Vec<u8>)> {
        let dir = unique_dir(tag);
        fs::create_dir_all(dir.join("scanned-path"))?;
        let archive = fixture_archive(FIXTURE_BINARY_CONTENT)?;
        let checksum = checksum_bytes(&archive);
        let options = base_options(&dir, FIXTURE_VERSION_TAG, rb::ASSET);
        Ok((dir, options, archive, checksum))
    }

    fn base_options(dir: &Path, release_tag: &str, asset_name: &str) -> ProveInstallOptions {
        ProveInstallOptions {
            archive: dir.join("staged").join(rb::ASSET),
            checksum: dir.join("staged").join(format!("{}.sha256", rb::ASSET)),
            release_tag: release_tag.to_owned(),
            asset_name: asset_name.to_owned(),
            install_dir: dir.join("install"),
            output: dir.join("receipt.json"),
            harness_revision: FIXTURE_REVISION.to_owned(),
            probe_path: Some(dir.join("scanned-path").display().to_string()),
            cargo_home: Some(dir.join("cargo-home")),
            now: Some(100),
        }
    }

    fn request() -> ReleaseRequest {
        ReleaseRequest {
            release_tag: FIXTURE_VERSION_TAG.to_owned(),
            asset_name: rb::ASSET.to_owned(),
            checksum_asset_name: format!("{}.sha256", rb::ASSET),
        }
    }

    fn planted_executable(dir: &Path, relative: &[&str]) -> Result<PathBuf> {
        let mut path = dir.to_path_buf();
        for part in relative {
            path = path.join(part);
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, b"#!/bin/sh\nexit 0\n")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
        }
        Ok(path)
    }

    fn receipt_absent(options: &ProveInstallOptions) {
        assert!(
            !options.output.exists(),
            "a refused run must not write a receipt"
        );
    }

    #[test]
    fn positive_install_produces_a_receipt_asserting_no_source_build() -> Result<()> {
        let (dir, options, archive, checksum) = fixture_environment("positive")?;
        let mut resolver = RecordingResolver::serving(archive.clone(), checksum);
        let probe = RecordingProbe::reporting("ub-review 0.1.3");
        let receipt = run_with_resolver(
            &dir,
            options.clone(),
            &mut resolver,
            &probe,
            ("linux", "x86_64"),
        )?;
        assert_eq!(receipt.schema(), RECEIPT_SCHEMA);
        assert!(!receipt.source_build_used());
        assert_eq!(receipt.release_tag(), FIXTURE_VERSION_TAG);
        assert_eq!(receipt.expected_version(), "0.1.3");
        assert_eq!(receipt.harness_revision(), FIXTURE_REVISION);
        assert_eq!(receipt.produced_at(), 100);
        assert_eq!(
            receipt.checksum_asset_name(),
            format!("{}.sha256", rb::ASSET)
        );
        assert_eq!(receipt.artifact().archive_sha256, rb::sha256(&archive));
        assert_eq!(receipt.artifact().source, "recording://fixture");
        assert_eq!(receipt.artifact().transport, "recording");
        assert_eq!(
            receipt.install().binary_sha256,
            rb::sha256(FIXTURE_BINARY_CONTENT)
        );
        assert_eq!(receipt.install().version_reported, "ub-review 0.1.3");
        assert_eq!(receipt.install().version_expected, "ub-review 0.1.3");
        // Every probe row in both scopes recorded the absence.
        let probes = receipt.probes();
        assert_eq!(probes.scanned_path.len(), 1);
        assert!(
            probes
                .probes
                .iter()
                .all(|row| row.outcome == ProbeOutcome::Absent)
        );
        assert_eq!(probes.probes.len(), 2 * FORBIDDEN_TOOLS.len());
        // The emitted receipt file parses back to the same assertion.
        let bytes = fs::read(&options.output)?;
        let parsed = InstallProofReceipt::from_json(&bytes)?;
        assert!(!parsed.source_build_used());
        assert_eq!(parsed.install().binary_path, receipt.install().binary_path);
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn corrupt_archive_refuses_without_a_receipt() -> Result<()> {
        let (dir, options, archive, checksum) = fixture_environment("corrupt")?;
        let mut corrupt = archive.clone();
        let last = corrupt.len() - 1;
        corrupt[last] ^= 0xff;
        let mut resolver = RecordingResolver::serving(corrupt, checksum);
        let probe = RecordingProbe::reporting("ub-review 0.1.3");
        let error = run_with_resolver(
            &dir,
            options.clone(),
            &mut resolver,
            &probe,
            ("linux", "x86_64"),
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("corrupt archive must refuse"))?;
        assert!(error.to_string().contains("checksum mismatch"), "{error:#}");
        assert_eq!(resolver.fetches.len(), 1);
        receipt_absent(&options);
        assert!(!options.install_dir.join(BINARY_NAME).exists());
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn malformed_checksum_receipt_refuses() -> Result<()> {
        let (dir, options, archive, _) = fixture_environment("checksum-receipt")?;
        let mut resolver = RecordingResolver::serving(archive, b"not-a-digest\n".to_vec());
        let probe = RecordingProbe::reporting("ub-review 0.1.3");
        let error = run_with_resolver(
            &dir,
            options.clone(),
            &mut resolver,
            &probe,
            ("linux", "x86_64"),
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("malformed checksum receipt must refuse"))?;
        assert!(
            error.to_string().contains("checksum receipt is malformed"),
            "{error:#}"
        );
        receipt_absent(&options);
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn layout_refusals_cover_nested_duplicate_and_non_regular_roots() -> Result<()> {
        let cases: Vec<(&str, Vec<u8>, &str)> = vec![
            (
                "nested",
                tar_archive(&[
                    tar_entry(b"ub-review", b'0', FIXTURE_BINARY_CONTENT),
                    tar_entry(b"dir/ub-review", b'0', FIXTURE_BINARY_CONTENT),
                ]),
                "nested 1",
            ),
            (
                "duplicate-root",
                tar_archive(&[
                    tar_entry(b"ub-review", b'0', FIXTURE_BINARY_CONTENT),
                    tar_entry(b"ub-review", b'0', FIXTURE_BINARY_CONTENT),
                ]),
                "root regular 2",
            ),
            (
                "directory-root",
                tar_archive(&[tar_entry(b"ub-review", b'5', b"")]),
                "root non-regular 1",
            ),
            (
                "symlink-root",
                tar_archive(&[tar_entry(b"ub-review", b'2', b"target")]),
                "root non-regular 1",
            ),
        ];
        for (tag, tar, detail) in cases {
            let dir = unique_dir(tag);
            let archive = gzip(&tar)?;
            let checksum = checksum_bytes(&archive);
            let mut resolver = RecordingResolver::serving(archive, checksum);
            let probe = RecordingProbe::reporting("ub-review 0.1.3");
            let error = install_release_binary(
                &mut resolver,
                &probe,
                &request(),
                ("linux", "x86_64"),
                &dir.join("install"),
            )
            .err()
            .ok_or_else(|| anyhow::anyhow!("{tag} layout must refuse"))?;
            match error {
                InstallStop::ArchiveLayout { detail: text } => {
                    assert!(text.contains(detail), "{tag}: {text}");
                }
                other => assert!(
                    matches!(other, InstallStop::ArchiveLayout { .. }),
                    "expected ArchiveLayout for {tag}, got {other:?}"
                ),
            }
            assert_eq!(resolver.fetches.len(), 1, "{tag}: gate runs after fetch");
            assert!(probe.calls.borrow().is_empty(), "{tag}: identity never ran");
            let _ = fs::remove_dir_all(&dir);
        }
        Ok(())
    }

    #[test]
    fn planted_cargo_aborts_before_the_resolver_is_consulted() -> Result<()> {
        let (dir, mut options, _, _) = fixture_environment("planted-cargo")?;
        let planted = planted_executable(&dir, &["scanned-path", "cargo"])?;
        options.probe_path = Some(dir.join("scanned-path").display().to_string());
        let mut resolver = RecordingResolver::serving(Vec::new(), Vec::new());
        let probe = RecordingProbe::reporting("ub-review 0.1.3");
        let error = run_with_resolver(
            &dir,
            options.clone(),
            &mut resolver,
            &probe,
            ("linux", "x86_64"),
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("planted cargo must abort"))?;
        let rendered = error.to_string();
        assert!(rendered.contains("cargo"), "{rendered}");
        assert!(rendered.contains(&planted.display().to_string()));
        assert!(
            resolver.fetches.is_empty(),
            "probes run before the resolver"
        );
        assert!(probe.calls.borrow().is_empty());
        receipt_absent(&options);
        assert!(!options.install_dir.exists());
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn planted_ub_review_in_cargo_home_is_detected() -> Result<()> {
        let (dir, mut options, _, _) = fixture_environment("planted-ub-review")?;
        let planted = planted_executable(&dir, &["cargo-home", "bin", "ub-review"])?;
        options.cargo_home = Some(dir.join("cargo-home"));
        let mut resolver = RecordingResolver::serving(Vec::new(), Vec::new());
        let probe = RecordingProbe::reporting("ub-review 0.1.3");
        let error = run_with_resolver(
            &dir,
            options.clone(),
            &mut resolver,
            &probe,
            ("linux", "x86_64"),
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("cargo-sourced ub-review must abort"))?;
        let rendered = error.to_string();
        assert!(rendered.contains("ub-review"), "{rendered}");
        assert!(rendered.contains("cargo-home"), "{rendered}");
        assert!(rendered.contains(&planted.display().to_string()));
        assert!(resolver.fetches.is_empty());
        receipt_absent(&options);
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn resolver_unavailability_fails_closed_without_source_fallback() -> Result<()> {
        let (dir, options, _, _) = fixture_environment("no-fallback")?;
        let mut resolver = RecordingResolver::unavailable("fixture outage");
        let probe = RecordingProbe::reporting("ub-review 0.1.3");
        let error = run_with_resolver(
            &dir,
            options.clone(),
            &mut resolver,
            &probe,
            ("linux", "x86_64"),
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("unavailable resolver must fail closed"))?;
        let rendered = error.to_string();
        assert!(
            rendered.contains("release binary unavailable"),
            "{rendered}"
        );
        assert_eq!(resolver.fetches.len(), 1);
        receipt_absent(&options);
        assert!(!options.install_dir.join(BINARY_NAME).exists());
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn version_identity_mismatch_refuses() -> Result<()> {
        let (dir, options, archive, checksum) = fixture_environment("identity")?;
        let mut resolver = RecordingResolver::serving(archive, checksum);
        let probe = RecordingProbe::reporting("ub-review 9.9.9");
        let error = run_with_resolver(
            &dir,
            options.clone(),
            &mut resolver,
            &probe,
            ("linux", "x86_64"),
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("version mismatch must refuse"))?;
        assert!(
            error
                .to_string()
                .contains("version mismatch (expected ub-review 0.1.3, got ub-review 9.9.9)"),
            "{error:#}"
        );
        receipt_absent(&options);
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn version_execution_failure_refuses() -> Result<()> {
        let (dir, options, archive, checksum) = fixture_environment("exec-failure")?;
        let mut resolver = RecordingResolver::serving(archive, checksum);
        let probe = RecordingProbe::failing("fixture probe crashed");
        let error = run_with_resolver(
            &dir,
            options.clone(),
            &mut resolver,
            &probe,
            ("linux", "x86_64"),
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("version execution failure must refuse"))?;
        assert!(
            error.to_string().contains("--version execution failed"),
            "{error:#}"
        );
        receipt_absent(&options);
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn unsupported_platform_refuses_before_fetch() -> Result<()> {
        for platform in [("windows", "x86_64"), ("linux", "aarch64")] {
            let mut resolver = RecordingResolver::serving(Vec::new(), Vec::new());
            let probe = RecordingProbe::reporting("ub-review 0.1.3");
            let error = install_release_binary(
                &mut resolver,
                &probe,
                &request(),
                platform,
                &unique_dir("platform"),
            )
            .err()
            .ok_or_else(|| anyhow::anyhow!("platform {platform:?} must refuse"))?;
            match error {
                InstallStop::PlatformUnsupported { os, arch } => {
                    assert_eq!(os, platform.0);
                    assert_eq!(arch, platform.1);
                }
                other => assert!(
                    matches!(other, InstallStop::PlatformUnsupported { .. }),
                    "expected PlatformUnsupported, got {other:?}"
                ),
            }
            assert!(resolver.fetches.is_empty());
        }
        Ok(())
    }

    #[test]
    fn invalid_release_requests_refuse_before_fetch() -> Result<()> {
        let cases = vec![
            ("", rb::ASSET),
            ("v0.1.3 extra", rb::ASSET),
            ("v0.1.3;rm", rb::ASSET),
            ("v0.1.3", "sub/dir/asset.tar.gz"),
            ("v0.1.3", ".."),
            ("v0.1.3", "asset name.tar.gz"),
        ];
        for (tag, asset) in cases {
            let mut resolver = RecordingResolver::serving(Vec::new(), Vec::new());
            let probe = RecordingProbe::reporting("ub-review 0.1.3");
            let bad_request = ReleaseRequest {
                release_tag: tag.to_owned(),
                asset_name: asset.to_owned(),
                checksum_asset_name: format!("{asset}.sha256"),
            };
            let error = install_release_binary(
                &mut resolver,
                &probe,
                &bad_request,
                ("linux", "x86_64"),
                &unique_dir("invalid-request"),
            )
            .err()
            .ok_or_else(|| anyhow::anyhow!("request ({tag}, {asset}) must refuse"))?;
            assert!(
                matches!(
                    error,
                    InstallStop::InvalidRequest { .. } | InstallStop::Unavailable { .. }
                ),
                "unexpected stop for ({tag}, {asset}): {error:?}"
            );
            assert!(resolver.fetches.is_empty());
        }
        Ok(())
    }

    #[test]
    fn no_cli_flag_or_api_can_stamp_the_assertion() -> Result<()> {
        let (dir, options, archive, checksum) = fixture_environment("authorable")?;
        let valid = vec![
            "--archive".to_owned(),
            options.archive.display().to_string(),
            "--checksum".to_owned(),
            options.checksum.display().to_string(),
            "--release-tag".to_owned(),
            FIXTURE_VERSION_TAG.to_owned(),
            "--asset".to_owned(),
            rb::ASSET.to_owned(),
            "--install-dir".to_owned(),
            options.install_dir.display().to_string(),
            "--output".to_owned(),
            options.output.display().to_string(),
            "--harness-revision".to_owned(),
            FIXTURE_REVISION.to_owned(),
        ];
        ProveInstallOptions::parse(valid.iter().cloned())
            .map_err(|error| anyhow::anyhow!("valid args must parse: {error}"))?;
        for stamped in ["--source-build-used", "--force-pass", "--receipt-assertion"] {
            let mut stamped_args = valid.clone();
            stamped_args.push(stamped.to_owned());
            stamped_args.push("true".to_owned());
            let error = ProveInstallOptions::parse(stamped_args.iter().cloned())
                .err()
                .ok_or_else(|| anyhow::anyhow!("{stamped} must be rejected"))?;
            assert!(
                error.to_string().contains(stamped),
                "rejection must name the flag: {error:#}"
            );
        }
        // The emitted receipt round-trips, rejects foreign fields, and is
        // bound to the fixed schema — no alternative receipt shape exists.
        let mut resolver = RecordingResolver::serving(archive, checksum);
        let probe = RecordingProbe::reporting("ub-review 0.1.3");
        let receipt = run_with_resolver(
            &dir,
            options.clone(),
            &mut resolver,
            &probe,
            ("linux", "x86_64"),
        )?;
        let json = receipt.to_json()?;
        let parsed = InstallProofReceipt::from_json(json.as_bytes())?;
        assert!(!parsed.source_build_used());
        let mut value: serde_json::Value = serde_json::from_str(&json)?;
        value["forged_assertion"] = serde_json::Value::Bool(true);
        let forged = serde_json::to_vec(&value)?;
        assert!(
            InstallProofReceipt::from_json(&forged).is_err(),
            "unknown receipt fields must be rejected"
        );
        value["forged_assertion"] = serde_json::Value::Null;
        value["schema"] = serde_json::Value::String("ub-review.install_proof.v0".to_owned());
        let foreign_schema = serde_json::to_vec(&value)?;
        assert!(
            InstallProofReceipt::from_json(&foreign_schema).is_err(),
            "foreign receipt schemas must be rejected"
        );
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn real_staged_run_executes_the_installed_binary_and_binds_its_identity() -> Result<()> {
        let dir = unique_dir("real-unix");
        fs::create_dir_all(dir.join("scanned-path"))?;
        let script = b"#!/bin/sh\necho \"ub-review 0.1.3\"\n";
        let archive = fixture_archive(script)?;
        let checksum = checksum_bytes(&archive);
        let staged = dir.join("staged");
        fs::create_dir_all(&staged)?;
        fs::write(staged.join(rb::ASSET), &archive)?;
        fs::write(staged.join(format!("{}.sha256", rb::ASSET)), &checksum)?;
        let options = base_options(&dir, FIXTURE_VERSION_TAG, rb::ASSET);
        // Full real CLI path: staged-file resolver, real --version probe,
        // real host platform (this test only runs on Linux).
        let output_path = options.output.clone();
        let install_path = options.install_dir.clone();
        let archive_path = options.archive.clone();
        let receipt = run(&dir, options)?;
        assert!(!receipt.source_build_used());
        assert_eq!(
            receipt.install().binary_sha256,
            rb::sha256(script),
            "the receipt binds the exact installed bytes"
        );
        assert_eq!(receipt.artifact().archive_sha256, rb::sha256(&archive));
        assert_eq!(receipt.artifact().transport, "staged-files");
        assert_eq!(
            receipt.artifact().source,
            archive_path.display().to_string()
        );
        assert_eq!(receipt.platform().os, "linux");
        let mode = fs::metadata(install_path.join(BINARY_NAME))?
            .permissions()
            .mode();
        assert_eq!(mode & 0o755, 0o755, "installed binary is executable");
        let bytes = fs::read(&output_path)?;
        assert!(!InstallProofReceipt::from_json(&bytes)?.source_build_used());
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn real_host_run_without_linux_support_refuses_before_fetch() -> Result<()> {
        let dir = unique_dir("real-host");
        fs::create_dir_all(dir.join("scanned-path"))?;
        let archive = fixture_archive(FIXTURE_BINARY_CONTENT)?;
        let checksum = checksum_bytes(&archive);
        let staged = dir.join("staged");
        fs::create_dir_all(&staged)?;
        fs::write(staged.join(rb::ASSET), &archive)?;
        fs::write(staged.join(format!("{}.sha256", rb::ASSET)), &checksum)?;
        let options = base_options(&dir, FIXTURE_VERSION_TAG, rb::ASSET);
        // Real CLI entry on the real host platform: the platform gate must
        // refuse before the resolver is consulted and write no receipt.
        let install_dir = options.install_dir.clone();
        let error = match run(&dir, options) {
            Ok(_) => anyhow::anyhow!("non-Linux host run must refuse"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("only Linux x64"), "{error:#}");
        assert!(!install_dir.join(BINARY_NAME).exists());
        assert!(!dir.join("receipt.json").exists());
        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }
}
