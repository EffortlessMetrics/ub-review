//! Proof planner model lane and follow-up model pass orchestration
//! (cleanup train step 50, pure code motion).

use crate::*;

pub(crate) fn should_run_proof_planner_model_lane(args: &RunArgs, diff: &DiffContext) -> bool {
    matches!(args.mode, RunMode::IntelligentCi)
        && !matches!(
            diff.diff_class,
            DiffClass::DocsOnly | DiffClass::ArtifactOnlySmoke
        )
}

pub(crate) fn run_proof_planner_model_lane(
    context: ProofPlannerRunContext<'_>,
    model_lanes: &mut Vec<ModelLaneReceipt>,
    missing_or_failed_model_evidence: &mut Vec<ModelEvidenceIssue>,
    model_observations: &mut Vec<Observation>,
    proof_requests: &mut Vec<ProofRequest>,
) -> Result<usize> {
    if !should_run_proof_planner_model_lane(context.args, context.diff) {
        return Ok(0);
    }

    let assignment = proof_planner_assignment_with_key_state(
        context.args,
        (context.key_present)(model_api_key_env(ModelProvider::OpenCodeGo)),
    );
    let lane = assignment.lane.clone();
    let mut spec = assignment.spec.clone();
    let mut receipt = ModelLaneReceipt {
        lane: lane.id.clone(),
        provider: spec.provider.key().to_owned(),
        model: spec.model.clone(),
        endpoint_kind: spec.endpoint_kind.key().to_owned(),
        status: "planned".to_owned(),
        reason: "planned advisory proof-planner lane for intelligent-ci".to_owned(),
        duration_ms: None,
        http_status: None,
        response_shape: None,
        fallback_from: None,
        cache_usage: ModelCacheUsage::default(),
        cohort_id: String::new(),
        shared_prefix_hash: String::new(),
        thread_id: String::new(),
        turn: 0,
        cohort_broken: false,
    };

    if context.model_calls_used >= context.args.max_model_calls {
        receipt.status = "skipped_budget".to_owned();
        receipt.reason = "model call budget exhausted before proof-planner lane".to_owned();
        model_lanes.push(receipt);
        return Ok(0);
    }
    let Some((selected_spec, fallback_from, preflight_reason)) =
        selected_provider_spec(&assignment, context.provider_preflights)
    else {
        receipt.status = "preflight_failed".to_owned();
        receipt.reason =
            provider_assignment_preflight_failed_reason(&assignment, context.provider_preflights);
        missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        model_lanes.push(receipt);
        return Ok(0);
    };
    spec = selected_spec;
    receipt.provider = spec.provider.key().to_owned();
    receipt.model = spec.model.clone();
    receipt.endpoint_kind = spec.endpoint_kind.key().to_owned();
    if let Some(original) = fallback_from {
        receipt.fallback_from = Some(original);
    }
    // Stamp cohort provenance from the actual (post-fallback) provider/model
    // against the cohort primary (assignment.spec). The shared prefix hash is
    // the run's cache-coherence proof; cohort_broken is true when the lane
    // used a different provider/model than the cohort primary. (Order 5 #678)
    let prefix_hash = sha256_hex(context.shared_context.as_bytes());
    let primary = &assignment.spec;
    let fb = receipt
        .fallback_from
        .as_ref()
        .map(|_| (spec.provider.key(), spec.model.as_str()));
    let (cohort_id, shared_prefix_hash, thread_id, turn, cohort_broken) = cohort_stamp(
        primary.provider.key(),
        &primary.model,
        &prefix_hash,
        &lane.id,
        0,
        fb,
    );
    receipt.cohort_id = cohort_id;
    receipt.shared_prefix_hash = shared_prefix_hash;
    receipt.thread_id = thread_id;
    receipt.turn = turn;
    receipt.cohort_broken = cohort_broken;
    if let Some(reason) = preflight_reason {
        receipt.reason = reason;
    }
    let env_name = model_api_key_env(spec.provider);
    if !(context.key_present)(env_name) {
        let key_label = model_api_key_label(spec.provider);
        receipt.status = "missing_key".to_owned();
        receipt.reason = format!("{key_label} not provided; proof-planner output unavailable");
        missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        model_lanes.push(receipt);
        return Ok(0);
    }

    let lane_dir = context.review_dir.join("model").join(&lane.id);
    fs::create_dir_all(&lane_dir)?;
    receipt.status = "running".to_owned();
    match call_model_proof_planner(
        context.root,
        &lane_dir,
        &lane,
        &spec,
        context.shared_context,
        context.diff,
        context.profile,
        context.box_state,
        context.pr_thread_context,
        proof_requests,
        context
            .impact_candidates
            .iter()
            .map(|c| ImpactCandidateSummary {
                target: c.target.clone(),
                reason: c.reason.clone(),
                owning_package: c.owning_package.clone(),
                estimated_cost: c.estimated_cost.to_owned(),
                expected_value: c.expected_value.to_owned(),
                rank: c.rank,
                selection: c.selection.to_owned(),
            })
            .collect(),
        context.args,
    ) {
        Ok(outcome) => {
            if outcome.degraded {
                receipt.status = "degraded".to_owned();
                receipt.reason =
                    "contentful proof-planner output was preserved as degraded evidence".to_owned();
            } else {
                receipt.status = "ok".to_owned();
                receipt.reason = "completed".to_owned();
            }
            receipt.duration_ms = Some(outcome.duration_ms);
            receipt.http_status = outcome.http_status;
            receipt.response_shape = Some(outcome.response_shape);
            receipt.cache_usage = outcome.cache_usage;
            apply_proof_planner_model_output(
                &lane,
                outcome.output,
                context.line_map,
                model_observations,
                proof_requests,
            );
        }
        Err(err) => {
            receipt.status = classify_model_error(&err);
            receipt.reason = format!("{err:#}");
            receipt.http_status = http_status_from_error(&err);
            missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        }
    }
    model_lanes.push(receipt);
    Ok(1)
}

