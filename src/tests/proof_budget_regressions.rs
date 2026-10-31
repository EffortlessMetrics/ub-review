//! File-disjoint proof-budget evidence. These tests observe legacy execution;
//! they do not activate #992 duration charging or #1306 queue authority.

use std::fs;
use std::time::Instant;

use anyhow::{Context, Result, ensure};

use crate::task_ledger::{
    TaskNonExecutionDisposition, TaskSnapshot, TaskSource, TaskState, TaskTerminalDisposition,
};
use crate::task_ledger_artifact::build_task_ledger_artifacts;
use crate::tests::{test_diff, test_plan, test_run_args, test_terminal_state};
use crate::*;

fn budget() -> ProofBudget {
    ProofBudget {
        max_focused_test_files: 4,
        max_focused_tests: 4,
        per_command_timeout_sec: 600,
        max_total_seconds: 1_500,
    }
}

fn build_profile() -> Profile {
    Profile {
        limits: Limits {
            builds: 1,
            ..Limits::default()
        },
        ..Profile::default()
    }
}

fn revision() -> RevisionRef {
    RevisionRef {
        digest: "a".repeat(64),
        semantics: "candidate_head".to_owned(),
        reviewed_commit: "b".repeat(40),
    }
}

fn request(id: &str, timeout_sec: u64) -> ProofRequest {
    ProofRequest {
        schema: PROOF_REQUEST_SCHEMA.to_owned(),
        id: id.to_owned(),
        lane: REQUIRED_PROOF_POLICY_LANE.to_owned(),
        requested_by: vec![REQUIRED_PROOF_POLICY_LANE.to_owned()],
        command: format!("cargo check --package {id} --locked"),
        reason: "required build obligation".to_owned(),
        cost: "focused-build".to_owned(),
        timeout_sec,
        required: true,
        status: "requested".to_owned(),
    }
}

fn runtime(deadline_remaining_seconds: u64) -> ProofPortfolioRuntime {
    ProofPortfolioRuntime {
        cpus: 4,
        free_mem_mb: Some(8_192),
        free_disk_mb: Some(20_000),
        load_1m: Some(0.25),
        github_actions: false,
        deadline_remaining_seconds,
        lease: ProofLeaseBudget {
            cpu: 1,
            memory_mb: 512,
            disk_mb: 64,
            network: false,
            scratch: true,
        },
    }
}

#[test]
fn granted_1500_seconds_preserves_63_seconds_of_actual_command_duration() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path().join("out");
    let requests = [
        request("first", 600),
        request("second", 600),
        request("third", 300),
    ];
    let tasks = focused_build_candidates_from_requests(&requests);
    ensure!(tasks.len() == 3);
    let mut observed_timeouts = Vec::new();
    let result = run_focused_build_proof_tasks_with_runner(
        temp.path(),
        &out,
        &test_diff(),
        &build_profile(),
        &test_run_args(out.clone()),
        budget(),
        tasks,
        None,
        ProofExecutionPhase::ModelRequest,
        |_root, _argv, _env, timeout, stdout, stderr, _observe| {
            observed_timeouts.push(timeout);
            fs::write(stdout, b"ok\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(0),
                timed_out: false,
                success: true,
                reason: "completed".to_owned(),
                duration_ms: 21_000,
            })
        },
    )?;
    ensure!(observed_timeouts == [600, 600, 300]);
    ensure!(result.resource_leases.len() == 3);
    ensure!(result.proof_receipts.len() == 3);
    let mut actual_ms = 0;
    for (receipt, lease) in result.proof_receipts.iter().zip(&result.resource_leases) {
        ensure!(lease.status == "granted");
        ensure!(lease.consumer == receipt.id);
        ensure!(receipt.result == "head_passed");
        ensure!(receipt.commands.len() == 1);
        let command = &receipt.commands[0];
        ensure!(command.duration_ms == 21_000);
        ensure!(command.timeout_sec == lease.timeout_sec);
        ensure!(command.status == "passed");
        ensure!(command.duration_ms < u128::from(lease.timeout_sec) * 1_000);
        actual_ms += command.duration_ms;
    }
    ensure!(actual_ms == 63_000);
    ensure!(
        result
            .resource_leases
            .iter()
            .map(|lease| lease.timeout_sec)
            .sum::<u64>()
            == 1_500
    );
    // Deliberately do not pin legacy remaining capacity as the desired result:
    // #992 must release the unused reservation after scheduler authority #991.
    Ok(())
}

