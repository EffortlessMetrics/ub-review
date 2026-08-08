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
        // Suggestions are gated on content, not on which lane found the
        // defect: a click-to-apply fix is the highest-value thing a line-level
        // reviewer produces, and restricting it to one sensor lane meant it
        // never reached an author. `validate_github_suggestion_text` is what
        // keeps a malformed edit out; `validate_github_review_payload_for_post`
        // additionally proves the edit applies at its anchor.
        if let Some(suggestion) = comment.suggestion.as_deref() {
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
    let anchor_text = right_side_diff_line_text(&patch);
    let right_lines = anchor_text.keys().cloned().collect::<BTreeSet<_>>();
    let source = diff_patch.display().to_string();
    validate_github_review_payload_for_right_lines(
        review,
        &right_lines,
        &source,
        &review_body_policy,
        waive_suppressible,
    )?;
    validate_github_review_suggestion_anchors(review, &anchor_text, &source)
}

/// Prove every `suggestion` block applies to the line it replaces, using the
/// same patch the anchors were just validated against. This runs only at post
/// time, where the reviewed diff on disk is the authority; the right-line
/// check above has already guaranteed each anchor is present in the map, so a
/// missing anchor here is a real inconsistency and not a tolerable gap.
pub(crate) fn validate_github_review_suggestion_anchors(
    review: &GitHubReview,
    anchor_text: &BTreeMap<(String, u32), String>,
    source: &str,
) -> Result<()> {
    for comment in &review.comments {
        let Some(suggestion) = comment.suggestion.as_deref() else {
            continue;
        };
        let path = normalize_repo_path(&comment.path);
        let anchor = anchor_text
            .get(&(path.clone(), comment.line))
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "github review suggestion {path}:{} has no RIGHT-side line in {source}",
                    comment.line
                )
            })?;
        validate_github_suggestion_anchor(anchor, suggestion).with_context(|| {
            format!(
                "github review suggestion {path}:{} does not apply in {source}",
                comment.line
            )
        })?;
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use anyhow::ensure;

    use super::*;

    const PATCH: &str = "\
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

    fn review_with(suggestion: Option<&str>) -> GitHubReview {
        GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Findings\n\n- [tests] the raw pointer is never asserted upon".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests] The raw pointer is taken but never asserted upon.".to_owned(),
                suggestion: suggestion.map(str::to_owned),
            }],
        }
    }

    /// The reviewed diff on disk is the authority at post time: a suggestion
    /// only posts when it demonstrably applies to the line it replaces.
    #[test]
    fn post_gate_proves_suggestions_apply_to_the_line_they_replace() -> Result<()> {
        let anchors = right_side_diff_line_text(PATCH);
        validate_github_review_suggestion_anchors(
            &review_with(Some("    let ptr = core::ptr::from_ref(&len);")),
            &anchors,
            "input/diff.patch",
        )?;
        validate_github_review_suggestion_anchors(
            &review_with(None),
            &anchors,
            "input/diff.patch",
        )?;

        let misindented = validate_github_review_suggestion_anchors(
            &review_with(Some("let ptr = core::ptr::from_ref(&len);")),
            &anchors,
            "input/diff.patch",
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("misindented suggestion passed the post gate"))?;
        ensure!(format!("{misindented:#}").contains("does not apply in input/diff.patch"));

        let noop = validate_github_review_suggestion_anchors(
            &review_with(Some("    let ptr = &len as *const usize;")),
            &anchors,
            "input/diff.patch",
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("no-op suggestion passed the post gate"))?;
        ensure!(format!("{noop:#}").contains("identical to the line"));

        let unanchored = validate_github_review_suggestion_anchors(
            &review_with(Some("    let ptr = core::ptr::from_ref(&len);")),
            &BTreeMap::new(),
            "input/diff.patch",
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("unanchored suggestion passed the post gate"))?;
        ensure!(format!("{unanchored:#}").contains("no RIGHT-side line"));
        Ok(())
    }
}
