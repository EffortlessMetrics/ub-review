//! Review text rendering: observation rendering, PR signal formatting,
//! proof receipt summaries, body section helpers, diff line parsing,
//! and hashing utilities (cleanup train step 51, pure code motion).

use crate::*;

pub(crate) fn render_review_observation(
    text: &mut String,
    observation: &ObservationGroup,
    tone: PrObservationTone,
) {
    match tone {
        PrObservationTone::Signal => render_pr_model_signal(text, &observation.claim),
        PrObservationTone::Verification => render_pr_model_verification(text, &observation.claim),
    }
}

pub(crate) fn render_pr_signal(text: &mut String, value: &str) {
    let sentence = pr_sentence(value);
    text.push_str(&format!("- {}\n", escape_md(&sentence)));
}

pub(crate) fn render_pr_verification(text: &mut String, value: &str) {
    let sentence = verification_sentence(value);
    text.push_str(&format!("- {}\n", escape_md(&sentence)));
}

pub(crate) fn render_pr_model_signal(text: &mut String, value: &str) {
    render_pr_signal(text, &reviewer_facing_pr_text(value));
}

pub(crate) fn render_pr_model_verification(text: &mut String, value: &str) {
    render_pr_verification(text, &reviewer_facing_pr_text(value));
}

pub(crate) fn reviewer_facing_pr_text(value: &str) -> String {
    let mut text = value.trim();
    if let Some(stripped) = strip_bracketed_lane_prefix(text) {
        text = stripped;
    }
    if let Some(stripped) = strip_raw_lane_metadata_prefix(text) {
        text = stripped;
    }
    strip_embedded_evidence_label(text).trim().to_owned()
}

pub(crate) fn strip_bracketed_lane_prefix(value: &str) -> Option<&str> {
    let trimmed = value.trim_start();
    if !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    if end > 80 {
        return None;
    }
    Some(trimmed[end + 1..].trim_start())
}

pub(crate) fn strip_raw_lane_metadata_prefix(value: &str) -> Option<&str> {
    let lower = value.to_ascii_lowercase();
    let at_index = lower.find(" at ")?;
    let prefix = lower[..at_index].trim();
    if !prefix
        .split_whitespace()
        .all(|token| matches!(token, "blocker" | "high" | "medium" | "low" | "medium-high"))
    {
        return None;
    }
    let after_at = &value[at_index + 4..];
    let body_index = after_at.find(": ")?;
    Some(after_at[body_index + 2..].trim_start())
}

pub(crate) fn strip_embedded_evidence_label(value: &str) -> &str {
    for marker in [" Evidence:", " evidence:"] {
        if let Some(index) = value.find(marker) {
            return value[..index].trim_end();
        }
    }
    value
}

pub(crate) fn render_proof_receipt_summary(text: &mut String, receipt: &ProofReceipt) {
    let command = receipt
        .commands
        .first()
        .map(|command| command.command.as_str())
        .unwrap_or("focused test");
    let head_status =
        proof_command_outcome(receipt, "head").unwrap_or_else(|| "HEAD status unknown".to_owned());
    let head_only_status = proof_command_status_for_side(receipt, "head")
        .unwrap_or_else(|| "status unknown".to_owned());
    let base_plus_tests_status = proof_command_outcome(receipt, "base-plus-tests")
        .unwrap_or_else(|| "base+tests status unknown".to_owned());
    let summary = match receipt.result.as_str() {
        "head_passed" if receipt.kind == "focused-build" => {
            format!("Focused build proof passed: `{command}`.")
        }
        "head_failed" if receipt.kind == "focused-build" => {
            format!("Focused build proof failed: `{command}`.")
        }
        "discriminating" => format!(
            "Focused red/green proof discriminates the patch: {head_status} and {base_plus_tests_status} for `{command}`."
        ),
        "non_discriminating" => format!(
            "Focused red/green proof did not discriminate the patch: {head_status} and {base_plus_tests_status} for `{command}`."
        ),
        "head_passed" => format!(
            "Focused HEAD proof {head_only_status}: `{command}`. Base+tests red/green was not run in this v0 proof."
        ),
        "head_failed" => format!(
            "Focused HEAD proof {head_only_status}: `{command}`. This is a current failure, not a red/green witness."
        ),
        _ => format!(
            "Focused proof result `{}` for `{}` is recorded in artifacts.",
            receipt.result, command
        ),
    };
    render_pr_signal(text, &summary);
}

pub(crate) fn proof_command_outcome(receipt: &ProofReceipt, side: &str) -> Option<String> {
    let command = receipt
        .commands
        .iter()
        .find(|command| command.side == side)?;
    let side_label = match side {
        "head" => "HEAD",
        "base-plus-tests" => "base+tests",
        other => other,
    };
    let outcome = format!("{side_label} {}", proof_command_status(command));
    Some(outcome)
}

pub(crate) fn proof_command_status_for_side(receipt: &ProofReceipt, side: &str) -> Option<String> {
    receipt
        .commands
        .iter()
        .find(|command| command.side == side)
        .map(proof_command_status)
}

