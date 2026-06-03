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
        "resolved-profile.json",
        "resolved-plan.json",
        "running-summary.md",
        "sensors/tokmd/ub-review-sensor-status.json",
        "sensors/ripr/ub-review-sensor-status.json",
        "sensors/unsafe-review/ub-review-sensor-status.json",
        "sensors/ast-grep/ub-review-sensor-status.json",
        "review/shared_context.md",
        "review/pr_thread_context.json",
        "review/terminal_state.json",
        "review/provider-preflight-status.json",
        "review/metrics.json",
        "review/review.json",
        "review/review.md",
        "review/observations.json",
        "review/unique_observations.json",
        "review/merged_observations.json",
        "review/dropped_observations.json",
        "review/witnesses.json",
        "review/proof_requests.json",
        "review/proof_receipts.json",
        "review/proof_plan.md",
        "review/resource_leases.json",
        "review/resource_plan.md",
        "proof_requests.ndjson",
        "proof_receipts.ndjson",
        "witnesses.ndjson",
        "resource_leases.ndjson",
        "review/github-review-skip.json",
    ] {
        assert!(out.join(path).exists(), "missing {}", path);
    }
    assert!(out.join("proof_requests").is_dir());

    assert!(!out.join("review/github-review.json").exists());
    let github_skip: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/github-review-skip.json"))?)?;
    assert_eq!(github_skip["status"], "skipped");
    assert_eq!(github_skip["review_payload_status"], "skipped_empty_smoke");
    assert_eq!(github_skip["terminal_state"], "artifact-only");
    let artifact_body = fs::read_to_string(out.join("review/review.md"))?;
    assert!(artifact_body.contains("## Confirmed findings"));
    assert!(artifact_body.contains("## Missing or failed evidence"));
    assert!(artifact_body.contains("## Model lanes"));
    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    assert_eq!(review["mode"], "review-direct");
    assert_eq!(review["provider_policy"], "minimax-primary");
    assert_eq!(review["model_provider_policy"], "minimax-primary");
    assert_eq!(review["runtime_profile"], "gh-runner");
    assert_eq!(review["depth"], "standard");
    assert_eq!(review["lane_width"], 10);
    assert_eq!(review["model_concurrency"], 8);
    assert_eq!(review["max_model_calls"], 14);
    let resolved_profile: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-profile.json"))?)?;
    assert_eq!(resolved_profile["schema"], "ub-review.resolved_profile.v1");
    assert_eq!(resolved_profile["selected_profile"], "gh-runner");
    assert_eq!(resolved_profile["selected_runtime_profile"], "gh-runner");
    assert_eq!(
        resolved_profile["profile"]["limits"]["tests"],
        serde_json::json!(2)
    );
    assert_eq!(
        resolved_profile["profile"]["limits"]["sensor_jobs"],
        serde_json::json!(4)
    );
    assert_eq!(
        resolved_profile["profile"]["budgets"]["proof_max_focused_tests"],
        serde_json::json!(1)
    );
    assert_eq!(
        resolved_profile["profile"]["budgets"]["proof_command_timeout_sec"],
        serde_json::json!(300)
    );
    assert_eq!(
        resolved_profile["profile"]["budgets"]["proof_cpu"],
        serde_json::json!(2)
    );
    assert_eq!(
        resolved_profile["profile"]["budgets"]["proof_memory_mb"],
        serde_json::json!(2048)
    );
    assert_eq!(resolved_profile["review"]["posting_engine"], "artifact");
    assert!(resolved_profile["tools"]["tokmd"]["enabled"].as_bool() == Some(true));
    let resolved_plan: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-plan.json"))?)?;
    assert_eq!(resolved_plan["schema"], "ub-review.resolved_plan.v1");
    assert_eq!(resolved_plan["profile_name"], "gh-runner");
    assert_eq!(resolved_plan["runtime_profile"], "gh-runner");
    assert_eq!(resolved_plan["diff_class"], "source-ub");
    assert_eq!(resolved_plan["budgets"]["default_timeout_sec"], 900);
    assert_eq!(resolved_plan["budgets"]["proof_max_focused_tests"], 1);
    assert_eq!(resolved_plan["budgets"]["proof_total_timeout_sec"], 600);
    assert_eq!(resolved_plan["budgets"]["proof_disk_mb"], 1024);
    assert_eq!(resolved_plan["limits"]["sensor_jobs"], 4);
    assert_eq!(resolved_plan["selectors"]["depth"], "standard");
    assert_eq!(resolved_plan["selectors"]["lane_width"], 10);
    assert_eq!(resolved_plan["selectors"]["model_concurrency"], 8);
    assert_eq!(resolved_plan["selectors"]["max_model_calls"], 14);
    assert_eq!(resolved_plan["selectors"]["lanes"], serde_json::json!([]));
    assert_eq!(
        resolved_plan["selectors"]["except_lanes"],
        serde_json::json!([])
    );
    assert_eq!(resolved_plan["selectors"]["tools"], serde_json::json!([]));
    assert_eq!(
        resolved_plan["selectors"]["except_tools"],
        serde_json::json!([])
    );
    assert_eq!(
        resolved_plan["selectors"]["effective_model_lanes"]
            .as_array()
            .map(std::vec::Vec::len),
        Some(10)
    );
    let capped_out = temp.path().join("packet-capped");
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
            path_str(&capped_out)?,
            "--runtime-profile",
            "cx23",
            "--model-concurrency",
            "99",
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
    )?;
    let capped_review: serde_json::Value =
        serde_json::from_slice(&fs::read(capped_out.join("review/review.json"))?)?;
    assert_eq!(capped_review["runtime_profile"], "cx23");
    assert_eq!(capped_review["model_concurrency"], 12);
    let capped_resolved_plan: serde_json::Value =
        serde_json::from_slice(&fs::read(capped_out.join("resolved-plan.json"))?)?;
    assert_eq!(capped_resolved_plan["runtime_profile"], "cx23");
    assert_eq!(capped_resolved_plan["limits"]["llm_in_flight"], 12);
    assert_eq!(capped_resolved_plan["limits"]["sensor_jobs"], 2);
    assert_eq!(
        capped_resolved_plan["budgets"]["proof_max_focused_tests"],
        2
    );
    assert_eq!(capped_resolved_plan["budgets"]["proof_cpu"], 1);
    assert_eq!(capped_resolved_plan["budgets"]["proof_memory_mb"], 1024);
    assert_eq!(capped_resolved_plan["selectors"]["model_concurrency"], 12);
    assert!(resolved_plan["sensors"].as_array().is_some_and(|sensors| {
        sensors
            .iter()
            .any(|sensor| sensor["id"] == "tokmd" && sensor["run"] == true)
    }));
    assert!(
        resolved_plan["lanes"]
            .as_array()
            .is_some_and(|lanes| { lanes.iter().any(|lane| lane["id"] == "source-route") })
    );
    assert_eq!(
        review["pr_thread_context"]["schema"],
        "ub-review.pr_thread_context.v1"
    );
    assert!(
        review["pr_thread_context"]["status"]
            .as_str()
            .is_some_and(|status| matches!(status, "seeded" | "absent" | "unavailable"))
    );
    let pr_thread_context: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/pr_thread_context.json"))?)?;
    assert_eq!(pr_thread_context, review["pr_thread_context"]);
    let terminal_state: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/terminal_state.json"))?)?;
    assert_eq!(terminal_state, review["terminal_state"]);
    assert_eq!(terminal_state["schema"], "ub-review.terminal_state.v1");
    assert_eq!(terminal_state["status"], "artifact-only");
    let shared_context = fs::read_to_string(out.join("review/shared_context.md"))?;
    assert!(shared_context.contains("## PR Thread Context"));
    assert!(shared_context.contains("- Status: `"));
    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    let diff_context: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("input/diff-context.json"))?)?;
    let plan_json: serde_json::Value = serde_json::from_slice(&fs::read(out.join("plan.json"))?)?;
    assert_eq!(metrics["schema_version"], 1);
    assert_eq!(metrics["shared_context_id"], review["shared_context_id"]);
    assert_eq!(metrics["terminal_state"], terminal_state["status"]);
    assert_eq!(metrics["profile_name"], "gh-runner");
    assert_eq!(metrics["runtime_profile"], "gh-runner");
    assert_eq!(metrics["depth"], "standard");
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
    assert_eq!(metrics["resource_leases"], 0);
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
fn run_with_pr_thread_context_seeds_shared_context() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
    write_file(
        &repo.join("Cargo.toml"),
        r#"[package]
name = "bun-ffi-mini"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )?;
    write_file(
        &repo.join("src/lib.rs"),
        r#"pub fn copy_len(len: usize) -> usize {
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
        r#"pub fn copy_len(len: usize) -> usize {
    let ptr = &len as *const usize;
    unsafe { *ptr }
}
"#,
    )?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "touch ffi route"])?;

    let thread = temp.path().join("thread.md");
    write_file(
        &thread,
        "Author reply: ASAN bad-free receipt attached; old base fails. This tail should be truncated away.",
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
            "--pr-thread-context",
            path_str(&thread)?,
            "--pr-thread-context-max-bytes",
            "64",
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
    )?;

    let pr_thread_context: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/pr_thread_context.json"))?)?;
    assert_eq!(
        pr_thread_context["schema"],
        "ub-review.pr_thread_context.v1"
    );
    assert_eq!(pr_thread_context["status"], "seeded");
    assert_eq!(pr_thread_context["thread_context_truncated"], true);
    assert!(
        pr_thread_context["thread_context"]
            .as_str()
            .is_some_and(|text| text.contains("ASAN bad-free receipt"))
    );
    assert!(
        !pr_thread_context["thread_context"]
            .as_str()
            .unwrap_or_default()
            .contains("tail should be truncated")
    );

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    assert_eq!(review["pr_thread_context"], pr_thread_context);
    let shared_context = fs::read_to_string(out.join("review/shared_context.md"))?;
    assert!(shared_context.contains("## PR Thread Context"));
    assert!(shared_context.contains("### Prior Review Thread"));
    assert!(shared_context.contains("ASAN bad-free receipt"));
    assert!(shared_context.contains("[truncated]"));
    assert!(!shared_context.contains("tail should be truncated"));
    Ok(())
}

