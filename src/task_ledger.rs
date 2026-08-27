//! Pure task lifecycle and accounting model (A2.1-A2.2, #952/#953).
//!
//! Callers inject monotonic timestamps and resource reservations. No clock,
//! filesystem, process, scheduler, or artifact I/O lives here. The reducer
//! binds every task to the admitted revision and rejects events atomically.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "tracked in policy/allow.toml#task-ledger-shadow")
)]

use crate::RevisionRef;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Stable identifier for exactly one task.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TaskId(String);

impl TaskId {
    /// Parse a non-empty task identifier.
    pub(crate) fn parse(value: &str) -> Result<Self> {
        Ok(Self(non_empty(value, "task id")?))
    }

    /// Return the validated identifier.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why the task entered the ledger.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskSource {
    Required,
    Configured,
    Impact,
    Sensor,
    Worker,
    ReviewerTurn { model_on: bool },
}

/// Whether one consumer requires this task for its own result.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskRequirement {
    Required,
    Optional,
}

/// The consumer-visible value of completing the task.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskValueClass {
    GateCritical,
    ClaimDirected,
    Advisory,
    Telemetry,
}

/// One interested party and its explicit requirement/value metadata.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskConsumer {
    id: String,
    requirement: TaskRequirement,
    value: TaskValueClass,
}

impl TaskConsumer {
    /// Construct a consumer without inferring metadata from its identifier.
    pub(crate) fn parse(
        id: &str,
        requirement: TaskRequirement,
        value: TaskValueClass,
    ) -> Result<Self> {
        Ok(Self {
            id: non_empty(id, "task consumer")?,
            requirement,
            value,
        })
    }

    /// Return the stable consumer identifier.
    pub(crate) fn id(&self) -> &str {
        &self.id
    }
}

/// Injected monotonic time in milliseconds from a caller-owned epoch.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct MonotonicInstant(u64);

impl MonotonicInstant {
    /// Construct an injected instant. The reducer never reads a real clock.
    pub(crate) const fn from_millis(value: u64) -> Self {
        Self(value)
    }

    /// Return the caller-owned monotonic value.
    pub(crate) const fn as_millis(self) -> u64 {
        self.0
    }
}

/// Resource classes recorded at admission, without capacity decisions.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskResourceClass {
    Cpu,
    Memory,
    Disk,
    Worktree,
    Network,
    Test,
    Build,
    Cargo,
}

/// One positive resource reservation recorded by the admitting scheduler.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ResourceReservation {
    class: TaskResourceClass,
    units: u64,
}

impl ResourceReservation {
    /// Construct a positive accounting reservation.
    pub(crate) fn new(class: TaskResourceClass, units: u64) -> Result<Self> {
        ensure!(
            units > 0,
            "[invalid_resource_accounting] reservation units must be positive"
        );
        Ok(Self { class, units })
    }

    /// Return the reserved resource class.
    pub(crate) const fn class(&self) -> TaskResourceClass {
        self.class
    }

    /// Return the reserved unit count.
    pub(crate) const fn units(&self) -> u64 {
        self.units
    }
}

/// A safety ceiling is admission metadata, never observed process duration.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskExecutionLimits {
    timeout_ceiling_ms: u64,
}

impl TaskExecutionLimits {
    /// Construct a positive timeout ceiling.
    pub(crate) fn new(timeout_ceiling_ms: u64) -> Result<Self> {
        ensure!(
            timeout_ceiling_ms > 0,
            "[invalid_timing] timeout ceiling must be positive"
        );
        Ok(Self { timeout_ceiling_ms })
    }

    /// Return the admission-metadata timeout ceiling.
    pub(crate) const fn timeout_ceiling_ms(&self) -> u64 {
        self.timeout_ceiling_ms
    }
}

/// Outcomes for work that entered resource-backed execution.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskTerminalDisposition {
    Succeeded,
    DeterministicFailure,
    TimedOut,
    Cancelled,
    SetupFailed,
}

/// Terminal outcomes that never reached admitted execution.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskNonExecutionDisposition {
    Unsupported,
    Refused,
    BudgetDeferred,
    LatestSafeStartDeferred,
    Superseded,
    SatisfiedByExistingReceipt,
}

/// Receipt accounting remains separate from the execution outcome.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskReceiptOutcome {
    Created { reference: String },
    CreationFailed { reason: String },
}

/// Distinct injected timing points and derived durations.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskTiming {
    pub(crate) queued_at: Option<MonotonicInstant>,
    pub(crate) resource_wait_started_at: Option<MonotonicInstant>,
    pub(crate) admitted_at: Option<MonotonicInstant>,
    pub(crate) setup_started_at: Option<MonotonicInstant>,
    pub(crate) process_started_at: Option<MonotonicInstant>,
    pub(crate) process_finished_at: Option<MonotonicInstant>,
    pub(crate) cleanup_finished_at: Option<MonotonicInstant>,
    pub(crate) receipt_recorded_at: Option<MonotonicInstant>,
    pub(crate) resources_released_at: Option<MonotonicInstant>,
    pub(crate) queue_ms: Option<u64>,
    pub(crate) resource_wait_ms: Option<u64>,
    pub(crate) setup_ms: Option<u64>,
    pub(crate) process_ms: Option<u64>,
    pub(crate) cleanup_ms: Option<u64>,
}

/// Executed work is not final until receipt attempt and release are recorded.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskState {
    Proposed,
    Selected,
    Queued,
    ResourceWait,
    Admitted,
    Setup,
    Running,
    Cleanup(TaskTerminalDisposition),
    ReceiptPending(TaskTerminalDisposition),
    ReleasePending(TaskTerminalDisposition),
    ResourcesReleased(TaskTerminalDisposition),
    TerminallyDeclined(TaskNonExecutionDisposition),
}