pub(crate) fn proof_command_status(command: &ProofCommandReceipt) -> String {
    let mut outcome = command.status.clone();
    if let Some(exit_code) = command.exit_code {
        outcome.push_str(&format!(" (exit {exit_code})"));
    }
    if command.timed_out && !outcome.contains("timed_out") {
        outcome.push_str(" (timed out)");
    }
    outcome
}

pub(crate) fn render_missing_proof_receipt_summary(text: &mut String, receipt: &ProofReceipt) {
    let command = receipt
        .commands
        .first()
        .map(|command| command.command.as_str())
        .unwrap_or("focused test");
    let summary = match receipt.result.as_str() {
        "base_patch_failed" => format!(
            "Base+tests proof was unavailable for `{command}` because the test-only patch did not apply cleanly."
        ),
        "non_discriminating" => {
            let head_status = proof_command_outcome(receipt, "head")
                .unwrap_or_else(|| "HEAD status unknown".to_owned());
            let base_plus_tests_status = proof_command_outcome(receipt, "base-plus-tests")
                .unwrap_or_else(|| "base+tests status unknown".to_owned());
            format!(
                "Focused red/green proof did not discriminate the patch: {head_status} and {base_plus_tests_status} for `{command}`."
            )
        }
        "timed_out" => format!("Focused proof timed out for `{command}`; logs are in artifacts."),
        "skipped_budget" => {
            format!(
                "Focused proof was skipped by budget for `{command}`; plan details are in artifacts."
            )
        }
        "skipped_profile" => {
            format!(
                "Focused proof was unavailable for `{command}`; profile/tool details are in artifacts."
            )
        }
        _ => format!(
            "Focused proof result `{}` for `{}` needs artifact review.",
            receipt.result, command
        ),
    };
    render_pr_signal(text, &summary);
}

pub(crate) fn verification_sentence(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "Confirm the unresolved review question.".to_owned();
    }
    let without_trailing = trimmed.trim_end_matches(&['.', '!', '?'][..]).trim();
    if without_trailing.is_empty() {
        return "Confirm the unresolved review question.".to_owned();
    }
    if is_actionable_verification_sentence(trimmed) {
        return pr_sentence(trimmed);
    }
    format!("Confirm {}.", lower_first_ascii(without_trailing))
}

pub(crate) fn is_actionable_verification_sentence(value: &str) -> bool {
    let normalized = value.trim_start().to_ascii_lowercase();
    value.trim_end().ends_with('?')
        || [
            "confirm ", "verify ", "check ", "ensure ", "run ", "add ", "can ", "does ", "do ",
            "is ", "are ", "will ", "should ", "could ", "did ",
        ]
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
}

pub(crate) fn pr_sentence(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "See review artifacts for the recorded evidence.".to_owned();
    }
    if trimmed
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, '.' | '!' | '?'))
    {
        trimmed.to_owned()
    } else {
        format!("{trimmed}.")
    }
}

pub(crate) fn lower_first_ascii(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_uppercase() => {
            format!("{}{}", first.to_ascii_lowercase(), chars.as_str())
        }
        Some(_) => value.to_owned(),
        None => String::new(),
    }
}

pub(crate) fn review_decision(
    missing_or_failed_sensor_evidence: &[SensorEvidenceIssue],
    missing_or_failed_model_evidence: &[ModelEvidenceIssue],
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
) -> &'static str {
    if has_actionable_review_finding(inline_comments, summary_only_findings) {
        "Needs reviewer attention before upstream: grounded findings or summary-only concerns remain."
    } else if !missing_or_failed_sensor_evidence.is_empty()
        || !missing_or_failed_model_evidence.is_empty()
    {
        "No blocking finding after bounded review; evidence is incomplete."
    } else {
        "No blocking finding after bounded review; residual risk remains for human review."
    }
}

pub(crate) fn has_actionable_review_finding(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
) -> bool {
    inline_comments
        .iter()
        .any(|comment| matches!(comment.severity.as_str(), "blocker" | "high" | "medium"))
        || summary_only_findings
            .iter()
            .any(|finding| matches!(finding.severity.as_str(), "blocker" | "high" | "medium"))
}

pub(crate) fn has_reviewer_value(inline_comments: &[ReviewInlineComment], pr_body: &str) -> bool {
    !inline_comments.is_empty() || pr_body_has_reviewer_value(pr_body)
}

pub(crate) fn is_parked_follow_up(finding: &SummaryOnlyFinding) -> bool {
    let reason = finding.reason.to_ascii_lowercase();
    let evidence = finding.evidence.to_ascii_lowercase();
    reason.contains("parked")
        || reason.contains("follow-up")
        || evidence.contains("parked")
        || evidence.contains("follow-up")
}

pub(crate) const REVIEW_BODY_TRUNCATED_SUFFIX: &str =
    "\n\n[review body truncated; see review artifacts]\n";
