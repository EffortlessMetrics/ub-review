//! Base+tests proof worktree preparation and cleanup.
//!
//! The base+tests worktree is the red/green discriminator: the PR's test
//! changes are applied on top of the base commit *without* its production
//! changes, so a passing base run means the new tests do not pin the change.
//! Selecting whole files by path prefix cannot express that for Rust, whose
//! unit tests live in a `#[cfg(test)] mod tests` block inside the production
//! file, so the patch is assembled from hunk-level classification instead
//! (see [`rust_test_regions`] and [`crate::proof::patch_split`]).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::Serialize;

use crate::*;

pub(crate) const BASE_PLUS_TESTS_SELECTION_SCHEMA: &str = "ub-review.base_plus_tests_selection.v1";

/// What the base+tests patch builder decided about one changed file.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct BasePlusTestsFileSelection {
    pub(crate) path: String,
    /// `whole-file`, `hunk-split`, `legacy-path-filter`, or `excluded`.
    pub(crate) mode: String,
    pub(crate) kept_hunks: usize,
    pub(crate) dropped_hunks: usize,
    pub(crate) reason: String,
}

/// Receipt of how the base+tests patch was assembled, written next to the patch
/// so a reader can see which changes reached the base run and which did not.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct BasePlusTestsSelection {
    pub(crate) schema: String,
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) files: Vec<BasePlusTestsFileSelection>,
    /// Set when no trustworthy patch could be built; the caller must turn this
    /// into `base_patch_failed` rather than run a misleading base proof.
    pub(crate) refusal: Option<String>,
    #[serde(skip)]
    pub(crate) patch: String,
}

