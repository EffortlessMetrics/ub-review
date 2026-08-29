//! TaskLedger shadow adapter for brokered proof commands (#956).
//!
//! This adapter observes the existing proof broker. It does not select,
//! approve, group, order, lease, or execute proof work. One executed command
//! side is one TaskLedger task; source-shaped requests remain separate
//! non-executing proposals so legacy broker grouping is visible without being
//! promoted to canonical equivalence.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, ensure};

use crate::task_ledger::{
    ResourceReservation, TaskConsumer, TaskEvent, TaskExecutionLimits, TaskId,
    TaskNonExecutionDisposition, TaskRequirement, TaskResourceClass, TaskSource,
    TaskTerminalDisposition, TaskValueClass,
};
use crate::task_ledger_artifact::TaskLedgerInput;
use crate::{
    DiffContext, FocusedBuildTask, FocusedTestTask, ProofReceipt, ProofRequest, ProofRequestV2,
    ResourceLease, RevisionRef, TaskLedgerRecorder, admit_revision, sanitize_artifact_name,
};

/// Production phase that caused one proof command to be executed.
///
/// The phase is carried explicitly because follow-up proof requests and the
/// primary model wave can have identical requester and claim metadata while
/// remaining distinct execution sources.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ProofExecutionPhase {
    InitialImpact,
    ModelRequest,
    FollowUp,
    Worker,
}

