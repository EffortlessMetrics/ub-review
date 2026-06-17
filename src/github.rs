//! GitHub integration: review posting, PR thread context ingestion,
//! issue broker, and post validation (cleanup train step 17, pure
//! code motion). The compiler is the only component allowed to post;
//! this module owns the post command, thread context collection,
//! issue broker execution, and the review payload validators.

use crate::*;

pub(crate) fn github_event_action() -> Option<String> {
    for name in ["UB_REVIEW_GITHUB_EVENT_ACTION", "GITHUB_EVENT_ACTION"] {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    github_event_action_from_path()
}

pub(crate) fn github_event_action_from_path() -> Option<String> {
    let path = std::env::var_os("GITHUB_EVENT_PATH")?;
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("action")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

pub(crate) fn validate_run_args(args: &RunArgs) -> Result<()> {
    ensure_supported_mode(args.mode)?;
    validate_selector_syntax(&args.selectors)?;
    if !matches!(args.lane_width, 6 | 10 | 20) {
        bail!("--lane-width must be one of 6, 10, or 20");
    }
    if args.model_timeout_sec == 0 {
        bail!("--model-timeout-sec must be greater than zero");
    }
    if args.model_concurrency == 0 {
        bail!("--model-concurrency must be greater than zero");
    }
    if args.review_body_max_bytes < 1_000 {
        bail!("--review-body-max-bytes must be at least 1000");
    }
    Ok(())
}

pub(crate) fn apply_runtime_profile_limits(args: &mut RunArgs, profile: &Profile) -> Result<()> {
    let llm_in_flight = profile.limits.llm_in_flight;
    if llm_in_flight == 0 {
        bail!(
            "runtime profile {} has llm_in_flight=0; model concurrency cannot be scheduled",
            profile.name
        );
    }
    args.model_concurrency = args.model_concurrency.min(llm_in_flight);
    Ok(())
}

pub(crate) fn cmd_post(args: PostArgs) -> Result<()> {
    fs::create_dir_all(&args.out)?;
    if !args.review_json.exists()
        && let Some(skip) = read_github_review_skip_receipt(&args.review_json)
    {
        fs::write(
            args.out.join("post-result.json"),
            serde_json::to_vec_pretty(&skip)?,
        )?;
        println!(
            "skipped GitHub review post; wrote {}/post-result.json",
            args.out.display()
        );
        run_issue_broker_step(&args);
        return Ok(());
    }
    let post_outcome = match post_github_review(&args) {
        Ok(value) => {
            fs::write(
                args.out.join("post-result.json"),
                serde_json::to_vec_pretty(&value)?,
            )?;
            println!("wrote {}/post-result.json", args.out.display());
            Ok(())
        }
        Err(err) => {
            let value = build_post_error_receipt(&args, &err);
            fs::write(
                args.out.join("post-error.json"),
                serde_json::to_vec_pretty(&value)?,
            )?;
            if args.fail_on_post_error {
                Err(err)
            } else {
                eprintln!(
                    "ub-review post failed; wrote {}/post-error.json",
                    args.out.display()
                );
                Ok(())
            }
        }
    };
    // The issue broker runs after the review submission attempt on every
    // path: it has its own receipts (issue_broker_results.json), the
    // fingerprint duplicate search makes it idempotent across passes, and
    // its failures never change the post exit code.
    run_issue_broker_step(&args);
    post_outcome
}

/// Execute the run-written broker plan, never fatally: read
/// review/issue_broker_plan.json next to the review payload, perform the
/// remote duplicate search and opens for `attempt` entries, and write
/// issue_broker_results artifacts. Absent plan means the broker was not
/// opted in; any whole-step error is reported to stderr and swallowed
/// (broker outcomes never affect the gate or the post exit code).
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
    pub(crate) diff_line_count: usize,
    pub(crate) off_diff_comment_count: usize,
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

pub(crate) fn write_github_review_skip_receipt(
    review_dir: &Path,
    receipt: GitHubReviewSkipReceipt,
) -> Result<()> {
    let review_json = review_dir.join("github-review.json");
    if review_json.exists() {
        fs::remove_file(&review_json)?;
    }
    fs::write(
        github_review_skip_path(&review_json),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(())
}

pub(crate) fn build_github_review_skip_receipt(
    args: &RunArgs,
    review: &ReviewArtifacts,
    summary_only_body: SummaryOnlyBodyPolicy,
) -> GitHubReviewSkipReceipt {
    // The receipt reason must name the skip cause, not restate the terminal
    // state: a pass excluded by the profile's posting policy says so directly
    // instead of borrowing a sentence that can read like a contradiction, and
    // a body withheld by the boilerplate suppressor names the configured
    // [review_body].summary_only_body value and the finding counts it ruled
    // on.
    let reason = if review.terminal_state.review_payload_status == "skipped_pass_policy" {
        format!(
            "pass `{}` is not in [gate].post_review_on; the profile keeps this pass artifact-only.",
            review.run_pass
        )
    } else if review.terminal_state.review_payload_status == "skipped_artifact_only_body" {
        format!(
            "summary_only_body = `{}` withheld the PR-facing body as no-value boilerplate: {} summary-only findings, {} substantive; diagnostics remain in artifacts.",
            summary_only_body.key(),
            review.terminal_state.summary_only_findings,
            review.terminal_state.substantive_summary_only_findings
        )
    } else if review.terminal_state.review_payload_status == "skipped_gate_failure_artifact_only" {
        "the gate concluded `fail` and no reviewer-postable content was prepared; blocking reasons are receipted in review/gate_outcome.json."
            .to_owned()
    } else {
        review.terminal_state.reason.clone()
    };
    GitHubReviewSkipReceipt {
        schema_version: 1,
        status: "skipped".to_owned(),
        reason,
        review_payload_status: review.terminal_state.review_payload_status.clone(),
        terminal_state: review.terminal_state.status.clone(),
        github_review_json: None,
        run_pass: review.run_pass.clone(),
        model_mode: args.model_mode.key().to_owned(),
        inline_comments: review.inline_comments.len(),
        summary_only_findings: review.summary_only_findings.len(),
        missing_or_failed_sensor_evidence: review.missing_or_failed_sensor_evidence.len(),
        missing_or_failed_model_evidence: review.missing_or_failed_model_evidence.len(),
    }
}

pub(crate) fn github_review_skip_path(review_json: &Path) -> PathBuf {
    review_json
        .parent()
        .map(|dir| dir.join("github-review-skip.json"))
        .unwrap_or_else(|| PathBuf::from("github-review-skip.json"))
}

pub(crate) fn collect_pr_thread_context(root: &Path, args: &RunArgs) -> Result<PrThreadContext> {
    let mut context = PrThreadContext {
        schema: PR_THREAD_CONTEXT_SCHEMA.to_owned(),
        status: "absent".to_owned(),
        max_bytes: args.pr_thread_context_max_bytes,
        sources: Vec::new(),
        warnings: Vec::new(),
        pull_number: None,
        title: None,
        body: None,
        body_truncated: false,
        thread_context_path: None,
        thread_context: None,
        thread_context_truncated: false,
    };

    if let Some(event_path) = std::env::var_os("GITHUB_EVENT_PATH") {
        let event_path = PathBuf::from(event_path);
        context
            .sources
            .push(format!("github-event:{}", event_path.display()));
        match read_github_event_pr_context(&event_path, args.pr_thread_context_max_bytes) {
            Ok(event_context) => {
                context.pull_number = event_context.pull_number;
                context.title = event_context.title;
                context.body = event_context.body;
                context.body_truncated = event_context.body_truncated;
            }
            Err(err) => context
                .warnings
                .push(format!("github-event unavailable: {err}")),
        }
    }
    context.pull_number = args.github_pull_number.or(context.pull_number);

    let configured_thread_path = args.pr_thread_context.trim();
    if !configured_thread_path.is_empty() {
        let configured_path = PathBuf::from(configured_thread_path);
        let path = if configured_path.is_absolute() {
            configured_path
        } else {
            root.join(configured_path)
        };
        context
            .sources
            .push(format!("thread-context-file:{}", path.display()));
        context.thread_context_path = Some(path.display().to_string());
        match read_bounded_text_with_status(&path, args.pr_thread_context_max_bytes) {
            Ok(text) => {
                context.thread_context = Some(text.text);
                context.thread_context_truncated = text.truncated;
            }
            Err(err) => context
                .warnings
                .push(format!("thread-context-file unavailable: {err}")),
        }
    }

    match github_thread_api_request(args, context.pull_number) {
        None => {}
        Some(Err(err)) => context
            .warnings
            .push(format!("github-api thread context unavailable: {err}")),
        Some(Ok(request)) => {
            match read_github_pr_thread_context(root, &request, args.pr_thread_context_max_bytes) {
                Ok(api_context) => {
                    context.sources.extend(api_context.sources);
                    append_thread_context(
                        &mut context,
                        &api_context.thread_context,
                        args.pr_thread_context_max_bytes,
                    );
                }
                Err(err) => context
                    .warnings
                    .push(format!("github-api thread context unavailable: {err}")),
            }
        }
    }

    context.status =
        if context.title.is_some() || context.body.is_some() || context.thread_context.is_some() {
            "seeded".to_owned()
        } else if context.warnings.is_empty() {
            "absent".to_owned()
        } else {
            "unavailable".to_owned()
        };

    Ok(context)
}

pub(crate) struct GitHubThreadApiRequest<'a> {
    pub(crate) auth: &'a str,
    pub(crate) repo: &'a str,
    pub(crate) pull_number: u64,
    pub(crate) api_url: &'a str,
}

pub(crate) struct GitHubThreadApiContext {
    pub(crate) sources: Vec<String>,
    pub(crate) thread_context: String,
}

pub(crate) fn github_thread_api_request<'a>(
    args: &'a RunArgs,
    event_pull_number: Option<u64>,
) -> Option<Result<GitHubThreadApiRequest<'a>>> {
    let auth = args
        .pr_thread_auth
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    let Some(repo) = args
        .github_repo
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return Some(Err(anyhow::anyhow!(
            "GitHub repository slug is unavailable"
        )));
    };
    if !is_valid_repo_slug(repo) {
        return Some(Err(anyhow::anyhow!(
            "GitHub repository slug is invalid: {repo}"
        )));
    }
    let Some(pull_number) = args.github_pull_number.or(event_pull_number) else {
        return Some(Err(anyhow::anyhow!("pull request number is unavailable")));
    };
    Some(Ok(GitHubThreadApiRequest {
        auth,
        repo,
        pull_number,
        api_url: args.github_api_url.trim_end_matches('/'),
    }))
}

