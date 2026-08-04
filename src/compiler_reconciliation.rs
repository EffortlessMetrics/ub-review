//! Receipt-backed accounting for the final compiler's public surfaces.
//!
//! The final compiler intentionally runs after claim-graph adjudication.  This
//! module records that reduction so artifact consumers can distinguish an
//! explained omission from a lost finding without reimplementing the Rust
//! claim matcher.

use crate::*;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct CompilerSurfaceReceipt {
    pub(crate) surface_id: String,
    pub(crate) kind: &'static str,
    pub(crate) source_artifact: &'static str,
    pub(crate) source_index: usize,
    pub(crate) claim_id: String,
    pub(crate) lane: String,
    pub(crate) subject: String,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) adjudicating_claim_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RemovedCompilerSurfaceReceipt {
    #[serde(flatten)]
    pub(crate) surface: CompilerSurfaceReceipt,
    pub(crate) disposition: &'static str,
    pub(crate) evidence_receipts: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct CompilerReconciliationReceipt {
    pub(crate) schema: &'static str,
    pub(crate) head_sha: String,
    pub(crate) status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    pub(crate) input_surfaces: Vec<CompilerSurfaceReceipt>,
    pub(crate) retained_surfaces: Vec<CompilerSurfaceReceipt>,
    pub(crate) removed_surfaces: Vec<RemovedCompilerSurfaceReceipt>,
}

#[derive(Clone, Debug)]
struct SurfaceIdentity {
    receipt: CompilerSurfaceReceipt,
}

pub(crate) struct CompilerReconciliationInput<'a> {
    pub(crate) head_sha: &'a str,
    pub(crate) observations: &'a [Observation],
    pub(crate) review_inline_comments: &'a [ReviewInlineComment],
    pub(crate) review_summary_only_findings: &'a [SummaryOnlyFinding],
    pub(crate) follow_up_summary_only_findings: &'a [SummaryOnlyFinding],
    pub(crate) resolved_away_candidates: &'a [&'a CandidateRecord],
    pub(crate) final_inline_comments: &'a [ReviewInlineComment],
    pub(crate) final_summary_only_findings: &'a [SummaryOnlyFinding],
    pub(crate) graph: &'a ClaimGraph,
}

pub(crate) fn build_compiler_reconciliation_receipt(
    input: CompilerReconciliationInput<'_>,
) -> Result<CompilerReconciliationReceipt> {
    let input_surfaces = input_surfaces(
        input.observations,
        input.review_inline_comments,
        input.review_summary_only_findings,
        input.follow_up_summary_only_findings,
        input.resolved_away_candidates,
    );
    let final_surfaces = final_surface_keys(
        input.final_inline_comments,
        input.final_summary_only_findings,
    );

    let mut available = input_surfaces.clone();
    let mut retained_surfaces = Vec::with_capacity(final_surfaces.len());
    for (surface_id, kind) in final_surfaces {
        let Some(position) = available.iter().position(|candidate| {
            candidate.receipt.surface_id == surface_id && candidate.receipt.kind == kind
        }) else {
            bail!(
                "final compiler surface {} was not present in reconciled input",
                surface_id
            );
        };
        retained_surfaces.push(available.remove(position).receipt);
    }

    let mut removed_surfaces = Vec::with_capacity(available.len());
    for surface in available {
        let (disposition, adjudicating_claim_ids, evidence_receipts) =
            removal_explanation(&surface, &input_surfaces, &retained_surfaces, input.graph)?;
        removed_surfaces.push(RemovedCompilerSurfaceReceipt {
            surface: CompilerSurfaceReceipt {
                adjudicating_claim_ids,
                ..surface.receipt
            },
            disposition,
            evidence_receipts,
        });
    }

    Ok(CompilerReconciliationReceipt {
        schema: COMPILER_RECONCILIATION_SCHEMA,
        head_sha: input.head_sha.to_owned(),
        status: "ok",
        error: None,
        input_surfaces: input_surfaces
            .into_iter()
            .map(|surface| surface.receipt)
            .collect(),
        retained_surfaces,
        removed_surfaces,
    })
}