const REVIEW_BODY_REQUIRED_HEADINGS: [&str; 7] = [
    "## Decision",
    "## Confirmed findings",
    "## Summary-only findings",
    "## Failed objections",
    "## Residual risk",
    "## Parked follow-ups",
    "## Missing or failed evidence",
];

pub(crate) fn cap_review_body(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    if REVIEW_BODY_REQUIRED_HEADINGS
        .iter()
        .all(|heading| text.contains(heading))
        && let Some(compact) = compact_review_body_sections(&text, max_bytes)
    {
        return compact;
    }
    cap_text_prefix(text, max_bytes)
}

pub(crate) fn cap_review_body_bullets(text: String, max_bullets: usize) -> String {
    let mut bullets = 0usize;
    let mut dropped = false;
    let mut output = String::with_capacity(text.len());
    for line in text.lines() {
        let trimmed = line.trim_start();
        let is_bullet = trimmed.starts_with("- ") || trimmed.starts_with("* ");
        if is_bullet {
            if bullets >= max_bullets {
                dropped = true;
                continue;
            }
            bullets += 1;
        }
        output.push_str(line);
        output.push('\n');
    }
    if dropped {
        output.push_str(REVIEW_BODY_TRUNCATED_SUFFIX);
    }
    output
}

pub(crate) fn compact_review_body_sections(text: &str, max_bytes: usize) -> Option<String> {
    for section_budget in [180, 120, 80, 48, 0] {
        let mut compact = String::new();
        if let Some(first_heading) = first_required_heading_index(text) {
            let prefix = text[..first_heading].trim_end();
            append_review_excerpt(&mut compact, prefix, 220);
            compact.push('\n');
        }
        for (index, heading) in REVIEW_BODY_REQUIRED_HEADINGS.iter().enumerate() {
            compact.push('\n');
            compact.push_str(heading);
            compact.push_str("\n\n");
            let next_heading = REVIEW_BODY_REQUIRED_HEADINGS.get(index + 1).copied();
            let section = review_body_section(text, heading, next_heading)?;
            append_review_excerpt(&mut compact, section, section_budget);
        }
        compact.push_str(REVIEW_BODY_TRUNCATED_SUFFIX);
        if compact.len() <= max_bytes {
            return Some(compact);
        }
    }
    None
}

pub(crate) fn first_required_heading_index(text: &str) -> Option<usize> {
    REVIEW_BODY_REQUIRED_HEADINGS
        .iter()
        .filter_map(|heading| text.find(heading))
        .min()
}

pub(crate) fn review_body_section<'a>(
    text: &'a str,
    heading: &str,
    next_heading: Option<&str>,
) -> Option<&'a str> {
    let start = text.find(heading)? + heading.len();
    let rest = &text[start..];
    let end = next_heading.and_then(|heading| rest.find(heading));
    Some(end.map_or(rest, |end| &rest[..end]))
}

pub(crate) fn append_review_excerpt(out: &mut String, text: &str, max_bytes: usize) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        out.push_str("- See review artifacts for full section.\n");
        return;
    }
    let line = trimmed
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("- See review artifacts for full section.");
    if max_bytes == 0 {
        out.push_str("- See review artifacts for full section.\n");
        return;
    }
    let mut excerpt = utf8_prefix(line, max_bytes);
    if excerpt.is_empty() {
        excerpt = "- See review artifacts for full section.".to_owned();
    }
    out.push_str(&excerpt);
    if line.len() > excerpt.len() {
        out.push_str(" ...");
    }
    out.push('\n');
}

pub(crate) fn utf8_prefix(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut boundary = max_bytes.min(text.len());
    while !text.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    text[..boundary].to_owned()
}

pub(crate) fn cap_text_prefix(mut text: String, max_bytes: usize) -> String {
    let keep = max_bytes
        .saturating_sub(REVIEW_BODY_TRUNCATED_SUFFIX.len())
        .max(1);
    let mut boundary = keep.min(text.len());
    while !text.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    text.truncate(boundary);
    text.push_str(REVIEW_BODY_TRUNCATED_SUFFIX);
    text
}

pub(crate) fn right_side_diff_lines(patch: &str) -> BTreeSet<(String, u32)> {
    let mut lines = BTreeSet::new();
    let mut current_path = String::new();
    let mut new_line: Option<u32> = None;
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            current_path = normalize_repo_path(path);
            continue;
        }
        if line.starts_with("@@") {
            new_line = parse_hunk_new_start(line);
            continue;
        }
        let Some(line_no) = new_line else {
            continue;
        };
        if current_path.is_empty() {
            continue;
        }
        let is_right_line =
            (line.starts_with('+') && !line.starts_with("+++")) || line.starts_with(' ');
        if is_right_line {
            lines.insert((current_path.clone(), line_no));
            new_line = line_no.checked_add(1);
        } else if line.starts_with('-') && !line.starts_with("---") {
            new_line = Some(line_no);
        } else if !line.starts_with('\\') {
            new_line = line_no.checked_add(1);
        }
    }
    lines
}

