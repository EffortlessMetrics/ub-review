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
    source_lane: String,
    subject: String,
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
        let seed = TopicSeed {
            path: Some(comment.path.clone()),
            line: Some(comment.line),
            failure_family: "inline-finding".to_owned(),
            mechanism: stable_mechanism("", &comment.body),
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

        for request in proof_requests {
            if request
                .requested_by
                .iter()
                .any(|lane| lane == &topic.source_lane)
                || request.id.contains(&topic.claim_id)
            {
                push_unique(&mut topic.proof_requests, request.id.clone());
            }
        }
        for receipt in proof_receipts {
            if receipt
                .requested_by
                .iter()
                .any(|lane| lane == &topic.source_lane)
                || receipt
                    .request_ids
                    .iter()
                    .any(|id| id.contains(&topic.claim_id))
            {
                push_unique(&mut topic.proof_receipts, receipt.id.clone());
                topic.evidence.push(EvidenceRef {
                    class: EvidenceClass::ProofReceipt,
                    reference: format!("review/proof_receipts.json#{}", receipt.id),
                    detail: format!("{}: {}", receipt.result, receipt.reason),
                });
            }
        }
        topic.thread_disposition = thread_disposition(topic, &matching_threads);
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

    ClaimGraph {
        schema: crate::artifacts::CLAIM_GRAPH_SCHEMA,
        head_sha: head_sha.to_owned(),
        claims,
        topics: topic_values,
        conflicts: Vec::new(),
        evidence_gaps,
        mode: "active",
    }
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
            let claim_id = topic_claim_id_for_inline(comment);
            let covered_by_current_thread = graph.topics.iter().any(|topic| {
                topic.claim_id == claim_id
                    && topic.path.as_deref() == Some(comment.path.as_str())
                    && topic.anchor == Some(comment.line)
                    && !topic.existing_threads.is_empty()
            });
            if covered_by_current_thread {
                return false;
            }
            seen_claims.insert(topic_claim_id_for_inline(comment))
        })
        .cloned()
        .collect()
}

pub(crate) fn topic_claim_id_for_inline(comment: &ReviewInlineComment) -> String {
    structural_claim_id(
        Some(&comment.path),
        Some(comment.line),
        "inline-finding",
        &stable_mechanism("", &comment.body),
        &comment.body,
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
        .filter(|token| !low_information_thread_token(token))
        .any(|token| token.len() >= 6 && thread_tokens.iter().any(|candidate| candidate == token))
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
                    state: Some("open".to_owned()),
                },
            ],
        }
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
        ensure!(graph.claims[0].state == ClaimState::Confirmed);
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
            request_ids: Vec::new(),
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
        ensure!(corroborated.topics[0].thread_disposition == "corroborated_with_new_evidence");

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
        ensure!(refuted.topics[0].thread_disposition == "refuted_by_new_evidence");

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
        ensure!(tradeoff.topics[0].thread_disposition == "accepted_tradeoff");

        let stale = build_active_claim_graph(
            head,
            std::slice::from_ref(&observation),
            &[],
            &[],
            &[],
            &[],
            &context("dddddddddddddddddddddddddddddddddddddddd"),
        );
        ensure!(stale.topics[0].thread_disposition == "superseded_by_head_change");
        Ok(())
    }

    #[test]
    fn active_graph_keeps_inline_delivery_and_proof_receipt_links() -> Result<()> {
        let head = "cccccccccccccccccccccccccccccccccccccccc";
        let graph = build_active_claim_graph(
            head,
            &[],
            &[ReviewInlineComment {
                lane: "tests".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: "src/parser.rs".to_owned(),
                line: 12,
                side: "RIGHT".to_owned(),
                body: "Finding".to_owned(),
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
        ensure!(reconciled.len() == 1);
        ensure!(reconciled[0].body == "attribute lowering finding");
        Ok(())
    }
}
