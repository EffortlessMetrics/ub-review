//! Fill ledger: sensor/proof fill entries, expected/actual signal
//! mapping, cost computation (runner, tokens, cache, floor seconds),
//! and prompt cache metrics (cleanup train step 29, pure code motion).

use crate::test_parse::push_unique;
use crate::*;

pub(crate) fn fill_sensor_entry(
    out: &Path,
    sensor: &SensorPlan,
    tool_gate_outcomes: &[ToolGateOutcomeEntry],
    gate_outcome: &GateOutcome,
) -> FillLedgerEntry {
    let receipt_path = format!("sensors/{}/ub-review-sensor-status.json", sensor.id);
    let receipt_exists = out.join(&receipt_path).is_file();
    let receipt = read_sensor_receipt(&out.join(&receipt_path));
    let actual_signal = if sensor.run {
        Some(
            receipt
                .as_ref()
                .map(|receipt| format!("{}: {}", receipt.status, receipt.reason))
                .unwrap_or_else(|| "planned sensor did not produce a status receipt".to_owned()),
        )
    } else {
        None
    };
    let time_spent_sec = receipt
        .as_ref()
        .and_then(|receipt| receipt.duration_ms)
        .map(|duration_ms| round_f64(duration_ms as f64 / 1000.0, 3))
        .unwrap_or(0.0);
    let artifact_path = if receipt_exists {
        Some(receipt_path.clone())
    } else {
        None
    };
    let mut source_artifacts = vec!["work_queue.json".to_owned(), "tool-status.json".to_owned()];
    if receipt_exists {
        source_artifacts.push(receipt_path.clone());
    }
    if tool_gate_outcomes
        .iter()
        .any(|outcome| outcome.tool == sensor.id)
    {
        source_artifacts.push("review/tool-gate-outcomes.json".to_owned());
    }

    FillLedgerEntry {
        check_id: sensor.id.clone(),
        kind: "sensor".to_owned(),
        selected: sensor.run,
        selection_reason: sensor.reason.clone(),
        cost: sensor_fill_cost(sensor).to_owned(),
        expected_signal: Some(sensor_expected_signal(&sensor.id).to_owned()),
        actual_signal,
        time_spent_sec,
        artifact_path,
        affected_merge: sensor
            .run
            .then(|| sensor_fill_entry_affected_merge(&sensor.id, gate_outcome)),
        source_artifacts,
    }
}

pub(crate) fn fill_proof_request_entry(
    request: &ProofRequest,
    proof_tasks: &[ProofTaskArtifact],
    proof_receipts: &[ProofReceipt],
    resource_leases: &[ResourceLease],
    gate_outcome: &GateOutcome,
) -> FillLedgerEntry {
    let selected = request.status == "requested";
    let task = proof_tasks.iter().find(|task| {
        task.request_ids
            .iter()
            .any(|request_id| request_id == &request.id)
    });
    let receipts = proof_receipts
        .iter()
        .filter(|receipt| {
            receipt
                .request_ids
                .iter()
                .any(|request_id| request_id == &request.id)
        })
        .collect::<Vec<_>>();
    let time_spent_ms = receipts
        .iter()
        .flat_map(|receipt| receipt.commands.iter())
        .map(|command| command.duration_ms)
        .sum::<u128>();
    let artifact_path = receipts
        .first()
        .map(|receipt| format!("review/proof_receipts.json#{}", receipt.id));
    let mut source_artifacts = vec![
        "review/proof_requests.json".to_owned(),
        "review/proof_planner_output.json".to_owned(),
    ];
    if !receipts.is_empty() {
        source_artifacts.push("review/proof_receipts.json".to_owned());
        for receipt in &receipts {
            for lease in resource_leases
                .iter()
                .filter(|lease| lease.consumer == receipt.id)
            {
                push_unique(
                    &mut source_artifacts,
                    &format!("review/resource_leases.json#{}", lease.id),
                );
            }
        }
    }

    FillLedgerEntry {
        check_id: request.id.clone(),
        kind: "proof-request".to_owned(),
        selected,
        selection_reason: request.reason.clone(),
        cost: request.cost.clone(),
        expected_signal: Some(proof_request_expected_signal(request, task)),
        actual_signal: proof_request_actual_signal(selected, &receipts),
        time_spent_sec: round_f64(time_spent_ms as f64 / 1000.0, 3),
        artifact_path,
        affected_merge: selected.then(|| {
            receipts.iter().any(|receipt| {
                gate_references_artifact(
                    gate_outcome,
                    &format!("review/proof_receipts.json#{}", receipt.id),
                )
            })
        }),
        source_artifacts,
    }
}

