//! Pure canonical identity for an already-approved proof execution.
//!
//! This module owns value normalization, canonical serialization, and digest
//! verification only.  It does not approve commands, select tasks, interact
//! with the planner, execute workers, or define subsumption semantics.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::sha256_hex;

pub(crate) const PROOF_EXECUTION_IDENTITY_SCHEMA: &str = "ub-review.proof_execution_identity.v1";

/// Values emitted by an existing proof-command approval boundary.
///
/// The type is intentionally data-only: constructing it does not authorize
/// execution.  Requesters, lanes, queue order, and receipts are not execution
/// dimensions and therefore are not represented here.
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

/// Stable identity for one approved proof execution.
///
/// `digest` is SHA-256 over canonical JSON containing every field except the
/// digest itself.  It is an identity key, not execution evidence or a receipt.
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

impl ProofExecutionIdentity {
    /// Canonicalize values from the existing approval boundary and derive the
    /// identity digest.  This constructor is deliberately not an approval API.
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
            tool_versions: normalize_tool_versions(input.tool_versions)?,
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

    /// Recompute every canonical field and digest before accepting an
    /// identity loaded from an artifact or other untrusted input.
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema != PROOF_EXECUTION_IDENTITY_SCHEMA {
            bail!(
                "unsupported proof execution identity schema `{}`",
                self.schema
            );
        }
        let expected = Self::from_approved(ApprovedProofExecution {
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
        })?;
        if expected != *self {
            bail!("proof execution identity digest or canonical fields do not match");
        }
        Ok(())
    }
}

// Keep this value-only foundation warning-free while its planner adapters are
// intentionally deferred to a follow-up lane.
const _: fn(ApprovedProofExecution) -> Result<ProofExecutionIdentity> =
    ProofExecutionIdentity::from_approved;
const _: fn(&ProofExecutionIdentity) -> Result<()> = ProofExecutionIdentity::validate;

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
    if input.argv.is_empty() {
        bail!("approved proof argv cannot be empty");
    }
    if input.working_root.trim().is_empty() {
        bail!("proof execution working root is required");
    }
    Ok(())
}

/// Approved argv is already tokenized. Preserve token contents exactly; in
/// particular, do not apply diff-path `b/` normalization to command args.
fn normalize_argv(argv: Vec<String>) -> Result<Vec<String>> {
    argv.into_iter()
        .map(|arg| {
            if arg.is_empty() || arg.contains('\0') || arg.chars().any(char::is_control) {
                bail!("approved proof argv contains an invalid argument");
            }
            Ok(arg)
        })
        .collect()
}

fn normalize_env(env: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let mut normalized = BTreeMap::new();
    for (key, value) in env {
        let key = normalize_token(&key, "environment key")?;
        if value.contains('\0') || value.chars().any(char::is_control) {
            bail!("invalid environment value");
        }
        if normalized.insert(key.clone(), value).is_some() {
            bail!("environment keys collide after normalization: {key}");
        }
    }
    Ok(normalized)
}

fn normalize_features(features: Vec<String>) -> Result<Vec<String>> {
    let mut normalized = features
        .into_iter()
        .map(|feature| normalize_token(&feature, "feature"))
        .collect::<Result<Vec<_>>>()?;
    normalized.sort();
    normalized.dedup();
    Ok(normalized)
}

fn normalize_tool_versions(versions: BTreeMap<String, String>) -> Result<BTreeMap<String, String>> {
    let mut normalized = BTreeMap::new();
    for (tool, version) in versions {
        let tool = normalize_token(&tool, "tool name")?;
        let version = normalize_token(&version, "tool version")?;
        if normalized.insert(tool.clone(), version).is_some() {
            bail!("tool-version keys collide after normalization: {tool}");
        }
    }
    Ok(normalized)
}