#[test]
fn required_wind_down_distinguishes_allocation_from_live_deadline() -> Result<()> {
    let required = request("required", 60);
    let tasks = focused_build_candidates_from_requests(std::slice::from_ref(&required));
    ensure!(tasks.len() == 1);
    for (allocation, deadline, expected_reason) in [
        (0, 900, "proof-time allocation is exhausted"),
        (
            1_500,
            0,
            "hard deadline has expired; proof was safely wound down",
        ),
        (
            1_500,
            59,
            "estimated task cost cannot fit the remaining hard-deadline wall time",
        ),
    ] {
        let case_budget = ProofBudget {
            max_total_seconds: allocation,
            ..budget()
        };
        let selection = select_proof_portfolio(ProofPortfolioInput {
            test_tasks: &[],
            build_tasks: &tasks,
            proof_requests: std::slice::from_ref(&required),
            proof_receipts: &[],
            head: "HEAD",
            budget: case_budget,
            runtime: runtime(deadline),
        });
        ensure!(selection.build_tasks.is_empty());
        ensure!(selection.decisions.len() == 1);
        let decision = &selection.decisions[0];
        ensure!(decision.required);
        ensure!(decision.status == "deferred_by_safe_wind_down");
        ensure!(decision.reason == expected_reason, "{decision:?}");
        ensure!(decision.receipt_ids.is_empty());
        ensure!(decision.request_ids.as_slice() == std::slice::from_ref(&required.id));
        let snapshot = serde_json::from_value(serde_json::json!({
            "decisions": selection.decisions,
        }))?;
        let accounting = crate::gate::build_planner_required_proof_accounting(
            &snapshot,
            &[],
            1,
            Some(&revision().digest),
        );
        ensure!(accounting.total == 1);
        ensure!(accounting.proven == 0);
        ensure!(accounting.unproven == 1);
        ensure!(accounting.gate_required_requests == 1);
        ensure!(accounting.unproven_tasks.len() == 1);
        ensure!(accounting.unproven_tasks[0].reason == expected_reason);
    }
    // Equality is admissible; one second short is the independent negative.
    let boundary = select_proof_portfolio(ProofPortfolioInput {
        test_tasks: &[],
        build_tasks: &tasks,
        proof_requests: std::slice::from_ref(&required),
        proof_receipts: &[],
        head: "HEAD",
        budget: budget(),
        runtime: runtime(60),
    });
    ensure!(boundary.build_tasks.len() == 1);
    ensure!(boundary.decisions[0].status == "selected");
    ensure!(boundary.remaining_seconds == 0);
    Ok(())
}

#[test]
fn unexecuted_required_requests_have_terminal_nonexecution_without_fake_receipts() -> Result<()> {
    for (status, expected_disposition) in [
        ("requested", TaskNonExecutionDisposition::BudgetDeferred),
        ("deferred", TaskNonExecutionDisposition::BudgetDeferred),
        ("unsupported", TaskNonExecutionDisposition::Unsupported),
        ("invalid", TaskNonExecutionDisposition::Refused),
    ] {
        let revision = revision();
        let recorder = TaskLedgerRecorder::new(&revision, &Instant::now())?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let mut required = request("unexecuted", 600);
        required.status = status.to_owned();
        ledger.record_source_requests(&[required], &[])?;
        let artifacts = build_task_ledger_artifacts(&revision, &recorder.inputs()?)?;
        let snapshot: serde_json::Value = serde_json::from_slice(&artifacts.snapshot_json)?;
        let tasks: Vec<TaskSnapshot> = serde_json::from_value(snapshot["tasks"].clone())?;
        ensure!(tasks.len() == 1);
        let task = tasks.first().context("missing Required task")?;
        ensure!(task.source == TaskSource::Required);
        ensure!(task.state == TaskState::TerminallyDeclined(expected_disposition));
        ensure!(task.non_execution_disposition == Some(expected_disposition));
        ensure!(task.execution_disposition.is_none());
        ensure!(task.receipt.is_none());
        ensure!(task.existing_receipt.is_none());
        ensure!(task.reservations.is_empty());
        ensure!(task.timing.process_ms.is_none());
        ensure!(!task.consumers.is_empty());
        let consumers = serde_json::to_value(&task.consumers)?;
        ensure!(
            consumers
                .as_array()
                .context("consumer array")?
                .iter()
                .all(|consumer| consumer["requirement"] == "Required"
                    && consumer["value"] == "GateCritical")
        );
    }
    Ok(())
}

