//! Summary rendering: lane packet summaries, review efficiency
//! section, evidence sections, and PR number detection (cleanup
//! train step 53, pure code motion).

use crate::*;

pub(crate) fn detect_pull_number_from_event() -> Option<u64> {
    let path = std::env::var_os("GITHUB_EVENT_PATH")?;
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .pointer("/pull_request/number")
        .and_then(serde_json::Value::as_u64)
}

pub(crate) struct LanePacketSummaryRow {
    id: String,
    model_display: String,
}

pub(crate) fn lane_packet_summary_rows(out: &Path, plan: &Plan) -> Vec<LanePacketSummaryRow> {
    if let Some(ids) = resolved_effective_model_lane_ids(out) {
        return ids
            .into_iter()
            .map(|id| LanePacketSummaryRow {
                model_display: lane_packet_model_display(out, &id)
                    .or_else(|| plan_lane_model_display(plan, &id))
                    .unwrap_or_else(|| "unknown".to_owned()),
                id,
            })
            .collect();
    }
    plan.lanes
        .iter()
        .map(|lane| LanePacketSummaryRow {
            id: lane.id.clone(),
            model_display: lane.model_display.clone(),
        })
        .collect()
}

pub(crate) fn lane_packet_count(out: &Path) -> usize {
    fs::read_dir(out.join("lanes"))
        .ok()
        .into_iter()
        .flat_map(|entries| entries.filter_map(std::result::Result::ok))
        .filter(|entry| {
            entry
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                == Some("md")
        })
        .count()
}

pub(crate) fn resolved_effective_model_lane_ids(out: &Path) -> Option<Vec<String>> {
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-plan.json")).ok()?).ok()?;
    let lanes = value
        .pointer("/selectors/effective_model_lanes")?
        .as_array()?
        .iter()
        .filter_map(|lane| lane.as_str().map(ToOwned::to_owned))
        .collect::<Vec<_>>();
    Some(lanes)
}

pub(crate) fn lane_packet_model_display(out: &Path, lane_id: &str) -> Option<String> {
    let text = fs::read_to_string(out.join("lanes").join(format!("{lane_id}.md"))).ok()?;
    text.lines().find_map(|line| {
        line.strip_prefix("Model: `")
            .and_then(|rest| rest.strip_suffix('`'))
            .map(ToOwned::to_owned)
    })
}

pub(crate) fn plan_lane_model_display(plan: &Plan, lane_id: &str) -> Option<String> {
    plan.lanes
        .iter()
        .find(|lane| lane.id == lane_id)
        .map(|lane| lane.model_display.clone())
}

pub(crate) fn render_summary(out: &Path, plan: &Plan, diff: &DiffContext) -> Result<String> {
    let mut text = String::new();
    text.push_str("# UB Review Packet\n\n");
    text.push_str("This is an advisory evidence packet plus review compiler. Posting is a separate grouped Pull Request Review step.\n\n");
    text.push_str(&format!("- Profile: `{}`\n", plan.profile_name));
    text.push_str(&format!("- Base: `{}`\n", plan.base));
    text.push_str(&format!("- Head: `{}`\n", plan.head));
    text.push_str(&format!(
        "- Changed files: `{}`\n",
        diff.changed_files.len()
    ));
    text.push_str(&format!("- Diff class: `{}`\n", diff.diff_class.key()));
    render_review_efficiency_section(&mut text, out);
    text.push_str("\n## Sensors\n\n");
    text.push_str("| Sensor | Planned | Status | Reason | Receipt |\n");
    text.push_str("|---|---:|---|---|---|\n");
    for sensor in &plan.sensors {
        let status_path = out
            .join("sensors")
            .join(&sensor.id)
            .join("ub-review-sensor-status.json");
        let receipt = read_sensor_receipt(&status_path);
        let status = receipt
            .as_ref()
            .map(|receipt| receipt.status.clone())
            .unwrap_or_else(|| {
                if sensor.run {
                    "receipt-absent".to_owned()
                } else {
                    "skipped".to_owned()
                }
            });
        let reason = receipt
            .as_ref()
            .map(|receipt| receipt.reason.as_str())
            .unwrap_or(&sensor.reason);
        let planned = if sensor.run { "yes" } else { "no" };
        let receipt = format!("`sensors/{}/`", sensor.id);
        text.push_str(&format!(
            "| `{}` | {} | `{}` | {} | {} |\n",
            sensor.id,
            planned,
            status,
            escape_md(reason),
            receipt
        ));
    }
    render_evidence_sections(&mut text, out, plan);
    render_model_status_sections(&mut text, out);
    text.push_str("\n## Lane packets\n\n");
    text.push_str("| Lane | Model | Packet |\n");
    text.push_str("|---|---|---|\n");
    for lane in lane_packet_summary_rows(out, plan) {
        text.push_str(&format!(
            "| `{}` | `{}` | `lanes/{}.md` |\n",
            lane.id, lane.model_display, lane.id
        ));
    }
    text.push_str("\n## Diff flags\n\n");
    text.push_str(&format!(
        "- Unsafe/native risk touched: `{}`\n",
        diff.flags.unsafe_or_native_risk
    ));
    text.push_str(&format!(
        "- Rust behavior or tests touched: `{}`\n",
        diff.flags.rust_changed || diff.flags.rust_tests_changed
    ));
    text.push_str(&format!(
        "- Source changed: `{}`\n",
        diff.flags.source_changed
    ));
    text.push_str("\n## Changed files\n\n");
    if diff.changed_files.is_empty() {
        text.push_str("- No changed files detected. Check checkout/base configuration.\n");
    } else {
        for file in &diff.changed_files {
            text.push_str(&format!("- `{file}`\n"));
        }
    }
    text.push_str("\n## Notes\n\n");
    if plan.notes.is_empty() {
        text.push_str("- No planner notes.\n");
    } else {
        for note in &plan.notes {
            text.push_str(&format!("- {}\n", escape_md(note)));
        }
    }
    text.push_str("\n## Review posture\n\n");
    text.push_str("A one-line approval shortcut is a failure mode. A no-finding lane must include what it checked, its strongest failed objection, and residual risk. Missing sensor evidence is not proof of safety.\n");
    Ok(text)
}