impl ProofExecutionPhase {
    fn consumer_id(self) -> &'static str {
        match self {
            Self::InitialImpact => "proof-phase:initial-impact",
            Self::ModelRequest => "proof-phase:model-request",
            Self::FollowUp => "proof-phase:follow-up",
            Self::Worker => "proof-phase:worker",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReceiptArtifact {
    Aggregate,
    Standalone,
}

impl ReceiptArtifact {
    fn reference(self, receipt_index: usize, command_index: usize) -> String {
        match self {
            Self::Aggregate => {
                format!("review/proof_receipts.json#/{receipt_index}/commands/{command_index}")
            }
            Self::Standalone => format!("proof_receipt.json#/commands/{command_index}"),
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProofTaskLedger {
    recorder: TaskLedgerRecorder,
    state: Arc<Mutex<ProofTaskLedgerState>>,
}

#[derive(Default)]
struct ProofTaskLedgerState {
    proposed: BTreeSet<String>,
    pending_receipts: BTreeSet<String>,
    expected_sides: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug)]
pub(crate) struct ProofCommandTask {
    receipt_id: String,
    side: String,
    kind: String,
    requested_by: Vec<String>,
    request_ids: Vec<String>,
    timeout_sec: u64,
    source: TaskSource,
    required: bool,
    execution_phase: ProofExecutionPhase,
}

impl ProofCommandTask {
    /// Build one focused-test command task for an explicit production phase.
    pub(crate) fn focused_test(
        task: &FocusedTestTask,
        side: &str,
        timeout_sec: u64,
        execution_phase: ProofExecutionPhase,
    ) -> Self {
        let required = task.required || is_configured_proof(&task.requested_by);
        Self {
            receipt_id: task.id.clone(),
            side: side.to_owned(),
            kind: "focused-test".to_owned(),
            requested_by: task.requested_by.clone(),
            request_ids: task.request_ids.clone(),
            timeout_sec,
            source: proof_command_source(required, execution_phase),
            required,
            execution_phase,
        }
    }

    /// Build one focused-build command task for an explicit production phase.
    pub(crate) fn focused_build(
        task: &FocusedBuildTask,
        timeout_sec: u64,
        execution_phase: ProofExecutionPhase,
    ) -> Self {
        let required = task.required || is_configured_proof(&task.requested_by);
        Self {
            receipt_id: task.id.clone(),
            side: "head".to_owned(),
            kind: "focused-build".to_owned(),
            requested_by: task.requested_by.clone(),
            request_ids: task.request_ids.clone(),
            timeout_sec,
            source: proof_command_source(required, execution_phase),
            required,
            execution_phase,
        }
    }

    /// Build the standalone worker capability-preflight task.
    pub(crate) fn worker_preflight(request: &ProofRequestV2, timeout_sec: u64) -> Self {
        Self::worker_side(
            request,
            "nightly-preflight",
            "worker-preflight",
            timeout_sec,
        )
    }

    /// Build the standalone worker's requested proof command task.
    pub(crate) fn worker(request: &ProofRequestV2, timeout_sec: u64) -> Self {
        Self::worker_side(request, "head", request.kind.key(), timeout_sec)
    }

    fn worker_side(request: &ProofRequestV2, side: &str, kind: &str, timeout_sec: u64) -> Self {
        Self {
            receipt_id: request.id.clone(),
            side: side.to_owned(),
            kind: kind.to_owned(),
            requested_by: request.requested_by.clone(),
            request_ids: request.claim_ids.clone(),
            timeout_sec,
            source: TaskSource::Worker,
            required: false,
            execution_phase: ProofExecutionPhase::Worker,
        }
    }

    fn task_id(&self) -> Result<TaskId> {
        proof_command_task_id(&self.receipt_id, &self.side)
    }

    fn source(&self) -> TaskSource {
        self.source.clone()
    }

    fn consumers(&self) -> Result<Vec<TaskConsumer>> {
        let requirement = if self.required {
            TaskRequirement::Required
        } else {
            TaskRequirement::Optional
        };
        let value = if self.required {
            TaskValueClass::GateCritical
        } else if self.request_ids.is_empty() {
            TaskValueClass::Advisory
        } else {
            TaskValueClass::ClaimDirected
        };
        let mut ids = BTreeSet::new();
        if self.kind == "focused-test" {
            ids.extend([
                "compiler".to_owned(),
                "opposition".to_owned(),
                "tests-oracle".to_owned(),
            ]);
        } else if self.kind == "focused-build" {
            ids.insert("compiler".to_owned());
        }
        ids.insert(self.execution_phase.consumer_id().to_owned());
        ids.extend(self.requested_by.iter().cloned());
        ids.extend(
            self.request_ids
                .iter()
                .map(|id| format!("proof-request:{id}")),
        );
        if ids.is_empty() {
            ids.insert(format!("proof-receipt:{}", self.receipt_id));
        }
        ids.into_iter()
            .map(|id| TaskConsumer::parse(&id, requirement, value))
            .collect()
    }
}

impl ProofTaskLedger {
    pub(crate) fn new(recorder: TaskLedgerRecorder) -> Self {
        Self {
            recorder,
            state: Arc::new(Mutex::new(ProofTaskLedgerState::default())),
        }
    }

    /// Publish the replay-verified shared TaskLedger stream.
    pub(crate) fn write_artifacts(&self, out: &std::path::Path) -> Result<()> {
        self.recorder.write_artifacts(out)
    }

    /// Record selection, queueing, admission, and setup for one approved
    /// command side. The existing lease remains the admission authority.
    pub(crate) fn begin_command(
        &self,
        task: &ProofCommandTask,
        lease: &ResourceLease,
    ) -> Result<()> {
        ensure!(
            lease.status == "granted",
            "proof command {} entered setup without a granted lease",
            task.receipt_id
        );
        ensure!(
            lease.consumer == task.receipt_id,
            "proof lease {} consumer {} does not match receipt {}",
            lease.id,
            lease.consumer,
            task.receipt_id
        );
        let task_id = task.task_id()?;
        self.register_command(task, &task_id)?;
        let at = self.recorder.now()?;
        let mut events = vec![input(
            &task_id,
            TaskEvent::Proposed {
                revision: self.recorder.revision().clone(),
                source: task.source(),
                limits: TaskExecutionLimits::new(task.timeout_sec.max(1).saturating_mul(1_000))?,
            },
        )];
        for consumer in task.consumers()? {
            events.push(input(&task_id, TaskEvent::ConsumerAttached { consumer }));
        }
        events.extend([
            input(&task_id, TaskEvent::Selected),
            input(&task_id, TaskEvent::Queued { at }),
            input(&task_id, TaskEvent::EnteredResourceWait { at }),
            input(
                &task_id,
                TaskEvent::Admitted {
                    at,
                    reservations: proof_reservations(lease, task)?,
                },
            ),
            input(&task_id, TaskEvent::SetupStarted { at }),
        ]);
        self.recorder.append(events)
    }

    /// Record a command-side decision that never entered admitted execution.
    pub(crate) fn decline_command(
        &self,
        task: &ProofCommandTask,
        disposition: TaskNonExecutionDisposition,
        reason: &str,
    ) -> Result<()> {
        let task_id = task.task_id()?;
        self.register_command(task, &task_id)?;
        let at = self.recorder.now()?;
        let mut events = vec![input(
            &task_id,
            TaskEvent::Proposed {
                revision: self.recorder.revision().clone(),
                source: task.source(),
                limits: TaskExecutionLimits::new(task.timeout_sec.max(1).saturating_mul(1_000))?,
            },
        )];
        for consumer in task.consumers()? {
            events.push(input(&task_id, TaskEvent::ConsumerAttached { consumer }));
        }
        events.push(input(
            &task_id,
            TaskEvent::TerminallyDeclined {
                at,
                disposition,
                reason: canonical_reason(reason),
                existing_receipt: None,
            },
        ));
        self.recorder.append(events)
    }

    pub(crate) fn run_started(&self, task: &ProofCommandTask) -> Result<()> {
        self.recorder.append_event(
            &task.task_id()?,
            TaskEvent::RunStarted {
                at: self.recorder.now()?,
            },
        )
    }

    pub(crate) fn process_finished(
        &self,
        task: &ProofCommandTask,
        disposition: TaskTerminalDisposition,
    ) -> Result<()> {
        ensure!(
            disposition != TaskTerminalDisposition::SetupFailed,
            "proof setup failure must use setup_failed"
        );
        self.recorder.append_event(
            &task.task_id()?,
            TaskEvent::ProcessFinished {
                at: self.recorder.now()?,
                disposition,
            },
        )
    }

    pub(crate) fn setup_failed(&self, task: &ProofCommandTask) -> Result<()> {
        let task_id = task.task_id()?;
        self.recorder.append_event(
            &task_id,
            TaskEvent::SetupFailed {
                at: self.recorder.now()?,
            },
        )?;
        self.mark_receipt_pending(&task_id)
    }

    pub(crate) fn cleanup_finished(&self, task: &ProofCommandTask) -> Result<()> {
        let task_id = task.task_id()?;
        self.recorder.append_event(
            &task_id,
            TaskEvent::CleanupFinished {
                at: self.recorder.now()?,
            },
        )?;
        self.mark_receipt_pending(&task_id)
    }

    /// Fail durable receipt publication after execution terminalized, then
    /// release admitted resources without claiming a receipt was created.
    pub(crate) fn receipt_creation_failed_and_resources_released(
        &self,
        task: &ProofCommandTask,
        reason: &str,
    ) -> Result<()> {
        let task_id = task.task_id()?;
        ensure!(
            self.receipt_pending(task)?,
            "proof task {} has no pending receipt to fail",
            task_id.as_str()
        );
        self.recorder.append([
            input(
                &task_id,
                TaskEvent::ReceiptCreationFailed {
                    at: self.recorder.now()?,
                    reason: canonical_reason(reason),
                },
            ),
            input(
                &task_id,
                TaskEvent::ResourcesReleased {
                    at: self.recorder.now()?,
                },
            ),
        ])?;
        let removed = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("proof task-ledger mutex poisoned"))?
            .pending_receipts
            .remove(task_id.as_str());
        ensure!(
            removed,
            "proof task {} pending receipt disappeared during failure reconciliation",
            task_id.as_str()
        );
        Ok(())
    }

    /// Return whether this command has completed cleanup or setup failure and
    /// is waiting for one current-attempt receipt outcome.
    pub(crate) fn receipt_pending(&self, task: &ProofCommandTask) -> Result<bool> {
        Ok(self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("proof task-ledger mutex poisoned"))?
            .pending_receipts
            .contains(task.task_id()?.as_str()))
    }

    /// Read the just-published aggregate receipt, validate its immutable
    /// revision and exact side order, then bind pending command tasks to their
    /// own receipt rows before releasing resources.
    pub(crate) fn reconcile_published_receipts(&self, out: &std::path::Path) -> Result<()> {
        let path = out.join("review/proof_receipts.json");
        let receipts: Vec<ProofReceipt> = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        let references = self.validate_receipt_rows(&receipts, ReceiptArtifact::Aggregate)?;
        self.reconcile_pending_references(&references)
    }

    /// Re-read and reconcile the canonical standalone worker receipt after it
    /// has been durably published.
    pub(crate) fn reconcile_worker_receipt(
        &self,
        out: &std::path::Path,
        request_id: &str,
    ) -> Result<()> {
        let path = out.join("proof_receipt.json");
        let receipt: ProofReceipt = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        ensure!(
            receipt.id == request_id,
            "worker receipt id {} does not match request {request_id}",
            receipt.id
        );
        let references = self
            .validate_receipt_rows(std::slice::from_ref(&receipt), ReceiptArtifact::Standalone)?;
        self.reconcile_pending_references(&references)
    }

    fn validate_receipt_rows(
        &self,
        receipts: &[ProofReceipt],
        artifact: ReceiptArtifact,
    ) -> Result<BTreeMap<String, String>> {
        let (proposed, expected_sides) = {
            let state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("proof task-ledger mutex poisoned"))?;
            (state.proposed.clone(), state.expected_sides.clone())
        };
        let mut references = BTreeMap::new();
        for (receipt_index, receipt) in receipts.iter().enumerate() {
            ensure!(
                receipt.revision.as_ref() == Some(self.recorder.revision()),
                "published proof receipt {} does not bind the admitted revision",
                receipt.id
            );
            let actual = receipt
                .commands
                .iter()
                .map(|command| command.side.as_str())
                .collect::<Vec<_>>();
            let expected = expected_sides
                .get(&receipt.id)
                .with_context(|| format!("published receipt {} has no proof task", receipt.id))?;
            ensure!(
                actual == expected.iter().map(String::as_str).collect::<Vec<_>>(),
                "published proof receipt {} command sides {:?} do not match executed order {:?}",
                receipt.id,
                actual,
                expected
            );
            for (command_index, command) in receipt.commands.iter().enumerate() {
                let task_id = proof_command_task_id(&receipt.id, &command.side)?;
                ensure!(
                    proposed.contains(task_id.as_str()),
                    "published proof command {}:{} has no proposed task",
                    receipt.id,
                    command.side
                );
                let previous = references.insert(
                    task_id.as_str().to_owned(),
                    artifact.reference(receipt_index, command_index),
                );
                ensure!(
                    previous.is_none(),
                    "published proof receipts map multiple rows to task {}",
                    task_id.as_str()
                );
            }
        }
        Ok(references)
    }

    fn reconcile_pending_references(&self, references: &BTreeMap<String, String>) -> Result<()> {
        let pending = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("proof task-ledger mutex poisoned"))?
            .pending_receipts
            .clone();
        let mut events = Vec::new();
        for task in &pending {
            let task_id = TaskId::parse(task)?;
            let reference = references
                .get(task)
                .with_context(|| format!("published proof receipt omitted task {task}"))?;
            events.extend([
                input(
                    &task_id,
                    TaskEvent::ReceiptCreated {
                        at: self.recorder.now()?,
                        reference: reference.clone(),
                    },
                ),
                input(
                    &task_id,
                    TaskEvent::ResourcesReleased {
                        at: self.recorder.now()?,
                    },
                ),
            ]);
        }
        self.recorder.append(events)?;
        self.state
            .lock()
            .map_err(|_| anyhow::anyhow!("proof task-ledger mutex poisoned"))?
            .pending_receipts
            .clear();
        Ok(())
    }

