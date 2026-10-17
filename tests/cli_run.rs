//! Run and proof-focused CLI integration tests.

use std::fs;
use std::path::Path;

use anyhow::Result;

#[path = "common/cli_run.rs"]
mod common;
use common::*;

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
fn run_preserves_actual_pr_thread_context_truncation() -> Result<()> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;
    let cases = [
        ("exact-boundary", 0, "x".repeat(1200), 8192, false),
        (
            "oversize-issue",
            0,
            "x".repeat(1200) + "OMITTED_REPLY",
            8192,
            true,
        ),
        (
            "oversize-summary",
            1,
            "x".repeat(1200) + "OMITTED_REPLY",
            8192,
            true,
        ),
        (
            "oversize-inline",
            2,
            "x".repeat(1200) + "OMITTED_REPLY",
            8192,
            true,
        ),
        (
            "oversize-utf8",
            2,
            "🔥".repeat(299) + "é🔥OMITTED_REPLY",
            8192,
            true,
        ),
        (
            "literal-marker",
            0,
            "original text says [truncated]".to_owned(),
            8192,
            false,
        ),
        // The body fits its individual budget; only the whole snapshot clips.
        ("global-only-limit", 2, "g".repeat(300), 512, true),
    ];
    let payloads = cases
        .iter()
        .map(|(_, kind, body, _, _)| {
            let mut responses = [b"[]".to_vec(), b"[]".to_vec(), b"[]".to_vec()];
            responses[*kind] = serde_json::to_vec(&serde_json::json!([{
                "id": 1, "user": {"login": "fixture-owner"}, "body": body,
                "path": "src/lib.rs", "line": 2
            }]))?;
            Ok(responses)
        })
        .collect::<Result<Vec<_>>>()?;
    // Test-only loopback fixture: exactly three requests per CLI invocation,
    // bounded idle/stream timeouts, and no real GitHub credential or access.
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let api_url = format!("http://{}", listener.local_addr()?);
    let server = std::thread::spawn(move || -> Result<()> {
        let mut served = 0;
        let mut deadline = Instant::now() + Duration::from_secs(60);
        while served < payloads.len() * 3 {
            let (mut stream, _) = match listener.accept() {
                Ok(connection) => connection,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    anyhow::ensure!(Instant::now() < deadline, "thread fixture timed out");
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            stream.set_nonblocking(false)?;
            stream.set_read_timeout(Some(Duration::from_secs(30)))?;
            stream.set_write_timeout(Some(Duration::from_secs(30)))?;
            let mut reader = BufReader::new(stream.try_clone()?);
            let mut request = String::new();
            loop {
                let mut line = String::new();
                anyhow::ensure!(reader.read_line(&mut line)? > 0, "incomplete HTTP request");
                if line == "\r\n" {
                    break;
                }
                request.push_str(&line);
            }
            let kind = if request.starts_with("GET /repos/example/fixture/issues/1/comments?") {
                0
            } else if request.starts_with("GET /repos/example/fixture/pulls/1/reviews?") {
                1
            } else {
                anyhow::ensure!(
                    request.starts_with("GET /repos/example/fixture/pulls/1/comments?"),
                    "unexpected request: {request}"
                );
                2
            };
            let body = &payloads[served / 3][kind];
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )?;
            stream.write_all(body)?;
            served += 1;
            deadline = Instant::now() + Duration::from_secs(60);
        }
        Ok(())
    });
    let config = Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml");
    let mut mismatches = Vec::new();
    for (name, _, body, max_bytes, expected) in cases {
        let out = temp.path().join(name);
        let output = isolated_command(env!("CARGO_BIN_EXE_ub-review"), temp.path())
            .env_remove("GITHUB_EVENT_PATH")
            .args([
                "run",
                "--dry-run",
                "--config",
                path_str(&config)?,
                "--root",
                path_str(&repo)?,
                "--base",
                "HEAD~1",
                "--out",
                path_str(&out)?,
                "--model-mode",
                "off",
                "--no-github-summary",
                "--github-token",
                "fixture-no-credential",
                "--github-repo",
                "example/fixture",
                "--github-pull-number",
                "1",
                "--github-api-url",
                &api_url,
                "--pr-thread-context-max-bytes",
                &max_bytes.to_string(),
            ])
            .output()?;
        anyhow::ensure!(
            output.status.success(),
            "{name}: CLI failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let context: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/pr_thread_context.json"))?)?;
        let review: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
        assert_eq!(review["pr_thread_context"], context, "{name}");
        assert_eq!(context["threads"][0]["body"], body, "{name}");
        let rendered = context["thread_context"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{name}: missing rendered context"))?;
        assert_eq!(
            rendered.contains("[truncated]"),
            expected || name == "literal-marker",
            "{name}"
        );
        assert!(!rendered.contains("OMITTED_REPLY"), "{name}");
        if name == "oversize-utf8" {
            assert!(rendered.contains(&("🔥".repeat(299) + "é\n[truncated]\n")));
        } else if name == "exact-boundary" {
            assert!(rendered.contains(&"x".repeat(1200)), "{name}");
        } else if name.starts_with("oversize-") {
            assert!(
                rendered.contains(&("x".repeat(1200) + "\n[truncated]\n")),
                "{name}"
            );
        }
        let shared = fs::read_to_string(out.join("review/shared_context.md"))?;
        assert!(shared.contains(rendered), "{name}");
        let actual = context["thread_context_truncated"].as_bool();
        eprintln!("{name}: CLI exit=0, truncation={actual:?}, expected={expected}");
        if actual != Some(expected) {
            mismatches.push(name);
        }
    }
    server
        .join()
        .map_err(|_| anyhow::anyhow!("thread fixture panicked"))??;
    anyhow::ensure!(
        mismatches.is_empty(),
        "truncation mismatches: {mismatches:?}"
    );
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
