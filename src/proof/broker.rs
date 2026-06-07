//! The proof broker: budgeted focused red/green and build execution,
//! base+tests worktrees, command receipts, and routed receipt evidence
//! (cleanup train step 8, pure code motion). Only the broker runs local
//! commands.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::*;

/// Bounded content excerpt for a routed proof receipt, or `None` for
/// non-receipt evidence kinds and unknown receipt ids. Each command
/// contributes its status line plus lossy-decoded byte tails of stderr and
/// stdout with explicit truncation markers, so follow-up packets stay
/// bounded no matter how loud the proof command was.
pub(crate) fn routed_proof_receipt_excerpt(
    out: &Path,
    evidence: &OrchestratorRoutedEvidence,
    proof_receipts: &[ProofReceipt],
) -> Option<String> {
    if evidence.kind != "proof-receipt" {
        return None;
    }
    let receipt = proof_receipts
        .iter()
        .find(|receipt| receipt.id == evidence.id)?;
    let mut excerpt = String::new();
    for command in &receipt.commands {
        excerpt.push_str(&format!(
            "  command `{}` side=`{}` status=`{}` exit={:?}\n",
            command.command, command.side, command.status, command.exit_code
        ));
        for (label, relative, cap) in [
            ("stderr", &command.stderr, ROUTED_RECEIPT_STDERR_TAIL_BYTES),
            ("stdout", &command.stdout, ROUTED_RECEIPT_STDOUT_TAIL_BYTES),
        ] {
            if relative.is_empty() {
                continue;
            }
            match fs::read(out.join(relative)) {
                Ok(bytes) if bytes.is_empty() => {
                    excerpt.push_str(&format!("    {label}: (empty)\n"));
                }
                Ok(bytes) => {
                    let truncated = bytes.len() > cap;
                    let mut start = bytes.len().saturating_sub(cap);
                    // Trim forward to a UTF-8 boundary so a mid-character
                    // byte cut cannot produce lossy-decode drift against the
                    // verifier's Python mirror of this excerpt.
                    while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
                        start += 1;
                    }
                    let tail = String::from_utf8_lossy(&bytes[start..]);
                    if truncated {
                        excerpt.push_str(&format!(
                            "    {label} (last {cap} bytes of {total}):\n",
                            total = bytes.len()
                        ));
                    } else {
                        excerpt.push_str(&format!("    {label}:\n"));
                    }
                    for line in tail.lines() {
                        excerpt.push_str("      ");
                        excerpt.push_str(line);
                        excerpt.push('\n');
                    }
                }
                Err(_) => {
                    // An unreadable stream is reported, never silently
                    // dropped: the model should know the excerpt is partial.
                    excerpt.push_str(&format!("    {label}: (unavailable at `{relative}`)\n"));
                }
            }
        }
    }
    (!excerpt.is_empty()).then_some(excerpt)
}

pub(crate) fn proof_receipt_routes_to_lanes(receipt: &ProofReceipt, lanes: &[String]) -> bool {
    receipt
        .requested_by
        .iter()
        .any(|lane| lane == "proof-broker")
        || receipt
            .requested_by
            .iter()
            .any(|lane| lanes.iter().any(|group_lane| group_lane == lane))
}

pub(crate) fn proof_receipt_routed_evidence(receipt: &ProofReceipt) -> OrchestratorRoutedEvidence {
    OrchestratorRoutedEvidence {
        schema: ORCHESTRATOR_ROUTED_EVIDENCE_SCHEMA.to_owned(),
        id: receipt.id.clone(),
        kind: "proof-receipt".to_owned(),
        artifact: "review/proof_receipts.json".to_owned(),
        status: routed_status_for_proof_receipt(receipt).to_owned(),
        result: receipt.result.clone(),
        reason: receipt.reason.clone(),
    }
}

pub(crate) fn resource_lease_routed_evidence(lease: &ResourceLease) -> OrchestratorRoutedEvidence {
    OrchestratorRoutedEvidence {
        schema: ORCHESTRATOR_ROUTED_EVIDENCE_SCHEMA.to_owned(),
        id: lease.id.clone(),
        kind: "resource-lease".to_owned(),
        artifact: "review/resource_leases.json".to_owned(),
        status: lease.status.clone(),
        result: lease.status.clone(),
        reason: lease.reason.clone(),
    }
}

pub(crate) fn proof_budget(profile: &Profile) -> Result<ProofBudget> {
    let budget = ProofBudget {
        max_focused_test_files: profile.budgets.proof_max_focused_test_files,
        max_focused_tests: profile.budgets.proof_max_focused_tests,
        per_command_timeout_sec: profile.budgets.proof_command_timeout_sec,
        max_total_seconds: profile.budgets.proof_total_timeout_sec,
    };
    if budget.max_focused_tests > 0 && budget.per_command_timeout_sec == 0 {
        bail!(
            "runtime profile {} has proof_command_timeout_sec=0 with focused proof enabled",
            profile.name
        );
    }
    if budget.max_focused_tests > 0 && budget.max_total_seconds == 0 {
        bail!(
            "runtime profile {} has proof_total_timeout_sec=0 with focused proof enabled",
            profile.name
        );
    }
    Ok(budget)
}