    /// Preserve every original request as a separate source proposal. The
    /// existing broker command task owns the receipt; grouped source tasks are
    /// explicitly superseded without claiming canonical equivalence.
    pub(crate) fn record_source_requests(
        &self,
        requests: &[ProofRequest],
        receipts: &[ProofReceipt],
    ) -> Result<()> {
        for request in requests {
            let task_id = proof_request_task_id(&request.id)?;
            self.register_proposal(&task_id)?;
            let at = self.recorder.now()?;
            let source = proof_request_source(request);
            let mut events = vec![input(
                &task_id,
                TaskEvent::Proposed {
                    revision: self.recorder.revision().clone(),
                    source,
                    limits: TaskExecutionLimits::new(
                        request.timeout_sec.max(1).saturating_mul(1_000),
                    )?,
                },
            )];
            for consumer in proof_request_consumers(request)? {
                events.push(input(&task_id, TaskEvent::ConsumerAttached { consumer }));
            }
            let matching_receipt = receipts
                .iter()
                .find(|receipt| receipt.request_ids.iter().any(|id| id == &request.id));
            let (disposition, reason) = source_request_disposition(request, matching_receipt);
            events.push(input(
                &task_id,
                TaskEvent::TerminallyDeclined {
                    at,
                    disposition,
                    reason,
                    existing_receipt: None,
                },
            ));
            self.recorder.append(events)?;
        }
        Ok(())
    }

