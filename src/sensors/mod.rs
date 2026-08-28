//! Sensor execution: command construction, the sensor runner, status
//! receipts, and per-sensor artifact enumeration (cleanup train step 6,
//! pure code motion). Plan-time trigger resolution stays in main.

pub(crate) mod command_build;
pub(crate) use command_build::*;
pub(crate) mod coverage;
pub(crate) mod ripr;
pub(crate) mod unsafe_review;

pub(crate) use coverage::*;
pub(crate) use ripr::*;
pub(crate) use unsafe_review::*;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::system_detect::command_on_path;
use crate::task_ledger::TaskTerminalDisposition;
use crate::*;

/// Run a set of sensors on a bounded worker pool. Individual sensor failures
/// are captured as `sensor_degraded` (missing evidence, never clean
/// evidence); only infrastructure failures (poisoned queue) error out.
pub(crate) fn run_sensor_pool(
    root: &Path,
    out: &Path,
    plan: &Plan,
    jobs: usize,
    sensors: VecDeque<SensorPlan>,
    event_log: &EventLog,
    task_ledger: Option<&SensorTaskLedger>,
) -> Result<()> {
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));

    thread::scope(|scope| {
        if jobs == 1 {
            let queue = Mutex::new(sensors);
            let failures = Arc::clone(&failures);
            scope.spawn(move || {
                drain_sensor_queue(root, out, plan, &queue, event_log, task_ledger, &failures)
            });
            return;
        }

        let (cargo_queue, parallel_queue) = partition_sensor_queues(sensors);
        let cargo_worker_count = usize::from(jobs > 0 && !cargo_queue.is_empty());
        if cargo_worker_count == 1 {
            let queue = Mutex::new(cargo_queue);
            let failures = Arc::clone(&failures);
            scope.spawn(move || {
                drain_sensor_queue(root, out, plan, &queue, event_log, task_ledger, &failures)
            });
        }

        let parallel_worker_count = jobs
            .saturating_sub(cargo_worker_count)
            .min(parallel_queue.len());
        let queue = Arc::new(Mutex::new(parallel_queue));
        for _ in 0..parallel_worker_count {
            let queue = Arc::clone(&queue);
            let failures = Arc::clone(&failures);
            scope.spawn(move || {
                drain_sensor_queue(root, out, plan, &queue, event_log, task_ledger, &failures)
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

/// Drain one sensor queue. A queue has one owner for shared Cargo-target work
/// and up to the remaining profile worker budget for independent commands.
fn drain_sensor_queue(
    root: &Path,
    out: &Path,
    plan: &Plan,
    queue: &Mutex<VecDeque<SensorPlan>>,
    event_log: &EventLog,
    task_ledger: Option<&SensorTaskLedger>,
    failures: &Mutex<Vec<String>>,
) {
    loop {
        let sensor = match queue.lock() {
            Ok(mut queue) => queue.pop_front(),
            Err(_) => None,
        };
        let Some(sensor) = sensor else {
            break;
        };
        let result = match task_ledger {
            Some(ledger) => {
                run_sensor_with_task_ledger(root, out, &sensor, event_log, plan, ledger)
            }
            None => run_sensor(root, out, &sensor, event_log, plan),
        };
        if let Err(err) = result
            && let Ok(mut failures) = failures.lock()
        {
            failures.push(format!("{}: {err:#}", sensor.id));
        }
    }
}

/// Keep every Cargo invocation that shares the checkout target directory on
/// one ordered worker. Cargo's artifact lock protects compilation, but it can
/// be released while a test binary is still executing; a concurrent Clippy or
/// doc build may then relink that binary and trigger Linux `ETXTBSY` (#1257).
fn partition_sensor_queues(
    sensors: VecDeque<SensorPlan>,
) -> (VecDeque<SensorPlan>, VecDeque<SensorPlan>) {
    sensors
        .into_iter()
        .partition(sensor_uses_shared_cargo_target)
}

fn sensor_uses_shared_cargo_target(sensor: &SensorPlan) -> bool {
    if matches!(
        sensor.id.as_str(),
        "cargo-fmt"
            | "cargo-check"
            | "cargo-test"
            | "cargo-clippy"
            | "cargo-doc"
            | "coverage"
            | "cargo-audit"
            | "cargo-deny"
    ) {
        return true;
    }
    let executable = sensor
        .command
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(sensor.command.as_str());
    executable.eq_ignore_ascii_case("cargo") || executable.eq_ignore_ascii_case("cargo.exe")
}

/// Handle for the late evidence phase (#325): heavy sensors (test, build,
/// coverage, lease-gated witnesses) running on a background pool while the
/// model wave investigates off the fast-sensor precontext. The phase is
/// joined before the reporter, review compile, and gate so every downstream
/// consumer evaluates complete sensor evidence; a late sensor without a
/// receipt at join time stays missing evidence, never clean evidence.
pub(crate) struct LateSensorPhase {
    handle: std::thread::JoinHandle<Result<()>>,
    pub(crate) sensor_ids: Vec<String>,
    run_loop: ActiveRunLoop,
}

/// Owned work transferred to the late sensor thread. Keeping the shadow
/// observer beside its exact sensor set prevents separate argument drift.
pub(crate) struct LateSensorWork {
    sensors: Vec<SensorPlan>,
    task_ledger: Option<SensorTaskLedger>,
}

impl LateSensorWork {
    pub(crate) fn new(sensors: Vec<SensorPlan>, task_ledger: Option<SensorTaskLedger>) -> Self {
        Self {
            sensors,
            task_ledger,
        }
    }
}

pub(crate) fn spawn_late_sensor_phase(
    root: &Path,
    out: &Path,
    plan: &Plan,
    profile: &Profile,
    event_log: &Arc<EventLog>,
    run_started: &Instant,
    work: LateSensorWork,
) -> Result<LateSensorPhase> {
    let LateSensorWork {
        sensors,
        task_ledger,
    } = work;
    let job_budget = sensor_job_count(profile, sensors.len())?;
    let jobs = sensor_pool_worker_count(job_budget, &sensors);
    let sensor_ids: Vec<String> = sensors.iter().map(|sensor| sensor.id.clone()).collect();
    let run_loop = start_run_loop(
        event_log,
        run_started,
        "evidence",
        "coordination",
        "late-sensors",
    )?;
    event_log.append(
        "late_sensor_phase_started",
        serde_json::json!({"sensors": sensor_ids, "jobs": jobs}),
    )?;
    let thread_root = root.to_path_buf();
    let thread_out = out.to_path_buf();
    let thread_plan = plan.clone();
    let thread_log = Arc::clone(event_log);
    let queue: VecDeque<SensorPlan> = sensors.into();
    let handle = std::thread::Builder::new()
        .name("ub-review-late-sensors".to_owned())
        .spawn(move || {
            run_sensor_pool(
                &thread_root,
                &thread_out,
                &thread_plan,
                jobs,
                queue,
                &thread_log,
                task_ledger.as_ref(),
            )
        })?;
    Ok(LateSensorPhase {
        handle,
        sensor_ids,
        run_loop,
    })
}

impl LateSensorPhase {
    /// Block until every late sensor has written its receipt (or failed),
    /// recording the scheduler phase and a join event. Must run before the
    /// reporter, tool-status/gate-outcome computation, and the review
    /// compile.
    pub(crate) fn join(
        self,
        event_log: &EventLog,
        run_started: &Instant,
        tracker: &mut RunLoopTracker,
    ) -> Result<()> {
        let phase = self.join_phase(event_log, run_started)?;
        tracker.record(phase);
        Ok(())
    }

    /// Join the late sensor owner and return its scheduler phase for a caller
    /// that must wait on another thread before recording shared run state.
    pub(crate) fn join_phase(
        self,
        event_log: &EventLog,
        run_started: &Instant,
    ) -> Result<RunLoopPhase> {
        let joined = self.handle.join();
        let status = match &joined {
            Ok(Ok(())) => "completed",
            Ok(Err(_)) => "failed",
            Err(_) => "panicked",
        };
        event_log.append(
            "late_sensor_phase_joined",
            serde_json::json!({"sensors": self.sensor_ids, "status": status}),
        )?;
        let phase = finish_run_loop_phase(event_log, run_started, self.run_loop, status)?;
        match joined {
            Ok(result) => {
                result?;
                Ok(phase)
            }
            Err(_) => Err(anyhow::anyhow!("late sensor phase thread panicked")),
        }
    }
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

fn sensor_pool_worker_count(job_budget: usize, sensors: &[SensorPlan]) -> usize {
    let cargo_worker_count =
        usize::from(job_budget > 0 && sensors.iter().any(sensor_uses_shared_cargo_target));
    let parallel_sensor_count = sensors
        .iter()
        .filter(|sensor| !sensor_uses_shared_cargo_target(sensor))
        .count();
    cargo_worker_count
        + job_budget
            .saturating_sub(cargo_worker_count)
            .min(parallel_sensor_count)
}

fn classify_sensor_command_result(
    sensor_id: &str,
    result: &CommandStatus,
) -> (&'static str, String) {
    if result.timed_out {
        return ("timed_out", result.reason.clone());
    }
    if result.success {
        return ("ok", result.reason.clone());
    }
    if sensor_id == "unsafe-review" && result.exit_code == Some(1) {
        return (
            "ok",
            "unsafe-review completed with policy findings (exit 1)".to_owned(),
        );
    }
    ("failed", result.reason.clone())
}

#[derive(Clone, Copy)]
enum SensorProcessObservation {
    AttemptPrepared,
    Spawned,
    Finished(TaskTerminalDisposition),
    CompletionUnconfirmed,
}

#[derive(Clone, Copy)]
struct SensorCommandAttempt<'a> {
    root: &'a Path,
    out: &'a Path,
    sensor: &'a SensorPlan,
    event_log: &'a EventLog,
    plan: &'a Plan,
    command_available: bool,
}

pub(crate) fn run_sensor(
    root: &Path,
    out: &Path,
    sensor: &SensorPlan,
    event_log: &EventLog,
    plan: &Plan,
) -> Result<()> {
    let command_available = command_on_path(&sensor.command);
    let mut ignore_process = |_| {};
    run_sensor_with_command_availability(
        root,
        out,
        sensor,
        event_log,
        plan,
        command_available,
        &mut ignore_process,
    )
}

fn run_sensor_with_task_ledger(
    root: &Path,
    out: &Path,
    sensor: &SensorPlan,
    event_log: &EventLog,
    plan: &Plan,
    task_ledger: &SensorTaskLedger,
) -> Result<()> {
    // Observation must never suppress the existing process attempt. Defer any
    // adapter error until after the runner and receipt path have completed.
    let command_available = command_on_path(&sensor.command);
    run_sensor_with_task_ledger_and_command_availability(
        root,
        out,
        sensor,
        event_log,
        plan,
        task_ledger,
        command_available,
    )
}

fn run_sensor_with_task_ledger_and_command_availability(
    root: &Path,
    out: &Path,
    sensor: &SensorPlan,
    event_log: &EventLog,
    plan: &Plan,
    task_ledger: &SensorTaskLedger,
    command_available: bool,
) -> Result<()> {
    run_sensor_attempt_with_task_ledger(
        SensorCommandAttempt {
            root,
            out,
            sensor,
            event_log,
            plan,
            command_available,
        },
        task_ledger,
        &mut remove_prior_sensor_status,
    )
}

fn run_sensor_attempt_with_task_ledger(
    attempt: SensorCommandAttempt<'_>,
    task_ledger: &SensorTaskLedger,
    prepare_attempt: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let SensorCommandAttempt {
        out,
        sensor,
        command_available,
        ..
    } = attempt;
    let setup_observation = task_ledger.setup_started(sensor);
    let mut process_started = false;
    let mut process_finished = false;
    let mut process_completion_unconfirmed = false;
    let mut attempt_prepared = false;
    let mut run_observation = Ok(());
    let mut finish_observation = Ok(());
    let mut observe_process = |observation| match observation {
        SensorProcessObservation::AttemptPrepared => {
            attempt_prepared = true;
        }
        SensorProcessObservation::Spawned => {
            if !process_started {
                process_started = true;
                if setup_observation.is_ok() {
                    run_observation = task_ledger.run_started(sensor);
                }
            }
        }
        SensorProcessObservation::Finished(disposition) => {
            if process_started && !process_finished {
                process_finished = true;
                if setup_observation.is_ok() && run_observation.is_ok() {
                    finish_observation = task_ledger.process_finished(sensor, disposition);
                }
            }
        }
        SensorProcessObservation::CompletionUnconfirmed => {
            process_completion_unconfirmed = true;
        }
    };
    let run_result = run_sensor_attempt(attempt, &mut observe_process, prepare_attempt);
    if process_started && !process_finished && !process_completion_unconfirmed {
        process_finished = true;
        if setup_observation.is_ok() && run_observation.is_ok() {
            finish_observation =
                task_ledger.process_finished(sensor, TaskTerminalDisposition::DeterministicFailure);
        }
    }
    let cleanup_observation = if setup_observation.is_ok()
        && run_observation.is_ok()
        && finish_observation.is_ok()
        && process_finished
    {
        task_ledger.cleanup_finished(sensor)
    } else {
        Ok(())
    };
    let receipt_result = if attempt_prepared {
        sensor_task_receipt_disposition(out, sensor, command_available)
    } else {
        Err(anyhow::anyhow!(
            "sensor {} attempt was not prepared",
            sensor.id
        ))
    };
    let receipt_created = receipt_result.is_ok();
    let terminal_observation = if setup_observation.is_err()
        || run_observation.is_err()
        || finish_observation.is_err()
        || cleanup_observation.is_err()
    {
        Ok(())
    } else if process_started {
        ensure!(
            process_finished,
            "sensor {} process completion was not observed",
            sensor.id
        );
        task_ledger.receipt_recorded_and_resources_released(sensor, receipt_created)
    } else {
        task_ledger.setup_failed(sensor, receipt_created)
    };
    run_result?;
    receipt_result?;
    setup_observation?;
    run_observation?;
    finish_observation?;
    cleanup_observation?;
    terminal_observation
}

fn sensor_task_receipt_disposition(
    out: &Path,
    sensor: &SensorPlan,
    command_available: bool,
) -> Result<TaskTerminalDisposition> {
    let path = out
        .join("sensors")
        .join(&sensor.id)
        .join("ub-review-sensor-status.json");
    let value: serde_json::Value = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("read sensor status {}", path.display()))?,
    )
    .with_context(|| format!("parse sensor status {}", path.display()))?;
    ensure!(
        value.get("sensor").and_then(serde_json::Value::as_str) == Some(sensor.id.as_str()),
        "sensor status identity does not match {}",
        sensor.id
    );
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .context("sensor status receipt has no string status")?;
    match status {
        "ok" => Ok(TaskTerminalDisposition::Succeeded),
        "failed" => Ok(TaskTerminalDisposition::DeterministicFailure),
        "timed_out" => Ok(TaskTerminalDisposition::TimedOut),
        "missing" if !command_available => Ok(TaskTerminalDisposition::SetupFailed),
        "missing" => bail!("running sensor {} reported missing command", sensor.id),
        other => bail!("sensor {} reported unsupported status {other}", sensor.id),
    }
}

fn run_sensor_with_command_availability(
    root: &Path,
    out: &Path,
    sensor: &SensorPlan,
    event_log: &EventLog,
    plan: &Plan,
    command_available: bool,
    observe_process: &mut dyn FnMut(SensorProcessObservation),
) -> Result<()> {
    run_sensor_attempt(
        SensorCommandAttempt {
            root,
            out,
            sensor,
            event_log,
            plan,
            command_available,
        },
        observe_process,
        &mut remove_prior_sensor_status,
    )
}

fn run_sensor_attempt(
    attempt: SensorCommandAttempt<'_>,
    observe_process: &mut dyn FnMut(SensorProcessObservation),
    prepare_attempt: &mut dyn FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let SensorCommandAttempt {
        root,
        out,
        sensor,
        event_log,
        plan,
        command_available,
    } = attempt;
    let dir = out.join("sensors").join(&sensor.id);
    fs::create_dir_all(&dir)?;
    prepare_attempt(&dir)?;
    observe_process(SensorProcessObservation::AttemptPrepared);
    let argv = build_sensor_argv(root, &dir, sensor, plan);
    if !command_available {
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
        return run_tokmd_sensor(root, out, sensor, event_log, plan, &argv, observe_process);
    }
    let stdout_path = dir.join("stdout.txt");
    let stderr_path = dir.join("stderr.txt");
    let mut process_spawned = false;
    let mut process_completion_unconfirmed = false;
    let result = {
        let mut observe_command_process = |observation| match observation {
            CommandProcessObservation::Spawned => {
                process_spawned = true;
                observe_process(SensorProcessObservation::Spawned);
            }
            CommandProcessObservation::CompletionUnconfirmed => {
                process_completion_unconfirmed = true;
                observe_process(SensorProcessObservation::CompletionUnconfirmed);
            }
        };
        run_sensor_command_to_files_with_spawn_observer(
            root,
            &argv,
            &BTreeMap::new(),
            sensor.timeout_sec,
            &stdout_path,
            &stderr_path,
            &mut observe_command_process,
        )
    };
    match result {
        Ok(result) => {
            let (status, reason) = classify_sensor_command_result(&sensor.id, &result);
            if process_spawned && !process_completion_unconfirmed {
                observe_process(SensorProcessObservation::Finished(
                    sensor_process_disposition(status),
                ));
            }
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
                write_ripr_exposure_gap_details(root, &dir, &sensor.command, sensor.timeout_sec);
            }
            write_sensor_status(
                out,
                sensor,
                SensorStatusWrite {
                    status,
                    argv: &argv,
                    duration_ms: result.duration_ms,
                    reason: &reason,
                    exit_code: result.exit_code,
                    timed_out: result.timed_out,
                },
            )?;
            event_log.append(
                if status == "ok" {
                    "sensor_completed"
                } else {
                    "sensor_failed"
                },
                serde_json::json!({"sensor": sensor.id, "exit_code": result.exit_code, "timed_out": result.timed_out, "reason": reason}),
            )?;
        }
        Err(err) => {
            if process_completion_unconfirmed {
                return Err(err).context(format!(
                    "{} child completion remains unconfirmed",
                    sensor.id
                ));
            }
            if process_spawned && !process_completion_unconfirmed {
                observe_process(SensorProcessObservation::Finished(
                    TaskTerminalDisposition::DeterministicFailure,
                ));
            }
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

fn sensor_process_disposition(status: &str) -> TaskTerminalDisposition {
    match status {
        "ok" => TaskTerminalDisposition::Succeeded,
        "timed_out" => TaskTerminalDisposition::TimedOut,
        _ => TaskTerminalDisposition::DeterministicFailure,
    }
}

fn remove_prior_sensor_status(dir: &Path) -> Result<()> {
    let path = dir.join("ub-review-sensor-status.json");
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove stale {}", path.display())),
    }
}

fn run_tokmd_sensor(
    root: &Path,
    out: &Path,
    sensor: &SensorPlan,
    event_log: &EventLog,
    plan: &Plan,
    aggregate_argv: &[String],
    observe_process: &mut dyn FnMut(SensorProcessObservation),
) -> Result<()> {
    run_tokmd_sensor_with_command_runner(
        TokmdSensorRun {
            root,
            out,
            sensor,
            event_log,
            plan,
            aggregate_argv,
            observe_process,
        },
        &mut |root, argv, env, timeout_sec, stdout_path, stderr_path, observer| {
            run_sensor_command_to_files_with_spawn_observer(
                root,
                argv,
                env,
                timeout_sec,
                stdout_path,
                stderr_path,
                observer,
            )
        },
    )
}

type SensorCommandRunner<'a> = dyn FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
        &mut dyn FnMut(CommandProcessObservation),
    ) -> Result<CommandStatus>
    + 'a;

struct TokmdSensorRun<'a> {
    root: &'a Path,
    out: &'a Path,
    sensor: &'a SensorPlan,
    event_log: &'a EventLog,
    plan: &'a Plan,
    aggregate_argv: &'a [String],
    observe_process: &'a mut dyn FnMut(SensorProcessObservation),
}

fn run_tokmd_sensor_with_command_runner(
    run: TokmdSensorRun<'_>,
    run_command: &mut SensorCommandRunner<'_>,
) -> Result<()> {
    let TokmdSensorRun {
        root,
        out,
        sensor,
        event_log,
        plan,
        aggregate_argv,
        observe_process,
    } = run;
    let dir = out.join("sensors").join(&sensor.id);
    let mut commands = vec![build_tokmd_version_preflight_command(&dir, sensor)];
    commands.extend(build_tokmd_sensor_commands(root, &dir, plan));
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
    let preflight = commands
        .first()
        .ok_or_else(|| anyhow::anyhow!("tokmd version preflight command missing"))?;
    event_log.append(
        "sensor_subcommand_started",
        serde_json::json!({"sensor": sensor.id, "label": preflight.label, "argv": preflight.argv}),
    )?;
    let mut process_spawned = false;
    let mut process_completion_unconfirmed = false;
    let preflight_result = {
        let mut observe_command_process = |observation| match observation {
            CommandProcessObservation::Spawned => {
                process_spawned = true;
                observe_process(SensorProcessObservation::Spawned);
            }
            CommandProcessObservation::CompletionUnconfirmed => {
                process_completion_unconfirmed = true;
                observe_process(SensorProcessObservation::CompletionUnconfirmed);
            }
        };
        run_command(
            root,
            &preflight.argv,
            &BTreeMap::new(),
            sensor.timeout_sec,
            &preflight.stdout_path,
            &preflight.stderr_path,
            &mut observe_command_process,
        )
    };
    match preflight_result {
        Ok(result) => {
            let preflight_failure_reason = tokmd_version_preflight_failure_reason(
                &result,
                &preflight.stdout_path,
                &preflight.stderr_path,
            );
            if preflight_failure_reason.is_some() && process_spawned {
                observe_process(SensorProcessObservation::Finished(if result.timed_out {
                    TaskTerminalDisposition::TimedOut
                } else {
                    TaskTerminalDisposition::DeterministicFailure
                }));
            }
            append_tokmd_subcommand_receipts(
                &aggregate_stdout_path,
                &aggregate_stderr_path,
                preflight,
                &result,
            )?;
            event_log.append(
                if result.success {
                    "sensor_subcommand_completed"
                } else {
                    "sensor_subcommand_failed"
                },
                serde_json::json!({"sensor": sensor.id, "label": preflight.label, "exit_code": result.exit_code, "timed_out": result.timed_out, "reason": result.reason}),
            )?;
            if let Some(reason) = preflight_failure_reason {
                write_sensor_status(
                    out,
                    sensor,
                    SensorStatusWrite {
                        status: if result.timed_out {
                            "timed_out"
                        } else {
                            "failed"
                        },
                        argv: aggregate_argv,
                        duration_ms: started.elapsed().as_millis(),
                        reason: &reason,
                        exit_code: result.exit_code,
                        timed_out: result.timed_out,
                    },
                )?;
                event_log.append(
                    "sensor_failed",
                    serde_json::json!({"sensor": sensor.id, "reason": reason}),
                )?;
                return Ok(());
            }
        }
        Err(err) => {
            if process_completion_unconfirmed {
                return Err(err).context("tokmd version-preflight completion remains unconfirmed");
            }
            if process_spawned && !process_completion_unconfirmed {
                observe_process(SensorProcessObservation::Finished(
                    TaskTerminalDisposition::DeterministicFailure,
                ));
            }
            let reason = format!(
                "tokmd version preflight failed: {err:#}; pin requires {} ({} preset); fix: {}",
                expected_standard_image_tool_version("tokmd")
                    .unwrap_or(STANDARD_IMAGE_TOKMD_VERSION),
                TOKMD_ANALYZE_PRESET,
                doctor_tool_version_fix(
                    "tokmd",
                    expected_standard_image_tool_version("tokmd")
                        .unwrap_or(STANDARD_IMAGE_TOKMD_VERSION)
                )
            );
            append_file(
                &aggregate_stdout_path,
                &format!(
                    "$ {}\nstatus=failed duration_ms=0\n\n",
                    display_command(&preflight.argv)
                ),
            )?;
            write_sensor_status(
                out,
                sensor,
                SensorStatusWrite {
                    status: "failed",
                    argv: aggregate_argv,
                    duration_ms: started.elapsed().as_millis(),
                    reason: &reason,
                    exit_code: None,
                    timed_out: false,
                },
            )?;
            event_log.append(
                "sensor_subcommand_failed",
                serde_json::json!({"sensor": sensor.id, "label": preflight.label, "reason": reason}),
            )?;
            event_log.append(
                "sensor_failed",
                serde_json::json!({"sensor": sensor.id, "reason": reason}),
            )?;
            return Ok(());
        }
    }

    for command in commands.iter().skip(1) {
        event_log.append(
            "sensor_subcommand_started",
            serde_json::json!({"sensor": sensor.id, "label": command.label, "argv": command.argv}),
        )?;
        let result = {
            let mut observe_command_process = |observation| match observation {
                CommandProcessObservation::Spawned => {
                    process_spawned = true;
                    observe_process(SensorProcessObservation::Spawned);
                }
                CommandProcessObservation::CompletionUnconfirmed => {
                    process_completion_unconfirmed = true;
                    observe_process(SensorProcessObservation::CompletionUnconfirmed);
                }
            };
            run_command(
                root,
                &command.argv,
                &BTreeMap::new(),
                sensor.timeout_sec,
                &command.stdout_path,
                &command.stderr_path,
                &mut observe_command_process,
            )
        };
        match result {
            Ok(result) => {
                append_tokmd_subcommand_receipts(
                    &aggregate_stdout_path,
                    &aggregate_stderr_path,
                    command,
                    &result,
                )?;
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
                if process_completion_unconfirmed {
                    return Err(err).with_context(|| {
                        format!("tokmd {} completion remains unconfirmed", command.label)
                    });
                }
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

    if process_spawned && !process_completion_unconfirmed {
        observe_process(SensorProcessObservation::Finished(if timed_out {
            TaskTerminalDisposition::TimedOut
        } else if failures.is_empty() {
            TaskTerminalDisposition::Succeeded
        } else {
            TaskTerminalDisposition::DeterministicFailure
        }));
    }
    let duration_ms = started.elapsed().as_millis();
    let context_path = dir.join("context.md");
    if !context_path.exists() {
        fs::write(
            &context_path,
            "No existing changed paths were available for bounded tokmd context.\n",
        )?;
    }

    let (status, reason) = if failures.is_empty() {
        ("ok", format!("{} tokmd receipts completed", commands.len()))
    } else if timed_out {
        (
            "timed_out",
            format!(
                "tokmd subcommands timed out or failed: {}",
                failures.join("; ")
            ),
        )
    } else {
        (
            "failed",
            format!("tokmd subcommands failed: {}", failures.join("; ")),
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

fn build_tokmd_version_preflight_command(dir: &Path, sensor: &SensorPlan) -> SensorSubcommand {
    SensorSubcommand {
        label: "version-preflight".to_owned(),
        argv: vec![sensor.command.clone(), "--version".to_owned()],
        stdout_path: dir.join("version.stdout.txt"),
        stderr_path: dir.join("version.stderr.txt"),
    }
}

fn append_tokmd_subcommand_receipts(
    aggregate_stdout_path: &Path,
    aggregate_stderr_path: &Path,
    command: &SensorSubcommand,
    result: &CommandStatus,
) -> Result<()> {
    append_file(
        aggregate_stdout_path,
        &format!(
            "$ {}\nstatus={} duration_ms={}\n\n",
            display_command(&command.argv),
            result.reason,
            result.duration_ms
        ),
    )?;
    append_existing_file(aggregate_stdout_path, &command.stdout_path)?;
    append_file(aggregate_stdout_path, "\n")?;
    append_existing_file(aggregate_stderr_path, &command.stderr_path)?;
    Ok(())
}

fn tokmd_version_preflight_failure_reason(
    result: &CommandStatus,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Option<String> {
    let expected = expected_standard_image_tool_version("tokmd")?;
    let fix = doctor_tool_version_fix("tokmd", expected);
    if !result.success {
        return Some(format!(
            "tokmd version preflight failed: {}; pin requires {} ({} preset); fix: {}",
            result.reason, expected, TOKMD_ANALYZE_PRESET, fix
        ));
    }
    let actual = first_non_empty_line_from_file(stdout_path)
        .or_else(|| first_non_empty_line_from_file(stderr_path));
    match actual {
        Some(actual) if command_version_matches(&actual, expected) => None,
        Some(actual) => Some(format!(
            "tokmd version drift: {} installed, pin requires {} ({} preset); fix: {}",
            actual, expected, TOKMD_ANALYZE_PRESET, fix
        )),
        None => Some(format!(
            "tokmd version preflight produced no --version output; pin requires {} ({} preset); fix: {}",
            expected, TOKMD_ANALYZE_PRESET, fix
        )),
    }
}

fn first_non_empty_line_from_file(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()?
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(160).collect())
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
        "phase": sensor.phase.key(),
    });
    if let Some(gate) = &sensor.gate {
        value["gate"] = serde_json::to_value(gate)?;
    }
    // Write via temp + rename so a status receipt is never observable
    // half-written: under the pipelined scheduler (#325) late sensors write
    // receipts while the main thread may concurrently read them for renders.
    let status_path = dir.join("ub-review-sensor-status.json");
    let tmp_path = dir.join("ub-review-sensor-status.json.tmp");
    fs::write(&tmp_path, serde_json::to_vec_pretty(&value)?)?;
    fs::rename(&tmp_path, &status_path)
        .with_context(|| format!("publish sensor status {}", status_path.display()))?;
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
            "version.stdout.txt".to_owned(),
            "version.stderr.txt".to_owned(),
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
    use std::path::{Path, PathBuf};

    use anyhow::{Context as _, Result, ensure};

    use super::{partition_sensor_queues, sensor_pool_worker_count};
    use crate::task_ledger::TaskTerminalDisposition;
    use crate::tests::{run_test_command, sensor_plan, sleeper_argv, test_diff, test_plan};
    use crate::*;

    #[test]
    fn sensor_pool_routes_shared_cargo_targets_to_one_ordered_queue() -> Result<()> {
        let sensors = VecDeque::from([
            sensor_plan("cargo-check", "cargo", true),
            sensor_plan("ripr", "ripr", true),
            sensor_plan("cargo-test", "custom-wrapper", true),
            sensor_plan("cargo-allow", "cargo-allow", true),
            sensor_plan("cargo-clippy", "C:\\Rust\\bin\\cargo.exe", true),
            sensor_plan("custom-cargo", "/opt/rust/bin/cargo", true),
        ]);

        let (cargo, parallel) = partition_sensor_queues(sensors);
        let cargo_ids = cargo
            .iter()
            .map(|sensor| sensor.id.as_str())
            .collect::<Vec<_>>();
        let parallel_ids = parallel
            .iter()
            .map(|sensor| sensor.id.as_str())
            .collect::<Vec<_>>();

        ensure!(
            cargo_ids == ["cargo-check", "cargo-test", "cargo-clippy", "custom-cargo"],
            "shared Cargo-target sensor order changed: {cargo_ids:?}"
        );
        ensure!(
            parallel_ids == ["ripr", "cargo-allow"],
            "independent sensor queue changed: {parallel_ids:?}"
        );
        let cargo_only = cargo.iter().cloned().collect::<Vec<_>>();
        ensure!(
            sensor_pool_worker_count(5, &cargo_only) == 1,
            "shared Cargo-target work must have exactly one owner"
        );
        let all = cargo.into_iter().chain(parallel).collect::<Vec<_>>();
        ensure!(
            sensor_pool_worker_count(5, &all) == 3,
            "one Cargo owner plus two independent workers should use three workers"
        );
        ensure!(
            sensor_pool_worker_count(1, &all) == 1,
            "the selected profile worker ceiling must remain authoritative"
        );
        ensure!(
            sensor_pool_worker_count(0, &all) == 0,
            "a zero worker budget must not admit a Cargo owner"
        );
        Ok(())
    }

    /// Subprocess budget for fixtures whose fake sensor command is a real
    /// spawned process. These tests assert receipt content, never latency, so
    /// the budget only has to be impossible to starve: the suite runs tests
    /// that shell out to real `cargo`/`git` for tens of seconds, and a 5s
    /// budget lost that race and produced a `timed_out` receipt instead of the
    /// receipt under test. A completing command never spends this budget.
    const FIXTURE_SENSOR_TIMEOUT_SEC: u64 = 120;

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
    fn baseline_gate_plan_phases_split_fast_signal_from_slow_suite() -> Result<()> {
        // #325: the self-gate plan must launch lanes off the non-compiling
        // fast signal while the slow suite (full check/test/clippy/doc,
        // leased coverage) defers to the late phase. cargo-fmt is class
        // `build` but pinned `phase = "fast"` in .ub-review.toml.
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
        let phase_of = |id: &str| -> Result<SensorPhase> {
            plan.sensors
                .iter()
                .find(|sensor| sensor.id == id)
                .map(|sensor| sensor.phase)
                .ok_or_else(|| anyhow::anyhow!("missing planned sensor {id}"))
        };
        for fast in [
            "cargo-fmt",
            "tokmd",
            "cargo-allow",
            "ripr",
            "unsafe-review",
            "ast-grep",
            "artifact-verifier",
        ] {
            assert_eq!(
                phase_of(fast)?,
                SensorPhase::Fast,
                "{fast} must be fast-phase signal"
            );
        }
        for late in [
            "cargo-check",
            "cargo-test",
            "cargo-clippy",
            "cargo-doc",
            "coverage",
        ] {
            assert_eq!(
                phase_of(late)?,
                SensorPhase::Late,
                "{late} must defer to the late phase"
            );
        }
        Ok(())
    }

    #[test]
    fn late_sensor_phase_spawn_and_join_write_receipts_and_events() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        let out = temp.path().join("out");
        fs::create_dir_all(&root)?;
        let event_log = std::sync::Arc::new(EventLog::open(&out.join("events.ndjson"))?);
        let run_started = Instant::now();
        let mut tracker = RunLoopTracker::new();
        let mut sensor = sensor_plan("late-probe", "ub-review-test-late-tool-missing", true);
        sensor.phase = SensorPhase::Late;
        sensor.timeout_sec = FIXTURE_SENSOR_TIMEOUT_SEC;
        let plan = test_plan(vec![sensor.clone()]);
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let task_ledger = SensorTaskLedger::initialize(&revision, &plan, false, &run_started)?;

        let phase = super::spawn_late_sensor_phase(
            &root,
            &out,
            &plan,
            &Profile::default(),
            &event_log,
            &run_started,
            LateSensorWork::new(vec![sensor], Some(task_ledger.clone())),
        )?;
        assert_eq!(phase.sensor_ids, vec!["late-probe".to_owned()]);
        phase.join(&event_log, &run_started, &mut tracker)?;

        // The receipt landed (missing command -> missing evidence, never
        // clean), stamped with its phase.
        let receipt: serde_json::Value = serde_json::from_slice(&fs::read(
            out.join("sensors/late-probe/ub-review-sensor-status.json"),
        )?)?;
        assert_eq!(receipt["status"], "missing");
        assert_eq!(receipt["phase"], "late");
        let events = fs::read_to_string(out.join("events.ndjson"))?;
        assert!(events.contains("late_sensor_phase_started"));
        assert!(events.contains("late_sensor_phase_joined"));
        assert!(
            events.contains("\"stage\":\"late-sensors\""),
            "late phase must open an evidence scheduler phase: {events}"
        );
        task_ledger.write_artifacts(&out)?;
        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/task_ledger_snapshot.json"))?)?;
        ensure!(
            snapshot
                .pointer("/tasks/0/state/ResourcesReleased")
                .and_then(serde_json::Value::as_str)
                == Some("SetupFailed")
        );
        ensure!(
            snapshot
                .pointer("/tasks/0/timing/process_started_at")
                .is_some_and(serde_json::Value::is_null)
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
    fn ripr_sensor_writes_badge_and_detail_receipts_from_configured_command() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        let out = temp.path().join("out");
        let fake_bin = temp.path().join("fake-bin");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(out.join("input"))?;
        fs::write(
            out.join("input/diff.patch"),
            "diff --git a/src/lib.rs b/src/lib.rs\n",
        )?;
        let fake_ripr = write_fake_ripr_command(&fake_bin)?;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let mut sensor = sensor_plan("ripr", &fake_ripr.display().to_string(), true);
        sensor.timeout_sec = FIXTURE_SENSOR_TIMEOUT_SEC;
        let plan = test_plan(vec![sensor.clone()]);
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let task_ledger = SensorTaskLedger::initialize(&revision, &plan, false, &Instant::now())?;

        super::run_sensor_with_task_ledger(&root, &out, &sensor, &event_log, &plan, &task_ledger)?;
        task_ledger.write_artifacts(&out)?;

        let sensor_dir = out.join("sensors/ripr");
        let status: serde_json::Value =
            serde_json::from_slice(&fs::read(sensor_dir.join("ub-review-sensor-status.json"))?)?;
        assert_eq!(status["status"], "ok");
        assert!(
            status["command"]
                .as_str()
                .is_some_and(|command| command.contains(&fake_ripr.display().to_string())),
            "sensor command must use configured ripr path: {status}"
        );
        assert_eq!(
            fs::read(sensor_dir.join("gate-decision.json"))?,
            fs::read(sensor_dir.join("stdout.txt"))?,
            "gate-decision.json must be the verbatim badge-json stdout receipt"
        );

        let gate: serde_json::Value =
            serde_json::from_slice(&fs::read(sensor_dir.join("gate-decision.json"))?)?;
        assert_eq!(gate["counts"]["unsuppressed_exposure_gaps"], 1);
        assert_eq!(gate["counts"]["suppressed_exposure_gaps"], 1);
        let detail: serde_json::Value =
            serde_json::from_slice(&fs::read(sensor_dir.join("exposure-gaps.json"))?)?;
        assert_eq!(detail["schema"], "ub-review.ripr_exposure_gaps.v3");
        assert_eq!(
            detail["status"], "ok",
            "detail pass did not succeed: {detail}"
        );
        assert_eq!(detail["semantics"], "raw_pre_policy");
        assert_eq!(
            detail["raw_outputs"],
            serde_json::json!({
                "stdout": "sensors/ripr/exposure-gaps.ripr.stdout",
                "stderr": "sensors/ripr/exposure-gaps.ripr.stderr",
            })
        );
        assert_eq!(detail["total_raw_findings"], 2);
        assert_eq!(detail["total_raw_gap_findings"], 2);
        let entries = detail["entries"]
            .as_array()
            .context("ripr detail entries")?;
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries.len(),
            detail["total_raw_gap_findings"]
                .as_u64()
                .context("total raw gap findings")? as usize
        );
        assert!(detail.get("entry_cap").is_none());
        assert!(detail.get("truncated").is_none());
        assert_eq!(entries[0]["id"], "probe:src_lib_rs:12:call_deletion");
        assert_eq!(entries[0]["path"], "src/lib.rs");
        assert_eq!(entries[0]["range"]["start_line"], 12);
        assert_eq!(entries[0]["range"]["end_line"], 12);
        assert_eq!(entries[0]["exposure_gap_class"], "weakly_exposed");
        assert!(entries[0].get("suppression_state").is_none());
        assert!(entries[0].get("threshold_contribution").is_none());
        assert_eq!(
            entries[0]["artifact_pointer"],
            "sensors/ripr/exposure-gaps.json#/entries/0"
        );
        assert!(entries[1].get("suppression_state").is_none());
        assert!(entries[1].get("threshold_contribution").is_none());
        assert_eq!(
            entries[1]["artifact_pointer"],
            "sensors/ripr/exposure-gaps.json#/entries/1"
        );
        assert!(sensor_dir.join("exposure-gaps.ripr.stdout").is_file());
        assert!(sensor_dir.join("exposure-gaps.ripr.stderr").is_file());
        assert!(!sensor_dir.join("exposure-gaps.stdout.tmp").exists());
        assert!(!sensor_dir.join("exposure-gaps.stderr.tmp").exists());
        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/task_ledger_snapshot.json"))?)?;
        ensure!(
            snapshot
                .pointer("/tasks/0/state/ResourcesReleased")
                .and_then(serde_json::Value::as_str)
                == Some("Succeeded")
        );
        Ok(())
    }

    #[test]
    fn process_finish_observation_precedes_ripr_receipt_post_processing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        let out = temp.path().join("out");
        let fake_bin = temp.path().join("fake-bin");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(out.join("input"))?;
        fs::write(
            out.join("input/diff.patch"),
            "diff --git a/src/lib.rs b/src/lib.rs\n",
        )?;
        let fake_ripr = write_fake_ripr_command(&fake_bin)?;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let mut sensor = sensor_plan("ripr", &fake_ripr.display().to_string(), true);
        sensor.timeout_sec = FIXTURE_SENSOR_TIMEOUT_SEC;
        let plan = test_plan(vec![sensor.clone()]);
        let sensor_dir = out.join("sensors/ripr");
        let mut observations = Vec::new();
        let mut receipt_absent_at_finish = false;
        let mut disposition = None;
        let mut observe_process = |observation| match observation {
            super::SensorProcessObservation::AttemptPrepared => {}
            super::SensorProcessObservation::Spawned => observations.push("spawned"),
            super::SensorProcessObservation::Finished(observed) => {
                observations.push("finished");
                disposition = Some(observed);
                receipt_absent_at_finish = !sensor_dir.join("gate-decision.json").exists()
                    && !sensor_dir.join("ub-review-sensor-status.json").exists();
            }
            super::SensorProcessObservation::CompletionUnconfirmed => {
                observations.push("completion-unconfirmed");
            }
        };

        super::run_sensor_with_command_availability(
            &root,
            &out,
            &sensor,
            &event_log,
            &plan,
            true,
            &mut observe_process,
        )?;

        ensure!(observations == ["spawned", "finished"]);
        ensure!(disposition == Some(TaskTerminalDisposition::Succeeded));
        ensure!(receipt_absent_at_finish);
        ensure!(sensor_dir.join("gate-decision.json").is_file());
        ensure!(sensor_dir.join("ub-review-sensor-status.json").is_file());
        Ok(())
    }

    #[test]
    fn ripr_sensor_keeps_primary_receipts_when_detail_output_is_unavailable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        let out = temp.path().join("out");
        let fake_bin = temp.path().join("fake-bin");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(out.join("input"))?;
        fs::write(
            out.join("input/diff.patch"),
            "diff --git a/src/lib.rs b/src/lib.rs\n",
        )?;
        fs::write(root.join("ripr-detail-truncated"), "force malformed JSON")?;
        let fake_ripr = write_fake_ripr_command(&fake_bin)?;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let mut sensor = sensor_plan("ripr", &fake_ripr.display().to_string(), true);
        sensor.timeout_sec = FIXTURE_SENSOR_TIMEOUT_SEC;
        let plan = test_plan(vec![sensor.clone()]);

        run_sensor(&root, &out, &sensor, &event_log, &plan)?;

        let sensor_dir = out.join("sensors/ripr");
        let status: serde_json::Value =
            serde_json::from_slice(&fs::read(sensor_dir.join("ub-review-sensor-status.json"))?)?;
        assert_eq!(status["status"], "ok");
        assert_eq!(
            fs::read(sensor_dir.join("gate-decision.json"))?,
            fs::read(sensor_dir.join("stdout.txt"))?
        );
        let gate: serde_json::Value =
            serde_json::from_slice(&fs::read(sensor_dir.join("gate-decision.json"))?)?;
        assert_eq!(gate["counts"]["unsuppressed_exposure_gaps"], 1);
        assert_eq!(gate["counts"]["suppressed_exposure_gaps"], 1);
        let detail: serde_json::Value =
            serde_json::from_slice(&fs::read(sensor_dir.join("exposure-gaps.json"))?)?;
        assert_eq!(detail["schema"], "ub-review.ripr_exposure_gaps.v3");
        assert_eq!(detail["status"], "detail_unavailable");
        assert!(
            detail["error"]
                .as_str()
                .is_some_and(|error| error.contains("parse ripr --format json output"))
        );
        assert!(detail.get("total_raw_findings").is_none());
        assert!(detail.get("entries").is_none());
        assert!(!sensor_dir.join("exposure-gaps.stdout.tmp").exists());
        assert!(!sensor_dir.join("exposure-gaps.stderr.tmp").exists());
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
    fn tokmd_sensor_fails_fast_on_stale_installed_version() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        let out = temp.path().join("out");
        let fake_bin = temp.path().join("fake-bin");
        fs::create_dir_all(&root)?;
        let fake_tokmd = write_fake_tokmd_command(&fake_bin, "1.11.1", None)?;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let mut sensor = sensor_plan("tokmd", &fake_tokmd.display().to_string(), true);
        sensor.timeout_sec = FIXTURE_SENSOR_TIMEOUT_SEC;
        let plan = test_plan(vec![sensor.clone()]);
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let task_ledger = SensorTaskLedger::initialize(&revision, &plan, false, &Instant::now())?;

        super::run_sensor_with_task_ledger(&root, &out, &sensor, &event_log, &plan, &task_ledger)?;
        task_ledger.write_artifacts(&out)?;

        let sensor_dir = out.join("sensors/tokmd");
        let status: serde_json::Value =
            serde_json::from_slice(&fs::read(sensor_dir.join("ub-review-sensor-status.json"))?)?;
        assert_eq!(status["status"], "failed");
        let reason = status["reason"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("status reason missing"))?;
        assert!(
            reason.contains("tokmd version drift"),
            "unexpected tokmd failure reason: {reason}"
        );
        assert!(reason.contains("tokmd 1.11.1 installed"));
        assert!(reason.contains("pin requires 1.12.0 (bun-ub preset)"));
        assert!(reason.contains("cargo install tokmd --locked --version 1.12.0 --force"));
        let commands: serde_json::Value =
            serde_json::from_slice(&fs::read(sensor_dir.join("commands.json"))?)?;
        let commands = commands
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("tokmd commands.json is not an array"))?;
        assert_eq!(
            commands.first().and_then(|value| value["label"].as_str()),
            Some("version-preflight")
        );
        assert!(sensor_dir.join("version.stdout.txt").is_file());
        assert!(
            !sensor_dir.join("analyze.md").exists(),
            "stale tokmd must fail before preset-bearing analyze commands run"
        );
        let aggregate_stdout = fs::read_to_string(sensor_dir.join("stdout.txt"))?;
        assert!(aggregate_stdout.contains("--version"));
        assert!(aggregate_stdout.contains("tokmd 1.11.1"));
        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/task_ledger_snapshot.json"))?)?;
        ensure!(
            snapshot
                .pointer("/tasks/0/state/ResourcesReleased")
                .and_then(serde_json::Value::as_str)
                == Some("DeterministicFailure")
        );
        Ok(())
    }

    #[test]
    fn tokmd_post_spawn_error_still_terminalizes_the_task() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        let out = temp.path().join("out");
        let fake_bin = temp.path().join("fake-bin");
        let aggregate_stdout = out.join("sensors/tokmd/stdout.txt");
        fs::create_dir_all(&root)?;
        let fake_tokmd = write_fake_tokmd_command(
            &fake_bin,
            STANDARD_IMAGE_TOKMD_VERSION,
            Some(&aggregate_stdout),
        )?;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let mut sensor = sensor_plan("tokmd", &fake_tokmd.display().to_string(), true);
        sensor.timeout_sec = FIXTURE_SENSOR_TIMEOUT_SEC;
        let plan = test_plan(vec![sensor.clone()]);
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let task_ledger = SensorTaskLedger::initialize(&revision, &plan, false, &Instant::now())?;

        let result = super::run_sensor_with_task_ledger(
            &root,
            &out,
            &sensor,
            &event_log,
            &plan,
            &task_ledger,
        );
        ensure!(result.is_err());
        task_ledger.write_artifacts(&out)?;

        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/task_ledger_snapshot.json"))?)?;
        ensure!(
            snapshot
                .pointer("/tasks/0/state/ResourcesReleased")
                .and_then(serde_json::Value::as_str)
                == Some("DeterministicFailure")
        );
        ensure!(
            snapshot
                .pointer("/tasks/0/receipt/CreationFailed/reason")
                .and_then(serde_json::Value::as_str)
                == Some("sensor status receipt missing or invalid")
        );
        Ok(())
    }

    #[test]
    fn tokmd_unconfirmed_subcommand_completion_stops_later_commands() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        let out = temp.path().join("out");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(out.join("sensors/tokmd"))?;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let mut sensor = sensor_plan("tokmd", "tokmd", true);
        sensor.timeout_sec = FIXTURE_SENSOR_TIMEOUT_SEC;
        let plan = test_plan(vec![sensor.clone()]);
        let aggregate_argv = vec!["tokmd".to_owned()];
        let mut command_calls = 0_usize;
        let mut run_command =
            |_root: &Path,
             _argv: &[String],
             _env: &BTreeMap<String, String>,
             _timeout_sec: u64,
             stdout_path: &Path,
             stderr_path: &Path,
             observe_command: &mut dyn FnMut(CommandProcessObservation)| {
                command_calls += 1;
                fs::write(stderr_path, b"")?;
                observe_command(CommandProcessObservation::Spawned);
                if command_calls == 1 {
                    fs::write(
                        stdout_path,
                        format!("tokmd {STANDARD_IMAGE_TOKMD_VERSION}\n"),
                    )?;
                    return Ok(CommandStatus {
                        exit_code: Some(0),
                        timed_out: false,
                        success: true,
                        reason: "completed".to_owned(),
                        duration_ms: 1,
                    });
                }
                observe_command(CommandProcessObservation::CompletionUnconfirmed);
                Err(anyhow::anyhow!("injected unconfirmed completion"))
            };

        let result = super::run_tokmd_sensor_with_command_runner(
            super::TokmdSensorRun {
                root: &root,
                out: &out,
                sensor: &sensor,
                event_log: &event_log,
                plan: &plan,
                aggregate_argv: &aggregate_argv,
                observe_process: &mut |_| {},
            },
            &mut run_command,
        );

        ensure!(result.is_err());
        ensure!(command_calls == 2);
        let planned_commands: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("sensors/tokmd/commands.json"))?)?;
        ensure!(
            planned_commands
                .as_array()
                .is_some_and(|commands| commands.len() > command_calls)
        );
        let aggregate_stdout = fs::read_to_string(out.join("sensors/tokmd/stdout.txt"))?;
        ensure!(
            aggregate_stdout
                .lines()
                .filter(|line| line.starts_with("$ "))
                .count()
                == 1
        );
        ensure!(
            !out.join("sensors/tokmd/ub-review-sensor-status.json")
                .exists()
        );
        Ok(())
    }

    fn write_fake_tokmd_command(
        dir: &Path,
        version: &str,
        aggregate_stdout_failure: Option<&Path>,
    ) -> Result<PathBuf> {
        fs::create_dir_all(dir)?;
        #[cfg(windows)]
        {
            let source = dir.join("fake_tokmd.rs");
            let sabotage = aggregate_stdout_failure.map_or_else(String::new, |path| {
                format!(
                    "let path = {path:?}; let _ = std::fs::remove_file(path); let _ = std::fs::create_dir(path);",
                    path = path.display().to_string()
                )
            });
            fs::write(
                &source,
                format!(
                    r#"use std::env;

const VERSION: &str = {version:?};

fn main() {{
    let args = env::args().skip(1).collect::<Vec<_>>();
    if args == ["--version"] {{
        println!("tokmd {{VERSION}}");
        {sabotage}
        return;
    }}
    eprintln!("unexpected tokmd subcommand: {{}}", args.join(" "));
    std::process::exit(42);
}}
"#
                ),
            )?;
            let exe = dir.join("fake_tokmd.exe");
            let source_arg = source.display().to_string();
            let exe_arg = exe.display().to_string();
            run_test_command(dir, "rustc", &[source_arg.as_str(), "-o", exe_arg.as_str()])?;
            Ok(exe)
        }
        #[cfg(not(windows))]
        {
            let script = dir.join("fake_tokmd");
            let sabotage = aggregate_stdout_failure.map_or_else(String::new, |path| {
                format!(
                    "rm -f -- {path:?}\nmkdir -- {path:?}\n",
                    path = path.display().to_string()
                )
            });
            fs::write(
                &script,
                format!(
                    "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo \"tokmd {version}\"\n{sabotage}exit 0; fi\necho \"unexpected tokmd subcommand: $*\" >&2\nexit 42\n"
                ),
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                let mut permissions = fs::metadata(&script)?.permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&script, permissions)?;
            }
            Ok(script)
        }
    }

    fn write_fake_ripr_command(dir: &Path) -> Result<PathBuf> {
        fs::create_dir_all(dir)?;
        #[cfg(windows)]
        {
            let source = dir.join("fake_ripr.rs");
            fs::write(
                &source,
                r####"use std::env;
use std::path::Path;

const BADGE: &str = r###"{"schema_version":"0.6","kind":"ripr","scope":"diff","basis":"finding_exposure","label":"ripr","message":"1","status":"warn","color":"yellow","counts":{"unsuppressed_exposure_gaps":1,"suppressed_exposure_gaps":1,"unsuppressed_test_efficiency_findings":0,"analyzed_findings":2},"policy":{"include_unknowns":false,"fail_on_nonzero":false},"warnings":[],"preview_skipped":[]}"###;
const DETAIL: &str = r###"{"schema_version":"0.2","tool":"ripr","mode":"ready","summary":{"findings":2},"findings":[{"id":"probe:src_lib_rs:12:call_deletion","classification":"weakly_exposed","probe":{"family":"call_deletion","file":"src/lib.rs","line":12,"expression":"crate::value()"},"ripr":{"reach":{"summary":"test reaches changed owner"},"discriminate":{"summary":"oracle does not distinguish behavior"}}},{"id":"probe:src_lib_rs:18:error_path","classification":"reachable_unrevealed","probe":{"family":"error_path","file":"src/lib.rs","line":18,"expression":"crate::fallible()"},"ripr":{"reach":{"summary":"raw reach gap"},"discriminate":{"summary":"raw oracle gap"}}}]}"###;

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    let format = args
        .windows(2)
        .find(|pair| pair[0] == "--format")
        .map(|pair| pair[1].as_str());
    match format {
        Some("badge-json") => println!("{BADGE}"),
        Some("json") if Path::new("ripr-detail-truncated").is_file() => print!("{{"),
        Some("json") => println!("{DETAIL}"),
        other => {
            eprintln!("unexpected ripr format: {other:?}; args={args:?}");
            std::process::exit(42);
        }
    }
}
"####,
            )?;
            let exe = dir.join("fake_ripr.exe");
            let source_arg = source.display().to_string();
            let exe_arg = exe.display().to_string();
            run_test_command(dir, "rustc", &[source_arg.as_str(), "-o", exe_arg.as_str()])?;
            Ok(exe)
        }
        #[cfg(not(windows))]
        {
            let script = dir.join("fake_ripr");
            fs::write(
                &script,
                r####"#!/bin/sh
case "$*" in
  *"--format badge-json"*)
    printf '%s\n' '{"schema_version":"0.6","kind":"ripr","scope":"diff","basis":"finding_exposure","label":"ripr","message":"1","status":"warn","color":"yellow","counts":{"unsuppressed_exposure_gaps":1,"suppressed_exposure_gaps":1,"unsuppressed_test_efficiency_findings":0,"analyzed_findings":2},"policy":{"include_unknowns":false,"fail_on_nonzero":false},"warnings":[],"preview_skipped":[]}'
    ;;
  *"--format json"*)
    if [ -f ripr-detail-truncated ]; then
      printf '{'
    else
      printf '%s\n' '{"schema_version":"0.2","tool":"ripr","mode":"ready","summary":{"findings":2},"findings":[{"id":"probe:src_lib_rs:12:call_deletion","classification":"weakly_exposed","probe":{"family":"call_deletion","file":"src/lib.rs","line":12,"expression":"crate::value()"},"ripr":{"reach":{"summary":"test reaches changed owner"},"discriminate":{"summary":"oracle does not distinguish behavior"}}},{"id":"probe:src_lib_rs:18:error_path","classification":"reachable_unrevealed","probe":{"family":"error_path","file":"src/lib.rs","line":18,"expression":"crate::fallible()"},"ripr":{"reach":{"summary":"raw reach gap"},"discriminate":{"summary":"raw oracle gap"}}}]}'
    fi
    ;;
  *)
    echo "unexpected ripr args: $*" >&2
    exit 42
    ;;
