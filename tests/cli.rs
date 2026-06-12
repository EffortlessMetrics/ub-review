use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

const CARGO_ALLOW_FOREIGN_REASON: &str = "policy/allow.toml is not a cargo-allow-dialect ledger; add \
     policy/cargo-allow.toml (see EffortlessMetrics/cargo-allow#1465)";

#[test]
fn gh_runner_tool_installer_pins_tokmd_for_bun_ub_sensor() -> Result<()> {
    let script = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/install-gh-runner-tools.sh"),
    )?;
    assert!(script.contains("UB_REVIEW_TOKMD_VERSION:-1.12.0"));
    assert!(script.contains("install_cargo_bin tokmd tokmd \"$tokmd_version\""));
    assert!(
        !script.contains("install_cargo_bin tokmd tokmd\n"),
        "hosted fallback must not install crates.io latest tokmd implicitly"
    );
    Ok(())
}

#[test]
fn review_image_tool_installer_uses_tool_dir_as_install_prefix() -> Result<()> {
    let script = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/install-review-image-tools.sh"),
    )?;
    assert!(script.contains("UB_REVIEW_TOKMD_VERSION:-1.12.0"));
    assert!(script.contains("UB_REVIEW_RIPR_VERSION:-0.8.0"));
    assert!(script.contains("UB_REVIEW_UNSAFE_REVIEW_VERSION:-0.3.4"));
    assert!(script.contains("prefix=\"${UB_REVIEW_TOOL_DIR:-/opt/ub-review}\""));
    assert!(script.contains("export PATH=\"$prefix/bin:\\$PATH\""));
    assert!(script.contains("export UB_REVIEW_TOOL_DIR=\"$prefix\""));
    assert!(
        !script.contains("export UB_REVIEW_TOOL_DIR=\"$prefix/bin\""),
        "UB_REVIEW_TOOL_DIR is the install prefix; PATH points at its bin directory"
    );
    Ok(())
}

