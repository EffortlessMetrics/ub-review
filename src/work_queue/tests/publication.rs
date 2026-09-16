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