pub(crate) fn fill_proof_planner_skip_entry(skip: ProofPlannerSkip) -> FillLedgerEntry {
    let expected_signal = proof_skip_expected_signal(&skip.kind).map(str::to_owned);
    let cost = skip.kind.clone();
    FillLedgerEntry {
        check_id: skip.kind,
        kind: "proof-skip".to_owned(),
        selected: false,
        selection_reason: skip.reason,
        cost,
        expected_signal,
        actual_signal: None,
        time_spent_sec: 0.0,
        artifact_path: None,
        affected_merge: None,
        source_artifacts: vec!["review/proof_planner_output.json".to_owned()],
    }
}

pub(crate) fn sensor_fill_cost(sensor: &SensorPlan) -> &'static str {
    match sensor.class {
        ToolClass::Packet => "packet",
        ToolClass::Static => "static",
        ToolClass::Search => "search",
        ToolClass::Workflow => "workflow",
        ToolClass::Security => "security",
        ToolClass::Coverage => "coverage",
        ToolClass::Test => "test",
        ToolClass::Build => "build",
        ToolClass::HeavyWitness => "heavy-witness",
    }
}

pub(crate) fn proof_skip_expected_signal(skip_kind: &str) -> Option<&'static str> {
    match skip_kind {
        "miri" => Some("Rust UB witness for unsafe/native execution paths"),
        "mutation" => Some("runtime mutation check for targeted test oracle strength"),
        "sanitizer" => Some("sanitizer runtime witness for memory-safety regressions"),
        "actionlint" => Some("workflow syntax and action composition signal"),
        _ => None,
    }
}

pub(crate) fn sensor_expected_signal(sensor_id: &str) -> &'static str {
    match sensor_id {
        "ripr" => "static mutation-exposure signal for changed behavior and tests",
        "unsafe-review" => "unsafe/native reviewability and coverage signal",
        "coverage" => "changed-line execution coverage telemetry",
        "actionlint" => "workflow syntax and action composition signal",
        "cargo-allow" => "source-tree exception policy receipt",
        "tokmd" => "structured code context packet for model lanes",
        "ast-grep" => "syntax-pattern evidence for changed source routes",
        _ => "optional sensor evidence for review and compiler context",
    }
}

pub(crate) fn sensor_fill_entry_affected_merge(
    sensor_id: &str,
    gate_outcome: &GateOutcome,
) -> bool {
    gate_outcome.reasons.iter().any(|reason| {
        reason.id == sensor_id
            || gate_references_artifact(
                gate_outcome,
                &format!("sensors/{sensor_id}/ub-review-sensor-status.json"),
            )
            || gate_references_artifact(
                gate_outcome,
                &format!("review/tool-gate-outcomes.json#{sensor_id}"),
            )
    })
}

pub(crate) fn proof_request_expected_signal(
    request: &ProofRequest,
    task: Option<&ProofTaskArtifact>,
) -> String {
    task.map(|task| format!("{} {}", task.value, task.purpose))
        .unwrap_or_else(|| {
            format!(
                "{} proof request `{}` with status `{}`",
                request.cost, request.command, request.status
            )
        })
}

pub(crate) fn proof_request_actual_signal(
    selected: bool,
    receipts: &[&ProofReceipt],
) -> Option<String> {
    if receipts.is_empty() {
        return selected.then(|| "no proof receipt matched request".to_owned());
    }
    if receipts.len() == 1 {
        let receipt = receipts[0];
        return Some(format!("{}: {}", receipt.result, receipt.reason));
    }
    let summary = receipts
        .iter()
        .map(|receipt| format!("{}={}", receipt.id, receipt.result))
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!("{} proof receipts: {summary}", receipts.len()))
}

pub(crate) fn gate_references_artifact(gate_outcome: &GateOutcome, artifact: &str) -> bool {
    gate_outcome.reasons.iter().any(|reason| {
        reason.receipt == artifact
            || (!artifact.contains('#')
                && reason
                    .receipt
                    .strip_prefix(artifact)
                    .is_some_and(|suffix| suffix.starts_with('#')))
    })
}

pub(crate) fn cost_run_id(metrics: &ReviewMetrics) -> String {
    env_value_present("GITHUB_RUN_ID")
        .then(|| std::env::var("GITHUB_RUN_ID").ok())
        .flatten()
        .unwrap_or_else(|| {
            let short = metrics
                .shared_context_id
                .get(..12)
                .unwrap_or(&metrics.shared_context_id);
            format!("local-{short}")
        })
}

