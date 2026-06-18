//! Quality backfill: build quality backfill artifacts from review
//! outcomes, compute comment/bool/rate/delta metrics, and track LLM
//! unavailability (cleanup train step 28, pure code motion).

use crate::*;

pub(crate) fn build_quality_backfill_artifact(
    window_days: u32,
    runs: &[QualityBackfillRun],
    outcomes: Option<&LoadedGithubQualityOutcomes>,
    previous: Option<&LoadedPreviousQualityBackfill>,
) -> QualityBackfillArtifact {
    let mut missing = Vec::new();
    let mut source_artifacts = Vec::new();
    for run in runs {
        quality_backfill_push_source(&mut source_artifacts, run.receipt_source.clone());
        if let Some(trend_source) = &run.trend_source {
            quality_backfill_push_source(&mut source_artifacts, trend_source.clone());
        } else {
            missing.push(quality_backfill_missing(
                "source_artifacts.quality_trend",
                &format!(
                    "quality trend artifact missing for run {}; backfill kept the run receipt but trend provenance is incomplete",
                    run.receipt.run_id
                ),
                &run.receipt_source,
            ));
        }
    }
    if let Some(outcomes) = outcomes {
        quality_backfill_push_source(&mut source_artifacts, outcomes.source_artifact.clone());
        for source in &outcomes.raw_source_artifacts {
            quality_backfill_push_source(&mut source_artifacts, source.clone());
        }
    }
    if let Some(previous) = previous {
        quality_backfill_push_source(&mut source_artifacts, previous.source_artifact.clone());
    }

    let window_runs = runs.len();
    let comments_prepared = runs
        .iter()
        .map(|run| run.receipt.comments_prepared)
        .sum::<usize>();
    let fills_total = runs
        .iter()
        .map(|run| run.receipt.fills_total)
        .sum::<usize>();
    let fills_with_signal = runs
        .iter()
        .map(|run| run.receipt.fills_with_signal)
        .sum::<usize>();
    let runs_with_llm_unavailable = runs
        .iter()
        .filter(|run| run.receipt.llm_unavailable_events > 0)
        .count();
    let fills_signal_rate = if fills_total == 0 {
        missing.push(quality_backfill_missing(
            "fills_signal_rate",
            "fill signal rate requires at least one selected optional fill in the backfill window",
            "review/quality-backfill.json",
        ));
        None
    } else {
        Some(fills_with_signal as f64 / fills_total as f64)
    };
    let llm_unavailable_rate = if window_runs == 0 {
        missing.push(quality_backfill_missing(
            "llm_unavailable_rate",
            "LLM unavailable rate requires at least one quality run receipt",
            "review/quality-backfill.json",
        ));
        None
    } else {
        Some(runs_with_llm_unavailable as f64 / window_runs as f64)
    };

    let (comments_posted, comments_accepted, comments_resolved, reviewer_overrides) =
        quality_backfill_comment_counts(outcomes, &mut missing);
    let outcome_source = outcomes
        .map(|outcome| outcome.source_artifact.as_str())
        .unwrap_or("github-api-review-threads");
    let comment_acceptance_rate = quality_backfill_rate(
        comments_accepted,
        comments_posted,
        "comment_acceptance_rate",
        "comment acceptance rate requires posted comments and accepted-state receipts",
        outcome_source,
        &mut missing,
    );
    let comment_resolution_rate = quality_backfill_rate(
        comments_resolved,
        comments_posted,
        "comment_resolution_rate",
        "comment resolution rate requires posted comments and resolved-state receipts",
        outcome_source,
        &mut missing,
    );
    let reviewer_override_rate = quality_backfill_rate(
        reviewer_overrides,
        comments_posted,
        "reviewer_override_rate",
        "reviewer override rate requires posted comments and reviewer override receipts",
        outcome_source,
        &mut missing,
    );
    let adopted_generated_tests = match outcomes {
        Some(outcome) if outcome.has_adopted_generated_tests => {
            Some(outcome.outcomes.adopted_generated_tests.len())
        }
        Some(outcome) => {
            missing.push(quality_backfill_missing(
                "adopted_generated_tests",
                "GitHub commit receipt did not include adopted_generated_tests",
                &outcome.source_artifact,
            ));
            None
        }
        None => {
            missing.push(quality_backfill_missing(
                "adopted_generated_tests",
                "adopted generated-test counts require GitHub commit backfill receipts",
                "github-api-commits",
            ));
            None
        }
    };

    let trend = QualityBackfillTrendSummary {
        comment_acceptance_rate_delta: quality_backfill_delta(
            "trend.comment_acceptance_rate_delta",
            comment_acceptance_rate,
            previous.and_then(|previous| previous.artifact.comment_acceptance_rate),
            previous.map(|previous| previous.source_artifact.as_str()),
            &mut missing,
        ),
        fills_signal_rate_delta: quality_backfill_delta(
            "trend.fills_signal_rate_delta",
            fills_signal_rate,
            previous.and_then(|previous| previous.artifact.fills_signal_rate),
            previous.map(|previous| previous.source_artifact.as_str()),
            &mut missing,
        ),
        llm_unavailable_rate_delta: quality_backfill_delta(
            "trend.llm_unavailable_rate_delta",
            llm_unavailable_rate,
            previous.and_then(|previous| previous.artifact.llm_unavailable_rate),
            previous.map(|previous| previous.source_artifact.as_str()),
            &mut missing,
        ),
        reviewer_override_rate_delta: quality_backfill_delta(
            "trend.reviewer_override_rate_delta",
            reviewer_override_rate,
            previous.and_then(|previous| previous.artifact.reviewer_override_rate),
            previous.map(|previous| previous.source_artifact.as_str()),
            &mut missing,
        ),
    };

    QualityBackfillArtifact {
        schema: QUALITY_BACKFILL_SCHEMA,
        as_of: Utc::now().date_naive().to_string(),
        window_scope: "rolling_v1",
        window_days,
        window_runs,
        source_artifacts,
        comments_prepared,
        comments_posted,
        comments_accepted,
        comments_resolved,
        comment_acceptance_rate,
        comment_resolution_rate,
        fills_signal_rate,
        llm_unavailable_rate,
        reviewer_overrides,
        reviewer_override_rate,
        adopted_generated_tests,
        trend,
        missing,
    }
}

