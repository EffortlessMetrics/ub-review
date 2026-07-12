use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

#[test]
fn gh_runner_tool_installer_pins_core_rust_sensor_versions() -> Result<()> {
    let script = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/install-gh-runner-tools.sh"),
    )?;
    assert!(script.contains("UB_REVIEW_TOKMD_VERSION:-1.12.0"));
    assert!(script.contains("UB_REVIEW_CARGO_ALLOW_VERSION:-0.1.8"));
    assert!(script.contains("install_cargo_bin tokmd tokmd \"$tokmd_version\""));
    assert!(script.contains("install_cargo_bin cargo-allow cargo-allow \"$cargo_allow_version\""));
    assert!(
        !script.contains("install_cargo_bin tokmd tokmd\n"),
        "hosted fallback must not install crates.io latest tokmd implicitly"
    );
    assert!(
        !script.contains("install_cargo_bin cargo-allow cargo-allow\n"),
        "hosted fallback must not install crates.io latest cargo-allow implicitly"
    );
    Ok(())
}

#[test]
fn review_image_tool_installer_uses_tool_dir_as_install_prefix() -> Result<()> {
    let script = fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/install-review-image-tools.sh"),
    )?;
    assert!(script.contains("UB_REVIEW_TOKMD_VERSION:-1.12.0"));
    assert!(script.contains("UB_REVIEW_CARGO_ALLOW_VERSION:-0.1.8"));
    assert!(script.contains("UB_REVIEW_RIPR_VERSION:-0.10.0"));
    let github_runner_script = std::fs::read_to_string("scripts/install-gh-runner-tools.sh")?;
    let doctor_source = std::fs::read_to_string("src/post_run_utils.rs")?;
    assert!(github_runner_script.contains("UB_REVIEW_RIPR_VERSION:-0.10.0"));
    assert!(doctor_source.contains("STANDARD_IMAGE_RIPR_VERSION: &str = \"0.10.0\""));
    assert!(script.contains("UB_REVIEW_UNSAFE_REVIEW_VERSION:-0.3.4"));
    assert!(script.contains(
        "install_tool cargo-allow cargo-allow \"${UB_REVIEW_CARGO_ALLOW_VERSION:-0.1.8}\""
    ));
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
fn action_forwards_prior_resolved_candidates_input() -> Result<()> {
    let action = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("action.yml"))?;
    assert!(action.contains("prior-resolved-candidates:"));
    assert!(action.contains("prior-resolved-candidates-artifact:"));
    assert!(action.contains("name: Resolve prior resolved candidates"));
    assert!(action.contains("MANUAL_PRIOR_RESOLVED_CANDIDATES"));
    assert!(action.contains("gh run list"));
    assert!(
        action
            .contains("gh run download \"$run_id\" --name \"$PRIOR_RESOLVED_CANDIDATES_ARTIFACT\"")
    );
    assert!(action.contains("review/resolved_candidates.json"));
    assert!(action.contains(".conclusion == \\\"success\\\" or .conclusion == \\\"failure\\\""));
    assert!(action.contains(
        "--prior-resolved-candidates \"${{ steps.prior_resolved_candidates.outputs.path }}\""
    ));
    Ok(())
}

#[test]
fn gate_workflow_grants_actions_read_for_prior_resolved_candidates_lookup() -> Result<()> {
    let workflow = include_str!("../.github/workflows/ub-review-gate.yml");
    assert!(workflow.contains("actions: read"));
    assert!(workflow.contains("uses: ./"));
    assert!(
        workflow.contains("id-token: write"),
        "coverage upload must retain OIDC permission"
    );
    assert!(
        workflow.contains("codecov/codecov-action@v7"),
        "self gate should use Codecov's Node 24-backed action line"
    );
    assert!(
        !workflow.contains("codecov/codecov-action@v5"),
        "Codecov v5 pulls actions/github-script on Node 20 and reintroduces hosted-runner deprecation noise"
    );
    Ok(())
}

#[test]
fn quality_backfill_workflow_is_artifact_only_not_pr_gate_noise() -> Result<()> {
    let workflow = include_str!("../.github/workflows/quality-backfill.yml");
    assert_eq!(workflow.lines().next(), Some("name: Quality Backfill"));
    assert_eq!(workflow.matches("schedule:").count(), 1);
    assert_eq!(workflow.matches("workflow_dispatch:").count(), 1);
    assert_eq!(workflow.matches("pull_request:").count(), 0);
    assert_eq!(workflow.matches("pull-requests: read").count(), 1);
    assert_eq!(workflow.matches("pull-requests: write").count(), 0);
    assert_eq!(
        workflow
            .matches("UB_REVIEW_QUALITY_WINDOW_DAYS: 30")
            .count(),
        1
    );
    assert_eq!(workflow.matches("created >= cutoff").count(), 1);
    assert_eq!(
        workflow
            .matches("GITHUB_TOKEN: ${{ github.token }}")
            .count(),
        1
    );
    assert_eq!(
        workflow.matches("ub-review quality-github-collect").count(),
        1
    );
    assert_eq!(
        workflow
            .matches("--pull-numbers-file target/ub-review-quality/source/github/pr-numbers.txt")
            .count(),
        1
    );
    assert_eq!(workflow.matches("gh api graphql").count(), 0);
    assert_eq!(workflow.matches("review-threads-${number}.json").count(), 0);
    assert_eq!(
        workflow
            .matches("ub-review quality-github-outcomes")
            .count(),
        1
    );
    assert_eq!(workflow.matches("ub-review quality-backfill").count(), 1);
    assert_eq!(workflow.matches("future GitHub thread").count(), 0);
    assert_eq!(
        workflow
            .matches("Download previous quality backfill")
            .count(),
        1
    );
    assert_eq!(
        workflow
            .matches("previous=(--previous \"$previous_file\")")
            .count(),
        1
    );
    assert_eq!(workflow.matches("actions/upload-artifact@v7").count(), 1);
    assert_eq!(workflow.matches("ub-review post").count(), 0);
    Ok(())
}

#[test]
fn quality_github_outcomes_cli_writes_thread_state_receipt() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let source = temp.path().join("github");
    fs::create_dir_all(&source)?;
    fs::write(source.join("actions-runs.json"), "[]")?;
    fs::write(source.join("pr-state.json"), "[]")?;
    fs::write(source.join("review-threads.graphql"), "query ReviewThreads")?;
    fs::write(source.join("review-threads-request-12.json"), "{}")?;
    fs::write(
        source.join("review-threads-12.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "data": {
                "repository": {
                    "pullRequest": {
                        "number": 12,
                        "mergedAt": "2026-06-13T01:29:45Z",
                        "files": {
                            "pageInfo": {"hasNextPage": false, "endCursor": null},
                            "nodes": [
                                {
                                    "path": "tests/generated.rs",
                                    "additions": 6,
                                    "deletions": 0,
                                    "changeType": "ADDED"
                                }
                            ]
                        },
                        "reviewThreads": {
                            "pageInfo": {"hasNextPage": false, "endCursor": null},
                            "nodes": [
                                {
                                    "id": "thread-one",
                                    "isResolved": true,
                                    "comments": {
                                        "pageInfo": {"hasNextPage": false, "endCursor": null},
                                        "nodes": [
                                            {
                                                "id": "comment-one",
                                                "body": "[tests] generated by ub-review",
                                                "createdAt": "2026-06-13T01:35:00Z",
                                                "url": "https://github.example/comment-one",
                                                "author": {"login": "github-actions"}
                                            },
                                            {
                                                "id": "comment-two",
                                                "body": "not ub-review",
                                                "createdAt": "2026-06-13T01:36:00Z",
                                                "url": "https://github.example/comment-two",
                                                "author": {"login": "coderabbitai"}
                                            }
                                        ]
                                    }
                                }
                            ]
                        }
                    }
                }
            }
        }))?,
    )?;
    let out = source.join("github-quality-outcomes.json");

    let output = Command::new(bin)
        .arg("quality-github-outcomes")
        .arg("--source-dir")
        .arg(&source)
        .arg("--out")
        .arg(&out)
        .output()?;
    assert!(
        output.status.success(),
        "quality-github-outcomes failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: serde_json::Value = serde_json::from_slice(&fs::read(out)?)?;

    assert_eq!(artifact["schema"], "ub-review.github_quality_outcomes.v1");
    assert_eq!(artifact["collection_status"], "complete");
    let source_artifacts = artifact["source_artifacts"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("source_artifacts missing"))?;
    assert_eq!(source_artifacts.len(), 5);
    let comments = artifact["comments"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("comments[] missing"))?;
    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0]["posted"], true);
    assert_eq!(comments[0]["accepted"], true);
    assert_eq!(comments[0]["resolved"], true);
    assert_eq!(comments[0]["reviewer_override"], false);
    let adopted = artifact["adopted_generated_tests"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("adopted_generated_tests missing"))?;
    assert_eq!(adopted.len(), 1);
    assert_eq!(adopted[0]["path"], "tests/generated.rs");
    assert_eq!(adopted[0]["source_pull_number"], 12);
    Ok(())
}

#[test]
fn onboarding_help_matches_supported_cli_contract() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let bin = env!("CARGO_BIN_EXE_ub-review");

    let init_help = run_capture_with_env(temp.path(), bin, &["init", "--help"], &[])?;
    assert!(
        init_help.contains("--profile <PROFILE>"),
        "init help should advertise the supported profile selector:\n{init_help}"
    );
    assert!(
        init_help.contains("--guide-out <GUIDE_OUT>"),
        "init help should advertise the file-driven setup guide:\n{init_help}"
    );
    assert!(
        init_help.contains("--no-guide"),
        "init help should advertise the explicit guide opt-out:\n{init_help}"
    );
    assert!(
        !init_help.contains("--mode"),
        "init help must not advertise unsupported onboarding mode flags:\n{init_help}"
    );

    let setup_ci_help = run_capture_with_env(temp.path(), bin, &["setup-ci", "--help"], &[])?;
    assert!(
        setup_ci_help.contains("--print-pr"),
        "setup-ci help should keep the local render path visible:\n{setup_ci_help}"
    );
    assert!(
        setup_ci_help.contains("--open-pr"),
        "setup-ci help should advertise the implemented PR-opening path:\n{setup_ci_help}"
    );
    for stale in [
        "print-pr only",
        "PR opening is a later slice",
        "no network, no GitHub calls",
    ] {
        assert!(
            !setup_ci_help.contains(stale),
            "setup-ci help leaked stale onboarding text `{stale}`:\n{setup_ci_help}"
        );
    }

    Ok(())
}

#[test]
fn setup_ci_print_pr_cli_materializes_accepted_preview_files() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let out = temp.path().join("target/ub-review");
    write_setup_ci_cli_audit_fixture(&out.join("ci-audit"))?;

    let action_sha = "d".repeat(40);
    let out_arg = path_str(&out)?;
    let integration_accept = "integration=cargo test --workspace --locked";
    let unit_accept = "unit=cargo test --lib --locked";
    let mut command = isolated_command(bin, temp.path());
    command
        .env_remove("GITHUB_REPOSITORY")
        .env_remove("GITHUB_TOKEN")
        .args([
            "setup-ci",
            "--print-pr",
            "--out",
            out_arg,
            "--accept",
            integration_accept,
            "--accept",
            unit_accept,
            "--action-sha",
            action_sha.as_str(),
        ]);
    let output = command.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "setup-ci --print-pr failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("# CI migration plan"),
        "setup-ci --print-pr should render the plan to stdout:\n{stdout}"
    );
    assert!(
        stderr.contains("wrote 4 setup-ci preview file(s)"),
        "setup-ci --print-pr should report the preview file materialization:\n{stderr}"
    );

    let ci_audit = out.join("ci-audit");
    let plan = fs::read_to_string(ci_audit.join("migration-plan.md"))?;
    assert_eq!(
        stdout, plan,
        "setup-ci stdout must exactly mirror migration-plan.md"
    );
    for expected in [
        "Fold 2 accepted job(s) into one required check `ub-review/gate`",
        "accepted; command `cargo test --workspace --locked`",
        "accepted; command `cargo test --lib --locked`",
        "ci-audit/correlation.json#integration",
        "ci-audit/correlation.json#unit",
        "old required checks unknown",
        "refuses to invent it",
    ] {
        assert!(
            plan.contains(expected),
            "migration plan missing `{expected}`:\n{plan}"
        );
    }

    let preview = ci_audit.join("preview");
    let files = collect_relative_file_paths(&preview)?;
    assert_eq!(
        files,
        vec![
            ".github/workflows/ub-review-gate.yml",
            ".ub-review.toml",
            "docs/ci/branch-protection-change.md",
            "docs/ci/ub-review-migration.md",
        ],
        "setup-ci --print-pr should materialize only the migration preview files"
    );

    let generated_config = fs::read_to_string(preview.join(".ub-review.toml"))?;
    for expected in [
        "required_check = \"ub-review/gate\"",
        "id = \"integration\"",
        "command = \"cargo test --workspace --locked\"",
        "required = false",
        "id = \"unit\"",
        "command = \"cargo test --lib --locked\"",
        "required = true",
    ] {
        assert!(
            generated_config.contains(expected),
            "generated config missing `{expected}`:\n{generated_config}"
        );
    }
    for forbidden in ["[providers]", "synchronize_mode", "[tools."] {
        assert!(
            !generated_config.contains(forbidden),
            "setup-ci generated decorative or inert config key `{forbidden}`:\n{generated_config}"
        );
    }

    let workflow = fs::read_to_string(preview.join(".github/workflows/ub-review-gate.yml"))?;
    for expected in [
        "name: ub-review/gate",
        &format!("uses: EffortlessMetrics/ub-review@{action_sha}"),
        "posting: artifact-only",
        "model-mode: 'off'",
    ] {
        assert!(
            workflow.contains(expected),
            "generated workflow missing `{expected}`:\n{workflow}"
        );
    }

    let migration = fs::read_to_string(preview.join("docs/ci/ub-review-migration.md"))?;
    assert_eq!(
        migration, plan,
        "preview migration doc must exactly mirror migration-plan.md"
    );
    let branch_doc = fs::read_to_string(preview.join("docs/ci/branch-protection-change.md"))?;
    for expected in [
        "Branch protection remains manual",
        "`setup-ci` opened a migration PR only; it did not mutate repository protection rules.",
        "one observed red proof",
        "does not prove an old-check remove list",
    ] {
        assert!(
            branch_doc.contains(expected),
            "branch-protection doc missing `{expected}`:\n{branch_doc}"
        );
    }
    assert!(
        !ci_audit.join("setup-pr-result.json").exists(),
        "print-pr must not write open-pr success receipts"
    );
    assert!(
        !ci_audit.join("setup-pr-error.json").exists(),
        "print-pr must not write open-pr failure receipts"
    );
    for relative in [
        "setup-pr-branch-payload.json",
        "setup-pr-pull-payload.json",
        "setup-pr-file-payload-0.json",
        "setup-pr-file-payload-1.json",
        "setup-pr-file-payload-2.json",
        "setup-pr-file-payload-3.json",
    ] {
        assert!(
            !ci_audit.join(relative).exists(),
            "print-pr must not write open-pr mutation payload `{relative}`"
        );
    }
    for relative in [
        ".ub-review.toml",
        ".github/workflows/ub-review-gate.yml",
        "docs/ci/ub-review-migration.md",
        "docs/ci/branch-protection-change.md",
    ] {
        assert!(
            !temp.path().join(relative).exists(),
            "print-pr must keep generated repo files under the preview directory, not write `{relative}`"
        );
    }

    Ok(())
}

