use super::*;

fn sensor_task(id: &str, status: &str) -> serde_json::Value {
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

fn proof_plan_task(id: &str) -> serde_json::Value {
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

fn proof_receipt(id: &str, request_ids: &[&str], requested_by: &[&str]) -> ProofReceipt {
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

fn write_plan(out: &Path, tasks: Vec<serde_json::Value>) -> Result<Vec<u8>> {
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

fn write_proof_tasks(out: &Path, tasks: &[(&str, &[&str])]) -> Result<()> {
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

fn write_sensor_receipt(out: &Path, id: &str, status: &str) -> Result<()> {
    let path = out
        .join("sensors")
        .join(id)
        .join("ub-review-sensor-status.json");
    fs::create_dir_all(path.parent().context("sensor receipt has no parent")?)?;
    fs::write(
        path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "sensor": id,
            "status": status,
            "reason": format!("sensor ended {status}")
        }))?,
    )?;
    Ok(())
}

fn receipt_reference_count(rows: &[serde_json::Value], receipt_id: &str) -> usize {
    rows.iter()
        .filter_map(|row| row["receipt_ids"].as_array())
        .flatten()
        .filter(|value| value.as_str() == Some(receipt_id))
        .count()
}

#[test]
fn terminal_projection_preserves_plan_and_accounts_for_receipts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    let plan_bytes = write_plan(
        out,
        vec![
            sensor_task("ok", "planned"),
            sensor_task("failed", "planned"),
            sensor_task("timed", "planned"),
            sensor_task("missing-command", "planned"),
            sensor_task("runtime-skipped", "planned"),
            sensor_task("absent", "planned"),
            sensor_task("skipped", "skipped"),
            proof_plan_task("proof-task-a"),
        ],
    )?;
    write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
    for (id, status) in [
        ("ok", "ok"),
        ("failed", "failed"),
        ("timed", "timed_out"),
        ("missing-command", "missing"),
        ("runtime-skipped", "skipped"),
    ] {
        write_sensor_receipt(out, id, status)?;
    }
    let receipts = vec![
        proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]),
        proof_receipt("impact-receipt", &["impact-a"], &["impact-planner"]),
    ];

    write_terminal_work_queue_artifacts(out, &receipts)?;
    let first = fs::read(out.join(TERMINAL_QUEUE_FILE))?;
    assert_eq!(fs::read(out.join("work_queue_plan.json"))?, plan_bytes);
    assert_eq!(fs::read(out.join("work_queue.json"))?, plan_bytes);
    write_terminal_work_queue_artifacts(out, &receipts)?;
    assert_eq!(fs::read(out.join(TERMINAL_QUEUE_FILE))?, first);

    let terminal: serde_json::Value = serde_json::from_slice(&first)?;
    assert_eq!(terminal["schema"], WORK_QUEUE_TERMINAL_SCHEMA);
    let rows = terminal["tasks"]
        .as_array()
        .context("terminal tasks missing")?;
    let by_id = rows
        .iter()
        .map(|row| {
            let id = row["id"].as_str().context("terminal task id missing")?;
            Ok((id.to_owned(), row))
        })
        .collect::<Result<BTreeMap<_, _>>>()?;
    for (id, status) in [
        ("sensor-ok", "ok"),
        ("sensor-failed", "failed"),
        ("sensor-timed", "timed_out"),
        ("sensor-missing-command", "missing"),
        ("sensor-runtime-skipped", "skipped"),
        ("sensor-absent", "missing_receipt"),
        ("sensor-skipped", "skipped"),
        ("proof-task-a", "head_passed"),
        ("impact-receipt", "head_passed"),
    ] {
        assert_eq!(
            by_id[id]["status"], status,
            "wrong terminal status for {id}"
        );
    }
    assert!(
        !by_id.contains_key("proof-receipt-a"),
        "joined receipt must not be duplicated as an independent task"
    );
    assert_eq!(
        by_id["proof-task-a"]["receipt_ids"],
        serde_json::json!(["proof-receipt-a"])
    );
    assert!(
        by_id["proof-task-a"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("join=request_identity"))
    );
    assert!(by_id["impact-receipt"]["plan_status"].is_null());
    assert_eq!(receipt_reference_count(rows, "proof-receipt-a"), 1);
    assert_eq!(receipt_reference_count(rows, "impact-receipt"), 1);
    assert_eq!(
        fs::read_to_string(out.join(TERMINAL_EVENTS_FILE))?
            .lines()
            .count(),
        rows.len()
    );
    assert!(!out.join(TERMINAL_QUEUE_TMP_FILE).exists());
    assert!(!out.join(TERMINAL_EVENTS_TMP_FILE).exists());
    Ok(())
}