pub(crate) fn quality_backfill_comment_counts(
    outcomes: Option<&LoadedGithubQualityOutcomes>,
    missing: &mut Vec<QualityBackfillMissingInput>,
) -> (Option<usize>, Option<usize>, Option<usize>, Option<usize>) {
    let Some(outcome) = outcomes else {
        for field in [
            "comments_posted",
            "comments_accepted",
            "comments_resolved",
            "reviewer_overrides",
        ] {
            missing.push(quality_backfill_missing(
                field,
                "reviewer outcome telemetry requires GitHub review-thread receipts",
                "github-api-review-threads",
            ));
        }
        return (None, None, None, None);
    };
    if !outcome.has_comments {
        for field in [
            "comments_posted",
            "comments_accepted",
            "comments_resolved",
            "reviewer_overrides",
        ] {
            missing.push(quality_backfill_missing(
                field,
                "GitHub outcome receipt did not include comments[]",
                &outcome.source_artifact,
            ));
        }
        return (None, None, None, None);
    }

    let posted_comments: Vec<&GithubQualityCommentOutcome> = outcome
        .outcomes
        .comments
        .iter()
        .filter(|comment| comment.posted.unwrap_or(true))
        .collect();
    let comments_posted = Some(posted_comments.len());
    let comments_accepted = quality_backfill_bool_count(
        &posted_comments,
        |comment| comment.accepted,
        "comments_accepted",
        "accepted-state receipts are missing for at least one posted comment",
        &outcome.source_artifact,
        missing,
    );
    let comments_resolved = quality_backfill_bool_count(
        &posted_comments,
        |comment| comment.resolved,
        "comments_resolved",
        "resolved-state receipts are missing for at least one posted comment",
        &outcome.source_artifact,
        missing,
    );
    let reviewer_overrides = quality_backfill_bool_count(
        &posted_comments,
        |comment| comment.reviewer_override,
        "reviewer_overrides",
        "reviewer override receipts are missing for at least one posted comment",
        &outcome.source_artifact,
        missing,
    );
    (
        comments_posted,
        comments_accepted,
        comments_resolved,
        reviewer_overrides,
    )
}