#[test]
fn setup_ci_open_pr_cli_creates_payloads_and_terminal_receipt() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let out = temp.path().join("target/ub-review");
    write_setup_ci_cli_audit_fixture(&out.join("ci-audit"))?;

    let action_sha = "e".repeat(40);
    let out_arg = path_str(&out)?;
    let integration_accept = "integration=cargo test --workspace --locked";
    let mut preview = isolated_command(bin, temp.path());
    preview
        .env_remove("GITHUB_REPOSITORY")
        .env_remove("GITHUB_TOKEN")
        .args([
            "setup-ci",
            "--print-pr",
            "--out",
            out_arg,
            "--accept",
            integration_accept,
            "--action-sha",
            action_sha.as_str(),
        ]);
    let preview_output = preview.output()?;
    let preview_stdout = String::from_utf8_lossy(&preview_output.stdout).to_string();
    let preview_stderr = String::from_utf8_lossy(&preview_output.stderr).to_string();
    assert!(
        preview_output.status.success(),
        "setup-ci --print-pr failed before open-pr contract\nstdout:\n{preview_stdout}\nstderr:\n{preview_stderr}"
    );

    let ci_audit = out.join("ci-audit");
    fs::write(ci_audit.join("setup-pr-error.json"), "{}")?;
    // Sequence: repo meta, base ref, base tree, create ref, 4 file PUTs,
    // open PR = 9 requests.
    let (api_url, handle) = spawn_fake_setup_ci_api(9, false)?;
    let mut command = isolated_command(bin, temp.path());
    command
        .env_remove("GITHUB_REPOSITORY")
        .env("GITHUB_TOKEN", "test-token")
        .args([
            "setup-ci",
            "--open-pr",
            "--out",
            out_arg,
            "--repo",
            "acme/widgets",
            "--github-api-url",
            api_url.as_str(),
            "--accept",
            integration_accept,
            "--action-sha",
            action_sha.as_str(),
        ]);
    let output = command.output()?;
    let requests = match handle.join() {
        Ok(result) => result?,
        Err(_) => bail!("fake setup-ci API thread panicked"),
    };
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "setup-ci --open-pr failed\nstdout:\n{stdout}\nstderr:\n{stderr}\nrequests:\n{requests:#?}"
    );
    assert!(
        stdout.contains("# CI migration plan"),
        "setup-ci --open-pr should render the migration plan before opening the PR:\n{stdout}"
    );
    assert!(
        stdout.contains("opened https://github.com/acme/widgets/pull/77"),
        "setup-ci --open-pr should print the opened PR URL:\n{stdout}"
    );
    assert!(
        stderr.contains("setup-pr-result.json"),
        "setup-ci --open-pr should report the terminal success receipt:\n{stderr}"
    );

    assert_eq!(requests.len(), 9, "unexpected setup-ci API request count");
    assert!(requests[0].starts_with("GET /repos/acme/widgets "));
    assert!(requests[1].contains("GET /repos/acme/widgets/git/ref/heads/main "));
    assert!(requests[2].contains("GET /repos/acme/widgets/git/trees/basesha"));
    assert!(requests[3].starts_with("POST /repos/acme/widgets/git/refs "));
    assert!(requests[3].contains("ub-review/setup-ci-migration"));
    assert!(requests[4].contains("PUT /repos/acme/widgets/contents/.ub-review.toml "));
    assert!(
        requests[5]
            .contains("PUT /repos/acme/widgets/contents/.github/workflows/ub-review-gate.yml ")
    );
    assert!(
        requests[6].contains("PUT /repos/acme/widgets/contents/docs/ci/ub-review-migration.md ")
    );
    assert!(
        requests[7]
            .contains("PUT /repos/acme/widgets/contents/docs/ci/branch-protection-change.md ")
    );
    assert!(requests[8].starts_with("POST /repos/acme/widgets/pulls "));
    let all_requests = requests.join("\n");
    for forbidden in ["/branches/", "/rulesets"] {
        assert!(
            !all_requests.contains(forbidden),
            "setup-ci --open-pr must not call branch-protection/ruleset APIs; saw `{forbidden}` in:\n{all_requests}"
        );
    }

    let result = read_json(&ci_audit.join("setup-pr-result.json"))?;
    assert_eq!(
        json_str_field(&result, "schema")?,
        "ub-review.setup_pr_result.v1"
    );
    assert_eq!(json_str_field(&result, "repo")?, "acme/widgets");
    assert_eq!(json_str_field(&result, "base")?, "main");
    assert_eq!(
        json_str_field(&result, "branch")?,
        "ub-review/setup-ci-migration"
    );
    assert_eq!(
        json_str_field(&result, "pr_url")?,
        "https://github.com/acme/widgets/pull/77"
    );
    assert_eq!(json_str_field(&result, "action_sha")?, action_sha);
    assert_eq!(
        json_array_field(&result, "files")?,
        &[
            serde_json::json!(".ub-review.toml"),
            serde_json::json!(".github/workflows/ub-review-gate.yml"),
            serde_json::json!("docs/ci/ub-review-migration.md"),
            serde_json::json!("docs/ci/branch-protection-change.md"),
        ]
    );
    assert!(
        !ci_audit.join("setup-pr-error.json").exists(),
        "successful setup-ci --open-pr must remove stale error receipts"
    );

    let branch_payload = read_json(&ci_audit.join("setup-pr-branch-payload.json"))?;
    assert_eq!(
        json_str_field(&branch_payload, "ref")?,
        "refs/heads/ub-review/setup-ci-migration"
    );
    assert_eq!(
        json_str_field(&branch_payload, "sha")?,
        "basesha0000000000000000000000000000000000"
    );
    let pull_payload = read_json(&ci_audit.join("setup-pr-pull-payload.json"))?;
    assert_eq!(
        json_str_field(&pull_payload, "title")?,
        "Adopt ub-review/gate from the CI audit"
    );
    assert_eq!(
        json_str_field(&pull_payload, "head")?,
        "ub-review/setup-ci-migration"
    );
    assert_eq!(json_str_field(&pull_payload, "base")?, "main");
    assert_eq!(
        json_str_field(&pull_payload, "body")?,
        fs::read_to_string(ci_audit.join("migration-plan.md"))?
    );

    let expected_files = [
        (
            ".ub-review.toml",
            "Add the ub-review gate policy from the CI audit",
        ),
        (
            ".github/workflows/ub-review-gate.yml",
            "Add the ub-review gate workflow",
        ),
        (
            "docs/ci/ub-review-migration.md",
            "Record the CI migration plan and its audit receipts",
        ),
        (
            "docs/ci/branch-protection-change.md",
            "Record the manual branch protection change",
        ),
    ];
    for (index, (path, message)) in expected_files.iter().enumerate() {
        let payload = read_json(&ci_audit.join(format!("setup-pr-file-payload-{index}.json")))?;
        assert_eq!(
            json_str_field(&payload, "message")?,
            *message,
            "{path} payload message"
        );
        assert_eq!(
            json_str_field(&payload, "branch")?,
            "ub-review/setup-ci-migration",
            "{path} payload branch"
        );
        let preview_bytes = fs::read(ci_audit.join("preview").join(path))
            .with_context(|| format!("read preview {path}"))?;
        assert_eq!(
            json_str_field(&payload, "content")?,
            base64_standard_for_test(&preview_bytes),
            "{path} payload must match the no-network preview bytes"
        );
    }
    for relative in [
        ".ub-review.toml",
        ".github/workflows/ub-review-gate.yml",
        "docs/ci/ub-review-migration.md",
        "docs/ci/branch-protection-change.md",
    ] {
        assert!(
            !temp.path().join(relative).exists(),
            "setup-ci --open-pr must not write generated repo files locally as `{relative}`"
        );
    }

    Ok(())
}