#[test]
fn plan_and_dry_run_write_expected_packet_tree() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
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
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
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
            "--run-pass",
            "opened",
            "--no-github-summary",
        ],
    )?;

    for path in [
        "input/changed-files.txt",
        "input/diff.patch",
        "input/diff-context.json",
        "events.ndjson",
        "work_queue.json",
        "work_events.ndjson",
        "resolved-profile.json",
        "resolved-plan.json",
        "resolved-tools.json",
        "tool-status.json",
        "tool-gate-outcomes.json",
        "running-summary.md",
        "sensors/tokmd/ub-review-sensor-status.json",
        "sensors/cargo-allow/ub-review-sensor-status.json",
        "sensors/ripr/ub-review-sensor-status.json",
        "sensors/unsafe-review/ub-review-sensor-status.json",
        "sensors/ast-grep/ub-review-sensor-status.json",
        "sensors/actionlint/ub-review-sensor-status.json",
        "review/shared_context.md",
        "review/shared_context_cache_block.md",
        "review/shared_context_hash.txt",
        "review/cache_manifest.json",
        "review/cache_events.ndjson",
        "review/pr_thread_context.json",
        "review/terminal_state.json",
        "review/gate_outcome.json",
        "review/resolved-tools.json",
        "review/tool-status.json",
        "review/tool-gate-outcomes.json",
        "review/provider-preflight-status.json",
        "review/metrics.json",
        "review/scheduler.json",
        "review/review.json",
        "review/review.md",
        "review/candidates.json",
        "review/observations.json",
        "review/unique_observations.json",
        "review/merged_observations.json",
        "review/dropped_observations.json",
        "review/orchestrator_plan.json",
        "review/final_orchestrator_plan.json",
        "review/follow_up_results.json",
        "review/follow_up_outputs.json",
        "review/follow_up_evidence.json",
        "review/resolved_candidates.json",
        "review/witnesses.json",
        "review/witness_registry.json",
        "review/proof_requests.json",
        "review/proof_planner_input.json",
        "review/proof_planner_output.json",
        "review/proof_receipts.json",
        "review/receipt_routes.json",
        "review/proof_plan.md",
        "review/resource_leases.json",
        "review/resource_plan.md",
        "candidates.ndjson",
        "follow_up_questions.ndjson",
        "follow_up_results.ndjson",
        "follow_up_outputs.ndjson",
        "resolved_candidates.ndjson",
        "proof_requests.ndjson",
        "proof_tasks.ndjson",
        "proof_receipts.ndjson",
        "receipt_routes.ndjson",
        "tool_gate_outcomes.ndjson",
        "witnesses.ndjson",
        "resource_leases.ndjson",
        "review/github-review-skip.json",
    ] {
        assert!(out.join(path).exists(), "missing {}", path);
    }
    assert!(out.join("candidates").is_dir());
    assert!(out.join("proof_requests").is_dir());
    assert!(out.join("questions").is_dir());

    assert!(!out.join("review/github-review.json").exists());
    let github_skip: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/github-review-skip.json"))?)?;
    assert_eq!(github_skip["status"], "skipped");
    assert_eq!(github_skip["review_payload_status"], "skipped_empty_smoke");
    assert_eq!(github_skip["terminal_state"], "artifact-only");
    assert!(github_skip["github_review_json"].is_null());
    assert_eq!(github_skip["run_pass"], "opened");
    let artifact_body = fs::read_to_string(out.join("review/review.md"))?;
    assert!(artifact_body.contains("## Confirmed findings"));
    assert!(artifact_body.contains("## Missing or failed evidence"));
    assert!(artifact_body.contains("## Model lanes"));
    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    assert_eq!(review["mode"], "review-byok");
    assert_eq!(review["review_profile"], "bun-ub-v0");
    // No --provider-policy flag and no [providers].policy in the fixture
    // config: the resolved policy stays `auto` (built-in minimax-primary
    // semantics), recorded as what it is rather than as a value nobody set.
    assert_eq!(review["provider_policy"], "auto");
    assert_eq!(review["model_provider_policy"], "auto");
    assert_eq!(review["runtime_profile"], "gh-runner");
    assert_eq!(review["run_pass"], "opened");
    assert_eq!(review["depth"], "standard");
    assert_eq!(review["lane_width"], 10);
    assert_eq!(review["model_concurrency"], 8);
    assert_eq!(review["max_model_calls"], 14);
    let resolved_profile: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-profile.json"))?)?;
    assert_eq!(resolved_profile["schema"], "ub-review.resolved_profile.v1");
    assert_eq!(resolved_profile["selected_profile"], "gh-runner");
    assert_eq!(resolved_profile["selected_review_profile"], "bun-ub-v0");
    assert_eq!(resolved_profile["selected_runtime_profile"], "gh-runner");
    assert_eq!(resolved_profile["review_profile"]["name"], "bun-ub-v0");
    assert_eq!(resolved_profile["review_profile"]["repo_kind"], "bun");
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
    assert_eq!(
        resolved_profile["profile"]["budgets"]["default_timeout_sec"],
        serde_json::json!(1800)
    );
    assert_eq!(
        resolved_profile["profile"]["budgets"]["hard_timeout_sec"],
        serde_json::json!(3600)
    );
    assert_eq!(
        resolved_profile["profile"]["trusted_repo"]["pass_triggers"],
        serde_json::json!(["opened", "ready_for_review"])
    );
    assert_eq!(
        resolved_profile["profile"]["trusted_repo"]["synchronize"],
        serde_json::json!(false)
    );
    assert_eq!(
        resolved_profile["review_body"]["include_successful_lane_table"],
        serde_json::json!(false)
    );
    assert_eq!(
        resolved_profile["review_body"]["include_provider_table"],
        serde_json::json!("on_failure")
    );
    assert_eq!(
        resolved_profile["review_body"]["include_execution_summary"],
        serde_json::json!("none")
    );
    assert_eq!(resolved_profile["review"]["posting_engine"], "artifact");
    assert!(resolved_profile["tools"]["tokmd"]["enabled"].as_bool() == Some(true));
    let resolved_plan: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-plan.json"))?)?;
    assert_eq!(resolved_plan["schema"], "ub-review.resolved_plan.v1");
    assert_eq!(resolved_plan["review_profile"], "bun-ub-v0");
    assert_eq!(resolved_plan["profile_name"], "gh-runner");
    assert_eq!(resolved_plan["runtime_profile"], "gh-runner");
    assert_eq!(resolved_plan["run_pass"], "opened");
    assert_eq!(resolved_plan["diff_class"], "source-ub");
    assert_eq!(
        resolved_plan["language_mix"]["languages"],
        serde_json::json!(["rust"])
    );
    assert_eq!(
        resolved_plan["language_mix"]["primary_language"],
        serde_json::json!("rust")
    );
    assert_eq!(
        resolved_plan["language_mix"]["mixed_language"],
        serde_json::json!(false)
    );
    assert!(
        resolved_plan["language_mix"]["surfaces"]
            .as_array()
            .is_some_and(|surfaces| {
                surfaces.iter().any(|surface| surface == "source")
                    && surfaces.iter().any(|surface| surface == "tests")
            })
    );
    assert_eq!(resolved_plan["budgets"]["default_timeout_sec"], 1800);
    assert_eq!(resolved_plan["budgets"]["hard_timeout_sec"], 3600);
    assert_eq!(resolved_plan["budgets"]["proof_max_focused_tests"], 1);
    assert_eq!(resolved_plan["budgets"]["proof_total_timeout_sec"], 600);
    assert_eq!(resolved_plan["budgets"]["proof_disk_mb"], 1024);
    assert_eq!(
        resolved_plan["trusted_repo"]["pass_triggers"],
        serde_json::json!(["opened", "ready_for_review"])
    );
    assert_eq!(
        resolved_plan["trusted_repo"]["synchronize"],
        serde_json::json!(false)
    );
    assert_eq!(
        resolved_plan["review_body"]["include_successful_lane_table"],
        serde_json::json!(false)
    );
    assert_eq!(
        resolved_plan["review_body"]["include_sensor_table"],
        serde_json::json!("on_failure")
    );
    assert_eq!(resolved_plan["limits"]["sensor_jobs"], 4);
    assert_eq!(resolved_plan["selectors"]["run_pass"], "opened");
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
    assert!(out.join("lanes/ub-memory-lifetime.md").exists());
    assert!(
        !out.join("lanes/ub.md").exists(),
        "lane packets should reflect routed effective lanes, not stale default lanes"
    );
    let resolved_tools: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/resolved-tools.json"))?)?;
    assert_eq!(resolved_tools["schema"], "ub-review.resolved_tools.v1");
    assert_eq!(resolved_tools["runtime_profile"], "gh-runner");
    assert!(resolved_tools["tools"].as_array().is_some_and(|tools| {
        tools.iter().any(|tool| {
            tool["id"] == "tokmd"
                && tool["required_if"] == "always"
                && tool["planned_run"] == true
                && tool["artifact_paths"].as_array().is_some_and(|paths| {
                    paths.iter().any(|path| path == "sensors/tokmd/analyze.md")
                })
        })
    }));
    let tools = resolved_tools["tools"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("resolved tools must be an array"))?;
    let cargo_allow = tools
        .iter()
        .find(|tool| tool["id"] == "cargo-allow")
        .ok_or_else(|| anyhow::anyhow!("cargo-allow resolved tool missing"))?;
    assert_eq!(cargo_allow["class"], "static");
    assert_eq!(cargo_allow["required_if"], "source-exception-changed");
    assert_eq!(
        cargo_allow["required_reason"],
        "source-tree exception surface changed"
    );
    let cargo_allow_paths = cargo_allow["artifact_paths"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("cargo-allow artifact_paths must be an array"))?;
    assert!(
        cargo_allow_paths
            .iter()
            .any(|path| path == "sensors/cargo-allow/cargo-allow.receipt.json")
    );
    assert!(
        cargo_allow_paths
            .iter()
            .any(|path| path == "sensors/cargo-allow/cargo-allow.md")
    );
    match (
        cargo_allow["planned_run"].as_bool(),
        cargo_allow["plan_reason"].as_str(),
    ) {
        (Some(false), Some("cargo-allow policy config not found")) => {}
        (Some(false), Some(CARGO_ALLOW_FOREIGN_REASON)) => {}
        (Some(true), Some("source-tree exception surface changed")) => {}
        _ => bail!("unexpected cargo-allow plan state: {cargo_allow}"),
    }
    assert!(resolved_tools["tools"].as_array().is_some_and(|tools| {
        tools.iter().any(|tool| {
            tool["id"] == "coverage"
                && tool["class"] == "coverage"
                && tool["enabled"] == false
                && tool["required_if"] == "manual"
                && tool["planned_run"] == false
                && tool["plan_reason"] == "disabled by config"
                && tool["requires_lease"] == true
                && tool["artifact_paths"].as_array().is_some_and(|paths| {
                    paths
                        .iter()
                        .any(|path| path == "sensors/coverage/status.json")
                        && paths
                            .iter()
                            .any(|path| path == "sensors/coverage/lcov.info")
                })
        })
    }));
    let tool_status: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/tool-status.json"))?)?;
    assert_eq!(tool_status["schema"], "ub-review.tool_status.v1");
    assert!(tool_status["tools"].as_array().is_some_and(|tools| {
        tools.iter().any(|tool| {
            tool["id"] == "ripr"
                && tool["planned_run"] == true
                && tool["timeout_sec"].as_u64().is_some()
                && tool["artifact_budget_mb"].as_u64().is_some()
                && tool["requires_lease"].as_bool().is_some()
                && tool["status"] == "skipped"
                && tool["reason"] == "dry-run; sensor not executed"
        })
    }));
    let cargo_allow_status = tool_status["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["id"] == "cargo-allow"))
        .ok_or_else(|| anyhow::anyhow!("cargo-allow tool status missing"))?;
    match (
        cargo_allow_status["planned_run"].as_bool(),
        cargo_allow_status["status"].as_str(),
        cargo_allow_status["reason"].as_str(),
    ) {
        (Some(false), Some("skipped"), Some("cargo-allow policy config not found")) => {}
        (Some(false), Some("skipped"), Some(CARGO_ALLOW_FOREIGN_REASON)) => {}
        (Some(true), Some("skipped"), Some("dry-run; sensor not executed")) => {}
        _ => bail!("unexpected cargo-allow tool status: {cargo_allow_status}"),
    }
    assert!(tool_status["tools"].as_array().is_some_and(|tools| {
        tools.iter().any(|tool| {
            tool["id"] == "coverage"
                && tool["class"] == "coverage"
                && tool["planned_run"] == false
                && tool["status"] == "skipped"
                && tool["reason"] == "disabled by config"
        })
    }));
    let root_resolved_tools: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-tools.json"))?)?;
    assert_eq!(root_resolved_tools, resolved_tools);
    let root_tool_status: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("tool-status.json"))?)?;
    assert_eq!(root_tool_status, tool_status);
    let tool_gate_outcomes: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/tool-gate-outcomes.json"))?)?;
    assert_eq!(
        tool_gate_outcomes["schema"],
        "ub-review.tool_gate_outcomes.v1"
    );
    assert_eq!(tool_gate_outcomes["runtime_profile"], "gh-runner");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(
            out.join("tool-gate-outcomes.json")
        )?)?,
        tool_gate_outcomes
    );
    let outcome_lines = fs::read_to_string(out.join("tool_gate_outcomes.ndjson"))?
        .lines()
        .filter(|line| !line.trim().is_empty())
        .count();
    assert_eq!(
        outcome_lines,
        tool_gate_outcomes["outcomes"]
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default()
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
            "--run-pass",
            "ready_for_review",
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
    assert_eq!(capped_review["run_pass"], "ready_for_review");
    assert_eq!(capped_review["model_concurrency"], 12);
    let capped_resolved_plan: serde_json::Value =
        serde_json::from_slice(&fs::read(capped_out.join("resolved-plan.json"))?)?;
    assert_eq!(capped_resolved_plan["runtime_profile"], "cx23");
    assert_eq!(capped_resolved_plan["run_pass"], "ready_for_review");
    assert_eq!(capped_resolved_plan["limits"]["llm_in_flight"], 12);
    assert_eq!(capped_resolved_plan["limits"]["sensor_jobs"], 2);
    assert_eq!(
        capped_resolved_plan["budgets"]["proof_max_focused_tests"],
        2
    );
    assert_eq!(capped_resolved_plan["budgets"]["proof_cpu"], 1);
    assert_eq!(capped_resolved_plan["budgets"]["proof_memory_mb"], 1024);
    assert_eq!(capped_resolved_plan["budgets"]["hard_timeout_sec"], 3600);
    assert_eq!(capped_resolved_plan["selectors"]["model_concurrency"], 12);
    assert_eq!(
        capped_resolved_plan["selectors"]["run_pass"],
        "ready_for_review"
    );
    let full_out = temp.path().join("packet-full-profile");
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
            path_str(&full_out)?,
            "--runtime-profile",
            "gh-runner-full",
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
    )?;
    let full_resolved_plan: serde_json::Value =
        serde_json::from_slice(&fs::read(full_out.join("resolved-plan.json"))?)?;
    assert_eq!(full_resolved_plan["runtime_profile"], "gh-runner-full");
    assert_eq!(full_resolved_plan["budgets"]["mutation"], true);
    assert_eq!(full_resolved_plan["budgets"]["sanitizer"], true);
    assert_eq!(
        full_resolved_plan["trusted_repo"]["pass_triggers"],
        serde_json::json!(["opened", "ready_for_review"])
    );
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
    let gate_outcome: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/gate_outcome.json"))?)?;
    assert_eq!(gate_outcome["schema"], "ub-review.gate_outcome.v1");
    assert_eq!(gate_outcome["conclusion"], "pass");
    assert_eq!(gate_outcome["terminal_status"], terminal_state["status"]);
    assert_eq!(gate_outcome["reasons"], serde_json::json!([]));
    assert_eq!(gate_outcome["tool_gates"]["evaluated"], 0);
    let shared_context = fs::read_to_string(out.join("review/shared_context.md"))?;
    assert!(shared_context.contains("- Changed languages: `rust`"));
    assert!(shared_context.contains("- Changed surfaces: `source, tests`"));
    assert!(shared_context.contains("- Primary language: `rust`"));
    assert!(shared_context.contains("- Mixed-language diff: `false`"));
    assert!(shared_context.contains("## Initial Work Queue"));
    assert!(shared_context.contains("- Rule: pending work is unfinished, not missing evidence."));
    assert!(shared_context.contains("### Ready Initial Packet Receipts"));
    assert!(shared_context.contains("### Pending Initial Packet Tasks"));
    assert!(shared_context.contains("## PR Thread Context"));
    assert!(shared_context.contains("- Status: `"));
    let shared_context_cache_block =
        fs::read_to_string(out.join("review/shared_context_cache_block.md"))?;
    assert_eq!(shared_context_cache_block, shared_context);
    let shared_context_hash = fs::read_to_string(out.join("review/shared_context_hash.txt"))?;
    let shared_context_hash = shared_context_hash.trim();
    assert_eq!(
        Some(shared_context_hash),
        review["shared_context_id"].as_str()
    );
    let cache_manifest: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/cache_manifest.json"))?)?;
    assert_eq!(cache_manifest["schema"], "ub-review.cache_manifest.v1");
    assert_eq!(cache_manifest["shared_context_hash"], shared_context_hash);
    assert_eq!(
        cache_manifest["cache_block_path"],
        "review/shared_context_cache_block.md"
    );
    assert_eq!(
        cache_manifest["explicit_cache_endpoint"],
        "anthropic-messages"
    );
    let cache_events = fs::read_to_string(out.join("review/cache_events.ndjson"))?;
    assert!(cache_events.contains("\"kind\":\"shared_context_prepared\""));
    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    let scheduler: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/scheduler.json"))?)?;
    assert_eq!(scheduler["schema"], "ub-review.scheduler.v1");
    assert_eq!(
        scheduler["scheduler_profile"],
        metrics["run"]["scheduler_profile"]
    );
    assert_eq!(
        scheduler["overlaps"]["investigation_proof_overlap_ms"],
        metrics["run"]["investigation_proof_overlap_ms"]
    );
    assert_eq!(
        scheduler["scheduler_roles"],
        metrics["run"]["scheduler_roles"]
    );
    assert!(
        scheduler["phases"]
            .as_array()
            .is_some_and(|phases| phases.iter().any(|phase| {
                phase["loop_id"] == "proof" && phase["stage"] == "initial-diff-broker"
            })),
        "scheduler artifact should include initial diff proof phase"
    );
    let diff_context: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("input/diff-context.json"))?)?;
    let plan_json: serde_json::Value = serde_json::from_slice(&fs::read(out.join("plan.json"))?)?;
    assert_eq!(metrics["schema_version"], 1);
    assert_eq!(metrics["shared_context_id"], review["shared_context_id"]);
    assert_eq!(metrics["terminal_state"], terminal_state["status"]);
    let final_orchestrator_plan: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/final_orchestrator_plan.json"))?)?;
    assert_eq!(
        metrics["final_follow_up_tasks"],
        serde_json::json!(
            final_orchestrator_plan["follow_up_tasks"]
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("final follow_up_tasks missing"))?
                .len()
        )
    );
    assert_eq!(
        terminal_state["final_follow_up_tasks"],
        metrics["final_follow_up_tasks"]
    );
    assert_eq!(
        metrics["run"]["concurrency_model"],
        "profiled-stream-scheduler-v0"
    );
    assert_eq!(
        metrics["run"]["scheduler_profile"],
        "default-three-stream-v0"
    );
    assert_eq!(metrics["run"]["local_proof_wall_excludes_model_wait"], true);
    for pointer in [
        "/final_follow_up_tasks",
        "/run/elapsed_wall_ms",
        "/run/coordination_wall_ms",
        "/run/investigation_wall_ms",
        "/run/proof_wall_ms",
        "/run/evidence_wall_ms",
        "/run/model_wall_ms",
        "/run/local_proof_wall_ms",
        "/run/compiler_wall_ms",
        "/run/model_call_duration_ms_sum",
        "/run/proof_command_duration_ms_sum",
        "/run/investigation_proof_overlap_ms",
        "/run/model_proof_overlap_ms",
        "/run/proof_overlap_ms",
        "/run/scheduler_roles/evidence/started_at_offset_ms",
        "/run/scheduler_roles/evidence/finished_at_offset_ms",
        "/run/scheduler_roles/evidence/wall_ms",
        "/run/scheduler_roles/model/started_at_offset_ms",
        "/run/scheduler_roles/model/finished_at_offset_ms",
        "/run/scheduler_roles/model/wall_ms",
        "/run/scheduler_roles/proof/started_at_offset_ms",
        "/run/scheduler_roles/proof/finished_at_offset_ms",
        "/run/scheduler_roles/proof/wall_ms",
        "/run/streams/coordination/started_at_offset_ms",
        "/run/streams/coordination/finished_at_offset_ms",
        "/run/streams/coordination/wall_ms",
        "/run/streams/investigation/started_at_offset_ms",
        "/run/streams/investigation/finished_at_offset_ms",
        "/run/streams/investigation/wall_ms",
        "/run/streams/proof/started_at_offset_ms",
        "/run/streams/proof/finished_at_offset_ms",
        "/run/streams/proof/wall_ms",
        "/run/loops/evidence/started_at_offset_ms",
        "/run/loops/evidence/finished_at_offset_ms",
        "/run/loops/evidence/wall_ms",
        "/run/loops/model/started_at_offset_ms",
        "/run/loops/model/finished_at_offset_ms",
        "/run/loops/model/wall_ms",
        "/run/loops/proof/started_at_offset_ms",
        "/run/loops/proof/finished_at_offset_ms",
        "/run/loops/proof/wall_ms",
        "/run/loops/compiler/started_at_offset_ms",
        "/run/loops/compiler/finished_at_offset_ms",
        "/run/loops/compiler/wall_ms",
        "/models/prompt_cache_creation_input_tokens",
        "/models/prompt_cache_read_input_tokens",
        "/models/prompt_cache_lane_hits",
        "/models/prompt_cache_lane_misses",
        "/models/prompt_cache_lane_unknown",
    ] {
        assert!(
            metrics
                .pointer(pointer)
                .and_then(serde_json::Value::as_u64)
                .is_some(),
            "missing non-negative metric {pointer}"
        );
    }
    assert_eq!(metrics["review_profile"], "bun-ub-v0");
    assert_eq!(metrics["profile_name"], "gh-runner");
    assert_eq!(metrics["runtime_profile"], "gh-runner");
    assert_eq!(metrics["run_pass"], review["run_pass"]);
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
        resolved_plan["selectors"]["effective_model_lanes"]
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
    let follow_up_results: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/follow_up_results.json"))?)?;
    assert_eq!(
        metrics["follow_up_results"]["total"]
            .as_u64()
            .unwrap_or_default(),
        follow_up_results
            .as_array()
            .map(std::vec::Vec::len)
            .unwrap_or_default() as u64
    );
    assert_eq!(
        sum_json_object_values(&metrics["follow_up_results"]["status_counts"]),
        metrics["follow_up_results"]["total"]
            .as_u64()
            .unwrap_or_default()
    );
    assert_eq!(metrics["follow_up_results"]["calls_attempted"], 0);
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

    let effective_lanes = resolved_plan["selectors"]["effective_model_lanes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("effective_model_lanes missing"))?;
    for lane in effective_lanes.iter().filter_map(serde_json::Value::as_str) {
        let path = out.join("lanes").join(format!("{lane}.md"));
        assert!(path.exists(), "missing lane {lane}");
        let text = fs::read_to_string(path)?;
        assert!(!has_standalone_approval_line(&text));
        assert!(text.contains(&format!("[{lane}]")));
    }
    assert!(!out.join("lanes/ub.md").exists());

    let summary = fs::read_to_string(out.join("running-summary.md"))?;
    assert!(!has_standalone_approval_line(&summary));
    assert!(summary.contains("## Missing evidence"));
    assert!(summary.contains("## Provider preflights"));
    assert!(summary.contains("## Model lane status"));
    assert!(summary.contains("## Missing or failed model evidence"));
    assert!(summary.contains("## Review efficiency"));
    assert!(summary.contains("Run streams:"));
    assert!(summary.contains("Loop detail:"));
    assert!(summary.contains("Follow-up results:"));
    assert!(summary.contains("`ub-memory-lifetime`"));
    assert!(summary.contains("MiniMax-M3"));
    assert!(summary.contains("## Lane packets"));
    let event_kinds = event_kinds(&out.join("events.ndjson"))?;
    let events = event_records(&out.join("events.ndjson"))?;
    let run_started = events
        .iter()
        .find(|event| event["kind"] == "run_started")
        .ok_or_else(|| anyhow::anyhow!("missing run_started event"))?;
    assert_eq!(run_started["payload"]["run_pass"], "opened");
    for kind in [
        "run_started",
        "evidence_loop_started",
        "evidence_stream_started",
        "evidence_stream_completed",
        "coordination_stream_started",
        "coordination_stream_completed",
        "model_loop_started",
        "model_loop_finished",
        "model_stream_started",
        "model_stream_completed",
        "investigation_stream_started",
        "investigation_stream_completed",
        "proof_loop_started",
        "proof_loop_finished",
        "proof_stream_started",
        "proof_stream_completed",
        "compiler_loop_started",
        "compiler_loop_finished",
        "terminal_state",
        "run_finished",
    ] {
        assert!(
            event_kinds.iter().any(|event| event == kind),
            "missing event {kind}"
        );
    }
    Ok(())
}

