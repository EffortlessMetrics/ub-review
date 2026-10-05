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

/// The existing run artifact, frozen before this post attempt. No prior
/// post-result/error file is used to confirm a new attempt.
pub(crate) struct PostPublication {
    path: PathBuf,
    source: serde_json::Value,
    review_sha256: Option<String>,
    expected_pull_number: Option<u64>,
    not_needed: bool,
}

fn publication_bytes(path: &Path) -> Result<Vec<u8>> {
    publication_reader_bytes(fs::File::open(path)?)
}

fn publication_reader_bytes(mut reader: impl std::io::Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 1_048_576, "publication input exceeds 1 MiB");
    Ok(bytes)
}

fn publication_source(mut gate: serde_json::Value) -> Result<serde_json::Value> {
    let object = gate
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("gate outcome must be an object"))?;
    anyhow::ensure!(
        object.get("schema").and_then(serde_json::Value::as_str)
            == Some("ub-review.gate_outcome.v1"),
        "unsupported gate outcome schema"
    );
    for field in [
        "publication_result",
        "gate_result",
        "delivery_result",
        "delivery_attempt",
        "delivery_reason",
        "delivery_receipt_sha256",
        "delivery_review_sha256",
    ] {
        object.remove(field);
    }
    if let Some(reasons) = object
        .get_mut("not_proven_reasons")
        .and_then(serde_json::Value::as_array_mut)
    {
        reasons.retain(|reason| {
            !reason
                .as_str()
                .is_some_and(|text| text.starts_with("publication:"))
        });
    }
    Ok(gate)
}

pub(crate) fn begin_post_publication(args: &PostArgs) -> Result<Option<PostPublication>> {
    let path = args
        .review_json
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("gate_outcome.json");
    if !path.exists() {
        return Ok(None); // Standalone posting keeps its existing contract.
    }
    let mut gate: serde_json::Value = serde_json::from_slice(&publication_bytes(&path)?)?;
    let source = publication_source(gate.clone())?;
    anyhow::ensure!(
        matches!(
            source["code_gate_result"].as_str(),
            Some("pass" | "finding" | "not_proven")
        ),
        "gate outcome has no supported independent code result"
    );
    anyhow::ensure!(
        source["not_proven_reasons"].is_array(),
        "gate outcome reasons must be an array"
    );
    let not_needed = gate["publication_result"] == "not_needed";
    let review_sha256 = args
        .review_json
        .exists()
        .then(|| publication_bytes(&args.review_json).map(|bytes| sha256_hex(&bytes)))
        .transpose()?;
    gate["publication_result"] = if not_needed && review_sha256.is_none() {
        "not_needed".into()
    } else {
        "not_proven".into()
    };
    gate["delivery_result"] = if review_sha256.is_some() {
        "prepared"
    } else {
        "unknown"
    }
    .into();
    gate["delivery_attempt"] = "not_attempted".into();
    gate["delivery_reason"] = "post_confirmation_unavailable".into();
    if let Some(object) = gate.as_object_mut() {
        object.remove("delivery_receipt_sha256");
    }
    gate["delivery_review_sha256"] = serde_json::to_value(&review_sha256)?;
    project_publication_result(&mut gate, "post_confirmation_unavailable")?;
    fs::write(&path, serde_json::to_vec_pretty(&gate)?)?;
    Ok(Some(PostPublication {
        path,
        source,
        review_sha256,
        expected_pull_number: args.pull_number.or_else(detect_pull_number_from_event),
        not_needed,
    }))
}

fn positive_review_id(value: &serde_json::Value) -> bool {
    value
        .as_u64()
        .or_else(|| value.as_str()?.parse().ok())
        .is_some_and(|id| id > 0)
}

fn project_publication_result(gate: &mut serde_json::Value, reason: &str) -> Result<()> {
    let publication_unproven = matches!(
        gate["publication_result"].as_str(),
        Some("failed" | "not_proven")
    );
    gate["gate_result"] = if publication_unproven {
        "not_proven".into()
    } else {
        gate["code_gate_result"].clone()
    };
    let reasons = gate
        .get_mut("not_proven_reasons")
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| anyhow::anyhow!("gate outcome reasons must be an array"))?;
    reasons.retain(|reason| {
        !reason
            .as_str()
            .is_some_and(|text| text.starts_with("publication:"))
    });
    if publication_unproven {
        reasons.push(format!("publication: {reason}").into());
    }
    Ok(())
}

