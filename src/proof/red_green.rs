//! Focused proof execution: HEAD-only and base+tests red/green command
//! receipts under the broker runtime budget.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::*;

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn run_focused_red_green_proof_tasks_with_runner<F, G>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
    budget: ProofBudget,
    tasks: Vec<FocusedTestTask>,
    mut runner: F,
    mut prepare_base_plus_tests: G,
) -> Result<ProofBrokerResult>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
    ) -> Result<CommandStatus>,
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
{
    let mut receipts = Vec::new();
    let mut leases = Vec::new();
    let mut executed_tasks = 0_usize;
    let mut executed_files = BTreeSet::new();
    let mut estimated_seconds = 0_u64;
    let lease_budget = proof_lease_budget(profile)?;
    for task in tasks {
        let task_timeout_sec = focused_test_task_command_timeout(&task, budget);
        if args.dry_run {
            leases.push(focused_test_resource_lease(
                &task,
                budget,
                lease_budget,
                "skipped_profile",
                "dry-run; resource broker did not grant a proof lease",
            ));
            receipts.push(skipped_focused_proof_receipt(
                out,
                diff,
                &task,
                "skipped_profile",
                "dry-run; proof broker did not execute focused tests",
            )?);
            continue;
        }
        if profile.limits.tests == 0 {
            leases.push(focused_test_resource_lease(
                &task,
                budget,
                lease_budget,
                "absent",
                "profile allows zero focused test leases",
            ));
            receipts.push(skipped_focused_proof_receipt(
                out,
                diff,
                &task,
                "skipped_profile",
                "profile allows zero focused test leases",
            )?);
            continue;
        }
        if !focused_proof_budget_allows_next(
            executed_tasks,
            &executed_files,
            &task.file,
            estimated_seconds,
            task_timeout_sec,
            task.mode.command_count(),
            budget,
        ) {
            leases.push(focused_test_resource_lease(
                &task,
                budget,
                lease_budget,
                "exhausted",
                "focused red/green proof lease budget exhausted by runtime profile",
            ));
            receipts.push(skipped_focused_proof_receipt(
                out,
                diff,
                &task,
                "skipped_budget",
                "focused red/green proof lease budget exhausted by runtime profile",
            )?);
            continue;
        }
        executed_files.insert(task.file.clone());
        let lease = focused_test_resource_lease(
            &task,
            budget,
            lease_budget,
            "granted",
            "focused red/green proof lease granted by runtime profile",
        );
        let receipt = match task.mode {
            FocusedProofMode::HeadOnly => run_focused_head_proof_task(
                root,
                out,
                diff,
                &task,
                task_timeout_sec,
                &lease,
                &mut runner,
            )?,
            FocusedProofMode::RedGreen => run_focused_red_green_proof_task(
                root,
                out,
                diff,
                &task,
                task_timeout_sec,
                &lease,
                &mut runner,
                &mut prepare_base_plus_tests,
            )?,
        };
        leases.push(lease);
        receipts.push(receipt);
        executed_tasks += 1;
        estimated_seconds = estimated_seconds
            .saturating_add(task_timeout_sec.saturating_mul(task.mode.command_count()));
    }
    Ok(ProofBrokerResult {
        proof_receipts: receipts,
        resource_leases: leases,
    })
}

fn run_focused_head_proof_task<F>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    task: &FocusedTestTask,
    timeout_sec: u64,
    lease: &ResourceLease,
    runner: &mut F,
) -> Result<ProofReceipt>
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
    let head_spec = proof_task_command_spec(task, "head");
    let head = run_proof_command_receipt_for_task(
        root,
        out,
        task,
        "head",
        &head_spec,
        timeout_sec,
        lease,
        runner,
    )?;
    let result = match head.status.as_str() {
        "passed" => "head_passed",
        "failed" => "head_failed",
        "timed_out" => "timed_out",
        _ => "skipped_profile",
    };
    let reason = format!("HEAD proof {}: {}", head.status, head.reason);
    Ok(focused_head_receipt(
        diff,
        task,
        vec![head],
        result.to_owned(),
        reason,
    ))
}

#[expect(
    clippy::too_many_arguments,
    reason = "lease is an execution precondition for the proof command; keeping it explicit pins the no-lease-no-command broker contract"
)]
pub(crate) fn run_focused_red_green_proof_task<F, G>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    task: &FocusedTestTask,
    timeout_sec: u64,
    lease: &ResourceLease,
    runner: &mut F,
    prepare_base_plus_tests: &mut G,
) -> Result<ProofReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
    ) -> Result<CommandStatus>,
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
{
    let head_spec = proof_task_command_spec(task, "head");
    let head = run_proof_command_receipt_for_task(
        root,
        out,
        task,
        "head",
        &head_spec,
        timeout_sec,
        lease,
        runner,
    )?;
    let head_status = head.status.clone();
    if head_status != "passed" {
        let result = match head_status.as_str() {
            "timed_out" => "timed_out",
            "failed" => "head_failed",
            _ => "skipped_profile",
        };
        let reason = format!("HEAD proof {}: {}", head.status, head.reason);
        return Ok(focused_red_green_receipt(
            diff,
            task,
            vec![head],
            result.to_owned(),
            reason,
        ));
    }

    let base_root = match prepare_base_plus_tests(root, out, diff) {
        Ok(path) => path,
        Err(error) => {
            let mut commands = vec![head];
            let base_spec = proof_task_command_spec(task, "base-plus-tests");
            let patch_reason = format!("base+tests patch failed: {error:#}");
            commands.push(skipped_proof_command_receipt(
                out,
                task,
                "base-plus-tests",
                &base_spec,
                "skipped",
                patch_reason.clone(),
            )?);
            return Ok(focused_red_green_receipt(
                diff,
                task,
                commands,
                "base_patch_failed".to_owned(),
                patch_reason,
            ));
        }
    };
    let base_spec = proof_task_command_spec(task, "base-plus-tests");
    let base = run_proof_command_receipt_for_task(
        &base_root,
        out,
        task,
        "base-plus-tests",
        &base_spec,
        timeout_sec,
        lease,
        runner,
    )?;
    let (result, reason) = match base.status.as_str() {
        "failed" => (
            "discriminating".to_owned(),
            format!("HEAD passed; base+tests failed: {}", base.reason),
        ),
        "passed" => (
            "non_discriminating".to_owned(),
            "HEAD and base+tests both passed".to_owned(),
        ),
        "timed_out" => (
            "timed_out".to_owned(),
            format!("base+tests timed out: {}", base.reason),
        ),
        _ => (
            "skipped_profile".to_owned(),
            format!("base+tests proof unavailable: {}", base.reason),
        ),
    };
    let _ = cleanup_base_plus_tests_worktree(root, &base_root);
    Ok(focused_red_green_receipt(
        diff,
        task,
        vec![head, base],
        result,
        reason,
    ))
}
