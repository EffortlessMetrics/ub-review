//! unsafe-review structured-output parsing: the gate file and comment-plan
//! entries the lane evidence renderer consumes.

use std::fs;
use std::path::{Component, Path, PathBuf};

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
/// do not break ingestion. The schema_version is routed before this shape is
/// bound: only `"unsafe-review-gate/v1"` is understood, and anything else is a
/// typed ingest gap naming the found version.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct UnsafeReviewGate {
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
    /// Optional upstream floor timing for cost receipts. Absent in
    /// unsafe-review 0.3.4; ub-review records the absence explicitly rather
    /// than synthesizing a duration.
    #[serde(default)]
    pub(crate) required_floor_wall_seconds: Option<f64>,
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

/// One entry from the `comments` array in the object envelope emitted by
/// unsafe-review 0.3.x (`comment-plan.json`, schema `0.1`).
///
/// Field names verified against real output: each entry carries `card_id`,
/// `path`, `line`, `changed_line`, `coverage_gap`, `selection_reason`,
/// `selection_reason_code`, `confirmation_state`, `operation_family`, and
/// `trust_boundary`. Only the fields ub-review uses are bound here; unknown
/// fields are tolerated so the plan stays loadable as unsafe-review extends
/// it. Required identity fields are validated by the envelope parser before
/// a candidate can be consumed.
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
    /// Opaque producer operation family.  The consumer preserves this value
    /// for identity joins and display; it deliberately does not impose a
    /// closed vocabulary on future unsafe-review producers.
    #[serde(default)]
    pub(crate) operation_family: Option<String>,
}

/// Parsed unsafe-review artifacts loaded from `--out-dir <dir>`.
pub(crate) struct UnsafeReviewArtifacts {
    /// Validated gate receipt (schema_version == "unsafe-review-gate/v1").
    pub(crate) gate: UnsafeReviewGate,
    /// comment-plan entries (bounded, ready for #360). Empty when absent.
    pub(crate) comment_plan: Vec<UnsafeReviewCommentPlanEntry>,
}

pub(crate) const UNSAFE_REVIEW_GATE_SCHEMA: &str = "unsafe-review-gate/v1";

/// Schema version emitted by unsafe-review 0.3.x `comment-plan.json`.
pub(crate) const UNSAFE_REVIEW_COMMENT_PLAN_SCHEMA: &str = "0.1";

pub(crate) const UNSAFE_REVIEW_OUTPUT_SUBDIR: &str = "unsafe-review-output";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum UnsafeReviewIngestGap {
    Absent,
    Unreadable(String),
    Malformed(String),
    UnknownSchema(String),
    CommentPlanMissingPointer,
    CommentPlanUnreadable(String),
    CommentPlanInvalidPointer(String),
    CommentPlanMalformed(String),
    CommentPlanUnknownSchema(String),
    CommentPlanMissingField(String),
    CommentPlanInvalidField(String),
}

impl UnsafeReviewIngestGap {
    pub(crate) fn reason(&self) -> String {
        match self {
            Self::Absent => {
                "unsafe-review-gate.json absent; structured evidence unavailable".to_owned()
            }
            Self::Unreadable(detail) => {
                format!("unsafe-review-gate.json unreadable: {detail}")
            }
            Self::Malformed(detail) => {
                format!("unsafe-review-gate.json malformed: {detail}")
            }
            Self::UnknownSchema(found) => format!(
                "unsafe-review-gate.json schema_version `{found}` not recognised \
                 (known: `{UNSAFE_REVIEW_GATE_SCHEMA}`); structured evidence not parsed"
            ),
            Self::CommentPlanMissingPointer => {
                "comment-plan.json pointer missing from unsafe-review-gate.json artifacts; structured comment evidence not parsed".to_owned()
            }
            Self::CommentPlanUnreadable(detail) => {
                format!("comment-plan.json unreadable: {detail}")
            }
            Self::CommentPlanInvalidPointer(detail) => {
                format!("comment-plan.json invalid artifact pointer: {detail}")
            }
            Self::CommentPlanMalformed(detail) => {
                format!("comment-plan.json malformed: {detail}")
            }
            Self::CommentPlanUnknownSchema(found) => format!(
                "comment-plan.json schema_version `{found}` not recognised \
                 (known: `{UNSAFE_REVIEW_COMMENT_PLAN_SCHEMA}`); structured comment evidence not parsed"
            ),
            Self::CommentPlanMissingField(field) => {
                format!("comment-plan.json missing required field: {field}")
            }
            Self::CommentPlanInvalidField(field) => {
                format!("comment-plan.json invalid required field: {field}")
            }
        }
    }
}

