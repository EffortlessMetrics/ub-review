//! Focused proof task and plan types.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::test_parse::{
    command_display, command_display_with_env, focused_test_names_for_file, push_unique,
};
use crate::*;

#[derive(Clone, Debug)]
pub(crate) struct FocusedTestTask {
    pub(crate) id: String,
    pub(crate) file: String,
    pub(crate) test_name: Option<String>,
    pub(crate) mode: FocusedProofMode,
    pub(crate) command_specs: Option<FocusedTestCommandSpecs>,
    pub(crate) timeout_sec: Option<u64>,
    pub(crate) required: bool,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct FocusedTestCommandSpecs {
    pub(crate) head: ProofCommandSpec,
    pub(crate) base_plus_tests: ProofCommandSpec,
}

#[derive(Clone, Debug)]
pub(crate) struct FocusedBuildTask {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) argv: Vec<String>,
    pub(crate) timeout_sec: u64,
    pub(crate) required: bool,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
}

pub(crate) fn proof_task_command_spec(task: &FocusedTestTask, side: &str) -> ProofCommandSpec {
    if let Some(command_specs) = &task.command_specs {
        return if side == "head" {
            command_specs.head.clone()
        } else {
            command_specs.base_plus_tests.clone()
        };
    }
    let mut env = BTreeMap::new();
    let mut argv = if side == "head" {
        vec![
            "bun".to_owned(),
            "bd".to_owned(),
            "test".to_owned(),
            task.file.clone(),
        ]
    } else {
        env.insert("USE_SYSTEM_BUN".to_owned(), "1".to_owned());
        vec!["bun".to_owned(), "test".to_owned(), task.file.clone()]
    };
    if let Some(name) = &task.test_name {
        argv.push("-t".to_owned());
        argv.push(name.clone());
    }
    ProofCommandSpec { argv, env }
}

pub(crate) fn proof_task_plan_command(
    task: &FocusedTestTask,
    side: &str,
    worktree: &str,
) -> String {
    let spec = proof_task_command_spec(task, side);
    format!(
        "cwd=target/ub-review/proof-worktrees/{worktree} {}",
        command_display_with_env(&spec.env, &spec.argv)
    )
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProofPlannerRuntimeBudget {
    pub(crate) target_timeout_sec: u64,
    pub(crate) hard_timeout_sec: u64,
    pub(crate) max_focused_tests: usize,
    pub(crate) per_command_timeout_sec: u64,
    pub(crate) total_proof_timeout_sec: u64,
}

pub(crate) fn canonical_proof_request_group_command(command: &str, cost: &str) -> String {
    if cost != "focused-test" {
        return command.to_owned();
    }
    let parts = command.split_whitespace().collect::<Vec<_>>();
    let Some((file, args)) = focused_bun_request_parts(&parts) else {
        return command.to_owned();
    };
    format!(
        "focused-bun:{}:{}",
        normalize_repo_path(file),
        focused_test_name_arg(args).unwrap_or_default()
    )
}

pub(crate) fn focused_proof_plans_from_diff(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
    impact: Option<&ImpactPlan>,
    budget: ProofBudget,
) -> Vec<FocusedProofPlan> {
    focused_test_tasks_from_diff(diff, proof_requests, impact, budget)
        .into_iter()
        .map(|task| {
            focused_proof_plan_for_task(
                task,
                budget,
                "planned",
                format!(
                    "planner-only focused test target under budget: max {} file(s), {} test(s), {}s per command, {}s total",
                    budget.max_focused_test_files,
                    budget.max_focused_tests,
                    budget.per_command_timeout_sec,
                    budget.max_total_seconds
                ),
            )
        })
        .collect()
}

pub(crate) fn focused_proof_candidate_plans_from_diff(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
    impact: Option<&ImpactPlan>,
    budget: ProofBudget,
) -> Vec<FocusedProofPlan> {
    let planned_ids = focused_test_tasks_from_diff(diff, proof_requests, impact, budget)
        .into_iter()
        .map(|task| task.id)
        .collect::<BTreeSet<_>>();
    let candidate_tasks = focused_test_candidates_from_diff(diff, proof_requests, impact);
    let mut plans = Vec::with_capacity(candidate_tasks.len());
    for task in candidate_tasks {
        let status = candidate_plan_status(planned_ids.contains(&task.id));
        plans.extend(std::iter::once(focused_proof_plan_for_task(
            task,
            budget,
            status,
            "candidate recorded for portfolio accounting; execution is budget-gated".to_owned(),
        )));
    }
    plans
}

fn candidate_plan_status(planned: bool) -> &'static str {
    if planned {
        "planned"
    } else {
        "deferred_by_budget"
    }
}

fn focused_proof_plan_for_task(
    task: FocusedTestTask,
    budget: ProofBudget,
    status: &str,
    reason: String,
) -> FocusedProofPlan {
    let timeout_sec = focused_test_task_command_timeout(&task, budget);
    let head_command = proof_task_plan_command(&task, "head", "head");
    let base_plus_tests_command = if task.mode == FocusedProofMode::RedGreen {
        proof_task_plan_command(&task, "base-plus-tests", "base-plus-tests")
    } else {
        "not planned for head-only proof".to_owned()
    };
    FocusedProofPlan {
        id: task.id,
        test_file: task.file,
        test_name: task.test_name,
        mode: task.mode,
        timeout_sec,
        head_command,
        base_plus_tests_command,
        requested_by: task.requested_by,
        request_ids: task.request_ids,
        status: status.to_owned(),
        reason,
    }
}

/// Admit candidates in order while they fit the budget.
///
/// #838: this used to `return` at the first rejection, so one expensive
/// candidate blocked every later, cheaper one — a 300s red/green candidate
/// could hide a 30s head-only cargo test that fit the remaining time easily.
/// Rejection is now per candidate: scanning continues and later candidates are
/// admitted whenever they independently fit the remaining task, file, and time
/// budgets. Input order is unchanged, so the admitted sequence stays
/// deterministic. Rejected candidates are not silently dropped —
/// [`focused_proof_candidate_plans_from_diff`] records them as
/// `deferred_by_budget` in the planner artifact.
pub(crate) fn focused_test_tasks_from_diff(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
    impact: Option<&ImpactPlan>,
    budget: ProofBudget,
) -> Vec<FocusedTestTask> {
    let candidates = focused_test_candidates_from_diff(diff, proof_requests, impact);
    let mut tasks = Vec::new();
    let mut files = BTreeSet::new();
    let mut estimated_seconds = 0_u64;
    for task in candidates {
        let task_timeout_sec = focused_test_task_command_timeout(&task, budget);
        if !focused_proof_budget_allows_next(
            tasks.len(),
            &files,
            &task.file,
            estimated_seconds,
            task_timeout_sec,
            task.mode.command_count(),
            budget,
        ) {
            continue;
        }
        files.insert(task.file.clone());
        estimated_seconds = estimated_seconds
            .saturating_add(task_timeout_sec.saturating_mul(task.mode.command_count()));
        tasks.push(task);
    }
    tasks
}

/// How many impact-plan test targets may enter the deterministic candidate
/// floor. The portfolio selector and the runtime budget still decide what
/// actually executes; this only bounds how much of the ranked catalog is
/// offered, so a wide refactor cannot flood the portfolio with hundreds of
/// equally ranked targets.
pub(crate) const MAX_IMPACT_CARGO_TEST_CANDIDATES: usize = 8;

/// Deterministic Cargo floor: turn the impact plan's ranked `test` targets
/// into focused proof tasks.
///
/// The impact plan already maps each changed file to its owning package,
/// closes over reverse dependencies, and ranks `test` targets above `lib`/
/// `bin` (see `impact_plan.rs`). Until now that ranking only reached a model
/// prompt, so on a Rust-only diff the executor received nothing. Every derived
/// command is re-validated by [`focused_cargo_test_command_spec`]: the
/// allowlist stays the single gate on what may run.
pub(crate) fn focused_cargo_test_candidates_from_impact_plan(
    plan: &ImpactPlan,
    limit: usize,
) -> Vec<FocusedTestTask> {
    let mut tasks = Vec::new();
    // Every brokered cargo command carries `--locked`, which refuses to create
    // a missing lock file. Without one the proof could only ever fail on that
    // precondition, and a PR author would read "your test failed". The plan
    // already records the absent lock file as an evidence gap; selecting
    // nothing keeps that missing evidence from turning into failed evidence.
    if !plan.cargo_lockfile {
        return tasks;
    }
    for candidate in plan
        .candidate_tasks
        .iter()
        .filter(|candidate| candidate.kind == "test")
    {
        if tasks.len() >= limit {
            break;
        }
        let Some(task) = focused_cargo_test_task_from_impact_candidate(candidate) else {
            continue;
        };
        merge_focused_test_task(&mut tasks, task);
    }
    tasks
}