#[test]
fn declared_pr_base_measures_only_stack_layer() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
    fs::create_dir_all(repo.join("docs"))?;
    let manifest = r#"[package]
name = "stack-diff-mini"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#;
    write_file(&repo.join("Cargo.toml"), manifest)?;
    write_file(&repo.join("src/lib.rs"), "pub fn value() -> u32 { 1 }\n")?;

    run(&repo, "git", &["init"])?;
    let git_email = ["config", "user.email", "ub-review@example.invalid"];
    run(&repo, "git", &git_email)?;
    run(&repo, "git", &["config", "user.name", "UB Review Test"])?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "baseline"])?;
    run(&repo, "git", &["switch", "-c", "stack-base"])?;
    write_file(&repo.join("docs/ancestor.md"), "ancestor-only\n")?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "ancestor layer"])?;
    run(&repo, "git", &["switch", "-c", "stack-head"])?;
    write_file(&repo.join("src/lib.rs"), "pub fn value() -> u32 { 2 }\n")?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "head layer"])?;

    let out = temp.path().join("packet");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
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
            "stack-base",
            "--head",
            "stack-head",
            "--out",
            path_str(&out)?,
            "--run-pass",
            "opened",
            "--no-github-summary",
        ],
    )?;

    let diff_context: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("input/diff-context.json"))?)?;
    let changed_files = diff_context["changed_files"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("changed_files missing"))?;
    let changed_files: Vec<&str> = changed_files
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(
        changed_files.contains(&"src/lib.rs"),
        "head-layer source change missing: {changed_files:?}"
    );
    assert!(
        !changed_files.contains(&"docs/ancestor.md"),
        "declared PR base should exclude ancestor-only stack files: {changed_files:?}"
    );
    Ok(())
}

#[test]
fn cargo_allow_foreign_policy_ledger_skips_with_linked_artifact_reason() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
    write_file(
        &repo.join("Cargo.toml"),
        r#"[package]
name = "foreign-policy-mini"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )?;
    write_file(&repo.join("src/lib.rs"), "pub fn value() -> u32 { 1 }\n")?;
    run(&repo, "git", &["init"])?;
    run(
        &repo,
        "git",
        &["config", "user.email", "ub-review@example.invalid"],
    )?;
    run(&repo, "git", &["config", "user.name", "UB Review Test"])?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "baseline"])?;

    fs::create_dir_all(repo.join("policy"))?;
    write_file(
        &repo.join("policy/allow.toml"),
        "schema_version = \"1\"\ntool = \"xtask-policy\"\n",
    )?;
    write_file(&repo.join("src/lib.rs"), "pub fn value() -> u32 { 2 }\n")?;
    run(&repo, "git", &["add", "."])?;
    run(
        &repo,
        "git",
        &["commit", "-m", "touch source with foreign policy ledger"],
    )?;

    let config = temp.path().join("ub-review.toml");
    write_file(
        &config,
        r#"review_profile = "bun-ub-v0"
profile = "gh-runner"

[repo]
kind = "rust"
ledger = ""
base = "HEAD~1"
head = "HEAD"

[tools.cargo-allow]
enabled = true
class = "static"
default = "source-exception-changed"
required = true
weight = 2
timeout_sec = 120
artifact_budget_mb = 64
requires_lease = false

[tools.cargo-allow.gate]
scope = "on-diff"
max_new_unsuppressed = 0
"#,
    )?;

    let out = temp.path().join("packet");
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
            "--run-pass",
            "opened",
            "--no-github-summary",
        ],
    )?;

    assert_cargo_allow_foreign_skip_artifacts(&out)
}

