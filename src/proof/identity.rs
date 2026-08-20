//! Canonical identity for an already-approved proof execution.
//!
//! This module is deliberately pure.  It does not approve commands, run
//! workers, or decide which queue owns a task.  Callers must pass the argv and
//! environment produced by an existing proof-command approval boundary.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use super::{FocusedTestTask, proof_task_command_spec};
use crate::{DiffContext, sha256_hex};

pub(crate) const PROOF_EXECUTION_IDENTITY_SCHEMA: &str = "ub-review.proof_execution_identity.v1";

/// The approved fields which contribute to an execution identity.
///
/// `requester_ids`, claim IDs, lane IDs, and arrival order intentionally do
/// not appear here.  They are consumers of an execution, rather than part of
/// the execution itself.  `argv` and `env` must come from the existing
/// allowlist-backed command parser; this type is not an authorization API.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ApprovedProofExecution {
    pub(crate) mode: String,
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) argv: Vec<String>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) working_root: String,
    pub(crate) package: String,
    pub(crate) target: String,
    #[serde(default)]
    pub(crate) test_filter: Option<String>,
    #[serde(default)]
    pub(crate) features: Vec<String>,
    #[serde(default)]
    pub(crate) tool_versions: BTreeMap<String, String>,
}

/// Stable, reviewable identity for one approved proof execution.
///
/// The digest is SHA-256 over the canonical JSON representation of every
/// field except `digest` itself.  Consumers may serialize the full value as a
/// fixture, but the digest is not execution evidence and must not be treated
/// as a receipt.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct ProofExecutionIdentity {
    pub(crate) schema: String,
    pub(crate) mode: String,
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) argv: Vec<String>,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) working_root: String,
    pub(crate) package: String,
    pub(crate) target: String,
    #[serde(default)]
    pub(crate) test_filter: Option<String>,
    #[serde(default)]
    pub(crate) features: Vec<String>,
    #[serde(default)]
    pub(crate) tool_versions: BTreeMap<String, String>,
    pub(crate) digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum ProofSubsumption {
    Identical,
    AllowedNarrower,
    Distinct,
    Rejected(String),
}

impl ProofExecutionIdentity {
    /// Normalize fields from an existing command-approval boundary.
    pub(crate) fn from_approved(input: ApprovedProofExecution) -> Result<Self> {
        validate_approved_input(&input)?;
        let canonical = CanonicalIdentity {
            schema: PROOF_EXECUTION_IDENTITY_SCHEMA.to_owned(),
            mode: normalize_token(&input.mode, "mode")?,
            base: normalize_revision(&input.base, "base")?,
            head: normalize_revision(&input.head, "head")?,
            argv: normalize_argv(input.argv)?,
            env: normalize_env(input.env)?,
            working_root: normalize_root(&input.working_root)?,
            package: normalize_token(&input.package, "package")?,
            target: normalize_token(&input.target, "target")?,
            test_filter: input
                .test_filter
                .map(|value| normalize_test_filter(&value))
                .transpose()?,
            features: normalize_features(input.features)?,
            tool_versions: normalize_map(input.tool_versions, "tool version")?,
        };
        let digest = sha256_hex(&serde_json::to_vec(&canonical)?);
        Ok(Self {
            schema: canonical.schema,
            mode: canonical.mode,
            base: canonical.base,
            head: canonical.head,
            argv: canonical.argv,
            env: canonical.env,
            working_root: canonical.working_root,
            package: canonical.package,
            target: canonical.target,
            test_filter: canonical.test_filter,
            features: canonical.features,
            tool_versions: canonical.tool_versions,
            digest,
        })
    }

