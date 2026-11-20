use super::*;

#[cfg(unix)]
fn init_guide_recommended_command(guide: &str, subcommand: &str) -> Result<String> {
    let prefix = format!("ub-review {subcommand} ");
    let mut recommended = false;
    let mut lines = guide.split('\n');
    while let Some(line) = lines.next() {
        let line = line.strip_prefix("   ").unwrap_or(line);
        let fence_length = line.bytes().take_while(|byte| *byte == b'`').count();
        if fence_length >= 3 {
            let language = &line[fence_length..];
            let mut body = Vec::new();
            let mut closed = false;
            for body_line in lines.by_ref() {
                let body_line = body_line.strip_prefix("   ").unwrap_or(body_line);
                if body_line.len() >= fence_length && body_line.bytes().all(|byte| byte == b'`') {
                    closed = true;
                    break;
                }
                body.push(body_line);
            }
            anyhow::ensure!(closed, "guide fence is not closed");
            let command = body.join("\n");
            if recommended && language == "sh" && command.starts_with(&prefix) {
                return Ok(command);
            }
        } else if line == "## Recommended path" {
            recommended = true;
        } else if recommended && let Some(start) = line.find(&prefix) {
            let suffix = match subcommand {
                "doctor" => {
                    "` and fix missing tools or provider keys before trusting the standard gate image."
                }
                "audit-ci" => "` for read-only CI receipts.",
                _ => bail!("unexpected guide subcommand"),
            };
            return line[start..]
                .strip_suffix(suffix)
                .map(str::to_owned)
                .context("legacy command suffix is missing");
        }
    }
    bail!("recommended command is missing: {subcommand}")
}

#[cfg(unix)]
fn verify_init_guide_posix_paths(root_name: &str, config_name: &str) -> Result<()> {
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join(root_name);
    let config = temp.path().join(config_name);
    let guide = temp.path().join("guide.md");
    let empty_path = temp.path().join("empty-path");
    fs::create_dir(&empty_path)?;
    write_file(
        &repo.join(".github/workflows/ci.yml"),
        "name: path-proof\non: [pull_request]\njobs:\n  path_witness:\n    runs-on: ubuntu-latest\n    steps:\n      - run: true\n",
    )?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    let init = Command::new(bin)
        .current_dir(temp.path())
        .env_clear()
        .env("PATH", &empty_path)
        .arg("init")
        .arg(format!("--root={root_name}"))
        .arg(format!("--path={config_name}"))
        .arg("--guide-out")
        .arg(&guide)
        .args(["--profile", "cx23"])
        .fixture_output()?;
    anyhow::ensure!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );
    let guide_text = fs::read_to_string(&guide)?;
    let workflow_before = fs::read(repo.join(".github/workflows/ci.yml"))?;
    for subcommand in ["doctor", "audit-ci"] {
        let command = init_guide_recommended_command(&guide_text, subcommand)?;
        let argv_path = temp.path().join(format!("{subcommand}.argv"));
        let script = format!(
            "record_ub_review() {{ printf '%s\\000' \"$@\" > \"$UB_REVIEW_TEST_ARGV\"; \"$UB_REVIEW_TEST_BINARY\" \"$@\"; }}\nalias ub-review=record_ub_review\n{command}"
        );
        let output = Command::new("/bin/sh")
            .current_dir(temp.path())
            .env_clear()
            .env("PATH", &empty_path)
            .env("UB_REVIEW_TEST_BINARY", bin)
            .env("UB_REVIEW_TEST_ARGV", &argv_path)
            .env("UB_REVIEW_TEST_EXPAND", "expanded")
            .env("GITHUB_REPOSITORY", "fixture/path-proof")
            .args(["-c", &script])
            .fixture_output()?;
        let raw_argv = fs::read(&argv_path).context("command did not invoke ub-review")?;
        anyhow::ensure!(raw_argv.last() == Some(&0), "argv is not NUL-terminated");
        let mut argv = Vec::new();
        for argument in raw_argv[..raw_argv.len() - 1].split(|byte| *byte == 0) {
            let argument = std::str::from_utf8(argument)?;
            if let Some(value) = argument.strip_prefix("--config=") {
                argv.extend(["--config".to_owned(), value.to_owned()]);
            } else if let Some(value) = argument.strip_prefix("--root=") {
                argv.extend(["--root".to_owned(), value.to_owned()]);
            } else {
                argv.push(argument.to_owned());
            }
        }
        let expected = if subcommand == "doctor" {
            vec![
                "doctor".to_owned(),
                "--config".to_owned(),
                config_name.to_owned(),
                "--root".to_owned(),
                root_name.to_owned(),
                "--require-core-tools".to_owned(),
            ]
        } else {
            vec![
                "audit-ci".to_owned(),
                "--root".to_owned(),
                root_name.to_owned(),
                "--out".to_owned(),
                "target/ub-review".to_owned(),
            ]
        };
        anyhow::ensure!(
            argv == expected,
            "{subcommand} changed path arguments: {argv:?}"
        );
        if subcommand == "doctor" {
            let stdout = String::from_utf8(output.stdout)?;
            let stderr = String::from_utf8(output.stderr)?;
            anyhow::ensure!(
                stdout.contains("Profile: cx23\n"),
                "doctor did not load the config"
            );
            anyhow::ensure!(
                stderr.contains("required core review tools missing"),
                "doctor did not reach the required-tool check: {stderr}"
            );
        } else {
            anyhow::ensure!(output.status.success(), "audit-ci failed");
            let inventory: serde_json::Value = serde_json::from_slice(&fs::read(
                temp.path().join("target/ub-review/ci-audit/inventory.json"),
            )?)?;
            anyhow::ensure!(inventory["repo"] == "fixture/path-proof");
            anyhow::ensure!(
                inventory["jobs"]
                    .as_array()
                    .is_some_and(|jobs| jobs.len() == 1)
            );
            anyhow::ensure!(inventory["jobs"][0]["job"] == "path_witness");
        }
    }
    anyhow::ensure!(config.is_file());
    anyhow::ensure!(fs::read(repo.join(".github/workflows/ci.yml"))? == workflow_before);
    Ok(())
}

