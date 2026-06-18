//! Candidate records, witness construction, and orchestrator plan
//! building (cleanup train step 26, pure code motion).

use crate::*;

pub(crate) fn build_witness_records(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    observations: &[Observation],
    proof_receipts: &[ProofReceipt],
) -> Vec<WitnessRecord> {
    let mut witnesses = Vec::new();
    for comment in inline_comments {
        witnesses.push(witness_record(WitnessRecordInput {
            status: "needs-witness",
            kind: "inline-finding",
            source: "inline-comment",
            claim: &comment.body,
            dedupe_key: &format!(
                "inline:{}:{}:{}",
                comment.path,
                comment.line,
                sha256_hex(comment.body.as_bytes())
            ),
            evidence: vec![comment.evidence.clone()],
            lane: Some(comment.lane.clone()),
            path: Some(comment.path.clone()),
            line: Some(comment.line),
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for finding in summary_only_findings {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_summary_finding(finding),
            kind: "summary-finding",
            source: "summary-only-finding",
            claim: &finding.reason,
            dedupe_key: &format!(
                "summary:{}:{}",
                finding.lane,
                sha256_hex(format!("{}\n{}", finding.reason, finding.evidence).as_bytes())
            ),
            evidence: vec![finding.evidence.clone()],
            lane: Some(finding.lane.clone()),
            path: None,
            line: None,
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for observation in observations {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_observation(observation),
            kind: &observation.kind,
            source: &observation.source,
            claim: &observation.claim,
            dedupe_key: &observation.dedupe_key,
            evidence: observation.evidence.clone(),
            lane: Some(observation.lane.clone()),
            path: observation.path.clone(),
            line: observation.line,
            observation_id: Some(observation.id.clone()),
            proof_receipt_id: None,
        }));
    }
    for receipt in proof_receipts {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_proof_receipt(receipt),
            kind: &receipt.kind,
            source: "proof-receipt",
            claim: &receipt.reason,
            dedupe_key: &receipt.id,
            evidence: proof_receipt_witness_evidence(receipt),
            lane: None,
            path: None,
            line: None,
            observation_id: None,
            proof_receipt_id: Some(receipt.id.clone()),
        }));
    }
    witnesses
}

