//! Claim graph v1: structured claims with typed evidence references, causal
//! relevance paths, conflict records, and claim states.
//!
//! **Shadow mode**: this module builds and emits the claim graph artifact
//! but does NOT change the review compiler's behavior. The compiler still
//! consumes raw observations and candidates. The shadow artifact lets us
//! compare what the claim graph WOULD say against what the current review
//! surface produces.
//!
//! Order 3 of the evidence-control-plane epic (#655).

use serde::Serialize;
use std::path::Path;

use crate::artifacts::CLAIM_GRAPH_SCHEMA;

/// The complete claim graph for a single run. Written to
/// `review/claim_graph.json` as a shadow artifact.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClaimGraph {
    pub(crate) schema: &'static str,
    /// All claims in the graph.
    pub(crate) claims: Vec<ClaimNode>,
    /// Detected conflicts between claims.
    pub(crate) conflicts: Vec<ConflictRecord>,
    /// Evidence gaps: claims that need evidence but don't have it.
    pub(crate) evidence_gaps: Vec<ClaimEvidenceGap>,
    /// Whether the claim graph is computed from real evidence or is a
    /// placeholder. "shadow" until promoted to active adjudication.
    pub(crate) mode: &'static str,
}

/// A single claim in the graph. Each claim corresponds to a candidate,
/// observation, or finding from the review lanes.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClaimNode {
    pub(crate) id: String,
    /// What the claim asserts (e.g., "this test does not discriminate the patch").
    pub(crate) subject: String,
    /// The lane or source that raised the claim.
    pub(crate) source_lane: String,
    /// Current adjudication state.
    pub(crate) state: ClaimState,
    /// Evidence supporting this claim.
    pub(crate) supporting_evidence: Vec<EvidenceRef>,
    /// Evidence contradicting this claim.
    pub(crate) contradicting_evidence: Vec<EvidenceRef>,
    /// Why this claim is relevant to the diff (causal path).
    pub(crate) relevance: RelevancePath,
    /// The claim's severity hypothesis: "high", "medium", "low".
    pub(crate) severity: String,
}

/// The adjudication state of a claim.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) enum ClaimState {
    /// Initial state — raised but not yet investigated.
    Hypothesized,
    /// Needs more evidence to confirm or refute.
    NeedsEvidence,
    /// Has supporting evidence but not yet confirmed.
    Supported,
    /// Confirmed by sufficient evidence.
    Confirmed,
    /// Refuted by contradicting evidence.
    Refuted,
    /// Conflicting evidence from multiple sources; cannot resolve.
    Conflicted,
    /// Evidence insufficient to confirm or refute.
    Inconclusive,
    /// Deferred for follow-up; not actionable in this PR.
    Parked,
    /// Withdrawn by the source lane or dropped as irrelevant.
    Dropped,
}

impl ClaimState {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            ClaimState::Hypothesized => "hypothesized",
            ClaimState::NeedsEvidence => "needs_evidence",
            ClaimState::Supported => "supported",
            ClaimState::Confirmed => "confirmed",
            ClaimState::Refuted => "refuted",
            ClaimState::Conflicted => "conflicted",
            ClaimState::Inconclusive => "inconclusive",
            ClaimState::Parked => "parked",
            ClaimState::Dropped => "dropped",
        }
    }
}

/// A typed reference to a piece of evidence.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct EvidenceRef {
    /// Evidence class (determines precedence in adjudication).
    pub(crate) class: EvidenceClass,
    /// Artifact path or identifier (e.g., "review/proof_receipts.json#proof-001").
    pub(crate) reference: String,
    /// Human-readable description of what this evidence shows.
    pub(crate) detail: String,
}

/// The class of evidence, used for precedence-based adjudication
/// (Order 4 of epic #655). Lower ordinal value = higher precedence.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) enum EvidenceClass {
    /// Highest precedence: a deterministic proof receipt.
    ProofReceipt,
    /// A mechanically validated repository fact.
    ValidatedFact,
    /// An exact source code or contract citation.
    ExactCitation,
    /// A model lane interpretation (reasoning, not proof).
    ModelInterpretation,
    /// An unsupported model assertion (no citation or evidence backing).
    UnsupportedAssertion,
}

impl EvidenceClass {
    /// Precedence rank: lower = stronger. Used by the adjudicator (Order 4).
    pub(crate) fn precedence(&self) -> u8 {
        match self {
            EvidenceClass::ProofReceipt => 0,
            EvidenceClass::ValidatedFact => 1,
            EvidenceClass::ExactCitation => 2,
            EvidenceClass::ModelInterpretation => 3,
            EvidenceClass::UnsupportedAssertion => 4,
        }
    }