pub(crate) fn read_github_pr_thread_context(
    root: &Path,
    request: &GitHubThreadApiRequest<'_>,
    max_bytes: usize,
) -> Result<GitHubThreadApiContext> {
    let endpoints = [
        (
            "issue-comments",
            format!(
                "{}/repos/{}/issues/{}/comments?per_page=30",
                request.api_url, request.repo, request.pull_number
            ),
        ),
        (
            "review-summaries",
            format!(
                "{}/repos/{}/pulls/{}/reviews?per_page=30",
                request.api_url, request.repo, request.pull_number
            ),
        ),
        (
            "review-comments",
            format!(
                "{}/repos/{}/pulls/{}/comments?per_page=50",
                request.api_url, request.repo, request.pull_number
            ),
        ),
    ];
    let mut sections = Vec::new();
    let mut sources = Vec::new();
    for (kind, url) in endpoints {
        let value = run_github_api_get(root, &url, request.auth)
            .with_context(|| format!("fetch GitHub PR thread {kind}"))?;
        sources.push(format!(
            "github-api:{}/{}/{}",
            request.repo, request.pull_number, kind
        ));
        sections.push(render_github_pr_thread_section(kind, &value, max_bytes));
    }

    let mut text = String::new();
    text.push_str("## GitHub PR Thread Snapshot\n\n");
    text.push_str(&format!(
        "Source: `{}` PR `#{}`. Bounded to lane context; full GitHub thread remains source of truth.\n\n",
        escape_md(request.repo),
        request.pull_number
    ));
    text.push_str(&sections.join("\n"));
    let bounded = bounded_string(&text, max_bytes);
    Ok(GitHubThreadApiContext {
        sources,
        thread_context: bounded.text,
    })
}

