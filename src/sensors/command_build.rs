//! Sensor command building: argv construction for tokmd and generic
//! sensors (cleanup train step 35, pure code motion from sensors/mod.rs).

// Explicit imports replacing the former `use super::*;` glob (#598 first
// module). Each symbol resolves directly to its defining module via the
// crate-root re-exports.
use crate::{
    CARGO_ALLOW_NATIVE_LEDGER, Plan, SensorPlan, SensorSubcommand, TOKMD_ANALYZE_PRESET,
    UNSAFE_REVIEW_OUTPUT_SUBDIR, absolute_path, changed_paths_for_tokmd_context,
    is_github_workflow_file,
};
use std::path::Path;

pub(crate) fn sensor_run_input_path(sensor_dir: &Path, name: &str) -> String {
    sensor_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or(sensor_dir)
        .join("input")
        .join(name)
        .display()
        .to_string()
}

pub(crate) fn build_sensor_argv(
    root: &Path,
    dir: &Path,
    sensor: &SensorPlan,
    plan: &Plan,
) -> Vec<String> {
    match sensor.id.as_str() {
        "tokmd" => vec![
            "tokmd".to_owned(),
            "bundle".to_owned(),
            "analyze".to_owned(),
            "cockpit".to_owned(),
            "context".to_owned(),
            "--base".to_owned(),
            plan.base.clone(),
            "--head".to_owned(),
            plan.head.clone(),
            "--out".to_owned(),
            dir.display().to_string(),
        ],
        // `ripr check --diff` against the run's own diff.patch is the receipt
        // producer for the [tools.ripr.gate] threshold: badge-json carries the
        // unsuppressed-exposure counter the gate evaluates (#316). The old
        // `ripr first-pr` invocation only assembled a packet from artifacts
        // that nothing had generated, so the configured threshold never
        // evaluated in production.
        "ripr" => vec![
            sensor.command.clone(),
            "check".to_owned(),
            "--root".to_owned(),
            root.display().to_string(),
            "--diff".to_owned(),
            sensor_run_input_path(dir, "diff.patch"),
            "--mode".to_owned(),
            "ready".to_owned(),
            "--format".to_owned(),
            "badge-json".to_owned(),
        ],
        "unsafe-review" => vec![
            "unsafe-review".to_owned(),
            "first-pr".to_owned(),
            "--root".to_owned(),
            root.display().to_string(),
            "--base".to_owned(),
            plan.base.clone(),
            // `first-pr` writes its bundle with `--out-dir`; `--out` is for
            // other unsafe-review subcommands and does not place receipts in
            // this sensor directory.
            "--out-dir".to_owned(),
            dir.join(UNSAFE_REVIEW_OUTPUT_SUBDIR).display().to_string(),
        ],
        "cargo-allow" => {
            let mut argv = vec!["cargo-allow".to_owned(), "check".to_owned()];
            // Prefer the repo's native cargo-allow ledger over cargo-allow's
            // default discovery. `policy/allow.toml` can be an xtask-owned
            // repo-policy ledger in a different dialect that squats
            // cargo-allow's default search path, which makes `check` fail on
            // an unsupported schema instead of reading a genuine ledger.
            // https://github.com/EffortlessMetrics/cargo-allow/issues/1465
            //
            // No `--mode` is passed: cargo-allow defaults to the
            // policy-configured source-tree gate mode, so the repo ledger
            // decides whether the check is enforcing or audit-stage.
            let explicit_config = root.join(CARGO_ALLOW_NATIVE_LEDGER);
            if explicit_config.is_file() {
                argv.push("--config".to_owned());
                argv.push(explicit_config.display().to_string());
            }
            argv.extend([
                "--format".to_owned(),
                "markdown".to_owned(),
                "--receipt".to_owned(),
                dir.join("cargo-allow.receipt.json").display().to_string(),
                "--output".to_owned(),
                dir.join("cargo-allow.md").display().to_string(),
            ]);
            argv
        }
        "cargo-fmt" => vec![
            "cargo".to_owned(),
            "fmt".to_owned(),
            "--all".to_owned(),
            "--check".to_owned(),
        ],
        "cargo-check" => vec![
            "cargo".to_owned(),
            "check".to_owned(),
            "--workspace".to_owned(),
            "--all-targets".to_owned(),
            "--locked".to_owned(),
        ],
        "cargo-test" => vec![
            "cargo".to_owned(),
            "test".to_owned(),
            "--workspace".to_owned(),
            "--all-targets".to_owned(),
            "--locked".to_owned(),
        ],
        "cargo-clippy" => vec![
            "cargo".to_owned(),
            "clippy".to_owned(),
            "--workspace".to_owned(),
            "--all-targets".to_owned(),
            "--locked".to_owned(),
            "--".to_owned(),
            "-D".to_owned(),
            "warnings".to_owned(),
        ],
        "cargo-doc" => vec![
            "cargo".to_owned(),
            "doc".to_owned(),
            "--workspace".to_owned(),
            "--no-deps".to_owned(),
            "--locked".to_owned(),
        ],
        "artifact-verifier" => vec![
            "python".to_owned(),
            "scripts/verify-bun-review-artifacts.py".to_owned(),
            "--self-test".to_owned(),
        ],
        "ast-grep" => {
            let config = root.join("tools/ub-rules/sgconfig.yml");
            if config.exists() {
                vec![
                    "ast-grep".to_owned(),
                    "scan".to_owned(),
                    "--config".to_owned(),
                    config.display().to_string(),
                    root.display().to_string(),
                ]
            } else {
                vec!["ast-grep".to_owned(), "--version".to_owned()]
            }
        }
        "semgrep" => vec![
            "semgrep".to_owned(),
            "scan".to_owned(),
            "--config".to_owned(),
            "auto".to_owned(),
            "--json".to_owned(),
            "--output".to_owned(),
            dir.join("report.json").display().to_string(),
        ],
        "actionlint" => {
            let mut argv = vec![
                "actionlint".to_owned(),
                "-format".to_owned(),
                "{{json .}}".to_owned(),
            ];
            argv.extend(
                plan.changed_files
                    .iter()
                    .filter(|path| is_github_workflow_file(path) && root.join(path).is_file())
                    .cloned(),
            );
            argv
        }
        "zizmor" => vec![
            "zizmor".to_owned(),
            ".github/workflows".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
        ],
        "coverage" => vec![
            "cargo".to_owned(),
            "llvm-cov".to_owned(),
            "--workspace".to_owned(),
            "--all-features".to_owned(),
            "--locked".to_owned(),
            "--lcov".to_owned(),
            "--output-path".to_owned(),
            dir.join("lcov.info").display().to_string(),
        ],
        "gitleaks" => vec![
            "gitleaks".to_owned(),
            "detect".to_owned(),
            "--redact".to_owned(),
            "--source".to_owned(),
            root.display().to_string(),
            "--report-format".to_owned(),
            "json".to_owned(),
            "--report-path".to_owned(),
            dir.join("report.json").display().to_string(),
        ],
        "osv-scanner" => vec![
            "osv-scanner".to_owned(),
            "scan".to_owned(),
            "--recursive".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            ".".to_owned(),
        ],
        "cargo-audit" => vec!["cargo".to_owned(), "audit".to_owned(), "--json".to_owned()],
        "cargo-deny" => vec!["cargo".to_owned(), "deny".to_owned(), "check".to_owned()],
        "shellcheck" => vec!["shellcheck".to_owned(), "--version".to_owned()],
        "cppcheck" => vec!["cppcheck".to_owned(), "--version".to_owned()],
        other => vec![other.to_owned(), "--version".to_owned()],
    }
}

