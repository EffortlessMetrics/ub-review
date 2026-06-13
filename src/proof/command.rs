//! Shared proof command receipt construction for broker-executed checks.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

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

#[derive(Default)]
pub(crate) struct ProofBrokerResult {
    pub(crate) proof_receipts: Vec<ProofReceipt>,
    pub(crate) resource_leases: Vec<ResourceLease>,
}

pub(crate) fn run_proof_command_receipt<F>(
    command_root: &Path,
    out: &Path,
    task: &FocusedTestTask,
    side: &str,
    spec: &ProofCommandSpec,
    timeout_sec: u64,
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
    ) -> Result<CommandStatus>,
{
    run_proof_command_receipt_for_id(command_root, out, &task.id, side, spec, timeout_sec, runner)
}

pub(crate) fn run_proof_command_receipt_for_id<F>(
    command_root: &Path,
    out: &Path,
    receipt_id: &str,
    side: &str,
    spec: &ProofCommandSpec,
    timeout_sec: u64,
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
    ) -> Result<CommandStatus>,
{
    let paths = proof_command_paths(out, receipt_id, side)?;
    let command = command_display_with_env(&spec.env, &spec.argv);
    let status = runner(
        command_root,
        &spec.argv,
        &spec.env,
        timeout_sec,
        &paths.stdout_path,
        &paths.stderr_path,
    );
    bound_proof_command_streams(&paths)?;
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
    use super::*;

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

        let receipt = run_proof_command_receipt_for_id(
            temp.path(),
            &out,
            "proof-command-001",
            "head",
            &spec,
            7,
            &mut |_root, _argv, _env, timeout, stdout, stderr| {
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

        let receipt = run_proof_command_receipt_for_id(
            temp.path(),
            &out,
            "proof-command-loud",
            "head",
            &spec,
            60,
            &mut |_root, _argv, _env, _timeout, stdout, stderr| {
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
}
