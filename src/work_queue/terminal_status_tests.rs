use super::helpers::*;
use super::*;

fn completed_command(side: &str, status: &str) -> ProofCommandReceipt {
    ProofCommandReceipt {
        side: side.to_owned(),
        command: "cargo test --locked queue_terminal".to_owned(),
        env: BTreeMap::new(),
        status: status.to_owned(),
        exit_code: Some(0),
        timed_out: false,
        timeout_sec: 30,
        duration_ms: 7,
        stdout: String::new(),
        stderr: String::new(),
        reason: "command completed".to_owned(),
    }
}

fn skipped_unresolved_command(side: &str) -> ProofCommandReceipt {
    ProofCommandReceipt {
        side: side.to_owned(),
        command: String::new(),
        env: BTreeMap::new(),
        status: "skipped".to_owned(),
        exit_code: None,
        timed_out: false,
        timeout_sec: 0,
        duration_ms: 0,
        stdout: String::new(),
        stderr: String::new(),
        reason: "proof command blocked: resource lease is not granted".to_owned(),
    }
}

fn skipped_unresolved_receipt(id: &str, commands: Vec<ProofCommandReceipt>) -> ProofReceipt {
    let mut receipt = proof_receipt(id, &["req-a"], &["tests-oracle"]);
    receipt.kind = "source-route-probe".to_owned();
    receipt.result = "skipped_unresolved".to_owned();
    receipt.reason = "source-route-probe: skipped_unresolved; nightly preflight passed".to_owned();
    receipt.commands = commands;
    receipt
}