#[test]
fn run_executes_focused_proof_and_writes_receipts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo)?;
    write_file(&repo.join("README.md"), "# focused proof fixture\n")?;

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
        &repo.join("test/js/bun/ffi/ffi.test.js"),
        r#"import { expect, test } from "bun:test";

test("no-finalizer toBuffer keeps caller memory alive", () => {
  expect(true).toBe(true);
});
"#,
    )?;
    run(&repo, "git", &["add", "."])?;
    run(
        &repo,
        "git",
        &["commit", "-m", "add focused ffi regression"],
    )?;

    let fake_bin = temp.path().join("fake-bin");
    write_fake_bun(&fake_bin)?;
    let path = prepend_to_path(&fake_bin)?;
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
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
        &[("PATH", path.as_str())],
    )?;

    let receipts: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
    assert_eq!(receipts.len(), 1);
    let receipt = &receipts[0];
    assert_eq!(receipt["schema"], "ub-review.proof_receipt.v1");
    assert_eq!(receipt["kind"], "focused-red-green");
    assert_eq!(receipt["test_patch_mode"], "base-plus-tests");
    assert_eq!(receipt["result"], "discriminating");
    assert_eq!(receipt["requested_by"], serde_json::json!(["proof-broker"]));
    assert_eq!(receipt["request_ids"], serde_json::json!([]));
    assert!(
        receipt["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("HEAD passed; base+tests failed"))
    );
    let commands = json_array_field(receipt, "commands")?;
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0]["side"], "head");
    assert_eq!(commands[0]["status"], "passed");
    assert_eq!(commands[1]["side"], "base-plus-tests");
    assert_eq!(commands[1]["status"], "failed");
    assert!(
        commands[0]["command"]
            .as_str()
            .is_some_and(|command| command.contains("test/js/bun/ffi/ffi.test.js"))
    );
    assert!(commands[0]["command"].as_str().is_some_and(|command| {
        command.contains("no-finalizer toBuffer keeps caller memory alive")
    }));
    assert!(
        commands[0]["command"]
            .as_str()
            .is_some_and(|command| command.contains(" -t "))
    );
    for command in commands {
        assert!(out.join(json_str_field(command, "stdout")?).exists());
        assert!(out.join(json_str_field(command, "stderr")?).exists());
    }

    let leases: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/resource_leases.json"))?)?;
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0]["schema"], "ub-review.resource_lease.v1");
    assert_eq!(leases[0]["kind"], "focused-test");
    assert_eq!(leases[0]["status"], "granted");
    assert_eq!(leases[0]["consumer"], receipt["id"]);
    assert_eq!(leases[0]["network"], false);

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    assert_eq!(review["proof_receipts"], serde_json::json!(receipts));
    assert_eq!(review["resource_leases"], serde_json::json!(leases));
    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    assert_eq!(metrics["proof_receipts"], 1);
    assert_eq!(metrics["resource_leases"], 1);

    let proof_plan = fs::read_to_string(out.join("review/proof_plan.md"))?;
    assert!(proof_plan.contains("Proof broker v0 executed focused proof under the runtime budget"));
    assert!(proof_plan.contains("result=`discriminating`"));
    let resource_plan = fs::read_to_string(out.join("review/resource_plan.md"))?;
    assert!(resource_plan.contains("status=`granted`"));

    let receipt_ndjson = fs::read_to_string(out.join("proof_receipts.ndjson"))?;
    assert_eq!(
        receipt_ndjson
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        1
    );
    let lease_ndjson = fs::read_to_string(out.join("resource_leases.ndjson"))?;
    assert_eq!(
        lease_ndjson
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        1
    );
    let head_stdout = fs::read_to_string(out.join(json_str_field(&commands[0], "stdout")?))?;
    assert!(head_stdout.contains("fake bun"));
    let base_stderr = fs::read_to_string(out.join(json_str_field(&commands[1], "stderr")?))?;
    assert!(base_stderr.contains("base failure"));
    assert!(!out.join("proof-worktrees/base-plus-tests").exists());
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
            "body": "## Test proof\n\n- Added bad-free tests pass on HEAD and fail on base+tests.",
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

