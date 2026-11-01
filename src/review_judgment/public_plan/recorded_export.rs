//! #1335's independent recorded source export/replay portion.
//! This API consumes admitted typed source objects, not an arbitrary directory,
//! Markdown, a transport transaction, or a model. It is not the final command
//! or AgentReviewResult: delivery-event admission and its watermark are absent.
pub(crate) mod files;

use anyhow::{Result, ensure};
use serde::Serialize;

use super::{PublicPlanInput, RecordedPublicPlan, validate_recorded_public_plan};
use crate::review_judgment::{RecordedLead, ReviewJudgment, validate_recorded_judgment};
use crate::sha256_hex;

/// Local machine export only. CI context remains a separate opaque byte object,
/// never public prose or a computed gate result. No public publisher consumes it.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RecordedReviewExport {
    pub(crate) view_json: Vec<u8>,
    pub(crate) imported_ci_bytes: Option<Vec<u8>>,
}

#[derive(Serialize)]
#[serde(tag = "availability", rename_all = "snake_case")]
enum JudgmentView<'a> {
    Absent,
    Invalid,
    Validated {
        judgment: &'a ReviewJudgment,
        answer_digest: &'a str,
        // The digest names the original wire bytes, not typed reserialization.
        recorded_bytes: &'a [u8],
    },
}

#[derive(Serialize)]
struct SourceExport<'a> {
    schema: &'static str,
    qualification: &'static str,
    source_manifest_digest: &'a str,
    source_review_complete: bool,
    current_revision: crate::RevisionRef,
    source_currentness: &'static str,
    judgment: JudgmentView<'a>,
    public_plan: &'a RecordedPublicPlan,
    public_plan_recorded_bytes: Option<&'a [u8]>,
    imported_ci_digest: Option<&'a str>,
    // Explicit missingness; no fabricated zero-event watermark or receipt.
    delivery_projection: &'static str,
    observation_watermark: Option<&'a str>,
    remote_effects: &'static str,
}

/// Execute both canonical validators before emitting any output. Moved sources
/// fail rather than being relabeled historical/current. Caller-owned source pins
/// and admission are required, just as at the producer boundaries. Public prose,
/// private material and evidence bodies are not copied into the view.
pub(crate) fn export_recorded_review(input: PublicPlanInput<'_>) -> Result<RecordedReviewExport> {
    // Keep the references for a second independent judgment check and exact-wire
    // preservation. The public-plan validator also checks admission size bounds.
    let source = input.judgment_source;
    let current_revision = input.current_revision;
    let source_digest = input.expected_source_digest;
    let answer = input.answer;
    let answer_digest = input.expected_answer_digest;
    let observed_ci = input.observed_ci;
    let plan_bytes = input.plan;
    let public_plan = validate_recorded_public_plan(input)?;
    let lead = validate_recorded_judgment(
        source,
        current_revision,
        source_digest,
        answer,
        answer_digest,
        observed_ci,
    )?;
    let judgment = match &lead {
        RecordedLead::Absent => JudgmentView::Absent,
        RecordedLead::Invalid { .. } => JudgmentView::Invalid,
        RecordedLead::Validated {
            judgment,
            answer_digest,
        } => JudgmentView::Validated {
            judgment,
            answer_digest,
            recorded_bytes: answer.ok_or_else(|| anyhow::anyhow!("validated lead lacks bytes"))?,
        },
    };
    let view = SourceExport {
        schema: "ub-review.recorded-source-export.v1",
        qualification: "shadow_recorded_only",
        source_manifest_digest: source_digest,
        source_review_complete: source.review_complete,
        current_revision: current_revision.revision_ref(),
        source_currentness: "matches_admitted_revision",
        judgment,
        public_plan_recorded_bytes: match &public_plan {
            RecordedPublicPlan::Validated { .. } => plan_bytes,
            _ => None,
        },
        public_plan: &public_plan,
        imported_ci_digest: source.imported_ci.as_ref().map(|ci| ci.digest),
        delivery_projection: "not_admitted",
        observation_watermark: None,
        remote_effects: "unknown",
    };
    let mut view_json = serde_json::to_vec(&view)?;
    view_json.push(b'\n');
    Ok(RecordedReviewExport {
        view_json,
        imported_ci_bytes: source.imported_ci.as_ref().map(|ci| ci.bytes.to_vec()),
    })
}

/// Re-execute source validation and compare the exact derived view plus opaque
/// CI bytes. A repinned altered view still fails; never trust its status, digests,
/// or a caller-supplied watermark. No delivery retry history is admitted here.
pub(crate) fn replay_recorded_review_export(
    input: PublicPlanInput<'_>,
    recorded: &RecordedReviewExport,
    expected_view_digest: &str,
) -> Result<()> {
    ensure!(
        recorded.view_json.len() <= 4 * 1024 * 1024
            && recorded
                .imported_ci_bytes
                .as_ref()
                .is_none_or(|ci| ci.len() <= 1024 * 1024),
        "recorded export exceeds byte limit"
    );
    ensure!(
        sha256_hex(&recorded.view_json) == expected_view_digest,
        "export byte pin mismatch"
    );
    let derived = export_recorded_review(input)?;
    ensure!(
        derived.view_json == recorded.view_json,
        "export replay mismatch"
    );
    ensure!(
        derived.imported_ci_bytes == recorded.imported_ci_bytes,
        "imported CI replay mismatch"
    );
    Ok(())
}
