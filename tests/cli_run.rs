//! Run and proof-focused CLI integration tests.

use std::fs;
use std::path::Path;

use anyhow::Result;

#[path = "common/cli_run.rs"]
mod common;
use common::*;

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
    let prior_resolved = temp.path().join("prior-resolved-candidates.json");
    write_file(
        &prior_resolved,
        &serde_json::to_string_pretty(&serde_json::json!([
            {
                "schema": "ub-review.resolved_candidate.v1",
                "candidate_id": "candidate-0001-deadbeef1234",
                "lane": "tests-oracle",
                "source": "summary-only-finding",
                "original_status": "summary-only",
                "original_disposition": "summary-only",
                "resolved_status": "resolved",
                "resolved_disposition": "dropped",
                "resolution_source": "orchestrator-follow-up",
                "source_artifacts": [
                    "review/candidates.json",
                    "review/follow_up_results.json",
                    "review/follow_up_outputs.json"
                ],
                "reason": "prior follow-up found this below materiality",
                "follow_up_task_ids": ["follow-prior"],
                "follow_up_stages": ["tertiary"],
                "follow_up_statuses": ["ok"],
                "evidence": ["Prior pass dropped the same candidate surface."]
            }
        ]))?,
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
            "--prior-resolved-candidates",
            path_str(&prior_resolved)?,
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
    let copied_prior_resolved_path = out.join("review/prior_resolved_candidates.json");
    assert!(
        copied_prior_resolved_path.is_file(),
        "run should copy the configured prior resolved-candidates receipt"
    );
    let shared_context = fs::read_to_string(out.join("review/shared_context.md"))?;
    assert!(shared_context.contains("## PR Thread Context"));
    assert!(shared_context.contains("### Prior Review Thread"));
    assert!(shared_context.contains("ASAN bad-free receipt"));
    assert!(shared_context.contains("[truncated]"));
    assert!(!shared_context.contains("tail should be truncated"));
    let mut lane_packets = fs::read_dir(out.join("lanes"))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("md"))
        .collect::<Vec<_>>();
    lane_packets.sort();
    let lane_packet_path = lane_packets
        .first()
        .ok_or_else(|| anyhow::anyhow!("expected at least one lane packet"))?;
    let lane_packet = fs::read_to_string(lane_packet_path)?;
    assert!(lane_packet.contains("## Seeded PR Thread Context"));
    assert!(lane_packet.contains("review/pr_thread_context.json"));
    assert!(lane_packet.contains("### Prior Review Thread"));
    assert!(lane_packet.contains("ASAN bad-free receipt"));
    assert!(lane_packet.contains("[truncated]"));
    assert!(!lane_packet.contains("tail should be truncated"));
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
