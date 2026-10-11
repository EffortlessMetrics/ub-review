//! Physical-child regressions at the runner → proof receipt → shadow ledger join.

use super::*;
use crate::task_ledger::{TaskEvent, TaskTerminalDisposition};
use anyhow::ensure;
use std::{thread, time::Duration};

#[derive(Clone, Copy)]
enum Stop {
    Interrupted,
    TimedOut,
}

#[test]
fn interrupted_live_child_cannot_become_successful_proof() -> Result<()> {
    stopped_child_receipt(Stop::Interrupted)
}

#[test]
fn timed_out_live_child_cannot_become_successful_proof() -> Result<()> {
    stopped_child_receipt(Stop::TimedOut)
}

fn stopped_child_receipt(stop: Stop) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let started = temp.path().join("started");
    let delayed = temp.path().join("delayed");
    let out = temp.path().join("out");
    let revision = RevisionRef {
        digest: "a".repeat(64),
        semantics: "candidate_head".to_owned(),
        reviewed_commit: "b".repeat(40),
    };
    let recorder = TaskLedgerRecorder::new(&revision, &Instant::now())?;
    let ledger = ProofTaskLedger::new(recorder.clone());
    let task = FocusedTestTask {
        id: "lifecycle-proof".to_owned(),
        file: "src/lib.rs".to_owned(),
        test_name: Some("child".to_owned()),
        mode: FocusedProofMode::HeadOnly,
        command_specs: None,
        timeout_sec: Some(1),
        required: true,
        requested_by: vec!["lifecycle-regression".to_owned()],
        request_ids: vec!["lifecycle-request".to_owned()],
    };
    let spec = ProofCommandSpec {
        argv: vec![
            std::env::current_exe()?.display().to_string(),
            "--exact".to_owned(),
            "lane_packets::tests::fake_delayed_marker_child".to_owned(),
            "--nocapture".to_owned(),
        ],
        env: BTreeMap::from([
            (
                "UB_REVIEW_WAIT_ERROR_STARTED".to_owned(),
                started.display().to_string(),
            ),
            (
                "UB_REVIEW_WAIT_ERROR_DELAYED".to_owned(),
                delayed.display().to_string(),
            ),
        ]),
    };
    let lease = ResourceLease {
        revision: None,
        schema: RESOURCE_LEASE_SCHEMA.to_owned(),
        id: "lease-lifecycle-proof".to_owned(),
        kind: "focused-test".to_owned(),
        consumer: task.id.clone(),
        status: "granted".to_owned(),
        reason: "physical-child lifecycle regression".to_owned(),
        cpu: 1,
        memory_mb: 512,
        disk_mb: 64,
        timeout_sec: 1,
        network: false,
        scratch: false,
        worktree: None,
        command: None,
    };
    let mut child_was_running = false;
    let mut cleanup_called = false;
    let mut cleanup = || {
        cleanup_called = true;
        // Proof-root cleanup must occur only after termination and reaping.
        ensure!(started.is_file());
        ensure!(!delayed.exists());
        Ok(())
    };
    let receipt = run_proof_command_receipt_for_task_with_cleanup(
        temp.path(),
        &out,
        &task,
        "head",
        &spec,
        1,
        &lease,
        Some(&ledger),
        ProofExecutionPhase::ModelRequest,
        Some(&mut cleanup),
        &mut |root, argv, env, timeout_sec, stdout_path, stderr_path, observe_process| {
            run_command_to_files_inner_with_wait(
                CommandRun {
                    root,
                    argv,
                    env,
                    ambient: &BTreeMap::new(),
                    timeout_sec,
                    stdout_path,
                    stderr_path,
                    quarantine_environment: false,
                    observe_process,
                },
                &mut |child, timeout| {
                    for _ in 0..100 {
                        if started.is_file() {
                            child_was_running = child.try_wait()?.is_none();
                            return match stop {
                                Stop::Interrupted => Err(io::Error::new(
                                    io::ErrorKind::Interrupted,
                                    "injected execution interruption",
                                )),
                                Stop::TimedOut => child.wait_timeout(timeout),
                            };
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "child start marker missing",
                    ))
                },
            )
        },
    )?;
    ensure!(
        child_was_running,
        "the regression must interrupt a live physical child"
    );
    ensure!(cleanup_called);
    let (status, disposition, timed_out) = match stop {
        Stop::Interrupted => ("skipped", TaskTerminalDisposition::Cancelled, false),
        Stop::TimedOut => ("timed_out", TaskTerminalDisposition::TimedOut, true),
    };
    ensure!(receipt.status == status);
    ensure!(receipt.exit_code.is_none());
    ensure!(receipt.timed_out == timed_out);
    if matches!(stop, Stop::Interrupted) {
        ensure!(receipt.reason.contains("injected execution interruption"));
    }
    // Check the public worker projection as well as the retained command row.
    ensure!(classify_worker_proof_result(&ProofKind::SanitizerWitness, &receipt) == status);
    let serialized = serde_json::to_value(&receipt)?;
    ensure!(serialized["status"] == status);
    let task_id = proof_command_task_id(&task.id, "head")?;
    let events = recorder
        .inputs()?
        .into_iter()
        .filter(|input| input.task_id == task_id)
        .map(|input| input.event)
        .collect::<Vec<_>>();
    ensure!(
        events
            .iter()
            .filter(|event| matches!(event, TaskEvent::RunStarted { .. }))
            .count()
            == 1
    );
    let terminal = events
        .iter()
        .position(|event| {
            matches!(event,
                TaskEvent::ProcessFinished { disposition: actual, .. } if *actual == disposition
            )
        })
        .context("physical interruption must retain its terminal disposition")?;
    let cleaned = events
        .iter()
        .position(|event| matches!(event, TaskEvent::CleanupFinished { .. }))
        .context("confirmed physical completion must permit cleanup")?;
    ensure!(terminal < cleaned);
    ensure!(!events.iter().any(|event| matches!(
        event,
        TaskEvent::SetupFailed { .. }
            | TaskEvent::ReceiptCreated { .. }
            | TaskEvent::ResourcesReleased { .. }
            | TaskEvent::ProcessFinished {
                disposition: TaskTerminalDisposition::Succeeded,
                ..
            }
    )));
    // Merely returning a command receipt does not publish proof or release its lease.
    let command_task =
        ProofCommandTask::focused_test(&task, "head", 1, ProofExecutionPhase::ModelRequest);
    ensure!(ledger.receipt_pending(&command_task)?);
    thread::sleep(Duration::from_millis(2300));
    ensure!(
        !delayed.exists(),
        "cancelled child survived long enough to publish proof"
    );
    Ok(())
}
