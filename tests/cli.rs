use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

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
        "review/observations.json",
        "review/unique_observations.json",
        "review/merged_observations.json",
        "review/dropped_observations.json",
        "review/proof_requests.json",
        "review/proof_plan.md",
        "proof_requests.ndjson",
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
        "## Review result",
        "## Residual risk",
        "## Missing evidence",
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
    assert!(!github_review_body.contains("## Confirmed findings"));
    assert!(!github_review_body.contains("## Summary-only findings"));
    assert!(!github_review_body.contains("## Failed objections"));
    assert!(!github_review_body.contains("## Model lanes"));
    assert!(!has_standalone_approval_line(github_review_body));
    let artifact_body = fs::read_to_string(out.join("review/review.md"))?;
    assert!(artifact_body.contains("## Confirmed findings"));
    assert!(artifact_body.contains("## Missing or failed evidence"));
    assert!(artifact_body.contains("## Model lanes"));
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
fn cache_warm_writes_base_and_rule_manifests() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo)?;
    write_file(
        &repo.join("README.md"),
        "# cache warm fixture\n\nThis repo only needs a committed tree.\n",
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

    let cache = temp.path().join("cache");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("configs/bun-gh-runner.toml");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    run(
        temp.path(),
        bin,
        &[
            "cache",
            "warm",
            "--config",
            path_str(&config)?,
            "--root",
            path_str(&repo)?,
            "--base",
            "HEAD",
            "--out",
            path_str(&cache)?,
        ],
    )?;

    let manifest_path = cache.join("latest-manifest.json");
    let manifest: serde_json::Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    let base_tree_sha = manifest["base_tree_sha"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("cache warm manifest missing base_tree_sha"))?;
    assert_eq!(base_tree_sha.len(), 40);
    assert!(base_tree_sha.chars().all(|ch| ch.is_ascii_hexdigit()));
    assert_eq!(manifest["profile"], "gh-runner");
    assert_eq!(manifest["base"], "HEAD");
    assert!(
        manifest["profile_hash"]
            .as_str()
            .is_some_and(|hash| hash.len() == 64)
    );
    assert_eq!(
        manifest["tools"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default(),
        4
    );

    let base_dir = cache.join("bases").join(base_tree_sha);
    assert!(base_dir.join("manifest.json").exists());
    for tool in ["tokmd", "ripr", "unsafe-review", "ast-grep"] {
        assert!(
            cache
                .join("rules")
                .join(tool)
                .join("manifest.json")
                .exists(),
            "missing rule cache manifest for {tool}"
        );
        assert!(
            base_dir.join(tool).join("manifest.json").exists(),
            "missing base cache manifest for {tool}"
        );
    }

    run(
        temp.path(),
        bin,
        &[
            "doctor",
            "--config",
            path_str(&config)?,
            "--root",
            path_str(&repo)?,
            "--base",
            "HEAD",
            "--cache-dir",
            path_str(&cache)?,
        ],
    )?;
    Ok(())
}

#[test]
fn doctor_require_core_tools_fails_missing_standard_image_tool() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let config = temp.path().join(".ub-review.toml");
    write_file(
        &config,
        r#"profile = "gh-runner"

[tools.tokmd]
id = "tokmd"
command = "ub-review-test-missing-tokmd"
"#,
    )?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let output = run_expect_failure(
        temp.path(),
        bin,
        &[
            "doctor",
            "--config",
            path_str(&config)?,
            "--require-core-tools",
        ],
    )?;
    assert!(output.contains("required core review tools missing from standard image"));
    assert!(output.contains("tokmd"));
    Ok(())
}

#[test]
fn run_with_ledger_path_writes_bounded_shared_context() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
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
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "touch rust behavior"])?;

    let ledger = temp.path().join("bun-ub-ledger.md");
    write_file(
        &ledger,
        "RAB resize follow-up: verify post-capture mutation before upstream. This tail should be truncated away.",
    )?;
    let out = temp.path().join("packet");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("configs/bun-gh-runner.toml");
    let bin = env!("CARGO_BIN_EXE_ub-review");
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
            "--ledger-path",
            path_str(&ledger)?,
            "--ledger-max-bytes",
            "64",
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
    )?;

    let shared_context = fs::read_to_string(out.join("review/shared_context.md"))?;
    assert!(shared_context.contains("## UB Ledger Context"));
    assert!(shared_context.contains("RAB resize follow-up"));
    assert!(shared_context.contains("[truncated]"));
    assert!(!shared_context.contains("tail should be truncated away"));
    assert!(!shared_context.contains("- No UB ledger configured"));

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    assert_eq!(review["ledger_path"], path_str(&ledger)?);
    assert_eq!(review["ledger_max_bytes"], 64);
    assert!(
        review["shared_context_id"]
            .as_str()
            .is_some_and(|value| value.len() == 64)
    );
    Ok(())
}

