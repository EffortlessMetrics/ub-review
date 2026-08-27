//! Deterministic task-ledger persistence and replay verification (A2.3, #954).
//!
//! This module is deliberately disconnected from production writers until
//! #925 instruments real task sources. Version 1 rejects unknown fields:
//! additive wire changes require a new schema version instead of being
//! silently discarded by an older verifier.
#![cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "tracked in policy/allow.toml#task-ledger-artifact-shadow"
    )
)]

use std::collections::BTreeMap;
use std::path::{Component, Path};

use crate::artifacts::{TASK_LEDGER_EVENT_SCHEMA, TASK_LEDGER_SNAPSHOT_SCHEMA};
use crate::task_ledger::{
    TaskEvent, TaskId, TaskReceiptOutcome, TaskReducer, TaskSnapshot, TaskState,
};
use crate::{RevisionRef, sha256_hex};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

const EVENT_DIGEST_DOMAIN: &[u8] = b"ub-review.task-ledger-event.digest.v1";
const STREAM_DIGEST_DOMAIN: &[u8] = b"ub-review.task-ledger-stream.digest.v1";

/// One caller-ordered event before it is sealed into the append-only stream.
#[derive(Clone, Debug)]
pub(crate) struct TaskLedgerInput {
    pub(crate) task_id: TaskId,
    pub(crate) event: TaskEvent,
}

/// One versioned, chained NDJSON record.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TaskLedgerEventRecord {
    schema: String,
    sequence: u64,
    task_id: TaskId,
    revision: RevisionRef,
    previous_digest: Option<String>,
    event: TaskEvent,
    digest: String,
}

/// Digest source excludes the digest field itself.
#[derive(Serialize)]
struct TaskLedgerEventSource<'a> {
    schema: &'a str,
    sequence: u64,
    task_id: &'a TaskId,
    revision: &'a RevisionRef,
    previous_digest: &'a Option<String>,
    event: &'a TaskEvent,
}

/// Derived cache committed beside the event stream.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct TaskLedgerSnapshotArtifact {
    schema: String,
    revision: RevisionRef,
    event_count: u64,
    source_digest: String,
    tasks: Vec<TaskSnapshot>,
}

/// Canonical bytes returned by the future writer and consumed by verifiers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TaskLedgerArtifactBytes {
    pub(crate) events_ndjson: Vec<u8>,
    pub(crate) snapshot_json: Vec<u8>,
}

/// Seal caller order into canonical records, replay it, and derive the cache.
pub(crate) fn build_task_ledger_artifacts(
    revision: &RevisionRef,
    inputs: &[TaskLedgerInput],
) -> Result<TaskLedgerArtifactBytes> {
    revision
        .validate()
        .context("[missing_strong_binding] admitted revision is invalid")?;
    let events_ndjson = encode_event_records(revision, inputs)?;
    let snapshot = replay_event_stream(&events_ndjson, revision)?;
    let snapshot_json = canonical_json_line(&snapshot)?;
    Ok(TaskLedgerArtifactBytes {
        events_ndjson,
        snapshot_json,
    })
}

/// Independently replay and compare the derived cache byte-for-byte.
pub(crate) fn verify_task_ledger_artifacts(
    events_ndjson: &[u8],
    snapshot_json: &[u8],
    admitted_revision: &RevisionRef,
) -> Result<()> {
    admitted_revision
        .validate()
        .context("[missing_strong_binding] admitted revision is invalid")?;
    let recomputed = replay_event_stream(events_ndjson, admitted_revision)?;
    let committed: TaskLedgerSnapshotArtifact = serde_json::from_slice(snapshot_json)
        .context("[unsupported_schema] snapshot is not strict v1 JSON")?;
    ensure!(
        committed.schema == TASK_LEDGER_SNAPSHOT_SCHEMA,
        "[unsupported_schema] task-ledger snapshot schema is {}",
        committed.schema
    );
    ensure!(
        committed.revision == *admitted_revision,
        "[stale_revision] snapshot revision does not match the admitted revision"
    );
    ensure!(
        committed == recomputed,
        "[forged_snapshot] snapshot does not match deterministic event replay"
    );
    ensure!(
        snapshot_json == canonical_json_line(&recomputed)?,
        "[forged_snapshot] snapshot bytes are not canonical replay output"
    );
    Ok(())
}

