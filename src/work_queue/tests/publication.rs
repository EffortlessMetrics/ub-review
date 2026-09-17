use super::helpers::*;
use super::*;

#[test]
fn proof_receipt_writer_invalidates_commit_marker_before_replacing_receipts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(out, vec![proof_plan_task("proof-task-a")])?;
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    let prior_json = b"prior proof receipt json";
    let prior_ndjson = b"prior proof receipt ndjson\n";
    fs::write(review_dir.join("proof_receipts.json"), prior_json)?;
    fs::write(out.join("proof_receipts.ndjson"), prior_ndjson)?;
    fs::create_dir(out.join(TERMINAL_QUEUE_FILE))?;
    let receipts = vec![proof_receipt(
        "proof-receipt-a",
        &["req-a"],
        &["tests-oracle"],
    )];

    let error = write_proof_receipt_artifacts(out, &receipts, None)
        .err()
        .context("receipt publication unexpectedly ignored an unremovable commit marker")?;
    assert!(
        format!("{error:#}").contains("remove stale terminal queue commit marker"),
        "unexpected error: {error:#}"
    );
    assert_eq!(
        fs::read(review_dir.join("proof_receipts.json"))?,
        prior_json
    );
    assert_eq!(fs::read(out.join("proof_receipts.ndjson"))?, prior_ndjson);
    assert!(out.join(TERMINAL_QUEUE_FILE).is_dir());
    Ok(())
}

#[test]
fn terminal_publication_fails_closed_at_filesystem_boundaries() -> Result<()> {
    {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        remove_terminal_work_queue_artifacts(out)?;
        for name in [
            TERMINAL_QUEUE_FILE,
            TERMINAL_EVENTS_FILE,
            TERMINAL_QUEUE_TMP_FILE,
            TERMINAL_EVENTS_TMP_FILE,
        ] {
            fs::write(out.join(name), name)?;
        }
        remove_terminal_work_queue_artifacts(out)?;
        assert!(!out.join(TERMINAL_QUEUE_FILE).exists());
        assert!(!out.join(TERMINAL_EVENTS_FILE).exists());
        assert!(!out.join(TERMINAL_QUEUE_TMP_FILE).exists());
        assert!(!out.join(TERMINAL_EVENTS_TMP_FILE).exists());
    }

    {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::create_dir(out.join(TERMINAL_QUEUE_FILE))?;
        let error = remove_terminal_work_queue_artifacts(out)
            .err()
            .context("terminal artifact directory unexpectedly removed as a file")?;
        assert!(format!("{error:#}").contains("remove stale terminal queue artifact"));
    }

    {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        publish_terminal_work_queue_artifacts(out, b"queue", b"events\n")?;
        assert_eq!(fs::read(out.join(TERMINAL_QUEUE_FILE))?, b"queue");
        assert_eq!(fs::read(out.join(TERMINAL_EVENTS_FILE))?, b"events\n");
        assert!(!out.join(TERMINAL_QUEUE_TMP_FILE).exists());
        assert!(!out.join(TERMINAL_EVENTS_TMP_FILE).exists());
    }

    {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::create_dir(out.join(TERMINAL_EVENTS_FILE))?;
        let error = publish_terminal_work_queue_artifacts(out, b"queue", b"events\n")
            .err()
            .context("terminal event publication unexpectedly replaced a directory")?;
        assert!(format!("{error:#}").contains("publish terminal event artifact"));
        assert!(!out.join(TERMINAL_QUEUE_FILE).exists());
        assert!(!out.join(TERMINAL_QUEUE_TMP_FILE).exists());
        assert!(!out.join(TERMINAL_EVENTS_TMP_FILE).exists());
    }

    {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::create_dir(out.join(TERMINAL_QUEUE_FILE))?;
        let error = publish_terminal_work_queue_artifacts(out, b"queue", b"events\n")
            .err()
            .context("terminal queue publication unexpectedly replaced a directory")?;
        assert!(format!("{error:#}").contains("publish terminal queue artifact"));
        assert!(!out.join(TERMINAL_EVENTS_FILE).exists());
        assert!(!out.join(TERMINAL_QUEUE_TMP_FILE).exists());
        assert!(!out.join(TERMINAL_EVENTS_TMP_FILE).exists());
    }
    Ok(())
}

#[test]
fn proof_receipt_writer_replaces_terminal_marker_and_receipt_truth() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    write_plan(out, vec![proof_plan_task("proof-task-a")])?;
    write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;

    let first = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
    write_proof_receipt_artifacts(out, &[first], None)?;
    assert!(out.join(TERMINAL_QUEUE_FILE).is_file());

    let second = proof_receipt("proof-receipt-b", &["req-a"], &["tests-oracle"]);
    write_proof_receipt_artifacts(out, &[second], None)?;
    let receipts: Vec<ProofReceipt> =
        serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].id, "proof-receipt-b");

    let terminal: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
    assert_eq!(
        terminal["tasks"][0]["receipt_ids"],
        serde_json::json!(["proof-receipt-b"])
    );
    Ok(())
}

