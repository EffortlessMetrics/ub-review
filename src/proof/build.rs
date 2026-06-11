//! Focused build proof execution under the broker runtime budget.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

use crate::*;

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn run_focused_build_proof_tasks_with_runner<F>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
    budget: ProofBudget,
    tasks: Vec<FocusedBuildTask>,
    mut runner: F,
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
{
    let mut receipts = Vec::new();
    let mut leases = Vec::new();
    let mut executed_tasks = 0_usize;
    let mut estimated_seconds = 0_u64;
    let lease_budget = proof_lease_budget(profile)?;
    for task in tasks {
        let task_timeout_sec = focused_build_task_command_timeout(&task, budget);
        if args.dry_run {
            leases.push(focused_build_resource_lease(
                &task,
                budget,
                lease_budget,
                "skipped_profile",
                "dry-run; resource broker did not grant a build proof lease",
            ));
            receipts.push(skipped_focused_build_receipt(
                out,
                diff,
                &task,
                "skipped_profile",
                "dry-run; proof broker did not execute focused build",
            )?);
            continue;
        }
        if profile.limits.builds == 0 {
            leases.push(focused_build_resource_lease(
                &task,
                budget,
                lease_budget,
                "skipped_profile",
                "profile allows zero focused build leases",
            ));
            receipts.push(skipped_focused_build_receipt(
                out,
                diff,
                &task,
                "skipped_profile",
                "profile allows zero focused build leases",
            )?);
            continue;
        }
        if !focused_build_budget_allows_next(
            executed_tasks,
            estimated_seconds,
            task_timeout_sec,
            budget,
        ) {
            leases.push(focused_build_resource_lease(
                &task,
                budget,
                lease_budget,
                "exhausted",
                "focused build proof lease budget exhausted by runtime profile",
            ));
            receipts.push(skipped_focused_build_receipt(
                out,
                diff,
                &task,
                "skipped_budget",
                "focused build proof lease budget exhausted by runtime profile",
            )?);
            continue;
        }
        leases.push(focused_build_resource_lease(
            &task,
            budget,
            lease_budget,
            "granted",
            "focused build proof lease granted by runtime profile",
        ));
        receipts.push(run_focused_build_proof_task(
            root,
            out,
            diff,
            &task,
            task_timeout_sec,
            &mut runner,
        )?);
        executed_tasks += 1;
        estimated_seconds = estimated_seconds.saturating_add(task_timeout_sec);
    }
    Ok(ProofBrokerResult {
        proof_receipts: receipts,
        resource_leases: leases,
    })
}

fn focused_build_budget_allows_next(
    current_tasks: usize,
    estimated_seconds: u64,
    next_timeout_sec: u64,
    budget: ProofBudget,
) -> bool {
    current_tasks < budget.max_focused_tests
        && estimated_seconds.saturating_add(next_timeout_sec) <= budget.max_total_seconds
}

fn run_focused_build_proof_task<F>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    task: &FocusedBuildTask,
    timeout_sec: u64,
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
    let spec = focused_build_command_spec_for_task(task);
    let head =
        run_proof_command_receipt_for_id(root, out, &task.id, "head", &spec, timeout_sec, runner)?;
    let result = match head.status.as_str() {
        "passed" => "head_passed",
        "failed" => "head_failed",
        "timed_out" => "timed_out",
        _ => "skipped_profile",
    };
    let reason = format!("HEAD build proof {}: {}", head.status, head.reason);
    Ok(focused_build_receipt(
        diff,
        task,
        vec![head],
        result.to_owned(),
        reason,
    ))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;

    use crate::tests::{test_diff, test_run_args};
    use crate::*;

    #[test]
    fn proof_broker_v0_executes_allowlisted_focused_build_request() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let proof_requests = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-build-001".to_owned(),
            lane: "architecture".to_owned(),
            requested_by: vec!["architecture".to_owned()],
            command: "cargo check --workspace --all-targets --locked".to_owned(),
            reason: "Run the requested focused build proof.".to_owned(),
            cost: "focused-build".to_owned(),
            timeout_sec: 90,
            required: false,
            status: "requested".to_owned(),
        }];
        let tasks = focused_build_candidates_from_requests(&proof_requests);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].command, proof_requests[0].command);
        assert_eq!(tasks[0].timeout_sec, 90);

        let args = test_run_args(out.clone());
        let profile = Profile {
            limits: Limits {
                builds: 1,
                ..Limits::default()
            },
            ..Profile::default()
        };
        let mut commands = Vec::<String>::new();
        let proof_result = super::run_focused_build_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &profile,
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 120,
                max_total_seconds: 120,
            },
            tasks,
            |_root, argv, env, timeout, stdout, stderr| {
                commands.push(command_display_with_env(env, argv));
                assert!(env.is_empty());
                assert_eq!(timeout, 90);
                fs::write(stdout, b"build ok\n")?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(0),
                    timed_out: false,
                    success: true,
                    reason: "completed".to_owned(),
                    duration_ms: 34,
                })
            },
        )?;

        assert_eq!(
            commands,
            vec!["cargo check --workspace --all-targets --locked"]
        );
        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        let receipt = &proof_result.proof_receipts[0];
        assert_eq!(receipt.kind, "focused-build");
        assert_eq!(receipt.test_patch_mode, "head-only");
        assert_eq!(receipt.result, "head_passed");
        assert_eq!(receipt.commands.len(), 1);
        assert_eq!(receipt.commands[0].side, "head");
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[0].command, proof_requests[0].command);
        assert!(out.join(&receipt.commands[0].stdout).exists());
        let lease = &proof_result.resource_leases[0];
        assert_eq!(lease.kind, "focused-build");
        assert_eq!(lease.status, "granted");
        assert_eq!(lease.timeout_sec, 90);
        assert_eq!(lease.worktree, None);
        assert_eq!(
            lease.command.as_deref(),
            Some("head: cargo check --workspace --all-targets --locked")
        );
        Ok(())
    }

    #[test]
    fn focused_build_request_skips_when_profile_disables_build_leases() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let proof_requests = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-build-001".to_owned(),
            lane: "architecture".to_owned(),
            requested_by: vec!["architecture".to_owned()],
            command: "cargo check --workspace --all-targets --locked".to_owned(),
            reason: "Run the requested focused build proof.".to_owned(),
            cost: "focused-build".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "requested".to_owned(),
        }];
        let tasks = focused_build_candidates_from_requests(&proof_requests);
        let args = test_run_args(out.clone());
        let proof_result = super::run_focused_build_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 120,
                max_total_seconds: 120,
            },
            tasks,
            |_root, _argv, _env, _timeout, _stdout, _stderr| {
                Err(anyhow::anyhow!("build runner should not execute"))
            },
        )?;

        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.proof_receipts[0].kind, "focused-build");
        assert_eq!(proof_result.proof_receipts[0].result, "skipped_profile");
        assert_eq!(proof_result.proof_receipts[0].commands[0].status, "skipped");
        assert_eq!(proof_result.resource_leases[0].kind, "focused-build");
        assert_eq!(proof_result.resource_leases[0].status, "skipped_profile");
        assert_eq!(
            proof_result.resource_leases[0].reason,
            "profile allows zero focused build leases"
        );
        Ok(())
    }
}