#[test]
fn dry_run_accepts_intelligent_ci_mode() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
    write_file(
        &repo.join("Cargo.toml"),
        r#"[package]
name = "ub-review-mode-mini"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )?;
    write_file(&repo.join("src/lib.rs"), "pub fn value() -> u32 { 1 }\n")?;

    run(&repo, "git", &["init"])?;
    run(
        &repo,
        "git",
        &["config", "user.email", "ub-review@example.invalid"],
    )?;
    run(&repo, "git", &["config", "user.name", "UB Review Test"])?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "baseline"])?;

    write_file(&repo.join("src/lib.rs"), "pub fn value() -> u32 { 2 }\n")?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "change value"])?;

    let out = temp.path().join("packet");
    let config = temp.path().join("ub-review.toml");
    write_file(
        &config,
        r#"review_profile = "bun-ub-v0"
profile = "gh-runner"

[repo]
kind = "rust"
ledger = ""
base = "HEAD~1"
head = "HEAD"

[[proof.required]]
id = "cargo-check"
languages = ["rust"]
diff_classes = ["source-general"]
command = "cargo check --workspace --locked"
reason = "Required Rust workspace check for intelligent CI."
cost = "focused-build"
timeout_sec = 300
required = true
"#,
    )?;
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
            "--mode",
            "intelligent-ci",
            "--model-mode",
            "off",
            "--run-pass",
            "ready_for_review",
            "--no-github-summary",
        ],
    )?;

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    assert_eq!(review["mode"], "intelligent-ci");
    assert_eq!(metrics["mode"], "intelligent-ci");
    assert_eq!(review["run_pass"], "ready_for_review");
    assert_eq!(metrics["run_pass"], "ready_for_review");
    let proof_requests_json: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_requests.json"))?)?;
    let proof_requests = proof_requests_json
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("proof_requests artifact is not an array"))?;
    assert_eq!(proof_requests.len(), 1);
    let request = &proof_requests[0];
    assert_eq!(request["lane"], "intelligent-ci-policy");
    assert_eq!(request["command"], "cargo check --workspace --locked");
    assert_eq!(request["status"], "requested");
    assert_eq!(request["cost"], "focused-build");
    assert_eq!(request["required"], true);
    let requested_by = request["requested_by"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("request requested_by is not an array"))?;
    assert!(
        requested_by
            .iter()
            .any(|value| value.as_str() == Some("proof-policy:cargo-check")),
        "policy requester missing from proof request"
    );
    let request_id = request["id"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("request id missing"))?;

    let proof_receipts: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
    let receipt = proof_receipts
        .as_array()
        .and_then(|receipts| {
            receipts.iter().find(|receipt| {
                receipt["request_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
            })
        })
        .ok_or_else(|| anyhow::anyhow!("missing proof receipt for policy request"))?;
    assert_eq!(receipt["kind"], "focused-build");
    assert_eq!(receipt["result"], "skipped_profile");

    let resource_leases: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/resource_leases.json"))?)?;
    let lease = resource_leases
        .as_array()
        .and_then(|leases| {
            leases.iter().find(|lease| {
                lease["kind"] == "focused-build"
                    && lease["status"] == "skipped_profile"
                    && lease["command"]
                        .as_str()
                        .is_some_and(|command| command.contains("cargo check --workspace --locked"))
            })
        })
        .ok_or_else(|| anyhow::anyhow!("missing skipped focused-build lease"))?;
    assert!(
        lease["consumer"]
            .as_str()
            .is_some_and(|consumer| consumer.starts_with("proof-build-"))
    );

    let resolved_plan: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-plan.json"))?)?;
    assert_eq!(
        resolved_plan["proof_policy"]["matched_required"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(
        resolved_plan["proof_policy"]["matched_required"][0]["id"],
        "cargo-check"
    );
    Ok(())
}

#[test]
fn cache_warm_writes_base_and_rule_manifests() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
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
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    run(
        temp.path(),
        bin,
        &[
            "cache",
            "warm",
            "--config",
            path_str(&config)?,
            // Pin the profile: auto box detection resolves differently on
            // runners with the full sensor image installed (gh-runner-full),
            // and this test asserts the manifest profile value.
            "--profile",
            "gh-runner",
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
        6
    );

    let base_dir = cache.join("bases").join(base_tree_sha);
    assert!(base_dir.join("manifest.json").exists());
    for tool in [
        "tokmd",
        "cargo-allow",
        "ripr",
        "unsafe-review",
        "ast-grep",
        "actionlint",
    ] {
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
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
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
    assert!(output.contains("Fixes:"));
    assert!(
        output.contains("tokmd missing: cargo install tokmd --locked --version 1.12.0 --force")
    );
    assert!(output.contains("see Fixes above"));
    Ok(())
}

#[test]
fn doctor_reports_provider_key_env_status_without_values() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let config = temp.path().join(".ub-review.toml");
    write_file(&config, r#"profile = "gh-runner""#)?;

    let bin = env!("CARGO_BIN_EXE_ub-review");
    let output = run_capture_with_env(
        temp.path(),
        bin,
        &["doctor", "--config", path_str(&config)?],
        &[
            ("UB_REVIEW_MINIMAX_API_KEY", "minimax-secret-value"),
            ("UB_REVIEW_OPENCODE_API_KEY", "   "),
        ],
    )?;

    assert!(output.contains("Providers:"));
    assert!(output.contains("Binary path:"));
    assert!(output.contains("path="));
    assert!(output.contains("minimax"));
    assert!(output.contains("present"));
    assert!(output.contains("env=UB_REVIEW_MINIMAX_API_KEY"));
    assert!(output.contains("opencode-go"));
    assert!(output.contains("missing"));
    assert!(output.contains("env=UB_REVIEW_OPENCODE_API_KEY"));
    assert!(!output.contains("minimax-secret-value"));
    Ok(())
}

#[test]
fn doctor_standard_image_env_requires_core_tools() -> Result<()> {
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
    let output = run_expect_failure_with_env(
        temp.path(),
        bin,
        &["doctor", "--config", path_str(&config)?],
        &[("UB_REVIEW_STANDARD_IMAGE", "true")],
    )?;
    assert!(output.contains("required core review tools missing from standard image"));
    assert!(output.contains("tokmd"));
    assert!(output.contains("Fixes:"));
    assert!(
        output.contains("tokmd missing: cargo install tokmd --locked --version 1.12.0 --force")
    );
    assert!(output.contains("see Fixes above"));
    Ok(())
}

#[test]
fn doctor_require_core_tools_fails_stale_tokmd_version() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let fake_bin = temp.path().join("fake-bin");
    write_fake_core_review_tools(&fake_bin, "1.10.0")?;
    let path = prepend_to_path(&fake_bin)?;
    let config = temp.path().join(".ub-review.toml");
    write_file(&config, r#"profile = "gh-runner""#)?;

    let bin = env!("CARGO_BIN_EXE_ub-review");
    let output = run_expect_failure_with_env(
        temp.path(),
        bin,
        &[
            "doctor",
            "--config",
            path_str(&config)?,
            "--require-core-tools",
        ],
        &[("PATH", path.as_str())],
    )?;
    assert!(output.contains("required core review tool versions drifted"));
    assert!(output.contains("tokmd expected 1.12.0"));
    assert!(output.contains("tokmd 1.10.0"));
    assert!(output.contains("Fixes:"));
    assert!(
        output
            .contains("tokmd version drift: cargo install tokmd --locked --version 1.12.0 --force")
    );
    assert!(output.contains("see Fixes above"));
    Ok(())
}

#[test]
fn run_with_ledger_path_writes_bounded_shared_context() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
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
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
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
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
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
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
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
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
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
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
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

    let planner_input: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_planner_input.json"))?)?;
    assert_eq!(planner_input["schema"], "ub-review.proof_planner_input.v1");
    assert_eq!(planner_input["diff_class"], "tests-only");
    assert_eq!(planner_input["runtime_budget"]["max_focused_tests"], 1);
    let shared_context = fs::read_to_string(out.join("review/shared_context.md"))?;
    assert!(shared_context.contains("## Initial Work Queue"));
    assert!(shared_context.contains("### Pending Initial Packet Tasks"));
    assert!(shared_context.contains("`late-follow-up`, `trust-affecting`"));
    assert!(shared_context.contains("review/proof_receipts.json"));

    let planner_output: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_planner_output.json"))?)?;
    assert_eq!(
        planner_output["schema"],
        "ub-review.proof_planner_output.v1"
    );
    assert_eq!(planner_output["lane"], "proof-planner");
    let proof_tasks = json_array_field(&planner_output, "proof_tasks")?;
    assert_eq!(proof_tasks.len(), 1);
    assert_eq!(proof_tasks[0]["schema"], "ub-review.proof_task.v1");
    assert_eq!(proof_tasks[0]["kind"], "focused-test");
    assert_eq!(proof_tasks[0]["status"], "planned");
    assert_eq!(proof_tasks[0]["mode"], "red-green");
    assert_eq!(proof_tasks[0]["lease"]["cpu"], 2);
    assert_eq!(proof_tasks[0]["lease"]["network"], false);
    assert!(
        proof_tasks[0]["purpose"]
            .as_str()
            .is_some_and(|purpose| { purpose.contains("fails on base+tests and passes on HEAD") })
    );
    let proof_tasks_ndjson = fs::read_to_string(out.join("proof_tasks.ndjson"))?;
    assert_eq!(
        proof_tasks_ndjson
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        1
    );

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
    assert_eq!(commands[0]["env"], serde_json::json!({}));
    assert_eq!(
        commands[1]["env"],
        serde_json::json!({"USE_SYSTEM_BUN": "1"})
    );
    assert!(
        commands[0]["command"]
            .as_str()
            .is_some_and(|command| command.starts_with("bun bd test "))
    );
    assert!(
        commands[1]["command"]
            .as_str()
            .is_some_and(|command| command.starts_with("USE_SYSTEM_BUN=1 bun test "))
    );
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
    let proof_command_duration_sum = commands
        .iter()
        .filter_map(|command| command["duration_ms"].as_u64())
        .sum::<u64>();

    let leases: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/resource_leases.json"))?)?;
    assert_eq!(leases.len(), 1);
    assert_eq!(leases[0]["schema"], "ub-review.resource_lease.v1");
    assert_eq!(leases[0]["kind"], "focused-test");
    assert_eq!(leases[0]["status"], "granted");
    assert_eq!(leases[0]["consumer"], receipt["id"]);
    assert_eq!(leases[0]["network"], false);

    let receipt_routes: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/receipt_routes.json"))?)?;
    assert_eq!(receipt_routes["schema"], "ub-review.receipt_routes.v1");
    let routes = json_array_field(&receipt_routes, "routes")?;
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0]["schema"], "ub-review.receipt_route.v1");
    assert_eq!(routes[0]["receipt_id"], receipt["id"]);
    assert_eq!(routes[0]["phase"], "initial-diff-receipt");
    assert_eq!(routes[0]["status"], "tool-confirmed");
    assert_eq!(
        routes[0]["consumers"],
        serde_json::json!(["tests-oracle", "opposition", "compiler"])
    );
    assert_eq!(routes[0]["lease_ids"], serde_json::json!([leases[0]["id"]]));
    assert_eq!(
        routes[0]["source_artifacts"],
        serde_json::json!(["review/proof_receipts.json", "review/resource_leases.json"])
    );
    let route_ndjson = fs::read_to_string(out.join("receipt_routes.ndjson"))?;
    assert_eq!(
        route_ndjson
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        routes.len()
    );

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    assert_eq!(review["proof_receipts"], serde_json::json!(receipts));
    assert_eq!(review["resource_leases"], serde_json::json!(leases));
    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    assert_eq!(metrics["proof_receipts"], 1);
    assert_eq!(metrics["resource_leases"], 1);
    assert_eq!(
        metrics["run"]["proof_command_duration_ms_sum"],
        proof_command_duration_sum
    );
    assert!(
        metrics["run"]["local_proof_wall_ms"]
            .as_u64()
            .is_some_and(|wall| wall >= proof_command_duration_sum)
    );
    assert_eq!(metrics["run"]["investigation_proof_overlap_ms"], 0);
    assert_eq!(metrics["run"]["model_proof_overlap_ms"], 0);
    assert_eq!(metrics["run"]["local_proof_wall_excludes_model_wait"], true);

    let proof_plan = fs::read_to_string(out.join("review/proof_plan.md"))?;
    assert!(proof_plan.contains("Proof broker v0 executed focused proof under the runtime budget"));
    assert!(proof_plan.contains("result=`discriminating`"));
    let resource_plan = fs::read_to_string(out.join("review/resource_plan.md"))?;
    assert!(resource_plan.contains("status=`granted`"));

    let proof_planner_output: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_planner_output.json"))?)?;
    let proof_tasks = proof_planner_output["proof_tasks"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("proof_tasks missing"))?;
    assert_eq!(proof_tasks.len(), 1);
    let proof_task = &proof_tasks[0];
    assert_eq!(proof_task["schema"], "ub-review.proof_task.v1");
    assert_eq!(proof_task["kind"], "focused-test");
    assert_eq!(proof_task["source"], "proof-planner");
    assert_eq!(proof_task["priority"], "high");
    assert_eq!(proof_task["packet_policy"], "late-follow-up");
    assert_eq!(proof_task["gate_policy"], "trust-affecting");
    assert!(
        proof_task["deadline_sec"]
            .as_u64()
            .is_some_and(|deadline| deadline > 0)
    );
    assert_eq!(
        proof_task["lease"]["timeout_sec"],
        proof_task["deadline_sec"]
    );
    assert_eq!(proof_task["timeout_sec"], proof_task["deadline_sec"]);

    let work_queue: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("work_queue.json"))?)?;
    let tool_status: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("tool-status.json"))?)?;
    let tool_status_tools = tool_status["tools"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("tool status tools missing"))?;
    assert_eq!(work_queue["schema"], "ub-review.work_queue.v1");
    assert_eq!(work_queue["initial_packet_deadline_sec"], 60);
    assert_eq!(work_queue["follow_up_deadline_sec"], 300);
    let queue_tasks = work_queue["tasks"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("work queue tasks missing"))?;
    assert_eq!(
        queue_tasks.len(),
        tool_status_tools.len() + proof_tasks.len()
    );
    let sensor_task = queue_tasks
        .iter()
        .find(|task| task["id"] == "sensor-ripr")
        .ok_or_else(|| anyhow::anyhow!("ripr sensor queue task missing"))?;
    let ripr_tool = tool_status_tools
        .iter()
        .find(|tool| tool["id"] == "ripr")
        .ok_or_else(|| anyhow::anyhow!("ripr tool status missing"))?;
    assert_eq!(sensor_task["schema"], "ub-review.work_queue_task.v1");
    assert_eq!(sensor_task["kind"], "sensor");
    assert_eq!(sensor_task["source"], "tool-registry");
    let expected_packet_policy = if ripr_tool["required"].as_bool() == Some(true) {
        "must-run"
    } else if ripr_tool["planned_run"].as_bool() == Some(true) {
        "include-if-ready"
    } else {
        "artifact-only"
    };
    assert_eq!(sensor_task["packet_policy"], expected_packet_policy);
    let expected_gate_policy = if ripr_tool["required"].as_bool() == Some(true) {
        "gate-required"
    } else if ripr_tool["gate"].is_object() {
        "trust-affecting"
    } else if ripr_tool["planned_run"].as_bool() == Some(true) {
        "review-context"
    } else {
        "artifact-only"
    };
    assert_eq!(sensor_task["gate_policy"], expected_gate_policy);
    assert_eq!(
        sensor_task["status"],
        if ripr_tool["planned_run"].as_bool() == Some(true) {
            "planned"
        } else {
            "skipped"
        }
    );
    assert_eq!(
        sensor_task["receipt_path"],
        "sensors/ripr/ub-review-sensor-status.json"
    );
    let sensor_receipt_ready = out
        .join(json_str_field(sensor_task, "receipt_path")?)
        .is_file();
    let expected_initial_packet_status = if sensor_task["status"] == "planned"
        && sensor_receipt_ready
        && matches!(
            sensor_task["packet_policy"].as_str(),
            Some("must-run" | "include-if-ready")
        ) {
        "ready_for_initial_packet"
    } else if sensor_task["status"] == "planned"
        && matches!(
            sensor_task["packet_policy"].as_str(),
            Some("must-run" | "include-if-ready" | "late-follow-up" | "adaptive")
        )
    {
        "pending_initial_packet"
    } else {
        "not_initial_packet"
    };
    assert_eq!(
        sensor_task["initial_packet_status"],
        expected_initial_packet_status
    );
    assert_eq!(sensor_task["task_path"], "resolved-tools.json");
    assert_eq!(
        sensor_task["consumers"],
        serde_json::json!(["tests-oracle", "proof-planner", "compiler"])
    );
    assert_eq!(
        sensor_task["lease"]["timeout_sec"],
        sensor_task["deadline_sec"]
    );
    let queue_task = queue_tasks
        .iter()
        .find(|task| task["kind"] == proof_task["kind"] && task["id"] == proof_task["id"])
        .ok_or_else(|| anyhow::anyhow!("proof queue task missing"))?;
    assert_eq!(queue_task["schema"], "ub-review.work_queue_task.v1");
    for field in [
        "id",
        "kind",
        "source",
        "priority",
        "packet_policy",
        "deadline_sec",
        "consumers",
        "gate_policy",
        "lease",
        "status",
    ] {
        assert_eq!(
            queue_task[field], proof_task[field],
            "queue field {field} should mirror proof task"
        );
    }
    assert_eq!(queue_task["receipt_path"], "review/proof_receipts.json");
    assert_eq!(queue_task["task_path"], "proof_tasks.ndjson");
    assert_eq!(
        queue_task["initial_packet_status"],
        "pending_initial_packet"
    );
    assert!(
        queue_task["dedupe_key"]
            .as_str()
            .is_some_and(|key| key.starts_with("proof-planner:focused-test:"))
    );
    let work_events = fs::read_to_string(out.join("work_events.ndjson"))?;
    let work_event_lines = work_events
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    assert_eq!(work_event_lines.len(), queue_tasks.len());
    let work_events = work_event_lines
        .iter()
        .map(|line| serde_json::from_str::<serde_json::Value>(line))
        .collect::<Result<Vec<_>, _>>()?;
    let work_event = work_events
        .iter()
        .find(|event| event["task_id"] == queue_task["id"])
        .ok_or_else(|| anyhow::anyhow!("proof work event missing"))?;
    assert_eq!(work_event["schema"], "ub-review.work_event.v1");
    assert_eq!(work_event["kind"], "task_planned");
    assert_eq!(work_event["task_id"], queue_task["id"]);
    assert_eq!(work_event["task_kind"], queue_task["kind"]);
    assert_eq!(work_event["source"], queue_task["source"]);
    assert_eq!(work_event["packet_policy"], queue_task["packet_policy"]);
    assert_eq!(work_event["deadline_sec"], queue_task["deadline_sec"]);
    assert_eq!(work_event["consumers"], queue_task["consumers"]);
    assert_eq!(work_event["gate_policy"], queue_task["gate_policy"]);
    assert_eq!(work_event["status"], queue_task["status"]);
    assert_eq!(work_event["receipt_path"], queue_task["receipt_path"]);
    assert_eq!(
        work_event["initial_packet_status"],
        queue_task["initial_packet_status"]
    );

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
    let event_kinds = event_kinds(&out.join("events.ndjson"))?;
    assert!(event_kinds.iter().any(|kind| kind == "proof_loop_started"));
    assert!(event_kinds.iter().any(|kind| kind == "proof_loop_finished"));
    Ok(())
}

