//! Live reporter coordination (Order 9 of #678) — the product center.
//!
//! The same-model coordinator that runs after the primary investigation wave,
//! reads what each lane concluded, and makes one same-model distillation call
//! (same cohort provider/model, same cached shared prefix) to identify the
//! most important findings, flag contradictions/gaps, and propose a verdict
//! direction.
//!
//! This is single-turn (turn 0) in this slice: it proves the same-model
//! distillation loop end-to-end. Multi-turn continuation (reporter asks → lane
//! answers → reporter refines) is the natural extension built on the
//! persistent threads (#692) and message queue (#694).
//!
//! The reporter's output is advisory: it feeds the compiler and the message
//! queue; it does not itself post or gate (Orders 10/11).

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::artifacts::REPORTER_THREAD_SCHEMA;

/// A lane's conclusion as the reporter sees it — a compact digest built from
/// the lane's ModelLaneReceipt.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct LaneDigest {
    pub(crate) lane: String,
    pub(crate) status: String,
    pub(crate) conclusion: String,
    pub(crate) thread_id: String,
}

/// The reporter's verdict on the PR (Order 11 of #678). Only meaningful when
/// `[gate].review_forward = true`; otherwise it is advisory and never feeds
/// the gate.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReporterVerdict {
    /// The reporter finds the PR safe to merge.
    Clear,
    /// The reporter requests changes before merge.
    ChangesRequested,
    /// The reporter cannot determine whether the PR is safe (insufficient
    /// evidence, conflicting lanes, etc.).
    Uncertain,
    /// No verdict was produced (model mode off, reporter skipped, or the model
    /// did not return a verdict).
    #[default]
    None,
}

/// The reporter's distilled conclusion, parsed from its model response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ReporterConclusion {
    pub(crate) schema: String,
    /// The reporter's free-form distillation text (what it would say to a
    /// human reviewer about the PR).
    pub(crate) distillation: String,
    /// Follow-up questions the reporter proposes (emitted as
    /// reporter_question messages). Empty if none.
    pub(crate) proposed_follow_ups: Vec<String>,
    pub(crate) cohort_id: String,
    pub(crate) thread_id: String,
    /// The reporter's structured verdict (Order 11). Only affects the gate
    /// when `[gate].review_forward = true`.
    #[serde(default)]
    pub(crate) verdict: ReporterVerdict,
}

/// Build the reporter's prompt: the shared cached prefix is provided
/// separately (as the cacheable prefix to the model call); this returns the
/// reporter *suffix* — the compact digest of what each lane concluded plus the
/// reporter's task instruction.
pub(crate) fn reporter_prompt(digests: &[LaneDigest]) -> String {
    let mut prompt = String::new();
    prompt.push_str("# Reporter Coordination Task\n\n");
    prompt.push_str(
        "You are the same-model reporter coordinating this review. Below are the \
         conclusions of each specialist investigation lane. Your job:\n\n",
    );
    prompt.push_str("1. Identify the most important findings worth surfacing.\n");
    prompt.push_str("2. Flag contradictions or gaps between lanes.\n");
    prompt.push_str("3. Propose a verdict direction (clear / changes_requested / uncertain).\n");
    prompt.push_str("4. List any targeted follow-up questions for named lanes.\n\n");
    prompt.push_str("## Lane conclusions\n\n");
    if digests.is_empty() {
        prompt.push_str("- No lanes reported (model mode off or all skipped).\n");
    }
    for d in digests {
        let conclusion = if d.conclusion.is_empty() {
            "(no detail)"
        } else {
            d.conclusion.as_str()
        };
        prompt.push_str(&format!(
            "### `{}` (status: `{}`)\n{}\n\n",
            d.lane, d.status, conclusion
        ));
    }
    prompt.push_str(
        "## Output\n\nReturn a JSON object: {\"distillation\": \"...\", \
         \"verdict\": \"clear\"|\"changes_requested\"|\"uncertain\", \
         \"proposed_follow_ups\": [\"question for lane X\", ...]}. The distillation \
         is what you would tell a human reviewer in 2-4 sentences.\n",
    );
    prompt
}

/// Build a lane digest from the executed receipts (only lanes with a
/// non-empty thread_id — i.e., that actually investigated).
pub(crate) fn lane_digests_from_receipts(receipts: &[crate::ModelLaneReceipt]) -> Vec<LaneDigest> {
    receipts
        .iter()
        .filter(|r| !r.thread_id.is_empty())
        .map(|r| LaneDigest {
            lane: r.lane.clone(),
            status: r.status.clone(),
            conclusion: r.reason.clone(),
            thread_id: r.thread_id.clone(),
        })
        .collect()
}