pub(crate) fn render_review_efficiency_section(text: &mut String, out: &Path) {
    let Some(metrics) = read_review_metrics(out) else {
        return;
    };
    let runtime = metrics
        .get("wall_clock_seconds")
        .and_then(serde_json::Value::as_u64)
        .map(format_seconds)
        .unwrap_or_else(|| "unknown".to_owned());
    let total_lanes = metrics
        .pointer("/models/model_lanes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let ok_lanes = metrics
        .pointer("/models/model_lane_status_counts/ok")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let degraded_lanes = metrics
        .pointer("/models/model_lane_status_counts/degraded")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let usable_lanes = ok_lanes.saturating_add(degraded_lanes);
    let inline_comments = metrics
        .get("github_review_comments")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let max_inline_comments = metrics
        .get("max_inline_comments")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let off_diff_rejected = metrics
        .get("off_diff_candidates_rejected")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let provider_failures = metrics
        .get("provider_evidence_failures")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let follow_up_results = metrics
        .pointer("/follow_up_results/total")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let follow_up_attempted = metrics
        .pointer("/follow_up_results/calls_attempted")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let follow_up_statuses = metrics
        .pointer("/follow_up_results/status_counts")
        .and_then(serde_json::Value::as_object)
        .map(format_json_status_counts)
        .unwrap_or_else(|| "none".to_owned());
    let payload_status = metrics
        .get("review_payload_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let post_status = metrics
        .get("post_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let terminal_state = metrics
        .get("terminal_state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let coordination_wall = metrics
        .pointer("/run/coordination_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let investigation_wall = metrics
        .pointer("/run/investigation_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let proof_stream_wall = metrics
        .pointer("/run/proof_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let model_wall = metrics
        .pointer("/run/model_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let proof_wall = metrics
        .pointer("/run/local_proof_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let compiler_wall = metrics
        .pointer("/run/compiler_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let overlap = metrics
        .pointer("/run/investigation_proof_overlap_ms")
        .or_else(|| metrics.pointer("/run/model_proof_overlap_ms"))
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let concurrency_model = metrics
        .pointer("/run/concurrency_model")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let scheduler_profile = metrics
        .pointer("/run/scheduler_profile")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");

    text.push_str("\n## Review efficiency\n\n");
    text.push_str(&format!("- Runtime: `{runtime}`\n"));
    text.push_str(&format!(
        "- Run streams: coordination `{coordination_wall}`, investigation `{investigation_wall}`, proof `{proof_stream_wall}`, investigation/proof overlap `{overlap}` (`{scheduler_profile}` via `{concurrency_model}`)\n"
    ));
    text.push_str(&format!(
        "- Loop detail: model `{model_wall}`, local proof `{proof_wall}`, compiler `{compiler_wall}`\n"
    ));
    text.push_str(&format!("- Terminal state: `{terminal_state}`\n"));
    if let Some(gate) = read_gate_outcome(out) {
        let conclusion = gate
            .get("conclusion")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown");
        let blocking_reasons = gate
            .get("reasons")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len)
            .unwrap_or(0);
        text.push_str(&format!(
            "- Gate: `{conclusion}` with `{blocking_reasons}` blocking reasons (`review/gate_outcome.json`)\n"
        ));
    }
    text.push_str(&format!(
        "- Model lanes: `{usable_lanes}/{total_lanes}` usable (`{ok_lanes}` ok, `{degraded_lanes}` degraded)\n"
    ));
    text.push_str(&format!(
        "- Inline comments: `{inline_comments}/{max_inline_comments}`\n"
    ));
    text.push_str(&format!(
        "- Off-diff candidates rejected: `{off_diff_rejected}`\n"
    ));
    text.push_str(&format!(
        "- Provider evidence failures: `{provider_failures}`\n"
    ));
    text.push_str(&format!(
        "- Follow-up results: `{follow_up_results}` total, `{follow_up_attempted}` attempted ({follow_up_statuses})\n"
    ));
    text.push_str(&format!(
        "- Review payload: `{payload_status}`; post: `{post_status}`\n"
    ));

    // Calibration summary (compact, scannable). Computed from
    // review/calibration.json — NOT from model prose.
    if let Some(cal) = read_calibration_summary(out) {
        text.push_str("\n## Calibration\n\n");
        text.push_str(&format!(
            "- Cohort: `{}`, cache-coherent: `{}`\n",
            cal.cohort_model, !cal.cohort_broken
        ));
        text.push_str(&format!(
            "- Lanes: `{}` executed, `{}` continued (turn-001)\n",
            cal.lanes_executed, cal.lane_continuations
        ));
        text.push_str(&format!(
            "- Reporter: `{}` questions\n",
            cal.reporter_questions
        ));
        text.push_str(&format!(
            "- Model-selected proof: `{}` requested, `{}` executed, `{}` routed\n",
            cal.proof_model_selected, cal.proof_executed, cal.proof_routed
        ));
        text.push_str(&format!(
            "- Proof changed conclusions: `{}`\n",
            cal.proof_changed_conclusions
        ));
        text.push_str(&format!(
            "- Final review: `{}` inline, `{}` summary | Gate: `{}`\n",
            cal.inline_comments, cal.summary_comments, cal.gate
        ));
    }
}

/// Compact calibration summary extracted from review/calibration.json.
struct CalibrationSummary {
    cohort_model: String,
    cohort_broken: bool,
    lanes_executed: usize,
    lane_continuations: usize,
    reporter_questions: usize,
    proof_model_selected: usize,
    proof_executed: usize,
    proof_routed: usize,
    proof_changed_conclusions: usize,
    inline_comments: usize,
    summary_comments: usize,
    gate: String,
}

fn read_calibration_summary(out: &Path) -> Option<CalibrationSummary> {
    let path = out.join("review").join("calibration.json");
    let text = fs::read_to_string(&path).ok()?;
    let cal: serde_json::Value = serde_json::from_str(&text).ok()?;
    let counts = cal.get("counts")?;
    let cohort = cal.get("cohort")?;
    Some(CalibrationSummary {
        cohort_model: cohort
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned(),
        cohort_broken: cohort
            .get("cohort_broken")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        lanes_executed: counts
            .get("lanes_executed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        lane_continuations: counts
            .get("lane_continuations")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        reporter_questions: counts
            .get("reporter_questions")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        proof_model_selected: counts
            .get("proof_requests_model_selected")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        proof_executed: counts
            .get("proof_requests_executed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        proof_routed: counts
            .get("proof_receipts_routed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        proof_changed_conclusions: counts
            .get("lane_conclusions_changed_by_proof")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        inline_comments: counts
            .get("inline_comments_posted")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        summary_comments: counts
            .get("summary_comments_posted")
            .and_then(|v| v.as_u64())
            .unwrap_or(0) as usize,
        gate: cal
            .get("gate_policy")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_owned(),
    })
}

pub(crate) fn format_millis(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else {
        format_seconds(ms / 1_000)
    }
}

pub(crate) fn format_json_status_counts(
    counts: &serde_json::Map<String, serde_json::Value>,
) -> String {
    let parts = counts
        .iter()
        .filter_map(|(status, count)| count.as_u64().map(|count| format!("{status}={count}")))
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

pub(crate) fn read_review_metrics(out: &Path) -> Option<serde_json::Value> {
    let text = fs::read_to_string(out.join("review/metrics.json")).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn read_gate_outcome(out: &Path) -> Option<serde_json::Value> {
    let text = fs::read_to_string(out.join("review/gate_outcome.json")).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn format_seconds(seconds: u64) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m{seconds:02}s")
    }
}

pub(crate) fn render_evidence_sections(text: &mut String, out: &Path, plan: &Plan) {
    let mut available = Vec::new();
    let mut missing = Vec::new();
    let mut failed = Vec::new();
    let mut scheduled_late = Vec::new();

    for sensor in &plan.sensors {
        let status_path = out
            .join("sensors")
            .join(&sensor.id)
            .join("ub-review-sensor-status.json");
        let Some(receipt) = read_sensor_receipt(&status_path) else {
            if sensor.run {
                // #325: a late-phase sensor without a receipt is scheduled
                // work, not missing evidence — its receipt lands before the
                // reporter/compile/gate. Fast sensors without receipts stay
                // missing evidence.
                if matches!(sensor.phase, SensorPhase::Late) {
                    scheduled_late.push(format!(
                        "{} scheduled in the late evidence phase; its receipt lands before the gate (late is not missing).",
                        sensor.id
                    ));
                } else {
                    missing.push(format!(
                        "{} receipt absent; {} unavailable.",
                        sensor.id,
                        evidence_label(&sensor.id)
                    ));
                }
            }
            continue;
        };
        match receipt.status.as_str() {
            "ok" => available.push(format!(
                "{} ran; {} available.",
                sensor.id,
                evidence_label(&sensor.id)
            )),
            "missing" => missing.push(format!(
                "{} not installed; {} unavailable.",
                sensor.id,
                evidence_label(&sensor.id)
            )),
            "failed" | "timed_out" => failed.push(format!(
                "{} {}; reason: {}.",
                sensor.id, receipt.status, receipt.reason
            )),
            "skipped" if is_sensor_evidence_issue(sensor, &receipt.status, &receipt.reason) => {
                missing.push(format!(
                    "{} skipped; {} unavailable; reason: {}.",
                    sensor.id,
                    evidence_label(&sensor.id),
                    receipt.reason
                ));
            }
            status if is_sensor_evidence_issue(sensor, status, &receipt.reason) => {
                missing.push(format!(
                    "{} {}; {} unavailable; reason: {}.",
                    sensor.id,
                    status,
                    evidence_label(&sensor.id),
                    receipt.reason
                ))
            }
            _ => {}
        }
    }

    // Configured-but-unevaluated required tool gates are the alarm class
    // from #316: the repo's policy text promised a threshold and the run
    // produced no verdict for it. Surface that beside missing sensor
    // evidence so the gap cannot idle quietly in artifacts again.
    if let Ok(bytes) = fs::read(out.join("tool-gate-outcomes.json"))
        && let Ok(artifact) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let Some(outcomes) = artifact.get("outcomes").and_then(|value| value.as_array())
    {
        for entry in outcomes {
            let required = entry
                .get("required")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let evaluated = entry
                .get("evaluated")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if required && !evaluated {
                let tool = entry
                    .get("tool")
                    .and_then(|value| value.as_str())
                    .unwrap_or("unknown-tool");
                let reason = entry
                    .get("reason")
                    .and_then(|value| value.as_str())
                    .unwrap_or("no reason recorded");
                missing.push(format!(
                    "{tool} gate threshold configured but not evaluated; reason: {reason}."
                ));
            }
        }
    }

    text.push_str("\n## Available evidence\n\n");
    if available.is_empty() {
        text.push_str("- No sensor evidence completed successfully.\n");
    } else {
        for item in available {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }

    text.push_str("\n## Missing evidence\n\n");
    if missing.is_empty() {
        text.push_str("- No planned sensor evidence is currently missing.\n");
    } else {
        for item in missing {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }

    if !failed.is_empty() {
        text.push_str("\n## Failed evidence\n\n");
        for item in failed {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }

    if !scheduled_late.is_empty() {
        text.push_str("\n## Scheduled late evidence\n\n");
        for item in scheduled_late {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }
}
