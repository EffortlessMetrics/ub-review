//! #1014 direct slot binding of an already admitted recorded public plan to
//! existing GitHubReview bytes. No rendering or prose/claim inference occurs.
//! One Summary material may own the whole body slot; fragmented/multiple-summary
//! joins need compiler-owned provenance and are deliberately unsupported here.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "policy/allow.toml#recorded-prepared-payload-shadow-1014"
    )
)]

use anyhow::{Result, ensure};
use serde::Serialize;
use std::collections::BTreeSet;

use super::{
    DeliverySubject, JudgmentSubject, PublicDisposition, PublicPlanInput, RecordedPublicPlan,
    bounded_json, validate_recorded_public_plan,
};
use crate::{GitHubReview, RevisionRef, sha256_hex};

/// Caller retains this source identity independently of the payload it names.
/// A hash proves recorded consistency, never compiler origin or posting.
#[derive(Serialize)]
pub(crate) struct RecordedPreparedPayload<'a> {
    pub(crate) subject: &'a JudgmentSubject,
    pub(crate) delivery_subject: &'a DeliverySubject,
    pub(crate) revision: &'a RevisionRef,
    pub(crate) judgment_digest: &'a str,
    pub(crate) public_plan_digest: &'a str,
    pub(crate) payload_digest: &'a str,
    #[serde(skip)]
    pub(crate) bytes: &'a [u8],
}

pub(crate) fn payload_source_digest(source: &RecordedPreparedPayload<'_>) -> Result<String> {
    let mut bytes = b"ub-review.recorded-prepared-payload-source.v1\0".to_vec();
    bytes.extend(bounded_json(source)?);
    Ok(sha256_hex(&bytes))
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub(crate) struct PreparedPayloadBinding {
    schema: &'static str,
    qualification: &'static str,
    subject: JudgmentSubject,
    delivery_subject: DeliverySubject,
    revision: RevisionRef,
    judgment_digest: String,
    public_plan_digest: String,
    payload_digest: String,
    payload_source_digest: String,
    whole_body_claim: Option<String>,
    inline_claims: Vec<String>,
}

/// Exact whole-field subset only. Public wording is already admitted by the
/// source owner; this does not turn a reason fragment into final compiler prose
/// or prove which claim arbitrary prose means. No payload/source bytes change.
pub(crate) fn validate_prepared_payload(
    input: PublicPlanInput<'_>,
    payload: &RecordedPreparedPayload<'_>,
    expected_payload_source_digest: &str,
) -> Result<PreparedPayloadBinding> {
    ensure!(
        payload.bytes.len() <= 1024 * 1024,
        "prepared payload exceeds byte limit"
    );
    ensure!(
        payload_source_digest(payload)? == expected_payload_source_digest,
        "payload source pin mismatch"
    );
    ensure!(
        sha256_hex(payload.bytes) == payload.payload_digest,
        "payload byte pin mismatch"
    );
    let public = input.public_source;
    let RecordedPublicPlan::Validated { plan, plan_digest } = validate_recorded_public_plan(input)?
    else {
        anyhow::bail!("validated public plan required for prepared payload");
    };
    ensure!(!plan.quiet, "quiet plan must not carry a prepared payload");
    ensure!(
        payload.subject == &plan.subject
            && payload.delivery_subject == &plan.delivery_subject
            && payload.revision == &plan.revision
            && payload.judgment_digest == plan.judgment_digest
            && payload.public_plan_digest == plan_digest,
        "prepared payload subject/source mismatch"
    );
    // Existing payload decoder is permissive. Fence its fields before shared
    // deserialization; suggestions have no representation in this plan subset.
    let value: serde_json::Value = serde_json::from_slice(payload.bytes)?;
    ensure!(
        value.as_object().is_some_and(|fields| fields.len() == 3
            && fields
                .keys()
                .all(|k| matches!(k.as_str(), "event" | "body" | "comments"))),
        "unknown prepared payload field"
    );
    let comments = value
        .get("comments")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("prepared comments must be an array"))?;
    ensure!(comments.len() <= 128, "prepared comments exceed row limit");
    for comment in comments {
        ensure!(
            comment.as_object().is_some_and(|fields| fields
                .keys()
                .all(|k| matches!(k.as_str(), "path" | "line" | "side" | "body" | "suggestion"))),
            "unknown prepared comment field"
        );
        ensure!(
            comment
                .get("suggestion")
                .is_none_or(serde_json::Value::is_null),
            "prepared suggestions unsupported"
        );
    }
    let review: GitHubReview = serde_json::from_slice(payload.bytes)?;
    ensure!(
        review.event == "COMMENT",
        "unsupported prepared review event"
    );
    let mut whole_body_claim = None;
    let mut inline_claims = Vec::new();
    let mut used = BTreeSet::new();
    for item in &plan.items {
        match &item.disposition {
            PublicDisposition::Summary { body_ref, .. } => {
                ensure!(
                    whole_body_claim.is_none(),
                    "multiple summary fragments require compiler provenance"
                );
                let material = public
                    .materials
                    .iter()
                    .find(|m| m.claim_id == item.claim_id && m.body_ref == body_ref)
                    .ok_or_else(|| anyhow::anyhow!("summary material absent"))?;
                ensure!(
                    review.body.as_bytes() == material.body,
                    "whole summary body slot mismatch"
                );
                whole_body_claim = Some(item.claim_id.clone());
            }
            PublicDisposition::Inline {
                body_ref, delivery, ..
            } => {
                let material = public
                    .materials
                    .iter()
                    .find(|m| m.claim_id == item.claim_id && m.body_ref == body_ref)
                    .ok_or_else(|| anyhow::anyhow!("inline material absent"))?;
                let (path, line, side) = delivery.location();
                let matches = review
                    .comments
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| {
                        c.path == path
                            && c.line == line
                            && c.side == side
                            && c.body.as_bytes() == material.body
                    })
                    .map(|(i, _)| i)
                    .collect::<Vec<_>>();
                ensure!(
                    matches.len() == 1 && used.insert(matches[0]),
                    "inline payload slot mismatch"
                );
                inline_claims.push(item.claim_id.clone());
            }
            PublicDisposition::Withheld { .. } => {}
        }
    }
    ensure!(
        whole_body_claim.is_some() || review.body.is_empty(),
        "unselected prepared body slot"
    );
    ensure!(
        used.len() == review.comments.len(),
        "extra prepared comment slot"
    );
    ensure!(
        whole_body_claim.is_some() || !inline_claims.is_empty(),
        "prepared payload requires a bound public slot"
    );
    Ok(PreparedPayloadBinding {
        schema: "ub-review.recorded-prepared-payload-binding.v1",
        qualification: "shadow_direct_slots_only",
        subject: plan.subject.clone(),
        delivery_subject: plan.delivery_subject.clone(),
        revision: plan.revision.clone(),
        judgment_digest: plan.judgment_digest.clone(),
        public_plan_digest: plan_digest,
        payload_digest: payload.payload_digest.to_owned(),
        payload_source_digest: expected_payload_source_digest.to_owned(),
        whole_body_claim,
        inline_claims,
    })
}
