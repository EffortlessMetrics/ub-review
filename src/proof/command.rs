//! Shared proof command receipt construction for broker-executed checks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::task_ledger::{TaskNonExecutionDisposition, TaskTerminalDisposition};
use crate::test_parse::command_display_with_env;
use crate::*;

struct ProofCommandPaths {
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    stdout_rel: String,
    stderr_rel: String,
}

const PROOF_COMMAND_STREAM_MAX_BYTES: usize = 256 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct ProofCommandSpec {
    pub(crate) argv: Vec<String>,
    pub(crate) env: BTreeMap<String, String>,
}

pub(crate) struct ProofCommandInvocation<'a, 'cleanup> {
    pub(crate) command_root: &'a Path,
    pub(crate) out: &'a Path,
    pub(crate) receipt_id: &'a str,
    pub(crate) side: &'a str,
    pub(crate) spec: &'a ProofCommandSpec,
    pub(crate) timeout_sec: u64,
    pub(crate) lease: &'a ResourceLease,
    pub(crate) task_ledger: Option<&'a ProofTaskLedger>,
    pub(crate) task: Option<&'a ProofCommandTask>,
    pub(crate) cleanup: Option<&'cleanup mut dyn FnMut() -> Result<()>>,
}

#[derive(Default)]
pub(crate) struct ProofBrokerResult {
    pub(crate) proof_receipts: Vec<ProofReceipt>,
    pub(crate) resource_leases: Vec<ResourceLease>,
}

