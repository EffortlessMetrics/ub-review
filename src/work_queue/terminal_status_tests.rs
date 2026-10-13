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

#[test]
fn unsupported_planner_status_cannot_become_terminal_truth() -> Result<()> {
    for kind in ["sensor", "focused-test", "focused-build"] {
        for plan_status in [
            "queued",
            "running",
            "future_status",
            "head_passed",
            " planned ",
        ] {
            for has_receipt in [false, true] {
                let temp = tempfile::tempdir()?;
                let out = temp.path();
                let mut task = if kind == "sensor" {
                    sensor_task("alpha", "planned")
                } else {
                    proof_plan_task("proof-task-a")
                };
                task["kind"] = serde_json::json!(kind);
                write_plan(out, vec![task.clone()])?;
                write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
                let receipts = if has_receipt && kind != "sensor" {
                    vec![proof_receipt("proof-task-a", &["req-a"], &["tests-oracle"])]
                } else {
                    Vec::new()
                };
                if has_receipt && kind == "sensor" {
                    write_sensor_receipt(out, "alpha", "ok")?;
                }
                write_proof_receipt_artifacts(out, &receipts, None)?;
                assert!(out.join(TERMINAL_QUEUE_FILE).is_file());
                fs::write(out.join(TERMINAL_QUEUE_TMP_FILE), b"stale queue staging")?;
                fs::write(out.join(TERMINAL_EVENTS_TMP_FILE), b"stale event staging")?;

                task["status"] = serde_json::json!(plan_status);
                write_plan(out, vec![task])?;
                let error = write_proof_receipt_artifacts(out, &receipts, None)
                    .err()
                    .context("unsupported planner status was published as terminal truth")?;
                let task_id = if kind == "sensor" {
                    "sensor-alpha"
                } else {
                    "proof-task-a"
                };
                assert_eq!(
                    error.to_string(),
                    format!(
                        "terminal queue {kind} task {task_id} has unsupported plan status {plan_status}"
                    )
                );
                for name in [
                    TERMINAL_QUEUE_FILE,
                    TERMINAL_EVENTS_FILE,
                    TERMINAL_QUEUE_TMP_FILE,
                    TERMINAL_EVENTS_TMP_FILE,
                ] {
                    assert!(!out.join(name).exists(), "invalid plan retained {name}");
                }
            }
        }
    }
    Ok(())
}

#[test]
fn producer_plan_statuses_preserve_nonexecution_without_receipts() -> Result<()> {
    for (kind, plan_status, terminal_status) in [
        ("sensor", "planned", "missing_receipt"),
        ("sensor", "skipped", "skipped"),
        ("focused-test", "planned", "not_executed"),
        ("focused-build", "planned", "not_executed"),
        ("focused-test", "deferred_by_budget", "deferred_by_budget"),
        ("focused-build", "deferred_by_budget", "deferred_by_budget"),
    ] {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let mut task = if kind == "sensor" {
            sensor_task("alpha", plan_status)
        } else {
            proof_plan_task("proof-task-a")
        };
        task["kind"] = serde_json::json!(kind);
        task["status"] = serde_json::json!(plan_status);
        write_plan(out, vec![task.clone()])?;
        write_proof_receipt_artifacts(out, &[], None)?;
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
        let rows = terminal["tasks"]
            .as_array()
            .context("terminal tasks missing")?;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["kind"], kind);
        assert_eq!(rows[0]["plan_status"], plan_status);
        assert_eq!(rows[0]["status"], terminal_status);
        assert_eq!(rows[0]["plan_task"], task);
        assert_eq!(rows[0]["receipt_ids"], serde_json::json!([]));
        assert_eq!(terminal["source_receipts"], serde_json::json!([]));
        let event_text = fs::read_to_string(out.join(TERMINAL_EVENTS_FILE))?;
        let event: serde_json::Value = serde_json::from_str(event_text.trim())?;
        assert_eq!(event["status"], terminal_status);
        assert_eq!(event["task_id"], rows[0]["id"]);
    }
    Ok(())
}

