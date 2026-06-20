//! GitHub review payload validation: body policy, right-line
//! validation, and effective review body config (cleanup train step 52,
//! pure code motion).

use crate::*;

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
    review_body: ReviewBodyPolicy,
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