pub(crate) fn compiler_reconciliation_failure(
    head_sha: &str,
    error: impl Into<String>,
) -> CompilerReconciliationReceipt {
    CompilerReconciliationReceipt {
        schema: COMPILER_RECONCILIATION_SCHEMA,
        head_sha: head_sha.to_owned(),
        status: "error",
        error: Some(error.into()),
        input_surfaces: Vec::new(),
        retained_surfaces: Vec::new(),
        removed_surfaces: Vec::new(),
    }
}

fn input_surfaces(
    observations: &[Observation],
    review_inline_comments: &[ReviewInlineComment],
    review_summary_only_findings: &[SummaryOnlyFinding],
    follow_up_summary_only_findings: &[SummaryOnlyFinding],
    resolved_away_candidates: &[&CandidateRecord],
) -> Vec<SurfaceIdentity> {
    let mut surfaces = Vec::new();
    for (source_index, comment) in review_inline_comments.iter().enumerate() {
        if resolved_away_candidates
            .iter()
            .any(|candidate| candidate_matches_inline_comment(candidate, comment))
        {
            continue;
        }
        surfaces.push(SurfaceIdentity {
            receipt: inline_surface_receipt(
                comment,
                observations,
                "review/review.json",
                source_index,
            ),
        });
    }
    for (source_index, finding) in review_summary_only_findings.iter().enumerate() {
        if resolved_away_candidates
            .iter()
            .any(|candidate| candidate_matches_summary_finding(candidate, finding))
        {
            continue;
        }
        surfaces.push(SurfaceIdentity {
            receipt: summary_surface_receipt(finding, "review/review.json", source_index),
        });
    }
    for (source_index, finding) in follow_up_summary_only_findings.iter().enumerate() {
        surfaces.push(SurfaceIdentity {
            receipt: summary_surface_receipt(
                finding,
                "review/follow_up_evidence.json",
                source_index,
            ),
        });
    }
    surfaces
}

fn final_surface_keys(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
) -> Vec<(String, &'static str)> {
    inline_comments
        .iter()
        .map(|comment| (inline_surface_id(comment), "inline"))
        .chain(
            summary_only_findings
                .iter()
                .map(|finding| (summary_surface_id(finding), "summary")),
        )
        .collect()
}

fn removal_explanation(
    surface: &SurfaceIdentity,
    input_surfaces: &[SurfaceIdentity],
    retained_surfaces: &[CompilerSurfaceReceipt],
    graph: &ClaimGraph,
) -> Result<(&'static str, Vec<String>, Vec<String>)> {
    let matching_loser_ids = match surface.receipt.kind {
        "inline" => graph_loser_claim_ids_for_inline(graph, &surface.receipt),
        "summary" => graph_loser_claim_ids_for_summary(graph, &surface.receipt),
        other => bail!("unsupported compiler surface kind {other}"),
    };
    if !matching_loser_ids.is_empty() {
        let mut evidence_receipts = matching_loser_ids
            .iter()
            .map(|claim_id| format!("review/claim_graph.json#claims/{claim_id}"))
            .collect::<Vec<_>>();
        for (index, conflict) in graph.conflicts.iter().enumerate() {
            if conflict
                .loser
                .as_ref()
                .is_some_and(|loser| matching_loser_ids.contains(loser))
            {
                evidence_receipts.push(format!("review/claim_graph.json#conflicts/{index}"));
            }
        }
        return Ok((
            "refuted_by_stronger_evidence",
            matching_loser_ids,
            evidence_receipts,
        ));
    }

    let matching_thread_claim_ids = match surface.receipt.kind {
        "inline" => graph_thread_claim_ids_for_inline(graph, &surface.receipt),
        "summary" => graph_thread_claim_ids_for_summary(graph, &surface.receipt),
        other => bail!("unsupported compiler surface kind {other}"),
    };
    if !matching_thread_claim_ids.is_empty() {
        let evidence_receipts = matching_thread_claim_ids
            .iter()
            .flat_map(|claim_id| {
                graph
                    .topics
                    .iter()
                    .filter(move |topic| topic.claim_id == *claim_id)
                    .flat_map(|topic| {
                        topic
                            .existing_threads
                            .iter()
                            .map(|thread_id| format!("review/pr_thread_context.json#{thread_id}"))
                    })
            })
            .collect();
        return Ok((
            "covered_by_current_head_thread",
            matching_thread_claim_ids,
            evidence_receipts,
        ));
    }

    let matching_inline_claims = retained_surfaces
        .iter()
        .filter(|retained| {
            retained.kind == "inline"
                && same_public_claim(&retained.subject, &surface.receipt.subject)
        })
        .collect::<Vec<_>>();
    if surface.receipt.kind == "summary" && matching_inline_claims.len() == 1 {
        let retained_inline = matching_inline_claims[0];
        return Ok((
            "duplicate_cross_surface",
            vec![retained_inline.claim_id.clone()],
            vec![format!(
                "review/claim_graph.json#claims/{}",
                retained_inline.claim_id
            )],
        ));
    }

    if retained_surfaces.iter().any(|retained| {
        retained.surface_id == surface.receipt.surface_id && retained.kind == surface.receipt.kind
    }) {
        return Ok((
            "duplicate_structurally_identical",
            vec![surface.receipt.claim_id.clone()],
            vec![format!(
                "review/claim_graph.json#claims/{}",
                surface.receipt.claim_id
            )],
        ));
    }

    let input_count = input_surfaces
        .iter()
        .filter(|candidate| {
            candidate.receipt.surface_id == surface.receipt.surface_id
                && candidate.receipt.kind == surface.receipt.kind
        })
        .count();
    bail!(
        "compiler surface {} was removed without an adjudication, current-head thread, or duplicate receipt (input count {input_count})",
        surface.receipt.surface_id
    )
}

