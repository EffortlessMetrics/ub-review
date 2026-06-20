//! Post command: issue broker execution, post error receipts,
//! GitHub review metadata, and diff coverage (cleanup train step 58,
//! pure code motion).

use crate::*;

pub(crate) fn run_issue_broker_step(args: &PostArgs) {
    let plan_path = args
        .review_json
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("issue_broker_plan.json");
    if !plan_path.exists() {
        return;
    }
    match execute_issue_broker(args, &plan_path) {
        Ok(results) => {
            if let Err(err) = write_issue_broker_results(&args.out, &results) {
                eprintln!("ub-review issue broker: failed to write results: {err:#}");
            } else {
                let opened = results.iter().filter(|r| r.action == "opened").count();
                let duplicates = results.iter().filter(|r| r.action == "duplicate").count();
                let failed = results
                    .iter()
                    .filter(|r| r.action == "failed_to_open")
                    .count();
                println!(
                    "issue broker: {opened} opened, {duplicates} duplicate, {failed} failed, \
                     {} skipped; wrote {}/issue_broker_results.json",
                    results.iter().filter(|r| r.action == "skipped").count(),
                    args.out.display()
                );
            }
        }
        Err(err) => {
            eprintln!("ub-review issue broker failed (tolerated): {err:#}");
        }
    }
}

/// Execute every plan entry. Skips mirror through as `skipped`; attempts run
/// the fingerprint duplicate search first and only open when the search
/// comes back empty. Per-entry failures become `failed_to_open` results, so
/// one bad target repo cannot abort the rest of the plan.
pub(crate) fn execute_issue_broker(
    args: &PostArgs,
    plan_path: &Path,
) -> Result<Vec<IssueBrokerResult>> {
    let plan: Vec<IssueBrokerPlanEntry> = serde_json::from_slice(
        &fs::read(plan_path).with_context(|| format!("read {}", plan_path.display()))?,
    )
    .with_context(|| format!("parse {}", plan_path.display()))?;
    let token = args
        .github_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let api_url = args.github_api_url.trim_end_matches('/');
    let mut results = Vec::new();
    for (index, entry) in plan.iter().enumerate() {
        let result = if entry.decision != "attempt" {
            IssueBrokerResult {
                schema: ISSUE_BROKER_RESULT_SCHEMA.to_owned(),
                candidate_id: entry.candidate_id.clone(),
                target_repo: entry.target_repo.clone(),
                action: "skipped".to_owned(),
                reason: entry.reason.clone(),
                url: None,
                error: None,
            }
        } else if let Some(token) = token {
            execute_issue_broker_attempt(args, api_url, token, entry, index)
        } else {
            IssueBrokerResult {
                schema: ISSUE_BROKER_RESULT_SCHEMA.to_owned(),
                candidate_id: entry.candidate_id.clone(),
                target_repo: entry.target_repo.clone(),
                action: "failed_to_open".to_owned(),
                reason: "broker attempt planned but no GitHub token was available at post time"
                    .to_owned(),
                url: None,
                error: Some("github token unavailable".to_owned()),
            }
        };
        results.push(result);
    }
    Ok(results)
}