/// Encode without replay so corruption tests can exercise the verifier.
fn encode_event_records(revision: &RevisionRef, inputs: &[TaskLedgerInput]) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut previous_digest = None;
    for (index, input) in inputs.iter().enumerate() {
        let sequence =
            u64::try_from(index).context("[event_order] task-ledger event count exceeds u64")?;
        let source = TaskLedgerEventSource {
            schema: TASK_LEDGER_EVENT_SCHEMA,
            sequence,
            task_id: &input.task_id,
            revision,
            previous_digest: &previous_digest,
            event: &input.event,
        };
        let digest = domain_digest(EVENT_DIGEST_DOMAIN, &serde_json::to_vec(&source)?);
        let record = TaskLedgerEventRecord {
            schema: TASK_LEDGER_EVENT_SCHEMA.to_owned(),
            sequence,
            task_id: input.task_id.clone(),
            revision: revision.clone(),
            previous_digest: previous_digest.clone(),
            event: input.event.clone(),
            digest: digest.clone(),
        };
        bytes.extend(canonical_json_line(&record)?);
        previous_digest = Some(digest);
    }
    Ok(bytes)
}

/// Parse strict v1 records, verify their chain, and replay every task.
fn replay_event_stream(
    bytes: &[u8],
    admitted_revision: &RevisionRef,
) -> Result<TaskLedgerSnapshotArtifact> {
    ensure!(
        !bytes.is_empty(),
        "[truncated_event_stream] task-ledger event stream is empty"
    );
    ensure!(
        bytes.ends_with(b"\n") && !bytes.contains(&b'\r'),
        "[truncated_event_stream] NDJSON must end each record with one LF"
    );

    let text =
        std::str::from_utf8(bytes).context("[truncated_event_stream] event stream is not UTF-8")?;
    let mut reducers: BTreeMap<String, TaskReducer> = BTreeMap::new();
    let mut previous_digest: Option<String> = None;
    let mut count = 0_u64;
    for (index, line) in text.split_terminator('\n').enumerate() {
        ensure!(
            !line.is_empty(),
            "[truncated_event_stream] event stream contains an empty record"
        );
        let raw: serde_json::Value = serde_json::from_str(line).with_context(|| {
            format!("[unsupported_schema] event record {index} is not strict v1 JSON")
        })?;
        validate_strict_revision_objects(&raw, index)?;
        let record: TaskLedgerEventRecord = serde_json::from_value(raw).with_context(|| {
            format!("[unsupported_schema] event record {index} is not strict v1 JSON")
        })?;
        let expected_sequence =
            u64::try_from(index).context("[event_order] task-ledger event count exceeds u64")?;
        ensure!(
            record.schema == TASK_LEDGER_EVENT_SCHEMA,
            "[unsupported_schema] task-ledger event schema is {}",
            record.schema
        );
        ensure!(
            record.sequence == expected_sequence,
            "[event_order] expected sequence {expected_sequence}, got {}",
            record.sequence
        );
        ensure!(
            record.revision == *admitted_revision,
            "[stale_revision] event {} does not match the admitted revision",
            record.sequence
        );
        ensure!(
            record.previous_digest == previous_digest,
            "[source_digest_mismatch] event {} breaks the digest chain",
            record.sequence
        );
        let source = TaskLedgerEventSource {
            schema: &record.schema,
            sequence: record.sequence,
            task_id: &record.task_id,
            revision: &record.revision,
            previous_digest: &record.previous_digest,
            event: &record.event,
        };
        let expected_digest = domain_digest(EVENT_DIGEST_DOMAIN, &serde_json::to_vec(&source)?);
        ensure!(
            record.digest == expected_digest,
            "[source_digest_mismatch] event {} digest is invalid",
            record.sequence
        );
        ensure!(
            line.as_bytes() == serde_json::to_vec(&record)?,
            "[source_digest_mismatch] event {} bytes are not canonical",
            record.sequence
        );
        validate_canonical_event_strings(&record.event)?;
        validate_receipt_references(&record.event)?;
        if let TaskEvent::Proposed { revision, .. } = &record.event {
            ensure!(
                revision == admitted_revision,
                "[stale_revision] proposal revision does not match the admitted revision"
            );
        }
        let normalized_id = TaskId::parse(record.task_id.as_str())?;
        ensure!(
            normalized_id == record.task_id,
            "[missing_strong_binding] task id is not canonical"
        );
        reducers
            .entry(record.task_id.as_str().to_owned())
            .or_default()
            .apply(&record.task_id, &record.event, admitted_revision)
            .with_context(|| {
                format!("event {} task {}", record.sequence, record.task_id.as_str())
            })?;
        previous_digest = Some(record.digest);
        count = count
            .checked_add(1)
            .context("[event_order] task-ledger event count overflow")?;
    }

    let mut tasks = Vec::with_capacity(reducers.len());
    for (id, reducer) in reducers {
        let snapshot = reducer
            .snapshot()
            .ok_or_else(|| anyhow::anyhow!("[missing_strong_binding] task {id} has no proposal"))?;
        match snapshot.state {
            TaskState::ResourcesReleased(_) | TaskState::TerminallyDeclined(_) => {}
            TaskState::ReleasePending(_) => {
                bail!("[unreleased_resources] task {id} reached a terminal outcome without release")
            }
            _ => bail!("[truncated_event_stream] task {id} is not terminal"),
        }
        validate_snapshot_receipt_references(snapshot)?;
        tasks.push(snapshot.clone());
    }

    Ok(TaskLedgerSnapshotArtifact {
        schema: TASK_LEDGER_SNAPSHOT_SCHEMA.to_owned(),
        revision: admitted_revision.clone(),
        event_count: count,
        source_digest: domain_digest(STREAM_DIGEST_DOMAIN, bytes),
        tasks,
    })
}

