//! Proof broker entry points: diff, seeded, request, and follow-up
//! orchestration that drives the focused test/build runners and writes
//! the proof receipts and resource leases the gate consumes (cleanup
//! train step 12, pure code motion). The focused runners, budgets, and
//! worktree helpers already live in the sibling proof/ submodules; this
//! module owns only the broker run orchestration and the lease
//! constructors for focused test/build tasks.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::time::Instant;

use anyhow::Result;

use crate::*;

fn current_portfolio_runtime(
    profile: &Profile,
    box_state: &BoxState,
    run_started: &Instant,
) -> Result<ProofPortfolioRuntime> {
    let hard_deadline_seconds = profile.budgets.hard_timeout_sec;
    let elapsed_seconds = run_started.elapsed().as_secs();
    Ok(ProofPortfolioRuntime::from_box_state(
        box_state,
        hard_deadline_seconds.saturating_sub(elapsed_seconds),
        proof_lease_budget(profile)?,
    ))
}

pub(crate) fn run_initial_diff_proof_broker_v0(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
    box_state: &BoxState,
    run_started: &Instant,
) -> Result<ProofBrokerResult> {
    let budget = proof_budget(profile)?;
    let runtime = current_portfolio_runtime(profile, box_state, run_started)?;
    let tasks = focused_test_candidates_from_diff(diff, &[]);
    let selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &tasks,
        build_tasks: &[],
        proof_requests: &[],
        proof_receipts: &[],
        budget,
        runtime,
    });
    let result = run_focused_red_green_proof_tasks_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        selection.test_tasks,
        run_command_to_files,
        prepare_base_plus_tests_worktree,
    )?;
    let final_selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &tasks,
        build_tasks: &[],
        proof_requests: &[],
        proof_receipts: &result.proof_receipts,
        budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    write_proof_portfolio_selection_artifact(out, diff, budget, tasks.len(), final_selection)?;
    Ok(result)
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
    box_state: &BoxState,
) -> Result<(ProofBrokerResult, Vec<RunLoopPhase>)> {
    let mut phases = Vec::new();
    let initial_result =
        run_initial_diff_proof_broker_v0(root, out, diff, profile, args, box_state, run_started);
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
            box_state,
            run_started,
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

pub(crate) struct ProofPortfolioInput<'a> {
    pub(crate) test_tasks: &'a [FocusedTestTask],
    pub(crate) build_tasks: &'a [FocusedBuildTask],
    pub(crate) proof_requests: &'a [ProofRequest],
    pub(crate) proof_receipts: &'a [ProofReceipt],
    pub(crate) budget: ProofBudget,
    pub(crate) runtime: ProofPortfolioRuntime,
}

pub(crate) struct ProofPortfolioSelection {
    pub(crate) test_tasks: Vec<FocusedTestTask>,
    pub(crate) build_tasks: Vec<FocusedBuildTask>,
    pub(crate) decisions: Vec<ProofPortfolioDecision>,
    pub(crate) remaining_seconds: u64,
    pub(crate) runtime: ProofPortfolioRuntime,
}

#[derive(Clone, Copy)]
enum PortfolioCandidate {
    Test(usize),
    Build(usize),
}

