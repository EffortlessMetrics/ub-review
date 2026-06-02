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
    assert!(out.join("review/post-error.json").exists());

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
    assert!(summary.contains("## Lane packets"));
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
