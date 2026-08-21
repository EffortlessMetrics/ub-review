//! Separated result truth for the gate artifact (#839).
//!
//! `review/gate_outcome.json` historically carried one verdict field
//! (`conclusion`) that answered three different questions at once: did we
//! investigate, did the reviewer receive the result, and does the check go
//! red. A run where every instrument failed and no model lane was usable
//! therefore recorded `conclusion: "pass"` — the gate said "clean" about a
//! review that never happened.
//!
//! This module derives three independent results next to the legacy
//! `conclusion` (whose meaning and enforcement behavior are unchanged):
//!
//! - `analysis_result`: `clean | findings | limited | not_proven` — what the
//!   investigation established;
//! - `publication_result`: `posted | not_needed | failed | not_proven` — whether
//!   reviewer-facing value reached the PR surface;
//! - `gate_result`: `pass | finding | not_proven` — the truthful check verdict.
//!
//! Everything here is a pure function of receipts already in the packet, so
//! the derivation is unit-testable and model output never feeds it.

use serde::Serialize;

use crate::gate::{GateReason, GateRequiredProofCounts};
use crate::{ModelEvidenceIssue, Plan, ReviewTerminalState, SensorEvidenceIssue};

/// Structured instrument coverage, so a consumer never has to parse prose to
/// learn how much of the plan actually reported. Totals count only sensors
/// that owed evidence: a sensor the plan deliberately did not schedule
/// (profile/trigger/heavy gating, and which produced no evidence issue) never
/// owed a verdict and is not coverage the run lost.
///
/// Invariant, checked by the packet verifier:
/// `required_total - required_completed + optional_total - optional_completed
/// == failed + timed_out + skipped`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct GateSensorCoverage {
    pub(crate) required_total: usize,
    pub(crate) required_completed: usize,
    pub(crate) optional_total: usize,
    pub(crate) optional_completed: usize,
    /// Sensors that ran and demonstrated a failure (`failed`).
    pub(crate) failed: usize,
    /// Sensors whose lease expired (`timed_out`).
    pub(crate) timed_out: usize,
    /// Sensors that owed evidence and produced none for a non-failure reason
    /// (`missing`, `receipt-absent`, `artifact-gap`, `skipped`, ...).
    pub(crate) skipped: usize,
}

impl GateSensorCoverage {
    pub(crate) fn total(&self) -> usize {
        self.required_total.saturating_add(self.optional_total)
    }

    pub(crate) fn completed(&self) -> usize {
        self.required_completed
            .saturating_add(self.optional_completed)
    }

    pub(crate) fn lost(&self) -> usize {
        self.failed
            .saturating_add(self.timed_out)
            .saturating_add(self.skipped)
    }
}

/// Model-lane coverage. `budget_exhausted` is recorded explicitly (#839) but is
/// deliberately not blocking on its own: it reduces coverage, and only matters
/// for the verdict where it starved a required investigation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct GateModelCoverage {
    pub(crate) lanes_total: usize,
    pub(crate) lanes_usable: usize,
    pub(crate) budget_exhausted_lanes: usize,
    pub(crate) budget_exhausted: bool,
}

/// The separated results plus the structured coverage they were derived from.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct GateTruth {
    pub(crate) analysis_result: String,
    pub(crate) publication_result: String,
    pub(crate) gate_result: String,
    pub(crate) sensor_coverage: GateSensorCoverage,
    pub(crate) model_coverage: GateModelCoverage,
    /// Every reason some part of the run was not proven, each prefixed with a
    /// stable machine-readable token (`terminal-state:`,
    /// `required-sensor-coverage:`, `required-proof:`, `model-coverage:`,
    /// `instrument-coverage:`, `publication:`, `gate-conclusion:`) so a
    /// workflow can branch without
    /// parsing prose. Non-empty whenever any of the three results is
    /// `not_proven`, and retained even when a demonstrated finding takes
    /// precedence in `analysis_result`, so coverage loss never disappears
    /// behind a finding.
    pub(crate) not_proven_reasons: Vec<String>,
}