    pub(crate) fn key(&self) -> &'static str {
        match self {
            EvidenceClass::ProofReceipt => "proof-receipt",
            EvidenceClass::ValidatedFact => "validated-fact",
            EvidenceClass::ExactCitation => "exact-citation",
            EvidenceClass::ModelInterpretation => "model-interpretation",
            EvidenceClass::UnsupportedAssertion => "unsupported-assertion",
        }
    }
}

/// A causal relevance path explaining why a claim is related to the diff.
/// (Order 5 of epic #655 enforces this for every surfaced claim.)
#[derive(Clone, Debug, Serialize)]
pub(crate) struct RelevancePath {
    /// The type of causal relationship.
    pub(crate) kind: RelevanceKind,
    /// Human-readable explanation of the causal chain.
    pub(crate) explanation: String,
}

/// Types of causal relevance between a claim and the diff.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) enum RelevanceKind {
    /// The claim is about a changed line.
    ChangedLine,
    /// The claim is about a changed symbol (function, type, constant).
    ChangedSymbol,
    /// The claim is about an unchanged caller of a changed symbol.
    CallerOfChangedSymbol,
    /// The claim is about an unchanged callee of a changed symbol.
    CalleeOfChangedSymbol,
    /// The claim is about a test that exercises changed behavior.
    TestOfChangedBehavior,
    /// The claim is about a package that depends on a changed package.
    ReverseDependentPackage,
    /// The claim is about a mirror artifact affected by a schema change.
    MirrorOfChangedArtifact,
    /// The claim is about a policy that affects the gate.
    PolicyAffectsGate,
    /// The claim is about a prior PR-thread comment still applicable.
    PriorThreadStillApplicable,
    /// Relevance not yet determined (shadow mode).
    Unresolved,
}

impl RelevanceKind {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            RelevanceKind::ChangedLine => "changed-line",
            RelevanceKind::ChangedSymbol => "changed-symbol",
            RelevanceKind::CallerOfChangedSymbol => "caller-of-changed-symbol",
            RelevanceKind::CalleeOfChangedSymbol => "callee-of-changed-symbol",
            RelevanceKind::TestOfChangedBehavior => "test-of-changed-behavior",
            RelevanceKind::ReverseDependentPackage => "reverse-dependent-package",
            RelevanceKind::MirrorOfChangedArtifact => "mirror-of-changed-artifact",
            RelevanceKind::PolicyAffectsGate => "policy-affects-gate",
            RelevanceKind::PriorThreadStillApplicable => "prior-thread-still-applicable",
            RelevanceKind::Unresolved => "unresolved",
        }
    }
}

/// A detected conflict between two or more claims.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ConflictRecord {
    /// IDs of the conflicting claims.
    pub(crate) claim_ids: Vec<String>,
    /// The nature of the conflict.
    pub(crate) description: String,
    /// Whether the conflict has been resolved (and how).
    pub(crate) resolution: ConflictResolution,
}

/// How a conflict was (or wasn't) resolved.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) enum ConflictResolution {
    /// Not yet resolved; awaiting evidence.
    Unresolved,
    /// Resolved by evidence precedence (higher-class evidence wins).
    ResolvedByPrecedence,
    /// Resolved by deterministic proof.
    ResolvedByProof,
    /// Both sides surfaced as a verification question.
    SurfacedAsVerification,
    /// Dropped as not material.
    Dropped,
}

/// An evidence gap: a claim that needs evidence but doesn't have it.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClaimEvidenceGap {
    pub(crate) claim_id: String,
    pub(crate) needed_evidence_class: String,
    pub(crate) detail: String,
}

