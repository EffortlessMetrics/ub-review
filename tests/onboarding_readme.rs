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
    /// Create a GitHub-shaped fixture with inert release discovery.
    fn new() -> Result<Self> {
        Self::new_with_writer(|path, contents| fs::write(path, contents))
    }

    /// Create a fixture while permitting tests to interpose on source writes.
    fn new_with_writer(mut write: impl FnMut(&Path, &[u8]) -> std::io::Result<()>) -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("repository with spaces");
        let tools = temp.path().join("tools");
        fs::create_dir_all(root.join(".github/workflows"))?;
        fs::create_dir_all(root.join("src"))?;
        fs::create_dir(&tools)?;
        write(
            &root.join(".github/workflows/ci.yml"),
            ORIGINAL_CI.as_bytes(),
        )?;
        write(
            &root.join("Cargo.toml"),
            b"[package]\nname = \"onboarding-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
        )?;
        write(&root.join("src/lib.rs"), b"pub fn value() -> u8 { 7 }\n")?;
        let curl = tools.join("curl");
        write(
            &curl,
            b"#!/bin/sh\nset -eu\nprintf 'lookup\\n' >> \"$FIXTURE_CURL_MARKER\"\nprintf '%s\\n' '{\"tag_name\":\"v0.1.1\",\"assets\":[{\"name\":\"ub-review-x86_64-unknown-linux-gnu.tar.gz\"},{\"name\":\"ub-review-x86_64-unknown-linux-gnu.tar.gz.sha256\"}]}'\n",
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
        let mut remote = Command::new("git");
        remote.args([
            "remote",
            "add",
            "origin",
            "https://github.com/onboarding-fixture/example.git",
        ]);
        let (status, output) = fixture.execute(remote)?;
        ensure!(status.success(), "fixture origin setup: {output}");
        Ok(fixture)
    }

    /// Run a bounded child without inherited credentials or shell parsing.
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

    /// Select Cargo's built executable, never an ambient installed binary.
    fn cli(&self, args: &[&str]) -> Result<(ExitStatus, String)> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ub-review"));
        command.args(args);
        self.execute(command)
    }

    /// Check independent sentinel bytes after generation and each refusal.
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

/// Extract only the reviewed executable vocabulary from the first setup block.
fn first_run_commands(readme: &str) -> Result<Vec<Vec<&str>>> {
    let (_, section) = readme
        .split_once("## Inspect before adopting\n")
        .context("README onboarding section missing")?;
    ensure!(!section.starts_with("## "), "command block missing");
    let section = section.split("\n## ").next().context("empty section")?;
    let (_, block) = section
        .split_once("```bash\n")
        .context("command block missing")?;
    let (block, _) = block
        .split_once("\n```")
        .context("unclosed command block")?;
    let commands: Vec<Vec<&str>> = block
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
        .collect::<Result<_>>()?;
    ensure!(!commands.is_empty(), "README must contain a first-run path");
    Ok(commands)
}

/// The documented generated path must work and refuse destructive repetition.
#[test]
fn readme_first_run_creates_advisory_setup_without_conflicting_commands() -> Result<()> {
    let readme = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))?;
    let commands = first_run_commands(&readme)?;
    ensure!(
        commands
            == [
                vec![
                    "enable",
                    "--inspect",
                    "--mode",
                    "advisory",
                    "--model",
                    "minimax"
                ],
                vec!["audit-ci"],
            ],
        "README first-run path must enable advisory review followed by audit-ci; keep optional migration commands separate"
    );
    let fixture = Fixture::new()?;
    for args in &commands {
        let (status, output) = fixture.cli(args)?;
        ensure!(
            status.success(),
            "documented command {args:?} failed: {output}"
        );
    }
    let workflow_path = fixture.root.join(".github/workflows/ub-review.yml");
    let workflow = fs::read_to_string(&workflow_path)?;
    ensure!(
        workflow.contains("review-mode: advisory\n"),
        "first run must be advisory"
    );
    ensure!(
        workflow.contains("install-mode: release\n"),
        "release-only fixture was not selected"
    );
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
        ensure!(
            !status.success() && output.contains("already exists"),
            "{output}"
        );
        ensure!(fs::read(&config_path)? == config_bytes);
        ensure!(fs::read_to_string(&workflow_path)? == workflow);
        fixture.original_files_are_unchanged()?;
    }
    Ok(())
}