/// Build one focused task for an impact-plan test target, or `None` when the
/// derived command is not allowlisted. Head-only mode is deliberate: a
/// diff-derived candidate claims "the package's tests pass at HEAD", not that
/// a specific test discriminates the patch. A model request naming the same
/// command merges into this task and upgrades it to red/green.
fn focused_cargo_test_task_from_impact_candidate(
    candidate: &ImpactCandidateTask,
) -> Option<FocusedTestTask> {
    let command = format!(
        "cargo test --locked --package {} --test {}",
        candidate.test_package, candidate.target
    );
    let spec = focused_cargo_test_command_spec(&command)?;
    let command_specs = FocusedTestCommandSpecs {
        head: spec.clone(),
        base_plus_tests: spec.clone(),
    };
    Some(FocusedTestTask {
        id: focused_test_task_id_for_target(
            &focused_cargo_test_target_label(&spec.argv),
            None,
            FocusedProofMode::HeadOnly,
            Some(&command_specs),
        ),
        file: focused_cargo_test_target_label(&spec.argv),
        test_name: None,
        mode: FocusedProofMode::HeadOnly,
        command_specs: Some(command_specs),
        timeout_sec: None,
        required: false,
        requested_by: vec!["impact-planner".to_owned()],
        request_ids: Vec::new(),
    })
}