pub(crate) fn run_follow_up_model_pass(
    context: FollowUpRunContext<'_>,
    follow_up_results: &mut Vec<FollowUpResult>,
    follow_up_outputs: &mut Vec<FollowUpOutputRecord>,
) -> Result<usize> {
    let assignment = follow_up_provider_assignment_with_key_state(
        context.args,
        (context.key_present)(model_api_key_env(ModelProvider::OpenCodeGo)),
    );
    let default_spec = assignment.spec.clone();
    let selected_provider = selected_provider_spec(&assignment, context.provider_preflights);
    let available = context
        .args
        .max_model_calls
        .saturating_sub(context.model_calls_used);
    let model_mode_enabled = matches!(context.args.model_mode, ModelMode::Auto);
    let mut calls = 0usize;
    for task in context.tasks {
        let model_lane = follow_up_model_lane_id(task);
        let packet_path = follow_up_packet_artifact_path(task);
        let packet = read_follow_up_packet(context.out, task)?;
        if !model_mode_enabled {
            let result = follow_up_result(FollowUpResultInput {
                task,
                packet_path: &packet_path,
                model_lane: &model_lane,
                spec: &default_spec,
                fallback_from: None,
                status: "skipped",
                reason: "model-mode off; follow-up task remains artifact-only",
                artifacts: FollowUpResultArtifacts::default(),
                output_counts: FollowUpOutputCounts::default(),
            });
            follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
            follow_up_results.push(result);
            continue;
        }
        let Some((spec, fallback_from, _preflight_reason)) = selected_provider.clone() else {
            let reason = provider_assignment_preflight_failed_reason(
                &assignment,
                context.provider_preflights,
            );
            let result = follow_up_result(FollowUpResultInput {
                task,
                packet_path: &packet_path,
                model_lane: &model_lane,
                spec: &default_spec,
                fallback_from: None,
                status: "preflight_failed",
                reason: &reason,
                artifacts: FollowUpResultArtifacts::default(),
                output_counts: FollowUpOutputCounts::default(),
            });
            follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
            follow_up_results.push(result);
            continue;
        };
        let fallback_from_for_result = fallback_from.clone();
        let key_present = (context.key_present)(model_api_key_env(spec.provider));
        if !key_present {
            let reason = format!(
                "{} not provided; follow-up task remains artifact-only",
                model_api_key_env(spec.provider)
            );
            let result = follow_up_result(FollowUpResultInput {
                task,
                packet_path: &packet_path,
                model_lane: &model_lane,
                spec: &spec,
                fallback_from: fallback_from_for_result,
                status: "missing_key",
                reason: &reason,
                artifacts: FollowUpResultArtifacts::default(),
                output_counts: FollowUpOutputCounts::default(),
            });
            follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
            follow_up_results.push(result);
            continue;
        }
        if calls >= available {
            let result = follow_up_result(FollowUpResultInput {
                task,
                packet_path: &packet_path,
                model_lane: &model_lane,
                spec: &spec,
                fallback_from: fallback_from_for_result,
                status: "skipped_budget",
                reason: "follow-up model call budget exhausted before task execution",
                artifacts: FollowUpResultArtifacts::default(),
                output_counts: FollowUpOutputCounts::default(),
            });
            follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
            follow_up_results.push(result);
            continue;
        }

        let task_dir = context.review_dir.join("model").join(&model_lane);
        fs::create_dir_all(&task_dir)?;
        calls += 1;
        match call_model_prompt_cached(
            context.root,
            &task_dir,
            &spec,
            context.shared_context,
            &packet.prompt,
            context.args,
        ) {
            Ok(outcome) => {
                let status = if outcome.degraded { "degraded" } else { "ok" };
                let reason = if outcome.degraded {
                    "contentful follow-up output was preserved as degraded evidence"
                } else {
                    "completed"
                };
                let output_counts = follow_up_output_counts(&outcome.output);
                let output_record = follow_up_output_record(
                    task,
                    &model_lane,
                    status,
                    reason,
                    outcome.output,
                    context.line_map,
                );
                let mut result = follow_up_result(FollowUpResultInput {
                    task,
                    packet_path: &packet_path,
                    model_lane: &model_lane,
                    spec: &spec,
                    fallback_from: fallback_from_for_result,
                    status,
                    reason,
                    artifacts: follow_up_result_artifacts(&model_lane, &task_dir),
                    output_counts,
                });
                result.duration_ms = Some(outcome.duration_ms);
                result.http_status = outcome.http_status;
                result.response_shape = Some(outcome.response_shape);
                result.cache_usage = outcome.cache_usage;
                follow_up_outputs.push(output_record);
                follow_up_results.push(result);
            }
            Err(err) => {
                let status = classify_model_error(&err);
                let mut result = follow_up_result(FollowUpResultInput {
                    task,
                    packet_path: &packet_path,
                    model_lane: &model_lane,
                    spec: &spec,
                    fallback_from: fallback_from_for_result,
                    status: &status,
                    reason: &format!("{err:#}"),
                    artifacts: follow_up_result_artifacts(&model_lane, &task_dir),
                    output_counts: FollowUpOutputCounts::default(),
                });
                result.http_status = http_status_from_error(&err);
                follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
                follow_up_results.push(result);
            }
        }
    }
    Ok(calls)
}

