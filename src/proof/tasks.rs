//! Focused proof task and plan types.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

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

pub(crate) fn is_bun_focused_test_file(path: &str) -> bool {
    let path = normalize_repo_path(path);
    if !is_repo_relative_path(&path) {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    (lower.starts_with("test/") || lower.starts_with("tests/"))
        && [
            ".test.ts",
            ".test.tsx",
            ".test.js",
            ".test.jsx",
            ".test.mjs",
            ".test.cjs",
        ]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

pub(crate) fn focused_cargo_test_command_spec(command: &str) -> Option<ProofCommandSpec> {
    if has_shell_control_token(command) {
        return None;
    }
    let argv = command
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let [program, subcommand, args @ ..] = argv.as_slice() else {
        return None;
    };
    if program != "cargo" || subcommand != "test" {
        return None;
    }
    if !args.iter().any(|arg| arg == "--locked") {
        return None;
    }
    if !focused_cargo_test_args_allowed(args) {
        return None;
    }
    if !focused_cargo_test_has_focus(&argv) {
        return None;
    }
    Some(ProofCommandSpec {
        argv,
        env: BTreeMap::new(),
    })
}

fn focused_cargo_test_args_allowed(args: &[String]) -> bool {
    let mut index = 0;
    let mut passthrough = false;
    while index < args.len() {
        let arg = args[index].as_str();
        if !passthrough && arg == "--" {
            passthrough = true;
            index += 1;
            continue;
        }
        if passthrough {
            match arg {
                "--exact" | "--nocapture" | "--show-output" | "--ignored" | "--include-ignored" => {
                    index += 1;
                }
                "--test-threads" => {
                    let Some(value) = args.get(index + 1) else {
                        return false;
                    };
                    if value.parse::<u16>().is_err() {
                        return false;
                    }
                    index += 2;
                }
                _ => return false,
            }
            continue;
        }
        match arg {
            "--locked"
            | "--workspace"
            | "--all-targets"
            | "--all-features"
            | "--no-default-features"
            | "--tests"
            | "--lib"
            | "--bins"
            | "--examples"
            | "--doc"
            | "--offline"
            | "--frozen" => {
                index += 1;
            }
            "-p" | "--package" | "--features" | "--target" | "--test" | "--bin" | "--example" => {
                let Some(value) = args.get(index + 1) else {
                    return false;
                };
                if !safe_cargo_build_arg_value(value) {
                    return false;
                }
                index += 2;
            }
            _ if arg.starts_with("--package=")
                || arg.starts_with("--features=")
                || arg.starts_with("--target=")
                || arg.starts_with("--test=")
                || arg.starts_with("--bin=")
                || arg.starts_with("--example=") =>
            {
                let Some((_, value)) = arg.split_once('=') else {
                    return false;
                };
                if value.is_empty() || !safe_cargo_build_arg_value(value) {
                    return false;
                }
                index += 1;
            }
            _ => {
                if !safe_cargo_test_filter_value(arg) {
                    return false;
                }
                index += 1;
            }
        }
    }
    true
}

fn focused_cargo_test_has_focus(argv: &[String]) -> bool {
    cargo_arg_value(argv, "--test").is_some()
        || focused_cargo_test_filter_name(argv)
            .as_deref()
            .is_some_and(safe_cargo_test_filter_value)
}

fn safe_cargo_test_filter_value(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric()
                || matches!(ch, '_' | '-' | '.' | '/' | ':' | ',' | '+' | '=')
        })
}

fn focused_cargo_test_target_label(argv: &[String]) -> String {
    if let Some(target) = cargo_arg_value(argv, "--test") {
        return format!("cargo-test:{target}");
    }
    if let Some(package) =
        cargo_arg_value(argv, "--package").or_else(|| cargo_arg_value(argv, "-p"))
    {
        return format!("cargo-package:{package}");
    }
    "cargo-test".to_owned()
}

fn focused_cargo_test_filter_name(argv: &[String]) -> Option<String> {
    let mut index = 2;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == "--" {
            return None;
        }
        if matches!(
            arg,
            "-p" | "--package" | "--features" | "--target" | "--test" | "--bin" | "--example"
        ) {
            index += 2;
            continue;
        }
        if arg.starts_with("--package=")
            || arg.starts_with("--features=")
            || arg.starts_with("--target=")
            || arg.starts_with("--test=")
            || arg.starts_with("--bin=")
            || arg.starts_with("--example=")
            || matches!(
                arg,
                "--locked"
                    | "--workspace"
                    | "--all-targets"
                    | "--all-features"
                    | "--no-default-features"
                    | "--tests"
                    | "--lib"
                    | "--bins"
                    | "--examples"
                    | "--doc"
                    | "--offline"
                    | "--frozen"
            )
        {
            index += 1;
            continue;
        }
        return Some(arg.to_owned());
    }
    None
}

