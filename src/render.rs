//! Review body rendering: render_review_body and
//! render_pull_request_review_body (cleanup train step 22, pure code motion).
//! These compose the structured review body and the GitHub PR review body
//! from findings, observations, proof receipts, and shared context.

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
        review_decision(
            missing_or_failed_sensor_evidence,
            missing_or_failed_model_evidence,
            inline_comments,
            summary_only_findings
        )
    ));

    if !has_actionable_review_finding(inline_comments, summary_only_findings) {
        text.push_str("\n## No blocking finding after checking\n\n");
        text.push_str("- Shared diff packet, changed-file list, diff flags, sensor receipts, model lane receipts, and Bun lane packet prompts.\n");
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
    if has_actionable_review_finding(inline_comments, summary_only_findings) {
        text.push_str("- Refuter and diff-line guardrails kept uncertain, duplicate, low-confidence, and off-diff objections out of inline comments.\n");
    } else {
        text.push_str("- Strongest failed objection: a model or sensor may have found a real issue, but no blocker/high/medium candidate survived the bounded lane run, diff-line validation, and refuter path.\n");
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
    let observation_items = unique_review_observations(observations);
    let pr_observation_items = observation_items
        .iter()
        .filter(|observation| {
            !is_pr_body_artifact_only_observation(observation)
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
                && !is_pr_body_stale_for_current_diff(finding, diff)
                && !summary_finding_has_cross_lane_conflict(finding, &observation_items)
                && !proof_receipts_answer_summary_test_witness_question(proof_receipts, finding)
                && !diff_structurally_answers_summary_test_witness_question(diff, finding)
                && !is_parked_follow_up(finding)
                && !is_verification_question(finding)
                && !summary_finding_matches_observations(finding, &observation_items)
        }));
    let has_specific_missing_evidence = !missing_observations.is_empty()
        || proof_receipts.iter().any(proof_receipt_is_missing_evidence);
    let proof_result_receipts = proof_receipts
        .iter()
        .filter(|receipt| proof_receipt_is_test_proof_result(receipt))
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
        || !proof_result_receipts.is_empty()
        || !parked.is_empty()
        || !parked_observations.is_empty()
        || has_specific_missing_evidence;
    if !has_reviewer_value_item {
        return String::new();
    }

    if let Some(decision_sentence) = decision_sentence {
        text.push_str("## Decision\n\n");
        text.push_str("- ");
        text.push_str(decision_sentence);
        text.push('\n');
    }

    if !inline_comments.is_empty()
        || !summary_concerns.is_empty()
        || !concern_observations.is_empty()
    {
        text.push_str("\n## Confirmed findings\n\n");
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
        text.push_str("\n## Verification questions\n\n");
        for observation in &verification_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Verification);
        }
        for finding in verification_questions {
            render_pr_model_verification(&mut text, &finding.reason);
        }
    } else if !verification_observations.is_empty() {
        text.push_str("\n## Verification questions\n\n");
        for observation in &verification_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Verification);
        }
    }

    if !refuted_observations.is_empty() {
        text.push_str("\n## Refuted\n\n");
        for observation in &refuted_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
    }

    if !proof_result_receipts.is_empty() {
        if proof_result_receipts
            .iter()
            .any(|receipt| receipt.kind == "focused-build")
        {
            text.push_str("\n## Proof results\n\n");
        } else {
            text.push_str("\n## Test proof\n\n");
        }
        for receipt in proof_result_receipts {
            render_proof_receipt_summary(&mut text, receipt);
        }
    }

    if !parked.is_empty() {
        text.push_str("\n## Parked follow-ups\n\n");
        for observation in &parked_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
        for finding in parked {
            render_pr_model_signal(&mut text, &finding.reason);
        }
    } else if !parked_observations.is_empty() {
        text.push_str("\n## Parked follow-ups\n\n");
        for observation in &parked_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
    }

    if has_specific_missing_evidence {
        text.push_str("\n## Evidence gaps\n\n");
        for observation in &missing_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
        for receipt in proof_receipts
            .iter()
            .filter(|receipt| proof_receipt_is_missing_evidence(receipt))
        {
            render_missing_proof_receipt_summary(&mut text, receipt);
        }
    }

    cap_review_body(text, review_body_max_bytes)
}