#[test]
fn skipped_unresolved_projection_preserves_nonexecution_and_exact_identities() -> Result<()> {
    for joined in [false, true] {
        for command_count in [0, 1, 2] {
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
            receipt.result = "skipped_unresolved".to_owned();
            receipt.reason = "typed executor could not resolve intent".to_owned();
            let mut command = receipt.commands[0].clone();
            command.status = "skipped".to_owned();
            command.exit_code = None;
            command.timed_out = false;
            command.duration_ms = 0;
            receipt.commands = vec![command; command_count];
            assert!(!proof_receipt_is_test_proof_result(&receipt));
            assert!(!proof_receipt_is_public_test_proof_result(&receipt));
            assert!(matches!(
                required_proof_receipt_class(&receipt),
                RequiredProofClass::Skipped
            ));
            write_proof_receipt_artifacts(out, std::slice::from_ref(&receipt), None)?;
            let queue: serde_json::Value =
                serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
            let rows = queue["tasks"].as_array().context("terminal rows absent")?;
            assert_eq!(rows.len(), 1);
            let row = &rows[0];
            assert_eq!(row["status"], "skipped_unresolved");
            assert_eq!(row["request_ids"], serde_json::json!(["req-a"]));
            assert_eq!(row["receipt_ids"], serde_json::json!(["proof-receipt-a"]));
            let reason = if joined {
                "proof-receipt-a=skipped_unresolved join=request_identity reason=typed executor could not resolve intent"
            } else {
                "typed executor could not resolve intent"
            };
            assert_eq!(row["reason"], reason);
            let text = fs::read_to_string(out.join(TERMINAL_EVENTS_FILE))?;
            let event: serde_json::Value = serde_json::from_str(text.trim())?;
            assert_eq!(event["status"], "skipped_unresolved");
            assert_eq!(event["reason"], reason);
            assert_eq!(event["task_id"], row["id"]);
            assert_eq!(event["request_ids"], row["request_ids"]);
            assert_eq!(event["receipt_ids"], row["receipt_ids"]);
        }
    }
    Ok(())
}

#[test]
fn skipped_unresolved_execution_contradictions_cannot_publish_terminal_truth() -> Result<()> {
    for joined in [false, true] {
        for contradiction in 0..7 {
            let temp = tempfile::tempdir()?;
            let out = temp.path();
            write_plan(
                out,
                if joined {
                    vec![proof_plan_task("proof-task-a")]
                } else {
                    Vec::new()
                },
            )?;
            write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
            let mut receipt = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
            write_proof_receipt_artifacts(out, std::slice::from_ref(&receipt), None)?;
            receipt.result = "skipped_unresolved".to_owned();
            receipt.commands[0].status = "skipped".to_owned();
            receipt.commands[0].exit_code = None;
            receipt.commands[0].timed_out = false;
            receipt.commands[0].duration_ms = 0;
            receipt.commands.push(receipt.commands[0].clone());
            let command = &mut receipt.commands[1];
            command.status = "skipped".to_owned();
            command.exit_code = None;
            command.timed_out = false;
            command.duration_ms = 0;
            match contradiction {
                0 => command.status = "passed".to_owned(),
                1 => command.status = "failed".to_owned(),
                2 => command.exit_code = Some(0),
                3 => command.timed_out = true,
                4 => command.exit_code = Some(1),
                5 => command.status = "timed_out".to_owned(),
                _ => command.duration_ms = 1,
            }
            let error = write_proof_receipt_artifacts(out, &[receipt], None)
                .err()
                .context("unresolved execution contradiction was accepted")?;
            assert!(
                error
                    .to_string()
                    .contains("contradictory execution metadata")
            );
            assert!(!out.join(TERMINAL_QUEUE_FILE).exists());
            assert!(!out.join(TERMINAL_EVENTS_FILE).exists());
        }
    }
    Ok(())
}

#[test]
fn executed_failure_and_timeout_remain_terminal_failures() -> Result<()> {
    for (result, timed_out) in [("head_failed", false), ("timed_out", true)] {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        write_plan(out, vec![proof_plan_task("proof-task-a")])?;
        write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
        let mut receipt = proof_receipt("proof-receipt-a", &["req-a"], &["tests-oracle"]);
        receipt.result = result.to_owned();
        receipt.commands[0].status = if timed_out { "timed_out" } else { "failed" }.to_owned();
        receipt.commands[0].exit_code = Some(1);
        receipt.commands[0].timed_out = timed_out;
        assert!(matches!(
            required_proof_receipt_class(&receipt),
            RequiredProofClass::Failed
        ));
        write_proof_receipt_artifacts(out, &[receipt], None)?;
        let queue: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
        assert_eq!(queue["tasks"][0]["status"], result);
        let text = fs::read_to_string(out.join(TERMINAL_EVENTS_FILE))?;
        let event: serde_json::Value = serde_json::from_str(text.trim())?;
        assert_eq!(event["status"], result);
    }
    Ok(())
}