pub(crate) fn run_github_api_get(root: &Path, url: &str, auth: &str) -> Result<serde_json::Value> {
    let mut command = ProcessCommand::new("curl");
    command
        .arg("-sS")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg("30")
        .arg("-w")
        .arg("\nUB_REVIEW_HTTP_STATUS:%{http_code}\n")
        .arg("-K")
        .arg("-")
        .arg(url)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().with_context(|| "spawn GitHub API curl")?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("curl stdin unavailable"))?;
        use std::io::Write as _;
        const AUTH_HEADER_NAME: &str = "Authorization";
        let auth_scheme = ["Bear", "er"].concat();
        for header in [
            "Accept: application/vnd.github+json",
            "X-GitHub-Api-Version: 2022-11-28",
            &format!("{AUTH_HEADER_NAME}: {auth_scheme} {auth}"),
        ] {
            writeln!(stdin, "header = \"{}\"", curl_config_quote(header))?;
        }
    }
    let output = child
        .wait_with_output()
        .with_context(|| "wait for GitHub API curl")?;
    let (stdout, http_status) = split_curl_http_status(output.stdout);
    if !output.status.success() {
        bail!(
            "GitHub API curl exited {:?} with http status {:?}: stderr: {}; stdout: {}",
            output.status.code(),
            http_status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&stdout)
        );
    }
    serde_json::from_slice(&stdout).with_context(|| "parse GitHub API response")
}

