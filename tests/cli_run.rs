//! Run and proof-focused CLI integration tests.

use std::fs;
use std::path::Path;

use anyhow::Result;

#[path = "common/cli_run.rs"]
mod common;
use common::*;

#[test]
fn summary_reports_unavailable_review_receipt() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;
    let config = temp.path().join("config.toml");
    write_file(&config, "[review]\ngithub_summary = false\n")?;
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
            "--model-mode",
            "off",
            "--posting",
            "artifact-only",
            "--no-github-summary",
        ],
    )?;
    let receipt_path = out.join("review/review.json");
    let receipt = fs::read(&receipt_path)?;
    let summary_path = out.join("running-summary.md");
    let original_summary = fs::read_to_string(&summary_path)?;
    let summary_args = ["summary", "--run-dir", path_str(&out)?];
    run(temp.path(), bin, &summary_args)?;
    assert_eq!(fs::read_to_string(&summary_path)?, original_summary);
    assert!(original_summary.contains("- Model mode: `off`"));
    assert!(original_summary.contains("## Model lane status"));
    assert!(!original_summary.contains("Review evidence is incomplete"));
    let shared_context = fs::read_to_string(out.join("review/shared_context.md"))?;
    assert!(!shared_context.contains("Review evidence is incomplete"));
    assert!(!shared_context.contains("Restore or regenerate"));

    let mut failures = Vec::new();
    for (case, damaged, diagnostic) in [
        ("missing", None, "is missing or unreadable"),
        (
            "truncated",
            Some(&receipt[..receipt.len() / 2]),
            "is invalid",
        ),
        ("invalid-json", Some(b"not JSON".as_slice()), "is invalid"),
        (
            "wrong-field-type",
            Some(b"{\"model_lanes\":false}".as_slice()),
            "is invalid",
        ),
        ("invalid-utf8", Some(b"\xff".as_slice()), "is invalid"),
    ] {
        if let Some(bytes) = damaged {
            fs::write(&receipt_path, bytes)?;
        } else {
            fs::remove_file(&receipt_path)?;
        }
        run(temp.path(), bin, &summary_args)?;
        let summary = fs::read_to_string(&summary_path)?;
        if !summary.contains("Review evidence is incomplete")
            || !summary.contains("`review/review.json`")
            || !summary.contains(diagnostic)
            || !summary.contains("Restore or regenerate")
            || !summary.contains("ub-review summary --run-dir <RUN_DIR>")
            || summary.contains("No planned model evidence is currently missing or failed.")
        {
            failures.push(format!(
                "{case}: unavailable review receipt was not actionable"
            ));
        }
        // Recovery must reproduce the intact packet, not retain an old warning.
        fs::write(&receipt_path, &receipt)?;
        run(temp.path(), bin, &summary_args)?;
        assert_eq!(
            fs::read_to_string(&summary_path)?,
            original_summary,
            "{case}"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[test]
fn trusted_diff_run_uses_base_root_and_explicit_objects() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
    fs::create_dir_all(repo.join("scripts"))?;
    write_file(
        &repo.join("src/lib.rs"),
        "pub fn answer() -> usize { 41 }\n",
    )?;
    write_file(&repo.join(".ub-review.toml"), "review_profile = \"safe\"\n")?;
    write_file(&repo.join("scripts/reviewer.sh"), "echo safe\n")?;
    run(&repo, "git", &["init"])?;
    run(
        &repo,
        "git",
        &["config", "user.email", "ub-review@example.invalid"],
    )?;
    run(&repo, "git", &["config", "user.name", "UB Review Test"])?;
    run(&repo, "git", &["add", "."])?;
    run(&repo, "git", &["commit", "-m", "trusted base"])?;

    let git_output = |args: &[&str]| -> Result<String> {
        let output = isolated_command("git", &repo).args(args).output()?;
        anyhow::ensure!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    };
    let base_tree = git_output(&["show", "-s", "--format=%T", "HEAD"])?;
    write_file(
        &repo.join(".ub-review.toml"),
        "candidate config must never be parsed\n",
    )?;
    write_file(
        &repo.join("scripts/reviewer.sh"),
        "echo candidate-script-must-not-run\n",
    )?;
    write_file(
        &repo.join("src/lib.rs"),
        "pub fn answer() -> usize { 42 }\n",
    )?;
    run(&repo, "git", &["add", "."])?;
    let changed = git_output(&["diff", "--cached", "--name-only", "--no-renames", "HEAD"])?;
    let patch = git_output(&["diff", "--cached", "--binary", "--no-renames", "HEAD"])?;
    let changed_path = temp.path().join("changed-files.txt");
    let patch_path = temp.path().join("diff.patch");
    write_file(&changed_path, &changed)?;
    write_file(&patch_path, &patch)?;
    run(&repo, "git", &["reset", "--hard", "HEAD"])?;

    let out = temp.path().join("packet");
    let hostile_thread = temp.path().join("hostile-thread.md");
    write_file(&hostile_thread, "candidate thread context must not load\n")?;
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let head_sha = "f".repeat(40);
    let args = [
        "run",
        "--dry-run",
        "--config",
        path_str(&config)?,
        "--root",
        path_str(&repo)?,
        "--out",
        path_str(&out)?,
        "--trusted-base-tree",
        base_tree.trim(),
        "--trusted-head-sha",
        &head_sha,
        "--trusted-changed-files",
        path_str(&changed_path)?,
        "--trusted-diff-patch",
        path_str(&patch_path)?,
        "--model-mode",
        "off",
        "--no-github-summary",
        "--pr-thread-context",
        path_str(&hostile_thread)?,
        "--github-token",
        "trusted-mode-must-not-use-this-token",
        "--github-repo",
        "example/repo",
        "--github-pull-number",
        "1",
    ];
    run(temp.path(), bin, &args)?;
    let mut sensor_args = args.to_vec();
    sensor_args.remove(1);
    let sensor_error = run_expect_failure(temp.path(), bin, &sensor_args)?;
    assert!(
        sensor_error.contains("requires --dry-run"),
        "{sensor_error}"
    );
    let mut provider_args = args;
    provider_args[17] = "auto";
    let provider_error = run_expect_failure(temp.path(), bin, &provider_args)?;
    assert!(
        provider_error.contains("requires --model-mode off"),
        "{provider_error}"
    );
    let mut posting_args = args.to_vec();
    posting_args.extend(["--posting", "review"]);
    let posting_error = run_expect_failure(temp.path(), bin, &posting_args)?;
    assert!(
        posting_error.contains("requires --posting artifact-only"),
        "{posting_error}"
    );
    let mut heavy_args = args.to_vec();
    heavy_args.push("--allow-heavy");
    let heavy_error = run_expect_failure(temp.path(), bin, &heavy_args)?;
    assert!(
        heavy_error.contains("does not allow --allow-heavy"),
        "{heavy_error}"
    );

    let diff: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("input/diff-context.json"))?)?;
    let admission: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("input/revision-admission.json"))?)?;
    assert_eq!(diff["base"], base_tree.trim());
    assert_eq!(diff["head"], head_sha);
    assert_eq!(
        diff["changed_files"],
        serde_json::json!([".ub-review.toml", "scripts/reviewer.sh", "src/lib.rs"])
    );
    assert_eq!(admission["reviewed_commit_oid"], "f".repeat(40));
    let thread_context: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/pr_thread_context.json"))?)?;
    assert_eq!(thread_context["status"], "absent");
    assert_eq!(thread_context["sources"], serde_json::json!([]));
    assert_eq!(thread_context["thread_context"], serde_json::Value::Null);
    assert_eq!(
        fs::read_to_string(repo.join(".ub-review.toml"))?,
        "review_profile = \"safe\"\n"
    );
    let dirty_config = repo.join(".ub-review.toml");
    write_file(
        &dirty_config,
        "candidate config must not parse before root admission\n",
    )?;
    let mut dirty_config_args = args;
    dirty_config_args[3] = path_str(&dirty_config)?;
    let dirty_error = run_expect_failure(temp.path(), bin, &dirty_config_args)?;
    assert!(
        dirty_error.contains("requires a clean base-owned root"),
        "{dirty_error}"
    );
    assert!(!dirty_error.contains("parse config"), "{dirty_error}");
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
fn missing_ledger_context_identifies_source_without_failing_run() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;
    let config = temp.path().join("review.toml");
    let available = repo.join("available.md");
    write_file(&available, "available override context")?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    for (name, ledger, override_path, expected) in [
        ("omitted", "", "", "No UB ledger configured"),
        (
            "configured-missing",
            "docs/missing.md",
            "",
            "configured by `[repo].ledger`",
        ),
        (
            "override-missing",
            "docs/config-missing.md",
            "override-missing.md",
            "configured by `--ledger-path` / `UB_REVIEW_LEDGER_PATH`",
        ),
        (
            "env-missing",
            "docs/config-missing.md",
            "env-missing.md",
            "configured by `--ledger-path` / `UB_REVIEW_LEDGER_PATH`",
        ),
        (
            "override-available",
            "docs/config-missing.md",
            "available.md",
            "available override context",
        ),
    ] {
        write_file(&config, &format!("[repo]\nledger = {ledger:?}\n"))?;
        let out = temp.path().join(name);
        let mut command = isolated_command(bin, temp.path());
        command
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", temp.path())
            .args([
                "run",
                "--root",
                path_str(&repo)?,
                "--config",
                path_str(&config)?,
                "--profile",
                "gh-runner",
                "--base",
                "HEAD^",
                "--head",
                "HEAD",
                "--dry-run",
                "--model-mode",
                "off",
                "--posting",
                "artifact-only",
                "--no-github-summary",
                "--out",
                path_str(&out)?,
            ]);
        if name == "env-missing" {
            command.env("UB_REVIEW_LEDGER_PATH", override_path);
        } else {
            command.args(["--ledger-path", override_path]);
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "{name}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        for file in ["shared_context.md", "shared_context_cache_block.md"] {
            let context = fs::read_to_string(out.join("review").join(file))?;
            assert!(context.contains(expected), "{name}: {context}");
            if name.ends_with("missing") {
                let missing = if override_path.is_empty() {
                    ledger
                } else {
                    override_path
                };
                assert!(
                    context.contains(path_str(&repo.join(missing))?),
                    "{name}: {context}"
                );
                assert!(
                    context.contains("optional context omitted"),
                    "{name}: {context}"
                );
            }
            if !override_path.is_empty() {
                assert!(
                    !context.contains("configured by `[repo].ledger`"),
                    "{name}: {context}"
                );
            }
        }
        let gate: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/gate_outcome.json"))?)?;
        assert!(
            gate["reasons"]
                .as_array()
                .is_some_and(|reasons| reasons.iter().all(|reason| reason["kind"] != "policy")),
            "{name}: {gate}"
        );
    }
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