/// Select the highest-value safe proof portfolio from the current candidate
/// set. The selector is deterministic and receipt-aware so the broker can
/// call it again after each execution phase without rerunning answered work.
pub(crate) fn select_proof_portfolio(input: ProofPortfolioInput<'_>) -> ProofPortfolioSelection {
    let request_by_id = input
        .proof_requests
        .iter()
        .map(|request| (request.id.as_str(), request))
        .collect::<BTreeMap<_, _>>();
    let mut candidates = Vec::new();
    candidates.extend((0..input.test_tasks.len()).map(PortfolioCandidate::Test));
    candidates.extend((0..input.build_tasks.len()).map(PortfolioCandidate::Build));
    candidates.sort_by(|left, right| {
        portfolio_priority(right, &input, &request_by_id)
            .cmp(&portfolio_priority(left, &input, &request_by_id))
            .then_with(|| portfolio_cost(left, &input).cmp(&portfolio_cost(right, &input)))
            .then_with(|| portfolio_id(left, &input).cmp(portfolio_id(right, &input)))
    });

    let mut selected_tests = Vec::new();
    let mut selected_builds = Vec::new();
    let mut selected_files = BTreeSet::new();
    let mut used_tasks = 0_usize;
    let mut used_seconds = 0_u64;
    let mut decisions = Vec::new();
    let effective_max_seconds = input
        .budget
        .max_total_seconds
        .min(input.runtime.deadline_remaining_seconds);

    for candidate in candidates {
        let (task_id, kind, request_ids, required, estimated_cost_sec) =
            portfolio_metadata(&candidate, &input, &request_by_id);
        let exact_receipts = input
            .proof_receipts
            .iter()
            .filter(|receipt| receipt.id == task_id)
            .collect::<Vec<_>>();
        let shared_receipts = if exact_receipts.is_empty() {
            input
                .proof_receipts
                .iter()
                .filter(|receipt| {
                    receipt_can_answer_shared_request(receipt)
                        && receipt
                            .request_ids
                            .iter()
                            .any(|id| request_ids.contains(id))
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if !exact_receipts.is_empty() {
            decisions.push(portfolio_decision(
                task_id,
                kind,
                "answered_by_existing_receipt",
                format!(
                    "task already has terminal receipt(s): {}",
                    exact_receipts
                        .iter()
                        .map(|receipt| receipt.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                PortfolioDecisionMetadata {
                    required,
                    estimated_cost_sec,
                    request_ids,
                    receipt_ids: exact_receipts
                        .iter()
                        .map(|receipt| receipt.id.clone())
                        .collect(),
                },
            ));
            continue;
        }
        if !shared_receipts.is_empty() {
            decisions.push(portfolio_decision(
                task_id,
                kind,
                "satisfied_by_existing_evidence",
                format!(
                    "receipt(s) answer the shared request: {}",
                    shared_receipts
                        .iter()
                        .map(|receipt| receipt.id.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
                PortfolioDecisionMetadata {
                    required,
                    estimated_cost_sec,
                    request_ids,
                    receipt_ids: shared_receipts
                        .iter()
                        .map(|receipt| receipt.id.clone())
                        .collect(),
                },
            ));
            continue;
        }
        if !candidate_has_open_request(&request_ids, &request_by_id) {
            decisions.push(portfolio_decision(
                task_id,
                kind,
                "superseded",
                "all associated requests are already terminal or unavailable".to_owned(),
                PortfolioDecisionMetadata {
                    required,
                    estimated_cost_sec,
                    request_ids,
                    receipt_ids: Vec::new(),
                },
            ));
            continue;
        }

        let is_test = matches!(candidate, PortfolioCandidate::Test(_));
        let file_available = !is_test
            || selected_files.contains(portfolio_file(&candidate, &input))
            || selected_files.len() < input.budget.max_focused_test_files;
        let fits_box = portfolio_fits_box(&input.runtime.lease, &input.runtime);
        let fits_budget = used_tasks < input.budget.max_focused_tests
            && file_available
            && used_seconds.saturating_add(estimated_cost_sec) <= effective_max_seconds;
        if fits_budget && fits_box {
            used_tasks += 1;
            used_seconds = used_seconds.saturating_add(estimated_cost_sec);
            if is_test {
                selected_files.insert(portfolio_file(&candidate, &input).to_owned());
                if let PortfolioCandidate::Test(index) = candidate {
                    selected_tests.push(input.test_tasks[index].clone());
                }
            } else if let PortfolioCandidate::Build(index) = candidate {
                selected_builds.push(input.build_tasks[index].clone());
            }
            decisions.push(portfolio_decision(
                task_id,
                kind,
                "selected",
                format!(
                    "selected for value-ranked execution; serves {} request(s)",
                    request_ids.len()
                ),
                PortfolioDecisionMetadata {
                    required,
                    estimated_cost_sec,
                    request_ids,
                    receipt_ids: Vec::new(),
                },
            ));
        } else {
            let deadline_cannot_fit = input.runtime.deadline_remaining_seconds
                < input.budget.max_total_seconds
                && estimated_cost_sec > effective_max_seconds.saturating_sub(used_seconds);
            let status = if !fits_box {
                "declined_for_box_capacity"
            } else if effective_max_seconds == 0
                || deadline_cannot_fit
                || input.budget.max_focused_tests == 0
                || (is_test && input.budget.max_focused_test_files == 0)
            {
                "deferred_by_safe_wind_down"
            } else {
                "declined_for_higher_value_proof"
            };
            let reason = if !fits_box {
                "proof lease does not fit the current runner capacity"
            } else if effective_max_seconds == 0 || deadline_cannot_fit {
                "hard deadline has expired; proof was safely wound down"
            } else if required {
                "required floor could not fit inside the remaining safe proof budget"
            } else {
                "remaining budget was reserved for higher-value candidates"
            };
            decisions.push(portfolio_decision(
                task_id,
                kind,
                status,
                reason.to_owned(),
                PortfolioDecisionMetadata {
                    required,
                    estimated_cost_sec,
                    request_ids,
                    receipt_ids: Vec::new(),
                },
            ));
        }
    }

    ProofPortfolioSelection {
        test_tasks: selected_tests,
        build_tasks: selected_builds,
        decisions,
        remaining_seconds: effective_max_seconds.saturating_sub(used_seconds),
        runtime: input.runtime,
    }
}

fn portfolio_fits_box(lease: &ProofLeaseBudget, runtime: &ProofPortfolioRuntime) -> bool {
    let cpu_fits = usize::try_from(lease.cpu)
        .ok()
        .is_some_and(|cpu| cpu <= runtime.cpus);
    let memory_fits = runtime
        .free_mem_mb
        .is_none_or(|free| free >= lease.memory_mb);
    let disk_fits = runtime
        .free_disk_mb
        .is_none_or(|free| free >= lease.disk_mb);
    cpu_fits && memory_fits && disk_fits
}

fn portfolio_priority(
    candidate: &PortfolioCandidate,
    input: &ProofPortfolioInput<'_>,
    request_by_id: &BTreeMap<&str, &ProofRequest>,
) -> (u8, u8, usize) {
    let request_ids = portfolio_request_ids(candidate, input);
    let required = request_ids.iter().any(|id| {
        request_by_id
            .get(id.as_str())
            .is_some_and(|request| request.required)
    });
    let kind_rank = match candidate {
        PortfolioCandidate::Test(index) => match input.test_tasks[*index].mode {
            FocusedProofMode::RedGreen => 3,
            FocusedProofMode::HeadOnly => 2,
        },
        PortfolioCandidate::Build(_) => 1,
    };
    (u8::from(required), kind_rank, request_ids.len())
}

fn portfolio_cost(candidate: &PortfolioCandidate, input: &ProofPortfolioInput<'_>) -> u64 {
    match candidate {
        PortfolioCandidate::Test(index) => {
            let task = &input.test_tasks[*index];
            task.timeout_sec
                .unwrap_or(input.budget.per_command_timeout_sec)
                .min(input.budget.per_command_timeout_sec)
                .saturating_mul(task.mode.command_count())
        }
        PortfolioCandidate::Build(index) => input.build_tasks[*index]
            .timeout_sec
            .min(input.budget.per_command_timeout_sec),
    }
}

fn portfolio_id<'a>(candidate: &PortfolioCandidate, input: &ProofPortfolioInput<'a>) -> &'a str {
    match candidate {
        PortfolioCandidate::Test(index) => &input.test_tasks[*index].id,
        PortfolioCandidate::Build(index) => &input.build_tasks[*index].id,
    }
}

fn portfolio_file<'a>(candidate: &PortfolioCandidate, input: &ProofPortfolioInput<'a>) -> &'a str {
    match candidate {
        PortfolioCandidate::Test(index) => &input.test_tasks[*index].file,
        PortfolioCandidate::Build(_) => "<build>",
    }
}

fn portfolio_request_ids(
    candidate: &PortfolioCandidate,
    input: &ProofPortfolioInput<'_>,
) -> Vec<String> {
    match candidate {
        PortfolioCandidate::Test(index) => input.test_tasks[*index].request_ids.clone(),
        PortfolioCandidate::Build(index) => input.build_tasks[*index].request_ids.clone(),
    }
}

fn portfolio_metadata(
    candidate: &PortfolioCandidate,
    input: &ProofPortfolioInput<'_>,
    request_by_id: &BTreeMap<&str, &ProofRequest>,
) -> (String, String, Vec<String>, bool, u64) {
    let request_ids = portfolio_request_ids(candidate, input);
    let required = request_ids.iter().any(|id| {
        request_by_id
            .get(id.as_str())
            .is_some_and(|request| request.required)
    });
    let kind = match candidate {
        PortfolioCandidate::Test(index) => match input.test_tasks[*index].mode {
            FocusedProofMode::HeadOnly => "focused-head",
            FocusedProofMode::RedGreen => "focused-red-green",
        },
        PortfolioCandidate::Build(_) => "focused-build",
    };
    (
        portfolio_id(candidate, input).to_owned(),
        kind.to_owned(),
        request_ids,
        required,
        portfolio_cost(candidate, input),
    )
}

fn candidate_has_open_request(
    request_ids: &[String],
    request_by_id: &BTreeMap<&str, &ProofRequest>,
) -> bool {
    request_ids.is_empty()
        || request_ids.iter().any(|id| {
            request_by_id
                .get(id.as_str())
                .is_some_and(|request| request.status == "requested")
        })
}

fn receipt_can_answer_shared_request(receipt: &ProofReceipt) -> bool {
    matches!(
        receipt.result.as_str(),
        "discriminating"
            | "non_discriminating"
            | "head_passed"
            | "head_failed"
            | "base_patch_failed"
            | "sanitizer_clean"
            | "sanitizer_ub_detected"
            | "executed"
            | "failed"
    )
}

struct PortfolioDecisionMetadata {
    required: bool,
    estimated_cost_sec: u64,
    request_ids: Vec<String>,
    receipt_ids: Vec<String>,
}

fn portfolio_decision(
    task_id: String,
    kind: String,
    status: &str,
    reason: String,
    metadata: PortfolioDecisionMetadata,
) -> ProofPortfolioDecision {
    ProofPortfolioDecision {
        task_id,
        kind,
        status: status.to_owned(),
        reason,
        required: metadata.required,
        estimated_cost_sec: metadata.estimated_cost_sec,
        request_ids: metadata.request_ids,
        receipt_ids: metadata.receipt_ids,
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
    box_state: &BoxState,
    run_started: &Instant,
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
    let test_candidates = focused_test_candidates_from_v2(&v2_requests);
    let build_candidates = focused_build_candidates_from_v2(&v2_requests);
    let selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: existing_receipts,
        budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    let mut result = run_follow_up_proof_broker_v0_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        selection.test_tasks,
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
    let replan = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: &existing_and_new_receipts,
        budget: remaining_budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    let build_result = run_focused_build_proof_tasks_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        remaining_budget,
        replan.build_tasks,
        run_command_to_files,
    )?;
    result.proof_receipts.extend(build_result.proof_receipts);
    result.resource_leases.extend(build_result.resource_leases);
    let final_budget = remaining_focused_proof_budget(total_budget, &result.resource_leases);
    let final_selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: &result.proof_receipts,
        budget: final_budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    write_proof_portfolio_selection_artifact(
        out,
        diff,
        final_budget,
        test_candidates.len() + build_candidates.len(),
        final_selection,
    )?;
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
    box_state: &BoxState,
    run_started: &Instant,
) -> Result<ProofBrokerResult> {
    let total_budget = proof_budget(profile)?;
    let budget = remaining_focused_proof_budget(total_budget, existing_leases);
    let test_candidates = focused_test_candidates_from_requests(proof_requests);
    let build_candidates = focused_build_candidates_from_requests(proof_requests);
    let selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: existing_receipts,
        budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    let mut result = run_follow_up_proof_broker_v0_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        selection.test_tasks,
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
    let replan = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: &existing_and_new_receipts,
        budget: remaining_budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    let build_result = run_focused_build_proof_tasks_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        remaining_budget,
        replan.build_tasks,
        run_command_to_files,
    )?;
    result.proof_receipts.extend(build_result.proof_receipts);
    result.resource_leases.extend(build_result.resource_leases);
    let final_budget = remaining_focused_proof_budget(total_budget, &result.resource_leases);
    let final_selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: &result.proof_receipts,
        budget: final_budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    write_proof_portfolio_selection_artifact(
        out,
        diff,
        final_budget,
        test_candidates.len() + build_candidates.len(),
        final_selection,
    )?;
    Ok(result)
}

fn write_proof_portfolio_selection_artifact(
    out: &Path,
    diff: &DiffContext,
    budget: ProofBudget,
    candidate_count: usize,
    selection: ProofPortfolioSelection,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    let selected_task_ids = selection
        .test_tasks
        .iter()
        .map(|task| task.id.clone())
        .chain(selection.build_tasks.iter().map(|task| task.id.clone()))
        .collect::<Vec<_>>();
    let artifact = ProofPortfolioArtifact {
        schema: PROOF_PORTFOLIO_SCHEMA,
        phase: "broker-final",
        head: diff.head.clone(),
        budget_seconds: budget.max_total_seconds,
        candidate_count,
        selected_task_ids,
        remaining_seconds: selection.remaining_seconds,
        runtime: selection.runtime,
        decisions: selection.decisions,
    };
    fs::write(
        review_dir.join("proof_portfolio.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    Ok(())
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

/// Execute a single sanitizer witness proof task (Order 2 PR 3 of epic #655).
/// Resolves ProofKind::SanitizerWitness via the executor adapter, runs the
/// approved command under a resource lease, and produces a ProofReceipt.
#[cfg(test)]
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
    let resource_lease = ResourceLease {
        schema: RESOURCE_LEASE_SCHEMA.to_owned(),
        id: format!("sanitizer-lease-{}", target.len()),
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
            id: format!("sanitizer-receipt-{}", target.len()),
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

#[cfg(test)]
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
    (
        ProofReceipt {
            schema: PROOF_RECEIPT_SCHEMA.to_owned(),
            id: format!("sanitizer-receipt-{}", target.len()),
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
    use anyhow::{Result, ensure};

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

    fn focused_test_task(id: &str, request_ids: Vec<String>, timeout_sec: u64) -> FocusedTestTask {
        FocusedTestTask {
            id: id.to_owned(),
            file: format!("tests/{id}.test.ts"),
            test_name: None,
            mode: FocusedProofMode::RedGreen,
            command_specs: None,
            timeout_sec: Some(timeout_sec),
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids,
        }
    }

    fn focused_build_task(
        id: &str,
        request_ids: Vec<String>,
        timeout_sec: u64,
    ) -> FocusedBuildTask {
        FocusedBuildTask {
            id: id.to_owned(),
            command: "cargo check --package parser --locked".to_owned(),
            argv: vec!["cargo".to_owned(), "check".to_owned()],
            timeout_sec,
            requested_by: vec!["architecture".to_owned()],
            request_ids,
        }
    }

    fn proof_request(id: &str, required: bool) -> ProofRequest {
        ProofRequest {
            schema: PROOF_REQUEST_SCHEMA.to_owned(),
            id: id.to_owned(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            command: format!("bun test tests/{id}.test.ts"),
            reason: "answer the proof question".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 30,
            required,
            status: "requested".to_owned(),
        }
    }

    fn proof_budget_for_test(max_tests: usize, max_seconds: u64) -> ProofBudget {
        ProofBudget {
            max_focused_test_files: 2,
            max_focused_tests: max_tests,
            per_command_timeout_sec: 300,
            max_total_seconds: max_seconds,
        }
    }

    fn portfolio_runtime_for_test(
        deadline_remaining_seconds: u64,
        cpus: usize,
        free_mem_mb: Option<u64>,
        free_disk_mb: Option<u64>,
    ) -> ProofPortfolioRuntime {
        let box_state = BoxState {
            cpus,
            free_mem_mb,
            free_disk_mb,
            load_1m: Some(0.25),
            github_actions: false,
        };
        ProofPortfolioRuntime::from_box_state(
            &box_state,
            deadline_remaining_seconds,
            ProofLeaseBudget {
                cpu: 1,
                memory_mb: 512,
                disk_mb: 64,
                network: false,
                scratch: true,
            },
        )
    }

    fn test_receipt(id: &str, request_ids: Vec<String>, result: &str) -> ProofReceipt {
        ProofReceipt {
            schema: PROOF_RECEIPT_SCHEMA.to_owned(),
            id: id.to_owned(),
            kind: "focused-red-green".to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            test_patch_mode: "base-plus-tests".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids,
            commands: Vec::new(),
            result: result.to_owned(),
            reason: "test receipt".to_owned(),
        }
    }

    #[test]
    fn portfolio_selects_required_multi_request_test_before_build() -> Result<()> {
        let test = focused_test_task(
            "parser",
            vec!["required".to_owned(), "shared".to_owned()],
            30,
        );
        let build = focused_build_task("build", vec!["build".to_owned()], 10);
        let requests = vec![
            proof_request("required", true),
            proof_request("shared", false),
            proof_request("build", false),
        ];
        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&test),
            build_tasks: std::slice::from_ref(&build),
            proof_requests: &requests,
            proof_receipts: &[],
            budget: proof_budget_for_test(1, 60),
            runtime: portfolio_runtime_for_test(60, 4, Some(8_192), Some(20_000)),
        });
        ensure!(selection.test_tasks.len() == 1);
        ensure!(selection.build_tasks.is_empty());
        let build_decision = selection
            .decisions
            .iter()
            .find(|decision| decision.task_id == "build")
            .ok_or_else(|| anyhow::anyhow!("missing build portfolio decision"))?;
        ensure!(build_decision.status == "declined_for_higher_value_proof");
        let test_decision = selection
            .decisions
            .iter()
            .find(|decision| decision.task_id == "parser")
            .ok_or_else(|| anyhow::anyhow!("missing test portfolio decision"))?;
        ensure!(test_decision.request_ids.len() == 2);
        Ok(())
    }

    #[test]
    fn portfolio_replan_satisfies_shared_build_from_test_receipt() -> Result<()> {
        let test = focused_test_task("parser", vec!["shared".to_owned()], 30);
        let build = focused_build_task("build", vec!["shared".to_owned()], 30);
        let requests = vec![proof_request("shared", false)];
        let receipt = test_receipt("parser", vec!["shared".to_owned()], "head_passed");
        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&test),
            build_tasks: std::slice::from_ref(&build),
            proof_requests: &requests,
            proof_receipts: std::slice::from_ref(&receipt),
            budget: proof_budget_for_test(2, 60),
            runtime: portfolio_runtime_for_test(60, 4, Some(8_192), Some(20_000)),
        });
        ensure!(selection.test_tasks.is_empty());
        ensure!(selection.build_tasks.is_empty());
        let build_decision = selection
            .decisions
            .iter()
            .find(|decision| decision.task_id == "build")
            .ok_or_else(|| anyhow::anyhow!("missing build portfolio decision"))?;
        ensure!(build_decision.status == "satisfied_by_existing_evidence");
        ensure!(build_decision.receipt_ids == vec!["parser".to_owned()]);
        Ok(())
    }

    #[test]
    fn portfolio_records_safe_wind_down_without_selecting_work() -> Result<()> {
        let test = focused_test_task("parser", Vec::new(), 30);
        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&test),
            build_tasks: &[],
            proof_requests: &[],
            proof_receipts: &[],
            budget: proof_budget_for_test(1, 0),
            runtime: portfolio_runtime_for_test(0, 4, Some(8_192), Some(20_000)),
        });
        ensure!(selection.test_tasks.is_empty());
        let decision = selection
            .decisions
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing wind-down decision"))?;
        ensure!(decision.status == "deferred_by_safe_wind_down");
        Ok(())
    }

    #[test]
    fn portfolio_declines_work_that_does_not_fit_the_current_box() -> Result<()> {
        let test = focused_test_task("parser", Vec::new(), 30);
        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&test),
            build_tasks: &[],
            proof_requests: &[],
            proof_receipts: &[],
            budget: proof_budget_for_test(1, 60),
            runtime: portfolio_runtime_for_test(60, 0, Some(128), Some(32)),
        });
        ensure!(selection.test_tasks.is_empty());
        let decision = selection
            .decisions
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing box-capacity decision"))?;
        ensure!(decision.status == "declined_for_box_capacity");
        ensure!(decision.reason.contains("current runner capacity"));
        Ok(())
    }

    #[test]
    fn portfolio_winds_down_when_the_remaining_deadline_cannot_fit_a_task() -> Result<()> {
        let test = focused_test_task("parser", Vec::new(), 30);
        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&test),
            build_tasks: &[],
            proof_requests: &[],
            proof_receipts: &[],
            budget: proof_budget_for_test(1, 60),
            runtime: portfolio_runtime_for_test(5, 4, Some(8_192), Some(20_000)),
        });
        ensure!(selection.test_tasks.is_empty());
        ensure!(selection.remaining_seconds == 5);
        let decision = selection
            .decisions
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing deadline decision"))?;
        ensure!(decision.status == "deferred_by_safe_wind_down");
        ensure!(decision.reason.contains("hard deadline"));
        Ok(())
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
}
