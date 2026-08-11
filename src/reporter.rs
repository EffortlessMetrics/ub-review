// ub-review: reporter module — the same-model coordinator (Order 9 of #678).
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

/// Digest of a late-phase sensor receipt (#325 stream-as-it-lands). Late
/// sensors finish while the lanes are already investigating; their receipts
/// are joined before the reporter runs and routed here so the reporter can
/// weigh the late deterministic evidence and carry it into lane continuation
/// turns.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct LateSensorDigest {
    pub(crate) sensor: String,
    pub(crate) status: String,
    pub(crate) reason: String,
    pub(crate) receipt_path: String,
}

impl LateSensorDigest {
    /// One-line excerpt for routing into a lane continuation prompt.
    pub(crate) fn excerpt(&self) -> String {
        format!(
            "late sensor `{}` status=`{}` reason=`{}` (receipt: {})",
            self.sensor, self.status, self.reason, self.receipt_path
        )
    }
}

/// Read the late-phase sensors' receipts into reporter-routable digests. A
/// receipt that is absent or unreadable is reported as `receipt-absent` —
/// missing evidence, never clean evidence.
pub(crate) fn late_sensor_receipt_digests(
    out: &Path,
    sensor_ids: &[String],
) -> Vec<LateSensorDigest> {
    sensor_ids
        .iter()
        .map(|id| {
            let receipt_path = format!("sensors/{id}/ub-review-sensor-status.json");
            let receipt = crate::read_sensor_receipt(&out.join(&receipt_path));
            let (status, reason) = receipt
                .map(|receipt| (receipt.status, receipt.reason))
                .unwrap_or_else(|| {
                    (
                        "receipt-absent".to_owned(),
                        "late sensor produced no receipt; treat as missing evidence".to_owned(),
                    )
                });
            LateSensorDigest {
                sensor: id.clone(),
                status,
                reason,
                receipt_path,
            }
        })
        .collect()
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
pub(crate) fn reporter_prompt(
    digests: &[LaneDigest],
    late_evidence: &[LateSensorDigest],
) -> String {
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
    // #325 stream-as-it-lands: late-phase deterministic evidence landed after
    // the lanes launched, so the lanes have not seen it. The reporter weighs
    // it here and can route it to lanes via follow-up questions.
    if !late_evidence.is_empty() {
        prompt.push_str("## Late deterministic evidence\n\n");
        prompt.push_str(
            "These sensor receipts landed after the lanes launched (the lanes \
             reviewed on the fast-sensor precontext and have not seen them). \
             Weigh them in your distillation; if one contradicts or confirms a \
             lane's conclusion, ask that lane a follow-up question naming the \
             receipt.\n\n",
        );
        for digest in late_evidence {
            prompt.push_str(&format!(
                "- `{}`: `{}` - {} (receipt: `{}`)\n",
                digest.sensor, digest.status, digest.reason, digest.receipt_path
            ));
        }
        prompt.push('\n');
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

/// One validated reporter turn selected as the public/gate authority for the
/// current PR head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedReporterTurn {
    pub(crate) turn: u32,
    pub(crate) receipt_ref: String,
    pub(crate) head_sha: String,
    pub(crate) thread_id: String,
    pub(crate) cohort_id: String,
    pub(crate) distillation: String,
    pub(crate) verdict: ReporterVerdict,
}

/// Result of resolving the reporter authority for a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReporterTurnResolution {
    /// Latest eligible turn is bound to the current head (or is a legacy
    /// unbound turn kept under the compatibility rule).
    Current(ResolvedReporterTurn),
    /// No reporter turn artifact exists.
    Absent,
    /// Latest turn exists but is bound to a different head.
    StaleHead {
        expected: String,
        found: String,
        receipt: String,
    },
    /// Latest turn path exists but could not be parsed.
    Malformed { receipt: String, error: String },
}

/// Gate-facing reporter evidence derived from one resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ReporterGateInput {
    /// Reporter did not run / no turn on disk.
    #[default]
    Absent,
    /// Structured verdict from a current-head turn.
    Verdict {
        verdict: ReporterVerdict,
        receipt: String,
    },
    /// Turn exists but is not usable as a deciding artifact.
    Unusable {
        kind: String,
        detail: String,
        receipt: String,
    },
}

impl ReporterTurnResolution {
    /// Public distillation for the review body. Only a current-head turn may
    /// surface; stale/malformed evidence stays artifact-only.
    pub(crate) fn public_distillation(&self) -> Option<&str> {
        match self {
            Self::Current(turn) if !turn.distillation.is_empty() => {
                Some(turn.distillation.as_str())
            }
            _ => None,
        }
    }