// Keep executable lookup and coverage output routing, but inherit no credentials
// or ub-review configuration in these config-admission controls.
fn config_admission_command(cwd: &Path) -> std::process::Command {
    let mut command = isolated_command(env!("CARGO_BIN_EXE_ub-review"), cwd);
    command.env_clear();
    for name in ["PATH", "LLVM_PROFILE_FILE"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

#[test]
fn explicit_config_paths_fail_before_packet_output() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;
    let directory = repo.join("directory.toml");
    fs::create_dir(&directory)?;
    let malformed = repo.join("malformed.toml");
    write_file(&malformed, "[broken")?;
    let non_text = repo.join("non-text.toml");
    fs::write(&non_text, [0xff])?;
    // Include explicitly spelling the default filename: path equality cannot
    // distinguish required input from optional discovery.
    for config in [
        "missing.toml",
        ".ub-review.toml",
        "directory.toml",
        "malformed.toml",
        "non-text.toml",
    ] {
        for from_env in [false, true] {
            let out = repo.join("packet");
            let mut command = config_admission_command(&repo);
            command.args([
                "run",
                "--root",
                ".",
                "--base",
                "HEAD~1",
                "--head",
                "HEAD",
                "--dry-run",
                "--model-mode",
                "off",
                "--posting",
                "artifact-only",
                "--no-github-summary",
                "--out",
                path_str(&out)?,
            ]);
            if from_env {
                command.env("UB_REVIEW_CONFIG", config);
            } else {
                command.args(["--config", config]);
            }
            let output = command.output()?;
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                !output.status.success(),
                "{config} env={from_env}: {output:?}"
            );
            assert!(stderr.contains(config), "{stderr}");
            if config != "malformed.toml" {
                assert!(stderr.contains("read "), "{stderr}");
                assert!(
                    stderr.contains("omit --config/UB_REVIEW_CONFIG"),
                    "{stderr}"
                );
            } else {
                assert!(stderr.contains("parse"), "{stderr}");
            }
            assert!(!out.exists(), "rejected config created packet: {config}");
        }
    }
    Ok(())
}

