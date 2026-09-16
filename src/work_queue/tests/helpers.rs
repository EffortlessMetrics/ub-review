use super::*;

pub(super) fn sensor_task(id: &str, status: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": WORK_QUEUE_TASK_SCHEMA,
        "id": format!("sensor-{id}"),
        "kind": "sensor",
        "source": "tool-registry",
        "status": status,
        "receipt_path": format!("sensors/{id}/ub-review-sensor-status.json"),
        "task_path": "resolved-tools.json"
    })
}

pub(super) fn proof_plan_task(id: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": WORK_QUEUE_TASK_SCHEMA,
        "id": id,
        "kind": "focused-test",
        "source": "proof-planner",
        "status": "planned",
        "receipt_path": "review/proof_receipts.json",
        "task_path": "proof_tasks.ndjson"
    })
}

pub(super) fn proof_receipt(id: &str, request_ids: &[&str], requested_by: &[&str]) -> ProofReceipt {
    ProofReceipt {
        schema: PROOF_RECEIPT_SCHEMA.to_owned(),
        id: id.to_owned(),
        kind: "focused-head".to_owned(),
        base: "a".repeat(40),
        head: "b".repeat(40),
        revision: None,
        test_patch_mode: "head-only".to_owned(),
        requested_by: requested_by
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        request_ids: request_ids
            .iter()
            .map(|value| (*value).to_owned())
            .collect(),
        commands: vec![ProofCommandReceipt {
            side: "head".to_owned(),
            command: "cargo test --locked queue_terminal".to_owned(),
            env: BTreeMap::new(),
            status: "passed".to_owned(),
            exit_code: Some(0),
            timed_out: false,
            timeout_sec: 30,
            duration_ms: 7,
            stdout: "proof-a.stdout".to_owned(),
            stderr: "proof-a.stderr".to_owned(),
            reason: "command passed".to_owned(),
        }],
        result: "head_passed".to_owned(),
        reason: "focused HEAD proof passed".to_owned(),
    }
}

pub(super) fn write_plan(out: &Path, tasks: Vec<serde_json::Value>) -> Result<Vec<u8>> {
    let plan = serde_json::json!({
        "schema": WORK_QUEUE_SCHEMA,
        "initial_packet_deadline_sec": 60,
        "follow_up_deadline_sec": 300,
        "tasks": tasks
    });
    let plan_bytes = serde_json::to_vec_pretty(&plan)?;
    fs::write(out.join("work_queue_plan.json"), &plan_bytes)?;
    fs::write(out.join("work_queue.json"), &plan_bytes)?;
    Ok(plan_bytes)
}

pub(super) fn write_proof_tasks(out: &Path, tasks: &[(&str, &[&str])]) -> Result<()> {
    let mut ndjson = String::new();
    for (id, request_ids) in tasks {
        ndjson.push_str(&serde_json::to_string(&serde_json::json!({
            "id": id,
            "request_ids": request_ids
        }))?);
        ndjson.push('\n');
    }
    fs::write(out.join("proof_tasks.ndjson"), ndjson)?;
    Ok(())
}

pub(super) fn write_sensor_receipt(out: &Path, id: &str, status: &str) -> Result<()> {
    write_sensor_receipt_value(
        out,
        id,
        &serde_json::json!({
            "sensor": id,
            "status": status,
            "reason": format!("sensor ended {status}")
        }),
    )
}

pub(super) fn write_sensor_receipt_value(out: &Path, id: &str, value: &serde_json::Value) -> Result<()> {
    let path = out
        .join("sensors")
        .join(id)
        .join("ub-review-sensor-status.json");
    fs::create_dir_all(path.parent().context("sensor receipt has no parent")?)?;
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

pub(super) fn receipt_reference_count(rows: &[serde_json::Value], receipt_id: &str) -> usize {
    rows.iter()
        .filter_map(|row| row["receipt_ids"].as_array())
        .flatten()
        .filter(|value| value.as_str() == Some(receipt_id))
        .count()
}

