//! Ordinary Git admission into the immutable revision identity (#949,
//! roadmap A1.2).
//!
//! This module resolves symbolic refs into exact commit/tree objects and
//! admits the reviewed revision as a `RevisionIdentity` with explicit
//! semantics. It never guesses: a GitHub-style synthetic merge checkout
//! without pull-request head metadata is an admission failure, not a
//! relabeled candidate head.

use crate::plan_build::{git_lines, git_text};
use crate::revision_identity::{CommitTree, ReviewSemantics, RevisionIdentity};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command as ProcessCommand;
use std::sync::atomic::{AtomicU64, Ordering};

/// Artifact schema for `input/revision-admission.json`.
pub(crate) const REVISION_ADMISSION_SCHEMA: &str = "ub-review.revision_admission.v1";

/// Admitted identity of one review pass over an exact revision.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct RevisionAdmission {
    pub(crate) schema: String,
    /// Canonical form of the admitted `RevisionIdentity`.
    pub(crate) identity_canonical: String,
    /// Domain-separated digest over [`Self::identity_canonical`].
    pub(crate) identity_digest: String,
    /// `"candidate_head"` or `"merge_result"`, mirroring the canonical form.
    pub(crate) semantics: String,
    /// Exact git commit object the run reviewed (the identity's reviewed
    /// pair), for consumers that bind to real objects such as delivery
    /// exact-head checks.
    pub(crate) reviewed_commit_oid: String,
    /// Resolved PR-head commit when hosted metadata was supplied.
    #[serde(default)]
    pub(crate) pr_head_commit: Option<String>,
    /// Typed evidence: the worktree had uncommitted or untracked entries at
    /// admission time. Dirt never changes commit objects, so it is recorded,
    /// not rejected.
    pub(crate) worktree_dirty: bool,
}

/// Complete trusted-base input set for an explicit diff admission. The head
/// SHA is an identity label only: admission never resolves or checks it out.
pub(crate) struct TrustedDiffInputs {
    pub(crate) base_tree: String,
    pub(crate) head_sha: String,
    pub(crate) changed_files: PathBuf,
    pub(crate) diff_patch: PathBuf,
    pub(crate) pr_head_sha: Option<String>,
}

const MAX_TRUSTED_CHANGED_FILES_BYTES: u64 = 1024 * 1024;
const MAX_TRUSTED_DIFF_PATCH_BYTES: u64 = 64 * 1024 * 1024;

/// Admits a candidate diff while the repository remains on a clean,
/// base-owned checkout. The patch is applied only to a temporary Git index
/// and temporary object directory, which derives a real candidate tree object
/// without populating or reading a hostile head checkout.
pub(crate) fn admit_trusted_diff(
    root: &Path,
    inputs: &TrustedDiffInputs,
) -> Result<(crate::DiffContext, RevisionAdmission)> {
    let base_tree = inputs.base_tree.trim();
    let head_sha = inputs.head_sha.trim();
    if let Some(pr_head_sha) = inputs
        .pr_head_sha
        .as_deref()
        .map(str::trim)
        .filter(|sha| !sha.is_empty())
        && pr_head_sha != head_sha
    {
        bail!(
            "trusted head SHA {head_sha} does not match supplied pull-request head {pr_head_sha}"
        );
    }

    let base = resolve_pair(root, "trusted base root", "HEAD")?;
    if base.tree_oid() != base_tree {
        bail!(
            "trusted base tree {base_tree} does not match the clean root tree {}",
            base.tree_oid()
        );
    }
    if !git_lines(root, &["status", "--porcelain"])?.is_empty() {
        bail!("trusted-base diff admission requires a clean base-owned root");
    }
    // Shape-check the supplied head label before reading or applying any
    // candidate-controlled diff bytes. The derived tree replaces this
    // placeholder when the final identity is constructed.
    CommitTree::new("trusted head label", head_sha, base_tree)?;

    let changed_files = read_trusted_changed_files(&inputs.changed_files)?;
    let patch = read_bounded_utf8(
        &inputs.diff_patch,
        MAX_TRUSTED_DIFF_PATCH_BYTES,
        "trusted diff patch",
    )?;
    if patch.is_empty() {
        bail!("trusted diff patch must not be empty");
    }

    let (derived_tree, derived_paths) = derive_patched_tree(root, base_tree, patch.as_bytes())?;
    if derived_paths != changed_files {
        bail!(
            "trusted changed-path object does not match the patch tree delta: supplied={changed_files:?}, derived={derived_paths:?}"
        );
    }

    let head = CommitTree::new("trusted head", head_sha, &derived_tree)?;
    if head.commit_oid() == base.commit_oid() && head.tree_oid() != base.tree_oid() {
        bail!("trusted head SHA equals the base commit but the supplied patch changes its tree");
    }
    let identity = RevisionIdentity::new(
        base,
        head.clone(),
        head,
        None,
        ReviewSemantics::CandidateHead,
        &RevisionIdentity::changed_paths_digest(&changed_files),
        &RevisionIdentity::diff_digest(patch.as_bytes()),
    )?;
    let admission = RevisionAdmission {
        schema: REVISION_ADMISSION_SCHEMA.to_owned(),
        identity_canonical: identity.canonical_form(),
        identity_digest: identity.identity_digest(),
        semantics: ReviewSemantics::CandidateHead.as_str().to_owned(),
        reviewed_commit_oid: head_sha.to_owned(),
        pr_head_commit: Some(head_sha.to_owned()),
        worktree_dirty: false,
    };
    admission.validate()?;
    let flags = crate::classify_diff(&changed_files, &patch);
    let diff_class = crate::classify_diff_class(&changed_files, &flags);
    let diff = crate::DiffContext {
        base: base_tree.to_owned(),
        head: head_sha.to_owned(),
        changed_files,
        patch,
        flags,
        diff_class,
    };
    Ok((diff, admission))
}

