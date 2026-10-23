//! Executable reader-only differential controls against the unchanged Python
//! Packet boundary. The synthetic packets deliberately establish no ledger
//! or projection coherence.

use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

const PYTHON_PACKET: &str = r#"
import importlib.util,json,sys
from pathlib import Path
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('packet_reference',sys.argv[1])
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
p=m.Packet(Path(sys.argv[2]))
paths=json.loads(sys.argv[3])
for name in paths:
    p.read(name)
print(json.dumps({'coverage':p.coverage,'source_artifacts':[p.sources[k] for k in sorted(p.sources)],'bytes_observed':p.total,'source_attempts':len(p.raw)},sort_keys=True,separators=(',',':'),ensure_ascii=False))
"#;

const PYTHON_JSON: &str = r#"
import importlib.util,sys
sys.dont_write_bytecode=True
spec=importlib.util.spec_from_file_location('packet_reference',sys.argv[1])
m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
try:
    m.strict_json(sys.argv[2].encode('utf-8'))
except (ValueError,UnicodeError):
    sys.exit(2)
"#;

fn write_json(root: &Path, path: &str, value: Value) -> Result<()> {
    let path = root.join(path);
    fs::create_dir_all(path.parent().context("fixture parent")?)?;
    fs::write(path, serde_json::to_vec(&value)?)?;
    Ok(())
}

fn review_inputs(root: &Path) -> Result<()> {
    for (path, schema, fields) in [
        (
            "input/revision-admission.json",
            "ub-review.revision_admission.v1",
            json!({}),
        ),
        (
            "review/task_ledger_snapshot.json",
            "ub-review.task_ledger_snapshot.v1",
            json!({"tasks":[]}),
        ),
        (
            "work_queue.json",
            "ub-review.work_queue.v1",
            json!({"tasks":[]}),
        ),
        (
            "review/proof_portfolio.json",
            "ub-review.proof_portfolio.v1",
            json!({"candidate_tasks":[],"decisions":[],"selected_task_ids":[],"runtime":{}}),
        ),
        (
            "review/gate_outcome.json",
            "ub-review.gate_outcome.v1",
            json!({"reader_fixture":"雪"}),
        ),
        (
            "review/receipt_routes.json",
            "ub-review.receipt_routes.v1",
            json!({"routes":[]}),
        ),
    ] {
        let mut fields = fields.as_object().context("fixture object")?.clone();
        fields.insert("schema".to_owned(), json!(schema));
        write_json(root, path, Value::Object(fields))?;
    }
    write_json(root, "review/proof_receipts.json", json!([]))?;
    write_json(root, "review/proof_requests.json", json!([]))?;
    fs::write(
        root.join("task_ledger_events.ndjson"),
        b"{\"reader_fixture\":true}\n",
    )?;
    Ok(())
}

fn invoke(root: &Path, worker: bool) -> Result<(i32, Value, Vec<u8>)> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_xtask"));
    command.arg("task-projection-reader").arg(root);
    if worker {
        command.args(["--kind", "worker"]);
    }
    let result = command
        .output()
        .context("execute actual native reader command")?;
    ensure!(
        result.stderr.is_empty(),
        "unexpected native reader stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value =
        serde_json::from_slice(&result.stdout).context("parse native reader-only output")?;
    Ok((
        result.status.code().context("native reader exit code")?,
        value,
        result.stdout,
    ))
}

#[test]
fn actual_reader_matches_python_packet_sources_and_cached_bytes() -> Result<()> {
    let root = tempfile::tempdir()?;
    review_inputs(root.path())?;
    write_json(
        root.path(),
        "sensors/native/ub-review-sensor-status.json",
        json!({"status":"ok"}),
    )?;
    let (exit, native, raw) = invoke(root.path(), false)?;
    ensure!(
        exit == 0 && native["reader_status"] == "inputs_read",
        "reader inputs unavailable: {native}"
    );
    ensure!(
        native["schema"] == "ub-review.task_projection_reader.v1",
        "reader output schema drift"
    );
    ensure!(
        native["authority"] == "reader-only-shadow" && native["reconciliation"] == "not_performed",
        "reader evidence implied projection authority"
    );
    ensure!(
        native.get("status").is_none() && native.get("gate_result").is_none(),
        "reader falsely emitted coherence/gate result"
    );
    let paths: Vec<&str> = native["coverage"]
        .as_object()
        .context("native coverage object")?
        .keys()
        .map(String::as_str)
        .collect();
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/reconcile-task-projections.py");
    let python = Command::new("python3")
        .args(["-c", PYTHON_PACKET])
        .arg(script)
        .arg(root.path())
        .arg(serde_json::to_string(&paths)?)
        .output()?;
    ensure!(
        python.status.success(),
        "Python Packet comparator failed: {}",
        String::from_utf8_lossy(&python.stderr)
    );
    let reference: Value = serde_json::from_slice(&python.stdout)?;
    for key in [
        "coverage",
        "source_artifacts",
        "bytes_observed",
        "source_attempts",
    ] {
        ensure!(
            native[key] == reference[key],
            "reader parity differs at {key}: native {}, Python {}",
            native[key],
            reference[key]
        );
    }
    let (_, repeated, repeated_raw) = invoke(root.path(), false)?;
    ensure!(
        native == repeated && raw == repeated_raw,
        "reader-only output is nondeterministic"
    );
    ensure!(
        raw.last() == Some(&b'\n') && raw.len() <= 512 * 1024,
        "canonical reader output bound/newline"
    );
    Ok(())
}

