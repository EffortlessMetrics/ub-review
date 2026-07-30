use std::fs;
use std::path::Path;
use std::process::Command;

use anyhow::{Result, bail};

#[path = "cli_lock.rs"]
mod cli_lock;
pub use cli_lock::cli_subprocess_test_lock;

/// Builds a child command with ambient ub-review/runtime profile state scrubbed.
pub fn isolated_command(program: &str, cwd: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(cwd);
    for (name, _) in std::env::vars_os() {
        let name_string = name.to_string_lossy();
        if name_string.starts_with("UB_REVIEW_")
            || matches!(
                name_string.as_ref(),
                "GITHUB_ACTIONS" | "RUNNER_ENVIRONMENT" | "RUNNER_NAME"
            )
        {
            command.env_remove(&name);
        }
    }
    command
}

pub fn run(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
    let output = isolated_command(program, cwd).args(args).output()?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{} {:?} failed\nstdout:\n{}\nstderr:\n{}",
        program,
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn run_expect_failure(cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = isolated_command(program, cwd).args(args).output()?;
    if !output.status.success() {
        return Ok(format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    bail!("{program} {args:?} unexpectedly succeeded");
}

pub fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", path.display()))
}

pub fn write_file(path: &Path, text: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, text)?;
    Ok(())
}

pub fn init_minimal_repo(repo: &Path) -> Result<()> {
    write_file(
        &repo.join("src/lib.rs"),
        "pub fn answer() -> usize {\n    41\n}\n",
    )?;
    run(repo, "git", &["init"])?;
    run(
        repo,
        "git",
        &["config", "user.email", "ub-review@example.invalid"],
    )?;
    run(repo, "git", &["config", "user.name", "UB Review Test"])?;
    run(repo, "git", &["add", "."])?;
    run(repo, "git", &["commit", "-m", "baseline"])?;
    write_file(
        &repo.join("src/lib.rs"),
        "pub fn answer() -> usize {\n    42\n}\n",
    )?;
    run(repo, "git", &["add", "."])?;
    run(repo, "git", &["commit", "-m", "change"])?;
    Ok(())
}