pub(crate) struct GateTruthInput<'a> {
    pub(crate) plan: &'a Plan,
    pub(crate) terminal_state: &'a ReviewTerminalState,
    pub(crate) sensor_issues: &'a [SensorEvidenceIssue],
    pub(crate) model_issues: &'a [ModelEvidenceIssue],
    pub(crate) reasons: &'a [GateReason],
    pub(crate) required_proof: GateRequiredProofCounts,
    /// The legacy verdict, whose meaning is unchanged: `pass | fail |
    /// inconclusive`. `gate_result` corrects it for truth without moving it.
    pub(crate) conclusion: &'a str,
}

/// Reason kinds that mean "we could not check", as opposed to "we checked and
/// found a defect". Shared with the `conclusion` derivation in `gate.rs` so the
/// two can never drift. Stale/malformed reporter turns (#857/#874) count as
/// evidence unavailable: the deciding artifact is unusable, not a demonstrated
/// code defect.
pub(crate) fn gate_reason_kind_is_evidence_unavailable(kind: &str) -> bool {
    matches!(
        kind,
        "required-sensor"
            | "required-tool-timeout"
            | "required-evidence-unavailable"
            | "reporter-evidence"
    )
}

/// A sensor evidence issue reduces coverage in exactly one bucket.
fn sensor_issue_bucket(coverage: &mut GateSensorCoverage, status: &str) {
    match status {
        "failed" => coverage.failed += 1,
        "timed_out" => coverage.timed_out += 1,
        _ => coverage.skipped += 1,
    }
}

pub(crate) fn build_sensor_coverage(
    plan: &Plan,
    issues: &[SensorEvidenceIssue],
) -> GateSensorCoverage {
    let mut coverage = GateSensorCoverage::default();
    for sensor in &plan.sensors {
        let issue = issues.iter().find(|issue| issue.sensor == sensor.id);
        if issue.is_none() && !sensor.run {
            continue;
        }
        if sensor.required {
            coverage.required_total += 1;
        } else {
            coverage.optional_total += 1;
        }
        match issue {
            Some(issue) => sensor_issue_bucket(&mut coverage, &issue.status),
            None => {
                if sensor.required {
                    coverage.required_completed += 1;
                } else {
                    coverage.optional_completed += 1;
                }
            }
        }
    }
    coverage
}

/// Required sensors that ran and demonstrated a failure. These are findings —
/// `gate.rs` raises them as `sensor-finding` reasons under intelligent-ci — and
/// they stay findings in the reported result even where repo policy left them
/// advisory and raised no gate reason.
pub(crate) fn required_failed_sensor_count(plan: &Plan, issues: &[SensorEvidenceIssue]) -> usize {
    issues
        .iter()
        .filter(|issue| {
            issue.status == "failed"
                && plan
                    .sensors
                    .iter()
                    .any(|sensor| sensor.id == issue.sensor && sensor.required)
        })
        .count()
}

/// Required sensors that owed evidence and could not deliver a verdict at all
/// (missing, timed out, receipt-absent, artifact-gap, ...). These leave a
/// requirement unproven rather than demonstrating a defect.
pub(crate) fn required_unreported_sensor_count(
    plan: &Plan,
    issues: &[SensorEvidenceIssue],
) -> usize {
    issues
        .iter()
        .filter(|issue| {
            issue.status != "failed"
                && plan
                    .sensors
                    .iter()
                    .any(|sensor| sensor.id == issue.sensor && sensor.required)
        })
        .count()
}

/// A budget-starved lane is recorded as a `skipped` model evidence issue whose
/// reason names the model call budget (see `is_model_skipped_evidence_issue`);
/// `model-mode off` uses the same status and must not read as exhaustion.
pub(crate) fn model_issue_is_budget_exhaustion(issue: &ModelEvidenceIssue) -> bool {
    issue.status == "skipped" && issue.reason.contains("budget")
}

