//! Pure immutable revision identity (#948, roadmap A1.1).
//!
//! This module defines the value contract only: validation, canonical
//! serialization, and a versioned, domain-separated digest. It performs no
//! Git subprocess work, touches no artifacts, and carries no scheduling
//! behavior. Symbolic ref labels are deliberately absent: two admissions
//! that point at the same objects share one identity regardless of the
//! human-readable names used to reach them.

use anyhow::{Result, bail};
use sha2::{Digest as ShaDigest, Sha256};

/// Domain separator for object identifiers accepted by this contract.
const OID_DOMAIN_NOTE: &str = "git object id (lowercase hex, sha1 or sha256 width)";

/// Semantic posture of a review pass over an immutable revision.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ReviewSemantics {
    /// The pull-request head itself was reviewed.
    CandidateHead,
    /// A synthetic merge of the head into its base was reviewed.
    MergeResult,
}

impl ReviewSemantics {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ReviewSemantics::CandidateHead => "candidate_head",
            ReviewSemantics::MergeResult => "merge_result",
        }
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "candidate_head" => Ok(ReviewSemantics::CandidateHead),
            "merge_result" => Ok(ReviewSemantics::MergeResult),
            other => bail!("unknown review semantics `{other}`"),
        }
    }
}

/// One validated commit/tree pair.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommitTree {
    commit: Oid,
    tree: Oid,
}

impl CommitTree {
    /// Validated commit object id.
    pub(crate) fn commit_oid(&self) -> &str {
        self.commit.as_str()
    }

    /// Validates and constructs one commit/tree side of an identity.
    pub(crate) fn new(label: &str, commit: &str, tree: &str) -> Result<Self> {
        let commit = Oid::parse(commit).map_err(|e| anyhow::anyhow!("{label} commit: {e}"))?;
        if commit.is_null() {
            bail!("{label} commit: null object id cannot identify a revision side");
        }
        let tree = Oid::parse(tree).map_err(|e| anyhow::anyhow!("{label} tree: {e}"))?;
        if tree.is_null() {
            bail!("{label} tree: null object id cannot identify a revision side");
        }
        Ok(CommitTree { commit, tree })
    }
}

/// A lowercase hexadecimal git object identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Oid {
    id: String,
}

impl Oid {
    fn parse(value: &str) -> Result<Self> {
        let bytes = value.as_bytes();
        if bytes.len() != 40 && bytes.len() != 64 {
            bail!(
                "malformed {OID_DOMAIN_NOTE}: expected 40 or 64 chars, got {}",
                bytes.len()
            );
        }
        for &b in bytes {
            if !(b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                bail!("malformed {OID_DOMAIN_NOTE}: `{value}` is not lowercase hex");
            }
        }
        Ok(Oid {
            id: value.to_owned(),
        })
    }

    fn as_str(&self) -> &str {
        &self.id
    }

    /// Rejects the git null object id, which names no real object.
    fn is_null(&self) -> bool {
        self.id.bytes().all(|b| b == b'0')
    }
}

/// Immutable identity of the revision a review pass covered.
///
/// Every field participates in the canonical form and therefore in the
/// identity digest. Ref labels, remote names, and PR numbers are excluded by
/// construction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RevisionIdentity {
    base: CommitTree,
    head: CommitTree,
    reviewed: CommitTree,
    merge: Option<CommitTree>,
    semantics: ReviewSemantics,
    changed_paths: String,
    diff: String,
}

/// Canonical-form version bumped on any breaking layout change.
const CANONICAL_VERSION: &str = "ub-review.revision-identity.v1";

/// Domain separator hashed ahead of the canonical form.
const DIGEST_DOMAIN: &str = "ub-review.revision-identity.digest.v1";

/// Domain separator for the changed-path set digest.
const CHANGED_PATHS_DOMAIN: &str = "ub-review.revision-identity.changed-paths.v1";

/// Domain separator for the diff-bytes digest.
const DIFF_DIGEST_DOMAIN: &str = "ub-review.revision-identity.diff.v1";

