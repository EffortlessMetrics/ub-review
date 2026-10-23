use super::*;
use anyhow::{Result, ensure};
use tempfile::tempdir;

fn limits(file_bytes: usize, input_bytes: usize, input_files: usize) -> Limits {
    Limits {
        file_bytes,
        input_bytes,
        input_files,
        rows: 2,
        issues: 2,
    }
}

#[test]
fn production_limits_are_pinned() -> Result<()> {
    ensure!(MAX_FILE_BYTES == 4_194_304);
    ensure!(MAX_INPUT_BYTES == 33_554_432);
    ensure!(MAX_INPUT_FILES == 256);
    ensure!(MAX_ROWS == 16_384);
    ensure!(MAX_ISSUES == 256);
    Ok(())
}

#[test]
fn root_must_exist_and_be_a_directory() -> Result<()> {
    let root = tempdir()?;
    ensure!(matches!(
        PacketReader::new(&root.path().join("absent")),
        Err(PacketError::InvalidRoot)
    ));
    let file = root.path().join("file");
    fs::write(&file, b"x")?;
    ensure!(matches!(
        PacketReader::new(&file),
        Err(PacketError::InvalidRoot)
    ));
    Ok(())
}

#[test]
fn successful_reads_cache_raw_bytes_and_source_digest() -> Result<()> {
    let root = tempdir()?;
    fs::write(root.path().join("source.json"), b"abc")?;
    let mut reader = PacketReader::new(root.path())?;
    ensure!(reader.read("source.json", false)? == Some(b"abc".as_slice()));
    fs::write(root.path().join("source.json"), b"changed")?;
    ensure!(reader.read("source.json", true)? == Some(b"abc".as_slice()));
    ensure!(reader.attempts() == 1 && reader.total_bytes() == 3);
    ensure!(
        reader.sources().get("source.json")
            == Some(&Source {
                path: "source.json".to_owned(),
                bytes: 3,
                sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                    .to_owned(),
            })
    );
    ensure!(reader.coverage().get("source.json").map(String::as_str) == Some("read"));
    ensure!(!reader.input_unavailable() && reader.issues().is_empty());
    Ok(())
}

#[test]
fn missing_optional_input_remains_cached_on_required_revisit() -> Result<()> {
    let root = tempdir()?;
    let mut reader = PacketReader::new(root.path())?;
    ensure!(reader.read("missing.json", false)?.is_none());
    fs::write(root.path().join("missing.json"), b"{}")?;
    ensure!(reader.load_json("missing.json", true)?.is_none());
    ensure!(reader.read("missing.json", true)?.is_none());
    ensure!(reader.attempts() == 1 && reader.total_bytes() == 0);
    ensure!(reader.sources().is_empty() && reader.issues().len() == 1);
    ensure!(
        reader
            .issues()
            .iter()
            .next()
            .is_some_and(|issue| issue.code == "missing_projection")
    );
    ensure!(reader.coverage().get("missing.json").map(String::as_str) == Some("unavailable"));
    ensure!(!reader.input_unavailable());
    Ok(())
}

#[test]
fn unique_attempt_limit_counts_missing_and_invalid_names() -> Result<()> {
    let root = tempdir()?;
    let mut reader = PacketReader::with_limits(root.path(), limits(4, 8, 2))?;
    ensure!(reader.read("missing", false)?.is_none());
    ensure!(reader.read("./bad", false) == Err(PacketError::UnsafePath));
    ensure!(reader.attempts() == 2);
    ensure!(reader.read("third", false) == Err(PacketError::FileBudgetExceeded));
    ensure!(reader.read("./bad", true) == Err(PacketError::UnsafePath));
    ensure!(reader.attempts() == 2 && reader.total_bytes() == 0);
    ensure!(reader.coverage().len() == 2);
    ensure!(reader.input_unavailable());
    Ok(())
}

#[test]
fn file_cap_accepts_exact_limit_and_empty_files() -> Result<()> {
    let root = tempdir()?;
    fs::write(root.path().join("exact"), b"1234")?;
    fs::write(root.path().join("empty"), b"")?;
    let mut reader = PacketReader::with_limits(root.path(), limits(4, 8, 3))?;
    ensure!(reader.read("exact", false)? == Some(b"1234".as_slice()));
    ensure!(reader.read("empty", false)? == Some(b"".as_slice()));
    ensure!(reader.total_bytes() == 4 && reader.sources().len() == 2);
    Ok(())
}