#[test]
fn model_auto_run_hits_fake_minimax_provider_and_writes_artifacts() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
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
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    run_with_env(
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
    let candidates: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/candidates.json"))?)?;
    let candidates = candidates
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("candidates missing"))?;
    let candidate = candidates
        .iter()
        .find(|candidate| candidate["lane"].as_str() == Some("ub"))
        .ok_or_else(|| anyhow::anyhow!("ub candidate missing"))?;
    assert_eq!(candidate["schema"], "ub-review.candidate.v1");
    assert_eq!(candidate["source"], "summary-only-finding");
    assert_eq!(candidate["status"], "summary-only");
    assert_eq!(candidate["disposition"], "summary-only");
    assert_eq!(candidate["claim"], "fake provider ok");
    assert_eq!(candidate["evidence"], "lane model summary");
    let candidate_file: serde_json::Value = serde_json::from_slice(&fs::read(
        out.join("candidates")
            .join(format!("{}.json", json_str_field(candidate, "id")?)),
    )?)?;
    assert_eq!(candidate_file, *candidate);
    let candidates_ndjson = fs::read_to_string(out.join("candidates.ndjson"))?;
    assert_eq!(
        candidates_ndjson
            .lines()
            .filter(|line| !line.trim().is_empty())
            .count(),
        candidates.len()
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
    let preflight_duration = preflight["duration_ms"].as_u64().unwrap_or_default();
    let lane_duration = ub_lane["duration_ms"].as_u64().unwrap_or_default();
    assert!(
        metrics["run"]["model_call_duration_ms_sum"]
            .as_u64()
            .is_some_and(|duration| duration >= preflight_duration + lane_duration)
    );
    assert!(
        metrics["run"]["model_wall_ms"]
            .as_u64()
            .is_some_and(|duration| duration > 0)
    );
    let event_kinds = event_kinds(&out.join("events.ndjson"))?;
    assert!(
        event_kinds
            .iter()
            .any(|kind| kind == "coordination_stream_started")
    );
    assert!(
        event_kinds
            .iter()
            .any(|kind| kind == "evidence_stream_started")
    );
    assert!(
        event_kinds
            .iter()
            .any(|kind| kind == "investigation_stream_started")
    );
    assert!(
        event_kinds
            .iter()
            .any(|kind| kind == "model_stream_started")
    );
    assert!(
        event_kinds
            .iter()
            .any(|kind| kind == "investigation_stream_completed")
    );
    assert!(
        event_kinds
            .iter()
            .any(|kind| kind == "model_stream_completed")
    );
    assert!(event_kinds.iter().any(|kind| kind == "model_loop_started"));
    assert!(event_kinds.iter().any(|kind| kind == "model_loop_finished"));

    let summary = fs::read_to_string(out.join("running-summary.md"))?;
    assert!(summary.contains(
        "| `minimax` | `MiniMax-M3` | `openai-chat` | `ok` | `200` | `openai` | completed |"
    ));
    assert!(summary.contains("| `ub` | `minimax` | `MiniMax-M3` | `openai-chat` | `ok` |"));
    assert!(!summary.contains(dummy_key));
    Ok(())
}

#[test]
fn intelligent_ci_runs_advisory_proof_planner_lane_before_request_broker() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
    write_file(
        &repo.join("Cargo.toml"),
        r#"[package]
name = "planner-lane-fixture"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )?;
    write_file(
        &repo.join("src/lib.rs"),
        r#"pub fn value() -> usize {
    1
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
        r#"pub fn value() -> usize {
    2
}
"#,
    )?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "change value"])?;

    let planner_content = serde_json::json!({
        "summary": null,
        "observations": [
            {
                "claim": "Planner identified one cheap focused proof request.",
                "question": "proof-planner",
                "kind": "test-gap",
                "status": "open",
                "severity": "medium",
                "confidence": "high",
                "evidence": ["fake planner response"]
            }
        ],
        "candidate_findings": [],
        "summary_only_findings": [],
        "failed_objections": [],
        "proof_requests": [
            {
                "command": "cargo test --locked planner_requested_proof",
                "reason": "Focused planner request should route through the central broker.",
                "cost": "focused-test",
                "timeout_sec": 60,
                "required": false
            }
        ]
    })
    .to_string();
    let dummy_key = "dummy-minimax-key-for-proof-planner-lane";
    let (provider_url, provider) = spawn_fake_openai_provider_with_contents(vec![
        fake_openai_lane_content(),
        fake_openai_lane_content(),
        planner_content,
    ])?;
    let out = temp.path().join("packet");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    run_with_env(
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
            "--mode",
            "intelligent-ci",
            "--no-github-summary",
            "--model-mode",
            "auto",
            "--provider-policy",
            "minimax-only",
            "--minimax-provider-kind",
            "openai",
            "--lanes",
            "tests-red-green",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "2",
            "--model-timeout-sec",
            "10",
        ],
        &[
            ("UB_REVIEW_MINIMAX_API_KEY", dummy_key),
            ("UB_REVIEW_MINIMAX_API_URL", provider_url.as_str()),
        ],
    )?;
    let provider_requests = join_fake_provider(provider)?;
    assert_eq!(provider_requests.len(), 3);

    let planner_artifact_dir = out.join("review/model/proof-planner");
    for name in [
        "request.json",
        "response.json",
        "content.json",
        "stderr.txt",
    ] {
        let path = planner_artifact_dir.join(name);
        assert!(path.exists(), "missing {}", path.display());
        let text = fs::read_to_string(&path)?;
        assert!(
            !text.contains(dummy_key),
            "secret leaked to {}",
            path.display()
        );
    }

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    let model_lanes = review["model_lanes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("model_lanes missing"))?;
    let planner_lane = model_lanes
        .iter()
        .find(|lane| lane["lane"].as_str() == Some("proof-planner"))
        .ok_or_else(|| anyhow::anyhow!("proof-planner model lane missing"))?;
    assert_eq!(planner_lane["status"], "ok");
    assert_eq!(planner_lane["provider"], "minimax");
    assert_eq!(planner_lane["endpoint_kind"], "openai-chat");
    assert!(
        review["observations"]
            .as_array()
            .is_some_and(|observations| observations.iter().any(|observation| {
                observation["lane"].as_str() == Some("proof-planner")
                    && observation["kind"].as_str() == Some("test-gap")
            }))
    );

    let proof_requests: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_requests.json"))?)?;
    let planner_request = proof_requests
        .iter()
        .find(|request| request["lane"].as_str() == Some("proof-planner"))
        .ok_or_else(|| anyhow::anyhow!("planner proof request missing"))?;
    assert_eq!(
        planner_request["command"],
        "cargo test --locked planner_requested_proof"
    );
    assert_eq!(planner_request["cost"], "focused-test");
    let request_id = json_str_field(planner_request, "id")?;

    let planner_input: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_planner_input.json"))?)?;
    assert!(
        planner_input["proof_requests"]
            .as_array()
            .is_some_and(|requests| requests.iter().any(|request| {
                request["id"].as_str() == Some(request_id)
                    && request["lane"].as_str() == Some("proof-planner")
            }))
    );
    let planner_output: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_planner_output.json"))?)?;
    assert!(
        planner_output["proof_tasks"]
            .as_array()
            .is_some_and(|tasks| tasks.iter().any(|task| {
                task["request_ids"]
                    .as_array()
                    .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
            }))
    );

    let receipts: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
    assert!(receipts.iter().any(|receipt| {
        receipt["request_ids"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
            && receipt["result"].as_str() == Some("skipped_profile")
    }));
    Ok(())
}

#[test]
fn model_auto_run_overlaps_initial_diff_proof_with_model_lanes() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo)?;
    write_file(&repo.join("README.md"), "# scheduler overlap fixture\n")?;

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
    let dummy_key = "dummy-minimax-key-for-overlap-test";
    let (provider_url, provider) = spawn_fake_openai_provider(2)?;
    let out = temp.path().join("packet");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
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
            "--lanes",
            "tests-red-green",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "1",
            "--model-timeout-sec",
            "10",
            "--tools",
            "cargo-allow",
        ],
        &[
            ("PATH", path.as_str()),
            ("UB_REVIEW_MINIMAX_API_KEY", dummy_key),
            ("UB_REVIEW_MINIMAX_API_URL", provider_url.as_str()),
            ("FAKE_BUN_SLEEP_MS", "600"),
            ("FAKE_BUN_SLEEP_SECONDS", "1"),
        ],
    )?;
    let provider_requests = join_fake_provider(provider)?;
    assert_eq!(provider_requests.len(), 2);

    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    assert_eq!(metrics["proof_receipts"], 1);
    assert_eq!(metrics["models"]["model_lane_calls_attempted"], 1);
    assert!(
        metrics["run"]["investigation_proof_overlap_ms"]
            .as_u64()
            .is_some_and(|overlap| overlap > 0),
        "expected initial proof to overlap model lane execution"
    );
    assert!(
        metrics["run"]["local_proof_wall_ms"]
            .as_u64()
            .is_some_and(|wall| wall >= 600),
        "fake focused proof should contribute local proof wall time"
    );

    let scheduler: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/scheduler.json"))?)?;
    assert_eq!(
        scheduler["overlaps"]["investigation_proof_overlap_ms"],
        metrics["run"]["investigation_proof_overlap_ms"]
    );
    assert_eq!(
        scheduler["scheduler_roles"],
        metrics["run"]["scheduler_roles"]
    );
    let phases = scheduler["phases"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("scheduler phases missing"))?;
    let proof_phase = phases
        .iter()
        .find(|phase| phase["loop_id"] == "proof" && phase["stage"] == "initial-diff-broker")
        .ok_or_else(|| anyhow::anyhow!("initial diff proof phase missing"))?;
    let model_phase = phases
        .iter()
        .find(|phase| phase["loop_id"] == "model" && phase["stage"] == "primary")
        .ok_or_else(|| anyhow::anyhow!("primary model phase missing"))?;
    let proof_started = proof_phase["started_at_offset_ms"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("proof phase missing start"))?;
    let proof_finished = proof_phase["finished_at_offset_ms"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("proof phase missing finish"))?;
    let model_started = model_phase["started_at_offset_ms"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("model phase missing start"))?;
    let model_finished = model_phase["finished_at_offset_ms"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("model phase missing finish"))?;
    assert!(proof_started <= model_started);
    assert!(proof_finished >= model_started);
    assert!(model_finished > model_started);

    let receipts: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["result"], "discriminating");
    Ok(())
}

