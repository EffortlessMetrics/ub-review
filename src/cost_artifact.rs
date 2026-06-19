//! Cost receipt, floor-trend, and cache-hit-rate artifact construction
//! (cleanup train step 38, pure code motion).

use crate::*;

pub(crate) fn write_cost_receipt_artifact(
    root: &Path,
    out: &Path,
    config: &Config,
    metrics: &ReviewMetrics,
    review: &ReviewArtifacts,
    follow_up_results: &[FollowUpResult],
) -> Result<CostReceipt> {
    let receipt = build_cost_receipt(root, out, config, metrics, review, follow_up_results);
    fs::write(
        out.join("review").join("ub-review-cost.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}

pub(crate) fn build_cost_receipt(
    root: &Path,
    out: &Path,
    config: &Config,
    metrics: &ReviewMetrics,
    review: &ReviewArtifacts,
    follow_up_results: &[FollowUpResult],
) -> CostReceipt {
    let mut missing = Vec::new();
    let (required_floor_wall_seconds, floor_missing) = unsafe_review_required_floor_seconds(out);
    if let Some(floor_missing) = floor_missing {
        missing.push(floor_missing);
    }
    let (linux_minute_rate_usd, rate_missing) = linux_minute_rate_usd(root);
    if let Some(rate_missing) = rate_missing {
        missing.push(rate_missing);
    }
    missing.push(cost_missing(
        "cache.cargo",
        "cargo cache telemetry is not emitted in cost receipt v1",
        "review/metrics.json",
    ));

    let runner_minutes = round_f64(metrics.run.elapsed_wall_ms as f64 / 60_000.0, 6);
    let estimated_cost_usd = linux_minute_rate_usd.map(|rate| round_f64(runner_minutes * rate, 6));
    if estimated_cost_usd.is_none() {
        missing.push(cost_missing(
            "estimated_cost_usd",
            "policy/ci-budget.toml budget.linux_minute_rate_usd is unavailable",
            "policy/ci-budget.toml",
        ));
    }

    CostReceipt {
        schema: COST_RECEIPT_SCHEMA,
        run_id: cost_run_id(metrics),
        runner_kind: runner_kind(),
        target_minutes: config.gate.target_minutes,
        cap_minutes: config.gate.hard_timeout_minutes,
        fallback_used: review
            .model_lanes
            .iter()
            .any(|receipt| receipt.fallback_from.is_some()),
        required_floor_wall_seconds,
        llm_seconds: round_f64(metrics.run.model_call_duration_ms_sum as f64 / 1000.0, 3),
        cache: CostCacheReceipt {
            cargo: "unknown".to_owned(),
            model_prefix: model_prefix_cache_status(&metrics.models).to_owned(),
        },
        tokens: cost_tokens(review, follow_up_results),
        estimated_cost_usd,
        cost_basis: CostBasisReceipt {
            runner_minutes,
            linux_minute_rate_usd,
            token_pricing: "excluded_v1".to_owned(),
        },
        source_artifacts: vec![
            "review/metrics.json".to_owned(),
            "review/provider-preflight-status.json".to_owned(),
            "review/model_stages.json".to_owned(),
            "review/follow_up_results.json".to_owned(),
            "policy/ci-budget.toml".to_owned(),
            "sensors/unsafe-review/unsafe-review-output/unsafe-review-gate.json".to_owned(),
        ],
        missing,
    }
}

pub(crate) fn write_floor_trend_artifact(out: &Path, cost: &CostReceipt) -> Result<()> {
    let artifact = build_floor_trend_artifact(cost);
    fs::write(
        out.join("review").join("floor-trend.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    Ok(())
}

pub(crate) fn build_floor_trend_artifact(cost: &CostReceipt) -> FloorTrendArtifact {
    let mut missing = Vec::new();
    let floor = cost.required_floor_wall_seconds;
    let cargo_cache_hit_rate = cache_hit_rate(&cost.cache.cargo);
    let model_prefix_cache_hit_rate = cache_hit_rate(&cost.cache.model_prefix);
    let floor_budget_pressure_detected = floor.map(|seconds| {
        let half_target_window_seconds = cost.target_minutes as f64 * 60.0 / 2.0;
        seconds > half_target_window_seconds
    });

    if floor.is_none() {
        missing.push(floor_trend_missing(
            "releases[].floor_wall_seconds_p50",
            "ub-review-cost.json required_floor_wall_seconds is unavailable",
            "review/ub-review-cost.json",
        ));
        missing.push(floor_trend_missing(
            "releases[].floor_wall_seconds_p95",
            "ub-review-cost.json required_floor_wall_seconds is unavailable",
            "review/ub-review-cost.json",
        ));
        missing.push(floor_trend_missing(
            "trend.floor_budget_pressure_detected",
            "floor budget pressure requires required_floor_wall_seconds",
            "review/ub-review-cost.json",
        ));
    }
    if cargo_cache_hit_rate.is_none() {
        missing.push(floor_trend_missing(
            "releases[].cargo_cache_hit_rate",
            "cargo cache status is unknown in cost receipt v1",
            "review/ub-review-cost.json",
        ));
    }
    if model_prefix_cache_hit_rate.is_none() {
        missing.push(floor_trend_missing(
            "releases[].model_prefix_cache_hit_rate",
            "model prefix cache status is unknown",
            "review/ub-review-cost.json",
        ));
    }
    if cost.estimated_cost_usd.is_none() {
        missing.push(floor_trend_missing(
            "releases[].avg_cost_usd",
            "ub-review-cost.json estimated_cost_usd is unavailable",
            "review/ub-review-cost.json",
        ));
    }
    for field in [
        "trend.floor_creep_detected",
        "trend.cache_hit_rate_delta",
        "trend.avg_cost_delta_usd",
    ] {
        missing.push(floor_trend_missing(
            field,
            "historical run artifacts are required; run-completion v1 has one sample",
            "review/ub-review-cost.json",
        ));
    }

    FloorTrendArtifact {
        schema: FLOOR_TREND_SCHEMA,
        run_id: cost.run_id.clone(),
        as_of: Utc::now().date_naive().to_string(),
        window_scope: "single_run_v1",
        window_runs: 1,
        source_artifacts: vec![
            "review/ub-review-cost.json".to_owned(),
            "review/metrics.json".to_owned(),
            "policy/ci-budget.toml".to_owned(),
        ],
        releases: vec![FloorTrendRelease {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            sample_runs: 1,
            floor_wall_seconds_p50: floor,
            floor_wall_seconds_p95: floor,
            cargo_cache_hit_rate,
            model_prefix_cache_hit_rate,
            fallback_used_rate: if cost.fallback_used { 1.0 } else { 0.0 },
            avg_cost_usd: cost.estimated_cost_usd,
        }],
        trend: FloorTrendSummary {
            floor_creep_detected: None,
            floor_budget_pressure_detected,
            cache_hit_rate_delta: None,
            avg_cost_delta_usd: None,
        },
        missing,
    }
}

pub(crate) fn cache_hit_rate(status: &str) -> Option<f64> {
    match status {
        "hit" => Some(1.0),
        "partial" => Some(0.5),
        "miss" => Some(0.0),
        _ => None,
    }
}

pub(crate) fn floor_trend_missing(
    field: &str,
    reason: &str,
    source_artifact: &str,
) -> FloorTrendMissingInput {
    FloorTrendMissingInput {
        field: field.to_owned(),
        reason: reason.to_owned(),
        source_artifact: source_artifact.to_owned(),
    }
}
