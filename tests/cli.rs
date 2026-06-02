use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

#[test]
fn plan_and_dry_run_write_expected_packet_tree() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
    fs::create_dir_all(repo.join("tests"))?;
    write_file(
        &repo.join("Cargo.toml"),
        r#"[package]
name = "bun-rab-mini"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )?;
    write_file(
        &repo.join("src/lib.rs"),
        r#"pub fn active_len(len: usize) -> usize {
    len
}
"#,
    )?;
    write_file(
        &repo.join("tests/rab.rs"),
        r#"#[test]
fn active_len_tracks_view() {
    assert_eq!(bun_rab_mini::active_len(4), 4);
}
"#,
    )?;

    run(&repo, "git", &["init"])?;
    run(
        &repo,
        "git",
        &["config", "user.email", "ub-review@example.invalid"],
    )?;
    run(&repo, "git", &["config", "user.name", "UB Review Test"])?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "baseline"])?;

    write_file(
        &repo.join("src/lib.rs"),
        r#"pub fn active_len(len: usize) -> usize {
    let ptr = &len as *const usize;
    unsafe { *ptr }
}
"#,
    )?;
    write_file(
        &repo.join("tests/rab.rs"),
        r#"#[test]
fn active_len_tracks_view_after_resize() {
    assert_eq!(bun_rab_mini::active_len(8), 8);
}
"#,
    )?;
    run(&repo, "git", &["add", "."])?;
    run(
        &repo,
        "git",
        &["commit", "-m", "touch rust behavior and tests"],
    )?;

    let out = temp.path().join("packet");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("configs/bun-gh-runner.toml");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    run(
        temp.path(),
        bin,
        &[
            "plan",
            "--config",
            path_str(&config)?,
            "--root",
            path_str(&repo)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            path_str(&out)?,
        ],
    )?;
    run(
        temp.path(),
        bin,
        &[
            "run",
            "--dry-run",
            "--config",
            path_str(&config)?,
            "--root",
            path_str(&repo)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            path_str(&out)?,
            "--no-github-summary",
        ],
    )?;

    for path in [
        "input/changed-files.txt",
        "input/diff.patch",
        "input/diff-context.json",
        "events.ndjson",
        "running-summary.md",
        "sensors/tokmd/ub-review-sensor-status.json",
        "sensors/ripr/ub-review-sensor-status.json",
        "sensors/unsafe-review/ub-review-sensor-status.json",
        "sensors/ast-grep/ub-review-sensor-status.json",
        "review/shared_context.md",
        "review/provider-preflight-status.json",
        "review/metrics.json",
        "review/review.json",
        "review/review.md",
        "review/github-review.json",
    ] {
        assert!(out.join(path).exists(), "missing {}", path);
    }

    let github_review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/github-review.json"))?)?;
    assert_eq!(github_review["event"], "COMMENT");
    let github_review_body = github_review["body"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("github review body missing"))?;
    let required_headings = [
        "## Decision",
        "## Confirmed findings",
        "## Summary-only findings",
        "## Failed objections",
        "## Residual risk",
        "## Parked follow-ups",
        "## Missing or failed evidence",
    ];
    let mut previous_heading_index = 0;
    for heading in required_headings {
        assert!(github_review_body.contains(heading), "missing {heading}");
        let heading_index = github_review_body
            .find(heading)
            .ok_or_else(|| anyhow::anyhow!("heading disappeared after contains check"))?;
        assert!(
            heading_index >= previous_heading_index,
            "{heading} rendered out of order"
        );
        previous_heading_index = heading_index;
    }
    assert!(!github_review_body.contains("## Missing or failed model evidence"));
    assert!(!has_standalone_approval_line(github_review_body));
    assert_eq!(
        github_review["comments"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default(),
        0
    );
    if let Some(comments) = github_review["comments"].as_array() {
        for comment in comments {
            if let Some(body) = comment["body"].as_str() {
                assert!(!has_standalone_approval_line(body));
            }
        }
    }
    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    assert_eq!(review["mode"], "review-direct");
    assert_eq!(review["provider_policy"], "minimax-primary");
    assert_eq!(review["model_provider_policy"], "minimax-primary");
    assert_eq!(review["lane_width"], 10);
    assert_eq!(review["model_concurrency"], 8);
    assert_eq!(review["max_model_calls"], 14);
    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    let diff_context: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("input/diff-context.json"))?)?;
    let plan_json: serde_json::Value = serde_json::from_slice(&fs::read(out.join("plan.json"))?)?;
    assert_eq!(metrics["schema_version"], 1);
    assert_eq!(metrics["shared_context_id"], review["shared_context_id"]);
    assert_eq!(metrics["profile_name"], "gh-runner");
    assert_eq!(
        metrics["changed_files"],
        diff_context["changed_files"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default()
    );
    assert_eq!(metrics["diff_flags"], diff_context["flags"]);
    assert_eq!(
        metrics["lane_packets"],
        plan_json["lanes"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default()
    );
    assert_eq!(
        metrics["sensors"]["total"],
        plan_json["sensors"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default()
    );
    assert_eq!(
        metrics["sensors"]["planned"].as_u64().unwrap_or_default()
            + metrics["sensors"]["skipped_by_plan"]
                .as_u64()
                .unwrap_or_default(),
        metrics["sensors"]["total"].as_u64().unwrap_or_default()
    );
    assert_eq!(
        sum_json_object_values(&metrics["sensors"]["status_counts"]),
        metrics["sensors"]["total"].as_u64().unwrap_or_default()
    );
    assert_eq!(
        metrics["models"]["model_lanes"],
        review["model_lanes"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default()
    );
    assert_eq!(metrics["models"]["model_lane_calls_attempted"], 0);
    assert_eq!(metrics["models"]["provider_preflight_calls_attempted"], 0);
    assert_eq!(metrics["models"]["model_fallbacks_used"], 0);
    assert_eq!(
        metrics["inline_comments"],
        review["inline_comments"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default()
    );
    assert_eq!(
        metrics["summary_only_findings"],
        review["summary_only_findings"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default()
    );
    assert_eq!(
        metrics["missing_or_failed_sensor_evidence"],
        review["missing_or_failed_sensor_evidence"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default()
    );
    assert_eq!(
        metrics["missing_or_failed_model_evidence"],
        review["missing_or_failed_model_evidence"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default()
    );
    assert_eq!(metrics["github_review_comments"], 0);
    assert_eq!(
        metrics["review_body_bytes"],
        review["body"].as_str().map(str::len).unwrap_or_default()
    );
    assert_eq!(metrics["review_body_truncated"], false);
    assert!(
        review["missing_or_failed_sensor_evidence"]
            .as_array()
            .is_some_and(|issues| !issues.is_empty())
    );
    assert!(
        review["missing_or_failed_model_evidence"]
            .as_array()
            .is_some_and(|issues| {
                !issues.is_empty()
                    && issues
                        .iter()
                        .all(|issue| issue["provider"].as_str() == Some("minimax"))
            })
    );
    let review_body = fs::read_to_string(out.join("review/review.md"))?;
    assert!(review_body.contains("## Missing or failed evidence"));
    assert!(!has_standalone_approval_line(&review_body));

    run(
        temp.path(),
        bin,
        &[
            "post",
            "--review-json",
            path_str(&out.join("review/github-review.json"))?,
            "--out",
            path_str(&out.join("review"))?,
            "--repo",
            "EffortlessMetrics/ub-review",
            "--pull-number",
            "1",
        ],
    )?;
    let post_error_path = out.join("review/post-error.json");
    assert!(post_error_path.exists());
    let post_error_text = fs::read_to_string(&post_error_path)?;
    let post_error: serde_json::Value = serde_json::from_str(&post_error_text)?;
    assert_eq!(post_error["schema_version"], 1);
    assert_eq!(post_error["status"], "failed");
    assert_eq!(post_error["error_kind"], "missing_token");
    assert_eq!(post_error["failure_stage"], "preflight");
    assert_eq!(post_error["repo"], "EffortlessMetrics/ub-review");
    assert_eq!(post_error["repo_valid"], true);
    assert_eq!(post_error["pull_number"], 1);
    assert_eq!(post_error["comments"], 0);
    assert_eq!(post_error["review_comment_count"], 0);
    assert_eq!(post_error["review_event"], "COMMENT");
    assert!(
        post_error["review_body_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
    );
    assert_eq!(post_error["review_json_exists"], true);
    assert_eq!(post_error["review_json_valid"], true);
    assert_eq!(post_error["token_present"], false);
    assert_eq!(post_error["payload_written"], false);
    assert_eq!(post_error["would_post"], false);
    assert_eq!(post_error["failure_tolerated"], true);
    assert_eq!(post_error["fail_on_post_error"], false);
    assert!(post_error["http_status"].is_null());
    assert!(
        post_error["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("github token is required"))
    );
    assert!(
        post_error["review_json"]
            .as_str()
            .is_some_and(|path| path.ends_with("github-review.json"))
    );
    assert!(!post_error_text.contains("github_token"));
    assert!(!post_error_text.contains("Authorization"));
    assert!(!post_error_text.contains("Bearer"));

    for lane in [
        "ub",
        "source-route",
        "tests",
        "arch",
        "opposition",
        "security",
    ] {
        let path = out.join("lanes").join(format!("{lane}.md"));
        assert!(path.exists(), "missing lane {lane}");
        let text = fs::read_to_string(path)?;
        assert!(!has_standalone_approval_line(&text));
        assert!(text.contains(&format!("[{lane}]")));
    }

    let summary = fs::read_to_string(out.join("running-summary.md"))?;
    assert!(!has_standalone_approval_line(&summary));
    assert!(summary.contains("## Missing evidence"));
    assert!(summary.contains("## Provider preflights"));
    assert!(summary.contains("## Model lane status"));
    assert!(summary.contains("## Missing or failed model evidence"));
    assert!(summary.contains("`ub-memory-lifetime`"));
    assert!(summary.contains("MiniMax-M3"));
    assert!(summary.contains("## Lane packets"));
    Ok(())
}

#[test]
fn post_receipt_marks_semantically_invalid_review_json_invalid() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let review_json = temp.path().join("github-review.json");
    let out = temp.path().join("post");
    let token = "test-token-redacted";
    fs::write(
        &review_json,
        serde_json::to_vec_pretty(&serde_json::json!({
            "event": "APPROVE",
            "body": "Review body",
            "comments": []
        }))?,
    )?;

    run(
        temp.path(),
        env!("CARGO_BIN_EXE_ub-review"),
        &[
            "post",
            "--review-json",
            path_str(&review_json)?,
            "--out",
            path_str(&out)?,
            "--repo",
            "EffortlessMetrics/ub-review",
            "--pull-number",
            "1",
            "--github-token",
            token,
        ],
    )?;

    let post_error_path = out.join("post-error.json");
    assert!(post_error_path.exists());
    let post_error_text = fs::read_to_string(post_error_path)?;
    let post_error: serde_json::Value = serde_json::from_str(&post_error_text)?;
    assert_eq!(post_error["status"], "failed");
    assert_eq!(post_error["error_kind"], "invalid_review_payload");
    assert_eq!(post_error["failure_stage"], "payload_validation");
    assert_eq!(post_error["review_json_exists"], true);
    assert_eq!(post_error["review_json_valid"], false);
    assert_eq!(post_error["review_event"], "APPROVE");
    assert_eq!(post_error["review_comment_count"], 0);
    assert_eq!(post_error["repo_valid"], true);
    assert_eq!(post_error["pull_number"], 1);
    assert_eq!(post_error["token_present"], true);
    assert_eq!(post_error["would_post"], false);
    assert_eq!(post_error["payload_written"], false);
    assert!(
        post_error["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("github review event must be COMMENT"))
    );
    assert!(!post_error_text.contains(token));
    assert!(!post_error_text.contains("Authorization"));
    assert!(!post_error_text.contains("Bearer"));
    Ok(())
}

#[test]
fn post_receipt_rejects_off_diff_inline_comment_before_payload_write() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let review_json = temp.path().join("github-review.json");
    let diff_patch = temp.path().join("input/diff.patch");
    let out = temp.path().join("post");
    let token = "test-token-redacted";
    write_file(
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
    fs::write(
        &review_json,
        serde_json::to_vec_pretty(&serde_json::json!({
            "event": "COMMENT",
            "body": "Review body",
            "comments": [
                {
                    "path": "src/lib.rs",
                    "line": 99,
                    "side": "RIGHT",
                    "body": "[tests] This test reaches the helper but not the boundary."
                }
            ]
        }))?,
    )?;

    run(
        temp.path(),
        env!("CARGO_BIN_EXE_ub-review"),
        &[
            "post",
            "--review-json",
            path_str(&review_json)?,
            "--diff-patch",
            path_str(&diff_patch)?,
            "--out",
            path_str(&out)?,
            "--repo",
            "EffortlessMetrics/ub-review",
            "--pull-number",
            "1",
            "--github-token",
            token,
        ],
    )?;

    let post_error_path = out.join("post-error.json");
    assert!(post_error_path.exists());
    assert!(!out.join("github-review-post-payload.json").exists());
    let post_error_text = fs::read_to_string(post_error_path)?;
    let post_error: serde_json::Value = serde_json::from_str(&post_error_text)?;
    assert_eq!(post_error["status"], "failed");
    assert_eq!(post_error["error_kind"], "invalid_review_payload");
    assert_eq!(post_error["failure_stage"], "payload_validation");
    assert_eq!(post_error["review_json_exists"], true);
    assert_eq!(post_error["review_json_valid"], false);
    assert_eq!(post_error["review_event"], "COMMENT");
    assert_eq!(post_error["review_comment_count"], 1);
    assert_eq!(post_error["repo_valid"], true);
    assert_eq!(post_error["pull_number"], 1);
    assert_eq!(post_error["token_present"], true);
    assert_eq!(post_error["would_post"], false);
    assert_eq!(post_error["payload_written"], false);
    assert_eq!(post_error["diff_patch_exists"], true);
    assert_eq!(post_error["diff_patch_valid"], true);
    assert_eq!(post_error["off_diff_comment_count"], 1);
    assert!(
        post_error["diff_line_count"]
            .as_u64()
            .is_some_and(|count| count > 0)
    );
    assert!(
        post_error["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("not a valid RIGHT-side diff line"))
    );
    assert!(
        post_error["diff_patch"]
            .as_str()
            .is_some_and(|path| path.ends_with("diff.patch"))
    );
    assert!(!post_error_text.contains(token));
    assert!(!post_error_text.contains("Authorization"));
    assert!(!post_error_text.contains("Bearer"));
    Ok(())
}

fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)?;
    Ok(())
}

fn run(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
    let output = Command::new(program).args(args).current_dir(cwd).output()?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{} {:?} failed\nstdout:\n{}\nstderr:\n{}",
        program,
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", path.display()))
}

fn sum_json_object_values(value: &serde_json::Value) -> u64 {
    value
        .as_object()
        .map(|values| values.values().filter_map(serde_json::Value::as_u64).sum())
        .unwrap_or_default()
}

fn has_standalone_approval_line(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line
            .trim()
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim()
            .to_ascii_lowercase();
        matches!(
            trimmed.as_str(),
            "lgtm"
                | "looks good"
                | "clean"
                | "solid"
                | "no issues found"
                | "no actionable findings"
                | "no actionable"
        )
    })
}