#[derive(Deserialize)]
struct UnsafeReviewSchemaProbe {
    #[serde(default)]
    schema_version: Option<String>,
}

#[derive(Deserialize)]
struct UnsafeReviewCommentPlanEnvelope {
    #[serde(default)]
    schema_version: Option<String>,
    #[serde(default)]
    comments: Option<Vec<UnsafeReviewCommentPlanEntry>>,
}

fn required_comment_plan_field(
    entry: &UnsafeReviewCommentPlanEntry,
    index: usize,
) -> Result<(), UnsafeReviewIngestGap> {
    let prefix = format!("comments[{index}]");
    if entry.card_id.as_deref().is_none_or(str::is_empty) {
        return Err(UnsafeReviewIngestGap::CommentPlanMissingField(format!(
            "{prefix}.card_id"
        )));
    }
    if entry.path.as_deref().is_none_or(str::is_empty) {
        return Err(UnsafeReviewIngestGap::CommentPlanMissingField(format!(
            "{prefix}.path"
        )));
    }
    if entry.line.is_none() {
        return Err(UnsafeReviewIngestGap::CommentPlanMissingField(format!(
            "{prefix}.line"
        )));
    }
    if entry.changed_line.is_none() {
        return Err(UnsafeReviewIngestGap::CommentPlanMissingField(format!(
            "{prefix}.changed_line"
        )));
    }
    if entry
        .operation_family
        .as_deref()
        .is_none_or(|family| family.trim().is_empty())
    {
        return Err(UnsafeReviewIngestGap::CommentPlanInvalidField(format!(
            "{prefix}.operation_family must be nonempty"
        )));
    }
    if entry
        .trust_boundary
        .as_deref()
        .is_none_or(|boundary| boundary.trim().is_empty())
    {
        return Err(UnsafeReviewIngestGap::CommentPlanInvalidField(format!(
            "{prefix}.trust_boundary must be nonempty"
        )));
    }
    Ok(())
}

fn parse_comment_plan(
    text: &str,
) -> Result<Vec<UnsafeReviewCommentPlanEntry>, UnsafeReviewIngestGap> {
    let value: serde_json::Value = serde_json::from_str(text)
        .map_err(|err| UnsafeReviewIngestGap::CommentPlanMalformed(err.to_string()))?;
    if !value.is_object() {
        return Err(UnsafeReviewIngestGap::CommentPlanMalformed(
            "top-level value must be an object envelope".to_owned(),
        ));
    }
    let envelope: UnsafeReviewCommentPlanEnvelope = serde_json::from_value(value)
        .map_err(|err| UnsafeReviewIngestGap::CommentPlanMalformed(err.to_string()))?;
    let Some(found_version) = envelope.schema_version else {
        return Err(UnsafeReviewIngestGap::CommentPlanMissingField(
            "schema_version".to_owned(),
        ));
    };
    if found_version != UNSAFE_REVIEW_COMMENT_PLAN_SCHEMA {
        return Err(UnsafeReviewIngestGap::CommentPlanUnknownSchema(
            found_version,
        ));
    }
    let Some(comments) = envelope.comments else {
        return Err(UnsafeReviewIngestGap::CommentPlanMissingField(
            "comments".to_owned(),
        ));
    };
    for (index, entry) in comments.iter().enumerate() {
        required_comment_plan_field(entry, index)?;
    }
    Ok(comments)
}

fn confined_comment_plan_path(
    out_dir: &Path,
    pointer: &str,
) -> Result<PathBuf, UnsafeReviewIngestGap> {
    if pointer.trim().is_empty() {
        return Err(UnsafeReviewIngestGap::CommentPlanInvalidPointer(
            "pointer must be a nonempty relative path".to_owned(),
        ));
    }
    let relative = Path::new(pointer);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(UnsafeReviewIngestGap::CommentPlanInvalidPointer(format!(
            "pointer must contain only normal relative components: {pointer:?}"
        )));
    }
    let canonical_out_dir = fs::canonicalize(out_dir).map_err(|err| {
        UnsafeReviewIngestGap::CommentPlanUnreadable(format!(
            "output directory could not be canonicalized: {err}"
        ))
    })?;
    let canonical_target = fs::canonicalize(out_dir.join(relative)).map_err(|err| {
        UnsafeReviewIngestGap::CommentPlanUnreadable(format!(
            "pointer target could not be canonicalized: {err}"
        ))
    })?;
    if !canonical_target.starts_with(&canonical_out_dir) {
        return Err(UnsafeReviewIngestGap::CommentPlanInvalidPointer(format!(
            "canonical target escapes output directory: {pointer:?}"
        )));
    }
    Ok(canonical_target)
}