#[test]
fn file_overflow_charges_exactly_one_probe_byte_and_caches_error() -> Result<()> {
    let root = tempdir()?;
    fs::write(root.path().join("over"), b"123456789")?;
    let mut reader = PacketReader::with_limits(root.path(), limits(4, 10, 2))?;
    ensure!(reader.read("over", false) == Err(PacketError::ByteBudgetExceeded));
    ensure!(reader.total_bytes() == 5 && reader.sources().is_empty());
    fs::write(root.path().join("over"), b"x")?;
    // Deliberately tighter than Python's poisoned None cache: a failed input
    // remains an error, and never becomes a missing or newly readable input.
    ensure!(reader.load_json("over", true) == Err(PacketError::ByteBudgetExceeded));
    ensure!(reader.total_bytes() == 5 && reader.attempts() == 1);
    ensure!(reader.coverage().get("over").map(String::as_str) == Some("invalid"));
    ensure!(
        reader
            .issues()
            .iter()
            .all(|issue| issue.code != "missing_projection")
    );
    Ok(())
}

#[test]
fn aggregate_cap_accepts_exact_total_then_refuses_even_empty_input() -> Result<()> {
    let root = tempdir()?;
    fs::write(root.path().join("first"), b"1234")?;
    fs::write(root.path().join("second"), b"56")?;
    fs::write(root.path().join("empty"), b"")?;
    let mut reader = PacketReader::with_limits(root.path(), limits(4, 6, 4))?;
    ensure!(reader.read("first", false)?.is_some());
    ensure!(reader.read("second", false)?.is_some());
    ensure!(reader.total_bytes() == 6);
    ensure!(reader.read("empty", false) == Err(PacketError::ByteBudgetExceeded));
    ensure!(reader.read("first", false)? == Some(b"1234".as_slice()));
    ensure!(reader.total_bytes() == 6 && reader.sources().len() == 2);
    Ok(())
}

#[test]
fn aggregate_overflow_reads_remaining_plus_one_and_charges_probe() -> Result<()> {
    let root = tempdir()?;
    fs::write(root.path().join("first"), b"1234")?;
    fs::write(root.path().join("second"), b"56789")?;
    fs::write(root.path().join("third"), b"0")?;
    let mut reader = PacketReader::with_limits(root.path(), limits(4, 6, 4))?;
    ensure!(reader.read("first", false)?.is_some());
    ensure!(reader.read("second", false) == Err(PacketError::ByteBudgetExceeded));
    ensure!(reader.total_bytes() == 7);
    ensure!(reader.read("second", false) == Err(PacketError::ByteBudgetExceeded));
    ensure!(reader.read("third", false) == Err(PacketError::ByteBudgetExceeded));
    ensure!(reader.total_bytes() == 7 && reader.sources().len() == 1);
    Ok(())
}

#[test]
fn paths_require_canonical_posix_relative_spelling() -> Result<()> {
    let root = tempdir()?;
    let reader = PacketReader::new(root.path())?;
    for path in [
        "",
        ".",
        "..",
        "/absolute",
        "//absolute",
        "./a",
        "a/.",
        "a/../b",
        "a//b",
        "a/",
        "a\\b",
        "a\0b",
    ] {
        ensure!(
            reader.confined_path(path) == Err(PacketError::UnsafePath),
            "path should be unsafe"
        );
    }
    ensure!(reader.confined_path("review/é.json")? == root.path().join("review/é.json"));
    ensure!(reader.attempts() == 0);
    Ok(())
}

