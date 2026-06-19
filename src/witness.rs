//! Witness records, follow-up question packets, and witness registry
//! artifact construction (cleanup train step 42, pure code motion).

use crate::*;

pub(crate) fn follow_up_stage_record(result: &FollowUpResult) -> ModelStageRecord {
    ModelStageRecord {
        schema: MODEL_STAGE_SCHEMA.to_owned(),
        lane: result.model_lane.clone(),
        source: "orchestrator-follow-up".to_owned(),
        stage: result.stage.clone(),
        stage_reason: follow_up_stage_reason(&result.stage).to_owned(),
        status: result.status.clone(),
        reason: result.reason.clone(),
        provider: result.provider.clone(),
        model: result.model.clone(),
        endpoint_kind: result.endpoint_kind.clone(),
        task_id: Some(result.task_id.clone()),
        group_id: Some(result.group_id.clone()),
        packet_path: Some(result.packet_path.clone()),
        duration_ms: result.duration_ms,
        http_status: result.http_status,
        response_shape: result.response_shape.clone(),
        cache_usage: result.cache_usage.clone(),
    }
}

pub(crate) fn follow_up_evidence_from_outputs(
    outputs: &[FollowUpOutputRecord],
) -> FollowUpEvidenceArtifact {
    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    let mut observations = Vec::new();
    let mut proof_requests = Vec::new();
    for output in outputs {
        inline_comments.extend(output.inline_comments.iter().cloned());
        summary_only_findings.extend(output.summary_only_findings.iter().cloned());
        observations.extend(output.observations.iter().cloned());
        proof_requests.extend(output.proof_requests.iter().cloned());
    }
    FollowUpEvidenceArtifact {
        schema: FOLLOW_UP_EVIDENCE_SCHEMA.to_owned(),
        follow_up_outputs: outputs.len(),
        inline_comments,
        summary_only_findings,
        observations,
        proof_requests,
    }
}

pub(crate) fn write_follow_up_evidence_artifact(
    out: &Path,
    evidence: &FollowUpEvidenceArtifact,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("follow_up_evidence.json"),
        serde_json::to_vec_pretty(evidence)?,
    )?;
    Ok(())
}

pub(crate) fn write_follow_up_question_packets(
    out: &Path,
    tasks: &[FollowUpQuestionTask],
    proof_receipts: &[ProofReceipt],
) -> Result<()> {
    let follow_up_dir = out.join("questions").join("orchestrator-follow-up");
    if follow_up_dir.exists() {
        fs::remove_dir_all(&follow_up_dir)
            .with_context(|| format!("remove {}", follow_up_dir.display()))?;
    }
    if tasks.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(&follow_up_dir)
        .with_context(|| format!("create {}", follow_up_dir.display()))?;
    for task in tasks {
        // Excerpts are resolved at packet-write time so the packet artifact
        // records exactly the prompt the model later receives.
        let excerpts = routed_receipt_excerpts_for_task(out, task, proof_receipts);
        let packet = follow_up_question_packet(task, &excerpts);
        fs::write(
            follow_up_dir.join(format!("{}.json", sanitize_artifact_name(&task.id))),
            serde_json::to_vec_pretty(&packet)?,
        )?;
    }
    Ok(())
}

pub(crate) fn follow_up_question_packet<'a>(
    task: &'a FollowUpQuestionTask,
    routed_excerpts: &BTreeMap<String, String>,
) -> FollowUpQuestionPacket<'a> {
    FollowUpQuestionPacket {
        schema: FOLLOW_UP_QUESTION_PACKET_SCHEMA,
        id: task.id.as_str(),
        task_id: task.id.as_str(),
        group_id: task.group_id.as_str(),
        stage: task.stage.as_str(),
        stage_reason: task.stage_reason.as_str(),
        evidence_need: task.evidence_need.as_str(),
        disposition: task.disposition.as_str(),
        candidate_ids: &task.candidate_ids,
        observation_group_ids: &task.observation_group_ids,
        routed_evidence: &task.routed_evidence,
        question: task.question.as_str(),
        status: task.status.as_str(),
        source_artifact: "review/orchestrator_plan.json",
        prompt: render_follow_up_question_prompt(task, routed_excerpts),
    }
}

pub(crate) struct WitnessRecordInput<'a> {
    pub(crate) status: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) source: &'a str,
    pub(crate) claim: &'a str,
    pub(crate) dedupe_key: &'a str,
    pub(crate) evidence: Vec<String>,
    pub(crate) lane: Option<String>,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<u32>,
    pub(crate) observation_id: Option<String>,
    pub(crate) proof_receipt_id: Option<String>,
}

