//! Deterministic terminal projection over the immutable planner queue.

use crate::*;

pub(super) const TERMINAL_QUEUE_FILE: &str = "work_queue_terminal.json";
const TERMINAL_EVENTS_FILE: &str = "work_events_terminal.ndjson";
const TERMINAL_QUEUE_TMP_FILE: &str = ".work_queue_terminal.json.tmp";
const TERMINAL_EVENTS_TMP_FILE: &str = ".work_events_terminal.ndjson.tmp";

#[derive(Debug, Serialize)]
struct TerminalQueue {
    schema: &'static str,
    source_plan: &'static str,
    source_plan_sha256: String,
    source_receipts: Vec<String>,
    tasks: Vec<TerminalTask>,
}

#[derive(Debug, Serialize)]
struct TerminalTask {
    id: String,
    kind: String,
    source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_status: Option<String>,
    status: String,
    reason: String,
    request_ids: Vec<String>,
    receipt_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    receipt_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plan_task: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct TerminalEvent<'a> {
    schema: &'static str,
    kind: &'static str,
    task_id: &'a str,
    task_kind: &'a str,
    source: &'a str,
    status: &'a str,
    reason: &'a str,
    request_ids: &'a [String],
    receipt_ids: &'a [String],
}

pub(super) fn write_terminal_work_queue_artifacts(
    out: &Path,
    proof_receipts: &[ProofReceipt],
) -> Result<()> {
    remove_terminal_work_queue_artifacts(out)?;

    let plan_path = out.join("work_queue_plan.json");
    let plan_bytes = fs::read(&plan_path)
        .with_context(|| format!("read terminal queue plan {}", plan_path.display()))?;
    let plan: serde_json::Value = serde_json::from_slice(&plan_bytes)
        .with_context(|| format!("parse terminal queue plan {}", plan_path.display()))?;
    anyhow::ensure!(
        plan.get("schema").and_then(serde_json::Value::as_str) == Some(WORK_QUEUE_SCHEMA),
        "terminal queue plan has unsupported schema"
    );
    let plan_tasks = plan
        .get("tasks")
        .and_then(serde_json::Value::as_array)
        .context("terminal queue plan has no task array")?;
    let proof_request_ids = load_proof_task_request_ids(out)?;
    let receipt_request_ids = validate_proof_receipts(proof_receipts)?;
    let mut source_receipts = BTreeSet::new();
    let mut tasks = Vec::new();
    let mut identities = BTreeSet::new();
    let mut joined_receipts = BTreeMap::<String, String>::new();

    for plan_task in plan_tasks {
        let task = terminalize_planned_task(
            out,
            plan_task,
            proof_receipts,
            &proof_request_ids,
            &receipt_request_ids,
            &mut source_receipts,
        )?;
        anyhow::ensure!(
            identities.insert(task.id.clone()),
            "terminal queue duplicate task identity {}",
            task.id
        );
        for receipt_id in &task.receipt_ids {
            if let Some(previous_task) = joined_receipts.insert(receipt_id.clone(), task.id.clone())
            {
                anyhow::bail!(
                    "terminal queue proof receipt {receipt_id} joins multiple planned tasks {previous_task} and {}",
                    task.id
                );
            }
        }
        tasks.push(task);
    }

    if !proof_receipts.is_empty() {
        source_receipts.insert("review/proof_receipts.json".to_owned());
    }
    for receipt in proof_receipts {
        if joined_receipts.contains_key(&receipt.id) {
            continue;
        }
        anyhow::ensure!(
            identities.insert(receipt.id.clone()),
            "terminal queue unjoined proof receipt identity {} collides with planned task identity",
            receipt.id
        );
        let request_ids = receipt_request_ids
            .get(&receipt.id)
            .cloned()
            .with_context(|| format!("validated proof receipt {} disappeared", receipt.id))?;
        tasks.push(TerminalTask {
            id: receipt.id.clone(),
            kind: receipt.kind.clone(),
            source: "proof-receipt".to_owned(),
            plan_status: None,
            status: terminal_proof_receipt_status(receipt)?,
            reason: receipt.reason.clone(),
            request_ids,
            receipt_ids: vec![receipt.id.clone()],
            receipt_path: Some(format!("review/proof_receipts.json#{}", receipt.id)),
            task_path: None,
            plan_task: None,
        });
    }

    tasks.sort_by(|left, right| left.id.cmp(&right.id));
    let artifact = TerminalQueue {
        schema: WORK_QUEUE_TERMINAL_SCHEMA,
        source_plan: "work_queue_plan.json",
        source_plan_sha256: sha256_hex(&plan_bytes),
        source_receipts: source_receipts.into_iter().collect(),
        tasks,
    };
    let queue_bytes = serde_json::to_vec_pretty(&artifact)?;

    let mut events = String::new();
    for task in &artifact.tasks {
        events.push_str(&serde_json::to_string(&TerminalEvent {
            schema: WORK_EVENT_TERMINAL_SCHEMA,
            kind: "task_terminal_projected",
            task_id: &task.id,
            task_kind: &task.kind,
            source: &task.source,
            status: &task.status,
            reason: &task.reason,
            request_ids: &task.request_ids,
            receipt_ids: &task.receipt_ids,
        })?);
        events.push('\n');
    }

    publish_terminal_work_queue_artifacts(out, &queue_bytes, events.as_bytes())
}