/// Deterministic candidate floor for a diff.
///
/// `impact` carries the Cargo workspace impact plan when the caller has one.
/// Callers that pass `None` (request metadata attribution, follow-up phases
/// that only re-plan requested work) keep the Bun-and-requests behavior.
pub(crate) fn focused_test_candidates_from_diff(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
    impact: Option<&ImpactPlan>,
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
    // Cargo branch. The Bun detector above matches nothing on a Rust repo,
    // which left the deterministic floor empty and handed test selection to
    // whatever the model happened to request.
    if let Some(impact) = impact {
        for task in
            focused_cargo_test_candidates_from_impact_plan(impact, MAX_IMPACT_CARGO_TEST_CANDIDATES)
        {
            merge_focused_test_task(&mut tasks, task);
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

/// Native v2 proof flow (Order 4b of #678): extract focused-test candidates
/// from typed `ProofRequestV2`s. Only `ProofKind::FocusedTest` requests map to
/// focused-test candidates; other kinds (SanitizerWitness, MiriWitness, ...)
/// are ignored here — they are not test/build candidates and must not be
/// misrouted. For a `FocusedTest` request the `target` carries the cargo-test
/// command string, which the existing allowlist (`focused_cargo_test_command
/// _spec`) and bun detector validate. This preserves the v1 security boundary
/// while making v2 the input contract.
///
/// The v2 request is normalized to a v1 `ProofRequest` and run through the
/// existing v1 extractor so the candidate output is byte-identical to a v1
/// request with the same command — pinned by `v2_focused_test_candidates_*
/// match_v1` in tests. The v2 broker receives already-approved v1 commands,
/// so identity canonicalization must not rewrite Cargo passthrough arguments
/// between the two artifact producers.
pub(crate) fn focused_test_candidates_from_v2(
    v2_requests: &[ProofRequestV2],
) -> Vec<FocusedTestTask> {
    let v1_requests = v2_requests
        .iter()
        .filter_map(proof_request_v2_to_v1_for_test)
        .collect::<Vec<_>>();
    focused_test_candidates_from_requests(&v1_requests)
}

/// Native v2 proof flow (Order 4b of #678): extract focused-build candidates
/// from typed `ProofRequestV2`s. Only `ProofKind::FocusedBuild` requests map
/// here; the `target` carries the cargo-build command string, validated by the
/// existing `focused_build_command_spec` allowlist.
pub(crate) fn focused_build_candidates_from_v2(
    v2_requests: &[ProofRequestV2],
) -> Vec<FocusedBuildTask> {
    let v1_requests = v2_requests
        .iter()
        .filter_map(proof_request_v2_to_v1_for_build)
        .collect::<Vec<_>>();
    focused_build_candidates_from_requests(&v1_requests)
}

/// Normalize a v2 `FocusedTest` request to a v1 `ProofRequest` for the
/// existing allowlist-backed extractor. Returns `None` for any other kind.
fn proof_request_v2_to_v1_for_test(req: &ProofRequestV2) -> Option<ProofRequest> {
    if !matches!(req.kind, ProofKind::FocusedTest) {
        return None;
    }
    Some(proof_request_v2_to_v1(req, "focused-test"))
}

/// Normalize a v2 `FocusedBuild` request to a v1 `ProofRequest`. Returns
/// `None` for any other kind.
fn proof_request_v2_to_v1_for_build(req: &ProofRequestV2) -> Option<ProofRequest> {
    if !matches!(req.kind, ProofKind::FocusedBuild) {
        return None;
    }
    Some(proof_request_v2_to_v1(req, "focused-build"))
}

/// Shared v2→v1 normalization. The v2 `target` is the command string; `cost`
/// is the v1 proof-class label for the kind. Other v1 fields are mapped from
/// their v2 equivalents. The command is normalized to match the broker's
/// allowlist syntax (Order 6 of #678): `-p` → `--package`, add `--locked`,
/// strip shell pipes. This lets the model express intent freely while the
/// deterministic layer enforces the exact allowlist.
fn proof_request_v2_to_v1(req: &ProofRequestV2, cost: &str) -> ProofRequest {
    ProofRequest {
        schema: "ub-review.proof_request.v1".to_owned(),
        // Drop the "-v2" suffix the shadow converter adds so dedup keys match.
        id: req.id.strip_suffix("-v2").unwrap_or(&req.id).to_owned(),
        lane: req
            .requested_by
            .first()
            .cloned()
            .unwrap_or_else(|| "proof-planner".to_owned()),
        requested_by: req.requested_by.clone(),
        command: req.target.clone(),
        reason: req.expected_interpretation.clone(),
        cost: cost.to_owned(),
        timeout_sec: req.timeout_sec,
        required: req.priority == "high",
        status: req.status.clone(),
    }
}

pub(crate) fn focused_build_plans_from_requests(
    proof_requests: &[ProofRequest],
    budget: ProofBudget,
) -> Vec<FocusedBuildPlan> {
    focused_build_candidates_from_requests(proof_requests)
        .into_iter()
        .take(budget.max_focused_tests)
        .map(|task| {
            let timeout_sec = focused_build_task_command_timeout(&task, budget);
            FocusedBuildPlan {
                id: task.id,
                command: command_display(&task.argv),
                timeout_sec,
                requested_by: task.requested_by,
                request_ids: task.request_ids,
                status: "planned".to_owned(),
                reason: format!(
                    "planner-only focused build target under budget: max {} command(s), {}s per command, {}s total",
                    budget.max_focused_tests, budget.per_command_timeout_sec, budget.max_total_seconds
                ),
            }
        })
        .collect()
}

pub(crate) fn focused_build_candidate_plans_from_requests(
    proof_requests: &[ProofRequest],
    budget: ProofBudget,
) -> Vec<FocusedBuildPlan> {
    let candidate_tasks = focused_build_candidates_from_requests(proof_requests);
    let mut plans = Vec::with_capacity(candidate_tasks.len());
    for task in candidate_tasks {
        let timeout_sec = focused_build_task_command_timeout(&task, budget);
        plans.extend(std::iter::once(FocusedBuildPlan {
            id: task.id,
            command: command_display(&task.argv),
            timeout_sec,
            requested_by: task.requested_by,
            request_ids: task.request_ids,
            status: candidate_plan_status(plans.len() < budget.max_focused_tests).to_owned(),
            reason: "candidate recorded for portfolio accounting; execution is budget-gated"
                .to_owned(),
        }));
    }
    plans
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

fn merge_focused_test_request_group_tasks(
    tasks: &mut Vec<FocusedTestTask>,
    request_groups: &[ProofRequestGroup],
) {
    for group in request_groups {
        let Some(target) = focused_test_request_target(group) else {
            continue;
        };
        merge_focused_test_task(
            tasks,
            FocusedTestTask {
                id: focused_test_task_id_for_target(
                    &target.file,
                    target.test_name.as_deref(),
                    FocusedProofMode::RedGreen,
                    target.command_specs.as_ref(),
                ),
                file: target.file,
                test_name: target.test_name,
                mode: FocusedProofMode::RedGreen,
                command_specs: target.command_specs,
                timeout_sec: Some(group.timeout_sec),
                required: group.required,
                requested_by: group.requested_by.clone(),
                request_ids: group.request_ids.clone(),
            },
        );
    }
}

fn focused_build_task_from_request_group(group: &ProofRequestGroup) -> Option<FocusedBuildTask> {
    if group.status != "requested" || group.cost != "focused-build" {
        return None;
    }
    let spec = focused_build_command_spec(&group.command)?;
    let command = command_display(&spec.argv);
    Some(FocusedBuildTask {
        id: focused_build_task_id(&command),
        command,
        argv: spec.argv,
        timeout_sec: group.timeout_sec,
        required: group.required,
        requested_by: group.requested_by.clone(),
        request_ids: group.request_ids.clone(),
    })
}

fn focused_build_task_id(command: &str) -> String {
    let fingerprint = sha256_hex(command.as_bytes());
    format!("proof-build-{}", &fingerprint[..12])
}

fn merge_focused_build_task(tasks: &mut Vec<FocusedBuildTask>, mut task: FocusedBuildTask) {
    if let Some(existing) = tasks
        .iter_mut()
        .find(|existing| existing.command == task.command)
    {
        existing.timeout_sec = existing.timeout_sec.max(task.timeout_sec);
        existing.required |= task.required;
        for lane in task.requested_by.drain(..) {
            push_unique(&mut existing.requested_by, &lane);
        }
        for request_id in task.request_ids.drain(..) {
            push_unique(&mut existing.request_ids, &request_id);
        }
        return;
    }
    tasks.push(task);
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

#[cfg(test)]
pub(crate) fn focused_test_task(
    file: &str,
    test_name: Option<String>,
    request_groups: &[ProofRequestGroup],
) -> FocusedTestTask {
    focused_test_task_with_mode(file, test_name, FocusedProofMode::RedGreen, request_groups)
}

fn focused_test_task_with_mode(
    file: &str,
    test_name: Option<String>,
    mode: FocusedProofMode,
    request_groups: &[ProofRequestGroup],
) -> FocusedTestTask {
    let mut requested_by = Vec::new();
    let mut request_ids = Vec::new();
    let mut timeout_sec = None;
    let mut required = false;
    for group in request_groups {
        if group.status == "requested"
            && group.command.contains(file)
            && test_name
                .as_ref()
                .is_none_or(|name| group.command.contains(name))
        {
            merge_task_timeout(&mut timeout_sec, Some(group.timeout_sec));
            required |= group.required;
            for lane in &group.requested_by {
                push_unique(&mut requested_by, lane);
            }
            for id in &group.request_ids {
                push_unique(&mut request_ids, id);
            }
        }
    }
    if requested_by.is_empty() {
        requested_by.push("proof-broker".to_owned());
    }
    FocusedTestTask {
        id: focused_test_task_id(file, test_name.as_deref(), mode),
        file: file.to_owned(),
        test_name,
        mode,
        command_specs: None,
        timeout_sec,
        required,
        requested_by,
        request_ids,
    }
}

fn focused_test_task_id_for_target(
    file: &str,
    test_name: Option<&str>,
    mode: FocusedProofMode,
    command_specs: Option<&FocusedTestCommandSpecs>,
) -> String {
    if let Some(command_specs) = command_specs {
        return focused_test_command_task_id(&command_display(&command_specs.head.argv), mode);
    }
    focused_test_task_id(file, test_name, mode)
}

fn focused_test_task_id(file: &str, test_name: Option<&str>, mode: FocusedProofMode) -> String {
    let fingerprint = sha256_hex(format!("{file}\n{}", test_name.unwrap_or("")).as_bytes());
    let prefix = match mode {
        FocusedProofMode::HeadOnly => "proof-head",
        FocusedProofMode::RedGreen => "proof-red-green",
    };
    format!("{prefix}-{}", &fingerprint[..12])
}

fn focused_test_command_task_id(command: &str, mode: FocusedProofMode) -> String {
    let fingerprint = sha256_hex(command.as_bytes());
    let prefix = match mode {
        FocusedProofMode::HeadOnly => "proof-head",
        FocusedProofMode::RedGreen => "proof-red-green",
    };
    format!("{prefix}-{}", &fingerprint[..12])
}

fn merge_focused_test_task(tasks: &mut Vec<FocusedTestTask>, mut task: FocusedTestTask) {
    if let Some(existing) = tasks.iter_mut().find(|existing| {
        focused_test_task_merge_key(existing) == focused_test_task_merge_key(&task)
    }) {
        if existing.mode == FocusedProofMode::HeadOnly && task.mode == FocusedProofMode::RedGreen {
            existing.mode = FocusedProofMode::RedGreen;
            existing.id = focused_test_task_id_for_target(
                &existing.file,
                existing.test_name.as_deref(),
                existing.mode,
                existing.command_specs.as_ref(),
            );
        }
        merge_task_timeout(&mut existing.timeout_sec, task.timeout_sec);
        existing.required |= task.required;
        for lane in task.requested_by.drain(..) {
            push_unique(&mut existing.requested_by, &lane);
        }
        for request_id in task.request_ids.drain(..) {
            push_unique(&mut existing.request_ids, &request_id);
        }
        return;
    }
    tasks.push(task);
}

fn merge_task_timeout(existing: &mut Option<u64>, incoming: Option<u64>) {
    let Some(incoming) = incoming else {
        return;
    };
    *existing = Some(existing.map_or(incoming, |current| current.max(incoming)));
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

fn focused_test_task_merge_key(task: &FocusedTestTask) -> String {
    if let Some(command_specs) = &task.command_specs {
        return format!("command:{}", command_display(&command_specs.head.argv));
    }
    format!(
        "bun:{}:{}",
        task.file,
        task.test_name.as_deref().unwrap_or_default()
    )
}

#[derive(Clone, Debug)]
struct FocusedTestRequestTarget {
    file: String,
    test_name: Option<String>,
    command_specs: Option<FocusedTestCommandSpecs>,
}

fn focused_test_request_target(group: &ProofRequestGroup) -> Option<FocusedTestRequestTarget> {
    if group.status != "requested" || group.cost != "focused-test" {
        return None;
    }
    let parts = group.command.split_whitespace().collect::<Vec<_>>();
    let Some((file, args)) = focused_bun_request_parts(&parts) else {
        let spec = focused_cargo_test_command_spec(&group.command)?;
        return Some(FocusedTestRequestTarget {
            file: focused_cargo_test_target_label(&spec.argv),
            test_name: focused_cargo_test_filter_name(&spec.argv),
            command_specs: Some(FocusedTestCommandSpecs {
                head: spec.clone(),
                base_plus_tests: spec,
            }),
        });
    };
    if !is_bun_focused_test_file(file) {
        return None;
    }
    Some(FocusedTestRequestTarget {
        file: normalize_repo_path(file),
        test_name: focused_test_name_arg(args),
        command_specs: None,
    })
}

pub(crate) fn focused_bun_request_parts<'a>(
    parts: &'a [&'a str],
) -> Option<(&'a str, &'a [&'a str])> {
    match parts {
        ["bun", "test", file, args @ ..] => Some((*file, args)),
        ["bun", "bd", "test", file, args @ ..] => Some((*file, args)),
        ["USE_SYSTEM_BUN=1", "bun", "test", file, args @ ..] => Some((*file, args)),
        _ => None,
    }
}

fn focused_test_name_arg(args: &[&str]) -> Option<String> {
    let index = args
        .iter()
        .position(|arg| matches!(*arg, "-t" | "--test-name-pattern"))?;
    let mut tokens = Vec::new();
    for token in &args[index + 1..] {
        if token.starts_with('-') {
            break;
        }
        tokens.push(*token);
    }
    let joined = tokens.join(" ");
    let value = strip_matching_quotes(joined.trim());
    (!value.is_empty()).then(|| value.to_owned())
}

fn strip_matching_quotes(value: &str) -> &str {
    if value.len() < 2 {
        return value;
    }
    let bytes = value.as_bytes();
    if matches!(
        (bytes.first(), bytes.last()),
        (Some(b'\''), Some(b'\'')) | (Some(b'"'), Some(b'"'))
    ) {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic impact plan carrying one ranked Cargo test target per
    /// entry, so the Cargo branch can be pinned without shelling out to
    /// `cargo metadata`.
    fn impact_plan_with_test_targets(targets: &[(&str, &str, u32)]) -> ImpactPlan {
        ImpactPlan {
            schema: crate::artifacts::IMPACT_PLAN_SCHEMA,
            changed_files: vec!["src/proof/tasks.rs".to_owned()],
            changed_packages: Vec::new(),
            affected_packages: Vec::new(),
            candidate_tasks: targets
                .iter()
                .map(|(package, target, rank)| ImpactCandidateTask {
                    target: (*target).to_owned(),
                    kind: "test".to_owned(),
                    reason: "package owns a changed file".to_owned(),
                    owning_package: (*package).to_owned(),
                    test_package: (*package).to_owned(),
                    estimated_cost: "low",
                    expected_value: "high",
                    rank: *rank,
                    selection: "selected",
                })
                .collect(),
            evidence_gaps: Vec::new(),
            selection_mode: "shadow",
            cargo_lockfile: true,
        }
    }

    /// Without a committed lock file the `--locked` command template cannot
    /// run, so the floor must select nothing rather than manufacture a receipt
    /// that reads as a failing test.
    #[test]
    fn a_workspace_without_a_lockfile_selects_no_cargo_candidate() -> Result<()> {
        let mut plan = impact_plan_with_test_targets(&[("ub-review", "cli", 190)]);
        plan.cargo_lockfile = false;
        anyhow::ensure!(
            focused_test_candidates_from_diff(&rust_only_diff(), &[], Some(&plan)).is_empty(),
            "a lock-less workspace must not yield an unrunnable cargo proof"
        );
        Ok(())
    }

    fn rust_only_diff() -> DiffContext {
        DiffContext {
            base: "base".to_owned(),
            head: "head".to_owned(),
            changed_files: vec!["src/proof/tasks.rs".to_owned()],
            patch: String::new(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::SourceGeneral,
        }
    }

    /// A Rust-only diff used to select nothing at all: the deterministic floor
    /// only recognized Bun `.test.ts` files, so test selection collapsed to
    /// whatever the model happened to request. It now yields a real focused
    /// cargo-test command that the broker allowlist accepts.
    #[test]
    fn rust_only_diff_yields_a_focused_cargo_test_candidate() -> Result<()> {
        let diff = rust_only_diff();
        anyhow::ensure!(
            focused_test_candidates_from_diff(&diff, &[], None).is_empty(),
            "without an impact plan the Bun-only floor still selects nothing"
        );

        let plan = impact_plan_with_test_targets(&[("ub-review", "cli", 190)]);
        let candidates = focused_test_candidates_from_diff(&diff, &[], Some(&plan));
        let candidate = candidates
            .first()
            .ok_or_else(|| anyhow::anyhow!("a Rust-only diff must yield a cargo test candidate"))?;
        let specs = candidate
            .command_specs
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("cargo candidate must carry an approved command"))?;
        anyhow::ensure!(
            specs.head.argv
                == [
                    "cargo",
                    "test",
                    "--locked",
                    "--package",
                    "ub-review",
                    "--test",
                    "cli"
                ],
            "unexpected cargo command {:?}",
            specs.head.argv
        );
        anyhow::ensure!(candidate.file == "cargo-test:cli");
        anyhow::ensure!(candidate.mode == FocusedProofMode::HeadOnly);
        anyhow::ensure!(candidate.requested_by == ["impact-planner"]);
        Ok(())
    }

    /// The allowlist stays the only gate: an impact-plan target whose derived
    /// command cannot pass `focused_cargo_test_command_spec` is dropped rather
    /// than smuggled into execution.
    #[test]
    fn impact_candidates_that_fail_the_allowlist_are_dropped() -> Result<()> {
        let diff = rust_only_diff();
        let plan = impact_plan_with_test_targets(&[
            ("ub-review", "cli; rm -rf /", 190),
            ("ub-review", "cli", 190),
        ]);
        let candidates = focused_test_candidates_from_diff(&diff, &[], Some(&plan));
        anyhow::ensure!(
            candidates.len() == 1,
            "only the allowlisted target may survive, got {:?}",
            candidates
                .iter()
                .map(|task| task.file.as_str())
                .collect::<Vec<_>>()
        );
        anyhow::ensure!(candidates[0].file == "cargo-test:cli");
        Ok(())
    }

    /// End-to-end over this repository's real Cargo metadata: a Rust-only diff
    /// produces at least one focused cargo-test candidate, and two runs over
    /// the same commit produce identical candidate identities and order.
    #[test]
    fn this_repo_rust_diff_produces_deterministic_cargo_candidates() -> Result<()> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let changed_files = ["src/proof/tasks.rs".to_owned()];
        let candidate_ids = |plan: &ImpactPlan| {
            focused_cargo_test_candidates_from_impact_plan(plan, MAX_IMPACT_CARGO_TEST_CANDIDATES)
                .into_iter()
                .map(|task| task.id)
                .collect::<Vec<_>>()
        };
        let first = candidate_ids(&build_impact_plan(root, &changed_files, "shadow"));
        let second = candidate_ids(&build_impact_plan(root, &changed_files, "shadow"));
        anyhow::ensure!(
            !first.is_empty(),
            "a Rust-only diff on this repo must select at least one cargo test target"
        );
        anyhow::ensure!(
            first == second,
            "candidate order must be deterministic: {first:?} vs {second:?}"
        );
        anyhow::ensure!(
            first.len() <= MAX_IMPACT_CARGO_TEST_CANDIDATES,
            "the candidate floor must stay bounded"
        );
        Ok(())
    }

    /// #838: the budget scan used to `return` at the first rejection, so one
    /// expensive candidate hid every later, cheaper one. The red/green Bun
    /// candidate below costs two commands and cannot fit; the head-only cargo
    /// candidate behind it costs one command and fits exactly.
    #[test]
    fn budget_rejection_does_not_block_a_later_cheaper_candidate() -> Result<()> {
        let diff = DiffContext {
            base: "base".to_owned(),
            head: "head".to_owned(),
            changed_files: vec![
                "test/js/bun/proof.test.ts".to_owned(),
                "src/proof/tasks.rs".to_owned(),
            ],
            patch: "diff --git a/test/js/bun/proof.test.ts b/test/js/bun/proof.test.ts\n@@ -1,2 +1,3 @@\n import { test } from 'bun:test';\n+test(\"expensive proof\", () => {});\n".to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::SourceGeneral,
        };
        let plan = impact_plan_with_test_targets(&[("ub-review", "cli", 190)]);
        let budget = ProofBudget {
            max_focused_test_files: 4,
            max_focused_tests: 4,
            per_command_timeout_sec: 60,
            max_total_seconds: 60,
        };

        let candidates = focused_test_candidates_from_diff(&diff, &[], Some(&plan));
        anyhow::ensure!(
            candidates.len() == 2 && candidates[0].mode == FocusedProofMode::RedGreen,
            "fixture must offer the expensive red/green candidate first"
        );

        let tasks = focused_test_tasks_from_diff(&diff, &[], Some(&plan), budget);
        anyhow::ensure!(
            tasks.len() == 1,
            "the cheaper later candidate must still be admitted, got {:?}",
            tasks
                .iter()
                .map(|task| task.file.as_str())
                .collect::<Vec<_>>()
        );
        anyhow::ensure!(tasks[0].file == "cargo-test:cli");
        anyhow::ensure!(
            tasks.iter().map(|task| task.id.clone()).collect::<Vec<_>>()
                == focused_test_tasks_from_diff(&diff, &[], Some(&plan), budget)
                    .iter()
                    .map(|task| task.id.clone())
                    .collect::<Vec<_>>(),
            "admitted task order must be deterministic across runs"
        );

        // The skipped candidate is accounted for as skipped, not as executed.
        let plans = focused_proof_candidate_plans_from_diff(&diff, &[], Some(&plan), budget);
        anyhow::ensure!(plans.len() == 2);
        let statuses = plans
            .iter()
            .map(|plan| (plan.test_file.as_str(), plan.status.as_str()))
            .collect::<Vec<_>>();
        anyhow::ensure!(
            statuses
                == [
                    ("test/js/bun/proof.test.ts", "deferred_by_budget"),
                    ("cargo-test:cli", "planned"),
                ],
            "planner artifact must record the skipped candidate explicitly, got {statuses:?}"
        );
        Ok(())
    }

    /// The Cargo floor reads exactly three things out of the impact plan: the
    /// `cargo_lockfile` precondition, each candidate's `kind`, and its package
    /// and target names. Everything else the planner records is advisory
    /// bookkeeping for the artifact and the model prompt, and must have no
    /// authority over which commands the broker is offered — otherwise a purely
    /// descriptive planner edit would silently change what executes.
    #[test]
    fn the_cargo_floor_ignores_advisory_impact_plan_fields() -> Result<()> {
        let baseline = impact_plan_with_test_targets(&[("ub-review", "cli", 190)]);
        let floor = |plan: &ImpactPlan| {
            focused_cargo_test_candidates_from_impact_plan(plan, MAX_IMPACT_CARGO_TEST_CANDIDATES)
                .into_iter()
                .map(|task| (task.file, task.id))
                .collect::<Vec<_>>()
        };
        let expected = floor(&baseline);
        assert_eq!(
            expected.len(),
            1,
            "fixture must offer exactly the cli target, got {expected:?}"
        );
        assert_eq!(expected[0].0, "cargo-test:cli");

        let advisory = [
            (
                "changed_files",
                (|plan: &mut ImpactPlan| {
                    plan.changed_files = vec!["docs/ARCHITECTURE.md".to_owned()];
                }) as fn(&mut ImpactPlan),
            ),
            ("changed_packages", |plan| {
                plan.changed_packages = vec![crate::ImpactPackage {
                    name: "ub-review".to_owned(),
                    manifest_path: "Cargo.toml".to_owned(),
                    relation: "changed",
                }];
            }),
            ("affected_packages", |plan| {
                plan.affected_packages = vec![crate::ImpactPackage {
                    name: "xtask".to_owned(),
                    manifest_path: "xtask/Cargo.toml".to_owned(),
                    relation: "reverse-dependency",
                }];
            }),
            ("evidence_gaps", |plan| {
                plan.evidence_gaps = vec![crate::ImpactEvidenceGap {
                    kind: "no-test-targets-found",
                    detail: "advisory only".to_owned(),
                }];
            }),
            ("estimated_cost", |plan| {
                for candidate in &mut plan.candidate_tasks {
                    candidate.estimated_cost = "high";
                }
            }),
            ("expected_value", |plan| {
                for candidate in &mut plan.candidate_tasks {
                    candidate.expected_value = "low";
                }
            }),
        ];
        for (field, mutate) in advisory {
            let mut plan = baseline.clone();
            mutate(&mut plan);
            assert_eq!(
                floor(&plan),
                expected,
                "mutating the advisory field {field} must not change the cargo floor"
            );
        }

        // The contrast case: `cargo_lockfile` is the one plan-level field the
        // floor obeys, because `--locked` refuses to create a missing lock file.
        let mut lockless = baseline.clone();
        lockless.cargo_lockfile = false;
        assert_eq!(
            floor(&lockless),
            Vec::new(),
            "cargo_lockfile = false must empty the cargo floor"
        );

        // And `kind` is the other: only `test` targets become proof tasks.
        let mut library_only = baseline.clone();
        for candidate in &mut library_only.candidate_tasks {
            candidate.kind = "lib".to_owned();
        }
        assert_eq!(
            floor(&library_only),
            Vec::new(),
            "only `test` targets may become executable proof tasks"
        );
        Ok(())
    }

    /// A cargo candidate's identity must be a pure function of the approved
    /// command, because the broker merges and de-duplicates tasks by id and a
    /// later model request naming the same command has to land on the same
    /// task. Two plan entries for one package/target therefore collapse into a
    /// single task regardless of rank, and the derived id must agree with
    /// `focused_test_task_id_for_target` over the head-only, unnamed-test
    /// shape the floor actually constructs.
    #[test]
    fn cargo_candidate_identity_follows_the_approved_command() -> Result<()> {
        let plan = impact_plan_with_test_targets(&[
            ("ub-review", "cli", 190),
            ("ub-review", "cli", 40),
            ("ub-review", "gate", 190),
        ]);
        let candidates =
            focused_cargo_test_candidates_from_impact_plan(&plan, MAX_IMPACT_CARGO_TEST_CANDIDATES);
        let labels = candidates
            .iter()
            .map(|task| task.file.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            labels,
            ["cargo-test:cli", "cargo-test:gate"],
            "duplicate targets must merge and distinct targets must stay separate"
        );

        for candidate in &candidates {
            let specs = candidate
                .command_specs
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("cargo candidate must carry an approved command"))?;
            let label = focused_cargo_test_target_label(&specs.head.argv);
            assert_eq!(
                candidate.test_name, None,
                "a head-only package proof names no single test"
            );
            assert_eq!(
                candidate.mode,
                FocusedProofMode::HeadOnly,
                "a diff-derived cargo candidate claims HEAD passes, not red/green"
            );
            assert_eq!(
                candidate.id,
                focused_test_task_id_for_target(
                    &label,
                    None,
                    FocusedProofMode::HeadOnly,
                    Some(specs),
                ),
                "id: focused_test_task_id_for_target must be fed the target label, no test \
                 name, head-only mode, and the approved command specs"
            );
            assert_ne!(
                candidate.id,
                focused_test_task_id_for_target(
                    &label,
                    None,
                    FocusedProofMode::RedGreen,
                    Some(specs),
                ),
                "the head-only mode must be part of the identity"
            );
            assert_eq!(
                specs.base_plus_tests.argv, specs.head.argv,
                "both command slots must hold the same allowlisted argv"
            );
        }
        Ok(())
    }

    /// The floor is bounded so a wide refactor cannot flood the portfolio with
    /// equally ranked targets, and it truncates from the front of the ranked
    /// catalog rather than sampling it.
    #[test]
    fn the_cargo_floor_truncates_the_ranked_catalog_at_the_limit() -> Result<()> {
        let targets = ["a", "b", "c", "d"]
            .map(|target| ("ub-review", target, 190))
            .to_vec();
        let plan = impact_plan_with_test_targets(&targets);
        for limit in 0..=targets.len() {
            let labels = focused_cargo_test_candidates_from_impact_plan(&plan, limit)
                .into_iter()
                .map(|task| task.file)
                .collect::<Vec<_>>();
            let expected = targets[..limit]
                .iter()
                .map(|(_, target, _)| format!("cargo-test:{target}"))
                .collect::<Vec<_>>();
            assert_eq!(
                labels, expected,
                "limit {limit} must take the first {limit} ranked targets"
            );
        }
        Ok(())
    }

    #[test]
    fn focused_proof_mode_keys_and_command_counts_are_stable() {
        assert_eq!(FocusedProofMode::HeadOnly.key(), "head-only");
        assert_eq!(FocusedProofMode::HeadOnly.command_count(), 1);
        assert_eq!(FocusedProofMode::RedGreen.key(), "red-green");
        assert_eq!(FocusedProofMode::RedGreen.command_count(), 2);
    }

    fn v2_focused_test_request(command: &str) -> ProofRequestV2 {
        ProofRequestV2 {
            schema: crate::artifacts::PROOF_REQUEST_V2_SCHEMA.to_owned(),
            id: "req-1-v2".to_owned(),
            kind: ProofKind::FocusedTest,
            target: command.to_owned(),
            claim_ids: vec!["claim-1".to_owned()],
            requested_by: vec!["tests-oracle".to_owned()],
            expected_interpretation: "confirm test discriminates the patch".to_owned(),
            priority: "high".to_owned(),
            timeout_sec: 300,
            status: "requested".to_owned(),
            base: String::new(),
            head: String::new(),
        }
    }

    fn v2_focused_build_request(command: &str) -> ProofRequestV2 {
        ProofRequestV2 {
            schema: crate::artifacts::PROOF_REQUEST_V2_SCHEMA.to_owned(),
            id: "req-2-v2".to_owned(),
            kind: ProofKind::FocusedBuild,
            target: command.to_owned(),
            claim_ids: Vec::new(),
            requested_by: vec!["correctness".to_owned()],
            expected_interpretation: String::new(),
            priority: "medium".to_owned(),
            timeout_sec: 120,
            status: "requested".to_owned(),
            base: String::new(),
            head: String::new(),
        }
    }

    fn focused_test_request(id: &str, command: &str, timeout_sec: u64) -> ProofRequest {
        ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: id.to_owned(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            command: command.to_owned(),
            reason: "exercise focused proof admission".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec,
            required: true,
            status: "requested".to_owned(),
        }
    }

    fn focused_test_diff(changed_files: &[&str]) -> DiffContext {
        DiffContext {
            base: "base".to_owned(),
            head: "head".to_owned(),
            changed_files: changed_files
                .iter()
                .map(|file| (*file).to_owned())
                .collect(),
            patch: String::new(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        }
    }

    #[test]
    fn focused_test_tasks_from_diff_continues_after_time_rejection() -> Result<()> {
        let diff = focused_test_diff(&[]);
        let requests = [
            focused_test_request(
                "request-a",
                "bun test test/js/bun/a.test.ts -t accepted-first",
                20,
            ),
            focused_test_request(
                "request-b",
                "bun test test/js/bun/b.test.ts -t rejected-expensive",
                30,
            ),
            focused_test_request(
                "request-c",
                "bun test test/js/bun/c.test.ts -t accepted-cheap",
                10,
            ),
        ];
        let budget = ProofBudget {
            max_focused_test_files: 3,
            max_focused_tests: 3,
            per_command_timeout_sec: 60,
            max_total_seconds: 60,
        };

        let tasks = focused_test_tasks_from_diff(&diff, &requests, None, budget);
        anyhow::ensure!(
            tasks
                .iter()
                .map(|task| (task.file.as_str(), task.test_name.as_deref()))
                .collect::<Vec<_>>()
                == vec![
                    ("test/js/bun/a.test.ts", Some("accepted-first")),
                    ("test/js/bun/c.test.ts", Some("accepted-cheap")),
                ],
            "a time-rejected candidate must not hide a later cheaper candidate"
        );

        let plans = focused_proof_candidate_plans_from_diff(&diff, &requests, None, budget);
        anyhow::ensure!(
            plans
                .iter()
                .map(|plan| (
                    plan.test_file.as_str(),
                    plan.test_name.as_deref(),
                    plan.status.as_str(),
                ))
                .collect::<Vec<_>>()
                == vec![
                    ("test/js/bun/a.test.ts", Some("accepted-first"), "planned",),
                    (
                        "test/js/bun/b.test.ts",
                        Some("rejected-expensive"),
                        "deferred_by_budget",
                    ),
                    ("test/js/bun/c.test.ts", Some("accepted-cheap"), "planned",),
                ],
            "mixed time-budget dispositions must preserve candidate order"
        );
        Ok(())
    }

    #[test]
    fn focused_test_tasks_from_diff_continues_after_new_file_rejection() -> Result<()> {
        let diff = focused_test_diff(&["test/js/bun/a.test.ts", "test/js/bun/b.test.ts"]);
        let requests = [focused_test_request(
            "request-a-later",
            "bun test test/js/bun/a.test.ts -t later-same-file",
            10,
        )];
        let budget = ProofBudget {
            max_focused_test_files: 1,
            max_focused_tests: 2,
            per_command_timeout_sec: 60,
            max_total_seconds: 240,
        };

        let plans = focused_proof_candidate_plans_from_diff(&diff, &requests, None, budget);
        anyhow::ensure!(
            plans
                .iter()
                .map(|plan| (
                    plan.test_file.as_str(),
                    plan.test_name.as_deref(),
                    plan.status.as_str(),
                ))
                .collect::<Vec<_>>()
                == vec![
                    ("test/js/bun/a.test.ts", None, "planned"),
                    ("test/js/bun/b.test.ts", None, "deferred_by_budget",),
                    ("test/js/bun/a.test.ts", Some("later-same-file"), "planned",),
                ],
            "a new-file rejection must not hide a later candidate in an admitted file"
        );
        Ok(())
    }

    #[test]
    fn focused_test_tasks_from_diff_count_cap_defers_all_later_candidates() -> Result<()> {
        let diff = focused_test_diff(&[]);
        let requests = [
            focused_test_request(
                "request-a",
                "bun test test/js/bun/a.test.ts -t selected",
                10,
            ),
            focused_test_request(
                "request-b",
                "bun test test/js/bun/b.test.ts -t deferred-one",
                10,
            ),
            focused_test_request(
                "request-c",
                "bun test test/js/bun/c.test.ts -t deferred-two",
                10,
            ),
        ];
        let budget = ProofBudget {
            max_focused_test_files: 3,
            max_focused_tests: 1,
            per_command_timeout_sec: 60,
            max_total_seconds: 240,
        };

        let plans = focused_proof_candidate_plans_from_diff(&diff, &requests, None, budget);
        let identities_and_statuses = plans
            .iter()
            .map(|plan| (plan.id.as_str(), plan.status.as_str()))
            .collect::<Vec<_>>();
        let repeated = focused_proof_candidate_plans_from_diff(&diff, &requests, None, budget);
        anyhow::ensure!(
            identities_and_statuses
                == repeated
                    .iter()
                    .map(|plan| (plan.id.as_str(), plan.status.as_str()))
                    .collect::<Vec<_>>(),
            "candidate identity, ordering, and disposition must be deterministic"
        );
        anyhow::ensure!(
            plans
                .iter()
                .map(|plan| plan.status.as_str())
                .collect::<Vec<_>>()
                == vec!["planned", "deferred_by_budget", "deferred_by_budget"],
            "the count cap must explicitly defer every later candidate"
        );
        Ok(())
    }

    #[test]
    fn requiredness_survives_request_grouping_and_candidate_merging() -> Result<()> {
        let focused_test_command = "cargo test --locked --test config_tests";
        let mut optional_test = focused_test_request("test-optional", focused_test_command, 30);
        optional_test.required = false;
        optional_test.lane = "opposition".to_owned();
        optional_test.requested_by = vec!["opposition".to_owned()];
        let mut required_test = optional_test.clone();
        required_test.id = "test-required".to_owned();
        required_test.required = true;

        let tests = focused_test_candidates_from_requests(&[optional_test, required_test]);
        anyhow::ensure!(tests.len() == 1, "equivalent test requests must merge");
        anyhow::ensure!(
            tests[0].required,
            "one required source request must make the merged test candidate required"
        );

        let focused_build_command = "cargo check --workspace --all-targets --locked";
        let optional_build = ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "build-optional".to_owned(),
            lane: "opposition".to_owned(),
            requested_by: vec!["opposition".to_owned()],
            command: focused_build_command.to_owned(),
            reason: "optional build evidence".to_owned(),
            cost: "focused-build".to_owned(),
            timeout_sec: 30,
            required: false,
            status: "requested".to_owned(),
        };
        let mut required_build = optional_build.clone();
        required_build.id = "build-required".to_owned();
        required_build.required = true;

        let builds = focused_build_candidates_from_requests(&[optional_build, required_build]);
        anyhow::ensure!(builds.len() == 1, "equivalent build requests must merge");
        anyhow::ensure!(
            builds[0].required,
            "one required source request must make the merged build candidate required"
        );
        Ok(())
    }

    /// Native v2 flow (Order 4b): a v2 `FocusedTest` request yields the SAME
    /// focused-test candidate the v1 extractor produces for the equivalent v1
    /// command. This pins the v2→v1 normalization so the security boundary
    /// (allowlist) is preserved byte-for-byte.
    #[test]
    fn v2_focused_test_candidates_match_v1() {
        let command = "cargo test --locked --test config_tests -- --nocapture";
        let v1 = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "req-1".to_owned(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            command: command.to_owned(),
            reason: String::new(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: true,
            status: "requested".to_owned(),
        }];
        let v2 = vec![v2_focused_test_request(command)];
        let from_v1 = focused_test_candidates_from_requests(&v1);
        let from_v2 = focused_test_candidates_from_v2(&v2);
        assert_eq!(
            from_v1.len(),
            from_v2.len(),
            "v1 and v2 extractors must produce the same candidate count"
        );
        assert_eq!(from_v1.len(), 1, "the allowlisted command must resolve");
        // The task identity keys off (file, test_name, mode) and must match.
        assert_eq!(from_v1[0].id, from_v2[0].id);
        assert_eq!(from_v1[0].file, from_v2[0].file);
        assert_eq!(from_v1[0].test_name, from_v2[0].test_name);
        assert_eq!(from_v1[0].mode, from_v2[0].mode);
        assert_eq!(
            from_v1[0]
                .command_specs
                .as_ref()
                .map(|specs| (&specs.head.argv, &specs.base_plus_tests.argv)),
            from_v2[0]
                .command_specs
                .as_ref()
                .map(|specs| (&specs.head.argv, &specs.base_plus_tests.argv)),
            "v1 and v2 command specifications must preserve Cargo passthrough arguments"
        );
    }

    #[test]
    fn candidate_plan_artifacts_preserve_identity_and_budget_status() -> Result<()> {
        assert_eq!(
            candidate_plan_status(true),
            "planned",
            "candidate_plan_status must mark budget-fitting work planned"
        );
        assert_eq!(
            candidate_plan_status(false),
            "deferred_by_budget",
            "candidate_plan_status must mark overflow work deferred"
        );
        let budget = ProofBudget {
            max_focused_test_files: 1,
            max_focused_tests: 1,
            per_command_timeout_sec: 120,
            max_total_seconds: 240,
        };
        let task = FocusedTestTask {
            id: "proof-red-green-direct".to_owned(),
            file: "cargo-package:ub-review".to_owned(),
            test_name: Some("head_binding".to_owned()),
            mode: FocusedProofMode::RedGreen,
            command_specs: None,
            timeout_sec: Some(30),
            required: false,
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids: vec!["request-1".to_owned()],
        };
        let direct = focused_proof_plan_for_task(
            task.clone(),
            budget,
            "deferred_by_budget",
            "candidate recorded for portfolio accounting; execution is budget-gated".to_owned(),
        );
        assert_eq!(
            focused_proof_plan_for_task(
                task,
                budget,
                "deferred_by_budget",
                "candidate recorded for portfolio accounting; execution is budget-gated".to_owned(),
            )
            .status,
            "deferred_by_budget",
            "focused_proof_plan_for_task must preserve candidate status"
        );
        assert_eq!(direct.id, "proof-red-green-direct");
        assert_eq!(direct.test_file, "cargo-package:ub-review");
        assert_eq!(direct.test_name.as_deref(), Some("head_binding"));
        assert_eq!(direct.mode, FocusedProofMode::RedGreen);
        assert_eq!(direct.timeout_sec, 30);
        assert_eq!(direct.status, "deferred_by_budget");
        assert!(direct.head_command.contains("head"));
        assert!(direct.base_plus_tests_command.contains("base-plus-tests"));
        assert_eq!(direct.requested_by, vec!["tests-oracle"]);
        assert_eq!(direct.request_ids, vec!["request-1"]);
        assert!(direct.reason.contains("portfolio accounting"));

        let diff = DiffContext {
            base: "base".to_owned(),
            head: "head".to_owned(),
            changed_files: vec!["test/js/bun/proof.test.ts".to_owned()],
            patch: "diff --git a/test/js/bun/proof.test.ts b/test/js/bun/proof.test.ts\nindex 1111111..2222222 100644\n@@ -1,2 +1,4 @@\n import { test } from 'bun:test';\n+test(\"first proof\", () => {});\n+test(\"second proof\", () => {});\n".to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        };
        let planner_plans = focused_proof_plans_from_diff(&diff, &[], None, budget);
        let planner_plan = planner_plans
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing focused planner plan"))?;
        assert_eq!(planner_plans.len(), 1);
        assert_eq!(planner_plan.test_file, "test/js/bun/proof.test.ts");
        assert_eq!(planner_plan.test_name, None);
        assert_eq!(planner_plan.mode, FocusedProofMode::RedGreen);
        assert_eq!(planner_plan.timeout_sec, 120);
        assert!(planner_plan.head_command.contains("bun bd test"));
        assert!(planner_plan.base_plus_tests_command.contains("bun test"));
        assert_eq!(planner_plan.requested_by, vec!["proof-broker"]);
        assert!(planner_plan.request_ids.is_empty());
        assert_eq!(planner_plan.status, "planned");
        assert!(
            planner_plan
                .reason
                .contains("planner-only focused test target")
        );

        let candidate_plans = focused_proof_candidate_plans_from_diff(&diff, &[], None, budget);
        assert_eq!(
            focused_proof_candidate_plans_from_diff(&diff, &[], None, budget).len(),
            1,
            "focused_proof_candidate_plans_from_diff must record the candidate"
        );
        assert_eq!(candidate_plans.len(), 1);
        assert_eq!(
            focused_proof_candidate_plans_from_diff(&diff, &[], None, budget)
                .iter()
                .filter(|plan| plan.status == "planned")
                .count(),
            1
        );
        assert_eq!(
            candidate_plans
                .iter()
                .map(|plan| {
                    (
                        plan.id.as_str(),
                        plan.test_file.as_str(),
                        plan.test_name.as_deref(),
                        plan.mode.key(),
                        plan.status.as_str(),
                        plan.timeout_sec,
                        plan.head_command.as_str(),
                        plan.base_plus_tests_command.as_str(),
                        plan.requested_by.as_slice(),
                        plan.request_ids.as_slice(),
                        plan.reason.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            planner_plans
                .iter()
                .map(|plan| {
                    (
                        plan.id.as_str(),
                        plan.test_file.as_str(),
                        plan.test_name.as_deref(),
                        plan.mode.key(),
                        "planned",
                        plan.timeout_sec,
                        plan.head_command.as_str(),
                        plan.base_plus_tests_command.as_str(),
                        plan.requested_by.as_slice(),
                        plan.request_ids.as_slice(),
                        "candidate recorded for portfolio accounting; execution is budget-gated",
                    )
                })
                .collect::<Vec<_>>(),
            "focused_proof_candidate_plans_from_diff must preserve the complete candidate"
        );
        let zero_test_budget = ProofBudget {
            max_focused_tests: 0,
            ..budget
        };
        let deferred_candidates =
            focused_proof_candidate_plans_from_diff(&diff, &[], None, zero_test_budget);
        assert_eq!(deferred_candidates.len(), 1);
        assert_eq!(
            deferred_candidates
                .iter()
                .filter(|plan| plan.status == "deferred_by_budget")
                .count(),
            1
        );

        let build_requests = [
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "build-1".to_owned(),
                lane: "architecture".to_owned(),
                requested_by: vec!["architecture".to_owned()],
                command: "cargo check --workspace --all-targets --locked".to_owned(),
                reason: "check".to_owned(),
                cost: "focused-build".to_owned(),
                timeout_sec: 60,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "build-2".to_owned(),
                lane: "architecture".to_owned(),
                requested_by: vec!["architecture".to_owned()],
                command: "cargo doc --workspace --no-deps --locked".to_owned(),
                reason: "docs".to_owned(),
                cost: "focused-build".to_owned(),
                timeout_sec: 60,
                required: false,
                status: "requested".to_owned(),
            },
        ];
        let build_plans = focused_build_candidate_plans_from_requests(&build_requests, budget);
        assert_eq!(
            focused_build_candidate_plans_from_requests(&build_requests, budget).len(),
            2,
            "focused_build_candidate_plans_from_requests must retain all candidates"
        );
        assert_eq!(build_plans.len(), 2);
        assert_eq!(
            focused_build_candidate_plans_from_requests(&build_requests, budget)
                .iter()
                .filter(|plan| plan.status == "planned")
                .count(),
            1
        );
        assert_eq!(
            build_plans
                .iter()
                .filter(|plan| plan.status == "deferred_by_budget")
                .count(),
            1
        );
        assert!(build_plans.iter().all(|plan| plan.timeout_sec == 60));
        assert_eq!(
            build_plans
                .iter()
                .map(|plan| {
                    (
                        plan.id.as_str(),
                        plan.command.as_str(),
                        plan.timeout_sec,
                        plan.requested_by.as_slice(),
                        plan.request_ids.as_slice(),
                        plan.status.as_str(),
                        plan.reason.as_str(),
                    )
                })
                .collect::<Vec<_>>(),
            vec![
                (
                    "proof-build-81dee1e1dd1f",
                    "cargo check --workspace --all-targets --locked",
                    60,
                    ["architecture".to_owned()].as_slice(),
                    ["build-1".to_owned()].as_slice(),
                    "planned",
                    "candidate recorded for portfolio accounting; execution is budget-gated",
                ),
                (
                    "proof-build-f5e93291b352",
                    "cargo doc --workspace --no-deps --locked",
                    60,
                    ["architecture".to_owned()].as_slice(),
                    ["build-2".to_owned()].as_slice(),
                    "deferred_by_budget",
                    "candidate recorded for portfolio accounting; execution is budget-gated",
                ),
            ],
            "focused_build_candidate_plans_from_requests must preserve every candidate field"
        );
        let first_build = build_plans
            .first()
            .ok_or_else(|| anyhow::anyhow!("missing first build candidate"))?;
        let second_build = build_plans
            .get(1)
            .ok_or_else(|| anyhow::anyhow!("missing second build candidate"))?;
        assert_eq!(
            first_build.command, "cargo check --workspace --all-targets --locked",
            "focused_build_candidate_plans_from_requests must preserve the first command"
        );
        assert_eq!(
            first_build.status, "planned",
            "focused_build_candidate_plans_from_requests must preserve planned status"
        );
        assert_eq!(
            first_build.requested_by,
            vec!["architecture"],
            "focused_build_candidate_plans_from_requests must preserve the first lane"
        );
        assert_eq!(
            first_build.request_ids,
            vec!["build-1"],
            "focused_build_candidate_plans_from_requests must preserve the first request"
        );
        assert_eq!(
            second_build.command, "cargo doc --workspace --no-deps --locked",
            "focused_build_candidate_plans_from_requests must preserve the second command"
        );
        assert_eq!(
            second_build.status, "deferred_by_budget",
            "focused_build_candidate_plans_from_requests must defer overflow"
        );
        assert_eq!(
            second_build.request_ids,
            vec!["build-2"],
            "focused_build_candidate_plans_from_requests must preserve the second request"
        );
        Ok(())
    }

    /// v2 build candidates match v1 for the same command.
    #[test]
    fn v2_focused_build_candidates_match_v1() {
        let command = "cargo check --workspace --all-targets --locked";
        let v1 = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "req-2".to_owned(),
            lane: "correctness".to_owned(),
            requested_by: vec!["correctness".to_owned()],
            command: command.to_owned(),
            reason: String::new(),
            cost: "focused-build".to_owned(),
            timeout_sec: 120,
            required: false,
            status: "requested".to_owned(),
        }];
        let v2 = vec![v2_focused_build_request(command)];
        let from_v1 = focused_build_candidates_from_requests(&v1);
        let from_v2 = focused_build_candidates_from_v2(&v2);
        assert_eq!(from_v1.len(), from_v2.len());
        assert_eq!(from_v1.len(), 1, "the allowlisted build must resolve");
        assert_eq!(from_v1[0].command, from_v2[0].command);
        assert_eq!(from_v1[0].argv, from_v2[0].argv);
    }

    /// Typed dispatch: a v2 request with a non-test/build kind
    /// (SanitizerWitness, MiriWitness) must produce NO focused-test or
    /// focused-build candidates — it must not be misrouted to test/build
    /// execution. This is the property that lets Order 4c wire sanitizer
    /// without disturbing test/build dispatch.
    #[test]
    fn v2_non_test_build_kinds_produce_no_test_build_candidates() {
        let sanitizer = ProofRequestV2 {
            kind: ProofKind::SanitizerWitness,
            target: "config_tests".to_owned(),
            ..v2_focused_test_request("unused")
        };
        let miri = ProofRequestV2 {
            kind: ProofKind::MiriWitness,
            target: "config_tests".to_owned(),
            ..v2_focused_test_request("unused")
        };
        let requests = vec![sanitizer, miri];
        assert!(
            focused_test_candidates_from_v2(&requests).is_empty(),
            "non-focused-test kinds must not produce focused-test candidates"
        );
        assert!(
            focused_build_candidates_from_v2(&requests).is_empty(),
            "non-focused-build kinds must not produce focused-build candidates"
        );
    }

    /// A v2 FocusedTest whose target is NOT allowlisted resolves to no
    /// candidate (the security boundary holds for v2 just as for v1).
    #[test]
    fn v2_focused_test_rejects_non_allowlisted_command() {
        let v2 = vec![v2_focused_test_request("rm -rf some-directory")];
        assert!(
            focused_test_candidates_from_v2(&v2).is_empty(),
            "a non-allowlisted command must produce no candidate (security boundary)"
        );
    }

    #[test]
    fn focused_proof_budget_allows_next_enforces_count_file_and_time_caps() {
        let budget = ProofBudget {
            max_focused_test_files: 1,
            max_focused_tests: 2,
            per_command_timeout_sec: 300,
            max_total_seconds: 600,
        };
        let mut files = BTreeSet::new();
        files.insert("test/a.test.ts".to_owned());

        assert!(focused_proof_budget_allows_next(
            1,
            &files,
            "test/a.test.ts",
            300,
            150,
            FocusedProofMode::RedGreen.command_count(),
            budget,
        ));
        assert!(!focused_proof_budget_allows_next(
            2,
            &files,
            "test/a.test.ts",
            0,
            150,
            FocusedProofMode::RedGreen.command_count(),
            budget,
        ));
        assert!(!focused_proof_budget_allows_next(
            1,
            &files,
            "test/b.test.ts",
            0,
            150,
            FocusedProofMode::RedGreen.command_count(),
            budget,
        ));
        assert!(!focused_proof_budget_allows_next(
            1,
            &files,
            "test/a.test.ts",
            500,
            150,
            FocusedProofMode::RedGreen.command_count(),
            budget,
        ));
    }

    #[test]
    fn bun_focused_test_file_classifier_requires_repo_relative_test_suffixes() {
        assert!(is_bun_focused_test_file(
            "test/js/bun/md/md-edge-cases.test.ts"
        ));
        assert!(is_bun_focused_test_file(
            "tests\\node\\fs\\fs-write.test.JS"
        ));
        assert!(!is_bun_focused_test_file(
            ".\\tests\\node\\fs\\fs-write.test.js"
        ));
        assert!(!is_bun_focused_test_file(
            "src/js/bun/md/md-edge-cases.test.ts"
        ));
        assert!(!is_bun_focused_test_file("test/js/bun/md/helper.ts"));
        assert!(!is_bun_focused_test_file(
            "../test/js/bun/md/escape.test.ts"
        ));
    }

    #[test]
    fn proof_task_plan_command_formats_default_bun_and_explicit_command_specs() {
        let task = FocusedTestTask {
            id: "proof-red-green:test/js/bun/ffi/ffi.test.js:ffi toBuffer bad free".to_owned(),
            file: "test/js/bun/ffi/ffi.test.js".to_owned(),
            test_name: Some("ffi toBuffer bad free".to_owned()),
            mode: FocusedProofMode::RedGreen,
            command_specs: None,
            timeout_sec: None,
            required: false,
            requested_by: Vec::new(),
            request_ids: Vec::new(),
        };
        assert_eq!(
            proof_task_plan_command(&task, "head", "head"),
            "cwd=target/ub-review/proof-worktrees/head bun bd test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'"
        );
        assert_eq!(
            proof_task_plan_command(&task, "base-plus-tests", "base-plus-tests"),
            "cwd=target/ub-review/proof-worktrees/base-plus-tests USE_SYSTEM_BUN=1 bun test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'"
        );

        let explicit = FocusedTestTask {
            id: "proof-red-green:command:cargo-test".to_owned(),
            file: "cargo-package:ub-review".to_owned(),
            test_name: Some("focused_proof".to_owned()),
            mode: FocusedProofMode::RedGreen,
            command_specs: Some(FocusedTestCommandSpecs {
                head: ProofCommandSpec {
                    argv: vec![
                        "cargo".to_owned(),
                        "test".to_owned(),
                        "--locked".to_owned(),
                        "focused_proof".to_owned(),
                    ],
                    env: BTreeMap::new(),
                },
                base_plus_tests: ProofCommandSpec {
                    argv: vec![
                        "cargo".to_owned(),
                        "test".to_owned(),
                        "--locked".to_owned(),
                        "focused_proof".to_owned(),
                    ],
                    env: BTreeMap::new(),
                },
            }),
            timeout_sec: None,
            required: false,
            requested_by: Vec::new(),
            request_ids: Vec::new(),
        };
        assert_eq!(
            proof_task_plan_command(&explicit, "base-plus-tests", "base-plus-tests"),
            "cwd=target/ub-review/proof-worktrees/base-plus-tests cargo test --locked focused_proof"
        );
    }

    #[test]
    fn canonical_proof_request_group_command_normalizes_focused_bun_requests() {
        let command = "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots input'";
        let focused = canonical_proof_request_group_command(command, "focused-test");

        assert_ne!(focused, command);
        assert_eq!(
            focused,
            "focused-bun:test/js/bun/md/md-edge-cases.test.ts:snapshots input"
        );
        assert_eq!(
            canonical_proof_request_group_command(command, "manual"),
            command
        );
    }

    #[test]
    fn focused_test_name_arg_strips_matching_quotes_without_promoting_empty_names() {
        assert_eq!(
            focused_test_name_arg(&["-t", "'snapshots", "input'"]),
            Some("snapshots input".to_owned())
        );
        assert_eq!(focused_test_name_arg(&["-t", "x"]), Some("x".to_owned()));
        assert_eq!(focused_test_name_arg(&["-t", "'x'"]), Some("x".to_owned()));
        assert_eq!(focused_test_name_arg(&["-t", "''"]), None);
        assert_eq!(
            focused_test_name_arg(&["--test-name-pattern", "\"\""]),
            None
        );
    }

    #[test]
    fn focused_build_command_spec_accepts_only_cargo_build_family_or_exact_policy_check() {
        assert_eq!(
            focused_build_command_spec("cargo check --workspace --locked").map(|spec| spec.argv),
            Some(vec![
                "cargo".to_owned(),
                "check".to_owned(),
                "--workspace".to_owned(),
                "--locked".to_owned()
            ])
        );
        assert_eq!(
            focused_build_command_spec("cargo xtask policy-check").map(|spec| spec.argv),
            Some(vec![
                "cargo".to_owned(),
                "xtask".to_owned(),
                "policy-check".to_owned()
            ])
        );
        assert_eq!(
            focused_build_command_spec("cargo run --locked -p xtask -- check-pr")
                .map(|spec| spec.argv),
            Some(vec![
                "cargo".to_owned(),
                "run".to_owned(),
                "--locked".to_owned(),
                "-p".to_owned(),
                "xtask".to_owned(),
                "--".to_owned(),
                "check-pr".to_owned()
            ])
        );
        for rejected in [
            "npm run build --locked",
            "cargo test --workspace --locked",
            "cargo check --workspace",
            "cargo check --workspace --locked && cargo test --locked",
            "cargo xtask",
            "cargo xtask policy-check --fix",
            "cargo run -p xtask -- check-pr",
            "cargo run --locked -p xtask -- fix-pr",
            "cargo run --locked -p other -- check-pr",
        ] {
            assert!(
                focused_build_command_spec(rejected).is_none(),
                "{rejected} must not be brokered as focused build proof"
            );
        }
    }

    #[test]
    fn focused_build_command_spec_accepts_only_exact_xtask_check_pr_run() {
        assert_eq!(
            focused_build_command_spec("cargo run --locked -p xtask -- check-pr")
                .map(|spec| spec.argv),
            Some(vec![
                "cargo".to_owned(),
                "run".to_owned(),
                "--locked".to_owned(),
                "-p".to_owned(),
                "xtask".to_owned(),
                "--".to_owned(),
                "check-pr".to_owned()
            ])
        );
        assert!(
            focused_build_command_spec("cargo run -p xtask -- check-pr").is_none(),
            "missing --locked must not be accepted"
        );
        assert!(
            focused_build_command_spec("cargo run --locked -p xtask -- fix-pr").is_none(),
            "only check-pr is allowed behind xtask"
        );
        assert!(
            focused_build_command_spec("cargo run --locked -p other -- check-pr").is_none(),
            "only the xtask package is allowed"
        );
        assert!(
            focused_build_command_spec("cargo run --locked -p xtask check-pr").is_none(),
            "the explicit cargo -- separator is required"
        );
        assert!(
            focused_build_command_spec("cargo run --locked -p xtask -- check-pr --fix").is_none(),
            "additional check-pr flags are not allowed"
        );
    }

    #[test]
    fn focused_cargo_test_command_spec_pins_focus_and_passthrough_allowlist() {
        assert_eq!(
            focused_cargo_test_command_spec(
                "cargo test --test proof --locked exact_filter -- --test-threads 1 --nocapture"
            )
            .map(|spec| spec.argv),
            Some(vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "--test".to_owned(),
                "proof".to_owned(),
                "--locked".to_owned(),
                "exact_filter".to_owned(),
                "--".to_owned(),
                "--test-threads".to_owned(),
                "1".to_owned(),
                "--nocapture".to_owned()
            ])
        );
        for rejected in [
            "cargo test --locked",
            "cargo test --test proof --locked -- --test-threads many",
            "cargo test --test proof --locked -- --format json",
            "cargo test --locked focused_case && cargo doc --locked --no-deps",
        ] {
            assert!(
                focused_cargo_test_command_spec(rejected).is_none(),
                "{rejected} must not be brokered as focused test proof"
            );
        }
    }
}