fn validate_canonical_event_strings(event: &TaskEvent) -> Result<()> {
    let value = match event {
        TaskEvent::ReceiptCreationFailed { reason, .. }
        | TaskEvent::TerminallyDeclined { reason, .. } => reason,
        _ => return Ok(()),
    };
    ensure!(
        !value.is_empty() && value.trim() == value,
        "[missing_strong_binding] event reason must be canonical and non-empty"
    );
    Ok(())
}

/// Keep v1 revision strictness local to this wire format. Other artifact
/// readers deliberately retain their additive-field compatibility.
fn validate_strict_revision_objects(value: &serde_json::Value, index: usize) -> Result<()> {
    let record = value.as_object().ok_or_else(|| {
        anyhow::anyhow!("[unsupported_schema] event record {index} must be an object")
    })?;
    validate_strict_revision_object(record.get("revision"), "event envelope")?;
    if let Some(proposal_revision) = record
        .get("event")
        .and_then(serde_json::Value::as_object)
        .and_then(|event| event.get("Proposed"))
        .and_then(serde_json::Value::as_object)
        .and_then(|proposal| proposal.get("revision"))
    {
        validate_strict_revision_object(Some(proposal_revision), "proposal")?;
    }
    Ok(())
}

fn validate_strict_revision_object(value: Option<&serde_json::Value>, label: &str) -> Result<()> {
    let object = value
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| {
            anyhow::anyhow!("[unsupported_schema] {label} revision must be an object")
        })?;
    ensure!(
        object.len() == 3
            && object.contains_key("digest")
            && object.contains_key("semantics")
            && object.contains_key("reviewed_commit"),
        "[unsupported_schema] {label} revision fields are not strict v1"
    );
    Ok(())
}

