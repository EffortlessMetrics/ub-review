//! Admission-time metadata, never a cleanliness gate or a content snapshot.
use crate::plan_build::git_bytes_bounded;
use crate::revision_identity::RevisionIdentity;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

const MAX_BYTES: usize = 64 * 1024;
const MAX_ENTRIES: usize = 256;
const TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct WorktreeEvidence {
    schema: String,
    identity_digest: String,
    reviewed_commit_oid: String,
    observed_at: String,
    /// complete / unavailable / truncated / racy; complete means only that
    /// bounded metadata observations agreed, never atomic byte equivalence.
    outcome: String,
    reason: Option<String>,
    checkout_head_oid: Option<String>,
    max_command_bytes: usize,
    max_entries_per_inventory: usize,
    command_timeout_ms: u64,
    status: Vec<StatusEntry>,
    /// Worktree versus index, excluding untracked and ignored files.
    tracked_diff_paths_hex: Vec<String>,
    /// Index versus the admitted reviewed commit (which can differ from HEAD).
    staged_diff_paths_hex: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct StatusEntry {
    /// Porcelain X (index) and Y (worktree); ?? is untracked.
    index_status: String,
    worktree_status: String,
    /// Lowercase hex of exact repository-relative Git path bytes. No lossy
    /// UTF-8 conversion, control characters or executable path interpretation.
    path_hex: String,
}

pub(crate) fn collect(
    root: &Path,
    identity: &RevisionIdentity,
    reviewed_commit_oid: &str,
) -> WorktreeEvidence {
    let mut evidence = WorktreeEvidence {
        schema: "ub-review.worktree_evidence.v1".to_owned(),
        identity_digest: identity.identity_digest(),
        reviewed_commit_oid: reviewed_commit_oid.to_owned(),
        observed_at: chrono::Utc::now().to_rfc3339(),
        outcome: "unavailable".to_owned(),
        reason: None,
        checkout_head_oid: None,
        max_command_bytes: MAX_BYTES,
        max_entries_per_inventory: MAX_ENTRIES,
        command_timeout_ms: TIMEOUT.as_millis() as u64,
        status: Vec::new(),
        tracked_diff_paths_hex: Vec::new(),
        staged_diff_paths_hex: Vec::new(),
    };
    let run = |args: &[&str]| git_bytes_bounded(root, args, MAX_BYTES, TIMEOUT);
    // Legacy admission helpers still honor caller-selected Git sources. Do
    // not silently associate a newly isolated checkout inventory with that
    // possibly different source. Such observations remain unavailable.
    let result = if inherited_git_source() {
        Err(anyhow::anyhow!("inherited_git_source"))
    } else {
        observe(&mut evidence, &run)
    };
    finish(&mut evidence, result);
    evidence
}

fn inherited_git_source() -> bool {
    std::env::vars_os().any(|(key, value)| {
        let key = key.to_string_lossy().to_ascii_uppercase();
        !value.is_empty()
            && (key.starts_with("GIT_CONFIG")
                || matches!(
                    key.as_str(),
                    "GIT_DIR"
                        | "GIT_WORK_TREE"
                        | "GIT_INDEX_FILE"
                        | "GIT_COMMON_DIR"
                        | "GIT_OBJECT_DIRECTORY"
                        | "GIT_ALTERNATE_OBJECT_DIRECTORIES"
                        | "GIT_SHALLOW_FILE"
                        | "GIT_NAMESPACE"
                ))
    })
}

fn finish(evidence: &mut WorktreeEvidence, result: Result<()>) {
    if let Err(error) = result {
        // All errors below are fixed labels; never include Git diagnostics.
        let reason = error.to_string();
        evidence.outcome = if matches!(reason.as_str(), "byte_limit" | "entry_limit") {
            "truncated"
        } else if reason == "metadata_changed" {
            "racy"
        } else {
            "unavailable"
        }
        .to_owned();
        evidence.reason = Some(reason);
    }
}

fn observe(
    evidence: &mut WorktreeEvidence,
    run: &impl Fn(&[&str]) -> Result<Vec<u8>>,
) -> Result<()> {
    let head_args = ["rev-parse", "--verify", "HEAD^{commit}"];
    let status_args = [
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
        "--ignored=no",
        "--no-renames",
        "--ignore-submodules=none",
    ];
    let head_before = run(&head_args)?;
    let head = std::str::from_utf8(&head_before)
        .map_err(|_| anyhow::anyhow!("invalid_head"))?
        .trim();
    ensure!(
        matches!(head.len(), 40 | 64) && head.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid_head"
    );
    evidence.checkout_head_oid = Some(head.to_owned());
    let before = run(&status_args)?;
    evidence.status = parse_status(&before, MAX_ENTRIES)?;
    evidence.tracked_diff_paths_hex = parse_paths(
        &run(&[
            "diff",
            "--name-only",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--ignore-submodules=none",
            "--",
        ])?,
        MAX_ENTRIES,
    )?;
    evidence.staged_diff_paths_hex = parse_paths(
        &run(&[
            "diff",
            "--cached",
            "--name-only",
            "-z",
            "--no-ext-diff",
            "--no-textconv",
            "--no-renames",
            "--ignore-submodules=none",
            &evidence.reviewed_commit_oid,
            "--",
        ])?,
        MAX_ENTRIES,
    )?;
    let after = run(&status_args)?;
    let head_after = run(&head_args)?;
    ensure!(
        before == after && head_before == head_after,
        "metadata_changed"
    );
    evidence.outcome = "complete".to_owned();
    Ok(())
}

fn path_hex(path: &[u8]) -> Result<String> {
    ensure!(
        !path.is_empty() && !path.starts_with(b"/") && !path.contains(&b'\\'),
        "unsafe_path"
    );
    ensure!(
        path.split(|b| *b == b'/')
            .all(|part| !part.is_empty() && part != b"." && part != b".."),
        "unsafe_path"
    );
    // Reject Windows drive / alternate-stream forms, even on Unix.
    ensure!(!path.contains(&b':'), "unsafe_path");
    Ok(path.iter().map(|byte| format!("{byte:02x}")).collect())
}

fn nul_records(bytes: &[u8]) -> Result<impl Iterator<Item = &[u8]>> {
    ensure!(
        bytes.is_empty() || bytes.last() == Some(&0),
        "incomplete_record"
    );
    let body = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    Ok(body
        .split(|byte| *byte == 0)
        .filter(move |_| !bytes.is_empty()))
}

fn parse_status(bytes: &[u8], limit: usize) -> Result<Vec<StatusEntry>> {
    let mut entries = Vec::new();
    for row in nul_records(bytes)? {
        ensure!(entries.len() < limit, "entry_limit");
        ensure!(row.len() > 3 && row[2] == b' ', "invalid_status");
        ensure!(
            row[..2].iter().all(|b| b" MADRCUT?!".contains(b)),
            "invalid_status"
        );
        ensure!(
            !row[..2].contains(&b'R') && !row[..2].contains(&b'C'),
            "rename_not_disabled"
        );
        entries.push(StatusEntry {
            index_status: char::from(row[0]).to_string(),
            worktree_status: char::from(row[1]).to_string(),
            path_hex: path_hex(&row[3..])?,
        });
    }
    Ok(entries)
}

fn parse_paths(bytes: &[u8], limit: usize) -> Result<Vec<String>> {
    let mut paths = Vec::new();
    for path in nul_records(bytes)? {
        ensure!(paths.len() < limit, "entry_limit");
        paths.push(path_hex(path)?);
    }
    Ok(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan_build::git_text;
    use crate::revision_admission::admit_revision;

    fn repo() -> Result<tempfile::TempDir> {
        let repo = tempfile::tempdir()?;
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.name", "worktree evidence test"],
            vec!["config", "user.email", "evidence@example.invalid"],
        ] {
            git_text(repo.path(), &args)?;
        }
        std::fs::write(repo.path().join("tracked.rs"), "base\n")?;
        std::fs::write(repo.path().join(".gitignore"), "target/\n")?;
        git_text(repo.path(), &["add", "."])?;
        git_text(repo.path(), &["commit", "-qm", "base"])?;
        Ok(repo)
    }

    fn admission(root: &Path) -> Result<crate::revision_admission::RevisionAdmission> {
        admit_revision(root, "HEAD", "HEAD", None, &[], "")
    }

    fn evidence(root: &Path) -> Result<WorktreeEvidence> {
        admission(root)?
            .worktree_evidence
            .ok_or_else(|| anyhow::anyhow!("missing evidence"))
    }

    #[test]
    fn pristine_ignored_bytecode_tracked_and_staged_are_distinct() -> Result<()> {
        let repo = repo()?;
        let root = repo.path();
        let pristine = admission(root)?;
        let clean = pristine
            .worktree_evidence
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing evidence"))?;
        assert_eq!(clean.outcome, "complete");
        assert!(clean.status.is_empty());
        assert!(clean.tracked_diff_paths_hex.is_empty());
        assert!(clean.staged_diff_paths_hex.is_empty());
        assert_eq!(clean.identity_digest, pristine.identity_digest);
        assert_eq!(clean.reviewed_commit_oid, pristine.reviewed_commit_oid);
        assert_eq!(
            clean.checkout_head_oid.as_deref(),
            Some(pristine.reviewed_commit_oid.as_str())
        );
        std::fs::create_dir(root.join("target"))?;
        std::fs::write(root.join("target/output"), "ignored\n")?;
        assert!(evidence(root)?.status.is_empty());
        assert!(!admission(root)?.worktree_dirty);
        std::fs::create_dir_all(root.join("scripts/__pycache__"))?;
        std::fs::write(
            root.join("scripts/__pycache__/fixture.pyc"),
            b"private bytes",
        )?;
        let bytecode = evidence(root)?;
        assert_eq!(bytecode.status.len(), 1);
        assert_eq!(bytecode.status[0].index_status, "?");
        assert_eq!(bytecode.status[0].worktree_status, "?");
        assert_eq!(
            bytecode.status[0].path_hex,
            path_hex(b"scripts/__pycache__/fixture.pyc")?
        );
        assert!(bytecode.tracked_diff_paths_hex.is_empty());
        assert!(bytecode.staged_diff_paths_hex.is_empty());
        assert!(admission(root)?.worktree_dirty);
        std::fs::write(root.join("tracked.rs"), "unstaged\n")?;
        let tracked = evidence(root)?;
        assert_eq!(
            tracked.tracked_diff_paths_hex,
            vec![path_hex(b"tracked.rs")?]
        );
        assert!(tracked.staged_diff_paths_hex.is_empty());
        assert!(
            tracked
                .status
                .iter()
                .any(|entry| entry.index_status == " " && entry.worktree_status == "M")
        );
        git_text(root, &["add", "tracked.rs"])?;
        let staged = evidence(root)?;
        assert!(staged.tracked_diff_paths_hex.is_empty());
        assert_eq!(staged.staged_diff_paths_hex, vec![path_hex(b"tracked.rs")?]);
        assert!(
            staged
                .status
                .iter()
                .any(|entry| entry.index_status == "M" && entry.worktree_status == " ")
        );
        std::fs::write(root.join("tracked.rs"), "both\n")?;
        let both = evidence(root)?;
        assert!(
            both.status
                .iter()
                .any(|entry| entry.index_status == "M" && entry.worktree_status == "M")
        );
        assert_eq!(both.tracked_diff_paths_hex, both.staged_diff_paths_hex);
        assert_eq!(admission(root)?.identity_digest, pristine.identity_digest);
        let encoded = serde_json::to_string(&both)?;
        assert!(!encoded.contains("private bytes"));
        assert!(!encoded.contains(&root.to_string_lossy().to_string()));
        Ok(())
    }

    #[test]
    fn unusual_paths_are_byte_exact_and_outside_paths_are_unavailable() -> Result<()> {
        let repo = repo()?;
        let name = " odd\t\n\"name.rs ";
        std::fs::write(repo.path().join(name), "private")?;
        let observed = evidence(repo.path())?;
        assert_eq!(observed.outcome, "complete");
        assert_eq!(observed.status[0].path_hex, path_hex(name.as_bytes())?);
        assert!(!serde_json::to_string(&observed)?.contains(name));
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let name = std::ffi::OsStr::from_bytes(b"non-utf8-\xff.rs");
            std::fs::write(repo.path().join(name), "private")?;
            assert!(
                evidence(repo.path())?
                    .status
                    .iter()
                    .any(|entry| entry.path_hex == "6e6f6e2d757466382dff2e7273")
            );
        }
        for name in [
            b"/absolute".as_slice(),
            b"../outside",
            b"a/../outside",
            b"a//b",
            b"C:/outside",
            b"a\\b",
        ] {
            assert_eq!(
                path_hex(name).err().map(|e| e.to_string()).as_deref(),
                Some("unsafe_path")
            );
        }
        Ok(())
    }

    #[test]
    fn limits_failures_and_races_have_explicit_outcomes() -> Result<()> {
        let repo = repo()?;
        let original = evidence(repo.path())?;
        let run = |args: &[&str]| git_bytes_bounded(repo.path(), args, MAX_BYTES, TIMEOUT);
        let mut racy = original.clone();
        let calls = std::cell::Cell::new(0);
        let result = observe(&mut racy, &|args| {
            let count = calls.get();
            calls.set(count + 1);
            if count == 4 {
                Ok(b"?? race\0".to_vec())
            } else {
                run(args)
            }
        });
        assert_eq!(
            result.as_ref().err().map(|e| e.to_string()).as_deref(),
            Some("metadata_changed")
        );
        finish(&mut racy, result);
        assert_eq!(racy.outcome, "racy");
        assert_eq!(racy.reason.as_deref(), Some("metadata_changed"));
        assert_eq!(
            parse_status(b"?? a\0?? b\0", 1)
                .err()
                .map(|e| e.to_string())
                .as_deref(),
            Some("entry_limit")
        );
        assert_eq!(
            parse_paths(b"a\0b\0", 1)
                .err()
                .map(|e| e.to_string())
                .as_deref(),
            Some("entry_limit")
        );
        assert_eq!(
            parse_status(b"?? incomplete", 1)
                .err()
                .map(|e| e.to_string())
                .as_deref(),
            Some("incomplete_record")
        );
        assert!(parse_paths(b"a\0\0", 2).is_err());
        let identity =
            RevisionIdentity::from_canonical(&admission(repo.path())?.identity_canonical)?;
        let missing = tempfile::tempdir()?;
        let unavailable = collect(missing.path(), &identity, &original.reviewed_commit_oid);
        assert_eq!(unavailable.outcome, "unavailable");
        assert_eq!(
            unavailable.reason.as_deref(),
            Some("checkout_root_required")
        );
        git_text(missing.path(), &["init", "-q"])?;
        let missing_head = collect(missing.path(), &identity, &original.reviewed_commit_oid);
        assert_eq!(missing_head.outcome, "unavailable");
        assert_eq!(missing_head.reason.as_deref(), Some("git_failed"));
        for index in 0..=MAX_ENTRIES {
            std::fs::write(repo.path().join(format!("untracked-{index}")), "private")?;
        }
        let truncated = evidence(repo.path())?;
        assert_eq!(truncated.outcome, "truncated");
        assert_eq!(truncated.reason.as_deref(), Some("entry_limit"));
        assert_eq!(
            git_bytes_bounded(repo.path(), &["status", "--porcelain=v1", "-z"], 8, TIMEOUT)
                .err()
                .map(|e| e.to_string())
                .as_deref(),
            Some("byte_limit")
        );
        Ok(())
    }

    #[test]
    fn old_admission_consumers_and_receipts_remain_compatible() -> Result<()> {
        let repo = repo()?;
        let admitted = admission(repo.path())?;
        let mut json = serde_json::to_value(&admitted)?;
        let object = json
            .as_object_mut()
            .ok_or_else(|| anyhow::anyhow!("not object"))?;
        object.remove("worktree_evidence");
        let legacy: crate::revision_admission::RevisionAdmission = serde_json::from_value(json)?;
        legacy.validate()?;
        assert!(legacy.worktree_evidence.is_none());
        assert_eq!(legacy.identity_digest, admitted.identity_digest);
        assert_eq!(legacy.worktree_dirty, admitted.worktree_dirty);
        Ok(())
    }

    #[test]
    fn staged_rename_inventory_is_independent_of_rename_configuration() -> Result<()> {
        let repo = repo()?;
        let root = repo.path();
        git_text(root, &["mv", "tracked.rs", "renamed.rs"])?;
        let expected = vec![path_hex(b"renamed.rs")?, path_hex(b"tracked.rs")?];
        for value in ["true", "false", "copies"] {
            git_text(root, &["config", "diff.renames", value])?;
            git_text(root, &["config", "status.renames", value])?;
            let observed = evidence(root)?;
            assert_eq!(observed.outcome, "complete");
            assert_eq!(observed.staged_diff_paths_hex, expected, "{value}");
            assert!(observed.tracked_diff_paths_hex.is_empty());
            assert_eq!(observed.status.len(), 2);
            assert!(
                observed
                    .status
                    .iter()
                    .any(|row| row.path_hex == expected[0] && row.index_status == "A")
            );
            assert!(
                observed
                    .status
                    .iter()
                    .any(|row| row.path_hex == expected[1] && row.index_status == "D")
            );
        }
        // Linked worktrees retain their own index after selector isolation.
        let linked_dir = tempfile::tempdir()?;
        let linked = linked_dir.path().join("checkout");
        git_text(
            root,
            &[
                "worktree",
                "add",
                "-q",
                "--detach",
                &linked.to_string_lossy(),
                "HEAD",
            ],
        )?;
        assert!(evidence(&linked)?.status.is_empty());
        assert_eq!(evidence(&linked)?.outcome, "complete");
        Ok(())
    }
}