pub(crate) fn parse_hunk_new_start(line: &str) -> Option<u32> {
    let plus = line.split_whitespace().find(|part| part.starts_with('+'))?;
    let start = plus
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse::<u32>()
        .ok()?;
    Some(start)
}

pub(crate) fn normalize_repo_path(path: &str) -> String {
    path.trim().trim_start_matches("b/").replace('\\', "/")
}

pub(crate) fn ensure_lane_prefix(lane: &str, body: &str) -> String {
    let prefix = format!("[{lane}]");
    if body.starts_with(&prefix) {
        body.to_owned()
    } else {
        format!("{prefix} {body}")
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn github_bearer_auth_header(token: &str) -> String {
    let header_name = ["Author", "ization"].concat();
    let scheme = ["Bear", "er"].concat();
    format!("{header_name}: {scheme} {token}")
}

fn expected_pr_head_sha(args: &PostArgs) -> Option<String> {
    if let Some(event_path) = std::env::var_os("GITHUB_EVENT_PATH")
        && let Ok(bytes) = fs::read(event_path)
        && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
        && let Some(sha) = value
            .pointer("/pull_request/head/sha")
            .and_then(serde_json::Value::as_str)
            .filter(|sha| !sha.trim().is_empty())
    {
        return Some(sha.to_owned());
    }

    let claim_graph_path = args.review_json.parent()?.join("claim_graph.json");
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(claim_graph_path).ok()?).ok()?;
    value
        .get("head_sha")
        .and_then(serde_json::Value::as_str)
        .filter(|sha| !sha.trim().is_empty())
        .map(ToOwned::to_owned)
}

fn verify_current_pr_head(
    args: &PostArgs,
    repo: &str,
    pull_number: u64,
    token: &str,
) -> Result<String> {
    let receipt_path = args.out.join("post-head-check.json");
    let Some(expected_sha) = expected_pr_head_sha(args) else {
        fs::write(
            &receipt_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "status": "unavailable",
                "reason": "no pull_request.head.sha event field or claim graph head_sha was available",
                "repo": repo,
                "pull_number": pull_number
            }))?,
        )?;
        bail!(
            "current pull request head is unavailable; refusing to post without an expected head SHA"
        );
    };

    let url = format!(
        "{}/repos/{repo}/pulls/{pull_number}",
        args.github_api_url.trim_end_matches('/')
    );
    let response = match run_github_api_get(Path::new("."), &url, token) {
        Ok(response) => response,
        Err(err) => {
            fs::write(
                &receipt_path,
                serde_json::to_vec_pretty(&serde_json::json!({
                    "status": "failed",
                    "expected_head_sha": expected_sha,
                    "reason": format!("current PR head lookup failed: {err:#}")
                }))?,
            )?;
            return Err(err).context("verify current GitHub pull request head");
        }
    };
    let current_sha = response
        .pointer("/head/sha")
        .and_then(serde_json::Value::as_str)
        .filter(|sha| !sha.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("GitHub pull request response omitted head.sha"))?;
    let matched = current_sha == expected_sha;
    fs::write(
        &receipt_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "status": if matched { "matched" } else { "mismatch" },
            "expected_head_sha": expected_sha,
            "current_head_sha": current_sha,
            "repo": repo,
            "pull_number": pull_number
        }))?,
    )?;
    if !matched {
        bail!(
            "current pull request head changed before posting (expected {expected_sha}, got {current_sha})"
        );
    }
    Ok(expected_sha)
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
    let reply_candidates = read_reply_candidates(args)?;
    let replies = reply_candidates
        .as_ref()
        .map(|artifact| artifact.replies.as_slice())
        .unwrap_or_default();
    if replies.is_empty() {
        validate_github_review_payload_for_post(args, &review)?;
    } else if review.event != "COMMENT" {
        bail!("github review event must be COMMENT when reply candidates are present");
    } else if !review.body.trim().is_empty() {
        let policy = post_review_body_policy(args);
        validate_pr_review_body_policy_with_waiver(
            &review.body,
            &policy,
            summary_only_body_waives_post_validation(&policy),
        )?;
    }
    if !replies.is_empty() && review.comments.is_empty() {
        let expected_sha = verify_current_pr_head(args, repo, pull_number, token)?;
        let artifact = reply_candidates
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("reply candidates disappeared while posting"))?;
        if artifact.head_sha != expected_sha {
            bail!(
                "reply candidate head {} does not match expected current head {}",
                artifact.head_sha,
                expected_sha
            );
        }
        let posted_reply_ids = post_github_replies(args, repo, pull_number, token, replies)?;
        return Ok(reply_only_post_result(
            args,
            repo,
            pull_number,
            &review,
            &expected_sha,
            posted_reply_ids,
        ));
    }
    let expected_sha = verify_current_pr_head(args, repo, pull_number, token)?;
    let comments = github_review_post_comments(&review)?;
    let pending_payload = GitHubPendingReviewPayload {
        commit_id: expected_sha.clone(),
        body: review.body.clone(),
        comments,
    };
    let legacy_payload = github_review_post_payload(&review)?;
    fs::write(
        args.out.join("github-review-post-payload.json"),
        serde_json::to_vec_pretty(&legacy_payload)?,
    )?;
    let pending_path = args.out.join("github-review-pending-payload.json");
    fs::write(&pending_path, serde_json::to_vec_pretty(&pending_payload)?)?;
    if let Some(artifact) = &reply_candidates
        && artifact.head_sha != expected_sha
    {
        bail!(
            "reply candidate head {} does not match expected current head {}",
            artifact.head_sha,
            expected_sha
        );
    }
    let url = format!(
        "{}/repos/{}/pulls/{}/reviews",
        args.github_api_url.trim_end_matches('/'),
        repo,
        pull_number
    );
    let pending_output = run_curl_json_post(
        Path::new("."),
        &url,
        &github_bearer_auth_header(token),
        &pending_path,
        &[
            "Accept: application/vnd.github+json",
            "Content-Type: application/json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    )
    .with_context(|| "create pending GitHub review")?;
    let pending_text = String::from_utf8_lossy(&pending_output.stdout).to_string();
    fs::write(args.out.join("pending-review-stdout.json"), &pending_text)?;
    fs::write(
        args.out.join("pending-review-stderr.txt"),
        String::from_utf8_lossy(&pending_output.stderr).as_ref(),
    )?;
    if !http_output_succeeded(&pending_output) {
        bail!(
            "pending GitHub review failed with exit code {:?} and http status {:?}: {}",
            pending_output.status.code(),
            pending_output.http_status,
            String::from_utf8_lossy(&pending_output.stderr)
        );
    }
    let pending_response: serde_json::Value = serde_json::from_str(&pending_text)
        .with_context(|| "parse pending GitHub review response")?;
    let review_id = pending_response
        .get("id")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| anyhow::anyhow!("pending GitHub review response omitted numeric id"))?;
    let pending_comments_url = format!("{url}/{review_id}/comments");
    let comments_output = match run_curl_json_request(
        Path::new("."),
        "GET",
        &pending_comments_url,
        &github_bearer_auth_header(token),
        None,
        &[
            "Accept: application/vnd.github+json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    ) {
        Ok(output) => output,
        Err(err) => {
            return Err(cleanup_pending_review_error(
                args, &url, review_id, token, err,
            ));
        }
    };
    let comments_text = String::from_utf8_lossy(&comments_output.stdout).to_string();
    if let Err(err) = fs::write(
        args.out.join("pending-review-comments.json"),
        &comments_text,
    ) {
        return Err(cleanup_pending_review_error(
            args,
            &url,
            review_id,
            token,
            err.into(),
        ));
    }
    let posted_comment_ids = match parse_github_review_comment_ids(&comments_text) {
        Ok(ids) => ids,
        Err(err) => {
            return Err(cleanup_pending_review_error(
                args, &url, review_id, token, err,
            ));
        }
    };
    if !http_output_succeeded(&comments_output)
        || posted_comment_ids.len() != pending_payload.comments.len()
    {
        let deleted = delete_pending_github_review(args, &url, review_id, token);
        bail!(
            "pending GitHub review comment receipt was incomplete (expected {}, got {}); cleanup={deleted}",
            pending_payload.comments.len(),
            posted_comment_ids.len()
        );
    }
    let submit_path = args.out.join("github-review-submit-payload.json");
    let submit_payload = GitHubSubmitReviewPayload {
        event: review.event.clone(),
        body: review.body.clone(),
    };
    let submit_bytes = match serde_json::to_vec_pretty(&submit_payload) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Err(cleanup_pending_review_error(
                args,
                &url,
                review_id,
                token,
                err.into(),
            ));
        }
    };
    if let Err(err) = fs::write(&submit_path, submit_bytes) {
        return Err(cleanup_pending_review_error(
            args,
            &url,
            review_id,
            token,
            err.into(),
        ));
    }
    // Revalidate immediately before submission. The pending review and its
    // inline comments may have taken long enough for the PR head to advance;
    // posting a review pinned to the earlier commit would create a stale
    // human-facing receipt. A mismatch cleans up the pending review and
    // leaves the post stage fail-closed.
    if let Err(err) = verify_current_pr_head(args, repo, pull_number, token) {
        return Err(cleanup_pending_review_error(
            args, &url, review_id, token, err,
        ));
    }
    let submit_output = match run_curl_json_post(
        Path::new("."),
        &format!("{url}/{review_id}/events"),
        &github_bearer_auth_header(token),
        &submit_path,
        &[
            "Accept: application/vnd.github+json",
            "Content-Type: application/json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    ) {
        Ok(output) => output,
        Err(err) => {
            return Err(cleanup_pending_review_error(
                args, &url, review_id, token, err,
            ));
        }
    };
    let submit_text = String::from_utf8_lossy(&submit_output.stdout).to_string();
    if let Err(err) = fs::write(args.out.join("submit-review-stdout.json"), &submit_text) {
        return Err(cleanup_pending_review_error(
            args,
            &url,
            review_id,
            token,
            err.into(),
        ));
    }
    if let Err(err) = fs::write(
        args.out.join("submit-review-stderr.txt"),
        String::from_utf8_lossy(&submit_output.stderr).as_ref(),
    ) {
        return Err(cleanup_pending_review_error(
            args,
            &url,
            review_id,
            token,
            err.into(),
        ));
    }
    if !http_output_succeeded(&submit_output) {
        let deleted = delete_pending_github_review(args, &url, review_id, token);
        bail!(
            "GitHub review submit failed with exit code {:?} and http status {:?}; cleanup={deleted}",
            submit_output.status.code(),
            submit_output.http_status
        );
    }
    write_inline_delivery_receipts(
        &args.out,
        &review,
        &expected_sha,
        review_id,
        &posted_comment_ids,
    )?;
    let posted_reply_ids = if replies.is_empty() {
        Vec::new()
    } else {
        post_github_replies(args, repo, pull_number, token, replies)?
    };
    let response = serde_json::from_str(&submit_text)
        .unwrap_or_else(|_| serde_json::json!({"raw": submit_text}));
    let review_metadata = read_github_review_metadata(args);
    Ok(PostResultReceipt {
        schema_version: 1,
        status: "ok".to_owned(),
        repo: repo.clone(),
        repo_valid: true,
        pull_number,
        comments: posted_comment_ids.len(),
        reply_count: posted_reply_ids.len(),
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
        http_status: submit_output.http_status,
        token_present: true,
        payload_written: pending_path.exists() && submit_path.exists(),
        post_stdout_written: args.out.join("submit-review-stdout.json").exists(),
        post_stderr_written: args.out.join("submit-review-stderr.txt").exists(),
        response,
        delivery_status: "submitted".to_owned(),
        review_id: Some(review_id),
        posted_comment_ids,
        posted_reply_ids,
        reply_delivery_status: if replies.is_empty() {
            "none".to_owned()
        } else {
            "posted".to_owned()
        },
        submitted: true,
        pending_review_deleted: false,
        head_sha: Some(expected_sha),
    })
}

