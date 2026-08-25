use std::fs;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result, ensure};

const ACTION: &str = include_str!("../action.yml");

fn source_runner_script() -> Result<String> {
    let start = ACTION
        .find("        set -euo pipefail\n\n        mode=")
        .context("source runner block start is missing")?;
    let end = ACTION[start..]
        .find("\n    - name: Install advisory sensors")
        .context("source runner block end is missing")?
        + start;
    Ok(ACTION[start..end]
        .lines()
        .map(|line| line.strip_prefix("        ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[test]
fn source_runner_requires_committed_regular_lockfile_before_locked_build() -> Result<()> {
    let runner = source_runner_script()?;
    ensure!(
        !runner.contains("cargo generate-lockfile"),
        "source mode must not mutate dependency resolution"
    );
    let lockfile = runner
        .find("lockfile=\"$workdir/Cargo.lock\"")
        .context("source runner must name the copied Cargo.lock")?;
    let guard = runner
        .find("if [[ -L \"$lockfile\" || ! -f \"$lockfile\" ]]; then")
        .context("source runner must reject symlinked and non-regular lockfiles")?;
    let build = runner
        .find("cargo build --manifest-path \"$workdir/Cargo.toml\" --locked --release --target-dir \"$cargo_target_dir\"")
        .context("source runner must execute one locked build")?;
    ensure!(lockfile < guard && guard < build, "lock admission must precede Cargo execution");
    ensure!(
        runner.contains("source install requires a committed regular Cargo.lock"),
        "missing lockfile failure must be actionable"
    );
    Ok(())
}

#[cfg(unix)]
mod unix {
    use std::os::unix::fs::{PermissionsExt, symlink};

    use super::*;

    #[derive(Clone, Copy)]
    enum LockFixture {
        Regular,
        Missing,
        Directory,
        Symlink,
    }

    fn write_action_fixture(root: &Path, lock: LockFixture) -> Result<()> {
        fs::create_dir_all(root)?;
        fs::write(
            root.join("Cargo.toml"),
            "[package]\nname = \"ub-review\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )?;
        match lock {
            LockFixture::Regular => fs::write(root.join("Cargo.lock"), "version = 4\n")?,
            LockFixture::Missing => {}
            LockFixture::Directory => fs::create_dir(root.join("Cargo.lock"))?,
            LockFixture::Symlink => {
                fs::write(root.join("committed.lock"), "version = 4\n")?;
                symlink("committed.lock", root.join("Cargo.lock"))?;
            }
        }
        Ok(())
    }

    fn write_fake_cargo(bin_dir: &Path) -> Result<()> {
        fs::create_dir_all(bin_dir)?;
        let cargo = bin_dir.join("cargo");
        fs::write(
            &cargo,
            r#"#!/bin/sh
set -eu
printf '%s\n' "$*" >> "$FAKE_CARGO_LOG"
if [ "${FAKE_CARGO_STALE:-}" = "1" ]; then
  echo "the lock file needs to be updated but --locked was passed" >&2
  exit 101
fi
target=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --target-dir)
      target="$2"
      shift 2
      ;;
    *)
      shift
      ;;
  esac
done
if [ -z "$target" ]; then
  echo "missing --target-dir" >&2
  exit 2
fi
mkdir -p "$target/release"
printf '#!/bin/sh\nexit 0\n' > "$target/release/ub-review"
chmod +x "$target/release/ub-review"
"#,
        )?;
        let mut permissions = fs::metadata(&cargo)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(cargo, permissions)?;
        Ok(())
    }

    fn run_source_fixture(root: &Path, lock: LockFixture, stale: bool) -> Result<(Output, String)> {
        let action_root = root.join("action");
        let runner_root = root.join("runner");
        let fake_bin = root.join("fake-bin");
        let cargo_log = root.join("cargo.log");
        let output_file = root.join("github-output");
        let target = root.join("target-cache");
        write_action_fixture(&action_root, lock)?;
        write_fake_cargo(&fake_bin)?;
        fs::create_dir_all(&runner_root)?;
        let script = runner_root.join("source-runner.sh");
        fs::write(&script, format!("{}\n", source_runner_script()?))?;
        let mut permissions = fs::metadata(&script)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&script, permissions)?;

        let inherited_path = std::env::var("PATH").unwrap_or_default();
        let path = format!("{}:{inherited_path}", fake_bin.display());
        let mut command = Command::new("bash");
        command
            .arg(&script)
            .current_dir(&runner_root)
            .env("PATH", path)
            .env("RUNNER_TEMP", &runner_root)
            .env("GITHUB_OUTPUT", &output_file)
            .env("GITHUB_ACTION_PATH", &action_root)
            .env("CARGO_TARGET_DIR", &target)
            .env("FAKE_CARGO_LOG", &cargo_log)
            .env("UB_REVIEW_INSTALL_MODE", "source")
            .env("UB_REVIEW_BINARY_PATH", "")
            .env("UB_REVIEW_RELEASE_VERSION", "")
            .env("UB_REVIEW_RELEASE_ASSET", "ub-review.tar.gz")
            .env("UB_REVIEW_ACTION_REPOSITORY", "")
            .env("UB_REVIEW_ACTION_REF", "")
            .env("UB_REVIEW_SERVER_URL", "https://fixture.invalid");
        if stale {
            command.env("FAKE_CARGO_STALE", "1");
        }
        let output = command.output().context("execute extracted source runner")?;
        let log = fs::read_to_string(cargo_log).unwrap_or_default();
        Ok((output, log))
    }

    fn combined_output(output: &Output) -> String {
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

    #[test]
    fn source_runner_executes_one_locked_build_with_regular_lockfile() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (output, log) = run_source_fixture(temp.path(), LockFixture::Regular, false)?;
        ensure!(output.status.success(), "valid source fixture failed: {}", combined_output(&output));
        ensure!(
            log.lines().count() == 1,
            "source mode must invoke Cargo exactly once: {log}"
        );
        ensure!(log.starts_with("build --manifest-path "), "unexpected Cargo command: {log}");
        ensure!(log.contains(" --locked --release --target-dir "), "locked build flags missing: {log}");
        ensure!(!log.contains("generate-lockfile"), "resolver mutation reached Cargo: {log}");
        Ok(())
    }

    #[test]
    fn source_runner_rejects_missing_directory_and_symlink_lockfiles_before_cargo() -> Result<()> {
        for lock in [
            LockFixture::Missing,
            LockFixture::Directory,
            LockFixture::Symlink,
        ] {
            let temp = tempfile::tempdir()?;
            let (output, log) = run_source_fixture(temp.path(), lock, false)?;
            let combined = combined_output(&output);
            ensure!(!output.status.success(), "invalid lockfile fixture unexpectedly passed");
            ensure!(
                combined.contains("source install requires a committed regular Cargo.lock"),
                "invalid lockfile did not report the admission failure: {combined}"
            );
            ensure!(log.trim().is_empty(), "Cargo ran before lockfile admission: {log}");
        }
        Ok(())
    }

    #[test]
    fn source_runner_fails_closed_when_locked_build_reports_stale_lockfile() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (output, log) = run_source_fixture(temp.path(), LockFixture::Regular, true)?;
        let combined = combined_output(&output);
        ensure!(!output.status.success(), "stale lockfile fixture unexpectedly passed");
        ensure!(
            combined.contains("the lock file needs to be updated but --locked was passed"),
            "Cargo stale-lock failure was not preserved: {combined}"
        );
        ensure!(log.contains(" --locked --release --target-dir "), "stale fixture did not use --locked: {log}");
        ensure!(!log.contains("generate-lockfile"), "stale lockfile was regenerated: {log}");
        Ok(())
    }
}