/// Manual init remains useful without authorizing enable to overwrite it.
#[test]
fn manual_init_is_an_alternative_and_enable_preserves_its_config() -> Result<()> {
    let fixture = Fixture::new()?;
    let (status, output) = fixture.cli(&["init", "--profile", "gh-runner"])?;
    ensure!(status.success(), "manual init failed: {output}");
    let config_path = fixture.root.join(".ub-review.toml");
    let config = fs::read(&config_path)?;
    for _ in 0..2 {
        let (status, output) = fixture.cli(&[
            "enable",
            "--inspect",
            "--mode",
            "advisory",
            "--model",
            "minimax",
        ])?;
        ensure!(
            !status.success() && output.contains("already exists"),
            "{output}"
        );
        ensure!(fs::read(&config_path)? == config);
        ensure!(
            !fixture
                .root
                .join(".github/workflows/ub-review.yml")
                .exists()
        );
        ensure!(!fixture.temp.path().join("curl-called").exists());
        fixture.original_files_are_unchanged()?;
    }
    Ok(())
}

/// Pin exact argument order independently of the README's current contents.
#[test]
fn first_run_commands_preserves_reviewed_argv_and_stops_at_first_block() -> Result<()> {
    let document = "## Inspect before adopting\n\n```bash\nub-review enable --inspect --mode advisory --model minimax\n\nub-review audit-ci\n```\n\n```bash\nnot a first-run command\n```\n";
    assert_eq!(
        first_run_commands(document)?,
        vec![
            vec![
                "enable",
                "--inspect",
                "--mode",
                "advisory",
                "--model",
                "minimax"
            ],
            vec!["audit-ci"],
        ]
    );
    Ok(())
}

/// Malformed structure must not borrow a command block from a later section.
#[test]
fn first_run_commands_rejects_missing_and_unclosed_blocks() -> Result<()> {
    for (document, diagnostic) in [
        (
            "# Other\n```bash\nub-review audit-ci\n```\n",
            "README onboarding section missing",
        ),
        (
            "## Inspect before adopting\nNo commands\n",
            "command block missing",
        ),
        (
            "## Inspect before adopting\n## Later\n```bash\nub-review audit-ci\n```\n",
            "command block missing",
        ),
        (
            "## Inspect before adopting\n```bash\nub-review audit-ci\n",
            "unclosed command block",
        ),
        (
            "## Inspect before adopting\n```bash\nub-review audit-ci\n## Later\n```\n",
            "unclosed command block",
        ),
    ] {
        let result = first_run_commands(document);
        assert!(result.is_err(), "malformed document accepted: {document}");
        let error = result.err().context("expected parser rejection")?;
        assert_eq!(error.to_string(), diagnostic);
    }
    Ok(())
}

/// An empty command block is not a successfully admitted first-use path.
#[test]
fn first_run_commands_rejects_empty_admission() -> Result<()> {
    for block in ["", "\n", " \n\t\n"] {
        let document = format!("## Inspect before adopting\n```bash\n{block}\n```\n");
        let result = first_run_commands(&document);
        assert!(result.is_err(), "empty first-run path was admitted");
        let error = result.err().context("expected empty-path rejection")?;
        assert_eq!(error.to_string(), "README must contain a first-run path");
    }
    Ok(())
}

