//! #1013's shadow recorded-answer/source-object boundary, for a later #1335
//! consumer. This is selection validation, not semantic adjudication, a final
//! lead call, public compilation, or qualification of upstream evidence.
//! Callers supply independently pinned, already admitted source objects. Pins
//! must never be learned from the answer being validated. No I/O lives here.
#![cfg_attr(
    not(test),
    expect(dead_code, reason = "policy/allow.toml#recorded-judgment-shadow-1013")
)]

use std::collections::BTreeSet;

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use crate::claim_graph::{ClaimGraph, ClaimState, EvidenceClass};
use crate::revision_identity::{RevisionIdentity, RevisionRef};
use crate::sha256_hex;

const SCHEMA: &str = "ub-review.recorded-judgment.v1";

/// Repository/spec/pass/attempt are not part of RevisionIdentity. Bind them
/// separately so an older clean pass cannot replace a fresh same-head pass.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct JudgmentSubject {
    pub(crate) repository: String,
    pub(crate) review_spec_digest: String,
    pub(crate) pass_id: String,
    pub(crate) attempt_id: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewResult {
    Findings,
    Clean,
    Limited,
    NotProven,
}

/// Minimum canonical recorded selection. No editorial prose, anchor, suggestion,
/// task, policy, CI or delivery fields are accepted in this first version.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct JudgmentSelection {
    pub(crate) claim_id: String,
    pub(crate) evidence_refs: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReviewJudgment {
    schema: String,
    subject: JudgmentSubject,
    revision: RevisionRef,
    source_manifest_digest: String,
    review_result: ReviewResult,
    selections: Vec<JudgmentSelection>,
}

/// Immutable artifact bytes with a caller-owned admission pin. A graph edge is
/// insufficient by itself: selected references must also resolve to these bytes.
#[derive(Serialize)]
pub(crate) struct RecordedEvidence<'a> {
    pub(crate) reference: &'a str,
    pub(crate) digest: &'a str,
    #[serde(skip)]
    pub(crate) bytes: &'a [u8],
}

/// Imported CI is opaque read-only context. No result is computed or parsed.
#[derive(Serialize)]
pub(crate) struct ImportedCi<'a> {
    pub(crate) digest: &'a str,
    #[serde(skip)]
    pub(crate) bytes: &'a [u8],
}

/// All fields are pinned by the independently retained source-manifest digest.
/// `review_complete` is an upstream recorded scope receipt, not inferred here.
#[derive(Serialize)]
pub(crate) struct RecordedJudgmentSource<'a> {
    pub(crate) subject: &'a JudgmentSubject,
    pub(crate) revision_canonical: &'a str,
    pub(crate) graph: &'a ClaimGraph,
    pub(crate) review_complete: bool,
    pub(crate) evidence: &'a [RecordedEvidence<'a>],
    pub(crate) imported_ci: Option<ImportedCi<'a>>,
}