    /// Gate evidence for `[gate].review_forward`. Stale/malformed remain
    /// explicit instead of collapsing into ordinary absence.
    pub(crate) fn gate_input(&self) -> ReporterGateInput {
        match self {
            Self::Absent => ReporterGateInput::Absent,
            Self::Current(turn) => ReporterGateInput::Verdict {
                verdict: turn.verdict.clone(),
                receipt: turn.receipt_ref.clone(),
            },
            Self::StaleHead {
                expected,
                found,
                receipt,
            } => ReporterGateInput::Unusable {
                kind: "reporter-stale-head".to_owned(),
                detail: format!(
                    "latest reporter turn is bound to head `{found}`, not current head `{expected}`"
                ),
                receipt: receipt.clone(),
            },
            Self::Malformed { receipt, error } => ReporterGateInput::Unusable {
                kind: "reporter-malformed".to_owned(),
                detail: format!("latest reporter turn is unreadable: {error}"),
                receipt: receipt.clone(),
            },
        }
    }
}

/// Serialize a structured verdict for the turn artifact.
pub(crate) fn verdict_to_wire(verdict: &ReporterVerdict) -> String {
    match verdict {
        ReporterVerdict::Clear => "clear".to_owned(),
        ReporterVerdict::ChangesRequested => "changes_requested".to_owned(),
        ReporterVerdict::Uncertain => "uncertain".to_owned(),
        ReporterVerdict::None => "none".to_owned(),
    }
}

/// Parse a wire verdict string (snake_case).
fn verdict_from_wire(raw: &str) -> ReporterVerdict {
    match raw.trim().to_ascii_lowercase().as_str() {
        "clear" => ReporterVerdict::Clear,
        "changes_requested" => ReporterVerdict::ChangesRequested,
        "uncertain" => ReporterVerdict::Uncertain,
        "none" | "" => ReporterVerdict::None,
        _ => ReporterVerdict::None,
    }
}

/// Write a reporter turn (turn N) with durable structured verdict + head bind.
pub(crate) fn write_reporter_turn(
    review_dir: &Path,
    conclusion: &ReporterConclusion,
    turn: u32,
    head_sha: &str,
    terminal_reason: &str,
    extra_routed_refs: &[String],
) -> Result<()> {
    let receipt_ref = format!("review/threads/reporter/turn-{turn:03}.json");
    let mut routed_evidence_refs: Vec<String> = conclusion
        .proposed_follow_ups
        .iter()
        .map(|q| format!("follow-up: {q}"))
        .collect();
    routed_evidence_refs.extend(extra_routed_refs.iter().cloned());
    let turn_record = crate::LaneThreadTurn {
        schema: REPORTER_THREAD_SCHEMA.to_owned(),
        thread_id: conclusion.thread_id.clone(),
        turn,
        stage: "reporter".to_owned(),
        prompt_packet_path: "review/threads/reporter/prompt.md".to_owned(),
        response_summary: conclusion.distillation.clone(),
        routed_evidence_refs,
        receipt_ref: receipt_ref.clone(),
        head_sha: Some(head_sha.to_owned()),
        verdict: Some(verdict_to_wire(&conclusion.verdict)),
    };
    crate::write_lane_thread_turn(
        review_dir,
        "reporter",
        &turn_record,
        &conclusion.cohort_id,
        terminal_reason,
    )?;
    Ok(())
}

/// Write the reporter's first-pass conclusion as turn-000.
pub(crate) fn write_reporter_thread(
    review_dir: &Path,
    conclusion: &ReporterConclusion,
    head_sha: &str,
) -> Result<()> {
    write_reporter_turn(
        review_dir,
        conclusion,
        0,
        head_sha,
        "reporter_completed",
        &[],
    )
}