#[test]
fn input_must_be_a_regular_file_and_missing_child_is_unavailable() -> Result<()> {
    let root = tempdir()?;
    fs::create_dir(root.path().join("directory"))?;
    fs::write(root.path().join("file"), b"x")?;
    let mut reader = PacketReader::new(root.path())?;
    ensure!(reader.read("directory", false) == Err(PacketError::NotRegularFile));
    ensure!(reader.read("file/child", false)?.is_none());
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn path_metadata_io_failure_is_distinct_and_cached() -> Result<()> {
    let root = tempdir()?;
    // Linux rejects this oversized component independently of permissions.
    let name = "x".repeat(64 * 1024);
    let mut reader = PacketReader::new(root.path())?;
    ensure!(reader.confined_path(&name) == Err(PacketError::InputIo));
    ensure!(reader.attempts() == 0);
    ensure!(reader.read(&name, false) == Err(PacketError::InputIo));
    // A cached failure remains available even when no new attempt is allowed.
    reader.limits.input_files = 0;
    ensure!(reader.load_json(&name, true) == Err(PacketError::InputIo));
    ensure!(reader.attempts() == 1 && reader.total_bytes() == 0);
    ensure!(reader.sources().is_empty() && reader.input_unavailable());
    ensure!(reader.coverage().get(&name).map(String::as_str) == Some("invalid"));
    ensure!(reader.issues().len() == 1);
    ensure!(
        reader
            .issues()
            .iter()
            .all(|issue| issue.code == "invalid_projection")
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn every_symlink_component_is_rejected_including_broken_links() -> Result<()> {
    use std::os::unix::fs::symlink;
    let root = tempdir()?;
    let outside = tempdir()?;
    fs::write(outside.path().join("input"), b"{}")?;
    symlink(outside.path(), root.path().join("dir-link"))?;
    symlink(outside.path().join("input"), root.path().join("file-link"))?;
    symlink(
        outside.path().join("missing"),
        root.path().join("broken-link"),
    )?;
    let mut reader = PacketReader::new(root.path())?;
    for path in [
        "dir-link/input",
        "dir-link/missing",
        "file-link",
        "broken-link",
    ] {
        ensure!(reader.read(path, false) == Err(PacketError::SymlinkInput));
    }
    ensure!(reader.total_bytes() == 0 && reader.sources().is_empty());
    Ok(())
}

#[cfg(unix)]
#[test]
fn root_is_resolved_once_and_root_symlink_is_allowed() -> Result<()> {
    use std::os::unix::fs::symlink;
    let parent = tempdir()?;
    let original = tempdir()?;
    let replacement = tempdir()?;
    fs::write(original.path().join("input"), b"original")?;
    fs::write(replacement.path().join("input"), b"replacement")?;
    let link = parent.path().join("root");
    symlink(original.path(), &link)?;
    let mut reader = PacketReader::new(&link)?;
    fs::remove_file(&link)?;
    symlink(replacement.path(), &link)?;
    ensure!(reader.read("input", false)? == Some(b"original".as_slice()));
    Ok(())
}

#[test]
fn strict_json_rejects_recursive_and_escape_decoded_duplicate_keys() -> Result<()> {
    for input in [
        r#"{"a":1,"a":2}"#,
        r#"{"outer":{"a":1,"a":2}}"#,
        r#"{"outer":[{"a":1,"a":2}]}"#,
        r#"{"a":1,"\u0061":2}"#,
        r#"{"outer":[{"$serde_json::private::Number":"1","$serde_json::private::Number":"2"}]}"#,
    ] {
        ensure!(strict_json(input.as_bytes()) == Err(PacketError::InvalidJson));
    }
    ensure!(strict_json(br#"{"a":{"x":1},"b":{"x":2}}"#)?.is_object());
    Ok(())
}

#[test]
fn strict_json_rejects_nonfinite_numbers_encoding_bom_and_trailing_data() -> Result<()> {
    for input in [
        "NaN",
        "Infinity",
        "-Infinity",
        "1e999",
        "-1e999",
        "{\"nested\":[1e999]}",
        "{} []",
        "\u{feff}{}",
    ] {
        ensure!(strict_json(input.as_bytes()) == Err(PacketError::InvalidJson));
    }
    ensure!(strict_json(&[0xff, 0xfe, b'{', 0, b'}', 0]) == Err(PacketError::InvalidJson));
    ensure!(strict_json(b"\"\xff\"") == Err(PacketError::InvalidJson));
    let deeply_nested = format!("{}0{}", "[".repeat(130), "]".repeat(130));
    ensure!(strict_json(deeply_nested.as_bytes()) == Err(PacketError::InvalidJson));
    ensure!(strict_json(b"1e308")?.is_number());
    Ok(())
}

#[test]
fn real_private_number_keys_remain_objects() -> Result<()> {
    let value = strict_json(br#"{"$serde_json::private::Number":"42","extra":true}"#)?;
    ensure!(value.is_object());
    ensure!(
        value
            .get("$serde_json::private::Number")
            .and_then(Value::as_str)
            == Some("42")
    );
    ensure!(value.get("extra").and_then(Value::as_bool) == Some(true));
    let value =
        strict_json(br#"[{"$serde_json::private::Number":"not a number"},18446744073709551616]"#)?;
    ensure!(
        value
            .as_array()
            .and_then(|array| array.first())
            .is_some_and(Value::is_object)
    );
    ensure!(
        value
            .as_array()
            .and_then(|array| array.get(1))
            .and_then(integer_u128)
            == Some(18_446_744_073_709_551_616)
    );
    Ok(())
}

#[test]
fn integer_domains_reject_bool_fraction_negative_and_overflow() -> Result<()> {
    for text in [
        "true", "false", "null", "\"1\"", "1.0", "1e0", "0.0", "-0.0", "-1", "[]", "{}",
    ] {
        let value = strict_json(text.as_bytes())?;
        ensure!(
            integer_u32(&value).is_none()
                && integer_u64(&value).is_none()
                && integer_u128(&value).is_none()
        );
    }
    for text in ["0", "-0"] {
        let value = strict_json(text.as_bytes())?;
        ensure!(
            integer_u32(&value) == Some(0)
                && integer_u64(&value) == Some(0)
                && integer_u128(&value) == Some(0)
        );
    }
    let max32 = strict_json(b"4294967295")?;
    let over32 = strict_json(b"4294967296")?;
    ensure!(integer_u32(&max32) == Some(u32::MAX) && integer_u32(&over32).is_none());
    ensure!(integer_u64(&over32) == Some(4_294_967_296));
    let max64 = strict_json(b"18446744073709551615")?;
    let over64 = strict_json(b"18446744073709551616")?;
    ensure!(integer_u64(&max64) == Some(u64::MAX) && integer_u64(&over64).is_none());
    ensure!(integer_u128(&over64) == Some(18_446_744_073_709_551_616));
    let max128 = strict_json(b"340282366920938463463374607431768211455")?;
    let over128 = strict_json(b"340282366920938463463374607431768211456")?;
    ensure!(integer_u128(&max128) == Some(u128::MAX) && integer_u128(&over128).is_none());
    Ok(())
}

#[test]
fn canonical_json_sorts_nested_utf8_keys_and_retains_exact_u128() -> Result<()> {
    let value = strict_json(
        "{\"é\":\"雪\",\"z\":{\"β\":2,\"a\":1},\"a\":[340282366920938463463374607431768211455]}"
            .as_bytes(),
    )?;
    ensure!(canonical_json(&value)? == "{\"a\":[340282366920938463463374607431768211455],\"z\":{\"a\":1,\"β\":2},\"é\":\"雪\"}\n".as_bytes());
    let value = strict_json(br#"{"s":"\n\t\u0000","slash":"/"}"#)?;
    ensure!(canonical_json(&value)? == b"{\"s\":\"\\n\\t\\u0000\",\"slash\":\"/\"}\n");
    ensure!(canonical_json(&strict_json(b"-0")?)? == b"0\n");
    Ok(())
}

#[test]
fn json_load_preserves_raw_inventory_on_invalid_json_shape_or_schema() -> Result<()> {
    let root = tempdir()?;
    fs::write(root.path().join("duplicate.json"), b"{\"a\":1,\"a\":2}")?;
    fs::write(root.path().join("array.json"), b"[]")?;
    fs::write(root.path().join("schema.json"), b"{\"schema\":\"actual\"}")?;
    let mut reader = PacketReader::new(root.path())?;
    ensure!(reader.load_json("duplicate.json", true) == Err(PacketError::InvalidJson));
    ensure!(reader.read("duplicate.json", false)?.is_some());
    ensure!(
        reader.load("array.json", JsonShape::Object, None, false)
            == Err(PacketError::WrongJsonShape)
    );
    ensure!(
        reader
            .load("array.json", JsonShape::Array, None, false)?
            .is_some()
    );
    ensure!(
        reader.load("schema.json", JsonShape::Object, Some("expected"), false)
            == Err(PacketError::UnsupportedSchema)
    );
    ensure!(reader.sources().len() == 3 && reader.input_unavailable());
    ensure!(
        reader
            .coverage()
            .values()
            .all(|coverage| coverage == "invalid")
    );
    let total = reader.total_bytes();
    ensure!(reader.load_json("duplicate.json", true) == Err(PacketError::InvalidJson));
    ensure!(reader.total_bytes() == total && reader.attempts() == 3);
    Ok(())
}

#[test]
fn bounded_row_and_string_helpers_fail_closed() -> Result<()> {
    let root = tempdir()?;
    let mut reader = PacketReader::with_limits(root.path(), limits(4, 8, 4))?;
    ensure!(reader.rows(None, "none", None).is_empty() && reader.issues().is_empty());
    let valid = strict_json(br#"{"rows":[{"id":"a"},{"id":"b"}],"strings":["a","b"]}"#)?;
    ensure!(reader.rows(Some(&valid), "valid", Some("rows")).len() == 2);
    ensure!(reader.strings(&valid, "valid", "strings") == ["a", "b"]);
    for input in [
        "{}",
        "[]",
        "{\"rows\":false}",
        "{\"rows\":[{},0]}",
        "{\"rows\":[{},{},{}]}",
    ] {
        let value = strict_json(input.as_bytes())?;
        ensure!(reader.rows(Some(&value), "bad", Some("rows")).is_empty());
    }
    for input in [
        "[]",
        "{}",
        "{\"strings\":[\"a\",false]}",
        "{\"strings\":[\"a\",\"b\",\"c\"]}",
    ] {
        let value = strict_json(input.as_bytes())?;
        ensure!(reader.strings(&value, "bad", "strings").is_empty());
    }
    let array = strict_json(b"[{},{}]")?;
    ensure!(reader.rows(Some(&array), "array", None).len() == 2);
    ensure!(reader.input_unavailable() && reader.issues().len() == 2 && !reader.issues_overflow());
    Ok(())
}

#[test]
fn mapping_missing_or_null_is_empty_but_nonobject_is_invalid() -> Result<()> {
    let root = tempdir()?;
    let mut reader = PacketReader::new(root.path())?;
    let valid = strict_json(br#"{"null":null,"mapping":{"a":1}}"#)?;
    ensure!(reader.mapping(&valid, "valid", "missing", "").is_none());
    ensure!(reader.mapping(&valid, "valid", "null", "").is_none());
    ensure!(
        reader
            .mapping(&valid, "valid", "mapping", "")
            .is_some_and(|mapping| mapping.len() == 1)
    );
    ensure!(!reader.input_unavailable() && reader.issues().is_empty());
    let invalid = strict_json(br#"{"mapping":false}"#)?;
    ensure!(reader.mapping(&invalid, "bad", "mapping", "id").is_none());
    ensure!(reader.mapping(&Value::Null, "bad", "mapping", "").is_none());
    ensure!(reader.input_unavailable() && reader.issues().len() == 2);
    Ok(())
}

#[test]
fn lease_quantities_obey_u32_and_u64_domains_without_bool_coercion() -> Result<()> {
    let root = tempdir()?;
    let mut reader = PacketReader::new(root.path())?;
    let valid = strict_json(
        br#"{"cpu":4294967295,"memory_mb":18446744073709551615,"disk_mb":0,"timeout_sec":1}"#,
    )?;
    let object = valid
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("fixture must be object"))?;
    ensure!(
        reader.lease_quantities(object, "valid", "id")
            == Some(LeaseQuantities {
                cpu: u32::MAX,
                memory_mb: u64::MAX,
                disk_mb: 0,
                timeout_sec: 1
            })
    );
    let invalid =
        strict_json(br#"{"cpu":4294967296,"memory_mb":true,"disk_mb":-1,"timeout_sec":1.0}"#)?;
    let object = invalid
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("fixture must be object"))?;
    ensure!(reader.lease_quantities(object, "bad", "id").is_none());
    ensure!(reader.issues().len() == 4 && reader.input_unavailable());
    ensure!(
        reader.issues().iter().all(
            |issue| issue.code == "invalid_lease_quantity" && issue.identity.starts_with("id:")
        )
    );
    Ok(())
}

#[test]
fn issues_are_deduplicated_bounded_and_sanitize_control_text() -> Result<()> {
    let root = tempdir()?;
    let mut reader = PacketReader::with_limits(root.path(), limits(4, 8, 4))?;
    let invalid = Value::Bool(false);
    ensure!(
        reader
            .mapping(&invalid, "first\n", "mapping", "id\0")
            .is_none()
    );
    ensure!(
        reader
            .mapping(&invalid, "first\n", "mapping", "id\0")
            .is_none()
    );
    ensure!(
        reader
            .mapping(&invalid, &"é".repeat(121), "mapping", "")
            .is_none()
    );
    ensure!(reader.issues().len() == 2 && !reader.issues_overflow());
    ensure!(reader.mapping(&invalid, "third", "mapping", "").is_none());
    ensure!(reader.issues().len() == 2 && reader.issues_overflow());
    ensure!(
        reader
            .issues()
            .iter()
            .all(|issue| issue.path.chars().count() <= 120
                && !issue.path.contains('\n')
                && !issue.identity.contains('\0'))
    );
    ensure!(label(&"é".repeat(121)).chars().count() == 113);
    Ok(())
}

#[test]
fn labels_use_bounded_debug_printable_unicode_rendering() -> Result<()> {
    ensure!(label("a\u{200b}b\u{2028}c\u{e000}d\u{0378}e") == "a?b?c?d?e");
    ensure!(label("😀雪") == "😀雪");
    // Intentional reader-only rendering delta: Python calls combining acute
    // printable; the native diagnostic renders it safely as a question mark.
    ensure!(label("e\u{301}") == "e?");
    let at_limit = "😀雪".repeat(60);
    ensure!(at_limit.chars().count() == 120 && label(&at_limit) == at_limit);
    let over_limit = format!("{at_limit}é");
    let rendered = label(&over_limit);
    ensure!(over_limit.chars().count() == 121 && rendered.chars().count() == 113);
    ensure!(rendered.starts_with(&"😀雪".repeat(48)));
    let digest = format!("{:x}", Sha256::digest(over_limit.as_bytes()));
    ensure!(rendered.ends_with(&digest[..16]));
    Ok(())
}

fn native(input: &[u8]) -> PacketResult<Value> {
    NativeDecoder {
        data: input,
        cursor: 0,
        nodes: 0,
    }
    .parse()
}

#[test]
fn native_decoder_independently_enforces_json_lexical_grammar() -> Result<()> {
    for input in [
        "",
        "01",
        "-01",
        "+1",
        "1.",
        "-.1",
        "1e",
        "1e+",
        "1e-",
        "--1",
        "1_0",
        "0x1",
        "[0,]",
        "{\"a\":0,}",
        "{\"a\" 0}",
        "{a:0}",
        "truex",
        "nul",
        "\"\\q\"",
        "\"\\uD800\"",
        "\"\\uDC00\"",
        "1e999",
        "-1e999",
    ] {
        ensure!(
            native(input.as_bytes()) == Err(PacketError::InvalidJson),
            "invalid native grammar: {input}"
        );
        ensure!(strict_json(input.as_bytes()) == Err(PacketError::InvalidJson));
    }
    for input in [
        "null",
        "true",
        "false",
        "0",
        "-0",
        "1.0",
        "1e0",
        "-0.0",
        "1e+10",
        "1E-10",
        "[]",
        "{}",
        "[0,1]",
        "{\"a\":0}",
    ] {
        ensure!(native(input.as_bytes())? == strict_json(input.as_bytes())?);
    }
    ensure!(native(b"\"\xff\"") == Err(PacketError::InvalidJson));
    Ok(())
}

#[test]
fn native_decoder_handles_odd_even_backslashes_and_surrogate_keys() -> Result<()> {
    for (input, expected) in [
        (br#""a\"b""#.as_slice(), "a\"b"),
        (br#""a\\b""#.as_slice(), "a\\b"),
        (br#""a\\\"b""#.as_slice(), "a\\\"b"),
        (br#""tail\\""#.as_slice(), "tail\\"),
        (br#""tail\\\"quote""#.as_slice(), "tail\\\"quote"),
    ] {
        ensure!(native(input)?.as_str() == Some(expected));
        ensure!(native(input)? == strict_json(input)?);
    }
    let duplicate = r#"{"\uD83D\uDE00":1,"😀":2}"#;
    ensure!(native(duplicate.as_bytes()) == Err(PacketError::InvalidJson));
    ensure!(strict_json(duplicate.as_bytes()) == Err(PacketError::InvalidJson));
    let distinct = r#"{"\uD83D\uDE01":1,"😀":2}"#;
    ensure!(
        native(distinct.as_bytes())?
            .as_object()
            .is_some_and(|object| object.len() == 2)
    );
    let private = native(br#"{"$serde_json::private::RawValue":{"$serde_json::private::Number":"42"},"$serde_json::private::Number":[1]}"#)?;
    ensure!(private.as_object().is_some_and(|object| object.len() == 2));
    ensure!(
        private
            .get("$serde_json::private::RawValue")
            .is_some_and(Value::is_object)
    );
    Ok(())
}

#[test]
fn native_decoder_bounds_source_depth_containers_and_total_nodes() -> Result<()> {
    let at_depth = format!("{}0{}", "[".repeat(127), "]".repeat(127));
    ensure!(native(at_depth.as_bytes()).is_ok());
    let over_depth = format!("{}0{}", "[".repeat(128), "]".repeat(128));
    ensure!(native(over_depth.as_bytes()) == Err(PacketError::InvalidJson));
    let at_container = format!("[{}0]", "0,".repeat(MAX_ROWS - 1));
    ensure!(
        native(at_container.as_bytes())?
            .as_array()
            .is_some_and(|array| array.len() == MAX_ROWS)
    );
    ensure!(
        strict_json(at_container.as_bytes())?
            .as_array()
            .is_some_and(|array| array.len() == MAX_ROWS)
    );
    let over_container = format!("[{}0]", "0,".repeat(MAX_ROWS));
    ensure!(native(over_container.as_bytes()) == Err(PacketError::InvalidJson));
    ensure!(strict_json(over_container.as_bytes()) == Err(PacketError::InvalidJson));
    let at_object = format!(
        "{{{}}}",
        (0..MAX_ROWS)
            .map(|index| format!("\"k{index}\":0"))
            .collect::<Vec<_>>()
            .join(",")
    );
    ensure!(
        native(at_object.as_bytes())?
            .as_object()
            .is_some_and(|object| object.len() == MAX_ROWS)
    );
    ensure!(
        strict_json(at_object.as_bytes())?
            .as_object()
            .is_some_and(|object| object.len() == MAX_ROWS)
    );
    let over_object = format!(
        "{{{}}}",
        (0..=MAX_ROWS)
            .map(|index| format!("\"k{index}\":0"))
            .collect::<Vec<_>>()
            .join(",")
    );
    ensure!(native(over_object.as_bytes()) == Err(PacketError::InvalidJson));
    ensure!(strict_json(over_object.as_bytes()) == Err(PacketError::InvalidJson));
    let over_nodes = format!("[{}]", vec![at_container; 17].join(","));
    ensure!(native(over_nodes.as_bytes()) == Err(PacketError::InvalidJson));
    ensure!(strict_json(over_nodes.as_bytes()) == Err(PacketError::InvalidJson));
    ensure!(MAX_JSON_NODES == 262_144);
    let over_bytes = vec![b' '; MAX_FILE_BYTES + 1];
    ensure!(native(&over_bytes) == Err(PacketError::InvalidJson));
    ensure!(strict_json(&over_bytes) == Err(PacketError::InvalidJson));
    // Integers outside the finite-f64 lexical envelope are an explicit tighter
    // reader-only boundary. Every u128 value and its immediate overflow fit.
    let huge_integer = "9".repeat(310);
    ensure!(native(huge_integer.as_bytes()) == Err(PacketError::InvalidJson));
    ensure!(strict_json(huge_integer.as_bytes()) == Err(PacketError::InvalidJson));
    Ok(())
}

#[test]
fn exact_production_file_aggregate_and_attempt_limits() -> Result<()> {
    let root = tempdir()?;
    let buffer = vec![b'x'; MAX_FILE_BYTES + 1];
    fs::write(root.path().join("exact"), &buffer[..MAX_FILE_BYTES])?;
    fs::write(root.path().join("over"), &buffer)?;
    fs::write(root.path().join("empty"), b"")?;
    {
        let mut reader = PacketReader::new(root.path())?;
        ensure!(
            reader
                .read("exact", false)?
                .is_some_and(|bytes| bytes.len() == MAX_FILE_BYTES)
        );
        ensure!(
            reader
                .read("exact", true)?
                .is_some_and(|bytes| bytes.len() == MAX_FILE_BYTES)
        );
        ensure!(reader.total_bytes() == MAX_FILE_BYTES && reader.attempts() == 1);
        ensure!(reader.read("over", false) == Err(PacketError::ByteBudgetExceeded));
        ensure!(reader.total_bytes() == 2 * MAX_FILE_BYTES + 1);
        ensure!(reader.read("over", false) == Err(PacketError::ByteBudgetExceeded));
        ensure!(reader.total_bytes() == 2 * MAX_FILE_BYTES + 1 && reader.attempts() == 2);
    }
    for index in 0..8 {
        fs::write(
            root.path().join(format!("aggregate-{index}")),
            &buffer[..MAX_FILE_BYTES],
        )?;
    }
    {
        let mut reader = PacketReader::new(root.path())?;
        for index in 0..8 {
            ensure!(reader.read(&format!("aggregate-{index}"), false)?.is_some());
        }
        ensure!(reader.total_bytes() == MAX_INPUT_BYTES && reader.sources().len() == 8);
        ensure!(reader.read("empty", false) == Err(PacketError::ByteBudgetExceeded));
        ensure!(reader.total_bytes() == MAX_INPUT_BYTES);
    }
    {
        let mut reader = PacketReader::new(root.path())?;
        for index in 0..7 {
            ensure!(reader.read(&format!("aggregate-{index}"), false)?.is_some());
        }
        ensure!(reader.read("over", false) == Err(PacketError::ByteBudgetExceeded));
        ensure!(reader.total_bytes() == MAX_INPUT_BYTES + 1 && reader.sources().len() == 7);
        ensure!(reader.read("over", false) == Err(PacketError::ByteBudgetExceeded));
        ensure!(reader.total_bytes() == MAX_INPUT_BYTES + 1);
    }
    {
        let mut reader = PacketReader::new(root.path())?;
        ensure!(reader.read("empty", false)?.is_some());
        for index in 1..MAX_INPUT_FILES {
            ensure!(reader.read(&format!("missing-{index}"), false)?.is_none());
        }
        ensure!(reader.attempts() == MAX_INPUT_FILES);
        ensure!(reader.read("attempt-257", false) == Err(PacketError::FileBudgetExceeded));
        ensure!(reader.read("empty", true)?.is_some());
        ensure!(reader.read("missing-255", true)?.is_none());
        ensure!(reader.attempts() == MAX_INPUT_FILES && reader.total_bytes() == 0);
        ensure!(
            reader
                .issues()
                .iter()
                .any(|issue| issue.code == "missing_projection" && issue.path == "missing-255")
        );
    }
    Ok(())
}

#[test]
fn canonical_report_metadata_is_sorted_compact_utf8_without_native_numbers() -> Result<()> {
    let value = serde_json::json!({"é":"雪", "z":{"β":2,"a":1},"a":[1]});
    ensure!(canonical(&value)? == "{\"a\":[1],\"z\":{\"a\":1,\"β\":2},\"é\":\"雪\"}\n".as_bytes());
    Ok(())
}