    fn register_command(&self, task: &ProofCommandTask, task_id: &TaskId) -> Result<()> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("proof task-ledger mutex poisoned"))?;
        ensure!(
            state.proposed.insert(task_id.as_str().to_owned()),
            "proof task {} was proposed twice",
            task_id.as_str()
        );
        state
            .expected_sides
            .entry(task.receipt_id.clone())
            .or_default()
            .push(task.side.clone());
        Ok(())
    }

    fn register_proposal(&self, task_id: &TaskId) -> Result<()> {
        let inserted = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("proof task-ledger mutex poisoned"))?
            .proposed
            .insert(task_id.as_str().to_owned());
        ensure!(
            inserted,
            "proof task {} was proposed twice",
            task_id.as_str()
        );
        Ok(())
    }

    fn mark_receipt_pending(&self, task_id: &TaskId) -> Result<()> {
        let inserted = self
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("proof task-ledger mutex poisoned"))?
            .pending_receipts
            .insert(task_id.as_str().to_owned());
        ensure!(
            inserted,
            "proof task {} entered receipt pending twice",
            task_id.as_str()
        );
        Ok(())
    }
}

pub(crate) fn proof_command_task_id(receipt_id: &str, side: &str) -> Result<TaskId> {
    TaskId::parse(&format!(
        "proof-command-{}-{}",
        sanitize_artifact_name(receipt_id),
        sanitize_artifact_name(side)
    ))
}

fn proof_request_task_id(request_id: &str) -> Result<TaskId> {
    TaskId::parse(&format!(
        "proof-request-{}",
        sanitize_artifact_name(request_id)
    ))
}

fn proof_command_source(required: bool, phase: ProofExecutionPhase) -> TaskSource {
    if required {
        return TaskSource::Required;
    }
    match phase {
        ProofExecutionPhase::InitialImpact => TaskSource::Impact,
        ProofExecutionPhase::ModelRequest | ProofExecutionPhase::FollowUp => {
            TaskSource::ReviewerTurn { model_on: true }
        }
        ProofExecutionPhase::Worker => TaskSource::Worker,
    }
}