fn reply_only_post_result(
    args: &PostArgs,
    repo: &str,
    pull_number: u64,
    review: &GitHubReview,
    head_sha: &str,
    posted_reply_ids: Vec<u64>,
) -> PostResultReceipt {
    PostResultReceipt {
        schema_version: 1,
        status: "ok".to_owned(),
        repo: repo.to_owned(),
        repo_valid: true,
        pull_number,
        comments: 0,
        reply_count: posted_reply_ids.len(),
        review_json: args.review_json.display().to_string(),
        review_json_exists: args.review_json.exists(),
        review_json_valid: true,
        review_event: Some(review.event.clone()),
        review_body_bytes: Some(review.body.len()),
        review_comment_count: Some(0),
        diff_patch: post_diff_patch_path(args).display().to_string(),
        diff_patch_exists: false,
        diff_patch_valid: false,
        diff_line_count: None,
        off_diff_comment_count: Some(0),
        http_status: None,
        token_present: true,
        payload_written: args.out.join("reply-delivery.json").exists(),
        post_stdout_written: false,
        post_stderr_written: false,
        response: serde_json::json!({"reply_comment_ids": posted_reply_ids}),
        delivery_status: "replies_submitted".to_owned(),
        review_id: None,
        posted_comment_ids: Vec::new(),
        posted_reply_ids,
        reply_delivery_status: "posted".to_owned(),
        submitted: false,
        pending_review_deleted: false,
        head_sha: Some(head_sha.to_owned()),
    }
}

