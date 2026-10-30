//! The first #1292 migration component: bounded input reading, never a
//! TaskLedger replay or a projection-coherence decision.

use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::json;

#[path = "../../src/task_projection_reconciliation/reader.rs"]
mod reader;

use reader::{JsonShape, JsonValue as Value, PacketReader, canonical, strict_json};

const READER_SCHEMA: &str = "ub-review.task_projection_reader.v1";
const MAX_REPORT_BYTES: usize = 512 * 1024;

const REVIEW_INPUTS: &[(&str, JsonShape, Option<&str>, bool)] = &[
    (
        "input/revision-admission.json",
        JsonShape::Object,
        Some("ub-review.revision_admission.v1"),
        true,
    ),
    (
        "review/task_ledger_snapshot.json",
        JsonShape::Object,
        Some("ub-review.task_ledger_snapshot.v1"),
        true,
    ),
    (
        "work_queue.json",
        JsonShape::Object,
        Some("ub-review.work_queue.v1"),
        true,
    ),
    (
        "review/proof_portfolio.json",
        JsonShape::Object,
        Some("ub-review.proof_portfolio.v1"),
        true,
    ),
    ("review/proof_receipts.json", JsonShape::Array, None, true),
    ("review/resource_leases.json", JsonShape::Array, None, false),
    (
        "review/gate_outcome.json",
        JsonShape::Object,
        Some("ub-review.gate_outcome.v1"),
        true,
    ),
    ("review/proof_requests.json", JsonShape::Array, None, true),
    (
        "review/fill-ledger.json",
        JsonShape::Object,
        Some("ub-review.fill_ledger.v1"),
        false,
    ),
    (
        "review/scheduler.json",
        JsonShape::Object,
        Some("ub-review.scheduler.v1"),
        false,
    ),
    (
        "review/receipt_routes.json",
        JsonShape::Object,
        Some("ub-review.receipt_routes.v1"),
        true,
    ),
    (
        "review/calibration.json",
        JsonShape::Object,
        Some("ub-review.calibration.v0"),
        false,
    ),
    ("review/metrics.json", JsonShape::Object, None, false),
    (
        "review/ub-review-cost.json",
        JsonShape::Object,
        Some("ub-review.cost_receipt.v1"),
        false,
    ),
];

const WORKER_INPUTS: &[(&str, JsonShape, Option<&str>, bool)] = &[
    (
        "proof_receipt.json",
        JsonShape::Object,
        Some("ub-review.proof_receipt.v1"),
        true,
    ),
    (
        "resource_lease.json",
        JsonShape::Object,
        Some("ub-review.resource_lease.v1"),
        true,
    ),
    (
        "review/task_ledger_snapshot.json",
        JsonShape::Object,
        Some("ub-review.task_ledger_snapshot.v1"),
        true,
    ),
];

pub(super) fn run(mut args: impl Iterator<Item = String>) -> Result<i32> {
    let root = args
        .next()
        .context("usage: cargo xtask task-projection-reader <packet> [--kind review|worker]")?;
    let mut kind = "review".to_owned();
    while let Some(arg) = args.next() {
        if arg != "--kind" {
            bail!("unexpected task-projection-reader argument `{arg}`");
        }
        kind = args.next().context("--kind requires review or worker")?;
        if !matches!(kind.as_str(), "review" | "worker") {
            bail!("--kind must be review or worker");
        }
    }
    let mut packet = match PacketReader::new(&PathBuf::from(root)) {
        Ok(packet) => packet,
        Err(_) => {
            let evidence = json!({
                "schema": READER_SCHEMA,
                "authority": "reader-only-shadow",
                "packet_kind": kind,
                "reader_status": "input_unavailable",
                "reconciliation": "not_performed",
                "input_unavailable": true,
                "issues": [{"code":"invalid_reader_root","path":"","identity":""}],
            });
            io::stdout().lock().write_all(&canonical(&evidence)?)?;
            return Ok(2);
        }
    };
    let inputs = if kind == "worker" {
        WORKER_INPUTS
    } else {
        REVIEW_INPUTS
    };
    for &(path, shape, schema, required) in inputs {
        match packet.load(path, shape, schema, required) {
            Ok(Some(document)) => inspect_collections(&mut packet, path, &document),
            Ok(None) if required => packet.mark_unavailable("missing_required_reader_input", path),
            Ok(None) | Err(_) => {}
        }
    }
    let mut invalid_event_lines = false;
    if let Ok(Some(events)) = packet.read("task_ledger_events.ndjson", true) {
        let mut line_count = 0usize;
        for line in events
            .split(|byte| *byte == b'\n')
            .filter(|line| !line.is_empty())
        {
            line_count += 1;
            if line_count > reader::MAX_ROWS || strict_json(line).is_err() {
                invalid_event_lines = true;
                break;
            }
        }
    } else if packet
        .coverage()
        .get("task_ledger_events.ndjson")
        .map(String::as_str)
        == Some("unavailable")
    {
        packet.mark_unavailable("missing_required_reader_input", "task_ledger_events.ndjson");
    }
    if invalid_event_lines {
        packet.mark_unavailable("invalid_ledger_json_lines", "task_ledger_events.ndjson");
    }
    if kind == "review" {
        inspect_sensor_inputs(&mut packet)?;
    }
    let evidence = json!({
        "schema": READER_SCHEMA,
        "authority": "reader-only-shadow",
        "packet_kind": kind,
        "reader_status": if packet.input_unavailable() { "input_unavailable" } else { "inputs_read" },
        "reconciliation": "not_performed",
        "bytes_observed": packet.total_bytes(),
        "source_attempts": packet.attempts(),
        "coverage": packet.coverage(),
        "source_artifacts": packet.sources().values().collect::<Vec<_>>(),
        "issues": packet.issues(),
        "issues_truncated": packet.issues_overflow(),
        "input_unavailable": packet.input_unavailable(),
    });
    let bytes = canonical(&evidence)?;
    if bytes.len() > MAX_REPORT_BYTES {
        let unavailable = json!({
            "schema": READER_SCHEMA,
            "authority": "reader-only-shadow",
            "packet_kind": kind,
            "reader_status": "input_unavailable",
            "reconciliation": "not_performed",
            "bytes_observed": packet.total_bytes(),
            "source_attempts": packet.attempts(),
            "source_artifacts_truncated": true,
            "issues": [{"code":"reader_report_budget","path":"","identity":""}],
            "input_unavailable": true,
        });
        io::stdout().lock().write_all(&canonical(&unavailable)?)?;
        return Ok(2);
    }
    io::stdout()
        .lock()
        .write_all(&bytes)
        .context("write reader-only evidence")?;
    Ok(if packet.input_unavailable() { 2 } else { 0 })
}