pub(crate) fn run_proof_command_receipt<F>(
    invocation: ProofCommandInvocation<'_, '_>,
    runner: &mut F,
) -> Result<ProofCommandReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
        &mut dyn FnMut(CommandProcessObservation),
    ) -> Result<CommandStatus>,
{
    let ProofCommandInvocation {
        command_root,
        out,
        receipt_id,
        side,
        spec,
        timeout_sec,
        lease,
        task_ledger,
        task,
        cleanup,
    } = invocation;
    let ledger_task = match (task_ledger, task) {
        (Some(ledger), Some(task)) => Some((ledger, task)),
        (None, None) => None,
        _ => bail!("proof task-ledger invocation must provide both ledger and task metadata"),
    };
    if lease.status != "granted" || lease.consumer != receipt_id {
        let reason = if lease.status != "granted" {
            format!(
                "proof command blocked: resource lease `{}` status `{}` is not granted",
                lease.id, lease.status
            )
        } else {
            format!(
                "proof command blocked: resource lease `{}` is assigned to `{}`, not `{}`",
                lease.id, lease.consumer, receipt_id
            )
        };
        if let Some((ledger, task)) = ledger_task {
            let disposition = if lease.status == "exhausted" {
                TaskNonExecutionDisposition::BudgetDeferred
            } else {
                TaskNonExecutionDisposition::Refused
            };
            ledger.decline_command(task, disposition, &reason)?;
        }
        return skipped_proof_command_receipt_for_id(
            out, receipt_id, side, spec, "skipped", reason,
        );
    }

    if let Some((ledger, task)) = ledger_task {
        ledger.begin_command(task, lease)?;
    }
    let paths = match proof_command_paths(out, receipt_id, side) {
        Ok(paths) => paths,
        Err(path_error) => {
            let setup_result = match ledger_task {
                Some((ledger, task)) => ledger.setup_failed(task),
                None => Ok(()),
            };
            let cleanup_result = match cleanup {
                Some(cleanup) => cleanup(),
                None => Ok(()),
            };
            return match (setup_result, cleanup_result) {
                (Ok(()), Ok(())) => Err(path_error),
                (Err(setup_error), Ok(())) => Err(setup_error).context(format!(
                    "record proof setup failure after receipt path error: {path_error:#}"
                )),
                (Ok(()), Err(cleanup_error)) => Err(cleanup_error).context(format!(
                    "cleanup proof command after receipt path error: {path_error:#}"
                )),
                (Err(setup_error), Err(cleanup_error)) => Err(cleanup_error).context(format!(
                    "cleanup proof command after receipt path error: {path_error:#}; proof setup reconciliation also failed: {setup_error:#}"
                )),
            };
        }
    };
    let command = command_display_with_env(&spec.env, &spec.argv);
    let mut process_spawned = false;
    let mut completion_unconfirmed = false;
    let mut observation_error = None;
    let mut observe_process = |observation| match observation {
        CommandProcessObservation::Spawned => {
            process_spawned = true;
            if let Some((ledger, task)) = ledger_task
                && observation_error.is_none()
                && let Err(error) = ledger.run_started(task)
            {
                observation_error = Some(error);
            }
        }
        CommandProcessObservation::CompletionUnconfirmed => {
            completion_unconfirmed = true;
        }
    };
    let status = runner(
        command_root,
        &spec.argv,
        &spec.env,
        timeout_sec,
        &paths.stdout_path,
        &paths.stderr_path,
        &mut observe_process,
    );
    if let Some(error) = observation_error {
        return Err(error).context("record proof process observation");
    }
    if completion_unconfirmed {
        return match status {
            Err(error) => Err(error).context("proof child completion remains unconfirmed"),
            Ok(_) => bail!("proof child completion remains unconfirmed"),
        };
    }
    let disposition = match &status {
        Ok(status) if status.timed_out => TaskTerminalDisposition::TimedOut,
        Ok(status) if status.success => TaskTerminalDisposition::Succeeded,
        Ok(_) => TaskTerminalDisposition::DeterministicFailure,
        Err(_) => TaskTerminalDisposition::Cancelled,
    };
    if let Some((ledger, task)) = ledger_task {
        if process_spawned {
            ledger.process_finished(task, disposition)?;
        } else {
            ledger.setup_failed(task)?;
        }
    }
    let stream_result = bound_proof_command_streams(&paths);
    let cleanup_result = match cleanup {
        Some(cleanup) => cleanup(),
        None => Ok(()),
    };
    if process_spawned
        && stream_result.is_ok()
        && cleanup_result.is_ok()
        && let Some((ledger, task)) = ledger_task
    {
        ledger.cleanup_finished(task)?;
    }
    match (stream_result, cleanup_result) {
        (Ok(()), Ok(())) => {}
        (Err(stream_error), Ok(())) => return Err(stream_error),
        (Ok(()), Err(cleanup_error)) => return Err(cleanup_error),
        (Err(stream_error), Err(cleanup_error)) => {
            return Err(cleanup_error).context(format!(
                "proof command stream cleanup also failed: {stream_error:#}"
            ));
        }
    }
    let (command_status, reason, exit_code, timed_out, duration_ms) = match status {
        Ok(status) if status.timed_out => (
            "timed_out".to_owned(),
            status.reason,
            status.exit_code,
            true,
            status.duration_ms,
        ),
        Ok(status) if status.success => (
            "passed".to_owned(),
            status.reason,
            status.exit_code,
            false,
            status.duration_ms,
        ),
        Ok(status) => (
            "failed".to_owned(),
            status.reason,
            status.exit_code,
            false,
            status.duration_ms,
        ),
        Err(error) => (
            "skipped".to_owned(),
            format!("focused proof command unavailable: {error:#}"),
            None,
            false,
            0,
        ),
    };
    Ok(ProofCommandReceipt {
        side: side.to_owned(),
        command,
        env: spec.env.clone(),
        status: command_status,
        exit_code,
        timed_out,
        timeout_sec,
        duration_ms,
        stdout: paths.stdout_rel,
        stderr: paths.stderr_rel,
        reason,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "task, side, timeout, and lease are the proof-command execution contract; the helper centralizes invocation construction in the command module"
)]
pub(crate) fn run_proof_command_receipt_for_task<F>(
    command_root: &Path,
    out: &Path,
    task: &FocusedTestTask,
    side: &str,
    spec: &ProofCommandSpec,
    timeout_sec: u64,
    lease: &ResourceLease,
    task_ledger: Option<&ProofTaskLedger>,
    execution_phase: ProofExecutionPhase,
    runner: &mut F,
) -> Result<ProofCommandReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
        &mut dyn FnMut(CommandProcessObservation),
    ) -> Result<CommandStatus>,
{
    run_proof_command_receipt_for_task_with_cleanup(
        command_root,
        out,
        task,
        side,
        spec,
        timeout_sec,
        lease,
        task_ledger,
        execution_phase,
        None,
        runner,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "base-plus-tests adds one physical worktree cleanup boundary to the shared proof-command execution contract"
)]
pub(crate) fn run_proof_command_receipt_for_task_with_cleanup<F>(
    command_root: &Path,
    out: &Path,
    task: &FocusedTestTask,
    side: &str,
    spec: &ProofCommandSpec,
    timeout_sec: u64,
    lease: &ResourceLease,
    task_ledger: Option<&ProofTaskLedger>,
    execution_phase: ProofExecutionPhase,
    cleanup: Option<&mut dyn FnMut() -> Result<()>>,
    runner: &mut F,
) -> Result<ProofCommandReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
        &mut dyn FnMut(CommandProcessObservation),
    ) -> Result<CommandStatus>,
{
    let command_task = ProofCommandTask::focused_test(task, side, timeout_sec, execution_phase);
    run_proof_command_receipt(
        ProofCommandInvocation {
            command_root,
            out,
            receipt_id: &task.id,
            side,
            spec,
            timeout_sec,
            lease,
            task_ledger,
            task: task_ledger.map(|_| &command_task),
            cleanup,
        },
        runner,
    )
}

