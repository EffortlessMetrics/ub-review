//! Trigger matching and lane packet construction (cleanup train step
//! 33, pure code motion).

use crate::diff_posture::review_posture_for_diff_class;
use crate::*;
use anyhow::Context;
use std::io;

pub(crate) fn trigger_match(trigger: Trigger, flags: &DiffFlags) -> Option<String> {
    match trigger {
        Trigger::Always => Some("always-on base packet".to_owned()),
        Trigger::SourceChanged if flags.source_changed => Some("source file changed".to_owned()),
        Trigger::SourceExceptionChanged
            if flags.source_changed
                || flags.workflow_changed
                || flags.shell_changed
                || flags.dependency_changed =>
        {
            Some("source-tree exception surface changed".to_owned())
        }
        Trigger::RustBehaviorOrTestsChanged if flags.rust_changed || flags.rust_tests_changed => {
            Some("Rust behavior or tests changed".to_owned())
        }
        Trigger::UnsafeOrNativeRiskChanged if flags.unsafe_or_native_risk => {
            Some("unsafe/native-risk pattern detected".to_owned())
        }
        Trigger::WorkflowChanged if flags.workflow_changed => {
            Some("workflow/action file changed".to_owned())
        }
        Trigger::DependencyChanged if flags.dependency_changed => {
            Some("dependency manifest or lockfile changed".to_owned())
        }
        Trigger::ShellChanged if flags.shell_changed => {
            Some("shell/script file changed".to_owned())
        }
        Trigger::CppChanged if flags.cpp_changed => Some("C/C++ file changed".to_owned()),
        Trigger::Diff => Some("diff-scoped advisory scan".to_owned()),
        Trigger::Manual | Trigger::Never => None,
        _ => None,
    }
}

pub(crate) fn guard_ok(profile: &Profile, box_state: &BoxState, notes: &mut Vec<String>) -> bool {
    let mut ok = true;
    if let Some(mem) = box_state.free_mem_mb
        && mem < profile.guards.min_free_mem_mb
    {
        ok = false;
        notes.push(format!(
            "free memory {mem}MiB is below profile floor {}MiB",
            profile.guards.min_free_mem_mb
        ));
    }
    if let Some(disk) = box_state.free_disk_mb
        && disk < profile.guards.min_free_disk_mb
    {
        ok = false;
        notes.push(format!(
            "free disk {disk}MiB is below profile floor {}MiB",
            profile.guards.min_free_disk_mb
        ));
    }
    if let Some(load) = box_state.load_1m
        && load > profile.guards.max_load_1m
    {
        ok = false;
        notes.push(format!(
            "load average {load:.2} exceeds profile ceiling {:.2}",
            profile.guards.max_load_1m
        ));
    }
    ok
}

pub(crate) fn sensor_order(id: &str) -> u8 {
    match id {
        "tokmd" => 0,
        "cargo-allow" => 1,
        "ripr" => 2,
        "unsafe-review" => 3,
        "ast-grep" => 4,
        "semgrep" => 5,
        "actionlint" => 6,
        "zizmor" => 7,
        "gitleaks" => 8,
        "osv-scanner" => 9,
        "cargo-audit" => 10,
        "cargo-deny" => 11,
        "coverage" => 12,
        _ => 50,
    }
}

pub(crate) fn write_skipped_sensor_receipts(
    root: &Path,
    out: &Path,
    plan: &Plan,
    event_log: &EventLog,
) -> Result<()> {
    for sensor in plan.sensors.iter().filter(|sensor| !sensor.run) {
        let dir = out.join("sensors").join(&sensor.id);
        let argv = build_sensor_argv(root, &dir, sensor, plan);
        write_sensor_status(
            out,
            sensor,
            SensorStatusWrite {
                status: "skipped",
                argv: &argv,
                duration_ms: 0,
                reason: &sensor.reason,
                exit_code: None,
                timed_out: false,
            },
        )?;
        event_log.append(
            "sensor_skipped",
            serde_json::json!({"sensor": sensor.id, "reason": sensor.reason}),
        )?;
    }
    Ok(())
}

