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

/// Run an approved proof/worker subprocess under the proof environment
/// contract (#1062) while exposing the same spawn and completion authority
/// used by sensors.
///
/// Proof and worker children execute candidate-controlled code, so they must
/// never inherit the action's provider, GitHub, OIDC, or unrelated runner
/// environment: the child sees only the allowlisted OS and toolchain entries
/// (see [`proof_child_environment_from`]) plus the command's explicitly
/// approved environment. Candidate grandchild processes inherit this same
/// contracted environment, closing the process tree against sentinel
/// credentials.
pub(crate) fn run_command_to_files_with_spawn_observer(
    root: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
    timeout_sec: u64,
    stdout_path: &Path,
    stderr_path: &Path,
    observe_process: &mut dyn FnMut(CommandProcessObservation),
) -> Result<CommandStatus> {
    let ambient = ambient_environment_snapshot();
    run_command_to_files_inner(CommandRun {
        root,
        argv,
        env,
        ambient: &ambient,
        timeout_sec,
        stdout_path,
        stderr_path,
        child_environment: ChildEnvironment::ProofWorkerAllowlist,
        observe_process,
    })
}

/// Snapshot the ambient parent environment for a quarantined spawn. Keys that
/// are not valid Unicode are dropped, mirroring the sensor contract: they are
/// inert rather than fatal.
fn ambient_environment_snapshot() -> BTreeMap<String, std::ffi::OsString> {
    std::env::vars_os()
        .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
        .collect()
}

/// Run a sensor subprocess with the deny-by-default environment contract.
#[cfg(test)]
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
    let ambient = ambient_environment_snapshot();
    run_command_to_files_inner(CommandRun {
        root,
        argv,
        env,
        ambient: &ambient,
        timeout_sec,
        stdout_path,
        stderr_path,
        child_environment: ChildEnvironment::SensorAllowlist,
        observe_process,
    })
}

/// Spawn-time environment policy for one child process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ChildEnvironment {
    /// Test-only raw seam: inherit the ambient parent environment plus the
    /// explicit command environment. Production callers must never use this
    /// policy for candidate-executing children.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the ambient-inheritance seam is test-only by construction; the expectation fails if production code ever constructs it"
        )
    )]
    AmbientPlusExplicit,
    /// Sensor contract (#879): deny-by-default OS allowlist.
    SensorAllowlist,
    /// Proof/worker contract (#1062): OS allowlist plus explicit toolchain
    /// homes; overrides are restricted to repo-template entries.
    ProofWorkerAllowlist,
}

struct CommandRun<'a> {
    root: &'a Path,
    argv: &'a [String],
    env: &'a BTreeMap<String, String>,
    ambient: &'a BTreeMap<String, std::ffi::OsString>,
    timeout_sec: u64,
    stdout_path: &'a Path,
    stderr_path: &'a Path,
    child_environment: ChildEnvironment,
    observe_process: &'a mut dyn FnMut(CommandProcessObservation),
}

fn run_command_to_files_inner(run: CommandRun<'_>) -> Result<CommandStatus> {
    run_command_to_files_inner_with_wait(run, &mut |child, timeout| child.wait_timeout(timeout))
}