/// Parse the reporter's model response into a ReporterConclusion. Tolerant of
/// non-JSON responses (uses the raw text as the distillation).
pub(crate) fn parse_reporter_conclusion(
    content: &str,
    cohort_id: &str,
    thread_id: &str,
) -> ReporterConclusion {
    // Try to parse as JSON {distillation, proposed_follow_ups}.
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
        let distillation = parsed
            .get("distillation")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let proposed_follow_ups = parsed
            .get("proposed_follow_ups")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        return ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation,
            proposed_follow_ups,
            verdict: parse_verdict(&parsed),
            cohort_id: cohort_id.to_owned(),
            thread_id: thread_id.to_owned(),
        };
    }
    // Fallback: the raw response text is the distillation.
    ReporterConclusion {
        schema: REPORTER_THREAD_SCHEMA.to_owned(),
        distillation: content.to_owned(),
        proposed_follow_ups: Vec::new(),
        verdict: ReporterVerdict::None,
        cohort_id: cohort_id.to_owned(),
        thread_id: thread_id.to_owned(),
    }
}

/// Parse the reporter's verdict from a JSON value. Recognizes the
/// snake_case strings from the prompt: "clear", "changes_requested",
/// "uncertain". Falls back to None for missing or unrecognized values.
fn parse_verdict(value: &serde_json::Value) -> ReporterVerdict {
    match value
        .get("verdict")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "clear" => ReporterVerdict::Clear,
        "changes_requested" => ReporterVerdict::ChangesRequested,
        "uncertain" => ReporterVerdict::Uncertain,
        _ => ReporterVerdict::None,
    }
}

/// Write the reporter's conclusion as a thread artifact
/// (review/threads/reporter/turn-000.json + thread.json), reusing the
/// Order 7 lane-thread writer for consistency.
pub(crate) fn write_reporter_thread(
    review_dir: &Path,
    conclusion: &ReporterConclusion,
) -> Result<()> {
    let turn = crate::LaneThreadTurn {
        schema: REPORTER_THREAD_SCHEMA.to_owned(),
        thread_id: conclusion.thread_id.clone(),
        turn: 0,
        stage: "reporter".to_owned(),
        prompt_packet_path: "review/threads/reporter/prompt.md".to_owned(),
        response_summary: conclusion.distillation.clone(),
        routed_evidence_refs: conclusion
            .proposed_follow_ups
            .iter()
            .map(|q| format!("follow-up: {q}"))
            .collect(),
        receipt_ref: "review/threads/reporter/turn-000.json".to_owned(),
    };
    crate::write_lane_thread_turn(
        review_dir,
        "reporter",
        &turn,
        &conclusion.cohort_id,
        "reporter_completed",
    )?;
    Ok(())
}

/// Read the reporter's distillation from review/threads/reporter/turn-000.json.
/// Returns None if the reporter didn't run or the artifact is absent.
pub(crate) fn read_reporter_distillation(review_dir: &Path) -> Option<String> {
    let turn_path = review_dir.join("threads/reporter/turn-000.json");
    let bytes = std::fs::read(&turn_path).ok()?;
    let turn: crate::LaneThreadTurn = serde_json::from_slice(&bytes).ok()?;
    if turn.response_summary.is_empty() {
        None
    } else {
        Some(turn.response_summary)
    }
}

