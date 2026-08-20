// ub-review: reporter module — the same-model coordinator (Order 9 of #678).
//! Live reporter coordination (Order 9 of #678) — the product center.
//!
//! The same-model coordinator that runs after the primary investigation wave,
//! reads what each lane concluded, and makes one same-model distillation call
//! (same cohort provider/model, same cached shared prefix) to identify the
//! most important findings, flag contradictions/gaps, and propose a verdict
//! direction.
//!
//! This is single-turn (turn 0) in this slice: it proves the same-model
//! distillation loop end-to-end. Multi-turn continuation (reporter asks → lane
//! answers → reporter refines) is the natural extension built on the
//! persistent threads (#692) and message queue (#694).
//!
//! The reporter's output is advisory: it feeds the compiler and the message
//! queue; it does not itself post or gate (Orders 10/11).

use std::{fs, path::Path};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::artifacts::REPORTER_THREAD_SCHEMA;

/// A lane's conclusion as the reporter sees it — a compact digest built from
/// the lane's ModelLaneReceipt.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct LaneDigest {
    pub(crate) lane: String,
    pub(crate) status: String,
    pub(crate) conclusion: String,
    pub(crate) thread_id: String,
    /// Private calibration context loaded from the lane model artifact. This
    /// is available to the reporter for contradiction/gap analysis but is
    /// never copied into public review sinks.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) internal_audit: Option<crate::InternalAudit>,
}

/// Digest of a late-phase sensor receipt (#325 stream-as-it-lands). Late
/// sensors finish while the lanes are already investigating; their receipts
/// are joined before the reporter runs and routed here so the reporter can
/// weigh the late deterministic evidence and carry it into lane continuation
/// turns.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct LateSensorDigest {
    pub(crate) sensor: String,
    pub(crate) status: String,
    pub(crate) reason: String,
    pub(crate) receipt_path: String,
}

impl LateSensorDigest {
    /// One-line excerpt for routing into a lane continuation prompt.
    pub(crate) fn excerpt(&self) -> String {
        format!(
            "late sensor `{}` status=`{}` reason=`{}` (receipt: {})",
            self.sensor, self.status, self.reason, self.receipt_path
        )
    }
}

/// Read the late-phase sensors' receipts into reporter-routable digests. A
/// receipt that is absent or unreadable is reported as `receipt-absent` —
/// missing evidence, never clean evidence.
pub(crate) fn late_sensor_receipt_digests(
    out: &Path,
    sensor_ids: &[String],
) -> Vec<LateSensorDigest> {
    sensor_ids
        .iter()
        .map(|id| {
            let receipt_path = format!("sensors/{id}/ub-review-sensor-status.json");
            let receipt = crate::read_sensor_receipt(&out.join(&receipt_path));
            let (status, reason) = receipt
                .map(|receipt| (receipt.status, receipt.reason))
                .unwrap_or_else(|| {
                    (
                        "receipt-absent".to_owned(),
                        "late sensor produced no receipt; treat as missing evidence".to_owned(),
                    )
                });
            LateSensorDigest {
                sensor: id.clone(),
                status,
                reason,
                receipt_path,
            }
        })
        .collect()
}

/// The reporter's verdict on the PR (Order 11 of #678). Only meaningful when
/// `[gate].review_forward = true`; otherwise it is advisory and never feeds
/// the gate.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReporterVerdict {
    /// The reporter finds the PR safe to merge.
    Clear,
    /// The reporter requests changes before merge.
    ChangesRequested,
    /// The reporter cannot determine whether the PR is safe (insufficient
    /// evidence, conflicting lanes, etc.).
    Uncertain,
    /// No verdict was produced (model mode off, reporter skipped, or the model
    /// did not return a verdict).
    #[default]
    None,
}

/// The reporter's distilled conclusion, parsed from its model response.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ReporterConclusion {
    pub(crate) schema: String,
    /// The reporter's free-form distillation text (what it would say to a
    /// human reviewer about the PR).
    pub(crate) distillation: String,
    /// Follow-up questions the reporter proposes (emitted as
    /// reporter_question messages). Empty if none.
    pub(crate) proposed_follow_ups: Vec<String>,
    pub(crate) cohort_id: String,
    pub(crate) thread_id: String,
    /// The reporter's structured verdict (Order 11). Only affects the gate
    /// when `[gate].review_forward = true`.
    #[serde(default)]
    pub(crate) verdict: ReporterVerdict,
}

/// Build the reporter's prompt: the shared cached prefix is provided
/// separately (as the cacheable prefix to the model call); this returns the
/// reporter *suffix* — the compact digest of what each lane concluded plus the
/// reporter's task instruction.
pub(crate) fn reporter_prompt(
    digests: &[LaneDigest],
    late_evidence: &[LateSensorDigest],
) -> String {
    let mut prompt = String::new();
    prompt.push_str("# Reporter Coordination Task\n\n");
    prompt.push_str(
        "You are the same-model reporter coordinating this review. Below are the \
         conclusions of each specialist investigation lane. Your job:\n\n",
    );
    prompt.push_str("1. Identify the most important findings worth surfacing.\n");
    prompt.push_str("2. Flag contradictions or gaps between lanes.\n");
    prompt.push_str("3. Propose a verdict direction (clear / changes_requested / uncertain).\n");
    prompt.push_str("4. List any targeted follow-up questions for named lanes.\n\n");
    prompt.push_str("## Lane conclusions\n\n");
    if digests.is_empty() {
        prompt.push_str("- No lanes reported (model mode off or all skipped).\n");
    }
    for d in digests {
        let conclusion = if d.conclusion.is_empty() {
            "(no detail)"
        } else {
            d.conclusion.as_str()
        };
        prompt.push_str(&format!(
            "### `{}` (status: `{}`)\n{}\n\n",
            d.lane, d.status, conclusion
        ));
        if let Some(audit) = &d.internal_audit {
            prompt.push_str(
                "Private internal audit (use only to check coverage and contradictions; ",
            );
            prompt.push_str("do not quote or emit this artifact-only context):\n");
            prompt.push_str(&format!(
                "- surfaces checked: {}\n",
                audit.surfaces_checked.join(", ")
            ));
            if let Some(hypothesis) = &audit.strongest_rejected_hypothesis {
                prompt.push_str(&format!("- rejected hypothesis: {hypothesis}\n"));
            }
            if let Some(uncertainty) = &audit.remaining_local_uncertainty {
                prompt.push_str(&format!("- remaining uncertainty: {uncertainty}\n"));
            }
            prompt.push('\n');
        }
    }
    // #325 stream-as-it-lands: late-phase deterministic evidence landed after
    // the lanes launched, so the lanes have not seen it. The reporter weighs
    // it here and can route it to lanes via follow-up questions.
    if !late_evidence.is_empty() {
        prompt.push_str("## Late deterministic evidence\n\n");
        prompt.push_str(
            "These sensor receipts landed after the lanes launched (the lanes \
             reviewed on the fast-sensor precontext and have not seen them). \
             Weigh them in your distillation; if one contradicts or confirms a \
             lane's conclusion, ask that lane a follow-up question naming the \
             receipt.\n\n",
        );
        for digest in late_evidence {
            prompt.push_str(&format!(
                "- `{}`: `{}` - {} (receipt: `{}`)\n",
                digest.sensor, digest.status, digest.reason, digest.receipt_path
            ));
        }
        prompt.push('\n');
    }
    prompt.push_str(
        "## Output\n\nReturn a JSON object: {\"distillation\": \"...\", \
         \"verdict\": \"clear\"|\"changes_requested\"|\"uncertain\", \
         \"proposed_follow_ups\": [\"question for lane X\", ...]}. The distillation \
         is what you would tell a human reviewer in 2-4 sentences.\n",
    );
    prompt
}