#[test]
fn malformed_source_retains_independent_later_input_evidence() -> Result<()> {
    let root = tempfile::tempdir()?;
    review_inputs(root.path())?;
    fs::write(
        root.path().join("input/revision-admission.json"),
        b"{\"schema\":\"a\",\"nested\":{\"x\":1,\"\\u0078\":2}}",
    )?;
    let (exit, native, _) = invoke(root.path(), false)?;
    ensure!(
        exit == 2 && native["input_unavailable"] == true,
        "malformed input did not fail closed"
    );
    ensure!(
        native["coverage"]["input/revision-admission.json"] == "invalid",
        "duplicate-key diagnostic absent"
    );
    ensure!(
        native["coverage"]["review/gate_outcome.json"] == "read",
        "independent later source was omitted"
    );
    ensure!(
        native["reconciliation"] == "not_performed",
        "malformed input claimed coherence"
    );
    Ok(())
}

#[test]
fn actual_reader_matches_python_strict_json_on_bounded_grammar_vectors() -> Result<()> {
    let root = tempfile::tempdir()?;
    review_inputs(root.path())?;
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/reconcile-task-projections.py");
    for (extra, accepted) in [
        (
            r#"{"$serde_json::private::Number":"42","extra":true}"#,
            true,
        ),
        (r#"{"$serde_json::private::RawValue":"null"}"#, true),
        (
            r#"[340282366920938463463374607431768211455,-0,1.5,1e3,"\ud83d\ude00"]"#,
            true,
        ),
        (r#"{"x":1,"\u0078":2}"#, false),
        (r#"{"nested":{"x":1,"x":2}}"#, false),
        ("01", false),
        ("1.", false),
        ("1e+", false),
    ] {
        let document =
            format!("{{\"schema\":\"ub-review.gate_outcome.v1\",\"reader_fixture\":{extra}}}");
        fs::write(root.path().join("review/gate_outcome.json"), &document)?;
        let (exit, native, _) = invoke(root.path(), false)?;
        let python = Command::new("python3")
            .args(["-c", PYTHON_JSON])
            .arg(&script)
            .arg(&document)
            .output()?;
        ensure!(
            python.stderr.is_empty(),
            "Python grammar comparator failed: {}",
            String::from_utf8_lossy(&python.stderr)
        );
        ensure!(
            python.status.success() == accepted,
            "Python oracle disagreed with grammar vector {extra}"
        );
        ensure!(
            (exit == 0) == accepted,
            "native parser grammar parity differs for {extra}: {native}"
        );
    }
    Ok(())
}

#[test]
fn actual_reader_records_explicit_tighter_python_encoding_and_number_boundary() -> Result<()> {
    let root = tempfile::tempdir()?;
    review_inputs(root.path())?;
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../scripts/reconcile-task-projections.py");
    for document in [
        "{\"schema\":\"ub-review.gate_outcome.v1\",\"reader_fixture\":1e999}",
        "{\"schema\":\"ub-review.gate_outcome.v1\",\"reader_fixture\":\"\\ud800\"}",
        "\u{feff}{\"schema\":\"ub-review.gate_outcome.v1\"}",
    ] {
        fs::write(root.path().join("review/gate_outcome.json"), document)?;
        let (exit, native, _) = invoke(root.path(), false)?;
        let python = Command::new("python3")
            .args(["-c", PYTHON_JSON])
            .arg(&script)
            .arg(document)
            .output()?;
        ensure!(
            python.status.success(),
            "Python tightened-boundary baseline changed: {}",
            String::from_utf8_lossy(&python.stderr)
        );
        ensure!(
            exit == 2 && native["input_unavailable"] == true,
            "documented tighter native boundary admitted input: {native}"
        );
        ensure!(
            native["coverage"]["review/gate_outcome.json"] == "invalid",
            "tighter refusal did not retain invalid source coverage"
        );
    }
    Ok(())
}

#[test]
fn worker_reader_preserves_full_u128_duration_domain() -> Result<()> {
    let root = tempfile::tempdir()?;
    write_json(
        root.path(),
        "resource_lease.json",
        json!({"schema":"ub-review.resource_lease.v1","cpu":1,"memory_mb":8,"disk_mb":8,"timeout_sec":60}),
    )?;
    write_json(
        root.path(),
        "review/task_ledger_snapshot.json",
        json!({"schema":"ub-review.task_ledger_snapshot.v1","tasks":[]}),
    )?;
    fs::write(
        root.path().join("task_ledger_events.ndjson"),
        b"{\"reader_fixture\":true}\n",
    )?;
    for duration in [
        "18446744073709551616",
        "340282366920938463463374607431768211455",
    ] {
        let proof = format!(
            "{{\"schema\":\"ub-review.proof_receipt.v1\",\"request_ids\":[],\"requested_by\":[],\"commands\":[{{\"duration_ms\":{duration}}}]}}"
        );
        fs::write(root.path().join("proof_receipt.json"), proof)?;
        let (exit, native, _) = invoke(root.path(), true)?;
        ensure!(
            exit == 0 && native["packet_kind"] == "worker",
            "valid u128 input refused: {native}"
        );
    }
    for duration in [
        "340282366920938463463374607431768211456",
        "true",
        "-1",
        "1.5",
    ] {
        let proof = format!(
            "{{\"schema\":\"ub-review.proof_receipt.v1\",\"request_ids\":[],\"requested_by\":[],\"commands\":[{{\"duration_ms\":{duration}}}]}}"
        );
        fs::write(root.path().join("proof_receipt.json"), proof)?;
        let (exit, native, _) = invoke(root.path(), true)?;
        ensure!(
            exit == 2 && native["input_unavailable"] == true,
            "invalid duration was admitted: {duration}"
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn sensor_parent_symlink_cannot_escape_reader_root() -> Result<()> {
    let root = tempfile::tempdir()?;
    let outside = tempfile::tempdir()?;
    review_inputs(root.path())?;
    std::os::unix::fs::symlink(outside.path(), root.path().join("sensors"))?;
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("task-projection-reader")
        .arg(root.path())
        .output()?;
    ensure!(
        !result.status.success(),
        "sensor symlink escaped confinement"
    );
    ensure!(
        !String::from_utf8_lossy(&result.stdout).contains("inputs_read"),
        "sensor symlink retained a success claim"
    );
    Ok(())
}

#[cfg(unix)]
#[test]
fn escaped_source_inventory_report_overflow_retains_bounded_unavailable_receipt() -> Result<()> {
    let root = tempfile::tempdir()?;
    review_inputs(root.path())?;
    for index in 0..241 {
        let name = format!("{}{index:03}", "\u{1}".repeat(249));
        write_json(
            root.path(),
            &format!("sensors/{name}/ub-review-sensor-status.json"),
            json!({}),
        )?;
    }
    let (exit, native, raw) = invoke(root.path(), false)?;
    ensure!(
        exit == 2 && native["reader_status"] == "input_unavailable",
        "report overflow lost reader exit/shape contract: {native}"
    );
    ensure!(
        native["issues"][0]["code"] == "reader_report_budget",
        "report overflow receipt missing"
    );
    ensure!(
        native["source_artifacts_truncated"] == true && native["source_attempts"] == 256,
        "report overflow did not identify omitted inventory"
    );
    ensure!(
        native["reconciliation"] == "not_performed" && raw.len() <= 512 * 1024,
        "report overflow exceeded bound or claimed coherence"
    );
    Ok(())
}
#[test]
fn serde_json_ambient_contract_keeps_private_number_keys_as_objects() -> Result<()> {
    for input in [
        r#"{"$serde_json::private::Number":"42"}"#,
        r#"{"$serde_json::private::Number":"42","extra":true}"#,
        r#"{"$serde_json::private::Number":"not a number"}"#,
    ] {
        let value: Value = serde_json::from_slice(input.as_bytes())?;
        ensure!(
            value.is_object(),
            "root parser reclassified an ordinary object: {value}"
        );
        ensure!(
            value
                .get("$serde_json::private::Number")
                .and_then(Value::as_str)
                .is_some(),
            "root parser lost ordinary Number key"
        );
    }
    Ok(())
}

#[test]
fn serde_json_ambient_contract_keeps_private_raw_value_keys_as_objects() -> Result<()> {
    for input in [
        r#"{"$serde_json::private::RawValue":"null"}"#,
        r#"{"$serde_json::private::RawValue":"{\"authorized\":true}","extra":true}"#,
        r#"{"$serde_json::private::RawValue":"not JSON"}"#,
    ] {
        let value: Value = serde_json::from_slice(input.as_bytes())?;
        ensure!(
            value.is_object(),
            "root parser reclassified an ordinary object: {value}"
        );
        ensure!(
            value
                .get("$serde_json::private::RawValue")
                .and_then(Value::as_str)
                .is_some(),
            "root parser lost ordinary RawValue key"
        );
    }
    Ok(())
}

#[test]
fn serde_json_ambient_contract_rejects_nonfinite_float_exponents() -> Result<()> {
    for input in ["1e999", "-1e999", "{\"extra\":[1e999]}"] {
        ensure!(
            serde_json::from_slice::<Value>(input.as_bytes()).is_err(),
            "root parser accepted an out-of-range exponent"
        );
    }
    Ok(())
}
