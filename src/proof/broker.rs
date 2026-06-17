//! Proof broker entry points: diff, seeded, request, and follow-up
//! orchestration that drives the focused test/build runners and writes
//! the proof receipts and resource leases the gate consumes (cleanup
//! train step 12, pure code motion). The focused runners, budgets, and
//! worktree helpers already live in the sibling proof/ submodules; this
//! module owns only the broker run orchestration and the lease
//! constructors for focused test/build tasks.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::Result;

use crate::*;

pub(crate) fn run_initial_diff_proof_broker_v0(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
) -> Result<ProofBrokerResult> {
    let budget = proof_budget(profile)?;
    let tasks = focused_test_candidates_from_diff(diff, &[]);
    run_focused_red_green_proof_tasks_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        tasks,
        run_command_to_files,
        prepare_base_plus_tests_worktree,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "seeded proof stream coordinates scheduler phases and proof broker inputs"
)]
pub(crate) fn run_seeded_proof_stream_v0(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
    seeded_proof_requests: &[ProofRequest],
    initial_proof_loop: ActiveRunLoop,
    event_log: &EventLog,
    run_started: &Instant,
) -> Result<(ProofBrokerResult, Vec<RunLoopPhase>)> {
    let mut phases = Vec::new();
    let initial_result = run_initial_diff_proof_broker_v0(root, out, diff, profile, args);
    let initial_status = if initial_result.is_ok() {
        "completed"
    } else {
        "failed"
    };
    phases.push(finish_run_loop_phase(
        event_log,
        run_started,
        initial_proof_loop,
        initial_status,
    )?);
    let mut proof_result = initial_result?;

    if has_unreceipted_proof_request_tasks(seeded_proof_requests, &proof_result.proof_receipts) {
        let seeded_request_loop = start_run_loop(
            event_log,
            run_started,
            "proof",
            "proof",
            "seeded-request-broker",
        )?;
        let request_result = run_request_proof_broker_v0(
            root,
            out,
            diff,
            profile,
            seeded_proof_requests,
            &proof_result.proof_receipts,
            &proof_result.resource_leases,
            args,
        );
        let request_status = if request_result.is_ok() {
            "completed"
        } else {
            "failed"
        };
        phases.push(finish_run_loop_phase(
            event_log,
            run_started,
            seeded_request_loop,
            request_status,
        )?);
        let request_result = request_result?;
        proof_result
            .proof_receipts
            .extend(request_result.proof_receipts);
        proof_result
            .resource_leases
            .extend(request_result.resource_leases);
    }

    Ok((proof_result, phases))
}

#[expect(
    clippy::too_many_arguments,
    reason = "primary request proof broker mirrors follow-up broker inputs"
)]
pub(crate) fn run_request_proof_broker_v0(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    proof_requests: &[ProofRequest],
    existing_receipts: &[ProofReceipt],
    existing_leases: &[ResourceLease],
    args: &RunArgs,
) -> Result<ProofBrokerResult> {
    let total_budget = proof_budget(profile)?;
    let budget = remaining_focused_proof_budget(total_budget, existing_leases);
    let tasks = unreceipted_focused_test_tasks(
        focused_test_candidates_from_requests(proof_requests),
        existing_receipts,
    );
    let mut result = run_follow_up_proof_broker_v0_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        tasks,
        run_command_to_files,
        prepare_base_plus_tests_worktree,
    )?;
    let mut consumed_leases = existing_leases.to_vec();
    consumed_leases.extend(result.resource_leases.clone());
    let remaining_budget = remaining_focused_proof_budget(total_budget, &consumed_leases);
    let existing_and_new_receipts = existing_receipts
        .iter()
        .chain(result.proof_receipts.iter())
        .cloned()
        .collect::<Vec<_>>();
    let build_tasks = focused_build_candidates_from_requests(proof_requests);
    let build_tasks = unreceipted_focused_build_tasks(build_tasks, &existing_and_new_receipts);
    let build_result = run_focused_build_proof_tasks_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        remaining_budget,
        build_tasks,
        run_command_to_files,
    )?;
    result.proof_receipts.extend(build_result.proof_receipts);
    result.resource_leases.extend(build_result.resource_leases);
    Ok(result)
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn run_follow_up_proof_broker_v0(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    proof_requests: &[ProofRequest],
    existing_receipts: &[ProofReceipt],
    existing_leases: &[ResourceLease],
    args: &RunArgs,
) -> Result<ProofBrokerResult> {
    let total_budget = proof_budget(profile)?;
    let budget = remaining_focused_proof_budget(total_budget, existing_leases);
    let tasks = unreceipted_focused_test_tasks(
        focused_test_candidates_from_requests(proof_requests),
        existing_receipts,
    );
    let mut result = run_follow_up_proof_broker_v0_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        tasks,
        run_command_to_files,
        prepare_base_plus_tests_worktree,
    )?;
    let mut consumed_leases = existing_leases.to_vec();
    consumed_leases.extend(result.resource_leases.clone());
    let remaining_budget = remaining_focused_proof_budget(total_budget, &consumed_leases);
    let existing_and_new_receipts = existing_receipts
        .iter()
        .chain(result.proof_receipts.iter())
        .cloned()
        .collect::<Vec<_>>();
    let build_tasks = unreceipted_focused_build_tasks(
        focused_build_candidates_from_requests(proof_requests),
        &existing_and_new_receipts,
    );
    let build_result = run_focused_build_proof_tasks_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        remaining_budget,
        build_tasks,
        run_command_to_files,
    )?;
    result.proof_receipts.extend(build_result.proof_receipts);
    result.resource_leases.extend(build_result.resource_leases);
    Ok(result)
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn run_follow_up_proof_broker_v0_with_runner<F, G>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
    budget: ProofBudget,
    tasks: Vec<FocusedTestTask>,
    runner: F,
    prepare_base_plus_tests: G,
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
    run_focused_red_green_proof_tasks_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        tasks,
        runner,
        prepare_base_plus_tests,
    )
}