fn post_publication_state(
    args: &PostArgs,
    publication: &PostPublication,
    receipt: &serde_json::Value,
) -> (&'static str, &'static str, &'static str, &'static str) {
    if receipt["schema_version"] != 1 {
        return ("not_proven", "unknown", "unknown", "invalid_post_receipt");
    }
    match receipt["status"].as_str() {
        Some("failed") => {
            let stage = receipt["failure_stage"].as_str();
            let attempt = match stage {
                Some("preflight" | "payload_validation") => "blocked",
                Some("network_post") => "attempted",
                _ => "unknown",
            };
            let reason = if receipt["error_kind"] == "missing_token" {
                "missing_token"
            } else if receipt["error_kind"] == "receipt_persistence" {
                "receipt_persistence"
            } else {
                "post_failed"
            };
            ("failed", "failed", attempt, reason)
        }
        Some("skipped") if publication.not_needed && publication.review_sha256.is_none() => (
            "not_needed",
            "not_needed",
            "not_attempted",
            "public_value_not_needed",
        ),
        Some("ok") => {
            let confirmation = &receipt["response"]["delivery_confirmation"];
            let revision =
                serde_json::from_value::<RevisionRef>(publication.source["revision"].clone());
            let current_revision = revision.as_ref().is_ok_and(|value| {
                value.validate().is_ok()
                    && receipt["response"]
                        .get("delivery_confirmation")
                        .is_none_or(|current| {
                            matches!(
                                current["kind"].as_str(),
                                Some("submitted_review" | "reconciled_comments")
                            ) && current["exact_head_sha"] == value.reviewed_commit
                        })
                    && match receipt["response"].get("commit_id") {
                        Some(commit) => commit.as_str() == Some(value.reviewed_commit.as_str()),
                        None => {
                            matches!(
                                confirmation["kind"].as_str(),
                                Some("submitted_review" | "reconciled_comments")
                            ) && confirmation["exact_head_sha"] == value.reviewed_commit
                        }
                    }
            });
            let grouped = matches!(
                receipt["response"]["state"].as_str(),
                Some("COMMENTED" | "commented")
            ) && positive_review_id(&receipt["response"]["id"])
                && matches!(
                    confirmation["kind"].as_str(),
                    None | Some("submitted_review")
                );
            let planned = confirmation["planned_count"].as_u64();
            let comments = confirmation["kind"] == "reconciled_comments"
                && planned.is_some_and(|count| count > 0)
                && confirmation["confirmed_count"].as_u64() == planned
                && receipt["comments"].as_u64() == planned
                && (receipt["response"]["state"] == "already_delivered"
                    || (receipt["response"]["state"] == "commented"
                        && positive_review_id(&receipt["response"]["id"])));
            let valid = current_revision
                && matches!(
                    publication.source["code_gate_result"].as_str(),
                    Some("pass" | "finding" | "not_proven")
                )
                && publication.review_sha256.is_some()
                && (grouped || comments)
                && receipt["http_status"]
                    .as_u64()
                    .is_some_and(|value| (199..301).contains(&value))
                && receipt["repo"].as_str().is_some_and(|repo| {
                    is_valid_repo_slug(repo) && args.repo.as_deref() == Some(repo)
                })
                && receipt["pull_number"].as_u64().is_some_and(|number| {
                    number > 0 && publication.expected_pull_number == Some(number)
                })
                && receipt["review_json"] == args.review_json.display().to_string()
                && receipt["repo_valid"] == true
                && receipt["review_json_exists"] == true
                && receipt["review_json_valid"] == true
                && receipt["token_present"] == true
                && receipt["payload_written"] == true;
            if valid {
                (
                    "posted",
                    "confirmed",
                    if receipt["response"]["state"] == "already_delivered" {
                        "not_attempted"
                    } else {
                        "attempted"
                    },
                    "current_revision_confirmed",
                )
            } else {
                (
                    "not_proven",
                    "unknown",
                    "attempted",
                    "post_confirmation_unverifiable",
                )
            }
        }
        _ => (
            "not_proven",
            "unknown",
            "unknown",
            "post_confirmation_unavailable",
        ),
    }
}

pub(crate) fn finalize_post_publication(
    args: &PostArgs,
    publication: &PostPublication,
    receipt: &serde_json::Value,
) -> Result<()> {
    let mut gate: serde_json::Value =
        serde_json::from_slice(&publication_bytes(&publication.path)?)?;
    anyhow::ensure!(
        publication_source(gate.clone())? == publication.source,
        "gate source changed during posting"
    );
    let current = args
        .review_json
        .exists()
        .then(|| publication_bytes(&args.review_json).map(|bytes| sha256_hex(&bytes)))
        .transpose()?;
    let state = if current == publication.review_sha256 {
        post_publication_state(args, publication, receipt)
    } else {
        (
            "not_proven",
            "unknown",
            "unknown",
            "prepared_review_changed",
        )
    };
    gate["publication_result"] = state.0.into();
    gate["delivery_result"] = state.1.into();
    gate["delivery_attempt"] = state.2.into();
    gate["delivery_reason"] = state.3.into();
    project_publication_result(&mut gate, state.3)?;
    gate["delivery_receipt_sha256"] = sha256_hex(&serde_json::to_vec(receipt)?).into();
    gate["delivery_review_sha256"] = serde_json::to_value(&publication.review_sha256)?;
    fs::write(&publication.path, serde_json::to_vec_pretty(&gate)?)?;
    Ok(())
}

pub(crate) fn write_post_receipt_and_finalize(
    args: &PostArgs,
    publication: Option<&PostPublication>,
    filename: &str,
    receipt: &impl Serialize,
) -> Result<()> {
    let value = serde_json::to_value(receipt)?;
    if let Err(error) = fs::write(args.out.join(filename), serde_json::to_vec_pretty(&value)?) {
        if let Some(publication) = publication {
            finalize_post_publication(
                args,
                publication,
                &serde_json::json!({
                    "schema_version":1, "status":"failed", "error_kind":"receipt_persistence",
                    "failure_stage":"receipt_persistence"
                }),
            )?;
        }
        return Err(error.into());
    }
    if let Some(publication) = publication {
        finalize_post_publication(args, publication, &value)?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "post_publication_tests.rs"]
mod publication_tests;
