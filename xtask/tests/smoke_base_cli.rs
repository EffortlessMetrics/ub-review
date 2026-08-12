use std::fs;
use std::process::Command;

use anyhow::{Context, Result, bail};

fn git(root: &std::path::Path, args: &[&str]) -> Result<()> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    if !output.status.success() {
        bail!("git {} failed", args.join(" "));
    }
    Ok(())
}

#[test]
fn smoke_base_cli_prints_one_usable_resolved_revision() -> Result<()> {
    let root = std::env::temp_dir().join(format!(
        "ub-review-smoke-base-cli-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    fs::create_dir_all(&root)?;
    let helper_error = git(&root, &["not-a-command"])
        .err()
        .map(|error| error.to_string())
        .context("invalid Git command unexpectedly succeeded")?;
    assert_eq!(helper_error, "git not-a-command failed");
    git(&root, &["init"])?;
    git(&root, &["config", "user.email", "xtask@example.invalid"])?;
    git(&root, &["config", "user.name", "xtask test"])?;
    fs::write(root.join("value.txt"), "one\n")?;
    git(&root, &["add", "value.txt"])?;
    git(&root, &["commit", "-m", "initial"])?;
    fs::write(root.join("value.txt"), "two\n")?;
    git(&root, &["add", "value.txt"])?;
    git(&root, &["commit", "-m", "change"])?;

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["smoke-base"])
        .current_dir(&root)
        .output()?;
    if !output.status.success() {
        bail!(
            "smoke-base failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = std::str::from_utf8(&output.stdout)?.trim();
    assert_eq!(stdout.lines().count(), 1, "stdout={stdout:?}");
    assert_eq!(stdout.len(), 40, "stdout={stdout:?}");
    let diff = Command::new("git")
        .args(["diff", "--quiet", stdout, "HEAD", "--"])
        .current_dir(&root)
        .status()
        .context("validate resolved smoke range")?;
    assert_eq!(diff.code(), Some(1));
    fs::remove_dir_all(root)?;
    Ok(())
}