#[test]
fn required_build_exhaustion_terminalizes_declined_command_and_releases_completed_command()
-> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path().join("out");
    let requests = [request("first", 600), request("second", 600)];
    let tasks = focused_build_candidates_from_requests(&requests);
    ensure!(tasks.len() == 2);
    let admitted_id = proof_command_task_id(&tasks[0].id, "head")?;
    let declined_id = proof_command_task_id(&tasks[1].id, "head")?;
    let revision = revision();
    let recorder = TaskLedgerRecorder::new(&revision, &Instant::now())?;
    let ledger = ProofTaskLedger::new(recorder.clone());
    let mut launched = 0;
    let mut result = run_focused_build_proof_tasks_with_runner(
        temp.path(),
        &out,
        &test_diff(),
        &build_profile(),
        &test_run_args(out.clone()),
        ProofBudget {
            max_total_seconds: 600,
            ..budget()
        },
        tasks,
        Some(&ledger),
        ProofExecutionPhase::ModelRequest,
        |_root, _argv, _env, timeout, stdout, stderr, observe| {
            launched += 1;
            ensure!(timeout == 600);
            observe(CommandProcessObservation::Spawned);
            fs::write(stdout, b"ok\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(0),
                timed_out: false,
                success: true,
                reason: "completed".to_owned(),
                duration_ms: 1_000,
            })
        },
    )?;
    ensure!(launched == 1);
    ensure!(result.proof_receipts.len() == 2);
    ensure!(result.resource_leases.len() == 2);
    ensure!(result.resource_leases[0].status == "granted");
    ensure!(result.resource_leases[1].status == "exhausted");
    ensure!(result.proof_receipts[0].result == "head_passed");
    ensure!(result.proof_receipts[0].commands[0].duration_ms == 1_000);
    ensure!(result.proof_receipts[1].result == "skipped_budget");
    ensure!(
        result.proof_receipts[1]
            .commands
            .iter()
            .all(|command| command.status == "skipped")
    );
    for receipt in &mut result.proof_receipts {
        receipt.revision = Some(revision.clone());
    }
    fs::create_dir_all(out.join("review"))?;
    fs::write(
        out.join("review/proof_receipts.json"),
        serde_json::to_vec(&result.proof_receipts)?,
    )?;
    ledger.reconcile_published_receipts(&out)?;
    ledger.record_source_requests(&requests, &result.proof_receipts)?;
    let artifacts = build_task_ledger_artifacts(&revision, &recorder.inputs()?)?;
    let snapshot: serde_json::Value = serde_json::from_slice(&artifacts.snapshot_json)?;
    let tasks: Vec<TaskSnapshot> = serde_json::from_value(snapshot["tasks"].clone())?;
    ensure!(
        tasks.len() == 4,
        "two commands and two source proposals must remain distinct"
    );
    let admitted = tasks
        .iter()
        .find(|task| task.id == admitted_id)
        .context("missing executed command")?;
    ensure!(admitted.state == TaskState::ResourcesReleased(TaskTerminalDisposition::Succeeded));
    ensure!(admitted.resources_released);
    ensure!(admitted.receipt.is_some());
    ensure!(admitted.limits.timeout_ceiling_ms() == 600_000);
    let declined = tasks
        .iter()
        .find(|task| task.id == declined_id)
        .context("missing declined command")?;
    ensure!(declined.source == TaskSource::Required);
    ensure!(
        declined.state
            == TaskState::TerminallyDeclined(TaskNonExecutionDisposition::BudgetDeferred)
    );
    ensure!(declined.execution_disposition.is_none());
    ensure!(declined.receipt.is_none());
    ensure!(declined.reservations.is_empty());
    ensure!(declined.timing.process_ms.is_none());
    let mut args = test_run_args(out.clone());
    args.mode = RunMode::IntelligentCi;
    let mut config = Config::default();
    config.gate.blocking.required_proof_unproven = true;
    let plan = test_plan(Vec::new());
    let terminal = test_terminal_state("sufficient");
    let gate = build_gate_outcome(GateOutcomeInput {
        args: &args,
        config: &config,
        plan: &plan,
        terminal_state: &terminal,
        proof_requests: &requests,
        proof_receipts: &result.proof_receipts,
        tool_gate_outcomes: &[],
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        reporter_gate: ReporterGateInput::Absent,
        planner_portfolio: None,
        revision_digest: Some(revision.digest),
    });
    ensure!(gate.conclusion == "fail");
    ensure!(gate.required_proof.matched == 2);
    ensure!(gate.required_proof.passed == 1);
    ensure!(gate.required_proof.skipped == 1);
    ensure!(gate.required_proof.failed == 0);
    ensure!(gate.reasons.len() == 1);
    ensure!(gate.reasons[0].id == "second");
    ensure!(
        gate.reasons[0].receipt
            == format!("review/proof_receipts.json#{}", result.proof_receipts[1].id)
    );
    Ok(())
}