fn bound_proof_command_streams(paths: &ProofCommandPaths) -> Result<()> {
    bound_proof_command_stream(&paths.stdout_path)?;
    bound_proof_command_stream(&paths.stderr_path)?;
    Ok(())
}

fn bound_proof_command_stream(path: &Path) -> Result<()> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    if bytes.len() <= PROOF_COMMAND_STREAM_MAX_BYTES {
        return Ok(());
    }
    let marker = format!(
        "[ub-review truncated proof command stream: capped at {cap} bytes from {total}]\n",
        cap = PROOF_COMMAND_STREAM_MAX_BYTES,
        total = bytes.len()
    );
    let tail_budget = PROOF_COMMAND_STREAM_MAX_BYTES.saturating_sub(marker.len());
    let tail_start = bytes.len().saturating_sub(tail_budget);
    let mut bounded = Vec::with_capacity(PROOF_COMMAND_STREAM_MAX_BYTES);
    bounded.extend_from_slice(marker.as_bytes());
    bounded.extend_from_slice(&bytes[tail_start..]);
    fs::write(path, bounded).with_context(|| format!("truncate {}", path.display()))?;
    Ok(())
}

pub(crate) fn skipped_proof_command_receipt(
    out: &Path,
    task: &FocusedTestTask,
    side: &str,
    spec: &ProofCommandSpec,
    status: &str,
    reason: String,
) -> Result<ProofCommandReceipt> {
    skipped_proof_command_receipt_for_id(out, &task.id, side, spec, status, reason)
}

fn skipped_proof_command_receipt_for_id(
    out: &Path,
    receipt_id: &str,
    side: &str,
    spec: &ProofCommandSpec,
    status: &str,
    reason: String,
) -> Result<ProofCommandReceipt> {
    let paths = proof_command_paths(out, receipt_id, side)?;
    Ok(ProofCommandReceipt {
        side: side.to_owned(),
        command: command_display_with_env(&spec.env, &spec.argv),
        env: spec.env.clone(),
        status: status.to_owned(),
        exit_code: None,
        timed_out: false,
        timeout_sec: 0,
        duration_ms: 0,
        stdout: paths.stdout_rel,
        stderr: paths.stderr_rel,
        reason,
    })
}

pub(crate) fn skipped_focused_proof_receipt(
    out: &Path,
    diff: &DiffContext,
    task: &FocusedTestTask,
    result: &str,
    reason: &str,
) -> Result<ProofReceipt> {
    let spec = proof_task_command_spec(task, "head");
    let command =
        skipped_proof_command_receipt(out, task, "head", &spec, "skipped", reason.to_owned())?;
    Ok(focused_receipt(
        diff,
        task,
        vec![command],
        result.to_owned(),
        reason.to_owned(),
    ))
}

