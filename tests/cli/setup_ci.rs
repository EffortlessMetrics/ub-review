use super::*;

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
            "setup-ci generated decorative or inert config key `{forbidden}`"
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
