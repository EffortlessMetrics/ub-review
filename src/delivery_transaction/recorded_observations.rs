//! Pure #959 recorded-observation contract for a later #1335 consumer.
//! Test-compiled until that owner admits the stream and its independent pins.
//! No transport, persistence, clock, publication permission or finalizer lives here.

use super::*;
use crate::review_judgment::JudgmentSubject;
use crate::review_judgment::public_plan::recorded_export::{
    RecordedReviewExport, export_recorded_review,
};
use crate::review_judgment::public_plan::{
    DeliverySubject, PublicDisposition, PublicItem, PublicPlanInput,
};
use crate::{RevisionRef, sha256_hex};
use serde::de::DeserializeOwned;

pub(crate) const STREAM_SCHEMA: &str = "ub-review.recorded-delivery-observations.v1";
pub(crate) const WATERMARK_SCHEMA: &str = "ub-review.recorded-delivery-watermark.v1";
const BYTE_LIMIT: usize = 1024 * 1024;
const OBJECT_LIMIT: usize = 256 * 1024;
const ROW_LIMIT: usize = 128;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservationSubject {
    pub(crate) judgment_subject: JudgmentSubject,
    pub(crate) delivery_subject: DeliverySubject,
    pub(crate) revision: RevisionRef,
    pub(crate) source_manifest_digest: String,
    pub(crate) public_source_digest: String,
    pub(crate) judgment_digest: String,
    pub(crate) plan_digest: String,
    pub(crate) payload_sha256: String,
}

