//! Persistent logical lane sessions (Order 7 of #678).
//!
//! Records each specialist lane's logical thread history as inspectable
//! artifacts under `review/threads/<lane>/`:
//! - `thread.json` — the rollup: thread_id, lane, cohort_id, the ordered turn
//!   ids, the latest conclusion, and a terminal reason.
//! - `turn-NNN.json` — one per execution turn: thread_id, turn number, stage
//!   ("primary" | "follow-up"), the prompt packet path, a response summary,
//!   routed evidence refs, and a receipt reference.
//!
//! This is observability/provenance today: it makes a lane's logical history
//! inspectable and gives the reporter (Order 9) a thread to address. The
//! actual multi-turn *continuation* (a follow-up that reuses the lane's prior
//! context rather than starting fresh) lands with the reporter.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::artifacts::LANE_THREAD_SCHEMA;

/// One turn within a lane's logical thread.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LaneThreadTurn {
    pub(crate) schema: String,
    pub(crate) thread_id: String,
    /// Turn number within the thread (0 for the first investigation wave).
    pub(crate) turn: u32,
    /// Execution stage: "primary" for the first wave, "follow-up" for
    /// subsequent turns (Order 9 reporter-driven continuation).
    pub(crate) stage: String,
    /// Path to the lane's prompt packet (`lanes/<lane>.md`).
    pub(crate) prompt_packet_path: String,
    /// Short summary of the model's response for this turn (truncated).
    pub(crate) response_summary: String,
    /// References to evidence routed into this turn (sensor/lane ids).
    pub(crate) routed_evidence_refs: Vec<String>,
    /// Reference to the `ModelLaneReceipt` for this turn (`review/model/<lane>/...`).
    pub(crate) receipt_ref: String,
    /// Exact PR head this turn was produced for. Required on new reporter
    /// turns; omitted on legacy turns and ordinary investigation lanes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) head_sha: Option<String>,
    /// Structured reporter verdict (`clear` / `changes_requested` /
    /// `uncertain` / `none`). Present only on reporter turns; never inferred
    /// from distillation prose.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verdict: Option<String>,
}

/// The rollup for a lane's logical thread. Written to
/// `review/threads/<lane>/thread.json`.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LaneThreadSession {
    pub(crate) schema: String,
    pub(crate) thread_id: String,
    pub(crate) lane: String,
    pub(crate) cohort_id: String,
    /// Ordered turn ids ("turn-000", "turn-001", ...).
    pub(crate) turns: Vec<String>,
    /// Highest-numbered turn id present on disk (same authority as
    /// `latest_conclusion` / identity fields below).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) latest_turn: Option<String>,
    /// Artifact path of the highest-numbered turn.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) latest_turn_ref: Option<String>,
    /// The latest conclusion the lane reached (truncated response tail).
    pub(crate) latest_conclusion: String,
    /// Head SHA bound on the highest-numbered turn, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) head_sha: Option<String>,
    /// Structured verdict on the highest-numbered turn, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) verdict: Option<String>,
    /// Why the thread terminated, if it has ("completed" | "skipped_budget" |
    /// "preflight_failed" | ...). Empty while the thread is active.
    pub(crate) terminal_reason: String,
}