pub(crate) fn runner_kind() -> String {
    if !env_value_present("GITHUB_ACTIONS") {
        return "local".to_owned();
    }
    match std::env::var("RUNNER_ENVIRONMENT")
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "github-hosted" => "github-hosted".to_owned(),
        "self-hosted" => "self-hosted".to_owned(),
        _ => "unknown".to_owned(),
    }
}

pub(crate) fn unsafe_review_required_floor_seconds(
    out: &Path,
) -> (Option<f64>, Option<CostMissingInput>) {
    let source_artifact = "sensors/unsafe-review/unsafe-review-output/unsafe-review-gate.json";
    match read_unsafe_review_artifacts(&out.join("sensors").join("unsafe-review")) {
        Ok(artifacts) => match artifacts.gate.required_floor_wall_seconds {
            Some(seconds) if seconds.is_finite() && seconds >= 0.0 => {
                (Some(round_f64(seconds, 3)), None)
            }
            Some(_) => (
                None,
                Some(cost_missing(
                    "required_floor_wall_seconds",
                    "unsafe-review-gate.json required_floor_wall_seconds was not finite and non-negative",
                    source_artifact,
                )),
            ),
            None => (
                None,
                Some(cost_missing(
                    "required_floor_wall_seconds",
                    "unsafe-review-gate.json did not include required_floor_wall_seconds",
                    source_artifact,
                )),
            ),
        },
        Err(gap) => (
            None,
            Some(cost_missing(
                "required_floor_wall_seconds",
                &gap.reason(),
                source_artifact,
            )),
        ),
    }
}

pub(crate) fn linux_minute_rate_usd(root: &Path) -> (Option<f64>, Option<CostMissingInput>) {
    let source_artifact = "policy/ci-budget.toml";
    let path = root.join(source_artifact);
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return (
                None,
                Some(cost_missing(
                    "cost_basis.linux_minute_rate_usd",
                    "policy/ci-budget.toml absent",
                    source_artifact,
                )),
            );
        }
        Err(err) => {
            return (
                None,
                Some(cost_missing(
                    "cost_basis.linux_minute_rate_usd",
                    &format!("policy/ci-budget.toml unreadable: {err}"),
                    source_artifact,
                )),
            );
        }
    };
    let parsed = match toml::from_str::<toml::Value>(&text) {
        Ok(parsed) => parsed,
        Err(err) => {
            return (
                None,
                Some(cost_missing(
                    "cost_basis.linux_minute_rate_usd",
                    &format!("policy/ci-budget.toml malformed: {err}"),
                    source_artifact,
                )),
            );
        }
    };
    let Some(value) = parsed
        .get("budget")
        .and_then(|budget| budget.get("linux_minute_rate_usd"))
    else {
        return (
            None,
            Some(cost_missing(
                "cost_basis.linux_minute_rate_usd",
                "budget.linux_minute_rate_usd missing",
                source_artifact,
            )),
        );
    };
    let rate = value
        .as_float()
        .or_else(|| value.as_integer().map(|number| number as f64));
    match rate {
        Some(rate) if rate.is_finite() && rate >= 0.0 => (Some(rate), None),
        _ => (
            None,
            Some(cost_missing(
                "cost_basis.linux_minute_rate_usd",
                "budget.linux_minute_rate_usd is not a finite non-negative number",
                source_artifact,
            )),
        ),
    }
}

pub(crate) fn cost_tokens(
    review: &ReviewArtifacts,
    follow_up_results: &[FollowUpResult],
) -> CostTokenReceipt {
    let mut tokens = CostTokenReceipt::default();
    for usage in review
        .provider_preflights
        .iter()
        .map(|receipt| &receipt.cache_usage)
        .chain(
            review
                .model_lanes
                .iter()
                .map(|receipt| &receipt.cache_usage),
        )
        .chain(follow_up_results.iter().map(|result| &result.cache_usage))
    {
        let cached = usage.cache_read_input_tokens.unwrap_or(0);
        tokens.cached_input = tokens.cached_input.saturating_add(cached);
        tokens.fresh_input = tokens
            .fresh_input
            .saturating_add(usage.input_tokens.unwrap_or(0).saturating_sub(cached));
        tokens.output = tokens
            .output
            .saturating_add(usage.output_tokens.unwrap_or(0));
    }
    tokens
}

pub(crate) fn model_prefix_cache_status(models: &ModelMetrics) -> &'static str {
    match (
        models.prompt_cache_lane_hits,
        models.prompt_cache_lane_misses,
        models.prompt_cache_lane_unknown,
    ) {
        (0, 0, 0) => "unknown",
        (hits, 0, 0) if hits > 0 => "hit",
        (0, misses, 0) if misses > 0 => "miss",
        (0, 0, unknown) if unknown > 0 => "unknown",
        _ => "partial",
    }
}

