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
use std::path::Path;

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
    /// Exact base-side commit from the admitted identity.
    #[serde(default)]
    pub(crate) base_commit_oid: String,
    /// Exact pull-request head commit from the admitted identity.
    #[serde(default)]
    pub(crate) head_commit_oid: String,
    /// Exact git commit object the run reviewed (head or synthetic merge).
    #[serde(default)]
    pub(crate) reviewed_commit_oid: String,
    /// Resolved PR-head commit when hosted metadata was supplied.
    #[serde(default)]
    pub(crate) pr_head_commit: Option<String>,
    /// Typed evidence: the worktree had uncommitted or untracked entries at
    /// admission time. Dirt never changes commit objects, so it is recorded,
    /// not rejected.
    pub(crate) worktree_dirty: bool,
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

    let base_commit_oid = base.commit_oid().to_owned();
    let head_commit_oid = head.commit_oid().to_owned();
    let reviewed_commit_oid = reviewed.commit_oid().to_owned();
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
        base_commit_oid,
        head_commit_oid,
        reviewed_commit_oid,
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
        if self.semantics != parsed.semantics_key() {
            bail!("revision admission semantics do not match its canonical identity");
        }
        if self
            .pr_head_commit
            .as_deref()
            .is_some_and(|commit| commit != parsed.head_commit_oid())
        {
            bail!("revision admission pull-request head does not match its canonical identity");
        }
        let stored_objects = [
            self.base_commit_oid.as_str(),
            self.head_commit_oid.as_str(),
            self.reviewed_commit_oid.as_str(),
        ];
        if stored_objects.iter().all(|value| value.is_empty()) {
            return Ok(());
        }
        if self.base_commit_oid != parsed.base_commit_oid()
            || self.head_commit_oid != parsed.head_commit_oid()
            || self.reviewed_commit_oid != parsed.reviewed_commit_oid()
        {
            bail!("revision admission object fields do not match its canonical identity");
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
    fn legacy_admission_fields_cannot_override_canonical_semantics_or_pr_head() -> Result<()> {
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

        let mut legacy = admission.clone();
        legacy.base_commit_oid.clear();
        legacy.head_commit_oid.clear();
        legacy.reviewed_commit_oid.clear();
        legacy.validate()?;

        let mut wrong_semantics = legacy.clone();
        wrong_semantics.semantics = "candidate_head".to_owned();
        let Err(semantics_error) = wrong_semantics.validate() else {
            bail!("legacy compatibility cannot override canonical semantics");
        };
        assert!(
            semantics_error
                .to_string()
                .contains("semantics do not match"),
            "{semantics_error}"
        );

        let mut wrong_head = legacy;
        wrong_head.pr_head_commit = Some("f".repeat(40));
        let Err(head_error) = wrong_head.validate() else {
            bail!("legacy compatibility cannot override canonical PR head");
        };
        assert!(
            head_error
                .to_string()
                .contains("pull-request head does not match"),
            "{head_error}"
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
        assert_eq!(r.base_commit, base_tip);
        assert_eq!(r.head_commit, pr_head);
        assert_eq!(r.reviewed_commit, merge);
        assert_ne!(r.head_commit, r.reviewed_commit);
        r.validate()?;

        // Delivery authority cannot be replaced with the synthetic merge.
        let mut wrong_head = r.clone();
        wrong_head.head_commit = wrong_head.reviewed_commit.clone();
        assert!(wrong_head.validate().is_err());

        // A tampered digest is visible: shape validation rejects it.
        let mut forged = r.clone();
        forged.digest = "z".repeat(64);
        assert!(forged.validate().is_err());

        // A candidate-head revision uses the same immutable object for
        // head and reviewed authority, but has a different join key.
        let other = admit_revision(
            repo.root(),
            "main",
            &pr_head,
            Some(&pr_head),
            &files_vec(),
            &sample_patch(),
        )?;
        let candidate = crate::RevisionRef::from_admission(&other);
        assert_eq!(candidate.semantics, "candidate_head");
        assert_eq!(candidate.head_commit, candidate.reviewed_commit);
        candidate.validate()?;
        assert_ne!(candidate.digest, r.digest);
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
