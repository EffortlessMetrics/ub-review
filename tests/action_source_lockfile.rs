use std::process::Command;

const ACTION: &str = include_str!("../action.yml");

/// Pins the source-install ordering and prevents resolver mutation from
/// reappearing before the locked build.
#[test]
fn source_runner_requires_committed_regular_lockfile_before_locked_build() {
    assert!(
        !ACTION.contains("cargo generate-lockfile"),
        "source mode must not mutate dependency resolution"
    );
    let lockfile = ACTION
        .find("lockfile=\"$workdir/Cargo.lock\"")
        .expect("source runner must name the copied Cargo.lock");
    let guard = ACTION
        .find("if [[ -L \"$lockfile\" || ! -f \"$lockfile\" ]]; then")
        .expect("source runner must reject symlinked and non-regular lockfiles");
    let build = ACTION
        .find("cargo build --manifest-path \"$workdir/Cargo.toml\" --locked --release --target-dir \"$cargo_target_dir\"")
        .expect("source runner must execute one locked build");
    assert!(
        lockfile < guard && guard < build,
        "lock admission must precede Cargo execution"
    );
    assert!(
        ACTION.contains("source install requires a committed regular Cargo.lock"),
        "missing lockfile failure must be actionable"
    );
}

/// Executes the extracted Action source runner against valid, invalid, and
/// stale lockfile fixtures without adding a second Rust implementation of it.
#[cfg(unix)]
#[test]
fn source_runner_lockfile_contract_executes_end_to_end() {
    let output = Command::new("bash")
        .arg("fixtures/action-source-lockfile/contract.sh")
        .arg("action.yml")
        .output()
        .expect("execute source lockfile fixture");
    assert!(
        output.status.success(),
        "source lockfile fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