impl RevisionIdentity {
    /// Validates and constructs an identity from resolved object ids.
    ///
    /// All object ids must share one width (all sha1 or all sha256): mixing
    /// repositories' identifier widths would silently compare unrelated
    /// histories. For `candidate_head` the reviewed pair must equal the head
    /// pair; for `merge_result` the synthetic merge pair is required and the
    /// reviewed pair must equal it.
    pub(crate) fn new(
        base: CommitTree,
        head: CommitTree,
        reviewed: CommitTree,
        merge: Option<CommitTree>,
        semantics: ReviewSemantics,
        changed_paths_digest: &str,
        diff_digest: &str,
    ) -> Result<Self> {
        let changed_paths = validate_sha256(changed_paths_digest, "changed-paths digest")?;
        let diff = validate_sha256(diff_digest, "diff digest")?;

        let mut oids: Vec<&Oid> = vec![
            &base.commit,
            &base.tree,
            &head.commit,
            &head.tree,
            &reviewed.commit,
            &reviewed.tree,
        ];
        if let Some(m) = &merge {
            oids.push(&m.commit);
            oids.push(&m.tree);
        }
        let mut widths = oids.iter().map(|o| o.id.len()).collect::<Vec<_>>();
        widths.sort_unstable();
        widths.dedup();
        if widths.len() != 1 {
            bail!("mixed object-id widths in one identity: {widths:?}");
        }
        if oids.iter().any(|o| o.is_null()) {
            bail!("null object id cannot identify a revision side");
        }

        match semantics {
            ReviewSemantics::CandidateHead => {
                if merge.is_some() {
                    bail!(
                        "contradictory semantics: candidate_head identity carries a synthetic merge"
                    );
                }
                if reviewed != head {
                    bail!(
                        "contradictory semantics: candidate_head must review the pr-head exactly"
                    );
                }
            }
            ReviewSemantics::MergeResult => {
                let Some(m) = &merge else {
                    bail!(
                        "contradictory semantics: merge_result identity requires a synthetic merge"
                    );
                };
                if reviewed != *m {
                    bail!(
                        "contradictory semantics: merge_result must review the synthetic merge exactly"
                    );
                }
            }
        }

        Ok(RevisionIdentity {
            base,
            head,
            reviewed,
            merge,
            semantics,
            changed_paths: changed_paths.to_owned(),
            diff: diff.to_owned(),
        })
    }

