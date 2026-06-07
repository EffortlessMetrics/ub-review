//! Sensor execution: command construction, the sensor runner, status
//! receipts, and per-sensor artifact enumeration (cleanup train step 6,
//! pure code motion). Plan-time trigger resolution stays in main.

pub(crate) mod coverage;
pub(crate) mod ripr;
pub(crate) mod unsafe_review;

pub(crate) use coverage::*;
pub(crate) use ripr::*;
pub(crate) use unsafe_review::*;

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::*;

pub(crate) fn run_sensors(
    root: &Path,
    out: &Path,
    plan: &Plan,
    profile: &Profile,
    event_log: &EventLog,
) -> Result<()> {
    let runnable = plan
        .sensors
        .iter()
        .filter(|sensor| sensor.run)
        .cloned()
        .collect::<VecDeque<_>>();
    if runnable.is_empty() {
        event_log.append("sensors_empty", serde_json::json!({}))?;
        return Ok(());
    }
    let jobs = sensor_job_count(profile, runnable.len())?;
    let queue = Arc::new(Mutex::new(runnable));
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));

    thread::scope(|scope| {
        for _ in 0..jobs {
            let queue = Arc::clone(&queue);
            let failures = Arc::clone(&failures);
            scope.spawn(move || {
                loop {
                    let sensor = match queue.lock() {
                        Ok(mut queue) => queue.pop_front(),
                        Err(_) => None,
                    };
                    let Some(sensor) = sensor else {
                        break;
                    };
                    if let Err(err) = run_sensor(root, out, &sensor, event_log, plan)
                        && let Ok(mut failures) = failures.lock()
                    {
                        failures.push(format!("{}: {err:#}", sensor.id));
                    }
                }
            });
        }
    });

    let failures = failures
        .lock()
        .map_err(|_| anyhow::anyhow!("failure list mutex poisoned"))?;
    if !failures.is_empty() {
        event_log.append(
            "sensor_degraded",
            serde_json::json!({"failures": &*failures}),
        )?;
    }
    Ok(())
}

pub(crate) fn sensor_job_count(profile: &Profile, runnable_len: usize) -> Result<usize> {
    if profile.limits.sensor_jobs == 0 {
        bail!(
            "runtime profile {} has sensor_jobs=0; sensors cannot be scheduled",
            profile.name
        );
    }
    Ok(profile.limits.sensor_jobs.min(runnable_len))
}

pub(crate) fn run_sensor(
    root: &Path,
    out: &Path,
    sensor: &SensorPlan,
    event_log: &EventLog,
    plan: &Plan,
) -> Result<()> {
    let dir = out.join("sensors").join(&sensor.id);
    fs::create_dir_all(&dir)?;
    let argv = build_sensor_argv(root, &dir, sensor, plan);
    if !command_on_path(&sensor.command) {
        write_sensor_status(
            out,
            sensor,
            SensorStatusWrite {
                status: "missing",
                argv: &argv,
                duration_ms: 0,
                reason: "command not found",
                exit_code: None,
                timed_out: false,
            },
        )?;
        event_log.append(
            "sensor_missing_command",
            serde_json::json!({"sensor": sensor.id, "command": sensor.command}),
        )?;
        return Ok(());
    }
    event_log.append(
        "sensor_started",
        serde_json::json!({"sensor": sensor.id, "argv": argv}),
    )?;
    if sensor.id == "tokmd" {
        return run_tokmd_sensor(root, out, &dir, sensor, event_log, plan, &argv);
    }
    let stdout_path = dir.join("stdout.txt");
    let stderr_path = dir.join("stderr.txt");
    let result = run_command_to_files(
        root,
        &argv,
        &BTreeMap::new(),
        sensor.timeout_sec,
        &stdout_path,
        &stderr_path,
    );
    match result {
        Ok(result) => {
            let status = if result.timed_out {
                "timed_out"
            } else if result.success {
                "ok"
            } else {
                "failed"
            };
            // ripr emits its badge-json receipt on stdout; the verbatim bytes
            // become the gate-decision receipt so the threshold evaluates
            // against exactly what the tool shipped (#316). Copy only on ok:
            // a failed or timed-out sensor must stay missing evidence, never
            // a half-written receipt.
            if sensor.id == "ripr" && status == "ok" {
                fs::copy(&stdout_path, dir.join("gate-decision.json"))
                    .with_context(|| format!("copy {} gate receipt", sensor.id))?;
                // #347: badge-json carries counts only, so a tool-gate red
                // was not diagnosable from artifacts. A second bounded ripr
                // pass persists per-finding exposure-gap detail; its failure
                // never changes the sensor status - the threshold input
                // stays the verbatim badge receipt above.
                write_ripr_exposure_gap_details(root, &dir, sensor.timeout_sec);
            }
            write_sensor_status(
                out,
                sensor,
                SensorStatusWrite {
                    status,
                    argv: &argv,
                    duration_ms: result.duration_ms,
                    reason: &result.reason,
                    exit_code: result.exit_code,
                    timed_out: result.timed_out,
                },
            )?;
            event_log.append(
                if result.success {
                    "sensor_completed"
                } else {
                    "sensor_failed"
                },
                serde_json::json!({"sensor": sensor.id, "exit_code": result.exit_code, "timed_out": result.timed_out, "reason": result.reason}),
            )?;
        }
        Err(err) => {
            let reason = format!("{err:#}");
            write_sensor_status(
                out,
                sensor,
                SensorStatusWrite {
                    status: "failed",
                    argv: &argv,
                    duration_ms: 0,
                    reason: &reason,
                    exit_code: None,
                    timed_out: false,
                },
            )?;
            event_log.append(
                "sensor_failed",
                serde_json::json!({"sensor": sensor.id, "reason": reason}),
            )?;
        }
    }
    Ok(())
}