fn read_trusted_changed_files(path: &Path) -> Result<Vec<String>> {
    let text = read_bounded_utf8(
        path,
        MAX_TRUSTED_CHANGED_FILES_BYTES,
        "trusted changed-path object",
    )?;
    let mut paths = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        if raw.is_empty() || raw != raw.trim() {
            bail!(
                "trusted changed path line {} must be non-empty and have no surrounding whitespace",
                index + 1
            );
        }
        validate_trusted_path(raw)
            .with_context(|| format!("trusted changed path line {}", index + 1))?;
        paths.push(raw.to_owned());
    }
    if paths.is_empty() {
        bail!("trusted changed-path object must contain at least one path");
    }
    if paths.windows(2).any(|pair| pair[0] >= pair[1]) {
        bail!("trusted changed paths must be strictly sorted and duplicate-free");
    }
    Ok(paths)
}

fn read_bounded_utf8(path: &Path, max_bytes: u64, label: &str) -> Result<String> {
    let file = fs::File::open(path).with_context(|| format!("open {label} {}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(max_bytes + 1)
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {label} {}", path.display()))?;
    if bytes.len() as u64 > max_bytes {
        bail!("{label} exceeds the {max_bytes}-byte admission limit");
    }
    String::from_utf8(bytes).with_context(|| format!("{label} must be valid UTF-8"))
}

fn validate_trusted_path(value: &str) -> Result<()> {
    if value.contains('\0') || value.contains('\\') || value.contains(':') || value.starts_with('/')
    {
        bail!("path must be a normalized repository-relative slash path: `{value}`");
    }
    let mut normalized = Vec::new();
    for component in Path::new(value).components() {
        match component {
            Component::Normal(name) if !name.eq_ignore_ascii_case(".git") => {
                normalized.push(name.to_string_lossy().into_owned());
            }
            _ => bail!("path contains a forbidden component: `{value}`"),
        }
    }
    if normalized.join("/") != value {
        bail!("path is not normalized: `{value}`");
    }
    Ok(())
}

static ADMISSION_SCRATCH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct AdmissionScratch {
    root: PathBuf,
    index: PathBuf,
    objects: PathBuf,
    patch: PathBuf,
}

impl AdmissionScratch {
    fn create() -> Result<Self> {
        for _ in 0..32 {
            let sequence = ADMISSION_SCRATCH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "ub-review-trusted-diff-{}-{sequence}",
                std::process::id()
            ));
            match fs::create_dir(&root) {
                Ok(()) => {
                    let objects = root.join("objects");
                    if let Err(error) = fs::create_dir(&objects) {
                        let _ = fs::remove_dir(&root);
                        return Err(error).context("create trusted-diff object directory");
                    }
                    return Ok(Self {
                        index: root.join("index"),
                        patch: root.join("candidate.patch"),
                        objects,
                        root,
                    });
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error).context("create trusted-diff scratch directory"),
            }
        }
        bail!("could not allocate a unique trusted-diff scratch directory")
    }

    fn close(mut self) -> Result<()> {
        fs::remove_dir_all(&self.root).with_context(|| {
            format!(
                "remove trusted-diff scratch directory {}",
                self.root.display()
            )
        })?;
        self.root = PathBuf::new();
        Ok(())
    }
}

