//! Cargo test/build command parsing: focused-test detection, cargo
//! arg validation, filter extraction, and build command spec derivation
//! (cleanup train step 36, pure code motion from proof/tasks.rs).

// Explicit imports replacing the former `use super::*;` glob (#598 second
// module). Types from proof siblings + functions from the crate root.
use super::{FocusedBuildTask, ProofCommandSpec};
use crate::{has_shell_control_token, is_repo_relative_path, normalize_repo_path};
use std::collections::BTreeMap;

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

pub(crate) fn focused_cargo_test_target_label(argv: &[String]) -> String {
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

pub(crate) fn focused_cargo_test_filter_name(argv: &[String]) -> Option<String> {
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
    // `cargo xtask policy-check` is the repo-local parse-only policy receipt
    // validation (see xtask/src/main.rs). Only this exact invocation is
    // brokered so xtask cannot smuggle arbitrary repo commands into the
    // focused proof lane.
    if subcommand == "xtask" {
        return (args == ["policy-check"]).then(|| ProofCommandSpec {
            argv: argv.clone(),
            env: BTreeMap::new(),
        });
    }
    // unsafe-review-swarm already requires `xtask check-pr`; broker only that
    // exact cargo invocation so generic cargo-run commands cannot enter proof.
    let args_str = args.iter().map(String::as_str).collect::<Vec<_>>();
    match (subcommand.as_str(), args_str.as_slice()) {
        ("run", ["--locked", "-p", "xtask", "--", "check-pr"]) => {
            return Some(ProofCommandSpec {
                argv: argv.clone(),
                env: BTreeMap::new(),
            });
        }
        ("run", _) => return None,
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
