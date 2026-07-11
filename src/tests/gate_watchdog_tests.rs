use super::*;

fn sha(byte: char) -> String {
    byte.to_string().repeat(40)
}

fn terminal_input(conclusion: &str) -> GateWatchdogInput {
    GateWatchdogInput {
        schema: GATE_WATCHDOG_INPUT_SCHEMA.to_owned(),
        expected_head_sha: sha('a'),
        required_check: GateWatchdogRequiredCheck {
            name: "ub-review/gate".to_owned(),
            app_id: 15_368,
        },
        expected_required_proof_ids: vec!["cargo-check".to_owned()],
        deadline_reached: true,
        observation_receipt: "input/gate-watchdog-observations.json".to_owned(),
        runs: vec![GateWatchdogRunObservation {
            run_id: 101,
            attempt: 1,
            check_run_id: 1_001,
            check_name: "ub-review/gate".to_owned(),
            check_app_id: 15_368,
            head_sha: sha('a'),
            status: GateWatchdogRunStatus::Completed,
            conclusion: Some(
                if conclusion == "pass" {
                    "success"
                } else {
                    "failure"
                }
                .to_owned(),
            ),
            receipt: "github/workflow-runs.json#101".to_owned(),
        }],
        gate_artifact: Some(GateWatchdogGateArtifact {
            run_id: 101,
            attempt: 1,
            head_sha: sha('a'),
            artifact_id: 2_001,
            artifact_name: "ub-review-review-101".to_owned(),
            schema: GATE_OUTCOME_SCHEMA.to_owned(),
            conclusion: conclusion.to_owned(),
            receipt: "review/gate_outcome.json".to_owned(),
        }),
        coordinator: Some(GateWatchdogCoordinatorObservation {
            run_id: 101,
            attempt: 1,
            head_sha: sha('a'),
            status: GateWatchdogCoordinatorStatus::Terminal,
            receipt: "review/coordinator-terminal.json".to_owned(),
        }),
        required_proofs: vec![GateWatchdogProofObservation {
            id: "cargo-check".to_owned(),
            run_id: 101,
            attempt: 1,
            head_sha: sha('a'),
            status: GateWatchdogProofStatus::Terminal,
            receipt: "review/proof_receipts.json#cargo-check".to_owned(),
        }],
    }
}

fn first_run_mut(input: &mut GateWatchdogInput) -> Result<&mut GateWatchdogRunObservation> {
    input
        .runs
        .first_mut()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no run"))
}

fn first_proof_mut(input: &mut GateWatchdogInput) -> Result<&mut GateWatchdogProofObservation> {
    input
        .required_proofs
        .first_mut()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no proof"))
}

fn ensure_classification_error_contains(input: &GateWatchdogInput, expected: &str) -> Result<()> {
    let error = classify_gate_watchdog(input)
        .err()
        .ok_or_else(|| anyhow::anyhow!("watchdog input unexpectedly classified successfully"))?;
    let rendered = format!("{error:#}");
    anyhow::ensure!(
        rendered.contains(expected),
        "watchdog error {rendered:?} did not contain {expected:?}"
    );
    Ok(())
}

#[test]
fn exact_head_terminal_conclusions_are_preserved() -> Result<()> {
    for conclusion in ["pass", "fail", "inconclusive"] {
        let artifact = classify_gate_watchdog(&terminal_input(conclusion))?;
        anyhow::ensure!(artifact.state == GateWatchdogState::Terminal);
        anyhow::ensure!(artifact.conclusion.as_deref() == Some(conclusion));
        anyhow::ensure!(artifact.reasons.is_empty());
        anyhow::ensure!(artifact.selected_run_id == Some(101));
    }
    Ok(())
}

#[test]
fn sha_hex_case_does_not_change_current_head_identity() -> Result<()> {
    let mut input = terminal_input("pass");
    input.expected_head_sha = input.expected_head_sha.to_ascii_uppercase();

    let artifact = classify_gate_watchdog(&input)?;

    anyhow::ensure!(artifact.state == GateWatchdogState::Terminal);
    anyhow::ensure!(artifact.conclusion.as_deref() == Some("pass"));
    anyhow::ensure!(artifact.selected_run_id == Some(101));
    Ok(())
}

