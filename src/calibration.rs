//! Calibration artifact (review/calibration.json) — run-level measurable signal.
//!
//! Computes a summary of the review-team loop from existing artifacts (not
//! model prose): cohort identity, lane execution, reporter questions,
//! model-selected proof, proof execution, proof routing, lane conclusion
//! changes, final review counts, and gate outcome.
//!
//! The two headline metrics:
//! - `lane_conclusions_changed_by_proof` — did proof change a lane conclusion?
//! - `inline_comments_posted` / `summary_comments_posted` — did the review
//!   say anything?
//!
//! If both stay near zero, ub-review is mostly an expensive review bot. If
//! they are consistently positive, ub-review is doing something distinctive.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::artifacts::CALIBRATION_SCHEMA;

/// Read review/messages.ndjson into a Vec<CrossLaneMessage>.
pub(crate) fn read_messages_ndjson(review_dir: &Path) -> Vec<crate::CrossLaneMessage> {
    let path = review_dir.join("messages.ndjson");
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    text.lines()
        .filter(|l| !l.is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The complete run-level calibration artifact.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CalibrationArtifact {
    pub(crate) schema: String,
    pub(crate) repo: String,
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) run_mode: String,
    pub(crate) gate_policy: String,
    pub(crate) cohort: CalibrationCohort,
    pub(crate) counts: CalibrationCounts,
    pub(crate) classification: CalibrationClassification,
    #[serde(default)]
    pub(crate) notable_events: Vec<CalibrationNotableEvent>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CalibrationCohort {
    pub(crate) provider: String,
    pub(crate) model: String,
    pub(crate) cohort_id: String,
    pub(crate) shared_prefix_hash: String,
    pub(crate) cohort_broken: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CalibrationCounts {
    pub(crate) lanes_planned: usize,
    pub(crate) lanes_executed: usize,
    pub(crate) lane_continuations: usize,
    pub(crate) reporter_questions: usize,
    pub(crate) messages: usize,
    pub(crate) proof_requests_model_selected: usize,
    pub(crate) proof_requests_executed: usize,
    pub(crate) proof_receipts_routed: usize,
    pub(crate) lane_conclusions_changed_by_proof: usize,
    pub(crate) inline_comments_posted: usize,
    pub(crate) summary_comments_posted: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct CalibrationClassification {
    pub(crate) run_class: String,
    pub(crate) suggested_class: String,
    pub(crate) infra_excluded: bool,
    /// Human classification is null initially; filled by the maintainer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) human_classification: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CalibrationNotableEvent {
    pub(crate) kind: String,
    pub(crate) proof_receipt: String,
    pub(crate) lane: String,
    pub(crate) before: String,
    pub(crate) after: String,
    pub(crate) reason: String,
}

/// Build and write `review/calibration.json` from the run's existing artifacts.
///
/// Inputs: model lane receipts, proof requests/receipts, messages.ndjson,
/// lane thread artifacts (turn-000 + turn-001), reporter thread, gate outcome,
/// and the review payload.
#[expect(
    clippy::too_many_arguments,
    reason = "calibration aggregates run-level inputs; tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn write_calibration_artifact(
    review_dir: &Path,
    model_lanes: &[crate::ModelLaneReceipt],
    proof_requests: &[crate::ProofRequest],
    proof_receipts: &[crate::ProofReceipt],
    messages: &[crate::CrossLaneMessage],
    inline_comment_count: usize,
    summary_comment_count: usize,
    gate_conclusion: &str,
    run_mode: &str,
    base: &str,
    head: &str,
) -> Result<()> {
    let artifact = build_calibration(
        review_dir,
        model_lanes,
        proof_requests,
        proof_receipts,
        messages,
        inline_comment_count,
        summary_comment_count,
        gate_conclusion,
        run_mode,
        base,
        head,
    );
    let path = review_dir.join("calibration.json");
    std::fs::write(&path, serde_json::to_vec_pretty(&artifact)?)?;
    Ok(())
}

#[expect(
    clippy::too_many_arguments,
    reason = "mirrors write_calibration_artifact inputs; tracked in policy/allow.toml"
)]
pub(crate) fn build_calibration(
    review_dir: &Path,
    model_lanes: &[crate::ModelLaneReceipt],
    proof_requests: &[crate::ProofRequest],
    proof_receipts: &[crate::ProofReceipt],
    messages: &[crate::CrossLaneMessage],
    inline_comment_count: usize,
    summary_comment_count: usize,
    gate_conclusion: &str,
    run_mode: &str,
    base: &str,
    head: &str,
) -> CalibrationArtifact {
    // Cohort: take from the first executed lane with cohort_id.
    let cohort_lane = model_lanes
        .iter()
        .find(|l| !l.cohort_id.is_empty())
        .cloned();
    let (provider, model, cohort_id, shared_prefix_hash, cohort_broken) = match cohort_lane {
        Some(l) => (
            l.provider.clone(),
            l.model.clone(),
            l.cohort_id.clone(),
            l.shared_prefix_hash.clone(),
            l.cohort_broken,
        ),
        None => Default::default(),
    };
    let cohort = CalibrationCohort {
        provider,
        model,
        cohort_id,
        shared_prefix_hash,
        cohort_broken,
    };

    // Counts.
    let lanes_planned = model_lanes.len();
    let lanes_executed = model_lanes
        .iter()
        .filter(|l| !l.thread_id.is_empty())
        .count();
    let lane_continuations = count_lane_continuations(review_dir);
    let reporter_questions = messages
        .iter()
        .filter(|m| m.kind == crate::CrossLaneMessageKind::ReporterQuestion)
        .count();
    let proof_requests_model_selected = proof_requests
        .iter()
        .filter(|r| {
            r.status == "requested"
                && r.requested_by
                    .iter()
                    .any(|rb| !rb.starts_with("proof-policy:") && rb != "intelligent-ci-policy")
        })
        .count();
    let proof_requests_executed = proof_receipts.len();
    let proof_receipts_routed = messages
        .iter()
        .filter(|m| m.kind == crate::CrossLaneMessageKind::LaneAnswer)
        .count();
    let (lane_conclusions_changed_by_proof, notable_events) =
        detect_proof_changed_conclusions(review_dir, proof_receipts);
    let infra_excluded = gate_conclusion == "inconclusive";

    // Suggested class.
    let suggested_class = if lane_conclusions_changed_by_proof > 0 {
        "proof-changed-conclusion".to_owned()
    } else if lanes_executed == 0 {
        "no-model-review".to_owned()
    } else if infra_excluded {
        "infra-excluded".to_owned()
    } else if inline_comment_count == 0 && summary_comment_count == 0 {
        "expected-quiet".to_owned()
    } else {
        "needs-human-classification".to_owned()
    };

    CalibrationArtifact {
        schema: CALIBRATION_SCHEMA.to_owned(),
        repo: String::new(),
        base: base.to_owned(),
        head: head.to_owned(),
        run_mode: run_mode.to_owned(),
        gate_policy: gate_conclusion.to_owned(),
        cohort,
        counts: CalibrationCounts {
            lanes_planned,
            lanes_executed,
            lane_continuations,
            reporter_questions,
            messages: messages.len(),
            proof_requests_model_selected,
            proof_requests_executed,
            proof_receipts_routed,
            lane_conclusions_changed_by_proof,
            inline_comments_posted: inline_comment_count,
            summary_comments_posted: summary_comment_count,
        },
        classification: CalibrationClassification {
            run_class: "needs-human-classification".to_owned(),
            suggested_class,
            infra_excluded,
            human_classification: None,
        },
        notable_events,
    }
}

/// Count lane continuation turns (turn-001+) across all lane threads.
pub(crate) fn count_lane_continuations(review_dir: &Path) -> usize {
    let threads_dir = review_dir.join("threads");
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(&threads_dir) {
        for entry in entries.flatten() {
            let lane_dir = entry.path();
            #[expect(
                clippy::collapsible_if,
                reason = "two fs::read_dir calls with different error handling; collapsing hurts readability"
            )]
            if lane_dir.is_dir() {
                if let Ok(lane_entries) = std::fs::read_dir(&lane_dir) {
                    for lane_entry in lane_entries.flatten() {
                        let name = lane_entry.file_name();
                        let name_str = name.to_string_lossy();
                        if name_str.starts_with("turn-") && name_str != "turn-000.json" {
                            count += 1;
                        }
                    }
                }
            }
        }
    }
    count
}

