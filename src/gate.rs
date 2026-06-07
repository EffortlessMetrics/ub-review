//! Gate verdict surface: the `gate_outcome.json` writer-side contract and
//! its enforcement. `build_gate_outcome` derives the deterministic verdict
//! (model output never feeds it); `cmd_gate_check` is the single source of
//! truth that turns a recorded `fail` conclusion into a non-zero exit.
//! Extracted from main.rs as pure code motion (cleanup train PR 2); spec
//! 0003 owns the field contract and the verifier audits the artifact.

use std::fs;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::cli::GateCheckArgs;
use crate::config::Config;
use crate::{
    ModelEvidenceIssue, Plan, ProofReceipt, ProofRequest, ReviewTerminalState, RunArgs,
    RunCompletion, RunMode, SensorEvidenceIssue, ToolGateOutcomeEntry, proof_command_status,
    sensor_issue_is_required,
};

/// Minimal read view of `review/gate_outcome.json` for enforcement. The full
/// `GateOutcome` struct is write-only; enforcement needs only the conclusion
/// and the blocking reason ids.
#[derive(Debug, Deserialize)]
pub(crate) struct GateCheckOutcome {
    #[serde(default)]
    pub(crate) schema: Option<String>,
    #[serde(default)]
    pub(crate) conclusion: Option<String>,
    #[serde(default)]
    pub(crate) reasons: Vec<GateCheckReason>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct GateCheckReason {
    #[serde(default)]
    pub(crate) id: String,
}

/// Single source of truth for gate enforcement: resolves `fail-on-gate` with
/// the same `FailOnGate::resolved` semantics `run` uses, then turns a recorded
/// `fail` conclusion into a non-zero exit. The action's "Enforce gate outcome"
/// step calls this instead of re-implementing the resolution in bash.
///
/// Enforcement fails closed: only a conclusion of exactly `pass` in an
/// artifact with the expected schema keeps the check green. A missing key, a
/// null, casing drift (`Fail`), or any other unrecognized verdict is treated
/// as a failure naming the unexpected value, so a corrupted or
/// future-incompatible artifact can never silently pass the gate.
pub(crate) fn cmd_gate_check(args: GateCheckArgs) -> Result<()> {
    let path = &args.gate_outcome;
    if !args.fail_on_gate.resolved(args.mode) {
        // Informational only: with enforcement off nothing fails, but a
        // schema drift in a readable artifact is still worth a log line.
        if let Ok(text) = fs::read_to_string(path)
            && let Ok(outcome) = serde_json::from_str::<GateCheckOutcome>(&text)
            && outcome.schema.as_deref() != Some(GATE_OUTCOME_SCHEMA)
        {
            println!(
                "note: {} schema is `{}` (expected `{GATE_OUTCOME_SCHEMA}`)",
                path.display(),
                outcome.schema.as_deref().unwrap_or("missing")
            );
        }
        println!(
            "gate enforcement is off (fail-on-gate={}, mode={}); not enforcing {}",
            args.fail_on_gate.key(),
            args.mode.key(),
            path.display()
        );
        return Ok(());
    }
    if !path.exists() {
        bail!("gate enforcement is on but {} is missing", path.display());
    }
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let outcome: GateCheckOutcome =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let schema = outcome.schema.as_deref().unwrap_or("missing");
    if schema != GATE_OUTCOME_SCHEMA {
        let message = format!(
            "gate enforcement is on but {} has unexpected schema `{schema}` (expected \
             `{GATE_OUTCOME_SCHEMA}`); failing closed",
            path.display()
        );
        println!("::error::{message}");
        bail!("{message}");
    }
    match outcome.conclusion.as_deref() {
        Some("pass") => {
            println!(
                "gate conclusion is `pass` in {}; check stays green",
                path.display()
            );
            Ok(())
        }
        Some("fail") => {
            let mut reason_ids = outcome
                .reasons
                .iter()
                .map(|reason| reason.id.as_str())
                .filter(|id| !id.trim().is_empty())
                .collect::<Vec<_>>()
                .join(", ");
            if reason_ids.is_empty() {
                reason_ids = "none".to_owned();
            }
            let message = format!(
                "ub-review gate failed (blocking reasons: {reason_ids}); receipts are in {}",
                path.display()
            );
            // GitHub Actions error annotation; the bail below sets the exit code.
            println!("::error::{message}");
            bail!("{message}");
        }
        other => {
            let value = other
                .map(|value| format!("`{value}`"))
                .unwrap_or_else(|| "missing".to_owned());
            let message = format!(
                "gate enforcement is on but {} records unrecognized conclusion {value} \
                 (expected exactly `pass` or `fail`); failing closed",
                path.display()
            );
            println!("::error::{message}");
            bail!("{message}");
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct GateOutcome {
    pub(crate) schema: String,
    pub(crate) conclusion: String,
    pub(crate) terminal_status: String,
    pub(crate) reasons: Vec<GateReason>,
    pub(crate) required_proof: GateRequiredProofCounts,
    pub(crate) tool_gates: GateToolGateCounts,
    pub(crate) evidence_gaps_blocking: usize,
    pub(crate) evidence_gaps_advisory: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct GateReason {
    pub(crate) kind: String,
    pub(crate) id: String,
    pub(crate) detail: String,
    pub(crate) receipt: String,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(crate) struct GateRequiredProofCounts {
    pub(crate) matched: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) skipped: usize,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(crate) struct GateToolGateCounts {
    pub(crate) evaluated: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
}

pub(crate) fn run_gate_failure_message(completion: &RunCompletion) -> Option<String> {
    if !completion.fail_on_gate || completion.gate_conclusion != "fail" {
        return None;
    }
    Some(format!(
        "gate conclusion is `fail`; receipts are in review/gate_outcome.json under {}",
        completion.run_dir.display()
    ))
}

pub(crate) const GATE_OUTCOME_SCHEMA: &str = "ub-review.gate_outcome.v1";
pub(crate) const REQUIRED_PROOF_POLICY_LANE: &str = "intelligent-ci-policy";

pub(crate) struct GateOutcomeInput<'a> {
    pub(crate) args: &'a RunArgs,
    pub(crate) config: &'a Config,
    pub(crate) plan: &'a Plan,
    pub(crate) terminal_state: &'a ReviewTerminalState,
    pub(crate) proof_requests: &'a [ProofRequest],
    pub(crate) proof_receipts: &'a [ProofReceipt],
    pub(crate) tool_gate_outcomes: &'a [ToolGateOutcomeEntry],
    pub(crate) missing_or_failed_sensor_evidence: &'a [SensorEvidenceIssue],
    pub(crate) missing_or_failed_model_evidence: &'a [ModelEvidenceIssue],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequiredProofClass {
    Passed,
    Failed,
    Skipped,
}

/// Derive the deterministic gate verdict from the terminal state, required
/// [[proof.required]] receipts, and required-sensor evidence gaps. Model output
/// never feeds this verdict directly: only policy-configured required proof
/// requests count, never model-flagged ones, and every fail reason points at an
/// existing receipt artifact.
pub(crate) fn build_gate_outcome(input: GateOutcomeInput<'_>) -> GateOutcome {
    let mut reasons = Vec::new();
    let blocking_policy = &input.config.gate.blocking;

    // Malformed policy sections recorded at config load are always blocking:
    // a config the repo wrote but the gate could not honor must never decay
    // into a silent default (roadmap #24). The exit decision still follows
    // `fail-on-gate`, so review-byok with enforcement off records the reason
    // without exiting non-zero.
    for error in &input.config.policy_errors {
        reasons.push(GateReason {
            kind: "policy".to_owned(),
            id: error.section.clone(),
            detail: error.detail.clone(),
            receipt: "effective-config.json".to_owned(),
        });
    }

    let mut required_proof = GateRequiredProofCounts::default();
    for request in input
        .proof_requests
        .iter()
        .filter(|request| proof_request_is_gate_required(request))
    {
        required_proof.matched += 1;
        let receipt = input.proof_receipts.iter().find(|receipt| {
            receipt
                .request_ids
                .iter()
                .any(|request_id| request_id == &request.id)
        });
        let Some(receipt) = receipt else {
            required_proof.skipped += 1;
            if blocking_policy.required_proof_unproven {
                reasons.push(GateReason {
                    kind: "blocking-finding".to_owned(),
                    id: required_proof_reason_id(request),
                    detail: format!(
                        "required proof `{}` produced no receipt; repo policy \
                         [gate.blocking] required_proof_unproven marks unproven required \
                         proof blocking",
                        required_proof_reason_id(request)
                    ),
                    receipt: "review/proof_requests.json".to_owned(),
                });
            }
            continue;
        };
        match required_proof_receipt_class(receipt) {
            RequiredProofClass::Passed => required_proof.passed += 1,
            RequiredProofClass::Skipped => {
                required_proof.skipped += 1;
                if blocking_policy.required_proof_unproven {
                    reasons.push(GateReason {
                        kind: "blocking-finding".to_owned(),
                        id: required_proof_reason_id(request),
                        detail: format!(
                            "required proof `{}` was not proven (result `{}`); repo policy \
                             [gate.blocking] required_proof_unproven marks unproven required \
                             proof blocking",
                            required_proof_reason_id(request),
                            receipt.result
                        ),
                        receipt: format!("review/proof_receipts.json#{}", receipt.id),
                    });
                }
            }
            RequiredProofClass::Failed => {
                // One reason per failed required request, keyed by its policy
                // identifier, so `required_proof.failed` and the blocking
                // reasons always count the same per-request way.
                required_proof.failed += 1;
                reasons.push(GateReason {
                    kind: "required-proof".to_owned(),
                    id: required_proof_reason_id(request),
                    detail: required_proof_failure_detail(request, receipt),
                    receipt: format!("review/proof_receipts.json#{}", receipt.id),
                });
            }
        }
    }

    // A tool-gate outcome entry exists only when repo config sets a
    // [tools.<id>.gate] policy; that explicit opt-in is what makes a failed
    // threshold blocking. A `failed` outcome means the threshold was actually
    // evaluated against a gate-decision receipt and actually exceeded; a
    // sensor crash or timeout is classified as `missing_evidence` upstream
    // because no threshold verdict exists. Tools without configured gates
    // never produce entries, so non-required tools without gate policies can
    // never redden the gate. `missing_evidence` and `not_evaluated` outcomes
    // stay non-blocking unless repo policy opts required tools in via
    // [gate.blocking] tool_gate_missing_evidence.
    let mut tool_gates = GateToolGateCounts::default();
    for entry in input.tool_gate_outcomes {
        if entry.evaluated {
            tool_gates.evaluated += 1;
        }
        match entry.outcome.as_str() {
            "passed" => tool_gates.passed += 1,
            "failed" => {
                tool_gates.failed += 1;
                reasons.push(GateReason {
                    kind: "tool-gate".to_owned(),
                    id: entry.tool.clone(),
                    detail: entry.reason.clone(),
                    receipt: format!("review/tool-gate-outcomes.json#{}", entry.tool),
                });
            }
            "missing_evidence" if entry.required && blocking_policy.tool_gate_missing_evidence => {
                reasons.push(GateReason {
                    kind: "blocking-finding".to_owned(),
                    id: entry.tool.clone(),
                    detail: format!(
                        "required tool gate evidence is missing ({}); repo policy \
                         [gate.blocking] tool_gate_missing_evidence marks this blocking",
                        entry.reason
                    ),
                    receipt: format!("review/tool-gate-outcomes.json#{}", entry.tool),
                });
            }
            _ => {}
        }
    }

    let enforce_required_sensors = matches!(input.args.mode, RunMode::IntelligentCi);
    let mut evidence_gaps_blocking = 0;
    let mut evidence_gaps_advisory = input.missing_or_failed_model_evidence.len();
    for issue in input.missing_or_failed_sensor_evidence {
        if enforce_required_sensors && sensor_issue_is_required(input.plan, issue) {
            evidence_gaps_blocking += 1;
            reasons.push(GateReason {
                kind: "required-sensor".to_owned(),
                id: issue.sensor.clone(),
                detail: format!(
                    "required sensor evidence gap (status `{}`): {}",
                    issue.status, issue.reason
                ),
                receipt: required_sensor_gap_receipt(issue),
            });
        } else {
            evidence_gaps_advisory += 1;
        }
    }

    // The gate fails if and only if deterministic blocking reasons exist.
    // A `failed-to-review` terminal status without blocking reasons is the
    // model-availability case (missing provider keys or a provider outage);
    // per ADR 0002 that degrades the review but never fails the gate, and the
    // gap stays visible through `evidence_gaps_advisory`. Genuine internal
    // failures abort the run with a non-zero exit through `anyhow` before
    // gate construction, so they never reach this point.
    let conclusion = if reasons.is_empty() { "pass" } else { "fail" };

    GateOutcome {
        schema: GATE_OUTCOME_SCHEMA.to_owned(),
        conclusion: conclusion.to_owned(),
        terminal_status: input.terminal_state.status.clone(),
        reasons,
        required_proof,
        tool_gates,
        evidence_gaps_blocking,
        evidence_gaps_advisory,
    }
}

/// Only policy-configured [[proof.required]] requests count toward the gate.
/// Model lanes can also mark proof requests as required, but model output must
/// never feed the gate verdict directly.
pub(crate) fn proof_request_is_gate_required(request: &ProofRequest) -> bool {
    request.required && request.lane == REQUIRED_PROOF_POLICY_LANE
}

pub(crate) fn required_proof_receipt_class(receipt: &ProofReceipt) -> RequiredProofClass {
    match receipt.result.as_str() {
        "head_passed" | "discriminating" => RequiredProofClass::Passed,
        "head_failed" | "timed_out" => RequiredProofClass::Failed,
        // Missing receipts, `skipped_budget`, `skipped_profile`,
        // `non_discriminating`, and `base_patch_failed` stay non-blocking by
        // default: a dry run must not redden the gate. Repos opt skipped
        // required proof into blocking via [gate.blocking]
        // required_proof_unproven = true.
        _ => RequiredProofClass::Skipped,
    }
}

/// Blocking reasons are keyed by the policy/request identifier (for example
/// `cargo-check`), so a red check names the failed requirement rather than an
/// internal request id.
pub(crate) fn required_proof_reason_id(request: &ProofRequest) -> String {
    request
        .requested_by
        .iter()
        .find_map(|requester| requester.strip_prefix("proof-policy:"))
        .map(str::to_owned)
        .unwrap_or_else(|| request.id.clone())
}

/// Failure details lead with the request's policy identifier so two required
/// requests resolved by one shared receipt still produce distinct,
/// per-requirement details.
pub(crate) fn required_proof_failure_detail(
    request: &ProofRequest,
    receipt: &ProofReceipt,
) -> String {
    let command_detail = receipt
        .commands
        .iter()
        .find(|command| command.side == "head")
        .or_else(|| receipt.commands.first())
        .map(|command| format!("`{}` {}", command.command, proof_command_status(command)))
        .unwrap_or_else(|| receipt.reason.clone());
    let detail = format!(
        "required proof `{}`: {command_detail}",
        required_proof_reason_id(request)
    );
    if receipt.result == "timed_out" && !detail.contains("timed") {
        format!("{detail} (proof timed out)")
    } else {
        detail
    }
}

/// A `receipt-absent` issue means the sensor never wrote its status receipt,
/// so the reason points at the terminal state instead of a nonexistent file.
pub(crate) fn required_sensor_gap_receipt(issue: &SensorEvidenceIssue) -> String {
    if issue.status == "receipt-absent" {
        "review/terminal_state.json".to_owned()
    } else {
        format!("sensors/{}/ub-review-sensor-status.json", issue.sensor)
    }
}