    /// Stable, order-independent digest over a changed-path set.
    ///
    /// Paths are sorted, de-duplicated, newline-joined, and hashed behind a
    /// domain separator, so admission order can never move the digest.
    pub(crate) fn changed_paths_digest<I, S>(paths: I) -> String
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut sorted: Vec<String> = paths.into_iter().map(|p| p.as_ref().to_owned()).collect();
        sorted.sort_unstable();
        sorted.dedup();
        let mut hasher = Sha256::new();
        hasher.update(CHANGED_PATHS_DOMAIN);
        hasher.update([0u8]);
        for path in sorted {
            hasher.update(path.as_bytes());
            hasher.update([0u8]);
        }
        hex(&hasher.finalize())
    }

    /// Domain-separated digest over exact diff bytes.
    pub(crate) fn diff_digest(diff_bytes: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(DIFF_DIGEST_DOMAIN);
        hasher.update([0u8]);
        hasher.update(diff_bytes);
        hex(&hasher.finalize())
    }

    /// Deterministic canonical serialization of this identity.
    pub(crate) fn canonical_form(&self) -> String {
        let mut out = String::with_capacity(320);
        out.push_str(CANONICAL_VERSION);
        out.push('\n');
        out.push_str("semantics=");
        out.push_str(self.semantics.as_str());
        out.push('\n');
        push_pair(&mut out, "base", &self.base);
        push_pair(&mut out, "head", &self.head);
        push_pair(&mut out, "reviewed", &self.reviewed);
        if let Some(m) = &self.merge {
            push_pair(&mut out, "merge", m);
        } else {
            out.push_str("merge=-\n");
        }
        out.push_str("changed_paths=");
        out.push_str(&self.changed_paths);
        out.push('\n');
        out.push_str("diff=");
        out.push_str(&self.diff);
        out.push('\n');
        out
    }

    /// Parses and fully re-validates a canonical serialization.
    ///
    /// Tolerates CRLF line endings from Windows tooling; canonical fields
    /// themselves can never contain carriage returns.
    pub(crate) fn from_canonical(text: &str) -> Result<Self> {
        let normalized = text.replace('\r', "");
        let body = normalized.strip_suffix('\n').unwrap_or(&normalized);
        let mut lines = body.split('\n');
        let version = lines
            .next()
            .ok_or_else(|| anyhow::anyhow!("empty canonical revision identity"))?;
        if version != CANONICAL_VERSION {
            bail!("unsupported canonical revision identity version `{version}`");
        }
        let mut semantics: Option<ReviewSemantics> = None;
        let mut base: Option<CommitTree> = None;
        let mut head: Option<CommitTree> = None;
        let mut reviewed: Option<CommitTree> = None;
        let mut merge_seen = false;
        let mut merge: Option<CommitTree> = None;
        let mut changed_paths: Option<String> = None;
        let mut diff: Option<String> = None;
        for line in lines {
            let (key, value) = split_field(line)?;
            match key {
                "semantics" => {
                    reject_duplicate(semantics.is_some(), "semantics")?;
                    semantics = Some(ReviewSemantics::parse(value)?);
                }
                "base" => {
                    reject_duplicate(base.is_some(), "base")?;
                    let (c, t) = pair_parts(value)?;
                    base = Some(CommitTree::new("base", c, t)?);
                }
                "head" => {
                    reject_duplicate(head.is_some(), "head")?;
                    let (c, t) = pair_parts(value)?;
                    head = Some(CommitTree::new("head", c, t)?);
                }
                "reviewed" => {
                    reject_duplicate(reviewed.is_some(), "reviewed")?;
                    let (c, t) = pair_parts(value)?;
                    reviewed = Some(CommitTree::new("reviewed", c, t)?);
                }
                "merge" => {
                    reject_duplicate(merge_seen, "merge")?;
                    merge_seen = true;
                    merge = if value == "-" {
                        None
                    } else {
                        let (c, t) = pair_parts(value)?;
                        Some(CommitTree::new("merge", c, t)?)
                    };
                }
                "changed_paths" => {
                    reject_duplicate(changed_paths.is_some(), "changed_paths")?;
                    changed_paths =
                        Some(validate_sha256(value, "changed-paths digest")?.to_owned());
                }
                "diff" => {
                    reject_duplicate(diff.is_some(), "diff")?;
                    diff = Some(validate_sha256(value, "diff digest")?.to_owned());
                }
                other => bail!("unknown canonical field `{other}`"),
            }
        }
        let semantics =
            semantics.ok_or_else(|| anyhow::anyhow!("canonical identity missing semantics"))?;
        let base = base.ok_or_else(|| anyhow::anyhow!("canonical identity missing base"))?;
        let head = head.ok_or_else(|| anyhow::anyhow!("canonical identity missing head"))?;
        let reviewed =
            reviewed.ok_or_else(|| anyhow::anyhow!("canonical identity missing reviewed"))?;
        let changed_paths = changed_paths
            .ok_or_else(|| anyhow::anyhow!("canonical identity missing changed_paths"))?;
        let diff = diff.ok_or_else(|| anyhow::anyhow!("canonical identity missing diff"))?;
        Self::new(
            base,
            head,
            reviewed,
            merge,
            semantics,
            &changed_paths,
            &diff,
        )
    }

    /// Versioned, domain-separated digest over the canonical form.
    ///
    /// The digest covers exactly the fields above and nothing else: swapping
    /// any two fields' values changes it, and no ordering or environmental
    /// input can influence it.
    pub(crate) fn identity_digest(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(DIGEST_DOMAIN);
        hasher.update([0u8]);
        hasher.update(self.canonical_form().as_bytes());
        hex(&hasher.finalize())
    }
}

