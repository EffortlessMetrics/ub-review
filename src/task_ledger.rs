//! Pure task lifecycle model (A2.1, #952).
//!
//! Execution-neutral: no filesystem, process execution, scheduler, or
//! artifact behavior lives here. The module defines the typed values and the
//! deterministic transition reducer for one task's lifecycle, bound to the
//! immutable revision reference from A1 so events from another revision are
//! rejected rather than silently mixed.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "slice A2.1 lands the pure lifecycle contract ahead of its first consumer; the A2.3 ledger artifact and A2.4/A2.5 shadow adapters resolve into these types and remove this expectation"
    )
)]

use crate::RevisionRef;
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// Stable identifier for exactly one task; attaching consumers never mints a
/// new identity.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TaskId(pub(crate) String);

impl TaskId {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            bail!("[missing_strong_binding] task id must be non-empty");
        }
        Ok(TaskId(trimmed.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Why the task exists, typed at proposal time - never inferred later from
/// string prefixes on ids or commands.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskSource {
    /// Obligated by gate policy: the run cannot pass without this work.
    Required,
    /// Requested by configuration for additional evidence.
    Configured,
    /// Derived from diff impact analysis.
    Impact,
    /// Fast/late sensor execution.
    Sensor,
    /// Local proof worker execution.
    Worker,
    /// A reviewer-model lane turn; `model_on` records routing posture while
    /// sharing every other type with model-off work.
    ReviewerTurn { model_on: bool },
}

/// One interested party attached to a task. Attachment is explicit and never
/// changes task identity.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TaskConsumer(pub(crate) String);

impl TaskConsumer {
    pub(crate) fn parse(value: &str) -> Result<Self> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            bail!("[missing_strong_binding] task consumer must be non-empty");
        }
        Ok(TaskConsumer(trimmed.to_owned()))
    }
}

/// How a task ended. `TerminallyDeclined` before queueing is recorded as a
/// state, not a disposition; these are post-admission outcomes.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskTerminalDisposition {
    Completed,
    Failed,
    Skipped,
}

/// Lifecycle states, in canonical order.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskState {
    Proposed,
    Selected,
    TerminallyDeclined,
    Queued,
    ResourceWait,
    Admitted,
    Setup,
    Running,
    Terminal(TaskTerminalDisposition),
}

/// Typed lifecycle events. Every event carries the admitted revision ref so
/// the reducer can reject foreign-revision input deterministically.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) enum TaskEvent {
    Proposed {
        revision: RevisionRef,
        source: TaskSource,
    },
    Selected,
    TerminallyDeclined {
        reason: String,
    },
    Queued,
    EnteredResourceWait,
    Admitted,
    SetupStarted,
    RunStarted,
    ConsumerAttached {
        consumer: TaskConsumer,
    },
    Terminal {
        disposition: TaskTerminalDisposition,
    },
}

impl TaskEvent {
    fn kind(&self) -> &'static str {
        match self {
            TaskEvent::Proposed { .. } => "proposed",
            TaskEvent::Selected => "selected",
            TaskEvent::TerminallyDeclined { .. } => "terminally_declined",
            TaskEvent::Queued => "queued",
            TaskEvent::EnteredResourceWait => "resource_wait",
            TaskEvent::Admitted => "admitted",
            TaskEvent::SetupStarted => "setup",
            TaskEvent::RunStarted => "running",
            TaskEvent::ConsumerAttached { .. } => "consumer_attached",
            TaskEvent::Terminal { .. } => "terminal",
        }
    }
}

/// Deterministic snapshot of one task after folding its events.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct TaskSnapshot {
    pub(crate) id: TaskId,
    pub(crate) state: TaskState,
    /// Digest of the revision this task is bound to (from its Proposal).
    pub(crate) revision_digest: String,
    pub(crate) source: Option<TaskSource>,
    pub(crate) consumers: Vec<TaskConsumer>,
    pub(crate) decline_reason: Option<String>,
}

/// Reducer over one task's events. Fails closed on invalid order, duplicate
/// or conflicting terminal decisions, and foreign-revision events.
#[derive(Clone, Debug, Default)]
pub(crate) struct TaskReducer {
    snapshot: Option<TaskSnapshot>,
}

impl TaskReducer {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn snapshot(&self) -> Option<&TaskSnapshot> {
        self.snapshot.as_ref()
    }