pub(crate) fn proof_lease_budget(profile: &Profile) -> Result<ProofLeaseBudget> {
    let budget = ProofLeaseBudget {
        cpu: profile.budgets.proof_cpu,
        memory_mb: profile.budgets.proof_memory_mb,
        disk_mb: profile.budgets.proof_disk_mb,
        network: profile.budgets.proof_network,
        scratch: profile.budgets.proof_scratch,
    };
    if profile.limits.tests > 0 && profile.budgets.proof_max_focused_tests > 0 {
        if budget.cpu == 0 {
            bail!(
                "runtime profile {} has proof_cpu=0 with focused proof enabled",
                profile.name
            );
        }
        if budget.memory_mb == 0 {
            bail!(
                "runtime profile {} has proof_memory_mb=0 with focused proof enabled",
                profile.name
            );
        }
        if budget.disk_mb == 0 {
            bail!(
                "runtime profile {} has proof_disk_mb=0 with focused proof enabled",
                profile.name
            );
        }
    }
    Ok(budget)
}

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
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
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
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
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

pub(crate) fn remaining_focused_proof_budget(
    mut budget: ProofBudget,
    existing_leases: &[ResourceLease],
) -> ProofBudget {
    let focused_leases = existing_leases
        .iter()
        .filter(|lease| focused_proof_lease_counts_budget(&lease.kind))
        .collect::<Vec<_>>();
    if focused_leases
        .iter()
        .any(|lease| lease.status == "exhausted")
    {
        budget.max_focused_test_files = 0;
        budget.max_focused_tests = 0;
        budget.max_total_seconds = 0;
        return budget;
    }

    let granted = focused_leases
        .iter()
        .filter(|lease| lease.status == "granted")
        .count();
    let granted_seconds = focused_leases
        .iter()
        .filter(|lease| lease.status == "granted")
        .map(|lease| lease.timeout_sec)
        .sum::<u64>();
    budget.max_focused_tests = budget.max_focused_tests.saturating_sub(granted);
    budget.max_focused_test_files = budget.max_focused_test_files.saturating_sub(granted);
    budget.max_total_seconds = budget.max_total_seconds.saturating_sub(granted_seconds);
    budget
}