#[test]
fn audit_ci_tokenless_cli_writes_receipts_without_repo_side_effects() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let repo = temp.path().join("repo");
    write_file(
        &repo.join(".github/workflows/ci.yml"),
        r#"name: CI

on:
  pull_request:
    paths:
      - "src/**"
      - "Cargo.toml"

permissions: read-all

jobs:
  fmt:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    permissions:
      contents: read
    steps:
      - run: cargo fmt --check
  integration:
    runs-on: ubuntu-latest
    timeout-minutes: 30
    steps:
      - uses: actions/checkout@v5
      - run: cargo test --workspace
  deploy:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      id-token: write
    steps:
      - run: ./scripts/deploy.sh
"#,
    )
    .context("write tokenless audit-ci workflow fixture")?;
    let out = temp.path().join("target/ub-review");
    let out_arg = path_str(&out)?;
    let root_arg = path_str(&repo)?;
    let mut command = isolated_command(bin, temp.path());
    command
        .env_remove("GITHUB_REPOSITORY")
        .env_remove("GITHUB_TOKEN")
        .args([
            "audit-ci",
            "--root",
            root_arg,
            "--out",
            out_arg,
            "--repo",
            "acme/widgets",
            "--window-days",
            "45",
        ]);
    let output = command.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "audit-ci failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("audit-ci: wrote"),
        "audit-ci should report the output directory:\n{stdout}"
    );
    assert!(
        stdout.contains("(3 jobs, inventory-only mode)"),
        "tokenless audit-ci should stay in inventory-only mode:\n{stdout}"
    );
    assert!(
        stderr.trim().is_empty(),
        "tokenless audit-ci should not write stderr noise:\n{stderr}"
    );

    let ci_audit = out.join("ci-audit");
    let files = collect_relative_file_paths(&ci_audit)?;
    assert_eq!(
        files,
        vec![
            "audit-report.md",
            "correlation.json",
            "costs.json",
            "history.json",
            "inventory.json",
            "recommendations.json",
            "runner-cancellations.json",
        ],
        "audit-ci should write only its read-only receipt set"
    );

    let inventory = read_json(&ci_audit.join("inventory.json"))?;
    assert_eq!(inventory["schema"], "ub-review.ci_inventory.v1");
    assert_eq!(inventory["repo"], "acme/widgets");
    assert_eq!(inventory["window_days"], 45);
    let inventory_jobs = json_array_field(&inventory, "jobs")?;
    assert_eq!(inventory_jobs.len(), 3);
    let inventory_job_names = inventory_jobs
        .iter()
        .map(|job| json_str_field(job, "job").map(ToOwned::to_owned))
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(inventory_job_names, vec!["deploy", "fmt", "integration"]);
    let deploy = inventory_jobs
        .iter()
        .find(|job| job["job"].as_str() == Some("deploy"))
        .context("deploy inventory job")?;
    assert_eq!(
        deploy["permissions"],
        serde_json::json!({"contents": "write", "id-token": "write"})
    );
    let fmt = inventory_jobs
        .iter()
        .find(|job| job["job"].as_str() == Some("fmt"))
        .context("fmt inventory job")?;
    assert_eq!(fmt["timeout_minutes"], 10);
    assert_eq!(fmt["required_check"], serde_json::Value::Null);
    assert_eq!(fmt["required_check_source"], "unknown");
    assert!(
        inventory["evidence_gaps"]
            .as_array()
            .is_some_and(|gaps| gaps.iter().any(|gap| gap
                .as_str()
                .is_some_and(|gap| gap.contains("no GitHub token")))),
        "inventory should receipt tokenless history limitations: {inventory:#?}"
    );

    let history = read_json(&ci_audit.join("history.json"))?;
    assert_eq!(history["schema"], "ub-review.ci_history.v1");
    assert_eq!(history["runs_fetched"], 0);
    assert_eq!(history["pages_fetched"], 0);
    assert!(json_array_field(&history, "jobs")?.is_empty());
    let costs = read_json(&ci_audit.join("costs.json"))?;
    assert_eq!(costs["schema"], "ub-review.ci_costs.v1");
    assert!(json_array_field(&costs, "jobs")?.is_empty());
    let correlation = read_json(&ci_audit.join("correlation.json"))?;
    assert_eq!(correlation["schema"], "ub-review.ci_correlation.v1");
    assert!(
        correlation["independent_failure_rule"]
            .as_str()
            .is_some_and(|rule| rule.contains("independent failure"))
    );
    assert!(json_array_field(&correlation, "jobs")?.is_empty());

    let recommendations = read_json(&ci_audit.join("recommendations.json"))?;
    assert_eq!(recommendations["schema"], "ub-review.ci_recommendations.v1");
    let recommendation_jobs = json_array_field(&recommendations, "jobs")?;
    assert_eq!(recommendation_jobs.len(), 3);
    for recommendation in recommendation_jobs {
        assert_eq!(recommendation["tier"], "flag-for-human");
        assert_eq!(recommendation["judgment"], "deterministic");
        let expected_receipt = format!(
            "ci-audit/inventory.json#{}",
            json_str_field(recommendation, "job")?
        );
        assert_eq!(
            json_array_field(recommendation, "receipts")?
                .first()
                .and_then(serde_json::Value::as_str),
            Some(expected_receipt.as_str())
        );
    }

    let runner_cancellations = read_json(&ci_audit.join("runner-cancellations.json"))?;
    assert_eq!(
        runner_cancellations["schema"],
        "ub-review.ci_runner_cancellations.v1"
    );
    assert!(json_array_field(&runner_cancellations, "classifications")?.is_empty());
    assert!(
        runner_cancellations["evidence_gaps"]
            .as_array()
            .is_some_and(|gaps| gaps.iter().any(|gap| gap
                .as_str()
                .is_some_and(|gap| gap.contains("audit-log cancellation event count")))),
        "runner cancellation receipt should preserve the audit-log gap: {runner_cancellations:#?}"
    );

    let report = fs::read_to_string(ci_audit.join("audit-report.md"))?;
    for expected in [
        "# CI audit: acme/widgets",
        "Window: 45 days. Inventory-only: no GitHub token, no run history fetched.",
        "### Human review required",
        "`ci-audit/inventory.json#fmt`",
        "`ci-audit/inventory.json#integration`",
        "required-check status unknown",
        "no GitHub token",
    ] {
        assert!(
            report.contains(expected),
            "audit report missing `{expected}`:\n{report}"
        );
    }

    for relative in [
        ".ub-review.toml",
        ".github/workflows/ub-review-gate.yml",
        "docs/ci/ub-review-migration.md",
        "docs/ci/branch-protection-change.md",
    ] {
        assert!(
            !temp.path().join(relative).exists(),
            "audit-ci must not write migration files at repo root: {relative}"
        );
    }
    assert_eq!(
        collect_relative_file_paths(&repo)?,
        vec![".github/workflows/ci.yml"],
        "audit-ci must leave the audited repo unchanged"
    );
    for forbidden in [
        "preview",
        "setup-pr-result.json",
        "setup-pr-error.json",
        "setup-pr-branch-payload.json",
        "setup-pr-pull-payload.json",
    ] {
        assert!(
            !ci_audit.join(forbidden).exists(),
            "audit-ci must not write setup-ci/open-pr artifact `{forbidden}`"
        );
    }

    Ok(())
}

#[test]
fn adoption_docs_match_setup_ci_current_surface() {
    let docs = [
        (
            "SPEC-0007",
            include_str!("../docs/specs/UB-REVIEW-SPEC-0007-audit-ci.md"),
        ),
        (
            "SPEC-0008",
            include_str!("../docs/specs/UB-REVIEW-SPEC-0008-setup-ci.md"),
        ),
        (
            "CI_AUDIT_WIZARD",
            include_str!("../docs/CI_AUDIT_WIZARD.md"),
        ),
        (
            "ADR-0002",
            include_str!("../docs/adr/0002-single-gate-and-ci-audit-wizard.md"),
        ),
        ("ROADMAP", include_str!("../docs/ROADMAP.md")),
        (
            "SPEC-0001",
            include_str!("../docs/specs/UB-REVIEW-SPEC-0001-release-surface.md"),
        ),
        ("README", include_str!("../README.md")),
    ];
    for (name, text) in &docs {
        for stale in [
            "spec 0008, unimplemented",
            "the (future) `setup-ci` migration PR generator",
            "Honest answer today: no.",
            "Until it ships",
            "the PR you write yourself",
            "`setup-ci` ships only after",
            "three new files",
            "none of this exists",
            "Contract intent (nothing here is implemented)",
            "must not present setup-ci as available until the slices below land",
        ] {
            assert!(
                !text.contains(stale),
                "{name} leaked stale setup-ci adoption claim `{stale}`"
            );
        }
    }

    let spec_0007 = docs[0].1;
    assert!(spec_0007.contains("`setup-ci` migration PR generator"));
    assert!(spec_0007.contains("`--open-pr` opens the new-files-only migration PR"));

    let spec_0008 = docs[1].1;
    assert!(spec_0008.contains("Honest answer today: yes, within the v0 boundary."));
    assert!(spec_0008.contains("`setup-ci --open-pr` opens one new-files-only migration PR"));

    let wizard = docs[2].1;
    assert!(wizard.contains("`setup-ci --print-pr`"));
    assert!(wizard.contains("new-files-only `setup-ci --open-pr`"));
    assert!(wizard.contains("Never mutates branch protection itself."));

    let readme = docs[6].1;
    assert!(readme.contains("four new files"));

    for (name, text) in &docs {
        if text.contains("--apply-branch-protection") {
            let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                normalized.contains("not implemented in the current CLI"),
                "{name} mentions branch-protection mutation without current CLI boundary"
            );
            assert!(
                normalized.contains("not part of the adoption path"),
                "{name} mentions branch-protection mutation without adoption-path boundary"
            );
        }
    }
}

#[test]
fn handoff_docs_cover_current_product_gate_surfaces() {
    let handoff = include_str!("../docs/REPO_OPERATING_HANDOFF.md");
    let porting = include_str!("../docs/PORTING_BASELINE.md");

    for required in [
        "review/fill-ledger.json",
        "selected and skipped optional proof",
        "review/proof_receipts.json#<receipt-id>",
        "review/resource_leases.json#<lease-id>",
        "sensors/ripr/exposure-gaps.json",
        "setup-ci --print-pr",
        "setup-ci --open-pr",
        "new-files-only migration PR",
        "never mutates branch protection",
    ] {
        assert!(
            handoff.contains(required),
            "handoff must keep current product gate surface `{required}` visible"
        );
    }

    for required in [
        "Receipt routes must carry exact source anchors",
        "review/proof_receipts.json#<receipt-id>",
        "review/resource_leases.json#<lease-id>",
    ] {
        assert!(
            porting.contains(required),
            "porting baseline must keep receipt-route source anchor `{required}` visible"
        );
    }
}

#[test]
fn artifact_contract_docs_match_ci_audit_verifier_coverage() {
    let spec_0004 = include_str!("../docs/specs/UB-REVIEW-SPEC-0004-artifact-contract.md");
    let verifier = include_str!("../scripts/verify-bun-review-artifacts.py");

    assert!(
        verifier.contains("def require_ci_audit_core_artifacts"),
        "ci-audit core receipt verifier disappeared"
    );
    assert!(
        spec_0004.contains("require_ci_audit_core_artifacts"),
        "SPEC-0004 must name the executable ci-audit verifier"
    );
    assert!(
        spec_0004.contains("ci-audit/audit-report.md"),
        "SPEC-0004 must keep the human audit report separate from JSON receipts"
    );
    assert!(
        verifier.contains("def require_ci_audit_report"),
        "ci-audit report verifier disappeared"
    );
    assert!(
        spec_0004.contains("require_ci_audit_report"),
        "SPEC-0004 must name the executable ci-audit report verifier"
    );
    assert!(
        verifier.contains("def require_setup_ci_terminal_receipts"),
        "setup-ci terminal receipt verifier disappeared"
    );
    assert!(
        spec_0004.contains("require_setup_ci_terminal_receipts"),
        "SPEC-0004 must name the executable setup-ci terminal receipt verifier"
    );
    assert!(
        spec_0004.contains("ci-audit/setup-pr-result.json XOR setup-pr-error.json"),
        "SPEC-0004 must document setup-ci result/error as an XOR terminal receipt"
    );
    for stale in [
        "ci-audit/*                              audit-ci output; contract pending",
        "give ci-audit/* its own contract spec before anyone builds on it",
        "`ci-audit/*` has a contract yet",
        "ci-audit/* pending spec 0007",
    ] {
        assert!(
            !spec_0004.contains(stale),
            "SPEC-0004 leaked stale ci-audit contract claim `{stale}`"
        );
    }
}

#[test]
fn artifact_contract_docs_pin_receipt_route_source_anchors() {
    let spec_0004 = include_str!("../docs/specs/UB-REVIEW-SPEC-0004-artifact-contract.md");
    let verifier = include_str!("../scripts/verify-bun-review-artifacts.py");

    assert!(
        verifier.contains("def receipt_route_source_artifacts"),
        "receipt route source-anchor verifier disappeared"
    );
    assert!(
        verifier.contains("receipt route missing exact source anchors"),
        "receipt route self-test must fail old artifact-only route sources"
    );
    for required in [
        "review/proof_receipts.json#<receipt-id>",
        "review/resource_leases.json#<lease-id>",
        "route entries carry exact proof receipt and matching lease anchors",
    ] {
        assert!(
            spec_0004.contains(required),
            "SPEC-0004 must document receipt route anchor contract `{required}`"
        );
    }
}

#[test]
fn init_writes_file_driven_setup_guide_from_repo_scan() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let fixture_files = [
        (
            "Cargo.toml",
            "[package]\nname = \"init-guide-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        ),
        (
            "src/lib.rs",
            "#[no_mangle]\npub extern \"C\" fn exported() -> usize {\n    unsafe { 42 }\n}\n",
        ),
        (
            "tests/smoke.rs",
            "#[test]\nfn smoke() {\n    assert_eq!(2 + 2, 4);\n}\n",
        ),
        (
            ".github/workflows/ci.yml",
            "name: CI\non: [pull_request]\nconcurrency:\n  cancel-in-progress: true\njobs:\n  test:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n",
        ),
        (
            "docs/specs/adoption.md",
            "# Adoption contract\n\nThis repo keeps docs claims tied to executable behavior.\n",
        ),
        ("policy/allow.toml", "# owner receipt placeholder\n"),
    ];
    for (relative, contents) in fixture_files {
        write_file(&repo.join(relative), contents)?;
    }
    for (relative, contents) in fixture_files {
        let actual = fs::read_to_string(repo.join(relative))?;
        assert_eq!(
            actual, contents,
            "fixture file `{relative}` must be written exactly before init runs"
        );
    }

    let config = repo.join(".ub-review.toml");
    let guide = repo.join("ub-review-init.md");
    run(
        temp.path(),
        bin,
        &[
            "init",
            "--root",
            path_str(&repo)?,
            "--path",
            path_str(&config)?,
            "--guide-out",
            path_str(&guide)?,
        ],
    )?;

    let config_text = fs::read_to_string(&config)?;
    assert!(
        config_text.contains("profile = \"gh-runner\""),
        "init should still write the starter config:\n{config_text}"
    );
    let guide_text = fs::read_to_string(&guide)?;
    for expected in [
        "# ub-review init guide",
        "Rust package `init-guide-fixture`",
        ".github/workflows/ci.yml",
        "unsafe-review",
        "cargo-allow",
        "## File-driven config proposal",
        "cargo check --workspace --all-targets --locked",
        "Do not materialize any candidate with `setup-ci --accept`",
        "`tests`: review oracle strength",
        "`gate-semantics`",
        "`spec-honesty`",
        "ub-review doctor --config",
        "--require-core-tools",
        "before trusting the standard gate image",
        "ub-review audit-ci --root",
        "ub-review setup-ci --print-pr",
        "--accept <job>=<command>",
        "only audited `adaptive` and `move-to-ub-review-required` recommendations can become generated proof",
        "`move-to-ub-review-required` materializes as required proof",
        "`adaptive` materializes as non-required proof",
        "`keep-required`, `flag-for-human`, risk-pack, nightly, release, deploy, provenance, and compliance jobs remain manual",
        "Branch protection",
    ] {
        assert!(
            guide_text.contains(expected),
            "init guide missing `{expected}`:\n{guide_text}"
        );
    }
    assert!(
        !guide_text.contains("## Audit-ci receipt summary"),
        "fresh init without audit receipts should not add an empty audit summary:\n{guide_text}"
    );

    let failure = run_expect_failure(
        temp.path(),
        bin,
        &[
            "init",
            "--root",
            path_str(&repo)?,
            "--path",
            path_str(&config)?,
            "--guide-out",
            path_str(&guide)?,
        ],
    )?;
    assert!(
        failure.contains("already exists; pass --force"),
        "init must refuse to overwrite config or guide without --force:\n{failure}"
    );

    let collision_result = run_capture_with_env(
        &repo,
        bin,
        &[
            "init",
            "--path",
            "collision.md",
            "--guide-out",
            "./collision.md",
        ],
        &[],
    );
    let collision_failure = match collision_result {
        Ok(output) => {
            bail!("init unexpectedly allowed normalized output path collision:\n{output}")
        }
        Err(error) => error.to_string(),
    };
    assert!(
        collision_failure.contains("--path and --guide-out must name different files"),
        "init must reject normalized output path collisions:\n{collision_failure}"
    );
    assert!(
        !repo.join("collision.md").exists(),
        "collision preflight must not write either output"
    );
    Ok(())
}