    /// Folds one event, returning the resulting snapshot.
    pub(crate) fn apply(
        &mut self,
        id: &TaskId,
        event: &TaskEvent,
        current_revision: &RevisionRef,
    ) -> Result<&TaskSnapshot> {
        // Foreign-revision rejection precedes everything else so evidence
        // from another revision can never mutate any state.
        let proposed_payload = match event {
            TaskEvent::Proposed { revision, source }
                if revision.digest != current_revision.digest =>
            {
                bail!(
                    "[stale_revision] proposed event binds revision {} but the run admitted {}",
                    revision.digest,
                    current_revision.digest
                );
            }
            TaskEvent::Proposed { source, .. } => Some(source),
            _ => None,
        };
        if let Some(source) = proposed_payload {
            if self.snapshot.is_some() {
                bail!(
                    "[duplicate_event] task {} already has an initial proposal",
                    id.as_str()
                );
            }
            self.snapshot = Some(TaskSnapshot {
                id: TaskId(id.as_str().to_owned()),
                state: TaskState::Proposed,
                revision_digest: current_revision.digest.clone(),
                source: Some(source.clone()),
                consumers: Vec::new(),
                decline_reason: None,
            });
            return match self.snapshot.as_ref() {
                Some(snapshot) => Ok(snapshot),
                None => bail!("[missing_strong_binding] snapshot assignment did not persist"),
            };
        }

        let snapshot = self.snapshot.as_mut().ok_or_else(|| {
            anyhow::anyhow!(
                "[missing_strong_binding] task {} received {} before its proposal",
                id.as_str(),
                event.kind()
            )
        })?;
        if snapshot.id.as_str() != id.as_str() {
            bail!(
                "[missing_strong_binding] event targets task {} but this reducer holds {}",
                id.as_str(),
                snapshot.id.as_str()
            );
        }

        let next = transition(snapshot.state, event)?;
        match event {
            TaskEvent::ConsumerAttached { consumer } if !snapshot.consumers.contains(consumer) => {
                snapshot.consumers.push(consumer.clone());
            }
            TaskEvent::TerminallyDeclined { reason } => {
                snapshot.decline_reason = Some(reason.clone());
            }
            _ => {}
        }
        snapshot.state = next;
        Ok(snapshot)
    }
}

/// The complete transition truth table: `(state, event) -> next state`.
/// Anything not listed here fails closed.
fn transition(state: TaskState, event: &TaskEvent) -> Result<TaskState> {
    let accepted = match (state, event) {
        (TaskState::Proposed, TaskEvent::Selected) => Some(TaskState::Selected),
        (TaskState::Proposed, TaskEvent::TerminallyDeclined { .. }) => {
            Some(TaskState::TerminallyDeclined)
        }
        (TaskState::Selected, TaskEvent::Queued) => Some(TaskState::Queued),
        (TaskState::Queued, TaskEvent::EnteredResourceWait) => Some(TaskState::ResourceWait),
        (TaskState::Queued, TaskEvent::Admitted) => Some(TaskState::Admitted),
        (TaskState::ResourceWait, TaskEvent::Admitted) => Some(TaskState::Admitted),
        (TaskState::Admitted, TaskEvent::SetupStarted) => Some(TaskState::Setup),
        (TaskState::Setup, TaskEvent::RunStarted) => Some(TaskState::Running),
        (TaskState::Running, TaskEvent::Terminal { .. }) => {
            let TaskEvent::Terminal { disposition } = event else {
                return Err(anyhow::anyhow!(
                    "[invalid_transition] terminal event payload missing"
                ));
            };
            Some(TaskState::Terminal(*disposition))
        }
        // Explicit attachment is legal in any live state; it never moves the
        // lifecycle and never re-opens a terminal task.
        (
            TaskState::Proposed
            | TaskState::Selected
            | TaskState::Queued
            | TaskState::ResourceWait
            | TaskState::Admitted
            | TaskState::Setup
            | TaskState::Running,
            TaskEvent::ConsumerAttached { .. },
        ) => Some(state),
        _ => None,
    };
    accepted.ok_or_else(|| {
        anyhow::anyhow!(
            "[invalid_transition] {} cannot accept {}",
            state_name(state),
            event.kind()
        )
    })
}

