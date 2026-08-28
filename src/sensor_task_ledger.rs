//! Shadow TaskLedger adapter for planned and executed sensors (#955).
//!
//! This module observes the existing sensor scheduler. It does not choose,
//! reorder, deduplicate, admit, or execute work. The caller-owned monotonic
//! epoch and the existing status receipt remain the execution authorities.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::{Context, Result, ensure};

use crate::task_ledger::{
    MonotonicInstant, ResourceReservation, TaskConsumer, TaskEvent, TaskExecutionLimits, TaskId,
    TaskNonExecutionDisposition, TaskRequirement, TaskResourceClass, TaskSource,
    TaskTerminalDisposition, TaskValueClass,
};
use crate::task_ledger_artifact::{
    TaskLedgerInput, remove_task_ledger_artifacts, write_task_ledger_artifacts,
};
use crate::{Plan, RevisionRef, SensorPlan, work_queue_sensor_consumers};

// tokmd runs one version preflight, four always-on report commands, and at
// most one changed-path context command. Each subprocess receives the sensor
// timeout independently, so admission must reserve the aggregate ceiling.
const TOKMD_MAX_SUBPROCESS_COUNT: u64 = 6;

/// Run-owned recorder shared by source-specific TaskLedger adapters.
///
/// Adapters own proposal identity and lifecycle mapping. This type owns only
/// the admitted revision, caller-relative monotonic epoch, ordered append-only
/// inputs, and replay-verified artifact publication.
#[derive(Clone)]
pub(crate) struct TaskLedgerRecorder {
    inner: Arc<TaskLedgerRecorderInner>,
}

struct TaskLedgerRecorderInner {
    revision: RevisionRef,
    run_started: Instant,
    inputs: Mutex<Vec<TaskLedgerInput>>,
}

impl TaskLedgerRecorder {
    pub(crate) fn new(revision: &RevisionRef, run_started: &Instant) -> Result<Self> {
        revision
            .validate()
            .context("task-ledger recorder revision")?;
        Ok(Self {
            inner: Arc::new(TaskLedgerRecorderInner {
                revision: revision.clone(),
                run_started: *run_started,
                inputs: Mutex::new(Vec::new()),
            }),
        })
    }

    /// Return the immutable revision admitted for every adapter sharing this recorder.
    pub(crate) fn revision(&self) -> &RevisionRef {
        &self.inner.revision
    }

    pub(crate) fn now(&self) -> Result<MonotonicInstant> {
        let elapsed = u64::try_from(self.inner.run_started.elapsed().as_millis())
            .context("task-ledger recorder monotonic time exceeds u64")?;
        Ok(MonotonicInstant::from_millis(elapsed))
    }

    pub(crate) fn append(&self, events: impl IntoIterator<Item = TaskLedgerInput>) -> Result<()> {
        self.inner
            .inputs
            .lock()
            .map_err(|_| anyhow::anyhow!("task-ledger recorder mutex poisoned"))?
            .extend(events);
        Ok(())
    }

    /// Append one event without making adapters rebuild the input envelope.
    pub(crate) fn append_event(&self, task_id: &TaskId, event: TaskEvent) -> Result<()> {
        self.append([TaskLedgerInput {
            task_id: task_id.clone(),
            event,
        }])
    }

    pub(crate) fn write_artifacts(&self, out: &std::path::Path) -> Result<()> {
        let inputs = self
            .inner
            .inputs
            .lock()
            .map_err(|_| anyhow::anyhow!("task-ledger recorder mutex poisoned"))?
            .clone();
        if inputs.is_empty() {
            return remove_task_ledger_artifacts(out);
        }
        write_task_ledger_artifacts(out, &self.inner.revision, &inputs)
    }

    #[cfg(test)]
    pub(crate) fn inputs(&self) -> Result<Vec<TaskLedgerInput>> {
        Ok(self
            .inner
            .inputs
            .lock()
            .map_err(|_| anyhow::anyhow!("task-ledger recorder mutex poisoned"))?
            .clone())
    }
}