    /// Recompute and verify the digest before using an identity as a key.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema != PROOF_EXECUTION_IDENTITY_SCHEMA {
            bail!(
                "unsupported proof execution identity schema `{}`",
                self.schema
            );
        }
        let input = ApprovedProofExecution {
            mode: self.mode.clone(),
            base: self.base.clone(),
            head: self.head.clone(),
            argv: self.argv.clone(),
            env: self.env.clone(),
            working_root: self.working_root.clone(),
            package: self.package.clone(),
            target: self.target.clone(),
            test_filter: self.test_filter.clone(),
            features: self.features.clone(),
            tool_versions: self.tool_versions.clone(),
        };
        let expected = Self::from_approved(input)?;
        if expected.digest != self.digest || expected != *self {
            bail!("proof execution identity digest or canonical fields do not match");
        }
        Ok(())
    }

    pub(crate) fn subsumption(&self, candidate: &Self) -> ProofSubsumption {
        if self.validate().is_err() || candidate.validate().is_err() {
            return ProofSubsumption::Rejected(
                "invalid proof execution identity cannot be subsumed".to_owned(),
            );
        }
        if self == candidate {
            return ProofSubsumption::Identical;
        }
        // A head-only execution can answer a narrower request for the same
        // exact target only when the candidate explicitly carries a filter.
        // Red/green and differing revisions are never subsumed by this pure
        // foundation slice.
        if self.mode == "head-only"
            && candidate.mode == "head-only"
            && self.base == candidate.base
            && self.head == candidate.head
            && self.argv == candidate.argv
            && self.env == candidate.env
            && self.working_root == candidate.working_root
            && self.package == candidate.package
            && self.target == candidate.target
            && self.features == candidate.features
            && self.tool_versions == candidate.tool_versions
            && self.test_filter.is_none()
            && candidate.test_filter.is_some()
        {
            return ProofSubsumption::AllowedNarrower;
        }
        ProofSubsumption::Distinct
    }
}

/// Build an identity from the command specification already selected for a
/// focused task.  The task adapter supplies no new command authority: its
/// `ProofCommandSpec` is produced by the existing allowlist parser.
pub(crate) fn identity_for_focused_task(
    diff: &DiffContext,
    task: &FocusedTestTask,
    side: &str,
    working_root: &str,
) -> Result<ProofExecutionIdentity> {
    let spec = proof_task_command_spec(task, side);
    ProofExecutionIdentity::from_approved(ApprovedProofExecution {
        mode: format!("{}:{side}", task.mode.key()),
        base: diff.base.clone(),
        head: diff.head.clone(),
        argv: spec.argv,
        env: spec.env,
        working_root: working_root.to_owned(),
        package: task.file.clone(),
        // Keep the semantic target stable and token-safe. Human-readable test
        // names may contain spaces; they belong in `test_filter`, not in the
        // normalized target field.
        target: task.file.clone(),
        test_filter: task.test_name.clone(),
        features: Vec::new(),
        tool_versions: BTreeMap::new(),
    })
}

#[derive(Serialize)]
struct CanonicalIdentity {
    schema: String,
    mode: String,
    base: String,
    head: String,
    argv: Vec<String>,
    env: BTreeMap<String, String>,
    working_root: String,
    package: String,
    target: String,
    test_filter: Option<String>,
    features: Vec<String>,
    tool_versions: BTreeMap<String, String>,
}

fn validate_approved_input(input: &ApprovedProofExecution) -> Result<()> {
    if input.mode.trim().is_empty() {
        bail!("proof execution mode is required");
    }
    if input.argv.is_empty() {
        bail!("approved proof argv cannot be empty");
    }
    if input.working_root.trim().is_empty() {
        bail!("proof execution working root is required");
    }
    if !matches!(
        input.argv.first().map(String::as_str),
        Some("cargo" | "bun")
    ) {
        bail!("proof identity accepts only an approved cargo or bun executable");
    }
    Ok(())
}

fn normalize_argv(argv: Vec<String>) -> Result<Vec<String>> {
    argv.into_iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains('\0') || arg.chars().any(char::is_control) {
                bail!("approved proof argv contains an invalid argument");
            }
            // argv is already emitted by the allowlist-backed command parser.
            // Preserve every token byte-for-byte: a generic path normalizer
            // would turn an ordinary argument such as `b/` into an alias.
            Ok(arg)
        })
        .collect()
}

fn normalize_env(env: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    normalize_map(env, "environment")
}

fn normalize_map(
    values: BTreeMap<String, String>,
    label: &str,
) -> Result<BTreeMap<String, String>> {
    values
        .into_iter()
        .map(|(key, value)| {
            if key.is_empty()
                || key.contains('\0')
                || key.chars().any(char::is_control)
                || value.contains('\0')
                || value.chars().any(char::is_control)
            {
                bail!("invalid {label} entry");
            }
            Ok((key, value))
        })
        .collect()
}

fn normalize_features(mut features: Vec<String>) -> Result<Vec<String>> {
    for feature in &features {
        if feature.is_empty() || feature.contains('\0') || feature.chars().any(char::is_control) {
            bail!("invalid proof feature");
        }
    }
    features.sort();
    features.dedup();
    Ok(features)
}