/// Append-only inputs to the pure reducer.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) enum TaskEvent {
    Proposed {
        revision: RevisionRef,
        source: TaskSource,
        limits: TaskExecutionLimits,
    },
    ConsumerAttached {
        consumer: TaskConsumer,
    },
    Selected,
    Queued {
        at: MonotonicInstant,
    },
    EnteredResourceWait {
        at: MonotonicInstant,
    },
    Admitted {
        at: MonotonicInstant,
        reservations: Vec<ResourceReservation>,
    },
    SetupStarted {
        at: MonotonicInstant,
    },
    RunStarted {
        at: MonotonicInstant,
    },
    ProcessFinished {
        at: MonotonicInstant,
        disposition: TaskTerminalDisposition,
    },
    SetupFailed {
        at: MonotonicInstant,
    },
    PreRunTerminated {
        at: MonotonicInstant,
        disposition: TaskTerminalDisposition,
    },
    CleanupFinished {
        at: MonotonicInstant,
    },
    ReceiptCreated {
        at: MonotonicInstant,
        reference: String,
    },
    ReceiptCreationFailed {
        at: MonotonicInstant,
        reason: String,
    },
    ResourcesReleased {
        at: MonotonicInstant,
    },
    TerminallyDeclined {
        at: MonotonicInstant,
        disposition: TaskNonExecutionDisposition,
        reason: String,
        existing_receipt: Option<String>,
    },
}

impl TaskEvent {
    /// Stable diagnostic name for invalid-transition errors.
    fn kind(&self) -> &'static str {
        match self {
            Self::Proposed { .. } => "proposed",
            Self::ConsumerAttached { .. } => "consumer_attached",
            Self::Selected => "selected",
            Self::Queued { .. } => "queued",
            Self::EnteredResourceWait { .. } => "resource_wait",
            Self::Admitted { .. } => "admitted",
            Self::SetupStarted { .. } => "setup",
            Self::RunStarted { .. } => "running",
            Self::ProcessFinished { .. } => "process_finished",
            Self::SetupFailed { .. } => "setup_failed",
            Self::PreRunTerminated { .. } => "pre_run_terminated",
            Self::CleanupFinished { .. } => "cleanup_finished",
            Self::ReceiptCreated { .. } => "receipt_created",
            Self::ReceiptCreationFailed { .. } => "receipt_creation_failed",
            Self::ResourcesReleased { .. } => "resources_released",
            Self::TerminallyDeclined { .. } => "terminally_declined",
        }
    }

    /// Injected time carried by accounting events.
    fn at(&self) -> Option<MonotonicInstant> {
        match self {
            Self::Queued { at }
            | Self::EnteredResourceWait { at }
            | Self::Admitted { at, .. }
            | Self::SetupStarted { at }
            | Self::RunStarted { at }
            | Self::ProcessFinished { at, .. }
            | Self::SetupFailed { at }
            | Self::PreRunTerminated { at, .. }
            | Self::CleanupFinished { at }
            | Self::ReceiptCreated { at, .. }
            | Self::ReceiptCreationFailed { at, .. }
            | Self::ResourcesReleased { at }
            | Self::TerminallyDeclined { at, .. } => Some(*at),
            Self::Proposed { .. } | Self::ConsumerAttached { .. } | Self::Selected => None,
        }
    }
}

/// Deterministic snapshot produced by replaying one task's events.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct TaskSnapshot {
    pub(crate) id: TaskId,
    pub(crate) state: TaskState,
    pub(crate) revision_digest: String,
    pub(crate) source: TaskSource,
    pub(crate) limits: TaskExecutionLimits,
    pub(crate) consumers: Vec<TaskConsumer>,
    pub(crate) reservations: Vec<ResourceReservation>,
    pub(crate) resources_released: bool,
    pub(crate) timing: TaskTiming,
    pub(crate) execution_disposition: Option<TaskTerminalDisposition>,
    pub(crate) non_execution_disposition: Option<TaskNonExecutionDisposition>,
    pub(crate) terminal_reason: Option<String>,
    pub(crate) receipt: Option<TaskReceiptOutcome>,
    pub(crate) existing_receipt: Option<String>,
    last_event_at: Option<MonotonicInstant>,
}

/// Pure, fail-closed reducer for one task identity.
#[derive(Clone, Debug, Default)]
pub(crate) struct TaskReducer {
    snapshot: Option<TaskSnapshot>,
}

impl TaskReducer {
    /// Construct an empty reducer.
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Return the current immutable snapshot, if proposed.
    pub(crate) fn snapshot(&self) -> Option<&TaskSnapshot> {
        self.snapshot.as_ref()
    }

    /// Fold one event atomically. Rejected events leave state unchanged.
    pub(crate) fn apply(
        &mut self,
        id: &TaskId,
        event: &TaskEvent,
        current_revision: &RevisionRef,
    ) -> Result<&TaskSnapshot> {
        normalized_non_empty(id.as_str(), "task id")?;
        if let TaskEvent::Proposed {
            revision,
            source,
            limits,
        } = event
        {
            ensure!(
                revision.digest == current_revision.digest,
                "[stale_revision] proposed event binds revision {} but the run admitted {}",
                revision.digest,
                current_revision.digest
            );
            ensure!(
                limits.timeout_ceiling_ms > 0,
                "[invalid_timing] timeout ceiling must be positive"
            );
            ensure!(
                self.snapshot.is_none(),
                "[duplicate_event] task {} already has an initial proposal",
                id.as_str()
            );
            self.snapshot = Some(TaskSnapshot {
                id: id.clone(),
                state: TaskState::Proposed,
                revision_digest: current_revision.digest.clone(),
                source: source.clone(),
                limits: *limits,
                consumers: Vec::new(),
                reservations: Vec::new(),
                resources_released: false,
                timing: TaskTiming::default(),
                execution_disposition: None,
                non_execution_disposition: None,
                terminal_reason: None,
                receipt: None,
                existing_receipt: None,
                last_event_at: None,
            });
            return self.snapshot.as_ref().ok_or_else(|| {
                anyhow::anyhow!("[missing_strong_binding] proposal did not persist")
            });
        }

        let current = self.snapshot.as_ref().ok_or_else(|| {
            anyhow::anyhow!(
                "[missing_strong_binding] task {} received {} before its proposal",
                id.as_str(),
                event.kind()
            )
        })?;
        ensure!(
            current.revision_digest == current_revision.digest,
            "[stale_revision] task binds revision {} but the run admitted {}",
            current.revision_digest,
            current_revision.digest
        );
        ensure!(
            current.id == *id,
            "[missing_strong_binding] event targets task {} but this reducer holds {}",
            id.as_str(),
            current.id.as_str()
        );
        let mut candidate = current.clone();
        reduce_existing(&mut candidate, event)?;
        self.snapshot = Some(candidate);
        self.snapshot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("[missing_strong_binding] reduction did not persist"))
    }
}