esac
"####,
            )?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                let mut permissions = fs::metadata(&script)?.permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&script, permissions)?;
            }
            Ok(script)
        }
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
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let task_ledger = SensorTaskLedger::initialize(&revision, &plan, false, &Instant::now())?;

        super::run_sensor_with_task_ledger(root, &out, &sensor, &event_log, &plan, &task_ledger)?;
        task_ledger.write_artifacts(&out)?;

        let status_path = out.join("sensors/ripr/ub-review-sensor-status.json");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(status_path)?)?;
        assert_eq!(value["sensor"], "ripr");
        assert_eq!(value["status"], "missing");
        assert_eq!(value["reason"], "command not found");
        assert!(out.join("sensors/ripr/stdout.txt").exists());
        assert!(out.join("sensors/ripr/stderr.txt").exists());
        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/task_ledger_snapshot.json"))?)?;
        ensure!(
            snapshot
                .pointer("/tasks/0/state/ResourcesReleased")
                .and_then(serde_json::Value::as_str)
                == Some("SetupFailed")
        );
        Ok(())
    }

    #[test]
    fn failed_attempt_cannot_reuse_a_prior_sensor_status_receipt() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        let out = temp.path().join("out");
        let fake_bin = temp.path().join("fake-bin");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(out.join("input"))?;
        fs::write(
            out.join("input/diff.patch"),
            "diff --git a/src/lib.rs b/src/lib.rs\n",
        )?;
        let fake_ripr = write_fake_ripr_command(&fake_bin)?;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let mut sensor = sensor_plan("ripr", &fake_ripr.display().to_string(), true);
        sensor.timeout_sec = FIXTURE_SENSOR_TIMEOUT_SEC;
        let plan = test_plan(vec![sensor.clone()]);
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let task_ledger = SensorTaskLedger::initialize(&revision, &plan, false, &Instant::now())?;
        let sensor_dir = out.join("sensors/ripr");
        fs::create_dir_all(&sensor_dir)?;
        fs::write(
            sensor_dir.join("ub-review-sensor-status.json"),
            serde_json::to_vec(&serde_json::json!({"sensor": "ripr", "status": "ok"}))?,
        )?;
        // A directory at the receipt target forces the post-process copy to
        // fail after the fake subprocess has started but before status publish.
        fs::create_dir_all(sensor_dir.join("gate-decision.json"))?;

        let result = super::run_sensor_with_task_ledger(
            &root,
            &out,
            &sensor,
            &event_log,
            &plan,
            &task_ledger,
        );
        ensure!(result.is_err());
        ensure!(!sensor_dir.join("ub-review-sensor-status.json").exists());
        task_ledger.write_artifacts(&out)?;

        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/task_ledger_snapshot.json"))?)?;
        ensure!(
            snapshot
                .pointer("/tasks/0/state/ResourcesReleased")
                .and_then(serde_json::Value::as_str)
                == Some("Succeeded")
        );
        ensure!(
            snapshot
                .pointer("/tasks/0/receipt/CreationFailed/reason")
                .and_then(serde_json::Value::as_str)
                == Some("sensor status receipt missing or invalid")
        );
        let events = fs::read_to_string(out.join("task_ledger_events.ndjson"))?;
        let process_finished = events
            .find("\"ProcessFinished\"")
            .context("task ledger has no process-finished event")?;
        let cleanup_finished = events
            .find("\"CleanupFinished\"")
            .context("task ledger has no cleanup-finished event")?;
        let receipt_failed = events
            .find("\"ReceiptCreationFailed\"")
            .context("task ledger has no receipt-failed event")?;
        let resources_released = events
            .find("\"ResourcesReleased\"")
            .context("task ledger has no resources-released event")?;
        ensure!(process_finished < cleanup_finished);
        ensure!(cleanup_finished < receipt_failed);
        ensure!(receipt_failed < resources_released);
        Ok(())
    }

    #[test]
    fn stale_status_removal_failure_cannot_create_a_receipt() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repo");
        let out = temp.path().join("out");
        fs::create_dir_all(&root)?;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let sensor = sensor_plan("probe", "probe", true);
        let plan = test_plan(vec![sensor.clone()]);
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let task_ledger = SensorTaskLedger::initialize(&revision, &plan, false, &Instant::now())?;
        let stale_status = out.join("sensors/probe/ub-review-sensor-status.json");
        fs::create_dir_all(
            stale_status
                .parent()
                .context("stale status fixture has no parent")?,
        )?;
        fs::write(
            &stale_status,
            serde_json::to_vec(&serde_json::json!({"sensor": "probe", "status": "ok"}))?,
        )?;
        let mut reject_removal = |_dir: &Path| Err(anyhow::anyhow!("injected removal failure"));

        let result = super::run_sensor_attempt_with_task_ledger(
            super::SensorCommandAttempt {
                root: &root,
                out: &out,
                sensor: &sensor,
                event_log: &event_log,
                plan: &plan,
                command_available: true,
            },
            &task_ledger,
            &mut reject_removal,
        );
        ensure!(result.is_err());
        let stale_value: serde_json::Value = serde_json::from_slice(&fs::read(&stale_status)?)?;
        ensure!(stale_value["status"] == "ok");
        task_ledger.write_artifacts(&out)?;

        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/task_ledger_snapshot.json"))?)?;
        ensure!(
            snapshot
                .pointer("/tasks/0/receipt/CreationFailed/reason")
                .and_then(serde_json::Value::as_str)
                == Some("sensor status receipt missing or invalid")
        );
        ensure!(
            snapshot
                .pointer("/tasks/0/state/ResourcesReleased")
                .and_then(serde_json::Value::as_str)
                == Some("SetupFailed")
        );
        ensure!(
            snapshot
                .pointer("/tasks/0/timing/process_started_at")
                .is_some_and(serde_json::Value::is_null)
        );
        let events = fs::read_to_string(out.join("task_ledger_events.ndjson"))?;
        ensure!(!events.contains("\"ProcessStarted\""));
        ensure!(!events.contains("\"CleanupFinished\""));
        ensure!(!events.contains("\"ReceiptCreated\""));
        ensure!(!events.contains("\"Succeeded\""));
        Ok(())
    }

    #[test]
    fn sensor_task_receipt_mapping_rejects_missing_mismatched_and_unknown_status() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let sensor = sensor_plan("probe", "probe", true);
        let status_path = out.join("sensors/probe/ub-review-sensor-status.json");
        let status_dir = status_path
            .parent()
            .context("sensor status fixture has no parent")?;
        fs::create_dir_all(status_dir)?;

        ensure!(super::sensor_task_receipt_disposition(&out, &sensor, true).is_err());
        fs::write(
            &status_path,
            serde_json::to_vec(&serde_json::json!({
                "sensor": "another-probe",
                "status": "ok"
            }))?,
        )?;
        ensure!(super::sensor_task_receipt_disposition(&out, &sensor, true).is_err());

        for (status, expected) in [
            ("ok", TaskTerminalDisposition::Succeeded),
            ("failed", TaskTerminalDisposition::DeterministicFailure),
            ("timed_out", TaskTerminalDisposition::TimedOut),
        ] {
            fs::write(
                &status_path,
                serde_json::to_vec(&serde_json::json!({
                    "sensor": "probe",
                    "status": status
                }))?,
            )?;
            ensure!(super::sensor_task_receipt_disposition(&out, &sensor, true)? == expected);
        }
        fs::write(
            &status_path,
            serde_json::to_vec(&serde_json::json!({
                "sensor": "probe",
                "status": "future-status"
            }))?,
        )?;
        ensure!(super::sensor_task_receipt_disposition(&out, &sensor, true).is_err());
        Ok(())
    }

    #[test]
    fn sensor_spawn_setup_failure_records_no_process_timing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("root");
        let out = temp.path().join("out");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(out.join("sensors/probe/stdout.txt"))?;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let sensor = sensor_plan("probe", "probe", true);
        let plan = test_plan(vec![sensor.clone()]);
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let task_ledger = SensorTaskLedger::initialize(&revision, &plan, false, &Instant::now())?;

        super::run_sensor_with_task_ledger_and_command_availability(
            &root,
            &out,
            &sensor,
            &event_log,
            &plan,
            &task_ledger,
            true,
        )?;
        task_ledger.write_artifacts(&out)?;

        let snapshot: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/task_ledger_snapshot.json"))?)?;
        ensure!(
            snapshot
                .pointer("/tasks/0/state/ResourcesReleased")
                .and_then(serde_json::Value::as_str)
                == Some("SetupFailed")
        );
        ensure!(
            snapshot
                .pointer("/tasks/0/timing/process_started_at")
                .is_some_and(serde_json::Value::is_null)
        );
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

        let status = run_sensor_command_to_files(
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
    fn unsafe_review_exit_one_is_completed_policy_signal() {
        let result = command_status(Some(1), false, false, "exit code Some(1)");

        let (status, reason) = super::classify_sensor_command_result("unsafe-review", &result);

        assert_eq!(status, "ok");
        assert_eq!(
            reason,
            "unsafe-review completed with policy findings (exit 1)"
        );
    }

    #[test]
    fn unsafe_review_exit_two_remains_tool_failure() {
        let result = command_status(Some(2), false, false, "exit code Some(2)");

        let (status, reason) = super::classify_sensor_command_result("unsafe-review", &result);

        assert_eq!(status, "failed");
        assert_eq!(reason, "exit code Some(2)");
    }

    #[test]
    fn non_unsafe_review_exit_one_stays_failed() {
        let result = command_status(Some(1), false, false, "exit code Some(1)");

        let (status, reason) = super::classify_sensor_command_result("ripr", &result);

        assert_eq!(status, "failed");
        assert_eq!(reason, "exit code Some(1)");
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
        assert!(
            !argv.iter().any(|arg| arg == "--out"),
            "--out must not be used for unsafe-review first-pr: {argv:?}"
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

    fn command_status(
        exit_code: Option<i32>,
        timed_out: bool,
        success: bool,
        reason: &str,
    ) -> CommandStatus {
        CommandStatus {
            exit_code,
            timed_out,
            success,
            reason: reason.to_owned(),
            duration_ms: 1,
        }
    }
}
