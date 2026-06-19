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
