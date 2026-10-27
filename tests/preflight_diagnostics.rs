//! Real, credential-free doctor/plan controls for semantic config preflight.

use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Result, ensure};

const CANARY: &str = "SYNTHETIC_REJECTED_SECRET_1555";

fn command(root: &Path, program: &str) -> Command {
    let mut command = Command::new(program);
    command.current_dir(root).env_clear();
    for name in ["PATH", "LLVM_PROFILE_FILE"] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

fn cli(root: &Path, subcommand: &str) -> Command {
    let mut command = command(root, env!("CARGO_BIN_EXE_ub-review"));
    command.arg(subcommand).args(["--root", "."]);
    if subcommand == "plan" {
        command.args(["--base", "HEAD^", "--head", "HEAD", "--out", "packet"]);
    }
    command
}

fn fixture() -> Result<tempfile::TempDir> {
    let temp = tempfile::tempdir()?;
    fs::write(temp.path().join("file.txt"), "before\n")?;
    for args in [
        vec!["init", "--quiet"],
        vec!["config", "user.name", "Preflight Fixture"],
        vec!["config", "user.email", "preflight@example.invalid"],
        vec!["add", "file.txt"],
        vec![
            "-c",
            "commit.gpgsign=false",
            "commit",
            "--quiet",
            "-m",
            "base",
        ],
    ] {
        ensure!(command(temp.path(), "git").args(args).status()?.success());
    }
    fs::write(temp.path().join("file.txt"), "after\n")?;
    ensure!(
        command(temp.path(), "git")
            .args(["add", "file.txt"])
            .status()?
            .success()
    );
    ensure!(
        command(temp.path(), "git")
            .args([
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--quiet",
                "-m",
                "head"
            ])
            .status()?
            .success()
    );
    Ok(temp)
}

fn console(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn invalid(output: &Output, source: &str, fields: &[&str]) -> Result<()> {
    ensure!(
        output.status.code() == Some(1),
        "expected semantic exit 1, got {:?}",
        output.status
    );
    let text = console(output);
    ensure!(
        text.contains("invalid configuration"),
        "missing invalid-config diagnosis"
    );
    ensure!(text.contains(source), "missing config source {source}");
    for field in fields {
        ensure!(text.contains(field), "missing policy field {field}");
    }
    ensure!(
        text.contains("Correct") || text.contains("Remove"),
        "missing correction"
    );
    ensure!(!text.contains(CANARY), "rejected value leaked to console");
    Ok(())
}

#[test]
fn doctor_reports_every_retained_error_with_source_and_correction() -> Result<()> {
    let temp = fixture()?;
    fs::write(
        temp.path().join("invalid.toml"),
        format!(
            "profile = \"unknown-runtime\"\n[gatee]\nrequired_check = \"unused\"\n[gate]\ntarget_minutes = \"{CANARY}\"\nhard_timeout_minutes = 71\n"
        ),
    )?;
    for from_env in [false, true] {
        let mut command = cli(temp.path(), "doctor");
        if from_env {
            command.env("UB_REVIEW_CONFIG", "invalid.toml");
        } else {
            command.args(["--config", "invalid.toml"]);
        }
        // Invalid policy must be distinct from missing required core tools.
        let output = command.arg("--require-core-tools").output()?;
        invalid(
            &output,
            "invalid.toml",
            &["gatee", "gate.target_minutes", "profile"],
        )?;
        ensure!(console(&output).contains("explicit"));
    }
    Ok(())
}

#[test]
fn plan_rejects_each_semantic_error_with_and_without_writing() -> Result<()> {
    for (text, field) in [
        ("[gatee]\nrequired_check = \"unused\"\n", "gatee"),
        ("[gate]\ntarget_minutes = \"oops\"\n", "gate.target_minutes"),
        ("profile = \"unknown-runtime\"\n", "profile"),
    ] {
        for write in [false, true] {
            let temp = fixture()?;
            fs::write(temp.path().join("invalid.toml"), text)?;
            let mut command = cli(temp.path(), "plan");
            command.args(["--config", "invalid.toml"]);
            if write {
                command.arg("--write");
            }
            invalid(&command.output()?, "invalid.toml", &[field])?;
            let packet = temp.path().join("packet");
            ensure!(packet.exists() == write, "write={write} output mismatch");
            if write {
                let effective: serde_json::Value =
                    serde_json::from_slice(&fs::read(packet.join("effective-config.json"))?)?;
                ensure!(effective["policy_errors"].as_array().map(Vec::len) == Some(1));
                ensure!(effective["policy_errors"][0]["section"] == field);
                ensure!(packet.join("plan.json").is_file());
                ensure!(packet.join("input/revision-admission.json").is_file());
            }
        }
    }
    Ok(())
}

#[test]
fn preflight_redacts_long_rejected_values_in_console_and_plan_artifacts() -> Result<()> {
    for subcommand in ["doctor", "plan"] {
        let temp = fixture()?;
        fs::write(
            temp.path().join("secrets.toml"),
            format!(
                "profile = \"{}\"\n[gate]\ntarget_minutes = \"{}\"\nhard_timeout_minutes = 71\n[providers]\npolicy = \"{CANARY}\"\n[providers.minimax]\nmax_concurrency = [{{ private_payload = \"{CANARY}\" }}]\n[providers.opencode]\nmax_concurrency = 2\n",
                CANARY.repeat(1024),
                CANARY.repeat(1024)
            ),
        )?;
        let mut command = cli(temp.path(), subcommand);
        command.args(["--config", "secrets.toml"]);
        if subcommand == "plan" {
            command.arg("--write");
        }
        let output = command.output()?;
        invalid(
            &output,
            "secrets.toml",
            &[
                "profile",
                "gate.target_minutes",
                "providers",
                "providers.minimax.max_concurrency",
            ],
        )?;
        ensure!(
            console(&output).len() < 8_000,
            "unbounded console diagnostics"
        );
        if subcommand == "plan" {
            let packet = temp.path().join("packet");
            for name in [
                "effective-config.json",
                "resolved-profile.json",
                "resolved-plan.json",
                "plan.json",
            ] {
                ensure!(
                    !fs::read_to_string(packet.join(name))?.contains(CANARY),
                    "rejected value leaked in {name}"
                );
            }
            let effective: serde_json::Value =
                serde_json::from_slice(&fs::read(packet.join("effective-config.json"))?)?;
            ensure!(effective["policy_errors"].as_array().map(Vec::len) == Some(4));
            ensure!(effective["gate"]["hard_timeout_minutes"] == 71);
            ensure!(effective["gate"]["target_minutes"] == 30);
            ensure!(effective["providers"]["opencode"]["max_concurrency"] == 2);
            ensure!(effective["profile"] == "gh-runner");
        }
    }
    Ok(())
}

#[test]
fn preflight_redacts_receipts_across_policy_surfaces_and_proof_ids() -> Result<()> {
    for subcommand in ["doctor", "plan"] {
        let temp = fixture()?;
        fs::write(
            temp.path().join("invalid.toml"),
            format!(
                "[impact]\nmode = \"{CANARY}\"\n[review_body]\nsummary_only_body = \"{CANARY}\"\n[tools.ripr]\nweight = \"{CANARY}\"\n[tools.tokmd.gate]\nscope = \"{CANARY}\"\n[[proof.required]]\nid = \"{CANARY}\"\ncommand = \"\"\n"
            ),
        )?;
        let mut command = cli(temp.path(), subcommand);
        command.args(["--config", "invalid.toml"]);
        if subcommand == "plan" {
            command.arg("--write");
        }
        let output = command.output()?;
        invalid(
            &output,
            "invalid.toml",
            &[
                "impact.mode",
                "review_body.summary_only_body",
                "tools.ripr.weight",
                "tools.tokmd.gate.scope",
                "proof.required.[redacted id]",
            ],
        )?;
        ensure!(
            console(&output)
                .lines()
                .filter(|line| line.starts_with("  - "))
                .count()
                == 5
        );
        if subcommand == "plan" {
            let effective = fs::read_to_string(temp.path().join("packet/effective-config.json"))?;
            ensure!(
                !effective.contains(CANARY),
                "rejected proof id or detail leaked"
            );
            let effective: serde_json::Value = serde_json::from_str(&effective)?;
            ensure!(effective["policy_errors"].as_array().map(Vec::len) == Some(5));
        }
    }
    Ok(())
}

#[test]
fn preflight_bounds_and_escapes_each_label_without_dropping_errors() -> Result<()> {
    let temp = fixture()?;
    let config = "review\nconfig.toml";
    fs::write(
        temp.path().join(config),
        format!(
            "[\"gatee\\nforged\\u001b\"]\nvalue = 1\n[{}]\nvalue = 2\n",
            "overlong".repeat(1024)
        ),
    )?;
    let output = cli(temp.path(), "doctor")
        .args(["--config", config])
        .output()?;
    invalid(
        &output,
        "review\\nconfig.toml",
        &["gatee\\nforged\\u{1b}", "overlong"],
    )?;
    let text = console(&output);
    ensure!(text.len() < 2_000, "unbounded config labels");
    ensure!(!text.contains('\u{1b}') && !text.contains("\nforged"));
    ensure!(text.contains("[truncated]"));
    ensure!(text.lines().filter(|line| line.starts_with("  - ")).count() == 2);
    Ok(())
}

#[test]
fn preflight_preserves_optional_discovery_and_config_profile_precedence() -> Result<()> {
    for subcommand in ["doctor", "plan"] {
        let temp = fixture()?;
        ensure!(
            cli(temp.path(), subcommand).output()?.status.success(),
            "optional absence failed"
        );
        fs::write(temp.path().join(".ub-review.toml"), "[gatee]\nvalue = 1\n")?;
        let output = cli(temp.path(), subcommand).output()?;
        invalid(&output, ".ub-review.toml", &["gatee"])?;
        ensure!(console(&output).contains("discovered"));
        fs::write(
            temp.path().join("valid.toml"),
            "profile = \"unknown-runtime\"\n[gate]\ntarget_minutes = 47\n",
        )?;
        let mut command = cli(temp.path(), subcommand);
        command
            .env("UB_REVIEW_CONFIG", "missing.toml")
            .env("UB_REVIEW_PROFILE", "cx23")
            .args(["--config", "valid.toml", "--runtime-profile", "gh-runner"]);
        if subcommand == "plan" {
            command.arg("--write");
        }
        let output = command.output()?;
        ensure!(output.status.success(), "valid override rejected");
        ensure!(!console(&output).contains("invalid configuration"));
        if subcommand == "plan" {
            let effective: serde_json::Value = serde_json::from_slice(&fs::read(
                temp.path().join("packet/effective-config.json"),
            )?)?;
            ensure!(effective["profile"] == "gh-runner");
            ensure!(effective["gate"]["target_minutes"] == 47);
            ensure!(effective.get("policy_errors").is_none());
        }
        fs::write(
            temp.path().join(".ub-review.toml"),
            "[gate]\ntarget_minutes = 43\n",
        )?;
        ensure!(
            cli(temp.path(), subcommand).output()?.status.success(),
            "valid discovery failed"
        );
    }
    Ok(())
}

#[test]
fn preflight_preserves_read_and_syntax_errors_before_output() -> Result<()> {
    for subcommand in ["doctor", "plan"] {
        let temp = fixture()?;
        fs::write(temp.path().join("broken.toml"), "[gate")?;
        fs::write(temp.path().join("non-text.toml"), [0xff])?;
        fs::create_dir(temp.path().join("directory.toml"))?;
        for (path, diagnostic) in [
            ("missing.toml", "read"),
            ("broken.toml", "parse"),
            ("non-text.toml", "read"),
            ("directory.toml", "read"),
        ] {
            let mut command = cli(temp.path(), subcommand);
            command.args(["--config", path]);
            if subcommand == "plan" {
                command.arg("--write");
            }
            let output = command.output()?;
            ensure!(output.status.code() == Some(1));
            ensure!(
                String::from_utf8_lossy(&output.stderr).contains(&format!("{diagnostic} {path}"))
            );
            ensure!(output.stdout.is_empty());
            ensure!(!temp.path().join("packet").exists());
        }
    }
    Ok(())
}

#[test]
fn plan_write_failure_remains_failure_and_keeps_semantic_diagnosis() -> Result<()> {
    let temp = fixture()?;
    fs::write(
        temp.path().join("invalid.toml"),
        "[gate]\ntarget_minutes = \"oops\"\n",
    )?;
    fs::create_dir_all(temp.path().join("packet/plan.json"))?;
    let output = cli(temp.path(), "plan")
        .args(["--config", "invalid.toml", "--write"])
        .output()?;
    invalid(&output, "invalid.toml", &["gate.target_minutes"])?;
    // A failed writer must not produce the ordinary plan-success exit either.
    ensure!(!temp.path().join("packet/effective-config.json").exists());
    Ok(())
}

#[test]
fn plan_preserves_policy_diagnosis_when_a_later_revision_lookup_fails() -> Result<()> {
    let temp = fixture()?;
    let root = temp.path();
    let config = root.join("invalid.toml");
    fs::write(&config, "[gatee]\nvalue = 1\n")?;
    let invoke = |base: &str| -> Result<Output> {
        Ok(command(root, env!("CARGO_BIN_EXE_ub-review"))
            .args([
                "plan",
                "--root",
                ".",
                "--config",
                "invalid.toml",
                "--base",
                base,
                "--head",
                "HEAD",
                "--out",
                "packet",
                "--write",
            ])
            .output()?)
    };
    let missing = "missing-preflight-review-base";
    let invalid_and_missing = invoke(missing)?;
    invalid(&invalid_and_missing, "invalid.toml", &["gatee"])?;
    ensure!(console(&invalid_and_missing).contains(missing));
    ensure!(!root.join("packet").exists());

    // Correcting policy must not hide the still-invalid revision or invent a
    // policy error. Correcting both inputs restores an actual written plan.
    fs::write(&config, "[gate]\ntarget_minutes = 43\n")?;
    let valid_and_missing = invoke(missing)?;
    ensure!(valid_and_missing.status.code() == Some(1));
    ensure!(console(&valid_and_missing).contains(missing));
    ensure!(!console(&valid_and_missing).contains("invalid configuration"));
    ensure!(!root.join("packet").exists());
    let restored = invoke("HEAD^")?;
    ensure!(restored.status.success(), "{}", console(&restored));
    ensure!(root.join("packet/input/revision-admission.json").is_file());
    Ok(())
}

#[test]
fn override_only_errors_do_not_claim_a_discovered_file_exists() -> Result<()> {
    for subcommand in ["doctor", "plan"] {
        for from_env in [false, true] {
            let temp = fixture()?;
            let root = temp.path();
            ensure!(!root.join(".ub-review.toml").exists());
            let mut invocation = cli(root, subcommand);
            if from_env {
                invocation.env("UB_REVIEW_RUNTIME_PROFILE", "custom");
            } else {
                invocation.args(["--runtime-profile", "custom"]);
            }
            let output = invocation.output()?;
            invalid(&output, "optional discovery/defaults", &["profile"])?;
            let text = console(&output);
            ensure!(text.contains("runtime-profile override"));
            ensure!(text.contains("remove a --runtime-profile/--profile"));
            ensure!(!text.contains("from discovered"));
            ensure!(!root.join(".ub-review.toml").exists());
            ensure!(!root.join("packet").exists());
        }
    }
    let temp = fixture()?;
    let restored = cli(temp.path(), "plan")
        .args(["--runtime-profile", "gh-runner"])
        .output()?;
    ensure!(restored.status.success(), "{}", console(&restored));
    Ok(())
}

#[test]
fn rejected_tool_identifiers_and_fields_stay_out_of_diagnostic_artifacts() -> Result<()> {
    for subcommand in ["doctor", "plan"] {
        let temp = fixture()?;
        let root = temp.path();
        fs::write(
            root.join("invalid.toml"),
            format!(
                "[gate]\nhard_timeout_minutes = 71\n[tools]\n\"{CANARY}\" = 1\n[tools.tokmd]\n\"{CANARY}\" = 2\n"
            ),
        )?;
        let mut invocation = cli(root, subcommand);
        invocation.args(["--config", "invalid.toml"]);
        if subcommand == "plan" {
            invocation.arg("--write");
        }
        let output = invocation.output()?;
        invalid(
            &output,
            "invalid.toml",
            &["tools.[redacted id]", "tools.tokmd.[redacted field]"],
        )?;
        ensure!(
            console(&output)
                .lines()
                .filter(|line| line.starts_with("  - "))
                .count()
                == 2
        );
        if subcommand == "plan" {
            for name in [
                "effective-config.json",
                "resolved-profile.json",
                "resolved-plan.json",
                "plan.json",
            ] {
                let artifact = fs::read_to_string(root.join("packet").join(name))?;
                ensure!(!artifact.contains(CANARY), "rejected identifier in {name}");
            }
            let effective: serde_json::Value = serde_json::from_slice(&fs::read(
                root.join("packet/effective-config.json"),
            )?)?;
            ensure!(effective["policy_errors"].as_array().map(Vec::len) == Some(2));
            ensure!(effective["gate"]["hard_timeout_minutes"] == 71);
        }
    }
    Ok(())
}

#[test]
fn trusted_diff_admission_still_precedes_any_config_diagnostic() -> Result<()> {
    let temp = fixture()?;
    fs::write(temp.path().join("broken.toml"), "[gate")?;
    let output = cli(temp.path(), "plan")
        .args([
            "--config",
            "broken.toml",
            "--trusted-base-tree",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--write",
        ])
        .output()?;
    ensure!(output.status.code() == Some(1));
    let text = console(&output);
    ensure!(text.contains("trusted-base diff admission requires"));
    ensure!(!text.contains("parse broken.toml"));
    ensure!(!text.contains("invalid configuration"));
    ensure!(!temp.path().join("packet").exists());
    Ok(())
}