/// Build the initial shadow claim graph. This is a placeholder that records
/// what we know (no claims yet) and what we don't (everything). As Order 3
/// PRs land, this gains claim extraction from candidates/observations,
/// evidence attachment, conflict detection, and state assignment.
pub(crate) fn build_shadow_claim_graph() -> ClaimGraph {
    // Reference each type variant so clippy doesn't flag dead code.
    // These will be removed when claim extraction lands (Order 3 PR 2+).
    let _e0 = EvidenceClass::ProofReceipt;
    let _e1 = EvidenceClass::ValidatedFact;
    let _e2 = EvidenceClass::ExactCitation;
    let _e3 = EvidenceClass::ModelInterpretation;
    let _e4 = EvidenceClass::UnsupportedAssertion;
    let _s0 = ClaimState::Hypothesized;
    let _s1 = ClaimState::NeedsEvidence;
    let _s2 = ClaimState::Supported;
    let _s3 = ClaimState::Confirmed;
    let _s4 = ClaimState::Refuted;
    let _s5 = ClaimState::Conflicted;
    let _s6 = ClaimState::Inconclusive;
    let _s7 = ClaimState::Parked;
    let _s8 = ClaimState::Dropped;
    let _r0 = RelevanceKind::ChangedLine;
    let _r1 = RelevanceKind::ChangedSymbol;
    let _r2 = RelevanceKind::CallerOfChangedSymbol;
    let _r3 = RelevanceKind::CalleeOfChangedSymbol;
    let _r4 = RelevanceKind::TestOfChangedBehavior;
    let _r5 = RelevanceKind::ReverseDependentPackage;
    let _r6 = RelevanceKind::MirrorOfChangedArtifact;
    let _r7 = RelevanceKind::PolicyAffectsGate;
    let _r8 = RelevanceKind::PriorThreadStillApplicable;
    let _r9 = RelevanceKind::Unresolved;
    let _c0 = ConflictResolution::Unresolved;
    let _c1 = ConflictResolution::ResolvedByPrecedence;
    let _c2 = ConflictResolution::ResolvedByProof;
    let _c3 = ConflictResolution::SurfacedAsVerification;
    let _c4 = ConflictResolution::Dropped;

    // Reference key() and precedence() methods to suppress dead-code warnings.
    // These methods will be called by the adjudicator in Order 4.
    let _pk = EvidenceClass::ProofReceipt.precedence();
    let _ck = ClaimState::Hypothesized.key();
    let _rk = RelevanceKind::Unresolved.key();
    let _ek = EvidenceClass::ProofReceipt.key();

    ClaimGraph {
        schema: CLAIM_GRAPH_SCHEMA,
        claims: Vec::new(),
        conflicts: Vec::new(),
        evidence_gaps: vec![ClaimEvidenceGap {
            claim_id: "global".to_owned(),
            needed_evidence_class: EvidenceClass::ModelInterpretation.key().to_owned(),
            detail: "Claim graph not yet populated from candidates/observations. \
                     Claim extraction (Order 3 PR 2), evidence attachment (PR 3), \
                     conflict detection (PR 4), and state assignment (PR 5) pending."
                .to_owned(),
        }],
        mode: "shadow",
    }
}

/// Write the claim graph as a shadow artifact.
pub(crate) fn write_claim_graph(out: &Path, graph: &ClaimGraph) -> anyhow::Result<()> {
    let path = out.join("review").join("claim_graph.json");
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(graph)?)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_claim_graph_has_schema_and_gap() {
        let graph = build_shadow_claim_graph();
        assert_eq!(graph.schema, "ub-review.claim_graph.v1");
        assert!(graph.claims.is_empty());
        assert!(graph.conflicts.is_empty());
        assert_eq!(graph.mode, "shadow");
        assert_eq!(graph.evidence_gaps.len(), 1);
        assert_eq!(graph.evidence_gaps[0].claim_id, "global");
    }

    #[test]
    fn claim_state_keys_are_stable() {
        assert_eq!(ClaimState::Hypothesized.key(), "hypothesized");
        assert_eq!(ClaimState::Confirmed.key(), "confirmed");
        assert_eq!(ClaimState::Refuted.key(), "refuted");
        assert_eq!(ClaimState::Conflicted.key(), "conflicted");
        assert_eq!(ClaimState::Inconclusive.key(), "inconclusive");
        assert_eq!(ClaimState::Parked.key(), "parked");
    }

    #[test]
    fn evidence_class_precedence_orders_correctly() {
        assert!(
            EvidenceClass::ProofReceipt.precedence() < EvidenceClass::ValidatedFact.precedence()
        );
        assert!(
            EvidenceClass::ValidatedFact.precedence() < EvidenceClass::ExactCitation.precedence()
        );
        assert!(
            EvidenceClass::ExactCitation.precedence()
                < EvidenceClass::ModelInterpretation.precedence()
        );
        assert!(
            EvidenceClass::ModelInterpretation.precedence()
                < EvidenceClass::UnsupportedAssertion.precedence()
        );
    }

    #[test]
    fn relevance_kind_keys_are_stable() {
        assert_eq!(RelevanceKind::ChangedLine.key(), "changed-line");
        assert_eq!(
            RelevanceKind::CallerOfChangedSymbol.key(),
            "caller-of-changed-symbol"
        );
        assert_eq!(
            RelevanceKind::ReverseDependentPackage.key(),
            "reverse-dependent-package"
        );
        assert_eq!(RelevanceKind::Unresolved.key(), "unresolved");
    }
}