pub(crate) fn cost_missing(field: &str, reason: &str, source_artifact: &str) -> CostMissingInput {
    CostMissingInput {
        field: field.to_owned(),
        reason: reason.to_owned(),
        source_artifact: source_artifact.to_owned(),
    }
}

pub(crate) fn round_f64(value: f64, places: i32) -> f64 {
    let scale = 10_f64.powi(places);
    (value * scale).round() / scale
}

pub(crate) fn write_scheduler_artifact(review_dir: &Path, run: &RunLoopMetrics) -> Result<()> {
    let artifact = SchedulerArtifact {
        schema: SCHEDULER_SCHEMA,
        concurrency_model: &run.concurrency_model,
        scheduler_profile: &run.scheduler_profile,
        local_proof_wall_excludes_model_wait: run.local_proof_wall_excludes_model_wait,
        elapsed_wall_ms: run.elapsed_wall_ms,
        scheduler_roles: &run.scheduler_roles,
        streams: &run.streams,
        loops: &run.loops,
        overlaps: SchedulerOverlapArtifact {
            investigation_proof_overlap_ms: run.investigation_proof_overlap_ms,
            model_proof_overlap_ms: run.model_proof_overlap_ms,
            proof_overlap_ms: run.proof_overlap_ms,
        },
        phases: &run.phases,
    };
    fs::write(
        review_dir.join("scheduler.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    Ok(())
}

pub(crate) fn model_call_duration_ms_sum(
    review: &ReviewArtifacts,
    follow_up_results: &[FollowUpResult],
) -> u128 {
    review
        .provider_preflights
        .iter()
        .filter_map(|receipt| receipt.duration_ms)
        .chain(
            review
                .model_lanes
                .iter()
                .filter_map(|receipt| receipt.duration_ms),
        )
        .chain(
            follow_up_results
                .iter()
                .filter_map(|result| result.duration_ms),
        )
        .sum()
}

pub(crate) fn proof_command_duration_ms_sum(proof_receipts: &[ProofReceipt]) -> u128 {
    proof_receipts
        .iter()
        .flat_map(|receipt| receipt.commands.iter())
        .map(|command| command.duration_ms)
        .sum()
}

pub(crate) struct PromptCacheMetrics {
    pub(crate) creation_input_tokens: u64,
    pub(crate) read_input_tokens: u64,
    pub(crate) lane_hits: usize,
    pub(crate) lane_misses: usize,
    pub(crate) lane_unknown: usize,
}

pub(crate) fn model_prompt_cache_metrics(
    review: &ReviewArtifacts,
    follow_up_results: &[FollowUpResult],
    args: &RunArgs,
) -> PromptCacheMetrics {
    let creation_input_tokens = review
        .provider_preflights
        .iter()
        .filter_map(|receipt| receipt.cache_usage.cache_creation_input_tokens)
        .chain(
            review
                .model_lanes
                .iter()
                .filter_map(|receipt| receipt.cache_usage.cache_creation_input_tokens),
        )
        .chain(
            follow_up_results
                .iter()
                .filter_map(|result| result.cache_usage.cache_creation_input_tokens),
        )
        .sum();
    let read_input_tokens = review
        .provider_preflights
        .iter()
        .filter_map(|receipt| receipt.cache_usage.cache_read_input_tokens)
        .chain(
            review
                .model_lanes
                .iter()
                .filter_map(|receipt| receipt.cache_usage.cache_read_input_tokens),
        )
        .chain(
            follow_up_results
                .iter()
                .filter_map(|result| result.cache_usage.cache_read_input_tokens),
        )
        .sum();
    let mut lane_hits = 0;
    let mut lane_misses = 0;
    let mut lane_unknown = 0;
    for receipt in &review.model_lanes {
        if !model_call_attempted_status(&receipt.status)
            || model_cache_mode_for_args(args, &receipt.provider, &receipt.endpoint_kind)
                != "explicit-anthropic-cache-control"
        {
            continue;
        }
        if receipt.cache_usage.cache_read_input_tokens.unwrap_or(0) > 0 {
            lane_hits += 1;
        } else if receipt.cache_usage.cache_creation_input_tokens.unwrap_or(0) > 0 {
            lane_misses += 1;
        } else {
            lane_unknown += 1;
        }
    }
    PromptCacheMetrics {
        creation_input_tokens,
        read_input_tokens,
        lane_hits,
        lane_misses,
        lane_unknown,
    }
}
