use anyhow::{Result, anyhow, ensure};
#[cfg(unix)]
use std::process::Command;

/// Pins source-install admission and, on Unix, executes the copied Action
/// runner against valid, invalid, and stale lockfile fixtures.
#[test]
fn source_runner_requires_committed_regular_lockfile_before_locked_build() -> Result<()> {
    let action = include_str!("../action.yml");
    ensure!(
        !action.contains("cargo generate-lockfile"),
        "source mode must not mutate dependency resolution"
    );
    let lockfile = action
        .find("lockfile=\"$workdir/Cargo.lock\"")
        .ok_or_else(|| anyhow!("source runner must name the copied Cargo.lock"))?;
    let guard = action
        .find("if [[ -L \"$lockfile\" || ! -f \"$lockfile\" ]]; then")
        .ok_or_else(|| anyhow!("source runner must reject symlinked and non-regular lockfiles"))?;
    let build = action
        .find("cargo build --manifest-path \"$workdir/Cargo.toml\" --locked --release --target-dir \"$cargo_target_dir\"")
        .ok_or_else(|| anyhow!("source runner must execute one locked build"))?;
    ensure!(
        lockfile < guard && guard < build,
        "lock admission must precede Cargo execution"
    );
    ensure!(
        action.contains("source install requires a committed regular Cargo.lock"),
        "missing lockfile failure must be actionable"
    );

    #[cfg(unix)]
    {
        let output = Command::new("bash")
            .arg("fixtures/action-source-lockfile/contract.sh")
            .arg("action.yml")
            .output()
            .map_err(|error| anyhow!("execute source lockfile fixture: {error}"))?;
        ensure!(
            output.status.success(),
            "source lockfile fixture failed\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(())
}