pub(crate) fn build_tokmd_sensor_commands(
    root: &Path,
    dir: &Path,
    plan: &Plan,
) -> Vec<SensorSubcommand> {
    let absolute_dir = absolute_path(dir);
    let context_paths = changed_paths_for_tokmd_context(root, plan);
    let analyze_paths = if context_paths.is_empty() {
        vec![".".to_owned()]
    } else {
        context_paths.clone()
    };
    let mut analyze_md_argv = vec![
        "tokmd".to_owned(),
        "analyze".to_owned(),
        "--preset".to_owned(),
        TOKMD_ANALYZE_PRESET.to_owned(),
        "--effort-base-ref".to_owned(),
        plan.base.clone(),
        "--effort-head-ref".to_owned(),
        plan.head.clone(),
        "--format".to_owned(),
        "md".to_owned(),
        "--no-progress".to_owned(),
    ];
    analyze_md_argv.extend(analyze_paths.clone());
    let mut analyze_json_argv = vec![
        "tokmd".to_owned(),
        "analyze".to_owned(),
        "--preset".to_owned(),
        TOKMD_ANALYZE_PRESET.to_owned(),
        "--effort-base-ref".to_owned(),
        plan.base.clone(),
        "--effort-head-ref".to_owned(),
        plan.head.clone(),
        "--format".to_owned(),
        "json".to_owned(),
        "--no-progress".to_owned(),
    ];
    analyze_json_argv.extend(analyze_paths);
    let mut commands = vec![
        SensorSubcommand {
            label: "analyze-md".to_owned(),
            argv: analyze_md_argv,
            stdout_path: dir.join("analyze.md"),
            stderr_path: dir.join("analyze.stderr.txt"),
        },
        SensorSubcommand {
            label: "analyze-json".to_owned(),
            argv: analyze_json_argv,
            stdout_path: dir.join("analyze.json"),
            stderr_path: dir.join("analyze-json.stderr.txt"),
        },
        SensorSubcommand {
            label: "cockpit-md".to_owned(),
            argv: vec![
                "tokmd".to_owned(),
                "cockpit".to_owned(),
                "--base".to_owned(),
                plan.base.clone(),
                "--head".to_owned(),
                plan.head.clone(),
                "--format".to_owned(),
                "md".to_owned(),
                "--no-progress".to_owned(),
            ],
            stdout_path: dir.join("cockpit.md"),
            stderr_path: dir.join("cockpit.stderr.txt"),
        },
        SensorSubcommand {
            label: "cockpit-json".to_owned(),
            argv: vec![
                "tokmd".to_owned(),
                "cockpit".to_owned(),
                "--base".to_owned(),
                plan.base.clone(),
                "--head".to_owned(),
                plan.head.clone(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-progress".to_owned(),
            ],
            stdout_path: dir.join("cockpit.json"),
            stderr_path: dir.join("cockpit-json.stderr.txt"),
        },
    ];
    if !context_paths.is_empty() {
        let mut argv = vec![
            "tokmd".to_owned(),
            "context".to_owned(),
            "--budget".to_owned(),
            "64000".to_owned(),
            "--output".to_owned(),
            absolute_dir.join("context.md").display().to_string(),
            "--force".to_owned(),
            "--no-progress".to_owned(),
        ];
        argv.extend(context_paths);
        commands.push(SensorSubcommand {
            label: "context-md".to_owned(),
            argv,
            stdout_path: dir.join("context.stdout.txt"),
            stderr_path: dir.join("context.stderr.txt"),
        });
    }
    commands
}