pub(crate) fn build_model_coverage(
    terminal_state: &ReviewTerminalState,
    issues: &[ModelEvidenceIssue],
) -> GateModelCoverage {
    let budget_exhausted_lanes = issues
        .iter()
        .filter(|issue| model_issue_is_budget_exhaustion(issue))
        .count();
    GateModelCoverage {
        lanes_total: terminal_state.model_lanes,
        lanes_usable: terminal_state.usable_model_lanes,
        budget_exhausted_lanes,
        budget_exhausted: budget_exhausted_lanes > 0,
    }
}

/// Derive the three separated results. Precedence is deliberate and follows
/// #839:
///
/// - insufficient evidence can never yield `clean`;
/// - a demonstrated finding is reported as a finding even when coverage was
///   incomplete, because the coverage loss is separately visible;
/// - material findings that never reached the PR surface yield `not_proven`,
///   because a finding trapped in artifacts proves nothing to the reviewer;
/// - optional-instrument absence is visible (`limited`) but does not by itself
///   poison an otherwise sufficient run — a wholesale instrument blackout,
///   where nothing reported at all, does.
pub(crate) fn build_gate_truth(input: GateTruthInput<'_>) -> GateTruth {
    let sensor_coverage = build_sensor_coverage(input.plan, input.sensor_issues);
    let model_coverage = build_model_coverage(input.terminal_state, input.model_issues);
    let mut not_proven_reasons = Vec::new();

    if input.terminal_state.status == "failed-to-review" {
        not_proven_reasons.push(format!(
            "terminal-state: the run ended `failed-to-review` ({} of {} model lanes usable, \
             {} proof receipts)",
            model_coverage.lanes_usable,
            model_coverage.lanes_total,
            input.terminal_state.proof_receipts
        ));
    }
    // A required sensor that RAN and demonstrated a failure produced evidence —
    // it is a finding, and `gate.rs` already raises it as one. Only a required
    // sensor that could not report at all leaves a requirement unproven.
    let required_unreported = required_unreported_sensor_count(input.plan, input.sensor_issues);
    if required_unreported > 0 {
        not_proven_reasons.push(format!(
            "required-sensor-coverage: {required_unreported} of {} required sensors could not \
             report (timed_out={}, skipped={})",
            sensor_coverage.required_total, sensor_coverage.timed_out, sensor_coverage.skipped
        ));
    }
    // Likewise for proof: a failed required proof is a demonstrated finding; a
    // required proof with no passing receipt is an unproven requirement.
    if input.required_proof.skipped > 0 {
        not_proven_reasons.push(format!(
            "required-proof: {} of {} required proof requests produced no passing receipt",
            input.required_proof.skipped, input.required_proof.matched
        ));
    }
    // A model fleet that was launched and produced nothing usable proves
    // nothing, even when the terminal state stayed `artifact-only` (a dry run
    // or a provider outage under a smoke diff). `model_mode = off` plans no
    // lanes at all, so opting out of model review never trips this.
    if model_coverage.lanes_total > 0 && model_coverage.lanes_usable == 0 {
        not_proven_reasons.push(format!(
            "model-coverage: 0 of {} model lanes produced usable output",
            model_coverage.lanes_total
        ));
    }
    // One successful instrument must not erase several failed ones: when every
    // sensor that owed evidence lost it, nothing was checked at all.
    if sensor_coverage.total() > 0 && sensor_coverage.completed() == 0 {
        not_proven_reasons.push(format!(
            "instrument-coverage: no sensor produced usable evidence ({} planned, failed={}, \
             timed_out={}, skipped={})",
            sensor_coverage.total(),
            sensor_coverage.failed,
            sensor_coverage.timed_out,
            sensor_coverage.skipped
        ));
    }
    let analysis_not_proven = !not_proven_reasons.is_empty();

    let material_findings = input
        .terminal_state
        .inline_comments
        .saturating_add(input.terminal_state.substantive_summary_only_findings);
    // A deterministic finding is one an instrument or proof demonstrated. It
    // counts even when repo policy left it advisory (a required sensor failure
    // outside intelligent-ci raises no gate reason), because the reported
    // result must not go quiet just because enforcement did.
    let deterministic_finding = input
        .reasons
        .iter()
        .any(|reason| !gate_reason_kind_is_evidence_unavailable(&reason.kind))
        || required_failed_sensor_count(input.plan, input.sensor_issues) > 0
        || input.required_proof.failed > 0;
    let findings_present = material_findings > 0 || deterministic_finding;

    let coverage_limited = sensor_coverage.lost() > 0
        || model_coverage.lanes_usable < model_coverage.lanes_total
        || model_coverage.budget_exhausted;

    let analysis_result = if findings_present {
        "findings"
    } else if analysis_not_proven {
        "not_proven"
    } else if coverage_limited {
        "limited"
    } else {
        "clean"
    };

    let publication_result = if input.terminal_state.review_payload_status == "prepared" {
        // `run` only prepares the grouped review; `post` submits it and fails
        // the job on a submission error, so a prepared payload is the strongest
        // publication claim this artifact can make.
        "posted"
    } else if input.terminal_state.status == "failed-to-review" {
        not_proven_reasons.push(
            "publication: the run never reached a reviewable state, so whether a PR review was \
             needed is unproven"
                .to_owned(),
        );
        "not_proven"
    } else if material_findings > 0 {
        not_proven_reasons.push(format!(
            "publication: {} inline and {} substantive summary findings were withheld from the \
             PR-facing review (review_payload_status `{}`)",
            input.terminal_state.inline_comments,
            input.terminal_state.substantive_summary_only_findings,
            input.terminal_state.review_payload_status
        ));
        "failed"
    } else {
        "not_needed"
    };

    let publication_unproven = matches!(publication_result, "failed" | "not_proven");
    let gate_result = if publication_unproven {
        "not_proven"
    } else if input.conclusion == "fail" {
        "finding"
    } else if analysis_result == "not_proven" {
        "not_proven"
    } else if input.conclusion == "inconclusive" {
        if not_proven_reasons.is_empty() {
            not_proven_reasons.push(
                "gate-conclusion: the recorded conclusion is `inconclusive` (required evidence \
                 was unavailable)"
                    .to_owned(),
            );
        }
        "not_proven"
    } else if deterministic_finding {
        // Advisory policy keeps `conclusion` at `pass`, so enforcement is
        // unchanged, but the gate did see a demonstrated finding and says so.
        "finding"
    } else {
        "pass"
    };

    if gate_result == "not_proven" && not_proven_reasons.is_empty() {
        not_proven_reasons.push(format!(
            "gate-conclusion: the gate could not prove a result from conclusion `{}`",
            input.conclusion
        ));
    }

    GateTruth {
        analysis_result: analysis_result.to_owned(),
        publication_result: publication_result.to_owned(),
        gate_result: gate_result.to_owned(),
        sensor_coverage,
        model_coverage,
        not_proven_reasons,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SensorPlan;
    use crate::tests::{sensor_plan, test_plan, test_terminal_state};

    /// A sensor the plan scheduled. `sensor_plan`'s third argument is `run`,
    /// and `required` defaults to false.
    fn planned_sensor(id: &str, required: bool) -> SensorPlan {
        let mut sensor = sensor_plan(id, id, true);
        sensor.required = required;
        sensor
    }

    fn model_issue(lane: &str, status: &str, reason: &str) -> ModelEvidenceIssue {
        ModelEvidenceIssue {
            lane: lane.to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: status.to_owned(),
            reason: reason.to_owned(),
        }
    }

    fn sensor_issue(sensor: &str, status: &str) -> SensorEvidenceIssue {
        SensorEvidenceIssue {
            sensor: sensor.to_owned(),
            status: status.to_owned(),
            reason: format!("{sensor} {status}"),
        }
    }

    fn truth(
        plan: &Plan,
        terminal_state: &ReviewTerminalState,
        sensor_issues: &[SensorEvidenceIssue],
        model_issues: &[ModelEvidenceIssue],
        reasons: &[GateReason],
        conclusion: &str,
    ) -> GateTruth {
        build_gate_truth(GateTruthInput {
            plan,
            terminal_state,
            sensor_issues,
            model_issues,
            reasons,
            required_proof: GateRequiredProofCounts::default(),
            conclusion,
        })
    }

    /// The live #839 reproduction: instruments failed, no model lane was
    /// usable, the terminal state was `failed-to-review`, and the legacy
    /// conclusion was `pass`. The separated results must not call that clean.
    #[test]
    fn failed_to_review_run_is_never_clean_or_pass() {
        let plan = test_plan(vec![
            planned_sensor("tokmd", false),
            planned_sensor("cargo-allow", false),
            planned_sensor("ast-grep", false),
        ]);
        let mut terminal_state = test_terminal_state("failed-to-review");
        terminal_state.model_lanes = 6;
        terminal_state.usable_model_lanes = 0;
        terminal_state.proof_receipts = 0;
        terminal_state.review_payload_status = "skipped_empty_smoke".to_owned();
        let sensor_issues = vec![
            sensor_issue("tokmd", "failed"),
            sensor_issue("cargo-allow", "failed"),
            sensor_issue("ast-grep", "missing"),
        ];
        let model_issues = (0..6)
            .map(|index| {
                model_issue(
                    &format!("lane-{index}"),
                    "missing_key",
                    "minimax API key not provided",
                )
            })
            .collect::<Vec<_>>();

        let truth = truth(
            &plan,
            &terminal_state,
            &sensor_issues,
            &model_issues,
            &[],
            "pass",
        );

        assert_eq!(truth.analysis_result, "not_proven");
        assert_eq!(truth.publication_result, "not_proven");
        assert_eq!(truth.gate_result, "not_proven");
        assert_eq!(truth.sensor_coverage.failed, 2);
        assert_eq!(truth.sensor_coverage.skipped, 1);
        assert_eq!(truth.sensor_coverage.optional_total, 3);
        assert_eq!(truth.sensor_coverage.optional_completed, 0);
        assert_eq!(truth.model_coverage.lanes_usable, 0);
        assert_eq!(truth.model_coverage.lanes_total, 6);
        assert!(
            truth
                .not_proven_reasons
                .iter()
                .any(|reason| reason.starts_with("terminal-state:")),
            "{:?}",
            truth.not_proven_reasons
        );
        assert!(
            truth
                .not_proven_reasons
                .iter()
                .any(|reason| reason.starts_with("instrument-coverage:")),
            "{:?}",
            truth.not_proven_reasons
        );
    }

    /// The #5797-shaped incident: substantive summary findings exist, several
    /// instruments failed, no inline position was available, and the
    /// publication selector suppressed the body.
    #[test]
    fn suppressed_body_with_substantive_findings_is_findings_and_not_proven() {
        let plan = test_plan(vec![
            planned_sensor("tokmd", false),
            planned_sensor("ripr", false),
            planned_sensor("cargo-allow", false),
        ]);
        let mut terminal_state = test_terminal_state("needs-reviewer-attention");
        terminal_state.model_lanes = 4;
        terminal_state.usable_model_lanes = 2;
        terminal_state.inline_comments = 0;
        terminal_state.summary_only_findings = 5;
        terminal_state.substantive_summary_only_findings = 3;
        terminal_state.reviewer_value_present = true;
        terminal_state.review_payload_status = "skipped_artifact_only_body".to_owned();
        let sensor_issues = vec![
            sensor_issue("tokmd", "failed"),
            sensor_issue("ripr", "timed_out"),
        ];
        let model_issues = vec![model_issue(
            "opposition",
            "skipped",
            "model call budget reached before lane execution",
        )];

        let truth = truth(
            &plan,
            &terminal_state,
            &sensor_issues,
            &model_issues,
            &[],
            "pass",
        );

        assert_eq!(truth.analysis_result, "findings");
        assert_eq!(truth.publication_result, "failed");
        assert_eq!(truth.gate_result, "not_proven");
        assert!(truth.model_coverage.budget_exhausted);
        assert_eq!(truth.model_coverage.budget_exhausted_lanes, 1);
        assert_eq!(truth.sensor_coverage.failed, 1);
        assert_eq!(truth.sensor_coverage.timed_out, 1);
        assert_eq!(truth.sensor_coverage.optional_completed, 1);
        assert!(
            truth
                .not_proven_reasons
                .iter()
                .any(|reason| reason.starts_with("publication:")
                    && reason.contains("skipped_artifact_only_body")),
            "{:?}",
            truth.not_proven_reasons
        );
    }

    /// A provider outage under a dry run leaves the terminal state
    /// `artifact-only`, so the `failed-to-review` rule never fires. A model
    /// fleet that produced nothing usable still proves nothing.
    #[test]
    fn artifact_only_run_with_no_usable_lane_is_not_proven() {
        let plan = test_plan(vec![planned_sensor("cargo-allow", false)]);
        let mut terminal_state = test_terminal_state("artifact-only");
        terminal_state.model_lanes = 1;
        terminal_state.usable_model_lanes = 0;
        terminal_state.review_payload_status = "skipped_empty_smoke".to_owned();
        let model_issues = vec![model_issue(
            "correctness",
            "preflight_failed",
            "provider returned 503",
        )];

        let truth = truth(&plan, &terminal_state, &[], &model_issues, &[], "pass");

        assert_eq!(truth.analysis_result, "not_proven");
        assert_eq!(truth.gate_result, "not_proven");
        assert!(
            truth
                .not_proven_reasons
                .iter()
                .any(|reason| reason.starts_with("model-coverage:")),
            "{:?}",
            truth.not_proven_reasons
        );
    }

    #[test]
    fn complete_run_without_findings_is_clean_and_pass() {
        let plan = test_plan(vec![
            planned_sensor("tokmd", true),
            planned_sensor("ripr", false),
        ]);
        let mut terminal_state = test_terminal_state("sufficient");
        terminal_state.model_lanes = 3;
        terminal_state.usable_model_lanes = 3;
        terminal_state.review_payload_status = "skipped_empty_smoke".to_owned();

        let truth = truth(&plan, &terminal_state, &[], &[], &[], "pass");

        assert_eq!(truth.analysis_result, "clean");
        assert_eq!(truth.publication_result, "not_needed");
        assert_eq!(truth.gate_result, "pass");
        assert!(truth.not_proven_reasons.is_empty());
        assert_eq!(truth.sensor_coverage.required_total, 1);
        assert_eq!(truth.sensor_coverage.required_completed, 1);
        assert_eq!(truth.sensor_coverage.optional_completed, 1);
        assert_eq!(truth.sensor_coverage.lost(), 0);
    }

    #[test]
    fn prepared_payload_reports_posted_publication() {
        let plan = test_plan(vec![planned_sensor("tokmd", true)]);
        let mut terminal_state = test_terminal_state("needs-reviewer-attention");
        terminal_state.model_lanes = 2;
        terminal_state.usable_model_lanes = 2;
        terminal_state.inline_comments = 2;
        terminal_state.reviewer_value_present = true;
        terminal_state.review_payload_status = "prepared".to_owned();

        let truth = truth(&plan, &terminal_state, &[], &[], &[], "pass");

        assert_eq!(truth.analysis_result, "findings");
        assert_eq!(truth.publication_result, "posted");
        assert_eq!(truth.gate_result, "pass");
        assert!(truth.not_proven_reasons.is_empty());
    }

    /// An optional sensor loss is visible without poisoning a run that still
    /// gathered evidence elsewhere.
    #[test]
    fn optional_sensor_loss_is_limited_not_unproven() {
        let plan = test_plan(vec![
            planned_sensor("tokmd", true),
            planned_sensor("ast-grep", false),
        ]);
        let mut terminal_state = test_terminal_state("sufficient");
        terminal_state.model_lanes = 2;
        terminal_state.usable_model_lanes = 2;
        terminal_state.review_payload_status = "skipped_empty_smoke".to_owned();
        let sensor_issues = vec![sensor_issue("ast-grep", "missing")];

        let truth = truth(&plan, &terminal_state, &sensor_issues, &[], &[], "pass");

        assert_eq!(truth.analysis_result, "limited");
        assert_eq!(truth.publication_result, "not_needed");
        assert_eq!(truth.gate_result, "pass");
        assert!(truth.not_proven_reasons.is_empty());
        assert_eq!(truth.sensor_coverage.skipped, 1);
        assert_eq!(truth.sensor_coverage.required_completed, 1);
    }

    /// A required sensor that could not report leaves a requirement unproven
    /// even outside intelligent-ci, where the legacy conclusion keeps it
    /// advisory. Enforcement is unchanged; the report is not.
    #[test]
    fn unreported_required_sensor_is_not_proven_even_when_advisory() {
        let plan = test_plan(vec![
            planned_sensor("tokmd", true),
            planned_sensor("ripr", false),
        ]);
        let mut terminal_state = test_terminal_state("sufficient");
        terminal_state.model_lanes = 2;
        terminal_state.usable_model_lanes = 2;
        terminal_state.review_payload_status = "skipped_empty_smoke".to_owned();
        let sensor_issues = vec![sensor_issue("tokmd", "timed_out")];

        let truth = truth(&plan, &terminal_state, &sensor_issues, &[], &[], "pass");

        assert_eq!(truth.analysis_result, "not_proven");
        assert_eq!(truth.gate_result, "not_proven");
        assert_eq!(truth.sensor_coverage.required_total, 1);
        assert_eq!(truth.sensor_coverage.required_completed, 0);
        assert!(
            truth
                .not_proven_reasons
                .iter()
                .any(|reason| reason.starts_with("required-sensor-coverage:")),
            "{:?}",
            truth.not_proven_reasons
        );
    }

    /// A required sensor that ran and demonstrated a failure is a finding, not
    /// missing evidence, and the reported gate result says `finding` even where
    /// repo policy left it advisory and `conclusion` stayed `pass`.
    #[test]
    fn failed_required_sensor_reports_finding_under_advisory_policy() {
        let plan = test_plan(vec![
            planned_sensor("tokmd", true),
            planned_sensor("ripr", false),
        ]);
        let mut terminal_state = test_terminal_state("sufficient");
        terminal_state.model_lanes = 2;
        terminal_state.usable_model_lanes = 2;
        terminal_state.review_payload_status = "skipped_empty_smoke".to_owned();
        let sensor_issues = vec![sensor_issue("tokmd", "failed")];

        let truth = truth(&plan, &terminal_state, &sensor_issues, &[], &[], "pass");

        assert_eq!(truth.analysis_result, "findings");
        assert_eq!(truth.publication_result, "not_needed");
        assert_eq!(truth.gate_result, "finding");
        assert_eq!(truth.sensor_coverage.failed, 1);
    }

    #[test]
    fn demonstrated_blocking_reason_reports_finding() {
        let plan = test_plan(vec![planned_sensor("cargo-clippy", true)]);
        let mut terminal_state = test_terminal_state("sufficient");
        terminal_state.model_lanes = 1;
        terminal_state.usable_model_lanes = 1;
        terminal_state.review_payload_status = "skipped_empty_smoke".to_owned();
        let reasons = vec![GateReason {
            kind: "sensor-finding".to_owned(),
            id: "cargo-clippy".to_owned(),
            detail: "required sensor demonstrated a failure".to_owned(),
            receipt: "sensors/cargo-clippy/ub-review-sensor-status.json".to_owned(),
            next_action: None,
        }];
        // The sensor issue is a demonstrated finding, so it is not a coverage
        // completion, but the verdict is a finding rather than an unproven run.
        let sensor_issues = vec![sensor_issue("cargo-clippy", "failed")];

        let truth = truth(
            &plan,
            &terminal_state,
            &sensor_issues,
            &[],
            &reasons,
            "fail",
        );

        assert_eq!(truth.analysis_result, "findings");
        assert_eq!(truth.publication_result, "not_needed");
        assert_eq!(truth.gate_result, "finding");
    }

    #[test]
    fn inconclusive_conclusion_reports_not_proven_with_a_reason() {
        let plan = test_plan(vec![planned_sensor("ripr", true)]);
        let mut terminal_state = test_terminal_state("sufficient");
        terminal_state.model_lanes = 1;
        terminal_state.usable_model_lanes = 1;
        terminal_state.review_payload_status = "skipped_empty_smoke".to_owned();
        let reasons = vec![GateReason {
            kind: "required-sensor".to_owned(),
            id: "ripr".to_owned(),
            detail: "required sensor evidence gap".to_owned(),
            receipt: "review/terminal_state.json".to_owned(),
            next_action: None,
        }];
        let sensor_issues = vec![sensor_issue("ripr", "receipt-absent")];

        let truth = truth(
            &plan,
            &terminal_state,
            &sensor_issues,
            &[],
            &reasons,
            "inconclusive",
        );

        assert_eq!(truth.analysis_result, "not_proven");
        assert_eq!(truth.gate_result, "not_proven");
        assert!(!truth.not_proven_reasons.is_empty());
    }

    /// Sensors the plan never scheduled owe no evidence, so they must not
    /// inflate the coverage denominator.
    #[test]
    fn unscheduled_sensors_are_outside_coverage() {
        let mut disabled = planned_sensor("gitleaks", false);
        disabled.run = false;
        let plan = test_plan(vec![planned_sensor("tokmd", true), disabled]);
        let mut terminal_state = test_terminal_state("sufficient");
        terminal_state.model_lanes = 1;
        terminal_state.usable_model_lanes = 1;
        terminal_state.review_payload_status = "skipped_empty_smoke".to_owned();

        let coverage = build_sensor_coverage(&plan, &[]);

        assert_eq!(coverage.total(), 1);
        assert_eq!(coverage.required_total, 1);
        assert_eq!(coverage.optional_total, 0);
        let truth = truth(&plan, &terminal_state, &[], &[], &[], "pass");
        assert_eq!(truth.analysis_result, "clean");
    }

    #[test]
    fn coverage_counts_add_up() {
        let plan = test_plan(vec![
            planned_sensor("a", true),
            planned_sensor("b", true),
            planned_sensor("c", false),
            planned_sensor("d", false),
        ]);
        let issues = vec![
            sensor_issue("a", "timed_out"),
            sensor_issue("c", "failed"),
            sensor_issue("d", "artifact-gap"),
        ];

        let coverage = build_sensor_coverage(&plan, &issues);

        assert_eq!(coverage.required_total, 2);
        assert_eq!(coverage.required_completed, 1);
        assert_eq!(coverage.optional_total, 2);
        assert_eq!(coverage.optional_completed, 0);
        assert_eq!(coverage.failed, 1);
        assert_eq!(coverage.timed_out, 1);
        assert_eq!(coverage.skipped, 1);
        assert_eq!(
            coverage.total() - coverage.completed(),
            coverage.lost(),
            "coverage loss must equal the bucketed issue count"
        );
    }
}