/// Build a lane digest from the executed receipts (only lanes with a
/// non-empty thread_id — i.e., that actually investigated).
pub(crate) fn lane_digests_from_receipts(
    review_dir: &Path,
    receipts: &[crate::ModelLaneReceipt],
) -> Vec<LaneDigest> {
    receipts
        .iter()
        .filter(|r| !r.thread_id.is_empty())
        .map(|r| LaneDigest {
            lane: r.lane.clone(),
            status: r.status.clone(),
            conclusion: r.reason.clone(),
            thread_id: r.thread_id.clone(),
            internal_audit: read_internal_audit(review_dir, &r.lane),
        })
        .collect()
}

pub(crate) fn read_internal_audit(review_dir: &Path, lane: &str) -> Option<crate::InternalAudit> {
    let path = crate::internal_audit_artifact_path(&review_dir.join("model"), lane).ok()?;
    let bytes = fs::read(path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    if value.get("schema")?.as_str()? != crate::artifacts::INTERNAL_AUDIT_SCHEMA
        || value.get("lane")?.as_str()? != lane
    {
        return None;
    }
    let audit: crate::InternalAudit = serde_json::from_value(value).ok()?;
    audit.has_value().then_some(audit)
}

/// Remove private audit material from every public reporter sink, including
/// adversarial model responses that ignore the prompt's no-echo contract.
pub(crate) fn withhold_internal_audit_echo(
    mut conclusion: ReporterConclusion,
    review_dir: &Path,
) -> ReporterConclusion {
    let model_dir = review_dir.join("model");
    let mut private_values = Vec::new();
    if let Ok(entries) = fs::read_dir(model_dir) {
        for entry in entries.flatten() {
            let path = entry.path().join("internal_audit.json");
            let Ok(bytes) = fs::read(path) else { continue };
            let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
                continue;
            };
            for key in [
                "surfaces_checked",
                "strongest_rejected_hypothesis",
                "remaining_local_uncertainty",
            ] {
                match value.get(key) {
                    Some(serde_json::Value::Array(values)) => private_values
                        .extend(values.iter().filter_map(|v| v.as_str()).map(str::to_owned)),
                    Some(serde_json::Value::String(value)) => private_values.push(value.clone()),
                    _ => {}
                }
            }
        }
    }
    for private in private_values
        .into_iter()
        .filter(|value| !value.trim().is_empty())
    {
        conclusion.distillation = conclusion
            .distillation
            .replace(&private, "[private internal audit withheld]");
        conclusion.proposed_follow_ups = conclusion
            .proposed_follow_ups
            .into_iter()
            .map(|follow_up| follow_up.replace(&private, "[private internal audit withheld]"))
            .collect();
    }
    conclusion
}

/// Parse the reporter's model response into a ReporterConclusion. Tolerant of
/// non-JSON responses (uses the raw text as the distillation).
pub(crate) fn parse_reporter_conclusion(
    content: &str,
    cohort_id: &str,
    thread_id: &str,
) -> ReporterConclusion {
    // Try to parse as JSON {distillation, proposed_follow_ups}.
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
        let distillation = parsed
            .get("distillation")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_owned();
        let proposed_follow_ups = parsed
            .get("proposed_follow_ups")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        return ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation,
            proposed_follow_ups,
            verdict: parse_verdict(&parsed),
            cohort_id: cohort_id.to_owned(),
            thread_id: thread_id.to_owned(),
        };
    }
    // Fallback: the raw response text is the distillation.
    ReporterConclusion {
        schema: REPORTER_THREAD_SCHEMA.to_owned(),
        distillation: content.to_owned(),
        proposed_follow_ups: Vec::new(),
        verdict: ReporterVerdict::None,
        cohort_id: cohort_id.to_owned(),
        thread_id: thread_id.to_owned(),
    }
}

/// Parse a reporter response that is about to replace an already-committed
/// turn. Re-distillation must be strict: once follow-up evidence exists, a
/// malformed replacement cannot leave the earlier conclusion authoritative.
pub(crate) fn parse_reporter_conclusion_strict(
    content: &str,
    cohort_id: &str,
    thread_id: &str,
) -> Result<ReporterConclusion> {
    let parsed: serde_json::Value =
        serde_json::from_str(content).context("parse reporter re-distillation JSON")?;
    let distillation = parsed
        .get("distillation")
        .and_then(|value| value.as_str())
        .context("reporter re-distillation is missing string `distillation`")?
        .to_owned();
    let verdict = parsed
        .get("verdict")
        .and_then(|value| value.as_str())
        .context("reporter re-distillation is missing string `verdict`")
        .and_then(verdict_from_wire)?;
    let proposed_follow_ups = match parsed.get("proposed_follow_ups") {
        None => Vec::new(),
        Some(value) => value
            .as_array()
            .context("reporter re-distillation `proposed_follow_ups` is not an array")?
            .iter()
            .map(|item| {
                item.as_str()
                    .map(str::to_owned)
                    .context("reporter re-distillation follow-up is not a string")
            })
            .collect::<Result<Vec<_>>>()?,
    };
    Ok(ReporterConclusion {
        schema: REPORTER_THREAD_SCHEMA.to_owned(),
        distillation,
        proposed_follow_ups,
        verdict,
        cohort_id: cohort_id.to_owned(),
        thread_id: thread_id.to_owned(),
    })
}

/// Parse the reporter's verdict from a JSON value. Recognizes the
/// snake_case strings from the prompt: "clear", "changes_requested",
/// "uncertain". Falls back to None for missing or unrecognized values.
fn parse_verdict(value: &serde_json::Value) -> ReporterVerdict {
    match value
        .get("verdict")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "clear" => ReporterVerdict::Clear,
        "changes_requested" => ReporterVerdict::ChangesRequested,
        "uncertain" => ReporterVerdict::Uncertain,
        _ => ReporterVerdict::None,
    }
}