pub(crate) fn focused_proof_lease_counts_budget(kind: &str) -> bool {
    matches!(kind, "focused-test" | "focused-build")
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
                "skipped_profile",
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
        leases.push(focused_test_resource_lease(
            &task,
            budget,
            lease_budget,
            "granted",
            "focused red/green proof lease granted by runtime profile",
        ));
        let receipt = match task.mode {
            FocusedProofMode::HeadOnly => {
                run_focused_head_proof_task(root, out, diff, &task, task_timeout_sec, &mut runner)?
            }
            FocusedProofMode::RedGreen => run_focused_red_green_proof_task(
                root,
                out,
                diff,
                &task,
                task_timeout_sec,
                &mut runner,
                &mut prepare_base_plus_tests,
            )?,
        };
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

pub(crate) fn focused_build_budget_allows_next(
    current_tasks: usize,
    estimated_seconds: u64,
    next_timeout_sec: u64,
    budget: ProofBudget,
) -> bool {
    current_tasks < budget.max_focused_tests
        && estimated_seconds.saturating_add(next_timeout_sec) <= budget.max_total_seconds
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

pub(crate) fn focused_test_candidates_from_diff(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
) -> Vec<FocusedTestTask> {
    let request_groups = proof_request_groups(proof_requests);
    let mut tasks = Vec::new();
    for file in diff
        .changed_files
        .iter()
        .filter(|path| is_bun_focused_test_file(path))
    {
        let names = focused_test_names_for_file(&diff.patch, file);
        if names.is_empty() {
            merge_focused_test_task(
                &mut tasks,
                focused_test_task_with_mode(
                    file,
                    None,
                    FocusedProofMode::RedGreen,
                    &request_groups,
                ),
            );
        } else {
            for name in names {
                merge_focused_test_task(
                    &mut tasks,
                    focused_test_task_with_mode(
                        file,
                        Some(name),
                        FocusedProofMode::RedGreen,
                        &request_groups,
                    ),
                );
            }
        }
    }
    merge_focused_test_request_group_tasks(&mut tasks, &request_groups);
    tasks
}

pub(crate) fn focused_test_candidates_from_requests(
    proof_requests: &[ProofRequest],
) -> Vec<FocusedTestTask> {
    let request_groups = proof_request_groups(proof_requests);
    let mut tasks = Vec::new();
    merge_focused_test_request_group_tasks(&mut tasks, &request_groups);
    tasks
}

pub(crate) fn focused_build_candidates_from_requests(
    proof_requests: &[ProofRequest],
) -> Vec<FocusedBuildTask> {
    let request_groups = proof_request_groups(proof_requests);
    let mut tasks = Vec::new();
    for group in &request_groups {
        let Some(task) = focused_build_task_from_request_group(group) else {
            continue;
        };
        merge_focused_build_task(&mut tasks, task);
    }
    tasks
}

pub(crate) fn focused_proof_budget_allows_next(
    current_tasks: usize,
    current_files: &BTreeSet<String>,
    next_file: &str,
    estimated_seconds: u64,
    next_timeout_sec: u64,
    next_command_count: u64,
    budget: ProofBudget,
) -> bool {
    current_tasks < budget.max_focused_tests
        && (current_files.contains(next_file)
            || current_files.len() < budget.max_focused_test_files)
        && estimated_seconds
            .saturating_add(next_timeout_sec)
            .saturating_add(next_timeout_sec.saturating_mul(next_command_count.saturating_sub(1)))
            <= budget.max_total_seconds
}

pub(crate) fn focused_test_task_command_timeout(
    task: &FocusedTestTask,
    budget: ProofBudget,
) -> u64 {
    task.timeout_sec
        .filter(|timeout| *timeout > 0)
        .unwrap_or(budget.per_command_timeout_sec)
        .min(budget.per_command_timeout_sec)
}

pub(crate) fn focused_build_task_command_timeout(
    task: &FocusedBuildTask,
    budget: ProofBudget,
) -> u64 {
    task.timeout_sec.max(1).min(budget.per_command_timeout_sec)
}

pub(crate) fn prepare_base_plus_tests_worktree(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
) -> Result<PathBuf> {
    let patch_files = base_plus_tests_patch_files(diff);
    let worktrees_dir = out.join("proof-worktrees");
    fs::create_dir_all(&worktrees_dir)
        .with_context(|| format!("create {}", worktrees_dir.display()))?;
    let worktree = worktrees_dir.join("base-plus-tests");
    if worktree.exists() {
        let _ = cleanup_base_plus_tests_worktree(root, &worktree);
        if worktree.exists() {
            safe_remove_dir_all_under(&worktrees_dir, &worktree)?;
        }
    }

    let add_args = vec![
        "worktree".to_owned(),
        "add".to_owned(),
        "--detach".to_owned(),
        worktree.to_string_lossy().to_string(),
        diff.base.clone(),
    ];
    git_text_owned(root, &add_args).with_context(|| {
        format!(
            "create base+tests worktree at {} from {}",
            worktree.display(),
            diff.base
        )
    })?;

    if !patch_files.is_empty() {
        let patch = base_plus_tests_patch(root, diff, &patch_files)?;
        let proof_dir = out.join("proof");
        fs::create_dir_all(&proof_dir)
            .with_context(|| format!("create {}", proof_dir.display()))?;
        let patch_path = proof_dir.join("base-plus-tests.patch");
        fs::write(&patch_path, patch).with_context(|| format!("write {}", patch_path.display()))?;

        let apply_args = vec![
            "apply".to_owned(),
            "--whitespace=nowarn".to_owned(),
            patch_path.to_string_lossy().to_string(),
        ];
        if let Err(error) = git_text_owned(&worktree, &apply_args)
            .with_context(|| format!("apply test-only patch in {}", worktree.display()))
        {
            let _ = cleanup_base_plus_tests_worktree(root, &worktree);
            return Err(error);
        }
    }

    Ok(worktree)
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use anyhow::Result;

    use crate::tests::{run_test_command, test_diff, test_proof_receipt, test_run_args};
    use crate::*;

    #[test]
    fn unreceipted_proof_request_tasks_skip_already_receipted_seeded_request() -> Result<()> {
        let proof_requests = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-policy-001".to_owned(),
            lane: "intelligent-ci-policy".to_owned(),
            requested_by: vec![
                "intelligent-ci-policy".to_owned(),
                "proof-policy:required-smoke".to_owned(),
            ],
            command: "cargo test --locked required_proof_smoke".to_owned(),
            reason: "Required Rust focused smoke for intelligent CI.".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: true,
            status: "requested".to_owned(),
        }];

        assert!(super::has_unreceipted_proof_request_tasks(
            &proof_requests,
            &[]
        ));

        let task = super::focused_test_candidates_from_requests(&proof_requests)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("focused request should produce a task"))?;
        let mut receipt = test_proof_receipt("head_passed", "passed");
        receipt.id = task.id;
        receipt.requested_by = task.requested_by;
        receipt.request_ids = task.request_ids;

        assert!(!super::has_unreceipted_proof_request_tasks(
            &proof_requests,
            &[receipt]
        ));
        Ok(())
    }

    #[test]
    fn proof_broker_v0_executes_allowlisted_request_as_red_green_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "USE_SYSTEM_BUN=1 bun test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'"
                    .to_owned(),
                reason: "Run the requested focused Bun proof.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 120,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-opposition-001".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command:
                    "bun bd test test/js/bun/ffi/ffi.test.js --test-name-pattern \"ffi toBuffer bad free\""
                        .to_owned(),
                reason: "Confirm the same requested focused Bun proof.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 180,
                required: true,
                status: "requested".to_owned(),
            },
        ];
        let tasks = focused_test_tasks_from_diff(
            &diff,
            &proof_requests,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].mode, super::FocusedProofMode::RedGreen);
        assert_eq!(tasks[0].file, "test/js/bun/ffi/ffi.test.js");
        assert_eq!(tasks[0].test_name.as_deref(), Some("ffi toBuffer bad free"));
        assert_eq!(tasks[0].timeout_sec, Some(180));
        assert_eq!(tasks[0].requested_by.len(), 2);
        assert!(
            tasks[0]
                .requested_by
                .iter()
                .any(|lane| lane == "tests-oracle")
        );
        assert!(
            tasks[0]
                .requested_by
                .iter()
                .any(|lane| lane == "opposition")
        );
        assert_eq!(tasks[0].request_ids.len(), 2);
        assert!(
            tasks[0]
                .request_ids
                .iter()
                .any(|request_id| request_id == "proof-tests-001")
        );
        assert!(
            tasks[0]
                .request_ids
                .iter()
                .any(|request_id| request_id == "proof-opposition-001")
        );

        let args = test_run_args(out.clone());
        let mut commands = Vec::<String>::new();
        let prepared_base_root = base_root.clone();
        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, timeout, stdout, stderr| {
                commands.push(super::command_display_with_env(env, argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                assert_eq!(timeout, 180);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 21,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(
            commands,
            vec![
                "bun bd test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'",
                "USE_SYSTEM_BUN=1 bun test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'",
            ]
        );
        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        let receipt = &proof_result.proof_receipts[0];
        assert_eq!(receipt.kind, "focused-red-green");
        assert_eq!(receipt.test_patch_mode, "base-plus-tests");
        assert_eq!(receipt.result, "discriminating");
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].side, "head");
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].side, "base-plus-tests");
        assert_eq!(receipt.commands[1].status, "failed");
        assert!(out.join(&receipt.commands[0].stdout).exists());
        assert!(out.join(&receipt.commands[1].stdout).exists());
        let lease = &proof_result.resource_leases[0];
        assert_eq!(lease.status, "granted");
        assert_eq!(lease.timeout_sec, 360);
        assert_eq!(lease.worktree, Some("base-plus-tests".to_owned()));
        assert!(lease.command.as_deref().is_some_and(
            |command| command.contains("head: cwd=") && command.contains("base+tests:")
        ));
        Ok(())
    }

    #[test]
    fn proof_broker_v0_executes_allowlisted_cargo_test_request_as_red_green_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let command =
            "cargo test --locked -p ub-review cargo_focused_red_green -- --exact".to_owned();
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: command.clone(),
                reason: "Run the requested focused Cargo proof.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-opposition-001".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command: command.clone(),
                reason: "Confirm the same requested focused Cargo proof.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: true,
                status: "requested".to_owned(),
            },
        ];
        let tasks = super::focused_test_candidates_from_requests(&proof_requests);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].mode, super::FocusedProofMode::RedGreen);
        assert_eq!(tasks[0].file, "cargo-package:ub-review");
        assert_eq!(
            tasks[0].test_name.as_deref(),
            Some("cargo_focused_red_green")
        );
        assert!(tasks[0].command_specs.is_some());
        assert_eq!(tasks[0].requested_by.len(), 2);
        assert_eq!(tasks[0].request_ids.len(), 2);

        let args = test_run_args(out.clone());
        let mut commands = Vec::<String>::new();
        let prepared_base_root = base_root.clone();
        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, timeout, stdout, stderr| {
                commands.push(super::command_display_with_env(env, argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert!(env.is_empty());
                assert_eq!(timeout, 300);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 21,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(commands, vec![command.clone(), command]);
        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        let receipt = &proof_result.proof_receipts[0];
        assert_eq!(receipt.kind, "focused-red-green");
        assert_eq!(receipt.test_patch_mode, "base-plus-tests");
        assert_eq!(receipt.result, "discriminating");
        assert_eq!(
            receipt.requested_by,
            vec!["tests-oracle".to_owned(), "opposition".to_owned()]
        );
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].side, "head");
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].side, "base-plus-tests");
        assert_eq!(receipt.commands[1].status, "failed");
        assert!(out.join(&receipt.commands[0].stdout).exists());
        assert!(out.join(&receipt.commands[1].stdout).exists());
        let lease = &proof_result.resource_leases[0];
        assert_eq!(lease.kind, "focused-test");
        assert_eq!(lease.status, "granted");
        assert_eq!(lease.timeout_sec, 600);
        assert_eq!(lease.worktree, Some("base-plus-tests".to_owned()));
        Ok(())
    }

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
        let tasks = super::focused_build_candidates_from_requests(&proof_requests);
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
                commands.push(super::command_display_with_env(env, argv));
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
    fn proof_budget_comes_from_runtime_profile_budgets() -> Result<()> {
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;
        let cx23 = profiles
            .iter()
            .find(|profile| profile.name == "cx23")
            .ok_or_else(|| anyhow::anyhow!("missing cx23 profile"))?;
        let cx43 = profiles
            .iter()
            .find(|profile| profile.name == "cx43")
            .ok_or_else(|| anyhow::anyhow!("missing cx43 profile"))?;

        assert_eq!(proof_budget(gh_runner)?.max_focused_tests, 1);
        assert_eq!(proof_budget(cx23)?.max_focused_tests, 2);
        assert_eq!(proof_budget(cx43)?.max_focused_tests, 6);
        assert_eq!(proof_budget(cx43)?.per_command_timeout_sec, 600);
        assert_eq!(proof_budget(cx43)?.max_total_seconds, 1_800);
        Ok(())
    }

    #[test]
    fn invalid_enabled_proof_budget_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_command_timeout_sec: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let err = proof_budget(&profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid proof budget unexpectedly passed"))?;

        assert!(
            err.to_string()
                .contains("runtime profile broken has proof_command_timeout_sec=0")
        );
        Ok(())
    }

    #[test]
    fn proof_lease_budget_comes_from_runtime_profile_budgets() -> Result<()> {
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;
        let cx23 = profiles
            .iter()
            .find(|profile| profile.name == "cx23")
            .ok_or_else(|| anyhow::anyhow!("missing cx23 profile"))?;
        let cx43 = profiles
            .iter()
            .find(|profile| profile.name == "cx43")
            .ok_or_else(|| anyhow::anyhow!("missing cx43 profile"))?;

        assert_eq!(proof_lease_budget(gh_runner)?.cpu, 2);
        assert_eq!(proof_lease_budget(gh_runner)?.memory_mb, 2_048);
        assert_eq!(proof_lease_budget(gh_runner)?.disk_mb, 1_024);
        assert_eq!(proof_lease_budget(cx23)?.cpu, 1);
        assert_eq!(proof_lease_budget(cx23)?.memory_mb, 1_024);
        assert_eq!(proof_lease_budget(cx43)?.cpu, 4);
        assert_eq!(proof_lease_budget(cx43)?.disk_mb, 2_048);
        Ok(())
    }

    #[test]
    fn invalid_enabled_proof_lease_budget_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_cpu: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let err = proof_lease_budget(&profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid proof lease budget unexpectedly passed"))?;

        assert!(
            err.to_string()
                .contains("runtime profile broken has proof_cpu=0")
        );
        Ok(())
    }

    #[test]
    fn proof_broker_v0_runs_budgeted_focused_red_green_targets_and_writes_receipts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let patch = "\
diff --git a/test/js/bun/md/md-edge-cases.test.ts b/test/js/bun/md/md-edge-cases.test.ts
index 1111111..2222222 100644
--- a/test/js/bun/md/md-edge-cases.test.ts
+++ b/test/js/bun/md/md-edge-cases.test.ts
@@ -1,2 +1,4 @@
 import { test } from 'bun:test';