/// Detect lanes whose turn-001 conclusion differs from turn-000, where the
/// turn-001 was a follow-up that received routed proof evidence.
pub(crate) fn detect_proof_changed_conclusions(
    review_dir: &Path,
    proof_receipts: &[crate::ProofReceipt],
) -> (usize, Vec<CalibrationNotableEvent>) {
    let threads_dir = review_dir.join("threads");
    let mut events = Vec::new();
    if let Ok(lane_entries) = std::fs::read_dir(&threads_dir) {
        for lane_entry in lane_entries.flatten() {
            let lane_name = lane_entry.file_name();
            let lane_str = lane_name.to_string_lossy();
            if lane_str == "reporter" {
                continue;
            }
            let lane_dir = lane_entry.path();
            if !lane_dir.is_dir() {
                continue;
            }
            // Read turn-000 and turn-001.
            let turn0 = read_turn_summary(&lane_dir, "turn-000.json");
            let turn1 = read_turn_summary(&lane_dir, "turn-001.json");
            if let (Some(t0), Some(t1)) = (turn0, turn1)
                && let Some(t1_full) = read_turn_file(&lane_dir, "turn-001.json")
            {
                // Check if turn-001 has routed evidence refs (proof).
                let has_proof_ref = t1_full
                    .routed_evidence_refs
                    .iter()
                    .any(|r| r.contains("proof") || r.contains("evidence"));
                // Check if the conclusion text changed.
                if t0 != t1 && has_proof_ref {
                    // Find a matching proof receipt for this lane.
                    let receipt_id = proof_receipts
                        .iter()
                        .find(|r| r.requested_by.iter().any(|rb| rb == &lane_str.to_string()))
                        .map(|r| r.id.clone())
                        .unwrap_or_default();
                    events.push(CalibrationNotableEvent {
                        kind: "proof_changed_conclusion".to_owned(),
                        proof_receipt: receipt_id,
                        lane: lane_str.to_string(),
                        before: t0,
                        after: t1,
                        reason: "lane turn-001 conclusion differs from turn-000 with routed proof evidence".to_owned(),
                    });
                }
            }
        }
    }
    (events.len(), events)
}

