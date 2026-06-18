//! Follow-up question routing, evidence routing, and orchestrator
//! group reasoning (cleanup train step 30, pure code motion).

use crate::*;

pub(crate) fn render_follow_up_question_prompt(
    task: &FollowUpQuestionTask,
    routed_excerpts: &BTreeMap<String, String>,
) -> String {
    let mut prompt = String::new();
    prompt.push_str("Follow-up question task\n\n");
    prompt.push_str(&format!("- Task: `{}`\n", task.id));
    prompt.push_str(&format!("- Group: `{}`\n", task.group_id));
    prompt.push_str(&format!(
        "- Stage: `{}` - {}\n",
        task.stage, task.stage_reason
    ));
    prompt.push_str(&format!("- Evidence need: `{}`\n", task.evidence_need));
    prompt.push_str(&format!("- Disposition: `{}`\n", task.disposition));
    if !task.candidate_ids.is_empty() {
        prompt.push_str(&format!(
            "- Candidate ids: `{}`\n",
            task.candidate_ids.join("`, `")
        ));
    }
    if !task.observation_group_ids.is_empty() {
        prompt.push_str(&format!(
            "- Observation group ids: `{}`\n",
            task.observation_group_ids.join("`, `")
        ));
    }
    prompt.push_str(&format!("\nQuestion: {}\n\n", task.question));
    if task.routed_evidence.is_empty() {
        prompt.push_str("Routed evidence: none.\n\n");
    } else {
        prompt.push_str("Routed evidence:\n");
        for evidence in &task.routed_evidence {
            prompt.push_str(&format!(
                "- `{}` kind=`{}` status=`{}` result=`{}` artifact=`{}` reason={}\n",
                evidence.id,
                evidence.kind,
                evidence.status,
                evidence.result,
                evidence.artifact,
                evidence.reason
            ));
            if let Some(excerpt) = routed_excerpts.get(&evidence.id) {
                prompt.push_str(
                    "  Receipt content (bounded command-output tails; full streams in the artifact):\n",
                );
                prompt.push_str(excerpt);
            }
        }
        prompt.push('\n');
    }
    match task.stage.as_str() {
        "tertiary" => prompt.push_str(
            "Stage instruction: use routed evidence to refine, refute, drop, or park the concern; do not repeat an already-resolved question.\n",
        ),
        _ => prompt.push_str(
            "Stage instruction: identify the smallest remaining evidence or proof request needed before promotion.\n",
        ),
    }
    prompt.push_str(
        &format!(
            "Return strict JSON with observations, summary_only_findings, failed_objections, and proof_requests. Use question `{}` for observations. Do not emit candidate_findings or inline_comments. Do not post, mutate, or run shell commands.\n",
            task.id
        ),
    );
    prompt
}

pub(crate) fn candidate_evidence_need(candidate: &CandidateRecord) -> String {
    match candidate.disposition.as_str() {
        "inline" => "accepted-inline-review".to_owned(),
        "parked-follow-up" => "parked-follow-up-confirmation".to_owned(),
        "refuted" => "refutation-confirmation".to_owned(),
        "dropped" => "dropped-candidate-audit".to_owned(),
        _ => {
            let text = format!("{}\n{}", candidate.claim, candidate.evidence).to_ascii_lowercase();
            if text.contains("proof") || text.contains("red") || text.contains("green") {
                "proof-confirmation".to_owned()
            } else if text.contains("route") || text.contains("sibling") {
                "source-route-confirmation".to_owned()
            } else if text.contains("test") || text.contains("oracle") {
                "test-oracle-confirmation".to_owned()
            } else {
                "summary-confirmation".to_owned()
            }
        }
    }
}

pub(crate) fn observation_evidence_need(observation: &ObservationGroup) -> String {
    if is_refutation_confirmation_observation(observation) {
        return "refutation-confirmation".to_owned();
    }
    if is_parked_observation(observation) {
        return "parked-follow-up-confirmation".to_owned();
    }
    if observation.kind == "test-gap" {
        return "test-oracle-confirmation".to_owned();
    }
    if observation.kind == "source-route-gap" {
        return "source-route-confirmation".to_owned();
    }

    let text =
        format!("{}\n{}", observation.claim, observation.evidence.join("\n")).to_ascii_lowercase();
    if text.contains("proof")
        || text.contains("red")
        || text.contains("green")
        || text.contains("base+tests")
    {
        "proof-confirmation".to_owned()
    } else if text.contains("route") || text.contains("sibling") {
        "source-route-confirmation".to_owned()
    } else if text.contains("test") || text.contains("oracle") {
        "test-oracle-confirmation".to_owned()
    } else if is_missing_evidence_observation(observation) {
        "evidence-gap-confirmation".to_owned()
    } else if is_residual_risk_observation(observation) {
        "residual-risk-confirmation".to_owned()
    } else {
        "observation-confirmation".to_owned()
    }
}

