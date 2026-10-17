//! Review body rendering: compose the structured review body and the GitHub PR
//! review body from findings, observations, proof receipts, and shared context.

use crate::*;

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn render_review_body(
    shared_context_id: &str,
    plan: &Plan,
    diff: &DiffContext,
    model_lanes: &[ModelLaneReceipt],
    missing_or_failed_sensor_evidence: &[SensorEvidenceIssue],
    missing_or_failed_model_evidence: &[ModelEvidenceIssue],
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    observations: &[Observation],
    proof_receipts: &[ProofReceipt],
    review_body_max_bytes: usize,
    audience: ReviewBodyAudience,
) -> String {
    if matches!(audience, ReviewBodyAudience::PullRequest) {
        return render_pull_request_review_body(
            shared_context_id,
            plan,
            diff,
            missing_or_failed_sensor_evidence,
            missing_or_failed_model_evidence,
            inline_comments,
            summary_only_findings,
            observations,
            proof_receipts,
            review_body_max_bytes,
        );
    }

    let actionable = has_actionable_review_finding(inline_comments, summary_only_findings);
    let usable_model_lanes = model_lanes
        .iter()
        .filter(|receipt| model_lane_is_usable_for_terminal_state(receipt))
        .count();
    let mut text = String::new();
    text.push_str(&format!(
        "# {}\n\n",
        pr_review_heading_for_diff_class(plan.diff_class)
    ));
    text.push_str(&format!("- Shared context: `{shared_context_id}`\n"));
    text.push_str(&format!("- Profile: `{}`\n", plan.profile_name));
    text.push_str(&format!("- Base: `{}`\n", plan.base));
    text.push_str(&format!("- Head: `{}`\n", plan.head));
    text.push_str(&format!(
        "- Changed files: `{}`\n",
        diff.changed_files.len()
    ));
    text.push_str(&format!("- Inline comments: `{}`\n", inline_comments.len()));
    text.push_str("\n## Decision\n\n");
    text.push_str(&format!(
        "- {}\n",
        if !actionable && usable_model_lanes == 0 {
            "Model review was not established; no actionable finding is recorded in this artifact."
        } else {
            review_decision(
                missing_or_failed_sensor_evidence,
                missing_or_failed_model_evidence,
                inline_comments,
                summary_only_findings,
            )
        }
    ));

    if !actionable && usable_model_lanes > 0 {
        text.push_str("\n## No blocking finding after checking\n\n");
        text.push_str(&format!(
            "- Usable model lane receipts recorded: `{usable_model_lanes}`; incomplete evidence is listed below.\n"
        ));
        text.push_str(
            "- Inline-comment guardrails found no validated candidate comments to post.\n",
        );
    }

    text.push_str("\n## Confirmed findings\n\n");
    if inline_comments.is_empty() {
        text.push_str("- None validated for inline posting.\n");
    } else {
        for comment in inline_comments {
            text.push_str(&format!(
                "- `[{}]` `{}` `{}` at `{}`:{}: {} Evidence: {}\n",
                comment.lane,
                comment.severity,
                comment.confidence,
                comment.path,
                comment.line,
                escape_md(&comment.body),
                escape_md(&comment.evidence)
            ));
        }
    }

    text.push_str("\n## Summary-only findings\n\n");
    if summary_only_findings.is_empty() {
        text.push_str("- None.\n");
    } else {
        for finding in summary_only_findings {
            text.push_str(&format!(
                "- `[{}]` `{}` `{}`: {} Evidence: {}\n",
                finding.lane,
                finding.severity,
                finding.confidence,
                escape_md(&finding.reason),
                escape_md(&finding.evidence)
            ));
        }
    }

    text.push_str("\n## Failed objections\n\n");
    let refuters = model_lanes
        .iter()
        .filter(|receipt| receipt.lane == "refuter")
        .collect::<Vec<_>>();
    if refuters.is_empty() {
        text.push_str(
            "- No refuter receipt was recorded; a completed refuter pass is not established.\n",
        );
    }
    for receipt in refuters {
        text.push_str(&format!(
            "- Refuter receipt: `{}` - {}\n",
            escape_md(&receipt.status),
            escape_md(&receipt.reason)
        ));
    }
    text.push_str("- Missing evidence is not a failed objection; it is listed separately below.\n");

    text.push_str("\n## Residual risk\n\n");
    text.push_str(&format!(
        "- {}\n",
        residual_risk_for_diff_class(plan.diff_class)
    ));

    text.push_str("\n## Parked follow-ups\n\n");
    let parked = summary_only_findings
        .iter()
        .filter(|finding| is_parked_follow_up(finding))
        .collect::<Vec<_>>();
    if parked.is_empty() {
        text.push_str(
            "- No parked follow-up was promoted from ledger or lane evidence in this run.\n",
        );
    } else {
        for finding in parked {
            text.push_str(&format!(
                "- `[{}]` {} Evidence: {}\n",
                finding.lane,
                escape_md(&finding.reason),
                escape_md(&finding.evidence)
            ));
        }
    }

    text.push_str("\n## Missing or failed evidence\n\n");
    if missing_or_failed_sensor_evidence.is_empty() && missing_or_failed_model_evidence.is_empty() {
        text.push_str("- None recorded.\n");
    } else {
        for issue in missing_or_failed_sensor_evidence {
            text.push_str(&format!(
                "- Sensor `{}`: `{}` - {}\n",
                issue.sensor,
                issue.status,
                escape_md(&issue.reason)
            ));
        }
        for issue in missing_or_failed_model_evidence {
            text.push_str(&format!(
                "- Lane `{}` via `{}` model `{}` endpoint `{}`: `{}` - {}\n",
                issue.lane,
                issue.provider,
                issue.model,
                issue.endpoint_kind,
                issue.status,
                escape_md(&issue.reason)
            ));
        }
    }

    if audience.include_successful_lane_table() {
        text.push_str("\n## Model lanes\n\n");
        for lane in model_lanes {
            text.push_str(&format!(
                "- Lane: `{}`\n  Provider: `{}`\n  Model: `{}`\n  Status: `{}` - {}\n",
                lane.lane,
                lane.provider,
                lane.model,
                lane.status,
                escape_md(&lane.reason)
            ));
        }
    }
    cap_review_body(text, review_body_max_bytes)
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn render_pull_request_review_body(
    _shared_context_id: &str,
    _plan: &Plan,
    diff: &DiffContext,
    _missing_or_failed_sensor_evidence: &[SensorEvidenceIssue],
    _missing_or_failed_model_evidence: &[ModelEvidenceIssue],
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    observations: &[Observation],
    proof_receipts: &[ProofReceipt],
    review_body_max_bytes: usize,
) -> String {
    let mut text = String::new();
    let observation_items =
        unique_review_observations_by_claim(unique_review_observations(observations));
    let pr_observation_items = observation_items
        .iter()
        .filter(|observation| {
            !is_pr_body_artifact_only_observation(observation)
                && !observation
                    .sources
                    .iter()
                    .any(|source| source == CROSS_LANE_CONFLICT_SOURCE)
                && !is_unexecuted_proof_homework_text(&observation.claim)
                && !is_pr_body_stale_for_current_diff_observation(observation, diff)
                && !proof_receipts_answer_observation_test_witness_question(
                    proof_receipts,
                    observation,
                )
                && !diff_structurally_answers_observation_test_witness_question(diff, observation)
        })
        .collect::<Vec<_>>();
    let refuted_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| is_pr_body_refuted_observation(observation))
        .collect::<Vec<_>>();
    let missing_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| is_missing_evidence_observation(observation))
        .collect::<Vec<_>>();
    let parked_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| is_parked_observation(observation))
        .collect::<Vec<_>>();
    let verification_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| {
            !is_refuted_observation(observation)
                && !is_missing_evidence_observation(observation)
                && !is_parked_observation(observation)
                && !is_residual_risk_observation(observation)
                && is_verification_observation(observation)
        })
        .collect::<Vec<_>>();
    let concern_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| {
            !is_refuted_observation(observation)
                && !is_missing_evidence_observation(observation)
                && !is_parked_observation(observation)
                && !is_residual_risk_observation(observation)
                && !is_verification_observation(observation)
        })
        .collect::<Vec<_>>();
    let parked = unique_summary_review_findings(summary_only_findings.iter().filter(|finding| {
        !is_pr_body_artifact_only_finding(finding)
            && !is_pr_body_stale_for_current_diff(finding, diff)
            && !summary_finding_has_cross_lane_conflict(finding, &observation_items)
            && !proof_receipts_answer_summary_test_witness_question(proof_receipts, finding)
            && !diff_structurally_answers_summary_test_witness_question(diff, finding)
            && is_parked_follow_up(finding)
            && !summary_finding_matches_observations(finding, &observation_items)
    }));
    let verification_questions =
        unique_summary_review_findings(summary_only_findings.iter().filter(|finding| {
            !is_pr_body_artifact_only_finding(finding)
                && !is_unexecuted_proof_homework_text(&finding.reason)
                && !is_pr_body_stale_for_current_diff(finding, diff)
                && !summary_finding_has_cross_lane_conflict(finding, &observation_items)
                && !proof_receipts_answer_summary_test_witness_question(proof_receipts, finding)
                && !diff_structurally_answers_summary_test_witness_question(diff, finding)
                && !is_parked_follow_up(finding)
                && is_verification_question(finding)
                && !summary_finding_matches_observations(finding, &observation_items)
        }));
    let summary_concerns =
        unique_summary_review_findings(summary_only_findings.iter().filter(|finding| {
            !is_pr_body_artifact_only_finding(finding)
                && !is_unexecuted_proof_homework_text(&finding.reason)
                && !is_pr_body_stale_for_current_diff(finding, diff)
                && !summary_finding_has_cross_lane_conflict(finding, &observation_items)
                && !proof_receipts_answer_summary_test_witness_question(proof_receipts, finding)
                && !diff_structurally_answers_summary_test_witness_question(diff, finding)
                && !is_parked_follow_up(finding)
                && !is_verification_question(finding)
                && !summary_finding_matches_observations(finding, &observation_items)
        }));
    // A missing receipt is useful to a reviewer only when it is still linked
    // to a surviving missing-evidence observation. A receipt from another
    // lane or an already-answered question must remain artifact-only; the
    // presence of a failed proof elsewhere is not itself a public claim.
    let missing_proof_receipts = proof_receipts
        .iter()
        .filter(|receipt| {
            proof_receipt_is_missing_evidence(receipt)
                && missing_observations
                    .iter()
                    .any(|observation| proof_receipt_matches_observation(receipt, observation))
        })
        .collect::<Vec<_>>();
    let has_specific_missing_evidence =
        !missing_observations.is_empty() || !missing_proof_receipts.is_empty();
    let proof_result_receipts = proof_receipts
        .iter()
        .filter(|receipt| proof_receipt_is_public_test_proof_result(receipt))
        .collect::<Vec<_>>();
    let current_proof_failure = proof_receipts
        .iter()
        .any(|receipt| receipt.result == "head_failed");
    let finding_count = inline_comments.len() + summary_concerns.len() + concern_observations.len();
    let verification_count = verification_questions.len() + verification_observations.len();
    let has_test_proof_verification = verification_questions
        .iter()
        .any(summary_finding_is_test_proof_decision_question)
        || verification_observations
            .iter()
            .any(observation_is_test_proof_decision_question);
    let decision_sentence = pr_decision_sentence(PrDecisionContext {
        finding_count,
        verification_count,
        has_test_proof_verification,
        current_proof_failure,
    });
    let has_decision_item = decision_sentence.is_some();
    let has_reviewer_value_item = has_decision_item
        || !inline_comments.is_empty()
        || !proof_result_receipts.is_empty()
        || !parked.is_empty()
        || !parked_observations.is_empty()
        || has_specific_missing_evidence;
    if !has_reviewer_value_item {
        return String::new();
    }

    if let Some(decision_sentence) = decision_sentence {
        push_pr_section_heading(&mut text, "## Decision");
        text.push_str("- ");
        text.push_str(decision_sentence);
        text.push('\n');
    }

    let summary_concerns = summary_concerns
        .into_iter()
        .filter(|finding| {
            !inline_comments
                .iter()
                .any(|comment| review_claims_match(&comment.body, &finding.reason))
        })
        .collect::<Vec<_>>();
    let concern_observations = concern_observations
        .into_iter()
        .filter(|observation| {
            !inline_comments
                .iter()
                .any(|comment| review_claims_match(&comment.body, &observation.claim))
        })
        .collect::<Vec<_>>();
    // One heading, emitted once. Inline comments lead because they are the
    // anchored findings; the summary-only items follow. Previously each group
    // opened its own `## Confirmed findings` block, so any review that had both
    // an inline comment and a summary finding shipped the same H2 twice.
    if !inline_comments.is_empty()
        || !summary_concerns.is_empty()
        || !concern_observations.is_empty()
    {
        push_pr_section_heading(&mut text, "## Confirmed findings");
        for comment in inline_comments {
            render_pr_model_signal(&mut text, &comment.body);
        }
        for observation in &concern_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
        for finding in summary_concerns {
            render_pr_model_signal(&mut text, &finding.reason);
        }
    }

    if !verification_questions.is_empty() {
        push_pr_section_heading(&mut text, "## Verification questions");
        for observation in &verification_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Verification);
        }
        for finding in verification_questions {
            render_pr_model_verification(&mut text, &finding.reason);
        }
    } else if !verification_observations.is_empty() {
        push_pr_section_heading(&mut text, "## Verification questions");
        for observation in &verification_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Verification);
        }
    }

    if !refuted_observations.is_empty() {
        push_pr_section_heading(&mut text, "## Refuted");
        for observation in &refuted_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
    }

    if !proof_result_receipts.is_empty() {
        if proof_result_receipts
            .iter()
            .any(|receipt| receipt.kind == "focused-build")
        {
            push_pr_section_heading(&mut text, "## Proof results");
        } else {
            push_pr_section_heading(&mut text, "## Test proof");
        }
        for receipt in proof_result_receipts {
            render_proof_receipt_summary(&mut text, receipt);
        }
    }

    if !parked.is_empty() {
        push_pr_section_heading(&mut text, "## Parked follow-ups");
        for observation in &parked_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
        for finding in parked {
            render_pr_model_signal(&mut text, &finding.reason);
        }
    } else if !parked_observations.is_empty() {
        push_pr_section_heading(&mut text, "## Parked follow-ups");
        for observation in &parked_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
    }

    if has_specific_missing_evidence {
        push_pr_section_heading(&mut text, "## Evidence gaps");
        for observation in &missing_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
        for receipt in missing_proof_receipts {
            render_missing_proof_receipt_summary(&mut text, receipt);
        }
    }

    cap_review_body(text, review_body_max_bytes)
}