pub(crate) fn witness_record(input: WitnessRecordInput<'_>) -> WitnessRecord {
    let fingerprint = sha256_hex(
        format!(
            "{}\n{}\n{}\n{}",
            input.status, input.kind, input.source, input.dedupe_key
        )
        .as_bytes(),
    );
    WitnessRecord {
        schema: WITNESS_SCHEMA.to_owned(),
        id: format!("witness-{}", &fingerprint[..12]),
        status: input.status.to_owned(),
        kind: input.kind.to_owned(),
        source: input.source.to_owned(),
        claim: input.claim.to_owned(),
        dedupe_key: input.dedupe_key.to_owned(),
        evidence: non_empty_evidence(input.evidence, "witness registry source artifact"),
        lane: input.lane,
        path: input.path,
        line: input.line,
        observation_id: input.observation_id,
        proof_receipt_id: input.proof_receipt_id,
    }
}

pub(crate) fn witness_status_for_summary_finding(finding: &SummaryOnlyFinding) -> &'static str {
    if is_parked_follow_up(finding) {
        "parked"
    } else {
        "needs-witness"
    }
}

pub(crate) fn witness_status_for_observation(observation: &Observation) -> &'static str {
    if observation.status == "refuted"
        || matches!(
            observation.kind.as_str(),
            "false-premise" | "resolved-check"
        )
    {
        "refuted"
    } else if observation.status == "parked" || observation.kind == "parked-follow-up" {
        "parked"
    } else if observation.status == "confirmed"
        || matches!(observation.kind.as_str(), "bug" | "security-risk")
    {
        "tool-confirmed"
    } else {
        "needs-witness"
    }
}

pub(crate) fn witness_status_for_proof_receipt(receipt: &ProofReceipt) -> &'static str {
    match receipt.result.as_str() {
        "discriminating" | "head_passed" | "head_failed" => "tool-confirmed",
        _ => "needs-witness",
    }
}

pub(crate) fn proof_receipt_witness_evidence(receipt: &ProofReceipt) -> Vec<String> {
    let mut evidence = Vec::new();
    for command in &receipt.commands {
        evidence.push(format!(
            "{} `{}` status=`{}` reason=`{}` stdout=`{}` stderr=`{}`",
            command.side,
            command.command,
            command.status,
            command.reason,
            command.stdout,
            command.stderr
        ));
    }
    if evidence.is_empty() {
        evidence.push(receipt.reason.clone());
    }
    evidence
}

pub(crate) fn witness_registry_artifact(witnesses: &[WitnessRecord]) -> WitnessRegistryArtifact {
    let mut status_counts = BTreeMap::new();
    let mut kind_counts = BTreeMap::new();
    let mut source_counts = BTreeMap::new();
    let mut follow_up_status_counts = BTreeMap::new();
    let mut witness_ids_by_status: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut follow_up_witness_ids_by_status: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut follow_up_total = 0;

    for witness in witnesses {
        *status_counts.entry(witness.status.clone()).or_insert(0) += 1;
        *kind_counts.entry(witness.kind.clone()).or_insert(0) += 1;
        *source_counts.entry(witness.source.clone()).or_insert(0) += 1;
        witness_ids_by_status
            .entry(witness.status.clone())
            .or_default()
            .push(witness.id.clone());

        if witness.source.starts_with("follow-up-") {
            follow_up_total += 1;
            *follow_up_status_counts
                .entry(witness.status.clone())
                .or_insert(0) += 1;
            follow_up_witness_ids_by_status
                .entry(witness.status.clone())
                .or_default()
                .push(witness.id.clone());
        }
    }

    WitnessRegistryArtifact {
        schema: WITNESS_REGISTRY_SCHEMA.to_owned(),
        total: witnesses.len(),
        status_counts,
        kind_counts,
        source_counts,
        follow_up_total,
        follow_up_status_counts,
        witness_ids_by_status,
        follow_up_witness_ids_by_status,
    }
}

pub(crate) fn write_witness_artifacts(out: &Path, witnesses: &[WitnessRecord]) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let registry = witness_registry_artifact(witnesses);
    fs::write(
        review_dir.join("witnesses.json"),
        serde_json::to_vec_pretty(witnesses)?,
    )?;
    fs::write(
        review_dir.join("witness_registry.json"),
        serde_json::to_vec_pretty(&registry)?,
    )?;
    let mut ndjson = String::new();
    for witness in witnesses {
        ndjson.push_str(&serde_json::to_string(witness)?);
        ndjson.push('\n');
    }
    fs::write(out.join("witnesses.ndjson"), ndjson)?;
    Ok(())
}