/// One validated reporter turn selected as the public/gate authority for the
/// current PR head.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedReporterTurn {
    pub(crate) turn: u32,
    pub(crate) receipt_ref: String,
    pub(crate) head_sha: String,
    pub(crate) thread_id: String,
    pub(crate) cohort_id: String,
    pub(crate) distillation: String,
    pub(crate) verdict: ReporterVerdict,
}

/// Result of resolving the reporter authority for a run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ReporterTurnResolution {
    /// Latest eligible turn is structurally valid and bound to the current
    /// invocation head.
    Current(ResolvedReporterTurn),
    /// No reporter turn artifact exists.
    Absent,
    /// Latest turn exists but is bound to a different head.
    StaleHead {
        expected: String,
        found: String,
        receipt: String,
    },
    /// Latest turn path exists but could not be parsed.
    Malformed { receipt: String, error: String },
}

/// Gate-facing reporter evidence derived from one resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum ReporterGateInput {
    /// Reporter did not run / no turn on disk.
    #[default]
    Absent,
    /// Structured verdict from a current-head turn.
    Verdict {
        verdict: ReporterVerdict,
        receipt: String,
    },
    /// Turn exists but is not usable as a deciding artifact.
    Unusable {
        kind: String,
        detail: String,
        receipt: String,
    },
}

impl ReporterTurnResolution {
    /// Public distillation for the review body. Only a current-head turn may
    /// surface; stale/malformed evidence stays artifact-only.
    pub(crate) fn public_distillation(&self) -> Option<&str> {
        match self {
            Self::Current(turn) if !turn.distillation.is_empty() => {
                Some(turn.distillation.as_str())
            }
            _ => None,
        }
    }

    /// Gate evidence for `[gate].review_forward`. Stale/malformed remain
    /// explicit instead of collapsing into ordinary absence.
    pub(crate) fn gate_input(&self) -> ReporterGateInput {
        match self {
            Self::Absent => ReporterGateInput::Absent,
            Self::Current(turn) => ReporterGateInput::Verdict {
                verdict: turn.verdict.clone(),
                receipt: turn.receipt_ref.clone(),
            },
            Self::StaleHead {
                expected,
                found,
                receipt,
            } => ReporterGateInput::Unusable {
                kind: "reporter-stale-head".to_owned(),
                detail: format!(
                    "latest reporter turn is bound to head `{found}`, not current head `{expected}`"
                ),
                receipt: receipt.clone(),
            },
            Self::Malformed { receipt, error } => ReporterGateInput::Unusable {
                kind: "reporter-malformed".to_owned(),
                detail: format!("latest reporter turn is unreadable: {error}"),
                receipt: receipt.clone(),
            },
        }
    }
}

/// Serialize a structured verdict for the turn artifact.
pub(crate) fn verdict_to_wire(verdict: &ReporterVerdict) -> String {
    match verdict {
        ReporterVerdict::Clear => "clear".to_owned(),
        ReporterVerdict::ChangesRequested => "changes_requested".to_owned(),
        ReporterVerdict::Uncertain => "uncertain".to_owned(),
        ReporterVerdict::None => "none".to_owned(),
    }
}

/// Parse a wire verdict string (snake_case).
fn verdict_from_wire(raw: &str) -> Result<ReporterVerdict> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "clear" => Ok(ReporterVerdict::Clear),
        "changes_requested" => Ok(ReporterVerdict::ChangesRequested),
        "uncertain" => Ok(ReporterVerdict::Uncertain),
        "none" | "" => Ok(ReporterVerdict::None),
        unknown => anyhow::bail!("unknown reporter verdict `{unknown}`"),
    }
}

/// Start a new reporter invocation without allowing a prior invocation on the
/// same checkout and head to retain authority. Prompt/model payloads remain;
/// only the reporter decision turns and their derived rollup are replaced.
pub(crate) fn prepare_reporter_run(review_dir: &Path) -> Result<()> {
    let thread_dir = review_dir.join("threads").join("reporter");
    invalidate_reporter_authority(review_dir)
        .context("invalidate prior reporter authority at invocation start")?;
    let entries = match std::fs::read_dir(&thread_dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err).context("read reporter thread directory"),
    };
    for entry in entries {
        let entry = entry.context("read reporter thread entry")?;
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("turn-") && name.ends_with(".json") {
            std::fs::remove_file(entry.path())
                .with_context(|| format!("remove prior reporter artifact `{name}`"))?;
        }
    }
    Ok(())
}

/// Remove the rollup that grants reporter authority. Call this before any
/// fallible operation that attempts to replace an already-committed turn.
pub(crate) fn invalidate_reporter_authority(review_dir: &Path) -> Result<()> {
    let rollup_path = review_dir.join("threads/reporter/thread.json");
    match std::fs::remove_file(&rollup_path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err).context("remove prior reporter authority rollup"),
    }
    Ok(())
}

/// Write a reporter turn (turn N) with durable structured verdict + head bind.
pub(crate) fn write_reporter_turn(
    review_dir: &Path,
    conclusion: &ReporterConclusion,
    turn: u32,
    head_sha: &str,
    terminal_reason: &str,
    extra_routed_refs: &[String],
) -> Result<()> {
    // The rollup is the authority selector. Invalidate it before writing a
    // replacement turn so any turn or rollup write failure is fail-closed;
    // an older reporter decision cannot remain authoritative after a failed
    // re-distillation commit.
    invalidate_reporter_authority(review_dir)
        .context("invalidate reporter authority before turn write")?;
    let receipt_ref = format!("review/threads/reporter/turn-{turn:03}.json");
    let mut routed_evidence_refs: Vec<String> = conclusion
        .proposed_follow_ups
        .iter()
        .map(|q| format!("follow-up: {q}"))
        .collect();
    routed_evidence_refs.extend(extra_routed_refs.iter().cloned());
    let turn_record = crate::LaneThreadTurn {
        schema: REPORTER_THREAD_SCHEMA.to_owned(),
        thread_id: conclusion.thread_id.clone(),
        turn,
        stage: "reporter".to_owned(),
        prompt_packet_path: "review/threads/reporter/prompt.md".to_owned(),
        response_summary: conclusion.distillation.clone(),
        routed_evidence_refs,
        receipt_ref: receipt_ref.clone(),
        head_sha: Some(head_sha.to_owned()),
        verdict: Some(verdict_to_wire(&conclusion.verdict)),
    };
    crate::write_lane_thread_turn(
        review_dir,
        "reporter",
        &turn_record,
        &conclusion.cohort_id,
        terminal_reason,
    )?;
    Ok(())
}

