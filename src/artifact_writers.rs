//! Final artifact writers: orchestrator plan, follow-up results/outputs,
//! resolved candidates, observation predicates, and model stage records
//! (cleanup train step 41, pure code motion).

use crate::*;

pub(crate) fn write_final_orchestrator_artifact(
    out: &Path,
    plan: &OrchestratorPlanArtifact,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("final_orchestrator_plan.json"),
        serde_json::to_vec_pretty(plan)?,
    )?;
    Ok(())
}

pub(crate) fn write_follow_up_result_artifacts(
    out: &Path,
    results: &[FollowUpResult],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("follow_up_results.json"),
        serde_json::to_vec_pretty(results)?,
    )?;
    let mut ndjson = String::new();
    for result in results {
        ndjson.push_str(&serde_json::to_string(result)?);
        ndjson.push('\n');
    }
    fs::write(out.join("follow_up_results.ndjson"), ndjson)?;
    Ok(())
}

pub(crate) fn write_follow_up_output_artifacts(
    out: &Path,
    outputs: &[FollowUpOutputRecord],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("follow_up_outputs.json"),
        serde_json::to_vec_pretty(outputs)?,
    )?;
    let mut ndjson = String::new();
    for output in outputs {
        ndjson.push_str(&serde_json::to_string(output)?);
        ndjson.push('\n');
    }
    fs::write(out.join("follow_up_outputs.ndjson"), ndjson)?;
    Ok(())
}

pub(crate) fn write_resolved_candidate_artifacts(
    out: &Path,
    records: &[ResolvedCandidateRecord],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("resolved_candidates.json"),
        serde_json::to_vec_pretty(records)?,
    )?;
    let mut ndjson = String::new();
    for record in records {
        ndjson.push_str(&serde_json::to_string(record)?);
        ndjson.push('\n');
    }
    fs::write(out.join("resolved_candidates.ndjson"), ndjson)?;
    Ok(())
}

/// Issue-candidate kinds the capture surface accepts (release lane step 4;
/// security/release/deploy/compliance classes are deliberately absent - they
/// stay suggest-only by contract even after the broker exists).
pub(crate) fn observation_is_refuted(observation: &Observation) -> bool {
    observation.status == "refuted"
}

pub(crate) fn observation_is_covered(observation: &Observation) -> bool {
    observation.status == "covered"
}

pub(crate) fn observation_is_parked(observation: &Observation) -> bool {
    observation.status == "parked"
}

pub(crate) fn write_model_stage_artifacts(
    out: &Path,
    model_lanes: &[ModelLaneReceipt],
    follow_up_results: &[FollowUpResult],
    args: &RunArgs,
) -> Result<()> {
    let records = model_stage_records(model_lanes, follow_up_results, args);
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("model_stages.json"),
        serde_json::to_vec_pretty(&records)?,
    )?;
    let mut ndjson = String::new();
    for record in &records {
        ndjson.push_str(&serde_json::to_string(record)?);
        ndjson.push('\n');
    }
    fs::write(out.join("model_stages.ndjson"), ndjson)?;
    Ok(())
}

pub(crate) fn write_final_compiler_input_artifact(
    out: &Path,
    artifact: FinalCompilerInputArtifact<'_>,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("final_compiler_input.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    Ok(())
}

pub(crate) fn model_stage_records(
    model_lanes: &[ModelLaneReceipt],
    follow_up_results: &[FollowUpResult],
    _args: &RunArgs,
) -> Vec<ModelStageRecord> {
    let mut records = model_lanes
        .iter()
        .map(model_lane_stage_record)
        .collect::<Vec<_>>();
    records.extend(follow_up_results.iter().map(follow_up_stage_record));
    records
}

pub(crate) fn model_lane_stage_record(receipt: &ModelLaneReceipt) -> ModelStageRecord {
    let (source, stage, stage_reason) = model_lane_stage_metadata(&receipt.lane);
    ModelStageRecord {
        schema: MODEL_STAGE_SCHEMA.to_owned(),
        lane: receipt.lane.clone(),
        source: source.to_owned(),
        stage: stage.to_owned(),
        stage_reason: stage_reason.to_owned(),
        status: receipt.status.clone(),
        reason: receipt.reason.clone(),
        provider: receipt.provider.clone(),
        model: receipt.model.clone(),
        endpoint_kind: receipt.endpoint_kind.clone(),
        task_id: None,
        group_id: None,
        packet_path: None,
        duration_ms: receipt.duration_ms,
        http_status: receipt.http_status,
        response_shape: receipt.response_shape.clone(),
        cache_usage: receipt.cache_usage.clone(),
    }
}

pub(crate) fn model_lane_stage_metadata(lane: &str) -> (&'static str, &'static str, &'static str) {
    match lane {
        "proof-planner" => (
            "proof-planner",
            "primary",
            "proof-planner scopes local proof from the shared packet and early lane evidence",
        ),
        "refuter" => (
            "refuter",
            "tertiary",
            "refuter classifies primary candidates before the final compiler pass",
        ),
        _ => (
            "model-lane",
            "primary",
            "initial cached lane turn over the shared PR packet",
        ),
    }
}
