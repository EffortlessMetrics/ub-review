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
        revision: None,
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
    // Outputs and receipts are immutable during this write. Share their
    // excerpts across consumers, then discard the snapshot before any later
    // proof phase or reused-output invocation can replace the source evidence.
    let mut excerpts_by_receipt = BTreeMap::new();
    for task in tasks {
        // Excerpts are resolved at packet-write time so the packet artifact
        // records exactly the prompt the model later receives.
        let excerpts =
            routed_receipt_excerpts_for_task(out, task, proof_receipts, &mut excerpts_by_receipt);
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

#[cfg(test)]
mod routed_packet_tests {
    use super::*;
    use anyhow::ensure;
    use std::io::{Seek, SeekFrom, Write};

    fn receipt() -> ProofReceipt {
        ProofReceipt {
            schema: "ub-review.proof_receipt.v1".to_owned(),
            id: "proof-1".to_owned(),
            kind: "focused-build".to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            revision: None,
            test_patch_mode: "head".to_owned(),
            requested_by: vec!["tests".to_owned()],
            request_ids: Vec::new(),
            commands: vec![ProofCommandReceipt {
                side: "head".to_owned(),
                command: "cargo check --locked".to_owned(),
                env: BTreeMap::new(),
                status: "passed".to_owned(),
                exit_code: Some(0),
                timed_out: false,
                timeout_sec: 60,
                duration_ms: 1,
                stdout: "stdout.txt".to_owned(),
                stderr: "stderr.txt".to_owned(),
                reason: "completed".to_owned(),
            }],
            result: "passed".to_owned(),
            reason: "completed".to_owned(),
        }
    }

    fn task(id: usize, receipt: &ProofReceipt) -> FollowUpQuestionTask {
        FollowUpQuestionTask {
            schema: "ub-review.follow_up_question_task.v1".to_owned(),
            id: format!("task-{id}"),
            group_id: "group".to_owned(),
            stage: "tertiary".to_owned(),
            stage_reason: "proof available".to_owned(),
            evidence_need: "proof-confirmation".to_owned(),
            disposition: "summary-only".to_owned(),
            candidate_ids: Vec::new(),
            observation_group_ids: Vec::new(),
            routed_evidence: vec![proof_receipt_routed_evidence(receipt)],
            question: "Does the proof resolve this?".to_owned(),
            status: "pending".to_owned(),
            reason: "proof available".to_owned(),
        }
    }

    fn packet(out: &Path, id: usize) -> Result<serde_json::Value> {
        Ok(serde_json::from_slice(&fs::read(out.join(format!(
            "questions/orchestrator-follow-up/task-{id}.json"
        )))?)?)
    }

    #[test]
    fn routed_packet_consumption_measurement() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        // Sparse files keep fixture setup cheap while exercising multi-MiB
        // streams. Only the final caps belong in each downstream packet.
        let stream_bytes = 4 * 1024 * 1024;
        for (name, cap) in [("stdout.txt", 600), ("stderr.txt", 1200)] {
            let mut file = fs::File::create(out.join(name))?;
            file.set_len(stream_bytes)?;
            file.seek(SeekFrom::End(-(cap as i64)))?;
            file.write_all(&vec![b'x'; cap])?;
        }
        let receipt = receipt();
        let tasks: Vec<_> = (0..16).map(|id| task(id, &receipt)).collect();
        let started = Instant::now();
        write_follow_up_question_packets(out, &tasks, &[receipt])?;
        let elapsed = started.elapsed();
        let expected_excerpt = format!(
            "  command `cargo check --locked` side=`head` status=`passed` exit=Some(0)\n    stderr (last 1200 bytes of {stream_bytes}):\n      {}\n    stdout (last 600 bytes of {stream_bytes}):\n      {}\n",
            "x".repeat(1200),
            "x".repeat(600)
        );
        let mut output_bytes = 0;
        for id in 0..16 {
            let value = packet(out, id)?;
            let prompt = value["prompt"].as_str().context("prompt")?;
            ensure!(prompt.contains(&expected_excerpt));
            ensure!(!prompt.contains('\0'));
            output_bytes += serde_json::to_vec_pretty(&value)?.len();
        }
        eprintln!(
            "routed packet measurement: 16 tasks, 2 x {stream_bytes} byte streams, {output_bytes} packet bytes, {elapsed:?}"
        );
        Ok(())
    }

    #[test]
    fn routed_packet_refreshes_sources_and_rejects_unmatched_routes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let mut receipt = receipt();
        let mut tasks = vec![task(0, &receipt), task(1, &receipt)];
        fs::write(out.join("stdout.txt"), "old-proof")?;
        fs::write(out.join("stderr.txt"), "")?;
        write_follow_up_question_packets(out, &tasks, &[receipt.clone()])?;
        ensure!(
            packet(out, 0)?["prompt"]
                .as_str()
                .context("prompt")?
                .contains("old-proof")
        );

        // Same path and length, new source bytes: no cross-write cache may
        // relabel the previous invocation's excerpt as current evidence.
        fs::write(out.join("stdout.txt"), "new-proof")?;
        receipt.commands[0].status = "failed".to_owned();
        receipt.commands[0].exit_code = Some(101);
        receipt.result = "failed".to_owned();
        tasks[0].routed_evidence = vec![proof_receipt_routed_evidence(&receipt)];
        tasks[1].routed_evidence = tasks[0].routed_evidence.clone();
        let mut wrong_kind = tasks[0].routed_evidence[0].clone();
        wrong_kind.kind = "resource-lease".to_owned();
        tasks[0].routed_evidence.insert(0, wrong_kind);
        let mut unknown = tasks[1].routed_evidence[0].clone();
        unknown.id = "absent-proof".to_owned();
        tasks[1].routed_evidence = vec![unknown];
        write_follow_up_question_packets(out, &tasks, &[receipt.clone()])?;
        let current = packet(out, 0)?;
        let prompt = current["prompt"].as_str().context("prompt")?;
        ensure!(prompt.contains("new-proof") && !prompt.contains("old-proof"));
        ensure!(prompt.contains("status=`failed` exit=Some(101)"));
        ensure!(
            !packet(out, 1)?["prompt"]
                .as_str()
                .context("prompt")?
                .contains("Receipt content")
        );

        fs::remove_file(out.join("stdout.txt"))?;
        write_follow_up_question_packets(out, &tasks, &[receipt])?;
        let unavailable = packet(out, 0)?;
        let prompt = unavailable["prompt"].as_str().context("prompt")?;
        ensure!(prompt.contains("stdout: (unavailable at `stdout.txt`)"));
        ensure!(!prompt.contains("new-proof"));
        Ok(())
    }

    #[test]
    fn routed_packet_batch_reuses_only_matching_receipt_excerpts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let receipt = receipt();
        let task = task(0, &receipt);
        fs::write(out.join("stdout.txt"), "same-batch-proof")?;
        fs::write(out.join("stderr.txt"), "")?;
        let receipts = [receipt];
        let mut excerpts = BTreeMap::new();
        let first = routed_receipt_excerpts_for_task(out, &task, &receipts, &mut excerpts);
        ensure!(first["proof-1"].contains("same-batch-proof"));
        // A second consumer must reuse the existing snapshot, not re-open
        // streams. A fresh batch must instead report their absence.
        fs::remove_file(out.join("stdout.txt"))?;
        let second = routed_receipt_excerpts_for_task(out, &task, &receipts, &mut excerpts);
        ensure!(first == second);
        let fresh = routed_receipt_excerpts_for_task(out, &task, &receipts, &mut BTreeMap::new());
        ensure!(fresh["proof-1"].contains("unavailable"));
        let mut unrelated = task.clone();
        unrelated.routed_evidence[0].kind = "resource-lease".to_owned();
        ensure!(
            routed_receipt_excerpts_for_task(out, &unrelated, &receipts, &mut excerpts).is_empty()
        );
        unrelated.routed_evidence[0].kind = "proof-receipt".to_owned();
        unrelated.routed_evidence[0].id = "other-proof".to_owned();
        ensure!(
            routed_receipt_excerpts_for_task(out, &unrelated, &receipts, &mut excerpts).is_empty()
        );
        Ok(())
    }

    #[test]
    fn routed_packet_tail_preserves_utf8_and_missing_stream_format() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let receipt = receipt();
        // The 600-byte cut lands inside the euro character. It must trim
        // continuation bytes exactly as the full-read verifier mirror does.
        fs::write(out.join("stdout.txt"), format!("xx€{}", "y".repeat(598)))?;
        fs::create_dir(out.join("stderr.txt"))?;
        let evidence = proof_receipt_routed_evidence(&receipt);
        let excerpt = routed_proof_receipt_excerpt(out, &evidence, std::slice::from_ref(&receipt))
            .context("excerpt")?;
        ensure!(excerpt.contains(&format!(
            "stdout (last 600 bytes of 603):\n      {}\n",
            "y".repeat(598)
        )));
        ensure!(!excerpt.contains('�'));
        ensure!(excerpt.contains("stderr: (unavailable at `stderr.txt`)"));
        fs::write(out.join("stdout.txt"), [0xff, b'x', b'\r', b'\n'])?;
        let excerpt =
            routed_proof_receipt_excerpt(out, &evidence, &[receipt]).context("excerpt")?;
        ensure!(excerpt.contains("stdout:\n      �x\n"));
        Ok(())
    }
}
