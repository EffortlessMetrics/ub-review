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

use crate::test_parse::push_unique;
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

/// Run the diff-derived proof floor.
///
/// `impact` is the run's single impact plan (already written to
/// `review/impact_plan.json`). It maps the changed files to owning Cargo
/// packages, closes over reverse dependencies, and ranks their test targets;
/// this is the phase that owns diff-derived selection, so this is where that
/// ranking becomes executable work instead of prompt text.
///
/// The plan's `selection_mode` is the *model planner* visibility gate
/// ([`ImpactPlan::proof_planner_candidates`]) and is deliberately not read
/// here: this path is deterministic Rust selection whose commands are
/// re-validated by the focused-cargo-test allowlist, not model-proposed work.
/// On a repo without cargo metadata the plan carries no candidates and the
/// floor stays Bun-only.
#[expect(
    clippy::too_many_arguments,
    reason = "the initial diff broker takes the run's checkout, packet, diff, profile, args, box state, clock, and impact plan; each is a distinct input the broker must not re-derive"
)]
pub(crate) fn run_initial_diff_proof_broker_v0(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
    box_state: &BoxState,
    run_started: &Instant,
    impact: &ImpactPlan,
    task_ledger: Option<&ProofTaskLedger>,
) -> Result<ProofBrokerResult> {
    let budget = proof_budget(profile)?;
    let runtime = current_portfolio_runtime(profile, box_state, run_started)?;
    let tasks = focused_test_candidates_from_diff(diff, &[], Some(impact));
    let selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &tasks,
        build_tasks: &[],
        proof_requests: &[],
        proof_receipts: &[],
        head: &diff.head,
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
        task_ledger,
        ProofExecutionPhase::InitialImpact,
        run_command_to_files_with_spawn_observer,
        prepare_base_plus_tests_worktree,
    )?;
    let final_budget = settled_focused_proof_budget(
        budget,
        &result.resource_leases,
        &result.proof_receipts,
        &diff.head,
    );
    let final_selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &tasks,
        build_tasks: &[],
        proof_requests: &[],
        proof_receipts: &result.proof_receipts,
        head: &diff.head,
        budget: final_budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    write_proof_portfolio_selection_artifact(
        out,
        diff,
        final_budget,
        &tasks,
        &[],
        &[],
        final_selection,
    )?;
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
    impact: &ImpactPlan,
    task_ledger: Option<&ProofTaskLedger>,
) -> Result<(ProofBrokerResult, Vec<RunLoopPhase>)> {
    let mut phases = Vec::new();
    let initial_result = run_initial_diff_proof_broker_v0(
        root,
        out,
        diff,
        profile,
        args,
        box_state,
        run_started,
        impact,
        task_ledger,
    );
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

    if has_unreceipted_proof_request_tasks(
        seeded_proof_requests,
        &proof_result.proof_receipts,
        &diff.head,
    ) {
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
            task_ledger,
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
    pub(crate) head: &'a str,
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

fn remaining_focused_proof_budget_after(
    total_budget: ProofBudget,
    existing_leases: &[ResourceLease],
    new_leases: &[ResourceLease],
    receipts: &[ProofReceipt],
    head: &str,
) -> ProofBudget {
    let mut all_leases = existing_leases.to_vec();
    all_leases.extend(new_leases.iter().cloned());
    settled_focused_proof_budget(total_budget, &all_leases, receipts, head)
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
        portfolio_priority(*right, input.test_tasks, input.build_tasks, &request_by_id)
            .cmp(&portfolio_priority(
                *left,
                input.test_tasks,
                input.build_tasks,
                &request_by_id,
            ))
            .then_with(|| {
                portfolio_cost(
                    *left,
                    input.test_tasks,
                    input.build_tasks,
                    input.budget.per_command_timeout_sec,
                )
                .cmp(&portfolio_cost(
                    *right,
                    input.test_tasks,
                    input.build_tasks,
                    input.budget.per_command_timeout_sec,
                ))
            })
            .then_with(|| {
                portfolio_id(*left, input.test_tasks, input.build_tasks).cmp(portfolio_id(
                    *right,
                    input.test_tasks,
                    input.build_tasks,
                ))
            })
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
        let candidate_task = portfolio_candidate_fields(
            candidate,
            input.test_tasks,
            input.build_tasks,
            input.budget.per_command_timeout_sec,
            &request_by_id,
        );
        let exact_receipts = input
            .proof_receipts
            .iter()
            .filter(|receipt| receipt_matches_task_on_head(receipt, &candidate_task.id, input.head))
            .collect::<Vec<_>>();
        let shared_receipts = if exact_receipts.is_empty() {
            input
                .proof_receipts
                .iter()
                .filter(|receipt| {
                    receipt_matches_shared_request_on_head(
                        receipt,
                        &candidate_task.request_ids,
                        input.head,
                    )
                })
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if !exact_receipts.is_empty() {
            decisions.push(portfolio_decision(
                candidate_task.id,
                candidate_task.kind,
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
                    required: candidate_task.required,
                    estimated_cost_sec: candidate_task.estimated_cost_sec,
                    request_ids: candidate_task.request_ids,
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
                candidate_task.id,
                candidate_task.kind,
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
                    required: candidate_task.required,
                    estimated_cost_sec: candidate_task.estimated_cost_sec,
                    request_ids: candidate_task.request_ids,
                    receipt_ids: shared_receipts
                        .iter()
                        .map(|receipt| receipt.id.clone())
                        .collect(),
                },
            ));
            continue;
        }
        if !candidate_has_open_request(&candidate_task.request_ids, &request_by_id) {
            decisions.push(portfolio_decision(
                candidate_task.id,
                candidate_task.kind,
                "superseded",
                "all associated requests are already terminal or unavailable".to_owned(),
                PortfolioDecisionMetadata {
                    required: candidate_task.required,
                    estimated_cost_sec: candidate_task.estimated_cost_sec,
                    request_ids: candidate_task.request_ids,
                    receipt_ids: Vec::new(),
                },
            ));
            continue;
        }

        let is_test = matches!(candidate, PortfolioCandidate::Test(_));
        let file_available = !is_test
            || selected_files.contains(portfolio_file(candidate, input.test_tasks))
            || selected_files.len() < input.budget.max_focused_test_files;
        let fits_box = portfolio_fits_box(&input.runtime.lease, &input.runtime);
        let fits_budget = used_tasks < input.budget.max_focused_tests
            && file_available
            && used_seconds.saturating_add(candidate_task.estimated_cost_sec)
                <= effective_max_seconds;
        if fits_budget && fits_box {
            used_tasks += 1;
            used_seconds = used_seconds.saturating_add(candidate_task.estimated_cost_sec);
            if is_test {
                selected_files.insert(portfolio_file(candidate, input.test_tasks).to_owned());
                if let PortfolioCandidate::Test(index) = candidate {
                    selected_tests.push(input.test_tasks[index].clone());
                }
            } else if let PortfolioCandidate::Build(index) = candidate {
                selected_builds.push(input.build_tasks[index].clone());
            }
            decisions.push(portfolio_decision(
                candidate_task.id,
                candidate_task.kind,
                "selected",
                format!(
                    "selected for value-ranked execution; serves {} request(s)",
                    candidate_task.request_ids.len()
                ),
                PortfolioDecisionMetadata {
                    required: candidate_task.required,
                    estimated_cost_sec: candidate_task.estimated_cost_sec,
                    request_ids: candidate_task.request_ids,
                    receipt_ids: Vec::new(),
                },
            ));
        } else {
            let deadline_cannot_fit = input.runtime.deadline_remaining_seconds
                < input.budget.max_total_seconds
                && candidate_task.estimated_cost_sec
                    > effective_max_seconds.saturating_sub(used_seconds);
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
            let remaining_allocation = input.budget.max_total_seconds.saturating_sub(used_seconds);
            let remaining_deadline = input
                .runtime
                .deadline_remaining_seconds
                .saturating_sub(used_seconds);
            let reason = if !fits_box {
                "proof lease does not fit the current runner capacity"
            } else if input.runtime.deadline_remaining_seconds == 0 {
                "hard deadline has expired; proof was safely wound down"
            } else if remaining_allocation == 0 {
                "proof-time allocation is exhausted"
            } else if used_tasks >= input.budget.max_focused_tests {
                "proof task capacity is exhausted"
            } else if !file_available {
                "focused test-file capacity is exhausted"
            } else if candidate_task.estimated_cost_sec > remaining_deadline {
                "estimated task cost cannot fit the remaining hard-deadline wall time"
            } else if candidate_task.estimated_cost_sec > remaining_allocation {
                "estimated task cost cannot fit the remaining proof-time allocation"
            } else if candidate_task.required {
                "required floor could not fit inside the remaining safe proof budget"
            } else {
                "remaining budget was reserved for higher-value candidates"
            };
            decisions.push(portfolio_decision(
                candidate_task.id,
                candidate_task.kind,
                status,
                reason.to_owned(),
                PortfolioDecisionMetadata {
                    required: candidate_task.required,
                    estimated_cost_sec: candidate_task.estimated_cost_sec,
                    request_ids: candidate_task.request_ids,
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
    candidate: PortfolioCandidate,
    test_tasks: &[FocusedTestTask],
    build_tasks: &[FocusedBuildTask],
    request_by_id: &BTreeMap<&str, &ProofRequest>,
) -> (u8, u8, usize) {
    let request_ids = portfolio_request_ids(candidate, test_tasks, build_tasks);
    let required = portfolio_required(request_ids, request_by_id);
    let kind_rank = match candidate {
        PortfolioCandidate::Test(index) => match test_tasks[index].mode {
            FocusedProofMode::RedGreen => 3,
            FocusedProofMode::HeadOnly => 2,
        },
        PortfolioCandidate::Build(_) => 1,
    };
    (u8::from(required), kind_rank, request_ids.len())
}

fn portfolio_cost(
    candidate: PortfolioCandidate,
    test_tasks: &[FocusedTestTask],
    build_tasks: &[FocusedBuildTask],
    per_command_timeout_sec: u64,
) -> u64 {
    match candidate {
        PortfolioCandidate::Test(index) => {
            let task = &test_tasks[index];
            task.timeout_sec
                .unwrap_or(per_command_timeout_sec)
                .min(per_command_timeout_sec)
                .saturating_mul(task.mode.command_count())
        }
        PortfolioCandidate::Build(index) => {
            build_tasks[index].timeout_sec.min(per_command_timeout_sec)
        }
    }
}

fn portfolio_id<'a>(
    candidate: PortfolioCandidate,
    test_tasks: &'a [FocusedTestTask],
    build_tasks: &'a [FocusedBuildTask],
) -> &'a str {
    match candidate {
        PortfolioCandidate::Test(index) => &test_tasks[index].id,
        PortfolioCandidate::Build(index) => &build_tasks[index].id,
    }
}

fn portfolio_kind(candidate: PortfolioCandidate, test_tasks: &[FocusedTestTask]) -> &'static str {
    match candidate {
        PortfolioCandidate::Test(index) => match test_tasks[index].mode {
            FocusedProofMode::HeadOnly => "focused-head",
            FocusedProofMode::RedGreen => "focused-red-green",
        },
        PortfolioCandidate::Build(_) => "focused-build",
    }
}

fn portfolio_file(candidate: PortfolioCandidate, test_tasks: &[FocusedTestTask]) -> &str {
    match candidate {
        PortfolioCandidate::Test(index) => &test_tasks[index].file,
        PortfolioCandidate::Build(_) => "<build>",
    }
}

fn portfolio_request_ids<'a>(
    candidate: PortfolioCandidate,
    test_tasks: &'a [FocusedTestTask],
    build_tasks: &'a [FocusedBuildTask],
) -> &'a [String] {
    match candidate {
        PortfolioCandidate::Test(index) => &test_tasks[index].request_ids,
        PortfolioCandidate::Build(index) => &build_tasks[index].request_ids,
    }
}

fn portfolio_required(
    request_ids: &[String],
    request_by_id: &BTreeMap<&str, &ProofRequest>,
) -> bool {
    request_ids.iter().any(|id| {
        request_by_id
            .get(id.as_str())
            .is_some_and(|request| request.required)
    })
}

/// The published identity of one broker candidate. Both the decision records
/// and the artifact's `candidate_tasks` entries are built from this single
/// derivation so their metadata cannot drift.
fn portfolio_candidate_fields(
    candidate: PortfolioCandidate,
    test_tasks: &[FocusedTestTask],
    build_tasks: &[FocusedBuildTask],
    per_command_timeout_sec: u64,
    request_by_id: &BTreeMap<&str, &ProofRequest>,
) -> ProofPortfolioCandidateTask {
    let request_ids = portfolio_request_ids(candidate, test_tasks, build_tasks).to_vec();
    ProofPortfolioCandidateTask {
        id: portfolio_id(candidate, test_tasks, build_tasks).to_owned(),
        kind: portfolio_kind(candidate, test_tasks).to_owned(),
        required: portfolio_required(&request_ids, request_by_id),
        estimated_cost_sec: portfolio_cost(
            candidate,
            test_tasks,
            build_tasks,
            per_command_timeout_sec,
        ),
        request_ids,
    }
}

/// Publish the full input catalog (test + build candidates, not just the
/// selected subset) in decision order so every portfolio decision resolves
/// against a known task even when its planner lane listed nothing.
pub(crate) fn portfolio_candidate_tasks(
    test_tasks: &[FocusedTestTask],
    build_tasks: &[FocusedBuildTask],
    budget: ProofBudget,
    proof_requests: &[ProofRequest],
) -> Vec<ProofPortfolioCandidateTask> {
    let request_by_id = proof_requests
        .iter()
        .map(|request| (request.id.as_str(), request))
        .collect::<BTreeMap<_, _>>();
    (0..test_tasks.len())
        .map(PortfolioCandidate::Test)
        .chain((0..build_tasks.len()).map(PortfolioCandidate::Build))
        .map(|candidate| {
            portfolio_candidate_fields(
                candidate,
                test_tasks,
                build_tasks,
                budget.per_command_timeout_sec,
                &request_by_id,
            )
        })
        .collect()
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
    task_ledger: Option<&ProofTaskLedger>,
) -> Result<ProofBrokerResult> {
    run_request_proof_broker_v0_with_runners(
        root,
        out,
        diff,
        profile,
        proof_requests,
        existing_receipts,
        existing_leases,
        args,
        box_state,
        run_started,
        task_ledger,
        run_command_to_files_with_spawn_observer,
        prepare_base_plus_tests_worktree,
        run_command_to_files_with_spawn_observer,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "request broker keeps proof phases and injected runner boundaries explicit"
)]
fn run_request_proof_broker_v0_with_runners<F, G, H>(
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
    task_ledger: Option<&ProofTaskLedger>,
    test_runner: F,
    prepare_base_plus_tests: G,
    build_runner: H,
) -> Result<ProofBrokerResult>
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
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
    H: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
        &mut dyn FnMut(CommandProcessObservation),
    ) -> Result<CommandStatus>,
{
    // Native v2 proof flow (Order 4b of #678): normalize v1 requests to typed
    // v2 once at ingestion, then extract candidates from v2. v2 is now the
    // internal contract; the candidate extractors key off ProofKind, so only
    // FocusedTest/FocusedBuild requests reach test/build execution and other
    // kinds (SanitizerWitness/MiriWitness/...) are routed by their own paths
    // (Order 4c, #681). The v2 extractors re-run the allowlist on the command
    // string, so the security boundary is preserved byte-for-byte.
    let v2_requests: Vec<ProofRequestV2> = proof_requests.iter().map(proof_request_to_v2).collect();
    let total_budget = proof_budget(profile)?;
    let budget =
        settled_focused_proof_budget(total_budget, existing_leases, existing_receipts, &diff.head);
    // Diff-derived Cargo candidates are owned by the initial diff broker and
    // already carry receipts by the time a request phase runs; this phase
    // re-plans requested work only.
    let mut test_candidates = focused_test_candidates_from_diff(diff, &[], None);
    merge_focused_test_candidates(
        &mut test_candidates,
        focused_test_candidates_from_v2(&v2_requests),
    );
    let build_candidates = focused_build_candidates_from_v2(&v2_requests);
    let selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: existing_receipts,
        head: &diff.head,
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
        task_ledger,
        ProofExecutionPhase::ModelRequest,
        test_runner,
        prepare_base_plus_tests,
    )?;
    let mut consumed_leases = existing_leases.to_vec();
    consumed_leases.extend(result.resource_leases.clone());
    let existing_and_new_receipts = existing_receipts
        .iter()
        .chain(result.proof_receipts.iter())
        .cloned()
        .collect::<Vec<_>>();
    let remaining_budget = settled_focused_proof_budget(
        total_budget,
        &consumed_leases,
        &existing_and_new_receipts,
        &diff.head,
    );
    let replan = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: &existing_and_new_receipts,
        head: &diff.head,
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
        task_ledger,
        ProofExecutionPhase::ModelRequest,
        build_runner,
    )?;
    result.proof_receipts.extend(build_result.proof_receipts);
    result.resource_leases.extend(build_result.resource_leases);
    let final_receipts = existing_receipts
        .iter()
        .chain(result.proof_receipts.iter())
        .cloned()
        .collect::<Vec<_>>();
    let final_budget = remaining_focused_proof_budget_after(
        total_budget,
        existing_leases,
        &result.resource_leases,
        &final_receipts,
        &diff.head,
    );
    let final_selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: &final_receipts,
        head: &diff.head,
        budget: final_budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    write_proof_portfolio_selection_artifact(
        out,
        diff,
        final_budget,
        &test_candidates,
        &build_candidates,
        proof_requests,
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
    task_ledger: Option<&ProofTaskLedger>,
) -> Result<ProofBrokerResult> {
    let total_budget = proof_budget(profile)?;
    let budget =
        settled_focused_proof_budget(total_budget, existing_leases, existing_receipts, &diff.head);
    // As in the request phase: diff-derived Cargo candidates belong to the
    // initial diff broker; follow-up re-plans requested work only.
    let mut test_candidates = focused_test_candidates_from_diff(diff, &[], None);
    merge_focused_test_candidates(
        &mut test_candidates,
        focused_test_candidates_from_requests(proof_requests),
    );
    let build_candidates = focused_build_candidates_from_requests(proof_requests);
    let selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: existing_receipts,
        head: &diff.head,
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
        task_ledger,
        ProofExecutionPhase::FollowUp,
        run_command_to_files_with_spawn_observer,
        prepare_base_plus_tests_worktree,
    )?;
    let mut consumed_leases = existing_leases.to_vec();
    consumed_leases.extend(result.resource_leases.clone());
    let existing_and_new_receipts = existing_receipts
        .iter()
        .chain(result.proof_receipts.iter())
        .cloned()
        .collect::<Vec<_>>();
    let remaining_budget = settled_focused_proof_budget(
        total_budget,
        &consumed_leases,
        &existing_and_new_receipts,
        &diff.head,
    );
    let replan = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: &existing_and_new_receipts,
        head: &diff.head,
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
        task_ledger,
        ProofExecutionPhase::FollowUp,
        run_command_to_files_with_spawn_observer,
    )?;
    result.proof_receipts.extend(build_result.proof_receipts);
    result.resource_leases.extend(build_result.resource_leases);
    let final_receipts = existing_receipts
        .iter()
        .chain(result.proof_receipts.iter())
        .cloned()
        .collect::<Vec<_>>();
    let final_budget = remaining_focused_proof_budget_after(
        total_budget,
        existing_leases,
        &result.resource_leases,
        &final_receipts,
        &diff.head,
    );
    let final_selection = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &test_candidates,
        build_tasks: &build_candidates,
        proof_requests,
        proof_receipts: &final_receipts,
        head: &diff.head,
        budget: final_budget,
        runtime: current_portfolio_runtime(profile, box_state, run_started)?,
    });
    // A follow-up pass with no newly requested proof must not replace the
    // primary broker's authoritative portfolio with an empty/zero-budget
    // snapshot. The primary portfolio contains the request-to-task decisions
    // that the proof-intent artifact points at; preserving it keeps those
    // identities verifiable even when follow-up produced no work.
    if should_write_follow_up_portfolio(out, proof_requests) {
        write_proof_portfolio_selection_artifact(
            out,
            diff,
            final_budget,
            &test_candidates,
            &build_candidates,
            proof_requests,
            final_selection,
        )?;
    }
    Ok(result)
}