/// Apply a non-proposal event to a cloned snapshot.
fn reduce_existing(snapshot: &mut TaskSnapshot, event: &TaskEvent) -> Result<()> {
    if let (Some(previous), Some(at)) = (snapshot.last_event_at, event.at()) {
        ensure!(
            at >= previous,
            "[invalid_timing] {} at {} precedes prior event at {}",
            event.kind(),
            at.as_millis(),
            previous.as_millis()
        );
    }
    let next = transition(snapshot.state, event)?;
    apply_payload(snapshot, event)?;
    snapshot.state = next;
    if let Some(at) = event.at() {
        snapshot.last_event_at = Some(at);
    }
    Ok(())
}

/// Validate the state/event pair and return the next state.
fn transition(state: TaskState, event: &TaskEvent) -> Result<TaskState> {
    let next = match (state, event) {
        (TaskState::Proposed, TaskEvent::Selected) => Some(TaskState::Selected),
        (TaskState::Selected, TaskEvent::Queued { .. }) => Some(TaskState::Queued),
        (TaskState::Queued, TaskEvent::EnteredResourceWait { .. }) => Some(TaskState::ResourceWait),
        (TaskState::Queued | TaskState::ResourceWait, TaskEvent::Admitted { .. }) => {
            Some(TaskState::Admitted)
        }
        (TaskState::Admitted, TaskEvent::SetupStarted { .. }) => Some(TaskState::Setup),
        (TaskState::Setup, TaskEvent::RunStarted { .. }) => Some(TaskState::Running),
        (TaskState::Running, TaskEvent::ProcessFinished { disposition, .. }) => {
            ensure!(
                *disposition != TaskTerminalDisposition::SetupFailed,
                "[invalid_transition] setup failure cannot be a process result"
            );
            Some(TaskState::Cleanup(*disposition))
        }
        (TaskState::Admitted | TaskState::Setup, TaskEvent::SetupFailed { .. }) => Some(
            TaskState::ReceiptPending(TaskTerminalDisposition::SetupFailed),
        ),
        (
            TaskState::Admitted | TaskState::Setup,
            TaskEvent::PreRunTerminated { disposition, .. },
        ) if matches!(
            disposition,
            TaskTerminalDisposition::TimedOut | TaskTerminalDisposition::Cancelled
        ) =>
        {
            Some(TaskState::ReceiptPending(*disposition))
        }
        (TaskState::Cleanup(disposition), TaskEvent::CleanupFinished { .. }) => {
            Some(TaskState::ReceiptPending(disposition))
        }
        (
            TaskState::ReceiptPending(disposition),
            TaskEvent::ReceiptCreated { .. } | TaskEvent::ReceiptCreationFailed { .. },
        ) => Some(TaskState::ReleasePending(disposition)),
        (TaskState::ReleasePending(disposition), TaskEvent::ResourcesReleased { .. }) => {
            Some(TaskState::ResourcesReleased(disposition))
        }
        (
            TaskState::Proposed | TaskState::Selected | TaskState::Queued | TaskState::ResourceWait,
            TaskEvent::TerminallyDeclined { disposition, .. },
        ) => Some(TaskState::TerminallyDeclined(*disposition)),
        (_, TaskEvent::ConsumerAttached { .. }) => Some(state),
        _ => None,
    };
    next.ok_or_else(|| {
        anyhow::anyhow!(
            "[invalid_transition] {} cannot accept {}",
            state_name(state),
            event.kind()
        )
    })
}