#[test]
fn config_discovery_and_explicit_sources_preserve_effective_values() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;
    let default_config = repo.join(".ub-review.toml");
    let custom_config = repo.join("custom.toml");
    write_file(&custom_config, "[gate]\ntarget_minutes = 47\n")?;
    for (index, source, expected) in [
        (0, "absent", 30),
        (1, "discovered", 43),
        (2, "flag", 47),
        (3, "env", 47),
        (4, "override", 47),
    ] {
        if source == "discovered" {
            write_file(&default_config, "[gate]\ntarget_minutes = 43\n")?;
        }
        let out = repo.join(format!("packet-{index}"));
        let mut command = config_admission_command(&repo);
        command.args([
            "run",
            "--root",
            ".",
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--dry-run",
            "--model-mode",
            "off",
            "--posting",
            "artifact-only",
            "--no-github-summary",
            "--profile",
            "gh-runner-full",
            "--out",
            path_str(&out)?,
        ]);
        match source {
            "flag" | "override" => {
                command.args(["--config", "custom.toml"]);
            }
            "env" => {
                command.env("UB_REVIEW_CONFIG", "custom.toml");
            }
            _ => {}
        }
        if source == "override" {
            command.env("UB_REVIEW_CONFIG", "must-not-load.toml");
        }
        let output = command.output()?;
        assert!(output.status.success(), "{source}: {output:?}");
        let effective: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("effective-config.json"))?)?;
        assert_eq!(effective["gate"]["target_minutes"], expected, "{source}");
        // Use a nondefault profile so dropping the override cannot pass.
        assert_eq!(effective["profile"], "gh-runner-full", "{source}");
        assert!(out.join("review/gate_outcome.json").is_file());
    }
    Ok(())
}