pub(crate) fn skipped_focused_build_receipt(
    out: &Path,
    diff: &DiffContext,
    task: &FocusedBuildTask,
    result: &str,
    reason: &str,
) -> Result<ProofReceipt> {
    let spec = focused_build_command_spec_for_task(task);
    let command = skipped_proof_command_receipt_for_id(
        out,
        &task.id,
        "head",
        &spec,
        "skipped",
        reason.to_owned(),
    )?;
    Ok(focused_build_receipt(
        diff,
        task,
        vec![command],
        result.to_owned(),
        reason.to_owned(),
    ))
}

fn focused_receipt(
    diff: &DiffContext,
    task: &FocusedTestTask,
    commands: Vec<ProofCommandReceipt>,
    result: String,
    reason: String,
) -> ProofReceipt {
    match task.mode {
        FocusedProofMode::HeadOnly => focused_head_receipt(diff, task, commands, result, reason),
        FocusedProofMode::RedGreen => {
            focused_red_green_receipt(diff, task, commands, result, reason)
        }
    }
}

pub(crate) fn focused_build_receipt(
    diff: &DiffContext,
    task: &FocusedBuildTask,
    commands: Vec<ProofCommandReceipt>,
    result: String,
    reason: String,
) -> ProofReceipt {
    ProofReceipt {
        revision: None,
        schema: PROOF_RECEIPT_SCHEMA.to_owned(),
        id: task.id.clone(),
        kind: "focused-build".to_owned(),
        base: diff.base.clone(),
        head: diff.head.clone(),
        test_patch_mode: "head-only".to_owned(),
        requested_by: task.requested_by.clone(),
        request_ids: task.request_ids.clone(),
        commands,
        result,
        reason,
    }
}

pub(crate) fn focused_head_receipt(
    diff: &DiffContext,
    task: &FocusedTestTask,
    commands: Vec<ProofCommandReceipt>,
    result: String,
    reason: String,
) -> ProofReceipt {
    ProofReceipt {
        revision: None,
        schema: PROOF_RECEIPT_SCHEMA.to_owned(),
        id: task.id.clone(),
        kind: "focused-head".to_owned(),
        base: diff.base.clone(),
        head: diff.head.clone(),
        test_patch_mode: "head-only".to_owned(),
        requested_by: task.requested_by.clone(),
        request_ids: task.request_ids.clone(),
        commands,
        result,
        reason,
    }
}

pub(crate) fn focused_red_green_receipt(
    diff: &DiffContext,
    task: &FocusedTestTask,
    commands: Vec<ProofCommandReceipt>,
    result: String,
    reason: String,
) -> ProofReceipt {
    ProofReceipt {
        revision: None,
        schema: PROOF_RECEIPT_SCHEMA.to_owned(),
        id: task.id.clone(),
        kind: "focused-red-green".to_owned(),
        base: diff.base.clone(),
        head: diff.head.clone(),
        test_patch_mode: "base-plus-tests".to_owned(),
        requested_by: task.requested_by.clone(),
        request_ids: task.request_ids.clone(),
        commands,
        result,
        reason,
    }
}