pub(crate) fn attach_request_metadata_to_focused_receipts(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
    proof_receipts: &mut [ProofReceipt],
) {
    if proof_requests.is_empty() || proof_receipts.is_empty() {
        return;
    }
    let request_metadata = focused_test_candidates_from_diff(diff, proof_requests)
        .into_iter()
        .filter(|task| !task.request_ids.is_empty())
        .map(|task| (task.id, (task.requested_by, task.request_ids)))
        .collect::<BTreeMap<_, _>>();
    for receipt in proof_receipts {
        let Some((requested_by, request_ids)) = request_metadata.get(&receipt.id) else {
            continue;
        };
        for lane in requested_by {
            push_unique(&mut receipt.requested_by, lane);
        }
        for request_id in request_ids {
            push_unique(&mut receipt.request_ids, request_id);
        }
    }
}

pub(crate) fn unreceipted_focused_test_tasks(
    tasks: Vec<FocusedTestTask>,
    existing_receipts: &[ProofReceipt],
) -> Vec<FocusedTestTask> {
    let existing_ids = existing_receipts
        .iter()
        .map(|receipt| receipt.id.clone())
        .collect::<BTreeSet<_>>();
    tasks
        .into_iter()
        .filter(|task| !existing_ids.contains(&task.id))
        .collect()
}

pub(crate) fn unreceipted_focused_build_tasks(
    tasks: Vec<FocusedBuildTask>,
    existing_receipts: &[ProofReceipt],
) -> Vec<FocusedBuildTask> {
    let existing_ids = existing_receipts
        .iter()
        .map(|receipt| receipt.id.clone())
        .collect::<BTreeSet<_>>();
    tasks
        .into_iter()
        .filter(|task| !existing_ids.contains(&task.id))
        .collect()
}

pub(crate) fn has_unreceipted_proof_request_tasks(
    proof_requests: &[ProofRequest],
    existing_receipts: &[ProofReceipt],
) -> bool {
    !unreceipted_focused_test_tasks(
        focused_test_candidates_from_requests(proof_requests),
        existing_receipts,
    )
    .is_empty()
        || !unreceipted_focused_build_tasks(
            focused_build_candidates_from_requests(proof_requests),
            existing_receipts,
        )
        .is_empty()
}

pub(crate) fn focused_test_resource_lease(
    task: &FocusedTestTask,
    budget: ProofBudget,
    lease_budget: ProofLeaseBudget,
    status: &str,
    reason: &str,
) -> ResourceLease {
    ResourceLease {
        schema: RESOURCE_LEASE_SCHEMA.to_owned(),
        id: format!("lease-{}", task.id),
        kind: "focused-test".to_owned(),
        consumer: task.id.clone(),
        status: status.to_owned(),
        reason: reason.to_owned(),
        cpu: lease_budget.cpu,
        memory_mb: lease_budget.memory_mb,
        disk_mb: lease_budget.disk_mb,
        timeout_sec: focused_test_task_command_timeout(task, budget)
            .saturating_mul(task.mode.command_count())
            .min(budget.max_total_seconds),
        network: lease_budget.network,
        scratch: lease_budget.scratch,
        worktree: if task.mode == FocusedProofMode::RedGreen {
            Some("base-plus-tests".to_owned())
        } else {
            None
        },
        command: Some(match task.mode {
            FocusedProofMode::HeadOnly => {
                format!("head: {}", proof_task_plan_command(task, "head", "head"))
            }
            FocusedProofMode::RedGreen => format!(
                "head: {}; base+tests: {}",
                proof_task_plan_command(task, "head", "head"),
                proof_task_plan_command(task, "base-plus-tests", "base-plus-tests")
            ),
        }),
    }
}

pub(crate) fn focused_build_resource_lease(
    task: &FocusedBuildTask,
    budget: ProofBudget,
    lease_budget: ProofLeaseBudget,
    status: &str,
    reason: &str,
) -> ResourceLease {
    ResourceLease {
        schema: RESOURCE_LEASE_SCHEMA.to_owned(),
        id: format!("lease-{}", task.id),
        kind: "focused-build".to_owned(),
        consumer: task.id.clone(),
        status: status.to_owned(),
        reason: reason.to_owned(),
        cpu: lease_budget.cpu,
        memory_mb: lease_budget.memory_mb,
        disk_mb: lease_budget.disk_mb,
        timeout_sec: focused_build_task_command_timeout(task, budget).min(budget.max_total_seconds),
        network: lease_budget.network,
        scratch: lease_budget.scratch,
        worktree: None,
        command: Some(format!("head: {}", task.command)),
    }
}