/// Event bytes retain original receipt/event formatting. The independent named
/// watermark commits to an ordered prefix of these bytes, not reserialization.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservationStream {
    pub(crate) schema: String,
    pub(crate) subject: ObservationSubject,
    pub(crate) events: Vec<Vec<u8>>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservationWatermark {
    pub(crate) schema: String,
    pub(crate) name: String,
    pub(crate) count: usize,
    pub(crate) chain_sha256: String,
    pub(crate) observed_head_sha: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReceiptBytes {
    pub(crate) sha256: String,
    pub(crate) bytes: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum RecordedObservation {
    Prepared {
        retry_of: Option<String>,
    },
    /// Local failure is not evidence that the remote side did nothing.
    Ambiguous {
        stage: DeliveryFailureStage,
        local_receipt: ReceiptBytes,
    },
    /// Complete inline subset only; never confirms summary or whole review.
    ConfirmedInline {
        observed: Vec<ObservedDelivery>,
        reconciliation: ReceiptBytes,
    },
    /// An independently retained observation, not a live head lookup.
    Superseded {
        observed_head_sha: String,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ObservationEvent {
    pub(crate) sequence: usize,
    pub(crate) previous_sha256: String,
    pub(crate) attempt_id: String,
    pub(crate) observation: RecordedObservation,
}

pub(crate) struct ObservationInput<'a> {
    pub(crate) source: PublicPlanInput<'a>,
    pub(crate) payload: &'a [u8],
    pub(crate) payload_sha256: &'a str,
    pub(crate) stream: &'a [u8],
    pub(crate) stream_sha256: &'a str,
    pub(crate) watermark: &'a [u8],
    pub(crate) watermark_sha256: &'a str,
    pub(crate) watermark_name: &'a str,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct RecordedObservationExport {
    pub(crate) source: RecordedReviewExport,
    pub(crate) observation_json: Vec<u8>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
enum RemoteObservation {
    Unknown,
    ConfirmedInline {
        reconciliation: DeliveryReconciliation,
        receipt_sha256: String,
    },
}

#[derive(Debug, Serialize)]
struct LocalFailure {
    stage: DeliveryFailureStage,
    receipt_sha256: String,
}

#[derive(Debug, Serialize)]
struct AttemptView {
    attempt_id: String,
    retry_of: Option<String>,
    local_failure: Option<LocalFailure>,
    remote: RemoteObservation,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Payload {
    event: String,
    body: String,
    comments: Vec<PayloadComment>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PayloadComment {
    path: String,
    line: u32,
    side: String,
    body: String,
}

/// Canonical delivery codecs predate strict unknown-field admission. Compare
/// their typed roundtrip here so an ignored nested field cannot become authority.
fn strict<T: DeserializeOwned + Serialize>(bytes: &[u8]) -> Result<T> {
    let typed: T = serde_json::from_slice(bytes)?;
    ensure!(
        serde_json::to_value(&typed)? == serde_json::from_slice::<serde_json::Value>(bytes)?,
        "unknown or noncanonical recorded field"
    );
    Ok(typed)
}

fn digest(bytes: &[u8], expected: &str, limit: usize) -> Result<()> {
    ensure!(
        !bytes.is_empty() && bytes.len() <= limit,
        "recorded byte limit"
    );
    ensure!(
        expected.len() == 64
            && expected
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid SHA256 pin"
    );
    ensure!(sha256_hex(bytes) == expected, "recorded byte pin mismatch");
    Ok(())
}

fn id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"_-.:".contains(&b)),
        "invalid recorded identity"
    );
    Ok(())
}

fn head(value: &str) -> Result<()> {
    ensure!(
        value.len() == 40
            && value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid recorded head"
    );
    Ok(())
}

pub(crate) fn genesis(subject: &ObservationSubject) -> Result<String> {
    let mut bytes = b"ub-review.recorded-delivery-genesis.v1\0".to_vec();
    bytes.extend(serde_json::to_vec(subject)?);
    Ok(sha256_hex(&bytes))
}

pub(crate) fn extend_chain(previous: &str, event: &[u8]) -> String {
    let mut bytes = b"ub-review.recorded-delivery-event.v1\0".to_vec();
    bytes.extend(previous.as_bytes());
    bytes.extend((event.len() as u64).to_be_bytes());
    bytes.extend(event);
    sha256_hex(&bytes)
}

fn payload_matches_inline(payload: &[u8], planned: &[PlannedDelivery]) -> Result<()> {
    let payload: Payload = serde_json::from_slice(payload)?;
    ensure!(
        payload.event == "COMMENT"
            && payload.body.len() <= 6000
            && payload.comments.len() <= ROW_LIMIT,
        "unsupported recorded payload"
    );
    ensure!(
        payload.comments.len() == planned.len(),
        "payload inline count mismatch"
    );
    let mut matched = BTreeSet::new();
    for comment in payload.comments {
        ensure!(comment.body.len() <= 6000, "payload body limit");
        let item = planned
            .iter()
            .find(|p| p.location() == (comment.path.as_str(), comment.line, comment.side.as_str()))
            .ok_or_else(|| anyhow::anyhow!("payload inline location mismatch"))?;
        ensure!(
            sha256_hex(comment.body.as_bytes()) == item.expected_body_digest(),
            "payload inline body mismatch"
        );
        ensure!(
            matched.insert(DeliveryIdentity::from(item)),
            "duplicate payload inline"
        );
    }
    Ok(())
}

/// Revalidate canonical sources first. Consume exactly the requested pinned
/// prefix; a later suffix cannot rewrite that historical view. Caller admission
/// authenticates observations and pins; internal consistency is not remote proof.
pub(crate) fn export_recorded_observations(
    input: ObservationInput<'_>,
) -> Result<RecordedObservationExport> {
    digest(input.payload, input.payload_sha256, OBJECT_LIMIT)?;
    digest(input.stream, input.stream_sha256, BYTE_LIMIT)?;
    digest(input.watermark, input.watermark_sha256, 4096)?;
    let subject = ObservationSubject {
        judgment_subject: input.source.judgment_source.subject.clone(),
        delivery_subject: input.source.public_source.delivery_subject.clone(),
        revision: input.source.current_revision.revision_ref(),
        source_manifest_digest: input.source.expected_source_digest.to_owned(),
        public_source_digest: input.source.expected_public_source_digest.to_owned(),
        judgment_digest: input
            .source
            .expected_answer_digest
            .ok_or_else(|| anyhow::anyhow!("observation requires a lead"))?
            .to_owned(),
        plan_digest: input
            .source
            .expected_plan_digest
            .ok_or_else(|| anyhow::anyhow!("observation requires a plan"))?
            .to_owned(),
        payload_sha256: input.payload_sha256.to_owned(),
    };
    let source = export_recorded_review(input.source)?;
    let source_view: serde_json::Value = serde_json::from_slice(&source.view_json)?;
    ensure!(
        source_view["public_plan"]["availability"] == "validated",
        "observation source plan unavailable"
    );
    let items: Vec<PublicItem> =
        serde_json::from_value(source_view["public_plan"]["plan"]["items"].clone())?;
    let planned = items
        .iter()
        .filter_map(|item| match &item.disposition {
            PublicDisposition::Inline { delivery, .. } => Some(delivery.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    payload_matches_inline(input.payload, &planned)?;
    let stream: ObservationStream = strict(input.stream)?;
    ensure!(
        stream.schema == STREAM_SCHEMA && stream.subject == subject,
        "observation source binding mismatch"
    );
    ensure!(
        stream.events.len() <= ROW_LIMIT
            && stream
                .events
                .iter()
                .all(|e| !e.is_empty() && e.len() <= OBJECT_LIMIT),
        "observation event limit"
    );
    let watermark: ObservationWatermark = strict(input.watermark)?;
    id(&watermark.name)?;
    ensure!(
        watermark.schema == WATERMARK_SCHEMA && watermark.name == input.watermark_name,
        "wrong named watermark"
    );
    ensure!(
        watermark.count > 0 && watermark.count <= stream.events.len(),
        "missing watermark prefix"
    );
    let mut chain = genesis(&subject)?;
    let mut current_head = subject.delivery_subject.head_sha.clone();
    let mut historical = false;
    let mut attempts: Vec<AttemptView> = Vec::new();
    let mut physical = BTreeMap::<String, DeliveryReceipt>::new();
    for (index, bytes) in stream.events.iter().take(watermark.count).enumerate() {
        let event: ObservationEvent = strict(bytes)?;
        ensure!(
            event.sequence == index + 1 && event.previous_sha256 == chain,
            "observation chain/order mismatch"
        );
        id(&event.attempt_id)?;
        let found = attempts
            .iter()
            .position(|a| a.attempt_id == event.attempt_id);
        match event.observation {
            RecordedObservation::Prepared { retry_of } => {
                ensure!(
                    !historical && found.is_none(),
                    "duplicate or historical prepared attempt"
                );
                match &retry_of {
                    None => ensure!(attempts.is_empty(), "retry requires predecessor"),
                    Some(previous) => {
                        let prior = attempts
                            .iter()
                            .find(|a| &a.attempt_id == previous)
                            .ok_or_else(|| anyhow::anyhow!("unknown retry predecessor"))?;
                        ensure!(
                            prior.local_failure.is_some()
                                || matches!(
                                    prior.remote,
                                    RemoteObservation::ConfirmedInline { .. }
                                ),
                            "retry predecessor still prepared"
                        );
                    }
                }
                attempts.push(AttemptView {
                    attempt_id: event.attempt_id,
                    retry_of,
                    local_failure: None,
                    remote: RemoteObservation::Unknown,
                });
            }
            RecordedObservation::Ambiguous {
                stage,
                local_receipt,
            } => {
                let attempt = attempts
                    .get_mut(found.ok_or_else(|| anyhow::anyhow!("unprepared observation"))?)
                    .ok_or_else(|| anyhow::anyhow!("missing attempt"))?;
                ensure!(
                    attempt.local_failure.is_none()
                        && matches!(attempt.remote, RemoteObservation::Unknown),
                    "conflicting local observation"
                );
                digest(&local_receipt.bytes, &local_receipt.sha256, OBJECT_LIMIT)?;
                let receipt: DeliveryTransaction = strict(&local_receipt.bytes)?;
                ensure!(
                    receipt.exact_head_sha == subject.delivery_subject.head_sha
                        && receipt.planned
                            == canonical_planned_deliveries(
                                &subject.delivery_subject.head_sha,
                                planned.clone()
                            )?
                        && matches!(
                            receipt.state,
                            DeliveryTransactionState::Failed | DeliveryTransactionState::CleanedUp
                        )
                        && receipt
                            .failure
                            .as_ref()
                            .is_some_and(|failure| failure.stage == stage),
                    "local transaction binding mismatch"
                );
                attempt.local_failure = Some(LocalFailure {
                    stage,
                    receipt_sha256: local_receipt.sha256,
                });
            }
            RecordedObservation::ConfirmedInline {
                observed,
                reconciliation,
            } => {
                let attempt = attempts
                    .get_mut(found.ok_or_else(|| anyhow::anyhow!("unprepared observation"))?)
                    .ok_or_else(|| anyhow::anyhow!("missing attempt"))?;
                ensure!(
                    matches!(attempt.remote, RemoteObservation::Unknown)
                        && !planned.is_empty()
                        && observed.len() <= ROW_LIMIT,
                    "conflicting or empty confirmation"
                );
                digest(&reconciliation.bytes, &reconciliation.sha256, OBJECT_LIMIT)?;
                let receipt: DeliveryReconciliation = strict(&reconciliation.bytes)?;
                let derived = reconcile_deliveries(
                    &subject.delivery_subject.head_sha,
                    &receipt.review_id,
                    &planned,
                    &observed,
                )?;
                ensure!(receipt == derived, "recorded reconciliation mismatch");
                ensure!(
                    receipt
                        .review_id
                        .parse::<u64>()
                        .is_ok_and(|n| n > 0 && n.to_string() == receipt.review_id),
                    "invalid recorded review id"
                );
                for item in &receipt.receipts {
                    ensure!(
                        item.comment_id
                            .parse::<u64>()
                            .is_ok_and(|n| n > 0 && n.to_string() == item.comment_id),
                        "invalid recorded comment id"
                    );
                    ensure!(
                        physical
                            .get(&item.comment_id)
                            .is_none_or(|previous| previous == item),
                        "conflicting physical receipt"
                    );
                    physical.insert(item.comment_id.clone(), item.clone());
                }
                attempt.remote = RemoteObservation::ConfirmedInline {
                    reconciliation: receipt,
                    receipt_sha256: reconciliation.sha256,
                };
            }
            RecordedObservation::Superseded { observed_head_sha } => {
                ensure!(found.is_some(), "unprepared head observation");
                head(&observed_head_sha)?;
                ensure!(
                    observed_head_sha != subject.delivery_subject.head_sha
                        && observed_head_sha != current_head,
                    "invalid head advance"
                );
                current_head = observed_head_sha;
                historical = true;
            }
        }
        chain = extend_chain(&chain, bytes);
    }
    ensure!(
        watermark.chain_sha256 == chain && watermark.observed_head_sha == current_head,
        "watermark prefix mismatch"
    );
    let view = serde_json::json!({
        "schema":"ub-review.recorded-delivery-observation-view.v1",
        "qualification":"shadow_recorded_only", "source_view_sha256":sha256_hex(&source.view_json),
        "subject":subject, "watermark":watermark, "attempts":attempts,
        "source_currentness":if historical { "historical_at_recorded_watermark" } else { "at_recorded_source_head" },
        "summary_payload_correspondence":if items.iter().any(|i| matches!(i.disposition, PublicDisposition::Summary { .. })) { "unverified" } else { "not_applicable" },
        "whole_review_delivery":"not_qualified"
    });
    let mut observation_json = serde_json::to_vec(&view)?;
    observation_json.push(b'\n');
    ensure!(
        observation_json.len() <= BYTE_LIMIT,
        "observation view byte limit"
    );
    Ok(RecordedObservationExport {
        source,
        observation_json,
    })
}

pub(crate) fn replay_recorded_observations(
    input: ObservationInput<'_>,
    recorded: &RecordedObservationExport,
    expected_view_sha256: &str,
) -> Result<()> {
    digest(&recorded.observation_json, expected_view_sha256, BYTE_LIMIT)?;
    let derived = export_recorded_observations(input)?;
    ensure!(derived == *recorded, "observation replay mismatch");
    Ok(())
}

#[cfg(test)]
mod tests;