#[cfg(unix)]
#[test]
fn init_guide_posix_commands_preserve_ordinary_paths() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    verify_init_guide_posix_paths("repo", "config.toml")
}

#[cfg(unix)]
#[test]
fn init_guide_posix_commands_preserve_paths_with_spaces() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    verify_init_guide_posix_paths("My Repo", "review config.toml")
}

#[cfg(unix)]
#[test]
fn init_guide_posix_commands_preserve_hostile_paths() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    for name in [
        "quotes ' \"",
        "dollar $UB_REVIEW_TEST_EXPAND $(printf injected)",
        "backticks `printf injected`",
        "separator ; printf injected #",
        "glob * ? [x] \\ tab\t",
        "newline\n```\nrest",
        "-leading-option",
        "unicode café 文",
    ] {
        verify_init_guide_posix_paths(name, &format!("{name}.toml"))?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn init_guide_rejects_non_utf8_command_paths_before_writes() -> Result<()> {
    use std::os::unix::ffi::OsStringExt;

    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    for invalid_root in [false, true] {
        let temp = tempfile::tempdir()?;
        let invalid = std::ffi::OsString::from_vec(b"invalid-\xff".to_vec());
        let root = if invalid_root {
            temp.path().join(&invalid)
        } else {
            temp.path().join("repo")
        };
        let config = if invalid_root {
            temp.path().join("config.toml")
        } else {
            temp.path().join(&invalid)
        };
        let guide = temp.path().join("guide.md");
        let empty_path = temp.path().join("empty-path");
        fs::create_dir(&root)?;
        fs::create_dir(&empty_path)?;
        let output = Command::new(env!("CARGO_BIN_EXE_ub-review"))
            .current_dir(temp.path())
            .env_clear()
            .env("PATH", &empty_path)
            .args(["init", "--root"])
            .arg(&root)
            .arg("--path")
            .arg(&config)
            .arg("--guide-out")
            .arg(&guide)
            .fixture_output()?;
        anyhow::ensure!(!output.status.success());
        anyhow::ensure!(
            String::from_utf8_lossy(&output.stderr)
                .contains("POSIX command paths must be valid UTF-8")
        );
        anyhow::ensure!(!config.exists());
        anyhow::ensure!(!guide.exists());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn init_guide_rejects_carriage_return_command_paths_before_writes() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    for name in ["carriage\rreturn", "pair\r\nreturn"] {
        for invalid_root in [false, true] {
            let temp = tempfile::tempdir()?;
            let root = temp.path().join(if invalid_root { name } else { "repo" });
            let config = temp
                .path()
                .join(if invalid_root { "config.toml" } else { name });
            let guide = temp.path().join("guide.md");
            let empty_path = temp.path().join("empty-path");
            fs::create_dir(&root)?;
            fs::create_dir(&empty_path)?;
            let output = Command::new(env!("CARGO_BIN_EXE_ub-review"))
                .current_dir(temp.path())
                .env_clear()
                .env("PATH", &empty_path)
                .args(["init", "--root"])
                .arg(&root)
                .arg("--path")
                .arg(&config)
                .arg("--guide-out")
                .arg(&guide)
                .fixture_output()?;
            anyhow::ensure!(
                !output.status.success(),
                "init accepted a carriage return in a command path"
            );
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::ensure!(
                stderr.contains("POSIX command paths must not contain carriage returns"),
                "init did not explain the path limitation: {stderr}"
            );
            anyhow::ensure!(!config.exists());
            anyhow::ensure!(!guide.exists());
        }
    }
    Ok(())
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
    for forbidden in [
        "policy = \"\"",
        "mode = \"\"",
        "kind = \"bun\"",
        "/home/steven",
    ] {
        assert!(
            !config_text.contains(forbidden),
            "init starter config retained `{forbidden}`:\n{config_text}"
        );
    }
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
