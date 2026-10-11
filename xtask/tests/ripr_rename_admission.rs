//! Independent CLI admission evidence for #1358/#1359. The runtime writer owns
//! the fix; these tests intentionally expose rename-out-of-Rust false skips.

#![cfg(unix)]

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};
use serde_json::Value;

struct Fixture {
    root: PathBuf,
    bin: PathBuf,
    git: PathBuf,
}

impl Fixture {
    fn new(quote_paths: &str) -> Result<Self> {
        let git = std::env::split_paths(&std::env::var_os("PATH").context("missing PATH")?)
            .map(|directory| directory.join("git"))
            .find(|candidate| candidate.is_file())
            .context("git executable missing from PATH")?
            .canonicalize()?;
        let root = std::env::temp_dir().join(format!(
            "ub-review-ripr-rename-cli-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        let bin = root.join("isolated-bin");
        fs::create_dir_all(&bin)?;
        symlink(&git, bin.join("git"))?;
        let fixture = Self { root, bin, git };
        fixture.git(&["init"])?;
        fixture.git(&["config", "user.email", "xtask@example.invalid"])?;
        fixture.git(&["config", "user.name", "xtask regression"])?;
        fixture.git(&["config", "commit.gpgsign", "false"])?;
        fixture.git(&["config", "core.quotePath", quote_paths])?;
        Ok(fixture)
    }

    fn git(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = Command::new(&self.git)
            .args(args)
            .current_dir(&self.root)
            .output()?;
        if !output.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(output.stdout)
    }

    fn stage_rename(&self, from: &[u8], to: &[u8]) -> Result<()> {
        let from = self.root.join(OsStr::from_bytes(from));
        let to = self.root.join(OsStr::from_bytes(to));
        fs::create_dir_all(from.parent().context("source has no parent")?)?;
        fs::create_dir_all(to.parent().context("destination has no parent")?)?;
        fs::write(&from, "pub fn value() -> u8 { 1 }\n")?;
        self.git(&["add", "--", "."])?;
        self.git(&["commit", "-m", "rename fixture"])?;
        fs::rename(from, to)?;
        self.git(&["add", "-A"])?;
        Ok(())
    }

    fn run(&self) -> Result<Output> {
        // Only Git is on PATH: admission must reach the missing-tool error for
        // Rust inputs without depending on a real RIPR installation or version.
        Ok(Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(["ripr", "--base", "HEAD"])
            .env("PATH", &self.bin)
            .current_dir(&self.root)
            .output()?)
    }

    fn receipt_dir(&self) -> PathBuf {
        self.root.join("target/xtask/ripr")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Err(error) = fs::remove_dir_all(&self.root) {
            eprintln!("fixture cleanup {}: {error}", self.root.display());
        }
    }
}

fn assert_admitted(fixture: &Fixture) -> Result<()> {
    let patch = fixture.git(&["diff", "--binary", "--no-ext-diff", "HEAD", "--"])?;
    assert!(
        patch
            .windows(b"rename from ".len())
            .any(|bytes| bytes == b"rename from ")
    );
    let output = fixture.run()?;
    assert!(
        !output.status.success(),
        "Rust rename falsely skipped: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("could not invoke ripr"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read(fixture.receipt_dir().join("diff.patch"))?, patch);
    assert!(!fixture.receipt_dir().join("receipt.json").exists());
    Ok(())
}

#[test]
fn rename_out_of_rust_inputs_reaches_tool_invocation() -> Result<()> {
    for quote_paths in ["true", "false"] {
        for (from, to) in [
            (b"src/old.rs".as_slice(), b"src/new.md".as_slice()),
            (b"src/old-\xff.rs".as_slice(), b"src/new-\xfe.md".as_slice()),
            (
                b"crates/old/Cargo.toml".as_slice(),
                b"crates/new/README.md".as_slice(),
            ),
            (
                b"crates/\xff/Cargo.lock".as_slice(),
                b"crates/\xfe/README.md".as_slice(),
            ),
        ] {
            let fixture = Fixture::new(quote_paths)?;
            fixture.stage_rename(from, to)?;
            assert_admitted(&fixture)?;
        }
    }
    Ok(())
}

#[test]
fn rename_into_rust_inputs_reaches_tool_invocation() -> Result<()> {
    for quote_paths in ["true", "false"] {
        let fixture = Fixture::new(quote_paths)?;
        fixture.stage_rename(b"src/old.md", b"src/new-\xff.rs")?;
        assert_admitted(&fixture)?;
    }
    Ok(())
}

#[test]
fn non_rust_rename_remains_skipped() -> Result<()> {
    for quote_paths in ["true", "false"] {
        let fixture = Fixture::new(quote_paths)?;
        fixture.stage_rename(b"src/old-\xff.md", b"src/new-\xfe.md")?;
        let output = fixture.run()?;
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: Value =
            serde_json::from_slice(&fs::read(fixture.receipt_dir().join("receipt.json"))?)?;
        assert_eq!(receipt["status"], "skipped");
        assert_eq!(receipt["reason"], "no Rust inputs changed");
        assert!(!fixture.receipt_dir().join("diff.patch").exists());
    }
    Ok(())
}
