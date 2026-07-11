//! Deterministic current-head watchdog classification.
//!
//! This module consumes frozen observations and writes a receipt artifact. It
//! does not query GitHub or publish checks; future stable-coordinator wiring
//! owns those side effects.

use std::fs;
use std::path::Path;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::artifacts::{GATE_OUTCOME_SCHEMA, GATE_WATCHDOG_INPUT_SCHEMA, GATE_WATCHDOG_SCHEMA};
use crate::cli::GateWatchdogArgs;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateWatchdogInput {
    pub(crate) schema: String,
    pub(crate) expected_head_sha: String,
    pub(crate) required_check: GateWatchdogRequiredCheck,
    pub(crate) expected_required_proof_ids: Vec<String>,
    pub(crate) deadline_reached: bool,
    pub(crate) observation_receipt: String,
    #[serde(default)]
    pub(crate) runs: Vec<GateWatchdogRunObservation>,
    #[serde(default)]
    pub(crate) gate_artifact: Option<GateWatchdogGateArtifact>,
    #[serde(default)]
    pub(crate) coordinator: Option<GateWatchdogCoordinatorObservation>,
    #[serde(default)]
    pub(crate) required_proofs: Vec<GateWatchdogProofObservation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateWatchdogRequiredCheck {
    pub(crate) name: String,
    pub(crate) app_id: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateWatchdogRunObservation {
    pub(crate) run_id: u64,
    pub(crate) attempt: u64,
    pub(crate) check_run_id: u64,
    pub(crate) check_name: String,
    pub(crate) check_app_id: u64,
    pub(crate) head_sha: String,
    pub(crate) status: GateWatchdogRunStatus,
    #[serde(default)]
    pub(crate) conclusion: Option<String>,
    pub(crate) receipt: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GateWatchdogRunStatus {
    Queued,
    InProgress,
    Completed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateWatchdogGateArtifact {
    pub(crate) run_id: u64,
    pub(crate) attempt: u64,
    pub(crate) head_sha: String,
    pub(crate) artifact_id: u64,
    pub(crate) artifact_name: String,
    pub(crate) schema: String,
    pub(crate) conclusion: String,
    pub(crate) receipt: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateWatchdogCoordinatorObservation {
    pub(crate) run_id: u64,
    pub(crate) attempt: u64,
    pub(crate) head_sha: String,
    pub(crate) status: GateWatchdogCoordinatorStatus,
    pub(crate) receipt: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GateWatchdogCoordinatorStatus {
    Terminal,
    Crashed,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GateWatchdogProofObservation {
    pub(crate) id: String,
    pub(crate) run_id: u64,
    pub(crate) attempt: u64,
    pub(crate) head_sha: String,
    pub(crate) status: GateWatchdogProofStatus,
    pub(crate) receipt: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GateWatchdogProofStatus {
    Terminal,
    Orphaned,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct GateWatchdogArtifact {
    pub(crate) schema: String,
    pub(crate) expected_head_sha: String,
    pub(crate) required_check_name: String,
    pub(crate) required_check_app_id: u64,
    pub(crate) state: GateWatchdogState,
    pub(crate) conclusion: Option<String>,
    pub(crate) selected_run_id: Option<u64>,
    pub(crate) selected_attempt: Option<u64>,
    pub(crate) selected_check_run_id: Option<u64>,
    pub(crate) observed_run_ids: Vec<u64>,
    pub(crate) observed_check_run_ids: Vec<u64>,
    pub(crate) reasons: Vec<GateWatchdogReason>,
    pub(crate) source_artifacts: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GateWatchdogState {
    Terminal,
    Pending,
    Inconclusive,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct GateWatchdogReason {
    pub(crate) kind: String,
    pub(crate) detail: String,
    pub(crate) receipt: String,
    pub(crate) retry_action: String,
}

pub(crate) fn cmd_gate_watchdog(args: GateWatchdogArgs) -> Result<()> {
    let text = fs::read_to_string(&args.observations)
        .with_context(|| format!("read {}", args.observations.display()))?;
    let input: GateWatchdogInput = serde_json::from_str(&text)
        .with_context(|| format!("parse {}", args.observations.display()))?;
    let artifact = classify_gate_watchdog(&input)?;
    write_gate_watchdog(&args.out, &artifact)?;
    println!(
        "gate watchdog state={} conclusion={} artifact={}",
        artifact.state.key(),
        artifact.conclusion.as_deref().unwrap_or("none"),
        args.out.display()
    );
    Ok(())
}

pub(crate) fn classify_gate_watchdog(input: &GateWatchdogInput) -> Result<GateWatchdogArtifact> {
    validate_input(input)?;
    let selected = input
        .runs
        .iter()
        .filter(|run| {
            is_required_check(input, run) && same_sha(&run.head_sha, &input.expected_head_sha)
        })
        .max_by(|left, right| run_order(left).cmp(&run_order(right)));

    let Some(run) = selected else {
        if let Some(stale) = input
            .runs
            .iter()
            .filter(|run| {
                is_required_check(input, run)
                    && !same_sha(&run.head_sha, &input.expected_head_sha)
                    && run.status == GateWatchdogRunStatus::Completed
                    && run.conclusion.as_deref() == Some("success")
            })
            .max_by(|left, right| run_order(left).cmp(&run_order(right)))
        {
            let (state, conclusion) = availability_outcome(input);
            return Ok(with_reason(
                input,
                state,
                conclusion,
                None,
                watchdog_reason(
                    "stale-success",
                    format!(
                        "successful run {} belongs to {}, not current head {}",
                        stale.run_id, stale.head_sha, input.expected_head_sha
                    ),
                    &stale.receipt,
                    "create and complete a gate run for the current head SHA",
                ),
            ));
        }
        if input.deadline_reached {
            return Ok(with_reason(
                input,
                GateWatchdogState::Inconclusive,
                Some("inconclusive"),
                None,
                watchdog_reason(
                    "missing-run",
                    format!(
                        "no gate run was observed for current head {} before the deadline",
                        input.expected_head_sha
                    ),
                    &input.observation_receipt,
                    "retry workflow creation for the current head SHA",
                ),
            ));
        }
        return Ok(with_reason(
            input,
            GateWatchdogState::Pending,
            None,
            None,
            watchdog_reason(
                "awaiting-run",
                format!(
                    "no gate run is visible yet for current head {}",
                    input.expected_head_sha
                ),
                &input.observation_receipt,
                "wait for workflow creation or retry after the observation deadline",
            ),
        ));
    };

    if matches!(
        run.status,
        GateWatchdogRunStatus::Queued | GateWatchdogRunStatus::InProgress
    ) {
        let (state, conclusion, reason) = if input.deadline_reached {
            (
                GateWatchdogState::Inconclusive,
                Some("inconclusive"),
                watchdog_reason(
                    "run-deadline-exceeded",
                    format!(
                        "gate run {} attempt {} did not reach completion before the deadline",
                        run.run_id, run.attempt
                    ),
                    &run.receipt,
                    "retry the gate run or investigate the executor queue",
                ),
            )
        } else {
            (
                GateWatchdogState::Pending,
                None,
                watchdog_reason(
                    "run-in-progress",
                    format!(
                        "gate run {} attempt {} is still {}",
                        run.run_id,
                        run.attempt,
                        run.status.key()
                    ),
                    &run.receipt,
                    "wait for the current replacement run to complete",
                ),
            )
        };
        return Ok(with_reason(input, state, conclusion, Some(run), reason));
    }

    match run.conclusion.as_deref() {
        Some("cancelled") => {
            let (state, conclusion) = availability_outcome(input);
            return Ok(with_reason(
                input,
                state,
                conclusion,
                Some(run),
                watchdog_reason(
                    "cancelled-without-replacement",
                    format!(
                        "gate run {} attempt {} was cancelled and no newer replacement is visible",
                        run.run_id, run.attempt
                    ),
                    &run.receipt,
                    "rerun the gate for the current head SHA",
                ),
            ));
        }
        None => {
            let (state, conclusion) = availability_outcome(input);
            return Ok(with_reason(
                input,
                state,
                conclusion,
                Some(run),
                watchdog_reason(
                    "run-conclusion-missing",
                    format!("completed gate run {} has no conclusion", run.run_id),
                    &run.receipt,
                    "refresh the workflow-run receipt and retry classification",
                ),
            ));
        }
        _ => {}
    }

    let Some(coordinator) = &input.coordinator else {
        let (state, conclusion) = availability_outcome(input);
        return Ok(with_reason(
            input,
            state,
            conclusion,
            Some(run),
            watchdog_reason(
                "coordinator-marker-missing",
                format!("gate run {} has no coordinator terminal marker", run.run_id),
                &input.observation_receipt,
                "retry the coordinator and preserve its terminal receipt",
            ),
        ));
    };
    if coordinator.run_id != run.run_id
        || coordinator.attempt != run.attempt
        || !same_sha(&coordinator.head_sha, &input.expected_head_sha)
    {
        return Ok(with_reason(
            input,
            GateWatchdogState::Inconclusive,
            Some("inconclusive"),
            Some(run),
            watchdog_reason(
                "coordinator-run-mismatch",
                format!(
                    "coordinator marker belongs to run {} attempt {} head {}, expected run {} attempt {} head {}",
                    coordinator.run_id,
                    coordinator.attempt,
                    coordinator.head_sha,
                    run.run_id,
                    run.attempt,
                    input.expected_head_sha
                ),
                &coordinator.receipt,
                "regenerate the coordinator marker for the selected current-head run attempt",
            ),
        ));
    }
    if coordinator.status == GateWatchdogCoordinatorStatus::Crashed {
        return Ok(with_reason(
            input,
            GateWatchdogState::Inconclusive,
            Some("inconclusive"),
            Some(run),
            watchdog_reason(
                "coordinator-crash",
                format!("the coordinator crashed during gate run {}", run.run_id),
                &coordinator.receipt,
                "retry the coordinator from the preserved observation packet",
            ),
        ));
    }

    if let Some(stale) = input
        .required_proofs
        .iter()
        .filter(|proof| {
            proof.run_id != run.run_id
                || proof.attempt != run.attempt
                || !same_sha(&proof.head_sha, &input.expected_head_sha)
        })
        .min_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then(left.receipt.cmp(&right.receipt))
        })
    {
        return Ok(with_reason(
            input,
            GateWatchdogState::Inconclusive,
            Some("inconclusive"),
            Some(run),
            watchdog_reason(
                "required-proof-run-mismatch",
                format!(
                    "required proof {} belongs to run {} attempt {} head {}, expected run {} attempt {} head {}",
                    stale.id,
                    stale.run_id,
                    stale.attempt,
                    stale.head_sha,
                    run.run_id,
                    run.attempt,
                    input.expected_head_sha
                ),
                &stale.receipt,
                "regenerate required proof receipts for the selected current-head run attempt",
            ),
        ));
    }

    if let Some(missing_id) = input.expected_required_proof_ids.iter().find(|expected| {
        !input
            .required_proofs
            .iter()
            .any(|proof| proof.id == expected.as_str())
    }) {
        let (state, conclusion) = availability_outcome(input);
        return Ok(with_reason(
            input,
            state,
            conclusion,
            Some(run),
            watchdog_reason(
                "missing-required-proof",
                format!(
                    "required proof {} has no observation for gate run {} attempt {}",
                    missing_id, run.run_id, run.attempt
                ),
                &input.observation_receipt,
                "requeue the missing required proof and rerun finalization",
            ),
        ));
    }

    if let Some(orphaned) = input
        .required_proofs
        .iter()
        .filter(|proof| proof.status == GateWatchdogProofStatus::Orphaned)
        .min_by(|left, right| {
            left.id
                .cmp(&right.id)
                .then(left.receipt.cmp(&right.receipt))
        })
    {
        return Ok(with_reason(
            input,
            GateWatchdogState::Inconclusive,
            Some("inconclusive"),
            Some(run),
            watchdog_reason(
                "required-proof-orphaned",
                format!(
                    "required proof {} was orphaned during gate run {}",
                    orphaned.id, run.run_id
                ),
                &orphaned.receipt,
                "requeue the orphaned proof and rerun finalization",
            ),
        ));
    }

    let Some(gate) = &input.gate_artifact else {
        let (state, conclusion) = availability_outcome(input);
        return Ok(with_reason(
            input,
            state,
            conclusion,
            Some(run),
            watchdog_reason(
                "missing-artifact",
                format!(
                    "completed gate run {} did not produce review/gate_outcome.json",
                    run.run_id
                ),
                &input.observation_receipt,
                "retry finalization and artifact upload for the current head",
            ),
        ));
    };
    if gate.run_id != run.run_id || gate.attempt != run.attempt {
        return Ok(artifact_gap(
            input,
            run,
            gate,
            "artifact-run-mismatch",
            format!(
                "gate artifact names run {} attempt {}, but selected current-head run is {} attempt {}",
                gate.run_id, gate.attempt, run.run_id, run.attempt
            ),
        ));
    }
    if !same_sha(&gate.head_sha, &input.expected_head_sha) {
        return Ok(artifact_gap(
            input,
            run,
            gate,
            "artifact-sha-mismatch",
            format!(
                "gate artifact belongs to {}, expected {}",
                gate.head_sha, input.expected_head_sha
            ),
        ));
    }
    if gate.schema != GATE_OUTCOME_SCHEMA {
        return Ok(artifact_gap(
            input,
            run,
            gate,
            "artifact-schema-mismatch",
            format!(
                "gate artifact schema is {}, expected {}",
                gate.schema, GATE_OUTCOME_SCHEMA
            ),
        ));
    }
    if !matches!(gate.conclusion.as_str(), "pass" | "fail" | "inconclusive") {
        return Ok(artifact_gap(
            input,
            run,
            gate,
            "artifact-conclusion-invalid",
            format!(
                "gate artifact conclusion {:?} is not pass, fail, or inconclusive",
                gate.conclusion
            ),
        ));
    }
    let expected_run_conclusion = if gate.conclusion == "pass" {
        "success"
    } else {
        "failure"
    };
    if run.conclusion.as_deref() != Some(expected_run_conclusion) {
        return Ok(artifact_gap(
            input,
            run,
            gate,
            "run-artifact-disagreement",
            format!(
                "run conclusion {:?} disagrees with gate conclusion {:?}",
                run.conclusion, gate.conclusion
            ),
        ));
    }

    Ok(build_artifact(
        input,
        GateWatchdogState::Terminal,
        Some(gate.conclusion.clone()),
        Some(run),
        Vec::new(),
    ))
}

fn artifact_gap(
    input: &GateWatchdogInput,
    run: &GateWatchdogRunObservation,
    gate: &GateWatchdogGateArtifact,
    kind: &str,
    detail: String,
) -> GateWatchdogArtifact {
    with_reason(
        input,
        GateWatchdogState::Inconclusive,
        Some("inconclusive"),
        Some(run),
        watchdog_reason(
            kind,
            detail,
            &gate.receipt,
            "regenerate the gate artifact from the current-head stable coordinator",
        ),
    )
}

fn watchdog_reason(
    kind: &str,
    detail: String,
    receipt: &str,
    retry_action: &str,
) -> GateWatchdogReason {
    GateWatchdogReason {
        kind: kind.to_owned(),
        detail,
        receipt: receipt.to_owned(),
        retry_action: retry_action.to_owned(),
    }
}

fn with_reason(
    input: &GateWatchdogInput,
    state: GateWatchdogState,
    conclusion: Option<&str>,
    selected: Option<&GateWatchdogRunObservation>,
    reason: GateWatchdogReason,
) -> GateWatchdogArtifact {
    build_artifact(
        input,
        state,
        conclusion.map(str::to_owned),
        selected,
        vec![reason],
    )
}

fn build_artifact(
    input: &GateWatchdogInput,
    state: GateWatchdogState,
    conclusion: Option<String>,
    selected: Option<&GateWatchdogRunObservation>,
    reasons: Vec<GateWatchdogReason>,
) -> GateWatchdogArtifact {
    let mut observed_run_ids = input
        .runs
        .iter()
        .filter(|run| is_required_check(input, run))
        .map(|run| run.run_id)
        .collect::<Vec<_>>();
    observed_run_ids.sort_unstable();
    observed_run_ids.dedup();
    let mut source_artifacts = vec![input.observation_receipt.clone()];
    source_artifacts.extend(input.runs.iter().map(|run| run.receipt.clone()));
    source_artifacts.extend(input.coordinator.iter().map(|item| item.receipt.clone()));
    source_artifacts.extend(
        input
            .required_proofs
            .iter()
            .map(|item| item.receipt.clone()),
    );
    source_artifacts.extend(input.gate_artifact.iter().map(|item| item.receipt.clone()));
    source_artifacts.sort();
    source_artifacts.dedup();
    GateWatchdogArtifact {
        schema: GATE_WATCHDOG_SCHEMA.to_owned(),
        expected_head_sha: input.expected_head_sha.clone(),
        required_check_name: input.required_check.name.clone(),
        required_check_app_id: input.required_check.app_id,
        state,
        conclusion,
        selected_run_id: selected.map(|run| run.run_id),
        selected_attempt: selected.map(|run| run.attempt),
        selected_check_run_id: selected.map(|run| run.check_run_id),
        observed_run_ids,
        observed_check_run_ids: {
            let mut ids = input
                .runs
                .iter()
                .filter(|run| is_required_check(input, run))
                .map(|run| run.check_run_id)
                .collect::<Vec<_>>();
            ids.sort_unstable();
            ids.dedup();
            ids
        },
        reasons,
        source_artifacts,
    }
}

fn run_order(run: &GateWatchdogRunObservation) -> (u64, u64, &str) {
    (run.run_id, run.attempt, run.receipt.as_str())
}

fn is_required_check(input: &GateWatchdogInput, run: &GateWatchdogRunObservation) -> bool {
    run.check_name == input.required_check.name && run.check_app_id == input.required_check.app_id
}

fn availability_outcome(input: &GateWatchdogInput) -> (GateWatchdogState, Option<&'static str>) {
    if input.deadline_reached {
        (GateWatchdogState::Inconclusive, Some("inconclusive"))
    } else {
        (GateWatchdogState::Pending, None)
    }
}

fn validate_input(input: &GateWatchdogInput) -> Result<()> {
    if input.schema != GATE_WATCHDOG_INPUT_SCHEMA {
        bail!(
            "gate watchdog input schema is {:?}; expected {:?}",
            input.schema,
            GATE_WATCHDOG_INPUT_SCHEMA
        );
    }
    validate_sha("expected_head_sha", &input.expected_head_sha)?;
    if input.required_check.name.trim().is_empty() || input.required_check.app_id == 0 {
        bail!("gate watchdog required check name and app_id must be present");
    }
    validate_expected_proofs(input)?;
    validate_receipt("observation_receipt", &input.observation_receipt)?;
    for (index, run) in input.runs.iter().enumerate() {
        if run.run_id == 0
            || run.attempt == 0
            || run.check_run_id == 0
            || run.check_app_id == 0
            || run.check_name.trim().is_empty()
        {
            bail!("gate watchdog run and check identities must be present");
        }
        validate_sha("run.head_sha", &run.head_sha)?;
        validate_receipt("run.receipt", &run.receipt)?;
        for other in input.runs.iter().skip(index + 1) {
            if run.check_run_id == other.check_run_id {
                bail!(
                    "gate watchdog duplicate check_run_id {} observations",
                    run.check_run_id
                );
            }
            if is_required_check(input, run)
                && is_required_check(input, other)
                && run.run_id == other.run_id
                && run.attempt == other.attempt
            {
                bail!(
                    "gate watchdog conflicting required-check observations for run {} attempt {}",
                    run.run_id,
                    run.attempt
                );
            }
            if run.run_id == other.run_id && !same_sha(&run.head_sha, &other.head_sha) {
                bail!(
                    "gate watchdog workflow run {} observations disagree on head SHA",
                    run.run_id
                );
            }
        }
    }
    if let Some(gate) = &input.gate_artifact {
        if gate.run_id == 0
            || gate.attempt == 0
            || gate.artifact_id == 0
            || gate.artifact_name.trim().is_empty()
        {
            bail!("gate watchdog artifact run and upload identities must be present");
        }
        validate_sha("gate_artifact.head_sha", &gate.head_sha)?;
        validate_receipt("gate_artifact.receipt", &gate.receipt)?;
        if gate.receipt.split('#').next() != Some("review/gate_outcome.json") {
            bail!("gate watchdog artifact receipt must name review/gate_outcome.json");
        }
    }
    if let Some(coordinator) = &input.coordinator {
        if coordinator.run_id == 0 || coordinator.attempt == 0 {
            bail!("gate watchdog coordinator run id and attempt must be positive");
        }
        validate_sha("coordinator.head_sha", &coordinator.head_sha)?;
        validate_receipt("coordinator.receipt", &coordinator.receipt)?;
    }
    for (index, proof) in input.required_proofs.iter().enumerate() {
        if proof.id.trim().is_empty() {
            bail!("gate watchdog required proof id is empty");
        }
        if proof.run_id == 0 || proof.attempt == 0 {
            bail!("gate watchdog required proof run id and attempt must be positive");
        }
        if !input
            .expected_required_proof_ids
            .iter()
            .any(|expected| expected == &proof.id)
        {
            bail!(
                "gate watchdog observed unexpected required proof {}",
                proof.id
            );
        }
        if input
            .required_proofs
            .iter()
            .skip(index + 1)
            .any(|other| other.id == proof.id)
        {
            bail!("gate watchdog duplicate required proof {}", proof.id);
        }
        validate_sha("required_proof.head_sha", &proof.head_sha)?;
        validate_receipt("required_proof.receipt", &proof.receipt)?;
    }
    Ok(())
}

fn validate_expected_proofs(input: &GateWatchdogInput) -> Result<()> {
    for (index, id) in input.expected_required_proof_ids.iter().enumerate() {
        if id.trim().is_empty() {
            bail!("gate watchdog expected required proof id is empty");
        }
        if input
            .expected_required_proof_ids
            .iter()
            .skip(index + 1)
            .any(|other| other == id)
        {
            bail!("gate watchdog duplicate expected required proof {}", id);
        }
    }
    Ok(())
}

fn validate_sha(label: &str, value: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("gate watchdog {label} must be a full 40-hex SHA; got {value:?}");
    }
    Ok(())
}

fn same_sha(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn validate_receipt(label: &str, value: &str) -> Result<()> {
    let Some(path) = value.split('#').next() else {
        bail!("gate watchdog {label} receipt is empty");
    };
    if path.is_empty()
        || path.starts_with('/')
        || path.contains('\\')
        || path.contains(':')
        || path.split('/').any(|component| component == "..")
    {
        bail!("gate watchdog {label} must be a packet-relative receipt; got {value:?}");
    }
    Ok(())
}

fn write_gate_watchdog(path: &Path, artifact: &GateWatchdogArtifact) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(artifact)?;
    fs::write(path, json).with_context(|| format!("write {}", path.display()))
}

impl GateWatchdogState {
    fn key(self) -> &'static str {
        match self {
            Self::Terminal => "terminal",
            Self::Pending => "pending",
            Self::Inconclusive => "inconclusive",
        }
    }
}

impl GateWatchdogRunStatus {
    fn key(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }
}

#[cfg(test)]
#[path = "tests/gate_watchdog_tests.rs"]
mod tests;