pub(super) fn remove_terminal_work_queue_artifacts(out: &Path) -> Result<()> {
    for name in [
        TERMINAL_QUEUE_FILE,
        TERMINAL_EVENTS_FILE,
        TERMINAL_QUEUE_TMP_FILE,
        TERMINAL_EVENTS_TMP_FILE,
    ] {
        let path = out.join(name);
        match fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("remove stale terminal queue artifact {}", path.display())
                });
            }
        }
    }
    Ok(())
}

fn publish_terminal_work_queue_artifacts(
    out: &Path,
    queue_bytes: &[u8],
    event_bytes: &[u8],
) -> Result<()> {
    let queue_path = out.join(TERMINAL_QUEUE_FILE);
    let events_path = out.join(TERMINAL_EVENTS_FILE);
    let queue_tmp = out.join(TERMINAL_QUEUE_TMP_FILE);
    let events_tmp = out.join(TERMINAL_EVENTS_TMP_FILE);

    fs::write(&queue_tmp, queue_bytes)
        .with_context(|| format!("stage terminal queue artifact {}", queue_tmp.display()))?;
    if let Err(error) = fs::write(&events_tmp, event_bytes) {
        let _ = fs::remove_file(&queue_tmp);
        let _ = fs::remove_file(&events_tmp);
        return Err(error)
            .with_context(|| format!("stage terminal event artifact {}", events_tmp.display()));
    }
    if let Err(error) = fs::rename(&events_tmp, &events_path) {
        let _ = fs::remove_file(&queue_tmp);
        let _ = fs::remove_file(&events_tmp);
        return Err(error)
            .with_context(|| format!("publish terminal event artifact {}", events_path.display()));
    }
    if let Err(error) = fs::rename(&queue_tmp, &queue_path) {
        let _ = fs::remove_file(&queue_tmp);
        let _ = fs::remove_file(&events_path);
        return Err(error)
            .with_context(|| format!("publish terminal queue artifact {}", queue_path.display()));
    }
    Ok(())
}

fn validate_proof_receipts(receipts: &[ProofReceipt]) -> Result<BTreeMap<String, Vec<String>>> {
    let mut result = BTreeMap::new();
    for receipt in receipts {
        anyhow::ensure!(
            receipt.schema == PROOF_RECEIPT_SCHEMA,
            "terminal queue proof receipt {} has unsupported schema {}",
            receipt.id,
            receipt.schema
        );
        anyhow::ensure!(
            !receipt.id.trim().is_empty(),
            "terminal queue proof receipt has empty identity"
        );
        terminal_proof_receipt_status(receipt)?;
        let mut request_ids = Vec::with_capacity(receipt.request_ids.len());
        for request_id in &receipt.request_ids {
            anyhow::ensure!(
                !request_id.trim().is_empty(),
                "terminal queue proof receipt {} contains an empty request identity",
                receipt.id
            );
            request_ids.push(request_id.clone());
        }
        request_ids.sort();
        request_ids.dedup();
        anyhow::ensure!(
            result.insert(receipt.id.clone(), request_ids).is_none(),
            "terminal queue duplicate proof receipt identity {}",
            receipt.id
        );
    }
    Ok(result)
}