fn http_output_succeeded(output: &HttpPostOutput) -> bool {
    output.status.success()
        && output
            .http_status
            .is_none_or(|status| (200..300).contains(&status))
}

fn cleanup_pending_review_error(
    args: &PostArgs,
    url: &str,
    review_id: u64,
    token: &str,
    error: anyhow::Error,
) -> anyhow::Error {
    let deleted = delete_pending_github_review(args, url, review_id, token);
    error.context(format!("pending GitHub review cleanup={deleted}"))
}

fn delete_pending_github_review(args: &PostArgs, url: &str, review_id: u64, token: &str) -> bool {
    let output = run_curl_json_request(
        Path::new("."),
        "DELETE",
        &format!("{url}/{review_id}"),
        &github_bearer_auth_header(token),
        None,
        &[
            "Accept: application/vnd.github+json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    );
    let ok = output.as_ref().is_ok_and(|output| output.status.success());
    let _ = fs::write(
        args.out.join("pending-review-delete.json"),
        serde_json::json!({"review_id": review_id, "deleted": ok}).to_string(),
    );
    ok
}

pub(crate) fn parse_github_review_comment_ids(body: &str) -> Result<Vec<u64>> {
    let comments: Vec<serde_json::Value> =
        serde_json::from_str(body).with_context(|| "parse pending GitHub review comments")?;
    comments
        .into_iter()
        .map(|comment| {
            comment
                .get("id")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| anyhow::anyhow!("pending GitHub review comment omitted numeric id"))
        })
        .collect()
}

/// Persist one current-head receipt for every inline comment confirmed by
/// GitHub. The API returns comment IDs in the same order as the pending
/// review's comment list, so the structural claim identity can be paired with
/// the confirmed ID without exposing claim metadata in the GitHub payload.
pub(crate) fn write_inline_delivery_receipts(
    out: &Path,
    review: &GitHubReview,
    head_sha: &str,
    review_id: u64,
    posted_comment_ids: &[u64],
) -> Result<()> {
    if review.comments.len() != posted_comment_ids.len() {
        bail!(
            "inline delivery receipt count mismatch (expected {}, got {})",
            review.comments.len(),
            posted_comment_ids.len()
        );
    }
    let receipts = review
        .comments
        .iter()
        .zip(posted_comment_ids)
        .map(|(comment, comment_id)| {
            serde_json::json!({
                "schema": "ub-review.github_inline_delivery_receipt.v1",
                "claim_id": delivery_claim_id(comment),
                "head_sha": head_sha,
                "path": comment.path,
                "line": comment.line,
                "side": comment.side,
                "review_id": review_id,
                "comment_id": comment_id,
                "status": "posted"
            })
        })
        .collect::<Vec<_>>();
    fs::write(
        out.join("inline-delivery.json"),
        serde_json::to_vec_pretty(&receipts)?,
    )?;
    Ok(())
}

fn delivery_claim_id(comment: &GitHubReviewComment) -> String {
    topic_claim_id_for_inline(&ReviewInlineComment {
        lane: String::new(),
        severity: String::new(),
        confidence: String::new(),
        path: comment.path.clone(),
        line: comment.line,
        side: comment.side.clone(),
        body: comment.body.clone(),
        evidence: String::new(),
        suggestion: comment.suggestion.clone(),
    })
}

pub(crate) fn github_review_post_payload(review: &GitHubReview) -> Result<GitHubReviewPostPayload> {
    let comments = github_review_post_comments(review)?;
    Ok(GitHubReviewPostPayload {
        event: review.event.clone(),
        body: review.body.clone(),
        comments,
    })
}

pub(crate) fn github_review_post_comments(
    review: &GitHubReview,
) -> Result<Vec<GitHubReviewPostComment>> {
    review
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
        .collect::<Result<Vec<_>>>()
}

const GITHUB_REVIEW_REPLY_CANDIDATES_SCHEMA: &str = "ub-review.github_review_reply_candidates.v1";

fn reply_candidates_path(args: &PostArgs) -> PathBuf {
    args.review_json
        .parent()
        .map(|parent| parent.join("reply-candidates.json"))
        .unwrap_or_else(|| args.out.join("reply-candidates.json"))
}

fn read_reply_candidates(args: &PostArgs) -> Result<Option<GitHubReviewReplyCandidates>> {
    let path = reply_candidates_path(args);
    if !path.exists() {
        return Ok(None);
    }
    let artifact: GitHubReviewReplyCandidates = serde_json::from_slice(
        &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
    )
    .with_context(|| format!("parse {}", path.display()))?;
    if artifact.schema != GITHUB_REVIEW_REPLY_CANDIDATES_SCHEMA {
        bail!("reply candidate schema must be {GITHUB_REVIEW_REPLY_CANDIDATES_SCHEMA}");
    }
    if artifact.head_sha.trim().is_empty() {
        bail!("reply candidate artifact head_sha must not be empty");
    }
    for reply in &artifact.replies {
        if reply.claim_id.trim().is_empty() {
            bail!("reply candidate claim_id must not be empty");
        }
        if reply.head_sha != artifact.head_sha {
            bail!("reply candidate head_sha must match the artifact head_sha");
        }
        if reply.comment_id == 0 {
            bail!("reply candidate comment_id must be positive");
        }
        if reply.body.trim().is_empty() || reply.body.chars().count() > 1_200 {
            bail!("reply candidate body must be non-empty and at most 1200 chars");
        }
        if !has_lane_prefix(&reply.body) {
            bail!("reply candidate body must start with a lane prefix");
        }
        if has_standalone_approval_line(&reply.body)
            || has_forbidden_pr_review_boilerplate(&reply.body)
        {
            bail!("reply candidate body contains forbidden review boilerplate");
        }
    }
    Ok(Some(artifact))
}

fn post_github_replies(
    args: &PostArgs,
    repo: &str,
    pull_number: u64,
    token: &str,
    replies: &[GitHubReviewReply],
) -> Result<Vec<u64>> {
    let base_url = format!(
        "{}/repos/{repo}/pulls/{pull_number}/comments",
        args.github_api_url.trim_end_matches('/')
    );
    let mut posted_reply_ids = Vec::new();
    for reply in replies {
        let payload_path = args
            .out
            .join(format!("reply-{}-payload.json", reply.comment_id));
        fs::write(
            &payload_path,
            serde_json::to_vec_pretty(&serde_json::json!({"body": reply.body}))?,
        )?;
        let output = match run_curl_json_post(
            Path::new("."),
            &format!("{base_url}/{}/replies", reply.comment_id),
            &github_bearer_auth_header(token),
            &payload_path,
            &[
                "Accept: application/vnd.github+json",
                "Content-Type: application/json",
                "X-GitHub-Api-Version: 2022-11-28",
            ],
            60,
        ) {
            Ok(output) => output,
            Err(err) => {
                write_reply_delivery_receipt(
                    args,
                    replies,
                    &posted_reply_ids,
                    "failed",
                    Some(&err.to_string()),
                )?;
                return Err(err)
                    .with_context(|| format!("post reply to review comment {}", reply.comment_id));
            }
        };
        fs::write(
            args.out
                .join(format!("reply-{}-stdout.json", reply.comment_id)),
            &output.stdout,
        )?;
        fs::write(
            args.out
                .join(format!("reply-{}-stderr.txt", reply.comment_id)),
            &output.stderr,
        )?;
        if !http_output_succeeded(&output) {
            let error = anyhow::anyhow!(
                "reply to review comment {} failed with exit code {:?} and http status {:?}",
                reply.comment_id,
                output.status.code(),
                output.http_status
            );
            write_reply_delivery_receipt(
                args,
                replies,
                &posted_reply_ids,
                "failed",
                Some(&error.to_string()),
            )?;
            return Err(error);
        }
        let response: serde_json::Value = match serde_json::from_slice(&output.stdout) {
            Ok(response) => response,
            Err(error) => {
                write_reply_delivery_receipt(
                    args,
                    replies,
                    &posted_reply_ids,
                    "failed",
                    Some(&error.to_string()),
                )?;
                return Err(error).with_context(|| {
                    format!("parse reply response for comment {}", reply.comment_id)
                });
            }
        };
        let reply_id = match response.get("id").and_then(serde_json::Value::as_u64) {
            Some(reply_id) => reply_id,
            None => {
                let error = anyhow::anyhow!("reply response omitted numeric id");
                write_reply_delivery_receipt(
                    args,
                    replies,
                    &posted_reply_ids,
                    "failed",
                    Some(&error.to_string()),
                )?;
                return Err(error);
            }
        };
        posted_reply_ids.push(reply_id);
        fs::write(
            args.out
                .join(format!("reply-{}-receipt.json", reply.comment_id)),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "ub-review.github_review_reply_receipt.v1",
                "status": "posted",
                "claim_id": reply.claim_id,
                "head_sha": reply.head_sha,
                "source_comment_id": reply.comment_id,
                "reply_comment_id": reply_id
            }))?,
        )?;
    }
    write_reply_delivery_receipt(args, replies, &posted_reply_ids, "posted", None)?;
    Ok(posted_reply_ids)
}