#[test]
fn skipped_unresolved_receipts_publish_for_supported_command_roles() -> Result<()> {
    // Worker lease refusal: completed setup preflight, skipped HEAD proof.
    let worker_receipt = || {
        skipped_unresolved_receipt(
            "proof-receipt-worker",
            vec![
                completed_command("nightly-preflight", "passed"),
                skipped_unresolved_command("head"),
            ],
        )
    };
    // Red/green partial: passed HEAD, skipped base-plus-tests proof side.
    let red_green_receipt = || {
        skipped_unresolved_receipt(
            "proof-receipt-red-green",
            vec![
                completed_command("head", "passed"),
                skipped_unresolved_command("base-plus-tests"),
            ],
        )
    };
    for receipt in [worker_receipt(), red_green_receipt()] {
        let receipt_id = receipt.id.clone();
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        write_plan(out, vec![proof_plan_task("proof-task-a")])?;
        write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
        write_proof_receipt_artifacts(out, &[receipt], None)?;
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
        assert_eq!(terminal["tasks"][0]["id"], "proof-task-a");
        assert_eq!(terminal["tasks"][0]["status"], "skipped_unresolved");
        assert_eq!(terminal["tasks"][0]["plan_status"], "planned");
        assert_eq!(
            terminal["tasks"][0]["reason"],
            format!("{receipt_id}=skipped_unresolved join=request_identity")
        );
        assert_eq!(
            terminal["tasks"][0]["request_ids"],
            serde_json::json!(["req-a"])
        );
        assert_eq!(
            terminal["tasks"][0]["receipt_ids"],
            serde_json::json!([receipt_id])
        );
        let event_text = fs::read_to_string(out.join(TERMINAL_EVENTS_FILE))?;
        let event: serde_json::Value = serde_json::from_str(event_text.trim())?;
        assert_eq!(event["status"], "skipped_unresolved");
        assert_eq!(event["receipt_ids"], terminal["tasks"][0]["receipt_ids"]);
    }
    // An unjoined receipt keeps its row keyed by receipt identity with the
    // exact producer result and reason preserved verbatim.
    for receipt in [worker_receipt(), red_green_receipt()] {
        let receipt_id = receipt.id.clone();
        let receipt_reason = receipt.reason.clone();
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        write_plan(out, Vec::new())?;
        write_proof_receipt_artifacts(out, &[receipt], None)?;
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
        assert_eq!(terminal["tasks"][0]["id"], receipt_id);
        assert_eq!(terminal["tasks"][0]["source"], "proof-receipt");
        assert_eq!(terminal["tasks"][0]["status"], "skipped_unresolved");
        assert_eq!(terminal["tasks"][0]["reason"], receipt_reason);
        assert_eq!(
            terminal["tasks"][0]["receipt_path"],
            format!("review/proof_receipts.json#{receipt_id}")
        );
    }
    Ok(())
}

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
fn unsupported_skipped_unresolved_command_roles_fail_closed() -> Result<()> {
    let mut forged_exit = skipped_unresolved_receipt(
        "proof-receipt-a",
        vec![
            completed_command("nightly-preflight", "passed"),
            skipped_unresolved_command("head"),
        ],
    );
    forged_exit.commands[1].exit_code = Some(0);
    let mut forged_timeout = skipped_unresolved_receipt(
        "proof-receipt-a",
        vec![
            completed_command("nightly-preflight", "passed"),
            skipped_unresolved_command("head"),
        ],
    );
    forged_timeout.commands[1].timed_out = true;
    let mut forged_duration = skipped_unresolved_receipt(
        "proof-receipt-a",
        vec![
            completed_command("nightly-preflight", "passed"),
            skipped_unresolved_command("head"),
        ],
    );
    forged_duration.commands[1].duration_ms = 5;
    let reversed_roles = skipped_unresolved_receipt(
        "proof-receipt-a",
        vec![
            skipped_unresolved_command("head"),
            completed_command("nightly-preflight", "passed"),
        ],
    );
    let mut timed_out_setup = skipped_unresolved_receipt(
        "proof-receipt-a",
        vec![
            completed_command("nightly-preflight", "passed"),
            skipped_unresolved_command("head"),
        ],
    );
    timed_out_setup.commands[0].status = "timed_out".to_owned();
    timed_out_setup.commands[0].timed_out = true;
    let skipped_setup = skipped_unresolved_receipt(
        "proof-receipt-a",
        vec![
            skipped_unresolved_command("nightly-preflight"),
            skipped_unresolved_command("head"),
        ],
    );
    let mut mismatched_setup_side = skipped_unresolved_receipt(
        "proof-receipt-a",
        vec![
            completed_command("head", "passed"),
            skipped_unresolved_command("base-plus-tests"),
        ],
    );
    mismatched_setup_side.commands[0].side = "focused-build".to_owned();
    let head_failed_red_green = skipped_unresolved_receipt(
        "proof-receipt-a",
        vec![
            completed_command("head", "failed"),
            skipped_unresolved_command("base-plus-tests"),
        ],
    );
    let single_skipped_command =
        skipped_unresolved_receipt("proof-receipt-a", vec![skipped_unresolved_command("head")]);
    let mut unknown_result = skipped_unresolved_receipt(
        "proof-receipt-a",
        vec![
            completed_command("nightly-preflight", "passed"),
            skipped_unresolved_command("head"),
        ],
    );
    unknown_result.result = "skipped_unresolved_typo".to_owned();
    let cases = [
        (forged_exit, "unsupported skipped_unresolved command roles"),
        (
            forged_timeout,
            "unsupported skipped_unresolved command roles",
        ),
        (
            forged_duration,
            "unsupported skipped_unresolved command roles",
        ),
        (
            reversed_roles,
            "unsupported skipped_unresolved command roles",
        ),
        (
            timed_out_setup,
            "unsupported skipped_unresolved command roles",
        ),
        (
            skipped_setup,
            "unsupported skipped_unresolved command roles",
        ),
        (
            mismatched_setup_side,
            "unsupported skipped_unresolved command roles",
        ),
        (
            head_failed_red_green,
            "unsupported skipped_unresolved command roles",
        ),
        (
            single_skipped_command,
            "unsupported skipped_unresolved command roles",
        ),
        (unknown_result, "unsupported terminal result"),
    ];
    for joined in [false, true] {
        for (receipt, diagnostic_fragment) in &cases {
            let temp = tempfile::tempdir()?;
            let out = temp.path();
            let tasks = if joined {
                vec![proof_plan_task("proof-task-a")]
            } else {
                Vec::new()
            };
            write_plan(out, tasks)?;
            write_proof_tasks(out, &[("proof-task-a", &["req-a"])])?;
            let valid = skipped_unresolved_receipt(
                "proof-receipt-seed",
                vec![
                    completed_command("nightly-preflight", "passed"),
                    skipped_unresolved_command("head"),
                ],
            );
            write_proof_receipt_artifacts(out, &[valid], None)?;
            assert!(out.join(TERMINAL_QUEUE_FILE).is_file());
            fs::write(out.join(TERMINAL_QUEUE_TMP_FILE), b"stale queue staging")?;
            fs::write(out.join(TERMINAL_EVENTS_TMP_FILE), b"stale event staging")?;

            let error = write_proof_receipt_artifacts(out, std::slice::from_ref(receipt), None)
                .err()
                .with_context(|| {
                    format!(
                        "unsupported skipped_unresolved shape {diagnostic_fragment} was published"
                    )
                })?;
            let diagnostic = format!("{error:#}");
            assert!(
                diagnostic.contains(diagnostic_fragment),
                "unexpected diagnostic: {diagnostic}"
            );
            assert!(diagnostic.contains("proof-receipt-a"));
            for name in [
                TERMINAL_QUEUE_FILE,
                TERMINAL_EVENTS_FILE,
                TERMINAL_QUEUE_TMP_FILE,
                TERMINAL_EVENTS_TMP_FILE,
            ] {
                assert!(!out.join(name).exists(), "invalid shape retained {name}");
            }
        }
    }
    Ok(())
}