#[test]
fn unavailable_current_head_states_are_never_pass() -> Result<()> {
    let mut cases = Vec::new();

    let mut missing = terminal_input("pass");
    missing.runs.clear();
    missing.gate_artifact = None;
    missing.coordinator = None;
    cases.push(("missing-run", missing));

    let mut cancelled = terminal_input("pass");
    first_run_mut(&mut cancelled)?.conclusion = Some("cancelled".to_owned());
    cancelled.gate_artifact = None;
    cases.push(("cancelled-without-replacement", cancelled));

    let mut missing_conclusion = terminal_input("pass");
    first_run_mut(&mut missing_conclusion)?.conclusion = None;
    missing_conclusion.gate_artifact = None;
    cases.push(("run-conclusion-missing", missing_conclusion));

    let mut stale = terminal_input("pass");
    first_run_mut(&mut stale)?.head_sha = sha('b');
    stale.gate_artifact = None;
    stale.coordinator = None;
    cases.push(("stale-success", stale));

    let mut missing_artifact = terminal_input("pass");
    missing_artifact.gate_artifact = None;
    cases.push(("missing-artifact", missing_artifact));

    let mut expired = terminal_input("pass");
    let run = first_run_mut(&mut expired)?;
    run.status = GateWatchdogRunStatus::InProgress;
    run.conclusion = None;
    expired.gate_artifact = None;
    cases.push(("run-deadline-exceeded", expired));

    let mut crashed = terminal_input("pass");
    crashed.coordinator = Some(GateWatchdogCoordinatorObservation {
        run_id: 101,
        attempt: 1,
        head_sha: sha('a'),
        status: GateWatchdogCoordinatorStatus::Crashed,
        receipt: "review/coordinator-terminal.json".to_owned(),
    });
    cases.push(("coordinator-crash", crashed));

    let mut orphaned = terminal_input("pass");
    first_proof_mut(&mut orphaned)?.status = GateWatchdogProofStatus::Orphaned;
    cases.push(("required-proof-orphaned", orphaned));

    let mut stale_coordinator = terminal_input("pass");
    let coordinator = stale_coordinator
        .coordinator
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no coordinator"))?;
    coordinator.attempt = 2;
    cases.push(("coordinator-run-mismatch", stale_coordinator));

    let mut stale_proof = terminal_input("pass");
    first_proof_mut(&mut stale_proof)?.attempt = 2;
    cases.push(("required-proof-run-mismatch", stale_proof));

    let mut stale_artifact = terminal_input("pass");
    let gate = stale_artifact
        .gate_artifact
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no gate artifact"))?;
    gate.attempt = 2;
    cases.push(("artifact-run-mismatch", stale_artifact));

    let mut wrong_sha = terminal_input("pass");
    let gate = wrong_sha
        .gate_artifact
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no gate artifact"))?;
    gate.head_sha = sha('b');
    cases.push(("artifact-sha-mismatch", wrong_sha));

    let mut wrong_schema = terminal_input("pass");
    let gate = wrong_schema
        .gate_artifact
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no gate artifact"))?;
    gate.schema = "ub-review.gate_outcome.v0".to_owned();
    cases.push(("artifact-schema-mismatch", wrong_schema));

    let mut invalid_conclusion = terminal_input("pass");
    let gate = invalid_conclusion
        .gate_artifact
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no gate artifact"))?;
    gate.conclusion = "unknown".to_owned();
    cases.push(("artifact-conclusion-invalid", invalid_conclusion));

    let mut disagreement = terminal_input("pass");
    let gate = disagreement
        .gate_artifact
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no gate artifact"))?;
    gate.conclusion = "fail".to_owned();
    cases.push(("run-artifact-disagreement", disagreement));

    for (expected_kind, input) in cases {
        let artifact = classify_gate_watchdog(&input)?;
        anyhow::ensure!(artifact.state == GateWatchdogState::Inconclusive);
        anyhow::ensure!(artifact.conclusion.as_deref() == Some("inconclusive"));
        anyhow::ensure!(artifact.conclusion.as_deref() != Some("pass"));
        let reason = artifact
            .reasons
            .first()
            .ok_or_else(|| anyhow::anyhow!("{expected_kind} produced no reason"))?;
        anyhow::ensure!(reason.kind == expected_kind);
        anyhow::ensure!(!reason.receipt.is_empty());
        anyhow::ensure!(!reason.retry_action.is_empty());
    }
    Ok(())
}