/// Write a lane thread turn artifact (`review/threads/<lane>/turn-NNN.json`)
/// and refresh the thread rollup (`thread.json`). Idempotent per (lane, turn):
/// re-writing the same turn overwrites; the rollup is rebuilt from the turn
/// files present on disk.
pub(crate) fn write_lane_thread_turn(
    review_dir: &Path,
    lane: &str,
    turn: &LaneThreadTurn,
    cohort_id: &str,
    terminal_reason: &str,
) -> Result<()> {
    let thread_dir = review_dir.join("threads").join(lane);
    fs::create_dir_all(&thread_dir)?;
    let turn_id = format!("turn-{:03}", turn.turn);
    let turn_path = thread_dir.join(format!("{turn_id}.json"));
    fs::write(&turn_path, serde_json::to_vec_pretty(turn)?)?;

    // Rebuild the rollup from the turn files on disk so it reflects every turn
    // written (including prior turns from earlier waves). All "latest" fields
    // must come from the same highest-numbered parsed turn; never mix identity
    // from the write call with conclusion from a different turn on disk.
    let mut turn_ids = Vec::new();
    for entry in fs::read_dir(&thread_dir)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("turn-") && name.ends_with(".json") {
            let stem = name.trim_end_matches(".json");
            turn_ids.push(stem.to_owned());
        }
    }
    turn_ids.sort();
    let mut latest_conclusion = String::new();
    let mut latest_turn = None;
    let mut latest_turn_ref = None;
    let mut latest_head_sha = None;
    let mut latest_verdict = None;
    let mut latest_thread_id = turn.thread_id.clone();
    let mut latest_cohort_id = cohort_id.to_owned();
    if let Some(last_id) = turn_ids.last() {
        let last_path = thread_dir.join(format!("{last_id}.json"));
        latest_turn = Some(last_id.clone());
        latest_turn_ref = Some(format!("review/threads/{lane}/{last_id}.json"));
        if let Ok(bytes) = fs::read(&last_path)
            && let Ok(parsed) = serde_json::from_slice::<LaneThreadTurn>(&bytes)
        {
            latest_conclusion = parsed.response_summary;
            latest_head_sha = parsed.head_sha;
            latest_verdict = parsed.verdict;
            latest_thread_id = parsed.thread_id;
            // cohort is not stored on the turn; keep the write-call cohort only
            // when this write produced the highest turn, otherwise leave the
            // previous rollup cohort if present.
            if parsed.turn == turn.turn {
                latest_cohort_id = cohort_id.to_owned();
            } else if let Ok(existing) = fs::read(thread_dir.join("thread.json"))
                && let Ok(prev) = serde_json::from_slice::<LaneThreadSession>(&existing)
            {
                latest_cohort_id = prev.cohort_id;
            }
        }
    }
    let session = LaneThreadSession {
        schema: LANE_THREAD_SCHEMA.to_owned(),
        thread_id: latest_thread_id,
        lane: lane.to_owned(),
        cohort_id: latest_cohort_id,
        turns: turn_ids,
        latest_turn,
        latest_turn_ref,
        latest_conclusion,
        head_sha: latest_head_sha,
        verdict: latest_verdict,
        terminal_reason: terminal_reason.to_owned(),
    };
    fs::write(
        thread_dir.join("thread.json"),
        serde_json::to_vec_pretty(&session)?,
    )?;
    Ok(())
}

/// Build a turn from a lane's first-wave execution. `response_summary` is
/// truncated to keep the artifact small (full content lives in
/// `review/model/<lane>/content.json`).
pub(crate) fn primary_turn(
    thread_id: &str,
    lane: &str,
    response_summary: &str,
    routed_evidence_refs: Vec<String>,
    receipt_ref: &str,
) -> LaneThreadTurn {
    LaneThreadTurn {
        schema: LANE_THREAD_SCHEMA.to_owned(),
        thread_id: thread_id.to_owned(),
        turn: 0,
        stage: "primary".to_owned(),
        prompt_packet_path: format!("lanes/{lane}.md"),
        response_summary: truncate_summary(response_summary),
        routed_evidence_refs,
        receipt_ref: receipt_ref.to_owned(),
        head_sha: None,
        verdict: None,
    }
}