/// One open attempt: fingerprint duplicate search, then create. Every
/// outcome is a result row, never an error.
pub(crate) fn execute_issue_broker_attempt(
    args: &PostArgs,
    api_url: &str,
    token: &str,
    entry: &IssueBrokerPlanEntry,
    index: usize,
) -> IssueBrokerResult {
    let mut result = IssueBrokerResult {
        schema: ISSUE_BROKER_RESULT_SCHEMA.to_owned(),
        candidate_id: entry.candidate_id.clone(),
        target_repo: entry.target_repo.clone(),
        action: "failed_to_open".to_owned(),
        reason: String::new(),
        url: None,
        error: None,
    };
    let marker = issue_broker_fingerprint_marker(&entry.fingerprint);
    let query = format!("repo:{} in:body \"{marker}\"", entry.target_repo);
    let search_url = format!(
        "{api_url}/search/issues?per_page=1&q={}",
        percent_encode_query(&query)
    );
    match run_github_api_get(Path::new("."), &search_url, token) {
        Ok(value) => {
            let total = value
                .get("total_count")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0);
            if total > 0 {
                let existing_url = value
                    .get("items")
                    .and_then(|items| items.get(0))
                    .and_then(|item| item.get("html_url"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                result.action = "duplicate".to_owned();
                result.reason = format!(
                    "fingerprint duplicate search found {total} existing issue(s) in {}",
                    entry.target_repo
                );
                result.url = Some(existing_url);
                return result;
            }
        }
        Err(err) => {
            result.reason =
                "fingerprint duplicate search failed; refusing to open without it".to_owned();
            result.error = Some(format!("{err:#}"));
            return result;
        }
    }
    let payload = serde_json::json!({
        "title": entry.title,
        "body": entry.body,
        "labels": entry.labels,
    });
    let payload_path = args
        .out
        .join(format!("issue-broker-payload-{index:03}.json"));
    if let Err(err) = serde_json::to_vec_pretty(&payload)
        .map_err(anyhow::Error::from)
        .and_then(|bytes| fs::write(&payload_path, bytes).map_err(anyhow::Error::from))
    {
        result.reason = "failed to write the issue create payload receipt".to_owned();
        result.error = Some(format!("{err:#}"));
        return result;
    }
    let create_url = format!("{api_url}/repos/{}/issues", entry.target_repo);
    match run_curl_json_post(
        Path::new("."),
        &create_url,
        &format!("Authorization: Bearer {token}"),
        &payload_path,
        &[
            "Accept: application/vnd.github+json",
            "Content-Type: application/json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    ) {
        Ok(output) if output.status.success() => {
            let response: serde_json::Value =
                serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null);
            let url = response
                .get("html_url")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            result.action = "opened".to_owned();
            result.reason = "no fingerprint duplicate found; issue opened".to_owned();
            result.url = Some(url);
            result
        }
        Ok(output) => {
            result.reason = "GitHub issue create returned a failure status".to_owned();
            result.error = Some(format!(
                "http status {:?}: {}",
                output.http_status,
                String::from_utf8_lossy(&output.stderr)
            ));
            result
        }
        Err(err) => {
            result.reason = "GitHub issue create request failed".to_owned();
            result.error = Some(format!("{err:#}"));
            result
        }
    }
}

/// Minimal percent-encoding for a GitHub search query string: keeps
/// unreserved characters, encodes everything else byte-wise.
pub(crate) fn percent_encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{byte:02X}"));
            }
        }
    }
    encoded
}

/// Persist broker results (review/issue_broker_results.json next to the
/// other review artifacts under out, plus the NDJSON twin at the out root).
pub(crate) fn write_issue_broker_results(out: &Path, results: &[IssueBrokerResult]) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("issue_broker_results.json"),
        serde_json::to_vec_pretty(results)?,
    )?;
    let mut lines = String::new();
    for result in results {
        lines.push_str(&serde_json::to_string(result)?);
        lines.push('\n');
    }
    fs::write(out.join("issue_broker_results.ndjson"), lines)?;
    Ok(())
}

pub(crate) fn read_github_review_skip_receipt(review_json: &Path) -> Option<serde_json::Value> {
    let skip_path = github_review_skip_path(review_json);
    let text = fs::read_to_string(skip_path).ok()?;
    serde_json::from_str(&text).ok()
}

pub(crate) fn build_post_error_receipt(args: &PostArgs, err: &anyhow::Error) -> PostErrorReceipt {
    let review_metadata = read_github_review_metadata(args);
    let repo_valid = args.repo.as_deref().is_some_and(is_valid_repo_slug);
    let pull_number = args.pull_number.or_else(detect_pull_number_from_event);
    let token_present = args
        .github_token
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    let review_json_valid = review_metadata
        .as_ref()
        .is_some_and(|metadata| metadata.valid);
    let http_status = http_status_from_error(err);
    let (error_kind, failure_stage) = classify_post_error(
        args,
        err,
        repo_valid,
        pull_number,
        review_json_valid,
        http_status,
    );
    let would_post = token_present && repo_valid && pull_number.is_some() && review_json_valid;
    let payload_written = failure_stage == "network_post"
        && args.out.join("github-review-post-payload.json").exists();
    PostErrorReceipt {
        schema_version: 1,
        status: "failed".to_owned(),
        error_kind,
        failure_stage,
        reason: format!("{err:#}"),
        review_json: args.review_json.display().to_string(),
        review_json_exists: args.review_json.exists(),
        review_json_valid,
        review_event: review_metadata.as_ref().map(|review| review.event.clone()),
        review_body_bytes: review_metadata.as_ref().map(|review| review.body_bytes),
        review_comment_count: review_metadata.as_ref().map(|review| review.comments),
        diff_patch: review_metadata
            .as_ref()
            .map(|review| review.diff_patch.display().to_string())
            .unwrap_or_else(|| post_diff_patch_path(args).display().to_string()),
        diff_patch_exists: review_metadata
            .as_ref()
            .is_some_and(|review| review.diff_patch_exists),
        diff_patch_valid: review_metadata
            .as_ref()
            .is_some_and(|review| review.diff_patch_valid),
        diff_line_count: review_metadata
            .as_ref()
            .and_then(|review| review.diff_line_count),
        off_diff_comment_count: review_metadata
            .as_ref()
            .and_then(|review| review.off_diff_comment_count),
        repo: args.repo.clone(),
        repo_valid,
        pull_number,
        comments: review_metadata.as_ref().map(|review| review.comments),
        http_status,
        token_present,
        payload_written,
        would_post,
        failure_tolerated: !args.fail_on_post_error,
        fail_on_post_error: args.fail_on_post_error,
    }
}