/// Read the reporter's verdict from the distillation text. The verdict is
/// embedded in the response_summary (which is the parsed JSON distillation
/// written by write_reporter_thread). Returns None if no reporter artifact
/// exists; returns Some(None) if the reporter ran but produced no verdict.
pub(crate) fn read_reporter_verdict(review_dir: &Path) -> Option<ReporterVerdict> {
    let distillation = read_reporter_distillation(review_dir)?;
    // The response_summary may be the raw distillation text or the JSON. Try
    // parsing as JSON first (the reporter's structured output).
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&distillation) {
        return Some(parse_verdict(&parsed));
    }
    // If it's not JSON, the reporter didn't produce a structured verdict.
    Some(ReporterVerdict::None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reporter_prompt_lists_lanes_and_includes_task() {
        let digests = vec![
            LaneDigest {
                lane: "tests-oracle".to_owned(),
                status: "ok".to_owned(),
                conclusion: "test discriminates the patch".to_owned(),
                thread_id: "tid1".to_owned(),
            },
            LaneDigest {
                lane: "opposition".to_owned(),
                status: "degraded".to_owned(),
                conclusion: "strongest objection: missing error path".to_owned(),
                thread_id: "tid2".to_owned(),
            },
        ];
        let prompt = reporter_prompt(&digests);
        assert!(prompt.contains("Reporter Coordination Task"));
        assert!(prompt.contains("tests-oracle"));
        assert!(prompt.contains("discriminates"));
        assert!(prompt.contains("opposition"));
        assert!(prompt.contains("missing error path"));
        assert!(prompt.contains("proposed_follow_ups"));
    }

    #[test]
    fn reporter_prompt_handles_no_lanes() {
        let prompt = reporter_prompt(&[]);
        assert!(prompt.contains("No lanes reported"));
    }

    #[test]
    fn parse_reporter_conclusion_json() {
        let content = r#"{"distillation": "PR is safe; one minor test gap.", "proposed_follow_ups": ["tests-oracle: confirm edge case"]}"#;
        let c = parse_reporter_conclusion(content, "cid", "tid");
        assert_eq!(c.distillation, "PR is safe; one minor test gap.");
        assert_eq!(
            c.proposed_follow_ups,
            vec!["tests-oracle: confirm edge case"]
        );
        assert_eq!(c.cohort_id, "cid");
    }

    #[test]
    fn parse_reporter_conclusion_fallback_for_non_json() {
        let content = "This is just prose, not JSON.";
        let c = parse_reporter_conclusion(content, "cid", "tid");
        assert_eq!(c.distillation, content);
        assert!(c.proposed_follow_ups.is_empty());
    }

    #[test]
    fn lane_digests_skip_unexecuted_lanes() {
        let mut receipt = crate::ModelLaneReceipt {
            lane: "x".to_owned(),
            provider: "minimax".to_owned(),
            model: "M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "done".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: crate::ModelCacheUsage::default(),
            cohort_id: "cid".to_owned(),
            shared_prefix_hash: "h".to_owned(),
            thread_id: "tid".to_owned(),
            turn: 0,
            cohort_broken: false,
        };
        let digests = lane_digests_from_receipts(std::slice::from_ref(&receipt));
        assert_eq!(digests.len(), 1);
        // A preflight-only receipt (empty thread_id) is skipped.
        receipt.thread_id = String::new();
        let digests = lane_digests_from_receipts(std::slice::from_ref(&receipt));
        assert!(digests.is_empty());
    }

    #[test]
    fn write_reporter_thread_creates_artifact() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "PR is safe to merge.".to_owned(),
            proposed_follow_ups: vec!["tests-oracle: edge case?".to_owned()],
            verdict: ReporterVerdict::Clear,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion)?;
        let turn_path = review_dir.join("threads/reporter/turn-000.json");
        assert!(turn_path.exists());
        let thread_path = review_dir.join("threads/reporter/thread.json");
        assert!(thread_path.exists());
        let session: crate::LaneThreadSession =
            serde_json::from_slice(&std::fs::read(&thread_path)?)?;
        assert_eq!(session.lane, "reporter");
        assert!(session.latest_conclusion.contains("safe to merge"));
        Ok(())
    }

    #[test]
    fn read_reporter_distillation_returns_none_when_absent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        assert!(read_reporter_distillation(&review_dir).is_none());
        Ok(())
    }

    #[test]
    fn read_reporter_distillation_reads_conclusion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "PR is safe to merge; tests cover the change.".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::None,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion)?;
        let distillation = read_reporter_distillation(&review_dir);
        let distillation = distillation
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("reporter distillation missing"))?;
        assert!(distillation.contains("safe to merge"));
        Ok(())
    }

    #[test]
    fn lane_continuation_prompt_includes_question_and_prior_conclusion() {
        let prompt = crate::lane_continuation_prompt(
            "tests-oracle",
            "specialist reviewer",
            "The test does not discriminate the patch.",
            "Does the test fail against base source plus the new fixture?",
            "PR looks safe; one test-gap concern from tests-oracle.",
            &[],
        );
        assert!(prompt.contains("tests-oracle"));
        assert!(prompt.contains("does not discriminate"));
        assert!(prompt.contains("Does the test fail"));
        assert!(prompt.contains("reporter"));
        assert!(prompt.contains("Revise, confirm, or withdraw"));
        assert!(prompt.contains("\"changed\""));
        // No proof evidence section when excerpts are empty.
        assert!(!prompt.contains("Routed proof evidence"));
    }

    #[test]
    fn lane_continuation_prompt_includes_proof_evidence_when_present() {
        let prompt = crate::lane_continuation_prompt(
            "tests-oracle",
            "specialist reviewer",
            "The test may not discriminate the patch.",
            "Does the test fail against base source?",
            "PR has a test-gap concern.",
            &[
                "proof `proof-001` result=`non_discriminating` reason=`base+tests passed the same`"
                    .to_owned(),
            ],
        );
        assert!(prompt.contains("Routed proof evidence"));
        assert!(prompt.contains("non_discriminating"));
        assert!(prompt.contains("Revise"));
    }

    #[test]
    fn resolve_lane_target_strips_question_for_prefix() {
        let lanes = ["tests-oracle", "workflow-proof", "opposition"];
        assert_eq!(
            crate::resolve_lane_target("Question for tests-oracle", &lanes),
            Some("tests-oracle".to_owned())
        );
        assert_eq!(
            crate::resolve_lane_target("Question for workflow-proof", &lanes),
            Some("workflow-proof".to_owned())
        );
    }

    #[test]
    fn resolve_lane_target_exact_and_suffix() {
        let lanes = ["tests-oracle", "opposition"];
        assert_eq!(
            crate::resolve_lane_target("opposition", &lanes),
            Some("opposition".to_owned())
        );
        assert_eq!(
            crate::resolve_lane_target("tests-oracle lane", &lanes),
            Some("tests-oracle".to_owned())
        );
    }

    #[test]
    fn resolve_lane_target_returns_none_for_unknown() {
        let lanes = ["tests-oracle"];
        assert_eq!(crate::resolve_lane_target("nonexistent-lane", &lanes), None);
    }
}