#[test]
fn init_guide_summarizes_existing_audit_ci_receipts() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    write_init_audit_ci_fixture(&repo.join("target/ub-review/ci-audit"))?;

    let config = repo.join(".ub-review.toml");
    let guide = repo.join("ub-review-init.md");
    run(
        temp.path(),
        bin,
        &[
            "init",
            "--root",
            path_str(&repo)?,
            "--path",
            path_str(&config)?,
            "--guide-out",
            path_str(&guide)?,
        ],
    )?;

    let config_text = fs::read_to_string(&config)?;
    for forbidden in [
        "ci-audit",
        "integration",
        "unit",
        "move-to-ub-review-required",
        "[[proof.required]]",
    ] {
        assert!(
            !config_text.contains(forbidden),
            "init must not materialize audit recommendations into starter config (`{forbidden}` leaked):\n{config_text}"
        );
    }

    let guide_text = fs::read_to_string(&guide)?;
    for expected in [
        "## Audit-ci receipt summary",
        "Existing audit-ci receipts: `target/ub-review/ci-audit`.",
        "Inventory: 4 jobs for `acme/widgets` over 90 days.",
        "Recommendations: 4 jobs for `acme/widgets` over 90 days.",
        "Right-size to adaptive (`adaptive`):",
        "`integration` from `.github/workflows/ci.yml` - expensive and quiet on unrelated diffs. receipts: `ci-audit/correlation.json#integration`",
        "Move into ub-review/gate (`move-to-ub-review-required`):",
        "`unit` from `.github/workflows/ci.yml`",
        "Keep required (`keep-required`):",
        "Human review required (`flag-for-human`):",
        "Inventory evidence gap: required checks unreadable from tokenless audit.",
        "Recommendation evidence gap: history window truncated.",
        "Acceptable setup-ci candidates (commands still maintainer-supplied):",
        "`integration` (`adaptive`): add `--accept integration=\"<maintainer command>\"` after running the command locally; receipts: `ci-audit/correlation.json#integration`",
        "`unit` (`move-to-ub-review-required`): add `--accept unit=\"<maintainer command>\"` after running the command locally; receipts: `ci-audit/correlation.json#unit`",
        "Leave `keep-required`, `flag-for-human`, risk-pack, nightly, release, deploy, provenance, and compliance jobs manual",
        "Setup boundary: audit receipts do not record runnable commands",
        "explicit `setup-ci --accept <job>=<command>`",
        "only for audited `adaptive` or `move-to-ub-review-required` jobs",
        "## Model-assisted config proposal input",
        "Bounded deterministic inputs: `target/ub-review/ci-audit/inventory.json`, `target/ub-review/ci-audit/recommendations.json`, and `target/ub-review/ci-audit/audit-report.md` when present and readable.",
        "Human audit report: `target/ub-review/ci-audit/audit-report.md` pairs tier summaries with backticked recommendation receipt pointers.",
        "Use recommendation receipts as pointers to supporting audit artifacts; do not infer from workflow names alone.",
        "Setup-ci accepts: 2 audited `adaptive` or `move-to-ub-review-required` jobs may be proposed only with maintainer-supplied commands.",
        "Manual boundary: keep-required, flag-for-human, risk-pack, nightly, release, deploy, provenance, and compliance jobs stay manual unless later receipts retier them.",
        "Evidence gaps: convert unresolved recommendation gaps into verification questions, not config changes.",
        "Proposal boundary: a model or external agent may draft rationale and open questions from these receipts, but must not invent commands, treat model judgment as proof, enable posting/blocking, or mutate branch protection.",
    ] {
        assert!(
            guide_text.contains(expected),
            "init guide missing `{expected}`:\n{guide_text}"
        );
    }
    assert!(
        !guide_text.contains("integration=cargo test"),
        "init must not invent runnable setup-ci --accept commands:\n{guide_text}"
    );
    for forbidden in [
        "--accept fmt=\"<maintainer command>\"",
        "--accept deploy=\"<maintainer command>\"",
    ] {
        assert!(
            !guide_text.contains(forbidden),
            "init guide must not make manual-tier jobs acceptable (`{forbidden}` leaked):\n{guide_text}"
        );
    }
    Ok(())
}

#[test]
fn init_guide_flags_bad_existing_audit_ci_receipts_without_failing() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    write_bad_init_audit_ci_fixture(&repo.join("target/ub-review/ci-audit"));

    let config = repo.join(".ub-review.toml");
    let guide = repo.join("ub-review-init.md");
    run(
        temp.path(),
        bin,
        &[
            "init",
            "--root",
            path_str(&repo)?,
            "--path",
            path_str(&config)?,
            "--guide-out",
            path_str(&guide)?,
        ],
    )?;

    let guide_text = fs::read_to_string(&guide)?;
    for expected in [
        "## Audit-ci receipt summary",
        "Inventory: unavailable; rerun `ub-review audit-ci --out target/ub-review` before setup-ci materialization.",
        "Recommendations: unavailable; rerun `ub-review audit-ci --out target/ub-review` before setup-ci materialization.",
        "Audit receipt evidence gap: `target/ub-review/ci-audit/inventory.json` unreadable:",
        "expected ub-review.ci_inventory.v1",
        "`target/ub-review/ci-audit/recommendations.json` missing; rerun `ub-review audit-ci --out target/ub-review`",
        "`target/ub-review/ci-audit/audit-report.md` missing; rerun `ub-review audit-ci --out target/ub-review`",
        "## Model-assisted config proposal input",
        "Recommendations are unavailable; rerun `ub-review audit-ci --out target/ub-review` before asking a model or external agent to propose setup-ci accepts.",
        "Human audit report: unavailable; rerun `ub-review audit-ci --out target/ub-review` before asking a model or external agent to propose setup-ci accepts.",
        "Receipt gaps: treat missing or unreadable audit receipts as blockers for materialization.",
    ] {
        assert!(
            guide_text.contains(expected),
            "init guide missing `{expected}`:\n{guide_text}"
        );
    }
    Ok(())
}

#[test]
fn init_guide_lists_existing_package_scripts_as_proof_candidates() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let bin = env!("CARGO_BIN_EXE_ub-review");
    write_file(
        &repo.join("package.json"),
        &serde_json::to_string_pretty(&serde_json::json!({
            "name": "init-js-fixture",
            "scripts": {
                "test": "vitest run",
                "lint": "eslint .",
                "typecheck": "tsc --noEmit",
                "build": "vite build",
                "dev": "vite --host 0.0.0.0",
                "pretest": "node scripts/prepare-fixtures.js",
                "deploy": "wrangler deploy",
                "docker:build": "docker build .",
                "lint;rm": "eslint ."
            }
        }))?,
    )?;
    write_file(&repo.join("pnpm-lock.yaml"), "# lockfile fixture\n")?;
    write_file(&repo.join("src/index.ts"), "export const value = 42;\n")?;

    let config = repo.join(".ub-review.toml");
    let guide = repo.join("ub-review-init.md");
    run(
        temp.path(),
        bin,
        &[
            "init",
            "--root",
            path_str(&repo)?,
            "--path",
            path_str(&config)?,
            "--guide-out",
            path_str(&guide)?,
        ],
    )?;

    let guide_text = fs::read_to_string(&guide)?;
    for expected in [
        "JavaScript/TypeScript (`package.json`)",
        "JavaScript/TypeScript package scripts detected in `package.json`",
        "`test`: `pnpm run test` (script: `vitest run`).",
        "`lint`: `pnpm run lint` (script: `eslint .`).",
        "`typecheck`: `pnpm run typecheck` (script: `tsc --noEmit`).",
        "`build`: `pnpm run build` (script: `vite build`).",
        "No root Cargo manifest detected",
    ] {
        assert!(
            guide_text.contains(expected),
            "init guide missing `{expected}`:\n{guide_text}"
        );
    }
    for unexpected in [
        "pnpm run dev",
        "pnpm run pretest",
        "pnpm run deploy",
        "pnpm run docker:build",
        "pnpm run lint;rm",
        "cargo check --workspace",
    ] {
        assert!(
            !guide_text.contains(unexpected),
            "init guide invented or over-selected `{unexpected}`:\n{guide_text}"
        );
    }
    Ok(())
}

#[test]
fn quality_backfill_cli_writes_rolling_artifact_with_source_receipts() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let run_a = temp.path().join("run-a");
    let run_b = temp.path().join("run-b");
    for (dir, run_id, comments, fills_with_signal, fills_total, llm_events) in [
        (&run_a, "run-a", 2_u64, 1_u64, 2_u64, 0_u64),
        (&run_b, "run-b", 1_u64, 1_u64, 1_u64, 1_u64),
    ] {
        let review_dir = dir.join("review");
        fs::create_dir_all(&review_dir)?;
        fs::write(
            review_dir.join("quality-receipt.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "ub-review.quality_receipt.v1",
                "run_id": run_id,
                "comments_prepared": comments,
                "fills_with_signal": fills_with_signal,
                "fills_total": fills_total,
                "llm_unavailable_events": llm_events
            }))?,
        )?;
        fs::write(
            review_dir.join("quality-trend.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "ub-review.quality_trend.v1"
            }))?,
        )?;
    }
    let gh_dir = temp.path().join("github");
    fs::create_dir_all(&gh_dir)?;
    fs::write(
        gh_dir.join("review-threads.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "query": "reviewThreads",
            "nodes": []
        }))?,
    )?;
    let outcomes = gh_dir.join("github-quality-outcomes.json");
    fs::write(
        &outcomes,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "ub-review.github_quality_outcomes.v1",
            "source_artifacts": ["review-threads.json"],
            "comments": [
                {"posted": true, "accepted": true, "resolved": true, "reviewer_override": false},
                {"posted": true, "accepted": false, "resolved": true, "reviewer_override": true}
            ],
            "adopted_generated_tests": [{"path": "tests/generated.rs"}]
        }))?,
    )?;
    let previous = temp.path().join("previous-quality-backfill.json");
    fs::write(
        &previous,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "ub-review.quality_backfill.v1",
            "comment_acceptance_rate": 0.25,
            "fills_signal_rate": 0.5,
            "llm_unavailable_rate": 0.25,
            "reviewer_override_rate": 0.0
        }))?,
    )?;

    let out = temp.path().join("out");
    let output = Command::new(bin)
        .arg("quality-backfill")
        .arg("--out")
        .arg(&out)
        .arg("--run-dir")
        .arg(&run_a)
        .arg("--run-dir")
        .arg(&run_b)
        .arg("--github-outcomes")
        .arg(&outcomes)
        .arg("--previous")
        .arg(&previous)
        .arg("--window-days")
        .arg("30")
        .output()?;
    assert!(
        output.status.success(),
        "quality-backfill failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/quality-backfill.json"))?)?;

    assert_eq!(artifact["schema"], "ub-review.quality_backfill.v1");
    assert_eq!(artifact["window_scope"], "rolling_v1");
    assert_eq!(artifact["window_runs"], 2);
    assert_eq!(artifact["comments_prepared"], 3);
    assert_eq!(artifact["comments_posted"], 2);
    assert_eq!(artifact["comments_accepted"], 1);
    assert_eq!(artifact["comments_resolved"], 2);
    assert_eq!(artifact["comment_acceptance_rate"], 0.5);
    assert_eq!(artifact["comment_resolution_rate"], 1.0);
    assert_eq!(artifact["reviewer_overrides"], 1);
    assert_eq!(artifact["reviewer_override_rate"], 0.5);
    assert_eq!(artifact["adopted_generated_tests"], 1);
    assert_eq!(artifact["trend"]["comment_acceptance_rate_delta"], 0.25);
    assert_eq!(artifact["trend"]["llm_unavailable_rate_delta"], 0.25);
    let sources = artifact["source_artifacts"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("source_artifacts is not an array"))?;
    assert!(
        sources.len() >= 7,
        "expected run, GitHub, raw, and previous sources"
    );
    for source in sources {
        let source = source
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("source_artifacts entry is not a string"))?;
        assert!(
            out.join(source).is_file(),
            "source artifact was not copied into the backfill tree: {source}"
        );
    }
    Ok(())
}