fn terminalize_planned_task(
    out: &Path,
    plan_task: &serde_json::Value,
    proof_receipts: &[ProofReceipt],
    proof_request_ids: &BTreeMap<String, Vec<String>>,
    receipt_request_ids: &BTreeMap<String, Vec<String>>,
    source_receipts: &mut BTreeSet<String>,
) -> Result<TerminalTask> {
    let object = plan_task
        .as_object()
        .context("terminal queue plan task is not an object")?;
    anyhow::ensure!(
        object.get("schema").and_then(serde_json::Value::as_str) == Some(WORK_QUEUE_TASK_SCHEMA),
        "terminal queue plan task has unsupported schema"
    );
    let id = string_field(object, "id")?;
    let kind = string_field(object, "kind")?;
    let source = string_field(object, "source")?;
    let plan_status = string_field(object, "status")?;
    // Admit only states emitted by the sensor and focused-proof planners.
    // A receipt result or an in-flight state cannot stand in for planner intent.
    let permitted_statuses: &[&str] = match kind.as_str() {
        "sensor" => &["planned", "skipped"],
        "focused-test" | "focused-build" => &["planned", "deferred_by_budget"],
        _ => anyhow::bail!("terminal queue plan task has unsupported kind {kind}"),
    };
    anyhow::ensure!(
        permitted_statuses.contains(&plan_status.as_str()),
        "terminal queue {kind} task {id} has unsupported plan status {plan_status}"
    );
    let receipt_path = optional_string_field(object, "receipt_path");
    let task_path = optional_string_field(object, "task_path");
    let (status, reason, request_ids, receipt_ids) = match kind.as_str() {
        "sensor" => terminalize_sensor(
            out,
            &id,
            &plan_status,
            receipt_path.as_deref(),
            source_receipts,
        )?,
        "focused-test" | "focused-build" => terminalize_proof(
            &id,
            &plan_status,
            proof_receipts,
            proof_request_ids,
            receipt_request_ids,
        )?,
        _ => anyhow::bail!("terminal queue plan task has unsupported kind {kind}"),
    };
    Ok(TerminalTask {
        id,
        kind,
        source,
        plan_status: Some(plan_status),
        status,
        reason,
        request_ids,
        receipt_ids,
        receipt_path,
        task_path,
        plan_task: Some(plan_task.clone()),
    })
}

fn terminalize_sensor(
    out: &Path,
    task_id: &str,
    plan_status: &str,
    receipt_path: Option<&str>,
    source_receipts: &mut BTreeSet<String>,
) -> Result<(String, String, Vec<String>, Vec<String>)> {
    if plan_status == "skipped" {
        return Ok((
            "skipped".to_owned(),
            "sensor was not selected by the immutable plan".to_owned(),
            Vec::new(),
            Vec::new(),
        ));
    }
    let receipt_path = receipt_path.context("planned sensor has no receipt path")?;
    let sensor_id = task_id
        .strip_prefix("sensor-")
        .context("sensor queue task identity lacks sensor- prefix")?;
    let path = sensor_receipt_path(out, sensor_id, receipt_path)?;
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok((
                "missing_receipt".to_owned(),
                "planned sensor produced no terminal status receipt".to_owned(),
                Vec::new(),
                Vec::new(),
            ));
        }
        Err(error) => {
            return Err(error).with_context(|| format!("read sensor receipt {}", path.display()));
        }
    };
    let receipt: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse sensor receipt {}", path.display()))?;
    anyhow::ensure!(
        receipt.get("sensor").and_then(serde_json::Value::as_str) == Some(sensor_id),
        "sensor receipt identity does not match terminal queue task {task_id}"
    );
    let status = receipt
        .get("status")
        .and_then(serde_json::Value::as_str)
        .context("sensor receipt has no string status")?;
    anyhow::ensure!(
        matches!(
            status,
            "ok" | "failed" | "timed_out" | "missing" | "skipped"
        ),
        "sensor receipt has unsupported terminal status {status}"
    );
    let reason = receipt
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("sensor terminal receipt supplied no reason")
        .to_owned();
    source_receipts.insert(receipt_path.to_owned());
    Ok((status.to_owned(), reason, Vec::new(), Vec::new()))
}

fn sensor_receipt_path(out: &Path, sensor_id: &str, receipt_path: &str) -> Result<PathBuf> {
    anyhow::ensure!(
        !sensor_id.is_empty()
            && !matches!(sensor_id, "." | "..")
            && !sensor_id.contains(['/', '\\', ':', '\0']),
        "sensor receipt path has invalid sensor identity {sensor_id}"
    );
    anyhow::ensure!(
        receipt_path == format!("sensors/{sensor_id}/ub-review-sensor-status.json"),
        "sensor receipt path does not match producer path for {sensor_id}"
    );
    // The output root is trusted and single-writer. Refuse existing redirects
    // below it; this is not isolation against concurrent filesystem mutation.
    let mut path = out.to_path_buf();
    for component in ["sensors", sensor_id, "ub-review-sensor-status.json"] {
        path.push(component);
        match fs::symlink_metadata(&path) {
            Ok(metadata) => anyhow::ensure!(
                !metadata.file_type().is_symlink(),
                "sensor receipt path contains a symlink: {}",
                path.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("inspect sensor receipt path {}", path.display()));
            }
        }
    }
    Ok(path)
}