/// Write the reporter's first-pass conclusion as turn-000.
pub(crate) fn write_reporter_thread(
    review_dir: &Path,
    conclusion: &ReporterConclusion,
    head_sha: &str,
) -> Result<()> {
    write_reporter_turn(
        review_dir,
        conclusion,
        0,
        head_sha,
        "reporter_completed",
        &[],
    )
}

/// Select the highest-numbered reporter turn and bind it to `current_head`.
pub(crate) fn resolve_reporter_turn(
    review_dir: &Path,
    current_head: &str,
) -> ReporterTurnResolution {
    let thread_dir = review_dir.join("threads").join("reporter");
    let rollup_path = thread_dir.join("thread.json");
    let rollup_bytes = match std::fs::read(&rollup_path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return ReporterTurnResolution::Absent;
        }
        Err(err) => {
            return ReporterTurnResolution::Malformed {
                receipt: "review/threads/reporter/thread.json".to_owned(),
                error: err.to_string(),
            };
        }
    };
    let rollup: crate::LaneThreadSession = match serde_json::from_slice(&rollup_bytes) {
        Ok(rollup) => rollup,
        Err(err) => {
            return ReporterTurnResolution::Malformed {
                receipt: "review/threads/reporter/thread.json".to_owned(),
                error: err.to_string(),
            };
        }
    };
    let Some(latest_id) = rollup.latest_turn.clone().or_else(|| {
        // Compatibility for legacy rollups: choose by numeric turn identity,
        // never lexicographic filename order.
        std::fs::read_dir(&thread_dir).ok().and_then(|entries| {
            entries
                .filter_map(|entry| entry.ok())
                .filter_map(|entry| {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let value = name
                        .strip_prefix("turn-")?
                        .strip_suffix(".json")?
                        .parse::<u32>()
                        .ok()?;
                    Some((value, format!("turn-{value:03}")))
                })
                .max_by_key(|(value, _)| *value)
                .map(|(_, id)| id)
        })
    }) else {
        return ReporterTurnResolution::Absent;
    };
    let receipt_ref = format!("review/threads/reporter/{latest_id}.json");
    if let Some(latest_ref) = rollup.latest_turn_ref.as_deref()
        && latest_ref != receipt_ref
    {
        return ReporterTurnResolution::Malformed {
            receipt: "review/threads/reporter/thread.json".to_owned(),
            error: format!(
                "rollup latest_turn `{latest_id}` disagrees with latest_turn_ref `{latest_ref}`"
            ),
        };
    }
    let turn_path = thread_dir.join(format!("{latest_id}.json"));
    let bytes = match std::fs::read(&turn_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return ReporterTurnResolution::Malformed {
                receipt: receipt_ref,
                error: err.to_string(),
            };
        }
    };
    let turn: crate::LaneThreadTurn = match serde_json::from_slice(&bytes) {
        Ok(turn) => turn,
        Err(err) => {
            return ReporterTurnResolution::Malformed {
                receipt: receipt_ref,
                error: err.to_string(),
            };
        }
    };
    let mut payload_mismatches = Vec::new();
    if rollup.schema != crate::artifacts::LANE_THREAD_SCHEMA {
        payload_mismatches.push("rollup.schema");
    }
    if rollup.lane != "reporter" {
        payload_mismatches.push("rollup.lane");
    }
    if turn.schema != REPORTER_THREAD_SCHEMA {
        payload_mismatches.push("turn.schema");
    }
    if turn.stage != "reporter" {
        payload_mismatches.push("turn.stage");
    }
    if turn.receipt_ref != receipt_ref {
        payload_mismatches.push("turn.receipt_ref");
    }
    if turn.response_summary != rollup.latest_conclusion {
        payload_mismatches.push("turn.response_summary/rollup.latest_conclusion");
    }
    if !payload_mismatches.is_empty() {
        return ReporterTurnResolution::Malformed {
            receipt: receipt_ref,
            error: format!(
                "selected reporter turn payload mismatches: {}",
                payload_mismatches.join(", ")
            ),
        };
    }
    let Some(found_head) = turn
        .head_sha
        .as_deref()
        .filter(|head| !head.is_empty())
        .map(str::to_owned)
    else {
        return ReporterTurnResolution::Malformed {
            receipt: receipt_ref,
            error: "selected reporter turn is not bound to an invocation head".to_owned(),
        };
    };
    if found_head != current_head {
        return ReporterTurnResolution::StaleHead {
            expected: current_head.to_owned(),
            found: found_head,
            receipt: receipt_ref,
        };
    }
    let mut identity_mismatches = Vec::new();
    if turn.turn
        != latest_id
            .strip_prefix("turn-")
            .and_then(|value| value.parse().ok())
            .unwrap_or(u32::MAX)
    {
        identity_mismatches.push("turn.turn/rollup.latest_turn");
    }
    if turn.thread_id != rollup.thread_id {
        identity_mismatches.push("turn.thread_id/rollup.thread_id");
    }
    if turn.head_sha != rollup.head_sha {
        identity_mismatches.push("turn.head_sha/rollup.head_sha");
    }
    if turn.verdict != rollup.verdict {
        identity_mismatches.push("turn.verdict/rollup.verdict");
    }
    if !identity_mismatches.is_empty() {
        return ReporterTurnResolution::Malformed {
            receipt: receipt_ref,
            error: format!(
                "selected reporter turn identity mismatches: {}",
                identity_mismatches.join(", ")
            ),
        };
    }
    let verdict = match turn.verdict.as_deref().map(verdict_from_wire).transpose() {
        Ok(verdict) => verdict.unwrap_or(ReporterVerdict::None),
        Err(err) => {
            return ReporterTurnResolution::Malformed {
                receipt: receipt_ref,
                error: err.to_string(),
            };
        }
    };
    ReporterTurnResolution::Current(ResolvedReporterTurn {
        turn: turn.turn,
        receipt_ref,
        head_sha: found_head,
        thread_id: turn.thread_id,
        cohort_id: rollup.cohort_id,
        distillation: turn.response_summary,
        verdict,
    })
}

