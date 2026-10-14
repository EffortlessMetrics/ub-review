//! Compiler-free plan previews must describe the selected native default roster.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde_json::{Value, json};

#[path = "common/cli_run.rs"]
mod common;
use common::*;

fn fixture(path: &str, text: &str, config: &str) -> Result<tempfile::TempDir> {
    let temp = tempfile::tempdir()?;
    init_minimal_repo(temp.path())?;
    if path != "src/lib.rs" {
        run(temp.path(), "git", &["reset", "--hard", "HEAD~1"])?;
        write_file(&temp.path().join(path), text)?;
        run(temp.path(), "git", &["add", "."])?;
        run(temp.path(), "git", &["commit", "-m", "selected diff"])?;
    }
    write_file(&temp.path().join(".ub-review.toml"), config)?;
    Ok(temp)
}

fn plan_args<'a>(out: &'a Path, selectors: &[&'a str]) -> Result<Vec<&'a str>> {
    let mut args = vec![
        "plan",
        "--root",
        ".",
        "--config",
        ".ub-review.toml",
        "--base",
        "HEAD~1",
        "--head",
        "HEAD",
        "--out",
        path_str(out)?,
    ];
    args.extend_from_slice(selectors);
    Ok(args)
}

fn preview(repo: &Path, name: &str, selectors: &[&str], expected: &[&str]) -> Result<Value> {
    let out = repo.join(name);
    let mut args = plan_args(&out, selectors)?;
    args.push("--write");
    let output = isolated_command(env!("CARGO_BIN_EXE_ub-review"), repo)
        .args(args)
        .output()?;
    anyhow::ensure!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout)?;
    let printed = stdout
        .split_once("Lanes:\n")
        .ok_or_else(|| anyhow::anyhow!("missing lane heading: {stdout}"))?
        .1
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect::<Vec<_>>();
    assert_eq!(printed, expected);
    let plan: Value = serde_json::from_slice(&fs::read(out.join("plan.json"))?)?;
    let resolved: Value = serde_json::from_slice(&fs::read(out.join("resolved-plan.json"))?)?;
    for artifact in [&plan, &resolved] {
        let lanes = artifact["lanes"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("lanes must be an array"))?;
        let ids = lanes
            .iter()
            .map(|lane| lane["id"].clone())
            .collect::<Vec<_>>();
        assert_eq!(ids, expected.iter().map(|id| json!(id)).collect::<Vec<_>>());
    }
    assert_eq!(
        resolved["selectors"]["effective_model_lanes"],
        json!(expected)
    );
    assert_eq!(resolved["selectors"]["lane_width"], 10);
    assert_eq!(resolved["selectors"]["depth"], "standard");
    assert_eq!(resolved["selectors"]["source"], "plan-default");
    assert_eq!(resolved["selectors"]["run_pass"], "plan-default");
    for absent in [
        "sensors",
        "lanes",
        "events.ndjson",
        "review/gate_outcome.json",
    ] {
        assert!(
            !out.join(absent).exists(),
            "planner executed work: {absent}"
        );
    }
    Ok(resolved)
}

#[test]
fn static_plan_filters_include_then_exclude_and_records_the_same_roster() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let repo = fixture("src/lib.rs", "", "")?;
    let all = [
        "correctness",
        "tests-red-green",
        "source-route",
        "architecture",
        "opposition",
    ];
    preview(repo.path(), "default", &[], &all)?;
    preview(
        repo.path(),
        "explicit-empty",
        &["--lanes", "", "--except-lanes", ""],
        &all,
    )?;
    let selected = preview(
        repo.path(),
        "selected",
        &[
            "--lanes",
            " opposition,tests-red-green,correctness,correctness ",
            "--except-lanes",
            "opposition",
        ],
        &["correctness", "tests-red-green"],
    )?;
    assert_eq!(
        selected["selectors"]["lanes"],
        json!(["correctness", "opposition", "tests-red-green"])
    );
    assert_eq!(selected["selectors"]["except_lanes"], json!(["opposition"]));
    preview(
        repo.path(),
        "excluded",
        &["--except-lanes", "architecture"],
        &[
            "correctness",
            "tests-red-green",
            "source-route",
            "opposition",
        ],
    )?;
    preview(
        repo.path(),
        "empty",
        &["--lanes", "correctness", "--except-lanes", "correctness"],
        &[],
    )?;
    Ok(())
}

