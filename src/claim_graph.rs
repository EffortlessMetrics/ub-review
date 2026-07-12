//! Claim graph v1: structured claims with typed evidence references, causal
//! relevance paths, conflict records, and claim states.
//!
//! The legacy shadow builder remains available for early-failure artifacts.
//! Production runs overwrite the same artifact at the final compiler boundary
//! with current-head `ReviewTopic` records that carry thread, proof, and
//! delivery links.
//!
//! Order 3 of the evidence-control-plane epic (#655).

use serde::Serialize;
use std::path::Path;

use crate::artifacts::CLAIM_GRAPH_SCHEMA;

/// The complete claim graph for a single run. Written to
/// `review/claim_graph.json`; `mode` distinguishes early shadow output from
/// the active final graph.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ClaimGraph {
    pub(crate) schema: &'static str,
    /// Exact PR head this graph certifies. Empty only for the legacy shadow
    /// graph emitted before final evidence exists.
    pub(crate) head_sha: String,
    /// All claims in the graph.
    pub(crate) claims: Vec<ClaimNode>,
    /// Current-head review topics compiled from claims, threads, and receipts.
    pub(crate) topics: Vec<crate::ReviewTopic>,
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

/// Result of checking whether the convergence loop should continue.
/// (Order 7 of epic #655.)
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConvergenceDecision {
    /// Whether to continue to the next round.
    pub(crate) should_continue: bool,
    /// Human-readable reason for the decision.
    pub(crate) reason: String,
}

/// Decide whether the follow-up convergence loop should continue.
///
/// The loop continues only when ALL of:
/// - Material unresolved claims exist (Hypothesized, NeedsEvidence, Conflicted)
/// - New evidence arrived in the previous round (at least one state transition)
/// - Model/proof budget remains
/// - Maximum rounds not exceeded
///
/// The loop stops when ANY of:
/// - All material claims resolved (Confirmed, Refuted, Parked, Dropped)
/// - No new evidence arrived (no state transition in previous round)
/// - Budget exhausted
/// - Maximum rounds exceeded
#[cfg(test)]
pub(crate) fn should_continue_convergence(
    claims: &[ClaimNode],
    previous_states: &[(String, ClaimState)],
    budget_remaining: usize,
    rounds_completed: usize,
    max_rounds: usize,
) -> ConvergenceDecision {
    // Check budget.
    if budget_remaining == 0 {
        return ConvergenceDecision {
            should_continue: false,
            reason: "Model/proof budget exhausted".to_owned(),
        };
    }

    // Check max rounds.
    if rounds_completed >= max_rounds {
        return ConvergenceDecision {
            should_continue: false,
            reason: format!("Maximum rounds ({max_rounds}) reached"),
        };
    }

    // Check for unresolved material claims.
    let has_unresolved = claims.iter().any(|c| {
        matches!(
            c.state,
            ClaimState::Hypothesized | ClaimState::NeedsEvidence | ClaimState::Conflicted
        )
    });
    if !has_unresolved {
        return ConvergenceDecision {
            should_continue: false,
            reason: "All material claims resolved".to_owned(),
        };
    }

    // Check for state transitions (evidence produced meaningful change).
    let current_states: std::collections::HashMap<&str, &ClaimState> =
        claims.iter().map(|c| (c.id.as_str(), &c.state)).collect();
    let transitions = previous_states
        .iter()
        .filter(|(id, prev_state)| {
            current_states
                .get(id.as_str())
                .is_some_and(|curr| *curr != prev_state)
        })
        .count();
    if transitions == 0 && rounds_completed > 0 {
        return ConvergenceDecision {
            should_continue: false,
            reason: "No state transitions in previous round; convergence reached".to_owned(),
        };
    }

    ConvergenceDecision {
        should_continue: true,
        reason: format!(
            "{transitions} state transition(s) in round {rounds_completed}; {budget_remaining} budget remaining",
        ),
    }
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
    // Reference each type variant so clippy doesn't flag them as dead code.
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
    let _pk = EvidenceClass::ProofReceipt.precedence();
    let _ck = ClaimState::Hypothesized.key();
    let _rk = RelevanceKind::Unresolved.key();
    let _ek = EvidenceClass::ProofReceipt.key();

    ClaimGraph {
        schema: CLAIM_GRAPH_SCHEMA,
        head_sha: String::new(),
        claims: Vec::new(),
        topics: Vec::new(),
        conflicts: Vec::new(),
        evidence_gaps: Vec::new(),
        mode: "shadow",
    }
}