fn write_fake_bun(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(windows)]
    {
        let source = dir.join("fake_bun.rs");
        write_file(
            &source,
            r#"use std::{env, fs, process};

fn main() {
    let cwd = env::current_dir().expect("current dir");
    let args = env::args().skip(1).collect::<Vec<_>>();
    println!("fake bun {}", args.join(" "));
    let mut base_plus_tests = cwd.to_string_lossy().contains("base-plus-tests");
    let git_file = cwd.join(".git");
    if git_file.is_file() {
        let text = fs::read_to_string(git_file).unwrap_or_default();
        base_plus_tests |= text.contains("base-plus-tests");
    }
    if base_plus_tests {
        eprintln!("base failure");
        process::exit(1);
    }
}
"#,
        )?;
        let exe = dir.join("bun.exe");
        run(dir, "rustc", &[path_str(&source)?, "-o", path_str(&exe)?])?;
    }
    #[cfg(not(windows))]
    {
        let script = dir.join("bun");
        write_file(
            &script,
            r#"#!/bin/sh
echo "fake bun $*"
if [ -f .git ] && grep -q base-plus-tests .git; then
  echo "base failure" >&2
  exit 1
fi
case "$PWD" in
  *base-plus-tests*)
    echo "base failure" >&2
    exit 1
    ;;
esac
exit 0
"#,
        )?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = fs::metadata(&script)?.permissions();
            permissions.set_mode(0o755);
            fs::set_permissions(&script, permissions)?;
        }
    }
    Ok(())
}

fn prepend_to_path(dir: &Path) -> Result<String> {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    Ok(std::env::join_paths(paths)?.to_string_lossy().into_owned())
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

fn json_array_field<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Result<&'a [serde_json::Value]> {
    value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow::anyhow!("JSON field `{field}` is not an array"))
}

fn json_str_field<'a>(value: &'a serde_json::Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("JSON field `{field}` is not a string"))
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
