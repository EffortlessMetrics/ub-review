#[cfg(unix)]
use std::process::Command;

/// Pins source-install admission and, on Unix, executes the copied Action
/// runner against valid, invalid, and stale lockfile fixtures.
#[test]
fn source_runner_requires_committed_regular_lockfile_before_locked_build() {
    let action = include_str!("../action.yml");
    assert!(
        !action.contains("cargo generate-lockfile"),
        "source mode must not mutate dependency resolution"
    );

    let missing = action.len();
    let lockfile = action
        .find("lockfile=\"$workdir/Cargo.lock\"")
        .unwrap_or(missing);
    let guard = action
        .find("if [[ -L \"$lockfile\" || ! -f \"$lockfile\" ]]; then")
        .unwrap_or(missing);
    let build = action
        .find("cargo build --manifest-path \"$workdir/Cargo.toml\" --locked --release --target-dir \"$cargo_target_dir\"")
        .unwrap_or(missing);
    assert_ne!(
        lockfile, missing,
        "source runner must name the copied Cargo.lock"
    );
    assert_ne!(
        guard, missing,
        "source runner must reject symlinked and non-regular lockfiles"
    );
    assert_ne!(
        build, missing,
        "source runner must execute one locked build"
    );
    assert!(
        lockfile < guard && guard < build,
        "lock admission must precede Cargo execution"
    );
    assert!(
        action.contains("source install requires a committed regular Cargo.lock"),
        "missing lockfile failure must be actionable"
    );

    #[cfg(unix)]
    assert!(
        Command::new("bash")
            .arg("fixtures/action-source-lockfile/contract.sh")
            .arg("action.yml")
            .status()
            .is_ok_and(|status| status.success()),
        "source lockfile fixture failed"
    );
}