/// Recompute the ordinary immutable revision admission from exact worker git
/// objects and the reviewed diff. Display labels never become ledger identity.
pub(crate) fn standalone_worker_revision(
    root: &std::path::Path,
    request: &ProofRequestV2,
) -> Result<RevisionRef> {
    validate_worker_oid("base", &request.base)?;
    validate_worker_oid("head", &request.head)?;
    let diff = DiffContext::from_git(root, &request.base, &request.head)
        .context("compute standalone worker diff")?;
    let admission = admit_revision(
        root,
        &request.base,
        &request.head,
        Some(&request.head),
        &diff.changed_files,
        &diff.patch,
    )
    .context("admit standalone worker revision")?;
    let revision = RevisionRef::from_admission(&admission);
    revision.validate().context("standalone worker revision")?;
    ensure!(
        revision.reviewed_commit == request.head,
        "standalone worker admitted commit {} does not match request head {}",
        revision.reviewed_commit,
        request.head
    );
    Ok(revision)
}

fn validate_worker_oid(label: &str, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 40 || bytes.len() == 64,
        "standalone worker request {label} must be a 40- or 64-character object id"
    );
    ensure!(
        bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)),
        "standalone worker request {label} must be lowercase hexadecimal"
    );
    ensure!(
        bytes.iter().any(|byte| *byte != b'0'),
        "standalone worker request {label} cannot be the null object id"
    );
    Ok(())
}

fn proof_reservations(
    lease: &ResourceLease,
    task: &ProofCommandTask,
) -> Result<Vec<ResourceReservation>> {
    let mut reservations = Vec::new();
    if lease.cpu > 0 {
        reservations.push(ResourceReservation::new(
            TaskResourceClass::Cpu,
            u64::from(lease.cpu),
        )?);
    }
    if lease.memory_mb > 0 {
        reservations.push(ResourceReservation::new(
            TaskResourceClass::Memory,
            lease.memory_mb,
        )?);
    }
    if lease.disk_mb > 0 {
        reservations.push(ResourceReservation::new(
            TaskResourceClass::Disk,
            lease.disk_mb,
        )?);
    }
    if task.side == "base-plus-tests" && lease.worktree.is_some() {
        reservations.push(ResourceReservation::new(TaskResourceClass::Worktree, 1)?);
    }
    if lease.network {
        reservations.push(ResourceReservation::new(TaskResourceClass::Network, 1)?);
    }
    reservations.push(ResourceReservation::new(
        if task.kind == "focused-build" {
            TaskResourceClass::Build
        } else {
            TaskResourceClass::Test
        },
        1,
    )?);
    Ok(reservations)
}

fn proof_request_source(request: &ProofRequest) -> TaskSource {
    if request.required {
        TaskSource::Required
    } else if is_configured_proof(&request.requested_by)
        || request.lane == crate::REQUIRED_PROOF_POLICY_LANE
    {
        TaskSource::Configured
    } else {
        TaskSource::ReviewerTurn { model_on: true }
    }
}

fn proof_request_consumers(request: &ProofRequest) -> Result<Vec<TaskConsumer>> {
    let requirement = if request.required {
        TaskRequirement::Required
    } else {
        TaskRequirement::Optional
    };
    let value = if request.required {
        TaskValueClass::GateCritical
    } else {
        TaskValueClass::ClaimDirected
    };
    let mut ids = BTreeSet::new();
    ids.insert(request.lane.clone());
    ids.extend(request.requested_by.iter().cloned());
    ids.into_iter()
        .map(|id| TaskConsumer::parse(&id, requirement, value))
        .collect()
}

fn source_request_disposition(
    request: &ProofRequest,
    matching_receipt: Option<&ProofReceipt>,
) -> (TaskNonExecutionDisposition, String) {
    if request.status == "unsupported" {
        return (
            TaskNonExecutionDisposition::Unsupported,
            canonical_reason(&request.reason),
        );
    }
    if request.status == "invalid" {
        return (
            TaskNonExecutionDisposition::Refused,
            canonical_reason(&request.reason),
        );
    }
    if let Some(receipt) = matching_receipt {
        return (
            TaskNonExecutionDisposition::Superseded,
            format!(
                "legacy proof broker grouped source request into command receipt {}; canonical equivalence is not inferred",
                receipt.id
            ),
        );
    }
    let disposition = if matches!(request.status.as_str(), "deferred" | "requested") {
        TaskNonExecutionDisposition::BudgetDeferred
    } else {
        TaskNonExecutionDisposition::Refused
    };
    (
        disposition,
        format!("proof request terminal status {}", request.status),
    )
}

fn is_configured_proof(requested_by: &[String]) -> bool {
    requested_by
        .iter()
        .any(|id| id == crate::REQUIRED_PROOF_POLICY_LANE || id.starts_with("proof-policy:"))
}

