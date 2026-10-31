//! Physical-child regressions at the runner → proof receipt → shadow ledger join.

use super::*;
use crate::task_ledger::{TaskEvent, TaskTerminalDisposition};
use anyhow::ensure;
use std::{thread, time::Duration};

#[derive(Clone, Copy)]
enum Stop {
    Interrupted,
    TimedOut,
    MissingMarker,
}

#[test]
fn interrupted_live_child_cannot_become_successful_proof() -> Result<()> {
    stopped_child_receipt(Stop::Interrupted)
}

#[test]
fn timed_out_live_child_cannot_become_successful_proof() -> Result<()> {
    stopped_child_receipt(Stop::TimedOut)
}

#[test]
fn missing_start_marker_cleans_up_live_child_and_cannot_publish_proof() -> Result<()> {
    stopped_child_receipt(Stop::MissingMarker)
}

fn stopped_child_receipt(stop: Stop) -> Result<()> {
    stopped_child_receipt_after_start_delay(stop, 0)
}

fn stopped_child_receipt_after_start_delay(stop: Stop, start_delay_ms: u64) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let started = temp.path().join("started");
    let missing = temp.path().join("never-created");
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
            "lane_packets::lifecycle_receipt_tests::startup_marker_child".to_owned(),
            "--nocapture".to_owned(),
        ],
        env: BTreeMap::from([
            (
                "UB_LIFECYCLE_START_DELAY_MS".to_owned(),
                start_delay_ms.to_string(),
            ),
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
    #[cfg(target_os = "linux")]
    let child_pid = std::cell::Cell::<Option<u32>>::new(None);
    let mut cleanup_called = false;
    let mut stop_action_called = false;
    let mut cleanup = || {
        cleanup_called = true;
        // Proof-root cleanup must occur only after termination and reaping.
        ensure!(started.is_file());
        ensure!(!delayed.exists());
        #[cfg(target_os = "linux")]
        {
            let pid = child_pid
                .get()
                .context("cleanup must observe the child PID")?;
            ensure!(
                !Path::new("/proc").join(pid.to_string()).exists(),
                "cleanup began before child {pid} was reaped"
            );
        }
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
                    if matches!(stop, Stop::MissingMarker) {
                        // Observe a real running child before testing the separate missing marker.
                        wait_for_start_marker(&started, Duration::from_secs(5))?;
                        child_was_running = child.try_wait()?.is_none();
                        #[cfg(target_os = "linux")]
                        {
                            child_pid.set(Some(child.id()));
                        }
                    }
                    let (marker, budget) = if matches!(stop, Stop::MissingMarker) {
                        (&missing, Duration::from_millis(50))
                    } else {
                        (&started, Duration::from_secs(5))
                    };
                    wait_for_start_marker(marker, budget)?;
                    {
                        child_was_running = child.try_wait()?.is_none();
                        #[cfg(target_os = "linux")]
                        {
                            if !Path::new("/proc").join(child.id().to_string()).is_dir() {
                                return Err(io::Error::other(
                                    "live child is not visible in procfs",
                                ));
                            }
                            child_pid.set(Some(child.id()));
                        }
                        stop_action_called = true;
                        match stop {
                            Stop::Interrupted => Err(io::Error::new(
                                io::ErrorKind::Interrupted,
                                "injected execution interruption",
                            )),
                            Stop::TimedOut => child.wait_timeout(timeout),
                            Stop::MissingMarker => Err(io::Error::other("unexpected stop action")),
                        }
                    }
                },
            )
        },
    )?;
    ensure!(
        child_was_running,
        "the regression must interrupt a live physical child"
    );
    ensure!(cleanup_called);
    ensure!(stop_action_called == !matches!(stop, Stop::MissingMarker));
    // A killed child can remain a zombie if the runner omits its reap. Marker
    // absence alone cannot detect that bug. Linux procfs retains the PID until
    // the parent collects its exit status; check before creating other children.
    #[cfg(target_os = "linux")]
    {
        let pid = child_pid.get().context("live child PID was not observed")?;
        ensure!(
            !Path::new("/proc").join(pid.to_string()).exists(),
            "proof runner returned before reaping child {pid}"
        );
    }
    let (status, disposition, timed_out) = match stop {
        Stop::Interrupted => ("skipped", TaskTerminalDisposition::Cancelled, false),
        Stop::TimedOut => ("timed_out", TaskTerminalDisposition::TimedOut, true),
        Stop::MissingMarker => ("skipped", TaskTerminalDisposition::Cancelled, false),
    };
    ensure!(receipt.status == status);
    ensure!(receipt.exit_code.is_none());
    ensure!(receipt.timed_out == timed_out);
    if matches!(stop, Stop::Interrupted) {
        ensure!(receipt.reason.contains("injected execution interruption"));
    }
    if matches!(stop, Stop::MissingMarker) {
        ensure!(receipt.reason.contains("child start marker missing"));
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

fn wait_for_start_marker(started: &Path, budget: Duration) -> io::Result<()> {
    let deadline = Instant::now() + budget;
    loop {
        if started.is_file() {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "child start marker missing",
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn delayed_start_still_checks_live_child_cleanup_and_receipt() -> Result<()> {
    stopped_child_receipt_after_start_delay(Stop::Interrupted, 1500)
}

#[test]
fn missing_start_marker_is_bounded() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let before = Instant::now();
    let error = wait_for_start_marker(&temp.path().join("missing"), Duration::from_millis(50))
        .err()
        .context("missing marker must time out")?;
    ensure!(error.kind() == io::ErrorKind::TimedOut);
    ensure!(before.elapsed() >= Duration::from_millis(50));
    ensure!(before.elapsed() < Duration::from_secs(2));
    Ok(())
}

#[test]
fn startup_marker_child() -> Result<()> {
    let Some(started) = std::env::var_os("UB_REVIEW_WAIT_ERROR_STARTED") else {
        return Ok(());
    };
    let delayed =
        std::env::var_os("UB_REVIEW_WAIT_ERROR_DELAYED").context("delayed marker missing")?;
    let delay: u64 = std::env::var("UB_LIFECYCLE_START_DELAY_MS")?.parse()?;
    thread::sleep(Duration::from_millis(delay));
    fs::write(started, b"started")?;
    thread::sleep(Duration::from_secs(2));
    fs::write(delayed, b"still running")?;
    Ok(())
}
