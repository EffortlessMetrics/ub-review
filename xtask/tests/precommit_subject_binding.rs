use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, ensure};

const INVALID: &str = "pub fn value() -> u8 {\n    missing_name\n}\n";
const VALID: &str = "pub fn value() -> u8 {\n    2\n}\n";

// Match the repository's native audit-stage ledger, rather than depending on
// cargo-allow being absent from the machine running these real CLI tests.
const CARGO_ALLOW_CONFIG: &str = r#"schema_version = "0.1"
policy = "cargo-allow"
owner = "ub-review/core"
status = "advisory"

[workspace]
root = "."
inventory = "git-tracked"
default_mode = "audit"
ignored = [".git/**", "target/**"]
generated = ["target/**"]
"#;

fn fixture_command(program: &str) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    // Preserve execution/toolchain routing, not ambient Git overrides,
    // credentials, analyzer options, or ub-review configuration.
    for name in [
        "PATH",
        "HOME",
        "USERPROFILE",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "LLVM_PROFILE_FILE",
        "SystemRoot",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
    command
}

fn git(root: &Path, args: &[&str]) -> Result<Output> {
    let output = fixture_command("git")
        .args(args)
        .current_dir(root)
        .output()?;
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
    fs::create_dir_all(root.join("policy"))?;
    fs::write(root.join("policy/cargo-allow.toml"), CARGO_ALLOW_CONFIG)?;
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

fn precommit_command(root: &Path) -> Command {
    let mut command = fixture_command(env!("CARGO_BIN_EXE_xtask"));
    command
        .arg("precommit")
        .env("CARGO_TARGET_DIR", root.join("target/build"))
        .env("CARGO_NET_OFFLINE", "true")
        .current_dir(root);
    command
}

fn precommit(root: &Path) -> Result<Output> {
    let output = precommit_command(root)
        .arg("--staged")
        .output()
        .context("execute real xtask precommit")?;
    // Logs must stay ignored even when a control invokes the fixture twice.
    fs::write(
        root.join("target/precommit/test-stdout.log"),
        &output.stdout,
    )?;
    fs::write(
        root.join("target/precommit/test-stderr.log"),
        &output.stderr,
    )?;
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
    let output = precommit_command(&root).output()?;
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

#[cfg(unix)]
fn executable_on_path(name: &str) -> Result<Option<PathBuf>> {
    let path = std::env::var_os("PATH").context("test tool PATH")?;
    for directory in std::env::split_paths(&path) {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Ok(Some(candidate.canonicalize()?));
        }
    }
    Ok(None)
}

#[cfg(unix)]
#[test]
fn cargo_allow_present_and_absent_preserve_real_cargo_and_custody() -> Result<()> {
    let root = fixture("cargo-allow-routing")?;
    fs::write(root.join("src/lib.rs"), VALID)?;
    git(&root, &["add", "src/lib.rs"])?;
    let tree = git(&root, &["write-tree"])?.stdout;
    let tools = root.join("target/test-tools");
    fs::create_dir_all(&tools)?;
    // Real executables only: keep Cargo, rustfmt, Clippy, Git and the native
    // linker available while making optional-analyzer presence explicit.
    for name in [
        "cargo",
        "rustc",
        "rustdoc",
        "cargo-fmt",
        "rustfmt",
        "cargo-clippy",
        "clippy-driver",
        "git",
        "cc",
        "as",
        "ld",
    ] {
        let executable =
            executable_on_path(name)?.with_context(|| format!("locate real {name}"))?;
        std::os::unix::fs::symlink(executable, tools.join(name))?;
    }
    let cargo_allow = executable_on_path("cargo-allow")?;
    for present in [false, true] {
        if present {
            let Some(executable) = &cargo_allow else {
                eprintln!("cargo-allow present control unavailable; install cargo-allow 0.1.8");
                break;
            };
            std::os::unix::fs::symlink(executable, tools.join("cargo-allow"))?;
        }
        let output = precommit_command(&root)
            .arg("--staged")
            .env("PATH", &tools)
            .output()?;
        ensure!(
            output.status.success(),
            "cargo-allow present={present}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("target/precommit/cargo-allow.json"))?)?;
        ensure!(receipt["skipped"] == !present);
        ensure!(receipt["success"] == present);
        ensure!(
            receipt["status"]
                == if present {
                    serde_json::json!(0)
                } else {
                    serde_json::Value::Null
                }
        );
        ensure!(
            fs::read_to_string(root.join("target/precommit/check-subject-probe.md"))?
                .contains("- success: true")
        );
        ensure!(git(&root, &["write-tree"])?.stdout == tree);
        ensure!(
            git(&root, &["ls-files", "--others", "--exclude-standard"])?
                .stdout
                .is_empty()
        );
        println!(
            "cargo-allow present={present}: real Cargo passed; receipt and staged tree verified"
        );
    }
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

#[test]
fn git_failure_during_staged_admission_keeps_no_success_summary() -> Result<()> {
    let root = fixture("git-comparison-error")?;
    fs::create_dir_all(root.join("target/precommit"))?;
    fs::write(root.join("target/precommit/summary.md"), "stale success\n")?;
    // Use an actual non-repository Git failure, not a mocked executable.
    fs::rename(root.join(".git"), root.join("target/saved-git"))?;
    let output = precommit(&root)?;
    ensure!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    ensure!(
        error.contains("could not compare precommit worktree with staged index"),
        "{error}"
    );
    ensure!(!root.join("target/precommit/summary.md").exists());
    ensure!(
        !root
            .join("target/precommit/check-subject-probe.md")
            .exists()
    );
    Ok(())
}

// Reuse the custody fixture and its owned environment/ledger for #1558. Each
// repository has a dependency-free real Cargo target; no Cargo or Git mocks.
fn commit_inputs(root: &Path) -> Result<()> {
    git(root, &["add", "--all"])?;
    git(
        root,
        &[
            "-c",
            "user.name=Subject probe",
            "-c",
            "user.email=probe@example.invalid",
            "commit",
            "-qm",
            "path baseline",
        ],
    )?;
    Ok(())
}

fn path_fixture(filename: &str) -> Result<PathBuf> {
    let root = fixture("path-intake")?;
    git(&root, &["reset", "--hard", "HEAD"])?;
    fs::write(root.join("src").join(filename), VALID)?;
    fs::write(
        root.join("src/lib.rs"),
        format!(
            "#[path = {}]\nmod part;\npub use part::value;\n",
            serde_json::to_string(filename)?
        ),
    )?;
    commit_inputs(&root)?;
    Ok(root)
}

fn intake_receipt(root: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::from_slice(&fs::read(
        root.join("target/precommit/affected-packages.json"),
    )?)?)
}

fn require_path_lines(root: &Path, path: &str, lines: &[u64]) -> Result<()> {
    let receipt = intake_receipt(root)?;
    let files = receipt["changed_files"]
        .as_array()
        .context("changed files")?;
    let file = files
        .iter()
        .find(|file| file["path"] == path)
        .with_context(|| format!("missing exact path {path:?}: {receipt}"))?;
    ensure!(file["lines"] == serde_json::json!(lines), "{file}");
    ensure!(receipt["affected_packages"][0]["name"] == "subject-probe");
    ensure!(
        receipt["affected_packages"]
            .as_array()
            .context("packages")?
            .len()
            == 1
    );
    Ok(())
}

fn broken_path_is_checked(filename: &str, quote: bool) -> Result<()> {
    let root = path_fixture(filename)?;
    let path = format!("src/{filename}");
    git(
        &root,
        &[
            "config",
            "core.quotePath",
            if quote { "true" } else { "false" },
        ],
    )?;
    fs::write(root.join(&path), INVALID)?;
    let direct = fixture_command("cargo")
        .args(["check", "--locked", "--offline"])
        .env("CARGO_TARGET_DIR", root.join("target/build"))
        .current_dir(&root)
        .output()?;
    ensure!(direct.status.code() == Some(101));
    ensure!(String::from_utf8_lossy(&direct.stderr).contains("missing_name"));
    for staged in [false, true] {
        if staged {
            git(&root, &["add", "--", &path])?;
            git(&root, &["diff", "--quiet"])?;
        }
        let mut command = precommit_command(&root);
        if staged {
            command.arg("--staged");
        }
        let output = command.output()?;
        ensure!(
            !output.status.success(),
            "false success for {path:?}, quote={quote}, staged={staged}: {}",
            String::from_utf8_lossy(&output.stdout)
        );
        require_path_lines(&root, &path, &[2])?;
        ensure!(
            fs::read_to_string(root.join("target/precommit/check-subject-probe.md"))?
                .contains("missing_name")
        );
        let findings: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("target/precommit/clippy.json"))?)?;
        ensure!(
            findings
                .as_array()
                .context("clippy messages")?
                .iter()
                .any(|message| message["message"]["spans"]
                    .as_array()
                    .is_some_and(|spans| spans
                        .iter()
                        .any(|span| span["file_name"] == path && span["line_start"] == 2)))
        );
        ensure!(
            fs::read_to_string(root.join("target/precommit/clippy-on-diff.md"))?
                .contains("missing_name")
        );
        println!("rejected {path:?}, quote={quote}, staged={staged}; direct Cargo=101");
    }
    fs::write(root.join(&path), "pub fn value() -> u8 {\n    3\n}\n")?;
    git(&root, &["add", "--", &path])?;
    let output = precommit(&root)?;
    ensure!(
        output.status.success(),
        "recovery failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    require_path_lines(&root, &path, &[2])?;
    let tree = git(&root, &["write-tree"])?;
    ensure!(
        fs::read_to_string(root.join("target/precommit/summary.md"))?
            .contains(String::from_utf8_lossy(&tree.stdout).trim())
    );
    // A different valid worktree must still be refused in staged mode.
    fs::write(root.join(&path), VALID)?;
    let output = precommit(&root)?;
    ensure!(!output.status.success());
    ensure!(
        String::from_utf8_lossy(&output.stderr).contains("working tree to match the staged index")
    );
    ensure!(!root.join("target/precommit/summary.md").exists());
    Ok(())
}

#[test]
fn path_intake_unicode_quoted_and_unquoted_reaches_real_cargo() -> Result<()> {
    for quote in [true, false] {
        broken_path_is_checked("é.rs", quote)?;
    }
    Ok(())
}

#[test]
fn path_intake_ascii_and_spaces_reach_real_cargo() -> Result<()> {
    for filename in ["ascii.rs", "space name.rs", " leading name.rs"] {
        broken_path_is_checked(filename, true)?;
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn path_intake_unix_tabs_newlines_and_backslashes_reach_real_cargo() -> Result<()> {
    for filename in ["tab\tname.rs", "line\nbreak.rs", "back\\slash.rs"] {
        broken_path_is_checked(filename, true)?;
    }
    Ok(())
}

#[test]
fn path_intake_mixed_headers_do_not_borrow_previous_file_lines() -> Result<()> {
    let root = path_fixture("é.rs")?;
    fs::write(root.join("src/a.rs"), "// one\n// two\n// three\n")?;
    commit_inputs(&root)?;
    fs::write(root.join("src/a.rs"), "// one\n// two\n// changed\n")?;
    fs::write(root.join("src/é.rs"), INVALID)?;
    let output = precommit_command(&root).output()?;
    ensure!(!output.status.success());
    require_path_lines(&root, "src/a.rs", &[3])?;
    require_path_lines(&root, "src/é.rs", &[2])?;
    ensure!(
        intake_receipt(&root)?["changed_files"]
            .as_array()
            .context("files")?
            .len()
            == 2
    );
    Ok(())
}

#[test]
fn path_intake_binary_and_clean_controls_preserve_modes() -> Result<()> {
    let root = path_fixture("é.rs")?;
    let clean = precommit_command(&root).output()?;
    ensure!(clean.status.success());
    ensure!(intake_receipt(&root)?["changed_files"] == serde_json::json!([]));
    fs::create_dir_all(root.join("assets"))?;
    fs::write(root.join("assets/logo.png"), b"\x89PNG\r\n\x1a\n")?;
    fs::write(root.join("notes with spaces.txt"), "ordinary text\n")?;
    let output = precommit_command(&root).output()?;
    ensure!(
        output.status.success(),
        "binary asset aborted intake: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let receipt = intake_receipt(&root)?;
    ensure!(receipt["affected_packages"] == serde_json::json!([]));
    ensure!(
        receipt["changed_files"]
            == serde_json::json!([
                {"path": "assets/logo.png", "lines": []},
                {"path": "notes with spaces.txt", "lines": []}
            ])
    );
    let output = precommit(&root)?;
    ensure!(!output.status.success());
    ensure!(String::from_utf8_lossy(&output.stderr).contains("nonignored untracked inputs"));
    git(&root, &["add", "--all"])?;
    ensure!(precommit(&root)?.status.success());
    ensure!(precommit_command(&root).output()?.status.success());
    for filename in ["new.rs", "é new.rs"] {
        fs::write(root.join("src").join(filename), VALID)?;
        ensure!(precommit_command(&root).output()?.status.success());
        require_path_lines(&root, &format!("src/{filename}"), &[1, 2, 3])?;
        fs::remove_file(root.join("src").join(filename))?;
    }
    Ok(())
}

#[test]
fn path_intake_invalid_utf8_rust_is_explicit_failure() -> Result<()> {
    for tracked in [false, true] {
        let root = path_fixture("é.rs")?;
        let path = if tracked { "src/é.rs" } else { "src/new.rs" };
        fs::write(root.join(path), b"pub fn value() {}\n\xff")?;
        for staged in [false, true] {
            if staged {
                git(&root, &["add", "--", path])?;
            }
            let mut command = precommit_command(&root);
            if staged {
                command.arg("--staged");
            }
            let output = command.output()?;
            ensure!(!output.status.success());
            let error = String::from_utf8_lossy(&output.stderr);
            ensure!(
                error.contains("not valid UTF-8") && error.contains(path),
                "{error}"
            );
            ensure!(!root.join("target/precommit/summary.md").exists());
            ensure!(
                !root
                    .join("target/precommit/check-subject-probe.md")
                    .exists()
            );
        }
    }
    Ok(())
}

#[test]
fn path_intake_deleted_and_renamed_sources_keep_existing_selection() -> Result<()> {
    let root = path_fixture("old.rs")?;
    fs::write(root.join("src/deleted.rs"), VALID)?;
    commit_inputs(&root)?;
    git(&root, &["mv", "src/old.rs", "src/é.rs"])?;
    fs::write(
        root.join("src/lib.rs"),
        "#[path = \"é.rs\"]\nmod part;\npub use part::value;\n",
    )?;
    git(&root, &["rm", "src/deleted.rs"])?;
    for staged in [false, true] {
        if staged {
            git(&root, &["add", "--all"])?;
        }
        let mut command = precommit_command(&root);
        if staged {
            command.arg("--staged");
        }
        let output = command.output()?;
        ensure!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        require_path_lines(&root, "src/é.rs", &[])?;
        let receipt = intake_receipt(&root)?;
        let files = receipt["changed_files"].as_array().context("files")?;
        ensure!(
            !files
                .iter()
                .any(|file| file["path"] == "src/deleted.rs" || file["path"] == "src/old.rs")
        );
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn path_intake_preserves_boundary_whitespace_and_nontext_patch_bytes() -> Result<()> {
    let root = path_fixture("é.rs")?;
    let paths = [
        " leading ",
        "trailing ",
        "tab\tname",
        "line\nbreak",
        "back\\slash",
    ];
    for path in paths {
        fs::write(root.join(path), b"old\n")?;
    }
    commit_inputs(&root)?;
    for path in paths {
        fs::write(root.join(path), b"new\xff\n")?;
    }
    // Prefix preferences must not change machine-parsed file identity.
    git(&root, &["config", "diff.noprefix", "true"])?;
    for staged in [false, true] {
        if staged {
            git(&root, &["add", "--all"])?;
        }
        let mut command = precommit_command(&root);
        if staged {
            command.arg("--staged");
        }
        ensure!(command.output()?.status.success());
        let receipt = intake_receipt(&root)?;
        let files = receipt["changed_files"].as_array().context("files")?;
        ensure!(files.len() == paths.len());
        for path in paths {
            ensure!(
                files
                    .iter()
                    .any(|file| file["path"] == path && file["lines"] == serde_json::json!([1])),
                "{receipt}"
            );
        }
        let patch = fs::read(root.join(if staged {
            "target/precommit/staged.diff"
        } else {
            "target/precommit/working-tree.diff"
        }))?;
        ensure!(patch.contains(&0xff), "nontext diff payload was altered");
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn path_intake_non_utf8_git_names_fail_with_exact_bytes() -> Result<()> {
    use std::os::unix::ffi::OsStringExt;
    let root = path_fixture("é.rs")?;
    let filename = std::ffi::OsString::from_vec(b"bad-\xff.rs".to_vec());
    fs::write(root.join(filename), VALID)?;
    for staged in [false, true] {
        if staged {
            git(&root, &["add", "--all"])?;
        }
        let mut command = precommit_command(&root);
        if staged {
            command.arg("--staged");
        }
        let output = command.output()?;
        ensure!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        ensure!(
            error.contains("non-UTF-8 Git path bytes [62, 61, 64, 2d, ff, 2e, 72, 73]"),
            "{error}"
        );
        ensure!(error.contains("use a UTF-8 path or a path-aware workflow"));
        ensure!(!root.join("target/precommit/summary.md").exists());
    }
    Ok(())
}

#[test]
fn path_intake_patch_like_source_cannot_replace_a_file_header() -> Result<()> {
    let root = path_fixture("é.rs")?;
    fs::write(root.join("notes.txt"), "old\n1\n2\n3\n4\n5\n6\n7\n8\nold\n")?;
    commit_inputs(&root)?;
    fs::write(
        root.join("notes.txt"),
        "++ b/fake.rs\n1\n2\n3\n4\n5\n6\n7\n8\nnew\n",
    )?;
    ensure!(precommit_command(&root).output()?.status.success());
    let receipt = intake_receipt(&root)?;
    ensure!(
        receipt["changed_files"]
            == serde_json::json!([
                {"path": "notes.txt", "lines": [1, 10]}
            ]),
        "{receipt}"
    );
    Ok(())
}
