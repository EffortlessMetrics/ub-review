//! Pure recorded #1335 composition; neither component becomes remote authority.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

use super::RecordedReviewExport;
use crate::delivery_transaction::recorded_observations::{
    ObservationInput, RecordedObservationExport, export_recorded_observations,
    replay_recorded_observations,
};
use crate::review_judgment::public_plan::payload::{
    RecordedPreparedPayload, validate_prepared_payload,
};
use crate::sha256_hex;

/// Component JSON remains original wire bytes, including source answer/plan
/// bytes and the independently named observation prefix. Consumers decode these
/// objects separately; a prepared-body binding cannot qualify summary delivery.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AgentReviewResult {
    schema: String,
    qualification: String,
    source_view_json: Vec<u8>,
    observation_view_json: Vec<u8>,
    prepared_payload_binding: serde_json::Value,
    accepted_intent: String,
    remote_authenticity: String,
    whole_review_delivery: String,
}

pub(super) fn export(
    input: ObservationInput<'_>,
    payload: &RecordedPreparedPayload<'_>,
    payload_source_pin: &str,
) -> Result<Vec<u8>> {
    ensure!(
        input.payload == payload.bytes && input.payload_sha256 == payload.payload_digest,
        "payload composition mismatch"
    );
    let binding = validate_prepared_payload(input.source, payload, payload_source_pin)?;
    let observed = export_recorded_observations(input)?;
    let view = AgentReviewResult {
        schema: "ub-review.recorded-agent-review-result.v1".into(),
        qualification: "shadow_recorded_only_pending_dependency_review".into(),
        source_view_json: observed.source.view_json,
        observation_view_json: observed.observation_json,
        prepared_payload_binding: serde_json::to_value(binding)?,
        accepted_intent: "not_admitted".into(),
        remote_authenticity: "unverified".into(),
        whole_review_delivery: "not_qualified".into(),
    };
    let mut bytes = serde_json::to_vec(&view)?;
    bytes.push(b'\n');
    ensure!(bytes.len() <= 16 * 1024 * 1024, "agent result byte limit");
    Ok(bytes)
}

pub(super) fn replay(
    input: ObservationInput<'_>,
    payload: &RecordedPreparedPayload<'_>,
    payload_source_pin: &str,
    recorded: &[u8],
    recorded_pin: &str,
) -> Result<()> {
    ensure!(
        recorded.len() <= 16 * 1024 * 1024 && sha256_hex(recorded) == recorded_pin,
        "agent result byte pin mismatch"
    );
    let view: AgentReviewResult = serde_json::from_slice(recorded)?;
    let prior = RecordedObservationExport {
        source: RecordedReviewExport {
            view_json: view.source_view_json,
            imported_ci_bytes: input
                .source
                .judgment_source
                .imported_ci
                .as_ref()
                .map(|ci| ci.bytes.to_vec()),
        },
        observation_json: view.observation_view_json,
    };
    replay_recorded_observations(
        ObservationInput {
            source: input.source,
            ..input
        },
        &prior,
        &sha256_hex(&prior.observation_json),
    )?;
    ensure!(
        export(input, payload, payload_source_pin)? == recorded,
        "agent result replay mismatch"
    );
    Ok(())
}