/// Select the highest-numbered reporter turn and bind it to `current_head`.
pub(crate) fn resolve_reporter_turn(
    review_dir: &Path,
    current_head: &str,
) -> ReporterTurnResolution {
    let thread_dir = review_dir.join("threads").join("reporter");
    let Ok(entries) = std::fs::read_dir(&thread_dir) else {
        return ReporterTurnResolution::Absent;
    };
    let mut turn_ids: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| name.starts_with("turn-") && name.ends_with(".json"))
        .map(|name| name.trim_end_matches(".json").to_owned())
        .collect();
    turn_ids.sort();
    let Some(latest_id) = turn_ids.last().cloned() else {
        return ReporterTurnResolution::Absent;
    };
    let receipt_ref = format!("review/threads/reporter/{latest_id}.json");
    let turn_path = thread_dir.join(format!("{latest_id}.json"));
    let bytes = match std::fs::read(&turn_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return ReporterTurnResolution::Malformed {
                receipt: receipt_ref,
                error: err.to_string(),
            };
        }
    };
    let turn: crate::LaneThreadTurn = match serde_json::from_slice(&bytes) {
        Ok(turn) => turn,
        Err(err) => {
            return ReporterTurnResolution::Malformed {
                receipt: receipt_ref,
                error: err.to_string(),
            };
        }
    };
    if let Some(found) = turn.head_sha.as_deref()
        && !found.is_empty()
        && found != current_head
    {
        return ReporterTurnResolution::StaleHead {
            expected: current_head.to_owned(),
            found: found.to_owned(),
            receipt: receipt_ref,
        };
    }
    let verdict = turn
        .verdict
        .as_deref()
        .map(verdict_from_wire)
        .unwrap_or(ReporterVerdict::None);
    let cohort_id = std::fs::read(thread_dir.join("thread.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<crate::LaneThreadSession>(&bytes).ok())
        .map(|session| session.cohort_id)
        .unwrap_or_default();
    ReporterTurnResolution::Current(ResolvedReporterTurn {
        turn: turn.turn,
        receipt_ref,
        head_sha: turn.head_sha.unwrap_or_else(|| current_head.to_owned()),
        thread_id: turn.thread_id,
        cohort_id,
        distillation: turn.response_summary,
        verdict,
    })
}

/// Read the current-head reporter distillation for the review compiler.
/// Stale/malformed turns return None (artifact-only).
pub(crate) fn read_reporter_distillation(review_dir: &Path, current_head: &str) -> Option<String> {
    resolve_reporter_turn(review_dir, current_head)
        .public_distillation()
        .map(str::to_owned)
}

/// Read the current-head reporter verdict for review-forward gating.
///
/// Returns:
/// - `None` when the reporter is absent;
/// - `Some(verdict)` for a current-head turn;
///
/// Prefer [`resolve_reporter_turn`] + [`ReporterTurnResolution::gate_input`] so
/// stale/malformed evidence stays explicit under review-forward.
pub(crate) fn read_reporter_verdict(
    review_dir: &Path,
    current_head: &str,
) -> Option<ReporterVerdict> {
    match resolve_reporter_turn(review_dir, current_head) {
        ReporterTurnResolution::Current(turn) => Some(turn.verdict),
        ReporterTurnResolution::Absent => None,
        ReporterTurnResolution::StaleHead { .. } | ReporterTurnResolution::Malformed { .. } => None,
    }
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
        let prompt = reporter_prompt(&digests, &[]);
        assert!(prompt.contains("Reporter Coordination Task"));
        assert!(prompt.contains("tests-oracle"));
        assert!(prompt.contains("discriminates"));
        assert!(prompt.contains("opposition"));
        assert!(prompt.contains("missing error path"));
        assert!(prompt.contains("proposed_follow_ups"));
        assert!(
            !prompt.contains("Late deterministic evidence"),
            "no late-evidence section without late receipts"
        );
    }

    #[test]
    fn reporter_prompt_handles_no_lanes() {
        let prompt = reporter_prompt(&[], &[]);
        assert!(prompt.contains("No lanes reported"));
    }

    #[test]
    fn reporter_prompt_routes_late_sensor_evidence() {
        let digests = vec![LaneDigest {
            lane: "tests-oracle".to_owned(),
            status: "ok".to_owned(),
            conclusion: "coverage gap suspected".to_owned(),
            thread_id: "tid1".to_owned(),
        }];
        let late = vec![LateSensorDigest {
            sensor: "cargo-test".to_owned(),
            status: "failed".to_owned(),
            reason: "exit code Some(101)".to_owned(),
            receipt_path: "sensors/cargo-test/ub-review-sensor-status.json".to_owned(),
        }];
        let prompt = reporter_prompt(&digests, &late);
        assert!(prompt.contains("Late deterministic evidence"));
        assert!(prompt.contains("`cargo-test`: `failed`"));
        assert!(prompt.contains("sensors/cargo-test/ub-review-sensor-status.json"));
        assert!(prompt.contains("landed after the lanes launched"));
    }

    #[test]
    fn late_sensor_digest_reads_receipt_and_reports_absence_as_missing() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        std::fs::create_dir_all(out.join("sensors/cargo-test"))?;
        std::fs::write(
            out.join("sensors/cargo-test/ub-review-sensor-status.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "sensor": "cargo-test",
                "status": "ok",
                "reason": "completed",
            }))?,
        )?;
        let digests =
            late_sensor_receipt_digests(out, &["cargo-test".to_owned(), "coverage".to_owned()]);
        assert_eq!(digests.len(), 2);
        assert_eq!(digests[0].sensor, "cargo-test");
        assert_eq!(digests[0].status, "ok");
        assert_eq!(digests[1].sensor, "coverage");
        assert_eq!(digests[1].status, "receipt-absent");
        assert!(digests[1].reason.contains("missing evidence"));
        Ok(())
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
        write_reporter_thread(&review_dir, &conclusion, "abc123")?;
        let turn_path = review_dir.join("threads/reporter/turn-000.json");
        assert!(turn_path.exists());
        let turn: crate::LaneThreadTurn = serde_json::from_slice(&std::fs::read(&turn_path)?)?;
        assert_eq!(turn.head_sha.as_deref(), Some("abc123"));
        assert_eq!(turn.verdict.as_deref(), Some("clear"));
        assert_eq!(turn.response_summary, "PR is safe to merge.");
        let thread_path = review_dir.join("threads/reporter/thread.json");
        assert!(thread_path.exists());
        let session: crate::LaneThreadSession =
            serde_json::from_slice(&std::fs::read(&thread_path)?)?;
        assert_eq!(session.lane, "reporter");
        assert!(session.latest_conclusion.contains("safe to merge"));
        assert_eq!(session.verdict.as_deref(), Some("clear"));
        assert_eq!(session.head_sha.as_deref(), Some("abc123"));
        Ok(())
    }

    #[test]
    fn read_reporter_distillation_returns_none_when_absent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        assert!(read_reporter_distillation(&review_dir, "abc").is_none());
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
        write_reporter_thread(&review_dir, &conclusion, "abc")?;
        let distillation = read_reporter_distillation(&review_dir, "abc");
        let distillation = distillation
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("reporter distillation missing"))?;
        assert!(distillation.contains("safe to merge"));
        Ok(())
    }

    #[test]
    fn structured_verdict_survives_write_resolve_round_trip() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let content =
            r#"{"distillation":"The null fallback is incorrect.","verdict":"changes_requested"}"#;
        let conclusion = parse_reporter_conclusion(content, "cid", "tid");
        assert_eq!(conclusion.verdict, ReporterVerdict::ChangesRequested);
        assert_eq!(conclusion.distillation, "The null fallback is incorrect.");
        write_reporter_thread(&review_dir, &conclusion, "head-a")?;
        match resolve_reporter_turn(&review_dir, "head-a") {
            ReporterTurnResolution::Current(turn) => {
                assert_eq!(turn.verdict, ReporterVerdict::ChangesRequested);
                assert_eq!(turn.distillation, "The null fallback is incorrect.");
                assert_eq!(turn.receipt_ref, "review/threads/reporter/turn-000.json");
                assert_eq!(turn.turn, 0);
            }
            other => anyhow::bail!("expected Current, got {other:?}"),
        }
        assert_eq!(
            read_reporter_verdict(&review_dir, "head-a"),
            Some(ReporterVerdict::ChangesRequested)
        );
        Ok(())
    }

    #[test]
    fn turn_001_supersedes_turn_000_for_public_and_gate() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let first = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "initial distillation".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::Clear,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_turn(&review_dir, &first, 0, "head-a", "reporter_completed", &[])?;
        let second = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "revised after follow-ups".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::ChangesRequested,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_turn(
            &review_dir,
            &second,
            1,
            "head-a",
            "reporter_re_distilled",
            &["lane-answer:tests-oracle".to_owned()],
        )?;
        match resolve_reporter_turn(&review_dir, "head-a") {
            ReporterTurnResolution::Current(turn) => {
                assert_eq!(turn.turn, 1);
                assert_eq!(turn.receipt_ref, "review/threads/reporter/turn-001.json");
                assert_eq!(turn.distillation, "revised after follow-ups");
                assert_eq!(turn.verdict, ReporterVerdict::ChangesRequested);
            }
            other => anyhow::bail!("expected Current turn-001, got {other:?}"),
        }
        assert_eq!(
            resolve_reporter_turn(&review_dir, "head-a").public_distillation(),
            Some("revised after follow-ups")
        );
        match resolve_reporter_turn(&review_dir, "head-a").gate_input() {
            ReporterGateInput::Verdict { verdict, receipt } => {
                assert_eq!(verdict, ReporterVerdict::ChangesRequested);
                assert_eq!(receipt, "review/threads/reporter/turn-001.json");
            }
            other => anyhow::bail!("expected Verdict, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn stale_head_is_not_public_and_is_explicit_gate_evidence() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "stale decision text".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::ChangesRequested,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion, "old-head")?;
        let resolution = resolve_reporter_turn(&review_dir, "new-head");
        assert!(resolution.public_distillation().is_none());
        match resolution {
            ReporterTurnResolution::StaleHead {
                expected,
                found,
                receipt,
            } => {
                assert_eq!(expected, "new-head");
                assert_eq!(found, "old-head");
                assert_eq!(receipt, "review/threads/reporter/turn-000.json");
            }
            other => anyhow::bail!("expected StaleHead, got {other:?}"),
        }
        match resolve_reporter_turn(&review_dir, "new-head").gate_input() {
            ReporterGateInput::Unusable { kind, receipt, .. } => {
                assert_eq!(kind, "reporter-stale-head");
                assert_eq!(receipt, "review/threads/reporter/turn-000.json");
            }
            other => anyhow::bail!("expected Unusable, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn legacy_turn_without_head_remains_readable() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let thread_dir = review_dir.join("threads").join("reporter");
        std::fs::create_dir_all(&thread_dir)?;
        let legacy = crate::LaneThreadTurn {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            thread_id: "tid".to_owned(),
            turn: 0,
            stage: "reporter".to_owned(),
            prompt_packet_path: "review/threads/reporter/prompt.md".to_owned(),
            response_summary: "legacy distillation".to_owned(),
            routed_evidence_refs: vec![],
            receipt_ref: "review/threads/reporter/turn-000.json".to_owned(),
            head_sha: None,
            verdict: None,
        };
        std::fs::write(
            thread_dir.join("turn-000.json"),
            serde_json::to_vec_pretty(&legacy)?,
        )?;
        std::fs::write(
            thread_dir.join("thread.json"),
            serde_json::to_vec_pretty(&crate::LaneThreadSession {
                schema: crate::artifacts::LANE_THREAD_SCHEMA.to_owned(),
                thread_id: "tid".to_owned(),
                lane: "reporter".to_owned(),
                cohort_id: "cid".to_owned(),
                turns: vec!["turn-000".to_owned()],
                latest_turn: Some("turn-000".to_owned()),
                latest_turn_ref: Some("review/threads/reporter/turn-000.json".to_owned()),
                latest_conclusion: "legacy distillation".to_owned(),
                head_sha: None,
                verdict: None,
                terminal_reason: "reporter_completed".to_owned(),
            })?,
        )?;
        match resolve_reporter_turn(&review_dir, "any-head") {
            ReporterTurnResolution::Current(turn) => {
                assert_eq!(turn.distillation, "legacy distillation");
                assert_eq!(turn.verdict, ReporterVerdict::None);
                assert_eq!(turn.receipt_ref, "review/threads/reporter/turn-000.json");
            }
            other => anyhow::bail!("expected Current legacy, got {other:?}"),
        }
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
        assert!(!prompt.contains("Routed late deterministic evidence"));
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
            &[],
        );
        assert!(prompt.contains("Routed proof evidence"));
        assert!(prompt.contains("non_discriminating"));
        assert!(prompt.contains("Revise"));
    }

    #[test]
    fn lane_continuation_prompt_routes_late_sensor_evidence_when_present() {
        let prompt = crate::lane_continuation_prompt(
            "tests-oracle",
            "specialist reviewer",
            "The test may not discriminate the patch.",
            "Does the full suite confirm your concern?",
            "PR has a test-gap concern.",
            &[],
            &[LateSensorDigest {
                sensor: "cargo-test".to_owned(),
                status: "failed".to_owned(),
                reason: "exit code Some(101)".to_owned(),
                receipt_path: "sensors/cargo-test/ub-review-sensor-status.json".to_owned(),
            }
            .excerpt()],
        );
        assert!(prompt.contains("Routed late deterministic evidence"));
        assert!(prompt.contains("late sensor `cargo-test` status=`failed`"));
        assert!(prompt.contains("landed after your primary turn"));
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