pub(crate) fn append_follow_up_evidence_witnesses(
    witnesses: &mut Vec<WitnessRecord>,
    evidence: &FollowUpEvidenceArtifact,
    proof_receipts: &[ProofReceipt],
) {
    for comment in &evidence.inline_comments {
        witnesses.push(witness_record(WitnessRecordInput {
            status: "needs-witness",
            kind: "inline-finding",
            source: "follow-up-inline-comment",
            claim: &comment.body,
            dedupe_key: &format!(
                "follow-up-inline:{}:{}:{}",
                comment.path,
                comment.line,
                sha256_hex(comment.body.as_bytes())
            ),
            evidence: vec![comment.evidence.clone()],
            lane: Some(comment.lane.clone()),
            path: Some(comment.path.clone()),
            line: Some(comment.line),
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for finding in &evidence.summary_only_findings {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_summary_finding(finding),
            kind: "summary-finding",
            source: "follow-up-summary-only-finding",
            claim: &finding.reason,
            dedupe_key: &format!(
                "follow-up-summary:{}:{}",
                finding.lane,
                sha256_hex(format!("{}\n{}", finding.reason, finding.evidence).as_bytes())
            ),
            evidence: vec![finding.evidence.clone()],
            lane: Some(finding.lane.clone()),
            path: None,
            line: None,
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for observation in &evidence.observations {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_observation(observation),
            kind: &observation.kind,
            source: &format!("follow-up-{}", observation.source),
            claim: &observation.claim,
            dedupe_key: &format!("follow-up-observation:{}", observation.dedupe_key),
            evidence: observation.evidence.clone(),
            lane: Some(observation.lane.clone()),
            path: observation.path.clone(),
            line: observation.line,
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for request in &evidence.proof_requests {
        let matched_receipt = proof_receipt_for_request(proof_receipts, &request.id);
        let (status, mut request_evidence, proof_receipt_id) = match matched_receipt {
            Some(receipt) => (
                witness_status_for_proof_receipt(receipt),
                proof_receipt_witness_evidence(receipt),
                Some(receipt.id.clone()),
            ),
            None => ("needs-witness", vec![request.command.clone()], None),
        };
        request_evidence.insert(0, format!("Follow-up proof request: {}", request.command));
        witnesses.push(witness_record(WitnessRecordInput {
            status,
            kind: "proof-request",
            source: "follow-up-proof-request",
            claim: &request.reason,
            dedupe_key: &format!("follow-up-proof-request:{}", request.id),
            evidence: request_evidence,
            lane: Some(request.lane.clone()),
            path: None,
            line: None,
            observation_id: None,
            proof_receipt_id,
        }));
    }
}

pub(crate) fn proof_receipt_for_request<'a>(
    proof_receipts: &'a [ProofReceipt],
    request_id: &str,
) -> Option<&'a ProofReceipt> {
    proof_receipts
        .iter()
        .find(|receipt| receipt.request_ids.iter().any(|id| id == request_id))
}

pub(crate) fn build_candidate_records(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
) -> Vec<CandidateRecord> {
    let mut candidates = Vec::new();
    for comment in inline_comments {
        let fingerprint = sha256_hex(
            format!(
                "inline-comment\n{}\n{}\n{}\n{}\n{}",
                comment.lane, comment.path, comment.line, comment.body, comment.evidence
            )
            .as_bytes(),
        );
        candidates.push(CandidateRecord {
            schema: CANDIDATE_SCHEMA.to_owned(),
            id: format!(
                "candidate-{index:04}-{short}",
                index = candidates.len(),
                short = &fingerprint[..12]
            ),
            lane: comment.lane.clone(),
            source: "inline-comment".to_owned(),
            status: "accepted-inline".to_owned(),
            disposition: "inline".to_owned(),
            severity: comment.severity.clone(),
            confidence: comment.confidence.clone(),
            claim: comment.body.clone(),
            evidence: comment.evidence.clone(),
            path: Some(comment.path.clone()),
            line: Some(comment.line),
            side: Some(comment.side.clone()),
        });
    }
    for finding in summary_only_findings {
        let fingerprint = sha256_hex(
            format!(
                "summary-only-finding\n{}\n{}\n{}",
                finding.lane, finding.reason, finding.evidence
            )
            .as_bytes(),
        );
        candidates.push(CandidateRecord {
            schema: CANDIDATE_SCHEMA.to_owned(),
            id: format!(
                "candidate-{index:04}-{short}",
                index = candidates.len(),
                short = &fingerprint[..12]
            ),
            lane: finding.lane.clone(),
            source: "summary-only-finding".to_owned(),
            status: "summary-only".to_owned(),
            disposition: candidate_disposition_for_summary_finding(finding).to_owned(),
            severity: finding.severity.clone(),
            confidence: finding.confidence.clone(),
            claim: finding.reason.clone(),
            evidence: finding.evidence.clone(),
            path: None,
            line: None,
            side: None,
        });
    }
    candidates
}

pub(crate) fn candidate_disposition_for_summary_finding(
    finding: &SummaryOnlyFinding,
) -> &'static str {
    let reason = finding.reason.to_ascii_lowercase();
    let evidence = finding.evidence.to_ascii_lowercase();
    if is_parked_follow_up(finding) {
        "parked-follow-up"
    } else if reason.contains("false premise")
        || reason.contains("refuted")
        || evidence.contains("false premise")
        || evidence.contains("refuted")
    {
        "refuted"
    } else if reason.contains("duplicate inline candidate merged")
        || reason.contains("summary-only guard rejected candidate")
        || is_submaterial_polish_finding(finding)
    {
        "dropped"
    } else {
        "summary-only"
    }
}

pub(crate) fn is_submaterial_polish_finding(finding: &SummaryOnlyFinding) -> bool {
    if !matches!(finding.severity.as_str(), "low") {
        return false;
    }
    let text = format!("{}\n{}", finding.reason, finding.evidence).to_ascii_lowercase();
    text.contains("submaterial polish")
        || text.contains("sub-material polish")
        || text.contains("below materiality")
        || text.contains("below the materiality")
        || text.contains("polish suggestion")
        || text.contains("style/polish")
}

pub(crate) fn write_candidate_artifacts(out: &Path, candidates: &[CandidateRecord]) -> Result<()> {
    let candidates_dir = out.join("candidates");
    if candidates_dir.exists() {
        fs::remove_dir_all(&candidates_dir)
            .with_context(|| format!("remove {}", candidates_dir.display()))?;
    }
    fs::create_dir_all(&candidates_dir)
        .with_context(|| format!("create {}", candidates_dir.display()))?;

    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("candidates.json"),
        serde_json::to_vec_pretty(candidates)?,
    )?;

    let mut ndjson = String::new();
    for candidate in candidates {
        ndjson.push_str(&serde_json::to_string(candidate)?);
        ndjson.push('\n');
        fs::write(
            candidates_dir.join(format!("{}.json", sanitize_artifact_name(&candidate.id))),
            serde_json::to_vec_pretty(candidate)?,
        )?;
    }
    fs::write(out.join("candidates.ndjson"), ndjson)?;
    Ok(())
}

pub(crate) fn read_candidate_review_surfaces(
    out: &Path,
) -> Result<(Vec<ReviewInlineComment>, Vec<SummaryOnlyFinding>)> {
    let candidates = read_candidate_records(out)?;
    candidate_review_surfaces(&candidates)
}

pub(crate) fn read_candidate_records(out: &Path) -> Result<Vec<CandidateRecord>> {
    let path = out.join("review/candidates.json");
    serde_json::from_slice(&fs::read(&path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("parse {}", path.display()))
}

pub(crate) fn candidate_review_surfaces(
    candidates: &[CandidateRecord],
) -> Result<(Vec<ReviewInlineComment>, Vec<SummaryOnlyFinding>)> {
    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    for candidate in candidates {
        if candidate.schema != CANDIDATE_SCHEMA {
            bail!("candidate {} has unsupported schema", candidate.id);
        }
        if !matches!(
            candidate.disposition.as_str(),
            "inline" | "summary-only" | "parked-follow-up" | "refuted" | "dropped"
        ) {
            bail!(
                "candidate {} has unsupported disposition {}",
                candidate.id,
                candidate.disposition
            );
        }
        match (candidate.source.as_str(), candidate.status.as_str()) {
            ("inline-comment", "accepted-inline") => {
                if candidate.disposition != "inline" {
                    bail!(
                        "inline candidate {} disposition must be inline",
                        candidate.id
                    );
                }
                let path = candidate
                    .path
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("candidate {} missing path", candidate.id))?;
                let line = candidate
                    .line
                    .ok_or_else(|| anyhow::anyhow!("candidate {} missing line", candidate.id))?;
                let side = candidate
                    .side
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("candidate {} missing side", candidate.id))?;
                if side != "RIGHT" {
                    bail!("candidate {} side must be RIGHT", candidate.id);
                }
                inline_comments.push(ReviewInlineComment {
                    lane: candidate.lane.clone(),
                    severity: candidate.severity.clone(),
                    confidence: candidate.confidence.clone(),
                    path,
                    line,
                    side,
                    body: candidate.claim.clone(),
                    evidence: candidate.evidence.clone(),
                    suggestion: None,
                });
            }
            ("summary-only-finding", "summary-only") => {
                if candidate.disposition == "inline" {
                    bail!(
                        "summary-only candidate {} disposition cannot be inline",
                        candidate.id
                    );
                }
                if candidate.path.is_some() || candidate.line.is_some() || candidate.side.is_some()
                {
                    bail!("summary-only candidate {} has inline fields", candidate.id);
                }
                summary_only_findings.push(SummaryOnlyFinding {
                    lane: candidate.lane.clone(),
                    severity: candidate.severity.clone(),
                    confidence: candidate.confidence.clone(),
                    reason: candidate.claim.clone(),
                    evidence: candidate.evidence.clone(),
                });
            }
            _ => bail!(
                "candidate {} has unsupported source/status {}/{}",
                candidate.id,
                candidate.source,
                candidate.status
            ),
        }
    }
    Ok((inline_comments, summary_only_findings))
}

pub(crate) fn build_orchestrator_plan(
    candidates: &[CandidateRecord],
    observations: &[ObservationGroup],
    proof_receipts: &[ProofReceipt],
    resource_leases: &[ResourceLease],
    tool_gate_outcomes: &[ToolGateOutcomeEntry],
) -> OrchestratorPlanArtifact {
    let mut grouped: BTreeMap<(String, String), Vec<&CandidateRecord>> = BTreeMap::new();
    for candidate in candidates {
        let evidence_need = candidate_evidence_need(candidate);
        grouped
            .entry((candidate.disposition.clone(), evidence_need))
            .or_default()
            .push(candidate);
    }

    let mut evidence_groups = Vec::new();
    let mut follow_up_tasks = Vec::new();
    for ((disposition, evidence_need), group_candidates) in grouped {
        let candidate_ids = group_candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let lanes = unique_sorted(
            group_candidates
                .iter()
                .map(|candidate| candidate.lane.clone())
                .collect(),
        );
        let routed_evidence = routed_evidence_for_group(
            &evidence_need,
            &lanes,
            proof_receipts,
            resource_leases,
            tool_gate_outcomes,
        );
        let fingerprint = sha256_hex(format!("{disposition}\n{evidence_need}").as_bytes());
        let group_id = format!("evidence-group-{}", &fingerprint[..12]);
        let group = OrchestratorEvidenceGroup {
            schema: ORCHESTRATOR_EVIDENCE_GROUP_SCHEMA.to_owned(),
            id: group_id.clone(),
            evidence_need: evidence_need.clone(),
            disposition: disposition.clone(),
            candidate_ids: candidate_ids.clone(),
            lanes,
            routed_evidence: routed_evidence.clone(),
            duplicate_count: candidate_ids.len().saturating_sub(1),
            reason: orchestrator_group_reason(&disposition, &evidence_need),
        };
        if let Some(task) = follow_up_task_for_group(
            &group_id,
            &disposition,
            &evidence_need,
            &candidate_ids,
            &routed_evidence,
        ) {
            follow_up_tasks.push(task);
        }
        evidence_groups.push(group);
    }

    let mut observation_groups = Vec::new();
    for observation in observations {
        let evidence_need = observation_evidence_need(observation);
        let routed_evidence = routed_evidence_for_group(
            &evidence_need,
            &observation.lanes,
            proof_receipts,
            resource_leases,
            tool_gate_outcomes,
        );
        let group_id = format!("orchestrator-{}", observation.id);
        let group = OrchestratorObservationGroup {
            schema: ORCHESTRATOR_OBSERVATION_GROUP_SCHEMA.to_owned(),
            id: group_id.clone(),
            observation_group_id: observation.id.clone(),
            dedupe_key: observation.dedupe_key.clone(),
            evidence_need: evidence_need.clone(),
            claim: observation.claim.clone(),
            kind: observation.kind.clone(),
            status: observation.status.clone(),
            lanes: observation.lanes.clone(),
            sources: observation.sources.clone(),
            observation_ids: observation.observation_ids.clone(),
            duplicate_count: observation.duplicate_count,
            routed_evidence: routed_evidence.clone(),
            reason: format!(
                "routed unique observation group `{}` under evidence need `{evidence_need}`",
                observation.id
            ),
        };
        if let Some(task) =
            follow_up_task_for_observation_group(observation, &group, &routed_evidence)
        {
            follow_up_tasks.push(task);
        }
        observation_groups.push(group);
    }

    OrchestratorPlanArtifact {
        schema: ORCHESTRATOR_PLAN_SCHEMA.to_owned(),
        candidates: candidates.len(),
        observations: observations.len(),
        evidence_groups,
        observation_groups,
        follow_up_tasks,
    }
}

pub(crate) fn build_final_orchestrator_plan(
    candidates: &[CandidateRecord],
    observations: &[ObservationGroup],
    proof_receipts: &[ProofReceipt],
    resource_leases: &[ResourceLease],
    tool_gate_outcomes: &[ToolGateOutcomeEntry],
) -> OrchestratorPlanArtifact {
    let mut plan = build_orchestrator_plan(
        candidates,
        observations,
        proof_receipts,
        resource_leases,
        tool_gate_outcomes,
    );
    plan.follow_up_tasks
        .retain(|task| !final_follow_up_task_resolved_by_tool_proof(task));
    plan
}

pub(crate) fn final_follow_up_task_resolved_by_tool_proof(task: &FollowUpQuestionTask) -> bool {
    matches!(
        task.evidence_need.as_str(),
        "proof-confirmation" | "test-oracle-confirmation" | "source-route-confirmation"
    ) && task
        .routed_evidence
        .iter()
        .any(|evidence| evidence.kind == "proof-receipt" && evidence.status == "tool-confirmed")
}

pub(crate) fn write_orchestrator_artifacts(
    out: &Path,
    plan: &OrchestratorPlanArtifact,
    proof_receipts: &[ProofReceipt],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("orchestrator_plan.json"),
        serde_json::to_vec_pretty(plan)?,
    )?;

    let mut ndjson = String::new();
    for task in &plan.follow_up_tasks {
        ndjson.push_str(&serde_json::to_string(task)?);
        ndjson.push('\n');
    }
    fs::write(out.join("follow_up_questions.ndjson"), ndjson)?;
    write_follow_up_question_packets(out, &plan.follow_up_tasks, proof_receipts)?;
    Ok(())
}
