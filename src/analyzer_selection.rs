//! #1066 recorded release selection. Read-only evidence validation, never tool
//! execution, installation, publication or activation. Producer authenticity is
//! established by independently retained intake pins, not by this verifier.
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path};

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

use crate::RevisionRef;
use crate::cli::VerifyAnalyzerBundleArgs;

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ObjectRef {
    path: String,
    sha256: String,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Scope {
    revision: RevisionRef,
    corpus_sha256: String,
    policy_sha256: String,
    suppression_sha256: String,
    platform: String,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct ProbeContract {
    argv: Vec<String>,
    stdout_sha256: String,
}

#[derive(Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Identity {
    package: String,
    version: String,
    source_url: String,
    source_sha: String,
    tag_sha: String,
    artifact: ObjectRef,
    executable: ObjectRef,
    output_schema: String,
    capability_sha256: String,
    probes: Vec<ProbeContract>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidencePins {
    public_identity: Option<ObjectRef>,
    install_probe: Option<ObjectRef>,
    compatibility: Option<ObjectRef>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolPins {
    id: String,
    supported_output_schemas: Vec<String>,
    supported_platforms: Vec<String>,
    package_validator_source: String,
    current_released: Identity,
    candidate: EvidencePins,
    rollback: EvidencePins,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Intake {
    schema: String,
    scope: Scope,
    resolved_tools: ObjectRef,
    bundle: ObjectRef,
    public_identity_owner: String,
    install_probe_owner: String,
    compatibility_owner: String,
    tools: Vec<ToolPins>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Bundle {
    schema: String,
    scope: Scope,
    tools: Vec<ToolRow>,
}

// Projection of the existing canonical receipt, not a new registry. The
// remaining plan fields continue to belong to the resolved-tools owner.
#[derive(Deserialize)]
struct ResolvedRegistry {
    schema: String,
    runtime_profile: String,
    tools: Vec<ResolvedRegistryRow>,
}

#[derive(Deserialize)]
struct ResolvedRegistryRow {
    id: String,
    enabled: bool,
    required: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ToolRow {
    id: String,
    required: bool,
    disposition: String,
    candidate: Option<Identity>,
    rollback: Option<Identity>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicIdentity {
    schema: String,
    owner: String,
    tool_id: String,
    package: String,
    version: String,
    source_url: String,
    source_sha: String,
    tag: String,
    tag_sha: String,
    artifact_sha256: String,
    public_verified: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservedProbe {
    argv: Vec<String>,
    executable_sha256: String,
    exit_code: i32,
    stdout: ObjectRef,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InstallProbe {
    schema: String,
    owner: String,
    package_validator_source: String,
    upstream_validation: ObjectRef,
    tool_id: String,
    version: String,
    scope: Scope,
    origin: String,
    source_url: String,
    artifact_sha256: String,
    executable_sha256: String,
    clean_prefix: bool,
    source_checkout_used: bool,
    install_argv: Vec<String>,
    output_schema: String,
    capability_sha256: String,
    probes: Vec<ObservedProbe>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Compatibility {
    schema: String,
    owner: String,
    tool_id: String,
    scope: Scope,
    candidate_artifact_sha256: String,
    candidate_executable_sha256: String,
    baseline_artifact_sha256: String,
    decision: String,
    rollback_qualified: bool,
    rollback_from_artifact_sha256: Option<String>,
}

#[derive(Serialize)]
struct SelectionReceipt {
    schema: &'static str,
    status: &'static str,
    reason: String,
    intake_sha256: String,
    bundle_sha256: Option<String>,
    validated_tools: Vec<String>,
    required_readiness: bool,
    claim_boundary: &'static str,
}

struct Failure {
    status: &'static str,
    reason: String,
}

type Checked<T> = std::result::Result<T, Failure>;

fn failure(status: &'static str, reason: impl Into<String>) -> Failure {
    Failure {
        status,
        reason: reason.into(),
    }
}

fn require(condition: bool, reason: &str) -> Checked<()> {
    if condition {
        Ok(())
    } else {
        Err(failure("contradictory_evidence", reason))
    }
}

fn hex(value: &str, width: usize) -> bool {
    value.len() == width
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && value.bytes().any(|b| b != b'0')
}

fn bytes(root: &Path, object: &ObjectRef, limit: u64) -> Checked<Vec<u8>> {
    require(hex(&object.sha256, 64), "invalid object digest")?;
    require(
        !object.path.is_empty()
            && Path::new(&object.path)
                .components()
                .all(|c| matches!(c, Component::Normal(_))),
        "unsafe evidence path",
    )?;
    let mut path = root.to_path_buf();
    for component in Path::new(&object.path).components() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path).map_err(|e| {
            failure(
                "missing_evidence",
                format!("evidence unavailable: {}: {e}", object.path),
            )
        })?;
        require(
            !metadata.file_type().is_symlink(),
            "symlink evidence rejected",
        )?;
    }
    let file = fs::File::open(&path).map_err(|e| failure("missing_evidence", e.to_string()))?;
    require(
        file.metadata()
            .map_err(|e| failure("missing_evidence", e.to_string()))?
            .is_file(),
        "evidence is not a regular file",
    )?;
    let mut content = Vec::new();
    file.take(limit + 1)
        .read_to_end(&mut content)
        .map_err(|e| failure("missing_evidence", e.to_string()))?;
    require(
        content.len() as u64 <= limit,
        "evidence exceeds bounded intake",
    )?;
    require(
        format!("{:x}", Sha256::digest(&content)) == object.sha256,
        "evidence digest mismatch",
    )?;
    Ok(content)
}

fn object<T: DeserializeOwned>(root: &Path, reference: &ObjectRef) -> Checked<T> {
    serde_json::from_slice(&bytes(root, reference, 512 * 1024)?).map_err(|e| {
        failure(
            "contradictory_evidence",
            format!("invalid recorded object: {e}"),
        )
    })
}

fn pinned<'a>(reference: &'a Option<ObjectRef>, label: &str) -> Checked<&'a ObjectRef> {
    reference
        .as_ref()
        .ok_or_else(|| failure("missing_evidence", format!("missing {label} receipt pin")))
}

fn scope(actual: &Scope, expected: &Scope) -> Checked<()> {
    if actual == expected {
        Ok(())
    } else {
        Err(failure(
            "stale_evidence",
            "receipt scope differs from independently pinned revision/corpus/policy/suppression/platform",
        ))
    }
}

fn validate_identity(
    root: &Path,
    intake: &Intake,
    row: &ToolRow,
    identity: &Identity,
    pins: &EvidencePins,
    rollback: bool,
    tool: &ToolPins,
) -> Checked<()> {
    require(
        !identity.package.is_empty()
            && identity
                .package
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid package identity",
    )?;
    require(
        !identity.version.is_empty()
            && identity
                .version
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-'),
        "invalid exact version",
    )?;
    // Initial bounded lane supports the current Cargo production source only.
    // Go/npm/GitHub archives require their source-owned verifier, never fallback.
    require(
        identity.source_url
            == format!(
                "https://static.crates.io/crates/{}/{}-{}.crate",
                identity.package, identity.package, identity.version
            ),
        "unsupported or nonpublic production source",
    )?;
    require(
        (hex(&identity.source_sha, 40) || hex(&identity.source_sha, 64))
            && identity.source_sha == identity.tag_sha,
        "source/tag identity conflict",
    )?;
    require(
        !identity.output_schema.is_empty() && hex(&identity.capability_sha256, 64),
        "missing schema/capability identity",
    )?;
    require(
        tool.supported_output_schemas
            .contains(&identity.output_schema),
        "unsupported output schema",
    )?;
    require(
        tool.supported_platforms.contains(&intake.scope.platform),
        "unsupported platform",
    )?;
    require(
        identity.probes.len() >= 2
            && identity.probes.iter().any(|p| p.argv == ["--version"])
            && identity
                .probes
                .iter()
                .any(|p| p.argv != ["--version"] && !p.argv.is_empty()),
        "version alone cannot qualify executable",
    )?;
    bytes(root, &identity.artifact, 32 * 1024 * 1024)?;
    bytes(root, &identity.executable, 32 * 1024 * 1024)?;
    let public: PublicIdentity = object(root, pinned(&pins.public_identity, "public identity")?)?;
    require(
        public.schema == "ub-review.analyzer_public_identity.v1"
            && public.owner == intake.public_identity_owner
            && public.tool_id == row.id
            && public.public_verified,
        "public identity receipt not admitted",
    )?;
    require(
        public.package == identity.package
            && public.version == identity.version
            && public.source_url == identity.source_url
            && public.source_sha == identity.source_sha
            && public.tag_sha == identity.tag_sha
            && public.tag == format!("v{}", identity.version)
            && public.artifact_sha256 == identity.artifact.sha256,
        "public artifact/source/tag contradiction",
    )?;
    let install: InstallProbe = object(root, pinned(&pins.install_probe, "install/probe")?)?;
    scope(&install.scope, &intake.scope)?;
    require(
        install.schema == "ub-review.analyzer_install_probe.v1"
            && install.owner == intake.install_probe_owner
            && install.tool_id == row.id
            && install.version == identity.version,
        "wrong tool/install receipt",
    )?;
    require(
        install.package_validator_source == tool.package_validator_source
            && (hex(&tool.package_validator_source, 40) || hex(&tool.package_validator_source, 64)),
        "package identity validator provenance mismatch",
    )?;
    // Retain source-owned package-validation evidence; never reproduce its
    // archive metadata/version parser or infer acceptance from version output.
    bytes(root, &install.upstream_validation, 512 * 1024)?;
    require(
        install.origin == "public_registry"
            && install.clean_prefix
            && !install.source_checkout_used
            && install.source_url == identity.source_url,
        "workspace substitution or unqualified installation",
    )?;
    require(
        install.install_argv
            == [
                "cargo",
                "install",
                &identity.package,
                "--version",
                &identity.version,
                "--locked",
                "--root",
                "<clean-prefix>",
            ],
        "nonpublic or unpinned install command",
    )?;
    require(
        install.artifact_sha256 == identity.artifact.sha256
            && install.executable_sha256 == identity.executable.sha256
            && install.output_schema == identity.output_schema
            && install.capability_sha256 == identity.capability_sha256,
        "installed executable/schema/capability identity conflict",
    )?;
    require(
        install.probes.len() == identity.probes.len(),
        "missing or extra probe",
    )?;
    let mut commands = BTreeSet::new();
    for (probe, expected) in install.probes.iter().zip(&identity.probes) {
        require(
            commands.insert(&probe.argv)
                && probe.argv == expected.argv
                && probe.exit_code == 0
                && probe.executable_sha256 == identity.executable.sha256
                && probe.stdout.sha256 == expected.stdout_sha256,
            "wrong executable/probe contract",
        )?;
        bytes(root, &probe.stdout, 512 * 1024)?;
    }
    let compatibility: Compatibility = object(root, pinned(&pins.compatibility, "compatibility")?)?;
    scope(&compatibility.scope, &intake.scope)?;
    require(
        compatibility.schema == "ub-review.analyzer_compatibility.v1"
            && compatibility.owner == intake.compatibility_owner
            && compatibility.tool_id == row.id
            && compatibility.candidate_artifact_sha256 == identity.artifact.sha256
            && compatibility.candidate_executable_sha256 == identity.executable.sha256
            && compatibility.baseline_artifact_sha256 == tool.current_released.artifact.sha256,
        "compatibility identity contradiction",
    )?;
    if compatibility.decision != "accepted" || (rollback && !compatibility.rollback_qualified) {
        return Err(failure(
            "not_selected",
            "compatibility or rollback qualification not accepted",
        ));
    }
    if rollback {
        let selected = row
            .candidate
            .as_ref()
            .ok_or_else(|| failure("missing_evidence", "missing rollback transition source"))?;
        require(
            compatibility.rollback_from_artifact_sha256.as_deref()
                == Some(selected.artifact.sha256.as_str()),
            "rollback transition differs from selected candidate",
        )?;
    }
    Ok(())
}

fn validate(root: &Path, intake: &Intake) -> Checked<Vec<String>> {
    require(
        intake.schema == "ub-review.analyzer_selection_intake.v1",
        "unsupported selection intake",
    )?;
    intake
        .scope
        .revision
        .validate()
        .map_err(|e| failure("contradictory_evidence", e.to_string()))?;
    require(
        hex(&intake.scope.corpus_sha256, 64)
            && hex(&intake.scope.policy_sha256, 64)
            && hex(&intake.scope.suppression_sha256, 64)
            && !intake.scope.platform.is_empty(),
        "incomplete frozen scope",
    )?;
    require(
        !intake.public_identity_owner.is_empty()
            && !intake.install_probe_owner.is_empty()
            && !intake.compatibility_owner.is_empty(),
        "missing producer owner",
    )?;
    let resolved: ResolvedRegistry = object(root, &intake.resolved_tools)?;
    require(
        resolved.schema == crate::artifacts::RESOLVED_TOOLS_SCHEMA
            && !resolved.runtime_profile.is_empty(),
        "unsupported resolved-tools receipt",
    )?;
    let mut registry = BTreeMap::new();
    let mut all_ids = BTreeSet::new();
    for entry in &resolved.tools {
        let id = entry.id.as_str();
        require(!id.is_empty(), "invalid resolved tool ID")?;
        require(all_ids.insert(id), "duplicate resolved tool")?;
        if entry.enabled {
            registry.insert(id, entry.required);
        }
    }
    let bundle: Bundle = object(root, &intake.bundle)?;
    require(
        bundle.schema == "ub-review.analyzer_bundle.v1",
        "unsupported analyzer bundle",
    )?;
    scope(&bundle.scope, &intake.scope)?;
    let mut pins = BTreeMap::new();
    for pin in &intake.tools {
        require(
            pins.insert(pin.id.as_str(), pin).is_none(),
            "duplicate tool intake pin",
        )?;
    }
    let mut rows = BTreeMap::new();
    for row in &bundle.tools {
        require(
            rows.insert(row.id.as_str(), row).is_none(),
            "duplicate bundle tool",
        )?;
    }
    require(
        !registry.is_empty() && registry.keys().eq(rows.keys()) && registry.keys().eq(pins.keys()),
        "bundle/resolved-tools/intake inventory mismatch",
    )?;
    for (id, row) in &rows {
        require(row.required == registry[id], "required role mismatch")?;
        match row.disposition.as_str() {
            "selected_required" | "retain_current_required" if row.required => {}
            "selected_advisory_only" if !row.required => {}
            "not_selected_incompatible"
            | "not_selected_unavailable"
            | "blocked_missing_public_identity" => {
                return Err(failure(
                    "not_selected",
                    format!("tool {id}: {}", row.disposition),
                ));
            }
            _ => {
                return Err(failure(
                    "contradictory_evidence",
                    "unknown or conflicting selection disposition",
                ));
            }
        }
        let pin = pins[id];
        let candidate = row
            .candidate
            .as_ref()
            .ok_or_else(|| failure("missing_evidence", "missing candidate identity"))?;
        let rollback = row
            .rollback
            .as_ref()
            .ok_or_else(|| failure("missing_evidence", "missing rollback identity"))?;
        require(
            *rollback == pin.current_released,
            "rollback differs from pinned current released identity",
        )?;
        if row.disposition == "retain_current_required" {
            require(
                *candidate == pin.current_released,
                "retained identity differs from current released bundle",
            )?;
        }
        validate_identity(root, intake, row, candidate, &pin.candidate, false, pin)?;
        validate_identity(root, intake, row, rollback, &pin.rollback, true, pin)?;
    }
    Ok(rows.keys().map(|id| (*id).to_owned()).collect())
}

pub(crate) fn cmd_verify_analyzer_bundle(args: VerifyAnalyzerBundleArgs) -> Result<()> {
    let root = fs::canonicalize(&args.evidence_root)?;
    let reference = ObjectRef {
        path: args.intake,
        sha256: args.intake_sha256.clone(),
    };
    let intake: Checked<Intake> = object(&root, &reference);
    let bundle_sha256 = intake.as_ref().ok().map(|i| i.bundle.sha256.clone());
    let checked = intake.and_then(|i| validate(&root, &i));
    let (status, reason, validated_tools) = match checked {
        Ok(tools) => (
            "selected_recorded_receipt",
            "All independently pinned recorded identities and qualification receipts agree"
                .to_owned(),
            tools,
        ),
        Err(error) => (error.status, error.reason, Vec::new()),
    };
    let receipt = SelectionReceipt {
        schema: crate::artifacts::ANALYZER_SELECTION_RECEIPT_SCHEMA,
        status,
        reason,
        intake_sha256: args.intake_sha256,
        bundle_sha256,
        validated_tools,
        required_readiness: false,
        claim_boundary: "Recorded evidence validation only. Producer authenticity and public installation execution are upstream obligations. No active pins, runtime admission, promotion or Required qualification.",
    };
    println!("{}", serde_json::to_string_pretty(&receipt)?);
    if status != "selected_recorded_receipt" {
        bail!("analyzer bundle not selected: {status}");
    }
    Ok(())
}