#[test]
fn model_auto_run_overlaps_seeded_required_proof_with_model_lanes() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
    write_file(
        &repo.join("Cargo.toml"),
        r#"[package]
name = "seeded-proof-overlap"
version = "0.1.0"
edition = "2024"

[lib]
path = "src/lib.rs"
"#,
    )?;
    write_file(&repo.join("src/lib.rs"), "pub fn value() -> u32 { 1 }\n")?;

    run(&repo, "git", &["init"])?;
    run(
        &repo,
        "git",
        &["config", "user.email", "ub-review@example.invalid"],
    )?;
    run(&repo, "git", &["config", "user.name", "UB Review Test"])?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "baseline"])?;

    write_file(&repo.join("src/lib.rs"), "pub fn value() -> u32 { 2 }\n")?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "change value"])?;

    let config = temp.path().join("ub-review.toml");
    write_file(
        &config,
        r#"review_profile = "bun-ub-v0"
profile = "gh-runner"

[repo]
kind = "rust"
ledger = ""
base = "HEAD~1"
head = "HEAD"

[[proof.required]]
id = "required-smoke"
languages = ["rust"]
diff_classes = ["source-general"]
command = "cargo test --locked required_proof_smoke"
reason = "Required Rust focused smoke for intelligent CI."
cost = "focused-test"
timeout_sec = 300
required = true
"#,
    )?;

    let fake_bin = temp.path().join("fake-bin");
    write_fake_cargo(&fake_bin)?;
    let path = prepend_to_path(&fake_bin)?;
    let dummy_key = "dummy-minimax-key-for-seeded-proof-overlap";
    let (provider_url, provider) = spawn_fake_openai_provider_with_delay(2, 500)?;
    let out = temp.path().join("packet");
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
            "--mode",
            "intelligent-ci",
            "--no-github-summary",
            "--model-mode",
            "auto",
            "--provider-policy",
            "minimax-only",
            "--minimax-provider-kind",
            "openai",
            "--lanes",
            "tests-red-green",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "1",
            "--model-timeout-sec",
            "10",
            "--tools",
            "cargo-allow",
        ],
        &[
            ("PATH", path.as_str()),
            ("UB_REVIEW_MINIMAX_API_KEY", dummy_key),
            ("UB_REVIEW_MINIMAX_API_URL", provider_url.as_str()),
            ("FAKE_CARGO_SLEEP_MS", "700"),
            ("FAKE_CARGO_SLEEP_SECONDS", "1"),
        ],
    )?;
    let provider_requests = join_fake_provider(provider)?;
    assert_eq!(provider_requests.len(), 2);

    let proof_requests: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_requests.json"))?)?;
    assert_eq!(proof_requests.len(), 1);
    let request_id = json_str_field(&proof_requests[0], "id")?;

    let receipts: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0]["kind"], "focused-red-green");
    assert!(receipts[0]["requested_by"].as_array().is_some_and(|lanes| {
        lanes
            .iter()
            .any(|lane| lane.as_str() == Some("intelligent-ci-policy"))
    }));
    assert!(
        receipts[0]["request_ids"]
            .as_array()
            .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
    );

    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    assert_eq!(metrics["models"]["model_lane_calls_attempted"], 1);
    assert!(
        metrics["run"]["investigation_proof_overlap_ms"]
            .as_u64()
            .is_some_and(|overlap| overlap > 0),
        "expected seeded required proof to overlap model lane execution"
    );
    assert!(
        metrics["run"]["local_proof_wall_ms"]
            .as_u64()
            .is_some_and(|wall| wall >= 1_000),
        "fake cargo proof should contribute local proof wall time"
    );

    let scheduler: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/scheduler.json"))?)?;
    let phases = scheduler["phases"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("scheduler phases missing"))?;
    let seeded_phase = phases
        .iter()
        .find(|phase| phase["loop_id"] == "proof" && phase["stage"] == "seeded-request-broker")
        .ok_or_else(|| anyhow::anyhow!("seeded proof phase missing"))?;
    let model_phase = phases
        .iter()
        .find(|phase| phase["loop_id"] == "model" && phase["stage"] == "primary")
        .ok_or_else(|| anyhow::anyhow!("primary model phase missing"))?;
    let seeded_started = seeded_phase["started_at_offset_ms"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("seeded proof phase missing start"))?;
    let seeded_finished = seeded_phase["finished_at_offset_ms"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("seeded proof phase missing finish"))?;
    let model_started = model_phase["started_at_offset_ms"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("model phase missing start"))?;
    let model_finished = model_phase["finished_at_offset_ms"]
        .as_u64()
        .ok_or_else(|| anyhow::anyhow!("model phase missing finish"))?;
    assert!(seeded_started <= model_finished);
    assert!(seeded_finished >= model_started);
    Ok(())
}

#[test]
fn model_auto_run_preserves_contentful_non_json_lane_output_as_degraded() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
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

    let raw_lane_content = "EncodedSlice route excerpt survived as text";
    let (provider_url, provider) = spawn_fake_openai_provider_with_contents(vec![
        fake_openai_lane_content(),
        raw_lane_content.to_owned(),
    ])?;
    let out = temp.path().join("packet");
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let minimax_key_env = ["UB", "_REVIEW_MINIMAX_API_KEY"].concat();
    run_with_env(
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
            (minimax_key_env.as_str(), "dummy-minimax-key"),
            ("UB_REVIEW_MINIMAX_API_URL", provider_url.as_str()),
        ],
    )?;
    let provider_requests = join_fake_provider(provider)?;
    assert_eq!(provider_requests.len(), 2);

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    let model_lanes = review["model_lanes"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("model_lanes missing"))?;
    let ub_lane = model_lanes
        .iter()
        .find(|lane| lane["lane"].as_str() == Some("ub"))
        .ok_or_else(|| anyhow::anyhow!("ub model lane missing"))?;
    assert_eq!(ub_lane["status"], "degraded");
    assert_eq!(
        ub_lane["reason"],
        "contentful lane output was preserved as degraded evidence"
    );
    assert_eq!(ub_lane["http_status"], 200);
    assert_eq!(ub_lane["response_shape"], "openai");
    // The degraded-but-contentful lane is not an evidence gap; the five
    // lanes skipped by the one-call budget are, and must be recorded as
    // missing model evidence rather than silently dropped.
    let model_evidence = review["missing_or_failed_model_evidence"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing_or_failed_model_evidence missing"))?;
    assert!(
        model_evidence
            .iter()
            .all(|issue| issue["lane"].as_str() != Some("ub")),
        "the degraded ub lane must not be a model evidence gap: {model_evidence:?}"
    );
    assert_eq!(
        model_evidence.len(),
        5,
        "budget-skipped lanes are missing model evidence: {model_evidence:?}"
    );
    assert!(
        model_evidence.iter().all(|issue| issue["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("model call budget"))),
        "every gap is the budget skip, not a failure: {model_evidence:?}"
    );
    assert!(
        review["observations"]
            .as_array()
            .is_some_and(|observations| observations.iter().any(|observation| {
                observation["lane"].as_str() == Some("ub")
                    && observation["kind"].as_str() == Some("missing-evidence")
                    && observation["dedupe_key"].as_str() == Some("lane-output-malformed-content")
                    && observation["claim"]
                        .as_str()
                        .is_some_and(|claim| claim.contains(raw_lane_content))
                    && observation["evidence"].as_array().is_some_and(|evidence| {
                        evidence.iter().any(|item| {
                            item.as_str().is_some_and(|text| {
                                let normalized = text.replace('\\', "/");
                                normalized.contains("Raw content artifact:")
                                    && normalized.contains("review/model/ub/content.json")
                            })
                        })
                    })
            }))
    );

    let lane_content = fs::read_to_string(out.join("review/model/ub/content.json"))?;
    assert_eq!(lane_content, raw_lane_content);

    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    assert_eq!(metrics["models"]["model_lane_status_counts"]["degraded"], 1);
    assert_eq!(metrics["models"]["model_lane_calls_attempted"], 1);
    // Five lanes were budget-skipped by --max-model-calls 1; each is missing
    // model evidence in the metrics, matching review.json above.
    assert_eq!(metrics["missing_or_failed_model_evidence"], 5);
    assert!(!out.join("review/github-review.json").exists());
    assert!(out.join("review/github-review-skip.json").exists());
    let pr_body = fs::read_to_string(out.join("review/review.md"))?;
    assert!(!pr_body.contains(raw_lane_content));
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
fn post_receipt_rejects_boilerplate_review_body_before_payload_write() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let review_json = temp.path().join("github-review.json");
    let out = temp.path().join("post");
    let token = "test-token-redacted";
    fs::write(
        &review_json,
        serde_json::to_vec_pretty(&serde_json::json!({
            "event": "COMMENT",
            "body": "## Model lanes\n\n- Lane: `ub`\n  Provider: `minimax`\n  Model: `MiniMax-M3`\n  Status: `ok` - completed",
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
    assert_eq!(post_error["would_post"], false);
    assert_eq!(post_error["payload_written"], false);
    assert!(
        post_error["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("successful lane table"))
    );
    assert!(!post_error_text.contains(token));
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

#[test]
fn gate_check_exit_codes_follow_fail_on_gate_resolution() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let pass = temp.path().join("gate-pass.json");
    write_file(
        &pass,
        r#"{"schema":"ub-review.gate_outcome.v1","conclusion":"pass","reasons":[]}"#,
    )?;
    let fail = temp.path().join("gate-fail.json");
    write_file(
        &fail,
        r#"{
  "schema": "ub-review.gate_outcome.v1",
  "conclusion": "fail",
  "reasons": [
    {"kind": "required-proof", "id": "cargo-check", "detail": "exit 101", "receipt": "review/proof_receipts.json#x"},
    {"kind": "tool-gate", "id": "ripr", "detail": "threshold exceeded", "receipt": "review/tool-gate-outcomes.json#ripr"}
  ]
}"#,
    )?;
    let missing = temp.path().join("absent/gate_outcome.json");

    // Exit 0: passing outcome under enforcement.
    run(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&pass)?,
            "--fail-on-gate",
            "true",
            "--mode",
            "review-byok",
        ],
    )?;
    // Exit 0: failing outcome with enforcement off (explicit and auto).
    run(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&fail)?,
            "--fail-on-gate",
            "false",
            "--mode",
            "intelligent-ci",
        ],
    )?;
    run(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&fail)?,
            "--fail-on-gate",
            "auto",
            "--mode",
            "review-byok",
        ],
    )?;
    run(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&missing)?,
            "--fail-on-gate",
            "auto",
            "--mode",
            "review-byok",
        ],
    )?;
    // Non-zero: failing outcome with enforcement on names reason ids + path.
    let enforced = run_expect_failure(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&fail)?,
            "--fail-on-gate",
            "true",
            "--mode",
            "review-byok",
        ],
    )?;
    assert!(
        enforced.contains("cargo-check, ripr"),
        "gate-check output must list blocking reason ids: {enforced}"
    );
    assert!(
        enforced.contains("gate-fail.json"),
        "gate-check output must name the artifact path: {enforced}"
    );
    // Non-zero: auto resolves to enforcement for intelligent-ci.
    run_expect_failure(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&fail)?,
            "--fail-on-gate",
            "auto",
            "--mode",
            "intelligent-ci",
        ],
    )?;
    // Non-zero: enforcement on with a missing artifact is a hard error.
    let missing_enforced = run_expect_failure(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&missing)?,
            "--fail-on-gate",
            "true",
            "--mode",
            "review-byok",
        ],
    )?;
    assert!(
        missing_enforced.contains("is missing"),
        "{missing_enforced}"
    );
    // Non-zero: enforcement fails closed on any conclusion that is not
    // exactly `pass` or `fail` (missing key, null, casing drift, other
    // strings), naming the unexpected value and the artifact path.
    let weird = temp.path().join("gate-weird.json");
    for conclusion_json in [r#""error""#, r#""Fail""#, "null"] {
        write_file(
            &weird,
            &format!(
                r#"{{"schema":"ub-review.gate_outcome.v1","conclusion":{conclusion_json},"reasons":[]}}"#
            ),
        )?;
        let failed_closed = run_expect_failure(
            temp.path(),
            bin,
            &[
                "gate-check",
                "--gate-outcome",
                path_str(&weird)?,
                "--fail-on-gate",
                "true",
                "--mode",
                "review-byok",
            ],
        )?;
        assert!(
            failed_closed.contains("unrecognized conclusion"),
            "conclusion {conclusion_json}: {failed_closed}"
        );
        assert!(
            failed_closed.contains("gate-weird.json"),
            "conclusion {conclusion_json}: {failed_closed}"
        );
        // Exit 0: enforcement off tolerates the same artifact.
        run(
            temp.path(),
            bin,
            &[
                "gate-check",
                "--gate-outcome",
                path_str(&weird)?,
                "--fail-on-gate",
                "false",
                "--mode",
                "intelligent-ci",
            ],
        )?;
    }
    // Non-zero: a schema mismatch fails closed under enforcement even when
    // the conclusion claims `pass`.
    let wrong_schema = temp.path().join("gate-wrong-schema.json");
    write_file(
        &wrong_schema,
        r#"{"schema":"ub-review.gate_outcome.v2","conclusion":"pass","reasons":[]}"#,
    )?;
    let schema_enforced = run_expect_failure(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&wrong_schema)?,
            "--fail-on-gate",
            "true",
            "--mode",
            "review-byok",
        ],
    )?;
    assert!(
        schema_enforced.contains("ub-review.gate_outcome.v2"),
        "{schema_enforced}"
    );
    // Exit 0: the schema drift is informational with enforcement off.
    run(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&wrong_schema)?,
            "--fail-on-gate",
            "false",
            "--mode",
            "intelligent-ci",
        ],
    )?;
    // Non-zero: clap rejects unknown fail-on-gate values, replacing the old
    // bash `*)` case.
    run_expect_failure(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&pass)?,
            "--fail-on-gate",
            "sometimes",
            "--mode",
            "review-byok",
        ],
    )?;
    Ok(())
}

