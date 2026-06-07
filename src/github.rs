//! GitHub integration seam (modularization train step 9a, pure code motion):
//! PR thread ingest into `PrThreadContext`, the `post` command path
//! (`cmd_post`, payload validation against the recorded diff, post-result/
//! post-error/skip receipts), and issue broker execution against the GitHub
//! API. The review-body/inline-comment compiler, the `is_*noise*`
//! classifiers, and the shared curl transport deliberately stay in
//! `main.rs`: the classifiers are read there by path by the verifier
//! phrase-parity self-test (module 9b moves them jointly with that
//! assumption), and the transport is shared with the model lanes.

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

fn github_event_action_from_path() -> Option<String> {
    let path = std::env::var_os("GITHUB_EVENT_PATH")?;
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("action")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
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
fn run_issue_broker_step(args: &PostArgs) {
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
fn execute_issue_broker(args: &PostArgs, plan_path: &Path) -> Result<Vec<IssueBrokerResult>> {
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
fn execute_issue_broker_attempt(
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
fn percent_encode_query(value: &str) -> String {
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
fn write_issue_broker_results(out: &Path, results: &[IssueBrokerResult]) -> Result<()> {
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

fn read_github_review_skip_receipt(review_json: &Path) -> Option<serde_json::Value> {
    let skip_path = github_review_skip_path(review_json);
    let text = fs::read_to_string(skip_path).ok()?;
    serde_json::from_str(&text).ok()
}

fn build_post_error_receipt(args: &PostArgs, err: &anyhow::Error) -> PostErrorReceipt {
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

fn classify_post_error(
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

struct GitHubReviewMetadata {
    valid: bool,
    comments: usize,
    event: String,
    body_bytes: usize,
    diff_patch: PathBuf,
    diff_patch_exists: bool,
    diff_patch_valid: bool,
    diff_line_count: Option<usize>,
    off_diff_comment_count: Option<usize>,
}

fn read_github_review_metadata(args: &PostArgs) -> Option<GitHubReviewMetadata> {
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

struct ReviewDiffMetadata {
    diff_line_count: usize,
    off_diff_comment_count: usize,
}

fn review_diff_metadata(diff_patch: &Path, review: &GitHubReview) -> Option<ReviewDiffMetadata> {
    let patch = fs::read_to_string(diff_patch).ok()?;
    let right_lines = right_side_diff_lines(&patch);
    Some(ReviewDiffMetadata {
        diff_line_count: right_lines.len(),
        off_diff_comment_count: off_diff_comment_count(review, &right_lines),
    })
}

fn off_diff_comment_count(review: &GitHubReview, right_lines: &BTreeSet<(String, u32)>) -> usize {
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

fn github_review_skip_path(review_json: &Path) -> PathBuf {
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

struct GitHubThreadApiRequest<'a> {
    auth: &'a str,
    repo: &'a str,
    pull_number: u64,
    api_url: &'a str,
}

struct GitHubThreadApiContext {
    sources: Vec<String>,
    thread_context: String,
}

fn github_thread_api_request<'a>(
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

fn read_github_pr_thread_context(
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

fn render_github_pr_thread_section(
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

fn render_github_pr_thread_item(kind: &str, item: &serde_json::Value, max_bytes: usize) -> String {
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

fn append_thread_context(context: &mut PrThreadContext, addition: &str, max_bytes: usize) {
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

struct GitHubEventPrContext {
    pull_number: Option<u64>,
    title: Option<String>,
    body: Option<String>,
    body_truncated: bool,
}

fn read_github_event_pr_context(path: &Path, max_bytes: usize) -> Result<GitHubEventPrContext> {
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

fn pr_thread_reuse_guidance(context: &PrThreadContext) -> Option<&'static str> {
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

fn post_github_review(args: &PostArgs) -> Result<PostResultReceipt> {
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
    let post_payload = args.out.join("github-review-post-payload.json");
    fs::write(&post_payload, serde_json::to_vec_pretty(&review)?)?;
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

/// Default-policy convenience wrapper kept for the payload contract tests;
/// production callers thread the effective policy and waiver explicitly.
#[cfg(test)]
fn validate_github_review_payload(review: &GitHubReview) -> Result<()> {
    validate_github_review_payload_with_policy_waiver(review, &ReviewBodyPolicy::default(), false)
}

fn validate_github_review_payload_with_policy_waiver(
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
    }
    Ok(())
}

fn validate_github_review_payload_for_post(args: &PostArgs, review: &GitHubReview) -> Result<()> {
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
fn summary_only_body_waives_post_validation(policy: &ReviewBodyPolicy) -> bool {
    !matches!(policy.summary_only_body, SummaryOnlyBodyPolicy::Suppress)
}

/// Subset of `effective-config.json` the post step needs: the `[review_body]`
/// policy the run prepared the payload under.
#[derive(Default, Deserialize)]
struct EffectiveReviewBodyConfig {
    #[serde(default)]
    review_body: ReviewBodyPolicy,
}

/// `[review_body]` policy for the post step, read from the run's
/// `effective-config.json` (the receipt written next to the `review/`
/// directory holding the payload). A missing or unreadable receipt falls back
/// to the conservative default policy.
fn post_review_body_policy(args: &PostArgs) -> ReviewBodyPolicy {
    let path = post_effective_config_path(args);
    fs::read(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<EffectiveReviewBodyConfig>(&bytes).ok())
        .map(|config| config.review_body)
        .unwrap_or_default()
}

fn post_effective_config_path(args: &PostArgs) -> PathBuf {
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

pub(crate) fn detect_pull_number_from_event() -> Option<u64> {
    let path = std::env::var_os("GITHUB_EVENT_PATH")?;
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .pointer("/pull_request/number")
        .and_then(serde_json::Value::as_u64)
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead as _, BufReader, Write as _};
    use std::net::{TcpListener, TcpStream};

    use crate::tests::{
        model_lane_receipt, test_pr_thread_context, test_run_args, test_terminal_state,
    };

    use super::*;

    #[test]
    fn github_review_payload_requires_comment_event_and_right_side() -> Result<()> {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
";
        let line_map = right_side_diff_lines(patch);
        let ok = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Verification questions\n\n- Confirm the added regression test fails on base+tests, not only that it passes on HEAD.".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests] This test reaches the helper but does not assert the boundary.".to_owned(),
                suggestion: None,
            }],
        };
        validate_github_review_payload(&ok)?;

        let temp = tempfile::tempdir()?;
        write_github_review_payload(
            temp.path(),
            &ok,
            &line_map,
            &ReviewBodyPolicy::default(),
            false,
        )?;
        assert!(temp.path().join("github-review.json").exists());

        let stale_line = GitHubReview {
            comments: vec![GitHubReviewComment {
                line: 99,
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        let stale_line_out = tempfile::tempdir()?;
        let err = write_github_review_payload(
            stale_line_out.path(),
            &stale_line,
            &line_map,
            &ReviewBodyPolicy::default(),
            false,
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("stale line unexpectedly wrote github-review.json"))?;
        assert!(err.to_string().contains("not a valid RIGHT-side diff line"));
        assert!(!stale_line_out.path().join("github-review.json").exists());

        let bad_event = GitHubReview {
            event: "APPROVE".to_owned(),
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&bad_event).is_err());
        let bad_event_out = tempfile::tempdir()?;
        assert!(
            write_github_review_payload(
                bad_event_out.path(),
                &bad_event,
                &line_map,
                &ReviewBodyPolicy::default(),
                false
            )
            .is_err()
        );
        assert!(!bad_event_out.path().join("github-review.json").exists());

        let bad_side = GitHubReview {
            comments: vec![GitHubReviewComment {
                side: "LEFT".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&bad_side).is_err());
        let bad_out = tempfile::tempdir()?;
        assert!(
            write_github_review_payload(
                bad_out.path(),
                &bad_side,
                &line_map,
                &ReviewBodyPolicy::default(),
                false
            )
            .is_err()
        );
        assert!(!bad_out.path().join("github-review.json").exists());

        let parent_path = GitHubReview {
            comments: vec![GitHubReviewComment {
                path: "../src/lib.rs".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&parent_path).is_err());

        let empty_body = GitHubReview {
            comments: vec![GitHubReviewComment {
                body: " ".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&empty_body).is_err());

        let missing_prefix = GitHubReview {
            comments: vec![GitHubReviewComment {
                body: "This test reaches the helper but does not assert the boundary.".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&missing_prefix).is_err());

        let overlong_body = GitHubReview {
            comments: vec![GitHubReviewComment {
                body: format!("[tests] {}", "x".repeat(1_201)),
                ..ok.comments[0].clone()
            }],
            ..ok
        };
        assert!(validate_github_review_payload(&overlong_body).is_err());
        Ok(())
    }

    #[test]
    fn github_review_payload_rejects_pr_body_boilerplate() -> Result<()> {
        let mut review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Model lanes\n\n- Lane: `ub`\n  Provider: `minimax`\n  Model: `m3`\n  Status: `ok` - completed".to_owned(),
            comments: Vec::new(),
        };

        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("model lane table unexpectedly passed"))?;
        assert!(err.to_string().contains("successful lane table"), "{err:#}");

        review.body = "## Decision\n\n- No blocking finding after bounded review; residual risk remains for human review.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("no-finding boilerplate unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "- Profile: `gh-runner`\n- Base: `origin/main`\n- Head: `HEAD`".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("execution summary unexpectedly passed"))?;
        assert!(err.to_string().contains("execution summary"), "{err:#}");

        review.body =
            "## Test proof\n\n- Provider status was ok and the model lane roster completed."
                .to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("status boilerplate unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Residual risk\n\n- External trust risk remains.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("residual-risk section unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Verification questions\n\n- Confirm the cached prior observation still matches; the refuter demoted inline candidate because Gate proof is pending.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("meta review prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Confirmed findings\n\n- Ub-review action receives secrets.MINIMAX_API_KEY and github.token at runtime; a malicious or compromised dad0f23 would exfiltrate these. Pinning to SHA is correct posture but does not eliminate upstream trust.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("standing workflow trust prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Confirmed findings\n\n- No pinning defect introduced. The only standing concern is upstream SHA trust for EffortlessMetrics/ub-review@e76ccbc, which is identical in posture to the prior pin and is a repo-level policy item, not a diff finding.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("no-defect pinning prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Confirmed findings\n\n- The diff is a 4-line mechanical SHA bump at the three expected sites: cache `key`, `restore-keys` prefix, and action `uses:`. No permission, trigger, or `with:` block change; net new secret/permission surface relative to the prior pin is zero.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("mechanical pin no-change prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Decision\n\n- Needs one verification check before upstream.\n\n## Verification questions\n\n- Confirm checkout credential persistence: workflows using pull_request from forks receive a read-only GITHUB_TOKEN; this lane did not change checkout config, so no new persistence vector is introduced. Actionlint receipt 'ok' supports no syntactic regression.\n\n## Refuted\n\n- Adding a workflow file to paths-ignore could grant implicit permission expansion; refuted because: paths-ignore only filters trigger activation; it does not alter token scopes, permissions blocks, or any job-level security context.\n\n## Parked follow-ups\n\n- paths-ignore is a literal substring/glob match; a future rename of ub-review-packet.yml silently re-enables Droid noise.\n\n## Evidence gaps\n\n- zizmor, gitleaks, osv-scanner, cargo-audit, cargo-deny, shellcheck, semgrep, coverage all disabled by config or trigger-mismatched. No security/pinning tool independently re-validated this workflow file.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("paths-ignore no-posture review unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Decision\n\n- Needs one verification check before upstream.\n\n## Verification questions\n\n- Confirm no focused smoke proof (workflow_run on a fork-PR dry-run, or a temporary pull_request_target guard test) was executed for the paths-ignore change. Trust rests on actionlint parse only; semantic skip behavior on the droid lane is not proven by sensors.\n\n## Refuted\n\n- adding ub-review-packet.yml to paths-ignore could mask future unpinned uses: additions in that file from Droid lane coverage; refuted because: paths-ignore lift is per-PR: any future PR that also touches ub-review-packet.yml (i.e. adds/changes uses:) will change the changed-files set and re-trigger Droid. Droid lanes are non-blocking/auxiliary by design; UB gate is the authoritative review.\n\n## Evidence gaps\n\n- PR body states actionlint is not installed locally, so the 'ok' receipt must come from the ub-review gate's own tooling rather than a local pre-push run; trust depends on that gate having actually executed actionlint v1 against this ref.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| {
                anyhow::anyhow!("paths-ignore smoke-proof review unexpectedly passed")
            })?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Decision\n\n- Needs one verification check before upstream.\n\n## Verification questions\n\n- Confirm actionlint receipt 'ok' confirms syntactic validity, but no semantic proof of skip behavior on the droid lane is available; trust rests on actionlint parse plus per-PR trigger semantics - the droid lane is auxiliary/non-blocking and the UB gate is authoritative, so residual workflow risk is bounded.\n\n## Refuted\n\n- paths-ignore addition could mask future unpinned uses: additions in ub-review-packet.yml from Droid lane coverage; refuted because: paths-ignore lift is per-PR: any future PR that also touches ub-review-packet.yml (adds/changes uses:) will change the changed-files set and re-trigger Droid. UB gate is the authoritative review surface and runs on the new pin.\n\n## Parked follow-ups\n\n- Residual workflow risk: cache key/restore-keys prefix is coupled to action SHA. Any future repin must update all three sites; a partial update silently mismatches cache restore. Not actionable in this PR (current state is consistent) - parked for follow-up lint rule or script.\n\n## Evidence gaps\n\n- trust gap: no focused smoke proof (workflow_run on fork-PR dry-run or pull_request_target guard) executed for the paths-ignore change; semantic skip behavior on Droid lane unproven beyond actionlint parse.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| {
                anyhow::anyhow!("paths-ignore actionlint skip-proof review unexpectedly passed")
            })?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Decision\n\n- Needs one verification check before upstream.\n\n## Verification questions\n\n- Workflow-pinning lane for PR #49. Two workflow YAML files touched. Pin lockstep verified across 3 sites, old pin absent, cache key/restore-keys prefix match, no other third-party actions changed.\n\n## Parked follow-ups\n\n- Cache key/restore-keys prefix is coupled to action SHA; any future partial repin silently mismatches restore. Current state consistent, parked for lint-rule follow-up.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| {
                anyhow::anyhow!("workflow lockstep summary review unexpectedly passed")
            })?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = format!(
            "## Decision\n\n- Needs focused cleanup before merge.\n\n## Verification questions\n\n{}",
            (1..=13)
                .map(|index| format!("- Confirm decision-relevant proof item {index}."))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("overlong bullet list unexpectedly passed"))?;
        assert!(err.to_string().contains("not concise enough"), "{err:#}");

        review.body = format!(
            "## Decision\n\n- Needs focused cleanup before merge.\n\n## Evidence gaps\n\n- {}",
            "proof gap ".repeat(800)
        );
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("oversized body unexpectedly passed"))?;
        assert!(err.to_string().contains("not concise enough"), "{err:#}");

        review.body = "## Confirmed findings\n\n- CodeRabbit's review-comment at ub-review-packet.yml:58 asserts the PR gate target SHA is 892e1bb44b7cb24753b7701b405d078f4ef11ee1, not be524219e33ff37edeab61ddc28c01250a08b492 used in the diff. If that claim is correct the workflow pin does not match the upstream gate.\n\n## Evidence gaps\n\n- CodeRabbit review-comment on .github/workflows/ub-review-packet.yml:58, scripted check showing 0 references to 892e1bb44b... in the file; PR body and droid-ub/droid-tests receipts only confirm internal lockstep, not match to gate target.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("stale bot target-SHA prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Refuted\n\n- cursor[bot] and coderabbitai[bot] comments claim target is e76ccbcb... and demand swap back; PR body, diff, and head tree all show ec8f890 as the actual target. Their objection is a false positive against the current diff and reopens nothing.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("stale bot refutation prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body =
            "## Refuted\n\n- A prior objection was false, and no finding remains.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("refuted-only body unexpectedly passed"))?;
        assert!(
            err.to_string().contains("refuted-only artifact note"),
            "{err:#}"
        );

        review.body = "## Evidence gaps\n\n- actionlint receipt is 'ok' per sensor table; no per-line output inlined into this lane packet, so re-verification of lint findings depends on the central proof broker artifact.\n- No fresh PR-build smoke run is available (build/test skipped, --allow-heavy required); only tokmd/actionlint receipts are present for this 4-line workflow pin.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("workflow tool-status gap prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Evidence gaps\n\n- Shared context hash and terminal state are available in artifacts.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("artifact pointer boilerplate unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );
        Ok(())
    }

    #[test]
    fn github_review_payload_rejects_inline_comment_boilerplate() -> Result<()> {
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Verification questions\n\n- Confirm the focused proof.".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests] No actionable findings after checking this path.".to_owned(),
                suggestion: None,
            }],
        };

        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("inline boilerplate unexpectedly passed"))?;
        assert!(
            err.to_string()
                .contains("comment contains artifact-only boilerplate"),
            "{err:#}"
        );
        Ok(())
    }

    #[test]
    fn github_review_post_payload_requires_recorded_right_diff_line() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let diff_patch = temp.path().join("diff.patch");
        fs::write(
            &diff_patch,
            "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
",
        )?;
        let args = PostArgs {
            review_json: temp.path().join("github-review.json"),
            diff_patch: Some(diff_patch),
            out: temp.path().join("post"),
            github_token: Some("token".to_owned()),
            repo: Some("EffortlessMetrics/ub-review".to_owned()),
            pull_number: Some(1),
            github_api_url: "https://api.github.com".to_owned(),
            fail_on_post_error: false,
        };
        let ok = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "Review body".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests] This test reaches the helper but not the boundary.".to_owned(),
                suggestion: None,
            }],
        };
        validate_github_review_payload_for_post(&args, &ok)?;

        let stale_line = GitHubReview {
            comments: vec![GitHubReviewComment {
                line: 99,
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        let err = validate_github_review_payload_for_post(&args, &stale_line)
            .err()
            .ok_or_else(|| anyhow::anyhow!("stale line unexpectedly passed diff validation"))?;
        assert!(err.to_string().contains("not a valid RIGHT-side diff line"));

        let wrong_file = GitHubReview {
            comments: vec![GitHubReviewComment {
                path: "src/other.rs".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok
        };
        let err = validate_github_review_payload_for_post(&args, &wrong_file)
            .err()
            .ok_or_else(|| anyhow::anyhow!("wrong file unexpectedly passed diff validation"))?;
        assert!(err.to_string().contains("not a valid RIGHT-side diff line"));
        Ok(())
    }

    #[test]
    fn post_command_accepts_explicit_skip_receipt_without_token() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_json = temp.path().join("review").join("github-review.json");
        let review_dir = review_json
            .parent()
            .ok_or_else(|| anyhow::anyhow!("review json parent missing"))?;
        fs::create_dir_all(review_dir)?;
        fs::write(
            github_review_skip_path(&review_json),
            serde_json::json!({
                "schema_version": 1,
                "status": "skipped",
                "reason": "empty smoke review",
                "review_payload_status": "skipped_empty_smoke"
            })
            .to_string(),
        )?;
        let args = PostArgs {
            review_json,
            diff_patch: None,
            out: temp.path().join("post"),
            github_token: None,
            repo: None,
            pull_number: None,
            github_api_url: "https://api.github.com".to_owned(),
            fail_on_post_error: true,
        };

        cmd_post(args)?;

        let result: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("post/post-result.json"))?)?;
        assert_eq!(result["status"], "skipped");
        assert_eq!(result["review_payload_status"], "skipped_empty_smoke");
        assert!(!temp.path().join("post/post-error.json").exists());
        Ok(())
    }

    #[test]
    fn pr_thread_context_reads_configured_file_bounded() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("thread.md"),
            "Author reply: ASAN bad-free receipt attached.\nThis tail should be truncated.",
        )?;
        let mut args = test_run_args(temp.path().join("out"));
        args.pr_thread_context = "thread.md".to_owned();
        args.pr_thread_context_max_bytes = 40;

        let context = collect_pr_thread_context(temp.path(), &args)?;
        let rendered = render_pr_thread_context(&context);

        assert_eq!(context.status, "seeded");
        assert!(
            context
                .thread_context_path
                .as_deref()
                .is_some_and(|path| path.ends_with("thread.md"))
        );
        assert!(
            context
                .thread_context
                .as_deref()
                .is_some_and(|text| text.contains("ASAN bad-free"))
        );
        assert!(context.thread_context_truncated);
        assert!(rendered.contains("### Prior Review Thread"));
        assert!(rendered.contains("[truncated]"));
        assert!(!rendered.contains("tail should be truncated"));
        Ok(())
    }

    #[test]
    fn seeded_pr_thread_context_tells_lanes_not_to_reask_answered_questions() {
        let mut context = test_pr_thread_context();
        context.status = "seeded".to_owned();
        context.title = Some("Harden bad-free proof".to_owned());
        context.body = Some(
            "PR body: base+tests receipt shows the new focused test fails on base and passes on HEAD."
                .to_owned(),
        );
        context.thread_context = Some(
            "Author reply: ASAN receipt attached; prior ub-review verification question is answered."
                .to_owned(),
        );

        let rendered = render_pr_thread_context(&context);

        assert!(rendered.contains("### Seeded Thread Reuse Rules"));
        assert!(rendered.contains("Treat PR body claims, author replies"));
        assert!(rendered.contains("proof receipts in this context as lane evidence"));
        assert!(rendered.contains("already answered and the current diff does not reopen it"));
        assert!(rendered.contains("`resolved-check` observation or `failed_objection`"));
        assert!(rendered.contains("makes the prior answer stale"));
    }

    #[test]
    fn absent_pr_thread_context_omits_reuse_rules() {
        let rendered = render_pr_thread_context(&test_pr_thread_context());

        assert!(!rendered.contains("### Seeded Thread Reuse Rules"));
        assert!(rendered.contains("- No PR thread context was provided for this run."));
    }

    #[test]
    fn pr_thread_context_reads_github_event_pr_metadata() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let event_path = temp.path().join("event.json");
        fs::write(
            &event_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "pull_request": {
                    "number": 37,
                    "title": "Harden FFI bad-free tests",
                    "body": "The ASAN receipt proves the old no-finalizer path fails on base+tests and passes on HEAD."
                }
            }))?,
        )?;

        let context = read_github_event_pr_context(&event_path, 48)?;

        assert_eq!(context.pull_number, Some(37));
        assert_eq!(context.title.as_deref(), Some("Harden FFI bad-free tests"));
        assert!(
            context
                .body
                .as_deref()
                .is_some_and(|body| body.contains("ASAN receipt"))
        );
        assert!(context.body_truncated);
        Ok(())
    }

    #[test]
    fn pr_thread_context_treats_non_pr_event_as_absent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let event_path = temp.path().join("event.json");
        fs::write(
            &event_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "repository": {
                    "full_name": "EffortlessMetrics/ub-review"
                }
            }))?,
        )?;

        let context = read_github_event_pr_context(&event_path, 65_536)?;

        assert_eq!(context.pull_number, None);
        assert_eq!(context.title, None);
        assert_eq!(context.body, None);
        assert!(!context.body_truncated);
        Ok(())
    }

    #[test]
    fn pr_thread_context_truncates_github_event_body_on_utf8_boundary() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let event_path = temp.path().join("event.json");
        fs::write(
            &event_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "pull_request": {
                    "number": 38,
                    "title": "Non-ASCII PR body",
                    "body": "🔥 receipt attached"
                }
            }))?,
        )?;

        let context = read_github_event_pr_context(&event_path, 1)?;

        assert_eq!(context.pull_number, Some(38));
        assert_eq!(context.body.as_deref(), Some("\n[truncated]\n"));
        assert!(context.body_truncated);
        Ok(())
    }

    #[test]
    fn pr_thread_context_fetches_github_thread_snapshot() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (github_api_url, handle) = spawn_fake_github_thread_api(3)?;
        let mut args = test_run_args(temp.path().join("out"));
        args.pr_thread_auth = Some("thread-token-redacted".to_owned());
        args.github_repo = Some("EffortlessMetrics/ub-review".to_owned());
        args.github_pull_number = Some(76);
        args.github_api_url = github_api_url;
        args.pr_thread_context_max_bytes = 8_192;

        let context = collect_pr_thread_context(temp.path(), &args)?;
        let requests = join_fake_github_thread_api(handle)?;
        let rendered = render_pr_thread_context(&context);

        assert_eq!(requests.len(), 3);
        assert!(requests.iter().any(|request| request.contains(
            "GET /repos/EffortlessMetrics/ub-review/issues/76/comments?per_page=30 HTTP/1.1"
        )));
        assert!(requests.iter().any(|request| request.contains(
            "GET /repos/EffortlessMetrics/ub-review/pulls/76/reviews?per_page=30 HTTP/1.1"
        )));
        assert!(requests.iter().any(|request| request.contains(
            "GET /repos/EffortlessMetrics/ub-review/pulls/76/comments?per_page=50 HTTP/1.1"
        )));
        let expected_auth = format!(
            "{}: {} thread-token-redacted",
            "Authorization",
            ["Bear", "er"].concat()
        );
        assert!(
            requests
                .iter()
                .all(|request| request.contains(&expected_auth))
        );
        assert_eq!(context.status, "seeded");
        assert_eq!(context.pull_number, Some(76));
        assert!(context.sources.iter().any(|source| {
            source.contains("github-api:EffortlessMetrics/ub-review/76/issue-comments")
        }));
        assert!(
            context
                .thread_context
                .as_deref()
                .is_some_and(|thread| thread.contains("ASAN receipt attached"))
        );
        assert!(rendered.contains("## GitHub PR Thread Snapshot"));
        assert!(rendered.contains("ub-review previous question resolved"));
        assert!(rendered.contains("`src/lib.rs`:`12`"));
        assert!(!rendered.contains("thread-token-redacted"));
        Ok(())
    }

    #[test]
    fn post_validation_honors_effective_summary_only_body_policy() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let run_dir = temp.path();
        let review_dir = run_dir.join("review");
        fs::create_dir_all(&review_dir)?;
        let args = PostArgs {
            review_json: review_dir.join("github-review.json"),
            diff_patch: None,
            out: review_dir.clone(),
            github_token: Some("token".to_owned()),
            repo: Some("EffortlessMetrics/ub-review".to_owned()),
            pull_number: Some(1),
            github_api_url: "https://api.github.com".to_owned(),
            fail_on_post_error: false,
        };
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Confirmed findings\n\n- [opposition] Residual risk remains for human review in the resize path.".to_owned(),
            comments: Vec::new(),
        };

        // Without an effective config the conservative default rejects the body.
        let err = super::validate_github_review_payload_for_post(&args, &review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("boilerplate body unexpectedly passed post checks"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        // With a posting posture in effective-config.json the post step honors
        // the run's compile decision and waives the suppressible classes.
        let mut config = Config::default();
        config.review_body.summary_only_body = SummaryOnlyBodyPolicy::PostSubstantive;
        fs::write(
            run_dir.join("effective-config.json"),
            serde_json::to_vec_pretty(&config)?,
        )?;
        super::validate_github_review_payload_for_post(&args, &review)?;
        Ok(())
    }

    #[test]
    fn suppressed_body_skip_receipt_names_policy_and_counts() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let mut terminal_state = test_terminal_state("needs-reviewer-attention");
        terminal_state.review_payload_status = "skipped_artifact_only_body".to_owned();
        terminal_state.summary_only_findings = 10;
        terminal_state.substantive_summary_only_findings = 0;
        let review = super::ReviewArtifacts {
            shared_context_id: "abc123".to_owned(),
            review_profile: DEFAULT_REVIEW_PROFILE.to_owned(),
            mode: "review-byok".to_owned(),
            posting: "review".to_owned(),
            runtime_profile: "gh-runner".to_owned(),
            run_pass: "opened".to_owned(),
            model_mode: "auto".to_owned(),
            depth: "standard".to_owned(),
            provider_policy: "minimax-only".to_owned(),
            model_provider_policy: "minimax-only".to_owned(),
            lane_width: 10,
            model_concurrency: 8,
            max_model_calls: 18,
            max_inline_comments: 8,
            model_timeout_sec: 300,
            ledger_path: String::new(),
            ledger_max_bytes: 65_536,
            pr_thread_context: test_pr_thread_context(),
            terminal_state,
            provider_preflights: Vec::new(),
            model_lanes: vec![model_lane_receipt("opposition", "ok")],
            missing_or_failed_sensor_evidence: Vec::new(),
            missing_or_failed_model_evidence: Vec::new(),
            inline_comments: Vec::new(),
            summary_only_findings: Vec::new(),
            observations: Vec::new(),
            proof_requests: Vec::new(),
            proof_receipts: Vec::new(),
            resource_leases: Vec::new(),
            body: "artifact body".to_owned(),
        };

        let receipt = super::build_github_review_skip_receipt(
            &args,
            &review,
            SummaryOnlyBodyPolicy::PostSubstantive,
        );
        assert_eq!(receipt.review_payload_status, "skipped_artifact_only_body");
        assert_eq!(
            receipt.reason,
            "summary_only_body = `post_substantive` withheld the PR-facing body as no-value boilerplate: 10 summary-only findings, 0 substantive; diagnostics remain in artifacts."
        );

        let mut suppress_review = review;
        suppress_review.terminal_state.summary_only_findings = 3;
        suppress_review
            .terminal_state
            .substantive_summary_only_findings = 2;
        let suppress_receipt = super::build_github_review_skip_receipt(
            &args,
            &suppress_review,
            SummaryOnlyBodyPolicy::Suppress,
        );
        assert_eq!(
            suppress_receipt.reason,
            "summary_only_body = `suppress` withheld the PR-facing body as no-value boilerplate: 3 summary-only findings, 2 substantive; diagnostics remain in artifacts."
        );
    }

    #[test]
    fn issue_broker_executes_plan_with_duplicate_search_and_receipts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("post-out");
        fs::create_dir_all(&out)?;
        let review_dir = temp.path().join("review");
        fs::create_dir_all(&review_dir)?;
        let entry = |id: &str, fingerprint: &str, decision: &str| IssueBrokerPlanEntry {
            schema: "ub-review.issue_broker_plan.v1".to_owned(),
            candidate_id: id.to_owned(),
            fingerprint: fingerprint.to_owned(),
            target_repo: "EffortlessMetrics/ripr-swarm".to_owned(),
            decision: decision.to_owned(),
            reason: "test".to_owned(),
            title: "Broker test issue".to_owned(),
            body: format!("body\n\nub-review-fingerprint: {fingerprint}\n"),
            labels: vec!["ub-review".to_owned()],
        };
        let plan = vec![
            entry("issue-candidate-000-aaaaaaaaaaaa", "fresh", "attempt"),
            entry("issue-candidate-001-bbbbbbbbbbbb", "existing", "attempt"),
            entry("issue-candidate-002-cccccccccccc", "skipme", "skip"),
        ];
        let plan_path = review_dir.join("issue_broker_plan.json");
        fs::write(&plan_path, serde_json::to_vec_pretty(&plan)?)?;

        // Fake GitHub API: search for "fresh" finds nothing then accepts the
        // create; search for "existing" returns one hit so no create happens.
        let (api_url, handle) = spawn_fake_issue_broker_api(3)?;
        let args = PostArgs {
            review_json: review_dir.join("github-review.json"),
            diff_patch: None,
            out: out.clone(),
            github_token: Some("test-token".to_owned()),
            repo: Some("EffortlessMetrics/ub-review".to_owned()),
            pull_number: Some(346),
            github_api_url: api_url,
            fail_on_post_error: false,
        };
        let results = execute_issue_broker(&args, &plan_path)?;
        let requests = join_fake_issue_broker_api(handle)?;
        assert_eq!(requests.len(), 3);
        assert!(requests[0].contains("GET /search/issues"));
        assert!(requests[0].contains("fresh"));
        assert!(requests[1].contains("POST /repos/EffortlessMetrics/ripr-swarm/issues"));
        assert!(requests[2].contains("GET /search/issues"));
        assert!(requests[2].contains("existing"));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].action, "opened");
        assert_eq!(
            results[0].url.as_deref(),
            Some("https://github.com/EffortlessMetrics/ripr-swarm/issues/9001")
        );
        assert_eq!(results[1].action, "duplicate");
        assert_eq!(
            results[1].url.as_deref(),
            Some("https://github.com/EffortlessMetrics/ripr-swarm/issues/1052")
        );
        assert_eq!(results[2].action, "skipped");

        // The create payload receipt is on disk and carries the marker body.
        let payload = fs::read_to_string(out.join("issue-broker-payload-000.json"))?;
        assert!(payload.contains("ub-review-fingerprint: fresh"));

        write_issue_broker_results(&out, &results)?;
        let written: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(out.join("review/issue_broker_results.json"))?)?;
        assert_eq!(written.len(), 3);
        let ndjson = fs::read_to_string(out.join("issue_broker_results.ndjson"))?;
        assert_eq!(ndjson.lines().count(), 3);

        // No token: planned attempts become failed_to_open, never errors.
        let no_token = PostArgs {
            github_token: None,
            ..args
        };
        let results = execute_issue_broker(&no_token, &plan_path)?;
        assert_eq!(results[0].action, "failed_to_open");
        assert_eq!(results[1].action, "failed_to_open");
        assert_eq!(results[2].action, "skipped");
        Ok(())
    }

    fn spawn_fake_issue_broker_api(
        expected_requests: usize,
    ) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || -> Result<Vec<String>> {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut requests = Vec::new();
            while requests.len() < expected_requests {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        requests.push(handle_fake_issue_broker_request(stream)?);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            bail!(
                                "fake issue broker API received {} of {} requests",
                                requests.len(),
                                expected_requests
                            );
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            Ok(requests)
        });
        Ok((url, handle))
    }

    fn handle_fake_issue_broker_request(mut stream: TcpStream) -> Result<String> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                bail!("fake issue broker request ended before headers finished");
            }
            headers.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        if content_length > 0 {
            let mut body = vec![0u8; content_length];
            use std::io::Read as _;
            reader.read_exact(&mut body)?;
        }
        let request_line = headers.lines().next().unwrap_or_default().to_owned();
        let (status_line, response_body) = if request_line.starts_with("GET /search/issues")
            && request_line.contains("existing")
        {
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({
                    "total_count": 1,
                    "items": [{
                        "html_url": "https://github.com/EffortlessMetrics/ripr-swarm/issues/1052"
                    }]
                }))?,
            )
        } else if request_line.starts_with("GET /search/issues") {
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({"total_count": 0, "items": []}))?,
            )
        } else {
            (
                "HTTP/1.1 201 Created",
                serde_json::to_vec(&serde_json::json!({
                    "html_url": "https://github.com/EffortlessMetrics/ripr-swarm/issues/9001"
                }))?,
            )
        };
        write!(
            stream,
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        )?;
        stream.write_all(&response_body)?;
        Ok(request_line)
    }

    fn join_fake_issue_broker_api(
        handle: thread::JoinHandle<Result<Vec<String>>>,
    ) -> Result<Vec<String>> {
        match handle.join() {
            Ok(result) => result,
            Err(_) => bail!("fake issue broker API thread panicked"),
        }
    }

    fn spawn_fake_github_thread_api(
        expected_requests: usize,
    ) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || -> Result<Vec<String>> {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut requests = Vec::new();
            while requests.len() < expected_requests {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        requests.push(handle_fake_github_thread_request(stream)?);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            bail!(
                                "fake GitHub thread API received {} of {} requests",
                                requests.len(),
                                expected_requests
                            );
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            Ok(requests)
        });
        Ok((url, handle))
    }

    fn handle_fake_github_thread_request(mut stream: TcpStream) -> Result<String> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                bail!("fake GitHub thread request ended before headers finished");
            }
            headers.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let request_line = headers.lines().next().unwrap_or_default();
        let response_body = if request_line.contains("/issues/76/comments?per_page=30") {
            serde_json::to_vec(&serde_json::json!([
                {
                    "created_at": "2026-06-03T10:00:00Z",
                    "user": {"login": "author"},
                    "body": "Author reply: ASAN receipt attached; prior verification question is answered."
                }
            ]))?
        } else if request_line.contains("/pulls/76/reviews?per_page=30") {
            serde_json::to_vec(&serde_json::json!([
                {
                    "created_at": "2026-06-03T10:05:00Z",
                    "user": {"login": "ub-review[bot]"},
                    "state": "COMMENTED",
                    "body": "ub-review previous question resolved by the receipt."
                }
            ]))?
        } else if request_line.contains("/pulls/76/comments?per_page=50") {
            serde_json::to_vec(&serde_json::json!([
                {
                    "created_at": "2026-06-03T10:10:00Z",
                    "user": {"login": "maintainer"},
                    "path": "src/lib.rs",
                    "line": 12,
                    "body": "Inline thread points at the route proof receipt."
                }
            ]))?
        } else {
            serde_json::to_vec(&serde_json::json!([]))?
        };
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        )?;
        stream.write_all(&response_body)?;
        Ok(headers)
    }

    fn join_fake_github_thread_api(
        handle: thread::JoinHandle<Result<Vec<String>>>,
    ) -> Result<Vec<String>> {
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("fake GitHub thread API thread panicked"))?
    }
}