/// Truncate a response summary to a bounded length for the turn artifact.
fn truncate_summary(s: &str) -> String {
    const MAX: usize = 1200;
    if s.chars().count() <= MAX {
        return s.to_owned();
    }
    let truncated: String = s.chars().take(MAX).collect();
    format!("{truncated}…[truncated; full content in review/model content.json]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_lane_thread_turn_creates_turn_and_rollup() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let turn = primary_turn(
            "cohort:minimax:M3:abc:def",
            "tests-oracle",
            "The new test does not discriminate the patch.",
            vec!["sensor:ripr".to_owned()],
            "review/model/tests-oracle/receipt.json",
        );
        write_lane_thread_turn(
            &review_dir,
            "tests-oracle",
            &turn,
            "cohort:minimax:M3:abc",
            "completed",
        )?;
        let turn_path = review_dir.join("threads/tests-oracle/turn-000.json");
        assert!(turn_path.exists(), "turn-000.json must exist");
        let parsed: LaneThreadTurn = serde_json::from_slice(&fs::read(&turn_path)?)?;
        assert_eq!(parsed.turn, 0);
        assert_eq!(parsed.stage, "primary");
        assert_eq!(parsed.prompt_packet_path, "lanes/tests-oracle.md");
        assert!(parsed.response_summary.contains("discriminate"));
        let thread_path = review_dir.join("threads/tests-oracle/thread.json");
        assert!(thread_path.exists(), "thread.json must exist");
        let session: LaneThreadSession = serde_json::from_slice(&fs::read(&thread_path)?)?;
        assert_eq!(session.lane, "tests-oracle");
        assert_eq!(session.thread_id, turn.thread_id);
        assert_eq!(session.turns, vec!["turn-000".to_owned()]);
        assert!(session.latest_conclusion.contains("discriminate"));
        assert_eq!(session.terminal_reason, "completed");
        Ok(())
    }

    #[test]
    fn multiple_turns_rollup_picks_latest_conclusion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let t0 = primary_turn("tid", "lane", "first conclusion", vec![], "r0");
        write_lane_thread_turn(&review_dir, "lane", &t0, "cid", "")?;
        let mut t1 = primary_turn("tid", "lane", "revised conclusion", vec![], "r1");
        t1.turn = 1;
        write_lane_thread_turn(&review_dir, "lane", &t1, "cid", "completed")?;
        let session: LaneThreadSession =
            serde_json::from_slice(&fs::read(review_dir.join("threads/lane/thread.json"))?)?;
        assert_eq!(session.turns, vec!["turn-000", "turn-001"]);
        assert!(session.latest_conclusion.contains("revised"));
        Ok(())
    }

    #[test]
    fn truncate_summary_bounds_long_responses() {
        let long = "x".repeat(5000);
        let s = truncate_summary(&long);
        assert!(s.chars().count() < 5000);
        assert!(s.contains("[truncated"));
    }

    #[test]
    fn rollup_rewriting_older_turn_keeps_latest_identity() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let mut t0 = primary_turn("tid-a", "lane", "initial", vec![], "r0");
        t0.head_sha = Some("aaa".to_owned());
        write_lane_thread_turn(&review_dir, "lane", &t0, "cid-a", "")?;
        let mut t1 = primary_turn("tid-b", "lane", "revised", vec![], "r1");
        t1.turn = 1;
        t1.head_sha = Some("bbb".to_owned());
        t1.verdict = Some("changes_requested".to_owned());
        write_lane_thread_turn(&review_dir, "lane", &t1, "cid-b", "completed")?;

        // Rewrite turn-000 under a different head/identity after turn-001 exists.
        let mut t0_rewrite = primary_turn("tid-stale", "lane", "rewritten-initial", vec![], "r0b");
        t0_rewrite.head_sha = Some("aaa".to_owned());
        write_lane_thread_turn(&review_dir, "lane", &t0_rewrite, "cid-stale", "rewritten")?;

        let session: LaneThreadSession =
            serde_json::from_slice(&fs::read(review_dir.join("threads/lane/thread.json"))?)?;
        assert_eq!(session.latest_turn.as_deref(), Some("turn-001"));
        assert_eq!(
            session.latest_turn_ref.as_deref(),
            Some("review/threads/lane/turn-001.json")
        );
        assert!(session.latest_conclusion.contains("revised"));
        assert_eq!(session.head_sha.as_deref(), Some("bbb"));
        assert_eq!(session.verdict.as_deref(), Some("changes_requested"));
        assert_eq!(session.thread_id, "tid-b");
        assert_eq!(session.cohort_id, "cid-b");
        Ok(())
    }
}