#[test]
fn run_records_policy_parse_error_as_receipted_gate_failure() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;
    let config = temp.path().join("bad-gate.toml");
    write_file(
        &config,
        r#"profile = "gh-runner"

[gate]
target_minutes = 45

[tools.ripr.gate]
max_new = 0

[tools.unsafe-review.gate]
max_new_unsuppressed = 0
"#,
    )?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let out = temp.path().join("packet");
    // review-byok with fail-on-gate auto (off): the run exits zero but still
    // records the malformed policy section as a blocking gate reason.
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
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
    )?;
    let gate: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/gate_outcome.json"))?)?;
    assert_eq!(gate["conclusion"], "fail");
    assert_eq!(gate["reasons"][0]["kind"], "policy");
    assert_eq!(gate["reasons"][0]["id"], "tools.ripr.gate");
    assert_eq!(gate["reasons"][0]["receipt"], "effective-config.json");
    assert!(
        gate["reasons"][0]["detail"]
            .as_str()
            .is_some_and(|detail| detail.contains("max_new")),
        "policy detail must name the parse error: {}",
        gate["reasons"][0]["detail"]
    );
    let effective: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("effective-config.json"))?)?;
    assert_eq!(effective["policy_errors"][0]["section"], "tools.ripr.gate");
    // Valid siblings survive the one malformed [tools.ripr.gate] table: the
    // [gate] table and the well-formed sibling tool gate are still applied.
    assert_eq!(effective["gate"]["target_minutes"], 45);
    assert_eq!(
        effective["tools"]["unsafe-review"]["gate"]["max_new_unsuppressed"],
        0
    );

    // The recorded failure becomes a red check once enforcement is on.
    let gate_outcome_path = out.join("review/gate_outcome.json");
    let enforced = run_expect_failure(
        temp.path(),
        bin,
        &[
            "gate-check",
            "--gate-outcome",
            path_str(&gate_outcome_path)?,
            "--fail-on-gate",
            "true",
            "--mode",
            "review-byok",
        ],
    )?;
    assert!(enforced.contains("tools.ripr.gate"), "{enforced}");

    // `run --fail-on-gate true` exits non-zero after writing all artifacts.
    let out_enforced = temp.path().join("packet-enforced");
    let run_failure = run_expect_failure(
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
            path_str(&out_enforced)?,
            "--fail-on-gate",
            "true",
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
    )?;
    assert!(
        run_failure.contains("gate conclusion is `fail`"),
        "{run_failure}"
    );
    assert!(out_enforced.join("review/gate_outcome.json").exists());
    Ok(())
}

/// Driver for `isolated_command_scrubs_ambient_profile_env_from_child_runs`.
/// Runs only when re-invoked by that test with `UB_SCRUB_*` coordinates; it
/// spawns `ub-review plan --write` through the scrubbing `isolated_command`
/// helper while ambient `UB_REVIEW_*` variables are present in this process.
#[test]
#[ignore = "driver: invoked as a subprocess by the env-scrub test"]
fn env_scrub_driver_invokes_isolated_plan() -> Result<()> {
    let (Ok(repo), Ok(out)) = (
        std::env::var("UB_SCRUB_REPO"),
        std::env::var("UB_SCRUB_OUT"),
    ) else {
        return Ok(());
    };
    let repo = Path::new(&repo);
    let bin = env!("CARGO_BIN_EXE_ub-review");
    run(
        repo,
        bin,
        &[
            "plan",
            "--write",
            "--root",
            path_str(repo)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            &out,
        ],
    )
}

#[test]
fn isolated_command_scrubs_ambient_profile_env_from_child_runs() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;
    let out = temp.path().join("packet");
    // Re-invoke this test binary so the gh-runner-full overrides live in the
    // *parent* process environment around an isolated_command-spawned run,
    // exactly like the dogfood gate's GitHub Actions step exports them.
    let driver = std::env::current_exe()?;
    let output = Command::new(&driver)
        .arg("env_scrub_driver_invokes_isolated_plan")
        .arg("--exact")
        .arg("--ignored")
        .arg("--nocapture")
        .env("UB_REVIEW_PROFILE", "gh-runner-full")
        .env("UB_REVIEW_RUNTIME_PROFILE", "gh-runner-full")
        .env("UB_SCRUB_REPO", &repo)
        .env("UB_SCRUB_OUT", &out)
        .output()?;
    assert!(
        output.status.success(),
        "scrub driver failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let resolved_profile: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-profile.json"))?)?;
    // Pin the invariant, not the symptom: ambient UB_REVIEW_* overrides in
    // the parent environment must never leak into isolated_command children,
    // so the child resolves gh-runner defaults rather than gh-runner-full.
    assert_eq!(
        resolved_profile["selected_profile"], "gh-runner",
        "ambient UB_REVIEW_PROFILE leaked into an isolated_command child"
    );
    assert_eq!(
        resolved_profile["selected_runtime_profile"], "gh-runner",
        "ambient UB_REVIEW_RUNTIME_PROFILE leaked into an isolated_command child"
    );
    Ok(())
}