fn inline_surface_receipt(
    comment: &ReviewInlineComment,
    observations: &[Observation],
    source_artifact: &'static str,
    source_index: usize,
) -> CompilerSurfaceReceipt {
    CompilerSurfaceReceipt {
        surface_id: inline_surface_id(comment),
        kind: "inline",
        source_artifact,
        source_index,
        claim_id: topic_claim_id_for_inline(comment, observations),
        lane: comment.lane.clone(),
        subject: comment.body.clone(),
        path: Some(comment.path.clone()),
        line: Some(comment.line),
        adjudicating_claim_ids: Vec::new(),
    }
}

fn inline_surface_id(comment: &ReviewInlineComment) -> String {
    compiler_surface_id(
        "inline",
        &comment.lane,
        Some(&comment.path),
        Some(comment.line),
        &comment.body,
        &comment.evidence,
        comment.suggestion.as_deref(),
    )
}

fn summary_surface_id(finding: &SummaryOnlyFinding) -> String {
    compiler_surface_id(
        "summary",
        &finding.lane,
        None,
        None,
        &finding.reason,
        &finding.evidence,
        None,
    )
}

fn summary_surface_receipt(
    finding: &SummaryOnlyFinding,
    source_artifact: &'static str,
    source_index: usize,
) -> CompilerSurfaceReceipt {
    CompilerSurfaceReceipt {
        surface_id: summary_surface_id(finding),
        kind: "summary",
        source_artifact,
        source_index,
        claim_id: topic_claim_id_for_summary(finding),
        lane: finding.lane.clone(),
        subject: finding.reason.clone(),
        path: None,
        line: None,
        adjudicating_claim_ids: Vec::new(),
    }
}

fn compiler_surface_id(
    kind: &str,
    lane: &str,
    path: Option<&str>,
    line: Option<u32>,
    text: &str,
    evidence: &str,
    suggestion: Option<&str>,
) -> String {
    let escape = |value: &str| value.replace('\\', "\\\\").replace('\n', "\\n");
    let escaped_path = escape(path.unwrap_or_default());
    let escaped_text = escape(text);
    let escaped_evidence = escape(evidence);
    let escaped_suggestion = escape(suggestion.unwrap_or_default());
    let identity = format!(
        "kind={kind}\nlane={lane}\npath={escaped_path}\nline={}\ntext={escaped_text}\nevidence={escaped_evidence}\nsuggestion={escaped_suggestion}",
        line.map_or_else(String::new, |value| value.to_string()),
    );
    format!("surface-{}", sha256_hex(identity.as_bytes()))
}

