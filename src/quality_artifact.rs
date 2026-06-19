//! Quality receipt and trend artifact construction (cleanup train
//! step 39, pure code motion).

use crate::*;

pub(crate) fn write_quality_receipt_artifact(
    out: &Path,
    metrics: &ReviewMetrics,
    review: &ReviewArtifacts,
    fill_ledger: &FillLedger,
) -> Result<QualityReceipt> {
    let receipt = build_quality_receipt(metrics, review, fill_ledger);
    fs::write(
        out.join("review").join("quality-receipt.json"),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(receipt)
}

pub(crate) fn build_quality_receipt(
    metrics: &ReviewMetrics,
    review: &ReviewArtifacts,
    fill_ledger: &FillLedger,
) -> QualityReceipt {
    let fills_total = fill_ledger
        .entries
        .iter()
        .filter(|entry| entry.selected)
        .count();
    let fills_with_signal = fill_ledger
        .entries
        .iter()
        .filter(|entry| {
            entry.selected
                && entry
                    .actual_signal
                    .as_deref()
                    .is_some_and(|signal| !signal.trim().is_empty())
        })
        .count();
    let missing = vec![
        quality_missing(
            "comments_posted",
            "`run` prepares review payloads; `post` owns post-result.json after GitHub submission",
            "post-result.json",
        ),
        quality_missing(
            "comments_accepted",
            "review comment acceptance requires GitHub thread state after reviewer action",
            "github-api-review-threads",
        ),
        quality_missing(
            "comments_resolved",
            "review comment resolution requires GitHub thread state after reviewer action",
            "github-api-review-threads",
        ),
        quality_missing(
            "reviewer_overrides",
            "reviewer override classification requires post-merge or reviewer-action backfill",
            "github-api-review-threads",
        ),
        quality_missing(
            "adopted_generated_tests",
            "adopted generated-test counts are not emitted by run artifacts in v1",
            "github-api-commits",
        ),
    ];

    QualityReceipt {
        schema: QUALITY_RECEIPT_SCHEMA,
        run_id: cost_run_id(metrics),
        source_artifacts: vec![
            "review/metrics.json".to_owned(),
            "review/fill-ledger.json".to_owned(),
            "review/provider-preflight-status.json".to_owned(),
            "review/review.json".to_owned(),
            quality_review_payload_source(metrics),
        ],
        review_payload_status: metrics.review_payload_status.clone(),
        comments_prepared: metrics.github_review_comments,
        comments_posted: None,
        comments_accepted: None,
        comments_resolved: None,
        comments_off_diff_rejected: metrics.off_diff_candidates_rejected,
        fills_with_signal,
        fills_total,
        llm_unavailable_events: quality_llm_unavailable_events(review),
        fallback_used_lanes: metrics.models.model_fallbacks_used,
        reviewer_overrides: None,
        adopted_generated_tests: None,
        missing,
    }
}

pub(crate) fn quality_review_payload_source(metrics: &ReviewMetrics) -> String {
    let source = if metrics.github_review_body_bytes > 0 || metrics.github_review_comments > 0 {
        "review/github-review.json"
    } else {
        "review/github-review-skip.json"
    };
    source.to_owned()
}

pub(crate) fn write_quality_trend_artifact(out: &Path, receipt: &QualityReceipt) -> Result<()> {
    let artifact = build_quality_trend_artifact(receipt);
    fs::write(
        out.join("review").join("quality-trend.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    Ok(())
}

pub(crate) fn build_quality_trend_artifact(receipt: &QualityReceipt) -> QualityTrendArtifact {
    let mut missing = Vec::new();
    let mut source_artifacts = vec!["review/quality-receipt.json".to_owned()];
    for source in &receipt.source_artifacts {
        if !source_artifacts.contains(source) {
            source_artifacts.push(source.clone());
        }
    }

    let fills_signal_rate = if receipt.fills_total == 0 {
        missing.push(quality_trend_missing(
            "fills_signal_rate",
            "fill signal rate requires at least one selected optional fill",
            "review/quality-receipt.json",
        ));
        None
    } else {
        Some(receipt.fills_with_signal as f64 / receipt.fills_total as f64)
    };

    for field in [
        "comments_posted",
        "comment_acceptance_rate",
        "comment_resolution_rate",
        "reviewer_override_rate",
        "adopted_generated_tests",
    ] {
        missing.push(quality_trend_missing(
            field,
            "reviewer outcome telemetry requires GitHub thread state after reviewer action",
            "review/quality-receipt.json",
        ));
    }
    for field in [
        "trend.comment_acceptance_rate_delta",
        "trend.fills_signal_rate_delta",
        "trend.llm_unavailable_rate_delta",
        "trend.reviewer_override_rate_delta",
    ] {
        missing.push(quality_trend_missing(
            field,
            "historical quality receipts are required; run-completion v1 has one sample",
            "review/quality-receipt.json",
        ));
    }

    QualityTrendArtifact {
        schema: QUALITY_TREND_SCHEMA,
        run_id: receipt.run_id.clone(),
        as_of: Utc::now().date_naive().to_string(),
        window_scope: "single_run_v1",
        window_runs: 1,
        source_artifacts,
        comments_prepared: receipt.comments_prepared,
        comments_posted: receipt.comments_posted,
        comment_acceptance_rate: None,
        comment_resolution_rate: None,
        fills_signal_rate,
        llm_unavailable_rate: if receipt.llm_unavailable_events > 0 {
            1.0
        } else {
            0.0
        },
        reviewer_override_rate: None,
        adopted_generated_tests: None,
        trend: QualityTrendSummary {
            comment_acceptance_rate_delta: None,
            fills_signal_rate_delta: None,
            llm_unavailable_rate_delta: None,
            reviewer_override_rate_delta: None,
        },
        missing,
    }
}

pub(crate) fn quality_trend_missing(
    field: &str,
    reason: &str,
    source_artifact: &str,
) -> QualityTrendMissingInput {
    QualityTrendMissingInput {
        field: field.to_owned(),
        reason: reason.to_owned(),
        source_artifact: source_artifact.to_owned(),
    }
}