pub(crate) fn render_github_pr_thread_section(
    kind: &str,
    value: &serde_json::Value,
    max_bytes: usize,
) -> String {
    let title = match kind {
        "issue-comments" => "Issue Comments",
        "review-summaries" => "Review Summaries",
        "review-comments" => "Review Comments",
        _ => "Thread Items",
    };
    let mut text = format!("### {title}\n\n");
    let Some(items) = value.as_array() else {
        text.push_str("- GitHub response was not an array.\n");
        return text;
    };
    if items.is_empty() {
        text.push_str("- None found.\n");
        return text;
    }
    for item in items {
        text.push_str(&render_github_pr_thread_item(kind, item, max_bytes));
    }
    text
}

pub(crate) fn render_github_pr_thread_item(
    kind: &str,
    item: &serde_json::Value,
    max_bytes: usize,
) -> String {
    let author = item
        .pointer("/user/login")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let created_at = item
        .get("created_at")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown-time");
    let state = item
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let path = item
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let line = item
        .get("line")
        .or_else(|| item.get("original_line"))
        .and_then(serde_json::Value::as_u64);
    let body = item
        .get("body")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let bounded_body = bounded_string(body.trim(), max_bytes.min(1200));
    let location = if !path.is_empty() {
        match line {
            Some(line) => format!(" `{}`:`{line}`", escape_md(path)),
            None => format!(" `{}`", escape_md(path)),
        }
    } else {
        String::new()
    };
    let state = if state.is_empty() {
        String::new()
    } else {
        format!(" `{}`", escape_md(state))
    };
    let item_kind = match kind {
        "issue-comments" => "issue-comment",
        "review-summaries" => "review",
        "review-comments" => "review-comment",
        _ => "thread-item",
    };
    let mut text = format!(
        "- `{}` `{}` by `{}`{}{}\n",
        item_kind,
        escape_md(created_at),
        escape_md(author),
        state,
        location
    );
    if !bounded_body.text.is_empty() {
        text.push_str("  ```text\n");
        text.push_str(&bounded_body.text);
        if !bounded_body.text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("  ```\n");
    }
    text
}

pub(crate) fn append_thread_context(
    context: &mut PrThreadContext,
    addition: &str,
    max_bytes: usize,
) {
    if addition.trim().is_empty() {
        return;
    }
    let mut merged = String::new();
    if let Some(existing) = context.thread_context.as_deref() {
        merged.push_str(existing);
        if !existing.ends_with('\n') {
            merged.push('\n');
        }
        merged.push('\n');
    }
    merged.push_str(addition);
    let bounded = bounded_string(&merged, max_bytes);
    context.thread_context = Some(bounded.text);
    context.thread_context_truncated |= bounded.truncated;
}

pub(crate) struct GitHubEventPrContext {
    pub(crate) pull_number: Option<u64>,
    pub(crate) title: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) body_truncated: bool,
}

pub(crate) fn read_github_event_pr_context(
    path: &Path,
    max_bytes: usize,
) -> Result<GitHubEventPrContext> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let Some(pull_request) = value.get("pull_request") else {
        return Ok(GitHubEventPrContext {
            pull_number: None,
            title: None,
            body: None,
            body_truncated: false,
        });
    };
    let body = pull_request
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(|body| bounded_string(body, max_bytes));
    Ok(GitHubEventPrContext {
        pull_number: pull_request
            .get("number")
            .and_then(serde_json::Value::as_u64),
        title: pull_request
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        body: body.as_ref().map(|body| body.text.clone()),
        body_truncated: body.as_ref().is_some_and(|body| body.truncated),
    })
}

