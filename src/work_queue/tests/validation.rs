use super::helpers::*;
use super::*;

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
fn terminal_projection_rejects_malformed_proof_receipts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(out, vec![proof_plan_task("proof-task-a")])?;
    write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;

    let mut receipt = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
    receipt.id.clear();
    let error = write_terminal_work_queue_artifacts(out, &[receipt])
        .err()
        .context("empty proof receipt identity unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("empty identity"));

    let mut receipt = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
    receipt.result.clear();
    let error = write_terminal_work_queue_artifacts(out, &[receipt])
        .err()
        .context("empty proof receipt result unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("empty terminal result"));

    let mut receipt = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
    receipt.request_ids = vec![String::new()];
    let error = write_terminal_work_queue_artifacts(out, &[receipt])
        .err()
        .context("empty proof request identity unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("empty request identity"));

    let duplicate = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
    let error = write_terminal_work_queue_artifacts(out, &[duplicate.clone(), duplicate])
        .err()
        .context("duplicate proof receipt identity unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("duplicate proof receipt identity"));
    Ok(())
}

#[test]
fn terminal_projection_validates_sensor_receipts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(out, vec![sensor_task("alpha", "planned")])?;

    write_sensor_receipt_value(
        out,
        "alpha",
        &serde_json::json!({"sensor": "beta", "status": "ok"}),
    )?;
    let error = write_terminal_work_queue_artifacts(out, &[])
        .err()
        .context("mismatched sensor receipt identity unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("identity does not match"));

    write_sensor_receipt_value(out, "alpha", &serde_json::json!({"sensor": "alpha"}))?;
    let error = write_terminal_work_queue_artifacts(out, &[])
        .err()
        .context("missing sensor receipt status unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("no string status"));

    write_sensor_receipt_value(
        out,
        "alpha",
        &serde_json::json!({"sensor": "alpha", "status": "queued"}),
    )?;
    let error = write_terminal_work_queue_artifacts(out, &[])
        .err()
        .context("unsupported sensor receipt status unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("unsupported terminal status queued"));

    let path = out
        .join("sensors")
        .join("alpha")
        .join("ub-review-sensor-status.json");
    fs::write(path, b"{not-json")?;
    let error = write_terminal_work_queue_artifacts(out, &[])
        .err()
        .context("malformed sensor receipt unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("parse sensor receipt"));
    Ok(())
}

#[test]
fn terminal_projection_validates_proof_task_catalog() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(out, vec![proof_plan_task("proof-task-a")])?;

    write_terminal_work_queue_artifacts(out, &[])?;
    let terminal: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
    assert_eq!(terminal["tasks"][0]["status"], "not_executed");

    for (row, expected) in [
        ("{", "parse proof_tasks.ndjson line 1"),
        ("[]", "proof task row is not an object"),
        (
            r#"{"id":"","request_ids":[]}"#,
            "terminal queue object has no nonempty id",
        ),
        (
            r#"{"id":"proof-task-a","request_ids":"req-a"}"#,
            "terminal queue object has no request_ids array",
        ),
        (
            r#"{"id":"proof-task-a","request_ids":["req-a",7]}"#,
            "request_ids contains a non-string identity",
        ),
    ] {
        fs::write(out.join("proof_tasks.ndjson"), format!("{row}\n"))?;
        let error = write_terminal_work_queue_artifacts(out, &[])
            .err()
            .with_context(|| format!("invalid proof task row unexpectedly succeeded: {row}"))?;
        assert!(
            format!("{error:#}").contains(expected),
            "unexpected error for {row}: {error:#}"
        );
    }

    fs::write(
        out.join("proof_tasks.ndjson"),
        concat!(
            "{\"id\":\"proof-task-a\",\"request_ids\":[]}\n",
            "{\"id\":\"proof-task-a\",\"request_ids\":[]}\n"
        ),
    )?;
    let error = write_terminal_work_queue_artifacts(out, &[])
        .err()
        .context("duplicate proof task catalog identity unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("duplicate proof task identity"));

    fs::remove_file(out.join("proof_tasks.ndjson"))?;
    fs::create_dir(out.join("proof_tasks.ndjson"))?;
    let error = write_terminal_work_queue_artifacts(out, &[])
        .err()
        .context("proof task catalog directory unexpectedly succeeded")?;
    assert!(format!("{error:#}").contains("read proof tasks"));
    Ok(())
}

#[test]
fn unsupported_proof_schema_cannot_publish_a_terminal_queue() -> Result<()> {
    for planned in [false, true] {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let tasks = if planned {
            vec![proof_plan_task("proof-task-a")]
        } else {
            Vec::new()
        };
        write_plan(out, tasks)?;
        write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
        let receipt = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
        write_proof_receipt_artifacts(out, std::slice::from_ref(&receipt), None)?;
        assert!(out.join(TERMINAL_QUEUE_FILE).is_file());
        fs::write(out.join(TERMINAL_QUEUE_TMP_FILE), b"stale queue staging")?;
        fs::write(out.join(TERMINAL_EVENTS_TMP_FILE), b"stale event staging")?;

        let mut invalid = receipt;
        invalid.schema = "ub-review.proof_receipt.invalid".to_owned();
        let error = write_proof_receipt_artifacts(out, &[invalid], None)
            .err()
            .context("unsupported proof schema unexpectedly published terminal truth")?;
        assert!(format!("{error:#}").contains("has unsupported schema"));
        for name in [
            TERMINAL_QUEUE_FILE,
            TERMINAL_EVENTS_FILE,
            TERMINAL_QUEUE_TMP_FILE,
            TERMINAL_EVENTS_TMP_FILE,
        ] {
            assert!(!out.join(name).exists(), "invalid schema retained {name}");
        }
    }
    Ok(())
}

#[test]
fn unsupported_planner_kinds_never_fall_through_to_proof_projection() -> Result<()> {
    for kind in ["focused-head", "unknown-task-kind"] {
        for has_receipt in [false, true] {
            let temp = tempfile::tempdir()?;
            let out = temp.path();
            write_plan(out, vec![proof_plan_task("proof-task-a")])?;
            write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
            let receipts = if has_receipt {
                vec![proof_receipt("proof-task-a", &["req-a"], &["tests-oracle"])]
            } else {
                Vec::new()
            };
            write_terminal_work_queue_artifacts(out, &receipts)?;
            assert!(out.join(TERMINAL_QUEUE_FILE).is_file());

            let mut invalid = proof_plan_task("proof-task-a");
            invalid["kind"] = serde_json::json!(kind);
            write_plan(out, vec![invalid])?;
            let error = write_terminal_work_queue_artifacts(out, &receipts)
                .err()
                .with_context(|| format!("unsupported planner kind {kind} was accepted"))?;
            let diagnostic = format!("{error:#}");
            assert!(diagnostic.contains("plan task has unsupported kind"));
            assert!(diagnostic.contains(kind));
            assert!(!out.join(TERMINAL_QUEUE_FILE).exists());
            assert!(!out.join(TERMINAL_EVENTS_FILE).exists());
            assert!(!out.join(TERMINAL_QUEUE_TMP_FILE).exists());
            assert!(!out.join(TERMINAL_EVENTS_TMP_FILE).exists());
        }
    }
    Ok(())
}

#[test]
fn supported_planner_proof_kinds_retain_unexecuted_truth() -> Result<()> {
    for kind in ["focused-test", "focused-build"] {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let mut task = proof_plan_task("proof-task-a");
        task["kind"] = serde_json::json!(kind);
        write_plan(out, vec![task])?;
        write_terminal_work_queue_artifacts(out, &[])?;
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
        assert_eq!(terminal["tasks"][0]["kind"], kind);
        assert_eq!(terminal["tasks"][0]["status"], "not_executed");
    }
    Ok(())
}
