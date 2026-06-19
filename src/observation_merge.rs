//! Observation summary, merge, and status metrics (cleanup train
//! step 45, pure code motion).

use crate::*;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ObservationGroup {
    pub(crate) schema: String,
    pub(crate) id: String,
    pub(crate) dedupe_key: String,
    pub(crate) claim: String,
    pub(crate) kind: String,
    pub(crate) status: String,
    pub(crate) severity: String,
    pub(crate) confidence: String,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<u32>,
    pub(crate) evidence: Vec<String>,
    pub(crate) lanes: Vec<String>,
    pub(crate) sources: Vec<String>,
    pub(crate) observation_ids: Vec<String>,
    pub(crate) duplicate_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct MergedObservationRecord {
    pub(crate) schema: String,
    pub(crate) group_id: String,
    pub(crate) dedupe_key: String,
    pub(crate) kept_observation_id: String,
    pub(crate) merged_observation_ids: Vec<String>,
    pub(crate) lanes: Vec<String>,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DroppedObservationRecord {
    pub(crate) schema: String,
    pub(crate) observation_id: String,
    pub(crate) group_id: String,
    pub(crate) dedupe_key: String,
    pub(crate) lane: String,
    pub(crate) reason: String,
}

pub(crate) struct ObservationSummaryArtifacts {
    pub(crate) unique: Vec<ObservationGroup>,
    pub(crate) merged: Vec<MergedObservationRecord>,
    pub(crate) dropped: Vec<DroppedObservationRecord>,
}

pub(crate) fn observation_summary_artifacts(
    observations: &[Observation],
) -> ObservationSummaryArtifacts {
    let mut indexes = BTreeMap::new();
    let mut groups = Vec::<ObservationGroup>::new();
    for observation in observations {
        let key = observation_group_key(observation);
        if let Some(index) = indexes.get(&key).copied() {
            merge_review_observation(&mut groups[index], observation);
        } else {
            let group_id = observation_group_id(groups.len(), &key);
            indexes.insert(key.clone(), groups.len());
            groups.push(ObservationGroup {
                schema: OBSERVATION_GROUP_SCHEMA.to_owned(),
                id: group_id,
                dedupe_key: key,
                claim: observation.claim.clone(),
                kind: observation.kind.clone(),
                status: observation.status.clone(),
                severity: observation.severity.clone(),
                confidence: observation.confidence.clone(),
                path: observation.path.clone(),
                line: observation.line,
                evidence: observation.evidence.iter().take(3).cloned().collect(),
                lanes: vec![observation.lane.clone()],
                sources: vec![observation.source.clone()],
                observation_ids: vec![observation.id.clone()],
                duplicate_count: 0,
            });
        }
    }
    let merged = groups
        .iter()
        .filter(|group| group.observation_ids.len() > 1)
        .map(|group| MergedObservationRecord {
            schema: MERGED_OBSERVATION_SCHEMA.to_owned(),
            group_id: group.id.clone(),
            dedupe_key: group.dedupe_key.clone(),
            kept_observation_id: group.observation_ids[0].clone(),
            merged_observation_ids: group.observation_ids[1..].to_vec(),
            lanes: group.lanes.clone(),
            reason: "merged_duplicate_dedupe_key".to_owned(),
        })
        .collect::<Vec<_>>();
    let observation_lanes = observations
        .iter()
        .map(|observation| (observation.id.as_str(), observation.lane.as_str()))
        .collect::<BTreeMap<_, _>>();
    let dropped = groups
        .iter()
        .flat_map(|group| {
            group
                .observation_ids
                .iter()
                .skip(1)
                .map(|observation_id| DroppedObservationRecord {
                    schema: DROPPED_OBSERVATION_SCHEMA.to_owned(),
                    observation_id: observation_id.clone(),
                    group_id: group.id.clone(),
                    dedupe_key: group.dedupe_key.clone(),
                    lane: observation_lanes
                        .get(observation_id.as_str())
                        .copied()
                        .unwrap_or("unknown")
                        .to_owned(),
                    reason: "merged_into_unique_observation".to_owned(),
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    ObservationSummaryArtifacts {
        unique: groups,
        merged,
        dropped,
    }
}

pub(crate) fn unique_review_observations(observations: &[Observation]) -> Vec<ObservationGroup> {
    observation_summary_artifacts(observations).unique
}

pub(crate) fn observation_group_key(observation: &Observation) -> String {
    if observation.dedupe_key.trim().is_empty() {
        observation.fingerprint.clone()
    } else {
        observation.dedupe_key.clone()
    }
}

pub(crate) fn observation_group_id(index: usize, dedupe_key: &str) -> String {
    let digest = sha256_hex(dedupe_key.as_bytes());
    format!("obsgrp-{index:04}-{}", &digest[..12])
}

pub(crate) fn merge_review_observation(group: &mut ObservationGroup, observation: &Observation) {
    if severity_rank(&observation.severity) > severity_rank(&group.severity) {
        group.severity = observation.severity.clone();
    }
    if confidence_rank(&observation.confidence) > confidence_rank(&group.confidence) {
        group.confidence = observation.confidence.clone();
    }
    if observation_status_rank(&observation.status) > observation_status_rank(&group.status) {
        group.status = observation.status.clone();
    }
    if group.path.is_none() {
        group.path = observation.path.clone();
    }
    if group.line.is_none() {
        group.line = observation.line;
    }
    if !group.lanes.contains(&observation.lane) {
        group.lanes.push(observation.lane.clone());
    }
    if !group.sources.contains(&observation.source) {
        group.sources.push(observation.source.clone());
    }
    group.observation_ids.push(observation.id.clone());
    group.duplicate_count = group.observation_ids.len().saturating_sub(1);
    for evidence in &observation.evidence {
        if group.evidence.len() >= 3 {
            break;
        }
        if !group.evidence.contains(evidence) {
            group.evidence.push(evidence.clone());
        }
    }
}

pub(crate) fn observation_status_rank(value: &str) -> u8 {
    match value {
        "refuted" => 7,
        "confirmed" => 6,
        "parked" => 5,
        "demoted" => 4,
        "open" => 3,
        "covered" => 2,
        "duplicate" => 1,
        _ => 0,
    }
}

pub(crate) fn sensor_status_for_metrics(out: &Path, sensor: &SensorPlan) -> String {
    let status_path = out
        .join("sensors")
        .join(&sensor.id)
        .join("ub-review-sensor-status.json");
    read_sensor_receipt(&status_path)
        .map(|receipt| receipt.status)
        .unwrap_or_else(|| {
            if sensor.run {
                "receipt-absent".to_owned()
            } else {
                "skipped".to_owned()
            }
        })
}

pub(crate) fn status_counts<'a>(
    statuses: impl Iterator<Item = &'a str>,
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for status in statuses {
        *counts.entry(status.to_owned()).or_insert(0) += 1;
    }
    counts
}

pub(crate) fn model_call_attempted_status(status: &str) -> bool {
    matches!(
        status,
        "ok" | "failed"
            | "degraded"
            | "invalid_json"
            | "timed_out"
            | "rate_limited"
            | "auth_failed"
            | "bad_envelope"
    )
}
