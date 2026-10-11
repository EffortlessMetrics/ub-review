//! #1014 pure shadow recorded public-plan contract, dependent on #1013.
//! Validate a recorded selection-to-surface mapping; never choose a verdict,
//! render lane prose, invoke a model, or prepare/post a production payload.
//! Public-content admission/classification and delivery-subject authority are
//! independently pinned caller obligations, not permissions minted here.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "policy/allow.toml#recorded-public-plan-shadow-1014"
    )
)]

use std::collections::BTreeSet;
use std::io::{self, Write};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::{
    JudgmentSubject, RecordedJudgmentSource, RecordedLead, ReviewResult, validate_recorded_judgment,
};
use crate::delivery_transaction::{DeliveryAction, DeliveryLocation, PlannedDelivery};
use crate::revision_identity::{RevisionIdentity, RevisionRef};
use crate::sha256_hex;

const SCHEMA: &str = "ub-review.recorded-public-plan.v1";
const MAX_OBJECT_BYTES: usize = 256 * 1024;
const MAX_SOURCE_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 128;
const MAX_BODY_BYTES: usize = 6000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct DeliverySubject {
    pub(crate) repository: String,
    pub(crate) pull_number: u64,
    pub(crate) head_sha: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MaterialClass {
    Public,
    PrivateAudit,
    Operational,
}

/// Already admitted exact public wording or a private/operational withholding
/// source. Raw bodies stay behind references, including in plan serialization.
#[derive(Serialize)]
pub(crate) struct RecordedMaterial<'a> {
    pub(crate) claim_id: &'a str,
    pub(crate) body_ref: &'a str,
    pub(crate) body_digest: &'a str,
    pub(crate) class: MaterialClass,
    #[serde(skip)]
    pub(crate) body: &'a [u8],
}

/// RIGHT-side current diff anchor. Merge-result-to-candidate mappings and reply
/// thread admission are intentionally unsupported in this recorded v1 subset.
#[derive(Serialize)]
pub(crate) struct RecordedAnchor<'a> {
    pub(crate) reference: &'a str,
    pub(crate) claim_id: &'a str,
    pub(crate) path: &'a str,
    pub(crate) line: u32,
    pub(crate) line_digest: &'a str,
}