fn inspect_collections(packet: &mut PacketReader, path: &str, document: &Value) {
    let rows_key = match path {
        "review/task_ledger_snapshot.json" | "work_queue.json" => Some("tasks"),
        "review/proof_portfolio.json" => Some("candidate_tasks"),
        "review/fill-ledger.json" => Some("entries"),
        "review/receipt_routes.json" => Some("routes"),
        _ => None,
    };
    if let Some(key) = rows_key {
        let _ = packet.rows(Some(document), path, Some(key));
    }
    if document.is_array() && packet.rows(Some(document), path, None).is_empty() {
        // Empty or rejected row inventories have nothing safe to inspect.
        // Never rescan a malformed collection after its bounded validation.
        return;
    }
    if path == "review/proof_portfolio.json" {
        let _ = packet.rows(Some(document), path, Some("decisions"));
        let _ = packet.strings(document, path, "selected_task_ids");
        let _ = packet.mapping(document, path, "runtime", "");
    }
    if path == "review/resource_leases.json" {
        for lease in document.as_array().into_iter().flatten() {
            if let Some(lease) = lease.as_object() {
                let _ = packet.lease_quantities(lease, path, "");
            }
        }
    }
    if path == "resource_lease.json"
        && let Some(lease) = document.as_object()
    {
        let _ = packet.lease_quantities(lease, path, "");
    }
    if matches!(path, "review/proof_receipts.json" | "proof_receipt.json") {
        for proof in std::iter::once(document)
            .filter(|_| path == "proof_receipt.json")
            .chain(document.as_array().into_iter().flatten())
        {
            let _ = packet.strings(proof, path, "request_ids");
            let _ = packet.strings(proof, path, "requested_by");
            for command in packet.rows(Some(proof), path, Some("commands")) {
                if command
                    .get("duration_ms")
                    .and_then(reader::integer_u128)
                    .is_none()
                {
                    packet.mark_unavailable("invalid_command_duration", path);
                }
            }
        }
    }
}

fn inspect_sensor_inputs(packet: &mut PacketReader) -> Result<()> {
    let directory = match packet.confined_path("sensors") {
        Ok(directory) => directory,
        Err(_) => {
            packet.mark_unavailable("invalid_sensor_inventory", "sensors");
            return Ok(());
        }
    };
    if !directory.exists() {
        return Ok(());
    }
    let mut names = Vec::new();
    let inventory = match fs::read_dir(&directory) {
        Ok(inventory) => inventory,
        Err(_) => {
            packet.mark_unavailable("invalid_sensor_inventory", "sensors");
            return Ok(());
        }
    };
    for item in inventory {
        if names.len() >= reader::MAX_INPUT_FILES {
            packet.mark_unavailable("invalid_sensor_inventory", "sensors");
            return Ok(());
        }
        let name = match item.and_then(|item| {
            item.file_name()
                .into_string()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "sensor name is not UTF-8"))
        }) {
            Ok(name) => name,
            Err(_) => {
                packet.mark_unavailable("invalid_sensor_inventory", "sensors");
                return Ok(());
            }
        };
        names.push(name);
    }
    names.sort();
    for name in names {
        let path = format!("sensors/{name}/ub-review-sensor-status.json");
        let _ = packet.load(&path, JsonShape::Object, None, false);
    }
    Ok(())
}