/// Rejected text cannot execute a valid prefix or a documentation-owned shell.
#[test]
fn first_run_commands_rejects_unreviewed_text_without_side_effects() -> Result<()> {
    let fixture = Fixture::new()?;
    let marker = fixture.root.join("unreviewed-command-ran");
    let shell_command = format!("touch '{}'", marker.display());
    for command in [
        shell_command.as_str(),
        "ub-review audit-ci; touch unreviewed-command-ran",
        "ub-review audit-ci $(touch unreviewed-command-ran)",
        "ub-review enable --inspect --mode advisory --model minimax --force",
        "ub-review audit-ci --unreviewed",
        " ub-review audit-ci",
    ] {
        let document =
            format!("## Inspect before adopting\n```bash\nub-review audit-ci\n{command}\n```\n");
        let result = first_run_commands(&document);
        assert!(result.is_err(), "unreviewed command accepted: {command}");
        let error = result.err().context("expected vocabulary rejection")?;
        assert_eq!(
            error.to_string(),
            format!("unreviewed onboarding command: {command}")
        );
        assert!(!marker.exists());
        assert!(!fixture.root.join("target").exists());
        assert!(!fixture.temp.path().join("curl-called").exists());
    }
    Ok(())
}

/// Fixture writes and the fake release lookup have independent content oracles.
#[test]
fn fixture_new_writes_exact_sources_and_inert_release_metadata() -> Result<()> {
    let fixture = Fixture::new()?;
    assert_eq!(
        fs::read_to_string(fixture.root.join("Cargo.toml"))?,
        "[package]\nname = \"onboarding-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("src/lib.rs"))?,
        "pub fn value() -> u8 { 7 }\n"
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join(".github/workflows/ci.yml"))?,
        ORIGINAL_CI
    );
    assert!(!fixture.temp.path().join("curl-called").exists());
    let (status, output) = fixture.execute(Command::new("curl"))?;
    assert_eq!(status.code(), Some(0));
    let metadata: serde_json::Value = serde_json::from_str(&output)?;
    assert_eq!(metadata["tag_name"], "v0.1.1");
    assert_eq!(metadata["assets"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        metadata["assets"][0]["name"],
        "ub-review-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(
        metadata["assets"][1]["name"],
        "ub-review-x86_64-unknown-linux-gnu.tar.gz.sha256"
    );
    assert_eq!(
        fs::read_to_string(fixture.temp.path().join("curl-called"))?,
        "lookup\n"
    );
    Ok(())
}

/// Constructor writes must propagate real target errors and release the fixture.
#[test]
fn fixture_new_reports_manifest_and_curl_write_errors_and_cleans_up() -> Result<()> {
    for (target_name, temp_depth) in [
        (".github/workflows/ci.yml", 4),
        ("Cargo.toml", 2),
        ("src/lib.rs", 3),
        ("tools/curl", 2),
    ] {
        let mut target_path = None;
        let mut temp_path = None;
        let result = Fixture::new_with_writer(|path, contents| {
            if path.ends_with(target_name) {
                fs::create_dir(path)?;
                target_path = Some(path.to_owned());
                // Include the repository/tools directory, not just nested parents.
                temp_path = path.ancestors().nth(temp_depth).map(Path::to_owned);
            }
            fs::write(path, contents)
        });
        let error = result
            .err()
            .with_context(|| format!("{target_name} write error was swallowed"))?;
        assert_eq!(
            error
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind),
            Some(std::io::ErrorKind::IsADirectory),
            "unexpected {target_name} write error: {error:#}"
        );
        let target_path = target_path.context("target write was not reached")?;
        let temp_path = temp_path.context("fixture temporary path was not captured")?;
        assert!(target_path.starts_with(&temp_path));
        assert!(!temp_path.exists(), "failed fixture was not cleaned up");
    }
    Ok(())
}