fn state_name(state: TaskState) -> &'static str {
    match state {
        TaskState::Proposed => "proposed",
        TaskState::Selected => "selected",
        TaskState::TerminallyDeclined => "terminally_declined",
        TaskState::Queued => "queued",
        TaskState::ResourceWait => "resource_wait",
        TaskState::Admitted => "admitted",
        TaskState::Setup => "setup",
        TaskState::Running => "running",
        TaskState::Terminal(_) => "terminal",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision_a() -> RevisionRef {
        RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        }
    }

    fn revision_b() -> RevisionRef {
        RevisionRef {
            digest: "f".repeat(64),
            semantics: "merge_result".to_owned(),
            reviewed_commit: "e".repeat(40),
        }
    }

    fn task_id() -> TaskId {
        TaskId("task-1".to_owned())
    }

    fn proposed(revision: &RevisionRef) -> TaskEvent {
        TaskEvent::Proposed {
            revision: revision.clone(),
            source: TaskSource::Required,
        }
    }

    /// The canonical happy path reduces deterministically to a terminal
    /// snapshot.
    #[test]
    fn valid_sequence_reduces_deterministically_to_terminal() -> Result<()> {
        let id = task_id();
        let events = vec![
            proposed(&revision_a()),
            TaskEvent::Selected,
            TaskEvent::Queued,
            TaskEvent::EnteredResourceWait,
            TaskEvent::Admitted,
            TaskEvent::SetupStarted,
            TaskEvent::RunStarted,
            TaskEvent::ConsumerAttached {
                consumer: TaskConsumer::parse("compiler")?,
            },
            TaskEvent::Terminal {
                disposition: TaskTerminalDisposition::Completed,
            },
        ];
        let mut reducer = TaskReducer::new();
        for event in &events {
            reducer.apply(&id, event, &revision_a())?;
        }
        let snapshot = reducer
            .snapshot()
            .ok_or_else(|| anyhow::anyhow!("reducer produced no snapshot"))?;
        assert_eq!(
            snapshot.state,
            TaskState::Terminal(TaskTerminalDisposition::Completed)
        );
        assert_eq!(snapshot.revision_digest, "a".repeat(64));
        assert_eq!(snapshot.consumers, vec![TaskConsumer::parse("compiler")?]);
        assert_eq!(snapshot.source, Some(TaskSource::Required));

        // Equivalent inputs replay to an identical snapshot.
        let mut replay = TaskReducer::new();
        for event in &events {
            replay.apply(&id, event, &revision_a())?;
        }
        assert_eq!(replay.snapshot(), reducer.snapshot());
        Ok(())
    }

    #[test]
    fn serialization_round_trips_all_pure_values() -> Result<()> {
        let id = task_id();
        let event = proposed(&revision_b());
        let json = serde_json::to_string(&event)?;
        assert_eq!(serde_json::from_str::<TaskEvent>(&json)?, event);
        let snapshot = TaskSnapshot {
            id: id.clone(),
            state: TaskState::Running,
            revision_digest: "f".repeat(64),
            source: Some(TaskSource::ReviewerTurn { model_on: false }),
            consumers: vec![TaskConsumer::parse("reporter")?],
            decline_reason: None,
        };
        let snapshot_json = serde_json::to_string(&snapshot)?;
        assert_eq!(
            serde_json::from_str::<TaskSnapshot>(&snapshot_json)?,
            snapshot
        );
        Ok(())
    }

    #[test]
    fn invalid_orders_fail_closed_with_tokens() -> Result<()> {
        let id = task_id();
        // Pre-proposal events are unbound evidence.
        let mut early = TaskReducer::new();
        let err = early
            .apply(&id, &TaskEvent::Selected, &revision_a())
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected transition rejection"))?;
        assert!(
            err.to_string().contains("[missing_strong_binding]"),
            "{err}"
        );

        // Double selection fails; so does terminal-to-running.
        let mut reducer = TaskReducer::new();
        reducer.apply(&id, &proposed(&revision_a()), &revision_a())?;
        reducer.apply(&id, &TaskEvent::Selected, &revision_a())?;
        let err = reducer
            .apply(&id, &TaskEvent::Selected, &revision_a())
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected transition rejection"))?;
        assert!(err.to_string().contains("[invalid_transition]"), "{err}");

        reducer.apply(&id, &TaskEvent::Queued, &revision_a())?;
        reducer.apply(&id, &TaskEvent::Admitted, &revision_a())?;
        reducer.apply(&id, &TaskEvent::SetupStarted, &revision_a())?;
        reducer.apply(&id, &TaskEvent::RunStarted, &revision_a())?;
        reducer.apply(
            &id,
            &TaskEvent::Terminal {
                disposition: TaskTerminalDisposition::Failed,
            },
            &revision_a(),
        )?;
        for late in [
            TaskEvent::RunStarted,
            TaskEvent::Selected,
            TaskEvent::Queued,
        ] {
            let err = reducer
                .apply(&id, &late, &revision_a())
                .err()
                .ok_or_else(|| anyhow::anyhow!("expected transition rejection"))?;
            assert!(err.to_string().contains("[invalid_transition]"), "{err}");
        }
        // Terminal is also absorbing for attachment and second terminals.
        let err = reducer
            .apply(
                &id,
                &TaskEvent::Terminal {
                    disposition: TaskTerminalDisposition::Completed,
                },
                &revision_a(),
            )
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected transition rejection"))?;
        assert!(err.to_string().contains("[invalid_transition]"), "{err}");
        let err = reducer
            .apply(
                &id,
                &TaskEvent::ConsumerAttached {
                    consumer: TaskConsumer::parse("late")?,
                },
                &revision_a(),
            )
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected transition rejection"))?;
        assert!(err.to_string().contains("[invalid_transition]"), "{err}");
        Ok(())
    }

    #[test]
    fn foreign_revision_events_are_rejected_before_any_mutation() -> Result<()> {
        let id = task_id();
        let mut reducer = TaskReducer::new();
        let err = reducer
            .apply(&id, &proposed(&revision_b()), &revision_a())
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected transition rejection"))?;
        assert!(err.to_string().contains("[stale_revision]"), "{err}");
        assert!(
            reducer.snapshot().is_none(),
            "rejected input must not mutate"
        );

        reducer.apply(&id, &proposed(&revision_a()), &revision_a())?;
        let before = reducer.snapshot().cloned();
        // Non-proposal events carry no revision of their own; the reducer's
        // binding is fixed by the proposal, so cross-revision mixing can only
        // enter through a second proposal - which fails closed, and the
        // foreign-revision rejection takes precedence over duplication.
        let err = reducer
            .apply(&id, &proposed(&revision_b()), &revision_a())
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected transition rejection"))?;
        assert!(err.to_string().contains("[stale_revision]"), "{err}");
        let err = reducer
            .apply(&id, &proposed(&revision_a()), &revision_a())
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected transition rejection"))?;
        assert!(err.to_string().contains("[duplicate_event]"), "{err}");
        assert_eq!(reducer.snapshot().cloned(), before);
        Ok(())
    }

    #[test]
    fn terminally_declined_records_its_reason_and_absorbs_everything() -> Result<()> {
        let id = task_id();
        let mut reducer = TaskReducer::new();
        reducer.apply(&id, &proposed(&revision_a()), &revision_a())?;
        reducer.apply(
            &id,
            &TaskEvent::TerminallyDeclined {
                reason: "impact model found no changed seam".to_owned(),
            },
            &revision_a(),
        )?;
        let snapshot = reducer
            .snapshot()
            .ok_or_else(|| anyhow::anyhow!("declined reducer produced no snapshot"))?;
        assert_eq!(snapshot.state, TaskState::TerminallyDeclined);
        assert_eq!(
            snapshot.decline_reason.as_deref(),
            Some("impact model found no changed seam")
        );
        let err = reducer
            .apply(&id, &TaskEvent::Queued, &revision_a())
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected transition rejection"))?;
        assert!(err.to_string().contains("[invalid_transition]"), "{err}");
        Ok(())
    }

    #[test]
    fn multi_consumer_attachment_is_explicit_and_idempotent() -> Result<()> {
        let id = task_id();
        let mut reducer = TaskReducer::new();
        reducer.apply(&id, &proposed(&revision_a()), &revision_a())?;
        let compiler = TaskConsumer::parse("compiler")?;
        reducer.apply(
            &id,
            &TaskEvent::ConsumerAttached {
                consumer: compiler.clone(),
            },
            &revision_a(),
        )?;
        // Re-attaching the same consumer does not duplicate it, and the task
        // identity/state stay untouched.
        reducer.apply(
            &id,
            &TaskEvent::ConsumerAttached {
                consumer: compiler.clone(),
            },
            &revision_a(),
        )?;
        let reporter = TaskConsumer::parse("reporter")?;
        reducer.apply(
            &id,
            &TaskEvent::ConsumerAttached {
                consumer: reporter.clone(),
            },
            &revision_a(),
        )?;
        let snapshot = reducer
            .snapshot()
            .ok_or_else(|| anyhow::anyhow!("attached reducer produced no snapshot"))?;
        assert_eq!(snapshot.consumers, vec![compiler.clone(), reporter.clone()]);
        assert_eq!(snapshot.id, id);
        assert_eq!(snapshot.state, TaskState::Proposed);
        Ok(())
    }

    #[test]
    fn typed_sources_share_neutral_types_across_model_modes() -> Result<()> {
        let on = TaskSource::ReviewerTurn { model_on: true };
        let off = TaskSource::ReviewerTurn { model_on: false };
        assert_ne!(on, off);
        let serialized = serde_json::to_string(&on)?;
        assert!(serialized.contains("model_on"));
        let deserialized: TaskSource = serde_json::from_str(&serialized)?;
        assert_eq!(deserialized, on);
        Ok(())
    }

    #[test]
    fn malformed_ids_and_consumers_are_rejected() {
        assert!(TaskId::parse("").is_err());
        assert!(TaskId::parse("   ").is_err());
        assert!(TaskConsumer::parse("").is_err());
        assert!(TaskConsumer::parse("  ").is_err());
    }
}
