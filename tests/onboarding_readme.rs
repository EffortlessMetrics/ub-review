//! Run the README's actual first-run commands against isolated repositories.
//! Release discovery is an inert fixture; no installer or provider is invoked.

#![cfg(unix)]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use wait_timeout::ChildExt;

const ORIGINAL_CI: &str = "name: existing-ci\non: [push]\njobs:\n  check:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo existing-ci\n";

struct Fixture {
    temp: tempfile::TempDir,
    root: PathBuf,
    path: OsString,
}

impl Fixture {
    fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repository with spaces");
        let tools = temp.path().join("tools");
        fs::create_dir_all(root.join(".github/workflows"))?;
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir(&tools)?;
        fs::write(root.join(".github/workflows/ci.yml"), ORIGINAL_CI)?;
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"onboarding-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
        )?;
        fs::write(root.join("src/lib.rs"), "pub fn value() -> u8 { 7 }\n")?;
        let curl = tools.join("curl");
        fs::write(
            &curl,
            "#!/bin/sh\nset -eu\nprintf 'lookup\\n' >> \"$FIXTURE_CURL_MARKER\"\nprintf '%s\\n' '{\"tag_name\":\"v0.1.1\",\"assets\":[{\"name\":\"ub-review-x86_64-unknown-linux-gnu.tar.gz\"},{\"name\":\"ub-review-x86_64-unknown-linux-gnu.tar.gz.sha256\"}]}'\n",
        )?;
        fs::set_permissions(&curl, fs::Permissions::from_mode(0o755))?;
        let mut paths = vec![tools];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").context("PATH missing")?,
        ));
        let path = std::env::join_paths(paths)?;
        let fixture = Self { temp, root, path };
        let mut git = Command::new("git");
        git.arg("init").arg("--quiet").arg(&fixture.root);
        let (status, output) = fixture.execute(git)?;
        ensure!(status.success(), "fixture git init: {output}");
        Ok(fixture)
    }

    fn execute(&self, mut command: Command) -> Result<(ExitStatus, String)> {
        let stdout = tempfile::NamedTempFile::new_in(self.temp.path())?;
        let stderr = tempfile::NamedTempFile::new_in(self.temp.path())?;
        command
            .current_dir(&self.root)
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", self.temp.path())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("FIXTURE_CURL_MARKER", self.temp.path().join("curl-called"))
            .stdin(Stdio::null())
            .stdout(stdout.reopen()?)
            .stderr(stderr.reopen()?);
        let mut child = command.spawn()?;
        let status = match child.wait_timeout(Duration::from_secs(20))? {
            Some(status) => status,
            None => {
                child.kill()?;
                child.wait()?;
                bail!("onboarding fixture command exceeded 20 seconds");
            }
        };
        let output = format!(
            "{}\n{}",
            fs::read_to_string(stdout.path())?,
            fs::read_to_string(stderr.path())?
        );
        Ok((status, output))
    }

    fn cli(&self, args: &[&str]) -> Result<(ExitStatus, String)> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ub-review"));
        command.args(args);
        self.execute(command)
    }

    fn original_files_are_unchanged(&self) -> Result<()> {
        ensure!(
            fs::read_to_string(self.root.join(".github/workflows/ci.yml"))? == ORIGINAL_CI,
            "onboarding changed existing CI"
        );
        ensure!(
            fs::read_to_string(self.root.join("src/lib.rs"))? == "pub fn value() -> u8 { 7 }\n",
            "onboarding changed repository source"
        );
        Ok(())
    }
}

fn first_run_commands(readme: &str) -> Result<Vec<Vec<&str>>> {
    let (_, section) = readme
        .split_once("## Inspect before adopting\n")
        .context("README onboarding section missing")?;
    let section = section.split("\n## ").next().context("empty section")?;
    let (_, block) = section.split_once("```bash\n").context("command block missing")?;
    let (block, _) = block.split_once("\n```").context("unclosed command block")?;
    block
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            // Execute only the bounded generator/read-only vocabulary, never
            // a shell or arbitrary code copied from documentation.
            ensure!(
                matches!(
                    line,
                    "ub-review init --profile gh-runner"
                        | "ub-review enable --inspect --mode gate --model minimax"
                        | "ub-review enable --inspect --mode advisory --model minimax"
                        | "ub-review audit-ci"
                        | "ub-review setup-ci --print-pr"
                ),
                "unreviewed onboarding command: {line}"
            );
            Ok(line.split_whitespace().skip(1).collect())
        })
        .collect()
}

#[test]
fn readme_first_run_creates_advisory_setup_without_conflicting_commands() -> Result<()> {
    let readme = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))?;
    let commands = first_run_commands(&readme)?;
    ensure!(!commands.is_empty(), "README must contain a first-run path");
    let fixture = Fixture::new()?;
    for args in &commands {
        let (status, output) = fixture.cli(args)?;
        ensure!(status.success(), "documented command {args:?} failed: {output}");
    }
    let workflow_path = fixture.root.join(".github/workflows/ub-review.yml");
    let workflow = fs::read_to_string(&workflow_path)?;
    ensure!(workflow.contains("review-mode: advisory\n"), "first run must be advisory");
    ensure!(workflow.contains("install-mode: release\n"), "release-only fixture was not selected");
    ensure!(workflow.contains("EffortlessMetrics/ub-review@v0.1.1\n"));
    ensure!(!workflow.contains("pull_request_target"));
    let config_path = fixture.root.join(".ub-review.toml");
    let config_bytes = fs::read(&config_path)?;
    let config: toml::Value = toml::from_str(std::str::from_utf8(&config_bytes)?)?;
    ensure!(config["repo"]["kind"].as_str() == Some("rust"));
    ensure!(config["repo"]["ledger"].as_str() == Some(""));
    ensure!(fs::read_to_string(fixture.temp.path().join("curl-called"))? == "lookup\n");
    fixture.original_files_are_unchanged()?;

    // Re-running the documented path must refuse, not overwrite. This also
    // prevents fixing the README regression by inserting blanket --force.
    let first = commands.first().context("first command missing")?;
    for _ in 0..2 {
        let (status, output) = fixture.cli(first)?;
        ensure!(!status.success() && output.contains("already exists"), "{output}");
        ensure!(fs::read(&config_path)? == config_bytes);
        ensure!(fs::read_to_string(&workflow_path)? == workflow);
        fixture.original_files_are_unchanged()?;
    }
    Ok(())
}

#[test]
fn manual_init_is_an_alternative_and_enable_preserves_its_config() -> Result<()> {
    let fixture = Fixture::new()?;
    let (status, output) = fixture.cli(&["init", "--profile", "gh-runner"])?;
    ensure!(status.success(), "manual init failed: {output}");
    let config_path = fixture.root.join(".ub-review.toml");
    let config = fs::read(&config_path)?;
    for _ in 0..2 {
        let (status, output) = fixture.cli(&[
            "enable", "--inspect", "--mode", "advisory", "--model", "minimax",
        ])?;
        ensure!(!status.success() && output.contains("already exists"), "{output}");
        ensure!(fs::read(&config_path)? == config);
        ensure!(!fixture.root.join(".github/workflows/ub-review.yml").exists());
        ensure!(!fixture.temp.path().join("curl-called").exists());
        fixture.original_files_are_unchanged()?;
    }
    Ok(())
}