#[test]
fn quality_backfill_cli_records_missing_trend_without_failing() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let run = temp.path().join("run-without-trend");
    let review_dir = run.join("review");
    fs::create_dir_all(&review_dir)?;
    fs::write(
        review_dir.join("quality-receipt.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "ub-review.quality_receipt.v1",
            "run_id": "run-without-trend",
            "comments_prepared": 1,
            "fills_with_signal": 0,
            "fills_total": 0,
            "llm_unavailable_events": 0
        }))?,
    )?;

    let out = temp.path().join("out");
    let output = Command::new(bin)
        .arg("quality-backfill")
        .arg("--out")
        .arg(&out)
        .arg("--run-dir")
        .arg(&run)
        .arg("--window-days")
        .arg("30")
        .output()?;
    assert!(
        output.status.success(),
        "quality-backfill should keep receipt-only historical runs\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let artifact: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/quality-backfill.json"))?)?;
    assert_eq!(artifact["window_runs"], 1);
    assert_eq!(artifact["comments_prepared"], 1);
    let source_artifacts = artifact["source_artifacts"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("source_artifacts is not an array"))?;
    assert_eq!(
        source_artifacts.len(),
        1,
        "missing trend provenance must not synthesize a source artifact"
    );
    let copied_receipt = source_artifacts[0]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("source_artifacts entry is not a string"))?;
    assert!(
        out.join(copied_receipt).is_file(),
        "run receipt source should still be copied: {copied_receipt}"
    );
    let missing = artifact["missing"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("missing[] is not an array"))?;
    assert!(
        missing.iter().any(|entry| {
            entry["field"] == "source_artifacts.quality_trend"
                && entry["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("run-without-trend"))
                && entry["source_artifact"] == copied_receipt
        }),
        "quality-backfill should receipt missing trend provenance: {missing:#?}"
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
        "review/ub-review-cost.json",
        "review/floor-trend.json",
        "review/fill-ledger.json",
        "review/quality-receipt.json",
        "review/quality-trend.json",
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
        "review/prior_resolved_candidates.json",
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
    let ripr_status = tool_status["tools"]
        .as_array()
        .and_then(|tools| tools.iter().find(|tool| tool["id"] == "ripr"))
        .ok_or_else(|| anyhow::anyhow!("ripr tool status missing"))?;
    assert!(ripr_status["timeout_sec"].as_u64().is_some());
    assert!(ripr_status["artifact_budget_mb"].as_u64().is_some());
    assert!(ripr_status["requires_lease"].as_bool().is_some());
    assert_eq!(
        ripr_status["status"], "skipped",
        "dry-run must not execute ripr: {ripr_status}"
    );
    if ripr_status["planned_run"].as_bool() == Some(true) {
        assert_eq!(ripr_status["reason"], "dry-run; sensor not executed");
    } else {
        assert!(
            ripr_status["reason"]
                .as_str()
                .is_some_and(|reason| !reason.is_empty()),
            "unplanned dry-run ripr status must explain why it skipped: {ripr_status}"
        );
    }
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
    let cost: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/ub-review-cost.json"))?)?;
    assert_eq!(cost["schema"], "ub-review.cost_receipt.v1");
    assert!(cost.get("suggested_fill_seconds").is_none());
    assert!(
        ["github-hosted", "self-hosted", "local", "unknown"]
            .contains(&cost["runner_kind"].as_str().unwrap_or_default())
    );
    assert_eq!(cost["target_minutes"], 30);
    assert_eq!(cost["cap_minutes"], 60);
    assert!(cost["fallback_used"].as_bool().is_some());
    assert!(cost["required_floor_wall_seconds"].is_null());
    assert!(cost["estimated_cost_usd"].is_null());
    assert_eq!(
        cost["llm_seconds"].as_f64().unwrap_or_default(),
        metrics["run"]["model_call_duration_ms_sum"]
            .as_u64()
            .unwrap_or_default() as f64
            / 1000.0
    );
    assert_eq!(
        cost["tokens"]["cached_input"],
        metrics["models"]["prompt_cache_read_input_tokens"]
    );
    for pointer in [
        "/tokens/fresh_input",
        "/tokens/cached_input",
        "/tokens/output",
        "/cost_basis/runner_minutes",
    ] {
        assert!(
            cost.pointer(pointer)
                .and_then(serde_json::Value::as_f64)
                .is_some(),
            "missing non-negative cost field {pointer}"
        );
    }
    let missing_cost_fields = cost["missing"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("cost missing[] absent"))?
        .iter()
        .filter_map(|entry| entry["field"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(missing_cost_fields.contains("required_floor_wall_seconds"));
    assert!(missing_cost_fields.contains("cost_basis.linux_minute_rate_usd"));
    assert!(missing_cost_fields.contains("estimated_cost_usd"));
    assert!(missing_cost_fields.contains("cache.cargo"));
    let floor_trend: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/floor-trend.json"))?)?;
    assert_eq!(floor_trend["schema"], "ub-review.floor_trend.v1");
    assert_eq!(floor_trend["run_id"], cost["run_id"]);
    assert_eq!(floor_trend["window_scope"], "single_run_v1");
    assert_eq!(floor_trend["window_runs"], 1);
    let as_of = floor_trend["as_of"].as_str().unwrap_or_default();
    assert_eq!(as_of.len(), "2026-06-12".len());
    assert_eq!(as_of.chars().nth(4), Some('-'));
    assert_eq!(as_of.chars().nth(7), Some('-'));
    let floor_releases = floor_trend["releases"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("floor-trend releases[] absent"))?;
    assert_eq!(floor_releases.len(), 1);
    assert_eq!(floor_releases[0]["sample_runs"], 1);
    assert_eq!(
        floor_releases[0]["fallback_used_rate"],
        if cost["fallback_used"] == true {
            1.0
        } else {
            0.0
        }
    );
    assert!(floor_trend["trend"]["floor_creep_detected"].is_null());
    assert!(floor_trend["trend"]["cache_hit_rate_delta"].is_null());
    assert!(floor_trend["trend"]["avg_cost_delta_usd"].is_null());
    let missing_floor_fields = floor_trend["missing"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("floor-trend missing[] absent"))?
        .iter()
        .filter_map(|entry| entry["field"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    assert!(missing_floor_fields.contains("trend.floor_creep_detected"));
    assert!(missing_floor_fields.contains("trend.cache_hit_rate_delta"));
    assert!(missing_floor_fields.contains("trend.avg_cost_delta_usd"));
    let fill_ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/fill-ledger.json"))?)?;
    assert_eq!(fill_ledger["schema"], "ub-review.fill_ledger.v1");
    assert_eq!(fill_ledger["catalog_scope"], "executed_work_queue_v1");
    assert_eq!(fill_ledger["run_id"], cost["run_id"]);
    let fill_entries = fill_ledger["entries"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("fill-ledger entries[] absent"))?;
    assert!(
        fill_entries.iter().any(|entry| {
            entry["kind"] == "sensor"
                && entry["selected"].as_bool().is_some()
                && entry["selection_reason"]
                    .as_str()
                    .is_some_and(|reason| !reason.is_empty())
                && entry["cost"].as_str().is_some_and(|cost| !cost.is_empty())
                && entry["time_spent_sec"].as_f64().is_some()
        }),
        "fill ledger should record optional sensor selection receipts"
    );
    assert!(
        fill_entries
            .iter()
            .any(|entry| entry["kind"] == "proof-skip" && entry["selected"] == false),
        "fill ledger should preserve deterministic skipped proof options"
    );
    for (check_id, expected_signal) in [
        (
            "mutation",
            "runtime mutation check for targeted test oracle strength",
        ),
        (
            "sanitizer",
            "sanitizer runtime witness for memory-safety regressions",
        ),
    ] {
        assert!(
            fill_entries.iter().any(|entry| {
                entry["check_id"] == check_id
                    && entry["kind"] == "proof-skip"
                    && entry["selected"] == false
                    && entry["cost"] == check_id
                    && entry["expected_signal"] == expected_signal
                    && entry["selection_reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("does not lease"))
            }),
            "fill ledger should explain skipped {check_id} heavy witness"
        );
    }
    let proof_planner_output: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_planner_output.json"))?)?;
    let planner_skips = proof_planner_output["skip"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("proof planner skip[] absent"))?;
    for check_id in ["mutation", "sanitizer"] {
        assert!(
            planner_skips.iter().any(|skip| {
                skip["kind"] == check_id
                    && skip["reason"]
                        .as_str()
                        .is_some_and(|reason| reason.contains("risk-pack/manual-heavy profile"))
            }),
            "proof planner should receipt skipped {check_id} heavy witness"
        );
    }
    let quality: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/quality-receipt.json"))?)?;
    assert_eq!(quality["schema"], "ub-review.quality_receipt.v1");
    assert_eq!(quality["run_id"], cost["run_id"]);
    assert_eq!(quality["run_id"], fill_ledger["run_id"]);
    assert_eq!(
        quality["review_payload_status"],
        metrics["review_payload_status"]
    );
    assert_eq!(
        quality["comments_prepared"],
        metrics["github_review_comments"]
    );
    assert!(quality["comments_posted"].is_null());
    assert!(quality["comments_accepted"].is_null());
    assert!(quality["comments_resolved"].is_null());
    assert!(quality["reviewer_overrides"].is_null());
    assert!(quality["adopted_generated_tests"].is_null());
    assert_eq!(
        quality["comments_off_diff_rejected"],
        metrics["off_diff_candidates_rejected"]
    );
    let selected_fill_count = fill_entries
        .iter()
        .filter(|entry| entry["selected"] == true)
        .count() as u64;
    let selected_signal_count = fill_entries
        .iter()
        .filter(|entry| {
            entry["selected"] == true
                && entry["actual_signal"]
                    .as_str()
                    .is_some_and(|signal| !signal.trim().is_empty())
        })
        .count() as u64;
    assert_eq!(quality["fills_total"], selected_fill_count);
    assert_eq!(quality["fills_with_signal"], selected_signal_count);
    assert_eq!(
        quality["fallback_used_lanes"],
        metrics["models"]["model_fallbacks_used"]
    );
    let missing_quality_fields = quality["missing"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("quality missing[] absent"))?
        .iter()
        .filter_map(|entry| entry["field"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for field in [
        "comments_posted",
        "comments_accepted",
        "comments_resolved",
        "reviewer_overrides",
        "adopted_generated_tests",
    ] {
        assert!(
            missing_quality_fields.contains(field),
            "quality receipt should mark {field} unavailable in run-completion v1"
        );
    }
    let quality_trend: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/quality-trend.json"))?)?;
    assert_eq!(quality_trend["schema"], "ub-review.quality_trend.v1");
    assert_eq!(quality_trend["run_id"], quality["run_id"]);
    assert_eq!(quality_trend["window_scope"], "single_run_v1");
    assert_eq!(quality_trend["window_runs"], 1);
    assert_eq!(
        quality_trend["comments_prepared"],
        quality["comments_prepared"]
    );
    assert!(quality_trend["comments_posted"].is_null());
    assert!(quality_trend["comment_acceptance_rate"].is_null());
    assert!(quality_trend["comment_resolution_rate"].is_null());
    assert!(quality_trend["reviewer_override_rate"].is_null());
    assert!(quality_trend["adopted_generated_tests"].is_null());
    if selected_fill_count == 0 {
        assert!(quality_trend["fills_signal_rate"].is_null());
    } else {
        assert_eq!(
            quality_trend["fills_signal_rate"]
                .as_f64()
                .unwrap_or_default(),
            selected_signal_count as f64 / selected_fill_count as f64
        );
    }
    assert_eq!(
        quality_trend["llm_unavailable_rate"],
        if quality["llm_unavailable_events"]
            .as_u64()
            .unwrap_or_default()
            > 0
        {
            1.0
        } else {
            0.0
        }
    );
    assert!(quality_trend["trend"]["comment_acceptance_rate_delta"].is_null());
    assert!(quality_trend["trend"]["fills_signal_rate_delta"].is_null());
    assert!(quality_trend["trend"]["llm_unavailable_rate_delta"].is_null());
    assert!(quality_trend["trend"]["reviewer_override_rate_delta"].is_null());
    let missing_quality_trend_fields = quality_trend["missing"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("quality-trend missing[] absent"))?
        .iter()
        .filter_map(|entry| entry["field"].as_str())
        .collect::<std::collections::BTreeSet<_>>();
    for field in [
        "comments_posted",
        "comment_acceptance_rate",
        "comment_resolution_rate",
        "reviewer_override_rate",
        "adopted_generated_tests",
        "trend.comment_acceptance_rate_delta",
        "trend.fills_signal_rate_delta",
        "trend.llm_unavailable_rate_delta",
        "trend.reviewer_override_rate_delta",
    ] {
        assert!(
            missing_quality_trend_fields.contains(field),
            "quality trend should mark {field} unavailable in single_run_v1"
        );
    }
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
    assert_eq!(request["status"], "deferred");
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
    assert!(output.contains("Install status:"));
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
fn doctor_reports_advisory_missing_tool_fix_without_requiring_core_tools() -> Result<()> {
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
    let output = run_capture_with_env(
        temp.path(),
        bin,
        &["doctor", "--config", path_str(&config)?],
        &[],
    )?;

    let tokmd_row = output
        .lines()
        .find(|line| line.trim_start().starts_with("tokmd "))
        .context("doctor output missing tokmd tool row")?;
    assert!(tokmd_row.contains("missing"), "{tokmd_row}");
    assert!(tokmd_row.contains("expected=1.12.0"), "{tokmd_row}");
    assert!(output.contains("Fixes:"), "{output}");
    assert!(
        output.contains("tokmd missing: cargo install tokmd --locked --version 1.12.0 --force"),
        "{output}"
    );
    assert!(!output.contains("required core review tools missing from standard image"));
    Ok(())
}

#[test]
fn doctor_reports_advisory_stale_tool_fix_without_requiring_core_tools() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let fake_bin = temp.path().join("fake-bin");
    write_fake_core_review_tools_with_versions(
        &fake_bin,
        &[
            ("tokmd", "1.12.0"),
            ("cargo-allow", "0.1.8"),
            ("ripr", "0.7.9"),
            ("unsafe-review", "0.3.4"),
            ("ast-grep", "0.0.0"),
            ("actionlint", "1.7.12"),
        ],
    )?;
    let path = prepend_to_path(&fake_bin)?;
    let config = temp.path().join(".ub-review.toml");
    write_file(&config, r#"profile = "gh-runner""#)?;

    let bin = env!("CARGO_BIN_EXE_ub-review");
    let output = run_capture_with_env(
        temp.path(),
        bin,
        &["doctor", "--config", path_str(&config)?],
        &[("PATH", path.as_str())],
    )?;

    let ripr_row = output
        .lines()
        .find(|line| line.trim_start().starts_with("ripr "))
        .context("doctor output missing ripr tool row")?;
    assert!(ripr_row.contains("version=ripr 0.7.9"), "{ripr_row}");
    assert!(ripr_row.contains("expected=0.10.0"), "{ripr_row}");
    assert!(output.contains("Fixes:"), "{output}");
    assert!(
        output.contains("ripr version drift: cargo install ripr --locked --version 0.10.0 --force"),
        "{output}"
    );
    assert!(!output.contains("required core review tool versions drifted"));
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
fn doctor_require_core_tools_fails_stale_cargo_allow_version() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let fake_bin = temp.path().join("fake-bin");
    let fake_tools_written = write_fake_core_review_tools_with_versions(
        &fake_bin,
        &[
            ("tokmd", "1.12.0"),
            ("cargo-allow", "0.1.7"),
            ("ripr", "0.10.0"),
            ("unsafe-review", "0.3.4"),
            ("ast-grep", "0.0.0"),
            ("actionlint", "1.7.12"),
        ],
    );
    assert!(
        fake_tools_written.is_ok(),
        "write stale cargo-allow fake core review tools: {fake_tools_written:?}"
    );
    assert_fake_core_review_tool_version(&fake_bin, "cargo-allow", "0.1.7")?;
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
    assert!(output.contains("cargo-allow expected 0.1.8"));
    assert!(output.contains("cargo-allow 0.1.7"));
    assert!(output.contains("Fixes:"));
    assert!(output.contains(
        "cargo-allow version drift: cargo install cargo-allow --locked --version 0.1.8 --force"
    ));
    assert!(output.contains("see Fixes above"));
    Ok(())
}

#[test]
fn doctor_require_core_tools_fails_stale_actionlint_version() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let fake_bin = temp.path().join("fake-bin");
    let fake_tools_written = write_fake_core_review_tools_with_versions(
        &fake_bin,
        &[
            ("tokmd", "1.12.0"),
            ("cargo-allow", "0.1.8"),
            ("ripr", "0.10.0"),
            ("unsafe-review", "0.3.4"),
            ("ast-grep", "0.0.0"),
            ("actionlint", "1.7.0"),
        ],
    );
    assert!(
        fake_tools_written.is_ok(),
        "write stale actionlint fake core review tools: {fake_tools_written:?}"
    );
    assert_fake_core_review_tool_version(&fake_bin, "actionlint", "1.7.0")?;
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
    assert!(output.contains("actionlint expected 1.7.12"));
    assert!(output.contains("actionlint 1.7.0"));
    assert!(output.contains("Fixes:"));
    assert!(output.contains(
        "actionlint version drift: go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.12; add $(go env GOPATH)/bin to PATH"
    ));
    assert!(output.contains("see Fixes above"));
    Ok(())
}

#[test]
fn fake_core_review_tools_with_versions_emit_requested_versions() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let fake_bin = temp.path().join("fake-bin");
    let fake_tools_written = write_fake_core_review_tools_with_versions(
        &fake_bin,
        &[
            ("tokmd", "9.9.1"),
            ("cargo-allow", "9.9.2"),
            ("ripr", "9.9.3"),
            ("unsafe-review", "9.9.4"),
            ("ast-grep", "9.9.5"),
            ("actionlint", "9.9.6"),
        ],
    );
    assert!(
        fake_tools_written.is_ok(),
        "write version-mapped fake core review tools: {fake_tools_written:?}"
    );

    assert_fake_core_review_tool_version(&fake_bin, "tokmd", "9.9.1")?;
    assert_fake_core_review_tool_version(&fake_bin, "actionlint", "9.9.6")?;
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
    let receipt_anchor = format!(
        "review/proof_receipts.json#{}",
        json_str_field(receipt, "id")?
    );
    let lease_anchor = format!(
        "review/resource_leases.json#{}",
        json_str_field(&leases[0], "id")?
    );
    assert_eq!(
        routes[0]["source_artifacts"],
        serde_json::json!([
            "review/proof_receipts.json",
            receipt_anchor,
            "review/resource_leases.json",
            lease_anchor
        ])
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
fn model_auto_run_retries_transient_primary_failure_on_provider_fallback() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(&repo)?;
    init_minimal_repo(&repo)?;

    let primary_key = "dummy-primary-key-for-fallback-test";
    let fallback_key = "dummy-fallback-key-for-fallback-test";
    let (primary_url, primary_provider) = spawn_fake_openai_provider_with_statuses(vec![503])?;
    let (fallback_url, fallback_provider) = spawn_fake_openai_provider(2)?;
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
            "primary-with-fallback",
            "--minimax-provider-kind",
            "openai",
            "--opencode-endpoint-kind",
            "openai-chat",
            "--lanes",
            "correctness",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "1",
            "--model-timeout-sec",
            "10",
        ],
        &[
            ("UB_REVIEW_MINIMAX_API_KEY", primary_key),
            ("UB_REVIEW_MINIMAX_API_URL", primary_url.as_str()),
            ("UB_REVIEW_OPENCODE_API_KEY", fallback_key),
            ("UB_REVIEW_OPENCODE_API_URL", fallback_url.as_str()),
        ],
    )?;
    let primary_requests = join_fake_provider(primary_provider)?;
    let fallback_requests = join_fake_provider(fallback_provider)?;
    assert_eq!(primary_requests.len(), 1);
    assert_eq!(fallback_requests.len(), 2);

    let preflights: serde_json::Value = serde_json::from_slice(&fs::read(
        out.join("review/provider-preflight-status.json"),
    )?)?;
    let primary_preflight = preflights
        .as_array()
        .and_then(|receipts| {
            receipts
                .iter()
                .find(|receipt| receipt["provider"] == "minimax")
        })
        .ok_or_else(|| anyhow::anyhow!("primary preflight receipt missing"))?;
    assert_eq!(primary_preflight["status"], "failed");
    assert_eq!(primary_preflight["http_status"], 503);
    let fallback_preflight = preflights
        .as_array()
        .and_then(|receipts| {
            receipts
                .iter()
                .find(|receipt| receipt["provider"] == "opencode-go")
        })
        .ok_or_else(|| anyhow::anyhow!("fallback preflight receipt missing"))?;
    assert_eq!(fallback_preflight["status"], "ok");

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    let correctness_lane = review["model_lanes"]
        .as_array()
        .and_then(|lanes| lanes.iter().find(|lane| lane["lane"] == "correctness"))
        .ok_or_else(|| anyhow::anyhow!("correctness model lane missing"))?;
    assert_eq!(correctness_lane["status"], "ok");
    assert_eq!(correctness_lane["provider"], "opencode-go");
    assert_eq!(
        correctness_lane["fallback_from"],
        "minimax:MiniMax-M3:openai-chat"
    );
    assert_eq!(correctness_lane["http_status"], 200);

    let metrics: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
    assert_eq!(metrics["models"]["model_fallbacks_used"], 1);
    for relative in collect_relative_file_paths(&out)? {
        if let Ok(text) = fs::read_to_string(out.join(&relative)) {
            assert!(
                !text.contains(primary_key),
                "primary secret leaked in {relative}"
            );
            assert!(
                !text.contains(fallback_key),
                "fallback secret leaked in {relative}"
            );
        }
    }
    Ok(())
}

#[test]
fn model_outage_degrades_review_without_reddening_enforced_gate() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;

    let primary_key = "dummy-primary-key-for-authority-test";
    let (provider_url, provider) = spawn_fake_openai_provider_with_statuses(vec![503])?;
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
            "--fail-on-gate",
            "true",
            "--no-github-summary",
            "--model-mode",
            "auto",
            "--provider-policy",
            "minimax-only",
            "--minimax-provider-kind",
            "openai",
            "--lanes",
            "correctness",
            "--tools",
            "cargo-allow",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "1",
            "--model-timeout-sec",
            "10",
        ],
        &[
            ("UB_REVIEW_MINIMAX_API_KEY", primary_key),
            ("UB_REVIEW_MINIMAX_API_URL", provider_url.as_str()),
        ],
    )?;
    let provider_requests = join_fake_provider(provider)?;
    assert_eq!(provider_requests.len(), 1);

    let preflights: serde_json::Value = serde_json::from_slice(&fs::read(
        out.join("review/provider-preflight-status.json"),
    )?)?;
    let preflight = preflights
        .as_array()
        .and_then(|receipts| receipts.first())
        .ok_or_else(|| anyhow::anyhow!("missing provider outage receipt"))?;
    assert_eq!(preflight["status"], "failed");
    assert_eq!(preflight["http_status"], 503);

    let review: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/review.json"))?)?;
    assert!(review["model_lanes"].as_array().is_some_and(|lanes| {
        lanes
            .iter()
            .any(|lane| lane["status"] == "preflight_failed")
    }));

    let terminal: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/terminal_state.json"))?)?;
    assert!(matches!(
        terminal["status"].as_str(),
        Some("artifact-only") | Some("failed-to-review")
    ));

    let gate: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/gate_outcome.json"))?)?;
    assert_eq!(gate["conclusion"], "pass");
    assert_eq!(gate["evidence_gaps_blocking"], 0);
    assert!(gate["evidence_gaps_advisory"].as_u64().unwrap_or_default() > 0);
    assert!(gate["reasons"].as_array().is_some_and(Vec::is_empty));
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
fn duplicate_model_proof_requests_execute_once_with_all_request_ids() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;

    let duplicate_command = "cargo test --locked duplicate_requested_proof";
    let duplicate_lane_content = serde_json::json!({
        "summary": null,
        "observations": [],
        "candidate_findings": [],
        "summary_only_findings": [],
        "failed_objections": [],
        "proof_requests": [
            {
                "command": duplicate_command,
                "reason": "A repeated focused proof request should run once with both request ids.",
                "cost": "focused-test",
                "timeout_sec": 60,
                "required": false
            }
        ]
    })
    .to_string();
    let dummy_key = "dummy-minimax-key-for-duplicate-proof-requests";
    let (provider_url, provider) = spawn_fake_openai_provider_with_contents(vec![
        fake_openai_lane_content(),
        duplicate_lane_content.clone(),
        duplicate_lane_content,
        fake_openai_lane_content(),
    ])
    .context("spawn fake OpenAI provider for duplicate proof requests")?;
    let fake_bin = temp.path().join("fake-bin");
    write_fake_cargo(&fake_bin)?;
    let path = prepend_to_path(&fake_bin)?;
    let fake_cargo_log = temp.path().join("fake-cargo.log");
    let fake_cargo_log_str = path_str(&fake_cargo_log)?.to_owned();
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
            "tests-red-green,opposition",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "3",
            "--model-timeout-sec",
            "10",
            "--tools",
            "cargo-allow",
        ],
        &[
            ("PATH", path.as_str()),
            ("UB_REVIEW_MINIMAX_API_KEY", dummy_key),
            ("UB_REVIEW_MINIMAX_API_URL", provider_url.as_str()),
            ("FAKE_CARGO_LOG", fake_cargo_log_str.as_str()),
        ],
    )?;
    let provider_requests = join_fake_provider(provider)?;
    assert_eq!(provider_requests.len(), 4);

    let proof_requests: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_requests.json"))?)?;
    let duplicate_requests = proof_requests
        .iter()
        .filter(|request| request["command"].as_str() == Some(duplicate_command))
        .collect::<Vec<_>>();
    assert_eq!(duplicate_requests.len(), 2);
    let request_ids = duplicate_requests
        .iter()
        .map(|request| json_str_field(request, "id").map(ToOwned::to_owned))
        .collect::<Result<Vec<_>>>()?;
    assert_ne!(request_ids[0], request_ids[1]);
    assert!(duplicate_requests.iter().all(|request| {
        matches!(
            request["status"].as_str(),
            Some("satisfied")
                | Some("executed")
                | Some("deferred")
                | Some("failed")
                | Some("deduplicated")
        )
    }));
    assert!(
        duplicate_requests
            .iter()
            .any(|request| request["status"].as_str() == Some("deduplicated")),
        "duplicate requests must expose a terminal deduplicated disposition"
    );

    let groups: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_request_groups.json"))?)?;
    let group = groups
        .iter()
        .find(|group| group["command"].as_str() == Some(duplicate_command))
        .ok_or_else(|| anyhow::anyhow!("duplicate proof request group missing"))?;
    assert_eq!(group["status"], "executed");
    assert_eq!(group["duplicate_count"], 2);
    assert_eq!(group["request_ids"], serde_json::json!(request_ids));
    assert!(group["requested_by"].as_array().is_some_and(|lanes| {
        lanes
            .iter()
            .any(|lane| lane.as_str() == Some("tests-red-green"))
            && lanes.iter().any(|lane| lane.as_str() == Some("opposition"))
    }));

    let planner_output: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_planner_output.json"))?)?;
    let duplicate_tasks = json_array_field(&planner_output, "proof_tasks")?
        .iter()
        .filter(|task| {
            task["command"].as_str().is_some_and(|command| {
                command.contains("cargo test --locked duplicate_requested_proof")
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        duplicate_tasks.len(),
        1,
        "duplicate requests must plan one broker task"
    );
    assert_eq!(
        duplicate_tasks[0]["request_ids"],
        serde_json::json!(request_ids)
    );

    let receipts: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
    let duplicate_receipts = receipts
        .iter()
        .filter(|receipt| receipt["request_ids"] == serde_json::json!(request_ids))
        .collect::<Vec<_>>();
    assert_eq!(
        duplicate_receipts.len(),
        1,
        "duplicate requests must produce one execution receipt"
    );
    let receipt = duplicate_receipts[0];
    assert_eq!(receipt["kind"], "focused-red-green");
    assert!(receipt["requested_by"].as_array().is_some_and(|lanes| {
        lanes
            .iter()
            .any(|lane| lane.as_str() == Some("tests-red-green"))
            && lanes.iter().any(|lane| lane.as_str() == Some("opposition"))
    }));
    let commands = json_array_field(receipt, "commands")?;
    assert_eq!(commands.len(), 2);
    assert_eq!(commands[0]["side"], "head");
    assert_eq!(commands[1]["side"], "base-plus-tests");

    let leases: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/resource_leases.json"))?)?;
    let duplicate_leases = leases
        .iter()
        .filter(|lease| lease["consumer"] == receipt["id"])
        .collect::<Vec<_>>();
    assert_eq!(duplicate_leases.len(), 1);
    assert_eq!(duplicate_leases[0]["status"], "granted");
    let receipt_source = format!(
        "review/proof_receipts.json#{}",
        json_str_field(receipt, "id")?
    );
    let lease_source = format!(
        "review/resource_leases.json#{}",
        json_str_field(duplicate_leases[0], "id")?
    );
    let fill_ledger: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/fill-ledger.json"))?)?;
    let duplicate_fill_entries = json_array_field(&fill_ledger, "entries")?
        .iter()
        .filter(|entry| {
            entry["kind"].as_str() == Some("proof-request")
                && request_ids
                    .iter()
                    .any(|request_id| entry["check_id"].as_str() == Some(request_id.as_str()))
        })
        .collect::<Vec<_>>();
    assert_eq!(
        duplicate_fill_entries.len(),
        2,
        "each duplicate proof request must keep its own fill-ledger decision"
    );
    for entry in duplicate_fill_entries {
        assert_eq!(
            entry["artifact_path"].as_str(),
            Some(receipt_source.as_str())
        );
        assert_eq!(entry["cost"].as_str(), Some("focused-test"));
        let sources = json_array_field(entry, "source_artifacts")?;
        assert!(
            sources
                .iter()
                .any(|source| source.as_str() == Some(lease_source.as_str())),
            "selected proof-request fill must cite the broker lease source `{lease_source}`: {entry:#?}"
        );
    }

    let log = fs::read_to_string(fake_cargo_log)?;
    assert_eq!(
        log.lines()
            .filter(|line| line.contains("duplicate_requested_proof"))
            .count(),
        2,
        "duplicate model proof requests should run one red/green task:\n{log}"
    );
    Ok(())
}

#[test]
fn model_suggested_manual_cost_proof_request_is_rejected_before_execution() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;

    let manual_command = "cargo test --locked manual_cost_should_not_execute";
    let planner_content = serde_json::json!({
        "summary": null,
        "observations": [],
        "candidate_findings": [],
        "summary_only_findings": [],
        "failed_objections": [],
        "proof_requests": [
            {
                "command": manual_command,
                "reason": "Manual-cost proof request must be parked instead of executed.",
                "cost": "manual",
                "timeout_sec": 60,
                "required": false
            }
        ]
    })
    .to_string();
    let dummy_key = "dummy-minimax-key-for-manual-cost-proof";
    let (provider_url, provider) = spawn_fake_openai_provider_with_contents(vec![
        fake_openai_lane_content(),
        fake_openai_lane_content(),
        planner_content,
    ])
    .context("spawn fake OpenAI provider for manual-cost proof request")?;
    let fake_bin = temp.path().join("fake-bin");
    write_fake_cargo(&fake_bin)?;
    let path = prepend_to_path(&fake_bin)?;
    let fake_cargo_log = temp.path().join("fake-cargo.log");
    let fake_cargo_log_str = path_str(&fake_cargo_log)?.to_owned();
    run_with_env(
        temp.path(),
        "cargo",
        &["--version"],
        &[
            ("PATH", path.as_str()),
            ("FAKE_CARGO_LOG", fake_cargo_log_str.as_str()),
        ],
    )
    .context("fake cargo sanity check before manual-cost proof run")?;
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
            "--tools",
            "cargo-allow",
        ],
        &[
            ("PATH", path.as_str()),
            ("UB_REVIEW_MINIMAX_API_KEY", dummy_key),
            ("UB_REVIEW_MINIMAX_API_URL", provider_url.as_str()),
            ("FAKE_CARGO_LOG", fake_cargo_log_str.as_str()),
        ],
    )?;
    let provider_requests = join_fake_provider(provider)?;
    assert_eq!(provider_requests.len(), 3);

    let proof_requests: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_requests.json"))?)?;
    let request = proof_requests
        .iter()
        .find(|request| request["command"].as_str() == Some(manual_command))
        .ok_or_else(|| anyhow::anyhow!("manual-cost proof request missing"))?;
    assert_eq!(request["status"], "unsupported");
    assert_eq!(request["cost"], "manual");
    let request_id = json_str_field(request, "id")?;

    let groups: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_request_groups.json"))?)?;
    let group = groups
        .iter()
        .find(|group| {
            group["request_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
        })
        .ok_or_else(|| anyhow::anyhow!("manual-cost proof request group missing"))?;
    assert_eq!(group["status"], "unsupported");

    let planner_output: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_planner_output.json"))?)?;
    let proof_tasks = json_array_field(&planner_output, "proof_tasks")?;
    assert!(
        proof_tasks.iter().all(|task| {
            !task["request_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
        }),
        "unsupported manual-cost request must not become a proof task"
    );

    let receipts: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
    assert!(
        receipts.iter().all(|receipt| {
            !receipt["request_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
        }),
        "unsupported manual-cost request must not receive an execution receipt"
    );
    let leases: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/resource_leases.json"))?)?;
    assert!(
        leases.iter().all(|lease| {
            !lease["command"]
                .as_str()
                .is_some_and(|command| command.contains(manual_command))
        }),
        "unsupported manual-cost request must not receive a command lease"
    );
    let log = fs::read_to_string(fake_cargo_log)?;
    assert!(log.contains("fake cargo --version"));
    assert!(!log.contains("manual_cost_should_not_execute"));
    Ok(())
}

#[test]
fn model_suggested_shell_token_proof_request_is_rejected_before_execution() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;

    let shell_command =
        "cargo test --locked shell_token_should_not_execute && cargo test --locked should_not_run";
    let planner_content = serde_json::json!({
        "summary": null,
        "observations": [],
        "candidate_findings": [],
        "summary_only_findings": [],
        "failed_objections": [],
        "proof_requests": [
            {
                "command": shell_command,
                "reason": "Shell-shaped proof request must be rejected before execution.",
                "cost": "focused-test",
                "timeout_sec": 60,
                "required": false
            }
        ]
    })
    .to_string();
    let dummy_key = "dummy-minimax-key-for-shell-token-proof";
    let (provider_url, provider) = spawn_fake_openai_provider_with_contents(vec![
        fake_openai_lane_content(),
        fake_openai_lane_content(),
        planner_content,
    ])?;
    let fake_bin = temp.path().join("fake-bin");
    write_fake_cargo(&fake_bin)?;
    let path = prepend_to_path(&fake_bin)?;
    let fake_cargo_log = temp.path().join("fake-cargo.log");
    let fake_cargo_log_str = path_str(&fake_cargo_log)?.to_owned();
    run_with_env(
        temp.path(),
        "cargo",
        &["--version"],
        &[
            ("PATH", path.as_str()),
            ("FAKE_CARGO_LOG", fake_cargo_log_str.as_str()),
        ],
    )?;
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
            "--tools",
            "cargo-allow",
        ],
        &[
            ("PATH", path.as_str()),
            ("UB_REVIEW_MINIMAX_API_KEY", dummy_key),
            ("UB_REVIEW_MINIMAX_API_URL", provider_url.as_str()),
            ("FAKE_CARGO_LOG", fake_cargo_log_str.as_str()),
        ],
    )?;
    let provider_requests = join_fake_provider(provider)?;
    assert_eq!(provider_requests.len(), 3);

    let proof_requests: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_requests.json"))?)?;
    let request = proof_requests
        .iter()
        .find(|request| request["command"].as_str() == Some(shell_command))
        .ok_or_else(|| anyhow::anyhow!("shell-token proof request missing"))?;
    assert_eq!(request["status"], "unsupported");
    assert_eq!(request["cost"], "focused-test");
    let request_id = json_str_field(request, "id")?;

    let groups: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_request_groups.json"))?)?;
    let group = groups
        .iter()
        .find(|group| {
            group["request_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
        })
        .ok_or_else(|| anyhow::anyhow!("shell-token proof request group missing"))?;
    assert_eq!(group["status"], "unsupported");

    let planner_output: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("review/proof_planner_output.json"))?)?;
    let proof_tasks = json_array_field(&planner_output, "proof_tasks")?;
    assert!(
        proof_tasks.iter().all(|task| {
            !task["request_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
        }),
        "unsupported shell-token request must not become a proof task"
    );

    let receipts: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
    assert!(
        receipts.iter().all(|receipt| {
            !receipt["request_ids"]
                .as_array()
                .is_some_and(|ids| ids.iter().any(|id| id.as_str() == Some(request_id)))
        }),
        "unsupported shell-token request must not receive an execution receipt"
    );
    let leases: Vec<serde_json::Value> =
        serde_json::from_slice(&fs::read(out.join("review/resource_leases.json"))?)?;
    assert!(
        leases.iter().all(|lease| {
            !lease["command"]
                .as_str()
                .is_some_and(|command| command.contains(shell_command))
        }),
        "unsupported shell-token request must not receive a command lease"
    );
    let log = fs::read_to_string(fake_cargo_log)?;
    assert!(log.contains("fake cargo --version"));
    assert!(!log.contains("shell_token_should_not_execute"));
    assert!(!log.contains("should_not_run"));
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
            "body": "## Decision\n\n- Needs proof.\n\n## model lanes\n\n- Lane: `ub`\n  Provider: `minimax`\n  Model: `MiniMax-M3`\n  Status: `ok` - completed",
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
    let event_path = temp.path().join("event.json");
    let out = temp.path().join("post");
    let token = "test-token-redacted";
    fs::write(
        &event_path,
        serde_json::to_vec(&serde_json::json!({
            "pull_request": {"head": {"sha": "test-head-sha"}}
        }))?,
    )?;
    fs::write(
        &review_json,
        serde_json::to_vec_pretty(&serde_json::json!({
            "event": "COMMENT",
            "body": "## Test proof\n\n- Added bad-free tests pass on HEAD and fail on base+tests.",
            "comments": []
        }))?,
    )?;
    let (github_api_url, handle) = spawn_fake_github_review_api_with_expected_requests(vec![], 4)?;

    run_with_env(
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
        &[("GITHUB_EVENT_PATH", path_str(&event_path)?)],
    )?;

    let requests = join_fake_provider(handle)?;
    assert_eq!(requests.len(), 4);
    assert!(requests[0].starts_with("GET /repos/EffortlessMetrics/ub-review/pulls/123 HTTP/1.1"));
    let request_text = &requests[1];
    assert!(
        request_text
            .starts_with("POST /repos/EffortlessMetrics/ub-review/pulls/123/reviews HTTP/1.1")
    );
    assert!(
        requests
            .iter()
            .any(|request| request.contains("Authorization: Bearer test-token-redacted"))
    );
    assert!(
        requests[2]
            .starts_with("GET /repos/EffortlessMetrics/ub-review/pulls/123/reviews/987/comments")
    );
    assert!(
        requests[3]
            .starts_with("POST /repos/EffortlessMetrics/ub-review/pulls/123/reviews/987/events")
    );
    assert!(requests[3].contains("\"event\": \"COMMENT\""));
    assert!(request_text.contains("\"comments\": []"));

    let post_result_path = out.join("post-result.json");
    assert!(post_result_path.exists());
    assert!(!out.join("post-error.json").exists());
    assert!(out.join("github-review-post-payload.json").exists());
    assert!(out.join("pending-review-stdout.json").exists());
    assert!(out.join("pending-review-stderr.txt").exists());
    assert!(out.join("submit-review-stdout.json").exists());
    assert!(out.join("submit-review-stderr.txt").exists());

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
    let head_check: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("post-head-check.json"))?)?;
    assert_eq!(head_check["status"], "matched");

    for path in [
        post_result_path,
        out.join("github-review-post-payload.json"),
        out.join("pending-review-stdout.json"),
        out.join("pending-review-stderr.txt"),
        out.join("submit-review-stdout.json"),
        out.join("submit-review-stderr.txt"),
    ] {
        let text = fs::read_to_string(path)?;
        assert!(!text.contains(token));
        assert!(!text.contains("Authorization"));
        assert!(!text.contains("Bearer"));
    }
    Ok(())
}

