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
        input.review_inline_comments,
        input.review_summary_only_findings,
        input.follow_up_summary_only_findings,
        input.resolved_away_candidates,
    );
    let final_surfaces = final_surfaces(
        input.final_inline_comments,
        input.final_summary_only_findings,
    );

    let mut available = input_surfaces.clone();
    let mut retained_surfaces = Vec::with_capacity(final_surfaces.len());
    for final_surface in final_surfaces {
        let Some(position) = available.iter().position(|candidate| {
            candidate.receipt.surface_id == final_surface.receipt.surface_id
                && candidate.receipt.kind == final_surface.receipt.kind
        }) else {
            bail!(
                "final compiler surface {} was not present in reconciled input",
                final_surface.receipt.surface_id
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
        input_surfaces: input_surfaces
            .into_iter()
            .map(|surface| surface.receipt)
            .collect(),
        retained_surfaces,
        removed_surfaces,
    })
}

fn input_surfaces(
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
            receipt: inline_surface_receipt(comment, "review/review.json", source_index),
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

fn final_surfaces(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
) -> Vec<SurfaceIdentity> {
    inline_comments
        .iter()
        .map(|comment| SurfaceIdentity {
            receipt: inline_surface_receipt(comment, "review/review.json", 0),
        })
        .chain(summary_only_findings.iter().map(|finding| SurfaceIdentity {
            receipt: summary_surface_receipt(finding, "review/review.json", 0),
        }))
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
    source_artifact: &'static str,
    source_index: usize,
) -> CompilerSurfaceReceipt {
    CompilerSurfaceReceipt {
        surface_id: compiler_surface_id(
            "inline",
            &comment.lane,
            Some(&comment.path),
            Some(comment.line),
            &comment.body,
            &comment.evidence,
            comment.suggestion.as_deref(),
        ),
        kind: "inline",
        source_artifact,
        source_index,
        claim_id: topic_claim_id_for_inline(comment),
        lane: comment.lane.clone(),
        subject: comment.body.clone(),
        path: Some(comment.path.clone()),
        line: Some(comment.line),
        adjudicating_claim_ids: Vec::new(),
    }
}

fn summary_surface_receipt(
    finding: &SummaryOnlyFinding,
    source_artifact: &'static str,
    source_index: usize,
) -> CompilerSurfaceReceipt {
    CompilerSurfaceReceipt {
        surface_id: compiler_surface_id(
            "summary",
            &finding.lane,
            None,
            None,
            &finding.reason,
            &finding.evidence,
            None,
        ),
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
    let identity = format!(
        "kind={kind}\nlane={lane}\npath={}\nline={}\ntext={text}\nevidence={evidence}\nsuggestion={}",
        path.unwrap_or_default(),
        line.map_or_else(String::new, |value| value.to_string()),
        suggestion.unwrap_or_default(),
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
        ClaimGraph {
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
        }
    }

    #[test]
    fn reconciliation_receipts_account_for_adjudicated_removals() -> Result<()> {
        let losing = summary("The parser claim is refuted by a focused receipt.");
        let retained = summary("A structurally distinct parser claim survives.");
        let graph = loser_graph(&losing);
        let receipt = build_compiler_reconciliation_receipt(CompilerReconciliationInput {
            head_sha: "HEAD",
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
}