fn terminalize_proof(
    task_id: &str,
    plan_status: &str,
    receipts: &[ProofReceipt],
    proof_request_ids: &BTreeMap<String, Vec<String>>,
    receipt_request_ids: &BTreeMap<String, Vec<String>>,
) -> Result<(String, String, Vec<String>, Vec<String>)> {
    let mut request_ids = proof_request_ids.get(task_id).cloned().unwrap_or_default();
    request_ids.sort();
    request_ids.dedup();
    let request_set = request_ids
        .iter()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut matching = receipts
        .iter()
        .filter_map(|receipt| {
            let request_identity_matches =
                receipt_request_ids
                    .get(&receipt.id)
                    .is_some_and(|receipt_requests| {
                        receipt_requests
                            .iter()
                            .any(|request_id| request_set.contains(request_id.as_str()))
                    });
            let task_identity_matches = receipt.id == task_id;
            let relation = match (request_identity_matches, task_identity_matches) {
                (true, true) => "request_and_task_identity",
                (true, false) => "request_identity",
                (false, true) => "task_identity",
                (false, false) => return None,
            };
            Some((receipt, relation))
        })
        .collect::<Vec<_>>();
    matching.sort_by(|(left, _), (right, _)| left.id.cmp(&right.id));
    let receipt_ids = matching
        .iter()
        .map(|(receipt, _)| receipt.id.clone())
        .collect::<Vec<_>>();
    if matching.is_empty() {
        let (status, reason) = if plan_status == "planned" {
            (
                "not_executed".to_owned(),
                "planned proof task has no terminal receipt joined by request or task identity"
                    .to_owned(),
            )
        } else {
            (
                plan_status.to_owned(),
                "proof task retained its terminal planner disposition".to_owned(),
            )
        };
        return Ok((status, reason, request_ids, receipt_ids));
    }
    let mut results = matching
        .iter()
        .map(|(receipt, _)| terminal_proof_receipt_status(receipt))
        .collect::<Result<Vec<_>>>()?;
    results.sort();
    results.dedup();
    let status = if results.len() == 1 {
        results[0].clone()
    } else {
        "multiple_terminal_receipts".to_owned()
    };
    let reason = matching
        .iter()
        .map(|(receipt, relation)| format!("{}={} join={relation}", receipt.id, receipt.result))
        .collect::<Vec<_>>()
        .join("; ");
    Ok((status, reason, request_ids, receipt_ids))
}

fn terminal_proof_receipt_status(receipt: &ProofReceipt) -> Result<String> {
    anyhow::ensure!(
        !receipt.result.trim().is_empty(),
        "proof receipt {} has empty terminal result",
        receipt.id
    );
    // These are the terminal results emitted by the current focused build
    // and red/green brokers. Unknown or in-flight states are not evidence of
    // completion, even when an otherwise valid receipt contains them.
    anyhow::ensure!(
        matches!(
            receipt.result.as_str(),
            "head_passed"
                | "head_failed"
                | "discriminating"
                | "non_discriminating"
                | "base_patch_failed"
                | "timed_out"
                | "skipped_budget"
                | "skipped_profile"
        ),
        "proof receipt {} has unsupported terminal result {}",
        receipt.id,
        receipt.result
    );
    Ok(receipt.result.clone())
}

fn load_proof_task_request_ids(out: &Path) -> Result<BTreeMap<String, Vec<String>>> {
    let path = out.join("proof_tasks.ndjson");
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error).with_context(|| format!("read proof tasks {}", path.display()));
        }
    };
    let mut result = BTreeMap::new();
    for (index, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let task: serde_json::Value = serde_json::from_str(line)
            .with_context(|| format!("parse proof_tasks.ndjson line {}", index + 1))?;
        let object = task
            .as_object()
            .context("proof task row is not an object")?;
        anyhow::ensure!(
            object.get("schema").and_then(serde_json::Value::as_str) == Some(PROOF_TASK_SCHEMA),
            "proof task catalog line {} has unsupported schema",
            index + 1
        );
        let id = string_field(object, "id")?;
        let request_ids = string_list_field(object, "request_ids")?;
        anyhow::ensure!(
            result.insert(id.clone(), request_ids).is_none(),
            "duplicate proof task identity {id}"
        );
    }
    Ok(result)
}

fn string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .with_context(|| format!("terminal queue object has no nonempty {field}"))
}

fn optional_string_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Option<String> {
    object
        .get(field)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

fn string_list_field(
    object: &serde_json::Map<String, serde_json::Value>,
    field: &str,
) -> Result<Vec<String>> {
    let values = object
        .get(field)
        .and_then(serde_json::Value::as_array)
        .with_context(|| format!("terminal queue object has no {field} array"))?;
    let mut result = Vec::with_capacity(values.len());
    for value in values {
        let value = value
            .as_str()
            .filter(|value| !value.trim().is_empty())
            .with_context(|| format!("terminal queue {field} contains a non-string identity"))?;
        result.push(value.to_owned());
    }
    result.sort();
    result.dedup();
    Ok(result)
}

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod tests;