#[test]
fn post_head_mismatch_fails_before_pending_review_creation() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let review_json = temp.path().join("github-review.json");
    let event_path = temp.path().join("event.json");
    let out = temp.path().join("post");
    fs::write(
        &review_json,
        serde_json::to_vec_pretty(&serde_json::json!({
            "event": "COMMENT",
            "body": "## Test proof\n\n- Current-head proof is required.",
            "comments": []
        }))?,
    )?;
    fs::write(
        &event_path,
        serde_json::to_vec(&serde_json::json!({
            "pull_request": {"head": {"sha": "stale-head-sha"}}
        }))?,
    )?;
    let (github_api_url, handle) = spawn_fake_github_review_api_with_expected_requests(vec![], 1)?;

    run_with_env(
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
            "test-token-redacted",
            "--github-api-url",
            &github_api_url,
        ],
        &[("GITHUB_EVENT_PATH", path_str(&event_path)?)],
    )?;

    let requests = join_fake_provider(handle)?;
    anyhow::ensure!(
        requests.len() == 1
            && requests[0].starts_with("GET /repos/EffortlessMetrics/ub-review/pulls/123 HTTP/1.1"),
        "head verification must be the only request before a mismatch: {requests:#?}"
    );
    let head_check: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("post-head-check.json"))?)?;
    anyhow::ensure!(
        head_check["status"] == "mismatch",
        "head mismatch must be receipted"
    );
    let post_error: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("post-error.json"))?)?;
    anyhow::ensure!(
        post_error["status"] == "failed"
            && post_error["reason"]
                .as_str()
                .is_some_and(|reason| reason.contains("current pull request head changed")),
        "stale head must fail closed: {post_error:#?}"
    );
    anyhow::ensure!(
        !out.join("pending-review-stdout.json").exists()
            && !out.join("submit-review-stdout.json").exists(),
        "stale head must not create or submit a pending review"
    );
    Ok(())
}