fn normalize_root(root: &str) -> Result<String> {
    let root = root.trim().replace('\\', "/");
    if root.is_empty() || root.contains('\0') || root.chars().any(char::is_control) {
        bail!("invalid proof working root");
    }
    if root.chars().all(|character| character == '/') {
        return Ok("/".to_owned());
    }

    // Preserve the semantic root marker while collapsing only redundant
    // separators. Do not resolve `.` or `..`: that would cross symlink and
    // filesystem boundaries and is not a value-only identity operation.
    let (prefix, rest) = if root == "/" {
        ("/", "")
    } else if root.starts_with("//") {
        ("//", root.trim_start_matches('/'))
    } else if root.len() >= 2 && root.as_bytes()[1] == b':' {
        let drive = &root[..2];
        let suffix = &root[2..];
        if suffix.is_empty() || !suffix.starts_with('/') {
            bail!("drive-relative working roots are not supported");
        }
        (drive, suffix.trim_start_matches('/'))
    } else {
        ("", root.as_str())
    };
    let mut result = prefix.to_owned();
    let segments = rest.split('/').filter(|segment| !segment.is_empty());
    for segment in segments {
        if !result.is_empty() && !result.ends_with('/') {
            result.push('/');
        }
        result.push_str(segment);
    }
    if result.is_empty() {
        bail!("invalid proof working root");
    }
    if result == "C:" || (result.len() == 2 && result.as_bytes()[1] == b':') {
        result.push('/');
    }
    Ok(result)
}

fn normalize_revision(value: &str, label: &str) -> Result<String> {
    normalize_token(value, label)
}

/// Test filters are human-readable selectors and may contain spaces. They
/// still reject control and NUL input, which prevents forged delimiters.
fn normalize_test_filter(value: &str) -> Result<String> {
    let normalized = value.trim().to_owned();
    if normalized.is_empty()
        || normalized.contains('\0')
        || normalized.chars().any(char::is_control)
    {
        bail!("invalid proof test filter");
    }
    Ok(normalized)
}