pub(crate) fn write_dry_run_sensor_receipts(
    root: &Path,
    out: &Path,
    plan: &Plan,
    event_log: &EventLog,
) -> Result<()> {
    for sensor in &plan.sensors {
        let dir = out.join("sensors").join(&sensor.id);
        let argv = build_sensor_argv(root, &dir, sensor, plan);
        let reason = if sensor.run {
            "dry-run; sensor not executed"
        } else {
            &sensor.reason
        };
        write_sensor_status(
            out,
            sensor,
            SensorStatusWrite {
                status: "skipped",
                argv: &argv,
                duration_ms: 0,
                reason,
                exit_code: None,
                timed_out: false,
            },
        )?;
        event_log.append(
            "sensor_skipped",
            serde_json::json!({"sensor": sensor.id, "reason": reason}),
        )?;
    }
    Ok(())
}

pub(crate) fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

pub(crate) fn changed_paths_for_tokmd_context(root: &Path, plan: &Plan) -> Vec<String> {
    git_lines(
        root,
        &[
            "diff",
            "--name-only",
            &format!("{}...{}", plan.base, plan.head),
        ],
    )
    .or_else(|_| git_lines(root, &["diff", "--name-only", &plan.base, &plan.head]))
    .unwrap_or_default()
    .into_iter()
    .filter(|path| root.join(path).is_file())
    .take(40)
    .collect()
}

pub(crate) fn commands_json(commands: &[SensorSubcommand]) -> serde_json::Value {
    serde_json::Value::Array(
        commands
            .iter()
            .map(|command| {
                serde_json::json!({
                    "label": command.label,
                    "command": display_command(&command.argv),
                    "stdout": command.stdout_path.display().to_string(),
                    "stderr": command.stderr_path.display().to_string(),
                })
            })
            .collect(),
    )
}