/// #319: name the actionable root cause when an out-of-pin tokmd rejects
/// `--preset bun-ub` with a clap usage error. Pure so the reason text is
/// testable without a tokmd binary. Returns `None` when the version matches
/// the pin or when no version could be determined - in the latter case the
/// run stays fail-closed through the natural subcommand failure path rather
/// than asserting a mismatch the preflight cannot prove.
pub(crate) fn tokmd_pin_mismatch_note(actual: Option<&str>, expected: &str) -> Option<String> {
    let actual = actual?;
    if command_version_matches(actual, expected) {
        return None;
    }
    Some(format!(
        "installed tokmd ({actual}) does not match pinned {expected} \
         (the bun-ub preset requires {expected})"
    ))
}

pub(crate) fn run_tokmd_sensor(
    root: &Path,
    out: &Path,
    dir: &Path,
    sensor: &SensorPlan,
    event_log: &EventLog,
    plan: &Plan,
    aggregate_argv: &[String],
) -> Result<()> {
    // #319: preflight the version pin before executing subcommands, so a
    // drifted runner image fails with "upgrade tokmd" readable from
    // status.json instead of a bare clap exit-2 on `--preset bun-ub`.
    let pin_note = expected_standard_image_tool_version("tokmd").and_then(|expected| {
        tokmd_pin_mismatch_note(command_version(&sensor.command).as_deref(), expected)
    });
    let commands = build_tokmd_sensor_commands(root, dir, plan);
    fs::write(
        dir.join("commands.json"),
        serde_json::to_vec_pretty(&commands_json(&commands))?,
    )?;
    let aggregate_stdout_path = dir.join("stdout.txt");
    let aggregate_stderr_path = dir.join("stderr.txt");
    fs::write(&aggregate_stdout_path, b"")?;
    fs::write(&aggregate_stderr_path, b"")?;

    let started = Instant::now();
    let mut failures = Vec::new();
    let mut timed_out = false;
    let mut exit_code = Some(0);
    for command in &commands {
        event_log.append(
            "sensor_subcommand_started",
            serde_json::json!({"sensor": sensor.id, "label": command.label, "argv": command.argv}),
        )?;
        let result = run_command_to_files(
            root,
            &command.argv,
            &BTreeMap::new(),
            sensor.timeout_sec,
            &command.stdout_path,
            &command.stderr_path,
        );
        match result {
            Ok(result) => {
                append_file(
                    &aggregate_stdout_path,
                    &format!(
                        "$ {}\nstatus={} duration_ms={}\n\n",
                        display_command(&command.argv),
                        result.reason,
                        result.duration_ms
                    ),
                )?;
                append_existing_file(&aggregate_stdout_path, &command.stdout_path)?;
                append_file(&aggregate_stdout_path, "\n")?;
                append_existing_file(&aggregate_stderr_path, &command.stderr_path)?;
                if result.timed_out {
                    timed_out = true;
                }
                if !result.success {
                    if exit_code == Some(0) {
                        exit_code = result.exit_code;
                    }
                    failures.push(format!("{} {}", command.label, result.reason));
                }
                event_log.append(
                    if result.success {
                        "sensor_subcommand_completed"
                    } else {
                        "sensor_subcommand_failed"
                    },
                    serde_json::json!({"sensor": sensor.id, "label": command.label, "exit_code": result.exit_code, "timed_out": result.timed_out, "reason": result.reason}),
                )?;
            }
            Err(err) => {
                let reason = format!("{err:#}");
                failures.push(format!("{} {reason}", command.label));
                if exit_code == Some(0) {
                    exit_code = None;
                }
                event_log.append(
                    "sensor_subcommand_failed",
                    serde_json::json!({"sensor": sensor.id, "label": command.label, "reason": reason}),
                )?;
            }
        }
    }

    let duration_ms = started.elapsed().as_millis();
    let context_path = dir.join("context.md");
    if !context_path.exists() {
        fs::write(
            &context_path,
            "No existing changed paths were available for bounded tokmd context.\n",
        )?;
    }

    // A version mismatch leads the failure reason: it is the actionable
    // root cause, and the missing-evidence line in the lane packet carries
    // whatever stands here. The ok path is untouched - a mismatched but
    // fully succeeding tokmd is not a failure to explain.
    let with_pin_note = |detail: String| match &pin_note {
        Some(note) => format!("{note}; {detail}"),
        None => detail,
    };
    let (status, reason) = if failures.is_empty() {
        ("ok", format!("{} tokmd receipts completed", commands.len()))
    } else if timed_out {
        (
            "timed_out",
            with_pin_note(format!(
                "tokmd subcommands timed out or failed: {}",
                failures.join("; ")
            )),
        )
    } else {
        (
            "failed",
            with_pin_note(format!("tokmd subcommands failed: {}", failures.join("; "))),
        )
    };
    write_sensor_status(
        out,
        sensor,
        SensorStatusWrite {
            status,
            argv: aggregate_argv,
            duration_ms,
            reason: &reason,
            exit_code,
            timed_out,
        },
    )?;
    event_log.append(
        if failures.is_empty() {
            "sensor_completed"
        } else {
            "sensor_failed"
        },
        serde_json::json!({"sensor": sensor.id, "reason": reason}),
    )?;
    Ok(())
}

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
            "ripr".to_owned(),
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
        // `first-pr` takes `--out-dir` (verified against the real 0.3.4
        // binary); `--out` belongs to `check`/`repo` and is SILENTLY ignored
        // by `first-pr` — exit 0, bundle written to the default
        // `target/unsafe-review` inside the checkout, gate artifact absent
        // from the sensor dir. That produced the "sensor ok, required
        // artifact absent" gate failure on PR #387 (run 27102118267).
        // Silent unknown-flag acceptance is filed upstream as
        // EffortlessMetrics/unsafe-review#531.
        "unsafe-review" => vec![
            "unsafe-review".to_owned(),
            "first-pr".to_owned(),
            "--root".to_owned(),
            root.display().to_string(),
            "--base".to_owned(),
            plan.base.clone(),
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

pub(crate) fn write_sensor_status(
    out: &Path,
    sensor: &SensorPlan,
    fields: SensorStatusWrite<'_>,
) -> Result<()> {
    let dir = out.join("sensors").join(&sensor.id);
    fs::create_dir_all(&dir)?;
    ensure_sensor_text_receipts(&dir)?;
    let mut value = serde_json::json!({
        "sensor": sensor.id,
        "status": fields.status,
        "command": display_command(fields.argv),
        "duration_ms": fields.duration_ms,
        "reason": fields.reason,
        "outputs": sensor_outputs(sensor),
        "exit_code": fields.exit_code,
        "timed_out": fields.timed_out,
        "timeout_sec": sensor.timeout_sec,
        "class": sensor.class,
        "requires_lease": sensor.requires_lease,
        "required": sensor.required,
    });
    if let Some(gate) = &sensor.gate {
        value["gate"] = serde_json::to_value(gate)?;
    }
    fs::write(
        dir.join("ub-review-sensor-status.json"),
        serde_json::to_vec_pretty(&value)?,
    )?;
    if sensor.id == "coverage" {
        write_coverage_status_receipt(&dir, fields)?;
    }
    Ok(())
}

pub(crate) fn ensure_sensor_text_receipts(dir: &Path) -> Result<()> {
    for name in ["stdout.txt", "stderr.txt"] {
        let path = dir.join(name);
        if !path.exists() {
            fs::write(path, b"")?;
        }
    }
    Ok(())
}

pub(crate) fn sensor_outputs(sensor: &SensorPlan) -> Vec<String> {
    let mut outputs = vec!["stdout.txt".to_owned(), "stderr.txt".to_owned()];
    match sensor.id.as_str() {
        "tokmd" => outputs.extend([
            "commands.json".to_owned(),
            "analyze.md".to_owned(),
            "analyze.json".to_owned(),
            "cockpit.md".to_owned(),
            "cockpit.json".to_owned(),
            "context.md".to_owned(),
        ]),
        "cargo-allow" => outputs.extend([
            "cargo-allow.md".to_owned(),
            "cargo-allow.receipt.json".to_owned(),
        ]),
        // The badge-json receipt copied verbatim from sensor stdout; the
        // [tools.ripr.gate] threshold evaluates against it (#316).
        "ripr" => outputs.extend([
            "gate-decision.json".to_owned(),
            // Per-finding gap detail (#347); detail_unavailable when the
            // second pass failed, so absence of detail is itself receipted.
            "exposure-gaps.json".to_owned(),
        ]),
        // unsafe-review 0.3.4 structured output bundle (#359). Filenames match
        // the REAL `first-pr --out-dir` manifest's `artifacts` map (note
        // `receipt-audit.md` and `pr-summary.md` are Markdown, not JSON). The
        // gate file and the artifact files it points to are all written under
        // the UNSAFE_REVIEW_OUTPUT_SUBDIR subdirectory of the sensor dir.
        "unsafe-review" => outputs.extend([
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/unsafe-review-gate.json"),
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/cards.json"),
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/comment-plan.json"),
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/repair-queue.json"),
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/receipt-audit.md"),
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/review-kit.json"),
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/pr-summary.md"),
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/cards.sarif"),
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/lsp.json"),
            format!("{UNSAFE_REVIEW_OUTPUT_SUBDIR}/policy-report.json"),
        ]),
        "ast-grep" | "semgrep" | "gitleaks" => outputs.push("report.json".to_owned()),
        "coverage" => outputs.extend([
            "status.json".to_owned(),
            "coverage-summary.json".to_owned(),
            "changed-lines.json".to_owned(),
            "upload.json".to_owned(),
            "lcov.info".to_owned(),
        ]),
        _ => {}
    }
    outputs
}

pub(crate) fn display_command(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg.chars().any(char::is_whitespace) {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::Result;

    use crate::tests::{run_test_command, sensor_plan, sleeper_argv, test_diff, test_plan};
    use crate::*;

    #[test]
    fn baseline_gate_sensor_argvs_match_required_matrix() -> Result<()> {
        let mut config: Config = toml::from_str(include_str!("../../.ub-review.toml"))?;
        config.merge_defaults();
        let plan = super::build_plan(
            &config,
            config.selected_profile()?,
            &BoxState {
                cpus: 4,
                free_mem_mb: Some(8_000),
                free_disk_mb: Some(20_000),
                load_1m: Some(0.5),
                github_actions: true,
            },
            &test_diff(),
            Path::new("."),
            true,
        );
        let cases = [
            ("cargo-fmt", vec!["cargo", "fmt", "--all", "--check"]),
            (
                "cargo-check",
                vec!["cargo", "check", "--workspace", "--all-targets", "--locked"],
            ),
            (
                "cargo-test",
                vec!["cargo", "test", "--workspace", "--all-targets", "--locked"],
            ),
            (
                "cargo-clippy",
                vec![
                    "cargo",
                    "clippy",
                    "--workspace",
                    "--all-targets",
                    "--locked",
                    "--",
                    "-D",
                    "warnings",
                ],
            ),
            (
                "cargo-doc",
                vec!["cargo", "doc", "--workspace", "--no-deps", "--locked"],
            ),
            (
                "artifact-verifier",
                vec![
                    "python",
                    "scripts/verify-bun-review-artifacts.py",
                    "--self-test",
                ],
            ),
        ];
        for (id, expected) in cases {
            let sensor = plan
                .sensors
                .iter()
                .find(|sensor| sensor.id == id)
                .ok_or_else(|| anyhow::anyhow!("missing planned sensor {id}"))?;
            assert!(sensor.run, "{id} should run in the self-gate plan");
            let argv = super::build_sensor_argv(
                Path::new("."),
                Path::new("target/ub-review/sensors/x"),
                sensor,
                &plan,
            );
            assert_eq!(argv, expected);
        }

        // The ripr sensor is the gate-receipt producer (#316): check mode
        // against the run's own diff.patch, badge-json on stdout.
        let ripr = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "ripr")
            .ok_or_else(|| anyhow::anyhow!("missing planned sensor ripr"))?;
        assert!(ripr.run, "ripr should run in the self-gate plan");
        let argv = super::build_sensor_argv(
            Path::new("."),
            Path::new("target/ub-review/sensors/x"),
            ripr,
            &plan,
        );
        let diff_path = Path::new("target/ub-review")
            .join("input")
            .join("diff.patch")
            .display()
            .to_string();
        assert_eq!(
            argv,
            vec![
                "ripr".to_owned(),
                "check".to_owned(),
                "--root".to_owned(),
                ".".to_owned(),
                "--diff".to_owned(),
                diff_path,
                "--mode".to_owned(),
                "ready".to_owned(),
                "--format".to_owned(),
                "badge-json".to_owned(),
            ]
        );
        Ok(())
    }

    #[test]
    fn cargo_allow_sensor_command_writes_exception_receipts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let out = root.join("out");
        let plan = test_plan(vec![sensor_plan("cargo-allow", "cargo-allow", true)]);
        let sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "cargo-allow")
            .ok_or_else(|| anyhow::anyhow!("cargo-allow sensor missing"))?;
        let dir = out.join("sensors/cargo-allow");
        let argv = super::build_sensor_argv(root, &dir, sensor, &plan);

        assert_eq!(
            argv,
            vec![
                "cargo-allow".to_owned(),
                "check".to_owned(),
                "--format".to_owned(),
                "markdown".to_owned(),
                "--receipt".to_owned(),
                dir.join("cargo-allow.receipt.json").display().to_string(),
                "--output".to_owned(),
                dir.join("cargo-allow.md").display().to_string(),
            ]
        );
        assert_eq!(
            super::sensor_outputs(sensor),
            vec![
                "stdout.txt".to_owned(),
                "stderr.txt".to_owned(),
                "cargo-allow.md".to_owned(),
                "cargo-allow.receipt.json".to_owned(),
            ]
        );
        Ok(())
    }

    #[test]
    fn cargo_allow_sensor_command_pins_native_ledger_when_present() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir_all(root.join("policy"))?;
        // A repo-policy ledger in a foreign dialect squatting cargo-allow's
        // default discovery path must not be what the sensor reads.
        fs::write(root.join("policy/allow.toml"), "schema_version = \"1\"\n")?;
        fs::write(
            root.join(super::CARGO_ALLOW_NATIVE_LEDGER),
            "schema_version = \"0.1\"\npolicy = \"cargo-allow\"\n",
        )?;
        let out = root.join("out");
        let plan = test_plan(vec![sensor_plan("cargo-allow", "cargo-allow", true)]);
        let sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "cargo-allow")
            .ok_or_else(|| anyhow::anyhow!("cargo-allow sensor missing"))?;
        let dir = out.join("sensors/cargo-allow");
        let argv = super::build_sensor_argv(root, &dir, sensor, &plan);

        assert_eq!(
            argv,
            vec![
                "cargo-allow".to_owned(),
                "check".to_owned(),
                "--config".to_owned(),
                root.join(super::CARGO_ALLOW_NATIVE_LEDGER)
                    .display()
                    .to_string(),
                "--format".to_owned(),
                "markdown".to_owned(),
                "--receipt".to_owned(),
                dir.join("cargo-allow.receipt.json").display().to_string(),
                "--output".to_owned(),
                dir.join("cargo-allow.md").display().to_string(),
            ]
        );
        Ok(())
    }

    #[test]
    fn cargo_allow_sensor_skips_without_policy_config() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut diff = test_diff();
        diff.flags.source_changed = false;
        diff.flags.workflow_changed = true;
        diff.diff_class = DiffClass::WorkflowTooling;
        diff.changed_files = vec![".github/workflows/ub-review-packet.yml".to_owned()];
        let tool = super::ToolPolicy {
            id: "cargo-allow".to_owned(),
            command: "cargo-allow".to_owned(),
            default: super::Trigger::SourceExceptionChanged,
            ..super::ToolPolicy::default()
        };

        let skipped = super::plan_tool(&tool, &Profile::default(), &diff, temp.path(), true, false);

        assert!(!skipped.run);
        assert_eq!(skipped.reason, "cargo-allow policy config not found");

        diff.flags.workflow_changed = false;
        let not_matched =
            super::plan_tool(&tool, &Profile::default(), &diff, temp.path(), true, false);

        assert!(!not_matched.run);
        assert_eq!(not_matched.reason, "trigger did not match this diff");

        diff.flags.workflow_changed = true;
        fs::create_dir_all(temp.path().join("policy"))?;
        // #318: a foreign-dialect ledger squatting cargo-allow's discovery
        // path is not a config; the sensor skips with a reason naming the
        // squatting file and linking the upstream issue instead of running
        // unpinned and red-failing on cargo-allow's exit-2 schema error.
        fs::write(
            temp.path().join("policy/allow.toml"),
            "schema_version = \"1\"\ntool = \"cargo-allow\"\n",
        )?;
        let foreign = super::plan_tool(&tool, &Profile::default(), &diff, temp.path(), true, false);

        assert!(!foreign.run);
        assert_eq!(
            foreign.reason,
            "policy/allow.toml is not a cargo-allow-dialect ledger; add \
             policy/cargo-allow.toml (see EffortlessMetrics/cargo-allow#1465)"
        );

        // The native ledger arriving makes the same diff plan the sensor;
        // the foreign file stays ignored rather than shadowing it.
        fs::write(
            temp.path().join(super::CARGO_ALLOW_NATIVE_LEDGER),
            "schema_version = \"0.1\"\npolicy = \"cargo-allow\"\n",
        )?;
        let planned = super::plan_tool(&tool, &Profile::default(), &diff, temp.path(), true, false);

        assert!(planned.run);
        assert_eq!(planned.reason, "source-tree exception surface changed");
        Ok(())
    }

    #[test]
    fn actionlint_sensor_uses_json_template_format() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let plan = test_plan(vec![sensor_plan("actionlint", "actionlint", true)]);
        let sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "actionlint")
            .ok_or_else(|| anyhow::anyhow!("actionlint sensor missing"))?;
        let dir = out.join("sensors/actionlint");
        let argv = super::build_sensor_argv(temp.path(), &dir, sensor, &plan);

        assert_eq!(
            argv,
            vec![
                "actionlint".to_owned(),
                "-format".to_owned(),
                "{{json .}}".to_owned(),
            ]
        );
        Ok(())
    }

    #[test]
    fn actionlint_sensor_scopes_to_changed_workflow_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join(".github/workflows"))?;
        fs::write(temp.path().join(".github/workflows/ci.yml"), "name: CI\n")?;
        let out = temp.path().join("out");
        let mut plan = test_plan(vec![sensor_plan("actionlint", "actionlint", true)]);
        plan.changed_files = vec![
            ".github/workflows/ci.yml".to_owned(),
            ".github/actions/setup/action.yml".to_owned(),
            "src/lib.rs".to_owned(),
        ];
        let sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "actionlint")
            .ok_or_else(|| anyhow::anyhow!("actionlint sensor missing"))?;
        let dir = out.join("sensors/actionlint");
        let argv = super::build_sensor_argv(temp.path(), &dir, sensor, &plan);

        assert_eq!(
            argv,
            vec![
                "actionlint".to_owned(),
                "-format".to_owned(),
                "{{json .}}".to_owned(),
                ".github/workflows/ci.yml".to_owned(),
            ]
        );
        Ok(())
    }

    #[test]
    fn tokmd_pin_mismatch_note_names_installed_and_pinned_versions() {
        // #319: a drifted tokmd must fail with the version delta readable
        // from the sensor reason, not a bare clap exit-2 on --preset bun-ub.
        let note =
            super::tokmd_pin_mismatch_note(Some("tokmd 1.11.1"), "1.12.0").unwrap_or_default();
        assert!(note.contains("tokmd 1.11.1"), "names installed: {note}");
        assert!(note.contains("1.12.0"), "names pin: {note}");
        assert!(note.contains("bun-ub preset"), "names the why: {note}");

        // Matching version (with or without a v prefix): no note, so the ok
        // and failure paths are byte-identical to the pre-#319 behavior.
        assert_eq!(
            super::tokmd_pin_mismatch_note(Some("tokmd 1.12.0"), "1.12.0"),
            None
        );
        assert_eq!(
            super::tokmd_pin_mismatch_note(Some("tokmd v1.12.0"), "1.12.0"),
            None
        );

        // No --version output: the preflight cannot prove a mismatch, so it
        // stays silent and the run fails closed through the subcommand
        // failures it would have had anyway.
        assert_eq!(super::tokmd_pin_mismatch_note(None, "1.12.0"), None);
    }

    #[test]
    fn tokmd_sensor_commands_use_on_diff_analyze_cockpit_and_context() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("repo");
        fs::create_dir_all(repo.join("src"))?;
        fs::write(repo.join("src/lib.rs"), "pub fn value() -> usize { 1 }\n")?;
        run_test_command(&repo, "git", &["init"])?;
        run_test_command(
            &repo,
            "git",
            &["config", "user.email", "ub-review@example.invalid"],
        )?;
        run_test_command(&repo, "git", &["config", "user.name", "UB Review Test"])?;
        run_test_command(&repo, "git", &["add", "."])?;
        run_test_command(&repo, "git", &["commit", "-m", "baseline"])?;
        fs::write(repo.join("src/lib.rs"), "pub fn value() -> usize { 2 }\n")?;
        run_test_command(&repo, "git", &["add", "."])?;
        run_test_command(&repo, "git", &["commit", "-m", "touch source"])?;

        let plan = test_plan(vec![sensor_plan("tokmd", "tokmd", true)]);
        let dir = temp.path().join("out/sensors/tokmd");
        let commands = build_tokmd_sensor_commands(&repo, &dir, &plan);
        let command_texts = commands
            .iter()
            .map(|command| command.argv.join(" "))
            .collect::<Vec<_>>();

        assert!(command_texts.iter().any(|command| command.contains(
            "tokmd analyze --preset bun-ub --effort-base-ref HEAD~1 --effort-head-ref HEAD"
        )));
        assert!(
            command_texts
                .iter()
                .filter(|command| command.contains("tokmd analyze --preset bun-ub"))
                .all(|command| command.contains("src/lib.rs")),
            "tokmd analyze commands should stay scoped to changed files: {command_texts:?}"
        );
        assert!(
            command_texts
                .iter()
                .any(|command| command.contains("tokmd cockpit --base HEAD~1 --head HEAD"))
        );
        assert!(
            command_texts
                .iter()
                .any(|command| command.contains("tokmd context"))
        );
        assert!(
            command_texts
                .iter()
                .any(|command| command.contains("src/lib.rs"))
        );
        assert!(
            command_texts
                .iter()
                .any(|command| command.contains("tokmd context --budget 64000"))
        );
        assert!(
            command_texts
                .iter()
                .all(|command| !command.contains("--mode bundle")),
            "tokmd context.md should be an auditable list receipt, not a source bundle"
        );
        assert!(
            commands
                .iter()
                .any(|command| command.stdout_path.ends_with("analyze.md"))
        );
        assert!(
            commands
                .iter()
                .any(|command| command.stdout_path.ends_with("cockpit.json"))
        );
        assert!(
            commands
                .iter()
                .any(|command| command.argv.iter().any(|arg| arg.ends_with("context.md")))
        );
        Ok(())
    }

    #[test]
    fn coverage_sensor_command_writes_lcov_artifact_when_enabled() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let out = root.join("out");
        let plan = test_plan(vec![sensor_plan("coverage", "cargo", true)]);
        let sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "coverage")
            .ok_or_else(|| anyhow::anyhow!("coverage sensor missing"))?;
        let dir = out.join("sensors/coverage");
        let argv = super::build_sensor_argv(root, &dir, sensor, &plan);

        assert_eq!(
            argv,
            vec![
                "cargo".to_owned(),
                "llvm-cov".to_owned(),
                "--workspace".to_owned(),
                "--all-features".to_owned(),
                "--locked".to_owned(),
                "--lcov".to_owned(),
                "--output-path".to_owned(),
                dir.join("lcov.info").display().to_string(),
            ]
        );
        assert_eq!(
            super::sensor_outputs(sensor),
            vec![
                "stdout.txt".to_owned(),
                "stderr.txt".to_owned(),
                "status.json".to_owned(),
                "coverage-summary.json".to_owned(),
                "changed-lines.json".to_owned(),
                "upload.json".to_owned(),
                "lcov.info".to_owned()
            ]
        );
        fs::create_dir_all(&dir)?;
        fs::write(
            dir.join("lcov.info"),
            "TN:\nSF:src/lib.rs\nFNF:2\nFNH:1\nLF:4\nLH:3\nend_of_record\n",
        )?;
        super::write_sensor_status(
            &out,
            sensor,
            SensorStatusWrite {
                status: "ok",
                argv: &argv,
                duration_ms: 0,
                reason: "completed",
                exit_code: Some(0),
                timed_out: false,
            },
        )?;
        let status: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("status.json"))?)?;
        let summary: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("coverage-summary.json"))?)?;
        let changed_lines: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("changed-lines.json"))?)?;
        let upload: serde_json::Value =
            serde_json::from_slice(&fs::read(dir.join("upload.json"))?)?;
        assert_eq!(status["schema"], "ub-review.coverage_status.v1");
        assert_eq!(status["status"], "ok");
        assert_eq!(status["execution_surface_only"], serde_json::json!(true));
        assert_eq!(status["correctness_claim"], serde_json::json!(false));
        assert_eq!(status["lcov"]["path"], "sensors/coverage/lcov.info");
        assert_eq!(status["lcov"]["present"], serde_json::json!(true));
        assert_eq!(
            status["summary"]["path"],
            "sensors/coverage/coverage-summary.json"
        );
        assert_eq!(summary["schema"], "ub-review.coverage_summary.v1");
        assert_eq!(summary["status"], "collected");
        assert_eq!(summary["line_totals"]["found"], 4);
        assert_eq!(summary["line_totals"]["hit"], 3);
        assert_eq!(summary["function_totals"]["found"], 2);
        assert_eq!(summary["function_totals"]["hit"], 1);
        assert_eq!(
            status["changed_lines"]["path"],
            "sensors/coverage/changed-lines.json"
        );
        assert_eq!(
            changed_lines["schema"],
            "ub-review.coverage_changed_lines.v1"
        );
        assert_eq!(changed_lines["status"], "not_collected");
        assert_eq!(changed_lines["correctness_claim"], serde_json::json!(false));
        assert_eq!(status["upload"]["path"], "sensors/coverage/upload.json");
        assert_eq!(upload["schema"], "ub-review.coverage_upload.v1");
        assert_eq!(status["upload"]["status"], "workflow_owned");
        assert_eq!(upload["correctness_claim"], serde_json::json!(false));
        Ok(())
    }

    #[test]
    fn missing_tool_status_is_recorded_as_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let out = root.join("out");
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let sensor = sensor_plan("ripr", "ub-review-test-tool-that-does-not-exist", true);
        let plan = test_plan(vec![sensor.clone()]);

        run_sensor(root, &out, &sensor, &event_log, &plan)?;

        let status_path = out.join("sensors/ripr/ub-review-sensor-status.json");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(status_path)?)?;
        assert_eq!(value["sensor"], "ripr");
        assert_eq!(value["status"], "missing");
        assert_eq!(value["reason"], "command not found");
        assert!(out.join("sensors/ripr/stdout.txt").exists());
        assert!(out.join("sensors/ripr/stderr.txt").exists());
        Ok(())
    }

    #[test]
    fn sensor_receipt_defaults_missing_exit_fields() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let status_path = temp
            .path()
            .join("sensors/ripr/ub-review-sensor-status.json");
        let parent = status_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("status path missing parent"))?;
        fs::create_dir_all(parent)?;
        fs::write(
            &status_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "sensor": "ripr",
                "status": "missing",
                "reason": "command not found"
            }))?,
        )?;

        let receipt = super::read_sensor_receipt(&status_path)
            .ok_or_else(|| anyhow::anyhow!("sensor receipt missing"))?;

        assert_eq!(receipt.status, "missing");
        assert_eq!(receipt.reason, "command not found");
        assert_eq!(receipt.exit_code, None);
        assert!(!receipt.timed_out);
        Ok(())
    }

    #[test]
    fn sensor_timeout_status_is_returned() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let stdout_path = temp.path().join("stdout.txt");
        let stderr_path = temp.path().join("stderr.txt");
        let argv = sleeper_argv();

        let status = run_command_to_files(
            temp.path(),
            &argv,
            &BTreeMap::new(),
            1,
            &stdout_path,
            &stderr_path,
        )?;

        assert!(status.timed_out);
        assert!(!status.success);
        assert_eq!(status.exit_code, None);
        assert!(status.duration_ms >= 1);
        Ok(())
    }

    #[test]
    fn unsafe_review_sensor_argv_uses_first_pr_out_dir_flag() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let out = root.join("out");
        let plan = test_plan(vec![sensor_plan("unsafe-review", "unsafe-review", true)]);
        let sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "unsafe-review")
            .ok_or_else(|| anyhow::anyhow!("unsafe-review sensor missing"))?;
        let dir = out.join("sensors/unsafe-review");
        let argv = super::build_sensor_argv(root, &dir, sensor, &plan);
        // The full argv is pinned: `first-pr` accepts `--out-dir`, and 0.3.4
        // SILENTLY ignores unknown flags (EffortlessMetrics/unsafe-review#531),
        // so a drifted flag here is an ok-status sensor with an absent gate
        // artifact, not a red sensor. Exact-match keeps the contract loud.
        assert_eq!(
            argv,
            vec![
                "unsafe-review".to_owned(),
                "first-pr".to_owned(),
                "--root".to_owned(),
                root.display().to_string(),
                "--base".to_owned(),
                plan.base.clone(),
                "--out-dir".to_owned(),
                dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR)
                    .display()
                    .to_string(),
            ]
        );
        // Regression guard for the exact production failure: `--out` is the
        // `check`/`repo` flag; on `first-pr` it routed the bundle to
        // `target/unsafe-review` inside the checkout (run 27102118267).
        assert!(
            !argv.iter().any(|arg| arg == "--out"),
            "--out must never reappear on the first-pr invocation: {argv:?}"
        );
        Ok(())
    }

    #[test]
    fn unsafe_review_sensor_outputs_includes_gate_and_artifact_files() -> Result<()> {
        let plan = test_plan(vec![sensor_plan("unsafe-review", "unsafe-review", true)]);
        let sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "unsafe-review")
            .ok_or_else(|| anyhow::anyhow!("unsafe-review sensor missing"))?;
        let outputs = super::sensor_outputs(sensor);
        let sub = super::UNSAFE_REVIEW_OUTPUT_SUBDIR;
        assert!(
            outputs
                .iter()
                .any(|o| o == &format!("{sub}/unsafe-review-gate.json")),
            "gate file missing from sensor_outputs"
        );
        assert!(
            outputs
                .iter()
                .any(|o| o == &format!("{sub}/comment-plan.json")),
            "comment-plan missing from sensor_outputs"
        );
        assert!(
            outputs
                .iter()
                .any(|o| o == &format!("{sub}/repair-queue.json")),
            "repair-queue missing from sensor_outputs"
        );
        // receipt-audit is Markdown on the real manifest, not JSON.
        assert!(
            outputs
                .iter()
                .any(|o| o == &format!("{sub}/receipt-audit.md")),
            "receipt-audit.md missing from sensor_outputs"
        );
        assert!(
            outputs.iter().any(|o| o == &format!("{sub}/cards.json")),
            "cards.json missing from sensor_outputs"
        );
        assert!(
            outputs.iter().any(|o| o == &format!("{sub}/cards.sarif")),
            "cards.sarif missing from sensor_outputs"
        );
        // Standard receipts must still be present
        assert!(outputs.contains(&"stdout.txt".to_owned()));
        assert!(outputs.contains(&"stderr.txt".to_owned()));
        Ok(())
    }
}
