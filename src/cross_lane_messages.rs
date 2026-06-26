//! Cross-lane message queue (Order 8 of #678).
//!
//! An append-only `review/messages.ndjson` that records the lossless,
//! ordered working-memory surface of a run: lane reports, reporter questions,
//! lane answers, proof requests/receipts, evidence routing, topic updates, and
//! thread terminations. Sequence numbers (not wall-clock) drive ordering and
//! replay.
//!
//! This is the message bus the reporter (Order 9) reads from and addresses. It
//! preserves rather than deletes information — every potentially material
//! cross-lane contribution stays visible until the reporter consumes it.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::artifacts::CROSS_LANE_MESSAGE_SCHEMA;

/// One typed message in the cross-lane queue.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct CrossLaneMessage {
    pub(crate) schema: String,
    /// Monotonic sequence number (drives ordering/replay, not wall-clock).
    pub(crate) seq: u64,
    pub(crate) ts: DateTime<Utc>,
    pub(crate) kind: CrossLaneMessageKind,
    /// Originating thread/lane/actor ("tests-oracle", "reporter", "proof-broker").
    pub(crate) from: String,
    /// Destination ("tests-oracle", "reporter", "all-lanes", or "" for broadcast).
    pub(crate) to: String,
    /// Turn within the origin's thread (0 for first wave).
    pub(crate) turn: u32,
    /// Artifact/claim references this message cites (thread turn paths, claim ids).
    pub(crate) references: Vec<String>,
    /// Kind-specific payload (free-form JSON for forward compatibility).
    pub(crate) payload: serde_json::Value,
}

/// The typed message kinds. Placeholder kinds (reporter_question, lane_answer,
/// topic_update) are emitted by the reporter in Order 9; the rest are emitted
/// by the production paths now.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CrossLaneMessageKind {
    /// A lane's investigation conclusion.
    LaneReport,
    /// The reporter asked a named lane a question (Order 9).
    ReporterQuestion,
    /// A lane answered a reporter question (Order 9).
    LaneAnswer,
    /// A proof request was issued.
    ProofRequest,
    /// A proof receipt was produced.
    ProofReceipt,
    /// Evidence was routed to a lane.
    EvidenceRouted,
    /// A topic/coverage annotation update (Order 9).
    TopicUpdate,
    /// A lane thread terminated.
    ThreadTerminal,
}

/// Append-only writer for `review/messages.ndjson`. Thread-safe; sequence
/// numbers are monotonic across the run.
pub(crate) struct MessageLog {
    file: Mutex<File>,
    path: PathBuf,
    seq: AtomicU64,
}

impl MessageLog {
    pub(crate) fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open message log {}", path.display()))?;
        Ok(Self {
            file: Mutex::new(file),
            path: path.to_path_buf(),
            seq: AtomicU64::new(0),
        })
    }

    /// Append a typed message. Sequence number is assigned atomically and
    /// returned. Errors are returned (the caller may log them to the event log
    /// rather than propagating, since the queue is observability).
    pub(crate) fn append(
        &self,
        kind: CrossLaneMessageKind,
        from: &str,
        to: &str,
        turn: u32,
        references: Vec<String>,
        payload: serde_json::Value,
    ) -> Result<u64> {
        let seq = self.seq.fetch_add(1, Ordering::SeqCst);
        let msg = CrossLaneMessage {
            schema: CROSS_LANE_MESSAGE_SCHEMA.to_owned(),
            seq,
            ts: Utc::now(),
            kind,
            from: from.to_owned(),
            to: to.to_owned(),
            turn,
            references,
            payload,
        };
        let mut file = self
            .file
            .lock()
            .map_err(|_| anyhow::anyhow!("message log mutex poisoned"))?;
        serde_json::to_writer(&mut *file, &msg)?;
        writeln!(&mut *file)?;
        Ok(seq)
    }

    /// Path of the underlying ndjson file (for tests/inspection).
    #[allow(dead_code)]
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_log_appends_monotonic_sequence() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("messages.ndjson");
        let log = MessageLog::open(&path)?;
        let s0 = log.append(
            CrossLaneMessageKind::LaneReport,
            "tests-oracle",
            "reporter",
            0,
            vec!["review/threads/tests-oracle/turn-000.json".to_owned()],
            serde_json::json!({"conclusion": "test discriminates"}),
        )?;
        let s1 = log.append(
            CrossLaneMessageKind::ProofReceipt,
            "proof-broker",
            "tests-oracle",
            0,
            vec![],
            serde_json::json!({"result": "discriminating"}),
        )?;
        assert!(s1 > s0, "sequence must be monotonic");
        // Read back and validate NDJSON.
        let text = std::fs::read_to_string(&path)?;
        let lines: Vec<&str> = text.trim().lines().collect();
        assert_eq!(lines.len(), 2);
        let m0: CrossLaneMessage = serde_json::from_str(lines[0])?;
        let m1: CrossLaneMessage = serde_json::from_str(lines[1])?;
        assert_eq!(m0.kind, CrossLaneMessageKind::LaneReport);
        assert_eq!(m0.from, "tests-oracle");
        assert_eq!(m0.to, "reporter");
        assert_eq!(m0.seq, s0);
        assert_eq!(m1.kind, CrossLaneMessageKind::ProofReceipt);
        assert!(m1.seq > m0.seq);
        assert_eq!(m0.schema, "ub-review.cross_lane_message.v1");
        Ok(())
    }

    #[test]
    fn message_kind_serializes_snake_case() -> Result<()> {
        // Pin the on-wire kind names the reporter (Order 9) will match on.
        let wire = serde_json::to_string(&CrossLaneMessageKind::LaneReport)?;
        assert_eq!(wire.trim_matches('"'), "lane_report");
        let wire = serde_json::to_string(&CrossLaneMessageKind::ReporterQuestion)?;
        assert_eq!(wire.trim_matches('"'), "reporter_question");
        let wire = serde_json::to_string(&CrossLaneMessageKind::ProofReceipt)?;
        assert_eq!(wire.trim_matches('"'), "proof_receipt");
        let wire = serde_json::to_string(&CrossLaneMessageKind::EvidenceRouted)?;
        assert_eq!(wire.trim_matches('"'), "evidence_routed");
        Ok(())
    }
}