/// Token fields are not paths. Trim only the field boundary and retain token
/// spelling; internal whitespace is rejected for semantic identifiers.
fn normalize_token(value: &str, label: &str) -> Result<String> {
    let normalized = value.trim().to_owned();
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

    fn input() -> ApprovedProofExecution {
        ApprovedProofExecution {
            mode: " head-only ".to_owned(),
            base: " base-sha ".to_owned(),
            head: " head-sha ".to_owned(),
            argv: vec!["cargo".to_owned(), "test".to_owned(), "--locked".to_owned()],
            env: BTreeMap::from([("RUSTFLAGS".to_owned(), "-Dwarnings".to_owned())]),
            working_root: r"C:\repo\\".to_owned(),
            package: " demo ".to_owned(),
            target: " unit ".to_owned(),
            test_filter: Some(" test with spaces ".to_owned()),
            features: vec!["b".to_owned(), "a".to_owned(), "a".to_owned()],
            tool_versions: BTreeMap::from([("cargo".to_owned(), "1.95".to_owned())]),
        }
    }

    #[test]
    fn canonical_equivalence_is_order_and_separator_stable() -> Result<()> {
        let left = ProofExecutionIdentity::from_approved(input())?;
        let mut right_input = input();
        right_input.env = BTreeMap::from([("RUSTFLAGS".to_owned(), "-Dwarnings".to_owned())]);
        right_input.features = vec!["a".to_owned(), "b".to_owned()];
        right_input.working_root = "C:/repo".to_owned();
        let right = ProofExecutionIdentity::from_approved(right_input)?;
        assert_eq!(left, right);
        assert_eq!(left.digest, right.digest);
        Ok(())
    }

    #[test]
    fn material_dimensions_have_distinct_digests() -> Result<()> {
        let base = ProofExecutionIdentity::from_approved(input())?;
        for mutate in [
            |value: &mut ApprovedProofExecution| value.base = "other-base".to_owned(),
            |value: &mut ApprovedProofExecution| value.head = "other-head".to_owned(),
            |value: &mut ApprovedProofExecution| value.mode = "red-green".to_owned(),
            |value: &mut ApprovedProofExecution| value.package = "other-package".to_owned(),
            |value: &mut ApprovedProofExecution| value.target = "other-target".to_owned(),
            |value: &mut ApprovedProofExecution| value.test_filter = None,
            |value: &mut ApprovedProofExecution| value.features.push("extra".to_owned()),
            |value: &mut ApprovedProofExecution| value.argv.push("--other".to_owned()),
            |value: &mut ApprovedProofExecution| {
                value.env.insert("OTHER".to_owned(), "value".to_owned());
            },
            |value: &mut ApprovedProofExecution| value.working_root = "C:/other".to_owned(),
            |value: &mut ApprovedProofExecution| {
                value
                    .tool_versions
                    .insert("cargo".to_owned(), "2".to_owned());
            },
        ] {
            let mut changed = input();
            mutate(&mut changed);
            let changed = ProofExecutionIdentity::from_approved(changed)?;
            assert_ne!(base, changed);
            assert_ne!(base.digest, changed.digest);
        }
        Ok(())
    }

    #[test]
    fn argv_is_not_diff_path_normalized_and_filters_keep_spaces() -> Result<()> {
        let mut changed = input();
        changed.argv.push("b/ordinary-token".to_owned());
        let identity = ProofExecutionIdentity::from_approved(changed)?;
        assert_eq!(
            identity.argv.last().map(String::as_str),
            Some("b/ordinary-token")
        );
        assert_eq!(identity.test_filter.as_deref(), Some("test with spaces"));
        Ok(())
    }

    #[test]
    fn filesystem_root_semantics_are_preserved() -> Result<()> {
        for (raw, expected) in [
            ("/", "/"),
            ("///", "/"),
            (r"C:\", "C:/"),
            ("C://repo///", "C:/repo"),
        ] {
            let mut value = input();
            value.working_root = raw.to_owned();
            assert_eq!(
                ProofExecutionIdentity::from_approved(value)?.working_root,
                expected
            );
        }
        Ok(())
    }

    #[test]
    fn drive_relative_roots_are_rejected() -> Result<()> {
        for raw in ["C:", "C:repo", r"C:repo\nested"] {
            let mut value = input();
            value.working_root = raw.to_owned();
            assert!(ProofExecutionIdentity::from_approved(value).is_err());
        }
        Ok(())
    }

    #[test]
    fn malformed_and_forged_values_fail_closed() -> Result<()> {
        let mut identity = ProofExecutionIdentity::from_approved(input())?;
        identity.head = "forged-head".to_owned();
        assert!(identity.validate().is_err());
        for mutate in [
            |value: &mut ApprovedProofExecution| value.argv = Vec::new(),
            |value: &mut ApprovedProofExecution| value.argv = vec!["cargo\0".to_owned()],
            |value: &mut ApprovedProofExecution| value.test_filter = Some("bad\0filter".to_owned()),
            |value: &mut ApprovedProofExecution| value.working_root = "  ".to_owned(),
        ] {
            let mut value = input();
            mutate(&mut value);
            assert!(ProofExecutionIdentity::from_approved(value).is_err());
        }
        Ok(())
    }

    #[test]
    fn executable_name_is_value_data_not_approval_policy() -> Result<()> {
        let mut value = input();
        value.argv[0] = "custom-approved-tool".to_owned();
        assert!(ProofExecutionIdentity::from_approved(value).is_ok());
        Ok(())
    }

    #[test]
    fn normalized_map_key_collisions_fail_closed() -> Result<()> {
        let mut value = input();
        value.env = BTreeMap::from([
            ("RUSTFLAGS".to_owned(), "one".to_owned()),
            (" RUSTFLAGS ".to_owned(), "two".to_owned()),
        ]);
        assert!(ProofExecutionIdentity::from_approved(value).is_err());

        let mut value = input();
        value.tool_versions = BTreeMap::from([
            ("cargo".to_owned(), "1".to_owned()),
            (" cargo ".to_owned(), "2".to_owned()),
        ]);
        assert!(ProofExecutionIdentity::from_approved(value).is_err());
        Ok(())
    }

    #[test]
    fn digest_rejects_mutated_argv_env_and_working_root() -> Result<()> {
        let original = ProofExecutionIdentity::from_approved(input())?;

        let mut argv = original.clone();
        argv.argv.push("--forged".to_owned());
        assert!(
            argv.validate().is_err(),
            "changing argv without recomputing the digest must fail validation"
        );

        let mut env = original.clone();
        env.env.insert("FORGED".to_owned(), "1".to_owned());
        assert!(
            env.validate().is_err(),
            "changing env without recomputing the digest must fail validation"
        );

        let mut working_root = original;
        working_root.working_root = "C:/other".to_owned();
        assert!(
            working_root.validate().is_err(),
            "changing working_root without recomputing the digest must fail validation"
        );
        Ok(())
    }

    #[test]
    fn serializing_and_reloading_preserves_valid_identity() -> Result<()> {
        let identity = ProofExecutionIdentity::from_approved(input())?;
        let encoded = serde_json::to_string(&identity)?;
        let decoded: ProofExecutionIdentity = serde_json::from_str(&encoded)?;
        decoded.validate()?;
        assert_eq!(decoded, identity);
        Ok(())
    }
}