#[test]
fn terminal_projection_joins_current_receipt_by_exact_task_identity() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(out, vec![proof_plan_task("proof-receipt-a")])?;
    write_proof_tasks(out, &[("proof-receipt-a", &["planned-request"])])?;
    let receipts = vec![proof_receipt(
        "proof-receipt-a",
        &["different-request"],
        &["impact-planner"],
    )];

    write_terminal_work_queue_artifacts(out, &receipts)?;
    let terminal: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
    let rows = terminal["tasks"]
        .as_array()
        .context("terminal tasks missing")?;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["id"], "proof-receipt-a");
    assert_eq!(rows[0]["status"], "head_passed");
    assert_eq!(
        rows[0]["receipt_ids"],
        serde_json::json!(["proof-receipt-a"])
    );
    assert!(
        rows[0]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("join=task_identity"))
    );
    assert_eq!(receipt_reference_count(rows, "proof-receipt-a"), 1);
    Ok(())
}

#[test]
fn terminal_projection_removes_stale_outputs_before_invalid_plan_fails() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(out, vec![proof_plan_task("proof-task-a")])?;
    write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
    let receipts = vec![proof_receipt(
        "proof-receipt-a",
        &["req-a"],
        &["tests-oracle"],
    )];
    write_terminal_work_queue_artifacts(out, &receipts)?;
    assert!(out.join(TERMINAL_QUEUE_FILE).is_file());
    assert!(out.join(TERMINAL_EVENTS_FILE).is_file());
    fs::write(out.join(TERMINAL_QUEUE_TMP_FILE), b"stale queue staging")?;
    fs::write(out.join(TERMINAL_EVENTS_TMP_FILE), b"stale event staging")?;

    let mut invalid = proof_plan_task("proof-task-a");
    invalid["schema"] = serde_json::json!("ub-review.work_queue_task.invalid");
    write_plan(out, vec![invalid])?;
    let error = write_terminal_work_queue_artifacts(out, &receipts)
        .err()
        .context("invalid plan task schema unexpectedly succeeded")?;
    assert!(
        format!("{error:#}").contains("plan task has unsupported schema"),
        "unexpected error: {error:#}"
    );
    for name in [
        TERMINAL_QUEUE_FILE,
        TERMINAL_EVENTS_FILE,
        TERMINAL_QUEUE_TMP_FILE,
        TERMINAL_EVENTS_TMP_FILE,
    ] {
        assert!(
            !out.join(name).exists(),
            "failed terminal projection retained stale artifact {name}"
        );
    }
    Ok(())
}

#[test]
fn terminal_projection_rejects_one_receipt_joined_to_multiple_plan_tasks() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(
        out,
        vec![
            proof_plan_task("proof-task-a"),
            proof_plan_task("proof-task-b"),
        ],
    )?;
    write_proof_tasks(
        out,
        &[("proof-task-a", &["req-a"]), ("proof-task-b", &["req-a"])],
    )?;
    let receipts = vec![proof_receipt(
        "proof-receipt-a",
        &["req-a"],
        &["tests-oracle"],
    )];

    let error = write_terminal_work_queue_artifacts(out, &receipts)
        .err()
        .context("ambiguous receipt join unexpectedly succeeded")?;
    assert!(
        format!("{error:#}").contains("joins multiple planned tasks"),
        "unexpected error: {error:#}"
    );
    Ok(())
}

#[test]
fn terminal_projection_rejects_conflicting_task_and_request_identity_joins() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(
        out,
        vec![
            proof_plan_task("proof-receipt-a"),
            proof_plan_task("proof-task-b"),
        ],
    )?;
    write_proof_tasks(
        out,
        &[
            ("proof-receipt-a", &["planned-request"]),
            ("proof-task-b", &["req-b"]),
        ],
    )?;
    let receipts = vec![proof_receipt(
        "proof-receipt-a",
        &["req-b"],
        &["impact-planner"],
    )];

    let error = write_terminal_work_queue_artifacts(out, &receipts)
        .err()
        .context("conflicting receipt joins unexpectedly succeeded")?;
    assert!(
        format!("{error:#}").contains("joins multiple planned tasks"),
        "unexpected error: {error:#}"
    );
    Ok(())
}