fn cargo_arg_value<'a>(argv: &'a [String], name: &str) -> Option<&'a str> {
    let equals_prefix = format!("{name}=");
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].as_str();
        if arg == name {
            return argv.get(index + 1).map(String::as_str);
        }
        if let Some(value) = arg.strip_prefix(&equals_prefix) {
            return Some(value);
        }
        index += 1;
    }
    None
}

pub(crate) fn focused_build_command_spec_for_task(task: &FocusedBuildTask) -> ProofCommandSpec {
    ProofCommandSpec {
        argv: task.argv.clone(),
        env: BTreeMap::new(),
    }
}

pub(crate) fn focused_build_command_spec(command: &str) -> Option<ProofCommandSpec> {
    if has_shell_control_token(command) {
        return None;
    }
    let argv = command
        .split_whitespace()
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    let [program, subcommand, args @ ..] = argv.as_slice() else {
        return None;
    };
    if program != "cargo" {
        return None;
    }
    let args_str = args.iter().map(String::as_str).collect::<Vec<_>>();
    // Exact repo-local xtask commands the proof broker may run:
    // - ub-review's parse-only policy receipt
    // - unsafe-review-swarm's existing required check-pr floor
    //
    // Keep both exact so generic xtask/cargo-run commands never become a
    // proof-broker escape hatch.
    match (subcommand.as_str(), args_str.as_slice()) {
        ("xtask", ["policy-check"]) | ("run", ["--locked", "-p", "xtask", "--", "check-pr"]) => {
            return Some(ProofCommandSpec {
                argv: argv.clone(),
                env: BTreeMap::new(),
            });
        }
        _ => {}
    }
    if !matches!(subcommand.as_str(), "build" | "check" | "doc") {
        return None;
    }
    if !args.iter().any(|arg| arg == "--locked") {
        return None;
    }
    if !focused_cargo_build_args_allowed(args) {
        return None;
    }
    Some(ProofCommandSpec {
        argv,
        env: BTreeMap::new(),
    })
}

fn focused_cargo_build_args_allowed(args: &[String]) -> bool {
    let mut index = 0;
    while index < args.len() {
        let arg = args[index].as_str();
        match arg {
            "--locked"
            | "--workspace"
            | "--all-targets"
            | "--all-features"
            | "--no-default-features"
            | "--release"
            | "--tests"
            | "--benches"
            | "--examples"
            | "--bins"
            | "--lib"
            | "--no-deps"
            | "--offline"
            | "--frozen" => {
                index += 1;
            }
            "-p" | "--package" | "--features" | "--target" => {
                let Some(value) = args.get(index + 1) else {
                    return false;
                };
                if !safe_cargo_build_arg_value(value) {
                    return false;
                }
                index += 2;
            }
            _ if arg.starts_with("--package=")
                || arg.starts_with("--features=")
                || arg.starts_with("--target=") =>
            {
                let Some((_, value)) = arg.split_once('=') else {
                    return false;
                };
                if value.is_empty() || !safe_cargo_build_arg_value(value) {
                    return false;
                }
                index += 1;
            }
            _ => return false,
        }
    }
    true
}

fn safe_cargo_build_arg_value(value: &str) -> bool {
    !value.is_empty()
        && value.chars().all(|ch| {
            ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '/' | ':' | ',' | '+')
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FocusedProofMode {
    HeadOnly,
    RedGreen,
}

impl FocusedProofMode {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::HeadOnly => "head-only",
            Self::RedGreen => "red-green",
        }
    }

    pub(crate) fn command_count(self) -> u64 {
        match self {
            Self::HeadOnly => 1,
            Self::RedGreen => 2,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FocusedProofPlan {
    pub(crate) id: String,
    pub(crate) test_file: String,
    pub(crate) test_name: Option<String>,
    pub(crate) mode: FocusedProofMode,
    pub(crate) timeout_sec: u64,
    pub(crate) head_command: String,
    pub(crate) base_plus_tests_command: String,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
    pub(crate) status: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug)]
pub(crate) struct FocusedBuildPlan {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) timeout_sec: u64,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
    pub(crate) status: String,
    pub(crate) reason: String,
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
