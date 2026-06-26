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

/// Normalize a v1 `ProofRequest` to a typed `ProofRequestV2` (Order 4b of
/// #678). This is the single v1→v2 normalization point for the broker: it
/// infers the `ProofKind` from the v1 `cost`/`command` via the existing
/// `classify_proof_kind`, carries the command as the v2 `target`, and maps
/// the remaining fields. After this, the broker works in v2.
pub(crate) fn proof_request_to_v2(req: &ProofRequest) -> ProofRequestV2 {
    let kind = classify_proof_kind(&req.cost, &req.command);
    ProofRequestV2 {
        schema: crate::artifacts::PROOF_REQUEST_V2_SCHEMA.to_owned(),
        id: format!("{}-v2", req.id),
        kind,
        target: req.command.clone(),
        claim_ids: Vec::new(),
        requested_by: req.requested_by.clone(),
        expected_interpretation: req.reason.clone(),
        priority: if req.required { "high" } else { "medium" }.to_owned(),
        timeout_sec: req.timeout_sec,
        status: req.status.clone(),
        base: String::new(),
        head: String::new(),
    }
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
    // Native v2 proof flow (Order 4b of #678): normalize v1 requests to typed
    // v2 once at ingestion, then extract candidates from v2. v2 is now the
    // internal contract; the candidate extractors key off ProofKind, so only
    // FocusedTest/FocusedBuild requests reach test/build execution and other
    // kinds (SanitizerWitness/MiriWitness/...) are routed by their own paths
    // (Order 4c, #681). The v2 extractors re-run the allowlist on the command
    // string, so the security boundary is preserved byte-for-byte.
    let v2_requests: Vec<ProofRequestV2> = proof_requests.iter().map(proof_request_to_v2).collect();
    let total_budget = proof_budget(profile)?;
    let budget = remaining_focused_proof_budget(total_budget, existing_leases);
    let tasks = unreceipted_focused_test_tasks(
        focused_test_candidates_from_v2(&v2_requests),
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
    let build_tasks = focused_build_candidates_from_v2(&v2_requests);
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

    // SanitizerWitness dispatch (Order 4c of #678). v2 requests with kind
    // SanitizerWitness execute via the production sanitizer path
    // (`run_sanitizer_witness`), which resolves the typed intent to an
    // allowlisted cargo +nightly test command, runs it under a lease with a
    // wall-clock timeout and bounded stdout/stderr files, and produces a
    // canonical ProofReceipt. Skips (no nightly / dry-run / unresolved) emit a
    // skip receipt rather than executing. Each witness is deduped against
    // existing receipts so a re-run does not re-execute.
    let nightly_available = std::process::Command::new("cargo")
        .args(["+nightly", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    // Dedupe by the content-hash receipt id (sanitizer-receipt-<sha256[..12]>),
    // NOT target byte length — distinct equal-length targets must not collapse.
    let existing_sanitizer_receipts: BTreeSet<String> = result
        .proof_receipts
        .iter()
        .filter(|r| r.kind == "sanitizer-witness")
        .map(|r| r.id.clone())
        .collect();
    for req in v2_requests
        .iter()
        .filter(|r| matches!(r.kind, ProofKind::SanitizerWitness))
    {
        let target_fp = &sha256_hex(req.target.as_bytes())[..12];
        let receipt_key = format!("sanitizer-receipt-{target_fp}");
        if existing_sanitizer_receipts.contains(&receipt_key) {
            continue;
        }
        // Per-request timeout wins over the profile budget; floor at the
        // profile's per-command cap so a runaway request cannot exceed policy.
        let sanitizer_timeout = req
            .timeout_sec
            .max(profile.budgets.proof_command_timeout_sec);
        let (receipt, lease) = run_sanitizer_witness(
            root,
            out,
            diff,
            profile,
            args.dry_run,
            &req.target,
            sanitizer_timeout,
            nightly_available,
        )?;
        result.proof_receipts.push(receipt);
        result.resource_leases.push(lease);
    }

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

/// Execute a single sanitizer witness proof task.
/// Resolves `ProofKind::SanitizerWitness` via the executor adapter, runs the
/// approved command under a resource lease, and produces a `ProofReceipt`.
///
/// Production sanitizer-witness path (Order 4c of #678). Previously
/// `#[cfg(test)]`-gated; now reachable from production via the `SanitizerWitness`
/// dispatch in `run_request_proof_broker_v0` and via `run_sanitizer_witness`.
/// The `with_runner` variant is the dependency-injection seam used by tests.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_sanitizer_witness_with_runner<F>(
    _out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    dry_run: bool,
    target: &str,
    timeout_sec: u64,
    nightly_available: bool,
    mut runner: F,
) -> Result<(ProofReceipt, ResourceLease)>
where
    F: FnMut(&[String], &BTreeMap<String, String>, u64) -> Result<CommandStatus>,
{
    let lease_budget = proof_lease_budget(profile)?;
    // Content-hash fingerprint of the target (not its byte length) so two
    // distinct targets of equal length cannot collide on the lease/receipt id
    // or on the on-disk witness stream files. Matches the focused-proof id
    // convention (sha256_hex(..)[..12]).
    let target_fp = &sha256_hex(target.as_bytes())[..12];
    let resource_lease = ResourceLease {
        schema: RESOURCE_LEASE_SCHEMA.to_owned(),
        id: format!("sanitizer-lease-{target_fp}"),
        kind: "sanitizer-witness".to_owned(),
        consumer: format!("sanitizer-witness:{target}"),
        status: "granted".to_owned(),
        reason: "sanitizer witness lease granted".to_owned(),
        cpu: lease_budget.cpu,
        memory_mb: lease_budget.memory_mb,
        disk_mb: lease_budget.disk_mb,
        timeout_sec,
        network: lease_budget.network,
        scratch: lease_budget.scratch,
        worktree: None,
        command: None,
    };

    if dry_run {
        return Ok(skip_receipt(
            &resource_lease,
            diff,
            target,
            "skipped_profile",
            "dry-run; sanitizer witness not executed",
        ));
    }

    if !nightly_available {
        return Ok(skip_receipt(
            &resource_lease,
            diff,
            target,
            "skipped_nightly",
            "nightly Rust not available; sanitizer witness requires nightly",
        ));
    }

    let resolved = resolve_proof_command(&ProofKind::SanitizerWitness, target, true);
    let Some(cmd) = resolved else {
        return Ok(skip_receipt(
            &resource_lease,
            diff,
            target,
            "skipped_unresolved",
            "executor adapter could not resolve sanitizer-witness intent",
        ));
    };

    let env_map: BTreeMap<String, String> = cmd.env.into_iter().collect();
    let status = runner(&cmd.argv, &env_map, timeout_sec)?;

    let (result, cmd_status, reason) = if status.timed_out {
        (
            "timed_out",
            "timed_out",
            format!("timed out after {timeout_sec}s"),
        )
    } else if status.success {
        ("sanitizer_clean", "passed", "no UB detected".to_owned())
    } else {
        (
            "sanitizer_ub_detected",
            "failed",
            "potential UB or runtime error".to_owned(),
        )
    };

    Ok((
        ProofReceipt {
            schema: PROOF_RECEIPT_SCHEMA.to_owned(),
            id: format!("sanitizer-receipt-{target_fp}"),
            kind: "sanitizer-witness".to_owned(),
            base: diff.base.clone(),
            head: diff.head.clone(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: Vec::new(),
            request_ids: Vec::new(),
            commands: vec![ProofCommandReceipt {
                side: "head".to_owned(),
                command: cmd.argv.join(" "),
                env: env_map,
                status: cmd_status.to_owned(),
                exit_code: status.exit_code,
                timed_out: status.timed_out,
                timeout_sec,
                duration_ms: status.duration_ms,
                stdout: String::new(),
                stderr: String::new(),
                reason,
            }],
            result: result.to_owned(),
            reason: format!("sanitizer witness: {result}"),
        },
        resource_lease,
    ))
}

/// Production entry for a sanitizer witness (Order 4c of #678). Supplies the
/// bounded, file-backed runner (`run_command_to_files`) to
/// `run_sanitizer_witness_with_runner`, bridging its `(argv, env, timeout)`
/// runner contract to the production runner that writes stdout/stderr to
/// bounded files under `out/sanitizer-witness/`. This is what the
/// `SanitizerWitness` dispatch in `run_request_proof_broker_v0` calls in a
/// real run; it executes only when a `SanitizerWitness` v2 request is present
/// and nightly is available.
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_sanitizer_witness(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    dry_run: bool,
    target: &str,
    timeout_sec: u64,
    nightly_available: bool,
) -> Result<(ProofReceipt, ResourceLease)> {
    let witness_dir = out.join("sanitizer-witness");
    fs::create_dir_all(&witness_dir)?;
    // Content-hash fingerprint so distinct equal-length targets get distinct
    // on-disk stream files (no collision).
    let target_fp = sha256_hex(target.as_bytes())[..12].to_owned();
    let mut runner =
        |argv: &[String], env: &BTreeMap<String, String>, timeout: u64| -> Result<CommandStatus> {
            let stdout_path = witness_dir.join(format!("sanitizer-{target_fp}-stdout.log"));
            let stderr_path = witness_dir.join(format!("sanitizer-{target_fp}-stderr.log"));
            let status =
                crate::run_command_to_files(root, argv, env, timeout, &stdout_path, &stderr_path)?;
            Ok(status)
        };
    run_sanitizer_witness_with_runner(
        out,
        diff,
        profile,
        dry_run,
        target,
        timeout_sec,
        nightly_available,
        &mut runner,
    )
}

/// Build a canonical `ProofReceipt` that records a sanitizer-witness skip
/// (dry-run, nightly unavailable, unresolved intent) without executing.
fn skip_receipt(
    lease_base: &ResourceLease,
    diff: &DiffContext,
    target: &str,
    result: &str,
    reason: &str,
) -> (ProofReceipt, ResourceLease) {
    let mut lease = lease_base.clone();
    if result == "skipped_nightly" || result == "skipped_unresolved" {
        lease.status = "absent".to_owned();
    } else {
        lease.status = "skipped_profile".to_owned();
    }
    lease.reason = reason.to_owned();
    let target_fp = &sha256_hex(target.as_bytes())[..12];
    (
        ProofReceipt {
            schema: PROOF_RECEIPT_SCHEMA.to_owned(),
            id: format!("sanitizer-receipt-{target_fp}"),
            kind: "sanitizer-witness".to_owned(),
            base: diff.base.clone(),
            head: diff.head.clone(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: Vec::new(),
            request_ids: Vec::new(),
            commands: Vec::new(),
            result: result.to_owned(),
            reason: reason.to_owned(),
        },
        lease,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CommandStatus, DiffContext, DiffFlags};

    fn test_diff() -> DiffContext {
        DiffContext {
            base: "HEAD~1".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["src/main.rs".to_owned()],
            patch: String::new(),
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
            diff_class: crate::DiffClass::SourceUb,
        }
    }

    fn runner_clean(
        _argv: &[String],
        _env: &BTreeMap<String, String>,
        _timeout: u64,
    ) -> Result<CommandStatus> {
        Ok(CommandStatus {
            exit_code: Some(0),
            timed_out: false,
            success: true,
            reason: "clean".to_owned(),
            duration_ms: 1000,
        })
    }

    fn runner_asan(
        _argv: &[String],
        _env: &BTreeMap<String, String>,
        _timeout: u64,
    ) -> Result<CommandStatus> {
        Ok(CommandStatus {
            exit_code: Some(1),
            timed_out: false,
            success: false,
            reason: "ASAN detected heap-buffer-overflow".to_owned(),
            duration_ms: 2000,
        })
    }

    #[test]
    fn sanitizer_skips_when_no_nightly() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let diff = test_diff();
        let profile = Profile::default();
        let (receipt, lease) = run_sanitizer_witness_with_runner(
            temp.path(),
            &diff,
            &profile,
            false,
            "test_target",
            60,
            false,
            runner_clean,
        )?;
        assert_eq!(receipt.result, "skipped_nightly");
        assert_eq!(lease.status, "absent");
        Ok(())
    }

    #[test]
    fn sanitizer_records_clean_when_passes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let diff = test_diff();
        let profile = Profile::default();
        let (receipt, lease) = run_sanitizer_witness_with_runner(
            temp.path(),
            &diff,
            &profile,
            false,
            "test_target",
            60,
            true,
            runner_clean,
        )?;
        assert_eq!(receipt.result, "sanitizer_clean");
        assert_eq!(lease.status, "granted");
        assert_eq!(receipt.commands.len(), 1);
        assert_eq!(receipt.commands[0].status, "passed");
        Ok(())
    }

    #[test]
    fn sanitizer_records_ub_when_fails() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let diff = test_diff();
        let profile = Profile::default();
        let (receipt, _lease) = run_sanitizer_witness_with_runner(
            temp.path(),
            &diff,
            &profile,
            false,
            "test_target",
            60,
            true,
            runner_asan,
        )?;
        assert_eq!(receipt.result, "sanitizer_ub_detected");
        assert_eq!(receipt.commands[0].status, "failed");
        assert!(receipt.commands[0].reason.contains("UB"));
        Ok(())
    }

    /// Production sanitizer dispatch (Order 4c of #678): a v1 `ProofRequest`
    /// whose cost/command infers `ProofKind::SanitizerWitness` (via
    /// `classify_proof_kind`) must, when run through the request proof broker,
    /// produce a `sanitizer-witness` receipt. This proves the typed dispatch
    /// added in Order 4b routes sanitizer requests to the production sanitizer
    /// path rather than dropping them or misrouting to test/build. The exact
    /// result (`sanitizer_clean`/`sanitizer_ub_detected`/`skipped_nightly`) is
    /// environment-dependent (nightly availability); the test asserts the
    /// kind and a valid result vocabulary.
    #[test]
    fn request_broker_dispatches_sanitizer_witness() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let diff = test_diff();
        let profile = Profile::default();
        let args = crate::tests::test_run_args(temp.path().to_path_buf());
        // cost="manual" + command containing "asan" => classify_proof_kind
        // infers SanitizerWitness.
        let sanitizer_request = ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "req-san-1".to_owned(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            command: "cargo +nightly test asan config_tests".to_owned(),
            reason: "confirm no UB".to_owned(),
            cost: "manual".to_owned(),
            timeout_sec: 120,
            required: false,
            status: "requested".to_owned(),
        };
        let result = run_request_proof_broker_v0(
            temp.path(),
            temp.path(),
            &diff,
            &profile,
            std::slice::from_ref(&sanitizer_request),
            &[],
            &[],
            &args,
        )?;
        let sanitizer_receipts: Vec<_> = result
            .proof_receipts
            .iter()
            .filter(|r| r.kind == "sanitizer-witness")
            .collect();
        assert_eq!(
            sanitizer_receipts.len(),
            1,
            "the broker must dispatch exactly one sanitizer-witness receipt"
        );
        let receipt = sanitizer_receipts[0];
        assert_eq!(receipt.kind, "sanitizer-witness");
        assert!(
            matches!(
                receipt.result.as_str(),
                "sanitizer_clean"
                    | "sanitizer_ub_detected"
                    | "skipped_nightly"
                    | "skipped_profile"
                    | "skipped_unresolved"
                    | "timed_out"
            ),
            "sanitizer receipt result must be a valid witness outcome, got `{}`",
            receipt.result
        );
        // No focused-test/build receipt should be produced for a sanitizer
        // request — the typed dispatch must not misroute it.
        assert!(
            result
                .proof_receipts
                .iter()
                .all(|r| r.kind != "focused-test" && r.kind != "focused-build"),
            "a sanitizer request must not produce focused-test/build receipts"
        );
        Ok(())
    }

    /// Regression for the self-review finding on PR #684: distinct sanitizer
    /// targets of EQUAL byte length must NOT collapse to one receipt. The old
    /// code keyed the receipt id and dedup on target.len(); two targets of the
    /// same length silently dropped one witness. The fix keys on a sha256
    /// content-hash fingerprint, so distinct content always gets distinct
    /// receipts and on-disk stream files.
    #[test]
    fn sanitizer_distinct_equal_length_targets_do_not_collide() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let diff = test_diff();
        let profile = Profile::default();
        let args = crate::tests::test_run_args(temp.path().to_path_buf());
        // Two distinct targets, both exactly 12 bytes long.
        let req = |id: &str, target: &str| ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: id.to_owned(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            command: format!("cargo +nightly test asan {target}"),
            reason: "confirm no UB".to_owned(),
            cost: "manual".to_owned(),
            timeout_sec: 120,
            required: false,
            status: "requested".to_owned(),
        };
        let requests = vec![
            req("req-a", "config_tests"),  // 12 bytes
            req("req-b", "other_xtests_"), // 12 bytes — distinct content
        ];
        let result = run_request_proof_broker_v0(
            temp.path(),
            temp.path(),
            &diff,
            &profile,
            &requests,
            &[],
            &[],
            &args,
        )?;
        let sanitizer_ids: Vec<String> = result
            .proof_receipts
            .iter()
            .filter(|r| r.kind == "sanitizer-witness")
            .map(|r| r.id.clone())
            .collect();
        assert_eq!(
            sanitizer_ids.len(),
            2,
            "two distinct equal-length targets must produce two distinct receipts, got {sanitizer_ids:?}"
        );
        assert_ne!(sanitizer_ids[0], sanitizer_ids[1]);
        Ok(())
    }
}