fn should_write_follow_up_portfolio(out: &Path, proof_requests: &[ProofRequest]) -> bool {
    !proof_requests.is_empty() || !out.join("review/proof_portfolio.json").exists()
}

/// Read view of a previously written portfolio artifact for run-wide
/// merging (#4271 round 2). Unknown fields are ignored and every retained
/// field defaults, so a foreign or partial file degrades to empty. `head`
/// guards against merging stale decisions from another run that reused the
/// output directory (round 2b): a head mismatch starts a new portfolio.
#[derive(Debug, serde::Deserialize)]
struct ProofPortfolioDiskView {
    #[serde(default)]
    head: String,
    #[serde(default)]
    candidate_tasks: Vec<ProofPortfolioCandidateTask>,
    #[serde(default)]
    selected_task_ids: Vec<String>,
    #[serde(default)]
    decisions: Vec<ProofPortfolioDecision>,
}

/// Merge portfolio entries by task identity: fresh entries supersede
/// same-id disk entries in place, disk entries the fresh write never saw
/// are retained in their original order.
fn merge_proof_portfolio_entries<T>(
    existing: Vec<T>,
    fresh: Vec<T>,
    id_of: impl Fn(&T) -> &str,
) -> Vec<T> {
    let mut merged = existing;
    for entry in fresh {
        if let Some(slot) = merged.iter_mut().find(|kept| id_of(kept) == id_of(&entry)) {
            *slot = entry;
        } else {
            merged.push(entry);
        }
    }
    merged
}

/// Merge same-id candidate catalog entries field-aware (round 4): task ids
/// omit request ids, so a follow-up write carrying a narrower request set
/// must not clear the earlier obligation from the published catalog the
/// way a fresh-supersedes merge would. `required` ORs together, request
/// ids union (existing order first, then fresh-only), and kind/estimated
/// cost come from the fresh entry because fresh broker state supersedes.
fn merge_proof_portfolio_candidate_tasks(
    existing: Vec<ProofPortfolioCandidateTask>,
    fresh: Vec<ProofPortfolioCandidateTask>,
) -> Vec<ProofPortfolioCandidateTask> {
    let mut merged = existing;
    for task in fresh {
        if let Some(slot) = merged.iter_mut().find(|kept| kept.id == task.id) {
            slot.required = slot.required || task.required;
            for request_id in &task.request_ids {
                if !slot.request_ids.contains(request_id) {
                    slot.request_ids.push(request_id.clone());
                }
            }
            slot.kind = task.kind.clone();
            slot.estimated_cost_sec = task.estimated_cost_sec;
        } else {
            merged.push(task);
        }
    }
    merged
}

/// Merge same-id portfolio decisions field-aware (round 2b): task ids
/// omit request ids, so an optional follow-up request mapping to an
/// already-recorded task id must not clear the earlier obligation.
/// `required` ORs together, request and receipt ids union (existing order
/// first, then fresh-only), and kind/status/reason come from the fresh
/// decision because fresh broker state supersedes.
fn merge_proof_portfolio_decisions(
    existing: Vec<ProofPortfolioDecision>,
    fresh: Vec<ProofPortfolioDecision>,
) -> Vec<ProofPortfolioDecision> {
    let mut merged = existing;
    for decision in fresh {
        if let Some(slot) = merged
            .iter_mut()
            .find(|kept| kept.task_id == decision.task_id)
        {
            slot.required = slot.required || decision.required;
            for request_id in &decision.request_ids {
                if !slot.request_ids.contains(request_id) {
                    slot.request_ids.push(request_id.clone());
                }
            }
            for receipt_id in &decision.receipt_ids {
                if !slot.receipt_ids.contains(receipt_id) {
                    slot.receipt_ids.push(receipt_id.clone());
                }
            }
            slot.kind = decision.kind.clone();
            slot.status = decision.status.clone();
            slot.reason = decision.reason.clone();
        } else {
            merged.push(decision);
        }
    }
    merged
}