/// Apply validated event data and derive timing/accounting fields.
fn apply_payload(snapshot: &mut TaskSnapshot, event: &TaskEvent) -> Result<()> {
    match event {
        TaskEvent::ConsumerAttached { consumer } => {
            normalized_non_empty(consumer.id(), "task consumer")?;
            if let Some(existing) = snapshot
                .consumers
                .iter()
                .find(|item| item.id() == consumer.id())
            {
                ensure!(
                    existing == consumer,
                    "[conflicting_consumer] consumer {} changed requirement/value metadata",
                    consumer.id()
                );
            } else {
                snapshot.consumers.push(consumer.clone());
            }
        }
        TaskEvent::Queued { at } => snapshot.timing.queued_at = Some(*at),
        TaskEvent::EnteredResourceWait { at } => {
            snapshot.timing.queue_ms = duration(snapshot.timing.queued_at, *at, "queue")?;
            snapshot.timing.resource_wait_started_at = Some(*at);
        }
        TaskEvent::Admitted { at, reservations } => {
            validate_reservations(reservations)?;
            if snapshot.state == TaskState::Queued {
                snapshot.timing.queue_ms = duration(snapshot.timing.queued_at, *at, "queue")?;
            }
            snapshot.timing.resource_wait_ms = match snapshot.timing.resource_wait_started_at {
                Some(started) => duration(Some(started), *at, "resource wait")?,
                None => None,
            };
            snapshot.timing.admitted_at = Some(*at);
            snapshot.reservations.clone_from(reservations);
        }
        TaskEvent::SetupStarted { at } => snapshot.timing.setup_started_at = Some(*at),
        TaskEvent::RunStarted { at } => {
            snapshot.timing.setup_ms = duration(snapshot.timing.setup_started_at, *at, "setup")?;
            snapshot.timing.process_started_at = Some(*at);
        }
        TaskEvent::ProcessFinished { at, disposition } => {
            snapshot.timing.process_ms =
                duration(snapshot.timing.process_started_at, *at, "process")?;
            snapshot.timing.process_finished_at = Some(*at);
            snapshot.execution_disposition = Some(*disposition);
        }
        TaskEvent::SetupFailed { at } => {
            snapshot.timing.setup_ms = match snapshot.timing.setup_started_at {
                Some(started) => duration(Some(started), *at, "setup")?,
                None => None,
            };
            snapshot.execution_disposition = Some(TaskTerminalDisposition::SetupFailed);
        }
        TaskEvent::PreRunTerminated { at, disposition } => {
            ensure!(
                matches!(
                    disposition,
                    TaskTerminalDisposition::TimedOut | TaskTerminalDisposition::Cancelled
                ),
                "[invalid_transition] pre-run termination must be timed out or cancelled"
            );
            snapshot.timing.setup_ms = match snapshot.timing.setup_started_at {
                Some(started) => duration(Some(started), *at, "setup")?,
                None => None,
            };
            snapshot.execution_disposition = Some(*disposition);
        }
        TaskEvent::CleanupFinished { at } => {
            snapshot.timing.cleanup_ms =
                duration(snapshot.timing.process_finished_at, *at, "cleanup")?;
            snapshot.timing.cleanup_finished_at = Some(*at);
        }
        TaskEvent::ReceiptCreated { at, reference } => {
            snapshot.receipt = Some(TaskReceiptOutcome::Created {
                reference: non_empty(reference, "receipt reference")?,
            });
            snapshot.timing.receipt_recorded_at = Some(*at);
        }
        TaskEvent::ReceiptCreationFailed { at, reason } => {
            snapshot.receipt = Some(TaskReceiptOutcome::CreationFailed {
                reason: non_empty(reason, "receipt failure reason")?,
            });
            snapshot.timing.receipt_recorded_at = Some(*at);
        }
        TaskEvent::ResourcesReleased { at } => {
            snapshot.resources_released = true;
            snapshot.timing.resources_released_at = Some(*at);
        }
        TaskEvent::TerminallyDeclined {
            at,
            disposition,
            reason,
            existing_receipt,
        } => {
            let reason = non_empty(reason, "terminal reason")?;
            match snapshot.state {
                TaskState::Queued => {
                    snapshot.timing.queue_ms = duration(snapshot.timing.queued_at, *at, "queue")?;
                }
                TaskState::ResourceWait => {
                    snapshot.timing.resource_wait_ms = duration(
                        snapshot.timing.resource_wait_started_at,
                        *at,
                        "resource wait",
                    )?;
                }
                _ => {}
            }
            match disposition {
                TaskNonExecutionDisposition::SatisfiedByExistingReceipt => ensure!(
                    existing_receipt
                        .as_deref()
                        .is_some_and(|value| !value.trim().is_empty()),
                    "[missing_strong_binding] satisfied task requires an existing receipt"
                ),
                _ => ensure!(
                    existing_receipt.is_none(),
                    "[invalid_transition] only satisfied work may cite an existing receipt"
                ),
            }
            snapshot.non_execution_disposition = Some(*disposition);
            snapshot.terminal_reason = Some(reason);
            snapshot.existing_receipt = existing_receipt
                .as_deref()
                .map(str::trim)
                .map(str::to_owned);
        }
        TaskEvent::Proposed { .. } | TaskEvent::Selected => {}
    }
    Ok(())
}

/// Calculate a duration after the transition table proves the start exists.
fn duration(
    start: Option<MonotonicInstant>,
    end: MonotonicInstant,
    phase: &str,
) -> Result<Option<u64>> {
    let start = start
        .ok_or_else(|| anyhow::anyhow!("[invalid_timing] {phase} finish has no matching start"))?;
    let elapsed = end
        .as_millis()
        .checked_sub(start.as_millis())
        .ok_or_else(|| anyhow::anyhow!("[invalid_timing] {phase} finish precedes its start"))?;
    Ok(Some(elapsed))
}

/// Reject duplicate resource classes so release accounting is unambiguous.
fn validate_reservations(reservations: &[ResourceReservation]) -> Result<()> {
    for (index, reservation) in reservations.iter().enumerate() {
        ensure!(
            reservation.units > 0,
            "[invalid_resource_accounting] reservation units must be positive"
        );
        ensure!(
            !reservations[..index]
                .iter()
                .any(|prior| prior.class == reservation.class),
            "[invalid_resource_accounting] duplicate {:?} reservation",
            reservation.class
        );
    }
    Ok(())
}

/// Validate a required non-empty string and return its trimmed value.
fn non_empty(value: &str, field: &str) -> Result<String> {
    let trimmed = value.trim();
    ensure!(
        !trimmed.is_empty(),
        "[missing_strong_binding] {field} must be non-empty"
    );
    Ok(trimmed.to_owned())
}

/// Revalidate serde-created identifiers without silently changing identity.
fn normalized_non_empty(value: &str, field: &str) -> Result<()> {
    ensure!(
        value == non_empty(value, field)?,
        "[missing_strong_binding] {field} must not contain surrounding whitespace"
    );
    Ok(())
}