pub(crate) fn render_pr_thread_context(context: &PrThreadContext) -> String {
    let mut text = String::new();
    text.push_str(&format!("- Status: `{}`\n", context.status));
    if context.sources.is_empty() {
        text.push_str("- Sources: none\n");
    } else {
        text.push_str("- Sources:\n");
        for source in &context.sources {
            text.push_str(&format!("  - `{}`\n", escape_md(source)));
        }
    }
    if !context.warnings.is_empty() {
        text.push_str("- Warnings:\n");
        for warning in &context.warnings {
            text.push_str(&format!("  - {}\n", escape_md(warning)));
        }
    }
    if let Some(number) = context.pull_number {
        text.push_str(&format!("- Pull request: `#{number}`\n"));
    }
    if let Some(title) = context.title.as_deref() {
        text.push_str(&format!("- Title: {}\n", escape_md(title)));
    }
    if let Some(guidance) = pr_thread_reuse_guidance(context) {
        text.push('\n');
        text.push_str(guidance);
    }
    if let Some(body) = context.body.as_deref() {
        text.push_str("\n### PR Body\n\n```text\n");
        text.push_str(body);
        if !body.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```\n");
    }
    if let Some(thread_context) = context.thread_context.as_deref() {
        text.push_str("\n### Prior Review Thread\n\n```text\n");
        text.push_str(thread_context);
        if !thread_context.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```\n");
    }
    if context.status == "absent" {
        text.push_str("- No PR thread context was provided for this run.\n");
    }
    text
}

pub(crate) fn pr_thread_reuse_guidance(context: &PrThreadContext) -> Option<&'static str> {
    if context.status != "seeded" {
        return None;
    }
    Some(
        "### Seeded Thread Reuse Rules\n\n\
- Treat PR body claims, author replies, prior ub-review comments, resolved/dismissed discussion notes, and proof receipts in this context as lane evidence.\n\
- Before emitting a verification question or proof request, compare it with the seeded thread. If the same concern is already answered and the current diff does not reopen it, emit a `resolved-check` observation or `failed_objection` instead of a fresh candidate.\n\
- If the current diff reopens an answered concern, cite the changed file/line or proof receipt that makes the prior answer stale.\n",
    )
}
pub(crate) fn post_github_review(args: &PostArgs) -> Result<PostResultReceipt> {
    let token = args
        .github_token
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("github token is required for posting"))?;
    let repo = args
        .repo
        .as_ref()
        .filter(|value| is_valid_repo_slug(value))
        .ok_or_else(|| anyhow::anyhow!("valid GitHub repository slug is required"))?;
    let pull_number = match args.pull_number {
        Some(number) => number,
        None => detect_pull_number_from_event()
            .ok_or_else(|| anyhow::anyhow!("pull request number is required for posting"))?,
    };
    let review: GitHubReview = serde_json::from_slice(
        &fs::read(&args.review_json)
            .with_context(|| format!("read {}", args.review_json.display()))?,
    )
    .with_context(|| format!("parse {}", args.review_json.display()))?;
    validate_github_review_payload_for_post(args, &review)?;
    let api_payload = github_review_post_payload(&review)?;
    let post_payload = args.out.join("github-review-post-payload.json");
    fs::write(&post_payload, serde_json::to_vec_pretty(&api_payload)?)?;
    let url = format!(
        "{}/repos/{}/pulls/{}/reviews",
        args.github_api_url.trim_end_matches('/'),
        repo,
        pull_number
    );
    let output = run_curl_json_post(
        Path::new("."),
        &url,
        &format!("Authorization: Bearer {token}"),
        &post_payload,
        &[
            "Accept: application/vnd.github+json",
            "Content-Type: application/json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    )
    .with_context(|| "run GitHub review curl")?;
    let response_text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();
    fs::write(args.out.join("post-stdout.json"), &response_text)?;
    fs::write(args.out.join("post-stderr.txt"), &stderr_text)?;
    let response = serde_json::from_str(&response_text).unwrap_or_else(|_| {
        serde_json::json!({
            "raw": response_text,
        })
    });
    if !output.status.success() {
        bail!(
            "GitHub review post failed with exit code {:?} and http status {:?}: {}",
            output.status.code(),
            output.http_status,
            stderr_text
        );
    }
    let review_metadata = read_github_review_metadata(args);
    Ok(PostResultReceipt {
        schema_version: 1,
        status: "ok".to_owned(),
        repo: repo.clone(),
        repo_valid: true,
        pull_number,
        comments: review.comments.len(),
        review_json: args.review_json.display().to_string(),
        review_json_exists: args.review_json.exists(),
        review_json_valid: review_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.valid),
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
        http_status: output.http_status,
        token_present: true,
        payload_written: post_payload.exists(),
        post_stdout_written: args.out.join("post-stdout.json").exists(),
        post_stderr_written: args.out.join("post-stderr.txt").exists(),
        response,
    })
}

pub(crate) fn github_review_post_payload(review: &GitHubReview) -> Result<GitHubReviewPostPayload> {
    let comments = review
        .comments
        .iter()
        .map(|comment| {
            Ok(GitHubReviewPostComment {
                path: comment.path.clone(),
                line: comment.line,
                side: comment.side.clone(),
                body: github_review_post_comment_body(comment)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(GitHubReviewPostPayload {
        event: review.event.clone(),
        body: review.body.clone(),
        comments,
    })
}

pub(crate) fn github_review_post_comment_body(comment: &GitHubReviewComment) -> Result<String> {
    let Some(suggestion) = comment.suggestion.as_deref() else {
        return Ok(comment.body.clone());
    };
    validate_github_suggestion_text(suggestion)?;
    Ok(format!(
        "{}\n\n```suggestion\n{}\n```",
        comment.body.trim_end(),
        suggestion.trim()
    ))
}

/// Default-policy convenience wrapper kept for the payload contract tests;
/// production callers thread the effective policy and waiver explicitly.
#[cfg(test)]
pub(crate) fn validate_github_review_payload(review: &GitHubReview) -> Result<()> {
    validate_github_review_payload_with_policy_waiver(review, &ReviewBodyPolicy::default(), false)
}

pub(crate) fn validate_github_review_payload_with_policy_waiver(
    review: &GitHubReview,
    policy: &ReviewBodyPolicy,
    waive_suppressible_body_policy: bool,
) -> Result<()> {
    if review.event != "COMMENT" {
        bail!("github review event must be COMMENT");
    }
    validate_pr_review_body_policy_with_waiver(
        &review.body,
        policy,
        waive_suppressible_body_policy,
    )?;
    if review.comments.is_empty() && !pr_body_has_reviewer_value(&review.body) {
        bail!("github review body is missing reviewer-value content");
    }
    if has_standalone_approval_line(&review.body) {
        bail!("github review body contains standalone approval language");
    }
    for comment in &review.comments {
        if comment.side != "RIGHT" {
            bail!("github review comments must use side=RIGHT");
        }
        if !is_repo_relative_path(&comment.path) {
            bail!("github review comment path must be repo-relative");
        }
        if comment.line == 0 {
            bail!("github review comment line must be positive");
        }
        if comment.body.trim().is_empty() {
            bail!("github review comment body must not be empty");
        }
        if comment.body.chars().count() > 1_200 {
            bail!("github review comment body must be 1200 chars or fewer");
        }
        if !has_lane_prefix(&comment.body) {
            bail!("github review comment body must start with a lane prefix");
        }
        if has_standalone_approval_line(&comment.body) {
            bail!("github review comment contains standalone approval language");
        }
        if has_forbidden_pr_review_boilerplate(&comment.body) {
            bail!("github review comment contains artifact-only boilerplate");
        }
        if let Some(suggestion) = comment.suggestion.as_deref() {
            if !comment.body.starts_with("[unsafe-review]") {
                bail!("github review suggestion must be sourced from unsafe-review");
            }
            validate_github_suggestion_text(suggestion)?;
        }
    }
    Ok(())
}

pub(crate) fn validate_github_review_payload_for_post(
    args: &PostArgs,
    review: &GitHubReview,
) -> Result<()> {
    let review_body_policy = post_review_body_policy(args);
    let waive_suppressible = summary_only_body_waives_post_validation(&review_body_policy);
    validate_github_review_payload_with_policy_waiver(
        review,
        &review_body_policy,
        waive_suppressible,
    )?;
    let diff_patch = post_diff_patch_path(args);
    if review.comments.is_empty() {
        return Ok(());
    }
    let patch = fs::read_to_string(&diff_patch)
        .with_context(|| format!("read {}", diff_patch.display()))?;
    let right_lines = right_side_diff_lines(&patch);
    validate_github_review_payload_for_right_lines(
        review,
        &right_lines,
        &diff_patch.display().to_string(),
        &review_body_policy,
        waive_suppressible,
    )
}

/// The post step trusts the run's compile decision for the suppressible
/// body-policy classes: when the effective `[review_body].summary_only_body`
/// is a posting posture (`post_substantive`/`post_all`), a prepared
/// `github-review.json` was either clean or deliberately posted under that
/// posture, so re-running the suppressible text checks here would silently
/// override the configured policy. Under `suppress` (and when no effective
/// config is readable) the conservative checks stay in force.
pub(crate) fn summary_only_body_waives_post_validation(policy: &ReviewBodyPolicy) -> bool {
    !matches!(policy.summary_only_body, SummaryOnlyBodyPolicy::Suppress)
}

/// Subset of `effective-config.json` the post step needs: the `[review_body]`
/// policy the run prepared the payload under.
#[derive(Default, Deserialize)]
pub(crate) struct EffectiveReviewBodyConfig {
    #[serde(default)]
    pub(crate) review_body: ReviewBodyPolicy,
}

/// `[review_body]` policy for the post step, read from the run's
/// `effective-config.json` (the receipt written next to the `review/`
/// directory holding the payload). A missing or unreadable receipt falls back
/// to the conservative default policy.
pub(crate) fn post_review_body_policy(args: &PostArgs) -> ReviewBodyPolicy {
    let path = post_effective_config_path(args);
    fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<EffectiveReviewBodyConfig>(&bytes).ok())
        .map(|config| config.review_body)
        .unwrap_or_default()
}

pub(crate) fn post_effective_config_path(args: &PostArgs) -> PathBuf {
    if let Some(review_dir) = args.review_json.parent()
        && review_dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "review")
        && let Some(run_dir) = review_dir.parent()
    {
        return run_dir.join("effective-config.json");
    }
    args.out
        .parent()
        .map(|run_dir| run_dir.join("effective-config.json"))
        .unwrap_or_else(|| PathBuf::from("target/ub-review/effective-config.json"))
}

pub(crate) fn validate_github_review_payload_for_right_lines(
    review: &GitHubReview,
    right_lines: &BTreeSet<(String, u32)>,
    source: &str,
    review_body_policy: &ReviewBodyPolicy,
    waive_suppressible_body_policy: bool,
) -> Result<()> {
    validate_github_review_payload_with_policy_waiver(
        review,
        review_body_policy,
        waive_suppressible_body_policy,
    )?;
    for comment in &review.comments {
        let path = normalize_repo_path(&comment.path);
        if !right_lines.contains(&(path.clone(), comment.line)) {
            bail!(
                "github review comment {}:{} is not a valid RIGHT-side diff line in {}",
                path,
                comment.line,
                source
            );
        }
    }
    Ok(())
}

pub(crate) fn post_diff_patch_path(args: &PostArgs) -> PathBuf {
    if let Some(path) = &args.diff_patch {
        return path.clone();
    }
    if let Some(review_dir) = args.review_json.parent()
        && review_dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "review")
        && let Some(run_dir) = review_dir.parent()
    {
        return run_dir.join("input").join("diff.patch");
    }
    args.out
        .parent()
        .map(|run_dir| run_dir.join("input").join("diff.patch"))
        .unwrap_or_else(|| PathBuf::from("target/ub-review/input/diff.patch"))
}

pub(crate) fn is_repo_relative_path(path: &str) -> bool {
    let path = normalize_repo_path(path);
    !path.is_empty()
        && !Path::new(&path).is_absolute()
        && !path.split('/').any(|part| part.is_empty() || part == "..")
}

pub(crate) fn has_lane_prefix(body: &str) -> bool {
    let trimmed = body.trim_start();
    trimmed.starts_with('[')
        && trimmed
            .find(']')
            .is_some_and(|position| position > 1 && position <= 32)
}

pub(crate) fn is_valid_repo_slug(value: &str) -> bool {
    let mut parts = value.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(repo) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && !owner.is_empty()
        && !repo.is_empty()
        && owner.chars().all(is_repo_slug_char)
        && repo.chars().all(is_repo_slug_char)
}

pub(crate) fn is_repo_slug_char(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.')
}

pub(crate) fn detect_pull_number_from_event() -> Option<u64> {
    let path = std::env::var_os("GITHUB_EVENT_PATH")?;
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .pointer("/pull_request/number")
        .and_then(serde_json::Value::as_u64)
}