fn normalize_root(root: &str) -> Result<String> {
    // Working roots are filesystem paths, not diff paths. Never apply the
    // diff-specific `b/` prefix removal here.
    let normalized = root.trim().replace('\\', "/");
    if normalized.is_empty()
        || normalized.contains('\0')
        || normalized.chars().any(char::is_control)
    {
        bail!("invalid proof working root");
    }
    // Do not turn filesystem roots into a different path: `C:/` must remain
    // `C:/`, and `/` must remain `/`. Other roots may drop redundant trailing
    // separators for stable identity bytes.
    let is_posix_root = normalized == "/";
    let is_drive_root = normalized.len() == 3
        && normalized.as_bytes().get(1) == Some(&b':')
        && normalized.ends_with('/');
    if is_posix_root || is_drive_root {
        Ok(normalized)
    } else {
        Ok(normalized.trim_end_matches('/').to_owned())
    }
}

fn normalize_revision(value: &str, label: &str) -> Result<String> {
    normalize_token(value, label)
}

fn normalize_test_filter(value: &str) -> Result<String> {
    let normalized = value.trim().to_owned();
    if normalized.is_empty()
        || normalized.contains('\0')
        || normalized.chars().any(char::is_control)
    {
        bail!("invalid proof test_filter");
    }
    Ok(normalized)
}