impl Drop for AdmissionScratch {
    fn drop(&mut self) {
        if !self.root.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

fn derive_patched_tree(
    root: &Path,
    base_tree: &str,
    patch: &[u8],
) -> Result<(String, Vec<String>)> {
    let scratch = AdmissionScratch::create()?;
    let object_dir_text = git_text(root, &["rev-parse", "--git-path", "objects"])?;
    let object_dir = PathBuf::from(object_dir_text.trim());
    let object_dir = if object_dir.is_absolute() {
        object_dir
    } else {
        root.join(object_dir)
    };
    fs::write(&scratch.patch, patch).context("write private trusted-diff patch copy")?;
    trusted_git(root, &scratch, &object_dir, ["read-tree", base_tree])?;
    trusted_git_paths(
        root,
        &scratch,
        &object_dir,
        &[
            "apply",
            "--cached",
            "--recount",
            "--whitespace=nowarn",
            "--",
        ],
        Some(&scratch.patch),
    )?;
    let head_tree = trusted_git(root, &scratch, &object_dir, ["write-tree"])?
        .trim()
        .to_owned();
    let output = trusted_git(
        root,
        &scratch,
        &object_dir,
        [
            "-c",
            "core.quotePath=false",
            "diff-tree",
            "--no-commit-id",
            "--name-only",
            "--no-renames",
            "-r",
            base_tree,
            &head_tree,
        ],
    )?;
    let mut paths = output.lines().map(str::to_owned).collect::<Vec<_>>();
    paths.sort();
    paths.dedup();
    for path in &paths {
        validate_trusted_path(path).context("derived patch path")?;
    }
    scratch.close()?;
    Ok((head_tree, paths))
}

fn trusted_git<const N: usize>(
    root: &Path,
    scratch: &AdmissionScratch,
    alternate_objects: &Path,
    args: [&str; N],
) -> Result<String> {
    trusted_git_paths(root, scratch, alternate_objects, &args, None)
}

fn trusted_git_paths(
    root: &Path,
    scratch: &AdmissionScratch,
    alternate_objects: &Path,
    args: &[&str],
    trailing_path: Option<&Path>,
) -> Result<String> {
    let mut command = ProcessCommand::new("git");
    command
        .arg("-C")
        .arg(root)
        .args(args)
        .env("GIT_INDEX_FILE", &scratch.index)
        .env("GIT_OBJECT_DIRECTORY", &scratch.objects)
        .env("GIT_ALTERNATE_OBJECT_DIRECTORIES", alternate_objects)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    if cfg!(windows) {
        command.env("GIT_CONFIG_GLOBAL", "NUL");
    } else {
        command.env("GIT_CONFIG_GLOBAL", "/dev/null");
    }
    if let Some(path) = trailing_path {
        command.arg(path);
    }
    let output = command.output().context("run trusted-diff git operation")?;
    if !output.status.success() {
        bail!(
            "trusted-diff git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Resolves `base_ref`/`head_ref` (symbolic aliases allowed) into exact
/// objects and admits them under explicit semantics.
///
/// `pr_head_sha` is hosted event metadata (`github.event.pull_request.head.sha`).
/// The digests passed in must be computed from the same changed-file set and
/// patch that the run reviews, so they bind to the admitted objects.
pub(crate) fn admit_revision(
    root: &Path,
    base_ref: &str,
    head_ref: &str,
    pr_head_sha: Option<&str>,
    changed_files: &[String],
    patch: &str,
) -> Result<RevisionAdmission> {
    let base = resolve_pair(root, "base", base_ref)?;
    let reviewed = resolve_pair(root, "reviewed", head_ref)?;
    let parents = commit_parents(root, reviewed.commit_oid())?;
    let worktree_dirty = !git_lines(root, &["status", "--porcelain"])?.is_empty();

    let (semantics, head, merge, pr_head_commit) = match parents.len() {
        0 | 1 => {
            // Candidate-head posture: the reviewed tip is the pull-request
            // head itself; hosted metadata, when supplied, must agree.
            if let Some(pr) = pr_head_sha {
                let pr = normalize_oid(pr);
                if reviewed.commit_oid() != pr {
                    bail!(
                        "supplied pull-request head {pr} does not match the reviewed candidate tip {}; stale metadata",
                        reviewed.commit_oid()
                    );
                }
            }
            (
                ReviewSemantics::CandidateHead,
                reviewed.clone(),
                None,
                Some(reviewed.commit_oid().to_owned()),
            )
        }
        n => {
            if n > 2 {
                bail!(
                    "reviewed checkout {} is an octopus merge with {n} parents; multi-head merges are not admissible review targets",
                    reviewed.commit_oid()
                );
            }
            let pr = pr_head_sha.ok_or_else(|| {
                anyhow::anyhow!(
                    "reviewed checkout {} is a merge commit with {n} parents but no pull-request head metadata was supplied (--pr-head-sha); refusing to relabel a synthetic merge as a candidate head",
                    reviewed.commit_oid()
                )
            })?;
            let pr = normalize_oid(pr);
            // GitHub's refs/pull/N/merge convention puts the base-side tip
            // first and the pull-request head second.
            let head_side = &parents[1];
            if head_side != &pr {
                bail!(
                    "supplied pull-request head {pr} is not the head-side parent {head_side} of merge-result checkout {}; stale or forged metadata",
                    reviewed.commit_oid()
                );
            }
            let head = resolve_pair(root, "pr-head", head_side)?;
            (
                ReviewSemantics::MergeResult,
                head,
                Some(reviewed.clone()),
                Some(pr),
            )
        }
    };

    let identity = RevisionIdentity::new(
        base,
        head,
        reviewed.clone(),
        merge,
        semantics,
        &RevisionIdentity::changed_paths_digest(changed_files),
        &RevisionIdentity::diff_digest(patch.as_bytes()),
    )?;
    Ok(RevisionAdmission {
        schema: REVISION_ADMISSION_SCHEMA.to_owned(),
        identity_canonical: identity.canonical_form(),
        identity_digest: identity.identity_digest(),
        semantics: semantics.as_str().to_owned(),
        reviewed_commit_oid: reviewed.commit_oid().to_owned(),
        pr_head_commit,
        worktree_dirty,
    })
}

impl RevisionAdmission {
    /// Re-validates the stored canonical identity on read paths.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema != REVISION_ADMISSION_SCHEMA {
            bail!("unsupported revision admission schema `{}`", self.schema);
        }
        let parsed = RevisionIdentity::from_canonical(&self.identity_canonical)?;
        if parsed.identity_digest() != self.identity_digest {
            bail!("revision admission digest does not match its canonical identity");
        }
        if parsed.canonical_form() != self.identity_canonical {
            bail!("revision admission canonical form is not normalized");
        }
        Ok(())
    }
}

fn normalize_oid(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

/// Resolves one rev into a validated commit/tree pair.
fn resolve_pair(root: &Path, label: &str, rev: &str) -> Result<CommitTree> {
    let commit = git_text(
        root,
        &["rev-parse", "--verify", &format!("{rev}^{{commit}}")],
    )
    .with_context(|| format!("resolve {label} ref `{rev}` to a commit"))?
    .trim()
    .to_owned();
    let tree = git_text(root, &["show", "-s", "--format=%T", &commit])
        .with_context(|| format!("resolve {label} tree for `{commit}`"))?
        .trim()
        .to_owned();
    CommitTree::new(label, &commit, &tree)
}

fn commit_parents(root: &Path, oid: &str) -> Result<Vec<String>> {
    let line = git_lines(root, &["rev-list", "--parents", "-n", "1", oid])
        .with_context(|| format!("list parents of `{oid}`"))?
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("rev-list produced no output for `{oid}`"))?;
    Ok(line.split_whitespace().skip(1).map(str::to_owned).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Repo {
        dir: tempfile::TempDir,
    }

    impl Repo {
        fn root(&self) -> &Path {
            self.dir.path()
        }
    }

    fn git(repo: &Repo, args: &[&str]) -> Result<String> {
        git_text(repo.root(), args)
    }

    fn init_repo() -> Result<Repo> {
        let dir = tempfile::tempdir()?;
        git_text(dir.path(), &["init", "-q", "-b", "main"])?;
        git_text(
            dir.path(),
            &["config", "user.email", "admission-test@example.invalid"],
        )?;
        git_text(dir.path(), &["config", "user.name", "admission test"])?;
        Ok(Repo { dir })
    }

    /// Commits one file and returns the resulting commit oid.
    fn commit_file(repo: &Repo, path: &str, content: &str, message: &str) -> Result<String> {
        let target = repo.root().join(path);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(target, content)?;
        git(repo, &["add", path])?;
        git(repo, &["commit", "-q", "-m", message])?;
        Ok(git(repo, &["rev-parse", "HEAD"])?.trim().to_owned())
    }

    /// Divergent history: `main` advances past the branch point while
    /// `feature` carries the pull-request commits.
    fn divergent_commits(repo: &Repo) -> Result<(String, String)> {
        commit_file(repo, "src/a.rs", "base\n", "base")?;
        git(repo, &["switch", "-q", "-c", "feature"])?;
        let pr_head = commit_file(repo, "src/a.rs", "feature\n", "feature work")?;
        git(repo, &["switch", "-q", "main"])?;
        let base_tip = commit_file(repo, "src/main.txt", "main line\n", "main advance")?;
        Ok((base_tip, pr_head))
    }

    /// Builds a GitHub-style synthetic merge commit purely from plumbing:
    /// first parent is the base-side tip, second parent is the PR head.
    fn synthetic_merge(repo: &Repo, base_tip: &str, pr_head: &str) -> Result<String> {
        let raw = git(
            repo,
            &[
                "merge-tree",
                "--write-tree",
                "--name-only",
                base_tip,
                pr_head,
            ],
        )?;
        let tree = raw
            .lines()
            .next()
            .ok_or_else(|| anyhow::anyhow!("merge-tree produced no tree"))?
            .trim()
            .to_owned();
        git_text(
            repo.root(),
            &[
                "commit-tree",
                &tree,
                "-p",
                base_tip,
                "-p",
                pr_head,
                "-m",
                "synthetic PR merge",
            ],
        )
        .map(|s| s.trim().to_owned())
    }

    const SAMPLE_FILES: &[&str] = &["src/a.rs"];

    fn sample_patch() -> String {
        "@@ -1 +1 @@\n-a\n+b\n".to_owned()
    }

    fn files_vec() -> Vec<String> {
        SAMPLE_FILES.iter().map(|s| (*s).to_owned()).collect()
    }

    struct TrustedFixture {
        base_tree: String,
        head_sha: String,
        changed_files: tempfile::NamedTempFile,
        diff_patch: tempfile::NamedTempFile,
    }

    impl TrustedFixture {
        fn inputs(&self) -> TrustedDiffInputs {
            TrustedDiffInputs {
                base_tree: self.base_tree.clone(),
                head_sha: self.head_sha.clone(),
                changed_files: self.changed_files.path().to_path_buf(),
                diff_patch: self.diff_patch.path().to_path_buf(),
                pr_head_sha: Some(self.head_sha.clone()),
            }
        }
    }

    fn trusted_fixture(repo: &Repo) -> Result<TrustedFixture> {
        fs::create_dir_all(repo.root().join("scripts"))?;
        fs::create_dir_all(repo.root().join("src"))?;
        fs::write(
            repo.root().join(".ub-review.toml"),
            "review_profile = \"safe\"\n",
        )?;
        fs::write(repo.root().join("scripts/reviewer.sh"), "echo safe\n")?;
        fs::write(repo.root().join("src/a.rs"), "pub fn value() -> u8 { 1 }\n")?;
        git(
            repo,
            &["add", ".ub-review.toml", "scripts/reviewer.sh", "src/a.rs"],
        )?;
        git(repo, &["commit", "-q", "-m", "trusted base"])?;
        let base_tree = git(repo, &["show", "-s", "--format=%T", "HEAD"])?
            .trim()
            .to_owned();

        fs::write(
            repo.root().join(".ub-review.toml"),
            "this is hostile candidate config, not TOML\n",
        )?;
        fs::write(
            repo.root().join("scripts/reviewer.sh"),
            "echo candidate-script-must-not-run\n",
        )?;
        fs::write(repo.root().join("src/a.rs"), "pub fn value() -> u8 { 2 }\n")?;
        git(
            repo,
            &["add", ".ub-review.toml", "scripts/reviewer.sh", "src/a.rs"],
        )?;
        let patch = git(
            repo,
            &["diff", "--cached", "--binary", "--no-renames", "HEAD"],
        )?;
        let changed = git(
            repo,
            &["diff", "--cached", "--name-only", "--no-renames", "HEAD"],
        )?;
        git(repo, &["reset", "--hard", "-q", "HEAD"])?;

        let mut changed_files = tempfile::NamedTempFile::new()?;
        std::io::Write::write_all(&mut changed_files, changed.as_bytes())?;
        let mut diff_patch = tempfile::NamedTempFile::new()?;
        std::io::Write::write_all(&mut diff_patch, patch.as_bytes())?;
        Ok(TrustedFixture {
            base_tree,
            head_sha: "f".repeat(40),
            changed_files,
            diff_patch,
        })
    }

    #[test]
    fn trusted_diff_admits_unresolved_head_without_loading_candidate_surfaces() -> Result<()> {
        let repo = init_repo()?;
        let fixture = trusted_fixture(&repo)?;
        let inputs = fixture.inputs();

        let (diff, admission) = admit_trusted_diff(repo.root(), &inputs)?;

        assert_eq!(diff.base, fixture.base_tree);
        assert_eq!(diff.head, fixture.head_sha);
        assert_eq!(
            diff.changed_files,
            vec![
                ".ub-review.toml".to_owned(),
                "scripts/reviewer.sh".to_owned(),
                "src/a.rs".to_owned()
            ]
        );
        assert!(diff.patch.contains("candidate-script-must-not-run"));
        assert_eq!(admission.reviewed_commit_oid, fixture.head_sha);
        assert_eq!(admission.semantics, "candidate_head");
        assert!(admission.identity_canonical.contains(&fixture.base_tree));
        assert!(admission.identity_canonical.contains(&fixture.head_sha));
        assert_eq!(
            fs::read_to_string(repo.root().join(".ub-review.toml"))?,
            "review_profile = \"safe\"\n"
        );
        assert_eq!(
            fs::read_to_string(repo.root().join("scripts/reviewer.sh"))?,
            "echo safe\n"
        );
        let head_tree = admission
            .identity_canonical
            .lines()
            .find_map(|line| {
                line.strip_prefix("head=")
                    .and_then(|pair| pair.split_once(' '))
            })
            .map(|(_, tree)| tree)
            .ok_or_else(|| anyhow::anyhow!("admission identity omitted head tree"))?;
        let object_status = ProcessCommand::new("git")
            .arg("-C")
            .arg(repo.root())
            .args(["cat-file", "-e", &format!("{head_tree}^{{tree}}")])
            .output()?;
        assert!(
            !object_status.status.success(),
            "derived candidate tree must remain outside the repository object database"
        );
        Ok(())
    }

    #[test]
    fn trusted_diff_rejects_changed_paths_that_do_not_match_patch() -> Result<()> {
        let repo = init_repo()?;
        let fixture = trusted_fixture(&repo)?;
        fs::write(fixture.changed_files.path(), "src/a.rs\n")?;

        let Err(error) = admit_trusted_diff(repo.root(), &fixture.inputs()) else {
            bail!("mismatched changed-path object must fail admission");
        };
        assert!(
            error
                .to_string()
                .contains("does not match the patch tree delta"),
            "{error}"
        );
        Ok(())
    }

    #[test]
    fn trusted_diff_rejects_dirty_or_wrong_base_roots() -> Result<()> {
        let repo = init_repo()?;
        let fixture = trusted_fixture(&repo)?;
        let mut wrong_tree = fixture.inputs();
        wrong_tree.base_tree = "e".repeat(40);
        let Err(error) = admit_trusted_diff(repo.root(), &wrong_tree) else {
            bail!("wrong trusted base tree must fail admission");
        };
        assert!(
            error
                .to_string()
                .contains("does not match the clean root tree")
        );

        fs::write(repo.root().join("src/a.rs"), "dirty\n")?;
        let Err(error) = admit_trusted_diff(repo.root(), &fixture.inputs()) else {
            bail!("dirty root must fail trusted-base admission");
        };
        assert!(
            error
                .to_string()
                .contains("requires a clean base-owned root")
        );
        Ok(())
    }

    #[test]
    fn trusted_diff_rejects_invalid_patch_and_unsafe_paths() -> Result<()> {
        let repo = init_repo()?;
        let fixture = trusted_fixture(&repo)?;
        fs::write(fixture.diff_patch.path(), "not a patch\n")?;
        let Err(error) = admit_trusted_diff(repo.root(), &fixture.inputs()) else {
            bail!("malformed patch must fail admission");
        };
        assert!(error.to_string().contains("trusted-diff git"), "{error}");

        let repo = init_repo()?;
        let fixture = trusted_fixture(&repo)?;
        fs::write(fixture.changed_files.path(), "../outside\n")?;
        let Err(error) = admit_trusted_diff(repo.root(), &fixture.inputs()) else {
            bail!("unsafe changed path must fail admission");
        };
        let message = format!("{error:#}");
        assert!(message.contains("forbidden component"), "{message}");
        Ok(())
    }

    #[test]
    fn trusted_diff_rejects_invalid_identity_and_input_objects() -> Result<()> {
        let repo = init_repo()?;
        let fixture = trusted_fixture(&repo)?;

        let mut invalid_head = fixture.inputs();
        invalid_head.head_sha = "not-a-sha".to_owned();
        invalid_head.pr_head_sha = None;
        let Err(error) = admit_trusted_diff(repo.root(), &invalid_head) else {
            bail!("invalid trusted head SHA must fail admission");
        };
        assert!(
            error.to_string().contains("malformed git object id"),
            "{error}"
        );

        let mut mismatched_pr_head = fixture.inputs();
        mismatched_pr_head.pr_head_sha = Some("e".repeat(40));
        let Err(error) = admit_trusted_diff(repo.root(), &mismatched_pr_head) else {
            bail!("mismatched pull-request head must fail admission");
        };
        assert!(error.to_string().contains("does not match"), "{error}");

        let mut missing_changed_files = fixture.inputs();
        missing_changed_files.changed_files = repo.root().join("missing-changed-files.txt");
        let Err(error) = admit_trusted_diff(repo.root(), &missing_changed_files) else {
            bail!("missing changed-path object must fail admission");
        };
        assert!(format!("{error:#}").contains("open trusted changed-path object"));

        let original_changed_files = fs::read(fixture.changed_files.path())?;
        fs::write(fixture.changed_files.path(), "src/a.rs\nsrc/a.rs\n")?;
        let Err(error) = admit_trusted_diff(repo.root(), &fixture.inputs()) else {
            bail!("duplicate changed paths must fail admission");
        };
        assert!(error.to_string().contains("duplicate-free"), "{error}");

        fs::write(fixture.changed_files.path(), original_changed_files)?;
        fs::write(fixture.diff_patch.path(), [])?;
        let Err(error) = admit_trusted_diff(repo.root(), &fixture.inputs()) else {
            bail!("empty diff patch must fail admission");
        };
        assert!(error.to_string().contains("must not be empty"), "{error}");
        Ok(())
    }

    #[test]
    fn trusted_input_reader_rejects_oversize_and_non_utf8_objects() -> Result<()> {
        let mut input = tempfile::NamedTempFile::new()?;
        std::io::Write::write_all(&mut input, b"12345")?;
        let Err(error) = read_bounded_utf8(input.path(), 4, "fixture") else {
            bail!("oversize trusted input must fail closed");
        };
        assert!(error.to_string().contains("exceeds the 4-byte"));

        std::io::Write::write_all(&mut input, &[0xff])?;
        let Err(error) = read_bounded_utf8(input.path(), 16, "fixture") else {
            bail!("non-UTF-8 trusted input must fail closed");
        };
        assert!(error.to_string().contains("valid UTF-8"));
        Ok(())
    }

    #[test]
    fn local_candidate_review_resolves_exact_objects() -> Result<()> {
        let repo = init_repo()?;
        let base = commit_file(&repo, "src/a.rs", "one\n", "base")?;
        let head = commit_file(&repo, "src/a.rs", "two\n", "head")?;

        let admission = admit_revision(
            repo.root(),
            "HEAD~1",
            "HEAD",
            None,
            &files_vec(),
            &sample_patch(),
        )?;
        assert_eq!(admission.semantics, "candidate_head");
        assert_eq!(admission.pr_head_commit.as_deref(), Some(head.as_str()));
        assert!(!admission.worktree_dirty);
        assert!(admission.identity_canonical.contains(&base));
        assert!(
            admission
                .identity_canonical
                .contains("semantics=candidate_head")
        );
        admission.validate()?;
        Ok(())
    }

    #[test]
    fn symbolic_aliases_admit_one_identity() -> Result<()> {
        let repo = init_repo()?;
        commit_file(&repo, "src/a.rs", "one\n", "base")?;
        let head = commit_file(&repo, "src/a.rs", "two\n", "head")?;
        git(&repo, &["tag", "review-tip", "HEAD"])?;

        let by_range = admit_revision(
            repo.root(),
            "HEAD~1",
            "HEAD",
            None,
            &files_vec(),
            &sample_patch(),
        )?;
        let by_tag = admit_revision(
            repo.root(),
            &format!("{head}~1"),
            "review-tip",
            None,
            &files_vec(),
            &sample_patch(),
        )?;
        let by_sha = admit_revision(
            repo.root(),
            &format!("{head}^"),
            &head,
            None,
            &files_vec(),
            &sample_patch(),
        )?;
        assert_eq!(by_range.identity_digest, by_tag.identity_digest);
        assert_eq!(by_range.identity_digest, by_sha.identity_digest);
        assert!(by_sha.identity_canonical.contains(&head));
        Ok(())
    }

    #[test]
    fn synthetic_merge_records_head_and_merge_distinctly() -> Result<()> {
        let repo = init_repo()?;
        let (base_tip, pr_head) = divergent_commits(&repo)?;
        let merge = synthetic_merge(&repo, &base_tip, &pr_head)?;
        let parents = commit_parents(repo.root(), &merge)?;
        assert_eq!(parents.len(), 2, "fixture must be a two-parent merge");
        assert_eq!(parents[1], pr_head, "head-side parent must be second");

        let admission = admit_revision(
            repo.root(),
            "main",
            &merge,
            Some(&pr_head),
            &files_vec(),
            &sample_patch(),
        )?;
        assert_eq!(admission.semantics, "merge_result");
        assert_eq!(admission.pr_head_commit.as_deref(), Some(pr_head.as_str()));
        assert!(admission.identity_canonical.contains(&merge));
        assert!(
            admission
                .identity_canonical
                .contains("semantics=merge_result")
        );

        // The same review admitted as a plain candidate head would be a
        // different identity even with identical digests.
        let candidate_view = admit_revision(
            repo.root(),
            "main",
            &pr_head,
            Some(&pr_head),
            &files_vec(),
            &sample_patch(),
        )?;
        assert_eq!(candidate_view.semantics, "candidate_head");
        assert_ne!(candidate_view.identity_digest, admission.identity_digest);

        // Canonical form re-parses and validates on read paths.
        admission.validate()?;
        Ok(())
    }

    #[test]
    fn merge_without_head_metadata_is_an_explicit_failure() -> Result<()> {
        let repo = init_repo()?;
        let (base_tip, pr_head) = divergent_commits(&repo)?;
        let merge = synthetic_merge(&repo, &base_tip, &pr_head)?;

        let Err(err) = admit_revision(
            repo.root(),
            "main",
            &merge,
            None,
            &files_vec(),
            &sample_patch(),
        ) else {
            bail!("missing-metadata merge must not be admitted");
        };
        let message = err.to_string();
        assert!(
            message.contains("no pull-request head metadata"),
            "{message}"
        );
        assert!(message.contains("refusing to relabel"), "{message}");
        Ok(())
    }

    #[test]
    fn stale_or_forged_head_metadata_is_rejected_on_merges() -> Result<()> {
        let repo = init_repo()?;
        let (base_tip, pr_head) = divergent_commits(&repo)?;
        let merge = synthetic_merge(&repo, &base_tip, &pr_head)?;

        // The base-side tip is a parent but not the head-side parent.
        let Err(err) = admit_revision(
            repo.root(),
            "main",
            &merge,
            Some(base_tip.trim()),
            &files_vec(),
            &sample_patch(),
        ) else {
            bail!("base-side parent metadata must be rejected");
        };
        assert!(
            err.to_string().contains("not the head-side parent"),
            "{}",
            err
        );

        let unrelated = "f".repeat(40);
        let Err(err) = admit_revision(
            repo.root(),
            "main",
            &merge,
            Some(&unrelated),
            &files_vec(),
            &sample_patch(),
        ) else {
            bail!("non-parent head metadata must be rejected");
        };
        assert!(
            err.to_string().contains("not the head-side parent"),
            "{}",
            err
        );
        Ok(())
    }

    #[test]
    fn mismatched_candidate_metadata_is_rejected() -> Result<()> {
        let repo = init_repo()?;
        commit_file(&repo, "src/a.rs", "one\n", "base")?;
        let head = commit_file(&repo, "src/a.rs", "two\n", "head")?;
        let unrelated = "0".repeat(40);

        let Err(err) = admit_revision(
            repo.root(),
            "HEAD~1",
            "HEAD",
            Some(&unrelated),
            &files_vec(),
            &sample_patch(),
        ) else {
            bail!("mismatched candidate metadata must be rejected");
        };
        let message = err.to_string();
        assert!(message.contains("stale metadata"), "{message}");
        assert!(message.contains(&head), "{message}");
        Ok(())
    }

    #[test]
    fn dirty_worktrees_are_typed_not_rejected() -> Result<()> {
        let repo = init_repo()?;
        commit_file(&repo, "src/a.rs", "one\n", "base")?;
        let head = commit_file(&repo, "src/a.rs", "two\n", "head")?;

        let clean = admit_revision(
            repo.root(),
            "HEAD~1",
            "HEAD",
            None,
            &files_vec(),
            &sample_patch(),
        )?;
        assert!(!clean.worktree_dirty);

        std::fs::write(repo.root().join("src/a.rs"), "uncommitted\n")?;
        let dirty = admit_revision(
            repo.root(),
            "HEAD~1",
            "HEAD",
            None,
            &files_vec(),
            &sample_patch(),
        )?;
        assert!(dirty.worktree_dirty);
        // Dirt does not move the committed identity.
        assert_eq!(clean.identity_digest, dirty.identity_digest);
        assert_eq!(clean.pr_head_commit.as_deref(), Some(head.as_str()));
        Ok(())
    }

    #[test]
    fn candidate_and_merge_packets_stay_distinct_end_to_end() -> Result<()> {
        let repo = init_repo()?;
        let (base_tip, pr_head) = divergent_commits(&repo)?;
        let merge = synthetic_merge(&repo, &base_tip, &pr_head)?;
        let files = files_vec();
        let patch = sample_patch();

        let candidate = admit_revision(
            repo.root(),
            "main",
            &pr_head,
            Some(&pr_head),
            &files,
            &patch,
        )?;
        let merge_result =
            admit_revision(repo.root(), "main", &merge, Some(&pr_head), &files, &patch)?;
        let candidate_ref = crate::RevisionRef::from_admission(&candidate);
        let merge_ref = crate::RevisionRef::from_admission(&merge_result);

        // Distinct semantics survive into the join keys.
        assert_eq!(candidate_ref.semantics, "candidate_head");
        assert_eq!(merge_ref.semantics, "merge_result");
        assert_ne!(candidate_ref.digest, merge_ref.digest);
        candidate_ref.validate()?;
        merge_ref.validate()?;

        // The same reviewed content under different semantics produces two
        // deterministic, reproducible packet identities.
        let again = admit_revision(repo.root(), "main", &merge, Some(&pr_head), &files, &patch)?;
        assert_eq!(
            crate::RevisionRef::from_admission(&again).digest,
            merge_ref.digest
        );
        Ok(())
    }

    #[test]
    fn revision_ref_joins_admission_and_validates_shape() -> Result<()> {
        let repo = init_repo()?;
        let (base_tip, pr_head) = divergent_commits(&repo)?;
        let merge = synthetic_merge(&repo, &base_tip, &pr_head)?;
        let admission = admit_revision(
            repo.root(),
            "main",
            &merge,
            Some(&pr_head),
            &files_vec(),
            &sample_patch(),
        )?;

        let r = crate::RevisionRef::from_admission(&admission);
        assert_eq!(r.semantics, "merge_result");
        assert_eq!(r.reviewed_commit, merge);
        r.validate()?;

        // A tampered digest is visible: shape validation rejects it.
        let mut forged = r.clone();
        forged.digest = "z".repeat(64);
        assert!(forged.validate().is_err());

        // A different revision produces a different join key.
        let other = admit_revision(
            repo.root(),
            "main",
            &pr_head,
            Some(&pr_head),
            &files_vec(),
            &sample_patch(),
        )?;
        assert_ne!(crate::RevisionRef::from_admission(&other).digest, r.digest);
        Ok(())
    }

    #[test]
    fn digests_in_the_canonical_form_bind_the_admitted_inputs() -> Result<()> {
        let repo = init_repo()?;
        commit_file(&repo, "src/a.rs", "one\n", "base")?;
        commit_file(&repo, "src/a.rs", "two\n", "head")?;

        let files = files_vec();
        let patch = sample_patch();
        let admission = admit_revision(repo.root(), "HEAD~1", "HEAD", None, &files, &patch)?;
        assert!(
            admission.identity_canonical.contains(&format!(
                "changed_paths={}",
                RevisionIdentity::changed_paths_digest(&files)
            )),
            "{}",
            admission.identity_canonical
        );
        assert!(
            admission.identity_canonical.contains(&format!(
                "diff={}",
                RevisionIdentity::diff_digest(patch.as_bytes())
            )),
            "{}",
            admission.identity_canonical
        );

        // A byte change to the diff must move the stored identity digest.
        let mutated = admit_revision(repo.root(), "HEAD~1", "HEAD", None, &files, "-a\n+c\n")?;
        assert_ne!(admission.identity_digest, mutated.identity_digest);
        Ok(())
    }
}