pub(crate) fn read_follow_up_packet(
    out: &Path,
    task: &FollowUpQuestionTask,
) -> Result<FollowUpQuestionPacketArtifact> {
    let path = out.join(follow_up_packet_artifact_path(task));
    let packet: FollowUpQuestionPacketArtifact = serde_json::from_slice(&fs::read(&path)?)?;
    if packet.schema != FOLLOW_UP_QUESTION_PACKET_SCHEMA
        || packet.task_id != task.id
        || packet.group_id != task.group_id
        || packet.id != task.id
        || packet.stage != task.stage
        || packet.stage_reason != task.stage_reason
    {
        bail!(
            "follow-up packet {} does not match task {}",
            path.display(),
            task.id
        );
    }
    Ok(packet)
}

pub(crate) fn follow_up_packet_artifact_path(task: &FollowUpQuestionTask) -> String {
    format!(
        "questions/orchestrator-follow-up/{}.json",
        sanitize_artifact_name(&task.id)
    )
}

pub(crate) fn follow_up_model_lane_id(task: &FollowUpQuestionTask) -> String {
    format!(
        "orchestrator-follow-up-{}",
        sanitize_artifact_name(&task.id)
    )
}

#[derive(Default)]
pub(crate) struct FollowUpResultArtifacts {
    request_path: Option<String>,
    response_path: Option<String>,
    content_path: Option<String>,
    normalized_content_path: Option<String>,
    stderr_path: Option<String>,
}

pub(crate) struct FollowUpResultInput<'a> {
    task: &'a FollowUpQuestionTask,
    packet_path: &'a str,
    model_lane: &'a str,
    spec: &'a ProviderSpec,
    fallback_from: Option<String>,
    status: &'a str,
    reason: &'a str,
    artifacts: FollowUpResultArtifacts,
    output_counts: FollowUpOutputCounts,
}

pub(crate) fn follow_up_result_artifacts(
    model_lane: &str,
    task_dir: &Path,
) -> FollowUpResultArtifacts {
    FollowUpResultArtifacts {
        request_path: follow_up_result_artifact_path(model_lane, task_dir, "request.json"),
        response_path: follow_up_result_artifact_path(model_lane, task_dir, "response.json"),
        content_path: follow_up_result_artifact_path(model_lane, task_dir, "content.json"),
        normalized_content_path: follow_up_result_artifact_path(
            model_lane,
            task_dir,
            "content-normalized.json",
        ),
        stderr_path: follow_up_result_artifact_path(model_lane, task_dir, "stderr.txt"),
    }
}

pub(crate) fn follow_up_result_artifact_path(
    model_lane: &str,
    task_dir: &Path,
    file_name: &str,
) -> Option<String> {
    task_dir
        .join(file_name)
        .exists()
        .then(|| format!("review/model/{model_lane}/{file_name}"))
}

