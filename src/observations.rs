//! Observation artifact writers and lane model output parsing (cleanup
//! train step 14, pure code motion). write_observation_artifacts
//! serializes the per-lane observations, questions, and follow-up tasks
//! into the review packet; parse_lane_model_output_or_degrade degrades
//! contentful-but-malformed model output into a receipted observation
//! rather than dropping it.

use crate::*;

pub(crate) fn write_observation_artifacts(out: &Path, observations: &[Observation]) -> Result<()> {
    let observations_dir = out.join("observations");
    if observations_dir.exists() {
        fs::remove_dir_all(&observations_dir)
            .with_context(|| format!("remove {}", observations_dir.display()))?;
    }
    fs::create_dir_all(&observations_dir)
        .with_context(|| format!("create {}", observations_dir.display()))?;

    let questions_dir = out.join("questions");
    if questions_dir.exists() {
        fs::remove_dir_all(&questions_dir)
            .with_context(|| format!("remove {}", questions_dir.display()))?;
    }
    fs::create_dir_all(&questions_dir)
        .with_context(|| format!("create {}", questions_dir.display()))?;

    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("observations.json"),
        serde_json::to_vec_pretty(observations)?,
    )?;
    let observation_summary = observation_summary_artifacts(observations);
    fs::write(
        review_dir.join("unique_observations.json"),
        serde_json::to_vec_pretty(&observation_summary.unique)?,
    )?;
    fs::write(
        review_dir.join("merged_observations.json"),
        serde_json::to_vec_pretty(&observation_summary.merged)?,
    )?;
    fs::write(
        review_dir.join("dropped_observations.json"),
        serde_json::to_vec_pretty(&observation_summary.dropped)?,
    )?;

    let mut by_lane: BTreeMap<&str, Vec<&Observation>> = BTreeMap::new();
    let mut by_question: BTreeMap<(String, String), QuestionObservationArtifact<'_>> =
        BTreeMap::new();
    for observation in observations {
        by_lane
            .entry(observation.lane.as_str())
            .or_default()
            .push(observation);
        let lane_name = sanitize_artifact_name(&observation.lane);
        let question_name = sanitize_artifact_name(&observation.question);
        let artifact = by_question
            .entry((lane_name, question_name))
            .or_insert_with(|| QuestionObservationArtifact {
                schema: QUESTION_OBSERVATIONS_SCHEMA,
                lane: &observation.lane,
                question: &observation.question,
                observations: Vec::new(),
            });
        if artifact.lane != observation.lane || artifact.question != observation.question {
            bail!(
                "questions artifact path collision for {}/{}",
                observation.lane,
                observation.question
            );
        }
        artifact.observations.push(observation);
    }
    for (lane, lane_observations) in by_lane {
        let path = observations_dir.join(format!("{}.ndjson", sanitize_artifact_name(lane)));
        let mut text = String::new();
        for observation in lane_observations {
            text.push_str(&serde_json::to_string(observation)?);
            text.push('\n');
        }
        fs::write(path, text)?;
    }
    for ((lane_name, question_name), artifact) in by_question {
        let lane_dir = questions_dir.join(lane_name);
        fs::create_dir_all(&lane_dir).with_context(|| format!("create {}", lane_dir.display()))?;
        fs::write(
            lane_dir.join(format!("{question_name}.json")),
            serde_json::to_vec_pretty(&artifact)?,
        )?;
    }
    Ok(())
}

pub(crate) fn parse_lane_model_output_or_degrade(
    json_payload: &str,
    parse_path: &Path,
) -> Result<(LaneModelOutput, bool)> {
    match serde_json::from_str::<LaneModelOutput>(json_payload) {
        Ok(output) => {
            let degraded = output.degraded;
            if degraded || lane_model_output_has_value(&output) {
                Ok((output, degraded))
            } else if lane_model_json_payload_has_content(json_payload) {
                Ok((
                    degraded_lane_model_output(
                        json_payload,
                        "Output parsed as JSON but did not contain recognized lane evidence.",
                        parse_path,
                    ),
                    true,
                ))
            } else {
                Err(anyhow::anyhow!("lane model output was empty or unusable"))
                    .with_context(|| format!("parse {}", parse_path.display()))
            }
        }
        Err(err) if lane_model_raw_content_is_usable(json_payload) => Ok((
            degraded_lane_model_output(json_payload, &format!("Parse error: {err}"), parse_path),
            true,
        )),
        Err(err) => {
            Err(anyhow::Error::new(err)).with_context(|| format!("parse {}", parse_path.display()))
        }
    }
}

pub(crate) fn lane_model_output_has_value(output: &LaneModelOutput) -> bool {
    output
        .summary
        .as_deref()
        .is_some_and(|summary| !summary.trim().is_empty())
        || !output.inline_comments.is_empty()
        || !output.candidate_findings.is_empty()
        || !output.summary_only_findings.is_empty()
        || !output.observations.is_empty()
        || !output.failed_objections.is_empty()
        || !output.proof_requests.is_empty()
        || !output.proof_intents.is_empty()
}

pub(crate) fn lane_model_json_payload_has_content(json_payload: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json_payload)
        .ok()
        .is_some_and(|value| lane_model_json_value_has_content(&value))
}

pub(crate) fn lane_model_json_value_has_content(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => true,
        serde_json::Value::String(raw) => !raw.trim().is_empty(),
        serde_json::Value::Array(items) => items.iter().any(lane_model_json_value_has_content),
        serde_json::Value::Object(fields) => fields.values().any(lane_model_json_value_has_content),
    }
}

pub(crate) fn lane_model_raw_content_is_usable(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.chars().any(char::is_alphabetic)
}

pub(crate) fn degraded_lane_model_output(
    raw: &str,
    reason: &str,
    parse_path: &Path,
) -> LaneModelOutput {
    LaneModelOutput {
        summary: None,
        inline_comments: Vec::new(),
        candidate_findings: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: vec![lane_output_malformed_content_observation(
            raw, reason, parse_path,
        )],
        failed_objections: Vec::new(),
        proof_requests: Vec::new(),
        proof_intents: Vec::new(),
        issue_candidates: Vec::new(),
        degraded: true,
    }
}

pub(crate) fn lane_output_malformed_content_observation(
    raw: &str,
    reason: &str,
    parse_path: &Path,
) -> ModelCandidateObservation {
    let raw = truncate_chars(raw.trim(), 240);
    ModelCandidateObservation {
        claim: truncate_chars(
            &format!(
                "Lane output was contentful but not valid JSON; preserved degraded text: {raw}"
            ),
            320,
        ),
        question: Some("lane-output-shape".to_owned()),
        kind: Some("missing-evidence".to_owned()),
        status: Some("open".to_owned()),
        severity: Some("low".to_owned()),
        confidence: Some("medium".to_owned()),
        path: None,
        line: None,
        evidence: vec![
            reason.to_owned(),
            format!("Raw content artifact: {}", parse_path.display()),
        ],
        dedupe_key: Some("lane-output-malformed-content".to_owned()),
    }
}