#[test]
fn config_reading_subcommands_reject_explicit_missing_paths() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    for args in [
        vec![
            "plan",
            "--config",
            "missing.toml",
            "--write",
            "--out",
            "packet",
        ],
        vec!["doctor", "--config", "missing.toml"],
        vec![
            "cache",
            "warm",
            "--config",
            "missing.toml",
            "--out",
            "cache",
        ],
    ] {
        let mut command = config_admission_command(temp.path());
        command.args(&args);
        let output = command.output()?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}: {output:?}");
        assert!(stderr.contains("read missing.toml"), "{args:?}: {stderr}");
        assert!(
            stderr.contains("omit --config/UB_REVIEW_CONFIG"),
            "{stderr}"
        );
        assert!(output.stdout.is_empty(), "{args:?}: {output:?}");
        assert!(!temp.path().join("packet").exists());
        assert!(!temp.path().join("cache").exists());
    }
    // An existing but unreadable optional discovery path must still fail.
    fs::create_dir(temp.path().join(".ub-review.toml"))?;
    let output = config_admission_command(temp.path())
        .args(["plan", "--write", "--out", "packet"])
        .output()?;
    assert!(!output.status.success(), "{output:?}");
    assert!(String::from_utf8_lossy(&output.stderr).contains("read .ub-review.toml"));
    assert!(!temp.path().join("packet").exists());
    Ok(())
}