pub(crate) fn follow_up_output_counts(output: &LaneModelOutput) -> FollowUpOutputCounts {
    FollowUpOutputCounts {
        observations: output.observations.len(),
        candidate_findings: output.candidate_findings.len() + output.inline_comments.len(),
        summary_only_findings: output.summary_only_findings.len()
            + usize::from(output.summary.is_some()),
        failed_objections: output.failed_objections.len(),
        proof_requests: output.proof_requests.len(),
    }
}

pub(crate) fn follow_up_output_record(
    task: &FollowUpQuestionTask,
    model_lane: &str,
    status: &str,
    reason: &str,
    output: LaneModelOutput,
    line_map: &BTreeSet<(String, u32)>,
) -> FollowUpOutputRecord {
    let lane = follow_up_lane(task, model_lane);
    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    let mut observations = Vec::new();
    let mut proof_requests = Vec::new();
    // Follow-up strict-JSON contracts do not request issue_candidates; the
    // sink is local so an off-contract emission is dropped, not crashed on.
    let mut follow_up_issue_candidates = Vec::new();
    apply_model_output(
        &lane,
        output,
        line_map,
        ModelOutputSinks {
            inline_comments: &mut inline_comments,
            summary_only_findings: &mut summary_only_findings,
            model_observations: &mut observations,
            proof_requests: &mut proof_requests,
            issue_candidates: &mut follow_up_issue_candidates,
        },
    );
    FollowUpOutputRecord {
        schema: FOLLOW_UP_OUTPUT_SCHEMA.to_owned(),
        task_id: task.id.clone(),
        group_id: task.group_id.clone(),
        stage: task.stage.clone(),
        disposition: task.disposition.clone(),
        evidence_need: task.evidence_need.clone(),
        candidate_ids: task.candidate_ids.clone(),
        observation_group_ids: task.observation_group_ids.clone(),
        model_lane: model_lane.to_owned(),
        status: status.to_owned(),
        reason: reason.to_owned(),
        inline_comments,
        summary_only_findings,
        observations,
        proof_requests,
    }
}

pub(crate) fn empty_follow_up_output_record(
    task: &FollowUpQuestionTask,
    model_lane: &str,
    result: &FollowUpResult,
) -> FollowUpOutputRecord {
    FollowUpOutputRecord {
        schema: FOLLOW_UP_OUTPUT_SCHEMA.to_owned(),
        task_id: task.id.clone(),
        group_id: task.group_id.clone(),
        stage: task.stage.clone(),
        disposition: task.disposition.clone(),
        evidence_need: task.evidence_need.clone(),
        candidate_ids: task.candidate_ids.clone(),
        observation_group_ids: task.observation_group_ids.clone(),
        model_lane: model_lane.to_owned(),
        status: result.status.clone(),
        reason: result.reason.clone(),
        inline_comments: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: Vec::new(),
        proof_requests: Vec::new(),
    }
}

pub(crate) fn follow_up_lane(task: &FollowUpQuestionTask, model_lane: &str) -> LanePlan {
    LanePlan {
        id: model_lane.to_owned(),
        role: "Orchestrator follow-up".to_owned(),
        model: "custom:MiniMax-M3-3".to_owned(),
        model_display: "MiniMax-M3".to_owned(),
        receives: vec![
            "orchestrator-plan".to_owned(),
            "routed-evidence".to_owned(),
            "follow-up-question".to_owned(),
        ],
        focus: task.question.clone(),
    }
}

pub(crate) fn follow_up_result(input: FollowUpResultInput<'_>) -> FollowUpResult {
    FollowUpResult {
        schema: FOLLOW_UP_RESULT_SCHEMA.to_owned(),
        task_id: input.task.id.clone(),
        group_id: input.task.group_id.clone(),
        stage: input.task.stage.clone(),
        disposition: input.task.disposition.clone(),
        evidence_need: input.task.evidence_need.clone(),
        candidate_ids: input.task.candidate_ids.clone(),
        observation_group_ids: input.task.observation_group_ids.clone(),
        packet_path: input.packet_path.to_owned(),
        model_lane: input.model_lane.to_owned(),
        status: input.status.to_owned(),
        reason: input.reason.to_owned(),
        provider: input.spec.provider.key().to_owned(),
        model: input.spec.model.clone(),
        endpoint_kind: input.spec.endpoint_kind.key().to_owned(),
        fallback_from: input.fallback_from,
        duration_ms: None,
        http_status: None,
        response_shape: None,
        cache_usage: ModelCacheUsage::default(),
        request_path: input.artifacts.request_path,
        response_path: input.artifacts.response_path,
        content_path: input.artifacts.content_path,
        normalized_content_path: input.artifacts.normalized_content_path,
        stderr_path: input.artifacts.stderr_path,
        output_counts: input.output_counts,
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PrObservationTone {
    Signal,
    Verification,
}