fn graph_loser_claim_ids_for_inline(
    graph: &ClaimGraph,
    surface: &CompilerSurfaceReceipt,
) -> Vec<String> {
    graph
        .topics
        .iter()
        .filter(|topic| {
            topic_is_adjudicated_loser(graph, topic)
                && topic.path == surface.path
                && topic.anchor == surface.line
                && (topic.claim_id == surface.claim_id
                    || subject_tokens_overlap(&topic.subject, &surface_subject(surface)))
        })
        .map(|topic| topic.claim_id.clone())
        .collect()
}

fn graph_loser_claim_ids_for_summary(
    graph: &ClaimGraph,
    surface: &CompilerSurfaceReceipt,
) -> Vec<String> {
    graph
        .topics
        .iter()
        .filter(|topic| {
            topic_is_adjudicated_loser(graph, topic)
                && topic.source_lane == surface.lane
                && (topic.claim_id == surface.claim_id
                    || subject_tokens_overlap(&topic.subject, &surface_subject(surface)))
        })
        .map(|topic| topic.claim_id.clone())
        .collect()
}

fn graph_thread_claim_ids_for_inline(
    graph: &ClaimGraph,
    surface: &CompilerSurfaceReceipt,
) -> Vec<String> {
    graph
        .topics
        .iter()
        .filter(|topic| {
            !topic_is_adjudicated_loser(graph, topic)
                && topic.path == surface.path
                && topic.anchor == surface.line
                && !topic.existing_threads.is_empty()
                && (topic.claim_id == surface.claim_id
                    || subject_tokens_overlap(&topic.subject, &surface_subject(surface)))
        })
        .map(|topic| topic.claim_id.clone())
        .collect()
}

fn graph_thread_claim_ids_for_summary(
    graph: &ClaimGraph,
    surface: &CompilerSurfaceReceipt,
) -> Vec<String> {
    graph
        .topics
        .iter()
        .filter(|topic| {
            !topic_is_adjudicated_loser(graph, topic)
                && topic.source_lane == surface.lane
                && !topic.existing_threads.is_empty()
                && (topic.claim_id == surface.claim_id
                    || subject_tokens_overlap(&topic.subject, &surface_subject(surface)))
        })
        .map(|topic| topic.claim_id.clone())
        .collect()
}