#[derive(Serialize)]
pub(crate) struct RecordedPublicSource<'a> {
    pub(crate) delivery_subject: &'a DeliverySubject,
    pub(crate) diff_digest: &'a str,
    #[serde(skip)]
    pub(crate) diff_patch: &'a [u8],
    pub(crate) materials: &'a [RecordedMaterial<'a>],
    pub(crate) anchors: &'a [RecordedAnchor<'a>],
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WithholdingReason {
    PrivateAudit,
    Operational,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum PublicDisposition {
    Summary {
        body_ref: String,
        body_digest: String,
    },
    Inline {
        body_ref: String,
        anchor_ref: String,
        delivery: PlannedDelivery,
    },
    // No arbitrary reason prose or absent-anchor excuse: public findings must
    // survive as summary rather than disappear because an inline slot is unsafe.
    Withheld {
        reason: WithholdingReason,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublicItem {
    pub(crate) claim_id: String,
    pub(crate) disposition: PublicDisposition,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct PublicReviewPlan {
    schema: String,
    judgment_digest: String,
    revision: RevisionRef,
    subject: JudgmentSubject,
    delivery_subject: DeliverySubject,
    public_source_digest: String,
    /// Deliberate silence is permitted only for an explicitly validated clean
    /// judgment. Empty limited/not-proven plans cannot declare quiet clean.
    quiet: bool,
    items: Vec<PublicItem>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(tag = "availability", rename_all = "snake_case")]
pub(crate) enum RecordedPublicPlan {
    LeadAbsent,
    LeadInvalid,
    PlanAbsent,
    /// Shadow consistency only; no CI/public/delivery authority.
    Validated {
        plan: Box<PublicReviewPlan>,
        plan_digest: String,
    },
}

pub(crate) struct PublicPlanInput<'a> {
    pub(crate) judgment_source: &'a RecordedJudgmentSource<'a>,
    pub(crate) current_revision: &'a RevisionIdentity,
    pub(crate) expected_source_digest: &'a str,
    pub(crate) answer: Option<&'a [u8]>,
    pub(crate) expected_answer_digest: Option<&'a str>,
    pub(crate) observed_ci: Option<&'a [u8]>,
    pub(crate) public_source: &'a RecordedPublicSource<'a>,
    pub(crate) expected_public_source_digest: &'a str,
    pub(crate) plan: Option<&'a [u8]>,
    pub(crate) expected_plan_digest: Option<&'a str>,
}

/// Bound serialization without allocating an unbounded intermediate. Caller
/// still owns admission/parsing and memory limits for its borrowed source data.
struct BoundedBytes(Vec<u8>);
impl Write for BoundedBytes {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_SOURCE_BYTES.saturating_sub(self.0.len()) {
            return Err(io::Error::other("recorded source exceeds byte limit"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn bounded_json(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut output = BoundedBytes(Vec::new());
    serde_json::to_writer(&mut output, value)?;
    Ok(output.0)
}

/// Domain-separated immutable source digest; actual body/diff/anchor bytes are
/// independently checked by the real validator, not compared as producer strings.
pub(crate) fn public_source_digest(source: &RecordedPublicSource<'_>) -> Result<String> {
    let mut bytes = b"ub-review.recorded-public-source.v1\0".to_vec();
    bytes.extend(bounded_json(source)?);
    Ok(sha256_hex(&bytes))
}

fn identifier(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 256
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_./#:-".contains(&b)),
        "invalid public identifier"
    );
    Ok(())
}

/// One executable value entrypoint. It reuses the unchanged #1387 validator,
/// recomputes source/answer/plan identities, checks every selected claim exactly
/// once, and keeps imported CI immutable through the entire boundary.
pub(crate) fn validate_recorded_public_plan(
    input: PublicPlanInput<'_>,
) -> Result<RecordedPublicPlan> {
    ensure!(
        input.answer.is_none_or(|b| b.len() <= MAX_OBJECT_BYTES)
            && input.plan.is_none_or(|b| b.len() <= MAX_OBJECT_BYTES),
        "recorded object exceeds byte limit"
    );
    ensure!(
        input
            .observed_ci
            .is_none_or(|b| b.len() <= MAX_SOURCE_BYTES),
        "CI observation exceeds byte limit"
    );
    let source = input.judgment_source;
    ensure!(
        source.graph.claims.len() <= MAX_ROWS
            && source.graph.topics.len() <= MAX_ROWS
            && source.evidence.len() <= MAX_ROWS,
        "judgment source exceeds row limit"
    );
    bounded_json(source)?;
    ensure!(
        source
            .evidence
            .iter()
            .try_fold(0usize, |sum, e| sum.checked_add(e.bytes.len()))
            .is_some_and(|sum| sum <= MAX_SOURCE_BYTES)
            && source
                .imported_ci
                .as_ref()
                .is_none_or(|ci| ci.bytes.len() <= MAX_SOURCE_BYTES),
        "recorded evidence exceeds byte limit"
    );
    let lead = validate_recorded_judgment(
        source,
        input.current_revision,
        input.expected_source_digest,
        input.answer,
        input.expected_answer_digest,
        input.observed_ci,
    )?;
    let (judgment, answer_digest) = match lead {
        RecordedLead::Absent => {
            ensure!(
                input.plan.is_none() && input.expected_plan_digest.is_none(),
                "plan without recorded lead"
            );
            return Ok(RecordedPublicPlan::LeadAbsent);
        }
        RecordedLead::Invalid { .. } => {
            ensure!(
                input.plan.is_none() && input.expected_plan_digest.is_none(),
                "plan from invalid lead"
            );
            return Ok(RecordedPublicPlan::LeadInvalid);
        }
        RecordedLead::Validated {
            judgment,
            answer_digest,
        } => (judgment, answer_digest),
    };
    let Some(plan_bytes) = input.plan else {
        ensure!(input.expected_plan_digest.is_none(), "pinned plan missing");
        return Ok(RecordedPublicPlan::PlanAbsent);
    };
    ensure!(
        Some(sha256_hex(plan_bytes).as_str()) == input.expected_plan_digest,
        "recorded plan bytes mismatch"
    );
    // Legacy RevisionRef and RawPlannedDelivery tolerate extras. Admit their
    // fields strictly here before shared deserialization can discard them;
    // preserve the existing identity/transport decoders' behavior.
    let value: serde_json::Value = serde_json::from_slice(plan_bytes)?;
    ensure!(
        value
            .get("revision")
            .and_then(serde_json::Value::as_object)
            .is_some_and(|m| m.len() == 3
                && m.keys()
                    .all(|k| matches!(k.as_str(), "digest" | "semantics" | "reviewed_commit"))),
        "unknown plan revision field"
    );
    if let Some(items) = value.get("items").and_then(serde_json::Value::as_array) {
        for item in items {
            if let Some(delivery) = item.pointer("/disposition/delivery") {
                ensure!(
                    delivery
                        .as_object()
                        .is_some_and(|fields| fields.keys().all(|key| {
                            matches!(
                                key.as_str(),
                                "exact_head_sha"
                                    | "claim_id"
                                    | "action"
                                    | "path"
                                    | "line"
                                    | "side"
                                    | "source_thread_id"
                                    | "expected_body_digest"
                            )
                        })),
                    "unknown plan delivery field"
                );
            }
        }
    }
    let plan: PublicReviewPlan = serde_json::from_slice(plan_bytes)?;
    let public = input.public_source;
    ensure!(
        public.materials.len() <= MAX_ROWS
            && public.anchors.len() <= MAX_ROWS
            && plan.items.len() <= MAX_ROWS
            && public.diff_patch.len() <= MAX_SOURCE_BYTES,
        "public source exceeds limit"
    );
    ensure!(
        public_source_digest(public)? == input.expected_public_source_digest,
        "public source digest mismatch"
    );
    ensure!(
        plan.schema == SCHEMA
            && plan.judgment_digest == answer_digest
            && plan.revision == input.current_revision.revision_ref()
            && plan.subject == judgment.subject
            && plan.public_source_digest == input.expected_public_source_digest
            && plan.delivery_subject == *public.delivery_subject,
        "public plan identity mismatch"
    );
    ensure!(
        crate::is_valid_repo_slug(&public.delivery_subject.repository)
            && public.delivery_subject.repository == source.subject.repository
            && public.delivery_subject.pull_number > 0
            && public.delivery_subject.head_sha == input.current_revision.candidate_head_commit(),
        "invalid delivery subject"
    );
    ensure!(
        input.current_revision.matches_diff_bytes(public.diff_patch)
            && RevisionIdentity::diff_digest(public.diff_patch) == public.diff_digest,
        "recorded diff mismatch"
    );
    let patch = std::str::from_utf8(public.diff_patch).context("recorded diff is not UTF-8")?;
    let lines = crate::right_side_diff_line_text(patch);
    let mut material_ids = BTreeSet::new();
    let mut body_refs = BTreeSet::new();
    let mut total_body_bytes = 0usize;
    for material in public.materials {
        identifier(material.claim_id)?;
        identifier(material.body_ref)?;
        ensure!(
            source
                .graph
                .claims
                .iter()
                .any(|claim| claim.id == material.claim_id),
            "public material has unknown claim"
        );
        ensure!(
            material_ids.insert(material.claim_id) && body_refs.insert(material.body_ref),
            "duplicate public material"
        );
        ensure!(
            material.body.len() <= MAX_BODY_BYTES
                && sha256_hex(material.body) == material.body_digest,
            "public body bytes mismatch or oversize"
        );
        total_body_bytes += material.body.len();
        ensure!(
            total_body_bytes <= MAX_SOURCE_BYTES,
            "public bodies exceed byte limit"
        );
    }
    let mut anchor_refs = BTreeSet::new();
    for anchor in public.anchors {
        identifier(anchor.reference)?;
        identifier(anchor.claim_id)?;
        ensure!(
            material_ids.contains(anchor.claim_id),
            "anchor has unknown material"
        );
        ensure!(
            anchor_refs.insert(anchor.reference) && crate::is_repo_relative_path(anchor.path),
            "invalid or duplicate anchor"
        );
        let line = lines
            .get(&(anchor.path.to_owned(), anchor.line))
            .ok_or_else(|| anyhow::anyhow!("anchor absent from recorded diff"))?;
        ensure!(
            sha256_hex(line.as_bytes()) == anchor.line_digest,
            "anchor text digest mismatch"
        );
    }
    ensure!(
        !plan.quiet
            || (judgment.review_result == ReviewResult::Clean
                && judgment.selections.is_empty()
                && plan.items.is_empty()),
        "unsupported deliberate silence"
    );
    ensure!(
        judgment.review_result != ReviewResult::Clean || plan.quiet,
        "clean plan must retain deliberate silence"
    );
    let mut represented = BTreeSet::new();
    let mut locations = BTreeSet::new();
    for item in &plan.items {
        identifier(&item.claim_id)?;
        ensure!(
            represented.insert(item.claim_id.as_str())
                && judgment
                    .selections
                    .iter()
                    .any(|s| s.claim_id == item.claim_id),
            "duplicate or unselected public claim"
        );
        let material = public
            .materials
            .iter()
            .find(|m| m.claim_id == item.claim_id)
            .ok_or_else(|| anyhow::anyhow!("selected public material absent"))?;
        match &item.disposition {
            PublicDisposition::Withheld { reason } => {
                ensure!(
                    matches!(
                        (reason, material.class),
                        (WithholdingReason::PrivateAudit, MaterialClass::PrivateAudit)
                            | (WithholdingReason::Operational, MaterialClass::Operational)
                    ),
                    "unqualified withholding"
                );
            }
            disposition => {
                ensure!(
                    material.class == MaterialClass::Public,
                    "private material in public plan"
                );
                let body =
                    std::str::from_utf8(material.body).context("public body is not UTF-8")?;
                ensure!(
                    !body.trim().is_empty()
                        && crate::redact_shared_context_secret_assignments(body) == body,
                    "public body needs redaction"
                );
                crate::validate_pr_review_body_policy(body, &crate::ReviewBodyPolicy::default())?;
                match disposition {
                    PublicDisposition::Summary {
                        body_ref,
                        body_digest,
                    } => ensure!(
                        body_ref == material.body_ref && body_digest == material.body_digest,
                        "summary body identity mismatch"
                    ),
                    PublicDisposition::Inline {
                        body_ref,
                        anchor_ref,
                        delivery,
                    } => {
                        ensure!(
                            body_ref == material.body_ref
                                && delivery.claim_id() == item.claim_id
                                && delivery.expected_body_digest() == material.body_digest
                                && delivery.action() == DeliveryAction::Inline
                                && body.chars().count() <= crate::INLINE_COMMENT_MAX_REVIEWER_CHARS,
                            "inline content identity mismatch"
                        );
                        ensure!(
                            plan.revision.reviewed_commit == plan.delivery_subject.head_sha,
                            "merge anchor needs candidate crosswalk"
                        );
                        let anchor = public
                            .anchors
                            .iter()
                            .find(|a| a.reference == anchor_ref && a.claim_id == item.claim_id)
                            .ok_or_else(|| anyhow::anyhow!("unknown claim anchor"))?;
                        let (path, line, side) = delivery.location();
                        ensure!(
                            path == anchor.path
                                && line == anchor.line
                                && side == "RIGHT"
                                && locations.insert((path, line, side)),
                            "invalid or duplicate public location"
                        );
                        let expected = PlannedDelivery::new(
                            &plan.delivery_subject.head_sha,
                            &item.claim_id,
                            DeliveryAction::Inline,
                            DeliveryLocation::new(path, line, side),
                            None,
                            material.body_digest,
                        )?;
                        ensure!(*delivery == expected, "inline delivery subject mismatch");
                    }
                    PublicDisposition::Withheld { .. } => {}
                }
            }
        }
    }
    ensure!(
        represented.len() == judgment.selections.len(),
        "lead-selected claim lost from public plan"
    );
    Ok(RecordedPublicPlan::Validated {
        plan: Box::new(plan),
        plan_digest: sha256_hex(plan_bytes),
    })
}

#[cfg(test)]
mod tests;