#[derive(Clone)]
pub(crate) struct SensorTaskLedger {
    recorder: TaskLedgerRecorder,
}

impl SensorTaskLedger {
    /// Propose every planned sensor in stable plan order. Runnable sensors
    /// enter the existing worker queue; skipped and dry-run sensors terminate
    /// without fabricated admission, setup, or process events.
    pub(crate) fn initialize(
        revision: &RevisionRef,
        plan: &Plan,
        dry_run: bool,
        run_started: &Instant,
    ) -> Result<Self> {
        revision.validate().context("sensor task-ledger revision")?;
        let ledger = Self {
            recorder: TaskLedgerRecorder::new(revision, run_started)?,
        };
        let queued_at = ledger.now()?;
        let mut initial = Vec::new();
        for sensor in &plan.sensors {
            let task_id = sensor_task_id(sensor)?;
            initial.push(input(
                &task_id,
                TaskEvent::Proposed {
                    revision: revision.clone(),
                    source: TaskSource::Sensor,
                    limits: TaskExecutionLimits::new(sensor_timeout_ms(sensor)?)?,
                },
            ));
            for consumer_id in work_queue_sensor_consumers(&sensor.id) {
                initial.push(input(
                    &task_id,
                    TaskEvent::ConsumerAttached {
                        consumer: sensor_consumer(sensor, &consumer_id)?,
                    },
                ));
            }
            if dry_run || !sensor.run {
                let reason = if dry_run && sensor.run {
                    "dry-run; sensor not executed"
                } else {
                    sensor.reason.as_str()
                };
                initial.push(input(
                    &task_id,
                    TaskEvent::TerminallyDeclined {
                        at: queued_at,
                        disposition: TaskNonExecutionDisposition::Refused,
                        reason: canonical_reason(reason)?,
                        existing_receipt: None,
                    },
                ));
            } else {
                initial.push(input(&task_id, TaskEvent::Selected));
                initial.push(input(&task_id, TaskEvent::Queued { at: queued_at }));
                initial.push(input(
                    &task_id,
                    TaskEvent::EnteredResourceWait { at: queued_at },
                ));
            }
        }
        ledger.append(initial)?;
        Ok(ledger)
    }

    /// Return the run-owned recorder so another source adapter can append to
    /// the exact same deterministic stream.
    pub(crate) fn recorder(&self) -> TaskLedgerRecorder {
        self.recorder.clone()
    }

    /// Observe the existing worker admitting a queued sensor and beginning
    /// command setup. Resource rows mirror the current work-queue lease.
    pub(crate) fn setup_started(&self, sensor: &SensorPlan) -> Result<()> {
        let task_id = sensor_task_id(sensor)?;
        let at = self.now()?;
        let mut reservations = vec![ResourceReservation::new(TaskResourceClass::Cpu, 1)?];
        if sensor.artifact_budget_mb > 0 {
            reservations.push(ResourceReservation::new(
                TaskResourceClass::Disk,
                sensor.artifact_budget_mb,
            )?);
        }
        self.append([
            input(&task_id, TaskEvent::Admitted { at, reservations }),
            input(&task_id, TaskEvent::SetupStarted { at }),
        ])
    }

    /// Observe the exact boundary after setup accepted the command and before
    /// the existing runner may spawn it.
    pub(crate) fn run_started(&self, sensor: &SensorPlan) -> Result<()> {
        let task_id = sensor_task_id(sensor)?;
        let at = self.now()?;
        self.append([input(&task_id, TaskEvent::RunStarted { at })])
    }