pub(crate) fn follow_up_task_for_group(
    group_id: &str,
    disposition: &str,
    evidence_need: &str,
    candidate_ids: &[String],
    routed_evidence: &[OrchestratorRoutedEvidence],
) -> Option<FollowUpQuestionTask> {
    if matches!(disposition, "inline" | "dropped") {
        return None;
    }
    let fingerprint = sha256_hex(format!("{group_id}\n{evidence_need}").as_bytes());
    let stage = follow_up_stage(disposition, evidence_need, routed_evidence);
    Some(FollowUpQuestionTask {
        schema: FOLLOW_UP_QUESTION_SCHEMA.to_owned(),
        id: format!("follow-up-{}", &fingerprint[..12]),
        group_id: group_id.to_owned(),
        stage: stage.to_owned(),
        stage_reason: follow_up_stage_reason(stage).to_owned(),
        evidence_need: evidence_need.to_owned(),
        disposition: disposition.to_owned(),
        candidate_ids: candidate_ids.to_vec(),
        observation_group_ids: Vec::new(),
        routed_evidence: routed_evidence.to_vec(),
        question: follow_up_question_text(disposition, evidence_need),
        status: "planned".to_owned(),
        reason: "deterministic orchestrator skeleton; no shell commands or posting side effects"
            .to_owned(),
    })
}

pub(crate) fn follow_up_task_for_observation_group(
    observation: &ObservationGroup,
    group: &OrchestratorObservationGroup,
    routed_evidence: &[OrchestratorRoutedEvidence],
) -> Option<FollowUpQuestionTask> {
    if is_pr_body_artifact_only_observation(observation)
        || matches!(observation.status.as_str(), "covered" | "duplicate")
    {
        return None;
    }
    let fingerprint = sha256_hex(format!("{}\n{}", group.id, group.evidence_need).as_bytes());
    let stage = follow_up_stage("observation", &group.evidence_need, routed_evidence);
    Some(FollowUpQuestionTask {
        schema: FOLLOW_UP_QUESTION_SCHEMA.to_owned(),
        id: format!("follow-up-{}", &fingerprint[..12]),
        group_id: group.id.clone(),
        stage: stage.to_owned(),
        stage_reason: follow_up_stage_reason(stage).to_owned(),
        evidence_need: group.evidence_need.clone(),
        disposition: "observation".to_owned(),
        candidate_ids: Vec::new(),
        observation_group_ids: vec![observation.id.clone()],
        routed_evidence: routed_evidence.to_vec(),
        question: observation_follow_up_question_text(&group.evidence_need),
        status: "planned".to_owned(),
        reason: "deterministic observation follow-up; no shell commands or posting side effects"
            .to_owned(),
    })
}

pub(crate) fn follow_up_stage(
    disposition: &str,
    evidence_need: &str,
    routed_evidence: &[OrchestratorRoutedEvidence],
) -> &'static str {
    if !routed_evidence.is_empty()
        || matches!(disposition, "refuted" | "parked-follow-up")
        || matches!(
            evidence_need,
            "refutation-confirmation" | "parked-follow-up-confirmation"
        )
    {
        "tertiary"
    } else {
        "secondary"
    }
}

pub(crate) fn follow_up_stage_reason(stage: &str) -> &'static str {
    match stage {
        "tertiary" => {
            "routed evidence or prior disposition is available; refine, refute, drop, or park instead of restating the concern"
        }
        _ => {
            "no routed proof receipt is available; ask for the smallest remaining evidence or proof request"
        }
    }
}

pub(crate) fn routed_evidence_for_group(
    evidence_need: &str,
    lanes: &[String],
    proof_receipts: &[ProofReceipt],
    resource_leases: &[ResourceLease],
    tool_gate_outcomes: &[ToolGateOutcomeEntry],
) -> Vec<OrchestratorRoutedEvidence> {
    if !matches!(
        evidence_need,
        "proof-confirmation" | "test-oracle-confirmation" | "source-route-confirmation"
    ) {
        return Vec::new();
    }
    let mut routed = Vec::new();
    for receipt in proof_receipts {
        if !proof_receipt_routes_to_lanes(receipt, lanes) {
            continue;
        }
        routed.push(proof_receipt_routed_evidence(receipt));
        for lease in resource_leases
            .iter()
            .filter(|lease| lease.consumer == receipt.id)
        {
            routed.push(resource_lease_routed_evidence(lease));
        }
    }
    for outcome in tool_gate_outcomes {
        if outcome.evaluated && tool_gate_outcome_routes_to_lanes(outcome, lanes) {
            routed.push(tool_gate_outcome_routed_evidence(outcome));
        }
    }
    routed
}