/// Each protected file must independently distinguish corruption from no change.
#[test]
fn original_files_are_unchanged_detects_each_corrupted_sentinel() -> Result<()> {
    let fixture = Fixture::new()?;
    assert!(fixture.original_files_are_unchanged().is_ok());
    let ci_path = fixture.root.join(".github/workflows/ci.yml");
    let source_path = fixture.root.join("src/lib.rs");
    fs::write(&ci_path, "changed CI\n")?;
    let ci_error = fixture
        .original_files_are_unchanged()
        .err()
        .context("CI corruption escaped")?;
    assert_eq!(ci_error.to_string(), "onboarding changed existing CI");
    fs::write(&ci_path, ORIGINAL_CI)?;
    assert!(fixture.original_files_are_unchanged().is_ok());
    fs::write(&source_path, "pub fn value() -> u8 { 8 }\n")?;
    let source_error = fixture
        .original_files_are_unchanged()
        .err()
        .context("source corruption escaped")?;
    assert_eq!(
        source_error.to_string(),
        "onboarding changed repository source"
    );
    fs::write(&source_path, "pub fn value() -> u8 { 7 }\n")?;
    assert!(fixture.original_files_are_unchanged().is_ok());
    Ok(())
}

/// Preserve both child streams and distinguish success from a nonzero exit.
#[test]
fn execute_preserves_exit_status_streams_and_clears_credentials() -> Result<()> {
    let fixture = Fixture::new()?;
    let entries_before = fs::read_dir(fixture.temp.path())?.count();
    for code in [0, 7] {
        let mut command = Command::new("sh");
        command.env("ONBOARDING_SHOULD_NOT_LEAK", "fixture-secret");
        command.args([
            "-c",
            "test -z \"${ONBOARDING_SHOULD_NOT_LEAK+x}\" || exit 99; printf stdout-marker; printf stderr-marker >&2; exit \"$1\"",
            "onboarding-child",
            &code.to_string(),
        ]);
        let (status, output) = fixture.execute(command)?;
        assert_eq!(status.code(), Some(code));
        assert_eq!(output, "stdout-marker\nstderr-marker");
        assert_eq!(fs::read_dir(fixture.temp.path())?.count(), entries_before);
    }
    Ok(())
}

/// Spawn failure must be an error and leave no temporary log files behind.
#[test]
fn execute_reports_missing_executable_and_removes_logs() -> Result<()> {
    let fixture = Fixture::new()?;
    let entries_before = fs::read_dir(fixture.temp.path())?.count();
    let result = fixture.execute(Command::new(fixture.root.join("missing-executable")));
    assert!(result.is_err());
    let error = result.err().context("spawn failure was swallowed")?;
    assert_eq!(
        error
            .downcast_ref::<std::io::Error>()
            .map(std::io::Error::kind),
        Some(std::io::ErrorKind::NotFound)
    );
    assert_eq!(fs::read_dir(fixture.temp.path())?.count(), entries_before);
    Ok(())
}

/// The timed-out process is replaced by sleep, so no shell descendant is left.
#[test]
fn execute_timeout_terminates_reaps_and_cleans_its_owned_child() -> Result<()> {
    let fixture = Fixture::new()?;
    let entries_before = fs::read_dir(fixture.temp.path())?.count();
    let mut child = Command::new("sh");
    child.args([
        "-c",
        "printf '%s' \"$$\" > timeout-child.pid; exec sleep 60",
    ]);
    let result = fixture.execute(child);
    assert!(result.is_err(), "timeout unexpectedly completed");
    let error = result.err().context("timeout was swallowed")?;
    assert_eq!(
        error.to_string(),
        "onboarding fixture command exceeded 20 seconds"
    );
    let pid_text = fs::read_to_string(fixture.root.join("timeout-child.pid"))?;
    let pid: u32 = pid_text.parse()?;
    assert_ne!(pid, std::process::id());
    let mut probe = Command::new("kill");
    probe.args(["-0", &pid_text]);
    let (status, _) = fixture.execute(probe)?;
    assert!(
        !status.success(),
        "timed-out child {pid} is still alive or unreaped"
    );
    assert_eq!(fs::read_dir(fixture.temp.path())?.count(), entries_before);
    let temp_path = fixture.temp.path().to_owned();
    drop(fixture);
    assert!(!temp_path.exists());
    Ok(())
}