#[test]
fn transient_observation_gaps_wait_until_deadline() -> Result<()> {
    let mut cases = Vec::new();

    let mut stale = terminal_input("pass");
    stale.deadline_reached = false;
    first_run_mut(&mut stale)?.head_sha = sha('b');
    stale.gate_artifact = None;
    stale.coordinator = None;
    cases.push(("stale-success", stale));

    let mut cancelled = terminal_input("pass");
    cancelled.deadline_reached = false;
    first_run_mut(&mut cancelled)?.conclusion = Some("cancelled".to_owned());
    cancelled.gate_artifact = None;
    cases.push(("cancelled-without-replacement", cancelled));

    let mut missing_conclusion = terminal_input("pass");
    missing_conclusion.deadline_reached = false;
    first_run_mut(&mut missing_conclusion)?.conclusion = None;
    missing_conclusion.gate_artifact = None;
    cases.push(("run-conclusion-missing", missing_conclusion));

    let mut missing_coordinator = terminal_input("pass");
    missing_coordinator.deadline_reached = false;
    missing_coordinator.coordinator = None;
    cases.push(("coordinator-marker-missing", missing_coordinator));

    let mut missing_proof = terminal_input("pass");
    missing_proof.deadline_reached = false;
    missing_proof.required_proofs.clear();
    cases.push(("missing-required-proof", missing_proof));

    let mut missing_artifact = terminal_input("pass");
    missing_artifact.deadline_reached = false;
    missing_artifact.gate_artifact = None;
    cases.push(("missing-artifact", missing_artifact));

    for (expected_kind, input) in cases {
        let artifact = classify_gate_watchdog(&input)?;
        anyhow::ensure!(artifact.state == GateWatchdogState::Pending);
        anyhow::ensure!(artifact.conclusion.is_none());
        let reason = artifact
            .reasons
            .first()
            .ok_or_else(|| anyhow::anyhow!("{expected_kind} produced no reason"))?;
        anyhow::ensure!(reason.kind == expected_kind);
    }
    Ok(())
}

#[test]
fn unrelated_check_cannot_satisfy_required_check() -> Result<()> {
    let mut input = terminal_input("pass");
    input.deadline_reached = false;
    let run = first_run_mut(&mut input)?;
    run.check_name = "unrelated/test".to_owned();
    input.gate_artifact = None;
    input.coordinator = None;
    input.required_proofs.clear();

    let pending = classify_gate_watchdog(&input)?;
    anyhow::ensure!(pending.state == GateWatchdogState::Pending);
    anyhow::ensure!(pending.selected_run_id.is_none());
    anyhow::ensure!(pending.observed_run_ids.is_empty());

    input.deadline_reached = true;
    let terminal = classify_gate_watchdog(&input)?;
    anyhow::ensure!(terminal.state == GateWatchdogState::Inconclusive);
    anyhow::ensure!(
        terminal.reasons.first().map(|reason| reason.kind.as_str()) == Some("missing-run")
    );
    Ok(())
}

#[test]
fn missing_required_proof_cannot_produce_terminal_pass() -> Result<()> {
    let mut input = terminal_input("pass");
    input.required_proofs.clear();

    let artifact = classify_gate_watchdog(&input)?;

    anyhow::ensure!(artifact.state == GateWatchdogState::Inconclusive);
    anyhow::ensure!(artifact.conclusion.as_deref() == Some("inconclusive"));
    anyhow::ensure!(
        artifact.reasons.first().map(|reason| reason.kind.as_str())
            == Some("missing-required-proof")
    );
    Ok(())
}

#[test]
fn conflicting_required_check_observations_are_rejected() -> Result<()> {
    let mut input = terminal_input("pass");
    let mut duplicate = input
        .runs
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no run"))?;
    duplicate.check_run_id = 1_002;
    duplicate.status = GateWatchdogRunStatus::InProgress;
    duplicate.conclusion = None;
    duplicate.receipt = "github/workflow-runs.json#duplicate".to_owned();
    input.runs.push(duplicate);

    ensure_classification_error_contains(
        &input,
        "conflicting required-check observations for run 101 attempt 1",
    )?;
    Ok(())
}