/// Byte caps for routed receipt-content tails. The second model turn judges
/// proof it was routed, so prompts carry bounded command-output tails instead
/// of only artifact pointers (the runner owns the read; direct provider lanes
/// cannot open files mid-call). stderr gets the larger budget: failure text
/// lives there.
pub(crate) const ROUTED_RECEIPT_STDERR_TAIL_BYTES: usize = 1200;
pub(crate) const ROUTED_RECEIPT_STDOUT_TAIL_BYTES: usize = 600;

/// Bounded content excerpt for a routed proof receipt, or `None` for
/// non-receipt evidence kinds and unknown receipt ids. Each command
/// contributes its status line plus lossy-decoded byte tails of stderr and
/// stdout with explicit truncation markers, so follow-up packets stay
/// bounded no matter how loud the proof command was.
pub(crate) fn routed_proof_receipt_excerpt(
    out: &Path,
    evidence: &OrchestratorRoutedEvidence,
    proof_receipts: &[ProofReceipt],
) -> Option<String> {
    if evidence.kind != "proof-receipt" {
        return None;
    }
    let receipt = proof_receipts
        .iter()
        .find(|receipt| receipt.id == evidence.id)?;
    let mut excerpt = String::new();
    for command in &receipt.commands {
        excerpt.push_str(&format!(
            "  command `{}` side=`{}` status=`{}` exit={:?}\n",
            command.command, command.side, command.status, command.exit_code
        ));
        for (label, relative, cap) in [
            ("stderr", &command.stderr, ROUTED_RECEIPT_STDERR_TAIL_BYTES),
            ("stdout", &command.stdout, ROUTED_RECEIPT_STDOUT_TAIL_BYTES),
        ] {
            if relative.is_empty() {
                continue;
            }
            match fs::read(out.join(relative)) {
                Ok(bytes) if bytes.is_empty() => {
                    excerpt.push_str(&format!("    {label}: (empty)\n"));
                }
                Ok(bytes) => {
                    let truncated = bytes.len() > cap;
                    let mut start = bytes.len().saturating_sub(cap);
                    // Trim forward to a UTF-8 boundary so a mid-character
                    // byte cut cannot produce lossy-decode drift against the
                    // verifier's Python mirror of this excerpt.
                    while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
                        start += 1;
                    }
                    let tail = String::from_utf8_lossy(&bytes[start..]);
                    if truncated {
                        excerpt.push_str(&format!(
                            "    {label} (last {cap} bytes of {total}):\n",
                            total = bytes.len()
                        ));
                    } else {
                        excerpt.push_str(&format!("    {label}:\n"));
                    }
                    for line in tail.lines() {
                        excerpt.push_str("      ");
                        excerpt.push_str(line);
                        excerpt.push('\n');
                    }
                }
                Err(_) => {
                    // An unreadable stream is reported, never silently
                    // dropped: the model should know the excerpt is partial.
                    excerpt.push_str(&format!("    {label}: (unavailable at `{relative}`)\n"));
                }
            }
        }
    }
    (!excerpt.is_empty()).then_some(excerpt)
}

/// Per-evidence-id excerpts for one follow-up task.
pub(crate) fn routed_receipt_excerpts_for_task(
    out: &Path,
    task: &FollowUpQuestionTask,
    proof_receipts: &[ProofReceipt],
) -> BTreeMap<String, String> {
    task.routed_evidence
        .iter()
        .filter_map(|evidence| {
            routed_proof_receipt_excerpt(out, evidence, proof_receipts)
                .map(|excerpt| (evidence.id.clone(), excerpt))
        })
        .collect()
}

pub(crate) fn proof_receipt_routes_to_lanes(receipt: &ProofReceipt, lanes: &[String]) -> bool {
    receipt
        .requested_by
        .iter()
        .any(|lane| lane == "proof-broker")
        || receipt
            .requested_by
            .iter()
            .any(|lane| lanes.iter().any(|group_lane| group_lane == lane))
}

pub(crate) fn proof_receipt_routed_evidence(receipt: &ProofReceipt) -> OrchestratorRoutedEvidence {
    OrchestratorRoutedEvidence {
        schema: ORCHESTRATOR_ROUTED_EVIDENCE_SCHEMA.to_owned(),
        id: receipt.id.clone(),
        kind: "proof-receipt".to_owned(),
        artifact: "review/proof_receipts.json".to_owned(),
        status: routed_status_for_proof_receipt(receipt).to_owned(),
        result: receipt.result.clone(),
        reason: receipt.reason.clone(),
    }
}

