//! Active current-head review topics.
//!
//! The legacy claim graph remains useful for its typed adjudication helpers,
//! but the production artifact must also carry the review surfaces that make a
//! claim actionable: current-head identity, thread links, proof links, and
//! delivery state. This module compiles those values at the final compiler
//! boundary without deleting the underlying observations or candidates.

use std::collections::BTreeMap;

use crate::*;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReviewTopic {
    pub(crate) claim_id: String,
    pub(crate) head_sha: String,
    pub(crate) path: Option<String>,
    pub(crate) anchor: Option<u32>,
    pub(crate) symbol: Option<String>,
    pub(crate) failure_family: String,
    pub(crate) mechanism: String,
    pub(crate) status: String,
    /// Current-head reconciliation result against the imported PR thread.
    pub(crate) thread_disposition: String,
    pub(crate) severity: String,
    pub(crate) evidence: Vec<EvidenceRef>,
    pub(crate) existing_threads: Vec<String>,
    pub(crate) stale_threads: Vec<String>,
    pub(crate) proof_requests: Vec<String>,
    pub(crate) proof_receipts: Vec<String>,
    pub(crate) delivery: String,
    /// Deterministic handoff for the delivery seam. This is a plan only;
    /// GitHub confirmation belongs to the posting transaction.
    pub(crate) planned_action: String,
    pub(crate) planned_thread_id: Option<String>,
    pub(crate) source_lane: String,
    pub(crate) subject: String,
}

#[derive(Clone, Debug)]
struct TopicSeed {
    path: Option<String>,
    line: Option<u32>,
    failure_family: String,
    mechanism: String,
    status: String,
    severity: String,
    evidence: Vec<EvidenceRef>,
    source_lane: String,
    subject: String,
    delivery: String,
}

pub(crate) fn build_active_claim_graph(
    head_sha: &str,
    observations: &[Observation],
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    proof_requests: &[ProofRequest],
    proof_receipts: &[ProofReceipt],
    thread_context: &PrThreadContext,
) -> ClaimGraph {
    let mut topics = BTreeMap::<String, ReviewTopic>::new();

    for observation in observations {
        let seed = TopicSeed {
            path: observation.path.clone(),
            line: observation.line,
            failure_family: observation.kind.clone(),
            mechanism: stable_mechanism(&observation.dedupe_key, &observation.claim),
            status: claim_status_key(&observation.status),
            severity: observation.severity.clone(),
            evidence: observation
                .evidence
                .iter()
                .take(3)
                .map(|detail| EvidenceRef {
                    class: EvidenceClass::ModelInterpretation,
                    reference: format!("observation:{}", observation.id),
                    detail: detail.clone(),
                })
                .collect(),
            source_lane: observation.lane.clone(),
            subject: observation.claim.clone(),
            delivery: "no-human-surface".to_owned(),
        };
        upsert_topic(&mut topics, head_sha, seed);
    }

    for comment in inline_comments {
        let matching_observation = observations.iter().find(|observation| {
            observation.path.as_deref() == Some(comment.path.as_str())
                && observation.line == Some(comment.line)
                && subject_tokens_overlap(&observation.claim, &comment.body)
        });
        let seed = TopicSeed {
            path: Some(comment.path.clone()),
            line: Some(comment.line),
            failure_family: matching_observation
                .map(|observation| observation.kind.clone())
                .unwrap_or_else(|| "inline-finding".to_owned()),
            mechanism: matching_observation
                .map(|observation| stable_mechanism(&observation.dedupe_key, &observation.claim))
                .unwrap_or_else(|| stable_mechanism("", &comment.body)),
            status: "confirmed".to_owned(),
            severity: comment.severity.clone(),
            evidence: vec![EvidenceRef {
                class: EvidenceClass::ValidatedFact,
                reference: format!("inline:{}:{}", comment.path, comment.line),
                detail: comment.evidence.clone(),
            }],
            source_lane: comment.lane.clone(),
            subject: comment.body.clone(),
            delivery: "inline-candidate".to_owned(),
        };
        upsert_topic(&mut topics, head_sha, seed);
    }

    for finding in summary_only_findings {
        let seed = TopicSeed {
            path: None,
            line: None,
            failure_family: "summary-finding".to_owned(),
            mechanism: stable_mechanism("", &finding.reason),
            status: "supported".to_owned(),
            severity: finding.severity.clone(),
            evidence: vec![EvidenceRef {
                class: EvidenceClass::ModelInterpretation,
                reference: "summary-only-finding".to_owned(),
                detail: finding.evidence.clone(),
            }],
            source_lane: finding.lane.clone(),
            subject: finding.reason.clone(),
            delivery: "summary-only".to_owned(),
        };
        upsert_topic(&mut topics, head_sha, seed);
    }

    for topic in topics.values_mut() {
        let matching_threads = thread_context
            .threads
            .iter()
            .filter(|thread| same_surface(topic, thread))
            .collect::<Vec<_>>();
        for thread in &matching_threads {
            let is_current = thread
                .commit_id
                .as_deref()
                .is_some_and(|commit| commit.eq_ignore_ascii_case(head_sha));
            if is_current {
                push_unique(&mut topic.existing_threads, thread.id.clone());
            } else {
                push_unique(&mut topic.stale_threads, thread.id.clone());
            }
        }
        // Multiple eligible current threads use a stable lexical tie-breaker
        // for the delivery handoff; input order is not a contract.
        topic.existing_threads.sort();
        topic.stale_threads.sort();

        let mut claim_bindings = vec![topic.claim_id.as_str()];
        for observation in observations.iter().filter(|observation| {
            observation.lane == topic.source_lane
                && observation.path == topic.path
                && observation.line == topic.anchor
                && subject_tokens_overlap(&observation.claim, &topic.subject)
        }) {
            if !observation.id.is_empty() {
                claim_bindings.push(observation.id.as_str());
            }
            if !observation.dedupe_key.is_empty() {
                claim_bindings.push(observation.dedupe_key.as_str());
            }
        }
        for request in proof_requests {
            if claim_bindings.iter().any(|binding| request.id == *binding) {
                push_unique(&mut topic.proof_requests, request.id.clone());
            }
        }
        for receipt in proof_receipts {
            if !receipt.head.eq_ignore_ascii_case(head_sha) {
                continue;
            }
            let receipt_matches = receipt.request_ids.iter().any(|receipt_request_id| {
                claim_bindings
                    .iter()
                    .any(|binding| receipt_request_id == *binding)
                    || topic
                        .proof_requests
                        .iter()
                        .any(|topic_request_id| receipt_request_id == topic_request_id)
            });
            if receipt_matches {
                push_unique(&mut topic.proof_receipts, receipt.id.clone());
                topic.evidence.push(EvidenceRef {
                    class: EvidenceClass::ProofReceipt,
                    reference: format!("review/proof_receipts.json#{}", receipt.id),
                    detail: format!("{}: {}", receipt.result, receipt.reason),
                });
            }
        }
        topic.thread_disposition = thread_disposition(topic, &matching_threads);
        topic.planned_action = planned_action(topic);
        topic.planned_thread_id = if topic.planned_action == "reply" {
            topic.existing_threads.first().cloned()
        } else {
            None
        };
    }

    let mut claims = Vec::with_capacity(topics.len());
    let mut evidence_gaps = Vec::new();
    let mut topic_values = topics.into_values().collect::<Vec<_>>();
    topic_values.sort_by(|left, right| left.claim_id.cmp(&right.claim_id));
    for topic in &topic_values {
        if topic.evidence.is_empty() {
            evidence_gaps.push(ClaimEvidenceGap {
                claim_id: topic.claim_id.clone(),
                needed_evidence_class: "proof-receipt".to_owned(),
                detail: format!("Claim `{}` has no deterministic evidence", topic.subject),
            });
        }
        claims.push(ClaimNode {
            id: topic.claim_id.clone(),
            subject: topic.subject.clone(),
            source_lane: topic.source_lane.clone(),
            state: claim_state(&topic.status),
            supporting_evidence: topic.evidence.clone(),
            contradicting_evidence: Vec::new(),
            relevance: relevance_for_path(topic.path.as_deref()),
            severity: topic.severity.clone(),
        });
    }

    let mut graph = ClaimGraph {
        schema: crate::artifacts::CLAIM_GRAPH_SCHEMA,
        head_sha: head_sha.to_owned(),
        claims,
        topics: topic_values,
        conflicts: Vec::new(),
        evidence_gaps,
        mode: "active",
    };
    let conflict_pairs = explicit_conflict_pairs(
        &graph.topics,
        inline_comments,
        summary_only_findings,
        observations,
    );
    adjudicate_claim_graph_conflicts(&mut graph, &conflict_pairs);
    graph
}

