//! Base+tests proof worktree preparation and cleanup.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::*;

pub(crate) fn prepare_base_plus_tests_worktree(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
) -> Result<PathBuf> {
    let patch_files = base_plus_tests_patch_files(diff);
    let worktrees_dir = out.join("proof-worktrees");
    fs::create_dir_all(&worktrees_dir)
        .with_context(|| format!("create {}", worktrees_dir.display()))?;
    let worktree = worktrees_dir.join("base-plus-tests");
    if worktree.exists() {
        let _ = cleanup_base_plus_tests_worktree(root, &worktree);
        if worktree.exists() {
            safe_remove_dir_all_under(&worktrees_dir, &worktree)?;
        }
    }

    let add_args = vec![
        "worktree".to_owned(),
        "add".to_owned(),
        "--detach".to_owned(),
        worktree.to_string_lossy().to_string(),
        diff.base.clone(),
    ];
    git_text_owned(root, &add_args).with_context(|| {
        format!(
            "create base+tests worktree at {} from {}",
            worktree.display(),
            diff.base
        )
    })?;

    if !patch_files.is_empty() {
        let patch = base_plus_tests_patch(root, diff, &patch_files)?;
        let proof_dir = out.join("proof");
        fs::create_dir_all(&proof_dir)
            .with_context(|| format!("create {}", proof_dir.display()))?;
        let patch_path = proof_dir.join("base-plus-tests.patch");
        fs::write(&patch_path, patch).with_context(|| format!("write {}", patch_path.display()))?;

        let apply_args = vec![
            "apply".to_owned(),
            "--whitespace=nowarn".to_owned(),
            patch_path.to_string_lossy().to_string(),
        ];
        if let Err(error) = git_text_owned(&worktree, &apply_args)
            .with_context(|| format!("apply test-only patch in {}", worktree.display()))
        {
            let _ = cleanup_base_plus_tests_worktree(root, &worktree);
            return Err(error);
        }
    }

    Ok(worktree)
}

fn base_plus_tests_patch(root: &Path, diff: &DiffContext, files: &[String]) -> Result<String> {
    let mut args = vec![
        "diff".to_owned(),
        "--patch".to_owned(),
        format!("{}...{}", diff.base, diff.head),
        "--".to_owned(),
    ];
    args.extend(files.iter().cloned());
    let patch = git_text_owned(root, &args).or_else(|_| {
        let mut fallback = vec![
            "diff".to_owned(),
            "--patch".to_owned(),
            diff.base.clone(),
            diff.head.clone(),
            "--".to_owned(),
        ];
        fallback.extend(files.iter().cloned());
        git_text_owned(root, &fallback)
    })?;
    if patch.trim().is_empty() {
        bail!("test-only diff for base+tests worktree was empty");
    }
    Ok(patch)
}

fn base_plus_tests_patch_files(diff: &DiffContext) -> Vec<String> {
    diff.changed_files
        .iter()
        .filter(|path| is_base_plus_tests_patch_file(path))
        .cloned()
        .collect()
}

fn is_base_plus_tests_patch_file(path: &str) -> bool {
    let path = normalize_repo_path(path);
    if !is_repo_relative_path(&path) {
        return false;
    }
    if is_bun_focused_test_file(&path) {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.starts_with("fixtures/")
        || lower.contains("/fixtures/")
        || lower.contains("/fixture/")
        || lower.contains("doc-test")
        || lower.contains("doctest")
}

pub(crate) fn cleanup_base_plus_tests_worktree(root: &Path, worktree: &Path) -> Result<()> {
    let worktree_arg = worktree.to_string_lossy().to_string();
    let remove_args = vec![
        "worktree".to_owned(),
        "remove".to_owned(),
        "--force".to_owned(),
        worktree_arg,
    ];
    let _ = git_text_owned(root, &remove_args);
    if worktree.exists() {
        let parent = worktree
            .parent()
            .context("base+tests worktree had no parent directory")?;
        safe_remove_dir_all_under(parent, worktree)?;
    }
    let prune_args = vec!["worktree".to_owned(), "prune".to_owned()];
    let _ = git_text_owned(root, &prune_args);
    Ok(())
}

fn safe_remove_dir_all_under(parent: &Path, target: &Path) -> Result<()> {
    let parent_abs = parent
        .canonicalize()
        .with_context(|| format!("resolve {}", parent.display()))?;
    let target_abs = target
        .canonicalize()
        .with_context(|| format!("resolve {}", target.display()))?;
    if !target_abs.starts_with(&parent_abs) {
        bail!(
            "refusing to remove {} outside {}",
            target_abs.display(),
            parent_abs.display()
        );
    }
    fs::remove_dir_all(&target_abs).with_context(|| format!("remove {}", target_abs.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::run_test_command;

    #[test]
    fn prepare_base_plus_tests_worktree_allows_source_only_request_without_test_patch() -> Result<()>
    {
        let repo = tempfile::tempdir()?;
        fs::create_dir_all(repo.path().join("src"))?;
        fs::write(repo.path().join("src/lib.rs"), "pub fn current() {}\n")?;
        run_test_command(repo.path(), "git", &["init"])?;
        run_test_command(
            repo.path(),
            "git",
            &["config", "user.email", "ub-review@example.invalid"],
        )?;
        run_test_command(
            repo.path(),
            "git",
            &["config", "user.name", "UB Review Test"],
        )?;
        run_test_command(repo.path(), "git", &["add", "."])?;
        run_test_command(
            repo.path(),
            "git",
            &["-c", "commit.gpgsign=false", "commit", "-m", "initial"],
        )?;

        let out = tempfile::tempdir()?;
        let diff = DiffContext {
            base: "HEAD".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["src/lib.rs".to_owned()],
            patch: "+pub fn changed() {}\n".to_owned(),
            flags: DiffFlags {
                source_changed: true,
                rust_changed: true,
                rust_tests_changed: false,
                workflow_changed: false,
                dependency_changed: false,
                shell_changed: false,
                cpp_changed: false,
                docs_only: false,
                unsafe_or_native_risk: true,
            },
            diff_class: DiffClass::SourceUb,
        };
        assert!(base_plus_tests_patch_files(&diff).is_empty());

        let worktree = prepare_base_plus_tests_worktree(repo.path(), out.path(), &diff)?;

        assert!(worktree.join("src/lib.rs").exists());
        assert!(!out.path().join("proof/base-plus-tests.patch").exists());
        cleanup_base_plus_tests_worktree(repo.path(), &worktree)?;
        Ok(())
    }

    #[test]
    fn base_plus_tests_patch_files_excludes_source_fix_files() {
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec![
                "src/native/write.rs".to_owned(),
                "test/js/node/fs/write.test.ts".to_owned(),
                "test/fixtures/fs/write.bin".to_owned(),
                "docs/usage.md".to_owned(),
                "tests/doctest/bytea.md".to_owned(),
            ],
            patch: String::new(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::SourceUb,
        };

        let files = base_plus_tests_patch_files(&diff);

        assert_eq!(
            files,
            vec![
                "test/js/node/fs/write.test.ts".to_owned(),
                "test/fixtures/fs/write.bin".to_owned(),
                "tests/doctest/bytea.md".to_owned(),
            ]
        );
    }
}
