//! unsafe-review structured-output parsing: the gate file and comment-plan
//! entries the lane evidence renderer consumes.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// unsafe-review `first-pr --out-dir <dir>` top-level artifact
/// (`unsafe-review-gate.json`, schema `unsafe-review-gate/v1`).
///
/// Shape verified against real `unsafe-review 0.3.4 first-pr --out-dir` output:
/// movement counts are NESTED under `summary`, `status` is the advisory word
/// (`"advisory"`), and the `artifacts` map keys are snake_case
/// (`comment_plan`, `repair_queue`, ...) while their values are the hyphenated
/// filenames.
///
/// Only the fields consumed by ub-review are bound; unknown fields are
/// silently ignored so forward-compatible additions in unsafe-review ≥0.3.5
/// do not break ingestion. The schema_version is validated before use: only
/// `"unsafe-review-gate/v1"` is understood; anything else degrades to the
/// status-only fallback rather than crashing.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UnsafeReviewGate {
    pub(crate) schema_version: String,
    /// Dialect marker on the real manifest (e.g. `"unsafe-review"`). Context
    /// only; surfaced if present, never a gate input.
    #[serde(default)]
    pub(crate) dialect: Option<String>,
    /// Advisory status word from unsafe-review. In 0.3.x this is `"advisory"`.
    /// Never used as a gate input; surfaced as context only.
    pub(crate) status: String,
    /// Movement summary relative to base, nested under `summary` on the real
    /// manifest. `#[serde(default)]` so a manifest without it reads zeroes
    /// rather than failing to parse.
    #[serde(default)]
    pub(crate) summary: UnsafeReviewSummary,
    /// Advisory boundary declared by the tool; must be preserved and surfaced.
    /// In 0.3.x this is the sentence "static unsafe-review coverage evidence;
    /// not proof, not a merge verdict".
    #[serde(default)]
    pub(crate) trust_boundary: Option<String>,
    /// Tool name on the real manifest (e.g. `"unsafe-review"`). Context only.
    #[serde(default)]
    pub(crate) tool: Option<String>,
    /// Tool version on the real manifest (e.g. `"0.3.4"`). Context only.
    #[serde(default)]
    pub(crate) tool_version: Option<String>,
    /// Relative artifact pointers within the output directory. Keys are
    /// snake_case (`cards`, `comment_plan`, `repair_queue`, `receipt_audit`,
    /// `review_kit`, `pr_summary`, `sarif`, `lsp`, `policy_report`); values are
    /// the hyphenated filenames.
    #[serde(default)]
    pub(crate) artifacts: std::collections::BTreeMap<String, String>,
}

/// Movement summary block nested under `summary` in `unsafe-review-gate/v1`.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct UnsafeReviewSummary {
    #[serde(default)]
    pub(crate) new_gaps: u32,
    #[serde(default)]
    pub(crate) worsened_gaps: u32,
    #[serde(default)]
    pub(crate) resolved_gaps: u32,
    #[serde(default)]
    pub(crate) inherited_gaps: u32,
}

/// One entry from `comment-plan.json` produced by unsafe-review 0.3.4.
///
/// Field names verified against real output: each entry carries `card_id`,
/// `path`, `line`, `changed_line`, `coverage_gap`, `selection_reason`,
/// `selection_reason_code`, `confirmation_state`, and `trust_boundary`. Only
/// the fields ub-review uses are bound here; unknown fields are tolerated so
/// the plan stays loadable as unsafe-review extends it. Structured for #360 to
/// consume directly; no further parsing is done here.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct UnsafeReviewCommentPlanEntry {
    #[serde(default)]
    pub(crate) card_id: Option<String>,
    #[serde(default)]
    pub(crate) path: Option<String>,
    #[serde(default)]
    pub(crate) line: Option<u32>,
    /// Whether the anchored line is a changed line in this diff.
    #[serde(default)]
    pub(crate) changed_line: Option<bool>,
    #[serde(default)]
    pub(crate) coverage_gap: Option<String>,
    #[serde(default)]
    pub(crate) selection_reason: Option<String>,
    /// Stable machine code for the selection reason (for #360 routing).
    #[serde(default)]
    pub(crate) selection_reason_code: Option<String>,
    /// e.g. "unconfirmed" — the confirmation lifecycle state.
    #[serde(default)]
    pub(crate) confirmation_state: Option<String>,
    /// Advisory boundary propagated per-entry to consumers.
    #[serde(default)]
    pub(crate) trust_boundary: Option<String>,
}