/// Resolve reporter authority after the production orchestration attempt.
/// A coordination failure is authoritative in memory: disk state is never
/// consulted because startup invalidation itself may have failed.
pub(crate) fn resolve_reporter_after_coordination(
    review_dir: &Path,
    current_head: &str,
    coordination_error: Option<&str>,
) -> ReporterTurnResolution {
    if let Some(error) = coordination_error {
        return ReporterTurnResolution::Malformed {
            receipt: "review/threads/reporter/thread.json".to_owned(),
            error: format!("reporter coordination failed before authority commit: {error}"),
        };
    }
    resolve_reporter_turn(review_dir, current_head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reporter_prompt_lists_lanes_and_includes_task() {
        let digests = vec![
            LaneDigest {
                lane: "tests-oracle".to_owned(),
                status: "ok".to_owned(),
                conclusion: "test discriminates the patch".to_owned(),
                thread_id: "tid1".to_owned(),
                internal_audit: None,
            },
            LaneDigest {
                lane: "opposition".to_owned(),
                status: "degraded".to_owned(),
                conclusion: "strongest objection: missing error path".to_owned(),
                thread_id: "tid2".to_owned(),
                internal_audit: None,
            },
        ];
        let prompt = reporter_prompt(&digests, &[]);
        assert!(prompt.contains("Reporter Coordination Task"));
        assert!(prompt.contains("tests-oracle"));
        assert!(prompt.contains("discriminates"));
        assert!(prompt.contains("opposition"));
        assert!(prompt.contains("missing error path"));
        assert!(prompt.contains("proposed_follow_ups"));
        assert!(
            !prompt.contains("Late deterministic evidence"),
            "no late-evidence section without late receipts"
        );
    }

    #[test]
    fn reporter_prompt_handles_no_lanes() {
        let prompt = reporter_prompt(&[], &[]);
        assert!(prompt.contains("No lanes reported"));
    }

    #[test]
    fn reporter_prompt_routes_late_sensor_evidence() {
        let digests = vec![LaneDigest {
            lane: "tests-oracle".to_owned(),
            status: "ok".to_owned(),
            conclusion: "coverage gap suspected".to_owned(),
            thread_id: "tid1".to_owned(),
            internal_audit: None,
        }];
        let late = vec![LateSensorDigest {
            sensor: "cargo-test".to_owned(),
            status: "failed".to_owned(),
            reason: "exit code Some(101)".to_owned(),
            receipt_path: "sensors/cargo-test/ub-review-sensor-status.json".to_owned(),
        }];
        let prompt = reporter_prompt(&digests, &late);
        assert!(prompt.contains("Late deterministic evidence"));
        assert!(prompt.contains("`cargo-test`: `failed`"));
        assert!(prompt.contains("sensors/cargo-test/ub-review-sensor-status.json"));
        assert!(prompt.contains("landed after the lanes launched"));
    }

    #[test]
    fn late_sensor_digest_reads_receipt_and_reports_absence_as_missing() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        std::fs::create_dir_all(out.join("sensors/cargo-test"))?;
        std::fs::write(
            out.join("sensors/cargo-test/ub-review-sensor-status.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "sensor": "cargo-test",
                "status": "ok",
                "reason": "completed",
            }))?,
        )?;
        let digests =
            late_sensor_receipt_digests(out, &["cargo-test".to_owned(), "coverage".to_owned()]);
        assert_eq!(digests.len(), 2);
        assert_eq!(digests[0].sensor, "cargo-test");
        assert_eq!(digests[0].status, "ok");
        assert_eq!(digests[1].sensor, "coverage");
        assert_eq!(digests[1].status, "receipt-absent");
        assert!(digests[1].reason.contains("missing evidence"));
        Ok(())
    }

    #[test]
    fn parse_reporter_conclusion_json() {
        let content = r#"{"distillation": "PR is safe; one minor test gap.", "proposed_follow_ups": ["tests-oracle: confirm edge case"]}"#;
        let c = parse_reporter_conclusion(content, "cid", "tid");
        assert_eq!(c.distillation, "PR is safe; one minor test gap.");
        assert_eq!(
            c.proposed_follow_ups,
            vec!["tests-oracle: confirm edge case"]
        );
        assert_eq!(c.cohort_id, "cid");
    }

    #[test]
    fn invalid_model_verdict_stays_explicitly_undecided() {
        let conclusion = parse_reporter_conclusion(
            r#"{"distillation":"not a decision","verdict":"approve-ish"}"#,
            "cid",
            "tid",
        );
        assert_eq!(conclusion.verdict, ReporterVerdict::None);
        assert_eq!(verdict_to_wire(&conclusion.verdict), "none");
    }

    #[test]
    fn parse_reporter_conclusion_fallback_for_non_json() {
        let content = "This is just prose, not JSON.";
        let c = parse_reporter_conclusion(content, "cid", "tid");
        assert_eq!(c.distillation, content);
        assert!(c.proposed_follow_ups.is_empty());
    }

    #[test]
    fn lane_digests_skip_unexecuted_lanes() -> Result<()> {
        let mut receipt = crate::ModelLaneReceipt {
            lane: "x".to_owned(),
            provider: "minimax".to_owned(),
            model: "M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "done".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: crate::ModelCacheUsage::default(),
            cohort_id: "cid".to_owned(),
            shared_prefix_hash: "h".to_owned(),
            thread_id: "tid".to_owned(),
            turn: 0,
            cohort_broken: false,
        };
        let temp = tempfile::tempdir()?;
        let digests = lane_digests_from_receipts(temp.path(), std::slice::from_ref(&receipt));
        assert_eq!(digests.len(), 1);
        // A preflight-only receipt (empty thread_id) is skipped.
        receipt.thread_id = String::new();
        let digests = lane_digests_from_receipts(temp.path(), std::slice::from_ref(&receipt));
        assert!(digests.is_empty());
        Ok(())
    }

    #[test]
    fn reporter_consumes_lane_audit_as_private_prompt_context() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let model_dir = review_dir.join("model");
        crate::write_internal_audit_artifact(
            &model_dir,
            "foo.bar",
            &crate::InternalAudit {
                surfaces_checked: vec!["src/parser.rs".to_owned()],
                strongest_rejected_hypothesis: Some("parser bypasses guard".to_owned()),
                remaining_local_uncertainty: None,
            },
        )?;
        let receipt = crate::ModelLaneReceipt {
            lane: "foo.bar".to_owned(),
            provider: "minimax".to_owned(),
            model: "M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "completed".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: crate::ModelCacheUsage::default(),
            cohort_id: "cid".to_owned(),
            shared_prefix_hash: "h".to_owned(),
            thread_id: "tid".to_owned(),
            turn: 0,
            cohort_broken: false,
        };
        let digests = lane_digests_from_receipts(&review_dir, &[receipt]);
        let prompt = reporter_prompt(&digests, &[]);
        assert!(prompt.contains("src/parser.rs"));
        assert!(prompt.contains("parser bypasses guard"));
        assert!(prompt.contains("do not quote or emit"));
        Ok(())
    }

    #[test]
    fn adversarial_reporter_echo_is_withheld_from_public_conclusion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        crate::write_internal_audit_artifact(
            &review_dir.join("model"),
            "tests-oracle",
            &crate::InternalAudit {
                surfaces_checked: vec!["PRIVATE_SURFACE".to_owned()],
                strongest_rejected_hypothesis: Some("PRIVATE_HYPOTHESIS".to_owned()),
                remaining_local_uncertainty: Some("PRIVATE_UNCERTAINTY".to_owned()),
            },
        )?;
        let conclusion = parse_reporter_conclusion(
            r#"{"distillation":"PRIVATE_SURFACE PRIVATE_HYPOTHESIS","proposed_follow_ups":["lane: PRIVATE_UNCERTAINTY"]}"#,
            "cid",
            "tid",
        );
        let sanitized = withhold_internal_audit_echo(conclusion, &review_dir);
        assert!(!sanitized.distillation.contains("PRIVATE_"));
        assert!(!sanitized.proposed_follow_ups[0].contains("PRIVATE_"));
        Ok(())
    }

    #[test]
    fn write_reporter_thread_creates_artifact() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "PR is safe to merge.".to_owned(),
            proposed_follow_ups: vec!["tests-oracle: edge case?".to_owned()],
            verdict: ReporterVerdict::Clear,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion, "abc123")?;
        let turn_path = review_dir.join("threads/reporter/turn-000.json");
        assert!(turn_path.exists());
        let turn: crate::LaneThreadTurn = serde_json::from_slice(&std::fs::read(&turn_path)?)?;
        assert_eq!(turn.head_sha.as_deref(), Some("abc123"));
        assert_eq!(turn.verdict.as_deref(), Some("clear"));
        assert_eq!(turn.response_summary, "PR is safe to merge.");
        let thread_path = review_dir.join("threads/reporter/thread.json");
        assert!(thread_path.exists());
        let session: crate::LaneThreadSession =
            serde_json::from_slice(&std::fs::read(&thread_path)?)?;
        assert_eq!(session.lane, "reporter");
        assert!(session.latest_conclusion.contains("safe to merge"));
        assert_eq!(session.verdict.as_deref(), Some("clear"));
        assert_eq!(session.head_sha.as_deref(), Some("abc123"));
        Ok(())
    }

    #[test]
    fn resolve_reporter_turn_absent_when_no_artifact() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        assert_eq!(
            resolve_reporter_turn(&review_dir, "abc"),
            ReporterTurnResolution::Absent
        );
        Ok(())
    }

    #[test]
    fn resolve_reporter_turn_reads_current_head_distillation() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "PR is safe to merge; tests cover the change.".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::None,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion, "abc")?;
        let resolved = resolve_reporter_turn(&review_dir, "abc");
        let distillation = resolved
            .public_distillation()
            .ok_or_else(|| anyhow::anyhow!("reporter distillation missing"))?;
        assert!(distillation.contains("safe to merge"));
        Ok(())
    }

    #[test]
    fn structured_verdict_survives_write_resolve_round_trip() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let content =
            r#"{"distillation":"The null fallback is incorrect.","verdict":"changes_requested"}"#;
        let conclusion = parse_reporter_conclusion(content, "cid", "tid");
        assert_eq!(conclusion.verdict, ReporterVerdict::ChangesRequested);
        assert_eq!(conclusion.distillation, "The null fallback is incorrect.");
        write_reporter_thread(&review_dir, &conclusion, "head-a")?;
        match resolve_reporter_turn(&review_dir, "head-a") {
            ReporterTurnResolution::Current(turn) => {
                assert_eq!(turn.verdict, ReporterVerdict::ChangesRequested);
                assert_eq!(turn.distillation, "The null fallback is incorrect.");
                assert_eq!(turn.receipt_ref, "review/threads/reporter/turn-000.json");
                assert_eq!(turn.turn, 0);
            }
            other => anyhow::bail!("expected Current, got {other:?}"),
        }
        match resolve_reporter_turn(&review_dir, "head-a").gate_input() {
            ReporterGateInput::Verdict { verdict, .. } => {
                assert_eq!(verdict, ReporterVerdict::ChangesRequested);
            }
            other => anyhow::bail!("expected Verdict, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn one_resolved_turn_drives_public_text_and_exact_gate_receipt() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let first = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "initial distillation".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::Clear,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_turn(&review_dir, &first, 0, "head-a", "reporter_completed", &[])?;
        let second = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "revised after follow-ups".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::ChangesRequested,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_turn(
            &review_dir,
            &second,
            1,
            "head-a",
            "reporter_re_distilled",
            &["lane-answer:tests-oracle".to_owned()],
        )?;
        match resolve_reporter_turn(&review_dir, "head-a") {
            ReporterTurnResolution::Current(turn) => {
                assert_eq!(turn.turn, 1);
                assert_eq!(turn.receipt_ref, "review/threads/reporter/turn-001.json");
                assert_eq!(turn.distillation, "revised after follow-ups");
                assert_eq!(turn.verdict, ReporterVerdict::ChangesRequested);
            }
            other => anyhow::bail!("expected Current turn-001, got {other:?}"),
        }
        assert_eq!(
            resolve_reporter_turn(&review_dir, "head-a").public_distillation(),
            Some("revised after follow-ups")
        );
        match resolve_reporter_turn(&review_dir, "head-a").gate_input() {
            ReporterGateInput::Verdict { verdict, receipt } => {
                assert_eq!(verdict, ReporterVerdict::ChangesRequested);
                assert_eq!(receipt, "review/threads/reporter/turn-001.json");
            }
            other => anyhow::bail!("expected Verdict, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn stale_head_is_not_public_and_is_explicit_gate_evidence() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "stale decision text".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::ChangesRequested,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion, "old-head")?;
        let resolution = resolve_reporter_turn(&review_dir, "new-head");
        assert!(resolution.public_distillation().is_none());
        match resolution {
            ReporterTurnResolution::StaleHead {
                expected,
                found,
                receipt,
            } => {
                assert_eq!(expected, "new-head");
                assert_eq!(found, "old-head");
                assert_eq!(receipt, "review/threads/reporter/turn-000.json");
            }
            other => anyhow::bail!("expected StaleHead, got {other:?}"),
        }
        match resolve_reporter_turn(&review_dir, "new-head").gate_input() {
            ReporterGateInput::Unusable { kind, receipt, .. } => {
                assert_eq!(kind, "reporter-stale-head");
                assert_eq!(receipt, "review/threads/reporter/turn-000.json");
            }
            other => anyhow::bail!("expected Unusable, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn new_reporter_run_rejects_prior_run_turns_on_the_same_head() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let prior = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "prior invocation turn one".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::ChangesRequested,
            cohort_id: "prior-cohort".to_owned(),
            thread_id: "prior-thread".to_owned(),
        };
        write_reporter_turn(&review_dir, &prior, 1, "same-head", "complete", &[])?;

        prepare_reporter_run(&review_dir)?;
        let current = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "current invocation turn zero".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::Clear,
            cohort_id: "current-cohort".to_owned(),
            thread_id: "current-thread".to_owned(),
        };
        write_reporter_thread(&review_dir, &current, "same-head")?;

        match resolve_reporter_turn(&review_dir, "same-head") {
            ReporterTurnResolution::Current(turn) => {
                assert_eq!(turn.turn, 0);
                assert_eq!(turn.thread_id, "current-thread");
                assert_eq!(turn.distillation, "current invocation turn zero");
            }
            other => anyhow::bail!("expected current invocation turn, got {other:?}"),
        }
        assert!(!review_dir.join("threads/reporter/turn-001.json").exists());
        Ok(())
    }

    #[test]
    fn startup_invalidation_failure_withholds_restored_disk_authority() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "must not be reused".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::Clear,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion, "head-a")?;
        let thread_dir = review_dir.join("threads/reporter");
        let rollup_path = thread_dir.join("thread.json");
        let saved_rollup = thread_dir.join("thread.saved.json");
        std::fs::rename(&rollup_path, &saved_rollup)?;
        std::fs::create_dir(&rollup_path)?;

        let startup_error = match prepare_reporter_run(&review_dir) {
            Ok(()) => anyhow::bail!("forced startup invalidation failure must propagate"),
            Err(error) => error,
        };

        // Simulate a transient filesystem failure: the old valid authority is
        // readable again before final compilation. The in-memory failure must
        // still dominate disk state.
        std::fs::remove_dir(&rollup_path)?;
        std::fs::rename(&saved_rollup, &rollup_path)?;
        let error = format!("{startup_error:#}");
        let resolution = resolve_reporter_after_coordination(&review_dir, "head-a", Some(&error));
        assert!(resolution.public_distillation().is_none());
        match resolution.gate_input() {
            ReporterGateInput::Unusable {
                kind,
                detail,
                receipt,
            } => {
                assert_eq!(kind, "reporter-malformed");
                assert!(detail.contains("invocation start"));
                assert_eq!(receipt, "review/threads/reporter/thread.json");
            }
            other => anyhow::bail!("expected explicit unusable reporter evidence, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn unknown_wire_verdict_is_malformed_and_fails_closed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "must not become public".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::Clear,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion, "head-a")?;
        let thread_dir = review_dir.join("threads/reporter");
        let turn_path = thread_dir.join("turn-000.json");
        let mut turn: crate::LaneThreadTurn = serde_json::from_slice(&std::fs::read(&turn_path)?)?;
        turn.verdict = Some("future_verdict".to_owned());
        std::fs::write(&turn_path, serde_json::to_vec_pretty(&turn)?)?;
        let rollup_path = thread_dir.join("thread.json");
        let mut rollup: crate::LaneThreadSession =
            serde_json::from_slice(&std::fs::read(&rollup_path)?)?;
        rollup.verdict = Some("future_verdict".to_owned());
        std::fs::write(&rollup_path, serde_json::to_vec_pretty(&rollup)?)?;

        let resolution = resolve_reporter_turn(&review_dir, "head-a");
        assert!(resolution.public_distillation().is_none());
        match resolution.gate_input() {
            ReporterGateInput::Unusable { kind, receipt, .. } => {
                assert_eq!(kind, "reporter-malformed");
                assert_eq!(receipt, "review/threads/reporter/turn-000.json");
            }
            other => anyhow::bail!("expected malformed gate evidence, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn malformed_rollup_fails_closed_without_selecting_a_turn() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "must not become public".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::Clear,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion, "head-a")?;
        std::fs::write(
            review_dir.join("threads/reporter/thread.json"),
            b"{ malformed",
        )?;

        let resolution = resolve_reporter_turn(&review_dir, "head-a");
        assert!(resolution.public_distillation().is_none());
        match resolution.gate_input() {
            ReporterGateInput::Unusable { kind, receipt, .. } => {
                assert_eq!(kind, "reporter-malformed");
                assert_eq!(receipt, "review/threads/reporter/thread.json");
            }
            other => anyhow::bail!("expected malformed rollup evidence, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn reporter_resolution_rejects_turn_rollup_identity_mismatches() -> Result<()> {
        let cases = [
            (
                "turn-schema",
                "turn",
                "schema",
                serde_json::json!("future"),
                "turn.schema",
            ),
            (
                "turn-stage",
                "turn",
                "stage",
                serde_json::json!("follow-up"),
                "turn.stage",
            ),
            (
                "turn-receipt",
                "turn",
                "receipt_ref",
                serde_json::json!("review/threads/reporter/turn-999.json"),
                "turn.receipt_ref",
            ),
            (
                "turn-number",
                "turn",
                "turn",
                serde_json::json!(7),
                "turn.turn/rollup.latest_turn",
            ),
            (
                "rollup-thread",
                "rollup",
                "thread_id",
                serde_json::json!("other-thread"),
                "rollup.thread_id",
            ),
            (
                "rollup-head",
                "rollup",
                "head_sha",
                serde_json::json!("other-head"),
                "rollup.head_sha",
            ),
            (
                "rollup-verdict",
                "rollup",
                "verdict",
                serde_json::json!("uncertain"),
                "rollup.verdict",
            ),
        ];
        for (name, artifact, field, replacement, expected_detail) in cases {
            let temp = tempfile::tempdir()?;
            let review_dir = temp.path().join("review");
            let conclusion = ReporterConclusion {
                schema: REPORTER_THREAD_SCHEMA.to_owned(),
                distillation: "authoritative text".to_owned(),
                proposed_follow_ups: vec![],
                verdict: ReporterVerdict::Clear,
                cohort_id: "cid".to_owned(),
                thread_id: "tid".to_owned(),
            };
            write_reporter_thread(&review_dir, &conclusion, "head-a")?;
            let path = review_dir.join(format!(
                "threads/reporter/{}",
                if artifact == "turn" {
                    "turn-000.json"
                } else {
                    "thread.json"
                }
            ));
            let mut value: serde_json::Value = serde_json::from_slice(&std::fs::read(&path)?)?;
            value[field] = replacement;
            std::fs::write(&path, serde_json::to_vec_pretty(&value)?)?;

            let resolution = resolve_reporter_turn(&review_dir, "head-a");
            let ReporterTurnResolution::Malformed { error, .. } = &resolution else {
                anyhow::bail!("{name}: expected malformed resolution, got {resolution:?}");
            };
            assert!(error.contains(expected_detail), "{name}: {error}");
            assert!(resolution.public_distillation().is_none(), "{name}");
        }
        Ok(())
    }

    #[test]
    fn turn_001_write_failure_is_propagated() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let thread_dir = review_dir.join("threads/reporter");
        let conclusion = ReporterConclusion {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            distillation: "initial".to_owned(),
            proposed_follow_ups: vec![],
            verdict: ReporterVerdict::ChangesRequested,
            cohort_id: "cid".to_owned(),
            thread_id: "tid".to_owned(),
        };
        write_reporter_thread(&review_dir, &conclusion, "head-a")?;
        std::fs::create_dir_all(thread_dir.join("turn-001.json"))?;
        let mut revised = conclusion;
        revised.distillation = "revised".to_owned();
        revised.verdict = ReporterVerdict::Clear;
        let err = match write_reporter_turn(
            &review_dir,
            &revised,
            1,
            "head-a",
            "reporter_re_distilled",
            &[],
        ) {
            Ok(_) => anyhow::bail!("turn-001 write failure must propagate"),
            Err(err) => err,
        };
        assert!(!err.to_string().is_empty());
        assert_eq!(
            resolve_reporter_turn(&review_dir, "head-a"),
            ReporterTurnResolution::Absent
        );
        Ok(())
    }

    #[test]
    fn legacy_turn_without_invocation_head_fails_closed() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        let thread_dir = review_dir.join("threads").join("reporter");
        std::fs::create_dir_all(&thread_dir)?;
        let legacy = crate::LaneThreadTurn {
            schema: REPORTER_THREAD_SCHEMA.to_owned(),
            thread_id: "tid".to_owned(),
            turn: 0,
            stage: "reporter".to_owned(),
            prompt_packet_path: "review/threads/reporter/prompt.md".to_owned(),
            response_summary: "legacy distillation".to_owned(),
            routed_evidence_refs: vec![],
            receipt_ref: "review/threads/reporter/turn-000.json".to_owned(),
            head_sha: None,
            verdict: None,
        };
        std::fs::write(
            thread_dir.join("turn-000.json"),
            serde_json::to_vec_pretty(&legacy)?,
        )?;
        std::fs::write(
            thread_dir.join("thread.json"),
            serde_json::to_vec_pretty(&crate::LaneThreadSession {
                schema: crate::artifacts::LANE_THREAD_SCHEMA.to_owned(),
                thread_id: "tid".to_owned(),
                lane: "reporter".to_owned(),
                cohort_id: "cid".to_owned(),
                turns: vec!["turn-000".to_owned()],
                latest_turn: Some("turn-000".to_owned()),
                latest_turn_ref: Some("review/threads/reporter/turn-000.json".to_owned()),
                latest_conclusion: "legacy distillation".to_owned(),
                head_sha: None,
                verdict: None,
                terminal_reason: "reporter_completed".to_owned(),
            })?,
        )?;
        let resolution = resolve_reporter_turn(&review_dir, "any-head");
        assert!(resolution.public_distillation().is_none());
        match resolution.gate_input() {
            ReporterGateInput::Unusable { kind, receipt, .. } => {
                assert_eq!(kind, "reporter-malformed");
                assert_eq!(receipt, "review/threads/reporter/turn-000.json");
            }
            other => anyhow::bail!("expected unusable legacy turn, got {other:?}"),
        }
        Ok(())
    }

    #[test]
    fn lane_continuation_prompt_includes_question_and_prior_conclusion() {
        let prompt = crate::lane_continuation_prompt(
            "tests-oracle",
            "specialist reviewer",
            "The test does not discriminate the patch.",
            "Does the test fail against base source plus the new fixture?",
            "PR looks safe; one test-gap concern from tests-oracle.",
            &[],
            &[],
        );
        assert!(prompt.contains("tests-oracle"));
        assert!(prompt.contains("does not discriminate"));
        assert!(prompt.contains("Does the test fail"));
        assert!(prompt.contains("reporter"));
        assert!(prompt.contains("Revise, confirm, or withdraw"));
        assert!(prompt.contains("\"changed\""));
        // No proof evidence section when excerpts are empty.
        assert!(!prompt.contains("Routed proof evidence"));
        assert!(!prompt.contains("Routed late deterministic evidence"));
    }

    #[test]
    fn lane_continuation_prompt_includes_proof_evidence_when_present() {
        let prompt = crate::lane_continuation_prompt(
            "tests-oracle",
            "specialist reviewer",
            "The test may not discriminate the patch.",
            "Does the test fail against base source?",
            "PR has a test-gap concern.",
            &[
                "proof `proof-001` result=`non_discriminating` reason=`base+tests passed the same`"
                    .to_owned(),
            ],
            &[],
        );
        assert!(prompt.contains("Routed proof evidence"));
        assert!(prompt.contains("non_discriminating"));
        assert!(prompt.contains("Revise"));
    }

    #[test]
    fn lane_continuation_prompt_routes_late_sensor_evidence_when_present() {
        let prompt = crate::lane_continuation_prompt(
            "tests-oracle",
            "specialist reviewer",
            "The test may not discriminate the patch.",
            "Does the full suite confirm your concern?",
            "PR has a test-gap concern.",
            &[],
            &[LateSensorDigest {
                sensor: "cargo-test".to_owned(),
                status: "failed".to_owned(),
                reason: "exit code Some(101)".to_owned(),
                receipt_path: "sensors/cargo-test/ub-review-sensor-status.json".to_owned(),
            }
            .excerpt()],
        );
        assert!(prompt.contains("Routed late deterministic evidence"));
        assert!(prompt.contains("late sensor `cargo-test` status=`failed`"));
        assert!(prompt.contains("landed after your primary turn"));
    }

    #[test]
    fn resolve_lane_target_strips_question_for_prefix() {
        let lanes = ["tests-oracle", "workflow-proof", "opposition"];
        assert_eq!(
            crate::resolve_lane_target("Question for tests-oracle", &lanes),
            Some("tests-oracle".to_owned())
        );
        assert_eq!(
            crate::resolve_lane_target("Question for workflow-proof", &lanes),
            Some("workflow-proof".to_owned())
        );
    }

    #[test]
    fn resolve_lane_target_exact_and_suffix() {
        let lanes = ["tests-oracle", "opposition"];
        assert_eq!(
            crate::resolve_lane_target("opposition", &lanes),
            Some("opposition".to_owned())
        );
        assert_eq!(
            crate::resolve_lane_target("tests-oracle lane", &lanes),
            Some("tests-oracle".to_owned())
        );
    }

    #[test]
    fn resolve_lane_target_returns_none_for_unknown() {
        let lanes = ["tests-oracle"];
        assert_eq!(crate::resolve_lane_target("nonexistent-lane", &lanes), None);
    }
}
