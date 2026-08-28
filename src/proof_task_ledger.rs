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
    FocusedBuildTask, FocusedTestTask, ProofReceipt, ProofRequest, ResourceLease,
    TaskLedgerRecorder, sanitize_artifact_name,
};

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
}

impl ProofCommandTask {
    pub(crate) fn focused_test(task: &FocusedTestTask, side: &str, timeout_sec: u64) -> Self {
        Self {
            receipt_id: task.id.clone(),
            side: side.to_owned(),
            kind: "focused-test".to_owned(),
            requested_by: task.requested_by.clone(),
            request_ids: task.request_ids.clone(),
            timeout_sec,
        }
    }

    pub(crate) fn focused_build(task: &FocusedBuildTask, timeout_sec: u64) -> Self {
        Self {
            receipt_id: task.id.clone(),
            side: "head".to_owned(),
            kind: "focused-build".to_owned(),
            requested_by: task.requested_by.clone(),
            request_ids: task.request_ids.clone(),
            timeout_sec,
        }
    }

    fn task_id(&self) -> Result<TaskId> {
        proof_command_task_id(&self.receipt_id, &self.side)
    }

    fn source(&self) -> TaskSource {
        if is_configured_proof(&self.requested_by) {
            TaskSource::Required
        } else if self.request_ids.is_empty() {
            TaskSource::Impact
        } else {
            TaskSource::ReviewerTurn { model_on: true }
        }
    }

    fn consumers(&self) -> Result<Vec<TaskConsumer>> {
        let required = is_configured_proof(&self.requested_by);
        let requirement = if required {
            TaskRequirement::Required
        } else {
            TaskRequirement::Optional
        };
        let value = if required {
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

    /// Read the just-published aggregate receipt, validate its immutable
    /// revision and exact side order, then bind pending command tasks to their
    /// own receipt rows before releasing resources.
    pub(crate) fn reconcile_published_receipts(&self, out: &std::path::Path) -> Result<()> {
        let path = out.join("review/proof_receipts.json");
        let receipts: Vec<ProofReceipt> = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?;
        let (proposed, expected_sides, pending) = {
            let state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("proof task-ledger mutex poisoned"))?;
            (
                state.proposed.clone(),
                state.expected_sides.clone(),
                state.pending_receipts.clone(),
            )
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
                    format!("review/proof_receipts.json#/{receipt_index}/commands/{command_index}"),
                );
                ensure!(
                    previous.is_none(),
                    "published proof receipts map multiple rows to task {}",
                    task_id.as_str()
                );
            }
        }
        for task in &pending {
            let task_id = TaskId::parse(task)?;
            let at = self.recorder.now()?;
            let reference = references
                .get(task)
                .with_context(|| format!("published proof receipt omitted task {task}"))?;
            self.recorder.append([
                input(
                    &task_id,
                    TaskEvent::ReceiptCreated {
                        at,
                        reference: reference.clone(),
                    },
                ),
                input(
                    &task_id,
                    TaskEvent::ResourcesReleased {
                        at: self.recorder.now()?,
                    },
                ),
            ])?;
        }
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
    use crate::task_ledger::{
        TaskReceiptOutcome, TaskReducer, TaskSnapshot, TaskState,
    };
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