+test(\"snapshots resizable ArrayBuffer input\", () => {});
+it('keeps stable bytes after getter reentry', () => {});
";
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["test/js/bun/md/md-edge-cases.test.ts".to_owned()],
            patch: patch.to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        };
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Need green witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-opposition-001".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Confirm the same focused test.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: true,
                status: "requested".to_owned(),
            },
        ];
        let tasks = focused_test_tasks_from_diff(
            &diff,
            &proof_requests,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 1_200,
            },
        );
        assert_eq!(tasks.len(), 2);
        let args = test_run_args(out.clone());
        let mut commands = Vec::<String>::new();
        let prepared_base_root = base_root.clone();
        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 1_200,
            },
            tasks,
            |_root, argv, env, timeout, stdout, stderr| {
                commands.push(command_display(argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: format!("completed with timeout {timeout}s"),
                    duration_ms: 42,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;
        let receipts = proof_result.proof_receipts;
        let resource_leases = proof_result.resource_leases;

        assert_eq!(commands.len(), 4);
        assert_eq!(receipts.len(), 2);
        assert_eq!(resource_leases.len(), 2);
        assert_eq!(resource_leases[0].schema, "ub-review.resource_lease.v1");
        assert_eq!(resource_leases[0].kind, "focused-test");
        assert_eq!(resource_leases[0].consumer, receipts[0].id);
        assert_eq!(resource_leases[0].status, "granted");
        assert_eq!(resource_leases[1].consumer, receipts[1].id);
        assert_eq!(resource_leases[1].status, "granted");
        assert_eq!(receipts[0].schema, "ub-review.proof_receipt.v1");
        assert_eq!(receipts[0].kind, "focused-red-green");
        assert_eq!(receipts[0].test_patch_mode, "base-plus-tests");
        assert_eq!(receipts[0].result, "discriminating");
        assert_eq!(
            receipts[0].requested_by,
            vec!["tests-oracle".to_owned(), "opposition".to_owned()]
        );
        assert_eq!(
            receipts[0].request_ids,
            vec![
                "proof-tests-001".to_owned(),
                "proof-opposition-001".to_owned()
            ]
        );
        assert_eq!(receipts[0].commands[0].status, "passed");
        assert_eq!(receipts[0].commands[1].side, "base-plus-tests");
        assert_eq!(receipts[0].commands[1].status, "failed");
        assert!(out.join(&receipts[0].commands[0].stdout).exists());
        assert!(out.join(&receipts[0].commands[1].stdout).exists());
        assert_eq!(receipts[1].result, "discriminating");
        assert_eq!(receipts[1].commands.len(), 2);
        assert_eq!(receipts[1].commands[0].status, "passed");
        assert_eq!(receipts[1].commands[1].status, "failed");

        write_proof_receipt_artifacts(&out, &receipts)?;
        write_resource_lease_artifacts(&out, &resource_leases)?;
        write_proof_request_artifacts(
            &out,
            &diff,
            &Profile::default(),
            &proof_requests,
            &receipts,
        )?;
        let receipt_json: Vec<ProofReceipt> =
            serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
        let receipt_ndjson = fs::read_to_string(out.join("proof_receipts.ndjson"))?;
        let lease_json: Vec<ResourceLease> =
            serde_json::from_slice(&fs::read(out.join("review/resource_leases.json"))?)?;
        let lease_ndjson = fs::read_to_string(out.join("resource_leases.ndjson"))?;
        let resource_plan = fs::read_to_string(out.join("review/resource_plan.md"))?;
        let proof_plan = fs::read_to_string(out.join("review/proof_plan.md"))?;
        assert_eq!(receipt_json.len(), 2);
        assert_eq!(receipt_ndjson.lines().count(), 2);
        assert_eq!(lease_json.len(), 2);
        assert_eq!(lease_ndjson.lines().count(), 2);
        assert!(resource_plan.contains("# Resource lease plan"));
        assert!(resource_plan.contains("status=`granted`"));
        assert!(!resource_plan.contains("status=`exhausted`"));
        assert!(
            proof_plan.contains("Proof broker v0 executed focused proof under the runtime budget")
        );
        assert!(proof_plan.contains("result=`discriminating`"));
        assert!(!proof_plan.contains("No proof broker commands were executed"));
        Ok(())
    }

    #[test]
    fn follow_up_proof_broker_executes_request_only_focused_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["src/lib.rs".to_owned()],
            patch: "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 pub fn route() {}
+pub fn patched_route() {}
"
            .to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::SourceGeneral,
        };
        let proof_requests = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-follow-up-001".to_owned(),
            lane: "orchestrator-follow-up-route-proof".to_owned(),
            requested_by: vec!["orchestrator-follow-up-route-proof".to_owned()],
            command: "bun test test/js/bun/fs/fs.write.test.ts -t route".to_owned(),
            reason: "Follow-up asked for a route witness.".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "requested".to_owned(),
        }];
        let tasks = super::focused_test_candidates_from_requests(&proof_requests);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].file, "test/js/bun/fs/fs.write.test.ts");
        assert_eq!(tasks[0].request_ids, vec!["proof-follow-up-001"]);

        let args = test_run_args(out.clone());
        let prepared_base_root = base_root.clone();
        let mut commands = Vec::<String>::new();
        let proof_result = super::run_follow_up_proof_broker_v0_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, _timeout, stdout, stderr| {
                commands.push(command_display(argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 42,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(commands.len(), 2);
        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.resource_leases[0].status, "granted");
        let receipt = &proof_result.proof_receipts[0];
        assert_eq!(receipt.kind, "focused-red-green");
        assert_eq!(receipt.result, "discriminating");
        assert_eq!(
            receipt.requested_by,
            vec!["orchestrator-follow-up-route-proof".to_owned()]
        );
        assert_eq!(receipt.request_ids, vec!["proof-follow-up-001"]);
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].status, "failed");
        Ok(())
    }

    #[test]
    fn follow_up_proof_broker_uses_remaining_focused_proof_budget() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let proof_requests = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-follow-up-002".to_owned(),
            lane: "orchestrator-follow-up-tests-proof".to_owned(),
            requested_by: vec!["orchestrator-follow-up-tests-proof".to_owned()],
            command: "bun test test/js/bun/md/md-edge-cases.test.ts -t snapshots".to_owned(),
            reason: "Follow-up asked for a second proof.".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "requested".to_owned(),
        }];
        let existing_leases = vec![ResourceLease {
            schema: "ub-review.resource_lease.v1".to_owned(),
            id: "lease-proof-red-green-existing".to_owned(),
            kind: "focused-test".to_owned(),
            consumer: "proof-red-green-existing".to_owned(),
            status: "granted".to_owned(),
            reason: "focused red/green proof lease granted by runtime profile".to_owned(),
            cpu: 2,
            memory_mb: 2048,
            disk_mb: 1024,
            timeout_sec: 600,
            network: false,
            scratch: true,
            worktree: Some("base-plus-tests".to_owned()),
            command: Some("head: bun bd test existing; base+tests: bun test existing".to_owned()),
        }];
        let remaining = super::remaining_focused_proof_budget(
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            &existing_leases,
        );
        assert_eq!(remaining.max_focused_tests, 0);
        assert_eq!(remaining.max_focused_test_files, 2);
        assert_eq!(remaining.max_total_seconds, 0);

        let args = test_run_args(out.clone());
        let tasks = super::focused_test_candidates_from_requests(&proof_requests);
        let mut commands = Vec::<String>::new();
        let proof_result = super::run_follow_up_proof_broker_v0_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            remaining,
            tasks,
            |_root, argv, _env, _timeout, _stdout, _stderr| {
                commands.push(command_display(argv));
                Ok(CommandStatus {
                    exit_code: Some(0),
                    timed_out: false,
                    success: true,
                    reason: "should not run".to_owned(),
                    duration_ms: 0,
                })
            },
            |_root, _out, _diff| {
                unreachable!("base+tests worktree should not be prepared after budget is spent")
            },
        )?;

        assert!(commands.is_empty());
        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.proof_receipts[0].result, "skipped_budget");
        assert_eq!(
            proof_result.proof_receipts[0].request_ids,
            vec!["proof-follow-up-002"]
        );
        assert_eq!(proof_result.resource_leases[0].status, "exhausted");
        assert_eq!(
            proof_result.resource_leases[0].reason,
            "focused red/green proof lease budget exhausted by runtime profile"
        );
        Ok(())
    }

    #[test]
    fn proof_broker_v0_exhausts_focused_tests_after_runtime_budget() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let tasks = vec![
            super::focused_test_task(
                "test/js/bun/md/md-edge-cases.test.ts",
                Some("snapshots input".to_owned()),
                &[] as &[ProofRequestGroup],
            ),
            super::focused_test_task(
                "test/js/bun/md/md-edge-cases.test.ts",
                Some("getter reentry".to_owned()),
                &[] as &[ProofRequestGroup],
            ),
        ];
        let args = test_run_args(out.clone());
        let prepared_base_root = base_root.clone();
        let mut commands = Vec::<String>::new();

        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, _timeout, stdout, stderr| {
                commands.push(command_display(argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 42,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(commands.len(), 2);
        assert_eq!(proof_result.proof_receipts.len(), 2);
        assert_eq!(proof_result.proof_receipts[0].result, "discriminating");
        assert_eq!(proof_result.proof_receipts[1].result, "skipped_budget");
        assert_eq!(proof_result.proof_receipts[1].commands[0].status, "skipped");
        assert_eq!(proof_result.resource_leases.len(), 2);
        assert_eq!(proof_result.resource_leases[0].status, "granted");
        assert_eq!(proof_result.resource_leases[1].status, "exhausted");
        assert_eq!(
            proof_result.resource_leases[1].reason,
            "focused red/green proof lease budget exhausted by runtime profile"
        );
        Ok(())
    }

    #[test]
    fn proof_broker_v0_records_candidates_beyond_focused_file_budget() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let patch = "\
diff --git a/test/js/bun/md/md-edge-cases.test.ts b/test/js/bun/md/md-edge-cases.test.ts
index 1111111..2222222 100644
--- a/test/js/bun/md/md-edge-cases.test.ts
+++ b/test/js/bun/md/md-edge-cases.test.ts
@@ -1,2 +1,3 @@
 import { test } from 'bun:test';
+test(\"snapshots resizable ArrayBuffer input\", () => {});
diff --git a/test/js/bun/ffi/ffi.test.js b/test/js/bun/ffi/ffi.test.js
index 3333333..4444444 100644
--- a/test/js/bun/ffi/ffi.test.js
+++ b/test/js/bun/ffi/ffi.test.js
@@ -1,2 +1,3 @@
 import { test } from 'bun:test';
+test(\"no-finalizer toBuffer keeps caller memory alive\", () => {});
";
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec![
                "test/js/bun/md/md-edge-cases.test.ts".to_owned(),
                "test/js/bun/ffi/ffi.test.js".to_owned(),
            ],
            patch: patch.to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        };
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-md-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Need md red/green witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-ffi-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/ffi/ffi.test.js -t 'no-finalizer toBuffer keeps caller memory alive'".to_owned(),
                reason: "Need ffi red/green witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
        ];
        let tasks = super::focused_test_candidates_from_diff(&diff, &proof_requests);
        assert_eq!(tasks.len(), 2);

        let args = test_run_args(out.clone());
        let prepared_base_root = base_root.clone();
        let mut commands = Vec::<String>::new();
        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 1,
                max_focused_tests: 6,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, _timeout, stdout, stderr| {
                commands.push(command_display(argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 42,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(commands.len(), 2);
        assert_eq!(proof_result.proof_receipts.len(), 2);
        assert_eq!(proof_result.resource_leases.len(), 2);
        assert_eq!(proof_result.proof_receipts[0].result, "discriminating");
        assert_eq!(
            proof_result.proof_receipts[0].request_ids,
            vec!["proof-md-001"]
        );
        assert_eq!(proof_result.resource_leases[0].status, "granted");
        assert_eq!(
            proof_result.resource_leases[0].consumer,
            proof_result.proof_receipts[0].id
        );
        assert_eq!(proof_result.proof_receipts[1].result, "skipped_budget");
        assert_eq!(
            proof_result.proof_receipts[1].request_ids,
            vec!["proof-ffi-001"]
        );
        assert_eq!(proof_result.proof_receipts[1].commands[0].status, "skipped");
        assert_eq!(proof_result.resource_leases[1].status, "exhausted");
        assert_eq!(
            proof_result.resource_leases[1].consumer,
            proof_result.proof_receipts[1].id
        );
        assert!(
            proof_result.proof_receipts[1]
                .reason
                .contains("lease budget exhausted"),
            "unexpected skipped-budget reason: {}",
            proof_result.proof_receipts[1].reason
        );
        Ok(())
    }

    #[test]
    fn proof_broker_v0_marks_base_plus_tests_pass_as_non_discriminating() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let task = super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        );
        let mut runner_calls = 0;
        let mut prepare_calls = 0;
        let mut runner = |_root: &Path,
                          _argv: &[String],
                          _env: &BTreeMap<String, String>,
                          _timeout: u64,
                          stdout: &Path,
                          stderr: &Path|
         -> Result<CommandStatus> {
            runner_calls += 1;
            fs::write(stdout, b"ok\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(0),
                timed_out: false,
                success: true,
                reason: "completed".to_owned(),
                duration_ms: 7,
            })
        };
        let prepared_base_root = base_root.clone();
        let mut prepare = |_root: &Path, _out: &Path, _diff: &DiffContext| -> Result<_> {
            prepare_calls += 1;
            Ok(prepared_base_root.clone())
        };

        let receipt = super::run_focused_red_green_proof_task(
            temp.path(),
            &out,
            &diff,
            &task,
            300,
            &mut runner,
            &mut prepare,
        )?;

        assert_eq!(runner_calls, 2);
        assert_eq!(prepare_calls, 1);
        assert_eq!(receipt.kind, "focused-red-green");
        assert_eq!(receipt.result, "non_discriminating");
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].status, "passed");
        Ok(())
    }

    #[test]
    fn proof_broker_v0_skips_base_plus_tests_when_head_fails() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let task = super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        );
        let mut prepare_calls = 0;
        let mut runner = |_root: &Path,
                          _argv: &[String],
                          _env: &BTreeMap<String, String>,
                          _timeout: u64,
                          stdout: &Path,
                          stderr: &Path|
         -> Result<CommandStatus> {
            fs::write(stdout, b"failed\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(1),
                timed_out: false,
                success: false,
                reason: "exit code Some(1)".to_owned(),
                duration_ms: 7,
            })
        };
        let mut prepare = |_root: &Path, _out: &Path, _diff: &DiffContext| -> Result<_> {
            prepare_calls += 1;
            Ok(temp.path().join("base-plus-tests"))
        };

        let receipt = super::run_focused_red_green_proof_task(
            temp.path(),
            &out,
            &diff,
            &task,
            300,
            &mut runner,
            &mut prepare,
        )?;

        assert_eq!(prepare_calls, 0);
        assert_eq!(receipt.result, "head_failed");
        assert_eq!(receipt.commands.len(), 1);
        assert_eq!(receipt.commands[0].status, "failed");
        Ok(())
    }

    #[test]
    fn proof_broker_v0_records_base_patch_failed_as_missing_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let task = super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        );
        let mut runner = |_root: &Path,
                          _argv: &[String],
                          _env: &BTreeMap<String, String>,
                          _timeout: u64,
                          stdout: &Path,
                          stderr: &Path|
         -> Result<CommandStatus> {
            fs::write(stdout, b"head ok\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(0),
                timed_out: false,
                success: true,
                reason: "completed".to_owned(),
                duration_ms: 7,
            })
        };
        let mut prepare = |_root: &Path, _out: &Path, _diff: &DiffContext| -> Result<PathBuf> {
            Err(anyhow::anyhow!("patch did not apply"))
        };

        let receipt = super::run_focused_red_green_proof_task(
            temp.path(),
            &out,
            &diff,
            &task,
            300,
            &mut runner,
            &mut prepare,
        )?;

        assert_eq!(receipt.result, "base_patch_failed");
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].side, "base-plus-tests");
        assert_eq!(receipt.commands[1].status, "skipped");
        assert!(super::proof_receipt_is_missing_evidence(&receipt));
        Ok(())
    }

    #[test]
    fn proof_broker_v0_does_not_execute_without_focused_test_lease() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let tasks = vec![super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        )];
        let args = test_run_args(out.clone());
        let mut profile = Profile::default();
        profile.limits.tests = 0;

        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &profile,
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, _argv, _env, _timeout, _stdout, _stderr| {
                unreachable!("proof command should not run without a lease")
            },
            |_root, _out, _diff| {
                unreachable!("base+tests worktree should not be prepared without a lease")
            },
        )?;

        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.proof_receipts[0].result, "skipped_profile");
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.resource_leases[0].status, "skipped_profile");
        assert_eq!(
            proof_result.resource_leases[0].reason,
            "profile allows zero focused test leases"
        );
        Ok(())
    }

    #[test]
    fn proof_broker_v0_does_not_execute_when_focused_test_budget_is_zero() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let tasks = vec![super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        )];
        let args = test_run_args(out.clone());

        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 0,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, _argv, _env, _timeout, _stdout, _stderr| {
                unreachable!("proof command should not run when proof budget is zero")
            },
            |_root, _out, _diff| {
                unreachable!("base+tests worktree should not be prepared when proof budget is zero")
            },
        )?;

        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.proof_receipts[0].result, "skipped_budget");
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.resource_leases[0].status, "exhausted");
        Ok(())
    }

    #[test]
    fn prepare_base_plus_tests_worktree_allows_source_only_request_without_test_patch() -> Result<()>
    {
        let repo = tempfile::tempdir()?;
        fs::create_dir_all(repo.path().join("src"))?;
        fs::write(repo.path().join("src/lib.rs"), "pub fn current() {}\n")?;
        run_test_command(repo.path(), "git", &["init"])?;
        run_test_command(
            repo.path(),
            "git",
            &["config", "user.email", "ub-review@example.invalid"],
        )?;
        run_test_command(
            repo.path(),
            "git",
            &["config", "user.name", "UB Review Test"],
        )?;
        run_test_command(repo.path(), "git", &["add", "."])?;
        run_test_command(
            repo.path(),
            "git",
            &["-c", "commit.gpgsign=false", "commit", "-m", "initial"],
        )?;

        let out = tempfile::tempdir()?;
        let diff = DiffContext {
            base: "HEAD".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["src/lib.rs".to_owned()],
            patch: "+pub fn changed() {}\n".to_owned(),
            flags: DiffFlags {
                source_changed: true,
                rust_changed: true,
                rust_tests_changed: false,
                workflow_changed: false,
                dependency_changed: false,
                shell_changed: false,
                cpp_changed: false,
                docs_only: false,
                unsafe_or_native_risk: true,
            },
            diff_class: DiffClass::SourceUb,
        };
        assert!(super::base_plus_tests_patch_files(&diff).is_empty());

        let worktree = super::prepare_base_plus_tests_worktree(repo.path(), out.path(), &diff)?;

        assert!(worktree.join("src/lib.rs").exists());
        assert!(!out.path().join("proof/base-plus-tests.patch").exists());
        super::cleanup_base_plus_tests_worktree(repo.path(), &worktree)?;
        Ok(())
    }
}