#[test]
fn static_plan_rejects_unknown_lanes_before_printing_or_writing() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let repo = fixture("src/lib.rs", "", "")?;
    for flag in ["--lanes", "--except-lanes"] {
        for write in [false, true] {
            let out = repo.path().join(format!("unknown-{flag}-{write}"));
            let mut args = plan_args(&out, &[flag, "missing-lane"])?;
            if write {
                args.push("--write");
            }
            let error = run_expect_failure(repo.path(), env!("CARGO_BIN_EXE_ub-review"), &args)?;
            assert!(
                error.contains("unknown lane selector(s): missing-lane"),
                "{error}"
            );
            assert!(
                !error.contains("Profile:"),
                "preview printed before rejection: {error}"
            );
            assert!(!out.exists());
        }
    }
    Ok(())
}

#[test]
fn static_plan_uses_standard_ub_width_instead_of_legacy_six_lanes() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let repo = fixture(
        "src/unsafe.rs",
        "pub fn read(ptr: *const i32) -> i32 { unsafe { *ptr } }\n",
        "",
    )?;
    let selected = preview(
        repo.path(),
        "ub-default",
        &[],
        &[
            "ub-memory-lifetime",
            "ub-active-view",
            "ub-worker-handoff",
            "tests-red-green",
            "tests-oracle",
            "source-route",
            "sibling-paths",
            "architecture",
            "security",
            "opposition",
        ],
    )?;
    assert_eq!(selected["diff_class"], "source-ub");
    preview(
        repo.path(),
        "oracle",
        &["--lanes", "tests-oracle"],
        &["tests-oracle"],
    )?;
    let out = repo.path().join("legacy");
    let error = run_expect_failure(
        repo.path(),
        env!("CARGO_BIN_EXE_ub-review"),
        &plan_args(&out, &["--lanes", "ub"])?,
    )?;
    assert!(error.contains("unknown lane selector(s): ub"), "{error}");
    assert!(!out.exists());
    Ok(())
}

#[test]
fn static_plan_merges_applicable_repo_lanes_before_filtering() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let config = r#"
[[lanes]]
id = "opposition"
role = "Repo-specific opposition"
focus = "Check local contract"
model = "custom:repo-opposition"
[[lanes]]
id = "repo-extra"
focus = "Check extra contract"
[[lanes]]
id = "docs-claims"
focus = "Check documentation claims"
diff_classes = ["docs-only"]
"#;
    let repo = fixture("src/lib.rs", "", config)?;
    let selected = preview(
        repo.path(),
        "repo-lanes",
        &[],
        &[
            "correctness",
            "tests-red-green",
            "source-route",
            "architecture",
            "opposition",
            "repo-extra",
        ],
    )?;
    assert_eq!(selected["lanes"][4]["model"], "custom:repo-opposition");
    assert_eq!(selected["lanes"][4]["role"], "Repo-specific opposition");
    assert_eq!(selected["lanes"][4]["focus"], "Check local contract");
    preview(
        repo.path(),
        "repo-only",
        &["--lanes", "repo-extra"],
        &["repo-extra"],
    )?;
    let docs = fixture("README.md", "A prose-only clarification.\n", config)?;
    preview(
        docs.path(),
        "docs",
        &[],
        &["opposition", "repo-extra", "docs-claims"],
    )?;
    let empty_docs = fixture("README.md", "A prose-only clarification.\n", "")?;
    let selected = preview(empty_docs.path(), "docs", &[], &[])?;
    assert_eq!(selected["diff_class"], "docs-only");
    let out = empty_docs.path().join("invalid-docs");
    let error = run_expect_failure(
        empty_docs.path(),
        env!("CARGO_BIN_EXE_ub-review"),
        &plan_args(&out, &["--lanes", "correctness"])?,
    )?;
    assert!(
        error.contains("unknown lane selector(s): correctness"),
        "{error}"
    );
    assert!(!out.exists());
    Ok(())
}

#[test]
fn static_plan_without_write_applies_selectors_without_creating_artifacts() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let repo = fixture("src/lib.rs", "", "")?;
    let out: PathBuf = repo.path().join("no-write");
    let output = isolated_command(env!("CARGO_BIN_EXE_ub-review"), repo.path())
        .args(plan_args(&out, &["--lanes", "source-route"])?)
        .output()?;
    anyhow::ensure!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout)?;
    let printed = stdout
        .split_once("Lanes:\n")
        .ok_or_else(|| anyhow::anyhow!("missing lane heading: {stdout}"))?
        .1
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect::<Vec<_>>();
    assert_eq!(printed, ["source-route"]);
    assert!(!out.exists());
    Ok(())
}