/// Merge a fresh portfolio write with the artifact already on disk (#4271
/// round 2): the follow-up broker writes follow-up-only decisions, which
/// must not erase the primary broker's required obligations. Catalog
/// entries merge field-aware via `merge_proof_portfolio_candidate_tasks`
/// (round 4) so a narrower follow-up request set cannot clear an earlier
/// task obligation; disk entries for unseen tasks are retained; decisions
/// merge field-aware via `merge_proof_portfolio_decisions`. Scalars
/// (budget, remaining, runtime, head, phase) always come from the fresh
/// write. Merge applies only when
/// the stored head equals the current head (round 2b); a reused output
/// directory under a different head starts a new portfolio instead of
/// absorbing stale decisions.
fn merge_proof_portfolio_with_disk(
    review_dir: &Path,
    head: &str,
    candidate_tasks: Vec<ProofPortfolioCandidateTask>,
    selected_task_ids: Vec<String>,
    decisions: Vec<ProofPortfolioDecision>,
) -> (
    Vec<ProofPortfolioCandidateTask>,
    Vec<String>,
    Vec<ProofPortfolioDecision>,
) {
    let existing: Option<ProofPortfolioDiskView> =
        fs::read(review_dir.join("proof_portfolio.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok());
    let Some(existing) = existing else {
        return (candidate_tasks, selected_task_ids, decisions);
    };
    if existing.head != head {
        return (candidate_tasks, selected_task_ids, decisions);
    }
    (
        merge_proof_portfolio_candidate_tasks(existing.candidate_tasks, candidate_tasks),
        merge_proof_portfolio_entries(existing.selected_task_ids, selected_task_ids, |id| {
            id.as_str()
        }),
        merge_proof_portfolio_decisions(existing.decisions, decisions),
    )
}

fn write_proof_portfolio_selection_artifact(
    out: &Path,
    diff: &DiffContext,
    budget: ProofBudget,
    test_candidates: &[FocusedTestTask],
    build_candidates: &[FocusedBuildTask],
    proof_requests: &[ProofRequest],
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
    let candidate_tasks =
        portfolio_candidate_tasks(test_candidates, build_candidates, budget, proof_requests);
    let (candidate_tasks, selected_task_ids, decisions) = merge_proof_portfolio_with_disk(
        &review_dir,
        &diff.head,
        candidate_tasks,
        selected_task_ids,
        selection.decisions,
    );
    let artifact = ProofPortfolioArtifact {
        schema: PROOF_PORTFOLIO_SCHEMA,
        phase: "broker-final",
        head: diff.head.clone(),
        budget_seconds: budget.max_total_seconds,
        candidate_count: candidate_tasks.len(),
        candidate_tasks,
        selected_task_ids,
        remaining_seconds: selection.remaining_seconds,
        runtime: selection.runtime,
        decisions,
    };
    fs::write(
        review_dir.join("proof_portfolio.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    Ok(())
}

fn merge_focused_test_candidates(
    candidates: &mut Vec<FocusedTestTask>,
    additional: Vec<FocusedTestTask>,
) {
    for mut task in additional {
        let matching_index = candidates
            .iter()
            .position(|candidate| candidate.id == task.id);
        if let Some(existing) = matching_index.and_then(|index| candidates.get_mut(index)) {
            existing.required |= task.required;
            if existing.timeout_sec < task.timeout_sec {
                existing.timeout_sec = task.timeout_sec;
            }
            for lane in task.requested_by.drain(..) {
                push_unique(&mut existing.requested_by, &lane);
            }
            for request_id in task.request_ids.drain(..) {
                push_unique(&mut existing.request_ids, &request_id);
            }
        } else {
            candidates.push(task);
        }
    }
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
    task_ledger: Option<&ProofTaskLedger>,
    execution_phase: ProofExecutionPhase,
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
        &mut dyn FnMut(CommandProcessObservation),
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
        task_ledger,
        execution_phase,
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
    // Request attribution only: impact-plan candidates carry no request ids
    // and are filtered out below regardless, so the plan is not needed here.
    let request_metadata = focused_test_candidates_from_diff(diff, proof_requests, None)
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
    head: &str,
) -> Vec<FocusedTestTask> {
    let existing_ids = current_head_receipt_ids(existing_receipts, head);
    tasks
        .into_iter()
        .filter(|task| !existing_ids.contains(&task.id))
        .collect()
}

pub(crate) fn unreceipted_focused_build_tasks(
    tasks: Vec<FocusedBuildTask>,
    existing_receipts: &[ProofReceipt],
    head: &str,
) -> Vec<FocusedBuildTask> {
    let existing_ids = current_head_receipt_ids(existing_receipts, head);
    tasks
        .into_iter()
        .filter(|task| !existing_ids.contains(&task.id))
        .collect()
}

pub(crate) fn has_unreceipted_proof_request_tasks(
    proof_requests: &[ProofRequest],
    existing_receipts: &[ProofReceipt],
    head: &str,
) -> bool {
    !unreceipted_focused_test_tasks(
        focused_test_candidates_from_requests(proof_requests),
        existing_receipts,
        head,
    )
    .is_empty()
        || !unreceipted_focused_build_tasks(
            focused_build_candidates_from_requests(proof_requests),
            existing_receipts,
            head,
        )
        .is_empty()
}

fn current_head_receipt_ids(receipts: &[ProofReceipt], head: &str) -> BTreeSet<String> {
    receipts
        .iter()
        .filter(|receipt| receipt.head.eq_ignore_ascii_case(head))
        .map(|receipt| receipt.id.clone())
        .collect()
}

fn receipt_matches_task_on_head(receipt: &ProofReceipt, task_id: &str, head: &str) -> bool {
    receipt.id == task_id && receipt.head.eq_ignore_ascii_case(head)
}

/// Request-identity intersection shared by the broker's head-scoped matcher
/// and the gate's revision-scoped planner accounting (#4271): a receipt
/// answers a task's requests when any receipt request id is one of the
/// task's request ids. Eligibility beyond this intersection (head label vs
/// revision digest, answered vs proven result class) stays with the caller.
pub(crate) fn receipt_shares_proof_request(receipt: &ProofReceipt, request_ids: &[String]) -> bool {
    receipt
        .request_ids
        .iter()
        .any(|id| request_ids.contains(id))
}

fn receipt_matches_shared_request_on_head(
    receipt: &ProofReceipt,
    request_ids: &[String],
    head: &str,
) -> bool {
    receipt.head.eq_ignore_ascii_case(head)
        && receipt_can_answer_shared_request(receipt)
        && receipt_shares_proof_request(receipt, request_ids)
}

pub(crate) fn focused_test_resource_lease(
    task: &FocusedTestTask,
    budget: ProofBudget,
    lease_budget: ProofLeaseBudget,
    status: &str,
    reason: &str,
) -> ResourceLease {
    ResourceLease {
        revision: None,
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
        revision: None,
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
        revision: None,
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
            revision: None,
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
            revision: None,
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
    use crate::task_ledger_artifact::build_task_ledger_artifacts;
    use crate::test_parse::command_display_with_env;
    use crate::tests::test_run_args;
    use crate::{CommandStatus, DiffContext, DiffFlags};

    #[test]
    fn request_attribution_reuses_only_the_executed_bun_target() -> Result<()> {
        let different_file = ["bun", "bd", "test", "test/parser.test.tsx"];
        let named = [
            "bun",
            "bd",
            "test",
            "test/parser.test.ts",
            "-t",
            "parser regression",
        ];
        let whole_file = ["bun", "bd", "test", "test/parser.test.ts"];
        let cargo_filter = [
            "cargo",
            "test",
            "--locked",
            "--test",
            "config_tests",
            "test/parser.test.ts",
        ];
        for (file, name, command, expected_command) in [
            (
                "test/parser.test.ts",
                None,
                "bun test test/parser.test.ts",
                None,
            ),
            (
                "test/parser.test.ts",
                None,
                "bun bd test test/parser.test.ts",
                None,
            ),
            (
                "test/parser.test.ts",
                None,
                "bun test test/parser.test.tsx",
                Some(different_file.as_slice()),
            ),
            (
                "test/parser.test.ts",
                Some("parser"),
                "bun test test/parser.test.ts -t 'parser regression'",
                Some(named.as_slice()),
            ),
            (
                "test/parser.test.ts",
                Some("parser regression"),
                "bun test test/parser.test.ts -t 'parser regression'",
                None,
            ),
            (
                "test/parser.test.ts",
                Some("parser"),
                "bun test test/parser.test.ts",
                Some(whole_file.as_slice()),
            ),
            (
                "test/parser.test.ts",
                None,
                "bun test test/parser.test.ts -t 'parser regression'",
                Some(named.as_slice()),
            ),
            (
                "test/parser.test.ts",
                None,
                "cargo test --locked --test config_tests test/parser.test.ts",
                Some(cargo_filter.as_slice()),
            ),
            (
                "b/test/parser.test.ts",
                None,
                "bun test test/parser.test.ts",
                Some(whole_file.as_slice()),
            ),
        ] {
            let reuse = expected_command.is_none();
            let temp = tempfile::tempdir()?;
            let out = temp.path().join("out");
            let base_root = temp.path().join("base-plus-tests");
            fs::create_dir_all(&base_root)?;
            let mut diff = test_diff();
            diff.changed_files = vec![file.to_owned()];
            diff.patch = format!(
                "diff --git a/{file} b/{file}\n\
                 --- a/{file}\n+++ b/{file}\n\
                 @@ -0,0 +1 @@\n+{}\n",
                name.map_or_else(
                    || "// changed test setup".to_owned(),
                    |name| { format!("test(\"{name}\", () => {{}});") }
                )
            );
            let mut profile = Profile::default();
            profile.limits.tests = 2;
            profile.budgets.proof_max_focused_test_files = 2;
            profile.budgets.proof_max_focused_tests = 2;
            profile.budgets.proof_command_timeout_sec = 2;
            profile.budgets.proof_total_timeout_sec = 20;
            let args = test_run_args(out.clone());
            let tasks = focused_test_candidates_from_diff(&diff, &[], None);
            ensure!(tasks.len() == 1);
            let mut initial_commands = Vec::new();
            let mut initial = run_focused_red_green_proof_tasks_with_runner(
                temp.path(),
                &out,
                &diff,
                &profile,
                &args,
                proof_budget(&profile)?,
                tasks,
                None,
                ProofExecutionPhase::InitialImpact,
                |_root, argv, _env, _timeout, stdout, stderr, _observe| {
                    initial_commands.push(argv.to_vec());
                    fs::write(stdout, b"initial target result\n")?;
                    fs::write(stderr, b"")?;
                    let success = !stdout.to_string_lossy().contains("base-plus-tests");
                    Ok(CommandStatus {
                        exit_code: Some(if success { 0 } else { 1 }),
                        timed_out: false,
                        success,
                        reason: "initial target fixture".to_owned(),
                        duration_ms: 1,
                    })
                },
                |_root, _out, _diff| Ok(base_root.clone()),
            )?;
            ensure!(initial_commands.len() == 2);
            let mut initial_head = vec!["bun", "bd", "test", file];
            let mut initial_base = vec!["bun", "test", file];
            if let Some(name) = name {
                initial_head.extend(["-t", name]);
                initial_base.extend(["-t", name]);
            }
            assert_eq!(initial_commands, vec![initial_head, initial_base]);
            ensure!(initial.proof_receipts.len() == 1);
            ensure!(initial.proof_receipts[0].result == "discriminating");
            let request = ProofRequest {
                schema: PROOF_REQUEST_SCHEMA.to_owned(),
                id: "required-target".to_owned(),
                lane: REQUIRED_PROOF_POLICY_LANE.to_owned(),
                requested_by: vec!["proof-policy:required-target".to_owned()],
                command: command.to_owned(),
                reason: "Run the requested target, not a similarly named target.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 2,
                required: true,
                status: "requested".to_owned(),
            };
            attach_request_metadata_to_focused_receipts(
                &diff,
                std::slice::from_ref(&request),
                &mut initial.proof_receipts,
            );
            let mut requested_commands = Vec::new();
            let result = run_request_proof_broker_v0_with_runners(
                temp.path(),
                &out,
                &diff,
                &profile,
                std::slice::from_ref(&request),
                &initial.proof_receipts,
                &initial.resource_leases,
                &args,
                &BoxState {
                    cpus: 4,
                    free_mem_mb: Some(8_192),
                    free_disk_mb: Some(20_000),
                    load_1m: Some(0.25),
                    github_actions: false,
                },
                &Instant::now(),
                None,
                |_root, argv, _env, _timeout, stdout, stderr, _observe| {
                    requested_commands.push(argv.to_vec());
                    fs::write(stdout, b"requested target fails\n")?;
                    fs::write(stderr, b"")?;
                    Ok(CommandStatus {
                        exit_code: Some(1),
                        timed_out: false,
                        success: false,
                        reason: "requested target fixture".to_owned(),
                        duration_ms: 1,
                    })
                },
                |_root, _out, _diff| Ok(base_root.clone()),
                |_root, _argv, _env, _timeout, _stdout, _stderr, _observe| {
                    anyhow::bail!("test requests must not run a build")
                },
            )?;
            initial.proof_receipts.extend(result.proof_receipts);
            let receipts: Vec<ProofReceipt> =
                serde_json::from_slice(&serde_json::to_vec(&initial.proof_receipts)?)?;
            let terminal =
                terminalize_proof_requests(&diff.head, std::slice::from_ref(&request), &receipts);
            let gate = build_gate_outcome(GateOutcomeInput {
                args: &args,
                config: &Config::default(),
                plan: &crate::tests::test_plan(Vec::new()),
                terminal_state: &crate::tests::test_terminal_state("sufficient"),
                proof_requests: &terminal,
                proof_receipts: &receipts,
                tool_gate_outcomes: &[],
                missing_or_failed_sensor_evidence: &[],
                missing_or_failed_model_evidence: &[],
                reporter_gate: ReporterGateInput::Absent,
                planner_portfolio: None,
                revision_digest: None,
            });
            let gate = serde_json::to_value(&gate)?;
            assert_eq!(gate["required_proof"]["matched"], 1);
            assert_eq!(
                gate["required_proof"]["passed"],
                usize::from(reuse),
                "{command}"
            );
            assert_eq!(
                gate["required_proof"]["failed"],
                usize::from(!reuse),
                "{command}"
            );
            assert_eq!(
                terminal[0].status,
                if reuse { "satisfied" } else { "failed" },
                "requested {command} after initial filter {name:?}; commands: {requested_commands:?}"
            );
            if reuse {
                assert!(requested_commands.is_empty(), "{command}");
                assert_eq!(receipts.len(), 1);
            } else if let Some(expected_command) = expected_command {
                assert_eq!(requested_commands.len(), 1, "{command}");
                assert_eq!(requested_commands[0], expected_command);
                assert!(receipts[0].request_ids.is_empty(), "{command}");
                assert_eq!(receipts.len(), 2);
                assert_eq!(receipts[1].result, "head_failed");
            }
        }
        Ok(())
    }

    #[test]
    fn accounting_matrix_releases_early_commands_before_required_build() -> Result<()> {
        let mut failures = Vec::new();
        for case in [
            "success",
            "failure",
            "timeout",
            "setup_failure",
            "cancellation",
        ] {
            let temp = tempfile::tempdir()?;
            let out = temp.path().join("out");
            let mut diff = test_diff();
            diff.head = "155e27501db0693432c143d4110ef5d0cbcf981e".to_owned();
            let mut test = proof_request("accounting-test", true);
            test.command = "cargo test --locked --test config_tests".to_owned();
            test.timeout_sec = 3;
            let mut build = proof_request("accounting-build", true);
            build.command = "cargo check --workspace --all-targets --locked".to_owned();
            build.cost = "focused-build".to_owned();
            build.timeout_sec = 4;
            let requests = vec![test, build];
            let mut profile = Profile::default();
            profile.limits.tests = 1;
            profile.limits.builds = 1;
            profile.budgets.proof_max_focused_test_files = 3;
            profile.budgets.proof_max_focused_tests = 3;
            profile.budgets.proof_command_timeout_sec = 4;
            profile.budgets.proof_total_timeout_sec = 6;
            profile.budgets.hard_timeout_sec = 30;
            let args = test_run_args(out.clone());
            let box_state = BoxState::detect()?;
            let started = Instant::now();
            let revision = RevisionRef {
                digest: "a".repeat(64),
                semantics: "candidate_head".to_owned(),
                reviewed_commit: diff.head.clone(),
            };
            let recorder = TaskLedgerRecorder::new(&revision, &started)?;
            let ledger = ProofTaskLedger::new(recorder.clone());
            let mut actual = Vec::new();
            let mut builds = 0;
            let mut build_started_ns = None;
            let result = run_request_proof_broker_v0_with_runners(
                temp.path(),
                &out,
                &diff,
                &profile,
                &requests,
                &[],
                &[],
                &args,
                &box_state,
                &started,
                Some(&ledger),
                |root, argv, env, timeout, stdout, stderr, observer| {
                    ensure!(argv.first().is_some_and(|arg| arg == "cargo"));
                    ensure!(timeout == 3);
                    let base = stdout.to_string_lossy().contains("base-plus-tests");
                    let clock = Instant::now();
                    // The real subprocess runner executes these deterministic fixtures;
                    // the injected seam does not change the broker command allowlist.
                    let fixture = match (case, base) {
                        ("timeout", _) => vec!["/bin/sleep".to_owned(), "10".to_owned()],
                        ("setup_failure", _) => {
                            std::thread::sleep(std::time::Duration::from_millis(20));
                            vec![
                                root.join("missing-accounting-program")
                                    .display()
                                    .to_string(),
                            ]
                        }
                        ("cancellation", _) => vec!["/bin/sleep".to_owned(), "0.05".to_owned()],
                        ("failure", _) | (_, true) => {
                            vec!["/bin/sh".to_owned(), "-c".to_owned(), "exit 7".to_owned()]
                        }
                        _ => vec!["/bin/sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()],
                    };
                    let status = run_command_to_files_with_spawn_observer(
                        root, &fixture, env, timeout, stdout, stderr, observer,
                    );
                    actual.push(serde_json::json!({
                        "argv": fixture, "timeout_sec": timeout,
                        "monotonic_duration_ns": clock.elapsed().as_nanos(),
                    }));
                    if case == "cancellation" {
                        status?;
                        return Err(anyhow::anyhow!(
                            "fixture cancellation after confirmed completion"
                        ));
                    }
                    status
                },
                |_root, _out, _diff| {
                    let base = temp.path().join("base-plus-tests");
                    fs::create_dir_all(&base)?;
                    Ok(base)
                },
                |root, _argv, env, timeout, stdout, stderr, observer| {
                    ensure!(timeout == 4);
                    builds += 1;
                    build_started_ns = Some(started.elapsed().as_nanos());
                    run_command_to_files_with_spawn_observer(
                        root,
                        &["/bin/sh".to_owned(), "-c".to_owned(), "exit 0".to_owned()],
                        env,
                        timeout,
                        stdout,
                        stderr,
                        observer,
                    )
                },
            )?;
            let artifact: serde_json::Value =
                serde_json::from_slice(&fs::read(out.join("review/proof_portfolio.json"))?)?;
            write_proof_receipt_artifacts(&out, &result.proof_receipts, Some(&revision))?;
            ledger.reconcile_published_receipts(&out)?;
            let ledger_bytes = build_task_ledger_artifacts(&revision, &recorder.inputs()?)?;
            let ledger_snapshot: serde_json::Value =
                serde_json::from_slice(&ledger_bytes.snapshot_json)?;
            let row = serde_json::json!({
                "case": case, "actual_commands": actual, "proof_receipts": result.proof_receipts,
                "leases": result.resource_leases, "portfolio": artifact,
                "ledger": ledger_snapshot, "required_build_executions": builds,
                "required_build_started_ns": build_started_ns, "box_state": box_state,
                "hard_deadline_sec": profile.budgets.hard_timeout_sec,
                "total_budget_sec": profile.budgets.proof_total_timeout_sec,
                "elapsed_ns": started.elapsed().as_nanos(),
            });
            eprintln!("ACCOUNTING_MATRIX {row}");
            if builds != usize::from(case != "timeout") {
                failures.push(format!("{case}: required build executions={builds}"));
            }
            if matches!(case, "cancellation" | "setup_failure")
                && row["proof_receipts"][0]["commands"][0]["duration_ms"] == 0
            {
                failures.push(format!("{case}: elapsed setup/cancellation was erased"));
            }
            let first_receipt = &result.proof_receipts[0];
            let measured_ms: u128 = first_receipt
                .commands
                .iter()
                .map(|command| {
                    command.duration_ms.max(if command.timed_out {
                        u128::from(command.timeout_sec) * 1_000
                    } else {
                        1
                    })
                })
                .sum();
            let first_charge = u64::try_from(measured_ms.div_ceil(1_000))?;
            let expected_remaining = 6_u64
                .saturating_sub(first_charge)
                .saturating_sub(builds as u64);
            if artifact["budget_seconds"].as_u64() != Some(expected_remaining) {
                failures.push(format!("{case}: settlement does not reconcile"));
            }
            ensure!(result.resource_leases[0].timeout_sec == 6);
            if let Some(at) = build_started_ns {
                ensure!(at < 30_000_000_000);
                ensure!(result.resource_leases[1].timeout_sec == 4);
            }
            if matches!(
                case,
                "failure" | "setup_failure" | "cancellation" | "timeout"
            ) {
                ensure!(
                    first_receipt.result != "head_passed"
                        && first_receipt.result != "discriminating"
                );
            }
            ensure!(
                artifact["runtime"]["deadline_remaining_seconds"]
                    .as_u64()
                    .is_some_and(|n| n > 0)
            );
        }
        ensure!(failures.is_empty(), "{}", failures.join("; "));
        Ok(())
    }

    #[test]
    fn composed_semantic_intents_preserve_refusal_and_release_early_budget() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("Cargo.toml"),
            "[package]\nname=\"fixture\"\nversion=\"0.0.0\"\nedition=\"2024\"\n[workspace]\n[[bin]]\nname=\"app\"\npath=\"app.rs\"\n[[test]]\nname=\"probe\"\npath=\"probe.rs\"\n",
        )?;
        for name in ["app.rs", "probe.rs"] {
            fs::write(temp.path().join(name), "fn main() {}\n")?;
        }
        let mut diff = test_diff();
        diff.changed_files.clear();
        let semantic = |kind, target: &str, timeout| ProofIntent {
            id: target.to_owned(),
            claim_id: "composition".to_owned(),
            question: "Does the declared proof answer the claim?".to_owned(),
            expected_answer_shape: "terminal receipt".to_owned(),
            proof_kind: kind,
            target: target.to_owned(),
            estimated_value: "high".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            status: "requested".to_owned(),
            timeout_sec: Some(timeout),
            resolved_request_ids: vec!["stale".to_owned()],
            resolution_reason: String::new(),
        };
        let mut profile = Profile::default();
        profile.limits.tests = 1;
        profile.limits.builds = 1;
        profile.budgets.proof_max_focused_test_files = 3;
        profile.budgets.proof_max_focused_tests = 3;
        profile.budgets.proof_command_timeout_sec = 4;
        profile.budgets.proof_total_timeout_sec = 6;
        profile.budgets.hard_timeout_sec = 30;
        let resolved = resolve_model_proof_intents(
            temp.path(),
            &diff,
            &[
                semantic(ProofKind::FocusedBuild, "cargo-target:fixture::app", 4),
                semantic(ProofKind::FocusedTest, "cargo-test:fixture::probe", 3),
                semantic(ProofKind::FocusedBuild, "cargo-package:fixture", 4),
            ],
            &[],
            proof_budget(&profile)?,
        )?;
        ensure!(resolved.intents[0].status == "unsupported_target");
        ensure!(resolved.intents[0].resolved_request_ids.is_empty());
        ensure!(resolved.proof_requests.len() == 2);
        let out = temp.path().join("out");
        let args = test_run_args(out.clone());
        let mut executed = Vec::new();
        let mut builds = 0;
        let result = run_request_proof_broker_v0_with_runners(
            temp.path(),
            &out,
            &diff,
            &profile,
            &resolved.proof_requests,
            &[],
            &[],
            &args,
            &BoxState::detect()?,
            &Instant::now(),
            None,
            |_root, argv, _env, timeout, stdout, stderr, _observer| {
                ensure!(argv.join(" ") == "cargo test --locked --package fixture --test probe");
                ensure!(timeout == 3);
                let base = stdout.to_string_lossy().contains("base-plus-tests");
                executed.push(if base { "base-plus-tests" } else { "head" });
                fs::write(stdout, b"controlled semantic test")?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    success: !base,
                    exit_code: Some(if base { 7 } else { 0 }),
                    timed_out: false,
                    duration_ms: 200,
                    reason: "fixture".to_owned(),
                })
            },
            |_root, _out, _diff| {
                let base = temp.path().join("base-plus-tests");
                fs::create_dir_all(&base)?;
                Ok(base)
            },
            |_root, argv, _env, timeout, stdout, stderr, _observer| {
                ensure!(argv.join(" ") == "cargo check --locked --package fixture");
                ensure!(timeout == 4);
                builds += 1;
                fs::write(stdout, b"controlled semantic build")?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    success: true,
                    exit_code: Some(0),
                    timed_out: false,
                    duration_ms: 200,
                    reason: "fixture".to_owned(),
                })
            },
        )?;
        ensure!(executed == ["head", "base-plus-tests"]);
        ensure!(
            builds == 1,
            "early completion must admit the approved package build"
        );
        ensure!(result.proof_receipts.len() == 2);
        ensure!(result.resource_leases.len() == 2);
        ensure!(result.resource_leases[0].timeout_sec == 6);
        ensure!(result.resource_leases[1].timeout_sec == 4);
        ensure!(result.proof_receipts[0].result == "discriminating");
        ensure!(result.proof_receipts[1].result == "head_passed");
        for request in &resolved.proof_requests {
            ensure!(
                result
                    .proof_receipts
                    .iter()
                    .any(|receipt| receipt.request_ids.contains(&request.id))
            );
        }
        let portfolio: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/proof_portfolio.json"))?)?;
        ensure!(portfolio["budget_seconds"] == 4);
        eprintln!(
            "COMPOSED_INTENT_ACCOUNTING {}",
            serde_json::json!({
                "intents": resolved.intents, "requests": resolved.proof_requests,
                "receipts": result.proof_receipts, "leases": result.resource_leases,
                "portfolio": portfolio,
            })
        );
        Ok(())
    }

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
            required: false,
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
            required: false,
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

    fn granted_focused_test_lease(id: &str, timeout_sec: u64) -> ResourceLease {
        focused_test_resource_lease(
            &focused_test_task(id, Vec::new(), timeout_sec),
            proof_budget_for_test(2, 120),
            ProofLeaseBudget {
                cpu: 1,
                memory_mb: 512,
                disk_mb: 64,
                network: false,
                scratch: true,
            },
            "granted",
            "test lease",
        )
    }

    #[test]
    fn final_budget_accounts_for_existing_and_new_leases() {
        let existing = granted_focused_test_lease("existing", 30);
        let new = granted_focused_test_lease("new", 30);
        let remaining = remaining_focused_proof_budget_after(
            proof_budget_for_test(2, 120),
            std::slice::from_ref(&existing),
            std::slice::from_ref(&new),
            &[],
            "head",
        );

        assert_eq!(remaining.max_focused_tests, 0);
        assert_eq!(remaining.max_focused_test_files, 0);
        assert_eq!(remaining.max_total_seconds, 0);
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
            revision: None,
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
            head: "head",
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
            head: "head",
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
    fn portfolio_does_not_reuse_receipt_from_another_head() -> Result<()> {
        let test = focused_test_task("parser", vec!["shared".to_owned()], 30);
        let request = proof_request("shared", true);
        let mut stale_receipt = test_receipt(&test.id, vec![request.id.clone()], "head_passed");
        stale_receipt.head = "previous-head".to_owned();
        let mut current_receipt = test_receipt(&test.id, vec![request.id.clone()], "head_passed");
        current_receipt.head = "CURRENT-HEAD".to_owned();
        assert!(current_receipt.head.eq_ignore_ascii_case("current-head"));
        assert!(!stale_receipt.head.eq_ignore_ascii_case("current-head"));
        assert!(receipt_matches_task_on_head(
            &current_receipt,
            &test.id,
            "current-head",
        ));
        assert!(!receipt_matches_task_on_head(
            &stale_receipt,
            &test.id,
            "current-head",
        ));
        assert!(receipt_matches_shared_request_on_head(
            &current_receipt,
            std::slice::from_ref(&request.id),
            "current-head",
        ));

        let stale_only_selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&test),
            build_tasks: &[],
            proof_requests: std::slice::from_ref(&request),
            proof_receipts: std::slice::from_ref(&stale_receipt),
            head: "current-head",
            budget: proof_budget_for_test(1, 60),
            runtime: portfolio_runtime_for_test(60, 4, Some(8_192), Some(20_000)),
        });
        assert_eq!(stale_only_selection.test_tasks.len(), 1);

        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&test),
            build_tasks: &[],
            proof_requests: std::slice::from_ref(&request),
            proof_receipts: &[stale_receipt.clone(), current_receipt.clone()],
            head: "current-head",
            budget: proof_budget_for_test(1, 60),
            runtime: portfolio_runtime_for_test(60, 4, Some(8_192), Some(20_000)),
        });

        assert_eq!(selection.test_tasks.len(), 0);
        let decision = selection
            .decisions
            .iter()
            .find(|decision| decision.task_id == current_receipt.id)
            .ok_or_else(|| anyhow::anyhow!("missing stale-receipt decision"))?;
        assert_eq!(decision.status, "answered_by_existing_receipt");
        assert_eq!(decision.receipt_ids, vec![current_receipt.id]);
        Ok(())
    }

    #[test]
    fn seeded_request_check_ignores_receipts_from_another_head() -> Result<()> {
        let request = proof_request("shared", true);
        let task = focused_test_candidates_from_requests(std::slice::from_ref(&request))
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("focused request should produce a task"))?;
        let mut stale_receipt = test_receipt(&task.id, task.request_ids.clone(), "head_passed");
        stale_receipt.head = "previous-head".to_owned();
        let current_head = "current-head";
        assert!(!stale_receipt.head.eq_ignore_ascii_case(current_head));

        assert!(has_unreceipted_proof_request_tasks(
            std::slice::from_ref(&request),
            std::slice::from_ref(&stale_receipt),
            current_head,
        ));
        assert!(!has_unreceipted_proof_request_tasks(
            std::slice::from_ref(&request),
            std::slice::from_ref(&test_receipt(&task.id, task.request_ids, "head_passed")),
            "head",
        ));
        Ok(())
    }

    #[test]
    fn current_head_receipts_filter_stale_entries_without_reordering() -> Result<()> {
        let current = test_receipt("current", Vec::new(), "head_passed");
        let mut stale = test_receipt("stale", Vec::new(), "head_passed");
        stale.head = "previous-head".to_owned();
        assert!(current.head.eq_ignore_ascii_case("HEAD"));
        assert!(!stale.head.eq_ignore_ascii_case("HEAD"));

        let receipts = [stale, current.clone()];
        let filtered = current_head_receipt_ids(&receipts, "HEAD");
        assert_eq!(filtered, BTreeSet::from([current.id]));
        Ok(())
    }

    #[test]
    fn current_head_receipt_ids_drive_both_unreceipted_task_filters() -> Result<()> {
        let test = focused_test_task("parser", vec!["parser".to_owned()], 30);
        let build = focused_build_task("build", vec!["build".to_owned()], 10);
        let current_test = test_receipt(&test.id, test.request_ids.clone(), "head_passed");
        let mut stale_build = test_receipt(&build.id, build.request_ids.clone(), "head_passed");
        stale_build.head = "previous-head".to_owned();
        let receipts = [current_test, stale_build];

        let remaining_tests = unreceipted_focused_test_tasks(vec![test.clone()], &receipts, "head");
        assert!(remaining_tests.is_empty());

        let remaining_builds =
            unreceipted_focused_build_tasks(vec![build.clone()], &receipts, "head");
        assert_eq!(
            remaining_builds
                .iter()
                .map(|task| task.id.clone())
                .collect::<Vec<_>>(),
            vec![build.id.clone()]
        );

        let current_build = test_receipt(&build.id, build.request_ids.clone(), "head_passed");
        let all_current_receipts = [
            test_receipt(&test.id, test.request_ids.clone(), "head_passed"),
            current_build,
        ];
        assert!(
            unreceipted_focused_test_tasks(vec![test], &all_current_receipts, "head").is_empty()
        );
        assert!(
            unreceipted_focused_build_tasks(vec![build], &all_current_receipts, "head").is_empty()
        );
        Ok(())
    }

    #[test]
    fn broker_keeps_same_named_tests_with_distinct_commands_separate() -> Result<()> {
        let binary = ProofRequest {
            command: "cargo test --locked --package ub-review --bin ub-review same_test".to_owned(),
            ..proof_request("binary", false)
        };
        let library = ProofRequest {
            command: "cargo test --locked --package ub-review --lib same_test".to_owned(),
            ..proof_request("library", false)
        };
        let mut candidates = focused_test_candidates_from_requests(std::slice::from_ref(&binary));
        let additional = focused_test_candidates_from_requests(std::slice::from_ref(&library));
        ensure!(candidates.len() == 1);
        ensure!(additional.len() == 1);

        merge_focused_test_candidates(&mut candidates, additional);

        assert_eq!(candidates.len(), 2);
        assert_eq!(
            candidates
                .iter()
                .flat_map(|task| task.request_ids.iter())
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([binary.id, library.id])
        );
        Ok(())
    }

    #[test]
    fn broker_merges_equivalent_focused_test_commands_and_request_ids() -> Result<()> {
        let first = ProofRequest {
            command: "cargo test --locked --package ub-review --bin ub-review same_test".to_owned(),
            ..proof_request("first", false)
        };
        let second = ProofRequest {
            command: first.command.clone(),
            ..proof_request("second", false)
        };
        let mut candidates = focused_test_candidates_from_requests(std::slice::from_ref(&first));
        let additional = focused_test_candidates_from_requests(std::slice::from_ref(&second));
        ensure!(candidates.len() == 1);
        ensure!(additional.len() == 1);

        merge_focused_test_candidates(&mut candidates, additional);

        assert_eq!(candidates.len(), 1);
        assert_eq!(
            candidates[0]
                .request_ids
                .iter()
                .cloned()
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([first.id, second.id])
        );
        Ok(())
    }

    #[test]
    fn broker_merge_preserves_required_obligations_in_either_order() -> Result<()> {
        for (existing_required, incoming_required, expected_required) in [
            (false, false, false),
            (false, true, true),
            (true, false, true),
            (true, true, true),
        ] {
            let mut existing = focused_test_task("shared", vec!["first".to_owned()], 15);
            existing.required = existing_required;
            let mut incoming = focused_test_task("shared", vec!["second".to_owned()], 30);
            incoming.required = incoming_required;
            incoming.requested_by = vec!["architecture".to_owned()];
            let mut candidates = vec![existing];

            for _ in 0..2 {
                merge_focused_test_candidates(&mut candidates, vec![incoming.clone()]);
                ensure!(candidates.len() == 1);
                let merged = candidates
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("merged candidate missing"))?;
                ensure!(merged.required == expected_required);
                ensure!(merged.timeout_sec == Some(30));
                ensure!(merged.request_ids == vec!["first".to_owned(), "second".to_owned()]);
                ensure!(
                    merged.requested_by
                        == vec!["tests-oracle".to_owned(), "architecture".to_owned()]
                );
            }
        }
        Ok(())
    }

    #[test]
    fn broker_merge_keeps_required_state_local_to_distinct_tasks() -> Result<()> {
        for (existing_required, incoming_required) in [(false, true), (true, false)] {
            let mut existing = focused_test_task("existing", Vec::new(), 15);
            existing.required = existing_required;
            let mut incoming = focused_test_task("incoming", Vec::new(), 30);
            incoming.required = incoming_required;
            let mut candidates = vec![existing];

            merge_focused_test_candidates(&mut candidates, vec![incoming]);

            let states = candidates
                .iter()
                .map(|task| (task.id.as_str(), task.required))
                .collect::<Vec<_>>();
            ensure!(
                states
                    == vec![
                        ("existing", existing_required),
                        ("incoming", incoming_required),
                    ]
            );
        }
        Ok(())
    }

    #[test]
    fn seeded_proof_requests_require_a_receipt_for_the_current_head() -> Result<()> {
        let seeded_proof_requests = vec![proof_request("seeded", true)];
        let task = focused_test_candidates_from_requests(&seeded_proof_requests)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("seeded request should produce a focused task"))?;
        let mut current_receipt = test_receipt(&task.id, task.request_ids.clone(), "head_passed");
        current_receipt.head = "CURRENT-HEAD".to_owned();

        assert!(!has_unreceipted_proof_request_tasks(
            &seeded_proof_requests,
            std::slice::from_ref(&current_receipt),
            "current-head",
        ));

        current_receipt.head = "PREVIOUS-HEAD".to_owned();
        assert!(has_unreceipted_proof_request_tasks(
            &seeded_proof_requests,
            std::slice::from_ref(&current_receipt),
            "current-head",
        ));
        Ok(())
    }

    #[test]
    fn seeded_stream_starts_current_head_request_broker_when_receipt_is_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let mut diff = test_diff();
        diff.head = "CURRENT-HEAD".to_owned();
        let mut profile = Profile::default();
        profile.budgets.proof_max_focused_test_files = 0;
        profile.budgets.proof_max_focused_tests = 0;
        profile.budgets.proof_total_timeout_sec = 0;
        let args = test_run_args(out.clone());
        let event_log = crate::EventLog::open(&temp.path().join("events.ndjson"))?;
        let run_started = Instant::now();
        let initial_loop = crate::start_run_loop(
            &event_log,
            &run_started,
            "proof",
            "proof",
            "initial-diff-broker",
        )?;

        let (_, phases) = run_seeded_proof_stream_v0(
            temp.path(),
            &out,
            &diff,
            &profile,
            &args,
            std::slice::from_ref(&proof_request("seeded", true)),
            initial_loop,
            &event_log,
            &run_started,
            &BoxState {
                cpus: 4,
                free_mem_mb: Some(8_192),
                free_disk_mb: Some(20_000),
                load_1m: Some(0.25),
                github_actions: false,
            },
            &build_impact_plan(temp.path(), &diff.changed_files, "shadow"),
            None,
        )?;

        assert!(
            phases
                .iter()
                .any(|phase| phase.stage == "seeded-request-broker")
        );
        let portfolio = fs::read_to_string(out.join("review/proof_portfolio.json"))?;
        assert!(portfolio.contains("seeded"));
        Ok(())
    }

    #[test]
    fn broker_replans_after_discriminating_test_receipt_before_build_wind_down() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let test_command = "cargo test --locked --test config_tests".to_owned();
        let proof_requests = vec![
            ProofRequest {
                schema: PROOF_REQUEST_SCHEMA.to_owned(),
                id: "required-test".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: test_command.clone(),
                reason: "Answer the required focused regression question.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 60,
                required: true,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: PROOF_REQUEST_SCHEMA.to_owned(),
                id: "architecture-build".to_owned(),
                lane: "architecture".to_owned(),
                requested_by: vec!["architecture".to_owned()],
                command: "cargo check --workspace --all-targets --locked".to_owned(),
                reason: "Answer the bounded architecture build question.".to_owned(),
                cost: "focused-build".to_owned(),
                timeout_sec: 60,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: PROOF_REQUEST_SCHEMA.to_owned(),
                id: "opposition-test".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command: test_command.clone(),
                reason: "Check the same focused regression independently.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 60,
                required: false,
                status: "requested".to_owned(),
            },
        ];
        let v2_requests = proof_requests
            .iter()
            .map(proof_request_to_v2)
            .collect::<Vec<_>>();
        let test_candidates = focused_test_candidates_from_v2(&v2_requests);
        let build_candidates = focused_build_candidates_from_v2(&v2_requests);
        ensure!(test_candidates.len() == 1);
        ensure!(build_candidates.len() == 1);

        let mut profile = Profile::default();
        profile.limits.tests = 1;
        profile.limits.builds = 1;
        profile.budgets.proof_max_focused_test_files = 1;
        profile.budgets.proof_max_focused_tests = 1;
        profile.budgets.proof_command_timeout_sec = 60;
        profile.budgets.proof_total_timeout_sec = 120;
        let args = test_run_args(out.clone());
        let box_state = BoxState {
            cpus: 4,
            free_mem_mb: Some(8_192),
            free_disk_mb: Some(20_000),
            load_1m: Some(0.25),
            github_actions: false,
        };
        let mut test_commands = Vec::new();
        let mut build_commands = Vec::new();
        let prepared_base_root = base_root.clone();
        let result = run_request_proof_broker_v0_with_runners(
            temp.path(),
            &out,
            &diff,
            &profile,
            &proof_requests,
            &[],
            &[],
            &args,
            &box_state,
            &Instant::now(),
            None,
            |_root, argv, env, timeout, stdout, stderr, _observe_process| {
                test_commands.push(command_display_with_env(env, argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                ensure!(timeout == 60);
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
                    reason: "fixture receipt".to_owned(),
                    duration_ms: 1,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
            |_root, argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                build_commands.push(argv.join(" "));
                Err(anyhow::anyhow!(
                    "build should remain unexecuted after test receipt"
                ))
            },
        )?;

        ensure!(test_commands.len() == 2);
        ensure!(build_commands.is_empty());
        ensure!(result.proof_receipts.len() == 1);
        ensure!(result.proof_receipts[0].result == "discriminating");
        ensure!(result.proof_receipts[0].request_ids.len() == 2);

        let artifact: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/proof_portfolio.json"))?)?;
        ensure!(
            artifact["selected_task_ids"]
                .as_array()
                .is_some_and(|tasks| tasks.is_empty())
        );
        let candidate_tasks = artifact["candidate_tasks"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing final portfolio candidate catalog"))?;
        ensure!(candidate_tasks.len() == 2);
        ensure!(
            artifact["candidate_count"].as_u64() == Some(candidate_tasks.len() as u64),
            "candidate_count must equal the published candidate catalog"
        );
        let decisions = artifact["decisions"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing final portfolio decisions"))?;
        for decision in decisions {
            let task_id = decision["task_id"].as_str().unwrap_or_default();
            let entry = candidate_tasks
                .iter()
                .find(|entry| entry["id"].as_str() == Some(task_id))
                .ok_or_else(|| anyhow::anyhow!("decision {task_id} missing from catalog"))?;
            ensure!(entry["kind"] == decision["kind"]);
            ensure!(entry["request_ids"] == decision["request_ids"]);
        }
        let test_decision = decisions
            .iter()
            .find(|decision| decision["task_id"].as_str() == Some(test_candidates[0].id.as_str()))
            .ok_or_else(|| anyhow::anyhow!("missing final test decision"))?;
        ensure!(test_decision["status"] == "answered_by_existing_receipt");
        ensure!(test_decision["receipt_ids"][0] == result.proof_receipts[0].id);
        let build_decision = decisions
            .iter()
            .find(|decision| decision["task_id"].as_str() == Some(build_candidates[0].id.as_str()))
            .ok_or_else(|| anyhow::anyhow!("missing final build decision"))?;
        ensure!(build_decision["status"] == "deferred_by_safe_wind_down");
        Ok(())
    }

    #[test]
    fn broker_satisfies_new_request_from_existing_receipt_without_execution() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let request = ProofRequest {
            schema: PROOF_REQUEST_SCHEMA.to_owned(),
            id: "new-request".to_owned(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            command: "cargo test --locked --test config_tests".to_owned(),
            reason: "Answer the newly arrived focused-test request.".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 60,
            required: true,
            status: "requested".to_owned(),
        };
        let v2_request = proof_request_to_v2(&request);
        let task = focused_test_candidates_from_v2(std::slice::from_ref(&v2_request))
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("request did not resolve to a focused test"))?;
        let receipt = ProofReceipt {
            revision: None,
            schema: PROOF_RECEIPT_SCHEMA.to_owned(),
            id: task.id.clone(),
            kind: "focused-red-green".to_owned(),
            base: diff.base.clone(),
            head: diff.head.clone(),
            test_patch_mode: "base-plus-tests".to_owned(),
            requested_by: task.requested_by.clone(),
            request_ids: vec![request.id.clone()],
            commands: Vec::new(),
            result: "discriminating".to_owned(),
            reason: "existing receipt answers the same focused proof".to_owned(),
        };
        let args = test_run_args(out.clone());
        let box_state = BoxState {
            cpus: 4,
            free_mem_mb: Some(8_192),
            free_disk_mb: Some(20_000),
            load_1m: Some(0.25),
            github_actions: false,
        };
        let result = run_request_proof_broker_v0_with_runners(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            std::slice::from_ref(&request),
            std::slice::from_ref(&receipt),
            &[],
            &args,
            &box_state,
            &Instant::now(),
            None,
            |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                Err(anyhow::anyhow!(
                    "existing evidence must prevent test execution"
                ))
            },
            |_root, _out, _diff| {
                Err(anyhow::anyhow!(
                    "existing evidence must prevent worktree preparation"
                ))
            },
            |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                Err(anyhow::anyhow!(
                    "existing evidence must prevent build execution"
                ))
            },
        )?;

        ensure!(result.proof_receipts.is_empty());
        let artifact: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/proof_portfolio.json"))?)?;
        let decision = artifact["decisions"]
            .as_array()
            .and_then(|decisions| {
                decisions
                    .iter()
                    .find(|decision| decision["task_id"].as_str() == Some(task.id.as_str()))
            })
            .ok_or_else(|| anyhow::anyhow!("missing existing-receipt portfolio decision"))?;
        ensure!(
            decision["status"] == "answered_by_existing_receipt",
            "task={} receipt={} decisions={}",
            task.id,
            receipt.id,
            artifact["decisions"]
        );
        ensure!(decision["receipt_ids"][0] == receipt.id);
        Ok(())
    }

    #[test]
    fn request_replan_retains_initial_diff_candidate_in_final_portfolio() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let mut diff = test_diff();
        diff.changed_files = vec!["test/js/bun/md/md-edge-cases.test.ts".to_owned()];
        diff.patch = "+test('parser regression', () => {});".to_owned();
        let initial_task = focused_test_candidates_from_diff(&diff, &[], None)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("changed test did not produce an initial task"))?;
        let initial_receipt = ProofReceipt {
            revision: None,
            schema: PROOF_RECEIPT_SCHEMA.to_owned(),
            id: initial_task.id.clone(),
            kind: "focused-red-green".to_owned(),
            base: diff.base.clone(),
            head: diff.head.clone(),
            test_patch_mode: "base-plus-tests".to_owned(),
            requested_by: initial_task.requested_by.clone(),
            request_ids: Vec::new(),
            commands: Vec::new(),
            result: "discriminating".to_owned(),
            reason: "initial diff proof receipt".to_owned(),
        };
        let build_request = ProofRequest {
            schema: PROOF_REQUEST_SCHEMA.to_owned(),
            id: "later-build".to_owned(),
            lane: "architecture".to_owned(),
            requested_by: vec!["architecture".to_owned()],
            command: "cargo check --workspace --all-targets --locked".to_owned(),
            reason: "later request exercises the final portfolio merge".to_owned(),
            cost: "focused-build".to_owned(),
            timeout_sec: 60,
            required: false,
            status: "requested".to_owned(),
        };
        let mut profile = Profile::default();
        profile.limits.builds = 0;
        let args = test_run_args(out.clone());
        let box_state = BoxState {
            cpus: 4,
            free_mem_mb: Some(8_192),
            free_disk_mb: Some(20_000),
            load_1m: Some(0.25),
            github_actions: false,
        };
        run_request_proof_broker_v0_with_runners(
            temp.path(),
            &out,
            &diff,
            &profile,
            std::slice::from_ref(&build_request),
            std::slice::from_ref(&initial_receipt),
            &[],
            &args,
            &box_state,
            &Instant::now(),
            None,
            |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                Err(anyhow::anyhow!(
                    "initial receipt must prevent rerunning the changed test"
                ))
            },
            |_root, _out, _diff| {
                Err(anyhow::anyhow!(
                    "initial receipt must prevent worktree preparation"
                ))
            },
            |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                Err(anyhow::anyhow!("build profile is disabled in this fixture"))
            },
        )?;

        let artifact: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/proof_portfolio.json"))?)?;
        let decision = artifact["decisions"]
            .as_array()
            .and_then(|decisions| {
                decisions
                    .iter()
                    .find(|decision| decision["task_id"].as_str() == Some(initial_task.id.as_str()))
            })
            .ok_or_else(|| anyhow::anyhow!("initial diff task disappeared from final portfolio"))?;
        ensure!(decision["status"] == "answered_by_existing_receipt");
        ensure!(decision["receipt_ids"][0] == initial_receipt.id);
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
            head: "head",
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
    fn empty_follow_up_proof_preserves_primary_portfolio() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        fs::create_dir_all(&review_dir)?;
        let portfolio_path = review_dir.join("proof_portfolio.json");

        ensure!(should_write_follow_up_portfolio(temp.path(), &[]));
        fs::write(&portfolio_path, br#"{"phase":"broker-final"}"#)?;
        ensure!(!should_write_follow_up_portfolio(temp.path(), &[]));

        let request = proof_request("new-follow-up", false);
        ensure!(should_write_follow_up_portfolio(
            temp.path(),
            std::slice::from_ref(&request)
        ));
        Ok(())
    }

    #[test]
    fn empty_follow_up_broker_preserves_existing_portfolio_artifact() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let review_dir = out.join("review");
        fs::create_dir_all(&review_dir)?;
        let portfolio_path = review_dir.join("proof_portfolio.json");
        let original = br#"{"phase":"primary","selected_task_ids":["task-1"]}"#;
        fs::write(&portfolio_path, original)?;

        let mut profile = Profile::default();
        profile.limits.tests = 1;
        profile.limits.builds = 1;
        profile.budgets.proof_max_focused_test_files = 1;
        profile.budgets.proof_max_focused_tests = 1;
        profile.budgets.proof_command_timeout_sec = 60;
        profile.budgets.proof_total_timeout_sec = 120;
        let args = test_run_args(out.clone());
        let box_state = BoxState {
            cpus: 4,
            free_mem_mb: Some(8_192),
            free_disk_mb: Some(20_000),
            load_1m: Some(0.25),
            github_actions: false,
        };
        let diff = test_diff();
        let result = run_follow_up_proof_broker_v0(
            temp.path(),
            &out,
            &diff,
            &profile,
            &[],
            &[],
            &[],
            &args,
            &box_state,
            &Instant::now(),
            None,
        )?;

        assert_eq!(result.proof_receipts.len(), 0);
        assert_eq!(result.resource_leases.len(), 0);
        assert_eq!(fs::read(&portfolio_path)?, original);
        Ok(())
    }

    /// #4271 round 2: a follow-up-only portfolio write must not erase the
    /// primary broker's required obligations. After a primary write with
    /// deferred required task A and a follow-up write with optional task
    /// B, the merged artifact retains both decisions; the gate accounting
    /// counts required A as unproven with its reason, excludes optional B,
    /// and the summary mentions A.
    #[test]
    fn nonempty_follow_up_write_merges_primary_required_decisions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let task_a = focused_test_task("task-a", vec!["req-a".to_owned()], 600);
        let requests_a = vec![proof_request("req-a", true)];
        let selection_a = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&task_a),
            build_tasks: &[],
            proof_requests: &requests_a,
            proof_receipts: &[],
            head: &diff.head,
            budget: proof_budget_for_test(0, 300),
            runtime: portfolio_runtime_for_test(300, 4, Some(8_192), Some(20_000)),
        });
        write_proof_portfolio_selection_artifact(
            &out,
            &diff,
            proof_budget_for_test(0, 300),
            std::slice::from_ref(&task_a),
            &[],
            &requests_a,
            selection_a,
        )?;
        let task_b = focused_test_task("task-b", vec!["req-b".to_owned()], 300);
        let requests_b = vec![proof_request("req-b", false)];
        let selection_b = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&task_b),
            build_tasks: &[],
            proof_requests: &requests_b,
            proof_receipts: &[],
            head: &diff.head,
            budget: proof_budget_for_test(0, 300),
            runtime: portfolio_runtime_for_test(300, 4, Some(8_192), Some(20_000)),
        });
        write_proof_portfolio_selection_artifact(
            &out,
            &diff,
            proof_budget_for_test(0, 300),
            std::slice::from_ref(&task_b),
            &[],
            &requests_b,
            selection_b,
        )?;
        let text = fs::read_to_string(out.join("review/proof_portfolio.json"))?;
        let snapshot: crate::gate::PlannerPortfolioSnapshot = serde_json::from_str(&text)?;
        ensure!(
            snapshot.decisions.len() == 2,
            "{}",
            snapshot.decisions.len()
        );
        let decision_a = snapshot
            .decisions
            .iter()
            .find(|decision| decision.task_id == "task-a")
            .ok_or_else(|| anyhow::anyhow!("merged portfolio lost primary task-a"))?;
        ensure!(decision_a.required);
        ensure!(decision_a.status == "deferred_by_safe_wind_down");
        ensure!(decision_a.reason == "proof task capacity is exhausted");
        ensure!(
            snapshot
                .decisions
                .iter()
                .any(|decision| decision.task_id == "task-b" && !decision.required)
        );
        let accounting = crate::gate::build_planner_required_proof_accounting(
            &snapshot,
            &[],
            0,
            Some("merge-test-digest"),
        );
        ensure!(accounting.total == 1);
        ensure!(accounting.proven == 0);
        ensure!(accounting.unproven == 1);
        ensure!(accounting.unproven_tasks[0].task_id == "task-a");
        fs::write(out.join("review/metrics.json"), "{}")?;
        let gate = serde_json::json!({
            "schema": "ub-review.gate_outcome.v1",
            "conclusion": "pass",
            "reasons": [],
            "planner_required_proofs": serde_json::to_value(&accounting)?,
            "revision": {"digest": "merge-test-digest", "semantics": "merge_result", "reviewed_commit": "c"},
        });
        fs::write(
            out.join("review/gate_outcome.json"),
            serde_json::to_vec_pretty(&gate)?,
        )?;
        let mut summary = String::new();
        crate::render_review_efficiency_section(&mut summary, &out);
        ensure!(summary.contains("task-a"), "{summary}");
        ensure!(summary.contains("req-a"), "{summary}");
        ensure!(
            summary.contains("proof task capacity is exhausted"),
            "{summary}"
        );
        ensure!(summary.contains("receipts: none"), "{summary}");
        ensure!(summary.contains("merge-test-digest"), "{summary}");
        Ok(())
    }

    /// Round 2b (CodeRabbit 4067904921): task ids omit request ids, so an
    /// optional follow-up request mapping to the SAME task id must not
    /// overwrite the earlier required obligation. The merged decision stays
    /// required with both request ids, the accounting still counts it, and
    /// the summary mentions it.
    #[test]
    fn follow_up_write_preserves_required_flag_on_shared_task_id() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let write_one =
            |task: &FocusedTestTask, requests: &[ProofRequest], out: &std::path::Path| {
                let selection = select_proof_portfolio(ProofPortfolioInput {
                    test_tasks: std::slice::from_ref(task),
                    build_tasks: &[],
                    proof_requests: requests,
                    proof_receipts: &[],
                    head: &diff.head,
                    budget: proof_budget_for_test(0, 300),
                    runtime: portfolio_runtime_for_test(300, 4, Some(8_192), Some(20_000)),
                });
                write_proof_portfolio_selection_artifact(
                    out,
                    &diff,
                    proof_budget_for_test(0, 300),
                    std::slice::from_ref(task),
                    &[],
                    requests,
                    selection,
                )
            };
        let task_a = focused_test_task("task-same", vec!["req-a".to_owned()], 600);
        write_one(&task_a, &[proof_request("req-a", true)], &out)?;
        let task_b = focused_test_task("task-same", vec!["req-b".to_owned()], 300);
        write_one(&task_b, &[proof_request("req-b", false)], &out)?;
        let text = fs::read_to_string(out.join("review/proof_portfolio.json"))?;
        let snapshot: crate::gate::PlannerPortfolioSnapshot = serde_json::from_str(&text)?;
        ensure!(
            snapshot.decisions.len() == 1,
            "{}",
            snapshot.decisions.len()
        );
        let merged = &snapshot.decisions[0];
        ensure!(merged.task_id == "task-same");
        ensure!(merged.required);
        ensure!(
            merged.request_ids == vec!["req-a".to_owned(), "req-b".to_owned()],
            "{:?}",
            merged.request_ids
        );
        let catalog: serde_json::Value = serde_json::from_str(&text)?;
        let candidate = catalog["candidate_tasks"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("missing portfolio candidate catalog"))?
            .first()
            .ok_or_else(|| anyhow::anyhow!("empty portfolio candidate catalog"))?
            .clone();
        ensure!(candidate["id"] == "task-same", "{candidate}");
        ensure!(
            candidate["required"] == serde_json::to_value(merged.required)?,
            "{candidate}"
        );
        ensure!(
            candidate["request_ids"] == serde_json::to_value(&merged.request_ids)?,
            "{candidate}"
        );
        let accounting = crate::gate::build_planner_required_proof_accounting(
            &snapshot,
            &[],
            0,
            Some("shared-id-digest"),
        );
        ensure!(accounting.total == 1);
        ensure!(accounting.unproven == 1);
        ensure!(accounting.unproven_tasks[0].task_id == "task-same");
        fs::write(out.join("review/metrics.json"), "{}")?;
        let gate = serde_json::json!({
            "schema": "ub-review.gate_outcome.v1",
            "conclusion": "pass",
            "reasons": [],
            "planner_required_proofs": serde_json::to_value(&accounting)?,
            "revision": {"digest": "shared-id-digest", "semantics": "merge_result", "reviewed_commit": "c"},
        });
        fs::write(
            out.join("review/gate_outcome.json"),
            serde_json::to_vec_pretty(&gate)?,
        )?;
        let mut summary = String::new();
        crate::render_review_efficiency_section(&mut summary, &out);
        ensure!(summary.contains("task-same"), "{summary}");
        ensure!(summary.contains("req-a"), "{summary}");
        ensure!(summary.contains("req-b"), "{summary}");
        Ok(())
    }

    /// Round 2b (CodeRabbit 4067904925): a reused output directory under a
    /// different head must not absorb stale decisions. The second head's
    /// write contains only its own decisions.
    #[test]
    fn portfolio_write_under_new_head_discards_stale_decisions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let task_a = focused_test_task("task-a", vec!["req-a".to_owned()], 600);
        let requests_a = vec![proof_request("req-a", true)];
        let selection_a = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&task_a),
            build_tasks: &[],
            proof_requests: &requests_a,
            proof_receipts: &[],
            head: &diff.head,
            budget: proof_budget_for_test(0, 300),
            runtime: portfolio_runtime_for_test(300, 4, Some(8_192), Some(20_000)),
        });
        write_proof_portfolio_selection_artifact(
            &out,
            &diff,
            proof_budget_for_test(0, 300),
            std::slice::from_ref(&task_a),
            &[],
            &requests_a,
            selection_a,
        )?;
        let mut other_diff = test_diff();
        other_diff.head = "other-head".to_owned();
        let task_b = focused_test_task("task-b", vec!["req-b".to_owned()], 300);
        let requests_b = vec![proof_request("req-b", false)];
        let selection_b = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&task_b),
            build_tasks: &[],
            proof_requests: &requests_b,
            proof_receipts: &[],
            head: &other_diff.head,
            budget: proof_budget_for_test(0, 300),
            runtime: portfolio_runtime_for_test(300, 4, Some(8_192), Some(20_000)),
        });
        write_proof_portfolio_selection_artifact(
            &out,
            &other_diff,
            proof_budget_for_test(0, 300),
            std::slice::from_ref(&task_b),
            &[],
            &requests_b,
            selection_b,
        )?;
        let text = fs::read_to_string(out.join("review/proof_portfolio.json"))?;
        let snapshot: crate::gate::PlannerPortfolioSnapshot = serde_json::from_str(&text)?;
        ensure!(
            snapshot.decisions.len() == 1,
            "{}",
            snapshot.decisions.len()
        );
        ensure!(snapshot.decisions[0].task_id == "task-b");
        let artifact: serde_json::Value = serde_json::from_str(&text)?;
        ensure!(artifact.get("head").and_then(|head| head.as_str()) == Some("other-head"));
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
            head: "head",
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
            head: "head",
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
        ensure!(
            decision.reason
                == "estimated task cost cannot fit the remaining hard-deadline wall time"
        );
        Ok(())
    }

    #[test]
    fn portfolio_reports_the_actual_safe_wind_down_constraint() -> Result<()> {
        let test = focused_test_task("parser", vec!["request".to_owned()], 30);
        let cases = [
            (
                proof_budget_for_test(1, 0),
                portfolio_runtime_for_test(3_406, 4, Some(8_192), Some(20_000)),
                "proof-time allocation is exhausted",
                "deferred_by_safe_wind_down",
            ),
            (
                proof_budget_for_test(1, 60),
                portfolio_runtime_for_test(0, 4, Some(8_192), Some(20_000)),
                "hard deadline has expired; proof was safely wound down",
                "deferred_by_safe_wind_down",
            ),
            (
                proof_budget_for_test(1, 60),
                portfolio_runtime_for_test(59, 4, Some(8_192), Some(20_000)),
                "estimated task cost cannot fit the remaining hard-deadline wall time",
                "deferred_by_safe_wind_down",
            ),
            (
                proof_budget_for_test(1, 59),
                portfolio_runtime_for_test(3_406, 4, Some(8_192), Some(20_000)),
                "estimated task cost cannot fit the remaining proof-time allocation",
                "declined_for_higher_value_proof",
            ),
            (
                proof_budget_for_test(0, 60),
                portfolio_runtime_for_test(60, 4, Some(8_192), Some(20_000)),
                "proof task capacity is exhausted",
                "deferred_by_safe_wind_down",
            ),
            (
                ProofBudget {
                    max_focused_test_files: 0,
                    ..proof_budget_for_test(1, 60)
                },
                portfolio_runtime_for_test(60, 4, Some(8_192), Some(20_000)),
                "focused test-file capacity is exhausted",
                "deferred_by_safe_wind_down",
            ),
            (
                proof_budget_for_test(1, 60),
                portfolio_runtime_for_test(60, 0, Some(8_192), Some(20_000)),
                "proof lease does not fit the current runner capacity",
                "declined_for_box_capacity",
            ),
            (
                proof_budget_for_test(1, 60),
                portfolio_runtime_for_test(60, 4, Some(511), Some(20_000)),
                "proof lease does not fit the current runner capacity",
                "declined_for_box_capacity",
            ),
            (
                proof_budget_for_test(1, 60),
                portfolio_runtime_for_test(60, 4, Some(8_192), Some(63)),
                "proof lease does not fit the current runner capacity",
                "declined_for_box_capacity",
            ),
            // Resource, deadline, allocation, task, and file constraints retain
            // their deterministic precedence when several apply together.
            (
                proof_budget_for_test(0, 0),
                portfolio_runtime_for_test(0, 0, Some(0), Some(0)),
                "proof lease does not fit the current runner capacity",
                "declined_for_box_capacity",
            ),
            (
                proof_budget_for_test(0, 0),
                portfolio_runtime_for_test(0, 4, Some(8_192), Some(20_000)),
                "hard deadline has expired; proof was safely wound down",
                "deferred_by_safe_wind_down",
            ),
            (
                ProofBudget {
                    max_focused_test_files: 0,
                    ..proof_budget_for_test(0, 0)
                },
                portfolio_runtime_for_test(3_406, 4, Some(8_192), Some(20_000)),
                "proof-time allocation is exhausted",
                "deferred_by_safe_wind_down",
            ),
            (
                ProofBudget {
                    max_focused_test_files: 0,
                    ..proof_budget_for_test(0, 60)
                },
                portfolio_runtime_for_test(59, 4, Some(8_192), Some(20_000)),
                "proof task capacity is exhausted",
                "deferred_by_safe_wind_down",
            ),
            (
                ProofBudget {
                    max_focused_test_files: 0,
                    ..proof_budget_for_test(1, 60)
                },
                portfolio_runtime_for_test(59, 4, Some(8_192), Some(20_000)),
                "focused test-file capacity is exhausted",
                "deferred_by_safe_wind_down",
            ),
        ];

        for (budget, runtime, expected_reason, expected_status) in cases {
            for required in [false, true] {
                let request = proof_request("request", required);
                let selection = select_proof_portfolio(ProofPortfolioInput {
                    test_tasks: std::slice::from_ref(&test),
                    build_tasks: &[],
                    proof_requests: std::slice::from_ref(&request),
                    proof_receipts: &[],
                    head: "head",
                    budget,
                    runtime,
                });
                ensure!(selection.test_tasks.is_empty());
                let decision = selection
                    .decisions
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("missing safe-wind-down decision"))?;
                ensure!(decision.status == expected_status, "{decision:?}");
                ensure!(decision.required == required);
                ensure!(decision.receipt_ids.is_empty());
                ensure!(decision.estimated_cost_sec == 60);
                ensure!(
                    decision.reason == expected_reason,
                    "expected {expected_reason:?}, got {:?}",
                    decision.reason
                );
                let temp = tempfile::tempdir()?;
                let out = temp.path().join("out");
                write_proof_portfolio_selection_artifact(
                    &out,
                    &test_diff(),
                    budget,
                    std::slice::from_ref(&test),
                    &[],
                    std::slice::from_ref(&request),
                    selection,
                )?;
                let snapshot =
                    serde_json::from_slice(&fs::read(out.join("review/proof_portfolio.json"))?)?;
                let accounting = crate::gate::build_planner_required_proof_accounting(
                    &snapshot,
                    &[],
                    0,
                    Some("constraint-digest"),
                );
                ensure!(accounting.total == usize::from(required));
                ensure!(accounting.proven == 0);
                ensure!(accounting.unproven == usize::from(required));
                if required {
                    ensure!(accounting.unproven_tasks[0].reason == expected_reason);
                    fs::write(out.join("review/metrics.json"), "{}")?;
                    fs::write(
                        out.join("review/gate_outcome.json"),
                        serde_json::to_vec(&serde_json::json!({
                            "schema": "ub-review.gate_outcome.v1",
                            "conclusion": "inconclusive",
                            "reasons": [],
                            "planner_required_proofs": accounting,
                        }))?,
                    )?;
                    let mut summary = String::new();
                    crate::render_review_efficiency_section(&mut summary, &out);
                    ensure!(summary.contains(expected_reason), "{summary}");
                    ensure!(summary.contains("receipts: none"), "{summary}");
                }
            }
        }
        Ok(())
    }

    #[test]
    fn portfolio_artifact_preserves_the_live_deadline_after_estimated_reservations() -> Result<()> {
        let requests = [
            proof_request("request-a", true),
            proof_request("request-b", true),
        ];
        let tasks = [
            focused_test_task("a", vec!["request-a".to_owned()], 30),
            focused_test_task("b", vec!["request-b".to_owned()], 30),
        ];
        let budget = proof_budget_for_test(2, 120);
        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: &tasks,
            build_tasks: &[],
            proof_requests: &requests,
            proof_receipts: &[],
            head: "HEAD",
            budget,
            runtime: portfolio_runtime_for_test(90, 4, Some(8_192), Some(20_000)),
        });
        let temp = tempfile::tempdir()?;
        write_proof_portfolio_selection_artifact(
            temp.path(),
            &test_diff(),
            budget,
            &tasks,
            &[],
            &requests,
            selection,
        )?;
        let artifact: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("review/proof_portfolio.json"))?)?;
        // Reservation estimates constrain selection while the published runtime
        // retains the live deadline supplied to this invocation.
        assert_eq!(
            artifact,
            serde_json::json!({
                "schema": "ub-review.proof_portfolio.v1",
                "phase": "broker-final",
                "head": "HEAD",
                "budget_seconds": 120,
                "remaining_seconds": 30,
                "candidate_count": 2,
                "candidate_tasks": [
                    {
                        "id": "a", "kind": "focused-red-green", "required": true,
                        "estimated_cost_sec": 60, "request_ids": ["request-a"]
                    },
                    {
                        "id": "b", "kind": "focused-red-green", "required": true,
                        "estimated_cost_sec": 60, "request_ids": ["request-b"]
                    }
                ],
                "selected_task_ids": ["a"],
                "runtime": {
                    "cpus": 4, "free_mem_mb": 8192, "free_disk_mb": 20000,
                    "load_1m": 0.25, "github_actions": false,
                    "deadline_remaining_seconds": 90,
                    "lease": {
                        "cpu": 1, "memory_mb": 512, "disk_mb": 64,
                        "network": false, "scratch": true
                    }
                },
                "decisions": [
                    {
                        "task_id": "a", "kind": "focused-red-green", "status": "selected",
                        "reason": "selected for value-ranked execution; serves 1 request(s)",
                        "required": true, "estimated_cost_sec": 60,
                        "request_ids": ["request-a"], "receipt_ids": []
                    },
                    {
                        "task_id": "b", "kind": "focused-red-green",
                        "status": "deferred_by_safe_wind_down",
                        "reason": "estimated task cost cannot fit the remaining hard-deadline wall time",
                        "required": true, "estimated_cost_sec": 60,
                        "request_ids": ["request-b"], "receipt_ids": []
                    }
                ]
            })
        );
        Ok(())
    }

    #[test]
    fn portfolio_reasons_account_for_selected_work_and_file_reuse() -> Result<()> {
        let first = focused_test_task("a", Vec::new(), 30);
        let second = focused_test_task("b", Vec::new(), 30);
        for (max_tasks, allocation, deadline, max_files, shared_file, expected_reason) in [
            (
                2,
                60,
                3_406,
                2,
                false,
                Some("proof-time allocation is exhausted"),
            ),
            (
                2,
                90,
                3_406,
                2,
                false,
                Some("estimated task cost cannot fit the remaining proof-time allocation"),
            ),
            (
                2,
                120,
                90,
                2,
                false,
                Some("estimated task cost cannot fit the remaining hard-deadline wall time"),
            ),
            (
                2,
                120,
                60,
                2,
                false,
                Some("estimated task cost cannot fit the remaining hard-deadline wall time"),
            ),
            (
                1,
                120,
                3_406,
                2,
                false,
                Some("proof task capacity is exhausted"),
            ),
            (
                2,
                120,
                3_406,
                1,
                false,
                Some("focused test-file capacity is exhausted"),
            ),
            (2, 120, 3_406, 1, true, None),
        ] {
            let mut tasks = [first.clone(), second.clone()];
            if shared_file {
                tasks[1].file = first.file.clone();
            }
            let selection = select_proof_portfolio(ProofPortfolioInput {
                test_tasks: &tasks,
                build_tasks: &[],
                proof_requests: &[],
                proof_receipts: &[],
                head: "head",
                budget: ProofBudget {
                    max_focused_test_files: max_files,
                    ..proof_budget_for_test(max_tasks, allocation)
                },
                runtime: portfolio_runtime_for_test(deadline, 4, Some(8_192), Some(20_000)),
            });
            ensure!(selection.decisions[0].status == "selected");
            ensure!(
                selection.remaining_seconds
                    == allocation
                        .min(deadline)
                        .saturating_sub(if expected_reason.is_some() { 60 } else { 120 })
            );
            if let Some(reason) = expected_reason {
                ensure!(selection.test_tasks.len() == 1);
                ensure!(
                    selection.decisions[1].reason == reason,
                    "{:?}",
                    selection.decisions
                );
                ensure!(
                    selection.decisions[1].status
                        == if deadline < allocation {
                            "deferred_by_safe_wind_down"
                        } else {
                            "declined_for_higher_value_proof"
                        }
                );
            } else {
                ensure!(selection.test_tasks.len() == 2);
                ensure!(selection.decisions[1].status == "selected");
            }
        }
        Ok(())
    }

    #[test]
    fn portfolio_build_and_exact_receipt_bypass_test_file_capacity() -> Result<()> {
        let request = proof_request("request", true);
        let test = focused_test_task("parser", vec![request.id.clone()], 30);
        let build = focused_build_task("build", Vec::new(), 30);
        let receipt = test_receipt(&test.id, test.request_ids.clone(), "head_passed");
        let mut stale = receipt.clone();
        stale.head = "other-head".to_owned();
        let disjoint = test_receipt(
            "other-task",
            vec!["other-request".to_owned()],
            "head_passed",
        );
        for (receipts, expected_status) in [
            (vec![stale, disjoint], "deferred_by_safe_wind_down"),
            (vec![receipt], "answered_by_existing_receipt"),
        ] {
            let selection = select_proof_portfolio(ProofPortfolioInput {
                test_tasks: std::slice::from_ref(&test),
                build_tasks: std::slice::from_ref(&build),
                proof_requests: std::slice::from_ref(&request),
                proof_receipts: &receipts,
                head: "head",
                budget: ProofBudget {
                    max_focused_test_files: 0,
                    ..proof_budget_for_test(1, 60)
                },
                runtime: portfolio_runtime_for_test(60, 4, Some(8_192), Some(20_000)),
            });
            ensure!(selection.test_tasks.is_empty());
            ensure!(selection.build_tasks.len() == 1);
            ensure!(selection.decisions[0].required);
            ensure!(selection.decisions[0].status == expected_status);
            ensure!(selection.decisions[1].status == "selected");
            ensure!(selection.remaining_seconds == 30);
            if expected_status == "answered_by_existing_receipt" {
                ensure!(selection.decisions[0].receipt_ids == vec![test.id.clone()]);
            } else {
                ensure!(selection.decisions[0].reason == "focused test-file capacity is exhausted");
                ensure!(selection.decisions[0].receipt_ids.is_empty());
            }
        }
        Ok(())
    }

    #[test]
    fn portfolio_receipt_precedes_exhausted_allocation_deadline_and_resources() -> Result<()> {
        let test = focused_test_task("parser", Vec::new(), 30);
        let receipt = test_receipt(&test.id, Vec::new(), "head_passed");
        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: std::slice::from_ref(&test),
            build_tasks: &[],
            proof_requests: &[],
            proof_receipts: std::slice::from_ref(&receipt),
            head: "head",
            budget: proof_budget_for_test(0, 0),
            runtime: portfolio_runtime_for_test(0, 0, Some(0), Some(0)),
        });
        ensure!(selection.test_tasks.is_empty());
        ensure!(selection.remaining_seconds == 0);
        ensure!(selection.decisions[0].status == "answered_by_existing_receipt");
        ensure!(selection.decisions[0].receipt_ids == vec![test.id]);
        Ok(())
    }

    #[test]
    fn portfolio_deadline_cost_boundary_is_inclusive_and_saturating() -> Result<()> {
        for (cost, allocation, deadline, selected) in [
            (30, 100, 60, true),
            (30, 100, 59, false),
            (30, 60, 3_406, true),
            (30, 59, 3_406, false),
            (u64::MAX, u64::MAX, u64::MAX, true),
            (u64::MAX, u64::MAX, u64::MAX - 1, false),
        ] {
            let test = focused_test_task("parser", Vec::new(), cost);
            let selection = select_proof_portfolio(ProofPortfolioInput {
                test_tasks: std::slice::from_ref(&test),
                build_tasks: &[],
                proof_requests: &[],
                proof_receipts: &[],
                head: "head",
                budget: ProofBudget {
                    per_command_timeout_sec: u64::MAX,
                    ..proof_budget_for_test(1, allocation)
                },
                runtime: portfolio_runtime_for_test(deadline, 4, Some(8_192), Some(20_000)),
            });
            ensure!((!selection.test_tasks.is_empty()) == selected);
            ensure!(selection.decisions[0].estimated_cost_sec == cost.saturating_mul(2));
            ensure!(
                selection.remaining_seconds
                    == if selected {
                        0
                    } else {
                        allocation.min(deadline)
                    }
            );
        }
        // Preserve existing saturating estimated-cost admission, including
        // addition after an earlier selected task; these are not elapsed times.
        let tasks = [
            focused_test_task("small", Vec::new(), 1),
            focused_test_task("saturating", Vec::new(), u64::MAX),
        ];
        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: &tasks,
            build_tasks: &[],
            proof_requests: &[],
            proof_receipts: &[],
            head: "head",
            budget: ProofBudget {
                per_command_timeout_sec: u64::MAX,
                ..proof_budget_for_test(2, u64::MAX)
            },
            runtime: portfolio_runtime_for_test(u64::MAX, 4, Some(8_192), Some(20_000)),
        });
        ensure!(selection.test_tasks.len() == 2);
        ensure!(selection.remaining_seconds == 0);
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