pub(crate) fn quality_backfill_bool_count<F>(
    comments: &[&GithubQualityCommentOutcome],
    value: F,
    field: &str,
    reason: &str,
    source_artifact: &str,
    missing: &mut Vec<QualityBackfillMissingInput>,
) -> Option<usize>
where
    F: Fn(&GithubQualityCommentOutcome) -> Option<bool>,
{
    if comments.iter().any(|comment| value(comment).is_none()) {
        missing.push(quality_backfill_missing(field, reason, source_artifact));
        return None;
    }
    Some(
        comments
            .iter()
            .filter(|comment| value(comment).unwrap_or(false))
            .count(),
    )
}

pub(crate) fn quality_backfill_rate(
    numerator: Option<usize>,
    denominator: Option<usize>,
    field: &str,
    reason: &str,
    source_artifact: &str,
    missing: &mut Vec<QualityBackfillMissingInput>,
) -> Option<f64> {
    let Some(numerator) = numerator else {
        missing.push(quality_backfill_missing(field, reason, source_artifact));
        return None;
    };
    let Some(denominator) = denominator else {
        missing.push(quality_backfill_missing(field, reason, source_artifact));
        return None;
    };
    if denominator == 0 {
        missing.push(quality_backfill_missing(field, reason, source_artifact));
        return None;
    }
    Some(numerator as f64 / denominator as f64)
}

pub(crate) fn quality_backfill_delta(
    field: &str,
    current: Option<f64>,
    previous: Option<f64>,
    previous_source_artifact: Option<&str>,
    missing: &mut Vec<QualityBackfillMissingInput>,
) -> Option<f64> {
    let source_artifact = previous_source_artifact.unwrap_or("previous-quality-backfill.json");
    match (current, previous) {
        (Some(current), Some(previous)) => Some(current - previous),
        _ => {
            missing.push(quality_backfill_missing(
                field,
                "trend delta requires current and previous backfill values",
                source_artifact,
            ));
            None
        }
    }
}

pub(crate) fn copy_quality_backfill_source(
    out: &Path,
    source: &Path,
    label: &str,
) -> Result<String> {
    let bytes = fs::read(source).with_context(|| format!("read {}", source.display()))?;
    let digest = sha256_hex(&bytes);
    let file_name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("source.json");
    let stem = sanitize_artifact_name(&format!("{label}-{file_name}"));
    let relative = format!(
        "review/quality-backfill-sources/{}-{}.json",
        stem,
        &digest[..16]
    );
    let destination = out.join(&relative);
    let parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("quality backfill destination has no parent"))?;
    fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    fs::write(&destination, bytes).with_context(|| format!("write {}", destination.display()))?;
    Ok(relative)
}

pub(crate) fn quality_backfill_push_source(sources: &mut Vec<String>, source: String) {
    if !sources.contains(&source) {
        sources.push(source);
    }
}

pub(crate) fn quality_backfill_missing(
    field: &str,
    reason: &str,
    source_artifact: &str,
) -> QualityBackfillMissingInput {
    QualityBackfillMissingInput {
        field: field.to_owned(),
        reason: reason.to_owned(),
        source_artifact: source_artifact.to_owned(),
    }
}

pub(crate) fn read_json_file<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))
}

pub(crate) fn quality_llm_unavailable_events(review: &ReviewArtifacts) -> usize {
    let provider_events = review
        .provider_preflights
        .iter()
        .filter(|receipt| quality_llm_unavailable_status(&receipt.status, receipt.http_status))
        .count();
    let lane_events = review
        .model_lanes
        .iter()
        .filter(|receipt| quality_llm_unavailable_status(&receipt.status, receipt.http_status))
        .count();
    let fallback_events = review
        .model_lanes
        .iter()
        .filter(|receipt| receipt.fallback_from.is_some())
        .count();
    provider_events + lane_events + fallback_events
}

pub(crate) fn quality_llm_unavailable_status(status: &str, http_status: Option<u16>) -> bool {
    matches!(
        status,
        "missing_key" | "preflight_failed" | "auth_failed" | "rate_limited" | "timed_out"
    ) || matches!(http_status, Some(500..=599))
}

pub(crate) fn quality_missing(
    field: &str,
    reason: &str,
    source_artifact: &str,
) -> QualityMissingInput {
    QualityMissingInput {
        field: field.to_owned(),
        reason: reason.to_owned(),
        source_artifact: source_artifact.to_owned(),
    }
}