pub(crate) fn read_turn_summary(lane_dir: &Path, filename: &str) -> Option<String> {
    let turn = read_turn_file(lane_dir, filename)?;
    Some(turn.response_summary)
}

pub(crate) fn read_turn_file(lane_dir: &Path, filename: &str) -> Option<crate::LaneThreadTurn> {
    let path = lane_dir.join(filename);
    let bytes = std::fs::read(&path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_round_trips_schema() -> Result<()> {
        let artifact = CalibrationArtifact {
            schema: CALIBRATION_SCHEMA.to_owned(),
            repo: "test".to_owned(),
            base: "abc".to_owned(),
            head: "def".to_owned(),
            run_mode: "advisory".to_owned(),
            gate_policy: "pass".to_owned(),
            cohort: CalibrationCohort {
                provider: "minimax".to_owned(),
                model: "MiniMax-M3".to_owned(),
                cohort_id: "cohort:minimax:MiniMax-M3:abc123".to_owned(),
                shared_prefix_hash: "abc123".to_owned(),
                cohort_broken: false,
            },
            counts: CalibrationCounts {
                lanes_planned: 5,
                lanes_executed: 5,
                lane_continuations: 2,
                reporter_questions: 3,
                messages: 15,
                proof_requests_model_selected: 1,
                proof_requests_executed: 1,
                proof_receipts_routed: 1,
                lane_conclusions_changed_by_proof: 1,
                inline_comments_posted: 1,
                summary_comments_posted: 1,
            },
            classification: CalibrationClassification {
                run_class: "needs-human-classification".to_owned(),
                suggested_class: "proof-changed-conclusion".to_owned(),
                infra_excluded: false,
                human_classification: None,
            },
            notable_events: vec![CalibrationNotableEvent {
                kind: "proof_changed_conclusion".to_owned(),
                proof_receipt: "proof-red-green-abc".to_owned(),
                lane: "tests-red-green".to_owned(),
                before: "changes requested".to_owned(),
                after: "withdrawn".to_owned(),
                reason: "non_discriminating proof resolved the concern".to_owned(),
            }],
        };
        let json = serde_json::to_string_pretty(&artifact)?;
        assert!(json.contains("ub-review.calibration.v0"));
        assert!(json.contains("proof_changed_conclusion"));
        assert!(json.contains("lane_conclusions_changed_by_proof"));
        let back: CalibrationArtifact = serde_json::from_str(&json)?;
        assert_eq!(back.counts.lane_conclusions_changed_by_proof, 1);
        assert_eq!(back.suggested_class(), "proof-changed-conclusion");
        Ok(())
    }

    #[test]
    fn calibration_suggested_class_expected_quiet() {
        let counts = CalibrationCounts::default();
        let class = suggest_class(0, false, 5, &counts);
        assert_eq!(class, "expected-quiet");
    }

    #[test]
    fn calibration_suggested_class_no_model() {
        let counts = CalibrationCounts::default();
        let class = suggest_class(0, false, 0, &counts);
        assert_eq!(class, "no-model-review");
    }

    #[test]
    fn calibration_suggested_class_infra_excluded() {
        let class = suggest_class(0, true, 5, &CalibrationCounts::default());
        assert_eq!(class, "infra-excluded");
    }

    fn suggest_class(
        proof_changed: usize,
        infra: bool,
        lanes_exec: usize,
        _counts: &CalibrationCounts,
    ) -> String {
        if proof_changed > 0 {
            "proof-changed-conclusion".to_owned()
        } else if lanes_exec == 0 {
            "no-model-review".to_owned()
        } else if infra {
            "infra-excluded".to_owned()
        } else {
            "expected-quiet".to_owned()
        }
    }

    impl CalibrationArtifact {
        fn suggested_class(&self) -> &str {
            &self.classification.suggested_class
        }
    }

    #[test]
    fn write_calibration_creates_artifact() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        std::fs::create_dir_all(&review_dir)?;
        // Empty inputs — no model lanes, no proof, no messages.
        write_calibration_artifact(
            &review_dir,
            &[],
            &[],
            &[],
            &[],
            0,
            0,
            "pass",
            "advisory",
            "abc",
            "def",
        )?;
        let path = review_dir.join("calibration.json");
        assert!(path.exists());
        let bytes = std::fs::read(&path)?;
        let artifact: CalibrationArtifact = serde_json::from_slice(&bytes)?;
        assert_eq!(artifact.schema, "ub-review.calibration.v0");
        assert_eq!(artifact.counts.lanes_planned, 0);
        assert_eq!(artifact.classification.suggested_class, "no-model-review");
        Ok(())
    }

    #[test]
    fn write_calibration_counts_model_selected_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        std::fs::create_dir_all(&review_dir)?;
        let receipt = crate::ModelLaneReceipt {
            lane: "tests-red-green".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "completed".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: crate::ModelCacheUsage::default(),
            cohort_id: "cohort:minimax:M3:abc".to_owned(),
            shared_prefix_hash: "abc".to_owned(),
            thread_id: "cohort:minimax:M3:abc:tests-red-green".to_owned(),
            turn: 0,
            cohort_broken: false,
        };
        let proof_req = crate::ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-0000-abc".to_owned(),
            lane: "tests-red-green".to_owned(),
            requested_by: vec!["tests-red-green".to_owned()],
            command: "cargo test --locked --test cli".to_owned(),
            reason: "test".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "requested".to_owned(),
        };
        write_calibration_artifact(
            &review_dir,
            std::slice::from_ref(&receipt),
            std::slice::from_ref(&proof_req),
            &[],
            &[],
            0,
            0,
            "pass",
            "advisory",
            "abc",
            "def",
        )?;
        let bytes = std::fs::read(review_dir.join("calibration.json"))?;
        let artifact: CalibrationArtifact = serde_json::from_slice(&bytes)?;
        assert_eq!(artifact.counts.lanes_executed, 1);
        assert_eq!(artifact.counts.proof_requests_model_selected, 1);
        assert_eq!(artifact.counts.proof_requests_executed, 0);
        Ok(())
    }

    /// Fixture: proof-changed-conclusion — a lane has turn-000 and turn-001
    /// with different conclusions, and the turn-001 has routed proof evidence
    /// refs. The calibration artifact should detect this as a notable event.
    #[test]
    fn calibration_detects_proof_changed_conclusion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let threads = review_dir.join("threads");
        let lane_dir = threads.join("tests-red-green");
        std::fs::create_dir_all(&lane_dir)?;
        // turn-000: original conclusion
        let turn0 = crate::LaneThreadTurn {
            schema: "ub-review.lane_thread.v1".to_owned(),
            thread_id: "tid".to_owned(),
            turn: 0,
            stage: "primary".to_owned(),
            prompt_packet_path: "lanes/tests-red-green.md".to_owned(),
            response_summary: "The test does not cover the error path; changes requested."
                .to_owned(),
            routed_evidence_refs: vec![],
            receipt_ref: "review/threads/tests-red-green/turn-000.json".to_owned(),
        };
        std::fs::write(
            lane_dir.join("turn-000.json"),
            serde_json::to_vec_pretty(&turn0)?,
        )?;
        // turn-001: revised conclusion with proof evidence routed
        let turn1 = crate::LaneThreadTurn {
            schema: "ub-review.lane_thread.v1".to_owned(),
            thread_id: "tid".to_owned(),
            turn: 1,
            stage: "follow-up".to_owned(),
            prompt_packet_path: "review/threads/tests-red-green/continuation-prompt-001.md"
                .to_owned(),
            response_summary:
                "Withdraw changes_requested — proof shows test is non-discriminating.".to_owned(),
            routed_evidence_refs: vec!["proof evidence: receipt result".to_owned()],
            receipt_ref: "review/threads/tests-red-green/turn-001.json".to_owned(),
        };
        std::fs::write(
            lane_dir.join("turn-001.json"),
            serde_json::to_vec_pretty(&turn1)?,
        )?;
        // A proof receipt matching the lane
        let proof_receipt = crate::ProofReceipt {
            schema: "ub-review.proof_receipt.v1".to_owned(),
            id: "proof-red-green-abc".to_owned(),
            kind: "focused-red-green".to_owned(),
            base: "abc".to_owned(),
            head: "def".to_owned(),
            test_patch_mode: "base-plus-tests".to_owned(),
            requested_by: vec!["tests-red-green".to_owned()],
            request_ids: vec!["proof-0000".to_owned()],
            commands: vec![],
            result: "non_discriminating".to_owned(),
            reason: "test does not discriminate".to_owned(),
        };
        let lane_receipt = crate::ModelLaneReceipt {
            lane: "tests-red-green".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "completed".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: crate::ModelCacheUsage::default(),
            cohort_id: "cohort:minimax:M3:abc".to_owned(),
            shared_prefix_hash: "abc".to_owned(),
            thread_id: "cohort:minimax:M3:abc:tests-red-green".to_owned(),
            turn: 0,
            cohort_broken: false,
        };
        write_calibration_artifact(
            &review_dir,
            std::slice::from_ref(&lane_receipt),
            &[],
            std::slice::from_ref(&proof_receipt),
            &[],
            0,
            0,
            "pass",
            "advisory",
            "abc",
            "def",
        )?;
        let bytes = std::fs::read(review_dir.join("calibration.json"))?;
        let artifact: CalibrationArtifact = serde_json::from_slice(&bytes)?;
        assert_eq!(
            artifact.counts.lane_conclusions_changed_by_proof, 1,
            "should detect 1 proof-changed conclusion"
        );
        assert_eq!(artifact.counts.lane_continuations, 1);
        assert_eq!(
            artifact.classification.suggested_class,
            "proof-changed-conclusion"
        );
        assert!(!artifact.notable_events.is_empty());
        assert_eq!(artifact.notable_events[0].lane, "tests-red-green");
        assert_eq!(
            artifact.notable_events[0].proof_receipt,
            "proof-red-green-abc"
        );
        Ok(())
    }

    /// Fixture: quiet-green/no-proof — pass gate, no proof, no comments.
    #[test]
    fn calibration_quiet_green_no_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        std::fs::create_dir_all(&review_dir)?;
        let lane = crate::ModelLaneReceipt {
            lane: "correctness".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "no findings".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: crate::ModelCacheUsage::default(),
            cohort_id: "cohort:minimax:M3:abc".to_owned(),
            shared_prefix_hash: "abc".to_owned(),
            thread_id: "cohort:minimax:M3:abc:correctness".to_owned(),
            turn: 0,
            cohort_broken: false,
        };
        write_calibration_artifact(
            &review_dir,
            std::slice::from_ref(&lane),
            &[],
            &[],
            &[],
            0,
            0,
            "pass",
            "advisory",
            "abc",
            "def",
        )?;
        let artifact: CalibrationArtifact =
            serde_json::from_slice(&std::fs::read(review_dir.join("calibration.json"))?)?;
        assert_eq!(artifact.gate_policy, "pass");
        assert_eq!(artifact.counts.proof_requests_model_selected, 0);
        assert_eq!(artifact.counts.lane_conclusions_changed_by_proof, 0);
        assert_eq!(artifact.counts.inline_comments_posted, 0);
        assert_eq!(artifact.classification.suggested_class, "expected-quiet");
        Ok(())
    }

    /// Fixture: reporter-skipped — model_mode=off, no lanes executed.
    #[test]
    fn calibration_reporter_skipped() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        std::fs::create_dir_all(&review_dir)?;
        // A lane with no thread_id (preflight only, model_mode=off)
        let lane = crate::ModelLaneReceipt {
            lane: "correctness".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "skipped_model_mode_off".to_owned(),
            reason: "model mode off".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: crate::ModelCacheUsage::default(),
            cohort_id: String::new(),
            shared_prefix_hash: String::new(),
            thread_id: String::new(),
            turn: 0,
            cohort_broken: false,
        };
        write_calibration_artifact(
            &review_dir,
            std::slice::from_ref(&lane),
            &[],
            &[],
            &[],
            0,
            0,
            "pass",
            "review-byok",
            "abc",
            "def",
        )?;
        let artifact: CalibrationArtifact =
            serde_json::from_slice(&std::fs::read(review_dir.join("calibration.json"))?)?;
        assert_eq!(artifact.counts.lanes_executed, 0);
        assert_eq!(artifact.counts.reporter_questions, 0);
        assert_eq!(artifact.counts.lane_continuations, 0);
        assert_eq!(artifact.cohort.provider, ""); // no cohort identity
        assert_eq!(artifact.classification.suggested_class, "no-model-review");
        Ok(())
    }

    /// Fixture: missing-proof — proof requested but no receipt produced.
    #[test]
    fn calibration_missing_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        std::fs::create_dir_all(&review_dir)?;
        let lane = crate::ModelLaneReceipt {
            lane: "tests-red-green".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "completed".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: crate::ModelCacheUsage::default(),
            cohort_id: "cohort:minimax:M3:abc".to_owned(),
            shared_prefix_hash: "abc".to_owned(),
            thread_id: "cohort:minimax:M3:abc:tests-red-green".to_owned(),
            turn: 0,
            cohort_broken: false,
        };
        // Proof requested but status=unsupported (broker rejected command)
        let proof_req = crate::ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-0000-abc".to_owned(),
            lane: "tests-red-green".to_owned(),
            requested_by: vec!["tests-red-green".to_owned()],
            command: "cargo test -p foo".to_owned(),
            reason: "test".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "unsupported".to_owned(),
        };
        write_calibration_artifact(
            &review_dir,
            std::slice::from_ref(&lane),
            std::slice::from_ref(&proof_req),
            &[], // no receipts
            &[],
            0,
            0,
            "pass",
            "advisory",
            "abc",
            "def",
        )?;
        let artifact: CalibrationArtifact =
            serde_json::from_slice(&std::fs::read(review_dir.join("calibration.json"))?)?;
        // Model-selected proof requested but unsupported → 0 model_selected
        // (status != "requested")
        assert_eq!(artifact.counts.proof_requests_model_selected, 0);
        assert_eq!(artifact.counts.proof_requests_executed, 0);
        assert_eq!(artifact.counts.lane_conclusions_changed_by_proof, 0);
        Ok(())
    }
}