#[test]
fn synchronize_pass_honors_profile_post_review_on_policy() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo)?;
    init_minimal_repo(&repo)?;
    let bin = env!("CARGO_BIN_EXE_ub-review");

    // Two-pass consumer default: synchronize is not in [gate].post_review_on,
    // so a posting=review synchronize pass must skip with a receipt naming
    // the pass policy instead of a reviewer-value sentence.
    let two_pass_out = temp.path().join("two-pass");
    let bun_config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
    run(
        temp.path(),
        bin,
        &[
            "run",
            "--dry-run",
            "--config",
            path_str(&bun_config)?,
            "--root",
            path_str(&repo)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            path_str(&two_pass_out)?,
            "--run-pass",
            "synchronize",
            "--posting",
            "review",
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
    )?;
    assert!(!two_pass_out.join("review/github-review.json").exists());
    let skip: serde_json::Value = serde_json::from_slice(&fs::read(
        two_pass_out.join("review/github-review-skip.json"),
    )?)?;
    assert_eq!(skip["status"], "skipped");
    assert_eq!(skip["review_payload_status"], "skipped_pass_policy");
    assert_eq!(skip["run_pass"], "synchronize");
    let reason = skip["reason"].as_str().unwrap_or_default();
    assert!(
        reason.contains("pass `synchronize` is not in [gate].post_review_on"),
        "skip reason should name the pass policy: {reason}"
    );

    // The repo's own gate policy keeps synchronize gate-only: the gate still
    // runs on every head SHA, but a posting=review synchronize pass must skip
    // with the pass-policy receipt, exactly like the consumer default.
    let self_pass_out = temp.path().join("self-pass");
    let self_config = Path::new(env!("CARGO_MANIFEST_DIR")).join(".ub-review.toml");
    run(
        temp.path(),
        bin,
        &[
            "run",
            "--dry-run",
            "--config",
            path_str(&self_config)?,
            "--root",
            path_str(&repo)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            path_str(&self_pass_out)?,
            "--run-pass",
            "synchronize",
            "--posting",
            "review",
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
    )?;
    let self_skip: serde_json::Value = serde_json::from_slice(&fs::read(
        self_pass_out.join("review/github-review-skip.json"),
    )?)?;
    assert_eq!(self_skip["run_pass"], "synchronize");
    assert_eq!(
        self_skip["review_payload_status"], "skipped_pass_policy",
        "repo gate policy keeps synchronize gate-only"
    );

    // A policy that lists synchronize must still be admitted by the pass
    // gate; this dry run has no reviewer-value content, so the only
    // acceptable skip is the empty-smoke one - never the pass policy.
    let every_pass_out = temp.path().join("every-pass");
    let every_pass_config = temp.path().join("every-pass.toml");
    fs::write(
        &every_pass_config,
        r#"[gate]
post_review_on = ["opened", "reopened", "ready_for_review", "synchronize"]
"#,
    )?;
    run(
        temp.path(),
        bin,
        &[
            "run",
            "--dry-run",
            "--config",
            path_str(&every_pass_config)?,
            "--root",
            path_str(&repo)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            path_str(&every_pass_out)?,
            "--run-pass",
            "synchronize",
            "--posting",
            "review",
            "--model-mode",
            "off",
            "--no-github-summary",
        ],
    )?;
    let every_pass_skip: serde_json::Value = serde_json::from_slice(&fs::read(
        every_pass_out.join("review/github-review-skip.json"),
    )?)?;
    assert_eq!(every_pass_skip["run_pass"], "synchronize");
    assert_ne!(
        every_pass_skip["review_payload_status"], "skipped_pass_policy",
        "a policy listing synchronize in [gate].post_review_on must admit the pass"
    );
    let every_pass_reason = every_pass_skip["reason"].as_str().unwrap_or_default();
    assert!(
        !every_pass_reason.contains("[gate].post_review_on"),
        "admitted pass must not skip for pass policy: {every_pass_reason}"
    );
    Ok(())
}

fn init_minimal_repo(repo: &Path) -> Result<()> {
    write_file(
        &repo.join("src/lib.rs"),
        "pub fn answer() -> usize {\n    41\n}\n",
    )?;
    run(repo, "git", &["init"])?;
    run(
        repo,
        "git",
        &["config", "user.email", "ub-review@example.invalid"],
    )?;
    run(repo, "git", &["config", "user.name", "UB Review Test"])?;
    run(repo, "git", &["add", "."])?;
    run(repo, "git", &["commit", "-m", "baseline"])?;
    write_file(
        &repo.join("src/lib.rs"),
        "pub fn answer() -> usize {\n    42\n}\n",
    )?;
    run(repo, "git", &["add", "."])?;
    run(repo, "git", &["commit", "-m", "change"])?;
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
        let cache_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ub-review-test-fake-bun");
        let source = cache_dir.join("fake_bun.rs");
        let cached_exe = cache_dir.join("bun.exe");
        let needs_compile = !cached_exe.exists()
            || fs::read_to_string(&source).unwrap_or_default() != FAKE_BUN_SOURCE;
        if needs_compile {
            fs::create_dir_all(&cache_dir)?;
            write_file(&source, FAKE_BUN_SOURCE)?;
            run(
                &cache_dir,
                "rustc",
                &[path_str(&source)?, "-o", path_str(&cached_exe)?],
            )?;
        }
        fs::copy(cached_exe, dir.join("bun.exe"))?;
    }
    #[cfg(not(windows))]
    {
        let script = dir.join("bun");
        write_file(
            &script,
            r#"#!/bin/sh
echo "fake bun $*"
if [ -n "$FAKE_BUN_SLEEP_SECONDS" ]; then
  sleep "$FAKE_BUN_SLEEP_SECONDS"
fi
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

fn write_fake_cargo(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(windows)]
    {
        let cache_dir =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ub-review-test-fake-cargo");
        let source = cache_dir.join("fake_cargo.rs");
        let cached_exe = cache_dir.join("cargo.exe");
        let needs_compile = !cached_exe.exists()
            || fs::read_to_string(&source).unwrap_or_default() != FAKE_CARGO_SOURCE;
        if needs_compile {
            fs::create_dir_all(&cache_dir)?;
            write_file(&source, FAKE_CARGO_SOURCE)?;
            run(
                &cache_dir,
                "rustc",
                &[path_str(&source)?, "-o", path_str(&cached_exe)?],
            )?;
        }
        fs::copy(cached_exe, dir.join("cargo.exe"))?;
    }
    #[cfg(not(windows))]
    {
        let script = dir.join("cargo");
        write_file(
            &script,
            r#"#!/bin/sh
echo "fake cargo $*"
if [ -n "$FAKE_CARGO_SLEEP_SECONDS" ]; then
  sleep "$FAKE_CARGO_SLEEP_SECONDS"
fi
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

#[cfg(windows)]
const FAKE_BUN_SOURCE: &str = r#"use std::{env, fs, process};

fn main() {
    let cwd = env::current_dir().expect("current dir");
    let args = env::args().skip(1).collect::<Vec<_>>();
    println!("fake bun {}", args.join(" "));
    if let Ok(value) = env::var("FAKE_BUN_SLEEP_MS") {
        if let Ok(ms) = value.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
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
"#;

#[cfg(windows)]
const FAKE_CARGO_SOURCE: &str = r#"use std::env;

fn main() {
    let args = env::args().skip(1).collect::<Vec<_>>();
    println!("fake cargo {}", args.join(" "));
    if let Ok(value) = env::var("FAKE_CARGO_SLEEP_MS") {
        if let Ok(ms) = value.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}
"#;

fn write_fake_core_review_tools(dir: &Path, tokmd_version: &str) -> Result<()> {
    fs::create_dir_all(dir)?;
    let tools = [
        "tokmd",
        "cargo-allow",
        "ripr",
        "unsafe-review",
        "ast-grep",
        "actionlint",
    ];

    #[cfg(windows)]
    {
        let source = dir.join("fake_review_tool.rs");
        write_file(
            &source,
            &format!(
                r#"use std::{{env, path::Path}};

const TOKMD_VERSION: &str = {tokmd_version:?};

fn main() {{
    let executable = env::args().next().unwrap_or_default();
    let name = Path::new(&executable)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("review-tool");
    let version = if name == "tokmd" {{ TOKMD_VERSION }} else {{ "0.0.0" }};
    println!("{{name}} {{version}}");
}}
"#
            ),
        )?;
        let exe = dir.join("fake_review_tool.exe");
        run(dir, "rustc", &[path_str(&source)?, "-o", path_str(&exe)?])?;
        for tool in tools {
            fs::copy(&exe, dir.join(format!("{tool}.exe")))?;
        }
    }

    #[cfg(not(windows))]
    {
        for tool in tools {
            let version = if tool == "tokmd" {
                tokmd_version
            } else {
                "0.0.0"
            };
            let script = dir.join(tool);
            write_file(&script, &format!("#!/bin/sh\necho \"{tool} {version}\"\n"))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;

                let mut permissions = fs::metadata(&script)?.permissions();
                permissions.set_mode(0o755);
                fs::set_permissions(&script, permissions)?;
            }
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
    spawn_fake_openai_provider_with_contents(
        (0..expected_requests)
            .map(|_| fake_openai_lane_content())
            .collect(),
    )
}

fn spawn_fake_openai_provider_with_delay(
    expected_requests: usize,
    delay_ms: u64,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    spawn_fake_openai_provider_with_contents_and_delay(
        (0..expected_requests)
            .map(|_| fake_openai_lane_content())
            .collect(),
        Duration::from_millis(delay_ms),
    )
}

fn spawn_fake_openai_provider_with_contents(
    contents: Vec<String>,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    spawn_fake_openai_provider_with_contents_and_delay(contents, Duration::ZERO)
}

fn spawn_fake_openai_provider_with_contents_and_delay(
    contents: Vec<String>,
    response_delay: Duration,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let handle = thread::spawn(move || -> Result<Vec<String>> {
        let expected_requests = contents.len();
        let mut deadline = Instant::now() + Duration::from_secs(120);
        let mut requests = Vec::new();
        while requests.len() < expected_requests {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    let content = contents
                        .get(requests.len())
                        .ok_or_else(|| anyhow::anyhow!("fake provider response missing"))?;
                    requests.push(handle_fake_openai_request(stream, content, response_delay)?);
                    deadline = Instant::now() + Duration::from_secs(120);
                }
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

fn cli_subprocess_test_lock() -> Result<MutexGuard<'static, ()>> {
    static CLI_SUBPROCESS_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // Recover a poisoned lock instead of erroring: one failing test must
    // produce one failure receipt, not cascade into every later subprocess
    // test in the suite.
    Ok(CLI_SUBPROCESS_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

fn fake_openai_lane_content() -> String {
    serde_json::json!({
        "summary": "fake provider ok",
        "inline_comments": [],
        "summary_only_findings": []
    })
    .to_string()
}

fn handle_fake_openai_request(
    mut stream: TcpStream,
    content: &str,
    response_delay: Duration,
) -> Result<String> {
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
    if !response_delay.is_zero() {
        thread::sleep(response_delay);
    }
    let response_body = serde_json::to_vec(&serde_json::json!({
        "choices": [
            {
                "message": {
                    "content": content
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

/// Builds a child command with every ambient `UB_REVIEW_*` variable scrubbed.
///
/// When the dogfood gate runs this suite, the surrounding GitHub Actions step
/// exports `UB_REVIEW_PROFILE`, `UB_REVIEW_RUNTIME_PROFILE`,
/// `UB_REVIEW_TOOL_BUNDLE`, and friends. The spawned `ub-review` binary picks
/// those up through clap `env = "UB_REVIEW_..."` fallbacks, so nested test
/// runs silently resolve a gh-runner profile and assertions about default
/// profile output fail only inside the gate. Scrubbing the prefix first keeps
/// tests hermetic; explicit per-test envs are applied afterwards and still
/// win.
fn isolated_command(program: &str, cwd: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(cwd);
    for (name, _) in std::env::vars_os() {
        if name.to_string_lossy().starts_with("UB_REVIEW_") {
            command.env_remove(&name);
        }
    }
    command
}

fn run(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
    let output = isolated_command(program, cwd).args(args).output()?;
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
    let mut command = isolated_command(program, cwd);
    command.args(args);
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

fn run_capture_with_env(
    cwd: &Path,
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<String> {
    let mut command = isolated_command(program, cwd);
    command.args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output()?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        return Ok(combined);
    }
    bail!("{program} {args:?} failed\n{combined}");
}

fn run_expect_failure(cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = isolated_command(program, cwd).args(args).output()?;
    if !output.status.success() {
        return Ok(format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    bail!("{program} {args:?} unexpectedly succeeded");
}

fn run_expect_failure_with_env(
    cwd: &Path,
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<String> {
    let mut command = isolated_command(program, cwd);
    command.args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output()?;
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

fn read_json(path: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

fn tool_entry<'a>(artifact: &'a serde_json::Value, tool_id: &str) -> Result<&'a serde_json::Value> {
    json_array_field(artifact, "tools")?
        .iter()
        .find(|tool| tool["id"] == tool_id)
        .ok_or_else(|| anyhow::anyhow!("{tool_id} tool entry missing"))
}

fn tool_gate_outcome<'a>(
    artifact: &'a serde_json::Value,
    tool_id: &str,
) -> Result<&'a serde_json::Value> {
    json_array_field(artifact, "outcomes")?
        .iter()
        .find(|outcome| outcome["tool"] == tool_id)
        .ok_or_else(|| anyhow::anyhow!("{tool_id} tool gate outcome missing"))
}

fn assert_cargo_allow_foreign_skip_artifacts(out: &Path) -> Result<()> {
    let resolved_tools = read_json(&out.join("resolved-tools.json"))?;
    let review_resolved_tools = read_json(&out.join("review/resolved-tools.json"))?;
    assert_eq!(resolved_tools, review_resolved_tools);
    let cargo_allow = tool_entry(&resolved_tools, "cargo-allow")?;
    assert_eq!(cargo_allow["planned_run"], serde_json::json!(false));
    assert_eq!(cargo_allow["plan_reason"], CARGO_ALLOW_FOREIGN_REASON);

    let sensor_status = read_json(&out.join("sensors/cargo-allow/ub-review-sensor-status.json"))?;
    assert_eq!(sensor_status["sensor"], "cargo-allow");
    assert_eq!(sensor_status["status"], "skipped");
    assert_eq!(sensor_status["reason"], CARGO_ALLOW_FOREIGN_REASON);

    let tool_status = read_json(&out.join("tool-status.json"))?;
    let review_tool_status = read_json(&out.join("review/tool-status.json"))?;
    assert_eq!(tool_status, review_tool_status);
    let cargo_allow_status = tool_entry(&tool_status, "cargo-allow")?;
    assert_eq!(cargo_allow_status["planned_run"], serde_json::json!(false));
    assert_eq!(cargo_allow_status["status"], "skipped");
    assert_eq!(cargo_allow_status["reason"], CARGO_ALLOW_FOREIGN_REASON);

    let tool_gate_outcomes = read_json(&out.join("tool-gate-outcomes.json"))?;
    let review_tool_gate_outcomes = read_json(&out.join("review/tool-gate-outcomes.json"))?;
    assert_eq!(tool_gate_outcomes, review_tool_gate_outcomes);
    let cargo_allow_outcome = tool_gate_outcome(&tool_gate_outcomes, "cargo-allow")?;
    assert_eq!(cargo_allow_outcome["planned_run"], serde_json::json!(false));
    assert_eq!(cargo_allow_outcome["sensor_status"], "skipped");
    assert_eq!(
        cargo_allow_outcome["sensor_reason"],
        CARGO_ALLOW_FOREIGN_REASON
    );
    assert_eq!(cargo_allow_outcome["outcome"], "not_evaluated");
    assert!(
        cargo_allow_outcome["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(CARGO_ALLOW_FOREIGN_REASON)),
        "tool gate reason should preserve linked cargo-allow skip reason: {cargo_allow_outcome}"
    );
    Ok(())
}

fn event_kinds(path: &Path) -> Result<Vec<String>> {
    let events = event_records(path)?;
    Ok(events
        .iter()
        .filter_map(|event| event["kind"].as_str().map(str::to_owned))
        .collect())
}

fn event_records(path: &Path) -> Result<Vec<serde_json::Value>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut kinds = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(&line)?;
        assert!(event["ts"].as_str().is_some(), "event missing ts: {event}");
        assert!(
            event["payload"].is_object(),
            "event missing payload object: {event}"
        );
        let kind = event["kind"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("event missing kind: {event}"))?;
        assert!(!kind.is_empty(), "event kind is empty: {event}");
        kinds.push(event);
    }
    Ok(kinds)
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
