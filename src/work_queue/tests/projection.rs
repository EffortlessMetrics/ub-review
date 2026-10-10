use super::helpers::*;
use super::*;

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
    assert_eq!(by_id["impact-receipt"]["source"], "proof-receipt");
    assert_eq!(
        by_id["impact-receipt"]["receipt_path"],
        "review/proof_receipts.json#impact-receipt"
    );
    assert!(by_id["impact-receipt"].get("task_path").is_none());
    assert!(by_id["impact-receipt"].get("plan_task").is_none());
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

#[test]
fn terminal_projection_distinguishes_unexecuted_and_multiple_receipts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(
        out,
        vec![
            proof_plan_task("proof-unexecuted"),
            proof_plan_task("proof-multi"),
            proof_plan_task("proof-both"),
        ],
    )?;
    write_proof_tasks(
        out,
        &[
            ("proof-unexecuted", &["req-none"]),
            ("proof-multi", &["req-multi"]),
            ("proof-both", &["req-both"]),
        ],
    )?;
    let passed = proof_receipt("proof-pass", &["req-multi"], &["tests-oracle"]);
    let mut failed = proof_receipt("proof-fail", &["req-multi"], &["tests-oracle"]);
    failed.result = "head_failed".to_owned();
    failed.reason = "focused HEAD proof failed".to_owned();
    let both = proof_receipt("proof-both", &["req-both"], &["tests-oracle"]);

    write_terminal_work_queue_artifacts(out, &[passed, failed, both])?;
    let terminal: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
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

    assert_eq!(by_id["proof-unexecuted"]["status"], "not_executed");
    assert_eq!(
        by_id["proof-unexecuted"]["receipt_ids"],
        serde_json::json!([])
    );
    assert_eq!(by_id["proof-multi"]["status"], "multiple_terminal_receipts");
    assert_eq!(
        by_id["proof-multi"]["receipt_ids"],
        serde_json::json!(["proof-fail", "proof-pass"])
    );
    let multi_reason = by_id["proof-multi"]["reason"]
        .as_str()
        .context("multiple receipt reason missing")?;
    assert!(multi_reason.contains("proof-fail=head_failed join=request_identity"));
    assert!(multi_reason.contains("proof-pass=head_passed join=request_identity"));
    assert_eq!(by_id["proof-both"]["status"], "head_passed");
    assert!(
        by_id["proof-both"]["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("join=request_and_task_identity"))
    );
    Ok(())
}