pub(crate) fn prepare_base_plus_tests_worktree(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
) -> Result<PathBuf> {
    let selection = base_plus_tests_selection(root, diff)?;
    if !selection.files.is_empty() {
        write_base_plus_tests_selection(out, &selection)?;
    }
    if let Some(refusal) = &selection.refusal {
        bail!("{refusal}");
    }
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

    if !selection.patch.trim().is_empty() {
        let proof_dir = out.join("proof");
        fs::create_dir_all(&proof_dir)
            .with_context(|| format!("create {}", proof_dir.display()))?;
        let patch_path = proof_dir.join("base-plus-tests.patch");
        fs::write(&patch_path, &selection.patch)
            .with_context(|| format!("write {}", patch_path.display()))?;

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

fn write_base_plus_tests_selection(out: &Path, selection: &BasePlusTestsSelection) -> Result<()> {
    let proof_dir = out.join("proof");
    fs::create_dir_all(&proof_dir).with_context(|| format!("create {}", proof_dir.display()))?;
    let path = proof_dir.join("base-plus-tests-selection.json");
    let body = serde_json::to_string_pretty(selection)?;
    fs::write(&path, format!("{body}\n")).with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// Build the base+tests patch: whole sections for genuinely test-only paths,
/// hunk-level test slices for Rust files that mix test and production changes,
/// and a refusal when neither is safe.
pub(crate) fn base_plus_tests_selection(
    root: &Path,
    diff: &DiffContext,
) -> Result<BasePlusTestsSelection> {
    let mut files = Vec::new();
    let mut whole_file_paths = Vec::new();
    let mut split_paths = Vec::new();
    for path in &diff.changed_files {
        let path = normalize_repo_path(path);
        if !is_repo_relative_path(&path) {
            files.push(excluded(&path, "path is not repo-relative"));
        } else if is_base_plus_tests_whole_file_path(&path) {
            whole_file_paths.push(path);
        } else if path.ends_with(".rs") {
            split_paths.push(path);
        } else {
            files.push(excluded(
                &path,
                "not a test-tree path and not Rust source; production change withheld from base",
            ));
        }
    }
    let mut selection = BasePlusTestsSelection {
        schema: BASE_PLUS_TESTS_SELECTION_SCHEMA.to_owned(),
        base: diff.base.clone(),
        head: diff.head.clone(),
        files,
        refusal: None,
        patch: String::new(),
    };
    if whole_file_paths.is_empty() && split_paths.is_empty() {
        selection
            .files
            .sort_by(|left, right| left.path.cmp(&right.path));
        return Ok(selection);
    }

    let mut pathspecs = whole_file_paths.clone();
    pathspecs.extend(split_paths.iter().cloned());
    pathspecs.sort();
    pathspecs.dedup();
    let (patch_text, old_rev) = base_plus_tests_diff(root, diff, &pathspecs)?;
    if patch_text.trim().is_empty() {
        if !whole_file_paths.is_empty() {
            bail!("test-only diff for base+tests worktree was empty");
        }
        for path in &split_paths {
            selection
                .files
                .push(excluded(path, "no textual change in the base..head range"));
        }
        selection
            .files
            .sort_by(|left, right| left.path.cmp(&right.path));
        return Ok(selection);
    }

    let parsed = match parse_unified_diff(&patch_text) {
        Ok(parsed) => parsed,
        Err(error) if split_paths.is_empty() => {
            // Only test-tree paths were requested, so the unparsed diff is
            // already whole-file test content and can be applied verbatim.
            for path in &whole_file_paths {
                selection.files.push(BasePlusTestsFileSelection {
                    path: path.clone(),
                    mode: "legacy-path-filter".to_owned(),
                    kept_hunks: 0,
                    dropped_hunks: 0,
                    reason: format!("diff could not be parsed for hunk splitting: {error:#}"),
                });
            }
            selection.patch = patch_text;
            selection
                .files
                .sort_by(|left, right| left.path.cmp(&right.path));
            return Ok(selection);
        }
        Err(error) => {
            selection.refusal = Some(format!(
                "could not parse the base..head diff for hunk classification: {error:#}"
            ));
            selection
                .files
                .sort_by(|left, right| left.path.cmp(&right.path));
            return Ok(selection);
        }
    };

    let mut sections = Vec::new();
    let mut seen = BTreeSet::new();
    for file in &parsed {
        let new_path = normalize_repo_path(&file.new_path);
        let key = if new_path.is_empty() {
            normalize_repo_path(&file.old_path)
        } else {
            new_path
        };
        seen.insert(key.clone());
        if whole_file_paths.contains(&key) {
            sections.push(file.raw.clone());
            selection.files.push(BasePlusTestsFileSelection {
                path: key,
                mode: "whole-file".to_owned(),
                kept_hunks: file.hunks.len(),
                dropped_hunks: 0,
                reason: "test-tree path applied whole".to_owned(),
            });
            continue;
        }
        if !split_paths.contains(&key) {
            selection.files.push(excluded(
                &key,
                "diff reported a path that was not a base+tests candidate",
            ));
            continue;
        }
        match classify_rust_patch_file(root, &old_rev, &diff.head, file)? {
            RustFileDecision::Split { slices, dropped } => {
                sections.push(render_selected_slices(file, &slices)?);
                selection.files.push(BasePlusTestsFileSelection {
                    path: key,
                    mode: "hunk-split".to_owned(),
                    kept_hunks: slices.len(),
                    dropped_hunks: dropped,
                    reason: "test hunks applied; production hunks withheld".to_owned(),
                });
            }
            RustFileDecision::Excluded(reason) => {
                selection.files.push(excluded(&key, &reason));
            }
            RustFileDecision::Refused(reason) => {
                selection.refusal = Some(reason);
                selection
                    .files
                    .sort_by(|left, right| left.path.cmp(&right.path));
                return Ok(selection);
            }
        }
    }
    for path in whole_file_paths.iter().chain(split_paths.iter()) {
        if !seen.contains(path) {
            selection
                .files
                .push(excluded(path, "no textual change in the base..head range"));
        }
    }
    selection
        .files
        .sort_by(|left, right| left.path.cmp(&right.path));
    selection.patch = sections.concat();
    Ok(selection)
}

fn excluded(path: &str, reason: &str) -> BasePlusTestsFileSelection {
    BasePlusTestsFileSelection {
        path: path.to_owned(),
        mode: "excluded".to_owned(),
        kept_hunks: 0,
        dropped_hunks: 0,
        reason: reason.to_owned(),
    }
}

enum RustFileDecision {
    Split {
        slices: Vec<SelectedSlice>,
        dropped: usize,
    },
    Excluded(String),
    Refused(String),
}

/// Classify one Rust file's hunks against the `#[cfg(test)]` regions of the old
/// and new file versions, keeping only slices whose every changed line is test
/// code on its own side.
fn classify_rust_patch_file(
    root: &Path,
    old_rev: &str,
    head_rev: &str,
    file: &PatchFile,
) -> Result<RustFileDecision> {
    if file.hunks.is_empty() {
        return Ok(RustFileDecision::Excluded(
            "no textual hunks (binary or mode-only change)".to_owned(),
        ));
    }
    if file.new_path.is_empty() || file.header_has("deleted file mode") {
        return Ok(RustFileDecision::Excluded(
            "file is deleted at head, so its tests cannot pin the change".to_owned(),
        ));
    }
    let mentions_test = patch_file_mentions_test_code(file);
    let Some(head_regions) = git_show_content(root, head_rev, &file.new_path)
        .as_deref()
        .and_then(rust_test_regions)
    else {
        return Ok(unscannable(
            mentions_test,
            &format!("{} at {head_rev}", file.new_path),
        ));
    };
    let needs_old_side = file
        .hunks
        .iter()
        .flat_map(|hunk| hunk.lines.iter())
        .any(|line| line.kind == PatchLineKind::Deletion);
    let old_regions = if needs_old_side && !file.old_path.is_empty() {
        match git_show_content(root, old_rev, &file.old_path)
            .as_deref()
            .and_then(rust_test_regions)
        {
            Some(regions) => regions,
            None => {
                return Ok(unscannable(
                    mentions_test,
                    &format!("{} at {old_rev}", file.old_path),
                ));
            }
        }
    } else {
        Vec::new()
    };

    let new_file = file.old_path.is_empty() || file.header_has("new file mode");
    let mut slices = Vec::new();
    let mut dropped = 0_usize;
    for (index, hunk) in file.hunks.iter().enumerate() {
        let mut kept: Vec<std::ops::Range<usize>> = Vec::new();
        for range in hunk_change_slices(hunk) {
            match classify_hunk_slice(hunk, &range, &old_regions, &head_regions) {
                Some(true) => kept.push(range),
                Some(false) => dropped += 1,
                None if new_file => {
                    return Ok(RustFileDecision::Refused(format!(
                        "{} is a new Rust file adding production code and its tests together; a base+tests patch cannot add the test without the code under test",
                        file.new_path
                    )));
                }
                None => {
                    return Ok(RustFileDecision::Refused(format!(
                        "{} hunk at old line {} mixes test and production changes; refusing to build a base+tests patch that could carry the fix",
                        file.new_path, hunk.old_start
                    )));
                }
            }
        }
        // Neighbouring slices share their context lines; merging them keeps the
        // emitted hunks disjoint.
        for range in kept {
            match slices.last_mut() {
                Some(SelectedSlice {
                    hunk: last_hunk,
                    lines,
                }) if *last_hunk == index && range.start <= lines.end => {
                    lines.end = lines.end.max(range.end);
                }
                _ => slices.push(SelectedSlice {
                    hunk: index,
                    lines: range,
                }),
            }
        }
    }
    if slices.is_empty() {
        return Ok(RustFileDecision::Excluded(
            "no test hunks; production-only change withheld from base".to_owned(),
        ));
    }
    Ok(RustFileDecision::Split { slices, dropped })
}

fn unscannable(mentions_test: bool, subject: &str) -> RustFileDecision {
    if mentions_test {
        RustFileDecision::Refused(format!(
            "could not resolve Rust test regions of {subject}, and the diff touches test code; refusing to guess which hunks are tests"
        ))
    } else {
        RustFileDecision::Excluded(format!(
            "could not resolve Rust test regions of {subject}, and the diff touches no test code"
        ))
    }
}

/// `Some(true)` for a test-only slice, `Some(false)` for a production-only
/// slice, `None` when the slice mixes both and cannot be trusted either way.
fn classify_hunk_slice(
    hunk: &PatchHunk,
    range: &std::ops::Range<usize>,
    old_regions: &[TestRegion],
    head_regions: &[TestRegion],
) -> Option<bool> {
    let mut test = false;
    let mut production = false;
    for line in hunk.lines.get(range.clone()).unwrap_or_default() {
        let in_test = match line.kind {
            PatchLineKind::Context => continue,
            PatchLineKind::Deletion => line
                .old_line
                .is_some_and(|number| line_in_test_regions(old_regions, number)),
            PatchLineKind::Addition => line
                .new_line
                .is_some_and(|number| line_in_test_regions(head_regions, number)),
        };
        if in_test {
            test = true;
        } else {
            production = true;
        }
    }
    match (test, production) {
        (true, false) => Some(true),
        (false, true) => Some(false),
        // An empty slice cannot exist (parsing rejects changeless hunks), and a
        // mixed slice is exactly the case we must not guess at.
        _ => None,
    }
}

/// True when any changed line names a Rust test construct, used to decide
/// whether an unclassifiable file must refuse or may simply be excluded.
fn patch_file_mentions_test_code(file: &PatchFile) -> bool {
    file.changed_line_texts().iter().any(|line| {
        line.contains("cfg(test")
            || line.contains("#[test]")
            || line.contains("::test]")
            || line.contains("mod tests")
            || line.contains("mod test ")
    })
}

fn git_show_content(root: &Path, rev: &str, path: &str) -> Option<String> {
    let args = vec!["show".to_owned(), format!("{rev}:{path}")];
    git_text_owned(root, &args).ok()
}

/// The patch and the revision its old side belongs to. `base...head` compares
/// against the merge base, so classification of removed lines must read the
/// merge base, not `base`.
fn base_plus_tests_diff(
    root: &Path,
    diff: &DiffContext,
    files: &[String],
) -> Result<(String, String)> {
    let merge_base = git_text_owned(
        root,
        &[
            "merge-base".to_owned(),
            diff.base.clone(),
            diff.head.clone(),
        ],
    )
    .ok()
    .and_then(|text| text.lines().next().map(str::trim).map(str::to_owned))
    .filter(|rev| !rev.is_empty());
    if let Some(merge_base) = merge_base {
        let args =
            base_plus_tests_diff_args(&format!("{}...{}", diff.base, diff.head), None, files);
        if let Ok(patch) = git_text_owned(root, &args) {
            return Ok((patch, merge_base));
        }
    }
    let args = base_plus_tests_diff_args(&diff.base, Some(&diff.head), files);
    let patch = git_text_owned(root, &args)?;
    Ok((patch, diff.base.clone()))
}

/// Pin every diff knob that repository or user configuration could otherwise
/// move, so the same input range always yields the same patch bytes.
fn base_plus_tests_diff_args(range: &str, second: Option<&str>, files: &[String]) -> Vec<String> {
    let mut args = vec![
        "diff".to_owned(),
        "--patch".to_owned(),
        "--no-color".to_owned(),
        "--no-ext-diff".to_owned(),
        "--src-prefix=a/".to_owned(),
        "--dst-prefix=b/".to_owned(),
        "--unified=3".to_owned(),
        range.to_owned(),
    ];
    if let Some(second) = second {
        args.push(second.to_owned());
    }
    args.push("--".to_owned());
    args.extend(files.iter().cloned());
    args
}

/// Paths whose whole diff is test content, so no hunk classification is needed.
fn is_base_plus_tests_whole_file_path(path: &str) -> bool {
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

    fn init_repo(root: &Path) -> Result<()> {
        run_test_command(root, "git", &["init", "--initial-branch=main"])?;
        run_test_command(
            root,
            "git",
            &["config", "user.email", "ub-review@example.invalid"],
        )?;
        run_test_command(root, "git", &["config", "user.name", "UB Review Test"])?;
        Ok(())
    }

    fn commit_all(root: &Path, message: &str) -> Result<String> {
        run_test_command(root, "git", &["add", "-A"])?;
        run_test_command(
            root,
            "git",
            &["-c", "commit.gpgsign=false", "commit", "-m", message],
        )?;
        Ok(
            git_text_owned(root, &["rev-parse".to_owned(), "HEAD".to_owned()])?
                .trim()
                .to_owned(),
        )
    }

    fn diff_context(base: &str, head: &str, changed: &[&str]) -> DiffContext {
        DiffContext {
            base: base.to_owned(),
            head: head.to_owned(),
            changed_files: changed.iter().map(|path| (*path).to_owned()).collect(),
            patch: String::new(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::SourceUb,
        }
    }

    /// This repository's own shape: the production change and the new unit test
    /// live in the same `src/*.rs` file.
    const BASE_SOURCE: &str = "pub fn classify(value: u8) -> &'static str {\n    if value > 10 {\n        \"high\"\n    } else {\n        \"low\"\n    }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn classifies_high() {\n        assert_eq!(classify(11), \"high\");\n    }\n}\n";
    const HEAD_SOURCE: &str = "pub fn classify(value: u8) -> &'static str {\n    if value >= 10 {\n        \"high\"\n    } else {\n        \"low\"\n    }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn classifies_high() {\n        assert_eq!(classify(11), \"high\");\n    }\n\n    #[test]\n    fn classifies_boundary() {\n        assert_eq!(classify(10), \"high\");\n    }\n}\n";

    fn mixed_rust_repo(root: &Path) -> Result<(String, String)> {
        init_repo(root)?;
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src/lib.rs"), BASE_SOURCE)?;
        let base = commit_all(root, "base")?;
        fs::write(root.join("src/lib.rs"), HEAD_SOURCE)?;
        let head = commit_all(root, "head")?;
        Ok((base, head))
    }

    #[test]
    fn mixed_rust_file_keeps_the_test_hunk_and_drops_the_production_hunk() -> Result<()> {
        let repo = tempfile::tempdir()?;
        let (base, head) = mixed_rust_repo(repo.path())?;
        let diff = diff_context(&base, &head, &["src/lib.rs"]);

        let selection = base_plus_tests_selection(repo.path(), &diff)?;

        assert!(selection.refusal.is_none(), "{selection:?}");
        let record = selection
            .files
            .first()
            .ok_or_else(|| anyhow::anyhow!("expected one selection record"))?;
        assert_eq!(record.mode, "hunk-split");
        assert_eq!(record.kept_hunks, 1);
        assert_eq!(record.dropped_hunks, 1);
        assert!(
            selection.patch.contains("fn classifies_boundary"),
            "{}",
            selection.patch
        );
        assert!(
            !selection.patch.contains("value >= 10"),
            "production change leaked into the base+tests patch:\n{}",
            selection.patch
        );
        Ok(())
    }

    #[test]
    fn mixed_rust_file_patch_applies_to_the_base_tree() -> Result<()> {
        let repo = tempfile::tempdir()?;
        let (base, head) = mixed_rust_repo(repo.path())?;
        let diff = diff_context(&base, &head, &["src/lib.rs"]);
        let out = tempfile::tempdir()?;

        let worktree = prepare_base_plus_tests_worktree(repo.path(), out.path(), &diff)?;
        let applied = fs::read_to_string(worktree.join("src/lib.rs"))?;

        assert!(applied.contains("fn classifies_boundary"), "{applied}");
        assert!(applied.contains("value > 10"), "{applied}");
        assert!(!applied.contains("value >= 10"), "{applied}");
        assert!(
            out.path()
                .join("proof/base-plus-tests-selection.json")
                .exists()
        );
        cleanup_base_plus_tests_worktree(repo.path(), &worktree)?;
        Ok(())
    }

    #[test]
    fn selection_is_byte_identical_across_runs() -> Result<()> {
        let repo = tempfile::tempdir()?;
        let (base, head) = mixed_rust_repo(repo.path())?;
        let diff = diff_context(&base, &head, &["src/lib.rs"]);

        let first = base_plus_tests_selection(repo.path(), &diff)?;
        let second = base_plus_tests_selection(repo.path(), &diff)?;

        assert_eq!(first.patch, second.patch);
        assert_eq!(
            serde_json::to_string(&first)?,
            serde_json::to_string(&second)?
        );
        Ok(())
    }

    #[test]
    fn production_only_rust_change_yields_no_patch() -> Result<()> {
        let repo = tempfile::tempdir()?;
        init_repo(repo.path())?;
        fs::create_dir_all(repo.path().join("src"))?;
        fs::write(repo.path().join("src/lib.rs"), BASE_SOURCE)?;
        let base = commit_all(repo.path(), "base")?;
        fs::write(
            repo.path().join("src/lib.rs"),
            BASE_SOURCE.replace("value > 10", "value >= 10"),
        )?;
        let head = commit_all(repo.path(), "head")?;
        let diff = diff_context(&base, &head, &["src/lib.rs"]);

        let selection = base_plus_tests_selection(repo.path(), &diff)?;

        assert!(selection.refusal.is_none(), "{selection:?}");
        assert!(selection.patch.is_empty(), "{}", selection.patch);
        let record = selection
            .files
            .first()
            .ok_or_else(|| anyhow::anyhow!("expected one selection record"))?;
        assert_eq!(record.mode, "excluded");
        assert!(record.reason.contains("production-only"), "{record:?}");
        Ok(())
    }

    #[test]
    fn mixed_hunk_refuses_instead_of_guessing() -> Result<()> {
        let repo = tempfile::tempdir()?;
        init_repo(repo.path())?;
        fs::create_dir_all(repo.path().join("src"))?;
        // The production line and the `#[cfg(test)]` line change together, so
        // one change run spans both sides of the test boundary.
        let base =
            "pub fn f() -> u8 { 1 }\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        let head = "pub fn f() -> u8 { 2 }\n#[cfg(all(test, not(miri)))]\nmod tests {\n    #[test]\n    fn t() {}\n}\n";
        fs::write(repo.path().join("src/lib.rs"), base)?;
        let base_rev = commit_all(repo.path(), "base")?;
        fs::write(repo.path().join("src/lib.rs"), head)?;
        let head_rev = commit_all(repo.path(), "head")?;
        let diff = diff_context(&base_rev, &head_rev, &["src/lib.rs"]);

        let selection = base_plus_tests_selection(repo.path(), &diff)?;

        let refusal = selection
            .refusal
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("expected a refusal, got {selection:?}"))?;
        assert!(refusal.contains("mixes test and production"), "{refusal}");
        Ok(())
    }

    #[test]
    fn new_rust_file_carrying_its_own_tests_refuses() -> Result<()> {
        let repo = tempfile::tempdir()?;
        init_repo(repo.path())?;
        fs::create_dir_all(repo.path().join("src"))?;
        fs::write(repo.path().join("src/lib.rs"), "pub mod added;\n")?;
        let base = commit_all(repo.path(), "base")?;
        fs::write(repo.path().join("src/added.rs"), BASE_SOURCE)?;
        let head = commit_all(repo.path(), "head")?;
        let diff = diff_context(&base, &head, &["src/added.rs"]);

        let selection = base_plus_tests_selection(repo.path(), &diff)?;

        let refusal = selection
            .refusal
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("expected a refusal, got {selection:?}"))?;
        assert!(refusal.contains("new Rust file"), "{refusal}");
        Ok(())
    }

    /// A brand-new file that is nothing but tests can still discriminate: it
    /// pins production code that already exists at base.
    #[test]
    fn new_test_only_rust_file_is_kept() -> Result<()> {
        let repo = tempfile::tempdir()?;
        init_repo(repo.path())?;
        fs::create_dir_all(repo.path().join("src"))?;
        fs::write(repo.path().join("src/lib.rs"), BASE_SOURCE)?;
        let base = commit_all(repo.path(), "base")?;
        fs::write(
            repo.path().join("src/extra_tests.rs"),
            "#![cfg(test)]\n\n#[test]\nfn boundary() {\n    assert_eq!(crate::classify(10), \"high\");\n}\n",
        )?;
        let head = commit_all(repo.path(), "head")?;
        let diff = diff_context(&base, &head, &["src/extra_tests.rs"]);

        let selection = base_plus_tests_selection(repo.path(), &diff)?;

        assert!(selection.refusal.is_none(), "{selection:?}");
        let record = selection
            .files
            .first()
            .ok_or_else(|| anyhow::anyhow!("expected one selection record"))?;
        assert_eq!(record.mode, "hunk-split");
        assert!(
            selection.patch.contains("fn boundary"),
            "{}",
            selection.patch
        );
        Ok(())
    }

    #[test]
    fn test_only_file_is_still_applied_whole() -> Result<()> {
        let repo = tempfile::tempdir()?;
        init_repo(repo.path())?;
        fs::create_dir_all(repo.path().join("test/js"))?;
        fs::create_dir_all(repo.path().join("src"))?;
        fs::write(
            repo.path().join("src/native.c"),
            "int f(void) { return 1; }\n",
        )?;
        fs::write(
            repo.path().join("test/js/write.test.ts"),
            "test(\"writes\", () => {});\n",
        )?;
        let base = commit_all(repo.path(), "base")?;
        fs::write(
            repo.path().join("src/native.c"),
            "int f(void) { return 2; }\n",
        )?;
        fs::write(
            repo.path().join("test/js/write.test.ts"),
            "test(\"writes\", () => {});\ntest(\"writes twice\", () => {});\n",
        )?;
        let head = commit_all(repo.path(), "head")?;
        let diff = diff_context(&base, &head, &["src/native.c", "test/js/write.test.ts"]);

        let selection = base_plus_tests_selection(repo.path(), &diff)?;

        assert!(selection.refusal.is_none(), "{selection:?}");
        assert!(
            selection.patch.contains("writes twice"),
            "{}",
            selection.patch
        );
        assert!(!selection.patch.contains("return 2"), "{}", selection.patch);
        let modes: Vec<(&str, &str)> = selection
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.mode.as_str()))
            .collect();
        assert_eq!(
            modes,
            vec![
                ("src/native.c", "excluded"),
                ("test/js/write.test.ts", "whole-file"),
            ]
        );
        Ok(())
    }

    #[test]
    fn unapplyable_patch_is_reported_by_the_caller_as_a_failure() -> Result<()> {
        let repo = tempfile::tempdir()?;
        let (merge_base, head) = mixed_rust_repo(repo.path())?;
        // A stale base that rewrote the very test region the kept hunk needs as
        // context: the patch is built against the merge base but applied to the
        // requested base, so it cannot apply.
        run_test_command(
            repo.path(),
            "git",
            &["checkout", "-b", "stale", &merge_base],
        )?;
        fs::write(
            repo.path().join("src/lib.rs"),
            BASE_SOURCE.replace("classifies_high", "classifies_high_value"),
        )?;
        let stale_base = commit_all(repo.path(), "stale base")?;
        let diff = diff_context(&stale_base, &head, &["src/lib.rs"]);
        let out = tempfile::tempdir()?;

        let error = prepare_base_plus_tests_worktree(repo.path(), out.path(), &diff)
            .err()
            .ok_or_else(|| anyhow::anyhow!("expected the patch to fail to apply"))?;

        let text = format!("{error:#}");
        assert!(text.contains("apply test-only patch"), "{text}");
        Ok(())
    }

    #[test]
    fn prepare_base_plus_tests_worktree_allows_source_only_request_without_test_patch() -> Result<()>
    {
        let repo = tempfile::tempdir()?;
        fs::create_dir_all(repo.path().join("src"))?;
        fs::write(repo.path().join("src/lib.rs"), "pub fn current() {}\n")?;
        init_repo(repo.path())?;
        commit_all(repo.path(), "initial")?;

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
        assert!(
            !diff
                .changed_files
                .iter()
                .any(|path| is_base_plus_tests_whole_file_path(path))
        );

        let worktree = prepare_base_plus_tests_worktree(repo.path(), out.path(), &diff)?;

        assert!(worktree.join("src/lib.rs").exists());
        assert!(!out.path().join("proof/base-plus-tests.patch").exists());
        cleanup_base_plus_tests_worktree(repo.path(), &worktree)?;
        Ok(())
    }

    #[test]
    fn whole_file_paths_exclude_source_fix_files() {
        let changed = [
            "src/native/write.rs",
            "test/js/node/fs/write.test.ts",
            "test/fixtures/fs/write.bin",
            "docs/usage.md",
            "tests/doctest/bytea.md",
        ];

        let files = changed
            .iter()
            .filter(|path| is_base_plus_tests_whole_file_path(path))
            .map(|path| (*path).to_owned())
            .collect::<Vec<_>>();

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