#[test]
fn malformed_watchdog_packets_report_exact_validation_errors() -> Result<()> {
    let mut missing_check = terminal_input("pass");
    missing_check.required_check.name.clear();
    ensure_classification_error_contains(
        &missing_check,
        "required check name and app_id must be present",
    )?;

    let mut duplicate_check_run = terminal_input("pass");
    let duplicate = duplicate_check_run
        .runs
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no run"))?;
    duplicate_check_run.runs.push(duplicate);
    ensure_classification_error_contains(
        &duplicate_check_run,
        "duplicate check_run_id 1001 observations",
    )?;

    let mut invalid_coordinator = terminal_input("pass");
    let coordinator = invalid_coordinator
        .coordinator
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("watchdog test fixture has no coordinator"))?;
    coordinator.run_id = 0;
    ensure_classification_error_contains(
        &invalid_coordinator,
        "coordinator run id and attempt must be positive",
    )?;

    let mut unexpected_proof = terminal_input("pass");
    first_proof_mut(&mut unexpected_proof)?.id = "cargo-clippy".to_owned();
    ensure_classification_error_contains(
        &unexpected_proof,
        "observed unexpected required proof cargo-clippy",
    )?;

    let mut duplicate_expected = terminal_input("pass");
    duplicate_expected
        .expected_required_proof_ids
        .push("cargo-check".to_owned());
    ensure_classification_error_contains(
        &duplicate_expected,
        "duplicate expected required proof cargo-check",
    )?;
    Ok(())
}

#[test]
fn newer_replacement_before_deadline_stays_pending() -> Result<()> {
    let mut input = terminal_input("pass");
    input.deadline_reached = false;
    first_run_mut(&mut input)?.attempt = 2;
    first_run_mut(&mut input)?.conclusion = Some("cancelled".to_owned());
    input.runs.push(GateWatchdogRunObservation {
        run_id: 102,
        attempt: 1,
        check_run_id: 1_002,
        check_name: "ub-review/gate".to_owned(),
        check_app_id: 15_368,
        head_sha: sha('a'),
        status: GateWatchdogRunStatus::InProgress,
        conclusion: None,
        receipt: "github/workflow-runs.json#102".to_owned(),
    });
    input.gate_artifact = None;

    let artifact = classify_gate_watchdog(&input)?;
    anyhow::ensure!(artifact.state == GateWatchdogState::Pending);
    anyhow::ensure!(artifact.conclusion.is_none());
    anyhow::ensure!(artifact.selected_run_id == Some(102));
    let reason = artifact
        .reasons
        .first()
        .ok_or_else(|| anyhow::anyhow!("pending replacement produced no reason"))?;
    anyhow::ensure!(reason.kind == "run-in-progress");
    Ok(())
}

#[test]
fn classifier_serialization_is_order_independent() -> Result<()> {
    let mut input = terminal_input("pass");
    input.runs.push(GateWatchdogRunObservation {
        run_id: 99,
        attempt: 1,
        check_run_id: 999,
        check_name: "ub-review/gate".to_owned(),
        check_app_id: 15_368,
        head_sha: sha('b'),
        status: GateWatchdogRunStatus::Completed,
        conclusion: Some("success".to_owned()),
        receipt: "github/workflow-runs.json#99".to_owned(),
    });
    input
        .expected_required_proof_ids
        .push("cargo-test".to_owned());
    input.required_proofs.push(GateWatchdogProofObservation {
        id: "cargo-test".to_owned(),
        run_id: 101,
        attempt: 1,
        head_sha: sha('a'),
        status: GateWatchdogProofStatus::Terminal,
        receipt: "review/proof_receipts.json#cargo-test".to_owned(),
    });
    let first = classify_gate_watchdog(&input)?;
    input.runs.reverse();
    input.required_proofs.reverse();
    let second = classify_gate_watchdog(&input)?;
    anyhow::ensure!(first == second);
    anyhow::ensure!(serde_json::to_vec_pretty(&first)? == serde_json::to_vec_pretty(&second)?);
    Ok(())
}

#[test]
fn command_writes_versioned_watchdog_artifact() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let input_path = temp.path().join("input/gate-watchdog-observations.json");
    let out = temp.path().join("review/gate_watchdog.json");
    let parent = input_path
        .parent()
        .ok_or_else(|| anyhow::anyhow!("watchdog input path has no parent"))?;
    fs::create_dir_all(parent)?;
    fs::write(
        &input_path,
        serde_json::to_vec_pretty(&terminal_input("pass"))?,
    )?;
    cmd_gate_watchdog(GateWatchdogArgs {
        observations: input_path,
        out: out.clone(),
    })?;
    let written: GateWatchdogArtifact = serde_json::from_slice(&fs::read(out)?)?;
    anyhow::ensure!(written.schema == GATE_WATCHDOG_SCHEMA);
    anyhow::ensure!(written.state == GateWatchdogState::Terminal);
    anyhow::ensure!(written.conclusion.as_deref() == Some("pass"));
    Ok(())
}