    /// Terminalize a setup refusal while still attempting and accounting for
    /// the status receipt and resource release.
    pub(crate) fn setup_failed(&self, sensor: &SensorPlan, receipt_created: bool) -> Result<()> {
        let task_id = sensor_task_id(sensor)?;
        let at = self.now()?;
        let mut events = vec![input(&task_id, TaskEvent::SetupFailed { at })];
        events.push(receipt_event(sensor, at, receipt_created)?);
        events.push(input(&task_id, TaskEvent::ResourcesReleased { at }));
        self.append(events)
    }

    /// Observe the process boundary before sensor post-processing and receipt
    /// publication begin.
    pub(crate) fn process_finished(
        &self,
        sensor: &SensorPlan,
        disposition: TaskTerminalDisposition,
    ) -> Result<()> {
        ensure!(
            disposition != TaskTerminalDisposition::SetupFailed,
            "setup failure must use SensorTaskLedger::setup_failed"
        );
        let task_id = sensor_task_id(sensor)?;
        let at = self.now()?;
        self.append([input(
            &task_id,
            TaskEvent::ProcessFinished { at, disposition },
        )])
    }

    /// Observe sensor post-processing completion before receipt validation.
    pub(crate) fn cleanup_finished(&self, sensor: &SensorPlan) -> Result<()> {
        let task_id = sensor_task_id(sensor)?;
        let at = self.now()?;
        self.append([input(&task_id, TaskEvent::CleanupFinished { at })])
    }

    /// Record the current-attempt receipt result, then release the existing
    /// worker reservation.
    pub(crate) fn receipt_recorded_and_resources_released(
        &self,
        sensor: &SensorPlan,
        receipt_created: bool,
    ) -> Result<()> {
        let task_id = sensor_task_id(sensor)?;
        let receipt_at = self.now()?;
        let released_at = self.now()?;
        self.append([
            receipt_event(sensor, receipt_at, receipt_created)?,
            input(&task_id, TaskEvent::ResourcesReleased { at: released_at }),
        ])
    }

    /// Persist the complete sensor shadow stream only after every late sensor
    /// has joined. The shared recorder replays before publishing.
    pub(crate) fn write_artifacts(&self, out: &std::path::Path) -> Result<()> {
        self.recorder.write_artifacts(out)
    }

    fn now(&self) -> Result<MonotonicInstant> {
        self.recorder.now()
    }

    fn append(&self, events: impl IntoIterator<Item = TaskLedgerInput>) -> Result<()> {
        self.recorder.append(events)
    }
}

fn input(task_id: &TaskId, event: TaskEvent) -> TaskLedgerInput {
    TaskLedgerInput {
        task_id: task_id.clone(),
        event,
    }
}

fn sensor_task_id(sensor: &SensorPlan) -> Result<TaskId> {
    TaskId::parse(&format!("sensor-{}", sensor.id))
}

fn sensor_timeout_ms(sensor: &SensorPlan) -> Result<u64> {
    let subprocess_count = match sensor.id.as_str() {
        "tokmd" => TOKMD_MAX_SUBPROCESS_COUNT,
        "ripr" => 2,
        _ => 1,
    };
    sensor
        .timeout_sec
        .checked_mul(subprocess_count)
        .context("sensor aggregate timeout seconds overflow")?
        .checked_mul(1_000)
        .context("sensor timeout milliseconds overflow")
}

fn sensor_consumer(sensor: &SensorPlan, id: &str) -> Result<TaskConsumer> {
    let requirement = if sensor.required {
        TaskRequirement::Required
    } else {
        TaskRequirement::Optional
    };
    let value = if sensor.required {
        TaskValueClass::GateCritical
    } else if sensor.gate.is_some() {
        TaskValueClass::ClaimDirected
    } else {
        TaskValueClass::Telemetry
    };
    TaskConsumer::parse(id, requirement, value)
}

