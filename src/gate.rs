//! Gate verdict surface: the `gate_outcome.json` writer-side contract and
//! its enforcement. `build_gate_outcome` derives the deterministic verdict
//! (model output never feeds it); `cmd_gate_check` is the single source of
//! truth that turns a recorded `fail` conclusion into a non-zero exit.
//! Extracted from main.rs as pure code motion (cleanup train PR 2); spec
//! 0003 owns the field contract and the verifier audits the artifact.

use std::fs;

use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::artifacts::GATE_OUTCOME_SCHEMA;
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::Result;

    use crate::tests::{
        sensor_plan, test_plan, test_proof_receipt, test_run_args, test_terminal_state,
    };
    use crate::*;

    fn required_policy_proof_request(id: &str) -> ProofRequest {
        ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: id.to_owned(),
            lane: "intelligent-ci-policy".to_owned(),
            requested_by: vec![
                "intelligent-ci-policy".to_owned(),
                "proof-policy:cargo-check".to_owned(),
            ],
            command: "cargo check --workspace --locked".to_owned(),
            reason: "Required Rust workspace check for intelligent CI.".to_owned(),
            cost: "focused-build".to_owned(),
            timeout_sec: 300,
            required: true,
            status: "requested".to_owned(),
        }
    }

    #[test]
    fn gate_outcome_maps_terminal_states_deterministically() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        // Without deterministic blocking reasons every terminal status passes,
        // including `failed-to-review` (the model-availability case).
        for status in [
            "sufficient",
            "artifact-only",
            "needs-reviewer-attention",
            "failed-to-review",
        ] {
            let terminal_state = test_terminal_state(status);
            let gate = build_gate_outcome(GateOutcomeInput {
                args: &args,
                config: &Config::default(),
                tool_gate_outcomes: &[],
                plan: &plan,
                terminal_state: &terminal_state,
                proof_requests: &[],
                proof_receipts: &[],
                missing_or_failed_sensor_evidence: &[],
                missing_or_failed_model_evidence: &[],
            });

            assert_eq!(gate.schema, "ub-review.gate_outcome.v1");
            assert_eq!(gate.conclusion, "pass", "terminal status {status}");
            assert_eq!(gate.terminal_status, status);
            assert_eq!(gate.tool_gates.evaluated, 0);
            assert!(gate.reasons.is_empty(), "terminal status {status}");
        }
    }

    #[test]
    fn gate_outcome_passes_on_model_unavailability() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let plan = test_plan(Vec::new());
        let mut terminal_state = test_terminal_state("failed-to-review");
        terminal_state.usable_model_lanes = 0;
        terminal_state.model_lanes = 0;
        terminal_state.proof_receipts = 0;
        let model_issues = vec![ModelEvidenceIssue {
            lane: "tests-oracle".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "missing_key".to_owned(),
            reason: "no provider key configured".to_owned(),
        }];

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &model_issues,
        });

        assert_eq!(gate.conclusion, "pass");
        assert_eq!(gate.terminal_status, "failed-to-review");
        assert!(gate.reasons.is_empty());
        assert_eq!(gate.evidence_gaps_blocking, 0);
        assert_eq!(gate.evidence_gaps_advisory, 1);
    }

    #[test]
    fn gate_outcome_fails_on_required_proof_failure_with_receipt() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let plan = test_plan(Vec::new());
        let request = required_policy_proof_request("proof-0000-cargocheck");
        let mut receipt = test_proof_receipt("head_failed", "failed");
        receipt.request_ids = vec![request.id.clone()];
        let terminal_state = test_terminal_state("needs-reviewer-attention");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: std::slice::from_ref(&request),
            proof_receipts: std::slice::from_ref(&receipt),
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.required_proof.matched, 1);
        assert_eq!(gate.required_proof.failed, 1);
        assert_eq!(gate.required_proof.passed, 0);
        assert_eq!(gate.required_proof.skipped, 0);
        assert_eq!(gate.reasons.len(), 1);
        assert_eq!(gate.reasons[0].kind, "required-proof");
        assert_eq!(gate.reasons[0].id, "cargo-check");
        assert_eq!(
            gate.reasons[0].receipt,
            format!("review/proof_receipts.json#{}", receipt.id)
        );
        assert!(!gate.reasons[0].detail.is_empty());
    }

    #[test]
    fn gate_outcome_fails_on_required_proof_timeout_with_receipt() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let plan = test_plan(Vec::new());
        let request = required_policy_proof_request("proof-0000-cargocheck");
        let mut receipt = test_proof_receipt("timed_out", "timed_out");
        receipt.request_ids = vec![request.id.clone()];
        let terminal_state = test_terminal_state("sufficient");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: std::slice::from_ref(&request),
            proof_receipts: std::slice::from_ref(&receipt),
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.required_proof.matched, 1);
        assert_eq!(gate.required_proof.failed, 1);
        assert_eq!(gate.reasons.len(), 1);
        assert_eq!(gate.reasons[0].kind, "required-proof");
        assert_eq!(gate.reasons[0].id, "cargo-check");
        assert_eq!(
            gate.reasons[0].receipt,
            format!("review/proof_receipts.json#{}", receipt.id)
        );
        assert!(
            gate.reasons[0].detail.contains("timed"),
            "timeout detail: {}",
            gate.reasons[0].detail
        );
    }

    #[test]
    fn gate_outcome_emits_one_reason_per_failed_required_request() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let plan = test_plan(Vec::new());
        let check_request = required_policy_proof_request("proof-0000-cargocheck");
        let mut clippy_request = required_policy_proof_request("proof-0001-cargoclippy");
        clippy_request.requested_by = vec![
            "intelligent-ci-policy".to_owned(),
            "proof-policy:cargo-clippy".to_owned(),
        ];
        let mut receipt = test_proof_receipt("head_failed", "failed");
        receipt.request_ids = vec![check_request.id.clone(), clippy_request.id.clone()];
        let requests = vec![check_request, clippy_request];
        let terminal_state = test_terminal_state("sufficient");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &requests,
            proof_receipts: std::slice::from_ref(&receipt),
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.required_proof.matched, 2);
        assert_eq!(gate.required_proof.failed, 2);
        assert_eq!(gate.reasons.len(), gate.required_proof.failed);
        let ids = gate
            .reasons
            .iter()
            .map(|reason| reason.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["cargo-check", "cargo-clippy"]);
        for reason in &gate.reasons {
            assert_eq!(
                reason.receipt,
                format!("review/proof_receipts.json#{}", receipt.id)
            );
            assert!(
                reason.detail.contains(&format!("`{}`", reason.id)),
                "detail must name its own policy id: {}",
                reason.detail
            );
        }
        // Two requests resolved by one shared receipt must still carry
        // distinct, per-requirement details.
        assert_ne!(gate.reasons[0].detail, gate.reasons[1].detail);
    }

    #[test]
    fn gate_outcome_counts_passed_and_skipped_required_proof_without_failing() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let plan = test_plan(Vec::new());
        let passed_request = required_policy_proof_request("proof-0000-passed");
        let budget_request = required_policy_proof_request("proof-0001-budget");
        let unproven_request = required_policy_proof_request("proof-0002-unproven");
        let mut passed_receipt = test_proof_receipt("head_passed", "passed");
        passed_receipt.id = "proof-receipt-passed".to_owned();
        passed_receipt.request_ids = vec![passed_request.id.clone()];
        let mut budget_receipt = test_proof_receipt("skipped_budget", "skipped");
        budget_receipt.id = "proof-receipt-budget".to_owned();
        budget_receipt.request_ids = vec![budget_request.id.clone()];
        let requests = vec![passed_request, budget_request, unproven_request];
        let receipts = vec![passed_receipt, budget_receipt];
        let terminal_state = test_terminal_state("sufficient");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &requests,
            proof_receipts: &receipts,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "pass");
        assert_eq!(gate.required_proof.matched, 3);
        assert_eq!(gate.required_proof.passed, 1);
        assert_eq!(gate.required_proof.failed, 0);
        assert_eq!(gate.required_proof.skipped, 2);
        assert!(gate.reasons.is_empty());
    }

    #[test]
    fn gate_outcome_ignores_model_flagged_required_proof() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let plan = test_plan(Vec::new());
        let mut request = required_policy_proof_request("proof-0000-model");
        request.lane = "tests-oracle".to_owned();
        request.requested_by = vec!["tests-oracle".to_owned()];
        let mut receipt = test_proof_receipt("head_failed", "failed");
        receipt.request_ids = vec![request.id.clone()];
        let terminal_state = test_terminal_state("needs-reviewer-attention");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: std::slice::from_ref(&request),
            proof_receipts: std::slice::from_ref(&receipt),
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "pass");
        assert_eq!(gate.required_proof.matched, 0);
        assert!(gate.reasons.is_empty());
    }

    #[test]
    fn gate_outcome_fails_on_required_sensor_gap_with_receipt() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let mut required_actionlint = sensor_plan("actionlint", "actionlint", false);
        required_actionlint.required = true;
        let plan = test_plan(vec![required_actionlint]);
        let issues = vec![SensorEvidenceIssue {
            sensor: "actionlint".to_owned(),
            status: "skipped".to_owned(),
            reason: "disabled by config".to_owned(),
        }];
        let terminal_state = test_terminal_state("failed-to-review");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            missing_or_failed_sensor_evidence: &issues,
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.evidence_gaps_blocking, 1);
        assert_eq!(gate.evidence_gaps_advisory, 0);
        assert_eq!(gate.reasons.len(), 1);
        assert_eq!(gate.reasons[0].kind, "required-sensor");
        assert_eq!(gate.reasons[0].id, "actionlint");
        assert_eq!(
            gate.reasons[0].receipt,
            "sensors/actionlint/ub-review-sensor-status.json"
        );

        args.mode = RunMode::ReviewByok;
        let terminal_state = test_terminal_state("sufficient");
        let review_byok_gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            missing_or_failed_sensor_evidence: &issues,
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(review_byok_gate.conclusion, "pass");
        assert_eq!(review_byok_gate.evidence_gaps_blocking, 0);
        assert_eq!(review_byok_gate.evidence_gaps_advisory, 1);
        assert!(review_byok_gate.reasons.is_empty());
    }

    #[test]
    fn gate_outcome_points_receipt_absent_sensor_gap_at_terminal_state() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let mut required_actionlint = sensor_plan("actionlint", "actionlint", false);
        required_actionlint.required = true;
        let plan = test_plan(vec![required_actionlint]);
        let issues = vec![SensorEvidenceIssue {
            sensor: "actionlint".to_owned(),
            status: "receipt-absent".to_owned(),
            reason: "sensor wrote no status receipt".to_owned(),
        }];
        let terminal_state = test_terminal_state("failed-to-review");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            missing_or_failed_sensor_evidence: &issues,
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.reasons.len(), 1);
        assert_eq!(gate.reasons[0].kind, "required-sensor");
        assert_eq!(gate.reasons[0].id, "actionlint");
        assert_eq!(gate.reasons[0].receipt, "review/terminal_state.json");
    }

    #[test]
    fn gate_outcome_keeps_missing_model_evidence_advisory() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let plan = test_plan(vec![sensor_plan("ripr", "ripr", true)]);
        let sensor_issues = vec![SensorEvidenceIssue {
            sensor: "ripr".to_owned(),
            status: "missing".to_owned(),
            reason: "command not found".to_owned(),
        }];
        let model_issues = vec![ModelEvidenceIssue {
            lane: "tests-oracle".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "missing_key".to_owned(),
            reason: "no provider key configured".to_owned(),
        }];
        let terminal_state = test_terminal_state("sufficient");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            missing_or_failed_sensor_evidence: &sensor_issues,
            missing_or_failed_model_evidence: &model_issues,
        });

        assert_eq!(gate.conclusion, "pass");
        assert_eq!(gate.evidence_gaps_blocking, 0);
        assert_eq!(gate.evidence_gaps_advisory, 2);
        assert!(gate.reasons.is_empty());
    }

    #[test]
    fn gate_outcome_fail_reasons_always_carry_receipts() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let mut required_actionlint = sensor_plan("actionlint", "actionlint", false);
        required_actionlint.required = true;
        let plan = test_plan(vec![required_actionlint]);
        let request = required_policy_proof_request("proof-0000-cargocheck");
        let mut receipt = test_proof_receipt("head_failed", "failed");
        receipt.request_ids = vec![request.id.clone()];
        let issues = vec![SensorEvidenceIssue {
            sensor: "actionlint".to_owned(),
            status: "failed".to_owned(),
            reason: "exit 1".to_owned(),
        }];
        let terminal_state = test_terminal_state("failed-to-review");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            tool_gate_outcomes: &[],
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: std::slice::from_ref(&request),
            proof_receipts: std::slice::from_ref(&receipt),
            missing_or_failed_sensor_evidence: &issues,
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.reasons.len(), 2);
        for reason in &gate.reasons {
            assert!(!reason.receipt.trim().is_empty(), "reason {}", reason.id);
        }
        assert!(
            gate.reasons.iter().all(|reason| reason.kind != "internal"),
            "specific reasons must replace the internal fallback"
        );
    }

    fn tool_gate_entry(
        tool: &str,
        outcome: &str,
        evaluated: bool,
        required: bool,
    ) -> ToolGateOutcomeEntry {
        ToolGateOutcomeEntry {
            schema: "ub-review.tool_gate_outcome.v1",
            tool: tool.to_owned(),
            policy: crate::ToolGatePolicy {
                scope: Some("on-diff".to_owned()),
                max_new_unsuppressed: Some(0),
            },
            required,
            planned_run: outcome != "not_evaluated",
            sensor_status: "ok".to_owned(),
            sensor_reason: "tool gate fixture".to_owned(),
            sensor_receipt_path: format!("sensors/{tool}/ub-review-sensor-status.json"),
            status_source: "tool-status.json",
            outcome: outcome.to_owned(),
            evaluated,
            reason: match outcome {
                "failed" => "new_unsuppressed=3 exceeds configured maximum 0".to_owned(),
                "passed" => "new_unsuppressed=0 is within configured maximum 0".to_owned(),
                other => format!("tool gate fixture outcome `{other}`"),
            },
            metrics: ToolGateOutcomeMetrics {
                new_unsuppressed: evaluated.then_some(if outcome == "failed" { 3 } else { 0 }),
            },
            source_artifacts: vec![
                format!("sensors/{tool}/ub-review-sensor-status.json"),
                "tool-status.json".to_owned(),
            ],
            packet_policy: "gate-only",
            gate_policy: "trust-affecting",
        }
    }

    #[test]
    fn gate_outcome_counts_tool_gates_and_fails_on_threshold_failure() {
        // review-byok on purpose: a configured [tools.*.gate] threshold is
        // repo policy, so the recorded verdict is mode-independent and only
        // the exit decision follows fail-on-gate.
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let entries = vec![
            tool_gate_entry("ripr", "passed", true, true),
            tool_gate_entry("unsafe-review", "failed", true, false),
            tool_gate_entry("semgrep", "not_evaluated", false, false),
        ];
        let terminal_state = test_terminal_state("sufficient");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            tool_gate_outcomes: &entries,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.tool_gates.evaluated, 2);
        assert_eq!(gate.tool_gates.passed, 1);
        assert_eq!(gate.tool_gates.failed, 1);
        assert_eq!(gate.reasons.len(), 1);
        assert_eq!(gate.reasons[0].kind, "tool-gate");
        assert_eq!(gate.reasons[0].id, "unsafe-review");
        assert_eq!(
            gate.reasons[0].receipt,
            "review/tool-gate-outcomes.json#unsafe-review"
        );
        assert!(
            gate.reasons[0]
                .detail
                .contains("exceeds configured maximum"),
            "detail: {}",
            gate.reasons[0].detail
        );
    }

    #[test]
    fn gate_outcome_keeps_unevaluated_tool_gates_non_blocking_by_default() {
        // Non-required tools whose triggers never matched, and gates without
        // evidence, must not redden the gate unless repo policy opts in.
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let entries = vec![
            tool_gate_entry("semgrep", "not_evaluated", false, false),
            tool_gate_entry("ripr", "missing_evidence", false, false),
            tool_gate_entry("unsafe-review", "missing_evidence", false, true),
        ];
        let terminal_state = test_terminal_state("sufficient");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            tool_gate_outcomes: &entries,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "pass");
        assert_eq!(gate.tool_gates.evaluated, 0);
        assert_eq!(gate.tool_gates.passed, 0);
        assert_eq!(gate.tool_gates.failed, 0);
        assert!(gate.reasons.is_empty());
    }

    #[test]
    fn gate_outcome_blocks_missing_tool_gate_evidence_only_when_policy_opts_in() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let mut config = Config::default();
        config.gate.blocking.tool_gate_missing_evidence = true;
        let entries = vec![
            tool_gate_entry("ripr", "missing_evidence", false, true),
            tool_gate_entry("semgrep", "missing_evidence", false, false),
        ];
        let terminal_state = test_terminal_state("sufficient");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &config,
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            tool_gate_outcomes: &entries,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        // Only the required tool blocks; the non-required gap stays advisory
        // even with the policy flag on.
        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.reasons.len(), 1);
        assert_eq!(gate.reasons[0].kind, "blocking-finding");
        assert_eq!(gate.reasons[0].id, "ripr");
        assert_eq!(
            gate.reasons[0].receipt,
            "review/tool-gate-outcomes.json#ripr"
        );
    }

    fn gated_tool_status_entry(tool: &str, required: bool, status: &str) -> crate::ToolStatusEntry {
        crate::ToolStatusEntry {
            id: tool.to_owned(),
            class: ToolClass::Static,
            command: tool.to_owned(),
            required_if: crate::Trigger::Diff,
            required,
            required_reason: "tool gate fixture".to_owned(),
            runtime_profile: "gh-runner".to_owned(),
            planned_run: true,
            timeout_sec: 120,
            artifact_budget_mb: 64,
            requires_lease: false,
            status: status.to_owned(),
            reason: format!("sensor fixture status `{status}`"),
            exit_code: Some(if status == "ok" { 0 } else { 101 }),
            timed_out: status == "timed_out",
            gate: Some(crate::ToolGatePolicy {
                scope: Some("on-diff".to_owned()),
                max_new_unsuppressed: Some(0),
            }),
            artifact_paths: vec![format!("sensors/{tool}/ub-review-sensor-status.json")],
        }
    }

    #[test]
    fn tool_gate_outcome_classifies_sensor_crash_as_missing_evidence() -> Result<()> {
        // A sensor that crashed or timed out never evaluated the threshold:
        // the outcome is missing evidence, never an evaluated `failed`.
        let temp = tempfile::tempdir()?;
        let tool = crate::ToolPolicy {
            id: "ripr".to_owned(),
            ..crate::ToolPolicy::default()
        };
        let policy = crate::ToolGatePolicy {
            scope: Some("on-diff".to_owned()),
            max_new_unsuppressed: Some(0),
        };
        for status in ["failed", "timed_out"] {
            let status_entry = gated_tool_status_entry("ripr", false, status);
            let outcome = crate::tool_gate_outcome_entry(
                temp.path(),
                &tool,
                policy.clone(),
                Some(&status_entry),
            );
            assert_eq!(
                outcome.outcome, "missing_evidence",
                "sensor status {status}"
            );
            assert!(!outcome.evaluated, "sensor status {status}");
            assert_eq!(outcome.metrics.new_unsuppressed, None);
            assert!(
                outcome.reason.contains("could not be evaluated"),
                "reason must say the threshold was never evaluated: {}",
                outcome.reason
            );
            assert!(
                outcome.reason.contains(status),
                "reason must name the sensor status: {}",
                outcome.reason
            );
        }
        Ok(())
    }

    #[test]
    fn gate_outcome_keeps_sensor_crash_advisory_unless_required_and_opted_in() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let tool = crate::ToolPolicy {
            id: "ripr".to_owned(),
            ..crate::ToolPolicy::default()
        };
        let policy = crate::ToolGatePolicy {
            scope: Some("on-diff".to_owned()),
            max_new_unsuppressed: Some(0),
        };
        let terminal_state = test_terminal_state("sufficient");
        let mut opted_in = Config::default();
        opted_in.gate.blocking.tool_gate_missing_evidence = true;

        // A crashed sensor on a gated NON-required tool never blocks, with
        // or without the missing-evidence opt-in.
        let non_required_status = gated_tool_status_entry("ripr", false, "failed");
        let non_required_entry = crate::tool_gate_outcome_entry(
            temp.path(),
            &tool,
            policy.clone(),
            Some(&non_required_status),
        );
        for config in [&Config::default(), &opted_in] {
            let gate = build_gate_outcome(GateOutcomeInput {
                args: &args,
                config,
                plan: &plan,
                terminal_state: &terminal_state,
                proof_requests: &[],
                proof_receipts: &[],
                tool_gate_outcomes: std::slice::from_ref(&non_required_entry),
                missing_or_failed_sensor_evidence: &[],
                missing_or_failed_model_evidence: &[],
            });
            assert_eq!(gate.conclusion, "pass");
            assert_eq!(gate.tool_gates.failed, 0);
            assert!(gate.reasons.is_empty());
        }

        // A crashed sensor on a gated REQUIRED tool blocks only through the
        // [gate.blocking] tool_gate_missing_evidence opt-in.
        let required_status = gated_tool_status_entry("ripr", true, "timed_out");
        let required_entry = crate::tool_gate_outcome_entry(
            temp.path(),
            &tool,
            policy.clone(),
            Some(&required_status),
        );
        let default_gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            tool_gate_outcomes: std::slice::from_ref(&required_entry),
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });
        assert_eq!(default_gate.conclusion, "pass");
        assert!(default_gate.reasons.is_empty());
        let opted_in_gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &opted_in,
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            tool_gate_outcomes: std::slice::from_ref(&required_entry),
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });
        assert_eq!(opted_in_gate.conclusion, "fail");
        assert_eq!(opted_in_gate.reasons.len(), 1);
        assert_eq!(opted_in_gate.reasons[0].kind, "blocking-finding");
        assert_eq!(opted_in_gate.reasons[0].id, "ripr");
        assert_eq!(
            opted_in_gate.reasons[0].receipt,
            "review/tool-gate-outcomes.json#ripr"
        );
        Ok(())
    }

    #[test]
    fn gate_outcome_blocks_evaluated_exceeded_threshold_with_receipt() -> Result<()> {
        // An ok sensor whose gate-decision receipt exceeds the configured
        // threshold is the one unconditionally-blocking tool-gate case.
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("sensors/ripr"))?;
        fs::write(
            temp.path().join("sensors/ripr/gate-decision.json"),
            br#"{"new_unsuppressed":3}"#,
        )?;
        let tool = crate::ToolPolicy {
            id: "ripr".to_owned(),
            ..crate::ToolPolicy::default()
        };
        let policy = crate::ToolGatePolicy {
            scope: Some("on-diff".to_owned()),
            max_new_unsuppressed: Some(0),
        };
        let status_entry = gated_tool_status_entry("ripr", false, "ok");
        let entry = crate::tool_gate_outcome_entry(temp.path(), &tool, policy, Some(&status_entry));
        assert_eq!(entry.outcome, "failed");
        assert!(entry.evaluated);
        assert_eq!(entry.metrics.new_unsuppressed, Some(3));

        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let terminal_state = test_terminal_state("sufficient");
        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &Config::default(),
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            tool_gate_outcomes: std::slice::from_ref(&entry),
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });
        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.tool_gates.failed, 1);
        assert_eq!(gate.reasons.len(), 1);
        assert_eq!(gate.reasons[0].kind, "tool-gate");
        assert_eq!(
            gate.reasons[0].receipt,
            "review/tool-gate-outcomes.json#ripr"
        );
        Ok(())
    }

    #[test]
    fn gate_outcome_fails_on_policy_parse_error_with_effective_config_receipt() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let config = Config {
            policy_errors: vec![crate::PolicyError {
                section: "tools.ripr.gate".to_owned(),
                detail: "invalid [tools.ripr.gate] table: unknown field `max_new`".to_owned(),
            }],
            ..Config::default()
        };
        let terminal_state = test_terminal_state("sufficient");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &config,
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &[],
            proof_receipts: &[],
            tool_gate_outcomes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        // Recorded in review-byok too: the verdict is mode-independent and
        // the exit decision follows fail-on-gate.
        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.reasons.len(), 1);
        assert_eq!(gate.reasons[0].kind, "policy");
        assert_eq!(gate.reasons[0].id, "tools.ripr.gate");
        assert!(gate.reasons[0].detail.contains("unknown field"));
        assert_eq!(gate.reasons[0].receipt, "effective-config.json");
    }

    #[test]
    fn gate_outcome_blocks_unproven_required_proof_only_when_policy_opts_in() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let plan = test_plan(Vec::new());
        let mut config = Config::default();
        config.gate.blocking.required_proof_unproven = true;
        let missing_receipt_request = required_policy_proof_request("proof-0000-cargocheck");
        let mut skipped_request = required_policy_proof_request("proof-0001-cargoclippy");
        skipped_request.requested_by = vec![
            "intelligent-ci-policy".to_owned(),
            "proof-policy:cargo-clippy".to_owned(),
        ];
        let mut skipped_receipt = test_proof_receipt("skipped_budget", "skipped");
        skipped_receipt.id = "proof-receipt-budget".to_owned();
        skipped_receipt.request_ids = vec![skipped_request.id.clone()];
        let requests = vec![missing_receipt_request, skipped_request];
        let terminal_state = test_terminal_state("sufficient");

        let gate = build_gate_outcome(GateOutcomeInput {
            args: &args,
            config: &config,
            plan: &plan,
            terminal_state: &terminal_state,
            proof_requests: &requests,
            proof_receipts: std::slice::from_ref(&skipped_receipt),
            tool_gate_outcomes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
        });

        assert_eq!(gate.conclusion, "fail");
        assert_eq!(gate.required_proof.matched, 2);
        assert_eq!(gate.required_proof.skipped, 2);
        assert_eq!(gate.required_proof.failed, 0);
        assert_eq!(gate.reasons.len(), 2);
        for reason in &gate.reasons {
            assert_eq!(reason.kind, "blocking-finding");
        }
        assert_eq!(gate.reasons[0].id, "cargo-check");
        assert_eq!(gate.reasons[0].receipt, "review/proof_requests.json");
        assert_eq!(gate.reasons[1].id, "cargo-clippy");
        assert_eq!(
            gate.reasons[1].receipt,
            "review/proof_receipts.json#proof-receipt-budget"
        );
    }
}