fn proof_command_paths(out: &Path, receipt_id: &str, side: &str) -> Result<ProofCommandPaths> {
    let rel_dir = format!("proof/{receipt_id}/{side}");
    let dir = out.join(&rel_dir);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let stdout_path = dir.join("stdout.txt");
    let stderr_path = dir.join("stderr.txt");
    if !stdout_path.exists() {
        fs::write(&stdout_path, b"")?;
    }
    if !stderr_path.exists() {
        fs::write(&stderr_path, b"")?;
    }
    Ok(ProofCommandPaths {
        stdout_path,
        stderr_path,
        stdout_rel: format!("{rel_dir}/stdout.txt"),
        stderr_rel: format!("{rel_dir}/stderr.txt"),
    })
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use anyhow::ensure;

    use super::*;
    use crate::task_ledger::{TaskEvent, TaskTerminalDisposition};

    fn revision() -> RevisionRef {
        RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        }
    }

    fn focused_task(id: &str) -> FocusedTestTask {
        FocusedTestTask {
            id: id.to_owned(),
            file: "src/lib.rs".to_owned(),
            test_name: Some("focused_case".to_owned()),
            mode: FocusedProofMode::HeadOnly,
            command_specs: None,
            timeout_sec: Some(7),
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids: vec!["request-a".to_owned()],
        }
    }

    fn granted_lease(id: &str) -> ResourceLease {
        ResourceLease {
            revision: None,
            schema: RESOURCE_LEASE_SCHEMA.to_owned(),
            id: id.to_owned(),
            kind: "focused-test".to_owned(),
            consumer: id.strip_prefix("lease-").unwrap_or(id).to_owned(),
            status: "granted".to_owned(),
            reason: "test lease granted".to_owned(),
            cpu: 1,
            memory_mb: 512,
            disk_mb: 64,
            timeout_sec: 60,
            network: false,
            scratch: false,
            worktree: None,
            command: Some("head: cargo test focused_case --locked".to_owned()),
        }
    }

    #[test]
    fn proof_command_receipt_records_timeout_and_artifact_paths() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let spec = ProofCommandSpec {
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "focused_case".to_owned(),
            ],
            env: BTreeMap::new(),
        };

        let receipt = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: "proof-command-001",
                side: "head",
                spec: &spec,
                timeout_sec: 7,
                lease: &granted_lease("lease-proof-command-001"),
                task_ledger: None,
                task: None,
                cleanup: None,
            },
            &mut |_root, _argv, _env, timeout, stdout, stderr, _observe_process| {
                fs::write(stdout, b"started\n")?;
                fs::write(stderr, b"timed out\n")?;
                Ok(CommandStatus {
                    exit_code: None,
                    timed_out: true,
                    success: false,
                    reason: format!("timed out after {timeout}s"),
                    duration_ms: 7_001,
                })
            },
        )?;

        assert_eq!(receipt.status, "timed_out");
        assert_eq!(receipt.timeout_sec, 7);
        assert!(receipt.timed_out);
        assert_eq!(receipt.stdout, "proof/proof-command-001/head/stdout.txt");
        assert_eq!(receipt.stderr, "proof/proof-command-001/head/stderr.txt");
        assert_eq!(fs::read_to_string(out.join(&receipt.stdout))?, "started\n");
        assert_eq!(
            fs::read_to_string(out.join(&receipt.stderr))?,
            "timed out\n"
        );
        Ok(())
    }

    #[test]
    fn proof_command_receipt_refuses_non_granted_lease_without_running() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let spec = ProofCommandSpec {
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "focused_case".to_owned(),
            ],
            env: BTreeMap::new(),
        };
        let mut lease = granted_lease("lease-proof-command-skipped");
        lease.status = "exhausted".to_owned();
        lease.reason = "profile budget exhausted".to_owned();
        let mut runner_called = false;

        let receipt = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: "proof-command-skipped",
                side: "head",
                spec: &spec,
                timeout_sec: 60,
                lease: &lease,
                task_ledger: None,
                task: None,
                cleanup: None,
            },
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                runner_called = true;
                Ok(CommandStatus {
                    exit_code: Some(0),
                    timed_out: false,
                    success: true,
                    reason: "should not run".to_owned(),
                    duration_ms: 1,
                })
            },
        )?;

        assert!(!runner_called, "proof command ran without a granted lease");
        assert_eq!(receipt.status, "skipped");
        assert_eq!(receipt.timeout_sec, 0);
        assert!(receipt.reason.contains("lease-proof-command-skipped"));
        assert!(receipt.reason.contains("exhausted"));
        assert_eq!(fs::read_to_string(out.join(&receipt.stdout))?, "");
        assert_eq!(fs::read_to_string(out.join(&receipt.stderr))?, "");
        Ok(())
    }

    #[test]
    fn proof_command_receipt_refuses_lease_for_different_consumer() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let spec = ProofCommandSpec {
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "focused_case".to_owned(),
            ],
            env: BTreeMap::new(),
        };
        let mut lease = granted_lease("lease-other-proof");
        lease.consumer = "other-proof".to_owned();
        let mut runner_called = false;

        let receipt = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: "proof-command-needs-own-lease",
                side: "head",
                spec: &spec,
                timeout_sec: 60,
                lease: &lease,
                task_ledger: None,
                task: None,
                cleanup: None,
            },
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                runner_called = true;
                Ok(CommandStatus {
                    exit_code: Some(0),
                    timed_out: false,
                    success: true,
                    reason: "should not run".to_owned(),
                    duration_ms: 1,
                })
            },
        )?;

        assert!(
            !runner_called,
            "proof command ran with another consumer's lease"
        );
        assert_eq!(receipt.status, "skipped");
        assert!(receipt.reason.contains("other-proof"));
        assert!(receipt.reason.contains("proof-command-needs-own-lease"));
        Ok(())
    }

    #[test]
    fn proof_command_receipt_bounds_stdout_and_stderr_artifacts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let spec = ProofCommandSpec {
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "loud_case".to_owned(),
            ],
            env: BTreeMap::new(),
        };
        let loud_stdout = vec![b'o'; PROOF_COMMAND_STREAM_MAX_BYTES + 4096];
        let loud_stderr = vec![b'e'; PROOF_COMMAND_STREAM_MAX_BYTES + 8192];

        let receipt = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: "proof-command-loud",
                side: "head",
                spec: &spec,
                timeout_sec: 60,
                lease: &granted_lease("lease-proof-command-loud"),
                task_ledger: None,
                task: None,
                cleanup: None,
            },
            &mut |_root, _argv, _env, _timeout, stdout, stderr, _observe_process| {
                fs::write(stdout, &loud_stdout)?;
                fs::write(stderr, &loud_stderr)?;
                Ok(CommandStatus {
                    exit_code: Some(1),
                    timed_out: false,
                    success: false,
                    reason: "exit code Some(1)".to_owned(),
                    duration_ms: 42,
                })
            },
        )?;

        let bounded_stdout = fs::read(out.join(&receipt.stdout))?;
        let bounded_stderr = fs::read(out.join(&receipt.stderr))?;
        assert!(bounded_stdout.len() <= PROOF_COMMAND_STREAM_MAX_BYTES);
        assert!(bounded_stderr.len() <= PROOF_COMMAND_STREAM_MAX_BYTES);
        let stdout_text = String::from_utf8_lossy(&bounded_stdout);
        let stderr_text = String::from_utf8_lossy(&bounded_stderr);
        assert!(stdout_text.starts_with("[ub-review truncated proof command stream:"));
        assert!(stderr_text.starts_with("[ub-review truncated proof command stream:"));
        assert!(bounded_stdout.ends_with(&[b'o'; 32]));
        assert!(bounded_stderr.ends_with(&[b'e'; 32]));
        Ok(())
    }

    #[test]
    fn proof_command_task_records_spawn_and_completion_but_waits_for_receipt_publication()
    -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let recorder = TaskLedgerRecorder::new(&revision(), &Instant::now())?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let focused = focused_task("proof-command-observed");
        let command_task =
            ProofCommandTask::focused_test(&focused, "head", 7, ProofExecutionPhase::ModelRequest);
        let spec = ProofCommandSpec {
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "focused_case".to_owned(),
            ],
            env: BTreeMap::new(),
        };
        let lease = granted_lease("lease-proof-command-observed");

        let receipt = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: &focused.id,
                side: "head",
                spec: &spec,
                timeout_sec: 7,
                lease: &lease,
                task_ledger: Some(&ledger),
                task: Some(&command_task),
                cleanup: None,
            },
            &mut |_root, _argv, _env, _timeout, stdout, stderr, observe_process| {
                observe_process(CommandProcessObservation::Spawned);
                fs::write(stdout, b"ok\n")?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(0),
                    timed_out: false,
                    success: true,
                    reason: "completed".to_owned(),
                    duration_ms: 2,
                })
            },
        )?;

        ensure!(receipt.status == "passed");
        let task_id = proof_command_task_id(&focused.id, "head")?;
        let events = recorder
            .inputs()?
            .into_iter()
            .filter(|input| input.task_id == task_id)
            .map(|input| input.event)
            .collect::<Vec<_>>();
        ensure!(
            events
                .iter()
                .any(|event| matches!(event, TaskEvent::RunStarted { .. }))
        );
        ensure!(events.iter().any(|event| matches!(
            event,
            TaskEvent::ProcessFinished {
                disposition: TaskTerminalDisposition::Succeeded,
                ..
            }
        )));
        ensure!(
            events
                .iter()
                .any(|event| matches!(event, TaskEvent::CleanupFinished { .. }))
        );
        ensure!(
            !events
                .iter()
                .any(|event| matches!(event, TaskEvent::ReceiptCreated { .. }))
        );
        ensure!(
            !events
                .iter()
                .any(|event| matches!(event, TaskEvent::ResourcesReleased { .. }))
        );
        Ok(())
    }

    #[test]
    fn completion_unconfirmed_stops_before_terminal_receipt_and_release() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let recorder = TaskLedgerRecorder::new(&revision(), &Instant::now())?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let focused = focused_task("proof-command-unconfirmed");
        let command_task =
            ProofCommandTask::focused_test(&focused, "head", 7, ProofExecutionPhase::ModelRequest);
        let spec = ProofCommandSpec {
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "focused_case".to_owned(),
            ],
            env: BTreeMap::new(),
        };
        let lease = granted_lease("lease-proof-command-unconfirmed");

        let error = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: &focused.id,
                side: "head",
                spec: &spec,
                timeout_sec: 7,
                lease: &lease,
                task_ledger: Some(&ledger),
                task: Some(&command_task),
                cleanup: None,
            },
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, observe_process| {
                observe_process(CommandProcessObservation::Spawned);
                observe_process(CommandProcessObservation::CompletionUnconfirmed);
                Err(anyhow::anyhow!("injected unconfirmed cleanup"))
            },
        )
        .err()
        .context("unconfirmed child must fail closed")?;
        ensure!(format!("{error:#}").contains("completion remains unconfirmed"));

        let task_id = proof_command_task_id(&focused.id, "head")?;
        let events = recorder
            .inputs()?
            .into_iter()
            .filter(|input| input.task_id == task_id)
            .map(|input| input.event)
            .collect::<Vec<_>>();
        ensure!(
            events
                .iter()
                .any(|event| matches!(event, TaskEvent::RunStarted { .. }))
        );
        for event in &events {
            ensure!(!matches!(
                event,
                TaskEvent::ProcessFinished { .. }
                    | TaskEvent::CleanupFinished { .. }
                    | TaskEvent::ReceiptCreated { .. }
                    | TaskEvent::ReceiptCreationFailed { .. }
                    | TaskEvent::ResourcesReleased { .. }
            ));
        }
        Ok(())
    }

    #[test]
    fn spawned_runner_error_is_cancelled_and_serialized_as_skipped() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let recorder = TaskLedgerRecorder::new(&revision(), &Instant::now())?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let focused = focused_task("proof-command-runner-error");
        let command_task =
            ProofCommandTask::focused_test(&focused, "head", 7, ProofExecutionPhase::ModelRequest);
        let spec = ProofCommandSpec {
            argv: vec!["proof-command".to_owned()],
            env: BTreeMap::new(),
        };
        let lease = granted_lease("lease-proof-command-runner-error");

        let receipt = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: &focused.id,
                side: "head",
                spec: &spec,
                timeout_sec: 7,
                lease: &lease,
                task_ledger: Some(&ledger),
                task: Some(&command_task),
                cleanup: None,
            },
            &mut |_root, _argv, _env, _timeout, stdout, stderr, observe_process| {
                observe_process(CommandProcessObservation::Spawned);
                fs::write(stdout, b"")?;
                fs::write(stderr, b"runner failed\n")?;
                Err(anyhow::anyhow!("injected runner failure"))
            },
        )?;
        ensure!(receipt.status == "skipped");

        let task_id = proof_command_task_id(&focused.id, "head")?;
        let events = recorder
            .inputs()?
            .into_iter()
            .filter(|input| input.task_id == task_id)
            .map(|input| input.event)
            .collect::<Vec<_>>();
        ensure!(events.iter().any(|event| matches!(
            event,
            TaskEvent::ProcessFinished {
                disposition: TaskTerminalDisposition::Cancelled,
                ..
            }
        )));
        ensure!(
            events
                .iter()
                .any(|event| matches!(event, TaskEvent::CleanupFinished { .. }))
        );
        Ok(())
    }

    #[test]
    fn stream_bounding_failure_does_not_claim_cleanup_completion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let recorder = TaskLedgerRecorder::new(&revision(), &Instant::now())?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let focused = focused_task("proof-command-stream-failure");
        let command_task =
            ProofCommandTask::focused_test(&focused, "head", 7, ProofExecutionPhase::ModelRequest);
        let spec = ProofCommandSpec {
            argv: vec!["proof-command".to_owned()],
            env: BTreeMap::new(),
        };
        let lease = granted_lease("lease-proof-command-stream-failure");
        let mut physical_cleanup_called = false;
        let mut cleanup = || {
            physical_cleanup_called = true;
            Ok(())
        };

        let error = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: &focused.id,
                side: "head",
                spec: &spec,
                timeout_sec: 7,
                lease: &lease,
                task_ledger: Some(&ledger),
                task: Some(&command_task),
                cleanup: Some(&mut cleanup),
            },
            &mut |_root, _argv, _env, _timeout, stdout, stderr, observe_process| {
                observe_process(CommandProcessObservation::Spawned);
                fs::remove_file(stdout)?;
                fs::create_dir_all(stdout)?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(0),
                    timed_out: false,
                    success: true,
                    reason: "completed".to_owned(),
                    duration_ms: 1,
                })
            },
        )
        .err()
        .context("directory-backed stream must fail bounding")?;
        ensure!(format!("{error:#}").contains("read"));
        ensure!(physical_cleanup_called);

        let task_id = proof_command_task_id(&focused.id, "head")?;
        let events = recorder
            .inputs()?
            .into_iter()
            .filter(|input| input.task_id == task_id)
            .map(|input| input.event)
            .collect::<Vec<_>>();
        ensure!(
            events
                .iter()
                .any(|event| matches!(event, TaskEvent::ProcessFinished { .. }))
        );
        ensure!(!events.iter().any(|event| matches!(
            event,
            TaskEvent::CleanupFinished { .. }
                | TaskEvent::ReceiptCreated { .. }
                | TaskEvent::ReceiptCreationFailed { .. }
                | TaskEvent::ResourcesReleased { .. }
        )));
        Ok(())
    }

    #[test]
    fn runner_setup_error_has_no_process_timing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let recorder = TaskLedgerRecorder::new(&revision(), &Instant::now())?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let focused = focused_task("proof-command-setup-failure");
        let command_task =
            ProofCommandTask::focused_test(&focused, "head", 7, ProofExecutionPhase::ModelRequest);
        let spec = ProofCommandSpec {
            argv: vec!["missing-proof-command".to_owned()],
            env: BTreeMap::new(),
        };
        let lease = granted_lease("lease-proof-command-setup-failure");

        let receipt = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: &focused.id,
                side: "head",
                spec: &spec,
                timeout_sec: 7,
                lease: &lease,
                task_ledger: Some(&ledger),
                task: Some(&command_task),
                cleanup: None,
            },
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                Err(anyhow::anyhow!("injected pre-spawn failure"))
            },
        )?;
        ensure!(receipt.status == "skipped");

        let task_id = proof_command_task_id(&focused.id, "head")?;
        let events = recorder
            .inputs()?
            .into_iter()
            .filter(|input| input.task_id == task_id)
            .map(|input| input.event)
            .collect::<Vec<_>>();
        ensure!(
            events
                .iter()
                .any(|event| matches!(event, TaskEvent::SetupFailed { .. }))
        );
        ensure!(
            !events
                .iter()
                .any(|event| matches!(event, TaskEvent::RunStarted { .. }))
        );
        ensure!(
            !events
                .iter()
                .any(|event| matches!(event, TaskEvent::ProcessFinished { .. }))
        );
        Ok(())
    }
}