fn normalize_token(value: &str, label: &str) -> Result<String> {
    let normalized = value.trim().replace('\\', "/");
    if normalized.is_empty()
        || normalized.contains('\0')
        || normalized.chars().any(char::is_control)
        || normalized.chars().any(char::is_whitespace)
    {
        bail!("invalid proof {label}");
    }
    Ok(normalized)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proof::command::ProofCommandSpec;
    use crate::proof::tasks::FocusedTestCommandSpecs;
    use crate::{DiffClass, DiffFlags, FocusedProofMode};

    fn input() -> ApprovedProofExecution {
        ApprovedProofExecution {
            mode: "head-only".to_owned(),
            base: "base-sha".to_owned(),
            head: "head-sha".to_owned(),
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "--locked".to_owned(),
                "--package".to_owned(),
                "demo".to_owned(),
                "--test".to_owned(),
                "unit".to_owned(),
            ],
            env: BTreeMap::from([
                ("RUSTFLAGS".to_owned(), "-Dwarnings".to_owned()),
                ("CARGO_TERM_COLOR".to_owned(), "never".to_owned()),
            ]),
            working_root: "C:\\repo\\".to_owned(),
            package: "demo".to_owned(),
            target: "unit".to_owned(),
            test_filter: None,
            features: vec!["b".to_owned(), "a".to_owned(), "a".to_owned()],
            tool_versions: BTreeMap::from([("cargo".to_owned(), "1.95".to_owned())]),
        }
    }

    #[test]
    fn equivalent_environment_and_feature_order_has_stable_digest() -> Result<()> {
        let left = ProofExecutionIdentity::from_approved(input())?;
        let mut right_input = input();
        right_input.env = BTreeMap::from([
            ("CARGO_TERM_COLOR".to_owned(), "never".to_owned()),
            ("RUSTFLAGS".to_owned(), "-Dwarnings".to_owned()),
        ]);
        right_input.features = vec!["a".to_owned(), "b".to_owned()];
        right_input.working_root = "C:/repo".to_owned();
        let right = ProofExecutionIdentity::from_approved(right_input)?;
        assert_eq!(left, right);
        assert_eq!(left.subsumption(&right), ProofSubsumption::Identical);
        Ok(())
    }

    #[test]
    fn material_dimensions_are_distinct() -> Result<()> {
        let base = ProofExecutionIdentity::from_approved(input())?;
        for mutate in [
            |value: &mut ApprovedProofExecution| value.head = "other-head".to_owned(),
            |value: &mut ApprovedProofExecution| value.base = "other-base".to_owned(),
            |value: &mut ApprovedProofExecution| value.mode = "red-green".to_owned(),
            |value: &mut ApprovedProofExecution| value.package = "other".to_owned(),
            |value: &mut ApprovedProofExecution| value.target = "other".to_owned(),
            |value: &mut ApprovedProofExecution| value.working_root = "C:/other".to_owned(),
            |value: &mut ApprovedProofExecution| {
                value
                    .tool_versions
                    .insert("cargo".to_owned(), "1.96".to_owned());
            },
        ] {
            let mut changed = input();
            mutate(&mut changed);
            let identity = ProofExecutionIdentity::from_approved(changed)?;
            assert_eq!(base.subsumption(&identity), ProofSubsumption::Distinct);
        }
        Ok(())
    }

    #[test]
    fn forged_or_invalid_identity_is_rejected_closed() -> Result<()> {
        let mut identity = ProofExecutionIdentity::from_approved(input())?;
        identity.head = "forged-head".to_owned();
        assert!(identity.validate().is_err());
        let mut invalid = input();
        invalid.argv = vec!["sh".to_owned(), "-c".to_owned(), "cargo test".to_owned()];
        assert!(ProofExecutionIdentity::from_approved(invalid).is_err());
        Ok(())
    }

    #[test]
    fn argv_tokens_and_working_root_keep_path_domains_distinct() -> Result<()> {
        let mut first = input();
        first.argv.push("b/ordinary-argument".to_owned());
        first.working_root = "b/repo".to_owned();
        let mut second = first.clone();
        second.argv.pop();
        second.argv.push("ordinary-argument".to_owned());
        second.working_root = "repo".to_owned();
        let left = ProofExecutionIdentity::from_approved(first)?;
        let right = ProofExecutionIdentity::from_approved(second)?;
        assert_ne!(left.argv, right.argv);
        assert_ne!(left.working_root, right.working_root);
        assert_eq!(left.subsumption(&right), ProofSubsumption::Distinct);
        Ok(())
    }

    #[test]
    fn explicit_narrower_relation_requires_same_execution_dimensions() -> Result<()> {
        let broad = ProofExecutionIdentity::from_approved(input())?;
        let mut narrow_input = input();
        narrow_input.test_filter = Some("specific_test".to_owned());
        let narrow = ProofExecutionIdentity::from_approved(narrow_input)?;
        assert_eq!(
            broad.subsumption(&narrow),
            ProofSubsumption::AllowedNarrower
        );
        let mut red_green_input = input();
        red_green_input.mode = "red-green".to_owned();
        red_green_input.test_filter = Some("specific_test".to_owned());
        let red_green = ProofExecutionIdentity::from_approved(red_green_input)?;
        assert_eq!(broad.subsumption(&red_green), ProofSubsumption::Distinct);
        Ok(())
    }

    #[test]
    fn red_green_sides_have_distinct_command_identities() -> Result<()> {
        let diff = DiffContext {
            base: "base-sha".to_owned(),
            head: "head-sha".to_owned(),
            changed_files: vec!["src/lib.rs".to_owned()],
            patch: String::new(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::SourceGeneral,
        };
        let task = FocusedTestTask {
            id: "identity-red-green".to_owned(),
            file: "cargo-package:demo".to_owned(),
            test_name: Some("unit".to_owned()),
            mode: FocusedProofMode::RedGreen,
            command_specs: Some(FocusedTestCommandSpecs {
                head: ProofCommandSpec {
                    argv: vec![
                        "cargo".to_owned(),
                        "test".to_owned(),
                        "--features=head".to_owned(),
                    ],
                    env: BTreeMap::new(),
                },
                base_plus_tests: ProofCommandSpec {
                    argv: vec![
                        "cargo".to_owned(),
                        "test".to_owned(),
                        "--features=base".to_owned(),
                    ],
                    env: BTreeMap::new(),
                },
            }),
            timeout_sec: Some(30),
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids: vec!["request-identity".to_owned()],
        };
        let head = identity_for_focused_task(
            &diff,
            &task,
            "head",
            "target/ub-review/proof-worktrees/head",
        )?;
        let base = identity_for_focused_task(
            &diff,
            &task,
            "base-plus-tests",
            "target/ub-review/proof-worktrees/base-plus-tests",
        )?;
        assert_ne!(head.argv, base.argv);
        assert_ne!(head.digest, base.digest);
        assert_eq!(head.subsumption(&base), ProofSubsumption::Distinct);
        Ok(())
    }

    #[test]
    fn filesystem_roots_keep_root_semantics() -> Result<()> {
        let mut posix = input();
        posix.working_root = "/".to_owned();
        assert_eq!(
            ProofExecutionIdentity::from_approved(posix)?.working_root,
            "/"
        );

        let mut windows = input();
        windows.working_root = r"C:\".to_owned();
        assert_eq!(
            ProofExecutionIdentity::from_approved(windows)?.working_root,
            "C:/"
        );
        Ok(())
    }
}
