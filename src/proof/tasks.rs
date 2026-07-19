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
    budget: ProofBudget,
) -> Vec<FocusedProofPlan> {
    focused_test_tasks_from_diff(diff, proof_requests, budget)
        .into_iter()
        .map(|task| {
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
                status: "planned".to_owned(),
                reason: format!(
                    "planner-only focused test target under budget: max {} file(s), {} test(s), {}s per command, {}s total",
                    budget.max_focused_test_files,
                    budget.max_focused_tests,
                    budget.per_command_timeout_sec,
                    budget.max_total_seconds
                ),
            }
        })
        .collect()
}

pub(crate) fn focused_test_tasks_from_diff(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
    budget: ProofBudget,
) -> Vec<FocusedTestTask> {
    let candidates = focused_test_candidates_from_diff(diff, proof_requests);
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
            return tasks;
        }
        files.insert(task.file.clone());
        estimated_seconds = estimated_seconds
            .saturating_add(task_timeout_sec.saturating_mul(task.mode.command_count()));
        tasks.push(task);
    }
    tasks
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
/// match_v1` in tests.
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
        command: crate::normalize_proof_command(&req.target),
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
    for group in request_groups {
        if group.status == "requested"
            && group.command.contains(file)
            && test_name
                .as_ref()
                .is_none_or(|name| group.command.contains(name))
        {
            merge_task_timeout(&mut timeout_sec, Some(group.timeout_sec));
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

    /// Native v2 flow (Order 4b): a v2 `FocusedTest` request yields the SAME
    /// focused-test candidate the v1 extractor produces for the equivalent v1
    /// command. This pins the v2→v1 normalization so the security boundary
    /// (allowlist) is preserved byte-for-byte.
    #[test]
    fn v2_focused_test_candidates_match_v1() {
        let command = "cargo test --locked --test config_tests";
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
            from_v1[0].command_specs.is_some(),
            from_v2[0].command_specs.is_some()
        );
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
