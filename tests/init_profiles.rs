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
                anyhow::ensure!(
                    diagnostic.contains("init --profile custom")
                        && diagnostic.contains("[profiles.custom]"),
                    "missing actionable rejection: {diagnostic}"
                );
                anyhow::ensure!(
                    !diagnostic.contains("wrote "),
                    "rejected init reported a write: {diagnostic}"
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
    table.insert("profile".to_owned(), toml::Value::String("custom".to_owned()));
    let profiles = table
        .get_mut("profiles")
        .and_then(toml::Value::as_table_mut)
        .context("starter has no profile definitions")?;
    let definition = profiles
        .get("gh-runner")
        .cloned()
        .context("starter has no gh-runner definition")?;
    profiles.insert("custom".to_owned(), definition);
    let config = temp.path().join("defined-custom.toml");
    write_file(&config, &toml::to_string_pretty(&custom)?)?;
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
            "--runtime-profile",
            "custom",
        ],
    )?;
    Ok(())
}