pub(crate) fn append_file(path: &Path, text: &str) -> Result<()> {
    use std::io::Write as _;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

pub(crate) fn append_existing_file(target: &Path, source: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(source).unwrap_or_else(|_| String::new());
    if text.is_empty() {
        return Ok(());
    }
    append_file(target, &text)
}

pub(crate) fn run_command_to_files(
    root: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
    timeout_sec: u64,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<CommandStatus> {
    let ambient = BTreeMap::new();
    let mut ignore_process = |_| {};
    run_command_to_files_inner(CommandRun {
        root,
        argv,
        env,
        ambient: &ambient,
        timeout_sec,
        stdout_path,
        stderr_path,
        quarantine_environment: false,
        observe_process: &mut ignore_process,
    })
}

/// Run a sensor subprocess with the deny-by-default environment contract.
pub(crate) fn run_sensor_command_to_files(
    root: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
    timeout_sec: u64,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<CommandStatus> {
    let mut ignore_process = |_| {};
    run_sensor_command_to_files_with_spawn_observer(
        root,
        argv,
        env,
        timeout_sec,
        stdout_path,
        stderr_path,
        &mut ignore_process,
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CommandProcessObservation {
    Spawned,
    CompletionUnconfirmed,
}

/// Run a sensor subprocess and report process creation or an unconfirmed
/// cleanup. The callback is observational and cannot suppress the child.
pub(crate) fn run_sensor_command_to_files_with_spawn_observer(
    root: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
    timeout_sec: u64,
    stdout_path: &Path,
    stderr_path: &Path,
    observe_process: &mut dyn FnMut(CommandProcessObservation),
) -> Result<CommandStatus> {
    let ambient = std::env::vars().collect();
    run_command_to_files_inner(CommandRun {
        root,
        argv,
        env,
        ambient: &ambient,
        timeout_sec,
        stdout_path,
        stderr_path,
        quarantine_environment: true,
        observe_process,
    })
}

struct CommandRun<'a> {
    root: &'a Path,
    argv: &'a [String],
    env: &'a BTreeMap<String, String>,
    ambient: &'a BTreeMap<String, String>,
    timeout_sec: u64,
    stdout_path: &'a Path,
    stderr_path: &'a Path,
    quarantine_environment: bool,
    observe_process: &'a mut dyn FnMut(CommandProcessObservation),
}

fn run_command_to_files_inner(run: CommandRun<'_>) -> Result<CommandStatus> {
    let CommandRun {
        root,
        argv,
        env,
        ambient,
        timeout_sec,
        stdout_path,
        stderr_path,
        quarantine_environment,
        observe_process,
    } = run;
    let Some((program, args)) = argv.split_first() else {
        bail!("empty command");
    };
    let stdout =
        File::create(stdout_path).with_context(|| format!("create {}", stdout_path.display()))?;
    let stderr =
        File::create(stderr_path).with_context(|| format!("create {}", stderr_path.display()))?;
    let started = Instant::now();
    let mut command = ProcessCommand::new(program);
    command
        .args(args)
        .current_dir(root)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    if quarantine_environment {
        // Sensors are advisory child processes. They must never inherit the
        // action's provider, GitHub, OIDC, or unrelated runner environment.
        command
            .env_clear()
            .envs(sensor_child_environment_from(ambient, env));
    } else {
        command.envs(env);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("spawn {program}"))?;
    observe_process(CommandProcessObservation::Spawned);
    let status = match child.wait_timeout(Duration::from_secs(timeout_sec)) {
        Ok(Some(status)) => status,
        Ok(None) => {
            terminate_and_reap_child(&mut child, observe_process)?;
            return Ok(CommandStatus {
                exit_code: None,
                timed_out: true,
                success: false,
                reason: format!("timed out after {timeout_sec}s"),
                duration_ms: started.elapsed().as_millis(),
            });
        }
        Err(wait_error) => {
            terminate_and_reap_child(&mut child, observe_process)
                .context("terminate child after wait-timeout failure")?;
            return Err(wait_error).context("wait for child process");
        }
    };
    Ok(CommandStatus {
        exit_code: status.code(),
        timed_out: false,
        success: status.success(),
        reason: if status.success() {
            "completed".to_owned()
        } else {
            format!("exit code {:?}", status.code())
        },
        duration_ms: started.elapsed().as_millis(),
    })
}

trait TerminateAndReap {
    fn kill_process(&mut self) -> io::Result<()>;
    fn reap_process(&mut self) -> io::Result<()>;
}

impl TerminateAndReap for std::process::Child {
    fn kill_process(&mut self) -> io::Result<()> {
        self.kill()
    }

    fn reap_process(&mut self) -> io::Result<()> {
        self.wait().map(|_| ())
    }
}

fn terminate_and_reap_child(
    child: &mut impl TerminateAndReap,
    observe_process: &mut dyn FnMut(CommandProcessObservation),
) -> Result<()> {
    let kill_error = child.kill_process().err();
    if let Err(wait_error) = child.reap_process() {
        observe_process(CommandProcessObservation::CompletionUnconfirmed);
        let kill_detail = kill_error.map_or_else(
            || "kill request succeeded".to_owned(),
            |error| format!("kill request failed: {error}"),
        );
        bail!("child completion remains unconfirmed ({kill_detail}; wait failed: {wait_error})");
    }
    Ok(())
}

/// Keep only the non-secret execution metadata needed by sensor tools.
fn sensor_child_environment_from(
    ambient: &BTreeMap<String, String>,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    const ALLOWED: [&str; 10] = [
        "PATH",
        "TEMP",
        "TMP",
        "TMPDIR",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "LANG",
        "LC_ALL",
    ];
    let is_allowed = |key: &str| {
        ALLOWED
            .iter()
            .any(|allowed| key.eq_ignore_ascii_case(allowed))
    };
    let mut result = ambient
        .iter()
        .filter(|(key, _)| is_allowed(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    for (key, value) in overrides {
        if is_allowed(key) {
            result.insert(key.clone(), value.clone());
        }
    }
    result
}

pub(crate) fn parse_lcov_count(value: &str) -> u64 {
    value.trim().parse().unwrap_or_default()
}

pub(crate) fn write_lane_packets(
    out: &Path,
    diff: &DiffContext,
    plan: &Plan,
    lanes: &[LanePlan],
    pr_thread_context: &PrThreadContext,
    event_log: &EventLog,
) -> Result<()> {
    // #325: late-phase sensors run behind lane launch; their receipts are not
    // stable when packets are written, so packets render them as scheduled
    // work deterministically instead of reading a racing receipt.
    let late_sensor_ids = plan
        .sensors
        .iter()
        .filter(|sensor| sensor.run && matches!(sensor.phase, SensorPhase::Late))
        .map(|sensor| sensor.id.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let lane_dir = out.join("lanes");
    fs::create_dir_all(&lane_dir)
        .with_context(|| format!("create lane packet directory {}", lane_dir.display()))?;
    for lane in lanes {
        let mut text = String::new();
        text.push_str(&format!("# Lane: `{}`\n\n", lane.id));
        text.push_str(&format!("Model: `{}`\n\n", lane.model_display));
        text.push_str(&format!("Role: {}\n\n", lane.role));
        text.push_str("## Focus\n\n");
        text.push_str(&lane.focus);
        text.push_str("\n\n## Shared diff\n\n");
        text.push_str(&format!(
            "Base: `{}`\n\nHead: `{}`\n\n",
            diff.base, diff.head
        ));
        text.push_str("Changed files:\n\n");
        for file in &diff.changed_files {
            text.push_str(&format!("- `{file}`\n"));
        }
        text.push_str("\n## Routed sensor evidence\n\n");
        for sensor_id in &lane.receives {
            if late_sensor_ids.contains(sensor_id.as_str()) {
                text.push_str(&format!(
                    "- `{sensor_id}`: `scheduled-late` (runs during the model wave; late is not missing — request it via the reporter follow-up if load-bearing)\n"
                ));
                continue;
            }
            let sensor_dir = out.join("sensors").join(sensor_id);
            let status_path = sensor_dir.join("ub-review-sensor-status.json");
            let status = read_sensor_receipt(&status_path)
                .map(|receipt| receipt.status)
                .unwrap_or_else(|| "receipt-absent".to_owned());
            text.push_str(&format!("- `{sensor_id}`: `{status}`\n"));
            if sensor_id == "unsafe-review" {
                text.push_str(&render_unsafe_review_lane_evidence(&sensor_dir, &status));
            }
        }
        text.push_str("\n## Seeded PR Thread Context\n\n");
        text.push_str(
            "This lane receives the cached shared context prefix from \
             `review/shared_context.md`; the PR-thread seed below is the same \
             bounded context recorded in `review/pr_thread_context.json`.\n\n",
        );
        text.push_str(&render_pr_thread_context(pr_thread_context));
        text.push_str("\n## Review posture\n\n");
        text.push_str(review_posture_for_diff_class(diff.diff_class));
        text.push_str("\n\n## Required output shape\n\n");
        text.push_str(&format!(
            "Start inline comments for this lane with `[{}]`. If no blocking finding exists, write an audit trail: what you checked, strongest failed objection, and residual risk. Do not infer safety from missing sensor receipts.\n",
            lane.id
        ));
        let packet_path = lane_dir.join(format!("{}.md", sanitize_artifact_name(&lane.id)));
        fs::write(&packet_path, text)
            .with_context(|| format!("write lane packet {}", packet_path.display()))?;
        event_log.append("lane_packet_written", serde_json::json!({"lane": lane.id}))?;
    }
    Ok(())
}

/// Build the structured evidence block for an unsafe-review sensor entry in a
/// lane packet.
///
/// When `unsafe-review-gate.json` is present and its schema_version matches
/// `UNSAFE_REVIEW_GATE_SCHEMA`, returns a Markdown block with the movement
/// summary and comment-plan entries (capped at 3, matching unsafe-review's own
/// bounded output). When the file is absent, the sensor failed, or the schema
/// is unrecognised, returns a note explaining the degradation so model lanes
/// never infer safety from silence.
///
/// Trust boundary is always surfaced: unsafe-review output is advisory only.
/// The deterministic floor (required-sensor gap logic) still gates; this
/// structured block is supplementary review context.
pub(crate) fn render_unsafe_review_lane_evidence(sensor_dir: &Path, status: &str) -> String {
    if status != "ok" {
        // Sensor did not succeed: no artifacts to read; nothing to add beyond
        // the status line already written by the caller.
        return String::new();
    }
    match read_unsafe_review_artifacts(sensor_dir) {
        Err(gap) => {
            format!("  - unsafe-review structured evidence: {}\n", gap.reason())
        }
        Ok(artifacts) => {
            let gate = &artifacts.gate;
            let trust = gate.trust_boundary.as_deref().unwrap_or("advisory");
            let summary = &gate.summary;
            let mut block = String::new();
            block.push_str(&format!(
                "  - unsafe-review movement (trust_boundary: `{trust}`, advisory only): \
                 new_gaps={}, worsened={}, resolved={}, inherited={}\n",
                summary.new_gaps,
                summary.worsened_gaps,
                summary.resolved_gaps,
                summary.inherited_gaps
            ));
            if artifacts.comment_plan.is_empty() {
                block.push_str(
                    "  - unsafe-review comment-plan: absent or empty \
                     (no comment candidates for this diff)\n",
                );
            } else {
                block.push_str(&format!(
                    "  - unsafe-review comment-plan ({} candidate(s), advisory):\n",
                    artifacts.comment_plan.len()
                ));
                for entry in &artifacts.comment_plan {
                    let path = entry.path.as_deref().unwrap_or("(unknown path)");
                    let line = entry
                        .line
                        .map(|l| l.to_string())
                        .unwrap_or_else(|| "?".to_owned());
                    let gap = entry
                        .coverage_gap
                        .as_deref()
                        .unwrap_or("(no gap description)");
                    block.push_str(&format!("    - `{path}:{line}` — {gap}\n"));
                }
            }
            block
        }
    }
}

pub(crate) fn render_pr_packet(diff: &DiffContext) -> String {
    let mut text = String::new();
    text.push_str("# PR evidence packet\n\n");
    text.push_str(&format!(
        "Base: `{}`\n\nHead: `{}`\n\n",
        diff.base, diff.head
    ));
    text.push_str("## Changed files\n\n");
    for file in &diff.changed_files {
        text.push_str(&format!("- `{file}`\n"));
    }
    text.push_str("\n## Diff flags\n\n");
    text.push_str(&format!(
        "```json\n{}\n```\n",
        serde_json::to_string_pretty(&diff.flags).unwrap_or_else(|_| "{}".to_owned())
    ));
    text
}

pub(crate) fn render_claim_prompt(diff: &DiffContext) -> String {
    let mut text = String::new();
    text.push_str("# PR claims extraction prompt\n\n");
    text.push_str("No PR body is available in no-token GH runner mode. Treat claims as absent unless supplied by a separate artifact.\n\n");
    text.push_str(
        "Reviewers should verify only claims grounded in the diff and available artifacts.\n\n",
    );
    text.push_str("Changed files:\n\n");
    for file in &diff.changed_files {
        text.push_str(&format!("- `{file}`\n"));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::{
        CommandProcessObservation, TerminateAndReap, run_sensor_command_to_files,
        terminate_and_reap_child,
    };
    use anyhow::{Result, ensure};
    use std::{collections::BTreeMap, fs, io};

    struct FakeChild {
        kill_fails: bool,
        reap_fails: bool,
        calls: Vec<&'static str>,
    }

    impl TerminateAndReap for FakeChild {
        fn kill_process(&mut self) -> io::Result<()> {
            self.calls.push("kill");
            if self.kill_fails {
                Err(io::Error::other("injected kill failure"))
            } else {
                Ok(())
            }
        }

        fn reap_process(&mut self) -> io::Result<()> {
            self.calls.push("wait");
            if self.reap_fails {
                Err(io::Error::other("injected wait failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn child_cleanup_waits_for_confirmed_exit_even_when_kill_reports_an_error() -> Result<()> {
        let mut child = FakeChild {
            kill_fails: true,
            reap_fails: false,
            calls: Vec::new(),
        };
        let mut observations = Vec::new();

        terminate_and_reap_child(&mut child, &mut |event| observations.push(event))?;

        ensure!(child.calls == ["kill", "wait"]);
        ensure!(observations.is_empty());
        Ok(())
    }

    #[test]
    fn child_cleanup_marks_release_unconfirmed_when_reaping_fails() -> Result<()> {
        let mut child = FakeChild {
            kill_fails: false,
            reap_fails: true,
            calls: Vec::new(),
        };
        let mut observations = Vec::new();

        let result = terminate_and_reap_child(&mut child, &mut |event| observations.push(event));

        ensure!(result.is_err());
        ensure!(child.calls == ["kill", "wait"]);
        ensure!(observations == [CommandProcessObservation::CompletionUnconfirmed]);
        Ok(())
    }

    #[test]
    fn sensor_environment_excludes_credentials_and_keeps_execution_metadata() -> Result<()> {
        let exe = std::env::current_exe()?;
        let status = std::process::Command::new(exe)
            .arg("--exact")
            .arg("lane_packets::tests::fake_sensor_cannot_read_actual_parent_credentials")
            .arg("--nocapture")
            .env("UB_REVIEW_ENV_PROBE_CHILD", "1")
            .env("UB_REVIEW_MINIMAX_API_KEY", "sentinel-minimax")
            .env("UB_REVIEW_GITHUB_TOKEN", "sentinel-github")
            .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "sentinel-oidc")
            .env("ACTIONS_RUNTIME_TOKEN", "sentinel-runner")
            .env("PATH", "sentinel-path")
            .status()?;
        ensure!(
            status.success(),
            "actual-parent probe process failed: {status}"
        );
        Ok(())
    }

    #[test]
    fn fake_sensor_cannot_read_actual_parent_credentials() -> Result<()> {
        if std::env::var_os("UB_REVIEW_ENV_PROBE_CHILD").is_none() {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let stdout = temp.path().join("stdout.txt");
        let stderr = temp.path().join("stderr.txt");
        #[cfg(unix)]
        let argv = vec![
            "/bin/sh".to_owned(),
            "-c".to_owned(),
            "printf '%s|%s|%s|%s|%s' \"${PATH:-missing}\" \"${UB_REVIEW_MINIMAX_API_KEY:-missing}\" \"${UB_REVIEW_GITHUB_TOKEN:-missing}\" \"${ACTIONS_ID_TOKEN_REQUEST_TOKEN:-missing}\" \"${ACTIONS_RUNTIME_TOKEN:-missing}\"".to_owned(),
        ];
        #[cfg(windows)]
        let argv = vec![
            std::env::var("COMSPEC")?,
            "/C".to_owned(),
            "echo %PATH% & if defined UB_REVIEW_MINIMAX_API_KEY (echo %UB_REVIEW_MINIMAX_API_KEY%) else (echo missing) & if defined UB_REVIEW_GITHUB_TOKEN (echo %UB_REVIEW_GITHUB_TOKEN%) else (echo missing) & if defined ACTIONS_ID_TOKEN_REQUEST_TOKEN (echo %ACTIONS_ID_TOKEN_REQUEST_TOKEN%) else (echo missing) & if defined ACTIONS_RUNTIME_TOKEN (echo %ACTIONS_RUNTIME_TOKEN%) else (echo missing)".to_owned(),
        ];
        let status =
            run_sensor_command_to_files(temp.path(), &argv, &BTreeMap::new(), 5, &stdout, &stderr)?;
        ensure!(status.success, "fake sensor failed: {}", status.reason);
        let observed = fs::read_to_string(stdout)?;
        ensure!(
            observed.contains("sentinel-path"),
            "fake sensor did not observe the allowlisted PATH: {observed:?}"
        );
        ensure!(!observed.contains("sentinel-minimax"));
        ensure!(!observed.contains("sentinel-github"));
        ensure!(!observed.contains("sentinel-oidc"));
        ensure!(!observed.contains("sentinel-runner"));
        Ok(())
    }
}