fn validate_receipt_references(event: &TaskEvent) -> Result<()> {
    match event {
        TaskEvent::ReceiptCreated { reference, .. } => validate_receipt_reference(reference),
        TaskEvent::TerminallyDeclined {
            existing_receipt: Some(reference),
            ..
        } => validate_receipt_reference(reference),
        _ => Ok(()),
    }
}

fn validate_snapshot_receipt_references(snapshot: &TaskSnapshot) -> Result<()> {
    if let Some(TaskReceiptOutcome::Created { reference }) = &snapshot.receipt {
        validate_receipt_reference(reference)?;
    }
    if let Some(reference) = &snapshot.existing_receipt {
        validate_receipt_reference(reference)?;
    }
    Ok(())
}

/// Shape-check only. A2.4 (#925) will join the reference to actual proof.
fn validate_receipt_reference(reference: &str) -> Result<()> {
    let trimmed = reference.trim();
    ensure!(
        trimmed == reference && !trimmed.is_empty(),
        "[invalid_receipt_reference] receipt reference must be canonical and non-empty"
    );
    ensure!(
        !trimmed.contains('\\')
            && !trimmed.contains(':')
            && !trimmed.starts_with('/')
            && !trimmed.chars().any(char::is_control),
        "[invalid_receipt_reference] receipt reference must be an artifact-relative POSIX path"
    );
    let mut parts = trimmed.split('#');
    let path = parts.next().unwrap_or_default();
    let fragment = parts.next();
    ensure!(
        parts.next().is_none() && fragment.is_none_or(|value| !value.is_empty()),
        "[invalid_receipt_reference] receipt reference has an invalid fragment"
    );
    let mut normalized = Vec::new();
    for component in Path::new(path).components() {
        match component {
            Component::Normal(name) if !name.eq_ignore_ascii_case(".git") => {
                normalized.push(name.to_string_lossy().into_owned());
            }
            _ => bail!(
                "[invalid_receipt_reference] receipt reference contains a forbidden component"
            ),
        }
    }
    ensure!(
        !normalized.is_empty() && normalized.join("/") == path,
        "[invalid_receipt_reference] receipt reference must not traverse or contain empty components"
    );
    Ok(())
}

