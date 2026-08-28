//! Shadow TaskLedger adapter for planned and executed sensors (#955).
//!
//! This module observes the existing sensor scheduler. It does not choose,
//! reorder, deduplicate, admit, or execute work. The caller-owned monotonic
//! epoch and the existing status receipt remain the execution authorities.

use std::sync::{Arc, Mutex};
use std::time::Instant;
use std::{fs, io};

use anyhow::{Context, Result, ensure};

use crate::task_ledger::{
    MonotonicInstant, ResourceReservation, TaskConsumer, TaskEvent, TaskExecutionLimits, TaskId,
    TaskNonExecutionDisposition, TaskRequirement, TaskResourceClass, TaskSource,
    TaskTerminalDisposition, TaskValueClass,
};
use crate::task_ledger_artifact::{TaskLedgerInput, write_task_ledger_artifacts};
use crate::{Plan, RevisionRef, SensorPlan, work_queue_sensor_consumers};

#[derive(Clone)]
pub(crate) struct SensorTaskLedger {
    inner: Arc<SensorTaskLedgerInner>,
}

struct SensorTaskLedgerInner {
    revision: RevisionRef,
    run_started: Instant,
    inputs: Mutex<Vec<TaskLedgerInput>>,
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
            inner: Arc::new(SensorTaskLedgerInner {
                revision: revision.clone(),
                run_started: *run_started,
                inputs: Mutex::new(Vec::new()),
            }),
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

    /// Terminalize a process outcome. Cleanup, receipt attempt, and resource
    /// release stay explicit even when the sensor failed or timed out.
    pub(crate) fn execution_finished(
        &self,
        sensor: &SensorPlan,
        disposition: TaskTerminalDisposition,
        receipt_created: bool,
    ) -> Result<()> {
        ensure!(
            disposition != TaskTerminalDisposition::SetupFailed,
            "setup failure must use SensorTaskLedger::setup_failed"
        );
        let task_id = sensor_task_id(sensor)?;
        let at = self.now()?;
        self.append([
            input(&task_id, TaskEvent::ProcessFinished { at, disposition }),
            input(&task_id, TaskEvent::CleanupFinished { at }),
            receipt_event(sensor, at, receipt_created)?,
            input(&task_id, TaskEvent::ResourcesReleased { at }),
        ])
    }

    /// Persist the complete sensor shadow stream only after every late sensor
    /// has joined. The generic artifact writer replays before publishing.
    pub(crate) fn write_artifacts(&self, out: &std::path::Path) -> Result<()> {
        let inputs = self
            .inner
            .inputs
            .lock()
            .map_err(|_| anyhow::anyhow!("sensor task-ledger mutex poisoned"))?
            .clone();
        if inputs.is_empty() {
            let events_result = remove_optional_artifact(&out.join("task_ledger_events.ndjson"));
            let snapshot_result =
                remove_optional_artifact(&out.join("review/task_ledger_snapshot.json"));
            events_result?;
            return snapshot_result;
        }
        write_task_ledger_artifacts(out, &self.inner.revision, &inputs)
    }

    fn now(&self) -> Result<MonotonicInstant> {
        let elapsed = u64::try_from(self.inner.run_started.elapsed().as_millis())
            .context("sensor task-ledger monotonic time exceeds u64")?;
        Ok(MonotonicInstant::from_millis(elapsed))
    }

    fn append(&self, events: impl IntoIterator<Item = TaskLedgerInput>) -> Result<()> {
        self.inner
            .inputs
            .lock()
            .map_err(|_| anyhow::anyhow!("sensor task-ledger mutex poisoned"))?
            .extend(events);
        Ok(())
    }
}

fn remove_optional_artifact(path: &std::path::Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove stale {}", path.display())),
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
    sensor
        .timeout_sec
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
        ledger.execution_finished(&runnable, TaskTerminalDisposition::Succeeded, true)?;
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
        ledger.execution_finished(&timed_out, TaskTerminalDisposition::TimedOut, false)?;
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
}
