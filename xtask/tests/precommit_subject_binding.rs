use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, ensure};

const INVALID: &str = "pub fn value() -> u8 {\n    missing_name\n}\n";
const VALID: &str = "pub fn value() -> u8 {\n    2\n}\n";

fn git(root: &Path, args: &[&str]) -> Result<Output> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    ensure!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(output)
}

fn fixture(name: &str) -> Result<PathBuf> {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "precommit-subject-{name}-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"subject-probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n[workspace]\n",
    )?;
    fs::write(
        root.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"subject-probe\"\nversion = \"0.1.0\"\n",
    )?;
    fs::write(root.join(".gitignore"), "/target\n")?;
    fs::write(
        root.join("src/lib.rs"),
        "pub fn value() -> u8 {\n    1\n}\n",
    )?;
    git(&root, &["init"])?;
    git(&root, &["add", "."])?;
    git(
        &root,
        &[
            "-c",
            "user.name=Subject probe",
            "-c",
            "user.email=probe@example.invalid",
            "commit",
            "-m",
            "initial",
        ],
    )?;
    fs::write(root.join("src/lib.rs"), INVALID)?;
    git(&root, &["add", "src/lib.rs"])?;
    Ok(root)
}

fn precommit(root: &Path) -> Result<Output> {
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["precommit", "--staged"])
        .env("CARGO_TARGET_DIR", root.join("target/build"))
        .env("CARGO_NET_OFFLINE", "true")
        .current_dir(root)
        .output()
        .context("execute real xtask precommit")?;
    fs::write(root.join("stdout.log"), &output.stdout)?;
    fs::write(root.join("stderr.log"), &output.stderr)?;
    println!("retained fixture: {}", root.display());
    Ok(output)
}

#[test]
fn matching_invalid_staged_and_worktree_source_is_blocked() -> Result<()> {
    let root = fixture("matching-invalid")?;
    let output = precommit(&root)?;
    ensure!(
        !output.status.success(),
        "invalid matching source unexpectedly passed"
    );
    ensure!(
        fs::read_to_string(root.join("target/precommit/check-subject-probe.md"))?
            .contains("missing_name")
    );
    Ok(())
}

#[test]
fn staged_invalid_source_cannot_pass_using_valid_worktree_source() -> Result<()> {
    let root = fixture("mismatched")?;
    fs::write(root.join("src/lib.rs"), VALID)?;
    let staged = git(&root, &["show", ":src/lib.rs"])?;
    ensure!(staged.stdout == INVALID.as_bytes());
    fs::create_dir_all(root.join("target/precommit"))?;
    fs::write(root.join("target/precommit/summary.md"), "stale success\n")?;
    let output = precommit(&root)?;
    ensure!(
        !output.status.success(),
        "staged invalid source was certified using a different valid worktree: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    ensure!(
        String::from_utf8_lossy(&output.stderr).contains("working tree to match the staged index")
    );
    ensure!(
        !root.join("target/precommit/summary.md").exists(),
        "rejected source left a success summary"
    );
    Ok(())
}

#[test]
fn matching_valid_staged_source_passes_and_names_its_tree() -> Result<()> {
    let root = fixture("matching-valid")?;
    fs::write(root.join("src/lib.rs"), VALID)?;
    git(&root, &["add", "src/lib.rs"])?;
    let output = precommit(&root)?;
    ensure!(
        output.status.success(),
        "valid matching source failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let tree = git(&root, &["write-tree"])?;
    let summary = fs::read_to_string(root.join("target/precommit/summary.md"))?;
    ensure!(summary.contains(String::from_utf8_lossy(&tree.stdout).trim()));
    Ok(())
}

#[test]
fn unstaged_input_outside_staged_diff_is_rejected() -> Result<()> {
    let root = fixture("other-input")?;
    fs::write(root.join("src/lib.rs"), VALID)?;
    git(&root, &["add", "src/lib.rs"])?;
    fs::write(root.join("Cargo.toml"), "invalid manifest worktree\n")?;
    let output = precommit(&root)?;
    ensure!(!output.status.success());
    ensure!(
        String::from_utf8_lossy(&output.stderr).contains("working tree to match the staged index")
    );
    Ok(())
}

#[test]
fn nonignored_untracked_input_is_rejected_but_ignored_output_is_allowed() -> Result<()> {
    for untracked in [true, false] {
        let root = fixture(if untracked { "untracked" } else { "ignored" })?;
        fs::write(root.join("src/lib.rs"), VALID)?;
        git(&root, &["add", "src/lib.rs"])?;
        let extra = if untracked {
            root.join("src/extra.rs")
        } else {
            root.join("target/extra.rs")
        };
        fs::create_dir_all(extra.parent().context("extra parent")?)?;
        fs::write(extra, "extra input\n")?;
        let output = precommit(&root)?;
        if untracked {
            ensure!(!output.status.success());
            ensure!(
                String::from_utf8_lossy(&output.stderr).contains("nonignored untracked inputs")
            );
        } else {
            ensure!(
                output.status.success(),
                "ignored output blocked precommit: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
    }
    Ok(())
}

#[test]
fn nonstaged_precommit_still_checks_valid_dirty_worktree() -> Result<()> {
    let root = fixture("nonstaged")?;
    fs::write(root.join("src/lib.rs"), VALID)?;
    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("precommit")
        .env("CARGO_TARGET_DIR", root.join("target/build"))
        .env("CARGO_NET_OFFLINE", "true")
        .current_dir(&root)
        .output()?;
    ensure!(
        output.status.success(),
        "nonstaged check failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    ensure!(
        fs::read_to_string(root.join("target/precommit/summary.md"))?
            .contains("- mode: working tree")
    );
    Ok(())
}

#[test]
fn input_changes_during_real_cargo_check_cannot_leave_success_summary() -> Result<()> {
    for change_index in [false, true] {
        let root = fixture(if change_index {
            "drifting-index"
        } else {
            "drifting-worktree"
        })?;
        fs::write(root.join("src/lib.rs"), VALID)?;
        let index_change = if change_index {
            "    let status = std::process::Command::new(\"git\").args([\"add\", \"src/lib.rs\"]).status()?;\n    if !status.success() {\n        return Err(\"git add failed\".into());\n    }\n"
        } else {
            ""
        };
        let build = format!(
            "fn main() -> Result<(), Box<dyn std::error::Error>> {{\n    std::fs::write(\"src/lib.rs\", \"pub fn value() -> u8 {{\\n    3\\n}}\\n\")?;\n{index_change}    Ok(())\n}}\n"
        );
        fs::write(root.join("build.rs"), build)?;
        git(&root, &["add", "src/lib.rs", "build.rs"])?;
        let output = precommit(&root)?;
        // Establish that the real Cargo build script ran before crediting rejection.
        ensure!(fs::read_to_string(root.join("src/lib.rs"))?.contains("    3\n"));
        ensure!(
            fs::read_to_string(root.join("target/precommit/check-subject-probe.md"))?
                .contains("- success: true")
        );
        ensure!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        ensure!(
            error.contains(if change_index {
                "staged index changed during precommit"
            } else {
                "working tree to match the staged index"
            }),
            "wrong rejection: {error}"
        );
        ensure!(!root.join("target/precommit/summary.md").exists());
    }
    Ok(())
}