/// A simplified input for claim extraction (avoids importing the full
/// review-surface types into this module). The caller converts candidates,
/// observations, and findings into these lightweight descriptors.
#[cfg(test)]
#[derive(Clone, Debug)]
pub(crate) struct ClaimInput {
    pub(crate) id: String,
    pub(crate) claim: String,
    pub(crate) lane: String,
    pub(crate) severity: String,
    pub(crate) evidence_text: String,
    pub(crate) path: Option<String>,
}

/// Extract claims from candidate/observation descriptors and build a
/// populated claim graph with initial states and evidence references.
/// (Order 3 PR 2 of epic #655.)
#[cfg(test)]
pub(crate) fn build_claim_graph_from_inputs(claims: &[ClaimInput]) -> ClaimGraph {
    let mut evidence_gaps = Vec::new();
    let nodes: Vec<ClaimNode> = claims
        .iter()
        .map(|c| {
            let (state, supporting) = if c.evidence_text.is_empty() {
                evidence_gaps.push(ClaimEvidenceGap {
                    claim_id: c.id.clone(),
                    needed_evidence_class: "proof-receipt".to_owned(),
                    detail: format!("Claim `{}` has no backing evidence", c.id),
                });
                (ClaimState::Hypothesized, Vec::new())
            } else {
                // Has evidence text — classify as model interpretation (lowest
                // non-unsupported class until proof receipts are attached).
                (
                    ClaimState::NeedsEvidence,
                    vec![EvidenceRef {
                        class: EvidenceClass::ModelInterpretation,
                        reference: format!("lane-evidence:{}", c.lane),
                        detail: c.evidence_text.clone(),
                    }],
                )
            };

            let relevance = if let Some(ref path) = c.path {
                RelevancePath {
                    kind: RelevanceKind::ChangedLine,
                    explanation: format!("Claim cites changed file: {path}"),
                }
            } else {
                RelevancePath {
                    kind: RelevanceKind::Unresolved,
                    explanation: "Relevance path not yet determined (shadow mode)".to_owned(),
                }
            };

            ClaimNode {
                id: c.id.clone(),
                subject: c.claim.clone(),
                source_lane: c.lane.clone(),
                state,
                supporting_evidence: supporting,
                contradicting_evidence: Vec::new(),
                relevance,
                severity: c.severity.clone(),
            }
        })
        .collect();

    // Detect conflicts: claims from different lanes with same subject
    // (simplified token-overlap detection, same as the existing cross-lane
    // conflict logic in observation_build.rs).
    let mut conflicts = Vec::new();
    for (i, a) in nodes.iter().enumerate() {
        for b in nodes.iter().skip(i + 1) {
            if a.source_lane != b.source_lane && subjects_overlap(&a.subject, &b.subject) {
                conflicts.push(ConflictRecord {
                    claim_ids: vec![a.id.clone(), b.id.clone()],
                    description: format!(
                        "Lanes `{}` and `{}` disagree on overlapping subject",
                        a.source_lane, b.source_lane
                    ),
                    resolution: ConflictResolution::Unresolved,
                });
            }
        }
    }

    ClaimGraph {
        schema: CLAIM_GRAPH_SCHEMA,
        head_sha: String::new(),
        claims: nodes,
        topics: Vec::new(),
        conflicts,
        evidence_gaps,
        mode: "shadow",
    }
}