/// Open a PR-body section, separated from the previous one by exactly one
/// blank line.
///
/// This owns the separator invariant rather than leaving each call site to
/// hardcode a leading newline. Every section but `## Decision` used to embed
/// its own `\n` prefix, so whenever the decision sentence was suppressed the
/// posted review opened with a blank line — a visible glitch at the very top
/// of the body, and one that no `contains` assertion could catch.
fn push_pr_section_heading(text: &mut String, heading: &str) {
    if !text.is_empty() {
        text.push('\n');
    }
    text.push_str(heading);
    text.push_str("\n\n");
}

/// Return true only when the receipt carries an exact identity emitted by the
/// surviving observation. Lane ownership is intentionally insufficient: a
/// lane may ask several unrelated questions, and one of those receipts must
/// not make every other question public.
fn proof_receipt_matches_observation(
    receipt: &ProofReceipt,
    observation: &ObservationGroup,
) -> bool {
    receipt.request_ids.iter().any(|request_id| {
        request_id == &observation.id
            || request_id == &observation.dedupe_key
            || observation
                .observation_ids
                .iter()
                .any(|id| id == request_id)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact_lane(lane: &str, status: &str, reason: &str) -> Result<ModelLaneReceipt> {
        Ok(serde_json::from_value(serde_json::json!({
            "lane": lane, "provider": "minimax", "model": "test-model",
            "endpoint_kind": "openai-chat", "status": status, "reason": reason,
            "duration_ms": null, "http_status": null, "response_shape": null,
            "fallback_from": null
        }))?)
    }

    fn artifact_surface(
        mode: ModelMode,
        lanes: &[ModelLaneReceipt],
        findings: &[SummaryOnlyFinding],
    ) -> Result<CompiledReviewSurface> {
        let mut args = crate::tests::test_run_args(PathBuf::from("unused-artifact-test"));
        args.model_mode = mode;
        let config = Config::default();
        let issues = lanes
            .iter()
            .filter(|lane| is_model_receipt_evidence_issue(lane))
            .map(model_issue_from_receipt)
            .collect::<Vec<_>>();
        compile_review_surface(ReviewCompilerInput {
            shared_context_id: "artifact-reporting-test",
            review_body_policy: &config.review_body,
            run_pass: RunPass::Manual,
            post_review_on: &config.gate.post_review_on,
            args: &args,
            plan: &crate::tests::test_plan(Vec::new()),
            diff: &crate::tests::test_diff(),
            model_lanes: lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &issues,
            inline_comments: &[],
            summary_only_findings: findings,
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })
    }

    fn unavailable_artifact(mode: ModelMode) -> Result<String> {
        let (status, reason) = match mode {
            ModelMode::Off => ("skipped", "model-mode off"),
            ModelMode::Auto => ("missing_key", "provider key not supplied"),
        };
        let mut lanes = vec![artifact_lane("tests", status, reason)?];
        if matches!(mode, ModelMode::Auto) {
            lanes.push(artifact_lane(
                "refuter",
                "skipped",
                "no inline candidates passed guardrails before refuter",
            )?);
        }
        let surface = artifact_surface(mode, &lanes, &[])?;
        assert_eq!(surface.terminal_state.usable_model_lanes, 0);
        Ok(surface.artifact_body)
    }

    #[test]
    fn artifact_reporting_model_off_does_not_claim_checked_review() -> Result<()> {
        let body = unavailable_artifact(ModelMode::Off)?;
        assert!(
            !body.contains("## No blocking finding after checking"),
            "{body}"
        );
        assert!(
            !body.contains("No blocking finding after bounded review"),
            "{body}"
        );
        assert!(body.contains("Model review was not established"), "{body}");
        Ok(())
    }

    #[test]
    fn artifact_reporting_model_off_does_not_claim_completed_refuter() -> Result<()> {
        let body = unavailable_artifact(ModelMode::Off)?;
        assert!(!body.contains("no blocker/high/medium candidate survived the bounded lane run, diff-line validation, and refuter path."), "{body}");
        assert!(body.contains("No refuter receipt was recorded"), "{body}");
        Ok(())
    }

    #[test]
    fn artifact_reporting_missing_key_does_not_claim_checked_review() -> Result<()> {
        let body = unavailable_artifact(ModelMode::Auto)?;
        assert!(
            !body.contains("## No blocking finding after checking"),
            "{body}"
        );
        assert!(
            !body.contains("No blocking finding after bounded review"),
            "{body}"
        );
        assert!(body.contains("Model review was not established"), "{body}");
        Ok(())
    }

    #[test]
    fn artifact_reporting_missing_key_does_not_claim_completed_refuter() -> Result<()> {
        let body = unavailable_artifact(ModelMode::Auto)?;
        assert!(!body.contains("no blocker/high/medium candidate survived the bounded lane run, diff-line validation, and refuter path."), "{body}");
        assert!(
            body.contains(
                "Refuter receipt: `skipped` - no inline candidates passed guardrails before refuter"
            ),
            "{body}"
        );
        Ok(())
    }

    #[test]
    fn artifact_reporting_complete_review_keeps_bounded_result_and_receipts() -> Result<()> {
        let lanes = [
            artifact_lane("tests", "ok", "completed")?,
            artifact_lane("refuter", "ok", "completed")?,
        ];
        let surface = artifact_surface(ModelMode::Auto, &lanes, &[])?;
        assert!(
            surface
                .artifact_body
                .contains("## No blocking finding after checking"),
            "{}",
            surface.artifact_body
        );
        assert!(surface.artifact_body.contains(
            "No blocking finding after bounded review; residual risk remains for human review."
        ));
        assert!(
            surface
                .artifact_body
                .contains("Refuter receipt: `ok` - completed")
        );
        assert!(surface.artifact_body.contains("Lane: `tests`"));
        assert!(
            !surface
                .artifact_body
                .contains("Model review was not established")
        );
        Ok(())
    }

    #[test]
    fn artifact_reporting_intentional_refuter_skip_preserves_usable_review() -> Result<()> {
        let lanes = [
            artifact_lane("tests", "ok", "completed")?,
            artifact_lane(
                "refuter",
                "skipped",
                "no inline candidates passed guardrails before refuter",
            )?,
        ];
        let surface = artifact_surface(ModelMode::Auto, &lanes, &[])?;
        assert!(
            surface
                .artifact_body
                .contains("## No blocking finding after checking")
        );
        assert!(surface.artifact_body.contains(
            "Refuter receipt: `skipped` - no inline candidates passed guardrails before refuter"
        ));
        assert!(!surface.artifact_body.contains("refuter path."));
        assert!(
            !surface
                .artifact_body
                .contains("Model review was not established")
        );
        assert!(!surface.artifact_body.contains("Lane `refuter` via"));
        Ok(())
    }

    #[test]
    fn artifact_reporting_actionable_finding_survives_missing_model_review() -> Result<()> {
        let finding = SummaryOnlyFinding {
            lane: "tests".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            reason: "Buffer boundary can overrun after length conversion.".to_owned(),
            evidence: "src/lib.rs current route".to_owned(),
        };
        let surface = artifact_surface(ModelMode::Off, &[], &[finding])?;
        assert!(
            surface
                .artifact_body
                .contains("Needs reviewer attention before upstream")
        );
        assert!(
            surface
                .artifact_body
                .contains("Buffer boundary can overrun after length conversion.")
        );
        assert!(
            !surface
                .artifact_body
                .contains("## No blocking finding after checking")
        );
        assert!(surface.artifact_body.contains("`medium` `high`"));
        Ok(())
    }
}
