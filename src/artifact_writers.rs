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
    revision: Option<&crate::RevisionRef>,
) -> Result<()> {
    let mut records = model_stage_records(model_lanes, follow_up_results, args);
    // A1.3 (#950): stamp every stage row with the packet's immutable revision.
    for record in &mut records {
        record.revision = revision.cloned();
    }
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

pub(crate) fn write_compiler_reconciliation_artifact(
    out: &Path,
    receipt: &CompilerReconciliationReceipt,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("compiler_reconciliation.json"),
        serde_json::to_vec_pretty(receipt)?,
    )?;
    Ok(())
}

pub(crate) fn write_output_degradation_artifact(
    out: &Path,
    receipt: &ReviewOutputDegradationReceipt,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("output_degradation.json"),
        serde_json::to_vec_pretty(receipt)?,
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
        revision: None,
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

#[cfg(test)]
mod output_degradation_artifact_tests {
    use super::*;

    #[test]
    fn output_degradation_artifact_round_trips_exact_head_and_receipt_fields() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let receipt = ReviewOutputDegradationReceipt {
            schema: OUTPUT_DEGRADATION_SCHEMA,
            exact_head_sha: "head-artifact".to_owned(),
            original_bytes: 100,
            final_bytes: 80,
            original_item_count: 3,
            final_item_count: 2,
            selected_mode: "concise_summary".to_owned(),
            retained_topic_ids: vec!["topic-a".to_owned(), "topic-b".to_owned()],
            dropped_topics: vec![ReviewOutputDroppedTopic {
                topic_id: "topic-c".to_owned(),
                reason: "lower_evidence_value_or_bullet_budget".to_owned(),
            }],
            max_bytes: 6_000,
            max_bullets: 12,
        };

        write_output_degradation_artifact(temp.path(), &receipt)?;
        let artifact_path = temp.path().join("review/output_degradation.json");
        assert!(artifact_path.is_file());
        let artifact_text = fs::read_to_string(&artifact_path)?;
        assert!(artifact_text.contains("\"schema\": \"ub-review.output_degradation.v1\""));
        let written: serde_json::Value = serde_json::from_str(&artifact_text)?;

        assert_eq!(written["schema"], "ub-review.output_degradation.v1");
        assert_eq!(written["exact_head_sha"], "head-artifact");
        assert_eq!(written["selected_mode"], "concise_summary");
        assert_eq!(
            written["retained_topic_ids"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(written["dropped_topics"].as_array().map(Vec::len), Some(1));
        Ok(())
    }
}