/// Simple token-overlap check for conflict detection (simplified version of
/// the existing cross-lane conflict heuristic).
#[cfg(test)]
fn subjects_overlap(a: &str, b: &str) -> bool {
    let tokens_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let tokens_b: std::collections::HashSet<&str> = b.split_whitespace().collect();
    let shared = tokens_a.intersection(&tokens_b).count();
    shared >= 3 // Same threshold as observation_build.rs
}

/// Result of adjudicating a conflict between two claims.
/// (Order 4 of epic #655.)
#[cfg(test)]
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct AdjudicationResult {
    /// The winning claim ID (if any).
    pub(crate) winner: Option<String>,
    /// The losing claim ID (if any).
    pub(crate) loser: Option<String>,
    /// How the conflict was resolved.
    pub(crate) resolution: ConflictResolution,
    /// Human-readable explanation of why this side won (or why it's inconclusive).
    pub(crate) reason: String,
    /// The new state for the winning claim.
    pub(crate) winner_state: ClaimState,
    /// The new state for the losing claim.
    pub(crate) loser_state: ClaimState,
}

/// Adjudicate a conflict between two claims using evidence precedence.
///
/// Precedence ladder (lower = stronger):
///   ProofReceipt(0) > ValidatedFact(1) > ExactCitation(2)
///   > ModelInterpretation(3) > UnsupportedAssertion(4)
///
/// Rules:
/// - If one side has strictly higher-precedence evidence, that side wins.
///   Winner → Confirmed, Loser → Refuted.
/// - If both sides have equal-precedence evidence, the conflict stays
///   Unresolved and both claims become Conflicted.
/// - If neither side has any evidence, both become Inconclusive.
/// - A ProofReceipt always wins (ResolvedByProof); other wins are
///   ResolvedByPrecedence.
#[cfg(test)]
pub(crate) fn adjudicate_conflict(claim_a: &ClaimNode, claim_b: &ClaimNode) -> AdjudicationResult {
    let best_a = best_evidence_class(&claim_a.supporting_evidence);
    let best_b = best_evidence_class(&claim_b.supporting_evidence);

    match (best_a, best_b) {
        (None, None) => AdjudicationResult {
            winner: None,
            loser: None,
            resolution: ConflictResolution::SurfacedAsVerification,
            reason: "Neither claim has supporting evidence; both remain inconclusive".to_owned(),
            winner_state: ClaimState::Inconclusive,
            loser_state: ClaimState::Inconclusive,
        },
        (Some(a_class), None) => AdjudicationResult {
            winner: Some(claim_a.id.clone()),
            loser: Some(claim_b.id.clone()),
            resolution: resolution_for_class(&a_class),
            reason: format!(
                "Claim `{}` wins: has {} evidence; claim `{}` has none",
                claim_a.id,
                a_class.key(),
                claim_b.id
            ),
            winner_state: ClaimState::Confirmed,
            loser_state: ClaimState::Refuted,
        },
        (None, Some(b_class)) => AdjudicationResult {
            winner: Some(claim_b.id.clone()),
            loser: Some(claim_a.id.clone()),
            resolution: resolution_for_class(&b_class),
            reason: format!(
                "Claim `{}` wins: has {} evidence; claim `{}` has none",
                claim_b.id,
                b_class.key(),
                claim_a.id
            ),
            winner_state: ClaimState::Confirmed,
            loser_state: ClaimState::Refuted,
        },
        (Some(a_class), Some(b_class)) => {
            let a_prec = a_class.precedence();
            let b_prec = b_class.precedence();
            if a_prec < b_prec {
                AdjudicationResult {
                    winner: Some(claim_a.id.clone()),
                    loser: Some(claim_b.id.clone()),
                    resolution: resolution_for_class(&a_class),
                    reason: format!(
                        "Claim `{}` wins: {} (precedence {}) beats {} (precedence {})",
                        claim_a.id,
                        a_class.key(),
                        a_prec,
                        b_class.key(),
                        b_prec
                    ),
                    winner_state: ClaimState::Confirmed,
                    loser_state: ClaimState::Refuted,
                }
            } else if b_prec < a_prec {
                AdjudicationResult {
                    winner: Some(claim_b.id.clone()),
                    loser: Some(claim_a.id.clone()),
                    resolution: resolution_for_class(&b_class),
                    reason: format!(
                        "Claim `{}` wins: {} (precedence {}) beats {} (precedence {})",
                        claim_b.id,
                        b_class.key(),
                        b_prec,
                        a_class.key(),
                        a_prec
                    ),
                    winner_state: ClaimState::Confirmed,
                    loser_state: ClaimState::Refuted,
                }
            } else {
                // Equal precedence — cannot resolve by precedence alone
                AdjudicationResult {
                    winner: None,
                    loser: None,
                    resolution: ConflictResolution::Unresolved,
                    reason: format!(
                        "Both claims have {} evidence (precedence {}); \
                         cannot resolve by precedence alone",
                        a_class.key(),
                        a_prec
                    ),
                    winner_state: ClaimState::Conflicted,
                    loser_state: ClaimState::Conflicted,
                }
            }
        }
    }
}