fn run_command_to_files_inner_with_wait(
    run: CommandRun<'_>,
    wait_for_child: &mut dyn FnMut(
        &mut std::process::Child,
        Duration,
    ) -> io::Result<Option<std::process::ExitStatus>>,
) -> Result<CommandStatus> {
    let CommandRun {
        root,
        argv,
        env,
        ambient,
        timeout_sec,
        stdout_path,
        stderr_path,
        child_environment,
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
    match child_environment {
        ChildEnvironment::AmbientPlusExplicit => {
            command.envs(env);
        }
        ChildEnvironment::SensorAllowlist => {
            // Sensors are advisory child processes. They must never inherit the
            // action's provider, GitHub, OIDC, or unrelated runner environment.
            command
                .env_clear()
                .envs(sensor_child_environment_from(ambient, env));
        }
        ChildEnvironment::ProofWorkerAllowlist => {
            // Proof and worker children execute candidate-controlled code, so
            // the same deny-by-default rule applies with the toolchain homes a
            // cargo child demonstrably needs (see the allowlist tables above
            // [`CHILD_ENVIRONMENT_OS_ENTRIES`]).
            command
                .env_clear()
                .envs(proof_child_environment_from(ambient, env));
        }
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("spawn {program}"))?;
    observe_process(CommandProcessObservation::Spawned);
    let status = match wait_for_child(&mut child, Duration::from_secs(timeout_sec)) {
        Ok(Some(status)) => status,
        Ok(None) => {
            return reconcile_command_timeout(&mut child, observe_process, timeout_sec, started);
        }
        Err(wait_error) => {
            return reconcile_wait_failure(&mut child, observe_process, wait_error);
        }
    };
    Ok(CommandStatus {
        exit_code: status.code(),
        timed_out: false,
        success: status.success(),
        reason: if status.success() {
            "completed".to_owned()
        } else if status.code().is_none() {
            format!("process terminated without an exit code: {status}")
        } else {
            format!("exit code {:?}", status.code())
        },
        duration_ms: started.elapsed().as_millis(),
    })
}

fn reconcile_command_timeout(
    child: &mut impl TerminateAndReap,
    observe_process: &mut dyn FnMut(CommandProcessObservation),
    timeout_sec: u64,
    started: Instant,
) -> Result<CommandStatus> {
    let reason = format!("timed out after {timeout_sec}s");
    let kill_error =
        terminate_and_reap_child(child, observe_process).with_context(|| reason.clone())?;
    Ok(CommandStatus {
        exit_code: None,
        timed_out: true,
        success: false,
        reason: match kill_error {
            Some(error) => format!("{reason}; kill request failed: {error}; child reap confirmed"),
            None => reason,
        },
        duration_ms: started.elapsed().as_millis(),
    })
}

fn reconcile_wait_failure(
    child: &mut impl TerminateAndReap,
    observe_process: &mut dyn FnMut(CommandProcessObservation),
    wait_error: io::Error,
) -> Result<CommandStatus> {
    match terminate_and_reap_child(child, observe_process) {
        Ok(None) => Err(wait_error).context("wait for child process"),
        Ok(Some(kill_error)) => Err(wait_error).context(format!(
            "wait for child process; kill request failed: {kill_error}; child reap confirmed"
        )),
        Err(cleanup_error) => Err(cleanup_error).context(format!(
            "terminate child after wait-timeout failure; original wait error: {wait_error}"
        )),
    }
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

// A confirmed reap permits physical cleanup even when the kill request failed.
// Return that failure separately so natural exit cannot erase its diagnostic.
fn terminate_and_reap_child(
    child: &mut impl TerminateAndReap,
    observe_process: &mut dyn FnMut(CommandProcessObservation),
) -> Result<Option<io::Error>> {
    const MAX_INTERRUPTED_REAP_ATTEMPTS: usize = 16;
    let kill_error = child.kill_process().err();
    let mut interrupted_attempts = 0_usize;
    while interrupted_attempts < MAX_INTERRUPTED_REAP_ATTEMPTS {
        match child.reap_process() {
            Ok(()) => return Ok(kill_error),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {
                interrupted_attempts += 1;
            }
            Err(wait_error) => {
                observe_process(CommandProcessObservation::CompletionUnconfirmed);
                let kill_detail = kill_error.map_or_else(
                    || "kill request succeeded".to_owned(),
                    |error| format!("kill request failed: {error}"),
                );
                bail!(
                    "child completion remains unconfirmed ({kill_detail}; wait failed: {wait_error})"
                );
            }
        }
    }
    observe_process(CommandProcessObservation::CompletionUnconfirmed);
    let kill_detail = kill_error.map_or_else(
        || "kill request succeeded".to_owned(),
        |error| format!("kill request failed: {error}"),
    );
    bail!(
        "child completion remains unconfirmed ({kill_detail}; reap interrupted {MAX_INTERRUPTED_REAP_ATTEMPTS} consecutive times)"
    )
}

/// Allowlisted child environment entries and their purpose.
///
/// Every quarantined child (sensor, proof, worker) inherits only these
/// non-secret OS entries from the ambient parent environment; every other
/// key, including provider, GitHub, OIDC, and cloud credentials, agent
/// config, credential helpers, and the home directory itself, is dropped at
/// spawn. Matching is ASCII case-insensitive so Windows-spelled keys resolve.
///
/// | Entry | Purpose |
/// |---|---|
/// | `PATH` | Resolve the child program and the tools it invokes (cargo, rustc and `+toolchain` proxies, cc/linkers, git, sensor binaries, platform shells). |
/// | `TEMP`, `TMP` | Windows per-process temporary directories used by toolchains and test fixtures. |
/// | `TMPDIR` | Unix temporary directory for the same purpose. |
/// | `SystemRoot`, `WINDIR` | Windows system directory required for system DLL loading and child process creation. |
/// | `COMSPEC` | Absolute cmd.exe location used by Windows shell fixtures. |
/// | `PATHEXT` | Windows executable-extension resolution for script fixtures. |
/// | `LANG`, `LC_ALL` | Locale for stable tool diagnostics. |
///
/// Proof and worker children additionally inherit the toolchain homes:
///
/// | Entry | Purpose |
/// |---|---|
/// | `CARGO_HOME` | Registry/source cache and cargo config for `--locked` proof builds; derived from the ambient home when the runner does not export it. |
/// | `RUSTUP_HOME` | rustup toolchain resolution for `cargo`/`cargo +nightly` proxies; derived from the ambient home when not exported. |
///
/// Proof and worker commands may also add these override entries, each of
/// which originates only from repo-approved command templates:
///
/// | Entry | Purpose |
/// |---|---|
/// | `RUSTFLAGS` | Sanitizer-witness template sets `-Zsanitizer=address`. |
/// | `USE_SYSTEM_BUN` | Base+tests bun variant selects the system bun. |
///
/// Candidate-supplied proof requests carry no environment (`ProofRequestV2`
/// has no env field) and candidate-provided command specs parse to an empty
/// env, and any override key outside the tables is dropped at spawn, so a
/// candidate cannot extend the child environment.
const CHILD_ENVIRONMENT_OS_ENTRIES: [&str; 10] = [
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

/// Toolchain homes a proof/worker cargo child needs on top of the OS entries.
const CHILD_ENVIRONMENT_TOOLCHAIN_ENTRIES: [&str; 2] = ["CARGO_HOME", "RUSTUP_HOME"];

/// Override entries a proof/worker command may add beyond the inherited
/// allowlist; see the table above for each entry's repo-template origin.
const CHILD_ENVIRONMENT_PROOF_OVERRIDE_ENTRIES: [&str; 2] = ["RUSTFLAGS", "USE_SYSTEM_BUN"];

/// Keep only the non-secret execution metadata needed by sensor tools.
fn sensor_child_environment_from(
    ambient: &BTreeMap<String, std::ffi::OsString>,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, std::ffi::OsString> {
    allowlisted_child_environment(
        ambient,
        overrides,
        &CHILD_ENVIRONMENT_OS_ENTRIES,
        &CHILD_ENVIRONMENT_OS_ENTRIES,
    )
}

/// Keep the OS entries, the explicit toolchain homes, and only the approved
/// template overrides a proof/worker cargo child needs. When the runner never
/// exported `CARGO_HOME`/`RUSTUP_HOME`, derive them from the ambient home
/// directory so the rustup proxies and `--locked` builds keep working without
/// inheriting that home directory itself.
fn proof_child_environment_from(
    ambient: &BTreeMap<String, std::ffi::OsString>,
    overrides: &BTreeMap<String, String>,
) -> BTreeMap<String, std::ffi::OsString> {
    let inherited = CHILD_ENVIRONMENT_OS_ENTRIES
        .iter()
        .chain(CHILD_ENVIRONMENT_TOOLCHAIN_ENTRIES.iter())
        .copied()
        .collect::<Vec<_>>();
    let override_allowed = CHILD_ENVIRONMENT_OS_ENTRIES
        .iter()
        .chain(CHILD_ENVIRONMENT_TOOLCHAIN_ENTRIES.iter())
        .chain(CHILD_ENVIRONMENT_PROOF_OVERRIDE_ENTRIES.iter())
        .copied()
        .collect::<Vec<_>>();
    let mut child = allowlisted_child_environment(
        ambient,
        overrides,
        inherited.as_slice(),
        override_allowed.as_slice(),
    );
    derive_toolchain_homes_from_ambient_home(ambient, &mut child);
    child
}

fn allowlisted_child_environment(
    ambient: &BTreeMap<String, std::ffi::OsString>,
    overrides: &BTreeMap<String, String>,
    inherited_entries: &[&str],
    override_entries: &[&str],
) -> BTreeMap<String, std::ffi::OsString> {
    let is_allowed = |key: &str, entries: &[&str]| {
        entries
            .iter()
            .any(|allowed| key.eq_ignore_ascii_case(allowed))
    };
    let mut result = ambient
        .iter()
        .filter(|(key, _)| is_allowed(key, inherited_entries))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<BTreeMap<_, _>>();
    for (key, value) in overrides {
        if is_allowed(key, override_entries) {
            result.insert(key.clone(), value.into());
        }
    }
    result
}

/// Forward the resolved toolchain homes into the child environment without
/// inheriting the ambient home directory entry itself: cargo and rustup
/// otherwise fall back to the (absent) child home and fail to locate the
/// installed toolchain. The derivation reads the ambient home path in the
/// trusted parent and exports only the resulting toolchain path.
fn derive_toolchain_homes_from_ambient_home(
    ambient: &BTreeMap<String, std::ffi::OsString>,
    child: &mut BTreeMap<String, std::ffi::OsString>,
) {
    let Some(home) = ambient_home_entry(ambient) else {
        return;
    };
    if !child_environment_has_entry(child, "CARGO_HOME") {
        child.insert(
            "CARGO_HOME".to_owned(),
            Path::new(home).join(".cargo").into_os_string(),
        );
    }
    if !child_environment_has_entry(child, "RUSTUP_HOME") {
        child.insert(
            "RUSTUP_HOME".to_owned(),
            Path::new(home).join(".rustup").into_os_string(),
        );
    }
}

fn child_environment_has_entry(child: &BTreeMap<String, std::ffi::OsString>, key: &str) -> bool {
    child
        .keys()
        .any(|existing| existing.eq_ignore_ascii_case(key))
}

/// The ambient home directory entry: `HOME` on Unix, `USERPROFILE` on Windows.
#[cfg(unix)]
fn ambient_home_entry(
    ambient: &BTreeMap<String, std::ffi::OsString>,
) -> Option<&std::ffi::OsString> {
    ambient.get("HOME")
}

#[cfg(windows)]
fn ambient_home_entry(
    ambient: &BTreeMap<String, std::ffi::OsString>,
) -> Option<&std::ffi::OsString> {
    ambient
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case("USERPROFILE"))
        .map(|(_, value)| value)
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
#[path = "lane_packets_lifecycle_tests.rs"]
mod lifecycle_receipt_tests;

#[cfg(test)]
mod tests {
    use super::{
        ChildEnvironment, CommandProcessObservation, CommandRun, TerminateAndReap,
        reconcile_command_timeout, reconcile_wait_failure, run_command_to_files_inner_with_wait,
        run_sensor_command_to_files, terminate_and_reap_child,
    };
    use anyhow::{Context, Result, ensure};
    use std::{collections::BTreeMap, collections::VecDeque, fs, io, thread, time::Duration};

    struct FakeChild {
        kill_fails: bool,
        reap_errors: VecDeque<io::ErrorKind>,
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
            self.reap_errors.pop_front().map_or(Ok(()), |kind| {
                Err(io::Error::new(kind, "injected wait failure"))
            })
        }
    }

    #[cfg(unix)]
    #[test]
    fn command_failure_reason_retains_signal_and_numeric_exit() -> Result<()> {
        let temp = tempfile::tempdir()?;
        for (script, expected_code, expected_reason) in [
            ("kill -TERM $$", None, "signal: 15"),
            ("exit 7", Some(7), "exit code Some(7)"),
        ] {
            let argv = vec!["sh".to_owned(), "-c".to_owned(), script.to_owned()];
            let status = super::run_command_to_files_with_spawn_observer(
                temp.path(),
                &argv,
                &BTreeMap::new(),
                5,
                &temp.path().join("stdout"),
                &temp.path().join("stderr"),
                &mut |_| {},
            )?;
            ensure!(!status.success && !status.timed_out);
            ensure!(status.exit_code == expected_code);
            ensure!(status.reason.contains(expected_reason), "{}", status.reason);
        }
        Ok(())
    }

    #[test]
    fn child_cleanup_waits_for_confirmed_exit_even_when_kill_reports_an_error() -> Result<()> {
        let mut child = FakeChild {
            kill_fails: true,
            reap_errors: VecDeque::new(),
            calls: Vec::new(),
        };
        let mut observations = Vec::new();

        terminate_and_reap_child(&mut child, &mut |event| observations.push(event))?;

        ensure!(child.calls == ["kill", "wait"]);
        ensure!(observations.is_empty());
        Ok(())
    }

    #[test]
    fn failed_kill_diagnostic_survives_confirmed_reap() -> Result<()> {
        let mut child = FakeChild {
            kill_fails: true,
            reap_errors: VecDeque::new(),
            calls: Vec::new(),
        };
        let mut observations = Vec::new();
        let result = terminate_and_reap_child(&mut child, &mut |event| observations.push(event));
        let kill_error = result?.context("failed kill must be returned after confirmed reap")?;
        ensure!(kill_error.to_string() == "injected kill failure");
        ensure!(child.calls == ["kill", "wait"]);
        ensure!(observations.is_empty());
        Ok(())
    }

    #[test]
    fn timeout_retains_kill_and_reap_outcomes() -> Result<()> {
        for kill_fails in [false, true] {
            for reap_fails in [false, true] {
                let mut child = FakeChild {
                    kill_fails,
                    reap_errors: if reap_fails {
                        VecDeque::from([io::ErrorKind::Other])
                    } else {
                        VecDeque::new()
                    },
                    calls: Vec::new(),
                };
                let mut observations = Vec::new();
                let started = std::time::Instant::now()
                    .checked_sub(Duration::from_secs(2))
                    .context("construct elapsed timeout interval")?;
                let minimum_duration_ms = started.elapsed().as_millis();
                let result = reconcile_command_timeout(
                    &mut child,
                    &mut |event| observations.push(event),
                    7,
                    started,
                );
                let message = if reap_fails {
                    let error = result.err().context("failed reap must fail closed")?;
                    ensure!(observations == [CommandProcessObservation::CompletionUnconfirmed]);
                    format!("{error:#}")
                } else {
                    let status = result?;
                    ensure!(status.timed_out && !status.success && status.exit_code.is_none());
                    ensure!(status.duration_ms >= minimum_duration_ms);
                    ensure!(status.duration_ms <= started.elapsed().as_millis());
                    ensure!(observations.is_empty());
                    status.reason
                };
                let expected = match (kill_fails, reap_fails) {
                    (false, false) => "timed out after 7s",
                    (true, false) => {
                        "timed out after 7s; kill request failed: injected kill failure; child reap confirmed"
                    }
                    (false, true) => {
                        "timed out after 7s: child completion remains unconfirmed (kill request succeeded; wait failed: injected wait failure)"
                    }
                    (true, true) => {
                        "timed out after 7s: child completion remains unconfirmed (kill request failed: injected kill failure; wait failed: injected wait failure)"
                    }
                };
                ensure!(message == expected);
                ensure!(child.calls == ["kill", "wait"]);
            }
        }
        Ok(())
    }

    #[test]
    fn wait_failure_retains_kill_and_reap_outcomes() -> Result<()> {
        for kill_fails in [false, true] {
            for reap_fails in [false, true] {
                let mut child = FakeChild {
                    kill_fails,
                    reap_errors: if reap_fails {
                        VecDeque::from([io::ErrorKind::Other])
                    } else {
                        VecDeque::new()
                    },
                    calls: Vec::new(),
                };
                let mut observations = Vec::new();
                let error = reconcile_wait_failure(
                    &mut child,
                    &mut |event| observations.push(event),
                    io::Error::other("original wait failure"),
                )
                .err()
                .context("original wait failure must propagate")?;
                let message = format!("{error:#}");
                let expected = match (kill_fails, reap_fails) {
                    (false, false) => "wait for child process: original wait failure",
                    (true, false) => {
                        "wait for child process; kill request failed: injected kill failure; child reap confirmed: original wait failure"
                    }
                    (false, true) => {
                        "terminate child after wait-timeout failure; original wait error: original wait failure: child completion remains unconfirmed (kill request succeeded; wait failed: injected wait failure)"
                    }
                    (true, true) => {
                        "terminate child after wait-timeout failure; original wait error: original wait failure: child completion remains unconfirmed (kill request failed: injected kill failure; wait failed: injected wait failure)"
                    }
                };
                ensure!(message == expected);
                ensure!(
                    observations
                        == if reap_fails {
                            vec![CommandProcessObservation::CompletionUnconfirmed]
                        } else {
                            Vec::new()
                        }
                );
                ensure!(child.calls == ["kill", "wait"]);
            }
        }
        Ok(())
    }

    #[test]
    fn child_cleanup_marks_release_unconfirmed_when_reaping_fails() -> Result<()> {
        let mut child = FakeChild {
            kill_fails: false,
            reap_errors: VecDeque::from([io::ErrorKind::Other]),
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
    fn child_cleanup_retries_interrupted_reaps_until_exit_is_confirmed() -> Result<()> {
        let mut child = FakeChild {
            kill_fails: false,
            reap_errors: VecDeque::from([io::ErrorKind::Interrupted, io::ErrorKind::Interrupted]),
            calls: Vec::new(),
        };
        let mut observations = Vec::new();

        let kill_error =
            terminate_and_reap_child(&mut child, &mut |event| observations.push(event))?;

        ensure!(kill_error.is_none());
        ensure!(child.calls == ["kill", "wait", "wait", "wait"]);
        ensure!(observations.is_empty());
        Ok(())
    }

    #[test]
    fn child_cleanup_bounds_persistent_interrupted_reaps() -> Result<()> {
        let mut child = FakeChild {
            kill_fails: false,
            reap_errors: VecDeque::from([io::ErrorKind::Interrupted; 16]),
            calls: Vec::new(),
        };
        let mut observations = Vec::new();

        let result = terminate_and_reap_child(&mut child, &mut |event| observations.push(event));

        let error = result
            .err()
            .context("persistent interrupts must fail closed")?;
        ensure!(format!("{error:#}").contains("reap interrupted 16 consecutive times"));
        ensure!(child.calls.len() == 17);
        ensure!(child.calls.first() == Some(&"kill"));
        ensure!(child.calls.iter().skip(1).all(|call| *call == "wait"));
        ensure!(observations == [CommandProcessObservation::CompletionUnconfirmed]);
        Ok(())
    }

    #[test]
    fn wait_cleanup_failure_preserves_wait_and_reap_errors() -> Result<()> {
        let mut child = FakeChild {
            kill_fails: false,
            reap_errors: VecDeque::from([io::ErrorKind::Other]),
            calls: Vec::new(),
        };
        let mut observations = Vec::new();

        let result = reconcile_wait_failure(
            &mut child,
            &mut |event| observations.push(event),
            io::Error::other("injected wait_timeout failure"),
        );

        let error = result.err().context("cleanup failure must propagate")?;
        let diagnostic = format!("{error:#}");
        ensure!(diagnostic.contains("injected wait_timeout failure"));
        ensure!(diagnostic.contains("injected wait failure"));
        ensure!(observations == [CommandProcessObservation::CompletionUnconfirmed]);
        Ok(())
    }

    #[test]
    fn wait_error_branch_reaps_live_child_before_returning() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let started_marker = temp.path().join("started");
        let delayed_marker = temp.path().join("delayed");
        let stdout = temp.path().join("stdout.txt");
        let stderr = temp.path().join("stderr.txt");
        let argv = vec![
            std::env::current_exe()?.display().to_string(),
            "--exact".to_owned(),
            "lane_packets::tests::fake_delayed_marker_child".to_owned(),
            "--nocapture".to_owned(),
        ];
        let env = BTreeMap::from([
            (
                "UB_REVIEW_WAIT_ERROR_STARTED".to_owned(),
                started_marker.display().to_string(),
            ),
            (
                "UB_REVIEW_WAIT_ERROR_DELAYED".to_owned(),
                delayed_marker.display().to_string(),
            ),
        ]);
        let ambient = BTreeMap::new();
        let mut observations = Vec::new();
        let mut wait_for_child = |_child: &mut std::process::Child, _timeout: Duration| {
            for _ in 0..100 {
                if started_marker.is_file() {
                    return Err(io::Error::other("injected wait_timeout failure"));
                }
                thread::sleep(Duration::from_millis(20));
            }
            Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "child did not publish its start marker",
            ))
        };

        let result = run_command_to_files_inner_with_wait(
            CommandRun {
                root: temp.path(),
                argv: &argv,
                env: &env,
                ambient: &ambient,
                timeout_sec: 5,
                stdout_path: &stdout,
                stderr_path: &stderr,
                child_environment: ChildEnvironment::AmbientPlusExplicit,
                observe_process: &mut |event| observations.push(event),
            },
            &mut wait_for_child,
        );

        let error = result
            .err()
            .context("injected wait failure must propagate")?;
        ensure!(format!("{error:#}").contains("injected wait_timeout failure"));
        ensure!(started_marker.is_file());
        ensure!(observations == [CommandProcessObservation::Spawned]);
        thread::sleep(Duration::from_millis(2300));
        ensure!(!delayed_marker.exists());
        Ok(())
    }

    #[test]
    fn fake_delayed_marker_child() -> Result<()> {
        let Some(started) = std::env::var_os("UB_REVIEW_WAIT_ERROR_STARTED") else {
            return Ok(());
        };
        let delayed = std::env::var_os("UB_REVIEW_WAIT_ERROR_DELAYED")
            .context("delayed marker path missing")?;
        fs::write(started, b"started")?;
        thread::sleep(Duration::from_secs(2));
        fs::write(delayed, b"still running")?;
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn sensor_environment_preserves_native_bytes_without_panicking() -> Result<()> {
        use std::ffi::OsString;
        use std::os::unix::ffi::OsStringExt;

        for (label, key, value, lang) in [
            (
                "unicode",
                OsString::from("INERT_SECRET"),
                OsString::from("inert"),
                OsString::from("C"),
            ),
            (
                "native-value",
                OsString::from("INERT_SECRET"),
                OsString::from_vec(vec![0xff]),
                OsString::from("C"),
            ),
            (
                "native-key",
                OsString::from_vec(vec![b'X', 0xff]),
                OsString::from("inert"),
                OsString::from("C"),
            ),
            (
                "native-allowed-value",
                OsString::from("INERT_SECRET"),
                OsString::from("inert"),
                OsString::from_vec(vec![b'C', 0xff]),
            ),
        ] {
            let output = std::process::Command::new(std::env::current_exe()?)
                .args([
                    "--exact",
                    "lane_packets::tests::fake_sensor_native_environment_child",
                    "--nocapture",
                ])
                .env_clear()
                .env("UB_REVIEW_NATIVE_ENV_CHILD", "1")
                .env("LANG", lang)
                .env(key, value)
                .output()?;
            ensure!(
                output.status.success(),
                "{label} failed: {} {}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn fake_sensor_native_environment_child() -> Result<()> {
        use std::os::unix::ffi::OsStrExt;

        if std::env::var_os("UB_REVIEW_NATIVE_ENV_CHILD").is_none() {
            return Ok(());
        }
        let expected = std::env::var_os("LANG").context("native LANG fixture missing")?;
        let temp = tempfile::tempdir()?;
        let stdout = temp.path().join("stdout");
        let stderr = temp.path().join("stderr");
        let argv = vec!["/usr/bin/env".to_owned()];
        let mut observations = Vec::new();
        let status = super::run_sensor_command_to_files_with_spawn_observer(
            temp.path(),
            &argv,
            &BTreeMap::new(),
            5,
            &stdout,
            &stderr,
            &mut |event| observations.push(event),
        )?;
        ensure!(status.success && !status.timed_out && status.exit_code == Some(0));
        ensure!(observations == [CommandProcessObservation::Spawned]);
        let mut expected_output = b"LANG=".to_vec();
        expected_output.extend_from_slice(expected.as_os_str().as_bytes());
        expected_output.push(b'\n');
        ensure!(fs::read(stdout)? == expected_output);
        ensure!(fs::read(stderr)?.is_empty());
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

    // #1062: the production proof/worker spawn path must quarantine the child
    // environment exactly as sensors do. The probe runs the real production
    // runner inside a test binary whose parent environment carries canary
    // provider keys and GitHub tokens, then asserts the child proof fixture
    // cannot see any of them while the allowlisted toolchain entries survive.

    #[test]
    fn proof_environment_excludes_credentials_and_keeps_toolchain_metadata() -> Result<()> {
        let exe = std::env::current_exe()?;
        let status = std::process::Command::new(exe)
            .arg("--exact")
            .arg("lane_packets::tests::fake_proof_cannot_read_actual_parent_credentials")
            .arg("--nocapture")
            .env("UB_REVIEW_PROOF_ENV_PROBE_CHILD", "1")
            .env("UB_REVIEW_MINIMAX_API_KEY", "sentinel-minimax")
            .env("UB_REVIEW_OPENCODE_API_KEY", "sentinel-opencode")
            .env("UB_REVIEW_ACTION_GITHUB_TOKEN", "sentinel-github")
            .env("UB_REVIEW_GITHUB_TOKEN", "sentinel-github-v1")
            .env("GH_TOKEN", "sentinel-gh-token")
            .env("ACTIONS_ID_TOKEN_REQUEST_TOKEN", "sentinel-oidc")
            .env("ACTIONS_RUNTIME_TOKEN", "sentinel-runner")
            .status()?;
        ensure!(
            status.success(),
            "actual-parent proof probe process failed: {status}"
        );
        Ok(())
    }

    #[test]
    fn fake_proof_cannot_read_actual_parent_credentials() -> Result<()> {
        if std::env::var_os("UB_REVIEW_PROOF_ENV_PROBE_CHILD").is_none() {
            return Ok(());
        }
        let temp = tempfile::tempdir()?;
        let stdout = temp.path().join("stdout.txt");
        let stderr = temp.path().join("stderr.txt");
        #[cfg(unix)]
        let argv = vec!["/bin/sh".to_owned(), "-c".to_owned(), "env".to_owned()];
        #[cfg(windows)]
        let argv = vec![std::env::var("COMSPEC")?, "/C".to_owned(), "set".to_owned()];
        // A caller-supplied environment cannot add entries to the allowlist:
        // this override key must be dropped at spawn even though the command
        // asked for it explicitly.
        let overrides = BTreeMap::from([(
            "UB_REVIEW_PROOF_SENTINEL_OVERRIDE".to_owned(),
            "sentinel-override".to_owned(),
        )]);
        let mut observations = Vec::new();
        let status = super::run_command_to_files_with_spawn_observer(
            temp.path(),
            &argv,
            &overrides,
            // Windows hosts can spend several seconds on a first spawn under
            // the cleared environment (real-time scanner warm-up); the budget
            // is generous so the assertion measures quarantine, not host speed.
            60,
            &stdout,
            &stderr,
            &mut |event| observations.push(event),
        )?;
        ensure!(
            status.success,
            "quarantined proof fixture failed: {}",
            status.reason
        );
        ensure!(observations == [CommandProcessObservation::Spawned]);
        let observed = fs::read_to_string(stdout)?;
        let lowered = observed.to_ascii_lowercase();
        // Positive control: the allowlist keeps the child executable. The
        // child must still see its ambient PATH value verbatim and at least
        // one explicitly resolved toolchain home.
        let expected_path = std::env::var("PATH").context("ambient PATH fixture missing")?;
        let expected_path_line = format!("path={}", expected_path.to_ascii_lowercase());
        ensure!(
            lowered.lines().any(|line| line == expected_path_line),
            "quarantined child did not observe the ambient PATH value verbatim: {observed:?}"
        );
        ensure!(
            lowered.contains("cargo_home=") || lowered.contains("rustup_home="),
            "quarantined child lost its toolchain home: {observed:?}"
        );
        // Sentinel negatives: provider keys, GitHub write tokens, and OIDC or
        // runner authority must not cross the spawn boundary.
        for sentinel in [
            "sentinel-minimax",
            "sentinel-opencode",
            "sentinel-github",
            "sentinel-github-v1",
            "sentinel-gh-token",
            "sentinel-oidc",
            "sentinel-runner",
        ] {
            ensure!(
                !lowered.contains(sentinel),
                "quarantined proof child observed sentinel credential {sentinel}: {observed:?}"
            );
        }
        // No ambient home/config credential leak: the home directory entries
        // themselves never cross the boundary even though the parent had one.
        ensure!(
            !lowered.lines().any(|line| {
                ["home=", "userprofile=", "homedrive=", "homepath="]
                    .iter()
                    .any(|entry| line.starts_with(entry))
            }),
            "quarantined proof child observed an ambient home entry: {observed:?}"
        );
        // Candidate-provided environment cannot add allowlist entries.
        ensure!(
            !lowered.contains("sentinel-override"),
            "quarantined proof child accepted a non-allowlisted override: {observed:?}"
        );
        Ok(())
    }

    #[test]
    fn proof_environment_runs_real_toolchain_child_under_allowlist() -> Result<()> {
        // The probe is only meaningful where the ambient environment resolves
        // cargo; hosts without a toolchain skip rather than fake signal.
        let Ok(probe) = std::process::Command::new("cargo")
            .arg("--version")
            .output()
        else {
            return Ok(());
        };
        ensure!(probe.status.success(), "ambient cargo probe failed");
        let temp = tempfile::tempdir()?;
        let stdout = temp.path().join("stdout.txt");
        let stderr = temp.path().join("stderr.txt");
        let status = super::run_command_to_files_with_spawn_observer(
            temp.path(),
            &["cargo".to_owned(), "--version".to_owned()],
            &BTreeMap::new(),
            120,
            &stdout,
            &stderr,
            &mut |_| {},
        )?;
        ensure!(
            status.success,
            "quarantined cargo child failed: {} (stderr: {})",
            status.reason,
            fs::read_to_string(&stderr).unwrap_or_default()
        );
        let observed = fs::read_to_string(&stdout)?;
        ensure!(
            observed.contains("cargo"),
            "unexpected quarantined cargo output: {observed:?}"
        );
        Ok(())
    }
}