/// Keep the final compiler from opening a second current-head comment at a
/// surface already covered by an existing thread. Stale threads do not block
/// delivery, and duplicate candidates for the same structural inline claim
/// collapse to one surface. The graph retains every omitted claim and its
/// thread references for artifact consumers.
pub(crate) fn reconcile_inline_comments(
    graph: &ClaimGraph,
    comments: &[ReviewInlineComment],
) -> Vec<ReviewInlineComment> {
    let mut seen_claims = std::collections::BTreeSet::new();
    comments
        .iter()
        .filter(|comment| {
            let claim_id = topic_claim_id_for_inline(comment, &[]);
            let refuted_by_adjudication = graph.topics.iter().any(|topic| {
                topic_is_adjudicated_loser(graph, topic)
                    && (topic.claim_id == claim_id
                        || subject_tokens_overlap(&topic.subject, &comment.body))
                    && topic.path.as_deref() == Some(comment.path.as_str())
                    && topic.anchor == Some(comment.line)
            });
            if refuted_by_adjudication {
                return false;
            }
            let covered_by_current_thread = graph.topics.iter().any(|topic| {
                (topic.claim_id == claim_id
                    || subject_tokens_overlap(&topic.subject, &comment.body))
                    && topic.path.as_deref() == Some(comment.path.as_str())
                    && topic.anchor == Some(comment.line)
                    && !topic.existing_threads.is_empty()
            });
            if covered_by_current_thread {
                return false;
            }
            seen_claims.insert(topic_claim_id_for_inline(comment, &[]))
        })
        .cloned()
        .collect()
}

/// Apply the same current-head adjudication to summary-only findings. A
/// refuted summary surface remains in the claim graph for auditability but
/// must not compete with the proof-backed disposition in public prose.
pub(crate) fn reconcile_summary_only_findings(
    graph: &ClaimGraph,
    findings: &[SummaryOnlyFinding],
) -> Vec<SummaryOnlyFinding> {
    findings
        .iter()
        .filter(|finding| {
            !graph.topics.iter().any(|topic| {
                topic_is_adjudicated_loser(graph, topic)
                    && subject_tokens_overlap(&topic.subject, &finding.reason)
                    && topic.source_lane == finding.lane
            })
        })
        .cloned()
        .collect()
}

/// Reconcile the two public compiler vectors as one surface set. Inline
/// delivery wins when the exact normalized claim is also present as a summary
/// finding; the summary copy is retained in the claim graph and accounted for
/// by the compiler reconciliation receipt.
pub(crate) fn reconcile_public_surfaces(
    graph: &ClaimGraph,
    comments: &[ReviewInlineComment],
    findings: &[SummaryOnlyFinding],
) -> (Vec<ReviewInlineComment>, Vec<SummaryOnlyFinding>) {
    let reconciled_comments = reconcile_inline_comments(graph, comments);
    let mut reconciled_findings = reconcile_summary_only_findings(graph, findings);
    reconciled_findings.retain(|finding| {
        reconciled_comments
            .iter()
            .filter(|comment| same_public_claim(comment.body.as_str(), finding.reason.as_str()))
            .count()
            != 1
    });
    (reconciled_comments, reconciled_findings)
}

/// Compare only an exact normalized public claim. Similarity is deliberately
/// not enough to suppress a distinct nearby finding.
pub(crate) fn same_public_claim(left: &str, right: &str) -> bool {
    let left = left
        .split_once(']')
        .filter(|(prefix, _)| prefix.starts_with('['))
        .map_or(left, |(_, body)| body.trim());
    let left = canonical_text(left);
    let right = canonical_text(right);
    !left.is_empty() && left == right
}

fn planned_action(topic: &ReviewTopic) -> String {
    match topic.thread_disposition.as_str() {
        "already_covered"
        | "accepted_tradeoff"
        | "fixed_on_current_head"
        | "superseded_by_head_change" => "none".to_owned(),
        "corroborated_with_new_evidence" | "refuted_by_new_evidence"
            if !topic.existing_threads.is_empty() =>
        {
            "reply".to_owned()
        }
        "novel" if topic.path.is_some() && topic.anchor.is_some() => "inline".to_owned(),
        "novel" => "summary".to_owned(),
        _ => "none".to_owned(),
    }
}

pub(crate) fn topic_is_adjudicated_loser(graph: &ClaimGraph, topic: &ReviewTopic) -> bool {
    graph
        .conflicts
        .iter()
        .any(|conflict| conflict.loser.as_deref() == Some(topic.claim_id.as_str()))
}