fn receipt_event(
    sensor: &SensorPlan,
    at: MonotonicInstant,
    receipt_created: bool,
) -> Result<TaskLedgerInput> {
    let task_id = sensor_task_id(sensor)?;
    let event = if receipt_created {
        TaskEvent::ReceiptCreated {
            at,
            reference: sensor_receipt_reference(sensor),
        }
    } else {
        TaskEvent::ReceiptCreationFailed {
            at,
            reason: "sensor status receipt missing or invalid".to_owned(),
        }
    };
    Ok(input(&task_id, event))
}

fn sensor_receipt_reference(sensor: &SensorPlan) -> String {
    format!("sensors/{}/ub-review-sensor-status.json", sensor.id)
}

fn canonical_reason(value: &str) -> Result<String> {
    ensure!(
        !value.is_empty() && value.trim() == value && !value.chars().any(char::is_control),
        "sensor task-ledger terminal reason must be canonical and non-empty"
    );
    Ok(value.to_owned())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::Instant;

    use anyhow::{Context, Result, ensure};

    use super::*;
    use crate::tests::{sensor_plan, test_plan};

    fn revision() -> RevisionRef {
        RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        }
    }

    fn snapshot_task<'a>(
        snapshot: &'a serde_json::Value,
        id: &str,
    ) -> Result<&'a serde_json::Value> {
        snapshot
            .get("tasks")
            .and_then(serde_json::Value::as_array)
            .context("task-ledger snapshot has no task array")?
            .iter()
            .find(|task| task.pointer("/id").and_then(serde_json::Value::as_str) == Some(id))
            .with_context(|| format!("task-ledger snapshot has no task {id}"))
    }

    fn read_snapshot(out: &std::path::Path) -> Result<serde_json::Value> {
        Ok(serde_json::from_slice(&fs::read(
            out.join("review/task_ledger_snapshot.json"),
        )?)?)
    }

    #[test]
    fn runnable_and_skipped_sensors_share_one_shadow_contract() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let runnable = sensor_plan("fast-probe", "probe", true);
        let mut skipped = sensor_plan("skipped-probe", "probe", false);
        skipped.reason = "resolved trigger did not select sensor".to_owned();
        let plan = test_plan(vec![runnable.clone(), skipped]);
        let ledger = SensorTaskLedger::initialize(&revision(), &plan, false, &Instant::now())?;

        ledger.setup_started(&runnable)?;
        ledger.run_started(&runnable)?;
        ledger.process_finished(&runnable, TaskTerminalDisposition::Succeeded)?;
        ledger.cleanup_finished(&runnable)?;
        ledger.receipt_recorded_and_resources_released(&runnable, true)?;
        ledger.write_artifacts(temp.path())?;

        ensure!(temp.path().join("task_ledger_events.ndjson").is_file());
        let snapshot = read_snapshot(temp.path())?;
        ensure!(snapshot["event_count"] == 15);
        let completed = snapshot_task(&snapshot, "sensor-fast-probe")?;
        ensure!(completed["state"] == serde_json::json!({"ResourcesReleased": "Succeeded"}));
        ensure!(completed["source"] == "Sensor");
        ensure!(completed["resources_released"] == true);
        ensure!(
            completed["receipt"]
                == serde_json::json!({
                    "Created": {
                        "reference": "sensors/fast-probe/ub-review-sensor-status.json"
                    }
                })
        );
        ensure!(
            completed
                .pointer("/timing/process_started_at")
                .is_some_and(|value| !value.is_null())
        );

        let declined = snapshot_task(&snapshot, "sensor-skipped-probe")?;
        ensure!(declined["state"] == serde_json::json!({"TerminallyDeclined": "Refused"}));
        ensure!(declined["receipt"].is_null());
        ensure!(
            declined
                .pointer("/timing/process_started_at")
                .is_some_and(serde_json::Value::is_null)
        );
        ensure!(declined["terminal_reason"] == "resolved trigger did not select sensor");
        Ok(())
    }

    #[test]
    fn setup_timeout_receipt_failure_and_release_remain_explicit() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let missing = sensor_plan("missing-probe", "missing", true);
        let timed_out = sensor_plan("timeout-probe", "timeout", true);
        let plan = test_plan(vec![missing.clone(), timed_out.clone()]);
        let ledger = SensorTaskLedger::initialize(&revision(), &plan, false, &Instant::now())?;

        ledger.setup_started(&missing)?;
        ledger.setup_failed(&missing, true)?;
        ledger.setup_started(&timed_out)?;
        ledger.run_started(&timed_out)?;
        ledger.process_finished(&timed_out, TaskTerminalDisposition::TimedOut)?;
        ledger.cleanup_finished(&timed_out)?;
        ledger.receipt_recorded_and_resources_released(&timed_out, false)?;
        ledger.write_artifacts(temp.path())?;

        let snapshot = read_snapshot(temp.path())?;
        let missing_task = snapshot_task(&snapshot, "sensor-missing-probe")?;
        ensure!(missing_task["state"] == serde_json::json!({"ResourcesReleased": "SetupFailed"}));
        ensure!(missing_task["resources_released"] == true);
        ensure!(
            missing_task
                .pointer("/timing/process_started_at")
                .is_some_and(serde_json::Value::is_null)
        );

        let timeout_task = snapshot_task(&snapshot, "sensor-timeout-probe")?;
        ensure!(timeout_task["state"] == serde_json::json!({"ResourcesReleased": "TimedOut"}));
        ensure!(
            timeout_task["receipt"]
                == serde_json::json!({
                    "CreationFailed": {
                        "reason": "sensor status receipt missing or invalid"
                    }
                })
        );
        ensure!(timeout_task["resources_released"] == true);
        Ok(())
    }

    #[test]
    fn empty_sensor_plan_omits_the_optional_artifact_pair() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let plan = test_plan(Vec::new());
        let ledger = SensorTaskLedger::initialize(&revision(), &plan, false, &Instant::now())?;
        fs::create_dir_all(temp.path().join("review"))?;
        fs::write(temp.path().join("task_ledger_events.ndjson"), b"stale\n")?;
        fs::write(
            temp.path().join("review/task_ledger_snapshot.json"),
            b"stale\n",
        )?;

        ledger.write_artifacts(temp.path())?;

        ensure!(!temp.path().join("task_ledger_events.ndjson").exists());
        ensure!(
            !temp
                .path()
                .join("review/task_ledger_snapshot.json")
                .exists()
        );
        Ok(())
    }

    #[test]
    fn tokmd_reserves_the_aggregate_subprocess_timeout_ceiling() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut tokmd = sensor_plan("tokmd", "tokmd", false);
        tokmd.timeout_sec = 7;
        tokmd.reason = "fixture does not execute tokmd".to_owned();
        let plan = test_plan(vec![tokmd]);
        let ledger = SensorTaskLedger::initialize(&revision(), &plan, false, &Instant::now())?;

        ledger.write_artifacts(temp.path())?;

        let snapshot = read_snapshot(temp.path())?;
        let task = snapshot_task(&snapshot, "sensor-tokmd")?;
        ensure!(task.pointer("/limits/timeout_ceiling_ms") == Some(&serde_json::json!(42_000)));
        Ok(())
    }

    #[test]
    fn ripr_reserves_primary_and_detail_subprocess_timeout_ceilings() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let revision = revision();
        let mut ripr = sensor_plan("ripr", "ripr", false);
        ripr.timeout_sec = 7;
        ripr.reason = "fixture does not execute ripr".to_owned();
        let plan = test_plan(vec![ripr]);
        let ledger = SensorTaskLedger::initialize(&revision, &plan, false, &Instant::now())?;

        ledger.write_artifacts(temp.path())?;

        let snapshot = read_snapshot(temp.path())?;
        let task = snapshot_task(&snapshot, "sensor-ripr")?;

        ensure!(task.pointer("/limits/timeout_ceiling_ms") == Some(&serde_json::json!(14_000)));
        Ok(())
    }
}