pub(crate) fn resource_lease_routed_evidence(lease: &ResourceLease) -> OrchestratorRoutedEvidence {
    OrchestratorRoutedEvidence {
        schema: ORCHESTRATOR_ROUTED_EVIDENCE_SCHEMA.to_owned(),
        id: lease.id.clone(),
        kind: "resource-lease".to_owned(),
        artifact: "review/resource_leases.json".to_owned(),
        status: lease.status.clone(),
        result: lease.status.clone(),
        reason: lease.reason.clone(),
    }
}

pub(crate) fn tool_gate_outcome_routes_to_lanes(
    outcome: &ToolGateOutcomeEntry,
    lanes: &[String],
) -> bool {
    let consumers = work_queue_sensor_consumers(&outcome.tool);
    consumers
        .iter()
        .any(|consumer| consumer == "all-lanes" || lanes.iter().any(|lane| lane == consumer))
}

pub(crate) fn tool_gate_outcome_routed_evidence(
    outcome: &ToolGateOutcomeEntry,
) -> OrchestratorRoutedEvidence {
    let mut reason = outcome.reason.clone();
    if let Some(new_unsuppressed) = outcome.metrics.new_unsuppressed {
        reason.push_str(&format!("; new_unsuppressed={new_unsuppressed}"));
    }
    if !outcome.source_artifacts.is_empty() {
        reason.push_str("; source_artifacts=");
        reason.push_str(&outcome.source_artifacts.join(","));
    }
    OrchestratorRoutedEvidence {
        schema: ORCHESTRATOR_ROUTED_EVIDENCE_SCHEMA.to_owned(),
        id: outcome.tool.clone(),
        kind: "tool-gate-outcome".to_owned(),
        artifact: format!("review/tool-gate-outcomes.json#{}", outcome.tool),
        status: routed_status_for_tool_gate_outcome(outcome).to_owned(),
        result: outcome.outcome.clone(),
        reason,
    }
}

pub(crate) fn routed_status_for_tool_gate_outcome(outcome: &ToolGateOutcomeEntry) -> &'static str {
    match outcome.outcome.as_str() {
        "passed" => "tool-gate-passed",
        "failed" => "tool-gate-failed",
        _ if outcome.evaluated => "tool-gate-evaluated",
        _ => "recorded",
    }
}

pub(crate) fn routed_status_for_proof_receipt(receipt: &ProofReceipt) -> &'static str {
    if proof_receipt_is_test_proof_result(receipt) {
        "tool-confirmed"
    } else if proof_receipt_is_missing_evidence(receipt) {
        "missing-evidence"
    } else if proof_receipt_is_residual_risk(receipt) {
        "residual-risk"
    } else {
        "recorded"
    }
}

pub(crate) fn observation_follow_up_question_text(evidence_need: &str) -> String {
    match evidence_need {
        "proof-confirmation" => {
            "Confirm whether routed proof evidence resolves this observation.".to_owned()
        }
        "source-route-confirmation" => {
            "Confirm the changed source route or sibling path before promoting this observation."
                .to_owned()
        }
        "test-oracle-confirmation" => {
            "Confirm the test oracle strength before promoting this observation.".to_owned()
        }
        "refutation-confirmation" => {
            "Confirm the observation refutation still matches current PR evidence.".to_owned()
        }
        "parked-follow-up-confirmation" => {
            "Confirm whether this observation remains parked outside current PR scope.".to_owned()
        }
        "evidence-gap-confirmation" => {
            "Confirm whether this observation is still trust-affecting missing evidence.".to_owned()
        }
        "residual-risk-confirmation" => {
            "Confirm whether this observation remains specific residual risk.".to_owned()
        }
        _ => "Confirm whether this observation needs promotion, refutation, or parking.".to_owned(),
    }
}

pub(crate) fn follow_up_question_text(disposition: &str, evidence_need: &str) -> String {
    match (disposition, evidence_need) {
        ("refuted", _) => "Confirm the refutation still matches the current PR evidence.".to_owned(),
        ("parked-follow-up", _) => {
            "Confirm whether this parked follow-up should remain outside current PR scope.".to_owned()
        }
        (_, "proof-confirmation") => {
            "Confirm whether focused proof can resolve this summary-only candidate.".to_owned()
        }
        (_, "source-route-confirmation") => {
            "Confirm the changed source route or sibling path before promoting this candidate."
                .to_owned()
        }
        (_, "test-oracle-confirmation") => {
            "Confirm the test oracle strength before promoting this candidate.".to_owned()
        }
        _ => "Confirm whether additional evidence should promote or keep this candidate summary-only."
            .to_owned(),
    }
}

pub(crate) fn orchestrator_group_reason(disposition: &str, evidence_need: &str) -> String {
    format!("grouped candidate disposition `{disposition}` under evidence need `{evidence_need}`")
}

pub(crate) fn unique_sorted(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