/// Preserve candidates resolved away by the follow-up pass in the claim
/// graph. They are intentionally absent from public delivery, but their
/// current-head disposition explains why the reviewer stays silent.
pub(crate) fn add_resolved_candidate_topics(
    graph: &mut ClaimGraph,
    head_sha: &str,
    candidates: &[&CandidateRecord],
    thread_context: &PrThreadContext,
) {
    for candidate in candidates {
        let mechanism = stable_mechanism(&candidate.disposition, &candidate.claim);
        let claim_id = structural_claim_id(
            candidate.path.as_deref(),
            candidate.line,
            "resolved-candidate",
            &mechanism,
            &candidate.claim,
        );
        if graph.claims.iter().any(|claim| claim.id == claim_id) {
            continue;
        }
        let mut topic = ReviewTopic {
            claim_id: claim_id.clone(),
            head_sha: head_sha.to_owned(),
            path: candidate.path.clone(),
            anchor: candidate.line,
            symbol: None,
            failure_family: "resolved-candidate".to_owned(),
            mechanism,
            status: "refuted".to_owned(),
            thread_disposition: "fixed_on_current_head".to_owned(),
            severity: candidate.severity.clone(),
            evidence: vec![EvidenceRef {
                class: EvidenceClass::ModelInterpretation,
                reference: format!("review/resolved_candidates.json#{}", candidate.id),
                detail: candidate.evidence.clone(),
            }],
            existing_threads: Vec::new(),
            stale_threads: Vec::new(),
            proof_requests: Vec::new(),
            proof_receipts: Vec::new(),
            delivery: "no-human-surface".to_owned(),
            planned_action: "none".to_owned(),
            planned_thread_id: None,
            source_lane: candidate.lane.clone(),
            subject: candidate.claim.clone(),
        };
        let matching_threads = thread_context
            .threads
            .iter()
            .filter(|thread| same_surface(&topic, thread))
            .collect::<Vec<_>>();
        for thread in matching_threads {
            if thread
                .commit_id
                .as_deref()
                .is_some_and(|commit| commit.eq_ignore_ascii_case(head_sha))
            {
                push_unique(&mut topic.existing_threads, thread.id.clone());
            } else {
                push_unique(&mut topic.stale_threads, thread.id.clone());
            }
        }
        graph.claims.push(ClaimNode {
            id: claim_id.clone(),
            subject: topic.subject.clone(),
            source_lane: topic.source_lane.clone(),
            state: ClaimState::Refuted,
            supporting_evidence: topic.evidence.clone(),
            contradicting_evidence: Vec::new(),
            relevance: relevance_for_path(topic.path.as_deref()),
            severity: topic.severity.clone(),
        });
        graph.topics.push(topic);
    }
    graph.claims.sort_by(|left, right| left.id.cmp(&right.id));
    graph
        .topics
        .sort_by(|left, right| left.claim_id.cmp(&right.claim_id));
}

pub(crate) fn topic_claim_id_for_inline(
    comment: &ReviewInlineComment,
    observations: &[Observation],
) -> String {
    let matching_observation = observations.iter().find(|observation| {
        observation.path.as_deref() == Some(comment.path.as_str())
            && observation.line == Some(comment.line)
            && subject_tokens_overlap(&observation.claim, &comment.body)
    });
    let failure_family = matching_observation
        .map(|observation| observation.kind.as_str())
        .unwrap_or("inline-finding");
    let mechanism = matching_observation
        .map(|observation| stable_mechanism(&observation.dedupe_key, &observation.claim))
        .unwrap_or_else(|| stable_mechanism("", &comment.body));
    let subject = matching_observation
        .map(|observation| observation.claim.as_str())
        .unwrap_or(&comment.body);
    structural_claim_id(
        Some(&comment.path),
        Some(comment.line),
        failure_family,
        &mechanism,
        subject,
    )
}

pub(crate) fn topic_claim_id_for_summary(finding: &SummaryOnlyFinding) -> String {
    structural_claim_id(
        None,
        None,
        "summary-finding",
        &stable_mechanism("", &finding.reason),
        &finding.reason,
    )
}

fn upsert_topic(topics: &mut BTreeMap<String, ReviewTopic>, head_sha: &str, seed: TopicSeed) {
    let claim_id = structural_claim_id(
        seed.path.as_deref(),
        seed.line,
        &seed.failure_family,
        &seed.mechanism,
        &seed.subject,
    );
    if let Some(topic) = topics.get_mut(&claim_id) {
        if topic.evidence.len() < 6 {
            for evidence in seed.evidence {
                if !topic.evidence.iter().any(|existing| {
                    existing.reference == evidence.reference && existing.detail == evidence.detail
                }) {
                    topic.evidence.push(evidence);
                }
            }
        }
        if topic.delivery == "no-human-surface" {
            topic.delivery = seed.delivery;
        }
        return;
    }
    topics.insert(
        claim_id.clone(),
        ReviewTopic {
            claim_id,
            head_sha: head_sha.to_owned(),
            path: seed.path,
            anchor: seed.line,
            symbol: None,
            failure_family: seed.failure_family,
            mechanism: seed.mechanism,
            status: seed.status,
            thread_disposition: "novel".to_owned(),
            severity: seed.severity,
            evidence: seed.evidence,
            existing_threads: Vec::new(),
            stale_threads: Vec::new(),
            proof_requests: Vec::new(),
            proof_receipts: Vec::new(),
            delivery: seed.delivery,
            planned_action: "none".to_owned(),
            planned_thread_id: None,
            source_lane: seed.source_lane,
            subject: seed.subject,
        },
    );
}

fn structural_claim_id(
    path: Option<&str>,
    line: Option<u32>,
    failure_family: &str,
    mechanism: &str,
    subject: &str,
) -> String {
    let identity = format!(
        "{}|{}|{}|{}|{}",
        path.unwrap_or("<none>").replace('\\', "/"),
        line.map_or_else(|| "<none>".to_owned(), |value| value.to_string()),
        failure_family,
        mechanism,
        canonical_text(subject),
    );
    format!("claim-{}", &sha256_hex(identity.as_bytes())[..16])
}

fn stable_mechanism(dedupe_key: &str, subject: &str) -> String {
    let value = if dedupe_key.trim().is_empty() {
        subject
    } else {
        dedupe_key
    };
    canonical_text(value)
}

fn canonical_text(value: &str) -> String {
    canonical_tokens(value)
        .into_iter()
        .take(24)
        .collect::<Vec<_>>()
        .join(" ")
}