/// Parsed unsafe-review artifacts loaded from `--out-dir <dir>`.
pub(crate) struct UnsafeReviewArtifacts {
    /// Validated gate receipt (schema_version == "unsafe-review-gate/v1").
    pub(crate) gate: UnsafeReviewGate,
    /// comment-plan entries (bounded, ready for #360). Empty when absent.
    pub(crate) comment_plan: Vec<UnsafeReviewCommentPlanEntry>,
}

pub(crate) const UNSAFE_REVIEW_GATE_SCHEMA: &str = "unsafe-review-gate/v1";

pub(crate) const UNSAFE_REVIEW_OUTPUT_SUBDIR: &str = "unsafe-review-output";

/// Parse the structured artifacts written by `unsafe-review first-pr --out-dir`.
///
/// Returns `None` when the gate file is absent (sensor did not run or failed),
/// or when `schema_version` is not `"unsafe-review-gate/v1"` (unknown schema —
/// degrade gracefully, fall back to status-only rendering in callers).
pub(crate) fn read_unsafe_review_artifacts(sensor_dir: &Path) -> Option<UnsafeReviewArtifacts> {
    let out_dir = sensor_dir.join(UNSAFE_REVIEW_OUTPUT_SUBDIR);
    let gate_path = out_dir.join("unsafe-review-gate.json");
    let text = fs::read_to_string(&gate_path).ok()?;
    let gate: UnsafeReviewGate = serde_json::from_str(&text).ok()?;
    if gate.schema_version != UNSAFE_REVIEW_GATE_SCHEMA {
        // Unknown schema — caller degrades to status-only; log nothing here
        // so the degradation is legible in lane packets rather than silent.
        return None;
    }
    // Follow the artifacts pointer for comment-plan.json (if present).
    // Key is snake_case `comment_plan` in unsafe-review-gate/v1 (the value is the
    // hyphenated filename); routing by the wrong key silently drops the plan.
    let comment_plan = gate
        .artifacts
        .get("comment_plan")
        .and_then(|rel| {
            let cp_path = out_dir.join(rel);
            fs::read_to_string(&cp_path).ok()
        })
        .and_then(|cp_text| {
            serde_json::from_str::<Vec<UnsafeReviewCommentPlanEntry>>(&cp_text).ok()
        })
        .unwrap_or_default();
    Some(UnsafeReviewArtifacts { gate, comment_plan })
}

#[cfg(test)]
mod tests {

    use anyhow::Result;

    use crate::*;

    /// v1 gate.json present with a comment-plan: ingestion succeeds, movement
    /// values come through the NESTED `summary` block, and the comment-plan
    /// loads via the snake_case `comment_plan` artifacts key. Fixture matches
    /// the REAL `unsafe-review 0.3.4 first-pr --out-dir` manifest shape.
    #[test]
    fn unsafe_review_artifacts_v1_gate_ingested() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;