fn surface_subject(surface: &CompilerSurfaceReceipt) -> String {
    surface.subject.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary(reason: &str) -> SummaryOnlyFinding {
        SummaryOnlyFinding {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            reason: reason.to_owned(),
            evidence: "focused receipt".to_owned(),
        }
    }

    fn loser_graph(finding: &SummaryOnlyFinding) -> ClaimGraph {
        let claim_id = topic_claim_id_for_summary(finding);
        let graph = ClaimGraph {
            schema: crate::artifacts::CLAIM_GRAPH_SCHEMA,
            head_sha: "HEAD".to_owned(),
            claims: Vec::new(),
            topics: vec![ReviewTopic {
                claim_id: claim_id.clone(),
                head_sha: "HEAD".to_owned(),
                path: None,
                anchor: None,
                symbol: None,
                failure_family: "summary-finding".to_owned(),
                mechanism: finding.reason.clone(),
                status: "refuted".to_owned(),
                thread_disposition: "refuted_by_new_evidence".to_owned(),
                severity: finding.severity.clone(),
                evidence: Vec::new(),
                existing_threads: Vec::new(),
                stale_threads: Vec::new(),
                proof_requests: Vec::new(),
                proof_receipts: Vec::new(),
                delivery: "no-human-surface".to_owned(),
                planned_action: "none".to_owned(),
                planned_thread_id: None,
                source_lane: finding.lane.clone(),
                subject: finding.reason.clone(),
            }],
            conflicts: vec![ConflictRecord {
                claim_ids: vec!["winner".to_owned(), claim_id],
                winner: Some("winner".to_owned()),
                loser: Some(topic_claim_id_for_summary(finding)),
                description: "focused self-test adjudication".to_owned(),
                resolution: ConflictResolution::ResolvedByProof,
            }],
            evidence_gaps: Vec::new(),
            mode: "active",
        };
        assert_eq!(graph.topics[0].planned_action, "none");
        assert_eq!(graph.topics[0].planned_thread_id, None);
        graph
    }

    #[test]
    fn reconciliation_receipts_account_for_adjudicated_removals() -> Result<()> {
        let losing = summary("The parser claim is refuted by a focused receipt.");
        let retained = summary("A structurally distinct parser claim survives.");
        let graph = loser_graph(&losing);
        let receipt = build_compiler_reconciliation_receipt(CompilerReconciliationInput {
            head_sha: "HEAD",
            observations: &[],
            review_inline_comments: &[],
            review_summary_only_findings: &[losing.clone(), retained.clone()],
            follow_up_summary_only_findings: &[],
            resolved_away_candidates: &[],
            final_inline_comments: &[],
            final_summary_only_findings: std::slice::from_ref(&retained),
            graph: &graph,
        })?;
        assert_eq!(receipt.input_surfaces.len(), 2);
        assert_eq!(receipt.retained_surfaces.len(), 1);
        assert_eq!(receipt.removed_surfaces.len(), 1);
        assert_eq!(
            receipt.removed_surfaces[0].disposition,
            "refuted_by_stronger_evidence"
        );
        assert_eq!(
            receipt.removed_surfaces[0].surface.adjudicating_claim_ids,
            vec![topic_claim_id_for_summary(&losing)]
        );
        Ok(())
    }

    #[test]
    fn unexplained_surface_loss_is_rejected() -> Result<()> {
        let losing = summary("The parser claim has no adjudication receipt.");
        let result = build_compiler_reconciliation_receipt(CompilerReconciliationInput {
            head_sha: "HEAD",
            observations: &[],
            review_inline_comments: &[],
            review_summary_only_findings: &[losing],
            follow_up_summary_only_findings: &[],
            resolved_away_candidates: &[],
            final_inline_comments: &[],
            final_summary_only_findings: &[],
            graph: &build_shadow_claim_graph("HEAD"),
        });
        let error = result
            .err()
            .ok_or_else(|| anyhow::anyhow!("unexplained surface loss unexpectedly passed"))?;
        assert!(
            error
                .to_string()
                .contains("removed without an adjudication")
        );
        Ok(())
    }

    #[test]
    fn current_head_thread_explains_removed_surface() -> Result<()> {
        let finding = summary("The parser claim is already covered by the current thread.");
        let claim_id = topic_claim_id_for_summary(&finding);
        let graph = ClaimGraph {
            schema: crate::artifacts::CLAIM_GRAPH_SCHEMA,
            head_sha: "HEAD".to_owned(),
            claims: Vec::new(),
            topics: vec![ReviewTopic {
                claim_id: claim_id.clone(),
                head_sha: "HEAD".to_owned(),
                path: None,
                anchor: None,
                symbol: None,
                failure_family: "summary-finding".to_owned(),
                mechanism: finding.reason.clone(),
                status: "supported".to_owned(),
                thread_disposition: "current_thread".to_owned(),
                severity: finding.severity.clone(),
                evidence: Vec::new(),
                existing_threads: vec!["thread-1".to_owned()],
                stale_threads: Vec::new(),
                proof_requests: Vec::new(),
                proof_receipts: Vec::new(),
                delivery: "summary-only".to_owned(),
                planned_action: "none".to_owned(),
                planned_thread_id: None,
                source_lane: finding.lane.clone(),
                subject: finding.reason.clone(),
            }],
            conflicts: Vec::new(),
            evidence_gaps: Vec::new(),
            mode: "active",
        };
        let receipt = build_compiler_reconciliation_receipt(CompilerReconciliationInput {
            head_sha: "HEAD",
            observations: &[],
            review_inline_comments: &[],
            review_summary_only_findings: &[finding],
            follow_up_summary_only_findings: &[],
            resolved_away_candidates: &[],
            final_inline_comments: &[],
            final_summary_only_findings: &[],
            graph: &graph,
        })?;
        assert_eq!(
            receipt.removed_surfaces[0].disposition,
            "covered_by_current_head_thread"
        );
        assert_eq!(
            receipt.removed_surfaces[0].evidence_receipts,
            vec!["review/pr_thread_context.json#thread-1"]
        );
        Ok(())
    }

    #[test]
    fn duplicate_surface_is_explicitly_accounted() -> Result<()> {
        let finding = summary("The parser claim is duplicated structurally.");
        let graph = build_shadow_claim_graph("HEAD");
        let receipt = build_compiler_reconciliation_receipt(CompilerReconciliationInput {
            head_sha: "HEAD",
            observations: &[],
            review_inline_comments: &[],
            review_summary_only_findings: &[finding.clone(), finding.clone()],
            follow_up_summary_only_findings: &[],
            resolved_away_candidates: &[],
            final_inline_comments: &[],
            final_summary_only_findings: std::slice::from_ref(&finding),
            graph: &graph,
        })?;
        assert_eq!(
            receipt.removed_surfaces[0].disposition,
            "duplicate_structurally_identical"
        );
        Ok(())
    }

    #[test]
    fn cross_surface_duplicate_is_explicitly_accounted() -> Result<()> {
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
        let summary_finding = summary("Parser drops the later subscript");
        let receipt = build_compiler_reconciliation_receipt(CompilerReconciliationInput {
            head_sha: "HEAD",
            observations: &[],
            review_inline_comments: std::slice::from_ref(&comment),
            review_summary_only_findings: std::slice::from_ref(&summary_finding),
            follow_up_summary_only_findings: &[],
            resolved_away_candidates: &[],
            final_inline_comments: std::slice::from_ref(&comment),
            final_summary_only_findings: &[],
            graph: &build_shadow_claim_graph("HEAD"),
        })?;
        assert_eq!(receipt.input_surfaces.len(), 2);
        assert_eq!(receipt.retained_surfaces.len(), 1);
        assert_eq!(receipt.removed_surfaces.len(), 1);
        assert_eq!(receipt.retained_surfaces[0].kind, "inline");
        assert_eq!(
            receipt.retained_surfaces[0].source_artifact,
            "review/review.json"
        );
        assert_eq!(receipt.retained_surfaces[0].source_index, 0);
        assert_eq!(receipt.removed_surfaces[0].surface.kind, "summary");
        assert_eq!(
            receipt.removed_surfaces[0].surface.source_artifact,
            "review/review.json"
        );
        assert_eq!(receipt.removed_surfaces[0].surface.source_index, 0);
        assert_eq!(
            receipt.removed_surfaces[0].surface.adjudicating_claim_ids,
            vec![receipt.retained_surfaces[0].claim_id.clone()]
        );
        assert_eq!(
            receipt.removed_surfaces[0].disposition,
            "duplicate_cross_surface"
        );
        assert_eq!(
            receipt.removed_surfaces[0].evidence_receipts,
            vec![format!(
                "review/claim_graph.json#claims/{}",
                receipt.retained_surfaces[0].claim_id
            )]
        );
        assert_eq!(
            (
                receipt.retained_surfaces[0].kind,
                receipt.retained_surfaces[0].source_artifact,
                receipt.retained_surfaces[0].source_index,
                receipt.removed_surfaces[0].surface.kind,
                receipt.removed_surfaces[0].surface.source_artifact,
                receipt.removed_surfaces[0].surface.source_index,
                receipt.removed_surfaces[0].disposition,
            ),
            (
                "inline",
                "review/review.json",
                0,
                "summary",
                "review/review.json",
                0,
                "duplicate_cross_surface",
            )
        );
        Ok(())
    }

    #[test]
    fn removal_explanation_returns_exact_cross_surface_receipt() -> Result<()> {
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
        let summary_finding = summary("Parser drops the later subscript");
        let retained = inline_surface_receipt(&comment, &[], "review/review.json", 0);
        let removed = summary_surface_receipt(&summary_finding, "review/review.json", 0);
        let explanation = removal_explanation(
            &SurfaceIdentity { receipt: removed },
            &[],
            std::slice::from_ref(&retained),
            &build_shadow_claim_graph("HEAD"),
        )?;
        assert_eq!(
            explanation,
            (
                "duplicate_cross_surface",
                vec![retained.claim_id.clone()],
                vec![format!(
                    "review/claim_graph.json#claims/{}",
                    retained.claim_id
                )]
            )
        );
        assert_eq!(explanation.0, "duplicate_cross_surface");
        assert_eq!(explanation.1, vec![retained.claim_id.clone()]);
        assert_eq!(
            explanation.2,
            vec![format!(
                "review/claim_graph.json#claims/{}",
                retained.claim_id
            )]
        );
        Ok(())
    }

    #[test]
    fn ambiguous_cross_surface_match_is_not_explained() -> Result<()> {
        let first = ReviewInlineComment {
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
        let second = ReviewInlineComment {
            path: "src/lowering.rs".to_owned(),
            line: 21,
            ..first.clone()
        };
        let finding = summary("Parser drops the later subscript");
        let result = build_compiler_reconciliation_receipt(CompilerReconciliationInput {
            head_sha: "HEAD",
            observations: &[],
            review_inline_comments: &[first.clone(), second.clone()],
            review_summary_only_findings: std::slice::from_ref(&finding),
            follow_up_summary_only_findings: &[],
            resolved_away_candidates: &[],
            final_inline_comments: &[first, second],
            final_summary_only_findings: &[],
            graph: &build_shadow_claim_graph("HEAD"),
        });
        let error = result
            .err()
            .ok_or_else(|| anyhow::anyhow!("ambiguous cross-surface match unexpectedly passed"))?;
        assert!(
            error
                .to_string()
                .contains("removed without an adjudication")
        );
        Ok(())
    }

    #[test]
    fn resolved_candidate_is_subtracted_before_accounting() -> Result<()> {
        let finding = summary("The parser claim was resolved by follow-up evidence.");
        let candidate = CandidateRecord {
            schema: "ub-review.candidate.v1".to_owned(),
            id: "candidate-1".to_owned(),
            lane: finding.lane.clone(),
            source: "summary-only-finding".to_owned(),
            status: "resolved".to_owned(),
            disposition: "dropped".to_owned(),
            severity: finding.severity.clone(),
            confidence: finding.confidence.clone(),
            claim: finding.reason.clone(),
            evidence: finding.evidence.clone(),
            path: None,
            line: None,
            side: None,
        };
        let candidate_refs = [&candidate];
        let receipt = build_compiler_reconciliation_receipt(CompilerReconciliationInput {
            head_sha: "HEAD",
            observations: &[],
            review_inline_comments: &[],
            review_summary_only_findings: &[finding],
            follow_up_summary_only_findings: &[],
            resolved_away_candidates: &candidate_refs,
            final_inline_comments: &[],
            final_summary_only_findings: &[],
            graph: &build_shadow_claim_graph("HEAD"),
        })?;
        assert!(receipt.input_surfaces.is_empty());
        assert!(receipt.removed_surfaces.is_empty());
        Ok(())
    }

    #[test]
    fn final_surface_missing_from_input_is_rejected() -> Result<()> {
        let input = summary("The input claim is not the final claim.");
        let final_only = summary("A final claim absent from the input.");
        let result = build_compiler_reconciliation_receipt(CompilerReconciliationInput {
            head_sha: "HEAD",
            observations: &[],
            review_inline_comments: &[],
            review_summary_only_findings: &[input],
            follow_up_summary_only_findings: &[],
            resolved_away_candidates: &[],
            final_inline_comments: &[],
            final_summary_only_findings: &[final_only],
            graph: &build_shadow_claim_graph("HEAD"),
        });
        let error = result
            .err()
            .ok_or_else(|| anyhow::anyhow!("missing final surface unexpectedly passed"))?;
        assert!(
            error
                .to_string()
                .contains("not present in reconciled input")
        );
        Ok(())
    }
}