fn canonical_tokens(value: &str) -> Vec<String> {
    value
        .split_whitespace()
        .map(|token| token.trim_matches(|character: char| !character.is_alphanumeric()))
        .filter(|token| !token.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

pub(crate) fn subject_tokens_overlap(left: &str, right: &str) -> bool {
    let left = canonical_tokens(left);
    let right = canonical_tokens(right);
    left.iter()
        .filter(|token| thread_token_is_distinctive(token))
        .filter(|token| right.iter().any(|candidate| candidate == *token))
        .count()
        >= 2
}

fn same_surface(topic: &ReviewTopic, thread: &ReviewThreadRecord) -> bool {
    topic.path.as_deref() == thread.path.as_deref()
        && topic.path.is_some()
        && topic.anchor.is_some()
        && topic.anchor == thread.line
        && thread_body_matches(topic, thread)
}

fn thread_body_matches(topic: &ReviewTopic, thread: &ReviewThreadRecord) -> bool {
    let topic_tokens = canonical_tokens(&format!("{} {}", topic.subject, topic.mechanism));
    let thread_tokens = canonical_tokens(&thread.body);
    topic_tokens
        .iter()
        .filter(|token| thread_token_is_distinctive(token))
        .any(|token| thread_tokens.iter().any(|candidate| candidate == token))
}

fn thread_token_is_distinctive(token: &str) -> bool {
    !low_information_thread_token(token)
        && (token.len() >= 6 || matches!(token, "ub" | "ffi" | "leak" | "lock" | "null" | "race"))
}

fn thread_disposition(topic: &ReviewTopic, matching_threads: &[&ReviewThreadRecord]) -> String {
    if topic.existing_threads.is_empty() {
        return if topic.stale_threads.is_empty() {
            "novel".to_owned()
        } else {
            "superseded_by_head_change".to_owned()
        };
    }
    if topic.status == "refuted" {
        return "refuted_by_new_evidence".to_owned();
    }
    if matching_threads.iter().any(|thread| {
        topic.existing_threads.contains(&thread.id) && accepted_tradeoff_thread(thread)
    }) {
        return "accepted_tradeoff".to_owned();
    }
    if !topic.proof_receipts.is_empty() {
        "corroborated_with_new_evidence".to_owned()
    } else {
        "already_covered".to_owned()
    }
}

fn accepted_tradeoff_thread(thread: &ReviewThreadRecord) -> bool {
    if !thread
        .state
        .as_deref()
        .is_some_and(|state| state.eq_ignore_ascii_case("resolved"))
    {
        return false;
    }
    let body = thread.body.to_ascii_lowercase();
    body.contains("accepted tradeoff")
        || body.contains("accepted trade-off")
        || body.contains("intentional tradeoff")
        || body.contains("intentional trade-off")
}

/// Find only conflicts already identified by the surface-aware cross-lane
/// detector. This avoids treating shared vocabulary as contradiction and keeps
/// structural distinctions such as separate parser mechanisms intact.
fn explicit_conflict_pairs(
    topics: &[ReviewTopic],
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    observations: &[Observation],
) -> Vec<(String, String)> {
    let surfaces = conflict_surfaces(inline_comments, summary_only_findings);
    let refutations = observations
        .iter()
        .filter(|observation| observation_is_cross_lane_refutation(observation))
        .collect::<Vec<_>>();
    let mut pairs: Vec<(String, String)> = Vec::new();
    for surface in surfaces {
        for refutation in &refutations {
            if surface.lane == refutation.lane
                || !surface_conflicts_with_refutation(&surface, refutation)
            {
                continue;
            }
            let Some(surface_topic) = topics
                .iter()
                .filter(|topic| {
                    topic.source_lane == surface.lane
                        && topic.path == surface.path
                        && topic.anchor == surface.line
                        && subject_tokens_overlap(&topic.subject, &surface.claim)
                })
                .min_by_key(|topic| topic.claim_id.as_str())
            else {
                continue;
            };
            let Some(refutation_topic) = topics
                .iter()
                .filter(|topic| {
                    topic.source_lane == refutation.lane
                        && topic.path == refutation.path
                        && topic.anchor == refutation.line
                        && subject_tokens_overlap(&topic.subject, &refutation.claim)
                })
                .min_by_key(|topic| topic.claim_id.as_str())
            else {
                continue;
            };
            if surface_topic.claim_id == refutation_topic.claim_id {
                continue;
            }
            let pair = (
                surface_topic.claim_id.clone(),
                refutation_topic.claim_id.clone(),
            );
            if !pairs.iter().any(|existing| {
                (existing.0 == pair.0 && existing.1 == pair.1)
                    || (existing.0 == pair.1 && existing.1 == pair.0)
            }) {
                pairs.push(pair);
            }
        }
    }
    pairs
}

fn low_information_thread_token(token: &str) -> bool {
    matches!(
        token,
        "finding"
            | "issue"
            | "problem"
            | "review"
            | "change"
            | "changed"
            | "code"
            | "line"
            | "path"
            | "check"
            | "needs"
            | "should"
    )
}

fn claim_status_key(status: &str) -> String {
    match status {
        "confirmed" | "refuted" | "parked" | "dropped" | "supported" | "inconclusive" => {
            status.to_owned()
        }
        "open" | "covered" | "demoted" => "needs_evidence".to_owned(),
        _ => "hypothesized".to_owned(),
    }
}

fn claim_state(status: &str) -> ClaimState {
    match status {
        "confirmed" => ClaimState::Confirmed,
        "refuted" => ClaimState::Refuted,
        "parked" => ClaimState::Parked,
        "dropped" => ClaimState::Dropped,
        "supported" => ClaimState::Supported,
        "inconclusive" => ClaimState::Inconclusive,
        "needs_evidence" => ClaimState::NeedsEvidence,
        _ => ClaimState::Hypothesized,
    }
}

fn relevance_for_path(path: Option<&str>) -> RelevancePath {
    match path {
        Some(path) => RelevancePath {
            kind: RelevanceKind::ChangedLine,
            explanation: format!("Claim cites changed file: {path}"),
        },
        None => RelevancePath {
            kind: RelevanceKind::Unresolved,
            explanation: "Claim has no current-head source anchor".to_owned(),
        },
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.contains(&value) {
        values.push(value);
    }
}

#[cfg(test)]
mod tests {
    use anyhow::{Result, ensure};

    use super::*;

    fn context(head: &str) -> PrThreadContext {
        PrThreadContext {
            schema: "ub-review.pr_thread_context.v1".to_owned(),
            status: "seeded".to_owned(),
            max_bytes: 65_536,
            sources: Vec::new(),
            warnings: Vec::new(),
            pull_number: Some(3627),
            title: Some("fixture".to_owned()),
            body: None,
            body_truncated: false,
            thread_context_path: None,
            thread_context: None,
            thread_context_truncated: false,
            threads: vec![
                ReviewThreadRecord {
                    id: "current-thread".to_owned(),
                    kind: "review-comments".to_owned(),
                    author: "factory-droid[bot]".to_owned(),
                    body: "subscript finding".to_owned(),
                    path: Some("src/parser.rs".to_owned()),
                    line: Some(12),
                    commit_id: Some(head.to_owned()),
                    head_binding: "current".to_owned(),
                    state: Some("open".to_owned()),
                },
                ReviewThreadRecord {
                    id: "stale-thread".to_owned(),
                    kind: "review-comments".to_owned(),
                    author: "factory-droid[bot]".to_owned(),
                    body: "old subscript finding".to_owned(),
                    path: Some("src/parser.rs".to_owned()),
                    line: Some(12),
                    commit_id: Some("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned()),
                    head_binding: "stale".to_owned(),
                    state: Some("open".to_owned()),
                },
            ],
        }
    }

    #[test]
    fn short_technical_thread_tokens_remain_distinctive() -> Result<()> {
        for token in ["ub", "ffi", "leak", "lock", "null", "race", "subscript"] {
            ensure!(thread_token_is_distinctive(token));
        }
        ensure!(!thread_token_is_distinctive("code"));
        ensure!(!thread_token_is_distinctive("line"));
        Ok(())
    }

    #[test]
    fn unrelated_same_lane_receipt_does_not_attach_or_suppress() -> Result<()> {
        let head = "1212121212121212121212121212121212121212";
        let observation = Observation {
            schema: "observation".to_owned(),
            id: "claim-linked".to_owned(),
            lane: "tests".to_owned(),
            question: "parser subscript behavior".to_owned(),
            claim: "Parser subscript finding".to_owned(),
            kind: "bug".to_owned(),
            status: "confirmed".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            fingerprint: "linked-fingerprint".to_owned(),
            evidence: Vec::new(),
            dedupe_key: "parser-subscript".to_owned(),
            source: "model".to_owned(),
        };
        let comment = ReviewInlineComment {
            lane: "tests".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/parser.rs".to_owned(),
            line: 12,
            side: "RIGHT".to_owned(),
            body: "Parser subscript finding".to_owned(),
            evidence: "changed-line evidence".to_owned(),
            suggestion: None,
        };
        let unrelated = ProofReceipt {
            schema: "proof".to_owned(),
            id: "receipt-unrelated".to_owned(),
            kind: "focused-test".to_owned(),
            base: "3434343434343434343434343434343434343434".to_owned(),
            head: head.to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["tests".to_owned()],
            request_ids: vec!["different-claim".to_owned()],
            commands: Vec::new(),
            result: "discriminating".to_owned(),
            reason: "answers a different request".to_owned(),
        };
        let graph = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            std::slice::from_ref(&comment),
            &[],
            &[],
            std::slice::from_ref(&unrelated),
            &PrThreadContext {
                threads: Vec::new(),
                ..context(head)
            },
        );
        ensure!(
            graph
                .topics
                .iter()
                .all(|topic| topic.proof_receipts.is_empty())
        );
        ensure!(graph.conflicts.is_empty());
        ensure!(reconcile_inline_comments(&graph, std::slice::from_ref(&comment)).len() == 1);
        Ok(())
    }

    #[test]
    fn inline_claim_id_uses_matching_observation_identity() -> Result<()> {
        let observation = Observation {
            schema: "observation".to_owned(),
            id: "observation-identity".to_owned(),
            lane: "tests".to_owned(),
            question: "parser subscript behavior".to_owned(),
            claim: "Parser subscript finding".to_owned(),
            kind: "bug".to_owned(),
            status: "confirmed".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            fingerprint: "fingerprint".to_owned(),
            evidence: Vec::new(),
            dedupe_key: "parser-subscript".to_owned(),
            source: "model".to_owned(),
        };
        let comment = ReviewInlineComment {
            lane: "tests".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/parser.rs".to_owned(),
            line: 12,
            side: "RIGHT".to_owned(),
            body: "Parser subscript finding".to_owned(),
            evidence: "receipt".to_owned(),
            suggestion: None,
        };
        let expected = structural_claim_id(
            Some("src/parser.rs"),
            Some(12),
            "bug",
            "parser-subscript",
            "Parser subscript finding",
        );
        ensure!(topic_claim_id_for_inline(&comment, &[observation]) == expected);
        Ok(())
    }

    #[test]
    fn active_graph_links_current_threads_and_separates_stale_threads() -> Result<()> {
        let head = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
        let graph = build_active_claim_graph(
            head,
            &[Observation {
                schema: "observation".to_owned(),
                id: "observation-1".to_owned(),
                lane: "tests".to_owned(),
                question: "test".to_owned(),
                claim: "later subscript is dropped".to_owned(),
                kind: "bug".to_owned(),
                status: "confirmed".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: Some("src/parser.rs".to_owned()),
                line: Some(12),
                fingerprint: "fingerprint".to_owned(),
                evidence: vec!["focused receipt".to_owned()],
                dedupe_key: "later-subscript".to_owned(),
                source: "test".to_owned(),
            }],
            &[],
            &[],
            &[],
            &[],
            &context(head),
        );
        ensure!(graph.mode == "active");
        ensure!(graph.head_sha == head);
        ensure!(graph.topics.len() == 1);
        ensure!(graph.topics[0].existing_threads == ["current-thread"]);
        ensure!(graph.topics[0].stale_threads == ["stale-thread"]);
        ensure!(graph.topics[0].thread_disposition == "already_covered");
        assert_eq!(graph.topics[0].planned_action, "none");
        assert_eq!(graph.topics[0].planned_thread_id, None);
        ensure!(graph.claims[0].state == ClaimState::Confirmed);
        Ok(())
    }

    #[test]
    fn active_graph_resolves_explicit_conflict_using_proof_precedence() -> Result<()> {
        let head = "dddddddddddddddddddddddddddddddddddddddd";
        let candidate = ReviewInlineComment {
            lane: "lane-a".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/buffer.rs".to_owned(),
            line: 10,
            side: "RIGHT".to_owned(),
            body: "Buffer resize preserves active view length".to_owned(),
            evidence: "validated active view length on the changed line".to_owned(),
            suggestion: None,
        };
        let refutation = Observation {
            schema: "observation".to_owned(),
            id: "refutation-1".to_owned(),
            lane: "lane-b".to_owned(),
            question: "does the resize preserve the view?".to_owned(),
            claim: "Buffer resize loses active view length".to_owned(),
            kind: "resolved-check".to_owned(),
            status: "refuted".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium".to_owned(),
            path: Some("src/buffer.rs".to_owned()),
            line: Some(10),
            fingerprint: "refutation-fingerprint".to_owned(),
            evidence: vec!["focused proof refutes active view length".to_owned()],
            dedupe_key: "buffer-resize-view-length".to_owned(),
            source: "tests".to_owned(),
        };
        let receipt = ProofReceipt {
            schema: "proof".to_owned(),
            id: "proof-refutation-1".to_owned(),
            kind: "focused-test".to_owned(),
            base: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned(),
            head: head.to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["lane-b".to_owned()],
            request_ids: vec![refutation.id.clone()],
            commands: Vec::new(),
            result: "head_failed".to_owned(),
            reason: "focused proof refutes the preservation claim".to_owned(),
        };

        let graph = build_active_claim_graph(
            head,
            std::slice::from_ref(&refutation),
            std::slice::from_ref(&candidate),
            &[],
            &[],
            std::slice::from_ref(&receipt),
            &context(head),
        );
        ensure!(graph.conflicts.len() == 1);
        ensure!(graph.conflicts[0].resolution == ConflictResolution::ResolvedByProof);
        ensure!(graph.claims.iter().any(|claim| {
            claim.source_lane == "lane-b" && claim.state == ClaimState::Confirmed
        }));
        ensure!(
            graph.claims.iter().any(|claim| {
                claim.source_lane == "lane-a" && claim.state == ClaimState::Refuted
            })
        );
        ensure!(graph.topics.iter().any(|topic| {
            topic.source_lane == "lane-a" && topic.delivery == "no-human-surface"
        }));
        ensure!(
            reconcile_inline_comments(&graph, std::slice::from_ref(&candidate)).is_empty(),
            "proof-refuted inline surface must not render"
        );
        let summary = SummaryOnlyFinding {
            lane: "lane-a".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            reason: candidate.body.clone(),
            evidence: candidate.evidence.clone(),
        };
        ensure!(
            reconcile_summary_only_findings(&graph, std::slice::from_ref(&summary)).is_empty(),
            "proof-refuted summary surface must not render"
        );
        let distinct = ReviewInlineComment {
            lane: "lane-a".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            path: "src/buffer.rs".to_owned(),
            line: 11,
            side: "RIGHT".to_owned(),
            body: "Buffer resize updates capacity bookkeeping".to_owned(),
            evidence: "separate capacity invariant".to_owned(),
            suggestion: None,
        };
        let graph_with_distinct = build_active_claim_graph(
            head,
            std::slice::from_ref(&refutation),
            &[candidate.clone(), distinct.clone()],
            &[],
            &[],
            std::slice::from_ref(&receipt),
            &context(head),
        );
        let remaining =
            reconcile_inline_comments(&graph_with_distinct, &[candidate, distinct.clone()]);
        ensure!(remaining.len() == 1);
        ensure!(remaining[0].line == distinct.line);
        Ok(())
    }

    #[test]
    fn thread_disposition_tracks_new_evidence_refutation_tradeoff_and_stale_heads() -> Result<()> {
        let head = "cccccccccccccccccccccccccccccccccccccccc";
        let observation = Observation {
            schema: "observation".to_owned(),
            id: "observation-disposition".to_owned(),
            lane: "tests".to_owned(),
            question: "answer".to_owned(),
            claim: "later subscript is dropped".to_owned(),
            kind: "bug".to_owned(),
            status: "confirmed".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            fingerprint: "fingerprint".to_owned(),
            evidence: vec!["focused proof".to_owned()],
            dedupe_key: "later-subscript".to_owned(),
            source: "test".to_owned(),
        };
        let receipt = ProofReceipt {
            schema: "proof".to_owned(),
            id: "proof-disposition".to_owned(),
            kind: "focused-test".to_owned(),
            base: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            head: head.to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["tests".to_owned()],
            request_ids: vec![observation.id.clone()],
            commands: Vec::new(),
            result: "discriminating".to_owned(),
            reason: "focused proof confirms the thread".to_owned(),
        };
        let corroborated = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            &[],
            &[],
            &[],
            std::slice::from_ref(&receipt),
            &context(head),
        );
        assert_eq!(
            corroborated.topics[0].thread_disposition,
            "corroborated_with_new_evidence"
        );
        assert_eq!(corroborated.topics[0].planned_action, "reply");
        assert_eq!(
            corroborated.topics[0].planned_thread_id,
            Some("current-thread".to_owned())
        );

        let mut refuted_observation = observation.clone();
        refuted_observation.status = "refuted".to_owned();
        let refuted = build_active_claim_graph(
            head,
            &[refuted_observation],
            &[],
            &[],
            &[],
            &[],
            &context(head),
        );
        assert_eq!(
            refuted.topics[0].thread_disposition,
            "refuted_by_new_evidence"
        );
        assert_eq!(refuted.topics[0].planned_action, "reply");
        assert_eq!(
            refuted.topics[0].planned_thread_id,
            Some("current-thread".to_owned())
        );

        let mut unanchored_observation = observation.clone();
        unanchored_observation.path = None;
        unanchored_observation.line = None;
        let novel_summary = build_active_claim_graph(
            head,
            std::slice::from_ref(&unanchored_observation),
            &[],
            &[],
            &[],
            &[],
            &PrThreadContext {
                threads: Vec::new(),
                ..context(head)
            },
        );
        assert_eq!(novel_summary.topics[0].thread_disposition, "novel");
        assert_eq!(novel_summary.topics[0].planned_action, "summary");
        assert_eq!(novel_summary.topics[0].planned_thread_id, None);

        let mut tradeoff_context = context(head);
        tradeoff_context.threads[0].state = Some("resolved".to_owned());
        tradeoff_context.threads[0].body =
            "accepted tradeoff: keep the current subscript behavior".to_owned();
        let tradeoff = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            &[],
            &[],
            &[],
            &[],
            &tradeoff_context,
        );
        assert_eq!(tradeoff.topics[0].thread_disposition, "accepted_tradeoff");
        assert_eq!(tradeoff.topics[0].planned_action, "none");
        assert_eq!(tradeoff.topics[0].planned_thread_id, None);

        let stale = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            &[],
            &[],
            &[],
            &[],
            &context("dddddddddddddddddddddddddddddddddddddddd"),
        );
        assert_eq!(
            stale.topics[0].thread_disposition,
            "superseded_by_head_change"
        );
        assert_eq!(stale.topics[0].planned_action, "none");
        assert_eq!(stale.topics[0].planned_thread_id, None);
        Ok(())
    }

    #[test]
    fn active_graph_does_not_reuse_stale_proof_receipts() -> Result<()> {
        let head = "ffffffffffffffffffffffffffffffffffffffff";
        let observation = Observation {
            schema: "observation".to_owned(),
            id: "observation-stale-proof".to_owned(),
            lane: "tests".to_owned(),
            question: "answer".to_owned(),
            claim: "later subscript is dropped".to_owned(),
            kind: "bug".to_owned(),
            status: "confirmed".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            fingerprint: "fingerprint".to_owned(),
            evidence: Vec::new(),
            dedupe_key: "later-subscript".to_owned(),
            source: "test".to_owned(),
        };
        let stale_receipt = ProofReceipt {
            schema: "proof".to_owned(),
            id: "proof-stale-head".to_owned(),
            kind: "focused-test".to_owned(),
            base: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            head: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee".to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["tests".to_owned()],
            request_ids: Vec::new(),
            commands: Vec::new(),
            result: "discriminating".to_owned(),
            reason: "proof belongs to an earlier head".to_owned(),
        };

        let graph = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            &[],
            &[],
            &[],
            std::slice::from_ref(&stale_receipt),
            &context(head),
        );

        ensure!(graph.topics.len() == 1);
        ensure!(graph.topics[0].proof_receipts.is_empty());
        ensure!(graph.topics[0].evidence.is_empty());
        assert_eq!(graph.topics[0].planned_action, "none");
        assert_eq!(graph.topics[0].planned_thread_id, None);
        ensure!(graph.evidence_gaps.len() == 1);
        Ok(())
    }

    #[test]
    fn resolved_candidates_remain_as_fixed_current_head_topics() -> Result<()> {
        let head = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let candidate = CandidateRecord {
            schema: "candidate".to_owned(),
            id: "candidate-fixed".to_owned(),
            lane: "tests".to_owned(),
            source: "inline-comment".to_owned(),
            status: "confirmed".to_owned(),
            disposition: "refuted".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            claim: "subscript finding".to_owned(),
            evidence: "follow-up proof refuted the candidate".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            side: Some("RIGHT".to_owned()),
        };
        let mut graph = build_active_claim_graph(
            head,
            &[],
            &[],
            &[],
            &[],
            &[],
            &context("dddddddddddddddddddddddddddddddddddddddd"),
        );
        add_resolved_candidate_topics(
            &mut graph,
            head,
            &[&candidate],
            &context("dddddddddddddddddddddddddddddddddddddddd"),
        );
        ensure!(graph.topics.len() == 1);
        ensure!(graph.claims.len() == 1);
        ensure!(graph.topics[0].thread_disposition == "fixed_on_current_head");
        assert_eq!(graph.topics[0].planned_action, "none");
        assert_eq!(graph.topics[0].planned_thread_id, None);
        ensure!(graph.topics[0].stale_threads == ["current-thread", "stale-thread"]);
        ensure!(graph.claims[0].state == ClaimState::Refuted);
        Ok(())
    }

    #[test]
    fn active_graph_keeps_inline_delivery_and_proof_receipt_links() -> Result<()> {
        let head = "cccccccccccccccccccccccccccccccccccccccc";
        let observation = Observation {
            schema: "observation".to_owned(),
            id: "proof-1".to_owned(),
            lane: "tests".to_owned(),
            question: "parser subscript behavior".to_owned(),
            claim: "Parser subscript finding".to_owned(),
            kind: "bug".to_owned(),
            status: "confirmed".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            fingerprint: "proof-link-fingerprint".to_owned(),
            evidence: vec!["receipt".to_owned()],
            dedupe_key: "parser-subscript".to_owned(),
            source: "test".to_owned(),
        };
        let graph = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            &[ReviewInlineComment {
                lane: "tests".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: "src/parser.rs".to_owned(),
                line: 12,
                side: "RIGHT".to_owned(),
                body: "Parser subscript finding".to_owned(),
                evidence: "receipt".to_owned(),
                suggestion: None,
            }],
            &[],
            &[ProofRequest {
                schema: "proof".to_owned(),
                id: "proof-1".to_owned(),
                lane: "tests".to_owned(),
                requested_by: vec!["tests".to_owned()],
                command: "cargo test --locked".to_owned(),
                reason: "answer claim".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 60,
                required: false,
                status: "executed".to_owned(),
            }],
            &[],
            &PrThreadContext {
                threads: Vec::new(),
                ..context(head)
            },
        );
        ensure!(graph.topics.len() == 1);
        ensure!(graph.topics[0].delivery == "inline-candidate");
        assert_eq!(graph.topics[0].planned_action, "inline");
        assert_eq!(graph.topics[0].planned_thread_id, None);
        ensure!(graph.topics[0].proof_requests == ["proof-1"]);
        Ok(())
    }

    #[test]
    fn current_thread_reconciliation_suppresses_only_current_surface_duplicates() -> Result<()> {
        let head = "dddddddddddddddddddddddddddddddddddddddd";
        let comments = vec![
            ReviewInlineComment {
                lane: "tests".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: "src/parser.rs".to_owned(),
                line: 12,
                side: "RIGHT".to_owned(),
                body: "subscript finding".to_owned(),
                evidence: "receipt".to_owned(),
                suggestion: None,
            },
            ReviewInlineComment {
                lane: "tests".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/other.rs".to_owned(),
                line: 9,
                side: "RIGHT".to_owned(),
                body: "novel".to_owned(),
                evidence: "receipt".to_owned(),
                suggestion: None,
            },
        ];
        let graph = build_active_claim_graph(head, &[], &comments, &[], &[], &[], &context(head));
        let reconciled = reconcile_inline_comments(&graph, &comments);
        ensure!(reconciled.len() == 1);
        ensure!(reconciled[0].path == "src/other.rs");
        Ok(())
    }

    #[test]
    fn current_thread_does_not_suppress_distinct_claim_at_same_anchor() -> Result<()> {
        let head = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let comments = vec![
            ReviewInlineComment {
                lane: "tests".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: "src/parser.rs".to_owned(),
                line: 12,
                side: "RIGHT".to_owned(),
                body: "subscript finding".to_owned(),
                evidence: "receipt".to_owned(),
                suggestion: None,
            },
            ReviewInlineComment {
                lane: "tests".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/parser.rs".to_owned(),
                line: 12,
                side: "RIGHT".to_owned(),
                body: "attribute lowering finding".to_owned(),
                evidence: "receipt".to_owned(),
                suggestion: None,
            },
        ];
        let graph = build_active_claim_graph(head, &[], &comments, &[], &[], &[], &context(head));
        let reconciled = reconcile_inline_comments(&graph, &comments);
        assert_eq!(reconciled.len(), 1);
        assert_eq!(reconciled[0].body, "attribute lowering finding");
        Ok(())
    }

    #[test]
    fn public_surface_reconciliation_prefers_inline_for_exact_duplicate_claims() -> Result<()> {
        let head = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";
        let comment = ReviewInlineComment {
            lane: "tests".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/parser.rs".to_owned(),
            line: 12,
            side: "RIGHT".to_owned(),
            body: "[tests] Parser drops the later subscript".to_owned(),
            evidence: "focused receipt".to_owned(),
            suggestion: None,
        };
        let duplicate = SummaryOnlyFinding {
            lane: "reviewer".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            reason: "Parser drops the later subscript".to_owned(),
            evidence: "source inspection".to_owned(),
        };
        let distinct = SummaryOnlyFinding {
            reason: "Parser drops the later percent sigil".to_owned(),
            ..duplicate.clone()
        };
        let graph = build_active_claim_graph(
            head,
            &[],
            std::slice::from_ref(&comment),
            &[duplicate.clone(), distinct.clone()],
            &[],
            &[],
            &PrThreadContext {
                threads: Vec::new(),
                ..context(head)
            },
        );
        let (comments, findings) = reconcile_public_surfaces(
            &graph,
            std::slice::from_ref(&comment),
            &[duplicate.clone(), distinct.clone()],
        );
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].path, comment.path);
        assert_eq!(comments[0].line, comment.line);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].reason, distinct.reason);
        assert_eq!(
            graph
                .topics
                .iter()
                .find(|topic| topic.path.is_none())
                .map(|topic| topic.planned_action.as_str()),
            Some("summary")
        );

        let second_comment = ReviewInlineComment {
            path: "src/other_parser.rs".to_owned(),
            line: 18,
            ..comment.clone()
        };
        let ambiguous_graph = build_active_claim_graph(
            head,
            &[],
            &[comment.clone(), second_comment.clone()],
            std::slice::from_ref(&duplicate),
            &[],
            &[],
            &PrThreadContext {
                threads: Vec::new(),
                ..context(head)
            },
        );
        let (ambiguous_comments, ambiguous_findings) = reconcile_public_surfaces(
            &ambiguous_graph,
            &[comment, second_comment],
            std::slice::from_ref(&duplicate),
        );
        ensure!(ambiguous_comments.len() == 2);
        ensure!(ambiguous_findings.len() == 1);
        Ok(())
    }

    #[test]
    fn reconcile_public_surfaces_exact_matrix() -> Result<()> {
        let graph = build_shadow_claim_graph("HEAD");
        let comment = |path: &str, line: u32, body: &str| ReviewInlineComment {
            lane: "tests".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: path.to_owned(),
            line,
            side: "RIGHT".to_owned(),
            body: body.to_owned(),
            evidence: "focused receipt".to_owned(),
            suggestion: None,
        };
        let finding = |reason: &str| SummaryOnlyFinding {
            lane: "reviewer".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            reason: reason.to_owned(),
            evidence: "source inspection".to_owned(),
        };
        let exact = comment("src/parser.rs", 12, "Parser drops the later subscript");
        let exact_summary = finding("Parser drops the later subscript");
        let (inline, summary) = reconcile_public_surfaces(
            &graph,
            std::slice::from_ref(&exact),
            std::slice::from_ref(&exact_summary),
        );
        assert_eq!(inline.len(), 1);
        assert_eq!(
            (inline[0].path.as_str(), inline[0].line),
            (exact.path.as_str(), exact.line)
        );
        assert_eq!(summary.len(), 0);

        let similar_summary = finding("Parser drops the later subscript in lowering");
        let (_, similar) = reconcile_public_surfaces(
            &graph,
            std::slice::from_ref(&exact),
            std::slice::from_ref(&similar_summary),
        );
        assert_eq!(similar.len(), 1);
        assert_eq!(similar[0].reason, similar_summary.reason);

        let other_anchor = comment(
            "src/other_parser.rs",
            18,
            "Parser drops the later subscript",
        );
        let (_, ambiguous) = reconcile_public_surfaces(
            &graph,
            &[exact.clone(), other_anchor.clone()],
            std::slice::from_ref(&exact_summary),
        );
        assert_eq!(ambiguous.len(), 1);
        assert_eq!(ambiguous[0].reason, exact_summary.reason);

        let distinct_summary = finding("Parser drops the later percent sigil");
        let (mixed_inline, mixed_summary) = reconcile_public_surfaces(
            &graph,
            std::slice::from_ref(&exact),
            &[exact_summary.clone(), distinct_summary.clone()],
        );
        assert_eq!(mixed_inline.len(), 1);
        assert_eq!(mixed_inline[0].body, exact.body);
        assert_eq!(mixed_summary.len(), 1);
        assert_eq!(mixed_summary[0].reason, distinct_summary.reason);

        let (summary_only_inline, summary_only) =
            reconcile_public_surfaces(&graph, &[], std::slice::from_ref(&distinct_summary));
        assert_eq!(summary_only_inline.len(), 0);
        assert_eq!(summary_only.len(), 1);
        assert_eq!(summary_only[0].reason, distinct_summary.reason);

        let (inline_only, inline_only_summary) =
            reconcile_public_surfaces(&graph, std::slice::from_ref(&exact), &[]);
        assert_eq!(inline_only.len(), 1);
        assert_eq!(inline_only[0].body, exact.body);
        assert_eq!(inline_only_summary.len(), 0);

        let inline_identity = inline_only
            .iter()
            .map(|comment| (comment.path.as_str(), comment.line, comment.body.as_str()))
            .collect::<Vec<_>>();
        let summary_identity = inline_only_summary
            .iter()
            .map(|finding| (finding.lane.as_str(), finding.reason.as_str()))
            .collect::<Vec<_>>();
        assert_eq!(
            inline_identity,
            vec![("src/parser.rs", 12, "Parser drops the later subscript")]
        );
        assert_eq!(summary_identity, Vec::<(&str, &str)>::new());

        let mixed_second = comment("src/lowering.rs", 21, "Lowering drops the later sigil");
        let mixed_second_summary = finding("Lowering drops the later sigil");
        let (all_inline, all_summary) = reconcile_public_surfaces(
            &graph,
            &[exact.clone(), mixed_second.clone()],
            &[exact_summary, mixed_second_summary],
        );
        assert_eq!(all_inline.len(), 2);
        assert_eq!(all_inline[0].path, exact.path);
        assert_eq!(all_inline[1].path, mixed_second.path);
        assert_eq!(all_summary.len(), 0);
        Ok(())
    }

    #[test]
    fn planned_action_exact_matrix_is_fail_closed() -> Result<()> {
        let head = "1212121212121212121212121212121212121212";
        let observation = Observation {
            schema: "observation".to_owned(),
            id: "planned-action-matrix".to_owned(),
            lane: "tests".to_owned(),
            question: "answer".to_owned(),
            claim: "later subscript is dropped".to_owned(),
            kind: "bug".to_owned(),
            status: "confirmed".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            fingerprint: "planned-action-matrix".to_owned(),
            evidence: vec!["focused receipt".to_owned()],
            dedupe_key: "planned-action-matrix".to_owned(),
            source: "tests".to_owned(),
        };
        let mut topic = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            &[],
            &[],
            &[],
            &[],
            &PrThreadContext {
                threads: Vec::new(),
                ..context(head)
            },
        )
        .topics[0]
            .clone();
        topic.thread_disposition = "novel".to_owned();
        topic.path = Some("src/parser.rs".to_owned());
        topic.anchor = Some(12);
        topic.existing_threads.clear();
        assert_eq!(planned_action(&topic), "inline".to_owned());

        topic.path = None;
        topic.anchor = None;
        assert_eq!(planned_action(&topic), "summary".to_owned());

        topic.thread_disposition = "already_covered".to_owned();
        topic.existing_threads = vec!["current-thread".to_owned()];
        assert_eq!(planned_action(&topic), "none".to_owned());

        topic.thread_disposition = "corroborated_with_new_evidence".to_owned();
        assert_eq!(planned_action(&topic), "reply".to_owned());

        topic.thread_disposition = "refuted_by_new_evidence".to_owned();
        assert_eq!(planned_action(&topic), "reply".to_owned());

        topic.thread_disposition = "accepted_tradeoff".to_owned();
        assert_eq!(planned_action(&topic), "none".to_owned());

        topic.thread_disposition = "superseded_by_head_change".to_owned();
        topic.existing_threads = vec!["stale-thread".to_owned()];
        assert_eq!(planned_action(&topic), "none".to_owned());

        topic.thread_disposition = "fixed_on_current_head".to_owned();
        topic.existing_threads.clear();
        assert_eq!(planned_action(&topic), "none".to_owned());

        topic.thread_disposition = "unknown-disposition".to_owned();
        topic.existing_threads = vec!["current-thread".to_owned()];
        assert_eq!(planned_action(&topic), "none".to_owned());
        Ok(())
    }

    #[test]
    fn planned_delivery_action_names_reply_and_source_thread() -> Result<()> {
        let head = "ffffffffffffffffffffffffffffffffffffffff";
        let observation = Observation {
            schema: "observation".to_owned(),
            id: "observation-reply-plan".to_owned(),
            lane: "tests".to_owned(),
            question: "answer".to_owned(),
            claim: "later subscript is dropped".to_owned(),
            kind: "bug".to_owned(),
            status: "confirmed".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            fingerprint: "reply-plan".to_owned(),
            evidence: vec!["focused proof".to_owned()],
            dedupe_key: "later-subscript".to_owned(),
            source: "tests".to_owned(),
        };
        let receipt = ProofReceipt {
            schema: "proof".to_owned(),
            id: "proof-reply-plan".to_owned(),
            kind: "focused-test".to_owned(),
            base: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            head: head.to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["tests".to_owned()],
            request_ids: vec![observation.id.clone()],
            commands: Vec::new(),
            result: "discriminating".to_owned(),
            reason: "focused proof adds evidence".to_owned(),
        };
        let graph = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            &[],
            &[],
            &[],
            std::slice::from_ref(&receipt),
            &context(head),
        );
        ensure!(graph.topics[0].thread_disposition == "corroborated_with_new_evidence");
        assert_eq!(graph.topics[0].planned_action, "reply");
        assert_eq!(
            graph.topics[0].planned_thread_id,
            Some("current-thread".to_owned())
        );
        let serialized = serde_json::to_value(&graph.topics[0])?;
        assert_eq!(serialized["planned_action"], "reply");
        assert_eq!(serialized["planned_thread_id"], "current-thread");

        let mut multiple_threads = context(head);
        multiple_threads.threads.push(ReviewThreadRecord {
            id: "another-current".to_owned(),
            kind: "review-comments".to_owned(),
            author: "factory-droid[bot]".to_owned(),
            body: "later subscript is dropped".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            commit_id: Some(head.to_owned()),
            head_binding: "current".to_owned(),
            state: Some("open".to_owned()),
        });
        let stable = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            &[],
            &[],
            &[],
            std::slice::from_ref(&receipt),
            &multiple_threads,
        );
        ensure!(stable.topics[0].existing_threads == ["another-current", "current-thread"]);
        assert_eq!(
            stable.topics[0].planned_thread_id,
            Some("another-current".to_owned())
        );

        let mut unknown = stable.topics[0].clone();
        unknown.thread_disposition = "unknown-disposition".to_owned();
        assert_eq!(planned_action(&unknown), "none");
        Ok(())
    }
}