/// Get the highest-precedence (lowest ordinal) evidence class from a list.
#[cfg(test)]
fn best_evidence_class(evidence: &[EvidenceRef]) -> Option<EvidenceClass> {
    evidence
        .iter()
        .map(|e| e.class.clone())
        .min_by_key(|c| c.precedence())
}

/// Determine the ConflictResolution based on the winning evidence class.
#[cfg(test)]
fn resolution_for_class(class: &EvidenceClass) -> ConflictResolution {
    match class {
        EvidenceClass::ProofReceipt => ConflictResolution::ResolvedByProof,
        _ => ConflictResolution::ResolvedByPrecedence,
    }
}

/// Check whether a claim has a valid causal relevance path to the diff.
/// (Order 5 of epic #655.)
///
/// Rules:
/// - `ChangedLine`: always valid (inline comments already require RIGHT-side lines).
/// - `ChangedSymbol`, `CallerOfChangedSymbol`, `CalleeOfChangedSymbol`: valid
///   if the explanation is non-empty (the explanation must name the symbol/caller).
/// - `TestOfChangedBehavior`: valid if explanation references a test target.
/// - `ReverseDependentPackage`: valid if explanation names the affected package.
/// - `MirrorOfChangedArtifact`, `PolicyAffectsGate`, `PriorThreadStillApplicable`:
///   valid if explanation is non-empty.
/// - `Unresolved`: ALWAYS invalid — this is the "no causal path" state.
///
/// Returns `RelevanceCheckResult::Eligible` if the claim can be surfaced,
/// or `ArtifactOnly` with a reason if it must remain in artifacts only.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum RelevanceCheckResult {
    /// The claim has a valid causal path and can be surfaced in the review.
    Eligible,
    /// The claim lacks a causal path and must remain artifact-only.
    ArtifactOnly { reason: String },
}

#[cfg(test)]
pub(crate) fn check_causal_relevance(relevance: &RelevancePath) -> RelevanceCheckResult {
    match relevance.kind {
        RelevanceKind::Unresolved => RelevanceCheckResult::ArtifactOnly {
            reason: "Claim has no causal relevance path to the diff; \
                     remaining artifact-only until relevance is established"
                .to_owned(),
        },
        RelevanceKind::ChangedLine => {
            // Inline comments are already validated via the line-map guard.
            // A ChangedLine relevance with a non-empty explanation is sufficient.
            if relevance.explanation.is_empty() {
                RelevanceCheckResult::ArtifactOnly {
                    reason: "ChangedLine relevance path has empty explanation".to_owned(),
                }
            } else {
                RelevanceCheckResult::Eligible
            }
        }
        RelevanceKind::ChangedSymbol
        | RelevanceKind::CallerOfChangedSymbol
        | RelevanceKind::CalleeOfChangedSymbol
        | RelevanceKind::TestOfChangedBehavior
        | RelevanceKind::ReverseDependentPackage
        | RelevanceKind::MirrorOfChangedArtifact
        | RelevanceKind::PolicyAffectsGate
        | RelevanceKind::PriorThreadStillApplicable => {
            if relevance.explanation.is_empty() {
                RelevanceCheckResult::ArtifactOnly {
                    reason: format!(
                        "{:?} relevance path has empty explanation; \
                         must name the specific symbol, caller, test, package, or artifact",
                        relevance.kind
                    ),
                }
            } else {
                RelevanceCheckResult::Eligible
            }
        }
    }
}