fn write_reply_delivery_receipt(
    args: &PostArgs,
    replies: &[GitHubReviewReply],
    posted_reply_ids: &[u64],
    status: &str,
    error: Option<&str>,
) -> Result<()> {
    fs::write(
        args.out.join("reply-delivery.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "ub-review.github_review_reply_delivery.v1",
            "status": status,
            "candidate_count": replies.len(),
            "posted_reply_ids": posted_reply_ids,
            "error": error
        }))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod inline_delivery_tests {
    use super::*;

    #[test]
    fn inline_delivery_receipts_bind_claims_to_current_head_and_github_ids() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: String::new(),
            comments: vec![GitHubReviewComment {
                path: "src/parser.rs".to_owned(),
                line: 122,
                side: "RIGHT".to_owned(),
                body: "[tests] postfix subscript is dropped".to_owned(),
                suggestion: None,
            }],
        };

        write_inline_delivery_receipts(temp.path(), &review, "head-sha", 123, &[456])?;

        let receipts: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(temp.path().join("inline-delivery.json"))?)?;
        assert_eq!(receipts.len(), 1);
        assert_eq!(
            receipts[0]["schema"],
            "ub-review.github_inline_delivery_receipt.v1"
        );
        assert_eq!(receipts[0]["head_sha"], "head-sha");
        assert_eq!(receipts[0]["path"], "src/parser.rs");
        assert_eq!(receipts[0]["line"], 122);
        assert_eq!(receipts[0]["review_id"], 123);
        assert_eq!(receipts[0]["comment_id"], 456);
        assert!(
            receipts[0]["claim_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("claim-"))
        );
        Ok(())
    }

    #[test]
    fn inline_delivery_receipts_reject_partial_github_confirmation() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: String::new(),
            comments: vec![GitHubReviewComment {
                path: "src/parser.rs".to_owned(),
                line: 122,
                side: "RIGHT".to_owned(),
                body: "[tests] postfix subscript is dropped".to_owned(),
                suggestion: None,
            }],
        };

        let error = write_inline_delivery_receipts(temp.path(), &review, "head-sha", 123, &[])
            .expect_err("partial confirmation must not be receipted as posted");
        assert!(
            error
                .to_string()
                .contains("inline delivery receipt count mismatch")
        );
        assert!(!temp.path().join("inline-delivery.json").exists());
        Ok(())
    }
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