fn push_pair(out: &mut String, label: &str, pair: &CommitTree) {
    out.push_str(label);
    out.push('=');
    out.push_str(pair.commit.as_str());
    out.push(' ');
    out.push_str(pair.tree.as_str());
    out.push('\n');
}

fn pair_parts(value: &str) -> Result<(&str, &str)> {
    match value.split(' ').collect::<Vec<_>>().as_slice() {
        [commit, tree] => Ok((commit, tree)),
        _ => bail!("pair must be exactly `<commit> <tree>`"),
    }
}

fn split_field(line: &str) -> Result<(&str, &str)> {
    let (key, value) = line
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("canonical line lacks `=`: `{line}`"))?;
    Ok((key, value))
}

fn reject_duplicate(seen: bool, field: &str) -> Result<()> {
    if seen {
        bail!("duplicate canonical field `{field}`");
    }
    Ok(())
}

fn validate_sha256<'a>(value: &'a str, label: &str) -> Result<&'a str> {
    let bytes = value.as_bytes();
    if bytes.len() != 64 {
        bail!("{label}: expected 64 hex chars, got {}", bytes.len());
    }
    for &b in bytes {
        if !(b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
            bail!("{label}: `{value}` is not lowercase hex");
        }
    }
    Ok(value)
}

fn hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[usize::from(b >> 4)] as char);
        out.push(HEX[usize::from(b & 0x0f)] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic 40-char object id whose bytes vary by position.
    fn oid40(seed: u8) -> String {
        let bytes: Vec<u8> = (0..20)
            .map(|i| seed.wrapping_add((i as u8).wrapping_mul(7)))
            .collect();
        hex(&bytes)
    }

    /// Deterministic 64-char digest-shaped value.
    fn digest64(seed: u8) -> String {
        let bytes: Vec<u8> = (0..32)
            .map(|i| seed.wrapping_add((i as u8).wrapping_mul(11)))
            .collect();
        hex(&bytes)
    }

    fn candidate_identity() -> Result<RevisionIdentity> {
        let changed = RevisionIdentity::changed_paths_digest(["src/main.rs", "src/gate.rs"]);
        let diff = RevisionIdentity::diff_digest(b"sample unified diff");
        RevisionIdentity::new(
            CommitTree::new("base", &oid40(0x01), &oid40(0x02))?,
            CommitTree::new("head", &oid40(0x03), &oid40(0x04))?,
            CommitTree::new("reviewed", &oid40(0x03), &oid40(0x04))?,
            None,
            ReviewSemantics::CandidateHead,
            &changed,
            &diff,
        )
    }

    fn merge_identity() -> Result<RevisionIdentity> {
        let changed = RevisionIdentity::changed_paths_digest(["src/main.rs", "src/gate.rs"]);
        let diff = RevisionIdentity::diff_digest(b"sample unified diff");
        RevisionIdentity::new(
            CommitTree::new("base", &oid40(0x01), &oid40(0x02))?,
            CommitTree::new("head", &oid40(0x03), &oid40(0x04))?,
            CommitTree::new("reviewed", &oid40(0x05), &oid40(0x06))?,
            Some(CommitTree::new("merge", &oid40(0x05), &oid40(0x06))?),
            ReviewSemantics::MergeResult,
            &changed,
            &diff,
        )
    }

    #[test]
    fn valid_candidate_and_merge_identities_round_trip_and_stay_distinct() -> Result<()> {
        let candidate = candidate_identity()?;
        let merge = merge_identity()?;
        assert_eq!(
            RevisionIdentity::from_canonical(&candidate.canonical_form())?,
            candidate
        );
        assert_eq!(
            RevisionIdentity::from_canonical(&merge.canonical_form())?,
            merge
        );
        assert_ne!(candidate.canonical_form(), merge.canonical_form());
        assert_ne!(candidate.identity_digest(), merge.identity_digest());
        Ok(())
    }

    #[test]
    fn symbolic_labels_never_enter_serialization_or_digest() -> Result<()> {
        // Two admissions reach the same objects through different human
        // labels (`origin/main` vs `refs/remotes/origin/main`, a local
        // branch vs its SHA). The contract takes resolved ids only, so the
        // serialized identity cannot tell the routes apart.
        let via_branch = candidate_identity()?;
        let via_sha = candidate_identity()?;
        assert_eq!(via_branch.canonical_form(), via_sha.canonical_form());
        assert_eq!(via_branch.identity_digest(), via_sha.identity_digest());
        assert!(!via_branch.canonical_form().contains("origin"));
        Ok(())
    }

    /// Builds an identity from raw string parts, propagating validation.
    #[expect(
        clippy::too_many_arguments,
        reason = "the test table needs per-field raw strings so each malformed case isolates exactly one invalid field"
    )]
    fn raw_identity(
        base_commit: &str,
        base_tree: &str,
        head_commit: &str,
        head_tree: &str,
        reviewed_commit: &str,
        reviewed_tree: &str,
        merge: Option<(&str, &str)>,
        semantics: ReviewSemantics,
        changed_paths_digest: &str,
        diff_digest: &str,
    ) -> Result<RevisionIdentity> {
        let base = CommitTree::new("base", base_commit, base_tree)?;
        let head = CommitTree::new("head", head_commit, head_tree)?;
        let reviewed = CommitTree::new("reviewed", reviewed_commit, reviewed_tree)?;
        let merge = match merge {
            Some((c, t)) => Some(CommitTree::new("merge", c, t)?),
            None => None,
        };
        RevisionIdentity::new(
            base,
            head,
            reviewed,
            merge,
            semantics,
            changed_paths_digest,
            diff_digest,
        )
    }

    #[test]
    fn malformed_constructions_are_rejected() {
        // Every entry must be rejected with a diagnostic.
        let cases: Vec<(&str, Result<RevisionIdentity>)> = vec![
            (
                "short commit id",
                raw_identity(
                    "abc123",
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                ),
            ),
            (
                "uppercase commit id",
                raw_identity(
                    &oid40(0x01).to_uppercase(),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                ),
            ),
            (
                "null base commit",
                raw_identity(
                    &"0".repeat(40),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                ),
            ),
            (
                "mixed id widths across sides",
                raw_identity(
                    &digest64(0x30),
                    &digest64(0x31),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                ),
            ),
            (
                "candidate_head carrying a synthetic merge",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    Some((&oid40(0x05), &oid40(0x06))),
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                ),
            ),
            (
                "merge_result without a synthetic merge",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::MergeResult,
                    &digest64(0x10),
                    &digest64(0x20),
                ),
            ),
            (
                "candidate_head not reviewing the pr-head exactly",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x07),
                    &oid40(0x08),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                ),
            ),
            (
                "merge_result not reviewing the synthetic merge exactly",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x09),
                    &oid40(0x0a),
                    Some((&oid40(0x05), &oid40(0x06))),
                    ReviewSemantics::MergeResult,
                    &digest64(0x10),
                    &digest64(0x20),
                ),
            ),
            (
                "short changed-paths digest",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10)[..63],
                    &digest64(0x20),
                ),
            ),
            (
                "non-hex diff digest",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &"z".repeat(64),
                ),
            ),
        ];
        for (name, result) in cases {
            assert!(result.is_err(), "{name} was accepted");
        }
    }

    #[test]
    fn changed_path_ordering_and_duplicates_cannot_move_the_digest() {
        let forward = RevisionIdentity::changed_paths_digest(["src/a.rs", "src/b.rs", "src/c.rs"]);
        let shuffled = RevisionIdentity::changed_paths_digest(["src/c.rs", "src/a.rs", "src/b.rs"]);
        assert_eq!(forward, shuffled);

        let dup_first =
            RevisionIdentity::changed_paths_digest(["src/a.rs", "src/b.rs", "src/a.rs"]);
        let dup_second =
            RevisionIdentity::changed_paths_digest(["src/b.rs", "src/a.rs", "src/b.rs"]);
        assert_eq!(dup_first, dup_second);
    }

    #[test]
    fn diff_byte_changes_move_the_diff_digest() {
        let before = RevisionIdentity::diff_digest(b"@@ -1,1 +1,1 @@\n-a\n+b\n");
        let flipped_byte = RevisionIdentity::diff_digest(b"@@ -1,1 +1,1 @@\n-a\n+c\n");
        let appended_bytes = RevisionIdentity::diff_digest(b"@@ -1,1 +1,1 @@\n-a\n+b\n+ctx\n");
        assert_ne!(before, flipped_byte);
        assert_ne!(before, appended_bytes);
    }

    #[test]
    fn every_field_boundary_flips_the_identity_digest() -> Result<()> {
        let baseline = candidate_identity()?;
        let diff_a = RevisionIdentity::diff_digest(b"sample unified diff");
        let changed_a = RevisionIdentity::changed_paths_digest(["src/main.rs", "src/gate.rs"]);
        let changed_b = RevisionIdentity::changed_paths_digest(["src/main.rs", "src/other.rs"]);

        let mutations: Vec<(&str, Result<RevisionIdentity>)> = vec![
            (
                "base commit",
                raw_identity(
                    &oid40(0x41),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &changed_a,
                    &diff_a,
                ),
            ),
            (
                "base tree",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x42),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &changed_a,
                    &diff_a,
                ),
            ),
            (
                "head commit",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x43),
                    &oid40(0x04),
                    &oid40(0x43),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &changed_a,
                    &diff_a,
                ),
            ),
            (
                "head tree",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x44),
                    &oid40(0x03),
                    &oid40(0x44),
                    None,
                    ReviewSemantics::CandidateHead,
                    &changed_a,
                    &diff_a,
                ),
            ),
            ("semantics", merge_identity()),
            (
                "changed paths",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &changed_b,
                    &diff_a,
                ),
            ),
            (
                "diff bytes",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &changed_a,
                    &RevisionIdentity::diff_digest(b"mutated unified diff"),
                ),
            ),
        ];
        for (name, build) in mutations {
            assert_ne!(
                baseline.identity_digest(),
                build?.identity_digest(),
                "{name} mutation did not move the identity digest"
            );
        }
        Ok(())
    }

    #[test]
    fn canonical_tampering_is_rejected() -> Result<()> {
        let baseline = candidate_identity()?;
        let form = baseline.canonical_form();

        let unknown_key = form.replace("diff=", "signature=");
        assert!(RevisionIdentity::from_canonical(&unknown_key).is_err());

        let duplicate_line = format!("{form}base={}\n", oid40(0x99));
        assert!(duplicate_line.contains("base="));
        let mut duplicated_lines = form.clone();
        duplicated_lines.push_str(&format!("head={} {}\n", oid40(0x03), oid40(0x04)));
        assert!(RevisionIdentity::from_canonical(&duplicated_lines).is_err());

        let missing_reviewed = form
            .lines()
            .filter(|l| !l.starts_with("reviewed="))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        assert!(RevisionIdentity::from_canonical(&missing_reviewed).is_err());

        let wrong_version = form.replacen(
            "ub-review.revision-identity.v1",
            "ub-review.revision-identity.v9",
            1,
        );
        assert!(RevisionIdentity::from_canonical(&wrong_version).is_err());

        let truncated = form.lines().next().unwrap_or_default().to_owned();
        assert!(RevisionIdentity::from_canonical(&truncated).is_err());
        Ok(())
    }

    #[test]
    fn crlf_canonical_forms_still_parse() -> Result<()> {
        let baseline = candidate_identity()?;
        let crlf = baseline.canonical_form().replace('\n', "\r\n");
        assert_eq!(RevisionIdentity::from_canonical(&crlf)?, baseline);
        Ok(())
    }

    /// Extracts a rejection message without panic-family calls.
    trait ExpectErrMessage {
        fn expect_err_message(self) -> String;
    }

    impl<T> ExpectErrMessage for Result<T> {
        fn expect_err_message(self) -> String {
            match self {
                Err(err) => err.to_string(),
                Ok(_) => String::from("expected rejection but construction succeeded"),
            }
        }
    }

    #[test]
    fn contract_surface_pins_tokens_labels_and_rejection_messages() -> Result<()> {
        // Every semantics arm's canonical token is pinned exactly, so a
        // rename silently breaks the serialized contract instead of
        // drifting both sides together.
        assert_eq!(ReviewSemantics::CandidateHead.as_str(), "candidate_head");
        assert_eq!(ReviewSemantics::MergeResult.as_str(), "merge_result");
        assert_eq!(
            ReviewSemantics::parse("candidate_head")?,
            ReviewSemantics::CandidateHead
        );
        assert_eq!(
            ReviewSemantics::parse("merge_result")?,
            ReviewSemantics::MergeResult
        );
        let Err(unknown_semantics) = ReviewSemantics::parse("synthetic") else {
            bail!("unknown review semantics must be rejected");
        };
        assert!(
            unknown_semantics
                .to_string()
                .contains("unknown review semantics `synthetic`"),
            "{unknown_semantics}"
        );

        // Every validation arm's message is pinned so the rejection reason
        // stays diagnosable at the admission boundary.
        let rejections: Vec<(&str, String)> = vec![
            (
                "short commit id",
                raw_identity(
                    "abc123",
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                )
                .expect_err_message(),
            ),
            (
                "null base commit",
                raw_identity(
                    &"0".repeat(40),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                )
                .expect_err_message(),
            ),
            (
                "mixed object-id widths",
                raw_identity(
                    &digest64(0x30),
                    &digest64(0x31),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                )
                .expect_err_message(),
            ),
            (
                "candidate_head carrying a synthetic merge",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    Some((&oid40(0x05), &oid40(0x06))),
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                )
                .expect_err_message(),
            ),
            (
                "merge_result without a synthetic merge",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::MergeResult,
                    &digest64(0x10),
                    &digest64(0x20),
                )
                .expect_err_message(),
            ),
            (
                "candidate_head reviewing something else",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x07),
                    &oid40(0x08),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10),
                    &digest64(0x20),
                )
                .expect_err_message(),
            ),
            (
                "short changed-paths digest",
                raw_identity(
                    &oid40(0x01),
                    &oid40(0x02),
                    &oid40(0x03),
                    &oid40(0x04),
                    &oid40(0x03),
                    &oid40(0x04),
                    None,
                    ReviewSemantics::CandidateHead,
                    &digest64(0x10)[..63],
                    &digest64(0x20),
                )
                .expect_err_message(),
            ),
        ];
        let expected_fragments = [
            "expected 40 or 64 chars",
            "null object id cannot identify a revision side",
            "mixed object-id widths",
            "candidate_head identity carries a synthetic merge",
            "merge_result identity requires a synthetic merge",
            "candidate_head must review the pr-head exactly",
            "changed-paths digest: expected 64 hex chars",
        ];
        for ((name, message), fragment) in rejections.iter().zip(expected_fragments) {
            assert!(
                message.contains(fragment),
                "{name}: `{message}` lacks `{fragment}`"
            );
        }
        assert_eq!(rejections.len(), expected_fragments.len());

        // The canonical form pins every labeled field, the version line,
        // and both merge postures.
        let candidate = candidate_identity()?;
        let candidate_form = candidate.canonical_form();
        assert!(
            candidate_form.starts_with("ub-review.revision-identity.v1\n"),
            "{candidate_form}"
        );
        for label in [
            "semantics=candidate_head",
            "base=",
            "head=",
            "reviewed=",
            "merge=-",
            "changed_paths=",
            "diff=",
        ] {
            assert!(
                candidate_form.contains(label),
                "canonical form lacks `{label}`"
            );
        }
        let merge = merge_identity()?;
        let merge_form = merge.canonical_form();
        assert!(!merge_form.contains("merge=-"), "{merge_form}");
        assert!(merge_form.starts_with("ub-review.revision-identity.v1\n"));

        // The three digest flavors are domain-separated: identical input
        // bytes under different domains must not collide.
        let as_paths = RevisionIdentity::changed_paths_digest(["src/a.rs"]);
        let as_diff = RevisionIdentity::diff_digest(b"src/a.rs");
        assert_ne!(as_paths, as_diff);

        Ok(())
    }
}