/// Parse the structured artifacts written by `unsafe-review first-pr --out-dir`.
///
/// Schema-routed before binding the typed v1 shape so an unknown version is
/// reported as an unknown version, not as a v1 parse failure.
pub(crate) fn read_unsafe_review_artifacts(
    sensor_dir: &Path,
) -> Result<UnsafeReviewArtifacts, UnsafeReviewIngestGap> {
    let out_dir = sensor_dir.join(UNSAFE_REVIEW_OUTPUT_SUBDIR);
    let gate_path = out_dir.join("unsafe-review-gate.json");
    let text = match fs::read_to_string(&gate_path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Err(UnsafeReviewIngestGap::Absent);
        }
        Err(err) => return Err(UnsafeReviewIngestGap::Unreadable(err.to_string())),
    };
    let probe: UnsafeReviewSchemaProbe = serde_json::from_str(&text)
        .map_err(|err| UnsafeReviewIngestGap::Malformed(err.to_string()))?;
    let Some(found_version) = probe.schema_version else {
        return Err(UnsafeReviewIngestGap::Malformed(
            "schema_version field missing".to_owned(),
        ));
    };
    if found_version != UNSAFE_REVIEW_GATE_SCHEMA {
        return Err(UnsafeReviewIngestGap::UnknownSchema(found_version));
    }
    let gate: UnsafeReviewGate = serde_json::from_str(&text)
        .map_err(|err| UnsafeReviewIngestGap::Malformed(err.to_string()))?;
    // Follow the required artifacts pointer for comment-plan.json.  The real
    // 0.3.x producer writes an object envelope, not the obsolete top-level
    // array.  Every read/parse/shape failure remains an explicit typed gap;
    // never turn a malformed plan into a valid-looking empty candidate set.
    let comment_plan_rel = gate
        .artifacts
        .get("comment_plan")
        .ok_or(UnsafeReviewIngestGap::CommentPlanMissingPointer)?;
    let comment_plan_path = confined_comment_plan_path(&out_dir, comment_plan_rel)?;
    let comment_plan_text = fs::read_to_string(&comment_plan_path)
        .map_err(|err| UnsafeReviewIngestGap::CommentPlanUnreadable(err.to_string()))?;
    let comment_plan = parse_comment_plan(&comment_plan_text)?;
    Ok(UnsafeReviewArtifacts { gate, comment_plan })
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
                "required_floor_wall_seconds": 12.5,
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict",
                "tool": "unsafe-review",
                "tool_version": "0.3.4"
            }"#,
        )?;
        // Real-shape 0.3.x comment-plan envelope: every field unsafe-review
        // actually emits is present, including the opaque operation family
        // and the fields #360 will route on.
        fs::write(
            out_dir.join("comment-plan.json"),
            r#"{
                "schema_version": "0.1",
                "tool": "unsafe-review",
                "mode": "plan_only",
                "policy": "advisory",
                "summary": {"selected_count": 1, "not_selected_count": 0, "budget": 3},
                "comments": [{
                    "card_id": "card-001",
                    "path": "src/lib.rs",
                    "line": 42,
                    "changed_line": true,
                    "coverage_gap": "raw pointer dereference without lifetime guard",
                    "selection_reason": "changed line in unsafe block",
                    "selection_reason_code": "changed-line-unsafe",
                    "confirmation_state": "unconfirmed",
                    "operation_family": "raw_pointer_read",
                    "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict"
                }],
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict"
            }"#,
        )?;

        let artifacts = super::read_unsafe_review_artifacts(&sensor_dir)
            .map_err(|gap| anyhow::anyhow!("expected ingested artifacts, got gap: {gap:?}"))?;
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
        assert_eq!(artifacts.gate.required_floor_wall_seconds, Some(12.5));
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
        assert_eq!(entry.operation_family.as_deref(), Some("raw_pointer_read"));
        assert_eq!(
            entry.trust_boundary.as_deref(),
            Some("static unsafe-review coverage evidence; not proof, not a merge verdict")
        );
        Ok(())
    }

    /// Unknown schema_version: ingestion must produce a gap naming the found
    /// version, not parse future output against the v1 shape.
    #[test]
    fn unsafe_review_artifacts_unknown_schema_is_gap_naming_found_version() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;
        // A future manifest whose `status` is no longer a string: if routing
        // ever parsed before checking the version, this would surface as a
        // misleading shape error instead of the unknown-version gap.
        let fixture_write = fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{
                "schema_version": "unsafe-review-gate/v2-future",
                "dialect": "unsafe-review",
                "status": {"word": "advisory", "code": 0},
                "tool": "unsafe-review",
                "tool_version": "0.4.0"
            }"#,
        );
        assert!(
            fixture_write.is_ok(),
            "write unknown-schema unsafe-review gate fixture: {fixture_write:?}"
        );
        let gap = match super::read_unsafe_review_artifacts(&sensor_dir) {
            Err(gap) => gap,
            Ok(_) => anyhow::bail!("expected UnknownSchema gap, got parsed artifacts"),
        };
        assert_eq!(
            gap,
            super::UnsafeReviewIngestGap::UnknownSchema("unsafe-review-gate/v2-future".to_owned())
        );
        assert_eq!(
            gap.reason(),
            "unsafe-review-gate.json schema_version `unsafe-review-gate/v2-future` not \
             recognised (known: `unsafe-review-gate/v1`); structured evidence not parsed"
        );
        Ok(())
    }

    /// Absent gate file: returns a typed gap.
    #[test]
    fn unsafe_review_artifacts_absent_gate_is_gap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let gap = match super::read_unsafe_review_artifacts(&sensor_dir) {
            Err(gap) => gap,
            Ok(_) => anyhow::bail!("expected Absent gap, got parsed artifacts"),
        };
        assert_eq!(gap, super::UnsafeReviewIngestGap::Absent);
        assert_eq!(
            gap.reason(),
            "unsafe-review-gate.json absent; structured evidence unavailable"
        );
        Ok(())
    }

    /// Missing schema_version: valid JSON still cannot be trusted as a routed
    /// unsafe-review gate artifact.
    #[test]
    fn read_unsafe_review_artifacts_missing_schema_version_returns_malformed_gap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;
        let fixture_write = fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{
                "dialect": "unsafe-review",
                "status": "advisory",
                "summary": {
                    "new_gaps": 0,
                    "worsened_gaps": 0,
                    "resolved_gaps": 0,
                    "inherited_gaps": 0
                },
                "artifacts": {},
                "tool": "unsafe-review",
                "tool_version": "0.3.4"
            }"#,
        );
        assert!(
            fixture_write.is_ok(),
            "write missing-schema unsafe-review gate fixture: {fixture_write:?}"
        );
        let parsed = super::read_unsafe_review_artifacts(&sensor_dir);
        assert!(matches!(
            &parsed,
            Err(super::UnsafeReviewIngestGap::Malformed(detail))
                if detail == "schema_version field missing"
        ));
        let gap = match parsed {
            Err(gap) => gap,
            Ok(_) => anyhow::bail!("expected Malformed gap, got parsed artifacts"),
        };
        assert_eq!(
            gap,
            super::UnsafeReviewIngestGap::Malformed("schema_version field missing".to_owned())
        );
        assert_eq!(
            gap.reason(),
            "unsafe-review-gate.json malformed: schema_version field missing"
        );
        Ok(())
    }

    /// Malformed JSON: a typed gap carries the parse detail.
    #[test]
    fn unsafe_review_artifacts_malformed_json_is_gap_with_reason() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;
        let fixture_write = fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{"schema_version": "unsafe-review-gate/v1", "status":"#,
        );
        assert!(
            fixture_write.is_ok(),
            "write malformed unsafe-review gate fixture: {fixture_write:?}"
        );
        let gap = match super::read_unsafe_review_artifacts(&sensor_dir) {
            Err(gap) => gap,
            Ok(_) => anyhow::bail!("expected Malformed gap, got parsed artifacts"),
        };
        let super::UnsafeReviewIngestGap::Malformed(detail) = &gap else {
            anyhow::bail!("expected Malformed, got {gap:?}");
        };
        assert!(!detail.is_empty(), "parse detail must be carried");
        assert!(
            gap.reason()
                .starts_with("unsafe-review-gate.json malformed: "),
            "gap reason should include malformed prefix: {}",
            gap.reason()
        );
        Ok(())
    }

    fn write_comment_plan_gate(sensor_dir: &Path, artifacts: &str) -> Result<std::path::PathBuf> {
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;
        fs::write(
            out_dir.join("unsafe-review-gate.json"),
            format!(
                r#"{{"schema_version":"unsafe-review-gate/v1","status":"advisory","artifacts":{artifacts}}}"#
            ),
        )?;
        Ok(out_dir)
    }

    #[test]
    fn comment_plan_envelope_accepts_additive_fields_and_retains_opaque_identity() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir =
            write_comment_plan_gate(&sensor_dir, r#"{"comment_plan":"comment-plan.json"}"#)?;
        fs::write(
            out_dir.join("comment-plan.json"),
            r#"{
                "schema_version":"0.1",
                "tool":"unsafe-review",
                "mode":"plan_only",
                "comments":[{
                    "card_id":"card-opaque",
                    "path":"src/lib.rs",
                    "line":8,
                    "changed_line":true,
                    "operation_family":"future_family_v9",
                    "trust_boundary":"producer advisory boundary",
                    "future_field":{"kept":true}
                }],
                "future_envelope_field":"accepted"
            }"#,
        )?;
        let artifacts = super::read_unsafe_review_artifacts(&sensor_dir)
            .map_err(|gap| anyhow::anyhow!("expected valid envelope, got {gap:?}"))?;
        assert_eq!(artifacts.comment_plan.len(), 1);
        assert_eq!(
            artifacts.comment_plan[0].operation_family.as_deref(),
            Some("future_family_v9")
        );
        assert_eq!(
            artifacts.comment_plan[0].trust_boundary.as_deref(),
            Some("producer advisory boundary")
        );
        Ok(())
    }

    #[test]
    fn comment_plan_wrong_top_level_type_is_explicit_gap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir =
            write_comment_plan_gate(&sensor_dir, r#"{"comment_plan":"comment-plan.json"}"#)?;
        fs::write(out_dir.join("comment-plan.json"), "[]")?;
        let gap = super::read_unsafe_review_artifacts(&sensor_dir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("array unexpectedly parsed as an envelope"))?;
        assert!(
            matches!(gap, super::UnsafeReviewIngestGap::CommentPlanMalformed(_)),
            "unexpected gap: {gap:?}"
        );
        assert!(gap.reason().contains("comment-plan.json malformed"));
        Ok(())
    }

    #[test]
    fn comment_plan_missing_comments_is_explicit_field_gap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir =
            write_comment_plan_gate(&sensor_dir, r#"{"comment_plan":"comment-plan.json"}"#)?;
        fs::write(
            out_dir.join("comment-plan.json"),
            r#"{"schema_version":"0.1","mode":"plan_only"}"#,
        )?;
        let gap = super::read_unsafe_review_artifacts(&sensor_dir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("missing comments unexpectedly parsed"))?;
        assert_eq!(
            gap,
            super::UnsafeReviewIngestGap::CommentPlanMissingField("comments".to_owned())
        );
        Ok(())
    }

    #[test]
    fn comment_plan_malformed_entry_is_explicit_field_gap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir =
            write_comment_plan_gate(&sensor_dir, r#"{"comment_plan":"comment-plan.json"}"#)?;
        fs::write(
            out_dir.join("comment-plan.json"),
            r#"{"schema_version":"0.1","comments":[{"path":"src/lib.rs","line":8,"changed_line":true,"operation_family":"raw_pointer_read","trust_boundary":"advisory"}]}"#,
        )?;
        let gap = super::read_unsafe_review_artifacts(&sensor_dir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("malformed entry unexpectedly parsed"))?;
        assert_eq!(
            gap,
            super::UnsafeReviewIngestGap::CommentPlanMissingField("comments[0].card_id".to_owned())
        );
        Ok(())
    }

    #[test]
    fn comment_plan_pointer_missing_or_file_missing_is_explicit_gap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let _out_dir = write_comment_plan_gate(&sensor_dir, "{}")?;
        let gap = super::read_unsafe_review_artifacts(&sensor_dir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("missing pointer unexpectedly parsed"))?;
        assert_eq!(gap, super::UnsafeReviewIngestGap::CommentPlanMissingPointer);

        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let _out_dir =
            write_comment_plan_gate(&sensor_dir, r#"{"comment_plan":"comment-plan.json"}"#)?;
        let gap = super::read_unsafe_review_artifacts(&sensor_dir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("missing file unexpectedly parsed"))?;
        assert!(matches!(
            gap,
            super::UnsafeReviewIngestGap::CommentPlanUnreadable(_)
        ));
        Ok(())
    }

    #[test]
    fn comment_plan_parent_relative_pointer_is_rejected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = write_comment_plan_gate(
            &sensor_dir,
            r#"{"comment_plan":"../outside-comment-plan.json"}"#,
        )?;
        fs::write(
            out_dir
                .parent()
                .ok_or_else(|| anyhow::anyhow!("sensor output parent missing"))?
                .join("outside-comment-plan.json"),
            r#"{"schema_version":"0.1","comments":[]}"#,
        )?;
        let gap = super::read_unsafe_review_artifacts(&sensor_dir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("parent-relative pointer unexpectedly parsed"))?;
        assert!(matches!(
            gap,
            super::UnsafeReviewIngestGap::CommentPlanInvalidPointer(_)
        ));
        Ok(())
    }

    #[test]
    fn comment_plan_absolute_pointer_is_rejected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = write_comment_plan_gate(&sensor_dir, "{}")?;
        let outside = temp.path().join("absolute-comment-plan.json");
        fs::write(&outside, r#"{"schema_version":"0.1","comments":[]}"#)?;
        fs::write(
            out_dir.join("unsafe-review-gate.json"),
            format!(
                r#"{{"schema_version":"unsafe-review-gate/v1","status":"advisory","artifacts":{{"comment_plan":{}}}}}"#,
                serde_json::to_string(&outside.to_string_lossy())?
            ),
        )?;
        let gap = super::read_unsafe_review_artifacts(&sensor_dir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("absolute pointer unexpectedly parsed"))?;
        assert!(matches!(
            gap,
            super::UnsafeReviewIngestGap::CommentPlanInvalidPointer(_)
        ));
        Ok(())
    }

    #[test]
    fn comment_plan_symlink_redirect_is_rejected() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = write_comment_plan_gate(&sensor_dir, r#"{"comment_plan":"redirect.json"}"#)?;
        let outside = temp.path().join("symlink-target-comment-plan.json");
        fs::write(&outside, r#"{"schema_version":"0.1","comments":[]}"#)?;
        let redirect = out_dir.join("redirect.json");
        let symlink_result = {
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(&outside, &redirect)
            }
            #[cfg(windows)]
            {
                std::os::windows::fs::symlink_file(&outside, &redirect)
            }
        };
        if let Err(err) = symlink_result {
            // Windows without symlink privilege (no Developer Mode) reports
            // ERROR_PRIVILEGE_NOT_HELD, which does not surface as
            // `PermissionDenied` on every toolchain; skip by raw code too.
            if err.kind() == std::io::ErrorKind::PermissionDenied
                || err.raw_os_error() == Some(1314)
            {
                return Ok(());
            }
            return Err(err.into());
        }
        let gap = super::read_unsafe_review_artifacts(&sensor_dir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("symlink pointer unexpectedly parsed"))?;
        assert!(matches!(
            gap,
            super::UnsafeReviewIngestGap::CommentPlanInvalidPointer(_)
        ));
        Ok(())
    }

    #[test]
    fn comment_plan_empty_operation_family_is_explicit_invalid_gap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir =
            write_comment_plan_gate(&sensor_dir, r#"{"comment_plan":"comment-plan.json"}"#)?;
        fs::write(
            out_dir.join("comment-plan.json"),
            r#"{"schema_version":"0.1","comments":[{"card_id":"card-1","path":"src/lib.rs","line":8,"changed_line":true,"operation_family":"  ","trust_boundary":"advisory"}]}"#,
        )?;
        let gap = super::read_unsafe_review_artifacts(&sensor_dir)
            .err()
            .ok_or_else(|| anyhow::anyhow!("empty family unexpectedly parsed"))?;
        assert_eq!(
            gap,
            super::UnsafeReviewIngestGap::CommentPlanInvalidField(
                "comments[0].operation_family must be nonempty".to_owned()
            )
        );
        Ok(())
    }
}