        // Real-shape v1 gate manifest: movement NESTED under `summary`, status
        // word `"advisory"`, snake_case `artifacts` keys, the real
        // `trust_boundary` sentence, plus `dialect`/`tool`/`tool_version`.
        fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{
                "schema_version": "unsafe-review-gate/v1",
                "dialect": "unsafe-review",
                "status": "advisory",
                "summary": {
                    "new_gaps": 2,
                    "worsened_gaps": 0,
                    "resolved_gaps": 1,
                    "inherited_gaps": 3
                },
                "artifacts": {
                    "cards": "cards.json",
                    "comment_plan": "comment-plan.json",
                    "repair_queue": "repair-queue.json",
                    "receipt_audit": "receipt-audit.md",
                    "review_kit": "review-kit.json",
                    "pr_summary": "pr-summary.md",
                    "sarif": "cards.sarif",
                    "lsp": "lsp.json",
                    "policy_report": "policy-report.json"
                },
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict",
                "tool": "unsafe-review",
                "tool_version": "0.3.4"
            }"#,
        )?;
        // Real-shape comment-plan entry: every field unsafe-review actually
        // emits is present, including the ones #360 will route on.
        fs::write(
            out_dir.join("comment-plan.json"),
            r#"[{
                "card_id": "card-001",
                "path": "src/lib.rs",
                "line": 42,
                "changed_line": true,
                "coverage_gap": "raw pointer dereference without lifetime guard",
                "selection_reason": "changed line in unsafe block",
                "selection_reason_code": "changed-line-unsafe",
                "confirmation_state": "unconfirmed",
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict"
            }]"#,
        )?;

        let artifacts = super::read_unsafe_review_artifacts(&sensor_dir)
            .ok_or_else(|| anyhow::anyhow!("expected Some(artifacts), got None"))?;
        assert_eq!(
            artifacts.gate.schema_version,
            super::UNSAFE_REVIEW_GATE_SCHEMA
        );
        assert_eq!(artifacts.gate.status, "advisory");
        assert_eq!(artifacts.gate.dialect.as_deref(), Some("unsafe-review"));
        assert_eq!(artifacts.gate.tool.as_deref(), Some("unsafe-review"));
        assert_eq!(artifacts.gate.tool_version.as_deref(), Some("0.3.4"));
        // Movement must come through the NESTED summary, not flat top-level.
        assert_eq!(artifacts.gate.summary.new_gaps, 2);
        assert_eq!(artifacts.gate.summary.worsened_gaps, 0);
        assert_eq!(artifacts.gate.summary.resolved_gaps, 1);
        assert_eq!(artifacts.gate.summary.inherited_gaps, 3);
        assert_eq!(
            artifacts.gate.trust_boundary.as_deref(),
            Some("static unsafe-review coverage evidence; not proof, not a merge verdict")
        );
        // comment-plan loaded via the snake_case `comment_plan` artifacts key.
        assert_eq!(artifacts.comment_plan.len(), 1);
        let entry = &artifacts.comment_plan[0];
        assert_eq!(entry.card_id.as_deref(), Some("card-001"));
        assert_eq!(entry.path.as_deref(), Some("src/lib.rs"));
        assert_eq!(entry.line, Some(42));
        assert_eq!(entry.changed_line, Some(true));
        assert_eq!(
            entry.selection_reason_code.as_deref(),
            Some("changed-line-unsafe")
        );
        assert_eq!(entry.confirmation_state.as_deref(), Some("unconfirmed"));
        assert_eq!(
            entry.trust_boundary.as_deref(),
            Some("static unsafe-review coverage evidence; not proof, not a merge verdict")
        );
        Ok(())
    }

    /// Unknown schema_version: read_unsafe_review_artifacts must return None
    /// (graceful degrade) rather than panicking or returning partial data.
    #[test]
    fn unsafe_review_artifacts_unknown_schema_degrades_gracefully() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;
        // Real-shape manifest but a future schema_version: nested summary,
        // advisory status, snake_case artifacts. Routing must still degrade.
        fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{
                "schema_version": "unsafe-review-gate/v2-future",
                "dialect": "unsafe-review",
                "status": "advisory",
                "summary": {
                    "new_gaps": 0,
                    "worsened_gaps": 0,
                    "resolved_gaps": 0,
                    "inherited_gaps": 0
                },
                "artifacts": {"comment_plan": "comment-plan.json"},
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict",
                "tool": "unsafe-review",
                "tool_version": "0.4.0"
            }"#,
        )?;
        // Must return None, not panic or error
        let result = super::read_unsafe_review_artifacts(&sensor_dir);
        assert!(
            result.is_none(),
            "expected None for unknown schema_version, got Some"
        );
        Ok(())
    }

    /// Absent gate file: returns None cleanly.
    #[test]
    fn unsafe_review_artifacts_absent_gate_returns_none() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        // Don't create any files — the output dir does not exist
        let result = super::read_unsafe_review_artifacts(&sensor_dir);
        assert!(result.is_none(), "expected None when gate file absent");
        Ok(())
    }
}