#[test]
fn new_plan_invalidates_every_terminal_artifact_before_publication() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    let prior_plan = write_plan(out, vec![proof_plan_task("prior-task")])?;
    write_terminal_work_queue_artifacts(out, &[])?;
    fs::write(out.join(TERMINAL_QUEUE_TMP_FILE), b"stale queue staging")?;
    fs::write(out.join(TERMINAL_EVENTS_TMP_FILE), b"stale event staging")?;

    let plan = crate::tests::test_plan(Vec::new());
    write_work_queue_artifacts(out, &plan, &[])?;
    for name in [
        TERMINAL_QUEUE_FILE,
        TERMINAL_EVENTS_FILE,
        TERMINAL_QUEUE_TMP_FILE,
        TERMINAL_EVENTS_TMP_FILE,
    ] {
        assert!(!out.join(name).exists(), "new plan retained {name}");
    }
    let current_plan = fs::read(out.join("work_queue_plan.json"))?;
    assert_ne!(current_plan, prior_plan);
    assert_eq!(fs::read(out.join("work_queue.json"))?, current_plan);
    assert_eq!(
        fs::read(out.join("work_events_plan.ndjson"))?,
        fs::read(out.join("work_events.ndjson"))?
    );

    write_terminal_work_queue_artifacts(out, &[])?;
    let terminal: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
    assert_eq!(terminal["source_plan_sha256"], sha256_hex(&current_plan));
    assert_eq!(terminal["tasks"], serde_json::json!([]));
    Ok(())
}

#[test]
fn new_plan_cleanup_failure_preserves_every_planner_artifact() -> Result<()> {
    for blocked in [
        TERMINAL_QUEUE_FILE,
        TERMINAL_EVENTS_FILE,
        TERMINAL_QUEUE_TMP_FILE,
        TERMINAL_EVENTS_TMP_FILE,
    ] {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let planner_files = [
            "work_queue.json",
            "work_events.ndjson",
            "work_queue_plan.json",
            "work_events_plan.ndjson",
        ];
        for name in planner_files {
            fs::write(out.join(name), b"previous planner bytes")?;
        }
        fs::create_dir(out.join(blocked))?;
        let plan = crate::tests::test_plan(Vec::new());
        let error = write_work_queue_artifacts(out, &plan, &[])
            .err()
            .with_context(|| format!("new plan ignored cleanup failure at {blocked}"))?;
        let diagnostic = format!("{error:#}");
        assert!(diagnostic.contains("remove stale terminal queue artifact"));
        assert!(diagnostic.contains(blocked));
        for name in planner_files {
            assert_eq!(fs::read(out.join(name))?, b"previous planner bytes");
        }
        assert!(out.join(blocked).is_dir());
    }
    Ok(())
}

#[test]
fn receipt_write_failure_invalidates_old_marker_before_terminal_rebuild() -> Result<()> {
    for blocked in ["review/proof_receipts.json", "proof_receipts.ndjson"] {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let plan_bytes = write_plan(out, vec![proof_plan_task("proof-task-a")])?;
        write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
        let first = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
        write_proof_receipt_artifacts(out, &[first], None)?;
        assert!(out.join(TERMINAL_QUEUE_FILE).is_file());
        let prior_ndjson = fs::read(out.join("proof_receipts.ndjson"))?;
        let blocked_path = out.join(blocked);
        fs::remove_file(&blocked_path)?;
        fs::create_dir(&blocked_path)?;
        let replacement = proof_receipt("proof-receipt-b", &["req-a"], &["tests-oracle"]);

        // Fail before terminal recomputation can remove the old marker again.
        // Both the first write and the second write need this generation fence.
        let error = write_proof_receipt_artifacts(out, std::slice::from_ref(&replacement), None)
            .err()
            .with_context(|| format!("receipt writer accepted directory at {blocked}"))?;
        assert!(error.downcast_ref::<std::io::Error>().is_some());
        assert!(blocked_path.is_dir());
        assert!(
            !out.join(TERMINAL_QUEUE_FILE).exists(),
            "failed replacement retained the previous terminal marker at {blocked}"
        );
        assert_eq!(fs::read(out.join("work_queue_plan.json"))?, plan_bytes);
        assert_eq!(fs::read(out.join("work_queue.json"))?, plan_bytes);
        for name in [TERMINAL_QUEUE_TMP_FILE, TERMINAL_EVENTS_TMP_FILE] {
            assert!(!out.join(name).exists(), "failed write left staging {name}");
        }
        if blocked == "review/proof_receipts.json" {
            assert_eq!(fs::read(out.join("proof_receipts.ndjson"))?, prior_ndjson);
        } else {
            let partial: Vec<ProofReceipt> =
                serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
            assert_eq!(partial.len(), 1);
            assert_eq!(partial[0].id, "proof-receipt-b");
        }
        // Existing terminal events alone are not a committed generation.
        // After the obstruction is removed, a normal retry must publish only
        // the replacement receipt, not recover the stale receipt identity.
        fs::remove_dir(&blocked_path)?;
        write_proof_receipt_artifacts(out, &[replacement], None)?;
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
        let tasks = terminal["tasks"]
            .as_array()
            .context("terminal tasks missing")?;
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["id"], "proof-task-a");
        assert_eq!(tasks[0]["receipt_ids"], serde_json::json!(["proof-receipt-b"]));
        assert_eq!(terminal["source_plan_sha256"], sha256_hex(&plan_bytes));
        let event_text = fs::read_to_string(out.join(TERMINAL_EVENTS_FILE))?;
        let events = event_text
            .lines()
            .map(serde_json::from_str)
            .collect::<serde_json::Result<Vec<serde_json::Value>>>()?;
        assert_eq!(events.len(), 1);
        assert_eq!(events[0]["task_id"], "proof-task-a");
        assert_eq!(events[0]["receipt_ids"], serde_json::json!(["proof-receipt-b"]));
        for name in [TERMINAL_QUEUE_TMP_FILE, TERMINAL_EVENTS_TMP_FILE] {
            assert!(!out.join(name).exists(), "retry left staging {name}");
        }
    }
    Ok(())
}