pub(crate) fn classify_post_error(
    args: &PostArgs,
    err: &anyhow::Error,
    repo_valid: bool,
    pull_number: Option<u64>,
    review_json_valid: bool,
    http_status: Option<u16>,
) -> (String, String) {
    let token_present = args
        .github_token
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    if !token_present {
        return ("missing_token".to_owned(), "preflight".to_owned());
    }
    if !repo_valid {
        return ("invalid_repo".to_owned(), "preflight".to_owned());
    }
    if pull_number.is_none() {
        return ("missing_pull_number".to_owned(), "preflight".to_owned());
    }
    if !review_json_valid {
        return (
            "invalid_review_payload".to_owned(),
            "payload_validation".to_owned(),
        );
    }
    if http_status.is_some() {
        return ("post_http_error".to_owned(), "network_post".to_owned());
    }
    let text = model_error_chain_text(err).to_ascii_lowercase();
    if text.contains("curl") || text.contains("github review post failed") {
        return ("post_failed".to_owned(), "network_post".to_owned());
    }
    ("failed".to_owned(), "unknown".to_owned())
}

pub(crate) struct GitHubReviewMetadata {
    pub(crate) valid: bool,
    pub(crate) comments: usize,
    pub(crate) event: String,
    pub(crate) body_bytes: usize,
    pub(crate) diff_patch: PathBuf,
    pub(crate) diff_patch_exists: bool,
    pub(crate) diff_patch_valid: bool,
    pub(crate) diff_line_count: Option<usize>,
    pub(crate) off_diff_comment_count: Option<usize>,
}

pub(crate) fn read_github_review_metadata(args: &PostArgs) -> Option<GitHubReviewMetadata> {
    let review: GitHubReview = serde_json::from_slice(&fs::read(&args.review_json).ok()?).ok()?;
    let diff_patch = post_diff_patch_path(args);
    let diff_metadata = review_diff_metadata(&diff_patch, &review);
    let diff_valid = review.comments.is_empty()
        || diff_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.off_diff_comment_count == 0);
    // Mirror validate_github_review_payload_for_post: the receipt's `valid`
    // marker must reflect the policy the payload was prepared under, not the
    // hardcoded default.
    let review_body_policy = post_review_body_policy(args);
    let valid = validate_github_review_payload_with_policy_waiver(
        &review,
        &review_body_policy,
        summary_only_body_waives_post_validation(&review_body_policy),
    )
    .is_ok()
        && diff_valid;
    Some(GitHubReviewMetadata {
        valid,
        comments: review.comments.len(),
        event: review.event,
        body_bytes: review.body.len(),
        diff_patch,
        diff_patch_exists: diff_metadata.is_some(),
        diff_patch_valid: diff_metadata.is_some(),
        diff_line_count: diff_metadata
            .as_ref()
            .map(|metadata| metadata.diff_line_count),
        off_diff_comment_count: diff_metadata.map(|metadata| metadata.off_diff_comment_count),
    })
}

pub(crate) struct ReviewDiffMetadata {
    diff_line_count: usize,
    off_diff_comment_count: usize,
}

pub(crate) fn review_diff_metadata(
    diff_patch: &Path,
    review: &GitHubReview,
) -> Option<ReviewDiffMetadata> {
    let patch = fs::read_to_string(diff_patch).ok()?;
    let right_lines = right_side_diff_lines(&patch);
    Some(ReviewDiffMetadata {
        diff_line_count: right_lines.len(),
        off_diff_comment_count: off_diff_comment_count(review, &right_lines),
    })
}

pub(crate) fn off_diff_comment_count(
    review: &GitHubReview,
    right_lines: &BTreeSet<(String, u32)>,
) -> usize {
    review
        .comments
        .iter()
        .filter(|comment| {
            let path = normalize_repo_path(&comment.path);
            !right_lines.contains(&(path, comment.line))
        })
        .count()
}