/// Stable state name used in diagnostics.
fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Proposed => "proposed",
        TaskState::Selected => "selected",
        TaskState::Queued => "queued",
        TaskState::ResourceWait => "resource_wait",
        TaskState::Admitted => "admitted",
        TaskState::Setup => "setup",
        TaskState::Running => "running",
        TaskState::Cleanup(_) => "cleanup",
        TaskState::ReceiptPending(_) => "receipt_pending",
        TaskState::ReleasePending(_) => "release_pending",
        TaskState::ResourcesReleased(_) => "resources_released",
        TaskState::TerminallyDeclined(_) => "terminally_declined",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct one shape-valid synthetic revision reference.
    fn revision(digest: char) -> RevisionRef {
        RevisionRef {
            digest: digest.to_string().repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: digest.to_string().repeat(40),
        }
    }

    /// Construct a proposal with caller-selected timeout metadata.
    fn proposed(revision: &RevisionRef, timeout_ms: u64) -> Result<TaskEvent> {
        Ok(TaskEvent::Proposed {
            revision: revision.clone(),
            source: TaskSource::Required,
            limits: TaskExecutionLimits::new(timeout_ms)?,
        })
    }

    /// Build the shared deterministic fixture through queue entry.
    fn reducer_at_queue() -> Result<(TaskReducer, TaskId, RevisionRef)> {
        let revision = revision('a');
        let id = TaskId::parse("task-1")?;
        let mut reducer = TaskReducer::new();
        reducer.apply(&id, &proposed(&revision, 600_000)?, &revision)?;
        reducer.apply(&id, &TaskEvent::Selected, &revision)?;
        reducer.apply(
            &id,
            &TaskEvent::Queued {
                at: MonotonicInstant::from_millis(10),
            },
            &revision,
        )?;
        Ok((reducer, id, revision))
    }

    /// Advance the shared fixture through resource wait and process start.
    fn admit_and_start(
        reducer: &mut TaskReducer,
        id: &TaskId,
        revision: &RevisionRef,
    ) -> Result<()> {
        reducer.apply(
            id,
            &TaskEvent::EnteredResourceWait {
                at: MonotonicInstant::from_millis(20),
            },
            revision,
        )?;
        reducer.apply(
            id,
            &TaskEvent::Admitted {
                at: MonotonicInstant::from_millis(30),
                reservations: vec![ResourceReservation::new(TaskResourceClass::Cpu, 2)?],
            },
            revision,
        )?;
        reducer.apply(
            id,
            &TaskEvent::SetupStarted {
                at: MonotonicInstant::from_millis(35),
            },
            revision,
        )?;
        reducer.apply(
            id,
            &TaskEvent::RunStarted {
                at: MonotonicInstant::from_millis(40),
            },
            revision,
        )?;
        Ok(())
    }

    /// Record process, cleanup, and either successful or failed receipt work.
    fn finish_execution(
        reducer: &mut TaskReducer,
        id: &TaskId,
        revision: &RevisionRef,
        disposition: TaskTerminalDisposition,
        receipt_fails: bool,
    ) -> Result<()> {
        reducer.apply(
            id,
            &TaskEvent::ProcessFinished {
                at: MonotonicInstant::from_millis(65),
                disposition,
            },
            revision,
        )?;
        reducer.apply(
            id,
            &TaskEvent::CleanupFinished {
                at: MonotonicInstant::from_millis(70),
            },
            revision,
        )?;
        let receipt = if receipt_fails {
            TaskEvent::ReceiptCreationFailed {
                at: MonotonicInstant::from_millis(72),
                reason: "disk full".to_owned(),
            }
        } else {
            TaskEvent::ReceiptCreated {
                at: MonotonicInstant::from_millis(72),
                reference: "receipts/task-1.json".to_owned(),
            }
        };
        reducer.apply(id, &receipt, revision)?;
        Ok(())
    }

    #[test]
    /// Distinct fake-clock intervals remain separately observable.
    fn fake_clock_keeps_all_execution_phases_distinct() -> Result<()> {
        let (mut reducer, id, revision) = reducer_at_queue()?;
        admit_and_start(&mut reducer, &id, &revision)?;
        finish_execution(
            &mut reducer,
            &id,
            &revision,
            TaskTerminalDisposition::Succeeded,
            false,
        )?;
        reducer.apply(
            &id,
            &TaskEvent::ResourcesReleased {
                at: MonotonicInstant::from_millis(75),
            },
            &revision,
        )?;
        let snapshot = reducer
            .snapshot()
            .ok_or_else(|| anyhow::anyhow!("snapshot"))?;
        ensure!(snapshot.state == TaskState::ResourcesReleased(TaskTerminalDisposition::Succeeded));
        ensure!(snapshot.timing.queue_ms == Some(10));
        ensure!(snapshot.timing.resource_wait_ms == Some(10));
        ensure!(snapshot.timing.setup_ms == Some(5));
        ensure!(snapshot.timing.process_ms == Some(25));
        ensure!(snapshot.timing.cleanup_ms == Some(5));
        ensure!(snapshot.resources_released);
        Ok(())
    }

    #[test]
    /// A timeout ceiling never substitutes for measured process time.
    fn timeout_ceiling_is_not_actual_process_duration() -> Result<()> {
        let (mut reducer, id, revision) = reducer_at_queue()?;
        admit_and_start(&mut reducer, &id, &revision)?;
        finish_execution(
            &mut reducer,
            &id,
            &revision,
            TaskTerminalDisposition::TimedOut,
            false,
        )?;
        let snapshot = reducer
            .snapshot()
            .ok_or_else(|| anyhow::anyhow!("snapshot"))?;
        ensure!(snapshot.limits.timeout_ceiling_ms == 600_000);
        ensure!(snapshot.timing.process_ms == Some(25));
        ensure!(snapshot.timing.process_ms != Some(snapshot.limits.timeout_ceiling_ms));
        Ok(())
    }

    #[test]
    /// Every process outcome and receipt outcome still requires release.
    fn every_executed_terminal_path_requires_receipt_then_release() -> Result<()> {
        for disposition in [
            TaskTerminalDisposition::Succeeded,
            TaskTerminalDisposition::DeterministicFailure,
            TaskTerminalDisposition::TimedOut,
            TaskTerminalDisposition::Cancelled,
        ] {
            for receipt_fails in [false, true] {
                let (mut reducer, id, revision) = reducer_at_queue()?;
                admit_and_start(&mut reducer, &id, &revision)?;
                finish_execution(&mut reducer, &id, &revision, disposition, receipt_fails)?;
                ensure!(
                    reducer
                        .apply(
                            &id,
                            &TaskEvent::ProcessFinished {
                                at: MonotonicInstant::from_millis(73),
                                disposition
                            },
                            &revision
                        )
                        .is_err()
                );
                reducer.apply(
                    &id,
                    &TaskEvent::ResourcesReleased {
                        at: MonotonicInstant::from_millis(75),
                    },
                    &revision,
                )?;
                let snapshot = reducer
                    .snapshot()
                    .ok_or_else(|| anyhow::anyhow!("snapshot"))?;
                ensure!(snapshot.state == TaskState::ResourcesReleased(disposition));
                ensure!(snapshot.execution_disposition == Some(disposition));
                ensure!(snapshot.resources_released && snapshot.receipt.is_some());
            }
        }
        Ok(())
    }

    #[test]
    /// Setup and receipt-write failures preserve outcome and release resources.
    fn setup_and_receipt_failure_still_release_resources() -> Result<()> {
        let (mut reducer, id, revision) = reducer_at_queue()?;
        reducer.apply(
            &id,
            &TaskEvent::Admitted {
                at: MonotonicInstant::from_millis(20),
                reservations: vec![ResourceReservation::new(TaskResourceClass::Memory, 512)?],
            },
            &revision,
        )?;
        reducer.apply(
            &id,
            &TaskEvent::SetupFailed {
                at: MonotonicInstant::from_millis(30),
            },
            &revision,
        )?;
        reducer.apply(
            &id,
            &TaskEvent::ReceiptCreationFailed {
                at: MonotonicInstant::from_millis(31),
                reason: "write refused".to_owned(),
            },
            &revision,
        )?;
        reducer.apply(
            &id,
            &TaskEvent::ResourcesReleased {
                at: MonotonicInstant::from_millis(32),
            },
            &revision,
        )?;
        let snapshot = reducer
            .snapshot()
            .ok_or_else(|| anyhow::anyhow!("snapshot"))?;
        ensure!(snapshot.timing.setup_started_at.is_none());
        ensure!(snapshot.timing.setup_ms.is_none());
        ensure!(snapshot.timing.process_ms.is_none());
        ensure!(matches!(
            snapshot.receipt,
            Some(TaskReceiptOutcome::CreationFailed { .. })
        ));
        ensure!(snapshot.execution_disposition == Some(TaskTerminalDisposition::SetupFailed));
        ensure!(snapshot.resources_released);
        Ok(())
    }

    #[test]
    /// Cancellation and timeout before a run retain their truthful disposition and timing.
    fn pre_run_termination_attempts_receipt_and_releases_resources() -> Result<()> {
        for disposition in [
            TaskTerminalDisposition::TimedOut,
            TaskTerminalDisposition::Cancelled,
        ] {
            for setup_started in [false, true] {
                let (mut reducer, id, revision) = reducer_at_queue()?;
                reducer.apply(
                    &id,
                    &TaskEvent::Admitted {
                        at: MonotonicInstant::from_millis(20),
                        reservations: vec![ResourceReservation::new(
                            TaskResourceClass::Memory,
                            512,
                        )?],
                    },
                    &revision,
                )?;
                if setup_started {
                    reducer.apply(
                        &id,
                        &TaskEvent::SetupStarted {
                            at: MonotonicInstant::from_millis(25),
                        },
                        &revision,
                    )?;
                }
                reducer.apply(
                    &id,
                    &TaskEvent::PreRunTerminated {
                        at: MonotonicInstant::from_millis(30),
                        disposition,
                    },
                    &revision,
                )?;
                reducer.apply(
                    &id,
                    &TaskEvent::ReceiptCreated {
                        at: MonotonicInstant::from_millis(31),
                        reference: "receipts/pre-run.json".to_owned(),
                    },
                    &revision,
                )?;
                reducer.apply(
                    &id,
                    &TaskEvent::ResourcesReleased {
                        at: MonotonicInstant::from_millis(32),
                    },
                    &revision,
                )?;

                let snapshot = reducer
                    .snapshot()
                    .ok_or_else(|| anyhow::anyhow!("snapshot"))?;
                ensure!(snapshot.execution_disposition == Some(disposition));
                ensure!(snapshot.timing.setup_ms == setup_started.then_some(5));
                ensure!(snapshot.timing.resource_wait_ms.is_none());
                ensure!(snapshot.timing.process_started_at.is_none());
                ensure!(snapshot.timing.process_ms.is_none());
                ensure!(snapshot.limits.timeout_ceiling_ms() == 600_000);
                let reservation = snapshot
                    .reservations
                    .first()
                    .ok_or_else(|| anyhow::anyhow!("reservation"))?;
                ensure!(reservation.class() == TaskResourceClass::Memory);
                ensure!(reservation.units() == 512);
                ensure!(snapshot.resources_released);
            }
        }
        Ok(())
    }

    #[test]
    /// Declined work records no invented execution or new proof receipt.
    fn non_executed_dispositions_fabricate_neither_timing_nor_receipt() -> Result<()> {
        for disposition in [
            TaskNonExecutionDisposition::Unsupported,
            TaskNonExecutionDisposition::Refused,
            TaskNonExecutionDisposition::BudgetDeferred,
            TaskNonExecutionDisposition::LatestSafeStartDeferred,
            TaskNonExecutionDisposition::Superseded,
            TaskNonExecutionDisposition::SatisfiedByExistingReceipt,
        ] {
            let (mut reducer, id, revision) = reducer_at_queue()?;
            let waited = disposition == TaskNonExecutionDisposition::LatestSafeStartDeferred;
            if waited {
                reducer.apply(
                    &id,
                    &TaskEvent::EnteredResourceWait {
                        at: MonotonicInstant::from_millis(15),
                    },
                    &revision,
                )?;
            }
            let existing_receipt = (disposition
                == TaskNonExecutionDisposition::SatisfiedByExistingReceipt)
                .then(|| "receipts/prior.json".to_owned());
            reducer.apply(
                &id,
                &TaskEvent::TerminallyDeclined {
                    at: MonotonicInstant::from_millis(20),
                    disposition,
                    reason: "not executed".to_owned(),
                    existing_receipt,
                },
                &revision,
            )?;
            let snapshot = reducer
                .snapshot()
                .ok_or_else(|| anyhow::anyhow!("snapshot"))?;
            ensure!(snapshot.state == TaskState::TerminallyDeclined(disposition));
            ensure!(
                snapshot.timing.process_started_at.is_none()
                    && snapshot.timing.process_ms.is_none()
            );
            ensure!(snapshot.timing.queue_ms == Some(if waited { 5 } else { 10 }));
            ensure!(snapshot.timing.resource_wait_ms == waited.then_some(5));
            ensure!(snapshot.receipt.is_none() && snapshot.reservations.is_empty());
            ensure!(!snapshot.resources_released);
        }
        Ok(())
    }

    #[test]
    /// Consumer metadata survives terminalization and post-terminal attachment.
    fn consumers_keep_independent_metadata_across_terminal_result() -> Result<()> {
        let (mut reducer, id, revision) = reducer_at_queue()?;
        let gate = TaskConsumer::parse(
            "gate",
            TaskRequirement::Required,
            TaskValueClass::GateCritical,
        )?;
        let reviewer = TaskConsumer::parse(
            "reviewer",
            TaskRequirement::Optional,
            TaskValueClass::ClaimDirected,
        )?;
        for consumer in [&gate, &reviewer, &gate] {
            reducer.apply(
                &id,
                &TaskEvent::ConsumerAttached {
                    consumer: consumer.clone(),
                },
                &revision,
            )?;
        }
        let conflicting =
            TaskConsumer::parse("gate", TaskRequirement::Optional, TaskValueClass::Telemetry)?;
        ensure!(
            reducer
                .apply(
                    &id,
                    &TaskEvent::ConsumerAttached {
                        consumer: conflicting
                    },
                    &revision
                )
                .is_err()
        );
        reducer.apply(
            &id,
            &TaskEvent::TerminallyDeclined {
                at: MonotonicInstant::from_millis(20),
                disposition: TaskNonExecutionDisposition::Refused,
                reason: "policy refused execution".to_owned(),
                existing_receipt: None,
            },
            &revision,
        )?;
        let auditor = TaskConsumer::parse(
            "auditor",
            TaskRequirement::Optional,
            TaskValueClass::Advisory,
        )?;
        reducer.apply(
            &id,
            &TaskEvent::ConsumerAttached {
                consumer: auditor.clone(),
            },
            &revision,
        )?;
        let snapshot = reducer
            .snapshot()
            .ok_or_else(|| anyhow::anyhow!("snapshot"))?;
        ensure!(snapshot.consumers == vec![gate, reviewer, auditor]);
        ensure!(
            snapshot.state == TaskState::TerminallyDeclined(TaskNonExecutionDisposition::Refused)
        );
        Ok(())
    }

    #[test]
    /// Invalid time and release order leave the prior snapshot intact.
    fn invalid_timing_and_resource_sequences_fail_without_mutation() -> Result<()> {
        let (mut reducer, id, revision) = reducer_at_queue()?;
        let before = reducer.snapshot().cloned();
        ensure!(
            reducer
                .apply(
                    &id,
                    &TaskEvent::EnteredResourceWait {
                        at: MonotonicInstant::from_millis(9)
                    },
                    &revision
                )
                .is_err()
        );
        ensure!(reducer.snapshot().cloned() == before);
        ensure!(
            reducer
                .apply(
                    &id,
                    &TaskEvent::ResourcesReleased {
                        at: MonotonicInstant::from_millis(11)
                    },
                    &revision
                )
                .is_err()
        );
        ensure!(reducer.snapshot().cloned() == before);
        ensure!(ResourceReservation::new(TaskResourceClass::Cpu, 0).is_err());
        ensure!(serde_json::from_str::<TaskEvent>(r#"{"Queued":{"at":-1}}"#).is_err());
        Ok(())
    }

    #[test]
    /// Invalid lifecycle order and conflicting outcomes fail closed.
    fn invalid_orders_and_conflicting_terminals_fail_closed() -> Result<()> {
        let revision = revision('a');
        let id = TaskId::parse("task-1")?;
        let mut reducer = TaskReducer::new();
        ensure!(reducer.apply(&id, &TaskEvent::Selected, &revision).is_err());
        reducer.apply(&id, &proposed(&revision, 10_000)?, &revision)?;
        reducer.apply(&id, &TaskEvent::Selected, &revision)?;
        let selected = reducer.snapshot().cloned();
        ensure!(reducer.apply(&id, &TaskEvent::Selected, &revision).is_err());
        ensure!(reducer.snapshot().cloned() == selected);

        reducer.apply(
            &id,
            &TaskEvent::Queued {
                at: MonotonicInstant::from_millis(1),
            },
            &revision,
        )?;
        reducer.apply(
            &id,
            &TaskEvent::Admitted {
                at: MonotonicInstant::from_millis(2),
                reservations: Vec::new(),
            },
            &revision,
        )?;
        reducer.apply(
            &id,
            &TaskEvent::SetupStarted {
                at: MonotonicInstant::from_millis(3),
            },
            &revision,
        )?;
        reducer.apply(
            &id,
            &TaskEvent::RunStarted {
                at: MonotonicInstant::from_millis(4),
            },
            &revision,
        )?;
        ensure!(
            reducer
                .apply(
                    &id,
                    &TaskEvent::RunStarted {
                        at: MonotonicInstant::from_millis(5),
                    },
                    &revision,
                )
                .is_err()
        );
        let setup_result_error = reducer
            .apply(
                &id,
                &TaskEvent::ProcessFinished {
                    at: MonotonicInstant::from_millis(5),
                    disposition: TaskTerminalDisposition::SetupFailed,
                },
                &revision,
            )
            .err()
            .ok_or_else(|| anyhow::anyhow!("setup failure was accepted as a process result"))?;
        ensure!(
            setup_result_error
                .to_string()
                .contains("setup failure cannot be a process result")
        );
        reducer.apply(
            &id,
            &TaskEvent::ProcessFinished {
                at: MonotonicInstant::from_millis(6),
                disposition: TaskTerminalDisposition::Succeeded,
            },
            &revision,
        )?;
        ensure!(
            reducer
                .apply(
                    &id,
                    &TaskEvent::ProcessFinished {
                        at: MonotonicInstant::from_millis(7),
                        disposition: TaskTerminalDisposition::DeterministicFailure,
                    },
                    &revision,
                )
                .is_err()
        );
        Ok(())
    }

    #[test]
    /// Serde cannot bypass validation enforced at the reducer boundary.
    fn malformed_deserialized_values_fail_at_reducer_boundary() -> Result<()> {
        let revision = revision('a');
        let forged_id: TaskId = serde_json::from_str(r#"" ""#)?;
        let forged_padded_id: TaskId = serde_json::from_str(r#"" task-1 ""#)?;
        let valid_id = TaskId::parse("task-1")?;
        let zero_limits: TaskExecutionLimits = serde_json::from_str(r#"{"timeout_ceiling_ms":0}"#)?;
        let proposal = TaskEvent::Proposed {
            revision: revision.clone(),
            source: TaskSource::Worker,
            limits: zero_limits,
        };
        let mut reducer = TaskReducer::new();
        ensure!(reducer.apply(&forged_id, &proposal, &revision).is_err());
        ensure!(
            reducer
                .apply(&forged_padded_id, &proposed(&revision, 10_000)?, &revision)
                .is_err()
        );
        ensure!(reducer.apply(&valid_id, &proposal, &revision).is_err());

        reducer.apply(&valid_id, &proposed(&revision, 10_000)?, &revision)?;
        let forged_consumer: TaskConsumer =
            serde_json::from_str(r#"{"id":" ","requirement":"Required","value":"GateCritical"}"#)?;
        let before = reducer.snapshot().cloned();
        ensure!(
            reducer
                .apply(
                    &valid_id,
                    &TaskEvent::ConsumerAttached {
                        consumer: forged_consumer,
                    },
                    &revision,
                )
                .is_err()
        );
        ensure!(reducer.snapshot().cloned() == before);
        let forged_padded_consumer: TaskConsumer = serde_json::from_str(
            r#"{"id":" review ","requirement":"Required","value":"GateCritical"}"#,
        )?;
        ensure!(
            reducer
                .apply(
                    &valid_id,
                    &TaskEvent::ConsumerAttached {
                        consumer: forged_padded_consumer,
                    },
                    &revision,
                )
                .is_err()
        );
        ensure!(reducer.snapshot().cloned() == before);
        Ok(())
    }

    #[test]
    /// Replay stays stable while stale revisions remain unable to mutate state.
    fn stale_revision_replay_and_serialization_are_deterministic() -> Result<()> {
        let admitted = revision('a');
        let foreign = revision('f');
        let id = TaskId::parse("task-1")?;
        let mut rejected = TaskReducer::new();
        ensure!(
            rejected
                .apply(&id, &proposed(&foreign, 10_000)?, &admitted)
                .is_err()
        );
        ensure!(rejected.snapshot().is_none());
        rejected.apply(&id, &proposed(&admitted, 10_000)?, &admitted)?;
        let before = rejected.snapshot().cloned();
        ensure!(rejected.apply(&id, &TaskEvent::Selected, &foreign).is_err());
        ensure!(rejected.snapshot().cloned() == before);
        let events = vec![
            proposed(&admitted, 10_000)?,
            TaskEvent::ConsumerAttached {
                consumer: TaskConsumer::parse(
                    "review",
                    TaskRequirement::Optional,
                    TaskValueClass::Advisory,
                )?,
            },
            TaskEvent::Selected,
            TaskEvent::Queued {
                at: MonotonicInstant::from_millis(1),
            },
            TaskEvent::TerminallyDeclined {
                at: MonotonicInstant::from_millis(2),
                disposition: TaskNonExecutionDisposition::BudgetDeferred,
                reason: "budget exhausted".to_owned(),
                existing_receipt: None,
            },
        ];
        let mut first = TaskReducer::new();
        let mut second = TaskReducer::new();
        for event in &events {
            first.apply(&id, event, &admitted)?;
            second.apply(
                &id,
                &serde_json::from_slice(&serde_json::to_vec(event)?)?,
                &admitted,
            )?;
        }
        ensure!(first.snapshot() == second.snapshot());
        ensure!(serde_json::to_vec(&first.snapshot())? == serde_json::to_vec(&second.snapshot())?);
        Ok(())
    }
}