#[test]
fn model_auto_run_hits_fake_minimax_provider_and_writes_artifacts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
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
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "touch rust behavior"])?;

    let dummy_key = "dummy-minimax-key-for-local-test";
    let (provider_url, provider) = spawn_fake_openai_provider(2)?;
    let out = temp.path().join("packet");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("configs/bun-gh-runner.toml");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    run_with_env(
        temp.path(),
        bin,
        &[
            "run",
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
            "--model-mode",
            "auto",
            "--provider-policy",
            "minimax-only",
            "--minimax-provider-kind",
            "openai",
            "--lane-width",
            "6",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "1",
            "--max-inline-comments",
            "1",
            "--model-timeout-sec",
            "10",
        ],
        &[
            ("UB_REVIEW_MINIMAX_API_KEY", dummy_key),
            ("UB_REVIEW_MINIMAX_API_URL", provider_url.as_str()),
        ],
    )?;
    let provider_requests = join_fake_provider(provider)?;
    assert_eq!(provider_requests.len(), 2);
    assert!(
        provider_requests
            .iter()
            .all(|request| request
                .contains("Authorization: Bearer dummy-minimax-key-for-local-test"))
    );

    let preflight_artifact_dir = out
        .join("review/provider-preflight")
        .join("minimax-MiniMax-M3-openai-chat");
    let lane_artifact_dir = out.join("review/model/ub");
    for path in [
        preflight_artifact_dir.join("request.json"),
        preflight_artifact_dir.join("response.json"),
        preflight_artifact_dir.join("content.json"),
        preflight_artifact_dir.join("stderr.txt"),
        lane_artifact_dir.join("request.json"),
        lane_artifact_dir.join("response.json"),
        lane_artifact_dir.join("content.json"),
        lane_artifact_dir.join("stderr.txt"),
    ] {
        assert!(path.exists(), "missing {}", path.display());
        let text = fs::read_to_string(&path)?;
        assert!(
            !text.contains(dummy_key),
            "secret leaked to {}",
            path.display()
        );
        assert!(
            !text.contains("Authorization"),
            "auth header leaked to {}",
            path.display()
        );
        assert!(
            !text.contains("Bearer"),
            "bearer header leaked to {}",
            path.display()
        );
    }

    let preflights: serde_json::Value = serde_json::from_slice(&fs::read(
        out.join("review/provider-preflight-status.json"),
    )?)?;
    let preflight = preflights
        .as_array()
        .and_then(|receipts| receipts.first())
        .ok_or_else(|| anyhow::anyhow!("missing preflight receipt"))?;
    assert_eq!(preflight["provider"], "minimax");
    assert_eq!(preflight["model"], "MiniMax-M3");
    assert_eq!(preflight["endpoint_kind"], "openai-chat");
    assert_eq!(preflight["status"], "ok");
    assert_eq!(preflight["http_status"], 200);
    assert_eq!(preflight["response_shape"], "openai");

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    let model_lanes = review["model_lanes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("model_lanes missing"))?;
    let ub_lane = model_lanes
        .iter()
        .find(|lane| lane["lane"].as_str() == Some("ub"))
        .ok_or_else(|| anyhow::anyhow!("ub model lane missing"))?;
    assert_eq!(ub_lane["provider"], "minimax");
    assert_eq!(ub_lane["model"], "MiniMax-M3");
    assert_eq!(ub_lane["endpoint_kind"], "openai-chat");
    assert_eq!(ub_lane["status"], "ok");
    assert_eq!(ub_lane["http_status"], 200);
    assert_eq!(ub_lane["response_shape"], "openai");
    assert!(
        review["summary_only_findings"]
            .as_array()
            .is_some_and(|findings| findings.iter().any(|finding| {
                finding["lane"].as_str() == Some("ub")
                    && finding["evidence"].as_str() == Some("lane model summary")
            }))
    );

    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    assert_eq!(metrics["models"]["provider_preflight_calls_attempted"], 1);
    assert_eq!(metrics["models"]["model_lane_calls_attempted"], 1);
    assert_eq!(
        metrics["models"]["provider_preflight_status_counts"]["ok"],
        1
    );
    assert_eq!(metrics["models"]["model_lane_status_counts"]["ok"], 1);

    let summary = fs::read_to_string(out.join("running-summary.md"))?;
    assert!(summary.contains(
        "| `minimax` | `MiniMax-M3` | `openai-chat` | `ok` | `200` | `openai` | completed |"
    ));
    assert!(summary.contains("| `ub` | `minimax` | `MiniMax-M3` | `openai-chat` | `ok` |"));
    assert!(!summary.contains(dummy_key));
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

#[test]
fn post_receipt_writes_success_receipt_with_fake_github_api() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let review_json = temp.path().join("github-review.json");
    let out = temp.path().join("post");
    let token = "test-token-redacted";
    fs::write(
        &review_json,
        serde_json::to_vec_pretty(&serde_json::json!({
            "event": "COMMENT",
            "body": "## Decision\n\nNo blocking finding after checking.\n\n## Residual risk\n\n- Live GitHub posting was not exercised by this fixture.",
            "comments": []
        }))?,
    )?;
    let (github_api_url, handle) = spawn_fake_github_api()?;

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
            "123",
            "--github-token",
            token,
            "--github-api-url",
            &github_api_url,
        ],
    )?;

    let requests = join_fake_provider(handle)?;
    assert_eq!(requests.len(), 1);
    let request_text = &requests[0];
    assert!(
        request_text
            .starts_with("POST /repos/EffortlessMetrics/ub-review/pulls/123/reviews HTTP/1.1")
    );
    assert!(request_text.contains("Authorization: Bearer test-token-redacted"));
    assert!(request_text.contains("\"event\": \"COMMENT\""));
    assert!(request_text.contains("\"comments\": []"));

    let post_result_path = out.join("post-result.json");
    assert!(post_result_path.exists());
    assert!(!out.join("post-error.json").exists());
    assert!(out.join("github-review-post-payload.json").exists());
    assert!(out.join("post-stdout.json").exists());
    assert!(out.join("post-stderr.txt").exists());

    let post_result_text = fs::read_to_string(&post_result_path)?;
    let post_result: serde_json::Value = serde_json::from_str(&post_result_text)?;
    assert_eq!(post_result["schema_version"], 1);
    assert_eq!(post_result["status"], "ok");
    assert_eq!(post_result["repo"], "EffortlessMetrics/ub-review");
    assert_eq!(post_result["repo_valid"], true);
    assert_eq!(post_result["pull_number"], 123);
    assert_eq!(post_result["comments"], 0);
    assert_eq!(post_result["review_json_exists"], true);
    assert_eq!(post_result["review_json_valid"], true);
    assert_eq!(post_result["review_event"], "COMMENT");
    assert_eq!(post_result["review_comment_count"], 0);
    assert_eq!(post_result["diff_patch_exists"], false);
    assert_eq!(post_result["http_status"], 201);
    assert_eq!(post_result["token_present"], true);
    assert_eq!(post_result["payload_written"], true);
    assert_eq!(post_result["post_stdout_written"], true);
    assert_eq!(post_result["post_stderr_written"], true);
    assert_eq!(post_result["response"]["id"], 987);
    assert_eq!(post_result["response"]["state"], "COMMENTED");

    for path in [
        post_result_path,
        out.join("github-review-post-payload.json"),
        out.join("post-stdout.json"),
        out.join("post-stderr.txt"),
    ] {
        let text = fs::read_to_string(path)?;
        assert!(!text.contains(token));
        assert!(!text.contains("Authorization"));
        assert!(!text.contains("Bearer"));
    }
    Ok(())
}

fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)?;
    Ok(())
}

fn spawn_fake_github_api() -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}", listener.local_addr()?);
    let handle = thread::spawn(move || -> Result<Vec<String>> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match listener.accept() {
                Ok((stream, _addr)) => return Ok(vec![handle_fake_github_request(stream)?]),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!("fake GitHub API received no requests");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(err.into()),
            }
        }
    });
    Ok((url, handle))
}

fn handle_fake_github_request(mut stream: TcpStream) -> Result<String> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            bail!("fake GitHub request ended before headers finished");
        }
        headers.push_str(&line);
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or_default();
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let request_text = format!("{headers}{}", String::from_utf8_lossy(&body));
    let response_body = serde_json::to_vec(&serde_json::json!({
        "id": 987,
        "state": "COMMENTED",
        "body": "fake review posted"
    }))?;
    write!(
        stream,
        "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response_body.len()
    )?;
    stream.write_all(&response_body)?;
    Ok(request_text)
}

fn spawn_fake_openai_provider(
    expected_requests: usize,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let handle = thread::spawn(move || -> Result<Vec<String>> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut requests = Vec::new();
        while requests.len() < expected_requests {
            match listener.accept() {
                Ok((stream, _addr)) => requests.push(handle_fake_openai_request(stream)?),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!(
                            "fake provider received {} of {} requests",
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

fn handle_fake_openai_request(mut stream: TcpStream) -> Result<String> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            bail!("fake provider request ended before headers finished");
        }
        headers.push_str(&line);
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or_default();
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let request_text = format!("{headers}{}", String::from_utf8_lossy(&body));
    let lane_output = serde_json::json!({
        "summary": "fake provider ok",
        "inline_comments": [],
        "summary_only_findings": []
    });
    let response_body = serde_json::to_vec(&serde_json::json!({
        "choices": [
            {
                "message": {
                    "content": lane_output.to_string()
                }
            }
        ]
    }))?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response_body.len()
    )?;
    stream.write_all(&response_body)?;
    Ok(request_text)
}

fn join_fake_provider(handle: thread::JoinHandle<Result<Vec<String>>>) -> Result<Vec<String>> {
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("fake provider thread panicked"))?
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

fn run_with_env(cwd: &Path, program: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<()> {
    let mut command = Command::new(program);
    command.args(args).current_dir(cwd);
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output()?;
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

fn run_expect_failure(cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = Command::new(program).args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        return Ok(format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    bail!("{program} {args:?} unexpectedly succeeded");
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