fn canonical_reason(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_control) {
        "proof work was not executed".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn input(task_id: &TaskId, event: TaskEvent) -> TaskLedgerInput {
    TaskLedgerInput {
        task_id: task_id.clone(),
        event,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use anyhow::{Context as _, Result, ensure};

    use super::*;
    use crate::task_ledger::{TaskReceiptOutcome, TaskReducer, TaskSnapshot, TaskState};
    use crate::{ProofCommandReceipt, RevisionRef, write_proof_receipt_artifacts};

    fn revision(digest: char) -> RevisionRef {
        RevisionRef {
            digest: digest.to_string().repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: digest.to_string().repeat(40),
        }
    }

    fn recorder() -> Result<TaskLedgerRecorder> {
        TaskLedgerRecorder::new(&revision('a'), &Instant::now())
    }

    fn task(
        receipt_id: &str,
        side: &str,
        requested_by: &[&str],
        request_ids: &[&str],
    ) -> ProofCommandTask {
        let phase = if request_ids.is_empty() {
            ProofExecutionPhase::InitialImpact
        } else {
            ProofExecutionPhase::ModelRequest
        };
        task_for_phase(receipt_id, side, requested_by, request_ids, phase)
    }

    fn task_for_phase(
        receipt_id: &str,
        side: &str,
        requested_by: &[&str],
        request_ids: &[&str],
        execution_phase: ProofExecutionPhase,
    ) -> ProofCommandTask {
        let required = requested_by
            .iter()
            .any(|id| *id == crate::REQUIRED_PROOF_POLICY_LANE || id.starts_with("proof-policy:"));
        ProofCommandTask {
            receipt_id: receipt_id.to_owned(),
            side: side.to_owned(),
            kind: "focused-test".to_owned(),
            requested_by: requested_by
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            request_ids: request_ids
                .iter()
                .map(|value| (*value).to_owned())
                .collect(),
            timeout_sec: 7,
            source: proof_command_source(required, execution_phase),
            required,
            execution_phase,
        }
    }

    fn granted_lease(receipt_id: &str) -> ResourceLease {
        ResourceLease {
            revision: None,
            schema: crate::RESOURCE_LEASE_SCHEMA.to_owned(),
            id: format!("lease-{receipt_id}"),
            kind: "focused-test".to_owned(),
            consumer: receipt_id.to_owned(),
            status: "granted".to_owned(),
            reason: "fixture lease".to_owned(),
            cpu: 1,
            memory_mb: 512,
            disk_mb: 64,
            timeout_sec: 7,
            network: false,
            scratch: false,
            worktree: Some("fixture-worktree".to_owned()),
            command: None,
        }
    }

    fn command(side: &str) -> ProofCommandReceipt {
        ProofCommandReceipt {
            side: side.to_owned(),
            command: format!("cargo test {side}"),
            env: BTreeMap::new(),
            status: "passed".to_owned(),
            exit_code: Some(0),
            timed_out: false,
            timeout_sec: 7,
            duration_ms: 1,
            stdout: format!("proof/receipt-a/{side}/stdout.txt"),
            stderr: format!("proof/receipt-a/{side}/stderr.txt"),
            reason: "completed".to_owned(),
        }
    }

    fn receipt(revision: RevisionRef, sides: &[&str]) -> ProofReceipt {
        ProofReceipt {
            revision: Some(revision),
            schema: crate::PROOF_RECEIPT_SCHEMA.to_owned(),
            id: "receipt-a".to_owned(),
            kind: "focused-red-green".to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            test_patch_mode: if sides.len() == 2 {
                "base-plus-tests".to_owned()
            } else {
                "head-only".to_owned()
            },
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids: vec!["request-a".to_owned()],
            commands: sides.iter().map(|side| command(side)).collect(),
            result: "fixture".to_owned(),
            reason: "fixture".to_owned(),
        }
    }

    fn execute_side(
        ledger: &ProofTaskLedger,
        task: &ProofCommandTask,
        lease: &ResourceLease,
    ) -> Result<()> {
        ledger.begin_command(task, lease)?;
        ledger.run_started(task)?;
        ledger.process_finished(task, TaskTerminalDisposition::Succeeded)?;
        ledger.cleanup_finished(task)
    }

    fn state_for(recorder: &TaskLedgerRecorder, task_id: &TaskId) -> Result<TaskSnapshot> {
        let mut reducer = TaskReducer::new();
        for input in recorder
            .inputs()?
            .iter()
            .filter(|input| input.task_id == *task_id)
        {
            reducer.apply(task_id, &input.event, recorder.revision())?;
        }
        reducer
            .snapshot()
            .cloned()
            .with_context(|| format!("snapshot omitted {}", task_id.as_str()))
    }

    #[test]
    fn command_sources_preserve_impact_required_and_model_provenance() -> Result<()> {
        ensure!(task("impact", "head", &["impact-planner"], &[]).source() == TaskSource::Impact);
        ensure!(
            task("required", "head", &["proof-policy:smoke"], &["request-a"]).source()
                == TaskSource::Required
        );
        ensure!(
            task("model", "head", &["opposition"], &["request-b"]).source()
                == TaskSource::ReviewerTurn { model_on: true }
        );
        let follow_up = task_for_phase(
            "follow-up",
            "head",
            &["opposition"],
            &["request-c"],
            ProofExecutionPhase::FollowUp,
        );
        ensure!(follow_up.source() == TaskSource::ReviewerTurn { model_on: true });
        ensure!(
            follow_up
                .consumers()?
                .iter()
                .any(|consumer| consumer.id() == "proof-phase:follow-up")
        );
        Ok(())
    }

    #[test]
    fn required_model_request_produces_required_gate_critical_command_task() -> Result<()> {
        let request = ProofRequest {
            schema: crate::PROOF_REQUEST_SCHEMA.to_owned(),
            id: "required-model-proof".to_owned(),
            lane: "opposition".to_owned(),
            requested_by: vec!["opposition".to_owned()],
            command: "cargo test --locked --test config_tests".to_owned(),
            reason: "required evidence for a load-bearing objection".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 30,
            required: true,
            status: "requested".to_owned(),
        };
        let candidate = crate::focused_test_candidates_from_requests(&[request])
            .into_iter()
            .next()
            .context("required request must produce a focused test candidate")?;
        ensure!(candidate.required);

        let command_task = ProofCommandTask::focused_test(
            &candidate,
            "head",
            30,
            ProofExecutionPhase::ModelRequest,
        );
        ensure!(command_task.required);
        ensure!(command_task.source() == TaskSource::Required);
        let consumers = command_task.consumers()?;
        ensure!(consumers.contains(&crate::task_ledger::TaskConsumer::parse(
            "proof-request:required-model-proof",
            crate::task_ledger::TaskRequirement::Required,
            crate::task_ledger::TaskValueClass::GateCritical,
        )?));
        ensure!(consumers.contains(&crate::task_ledger::TaskConsumer::parse(
            "proof-phase:model-request",
            crate::task_ledger::TaskRequirement::Required,
            crate::task_ledger::TaskValueClass::GateCritical,
        )?));
        Ok(())
    }

    #[test]
    fn worker_tasks_are_optional_and_source_bound() -> Result<()> {
        let request = ProofRequestV2 {
            schema: crate::artifacts::PROOF_REQUEST_V2_SCHEMA.to_owned(),
            id: "worker-source".to_owned(),
            kind: crate::ProofKind::FocusedTest,
            target: "cargo test --locked worker_source".to_owned(),
            claim_ids: vec!["claim-a".to_owned()],
            requested_by: vec!["controller".to_owned()],
            expected_interpretation: "worker source remains explicit".to_owned(),
            priority: "high".to_owned(),
            timeout_sec: 7,
            status: "approved".to_owned(),
            base: "a".repeat(40),
            head: "b".repeat(40),
        };
        let worker = ProofCommandTask::worker(&request, 7);

        ensure!(worker.source() == TaskSource::Worker);
        ensure!(!worker.required);
        Ok(())
    }

    #[test]
    fn command_receipt_and_release_wait_for_current_aggregate_publication() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let recorder = recorder()?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let command_task = task("receipt-a", "head", &["tests-oracle"], &["request-a"]);
        execute_side(&ledger, &command_task, &granted_lease("receipt-a"))?;

        let before = state_for(&recorder, &command_task.task_id()?)?;
        ensure!(before.state == TaskState::ReceiptPending(TaskTerminalDisposition::Succeeded));
        ensure!(before.receipt.is_none());
        ensure!(before.timing.resources_released_at.is_none());

        let published = receipt(revision('a'), &["head"]);
        write_proof_receipt_artifacts(temp.path(), &[published], Some(&revision('a')))?;
        ledger.reconcile_published_receipts(temp.path())?;

        let after = state_for(&recorder, &command_task.task_id()?)?;
        ensure!(after.state == TaskState::ResourcesReleased(TaskTerminalDisposition::Succeeded));
        ensure!(matches!(
            after.receipt,
            Some(TaskReceiptOutcome::Created { reference })
                if reference == "review/proof_receipts.json#/0/commands/0"
        ));
        Ok(())
    }

    #[test]
    fn stale_revision_receipt_cannot_satisfy_current_attempt() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let recorder = recorder()?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let command_task = task("receipt-a", "head", &["tests-oracle"], &["request-a"]);
        execute_side(&ledger, &command_task, &granted_lease("receipt-a"))?;

        let review = temp.path().join("review");
        fs::create_dir_all(&review)?;
        fs::write(
            review.join("proof_receipts.json"),
            serde_json::to_vec_pretty(&vec![receipt(revision('b'), &["head"])])?,
        )?;

        let error = ledger
            .reconcile_published_receipts(temp.path())
            .err()
            .context("stale revision must be rejected")?;
        ensure!(format!("{error:#}").contains("does not bind the admitted revision"));
        let task = state_for(&recorder, &command_task.task_id()?)?;
        ensure!(task.state == TaskState::ReceiptPending(TaskTerminalDisposition::Succeeded));
        ensure!(task.receipt.is_none());
        ensure!(task.timing.resources_released_at.is_none());
        Ok(())
    }

    #[test]
    fn worker_receipt_reconciliation_binds_standalone_artifact() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let revision = revision('a');
        let recorder = TaskLedgerRecorder::new(&revision, &Instant::now())?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let request = ProofRequestV2 {
            schema: crate::artifacts::PROOF_REQUEST_V2_SCHEMA.to_owned(),
            id: "worker-request".to_owned(),
            kind: crate::ProofKind::FocusedTest,
            target: "cargo test --locked".to_owned(),
            claim_ids: vec!["claim-a".to_owned()],
            requested_by: vec!["controller".to_owned()],
            expected_interpretation: "same receipt contract".to_owned(),
            priority: "high".to_owned(),
            timeout_sec: 7,
            status: "approved".to_owned(),
            base: "a".repeat(40),
            head: "b".repeat(40),
        };
        let preflight = ProofCommandTask::worker_preflight(&request, 7);
        let command_task = ProofCommandTask::worker(&request, 7);
        execute_side(&ledger, &preflight, &granted_lease("worker-request"))?;
        execute_side(&ledger, &command_task, &granted_lease("worker-request"))?;
        let published = ProofReceipt {
            revision: Some(revision),
            schema: crate::PROOF_RECEIPT_SCHEMA.to_owned(),
            id: request.id.clone(),
            kind: request.kind.key().to_owned(),
            base: request.base,
            head: request.head,
            test_patch_mode: "head-only".to_owned(),
            requested_by: request.requested_by,
            request_ids: request.claim_ids,
            commands: vec![command("nightly-preflight"), command("head")],
            result: "passed".to_owned(),
            reason: "fixture".to_owned(),
        };
        fs::write(
            temp.path().join("proof_receipt.json"),
            serde_json::to_vec_pretty(&published)?,
        )?;

        ledger.reconcile_worker_receipt(temp.path(), "worker-request")?;

        for (task, index) in [(&preflight, 0), (&command_task, 1)] {
            let snapshot = state_for(&recorder, &task.task_id()?)?;
            ensure!(matches!(
                snapshot.receipt,
                Some(TaskReceiptOutcome::Created { reference })
                    if reference == format!("proof_receipt.json#/commands/{index}")
            ));
            ensure!(snapshot.timing.resources_released_at.is_some());
        }
        Ok(())
    }

    #[test]
    fn red_green_side_swap_is_rejected_without_receipt_or_release() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let recorder = recorder()?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let lease = granted_lease("receipt-a");
        let head = task("receipt-a", "head", &["tests-oracle"], &["request-a"]);
        let base = task(
            "receipt-a",
            "base-plus-tests",
            &["tests-oracle"],
            &["request-a"],
        );
        execute_side(&ledger, &head, &lease)?;
        execute_side(&ledger, &base, &lease)?;
        let review = temp.path().join("review");
        fs::create_dir_all(&review)?;
        fs::write(
            review.join("proof_receipts.json"),
            serde_json::to_vec_pretty(&vec![receipt(revision('a'), &["base-plus-tests", "head"])])?,
        )?;

        let error = ledger
            .reconcile_published_receipts(temp.path())
            .err()
            .context("side swap must be rejected")?;
        ensure!(format!("{error:#}").contains("do not match executed order"));
        for command_task in [&head, &base] {
            let snapshot = state_for(&recorder, &command_task.task_id()?)?;
            ensure!(
                snapshot.state == TaskState::ReceiptPending(TaskTerminalDisposition::Succeeded)
            );
            ensure!(snapshot.receipt.is_none());
            ensure!(snapshot.timing.resources_released_at.is_none());
        }
        Ok(())
    }

    #[test]
    fn duplicate_source_requests_remain_distinct_without_receipt_substitution() -> Result<()> {
        let recorder = recorder()?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let requests = [
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "request-a".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "cargo test --locked".to_owned(),
                reason: "first source".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 7,
                required: false,
                status: "satisfied".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "request-b".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command: "cargo test --locked".to_owned(),
                reason: "second source".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 7,
                required: false,
                status: "deduplicated".to_owned(),
            },
        ];
        let receipts = [ProofReceipt {
            revision: Some(revision('a')),
            schema: crate::PROOF_RECEIPT_SCHEMA.to_owned(),
            id: "receipt-a".to_owned(),
            kind: "focused-test".to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["tests-oracle".to_owned(), "opposition".to_owned()],
            request_ids: vec!["request-a".to_owned(), "request-b".to_owned()],
            commands: Vec::new(),
            result: "head_passed".to_owned(),
            reason: "fixture".to_owned(),
        }];

        ledger.record_source_requests(&requests, &receipts)?;

        let inputs = recorder.inputs()?;
        let ids = inputs
            .iter()
            .filter_map(|input| match &input.event {
                TaskEvent::Proposed { .. }
                    if input.task_id.as_str().starts_with("proof-request-") =>
                {
                    Some(input.task_id.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        ensure!(ids == ["proof-request-request-a", "proof-request-request-b"]);
        ensure!(
            inputs
                .iter()
                .filter(|input| matches!(
                    input.event,
                    TaskEvent::TerminallyDeclined {
                        disposition: TaskNonExecutionDisposition::Superseded,
                        existing_receipt: None,
                        ..
                    }
                ))
                .count()
                == 2
        );
        Ok(())
    }
}
