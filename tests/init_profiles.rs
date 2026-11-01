//! Executable init-profile contracts for issue #1556.

use std::fs;

use anyhow::{Context, Result};

#[path = "common/cli_run.rs"]
mod common;
use common::*;

#[test]
fn init_custom_profile_refuses_before_creating_or_overwriting_outputs() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    for no_guide in [false, true] {
        for force in [false, true] {
            for existing in [false, true] {
                let temp = tempfile::tempdir()?;
                let config = temp.path().join("custom.toml");
                let guide = temp.path().join("custom-guide.md");
                if existing {
                    write_file(&config, "config sentinel\n")?;
                    write_file(&guide, "guide sentinel\n")?;
                }
                let mut args = vec![
                    "init",
                    "--profile",
                    "custom",
                    "--path",
                    path_str(&config)?,
                    "--guide-out",
                    path_str(&guide)?,
                ];
                if no_guide {
                    args.push("--no-guide");
                }
                if force {
                    args.push("--force");
                }
                let diagnostic = run_expect_failure(temp.path(), bin, &args)?;
                assert_eq!(
                    diagnostic,
                    "\nError: `init --profile custom` cannot generate a valid starter without a \
                     [profiles.custom] definition; use a built-in --profile value or author the \
                     config manually with [profiles.custom] before running `ub-review doctor`\n",
                    "custom init must return the specific recovery diagnostic without reporting writes"
                );
                if existing {
                    anyhow::ensure!(fs::read(&config)? == b"config sentinel\n");
                    anyhow::ensure!(fs::read(&guide)? == b"guide sentinel\n");
                } else {
                    anyhow::ensure!(!config.exists(), "rejected init created its config");
                    anyhow::ensure!(!guide.exists(), "rejected init created its guide");
                }
            }
        }
    }
    Ok(())
}

#[test]
fn init_builtin_profiles_and_config_owned_custom_remain_usable() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    for profile in [
        "gh-runner",
        "gh-runner-standard",
        "gh-runner-full",
        "cx23",
        "cx33",
        "cx43",
        "auto",
    ] {
        let config = temp.path().join(format!("{profile}.toml"));
        run(
            &repo,
            bin,
            &[
                "init",
                "--profile",
                profile,
                "--path",
                path_str(&config)?,
                "--no-guide",
            ],
        )?;
        let text = fs::read_to_string(&config)?;
        let parsed: toml::Value = toml::from_str(&text)?;
        anyhow::ensure!(parsed.get("profile").and_then(toml::Value::as_str) == Some(profile));
        run(
            &repo,
            bin,
            &[
                "plan",
                "--root",
                path_str(&repo)?,
                "--base",
                "HEAD^",
                "--head",
                "HEAD",
                "--config",
                path_str(&config)?,
            ],
        )?;
    }

    let built_in = fs::read_to_string(temp.path().join("gh-runner.toml"))?;
    let mut custom: toml::Value = toml::from_str(&built_in)?;
    let table = custom.as_table_mut().context("starter is not a table")?;
    table.insert(
        "profile".to_owned(),
        toml::Value::String("custom".to_owned()),
    );
    let profiles = table
        .get_mut("profiles")
        .and_then(toml::Value::as_table_mut)
        .context("starter has no profile definitions")?;
    let mut definition = profiles
        .get("gh-runner")
        .cloned()
        .context("starter has no gh-runner definition")?;
    let definition_table = definition
        .as_table_mut()
        .context("profile definition is not a table")?;
    // The table key selects the profile even when the embedded name is omitted.
    definition_table.remove("name");
    let limits = definition_table
        .get_mut("limits")
        .and_then(toml::Value::as_table_mut)
        .context("profile definition has no limits")?;
    limits.insert("llm_in_flight".to_owned(), toml::Value::Integer(1));
    profiles.insert("custom".to_owned(), definition);
    let config = temp.path().join("defined-custom.toml");
    write_file(&config, &toml::to_string_pretty(&custom)?)?;
    let out = temp.path().join("custom-plan");
    let output = isolated_command(bin, &repo)
        .args([
            "plan",
            "--root",
            path_str(&repo)?,
            "--base",
            "HEAD^",
            "--head",
            "HEAD",
            "--config",
            path_str(&config)?,
            "--runtime-profile",
            "custom",
            "--write",
            "--out",
            path_str(&out)?,
        ])
        .output()?;
    anyhow::ensure!(
        output.status.success(),
        "custom plan failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout)?;
    assert!(
        stdout.lines().any(|line| line == "Profile: custom"),
        "plan did not select the custom profile: {stdout}"
    );
    let effective: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("effective-config.json"))?)?;
    assert_eq!(effective["profile"], "custom");
    assert!(effective.get("policy_errors").is_none());
    let resolved: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("resolved-profile.json"))?)?;
    assert_eq!(resolved["selected_runtime_profile"], "custom");
    assert_eq!(resolved["profile"]["name"], "custom");
    assert_eq!(resolved["profile"]["limits"]["llm_in_flight"], 1);
    Ok(())
}

#[test]
fn plan_preserves_explicit_builtin_sensor_timeout_zero() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    init_minimal_repo(&repo)?;
    let bin = env!("CARGO_BIN_EXE_ub-review");
    for (name, timeout, expected) in [
        ("profile", "", 720),
        ("zero", "timeout_sec = 0\n", 1),
        ("explicit", "timeout_sec = 123\n", 123),
    ] {
        let config = temp.path().join(format!("{name}.toml"));
        write_file(
            &config,
            &format!("profile = \"gh-runner\"\n[tools.ripr]\ndefault = \"always\"\n{timeout}"),
        )?;
        let out = temp.path().join(name);
        run(
            &repo,
            bin,
            &[
                "plan",
                "--root",
                path_str(&repo)?,
                "--base",
                "HEAD^",
                "--head",
                "HEAD",
                "--config",
                path_str(&config)?,
                "--write",
                "--out",
                path_str(&out)?,
            ],
        )?;
        let plan: serde_json::Value = serde_json::from_slice(&fs::read(out.join("plan.json"))?)?;
        let sensors = plan["sensors"].as_array().context("plan sensors missing")?;
        let ripr = sensors
            .iter()
            .find(|sensor| sensor["id"] == "ripr")
            .context("ripr missing")?;
        assert_eq!(ripr["timeout_sec"], expected, "case {name}");
    }
    Ok(())
}