/// Recompute from full source objects, including the graph, not two claimed
/// producer hashes. Evidence and CI byte pins are checked separately on entry.
pub(crate) fn source_manifest_digest(source: &RecordedJudgmentSource<'_>) -> Result<String> {
    let mut bytes = b"ub-review.recorded-judgment-source.v1\0".to_vec();
    bytes.extend(serde_json::to_vec(source)?);
    Ok(sha256_hex(&bytes))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RecordedLead {
    Absent,
    Invalid {
        reason: String,
    },
    /// Shadow validity only; no public authority or complete #1013 qualification.
    Validated {
        judgment: Box<ReviewJudgment>,
        answer_digest: String,
    },
}

/// Source/CI corruption is an error even with no lead. Missing/invalid lead is
/// explicit and never converted into a clean judgment. The CI observation must
/// have the exact original bytes, including whitespace, or remain absent only
/// when there was no imported CI at all.
pub(crate) fn validate_recorded_judgment(
    source: &RecordedJudgmentSource<'_>,
    current_revision: &RevisionIdentity,
    expected_source_digest: &str,
    answer: Option<&[u8]>,
    expected_answer_digest: Option<&str>,
    observed_ci: Option<&[u8]>,
) -> Result<RecordedLead> {
    ensure!(
        source_manifest_digest(source)? == expected_source_digest,
        "source manifest mismatch"
    );
    ensure!(
        RevisionIdentity::from_canonical(source.revision_canonical)? == *current_revision,
        "stale source revision"
    );
    let revision = current_revision.revision_ref();
    ensure!(
        source.graph.revision.as_ref() == Some(&revision),
        "graph revision mismatch"
    );
    ensure!(
        source.graph.schema == crate::artifacts::CLAIM_GRAPH_SCHEMA
            && source.graph.mode == "active",
        "unavailable active graph"
    );
    ensure!(
        source.graph.head_sha == revision.reviewed_commit,
        "graph head mismatch"
    );
    ensure!(
        source.subject.repository.split('/').count() == 2
            && !source.subject.repository.split('/').any(str::is_empty),
        "invalid repository subject"
    );
    ensure!(
        !source.subject.pass_id.trim().is_empty() && !source.subject.attempt_id.trim().is_empty(),
        "missing pass/attempt"
    );
    ensure!(
        source.subject.review_spec_digest.len() == 64
            && source
                .subject
                .review_spec_digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid spec digest"
    );
    let mut claims = BTreeSet::new();
    for claim in &source.graph.claims {
        ensure!(
            !claim.id.is_empty() && claims.insert(&claim.id),
            "ambiguous source claim"
        );
    }
    let mut references = BTreeSet::new();
    for evidence in source.evidence {
        ensure!(
            !evidence.reference.is_empty() && references.insert(evidence.reference),
            "ambiguous evidence reference"
        );
        ensure!(
            sha256_hex(evidence.bytes) == evidence.digest,
            "evidence bytes mismatch"
        );
    }
    match (&source.imported_ci, observed_ci) {
        (None, None) => {}
        (Some(ci), Some(observed)) => {
            ensure!(
                sha256_hex(ci.bytes) == ci.digest
                    && sha256_hex(observed) == ci.digest
                    && observed == ci.bytes,
                "imported CI bytes changed"
            );
        }
        _ => anyhow::bail!("imported CI observation missing or unexpected"),
    }
    let Some(answer) = answer else {
        ensure!(expected_answer_digest.is_none(), "pinned answer missing");
        return Ok(RecordedLead::Absent);
    };
    let validation = (|| -> Result<ReviewJudgment> {
        ensure!(
            Some(sha256_hex(answer).as_str()) == expected_answer_digest,
            "recorded answer bytes mismatch"
        );
        // RevisionRef is a legacy tolerant deserializer. Reject nested authority
        // fields here without changing that existing shared wire contract.
        let value: serde_json::Value = serde_json::from_slice(answer)?;
        let revision_fields = value.get("revision").and_then(serde_json::Value::as_object);
        ensure!(
            revision_fields.is_some_and(|fields| fields.len() == 3
                && fields
                    .keys()
                    .all(|key| matches!(key.as_str(), "digest" | "semantics" | "reviewed_commit"))),
            "unknown revision field"
        );
        let judgment: ReviewJudgment = serde_json::from_slice(answer)?;
        judgment.revision.validate()?;
        ensure!(judgment.schema == SCHEMA, "unknown judgment schema");
        ensure!(
            judgment.subject == *source.subject
                && judgment.revision == revision
                && judgment.source_manifest_digest == expected_source_digest,
            "judgment source identity mismatch"
        );
        ensure!(
            judgment.review_result != ReviewResult::Clean
                || (source.review_complete
                    && source.graph.evidence_gaps.is_empty()
                    && source.graph.claims.iter().all(|claim| matches!(
                        claim.state,
                        ClaimState::Refuted | ClaimState::Dropped
                    ))),
            "unsupported clean judgment"
        );
        ensure!(
            judgment.review_result != ReviewResult::Clean || judgment.selections.is_empty(),
            "clean judgment selects claims"
        );
        ensure!(
            judgment.review_result != ReviewResult::Findings || !judgment.selections.is_empty(),
            "findings judgment has no selections"
        );
        let mut selected = BTreeSet::new();
        for selection in &judgment.selections {
            ensure!(
                selected.insert(&selection.claim_id),
                "duplicate selected claim"
            );
            let claim = source
                .graph
                .claims
                .iter()
                .find(|claim| claim.id == selection.claim_id)
                .ok_or_else(|| anyhow::anyhow!("unknown selected claim"))?;
            ensure!(
                !matches!(claim.state, ClaimState::Refuted | ClaimState::Dropped),
                "rejected source claim"
            );
            ensure!(
                !selection.evidence_refs.is_empty(),
                "selected claim has no evidence"
            );
            let mut edges = BTreeSet::new();
            for reference in &selection.evidence_refs {
                ensure!(
                    edges.insert(reference) && references.contains(reference.as_str()),
                    "unknown or duplicate evidence reference"
                );
                ensure!(
                    claim
                        .supporting_evidence
                        .iter()
                        .chain(&claim.contradicting_evidence)
                        .any(|edge| edge.reference == *reference
                            && edge.class != EvidenceClass::UnsupportedAssertion),
                    "evidence not bound to selected claim"
                );
            }
        }
        Ok(judgment)
    })();
    Ok(match validation {
        Ok(judgment) => RecordedLead::Validated {
            judgment: Box::new(judgment),
            answer_digest: sha256_hex(answer),
        },
        Err(error) => RecordedLead::Invalid {
            reason: error.to_string(),
        },
    })
}

#[cfg(test)]
mod tests;
