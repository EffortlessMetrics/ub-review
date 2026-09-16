use super::helpers::*;
use super::*;

#[test]
fn nonterminal_and_unknown_proof_results_cannot_commit_terminal_output() -> Result<()> {
    for joined in [false, true] {
        for result in [
            "planned",
            "queued",
            "running",
            "future_result",
            " head_passed ",
        ] {
            let temp = tempfile::tempdir()?;
            let out = temp.path();
            let tasks = if joined {
                vec![proof_plan_task("proof-task-a")]
            } else {
                Vec::new()
            };
            write_plan(out, tasks)?;
            write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
            let mut receipt = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
            write_proof_receipt_artifacts(out, std::slice::from_ref(&receipt), None)?;
            assert!(out.join(TERMINAL_QUEUE_FILE).is_file());
            fs::write(out.join(TERMINAL_QUEUE_TMP_FILE), b"stale queue staging")?;
            fs::write(out.join(TERMINAL_EVENTS_TMP_FILE), b"stale event staging")?;

            receipt.result = result.to_owned();
            let error = write_proof_receipt_artifacts(out, &[receipt], None)
                .err()
                .with_context(|| format!("unsupported proof result {result} was published"))?;
            let diagnostic = format!("{error:#}");
            assert!(diagnostic.contains("unsupported terminal result"));
            assert!(diagnostic.contains("proof-receipt-a"));
            assert!(diagnostic.contains(result));
            for name in [
                TERMINAL_QUEUE_FILE,
                TERMINAL_EVENTS_FILE,
                TERMINAL_QUEUE_TMP_FILE,
                TERMINAL_EVENTS_TMP_FILE,
            ] {
                assert!(!out.join(name).exists(), "invalid result retained {name}");
            }
        }
    }
    Ok(())
}

#[test]
fn supported_broker_result_vocabulary_is_preserved_without_promoting_success() -> Result<()> {
    // This pins the projection vocabulary, not command-result verification.
    // Failure, setup failure, and nonexecution must retain distinct results.
    for result in [
        "head_passed",
        "head_failed",
        "discriminating",
        "non_discriminating",
        "base_patch_failed",
        "timed_out",
        "skipped_budget",
        "skipped_profile",
    ] {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        write_plan(out, vec![proof_plan_task("proof-task-a")])?;
        write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
        let mut receipt = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
        receipt.result = result.to_owned();
        write_proof_receipt_artifacts(out, &[receipt], None)?;
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
        assert_eq!(terminal["tasks"][0]["status"], result);
        assert_eq!(terminal["tasks"][0]["plan_status"], "planned");
        assert_eq!(
            terminal["tasks"][0]["receipt_ids"],
            serde_json::json!(["proof-receipt-a"])
        );
        let event_text = fs::read_to_string(out.join(TERMINAL_EVENTS_FILE))?;
        let event: serde_json::Value = serde_json::from_str(event_text.trim())?;
        assert_eq!(event["status"], result);
        assert_eq!(event["task_id"], "proof-task-a");
    }
    Ok(())
}
