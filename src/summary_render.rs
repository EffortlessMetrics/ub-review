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
    let text = fs::read_to_string(
        out.join("lanes")
            .join(format!("{}.md", sanitize_artifact_name(lane_id))),
    )
    .ok()?;
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
    render_summary_for_phase(out, plan, diff, true)
}

pub(crate) fn render_preliminary_summary(
    out: &Path,
    plan: &Plan,
    diff: &DiffContext,
) -> Result<String> {
    render_summary_for_phase(out, plan, diff, false)
}

fn render_summary_for_phase(
    out: &Path,
    plan: &Plan,
    diff: &DiffContext,
    review_evidence_expected: bool,
) -> Result<String> {
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
    if review_evidence_expected {
        render_review_efficiency_section(&mut text, out);
    }
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
    if review_evidence_expected {
        render_model_status_sections(&mut text, out);
    }
    text.push_str("\n## Lane packets\n\n");
    text.push_str("| Lane | Model | Packet |\n");
    text.push_str("|---|---|---|\n");
    for lane in lane_packet_summary_rows(out, plan) {
        text.push_str(&format!(
            "| `{}` | `{}` | `lanes/{}.md` |\n",
            lane.id,
            lane.model_display,
            sanitize_artifact_name(&lane.id)
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
    // A written plan has no terminal evidence yet. Once any terminal input
    // exists, an unavailable sibling must be explicit rather than hidden.
    if ![
        "review/gate_outcome.json",
        "review/metrics.json",
        "review/calibration.json",
        "review/terminal_state.json",
        "review/review.json",
    ]
    .iter()
    .any(|path| out.join(path).try_exists().unwrap_or(true))
    {
        return;
    }

    text.push_str("\n## Review efficiency\n\n");
    match read_gate_outcome(out) {
        Ok((gate, admitted)) => {
            // Admission below restricts the conclusion to the gate-check
            // vocabulary; calibration is never a substitute for this input.
            let conclusion = admitted.conclusion.as_deref().unwrap_or("unknown");
            let blocking_reasons = admitted.reasons.len();
            text.push_str(&format!(
                "- Gate: `{conclusion}` with `{blocking_reasons}` blocking reasons (`review/gate_outcome.json`)\n"
            ));
            render_planner_required_proofs(text, &gate);
            render_configured_required_proofs(text, &gate);
        }
        Err(reason) => {
            text.push_str(&format!(
                "- Gate: unavailable — `review/gate_outcome.json` {reason}. Recover the original packet or create a new identified run; rerendering cannot recreate gate or proof evidence.\n"
            ));
        }
    }
    match read_review_metrics(out) {
        Ok(metrics) => render_available_review_metrics(text, &metrics),
        Err(reason) => text.push_str(&format!(
            "- Review metrics: unavailable — `review/metrics.json` {reason}. Available gate evidence is reported independently.\n"
        )),
    }
    render_calibration_summary(text, out);
}

fn render_available_review_metrics(text: &mut String, metrics: &serde_json::Value) {
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

    text.push_str(&format!("- Runtime: `{runtime}`\n"));
    text.push_str(&format!(
        "- Run streams: coordination `{coordination_wall}`, investigation `{investigation_wall}`, proof `{proof_stream_wall}`, investigation/proof overlap `{overlap}` (`{scheduler_profile}` via `{concurrency_model}`)\n"
    ));
    text.push_str(&format!(
        "- Loop detail: model `{model_wall}`, local proof `{proof_wall}`, compiler `{compiler_wall}`\n"
    ));
    text.push_str(&format!("- Terminal state: `{terminal_state}`\n"));
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
}

fn render_calibration_summary(text: &mut String, out: &Path) {
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
            "- Final review: `{}` inline, `{}` summary\n",
            cal.inline_comments, cal.summary_comments
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

pub(crate) fn read_review_metrics(
    out: &Path,
) -> std::result::Result<serde_json::Value, &'static str> {
    let bytes = read_summary_bytes(&out.join("review/metrics.json"))?;
    let value = parse_summary_object(&bytes)?;
    // Read the original bytes so duplicate version fields cannot be collapsed.
    #[derive(Deserialize)]
    struct MetricsSchema {
        schema_version: Option<u64>,
    }
    let schema: MetricsSchema =
        serde_json::from_slice(&bytes).map_err(|_| "has an invalid metrics schema version")?;
    if schema.schema_version != Some(1) {
        return Err("has a missing or unsupported metrics schema version");
    }
    Ok(value)
}

fn read_summary_bytes(path: &Path) -> std::result::Result<Vec<u8>, &'static str> {
    fs::read(path).map_err(|error| match error.kind() {
        std::io::ErrorKind::NotFound => "is missing",
        _ => "cannot be read",
    })
}

fn parse_summary_object(bytes: &[u8]) -> std::result::Result<serde_json::Value, &'static str> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| "contains malformed JSON")?;
    if !value.is_object() {
        return Err("does not contain a JSON object");
    }
    Ok(value)
}

/// Sanitize broker-supplied plain text for the step summary (#4271 round
/// 2): portfolio fields are not trusted prose — a crafted reason could
/// forge trusted CI output with links, images, code spans, or embedded
/// newlines. Collapse line breaks to spaces and backslash-escape
/// Markdown-significant punctuation so the text renders literally.
fn sanitize_planner_summary_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\r' | '\n' => out.push(' '),
            '\\' | '`' | '*' | '_' | '[' | ']' | '(' | ')' | '#' | '!' | '|' | '<' | '>' | '~' => {
                out.push('\\');
                out.push(ch);
            }
            _ => out.push(ch),
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Keep configured policy requests separate from planner portfolio obligations.
/// This reports evidence completeness; the artifact's enforcement conclusion
/// still follows repository policy (skips are advisory unless opted in).
fn render_configured_required_proofs(text: &mut String, gate: &serde_json::Value) {
    let counts = gate.get("required_proof").and_then(|accounting| {
        let matched = accounting.get("matched")?.as_u64()?;
        let passed = accounting.get("passed")?.as_u64()?;
        let failed = accounting.get("failed")?.as_u64()?;
        let skipped = accounting.get("skipped")?.as_u64()?;
        (passed.checked_add(failed)?.checked_add(skipped)? == matched)
            .then_some((matched, passed, failed, skipped))
    });
    let Some((matched, passed, failed, skipped)) = counts else {
        text.push_str("- Configured Required proofs: accounting unavailable (`review/gate_outcome.json`); qualification cannot be inferred.\n");
        return;
    };
    text.push_str(&format!(
        "- Configured Required proofs: `{passed}/{matched}` passed (`{failed}` failed, `{skipped}` skipped); gate-required `[[proof.required]]` requests, separate from planner obligations (`review/gate_outcome.json`).\n"
    ));
    if failed > 0 || skipped > 0 {
        text.push_str("- Required proof evidence is incomplete. A skipped or missing proof supplies no passing execution evidence; an advisory gate pass does not qualify these obligations. Inspect `review/proof_requests.json` and `review/proof_receipts.json`, resolve failed or unavailable execution, then obtain passing request-bound proof on the candidate revision.\n");
    }
}

/// Retained planner-required proof obligations (#4271): the
/// `planner_required_proofs` accounting from `review/gate_outcome.json`,
/// rendered right after the Gate line so a deferred required portfolio task
/// (task/request ids, deferral reason, receipt absence, revision digest) is
/// reachable from the final summary text. Every copied planner field is
/// sanitized (`init_markdown_inline_code` for inline-code fields,
/// `sanitize_planner_summary_text` for the reason) so portfolio text can
/// never forge trusted summary output. Only unproven tasks render per-task
/// lines; nothing here ever claims a task is satisfied.
fn render_planner_required_proofs(text: &mut String, gate: &serde_json::Value) {
    let Some(accounting) = gate.get("planner_required_proofs") else {
        return;
    };
    let total = accounting
        .get("total")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let proven = accounting
        .get("proven")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let unproven = accounting
        .get("unproven")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    text.push_str(&format!(
        "- Required planner proofs: `{proven}/{total}` proven (`{unproven}` unproven; planner/broker denominator from `review/proof_portfolio.json`, distinct from gate-required `[[proof.required]]` requests)\n"
    ));
    let digest = crate::init_markdown_inline_code(
        gate.get("revision")
            .and_then(|revision| revision.get("digest"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("unknown"),
    );
    let tasks = accounting
        .get("unproven_tasks")
        .and_then(serde_json::Value::as_array);
    let Some(tasks) = tasks else { return };
    for task in tasks {
        let task_id = crate::init_markdown_inline_code(
            task.get("task_id")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
        );
        let kind = crate::init_markdown_inline_code(
            task.get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
        );
        let status = crate::init_markdown_inline_code(
            task.get("status")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
        );
        let reason = sanitize_planner_summary_text(
            task.get("reason")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("unknown"),
        );
        let request_ids = task
            .get("request_ids")
            .and_then(serde_json::Value::as_array)
            .map(|ids| {
                ids.iter()
                    .filter_map(serde_json::Value::as_str)
                    .map(crate::init_markdown_inline_code)
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_else(|| "unknown".to_owned());
        text.push_str(&format!(
            "- Required planner proof unproven: `{task_id}` ({kind}, {status}): {reason}; requests: {request_ids}; receipts: none; revision `{digest}` (`review/proof_portfolio.json`, `review/gate_outcome.json`)\n"
        ));
    }
}

pub(crate) fn read_gate_outcome(
    out: &Path,
) -> std::result::Result<(serde_json::Value, crate::gate::GateCheckOutcome), &'static str> {
    let bytes = read_summary_bytes(&out.join("review/gate_outcome.json"))?;
    let value = parse_summary_object(&bytes)?;
    // Deserialize the original bytes like gate-check: a Value round-trip
    // would silently collapse duplicate schema, conclusion or reason fields.
    let admitted: crate::gate::GateCheckOutcome =
        serde_json::from_slice(&bytes).map_err(|_| "has an invalid gate shape")?;
    if admitted.schema.as_deref() != Some(crate::artifacts::GATE_OUTCOME_SCHEMA) {
        return Err("has a missing or unsupported gate schema");
    }
    if !matches!(
        admitted.conclusion.as_deref(),
        Some("pass" | "fail" | "inconclusive")
    ) {
        return Err("has a missing or unsupported gate conclusion");
    }
    Ok((value, admitted))
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
            "ok" => {
                if sensor.id == "unsafe-review"
                    && let Err(gap) =
                        read_unsafe_review_artifacts(&out.join("sensors").join(&sensor.id))
                {
                    missing.push(format!(
                        "{} artifact-gap; {} unavailable; reason: {}.",
                        sensor.id,
                        evidence_label(&sensor.id),
                        gap.reason()
                    ));
                } else {
                    available.push(format!(
                        "{} ran; {} available.",
                        sensor.id,
                        evidence_label(&sensor.id)
                    ));
                }
            }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_metrics_summary_rejects_unsupported_schema_versions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::create_dir_all(out.join("review"))?;
        let path = out.join("review/metrics.json");
        fs::write(
            out.join("review/gate_outcome.json"),
            r#"{"schema":"ub-review.gate_outcome.v1","conclusion":"pass","reasons":[]}"#,
        )?;
        let mut metrics = serde_json::json!({
            "schema_version": 1,
            "wall_clock_seconds": 42,
            "github_review_comments": 7
        });
        fs::write(&path, serde_json::to_vec(&metrics)?)?;
        let mut valid = String::new();
        render_review_efficiency_section(&mut valid, out);
        assert!(!valid.contains("Review metrics: unavailable"), "{valid}");
        assert!(valid.contains("- Inline comments: `7/0`"), "{valid}");

        for version in [
            serde_json::json!(2),
            serde_json::json!("1"),
            serde_json::Value::Null,
        ] {
            metrics["schema_version"] = version;
            fs::write(&path, serde_json::to_vec(&metrics)?)?;
            let mut summary = String::new();
            render_review_efficiency_section(&mut summary, out);
            assert!(summary.contains("Review metrics: unavailable"), "{summary}");
            assert!(summary.contains("- Gate: `pass`"), "{summary}");
            assert!(!summary.contains("Inline comments: `7"), "{summary}");
        }
        metrics
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("metrics fixture is not an object"))?
            .remove("schema_version");
        fs::write(&path, serde_json::to_vec(&metrics)?)?;
        let mut summary = String::new();
        render_review_efficiency_section(&mut summary, out);
        assert!(summary.contains("Review metrics: unavailable"), "{summary}");
        fs::write(
            &path,
            r#"{"schema_version":2,"schema_version":1,"github_review_comments":7}"#,
        )?;
        let mut summary = String::new();
        render_review_efficiency_section(&mut summary, out);
        assert!(summary.contains("Review metrics: unavailable"), "{summary}");

        Ok(())
    }

    #[test]
    fn configured_required_proof_summary_retains_gate_accounting() -> Result<()> {
        use crate::tests::{test_plan, test_proof_receipt, test_run_args, test_terminal_state};

        for (result, command_status, blocking, expected_gate, expected_counts, incomplete) in [
            (
                "head_passed",
                "passed",
                false,
                "pass",
                "`1/1` passed (`0` failed, `0` skipped)",
                false,
            ),
            (
                "skipped_budget",
                "skipped",
                false,
                "pass",
                "`0/1` passed (`0` failed, `1` skipped)",
                true,
            ),
            (
                "skipped_profile",
                "skipped",
                true,
                "fail",
                "`0/1` passed (`0` failed, `1` skipped)",
                true,
            ),
            (
                "missing",
                "skipped",
                false,
                "pass",
                "`0/1` passed (`0` failed, `1` skipped)",
                true,
            ),
            (
                "head_failed",
                "failed",
                false,
                "fail",
                "`0/1` passed (`1` failed, `0` skipped)",
                true,
            ),
            (
                "timed_out",
                "timed_out",
                false,
                "fail",
                "`0/1` passed (`1` failed, `0` skipped)",
                true,
            ),
        ] {
            let temp = tempfile::tempdir()?;
            let out = temp.path();
            fs::create_dir_all(out.join("review"))?;
            fs::write(out.join("review/metrics.json"), "{}")?;
            let mut args = test_run_args(out.to_path_buf());
            args.mode = RunMode::IntelligentCi;
            let mut config = Config::default();
            config.gate.blocking.required_proof_unproven = blocking;
            let request = ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "configured-check".to_owned(),
                lane: "intelligent-ci-policy".to_owned(),
                requested_by: vec!["proof-policy:cargo-check".to_owned()],
                command: "cargo check --workspace --locked".to_owned(),
                reason: "Required check".to_owned(),
                cost: "focused-build".to_owned(),
                timeout_sec: 300,
                required: true,
                status: "requested".to_owned(),
            };
            let mut receipt = test_proof_receipt(result, command_status);
            receipt.request_ids = vec![request.id.clone()];
            let receipts = if result == "missing" {
                vec![]
            } else {
                vec![receipt]
            };
            let gate = crate::gate::build_gate_outcome(crate::gate::GateOutcomeInput {
                args: &args,
                config: &config,
                tool_gate_outcomes: &[],
                plan: &test_plan(vec![]),
                terminal_state: &test_terminal_state("sufficient"),
                proof_requests: &[request],
                proof_receipts: &receipts,
                missing_or_failed_sensor_evidence: &[],
                missing_or_failed_model_evidence: &[],
                reporter_gate: crate::ReporterGateInput::Absent,
                planner_portfolio: None,
                revision_digest: None,
            });
            assert_eq!(gate.conclusion, expected_gate, "{result}");
            fs::write(
                out.join("review/gate_outcome.json"),
                serde_json::to_vec(&gate)?,
            )?;
            let mut summary = String::new();
            render_review_efficiency_section(&mut summary, out);
            assert!(
                summary.contains(&format!("- Gate: `{expected_gate}`")),
                "{summary}"
            );
            assert!(
                summary.contains(&format!("Configured Required proofs: {expected_counts}")),
                "{result}: {summary}"
            );
            assert_eq!(
                summary.contains("Required proof evidence is incomplete"),
                incomplete,
                "{summary}"
            );
            if incomplete {
                assert!(
                    summary.contains(
                        "A skipped or missing proof supplies no passing execution evidence"
                    ),
                    "{summary}"
                );
                assert!(summary.contains("review/proof_requests.json"), "{summary}");
                assert!(summary.contains("review/proof_receipts.json"), "{summary}");
            }
            assert!(!summary.contains("Required planner proofs:"), "{summary}");
        }

        Ok(())
    }

    #[test]
    fn unsafe_review_evidence_sections_preserve_exact_availability() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let sensor = crate::tests::sensor_plan("unsafe-review", "unsafe-review", true);
        let plan = crate::tests::test_plan(vec![sensor.clone()]);
        write_sensor_status(
            out,
            &sensor,
            SensorStatusWrite {
                status: "ok",
                argv: &["unsafe-review".to_owned()],
                duration_ms: 10,
                reason: "completed",
                exit_code: Some(0),
                timed_out: false,
            },
        )?;
        let mut missing = String::new();
        render_evidence_sections(&mut missing, out, &plan);
        assert_eq!(
            missing,
            concat!(
                "\n## Available evidence\n\n",
                "- No sensor evidence completed successfully.\n",
                "\n## Missing evidence\n\n",
                "- unsafe-review artifact-gap; unsafe/native reviewability packet unavailable; ",
                "reason: unsafe-review-gate.json absent; structured evidence unavailable.\n"
            )
        );

        let structured = out
            .join("sensors/unsafe-review")
            .join(UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&structured)?;
        fs::write(
            structured.join("unsafe-review-gate.json"),
            r#"{"schema_version":"unsafe-review-gate/v1","status":"advisory","artifacts":{"comment_plan":"comment-plan.json"}}"#,
        )?;
        fs::write(
            structured.join("comment-plan.json"),
            r#"{"schema_version":"0.1","comments":[]}"#,
        )?;
        let mut available = String::new();
        render_evidence_sections(&mut available, out, &plan);
        assert_eq!(
            available,
            concat!(
                "\n## Available evidence\n\n",
                "- unsafe-review ran; unsafe/native reviewability packet available.\n",
                "\n## Missing evidence\n\n",
                "- No planned sensor evidence is currently missing.\n"
            )
        );

        // An ordinary successful sensor needs no unsafe-review artifacts.
        let tokmd = crate::tests::sensor_plan("tokmd", "tokmd", true);
        write_sensor_status(
            out,
            &tokmd,
            SensorStatusWrite {
                status: "ok",
                argv: &["tokmd".to_owned()],
                duration_ms: 10,
                reason: "completed",
                exit_code: Some(0),
                timed_out: false,
            },
        )?;
        let mut ordinary = String::new();
        render_evidence_sections(&mut ordinary, out, &crate::tests::test_plan(vec![tokmd]));
        assert_eq!(
            ordinary,
            concat!(
                "\n## Available evidence\n\n",
                "- tokmd ran; deterministic repository/diff packet available.\n",
                "\n## Missing evidence\n\n",
                "- No planned sensor evidence is currently missing.\n"
            )
        );
        Ok(())
    }

    #[test]
    fn configured_required_proof_summary_distinguishes_unknown_and_empty() {
        for accounting in [
            serde_json::Value::Null,
            serde_json::json!({}),
            serde_json::json!({"matched": 1, "passed": 0, "failed": 0}),
            serde_json::json!({"matched": 1, "passed": "hostile\n[link](url)", "failed": 0, "skipped": 1}),
            serde_json::json!({"matched": 0, "passed": 1, "failed": 0, "skipped": 0}),
            serde_json::json!({"matched": 1, "passed": u64::MAX, "failed": 2, "skipped": 0}),
        ] {
            let mut summary = String::new();
            render_configured_required_proofs(
                &mut summary,
                &serde_json::json!({"required_proof": accounting}),
            );
            assert_eq!(
                summary,
                "- Configured Required proofs: accounting unavailable (`review/gate_outcome.json`); qualification cannot be inferred.\n"
            );
        }
        let mut legacy = String::new();
        render_configured_required_proofs(&mut legacy, &serde_json::json!({}));
        assert!(legacy.contains("accounting unavailable"));
        let mut empty = String::new();
        render_configured_required_proofs(
            &mut empty,
            &serde_json::json!({"required_proof": {"matched": 0, "passed": 0, "failed": 0, "skipped": 0}}),
        );
        assert!(empty.contains("`0/0` passed (`0` failed, `0` skipped)"));
        assert!(!empty.contains("evidence is incomplete"));
    }

    #[test]
    fn lane_packet_model_display_resolves_encoded_hostile_lane_id() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::create_dir_all(out.join("lanes"))?;
        let lane = "../foo/bar é";
        let encoded = sanitize_artifact_name(lane);
        fs::write(
            out.join("lanes").join(format!("{encoded}.md")),
            "# Lane: hostile\n\nModel: `test-model`\n",
        )?;

        assert_eq!(
            lane_packet_model_display(out, lane).as_deref(),
            Some("test-model")
        );
        assert_eq!(
            fs::read_to_string(out.join("lanes").join("~2E~2E~2Ffoo~2Fbar~20~C3~A9.md"))?,
            "# Lane: hostile\n\nModel: `test-model`\n"
        );
        assert!(!out.join("foo").exists());
        assert_eq!(lane_packet_model_display(out, "missing/lane"), None);
        Ok(())
    }

    #[test]
    fn lane_packet_model_display_returns_none_for_missing_packet() -> Result<()> {
        let temp = tempfile::tempdir()?;
        assert_eq!(lane_packet_model_display(temp.path(), "missing/lane"), None);
        Ok(())
    }

    /// F5 portfolio-to-summary consistency: the rendered summary retains
    /// the deferred required task's identities, verbatim reason, receipt
    /// absence, and the exact revision digest from the gate artifact.
    #[test]
    fn deferred_required_planner_proof_reaches_summary_with_digest() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::create_dir_all(out.join("review"))?;
        fs::write(out.join("review/metrics.json"), "{}")?;
        let digest = "66e7daf76a735dc2678f4889523cd86d3dd41b1cca9976c83053c2d032322ed3";
        let gate = serde_json::json!({
            "schema": "ub-review.gate_outcome.v1",
            "conclusion": "pass",
            "reasons": [],
            "required_proof": {"matched": 0, "passed": 0, "failed": 0, "skipped": 0},
            "planner_required_proofs": {
                "denominator": "required planner proof portfolio tasks",
                "gate_required_requests": 0,
                "total": 1,
                "proven": 0,
                "unproven": 1,
                "unproven_tasks": [{
                    "task_id": "proof-red-green-b27777653387",
                    "kind": "focused-red-green",
                    "status": "deferred_by_safe_wind_down",
                    "reason": "required floor could not fit inside the remaining safe proof budget",
                    "request_ids": [
                        "proof-intent-c763f38728d999cd",
                        "proof-intent-48940c2f9ea4a58c"
                    ],
                    "receipt_ids": [],
                    "receipts_present": false
                }]
            },
            "revision": {"digest": digest, "semantics": "merge_result", "reviewed_commit": "5137d8e5"}
        });
        fs::write(
            out.join("review/gate_outcome.json"),
            serde_json::to_vec_pretty(&gate)?,
        )?;
        let mut summary = String::new();
        render_review_efficiency_section(&mut summary, out);
        for needle in [
            "proof-red-green-b27777653387",
            "proof-intent-c763f38728d999cd",
            "proof-intent-48940c2f9ea4a58c",
            "required floor could not fit inside the remaining safe proof budget",
            "receipts: none",
            digest,
        ] {
            assert!(summary.contains(needle), "missing {needle}:\n{summary}");
        }
        assert!(!summary.contains("satisfied"), "{summary}");
        Ok(())
    }

    /// Portfolio text is untrusted: a crafted task id/reason carrying
    /// backticks, pipes, brackets, newlines, and a link/image injection
    /// must render only in escaped form, never as live Markdown.
    #[test]
    fn hostile_planner_fields_render_escaped_not_live() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::create_dir_all(out.join("review"))?;
        fs::write(out.join("review/metrics.json"), "{}")?;
        let gate = serde_json::json!({
            "schema": "ub-review.gate_outcome.v1",
            "conclusion": "pass",
            "reasons": [],
            "planner_required_proofs": {
                "denominator": "required planner proof portfolio tasks",
                "gate_required_requests": 0,
                "total": 1,
                "proven": 0,
                "unproven": 1,
                "unproven_tasks": [{
                    "task_id": "task-`x` | y",
                    "kind": "focused-red-green",
                    "status": "deferred_by_safe_wind_down",
                    "reason": "budget blew](https://evil) ![pwn](https://evil/img)\n`code` | led \\ *_#~<>+\r tail",
                    "request_ids": ["req-`z`"],
                    "receipt_ids": [],
                    "receipts_present": false
                }]
            },
            "revision": {"digest": "abc123", "semantics": "merge_result", "reviewed_commit": "c"}
        });
        fs::write(
            out.join("review/gate_outcome.json"),
            serde_json::to_vec_pretty(&gate)?,
        )?;
        let mut summary = String::new();
        render_review_efficiency_section(&mut summary, out);
        // Inline-code fields: backticks become quotes, pipes stay literal
        // inside the code span.
        assert!(summary.contains("`task-'x' | y`"), "{summary}");
        assert!(summary.contains("req-'z'"), "{summary}");
        // No raw link/image constructs survive anywhere in the task line.
        assert!(!summary.contains("](https://evil)"), "{summary}");
        assert!(!summary.contains("![pwn]"), "{summary}");
        // The reason renders backslash-escaped: brackets, parens, bang,
        // backticks, and pipes cannot form Markdown.
        assert!(
            summary.contains("budget blew\\]\\(https://evil\\)"),
            "{summary}"
        );
        assert!(summary.contains("\\!\\[pwn\\]"), "{summary}");
        assert!(summary.contains("\\`code\\` \\| led"), "{summary}");
        // The embedded newline collapsed to a space; the reason stays on
        // its single summary line.
        assert!(summary.contains("img\\) \\`code\\`"), "{summary}");
        // Every remaining escape arm renders literally: backslash, star,
        // underscore, hash, tilde, and angles; `+` stays raw mid-line and
        // the CR collapses into the single space before `tail`.
        assert!(
            summary.contains("led \\\\ \\*\\_\\#\\~\\<\\>+ tail"),
            "{summary}"
        );
        Ok(())
    }

    /// Absent accounting counters default to zero rather than failing the
    /// render: the aggregate line pins the exact `0/0` / `0` unproven text.
    #[test]
    fn planner_accounting_defaults_render_as_zero() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        fs::create_dir_all(out.join("review"))?;
        fs::write(out.join("review/metrics.json"), "{}")?;
        let gate = serde_json::json!({
            "schema": "ub-review.gate_outcome.v1",
            "conclusion": "pass",
            "reasons": [],
            "planner_required_proofs": {
                "denominator": "required planner proof portfolio tasks",
                "gate_required_requests": 0,
                "unproven_tasks": []
            },
            "revision": {"digest": "abc123", "semantics": "merge_result", "reviewed_commit": "c"}
        });
        fs::write(
            out.join("review/gate_outcome.json"),
            serde_json::to_vec_pretty(&gate)?,
        )?;
        let mut summary = String::new();
        render_review_efficiency_section(&mut summary, out);
        assert!(summary.contains("`0/0` proven (`0` unproven"), "{summary}");
        Ok(())
    }
}