#[test]
fn post_comment_receipt_mismatch_deletes_pending_review() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let review_json = temp.path().join("github-review.json");
    let diff_patch = temp.path().join("diff.patch");
    let out = temp.path().join("post");
    fs::write(
        &diff_patch,
        "diff --git a/src/lib.rs b/src/lib.rs\nindex 1111111..2222222 100644\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,3 +1,4 @@\n pub fn active_len(len: usize) -> usize {\n+    let header = unsafe { ptr.cast::<Header>().read() };\n     len\n}\n",
    )?;
    fs::write(
        &review_json,
        serde_json::to_vec_pretty(&serde_json::json!({
            "event": "COMMENT",
            "body": "## Verification questions\n\n- Confirm the focused proof.",
            "comments": [{
                "path": "src/lib.rs",
                "line": 1,
                "side": "RIGHT",
                "body": "[tests] focused proof receipt is missing."
            }]
        }))?,
    )?;
    let (github_api_url, handle) =
        spawn_fake_github_review_api_with_expected_requests(Vec::new(), 3)?;

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
            "123",
            "--github-token",
            "test-token-redacted",
            "--github-api-url",
            &github_api_url,
        ],
    )?;

    let requests = join_fake_provider(handle)?;
    assert_eq!(requests.len(), 3);
    assert!(
        requests[2].starts_with("DELETE /repos/EffortlessMetrics/ub-review/pulls/123/reviews/987")
    );
    let post_error: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("post-error.json"))?)?;
    assert_eq!(post_error["status"], "failed");
    assert!(
        post_error["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains("cleanup=true"))
    );
    let deletion: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("pending-review-delete.json"))?)?;
    assert_eq!(deletion["review_id"], 987);
    assert_eq!(deletion["deleted"], true);
    Ok(())
}