fn canonical_json_line<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn domain_digest(domain: &[u8], payload: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(domain.len() + payload.len() + 1);
    bytes.extend_from_slice(domain);
    bytes.push(0);
    bytes.extend_from_slice(payload);
    sha256_hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_ledger::{
        MonotonicInstant, ResourceReservation, TaskConsumer, TaskExecutionLimits,
        TaskNonExecutionDisposition, TaskRequirement, TaskResourceClass, TaskSource,
        TaskTerminalDisposition, TaskValueClass,
    };
    use anyhow::ensure;

    fn revision(digit: char) -> RevisionRef {
        RevisionRef {
            digest: digit.to_string().repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: digit.to_string().repeat(40),
        }
    }

    fn input(id: &TaskId, event: TaskEvent) -> TaskLedgerInput {
        TaskLedgerInput {
            task_id: id.clone(),
            event,
        }
    }

    fn completed_inputs(revision: &RevisionRef, receipt: &str) -> Result<Vec<TaskLedgerInput>> {
        let id = TaskId::parse("proof/cargo-test")?;
        Ok(vec![
            input(
                &id,
                TaskEvent::Proposed {
                    revision: revision.clone(),
                    source: TaskSource::Required,
                    limits: TaskExecutionLimits::new(60_000)?,
                },
            ),
            input(
                &id,
                TaskEvent::ConsumerAttached {
                    consumer: TaskConsumer::parse(
                        "gate",
                        TaskRequirement::Required,
                        TaskValueClass::GateCritical,
                    )?,
                },
            ),
            input(&id, TaskEvent::Selected),
            input(
                &id,
                TaskEvent::Queued {
                    at: MonotonicInstant::from_millis(10),
                },
            ),
            input(
                &id,
                TaskEvent::Admitted {
                    at: MonotonicInstant::from_millis(20),
                    reservations: vec![ResourceReservation::new(TaskResourceClass::Cargo, 1)?],
                },
            ),
            input(
                &id,
                TaskEvent::SetupStarted {
                    at: MonotonicInstant::from_millis(21),
                },
            ),
            input(
                &id,
                TaskEvent::RunStarted {
                    at: MonotonicInstant::from_millis(25),
                },
            ),
            input(
                &id,
                TaskEvent::ProcessFinished {
                    at: MonotonicInstant::from_millis(40),
                    disposition: TaskTerminalDisposition::Succeeded,
                },
            ),
            input(
                &id,
                TaskEvent::CleanupFinished {
                    at: MonotonicInstant::from_millis(42),
                },
            ),
            input(
                &id,
                TaskEvent::ReceiptCreated {
                    at: MonotonicInstant::from_millis(43),
                    reference: receipt.to_owned(),
                },
            ),
            input(
                &id,
                TaskEvent::ResourcesReleased {
                    at: MonotonicInstant::from_millis(44),
                },
            ),
        ])
    }

    fn error_contains(result: Result<()>, expected: &str) -> Result<()> {
        let error = result
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected failure containing {expected}"))?;
        ensure!(
            format!("{error:#}").contains(expected),
            "unexpected error: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn golden_replay_is_byte_stable_and_snapshot_is_derived() -> Result<()> {
        let revision = revision('a');
        let inputs = completed_inputs(&revision, "review/proof_receipts.json#cargo-test")?;
        let first = build_task_ledger_artifacts(&revision, &inputs)?;
        let second = build_task_ledger_artifacts(&revision, &inputs)?;
        ensure!(first == second);
        verify_task_ledger_artifacts(&first.events_ndjson, &first.snapshot_json, &revision)?;
        ensure!(
            sha256_hex(&first.events_ndjson)
                == "b4fbc4688e3bcaa5740ba7bcbccb960aa3d1d6db4a03978bdcfb6ef1253e5427",
            "event golden changed: {}",
            sha256_hex(&first.events_ndjson)
        );
        ensure!(
            sha256_hex(&first.snapshot_json)
                == "d13b28c2dbccf7276e1f82d71eb0e77637839d70bc11a7161f7b9c92aeb231cb",
            "snapshot golden changed: {}",
            sha256_hex(&first.snapshot_json)
        );
        Ok(())
    }

    #[test]
    fn corruption_reordering_truncation_and_duplicate_terminal_fail_closed() -> Result<()> {
        let revision = revision('a');
        let inputs = completed_inputs(&revision, "receipts/cargo-test.json")?;
        let valid = build_task_ledger_artifacts(&revision, &inputs)?;

        let mut mutated = valid.events_ndjson.clone();
        let marker = b"\"digest\":\"";
        let position = mutated
            .windows(marker.len())
            .rposition(|window| window == marker)
            .map(|index| index + marker.len())
            .ok_or_else(|| anyhow::anyhow!("fixture contains no digest"))?;
        mutated[position] = if mutated[position] == b'a' {
            b'b'
        } else {
            b'a'
        };
        error_contains(
            verify_task_ledger_artifacts(&mutated, &valid.snapshot_json, &revision),
            "[source_digest_mismatch]",
        )?;

        let first_line_end = valid
            .events_ndjson
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| anyhow::anyhow!("fixture contains no NDJSON line"))?;
        let noncanonical_line = String::from_utf8(valid.events_ndjson[..first_line_end].to_vec())?
            .replacen("\"schema\":", "\"schema\" :", 1);
        let mut noncanonical = noncanonical_line.into_bytes();
        noncanonical.push(b'\n');
        noncanonical.extend_from_slice(&valid.events_ndjson[first_line_end + 1..]);
        error_contains(
            verify_task_ledger_artifacts(&noncanonical, &valid.snapshot_json, &revision),
            "[source_digest_mismatch]",
        )?;

        let lines: Vec<&[u8]> = valid
            .events_ndjson
            .split_inclusive(|byte| *byte == b'\n')
            .collect();
        let mut reordered = Vec::new();
        reordered.extend_from_slice(lines[1]);
        reordered.extend_from_slice(lines[0]);
        for line in &lines[2..] {
            reordered.extend_from_slice(line);
        }
        error_contains(
            verify_task_ledger_artifacts(&reordered, &valid.snapshot_json, &revision),
            "[event_order]",
        )?;

        let truncated_inputs = &inputs[..inputs.len() - 1];
        let truncated = encode_event_records(&revision, truncated_inputs)?;
        error_contains(
            verify_task_ledger_artifacts(&truncated, &valid.snapshot_json, &revision),
            "[unreleased_resources]",
        )?;

        let mut duplicate_inputs = inputs.clone();
        let id = TaskId::parse("proof/cargo-test")?;
        duplicate_inputs.push(input(
            &id,
            TaskEvent::ResourcesReleased {
                at: MonotonicInstant::from_millis(45),
            },
        ));
        let duplicate = encode_event_records(&revision, &duplicate_inputs)?;
        error_contains(
            verify_task_ledger_artifacts(&duplicate, &valid.snapshot_json, &revision),
            "[invalid_transition]",
        )?;
        Ok(())
    }

    #[test]
    fn stale_revision_forged_snapshot_and_unknown_fields_fail_closed() -> Result<()> {
        let admitted = revision('a');
        let foreign = revision('b');
        let inputs = completed_inputs(&foreign, "receipts/cargo-test.json")?;
        let stale = encode_event_records(&foreign, &inputs)?;
        let valid = build_task_ledger_artifacts(
            &admitted,
            &completed_inputs(&admitted, "receipts/cargo-test.json")?,
        )?;
        error_contains(
            verify_task_ledger_artifacts(&stale, &valid.snapshot_json, &admitted),
            "[stale_revision]",
        )?;

        let forged = String::from_utf8(valid.snapshot_json.clone())?
            .replace(
                "\"resources_released\":true",
                "\"resources_released\":false",
            )
            .into_bytes();
        error_contains(
            verify_task_ledger_artifacts(&valid.events_ndjson, &forged, &admitted),
            "[forged_snapshot]",
        )?;

        let first_line_end = valid
            .events_ndjson
            .iter()
            .position(|byte| *byte == b'\n')
            .ok_or_else(|| anyhow::anyhow!("fixture contains no NDJSON line"))?;
        let mut value: serde_json::Value =
            serde_json::from_slice(&valid.events_ndjson[..first_line_end])?;
        value
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("fixture line is not an object"))?
            .insert("future".to_owned(), serde_json::json!(true));
        let mut unknown = serde_json::to_vec(&value)?;
        unknown.push(b'\n');
        unknown.extend_from_slice(&valid.events_ndjson[first_line_end + 1..]);
        error_contains(
            verify_task_ledger_artifacts(&unknown, &valid.snapshot_json, &admitted),
            "[unsupported_schema]",
        )?;

        let mut nested_value: serde_json::Value =
            serde_json::from_slice(&valid.events_ndjson[..first_line_end])?;
        nested_value["event"]["Proposed"]
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("proposal fixture is not an object"))?
            .insert("future".to_owned(), serde_json::json!(true));
        let mut nested_unknown = serde_json::to_vec(&nested_value)?;
        nested_unknown.push(b'\n');
        nested_unknown.extend_from_slice(&valid.events_ndjson[first_line_end + 1..]);
        error_contains(
            verify_task_ledger_artifacts(&nested_unknown, &valid.snapshot_json, &admitted),
            "[unsupported_schema]",
        )?;

        let additive_revision: RevisionRef = serde_json::from_str(&format!(
            r#"{{"digest":"{}","semantics":"candidate_head","reviewed_commit":"{}","future":true}}"#,
            "a".repeat(64),
            "a".repeat(40)
        ))?;
        ensure!(additive_revision == admitted);
        Ok(())
    }

    #[test]
    fn receipt_refs_are_structural_only_and_consumers_remain_integral() -> Result<()> {
        let revision = revision('a');
        let nonexistent = "external/not-produced-here.json#receipt-7";
        let valid =
            build_task_ledger_artifacts(&revision, &completed_inputs(&revision, nonexistent)?)?;
        verify_task_ledger_artifacts(&valid.events_ndjson, &valid.snapshot_json, &revision)?;

        let invalid_inputs = completed_inputs(&revision, "../outside.json")?;
        let invalid = encode_event_records(&revision, &invalid_inputs)?;
        error_contains(
            verify_task_ledger_artifacts(&invalid, &valid.snapshot_json, &revision),
            "[invalid_receipt_reference]",
        )?;
        for hostile in [
            "C:/outside.json",
            ".git/config",
            "receipts/bad\nname.json",
            "receipts/bad\u{85}name.json",
        ] {
            let hostile_bytes =
                encode_event_records(&revision, &completed_inputs(&revision, hostile)?)?;
            error_contains(
                verify_task_ledger_artifacts(&hostile_bytes, &valid.snapshot_json, &revision),
                "[invalid_receipt_reference]",
            )?;
        }

        let id = TaskId::parse("declined")?;
        let conflicting = vec![
            input(
                &id,
                TaskEvent::Proposed {
                    revision: revision.clone(),
                    source: TaskSource::Worker,
                    limits: TaskExecutionLimits::new(1_000)?,
                },
            ),
            input(
                &id,
                TaskEvent::ConsumerAttached {
                    consumer: TaskConsumer::parse(
                        "reviewer",
                        TaskRequirement::Optional,
                        TaskValueClass::Advisory,
                    )?,
                },
            ),
            input(
                &id,
                TaskEvent::ConsumerAttached {
                    consumer: TaskConsumer::parse(
                        "reviewer",
                        TaskRequirement::Required,
                        TaskValueClass::GateCritical,
                    )?,
                },
            ),
            input(
                &id,
                TaskEvent::TerminallyDeclined {
                    at: MonotonicInstant::from_millis(1),
                    disposition: TaskNonExecutionDisposition::Refused,
                    reason: "policy".to_owned(),
                    existing_receipt: None,
                },
            ),
        ];
        let conflicting_bytes = encode_event_records(&revision, &conflicting)?;
        error_contains(
            verify_task_ledger_artifacts(&conflicting_bytes, &valid.snapshot_json, &revision),
            "[conflicting_consumer]",
        )?;

        let whitespace_reason = vec![
            input(
                &id,
                TaskEvent::Proposed {
                    revision: revision.clone(),
                    source: TaskSource::Worker,
                    limits: TaskExecutionLimits::new(1_000)?,
                },
            ),
            input(
                &id,
                TaskEvent::TerminallyDeclined {
                    at: MonotonicInstant::from_millis(1),
                    disposition: TaskNonExecutionDisposition::Refused,
                    reason: " policy ".to_owned(),
                    existing_receipt: None,
                },
            ),
        ];
        error_contains(
            build_task_ledger_artifacts(&revision, &whitespace_reason).map(|_| ()),
            "[missing_strong_binding]",
        )?;

        let nested_source = serde_json::from_str::<TaskEvent>(
            r#"{"Proposed":{"revision":{"digest":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","semantics":"candidate_head","reviewed_commit":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"source":{"ReviewerTurn":{"model_on":true,"future":true}},"limits":{"timeout_ceiling_ms":1000}}}"#,
        );
        ensure!(
            nested_source.is_err(),
            "nested task-source additions must fail v1"
        );
        Ok(())
    }
}