/// Write the claim graph artifact.
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
    fn shadow_claim_graph_is_empty_by_default() {
        let graph = build_shadow_claim_graph();
        assert_eq!(graph.schema, "ub-review.claim_graph.v1");
        assert!(graph.claims.is_empty());
        assert!(graph.conflicts.is_empty());
        assert!(graph.evidence_gaps.is_empty());
        assert_eq!(graph.mode, "shadow");
    }

    #[test]
    fn claim_extraction_assigns_needs_evidence_when_evidence_present() {
        let inputs = vec![ClaimInput {
            id: "claim-1".to_owned(),
            claim: "The test does not discriminate the patch".to_owned(),
            lane: "tests-oracle".to_owned(),
            severity: "high".to_owned(),
            evidence_text: "base+tests result was non_discriminating".to_owned(),
            path: Some("src/config.rs".to_owned()),
        }];
        let graph = build_claim_graph_from_inputs(&inputs);
        assert_eq!(graph.claims.len(), 1);
        assert_eq!(graph.claims[0].state, ClaimState::NeedsEvidence);
        assert_eq!(graph.claims[0].supporting_evidence.len(), 1);
        assert_eq!(
            graph.claims[0].supporting_evidence[0].class,
            EvidenceClass::ModelInterpretation
        );
        assert_eq!(graph.claims[0].relevance.kind, RelevanceKind::ChangedLine);
    }

    #[test]
    fn claim_extraction_assigns_hypothesized_when_no_evidence() {
        let inputs = vec![ClaimInput {
            id: "claim-2".to_owned(),
            claim: "Potential memory leak in new buffer".to_owned(),
            lane: "ub-memory-lifetime".to_owned(),
            severity: "medium".to_owned(),
            evidence_text: String::new(),
            path: None,
        }];
        let graph = build_claim_graph_from_inputs(&inputs);
        assert_eq!(graph.claims[0].state, ClaimState::Hypothesized);
        assert!(graph.claims[0].supporting_evidence.is_empty());
        assert!(!graph.evidence_gaps.is_empty());
        assert_eq!(graph.claims[0].relevance.kind, RelevanceKind::Unresolved);
    }

    #[test]
    fn claim_extraction_detects_cross_lane_conflicts() {
        let inputs = vec![
            ClaimInput {
                id: "claim-a".to_owned(),
                claim: "The buffer resize is safe and correct".to_owned(),
                lane: "ub-memory-lifetime".to_owned(),
                severity: "low".to_owned(),
                evidence_text: "resize logic verified".to_owned(),
                path: Some("src/buffer.rs".to_owned()),
            },
            ClaimInput {
                id: "claim-b".to_owned(),
                claim: "The buffer resize is unsafe due to stale length".to_owned(),
                lane: "ub-active-view".to_owned(),
                severity: "high".to_owned(),
                evidence_text: "stale pointer risk detected".to_owned(),
                path: Some("src/buffer.rs".to_owned()),
            },
        ];
        let graph = build_claim_graph_from_inputs(&inputs);
        assert_eq!(graph.claims.len(), 2);
        assert!(!graph.conflicts.is_empty(), "should detect the conflict");
        assert_eq!(
            graph.conflicts[0].resolution,
            ConflictResolution::Unresolved
        );
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

    #[test]
    fn convergence_stops_when_budget_exhausted() {
        let decision = should_continue_convergence(&[], &[], 0, 0, 3);
        assert!(!decision.should_continue);
        assert!(decision.reason.contains("budget"));
    }

    #[test]
    fn convergence_stops_at_max_rounds() {
        let decision = should_continue_convergence(&[], &[], 10, 3, 3);
        assert!(!decision.should_continue);
        assert!(decision.reason.contains("Maximum rounds"));
    }

    #[test]
    fn convergence_stops_when_all_resolved() {
        let claims = vec![ClaimNode {
            id: "c1".to_owned(),
            subject: "test".to_owned(),
            source_lane: "lane".to_owned(),
            state: ClaimState::Confirmed,
            supporting_evidence: Vec::new(),
            contradicting_evidence: Vec::new(),
            relevance: RelevancePath {
                kind: RelevanceKind::ChangedLine,
                explanation: "test".to_owned(),
            },
            severity: "low".to_owned(),
        }];
        let decision = should_continue_convergence(&claims, &[], 10, 0, 3);
        assert!(!decision.should_continue);
        assert!(decision.reason.contains("resolved"));
    }

    #[test]
    fn convergence_continues_with_unresolved_and_transitions() {
        let claims = vec![ClaimNode {
            id: "c1".to_owned(),
            subject: "test".to_owned(),
            source_lane: "lane".to_owned(),
            state: ClaimState::NeedsEvidence,
            supporting_evidence: Vec::new(),
            contradicting_evidence: Vec::new(),
            relevance: RelevancePath {
                kind: RelevanceKind::ChangedLine,
                explanation: "test".to_owned(),
            },
            severity: "high".to_owned(),
        }];
        let prev = vec![("c1".to_owned(), ClaimState::Hypothesized)];
        let decision = should_continue_convergence(&claims, &prev, 10, 1, 3);
        assert!(decision.should_continue);
        assert!(decision.reason.contains("transition"));
    }

    #[test]
    fn convergence_stops_when_no_state_change() {
        let claims = vec![ClaimNode {
            id: "c1".to_owned(),
            subject: "test".to_owned(),
            source_lane: "lane".to_owned(),
            state: ClaimState::NeedsEvidence,
            supporting_evidence: Vec::new(),
            contradicting_evidence: Vec::new(),
            relevance: RelevancePath {
                kind: RelevanceKind::ChangedLine,
                explanation: "test".to_owned(),
            },
            severity: "high".to_owned(),
        }];
        let prev = vec![("c1".to_owned(), ClaimState::NeedsEvidence)];
        let decision = should_continue_convergence(&claims, &prev, 10, 2, 3);
        assert!(!decision.should_continue);
        assert!(decision.reason.contains("No state transitions"));
    }

    #[test]
    fn unresolved_relevance_is_always_artifact_only() {
        let result = check_causal_relevance(&RelevancePath {
            kind: RelevanceKind::Unresolved,
            explanation: "some explanation".to_owned(),
        });
        assert!(matches!(result, RelevanceCheckResult::ArtifactOnly { .. }));
    }

    #[test]
    fn changed_line_with_explanation_is_eligible() {
        let result = check_causal_relevance(&RelevancePath {
            kind: RelevanceKind::ChangedLine,
            explanation: "Claim cites changed line in src/config.rs:42".to_owned(),
        });
        assert_eq!(result, RelevanceCheckResult::Eligible);
    }

    #[test]
    fn changed_line_without_explanation_is_artifact_only() {
        let result = check_causal_relevance(&RelevancePath {
            kind: RelevanceKind::ChangedLine,
            explanation: String::new(),
        });
        assert!(matches!(result, RelevanceCheckResult::ArtifactOnly { .. }));
    }

    #[test]
    fn caller_of_changed_symbol_with_explanation_is_eligible() {
        let result = check_causal_relevance(&RelevancePath {
            kind: RelevanceKind::CallerOfChangedSymbol,
            explanation: "src/main.rs:100 calls config::load() which changed signature".to_owned(),
        });
        assert_eq!(result, RelevanceCheckResult::Eligible);
    }

    #[test]
    fn reverse_dependent_package_without_explanation_is_artifact_only() {
        let result = check_causal_relevance(&RelevancePath {
            kind: RelevanceKind::ReverseDependentPackage,
            explanation: String::new(),
        });
        assert!(matches!(result, RelevanceCheckResult::ArtifactOnly { .. }));
    }

    fn make_claim(id: &str, lane: &str, evidence_classes: &[EvidenceClass]) -> ClaimNode {
        ClaimNode {
            id: id.to_owned(),
            subject: format!("test claim {id}"),
            source_lane: lane.to_owned(),
            state: ClaimState::NeedsEvidence,
            supporting_evidence: evidence_classes
                .iter()
                .map(|c| EvidenceRef {
                    class: c.clone(),
                    reference: format!("ref-{id}"),
                    detail: format!("evidence for {id}"),
                })
                .collect(),
            contradicting_evidence: Vec::new(),
            relevance: RelevancePath {
                kind: RelevanceKind::ChangedLine,
                explanation: "test".to_owned(),
            },
            severity: "medium".to_owned(),
        }
    }

    #[test]
    fn adjudicate_proof_beats_model_interpretation() {
        let a = make_claim("a", "lane-a", &[EvidenceClass::ProofReceipt]);
        let b = make_claim("b", "lane-b", &[EvidenceClass::ModelInterpretation]);
        let result = adjudicate_conflict(&a, &b);
        assert_eq!(result.winner.as_deref(), Some("a"));
        assert_eq!(result.loser.as_deref(), Some("b"));
        assert_eq!(result.resolution, ConflictResolution::ResolvedByProof);
        assert_eq!(result.winner_state, ClaimState::Confirmed);
        assert_eq!(result.loser_state, ClaimState::Refuted);
    }

    #[test]
    fn adjudicate_citation_beats_unsupported() {
        let a = make_claim("a", "lane-a", &[EvidenceClass::ExactCitation]);
        let b = make_claim("b", "lane-b", &[EvidenceClass::UnsupportedAssertion]);
        let result = adjudicate_conflict(&a, &b);
        assert_eq!(result.winner.as_deref(), Some("a"));
        assert_eq!(result.resolution, ConflictResolution::ResolvedByPrecedence);
    }

    #[test]
    fn adjudicate_equal_precedence_stays_conflicted() {
        let a = make_claim("a", "lane-a", &[EvidenceClass::ModelInterpretation]);
        let b = make_claim("b", "lane-b", &[EvidenceClass::ModelInterpretation]);
        let result = adjudicate_conflict(&a, &b);
        assert!(result.winner.is_none());
        assert_eq!(result.resolution, ConflictResolution::Unresolved);
        assert_eq!(result.winner_state, ClaimState::Conflicted);
        assert_eq!(result.loser_state, ClaimState::Conflicted);
    }

    #[test]
    fn adjudicate_no_evidence_on_either_is_inconclusive() {
        let a = make_claim("a", "lane-a", &[]);
        let b = make_claim("b", "lane-b", &[]);
        let result = adjudicate_conflict(&a, &b);
        assert!(result.winner.is_none());
        assert_eq!(
            result.resolution,
            ConflictResolution::SurfacedAsVerification
        );
        assert_eq!(result.winner_state, ClaimState::Inconclusive);
    }

    #[test]
    fn adjudicate_best_evidence_wins_not_first_listed() {
        // Claim A has ModelInterpretation + ProofReceipt; B has ExactCitation.
        // A's best evidence is ProofReceipt (precedence 0), which beats B's
        // ExactCitation (precedence 2).
        let a = make_claim(
            "a",
            "lane-a",
            &[
                EvidenceClass::ModelInterpretation,
                EvidenceClass::ProofReceipt,
            ],
        );
        let b = make_claim("b", "lane-b", &[EvidenceClass::ExactCitation]);
        let result = adjudicate_conflict(&a, &b);
        assert_eq!(result.winner.as_deref(), Some("a"));
        assert_eq!(result.resolution, ConflictResolution::ResolvedByProof);
    }
}