#[test]
fn post_payload_renders_suggestion_blocks_for_github_api() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let review_json = temp.path().join("github-review.json");
    let diff_patch = temp.path().join("diff.patch");
    let out = temp.path().join("post");
    let token = "test-token-redacted";
    fs::write(
        &diff_patch,
        "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let header = unsafe { ptr.cast::<Header>().read() };
     len
}
",
    )
    .with_context(|| format!("write {}", diff_patch.display()))?;
    fs::write(
        &review_json,
        serde_json::to_vec_pretty(&serde_json::json!({
            "event": "COMMENT",
            "body": "## Verification questions\n\n- Confirm the unsafe guard proof.",
            "comments": [
                {
                    "path": "src/lib.rs",
                    "line": 2,
                    "side": "RIGHT",
                    "body": "[unsafe-review] Guard evidence is missing.",
                    "suggestion": "let header = guarded_header_read(ptr)?;"
                }
            ]
        }))?,
    )
    .with_context(|| format!("write {}", review_json.display()))?;
    let (github_api_url, handle) = spawn_fake_github_review_api(vec![654])?;

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
            "123",
            "--github-token",
            token,
            "--github-api-url",
            &github_api_url,
        ],
    )?;

    let requests = join_fake_provider(handle)?;
    assert_eq!(requests.len(), 3);
    let request_text = &requests[0];
    assert!(request_text.contains("```suggestion\\nlet header = guarded_header_read(ptr)?;\\n```"));
    assert!(
        !request_text.contains("\"suggestion\""),
        "GitHub API payload must not contain internal suggestion field: {request_text}"
    );
    assert!(
        requests[1]
            .starts_with("GET /repos/EffortlessMetrics/ub-review/pulls/123/reviews/987/comments")
    );
    assert!(
        requests[2]
            .starts_with("POST /repos/EffortlessMetrics/ub-review/pulls/123/reviews/987/events")
    );

    let post_payload_text = fs::read_to_string(out.join("github-review-post-payload.json"))?;
    assert!(
        post_payload_text.contains("```suggestion\\nlet header = guarded_header_read(ptr)?;\\n```")
    );
    assert!(
        !post_payload_text.contains("\"suggestion\""),
        "post payload artifact must not leak internal suggestion field: {post_payload_text}"
    );
    let post_result: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(out.join("post-result.json"))?)?;
    assert_eq!(post_result["status"], "ok");
    assert_eq!(post_result["comments"], 1);
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
        .env("GITHUB_ACTIONS", "true")
        .env("RUNNER_ENVIRONMENT", "github-hosted")
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
    let box_state: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("box-state.json"))?)?;
    assert_eq!(
        box_state["github_actions"], false,
        "ambient GITHUB_ACTIONS leaked into an isolated_command child"
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

fn collect_relative_file_paths(root: &Path) -> Result<Vec<String>> {
    fn visit(base: &Path, dir: &Path, files: &mut Vec<String>) -> Result<()> {
        for entry in fs::read_dir(dir).with_context(|| format!("read {}", dir.display()))? {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                visit(base, &path, files)?;
            } else if file_type.is_file() {
                let relative = path
                    .strip_prefix(base)
                    .with_context(|| format!("strip prefix {}", base.display()))?
                    .to_string_lossy()
                    .replace('\\', "/");
                files.push(relative);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort();
    Ok(files)
}

fn write_setup_ci_cli_audit_fixture(dir: &Path) -> Result<()> {
    write_init_audit_ci_fixture(dir)?;
    for (name, schema) in [
        ("history.json", "ub-review.ci_history.v1"),
        ("costs.json", "ub-review.ci_costs.v1"),
        ("correlation.json", "ub-review.ci_correlation.v1"),
    ] {
        fs::write(
            dir.join(name),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": schema,
                "repo": "acme/widgets",
                "window_days": 90,
                "jobs": [],
                "evidence_gaps": [],
            }))?,
        )?;
    }
    Ok(())
}

fn write_init_audit_ci_fixture(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir)?;
    let job = |name: &str| {
        serde_json::json!({
            "workflow": ".github/workflows/ci.yml",
            "job": name,
            "name": name,
            "triggers": ["pull_request"],
            "path_filters": [],
            "matrix_size": 1,
            "timeout_minutes": 30,
            "permissions": null,
            "uses_secrets": [],
            "required_check": null,
            "required_check_source": "unknown",
            "required_check_context": null,
        })
    };
    fs::write(
        dir.join("inventory.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "ub-review.ci_inventory.v1",
            "generated_at": "2026-06-14T00:00:00Z",
            "repo": "acme/widgets",
            "window_days": 90,
            "jobs": [job("integration"), job("unit"), job("fmt"), job("deploy")],
            "evidence_gaps": ["required checks unreadable from tokenless audit"],
        }))?,
    )?;
    let recommendation = |name: &str, tier: &str, reason: &str| {
        serde_json::json!({
            "job": name,
            "workflow": ".github/workflows/ci.yml",
            "tier": tier,
            "positioned_to_catch": "regressions in its scope",
            "has_caught": "2 independent failures in the window",
            "receipts": [format!("ci-audit/correlation.json#{name}")],
            "proposed_policy": "per tier",
            "confidence": "medium",
            "judgment": "deterministic",
            "reason": reason,
            "report_note": "",
        })
    };
    fs::write(
        dir.join("recommendations.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "ub-review.ci_recommendations.v1",
            "repo": "acme/widgets",
            "window_days": 90,
            "jobs": [
                recommendation("integration", "adaptive", "expensive and quiet on unrelated diffs"),
                recommendation("unit", "move-to-ub-review-required", "already merge-relevant and cheap"),
                recommendation("fmt", "keep-required", "cheap deterministic floor"),
                recommendation("deploy", "flag-for-human", "release-sensitive job"),
            ],
            "evidence_gaps": ["history window truncated"],
        }))?,
    )?;
    let report_path = dir.join("audit-report.md");
    fs::write(
        &report_path,
        "# CI audit: acme/widgets\n\n## Jobs\n\n### Right-size to adaptive\n\n- `integration` (ci.yml) [medium]: p50 unknown, 2 runs, 0 independent failures, ~4 runner-min/mo, receipts: `ci-audit/correlation.json#integration`\n",
    )
    .with_context(|| format!("write {}", report_path.display()))?;
    Ok(())
}

fn write_bad_init_audit_ci_fixture(dir: &Path) {
    let created = fs::create_dir_all(dir);
    assert!(
        created.is_ok(),
        "bad audit fixture directory should be writable: {created:?}"
    );
    let written = fs::write(
        dir.join("inventory.json"),
        "{ \"schema\": \"wrong.schema\", \"jobs\": [] }\n",
    );
    assert!(
        written.is_ok(),
        "bad audit fixture inventory receipt should be writable: {written:?}"
    );
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
if [ -n "$FAKE_CARGO_LOG" ]; then
  printf '%s\n' "fake cargo $*" >> "$FAKE_CARGO_LOG"
fi
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
    if let Ok(path) = env::var("FAKE_CARGO_LOG") {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut file| {
                use std::io::Write;
                writeln!(file, "fake cargo {}", args.join(" "))
            });
    }
    if let Ok(value) = env::var("FAKE_CARGO_SLEEP_MS") {
        if let Ok(ms) = value.parse::<u64>() {
            std::thread::sleep(std::time::Duration::from_millis(ms));
        }
    }
}
"#;

fn write_fake_core_review_tools(dir: &Path, tokmd_version: &str) -> Result<()> {
    write_fake_core_review_tools_with_versions(
        dir,
        &[
            ("tokmd", tokmd_version),
            ("cargo-allow", "0.1.8"),
            ("ripr", "0.10.0"),
            ("unsafe-review", "0.3.4"),
            ("ast-grep", "0.0.0"),
            ("actionlint", "1.7.12"),
        ],
    )
}

fn write_fake_core_review_tools_with_versions(dir: &Path, versions: &[(&str, &str)]) -> Result<()> {
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
        let mut match_arms = String::new();
        for (tool, version) in versions {
            match_arms.push_str(&format!("{tool:?} => {version:?},\n"));
        }
        write_file(
            &source,
            &format!(
                r#"use std::{{env, path::Path}};

fn main() {{
    let executable = env::args().next().unwrap_or_default();
    let name = Path::new(&executable)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("review-tool");
    let version = match name {{
        {match_arms}
        _ => "0.0.0",
    }};
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
            let version = versions
                .iter()
                .find_map(|(name, version)| (*name == tool).then_some(*version))
                .unwrap_or("0.0.0");
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

mod common;
use common::*;
