//! Production publish-path adapter (#1344): admit exactly the authorized
//! candidate bytes, then create the tag, release and assets exactly once.
//!
//! Trust rules implemented here (see `docs/RELEASE_RUNBOOK.md` and
//! `xtask/src/release_bytes.rs` for the admission contract):
//! - The staged candidate directory is untrusted input. It supplies only the
//!   candidate bytes and the candidate packet. Every producer, tag, version
//!   and authorization fact arrives through [`TrustedFacts`], which the
//!   workflow resolves from GitHub API observations and the dispatch
//!   authorization, never from candidate-controlled files.
//! - [`resolve_publication`] admits the bytes through the merged #1343
//!   validator (`ub-review.authorized_release.v1`) and rechecks freshness at
//!   the publication boundary. Every rejection is a refusal with zero
//!   create/upload side effects.
//! - [`ReleasePublisher`] can only survey and create. The trait has no
//!   overwrite, move or delete capability, so a stop can never restore
//!   overwrite behavior; occupied tags/releases, partial external state and
//!   API outages require a separately authorized recovery decision.
//! - Candidate-controlled code never executes with publishing credentials:
//!   the publish job builds this adapter from the dispatch ref, and the
//!   package job that runs candidate code holds only `contents: read`.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde_json::{Value as JsonValue, json};

use crate::release_bytes::{self as rb};

/// Decision receipt schema printed by `validate-release`.
pub const DECISION_SCHEMA: &str = "ub-review.release_publish_decision.v1";
/// Candidate packet file name inside the staged artifact.
pub const PACKET_FILE: &str = "authorized-release-packet.json";
/// Offline-rendered release notes file name inside the staged artifact.
pub const NOTES_FILE: &str = "release-notes.md";
/// Digest receipt schema printed by `release-packet-digest`.
pub const PACKET_DIGEST_SCHEMA: &str = "ub-review.release_packet_digest.v1";

/// Candidate-controlled staged bytes plus offline-rendered notes. Notes are
/// candidate prose pinned by the offline renderer during preparation; they
/// are never executed and carry no authority.
#[derive(Clone)]
pub struct StagedCandidate {
    pub packet: Vec<u8>,
    pub archive: Vec<u8>,
    pub checksum: Vec<u8>,
    pub notes: String,
}

/// Facts resolved independently of the candidate artifact. Each field maps to
/// a GitHub API observation or a dispatch-input authorization field; the
/// workflow comments in `release-binary.yml` carry the exact mapping.
pub struct TrustedFacts {
    pub repository: String,
    pub workflow: String,
    pub head_sha: String,
    pub tree_sha: String,
    pub producer_ref: String,
    pub run_id: u64,
    pub run_attempt: u64,
    pub artifact_id: u64,
    pub artifact_name: String,
    /// Independently observed artifact expiry horizon (epoch seconds).
    pub retained_until: u64,
    pub proposed_tag: String,
    /// Package version observed from the candidate commit's Cargo.toml.
    pub package_version: String,
    pub authorization_record: String,
    pub authorization_verifier: String,
    pub packet_sha256: String,
    pub authorization_expires_at: u64,
    /// Trusted current time at the publication boundary.
    pub now: u64,
}

/// Admit the staged candidate against the trusted facts. Returns the verified
/// publication with privately captured bytes, or a rejection with zero side
/// effects.
pub fn resolve_publication(
    staged: &StagedCandidate,
    facts: &TrustedFacts,
) -> std::result::Result<rb::VerifiedPublication, rb::Rejection> {
    // Independent package-version evidence: the packet's version claims must
    // match Cargo.toml observed at the admitted commit before admission.
    let packet = rb::parse_packet(&staged.packet)?;
    if packet.package_version != facts.package_version {
        return Err(rb::Rejection::Identity("package_version"));
    }
    let producer = rb::Observation::Success(rb::Producer {
        repository: facts.repository.clone(),
        head_sha: facts.head_sha.clone(),
        tree_sha: facts.tree_sha.clone(),
        workflow: facts.workflow.clone(),
        producer_ref: facts.producer_ref.clone(),
        run_id: facts.run_id,
        run_attempt: facts.run_attempt,
        artifact_id: facts.artifact_id,
        artifact_name: facts.artifact_name.clone(),
        retained_until: facts.retained_until,
    });
    // The proposed release tag must be absent, with the admitted candidate
    // commit as its authorized expected peel.
    let tag = rb::Observation::Success(rb::UnusedTag {
        repository: facts.repository.clone(),
        tag: facts.proposed_tag.clone(),
        peeled_commit: facts.head_sha.clone(),
    });
    // The authorization packet is supplied by the dispatching authority, not
    // by the candidate. `validate` binds its record, verifier identity and
    // exact packet digest before trusting it.
    let authority = rb::Authority::Trusted(rb::Authorization {
        record: facts.authorization_record.clone(),
        packet_sha256: facts.packet_sha256.clone(),
        verifier: facts.authorization_verifier.clone(),
        expires_at: facts.authorization_expires_at,
    });
    let verified = rb::validate(
        &staged.packet,
        &staged.archive,
        &staged.checksum,
        &producer,
        &tag,
        &authority,
        facts.now,
    )?;
    verified.ensure_current(facts.now)?;
    Ok(verified)
}

/// Read-only observation of the external publication state for a tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalState {
    /// Observed refs under `refs/tags/<tag>`; non-empty means occupied.
    pub tag_refs: Vec<String>,
    /// Observed release (including drafts) carrying `tag`; `None` means no
    /// release exists, `Some(assets)` lists its asset names.
    pub release_assets: Option<Vec<String>>,
}

/// A refused publication attempt. Every variant leaves external state
/// untouched: the caller must stop and obtain explicit recovery authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PublishStop {
    OccupiedTag {
        refs: Vec<String>,
        detail: String,
    },
    OccupiedRelease {
        assets: Vec<String>,
    },
    PartialPublication {
        existing_assets: Vec<String>,
        missing_assets: Vec<String>,
    },
    ApiOutage {
        detail: String,
    },
    StageFailed {
        detail: String,
    },
    CreateFailed {
        detail: String,
    },
    /// The tag ref exists after creation but does not resolve to the admitted
    /// commit. External state is retained; recovery is a separate decision.
    TagTargetMismatch {
        expected: String,
        observed: String,
    },
}

/// The exact create request for one publication. Byte slices are the
/// validator-captured, privately owned bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseRequest {
    pub repository: String,
    pub tag: String,
    pub target_commit: String,
    pub title: String,
    pub notes: String,
    pub archive_name: String,
    pub archive: Vec<u8>,
    pub checksum_name: String,
    pub checksum: Vec<u8>,
}

/// Publishes admitted bytes. The trait exposes exactly one positive
/// capability — a first publication of the proposed tag — plus read-only
/// survey. There is no overwrite, move or delete capability, so no caller
/// can restore overwrite behavior.
pub trait ReleasePublisher {
    /// Observe current external state for `tag` without side effects.
    fn survey(&mut self, repository: &str, tag: &str) -> Result<ExternalState, PublishStop>;
    /// Create `refs/tags/<tag>` exactly at `target` atomically. A ref that
    /// already exists (historical or concurrently created) can only make
    /// this fail; it is never adopted or moved.
    fn create_tag(&mut self, repository: &str, tag: &str, target: &str) -> Result<(), PublishStop>;
    /// Read back the created ref and require it to resolve to `target`.
    fn verify_tag(&mut self, repository: &str, tag: &str, target: &str) -> Result<(), PublishStop>;
    /// Create the release for the existing verified tag and upload both
    /// assets from the captured bytes exactly once.
    fn create_release(&mut self, request: &ReleaseRequest) -> Result<(), PublishStop>;
    /// The one authorized publication sequence: refuse occupied state,
    /// create the tag atomically at the admitted commit, verify its target,
    /// then create the release with both assets. Any stop leaves external
    /// state retained and requires a separately authorized recovery.
    fn publish(&mut self, request: &ReleaseRequest) -> Result<(), PublishStop> {
        let state = self.survey(&request.repository, &request.tag)?;
        decide_external_state(
            &state,
            &[
                request.archive_name.as_str(),
                request.checksum_name.as_str(),
            ],
        )?;
        // The tag ref is created first and verified before any release or
        // asset exists, so a concurrently created historical tag can only
        // stop the path (create_tag fails on the existing ref); it is never
        // silently adopted as the release target.
        self.create_tag(&request.repository, &request.tag, &request.target_commit)?;
        self.verify_tag(&request.repository, &request.tag, &request.target_commit)?;
        self.create_release(request)
    }
}

/// Shared refusal policy over an observed external state: any occupied tag or
/// release, or partial asset set, stops the publication.
pub fn decide_external_state(
    state: &ExternalState,
    asset_names: &[&str],
) -> Result<(), PublishStop> {
    if !state.tag_refs.is_empty() {
        return Err(PublishStop::OccupiedTag {
            refs: state.tag_refs.clone(),
            detail: "survey observed an occupied tag before any create".into(),
        });
    }
    if let Some(assets) = &state.release_assets {
        let missing: Vec<String> = asset_names
            .iter()
            .filter(|name| !assets.iter().any(|asset| asset == *name))
            .map(|name| (*name).to_owned())
            .collect();
        if missing.is_empty() {
            return Err(PublishStop::OccupiedRelease {
                assets: assets.clone(),
            });
        }
        return Err(PublishStop::PartialPublication {
            existing_assets: assets.clone(),
            missing_assets: missing,
        });
    }
    Ok(())
}

fn bounded(text: &str) -> String {
    text.trim().chars().take(400).collect()
}

fn epoch_nanos() -> Result<u128> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")
        .map(|duration| duration.as_nanos())
}

enum GhFailure {
    NotFound(String),
    Conflict(String),
    Outage(String),
}

/// Real publisher over the `gh` CLI. All calls are read-only surveys, an
/// atomic tag-ref create, or one release create; no `--clobber`, no
/// upload-to-existing-release, no deletion.
pub struct GhPublisher {
    gh: String,
}

/// Page budget for the release survey. The exact tag-ref lookup catches every
/// published collision regardless of this bound; the page scan exists to
/// observe draft releases (which the by-tag endpoint never returns), and a
/// draft carrying the proposed fresh tag older than the most recent releases
/// in a repository is not a realistic collision.
const RELEASE_SURVEY_PAGES: u32 = 10;
const RELEASE_PAGE_SIZE: u32 = 100;

impl GhPublisher {
    pub fn new() -> Self {
        Self {
            gh: "gh".to_owned(),
        }
    }

    fn api_args(&self, args: &[&str]) -> std::result::Result<JsonValue, GhFailure> {
        let output = Command::new(&self.gh)
            .args(args)
            .output()
            .map_err(|error| GhFailure::Outage(format!("could not invoke gh api: {error}")))?;
        if output.status.success() {
            return serde_json::from_slice(&output.stdout).map_err(|error| {
                GhFailure::Outage(format!("gh api returned unparseable JSON: {error}"))
            });
        }
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        if stderr.contains("HTTP 404") {
            return Err(GhFailure::NotFound(bounded(&stderr)));
        }
        if stderr.contains("HTTP 422") {
            return Err(GhFailure::Conflict(bounded(&stderr)));
        }
        Err(GhFailure::Outage(bounded(&stderr)))
    }

    fn api(&self, path_and_query: &str) -> std::result::Result<JsonValue, GhFailure> {
        self.api_args(&["api", path_and_query])
    }

    fn post_json(
        &self,
        path_and_query: &str,
        fields: &[&str],
    ) -> std::result::Result<JsonValue, GhFailure> {
        let mut args = vec!["api", "--method", "POST", path_and_query];
        args.extend_from_slice(fields);
        self.api_args(&args)
    }

    fn string_field(value: &JsonValue, field: &str) -> String {
        value
            .get(field)
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
            .to_owned()
    }
}

impl Default for GhPublisher {
    fn default() -> Self {
        Self::new()
    }
}

impl ReleasePublisher for GhPublisher {
    fn survey(&mut self, repository: &str, tag: &str) -> Result<ExternalState, PublishStop> {
        let outage = |detail: String| PublishStop::ApiOutage { detail };
        let tag_refs = match self.api(&format!("repos/{repository}/git/ref/tags/{tag}")) {
            Ok(value) => vec![Self::string_field(&value, "ref")],
            Err(GhFailure::NotFound(_)) => Vec::new(),
            Err(GhFailure::Conflict(detail)) => return Err(outage(detail)),
            Err(GhFailure::Outage(detail)) => return Err(outage(detail)),
        };
        // Scan release pages for the tag: the list endpoint (unlike the
        // by-tag endpoint) also observes draft releases left behind by a
        // partial create. The exact tag-ref lookup above already catches
        // every published collision; this scan is bounded by the budget
        // because a proposed tag is always freshly derived from the current
        // package version.
        let mut release_assets = None;
        for page in 1..=RELEASE_SURVEY_PAGES {
            let releases = match self.api(&format!(
                "repos/{repository}/releases?per_page={RELEASE_PAGE_SIZE}&page={page}"
            )) {
                Ok(value) => value,
                Err(GhFailure::NotFound(detail)) => return Err(outage(detail)),
                Err(GhFailure::Conflict(detail)) => return Err(outage(detail)),
                Err(GhFailure::Outage(detail)) => return Err(outage(detail)),
            };
            let Some(list) = releases.as_array() else {
                return Err(outage("release survey returned a non-array page".into()));
            };
            if let Some(release) = list
                .iter()
                .find(|release| Self::string_field(release, "tag_name") == tag)
            {
                release_assets = Some(
                    release
                        .get("assets")
                        .and_then(JsonValue::as_array)
                        .map(|assets| {
                            assets
                                .iter()
                                .map(|asset| Self::string_field(asset, "name"))
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default(),
                );
                break;
            }
            if list.len() < RELEASE_PAGE_SIZE as usize {
                break;
            }
        }
        Ok(ExternalState {
            tag_refs,
            release_assets,
        })
    }

    fn create_tag(&mut self, repository: &str, tag: &str, target: &str) -> Result<(), PublishStop> {
        match self.post_json(
            &format!("repos/{repository}/git/refs"),
            &[
                "-f",
                "ref",
                &format!("refs/tags/{tag}"),
                "-f",
                "sha",
                target,
            ],
        ) {
            Ok(_) => Ok(()),
            // HTTP 422: the ref already exists — a historical or concurrently
            // created tag. It is never adopted; the path stops.
            Err(GhFailure::Conflict(detail)) => Err(PublishStop::OccupiedTag {
                refs: vec![format!("refs/tags/{tag}")],
                detail,
            }),
            Err(GhFailure::NotFound(detail)) => Err(PublishStop::ApiOutage { detail }),
            Err(GhFailure::Outage(detail)) => Err(PublishStop::ApiOutage { detail }),
        }
    }

    fn verify_tag(&mut self, repository: &str, tag: &str, target: &str) -> Result<(), PublishStop> {
        match self.api(&format!("repos/{repository}/git/ref/tags/{tag}")) {
            Ok(value) => {
                let observed = value
                    .get("object")
                    .and_then(|object| object.get("sha"))
                    .and_then(JsonValue::as_str)
                    .unwrap_or_default()
                    .to_owned();
                if observed == target {
                    Ok(())
                } else {
                    Err(PublishStop::TagTargetMismatch {
                        expected: target.to_owned(),
                        observed,
                    })
                }
            }
            Err(GhFailure::NotFound(detail)) => Err(PublishStop::ApiOutage {
                detail: format!("tag ref vanished immediately after creation: {detail}"),
            }),
            Err(GhFailure::Conflict(detail)) => Err(PublishStop::ApiOutage { detail }),
            Err(GhFailure::Outage(detail)) => Err(PublishStop::ApiOutage { detail }),
        }
    }

    fn create_release(&mut self, request: &ReleaseRequest) -> Result<(), PublishStop> {
        let staging = stage_private_bytes(request)?;
        let created = self.create_via_gh(request, &staging);
        let _ = fs::remove_dir_all(&staging.dir);
        created
    }
}

struct PrivateStaging {
    dir: PathBuf,
    archive: PathBuf,
    checksum: PathBuf,
    notes: PathBuf,
}

/// Write the captured bytes to a fresh private directory and prove the files
/// are byte-identical before handing paths to `gh`. Between admission and
/// upload, the bytes come from memory, not from the mutable staging input.
fn stage_private_bytes(request: &ReleaseRequest) -> Result<PrivateStaging, PublishStop> {
    let failure = |detail: String| PublishStop::StageFailed { detail };
    let nanos = epoch_nanos().map_err(|error| failure(error.to_string()))?;
    let dir =
        std::env::temp_dir().join(format!("ub-review-publish-{}-{nanos}", std::process::id()));
    fs::create_dir(&dir).map_err(|error| failure(format!("create private staging: {error}")))?;
    let archive = dir.join(&request.archive_name);
    let checksum = dir.join(&request.checksum_name);
    let notes = dir.join(NOTES_FILE);
    for (path, bytes) in [(&archive, &request.archive), (&checksum, &request.checksum)] {
        if let Err(error) = fs::write(path, bytes) {
            return Err(failure(format!("write {}: {error}", path.display())));
        }
    }
    if let Err(error) = fs::write(&notes, &request.notes) {
        return Err(failure(format!("write {}: {error}", notes.display())));
    }
    for (path, bytes) in [(&archive, &request.archive), (&checksum, &request.checksum)] {
        match fs::read(path) {
            Ok(actual) if actual == *bytes => {}
            Ok(actual) => {
                return Err(failure(format!(
                    "private staging for {} diverged ({} vs {} bytes)",
                    path.display(),
                    actual.len(),
                    bytes.len()
                )));
            }
            Err(error) => return Err(failure(format!("re-read {}: {error}", path.display()))),
        }
    }
    restrict_permissions(&dir, &[&archive, &checksum, &notes])
        .map_err(|error| failure(format!("restrict staging permissions: {error}")))?;
    Ok(PrivateStaging {
        dir,
        archive,
        checksum,
        notes,
    })
}

#[cfg(unix)]
fn restrict_permissions(dir: &Path, files: &[&Path]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    for path in files {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn restrict_permissions(_dir: &Path, _files: &[&Path]) -> std::io::Result<()> {
    Ok(())
}

impl GhPublisher {
    fn create_via_gh(
        &self,
        request: &ReleaseRequest,
        staging: &PrivateStaging,
    ) -> Result<(), PublishStop> {
        // The tag ref was created and verified by the earlier sequence steps,
        // so --verify-tag (abort if the tag is missing) guards a concurrent
        // deletion without ever letting gh create or move a tag itself.
        let output = Command::new(&self.gh)
            .args([
                "release".to_owned(),
                "create".to_owned(),
                request.tag.clone(),
                "--repo".to_owned(),
                request.repository.clone(),
                "--verify-tag".to_owned(),
                "--title".to_owned(),
                request.title.clone(),
                "--notes-file".to_owned(),
                staging.notes.display().to_string(),
                staging.archive.display().to_string(),
                staging.checksum.display().to_string(),
            ])
            .output()
            .map_err(|error| PublishStop::CreateFailed {
                detail: format!("could not invoke gh release create: {error}"),
            })?;
        if output.status.success() {
            return Ok(());
        }
        Err(PublishStop::CreateFailed {
            detail: bounded(&String::from_utf8_lossy(&output.stderr)),
        })
    }
}

/// `cargo xtask validate-release` options. Every fact arrives as an explicit
/// flag so the workflow's trusted resolution is visible in the command line.
#[derive(Clone, Debug)]
pub struct ValidateReleaseOptions {
    pub staging: PathBuf,
    pub repository: String,
    pub workflow: String,
    pub head_sha: String,
    pub tree_sha: String,
    pub producer_ref: String,
    pub run_id: u64,
    pub run_attempt: u64,
    pub artifact_id: u64,
    pub artifact_name: String,
    pub retained_until: u64,
    pub proposed_tag: String,
    pub package_version: String,
    pub authorization_record: String,
    pub authorization_verifier: String,
    pub packet_sha256: String,
    pub authorization_expires_at: u64,
    pub now: u64,
    pub publish: bool,
}

impl ValidateReleaseOptions {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut staging: Option<PathBuf> = None;
        let mut repository: Option<String> = None;
        let mut workflow: Option<String> = None;
        let mut head_sha: Option<String> = None;
        let mut tree_sha: Option<String> = None;
        let mut producer_ref: Option<String> = None;
        let mut run_id: Option<u64> = None;
        let mut run_attempt: Option<u64> = None;
        let mut artifact_id: Option<u64> = None;
        let mut artifact_name: Option<String> = None;
        let mut retained_until: Option<u64> = None;
        let mut proposed_tag: Option<String> = None;
        let mut package_version: Option<String> = None;
        let mut authorization_record: Option<String> = None;
        let mut authorization_verifier: Option<String> = None;
        let mut packet_sha256: Option<String> = None;
        let mut authorization_expires_at: Option<u64> = None;
        let mut now: Option<u64> = None;
        let mut publish = false;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--publish" => publish = true,
                "--staging" => staging = Some(PathBuf::from(take_value(&mut args, "--staging")?)),
                "--repository" => repository = Some(take_value(&mut args, "--repository")?),
                "--workflow" => workflow = Some(take_value(&mut args, "--workflow")?),
                "--head-sha" => head_sha = Some(take_value(&mut args, "--head-sha")?),
                "--tree-sha" => tree_sha = Some(take_value(&mut args, "--tree-sha")?),
                "--producer-ref" => producer_ref = Some(take_value(&mut args, "--producer-ref")?),
                "--artifact-name" => {
                    artifact_name = Some(take_value(&mut args, "--artifact-name")?)
                }
                "--proposed-tag" => proposed_tag = Some(take_value(&mut args, "--proposed-tag")?),
                "--package-version" => {
                    package_version = Some(take_value(&mut args, "--package-version")?)
                }
                "--authorization-record" => {
                    authorization_record = Some(take_value(&mut args, "--authorization-record")?)
                }
                "--authorization-verifier" => {
                    authorization_verifier =
                        Some(take_value(&mut args, "--authorization-verifier")?)
                }
                "--packet-sha256" => {
                    packet_sha256 = Some(take_value(&mut args, "--packet-sha256")?)
                }
                "--run-id" => run_id = Some(take_u64(&mut args, "--run-id")?),
                "--run-attempt" => run_attempt = Some(take_u64(&mut args, "--run-attempt")?),
                "--artifact-id" => artifact_id = Some(take_u64(&mut args, "--artifact-id")?),
                "--retained-until" => {
                    retained_until = Some(take_u64(&mut args, "--retained-until")?)
                }
                "--authorization-expires-at" => {
                    authorization_expires_at =
                        Some(take_u64(&mut args, "--authorization-expires-at")?)
                }
                "--now" => now = Some(take_u64(&mut args, "--now")?),
                other => bail!("unexpected validate-release argument `{other}`"),
            }
        }
        let missing =
            |name: &'static str| -> anyhow::Error { anyhow::anyhow!("missing required {name}") };
        Ok(Self {
            staging: staging.ok_or_else(|| missing("--staging <dir>"))?,
            repository: repository.ok_or_else(|| missing("--repository <repo>"))?,
            workflow: workflow.ok_or_else(|| missing("--workflow <path>"))?,
            head_sha: head_sha.ok_or_else(|| missing("--head-sha <sha>"))?,
            tree_sha: tree_sha.ok_or_else(|| missing("--tree-sha <sha>"))?,
            producer_ref: producer_ref.ok_or_else(|| missing("--producer-ref <ref>"))?,
            run_id: run_id.ok_or_else(|| missing("--run-id <id>"))?,
            run_attempt: run_attempt.ok_or_else(|| missing("--run-attempt <n>"))?,
            artifact_id: artifact_id.ok_or_else(|| missing("--artifact-id <id>"))?,
            artifact_name: artifact_name.ok_or_else(|| missing("--artifact-name <name>"))?,
            retained_until: retained_until.ok_or_else(|| missing("--retained-until <epoch>"))?,
            proposed_tag: proposed_tag.ok_or_else(|| missing("--proposed-tag <tag>"))?,
            package_version: package_version.ok_or_else(|| missing("--package-version <ver>"))?,
            authorization_record: authorization_record
                .ok_or_else(|| missing("--authorization-record <record>"))?,
            authorization_verifier: authorization_verifier
                .ok_or_else(|| missing("--authorization-verifier <identity>"))?,
            packet_sha256: packet_sha256.ok_or_else(|| missing("--packet-sha256 <digest>"))?,
            authorization_expires_at: authorization_expires_at
                .ok_or_else(|| missing("--authorization-expires-at <epoch>"))?,
            now: now.ok_or_else(|| missing("--now <epoch>"))?,
            publish,
        })
    }
}

fn take_value(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<String> {
    let value = args
        .next()
        .with_context(|| format!("{flag} requires a value"))?;
    if value.is_empty() {
        bail!("{flag} must not be empty");
    }
    Ok(value)
}

fn take_u64(args: &mut impl Iterator<Item = String>, flag: &str) -> Result<u64> {
    let value = take_value(args, flag)?;
    value
        .parse::<u64>()
        .with_context(|| format!("{flag} must be a non-negative integer, got `{value}`"))
}

/// CLI entry for `cargo xtask validate-release`. Uses the real `gh`-based
/// publisher; tests inject a recording publisher through
/// [`run_with_publisher`].
pub fn run(root: &Path, options: ValidateReleaseOptions) -> Result<()> {
    let mut publisher = GhPublisher::new();
    run_with_publisher(root, options, &mut publisher)
}

fn resolve_staging_path(root: &Path, staging: &Path) -> PathBuf {
    if staging.is_absolute() {
        staging.to_path_buf()
    } else {
        root.join(staging)
    }
}

fn read_staged_candidate(staging: &Path) -> Result<StagedCandidate> {
    let packet = fs::read(staging.join(PACKET_FILE)).with_context(|| {
        format!(
            "read staged candidate packet {}",
            staging.join(PACKET_FILE).display()
        )
    })?;
    let archive = fs::read(staging.join(rb::ASSET))
        .with_context(|| format!("read staged archive {}", staging.join(rb::ASSET).display()))?;
    let checksum = fs::read(staging.join(rb::CHECKSUM_ASSET)).with_context(|| {
        format!(
            "read staged checksum {}",
            staging.join(rb::CHECKSUM_ASSET).display()
        )
    })?;
    let notes = fs::read_to_string(staging.join(NOTES_FILE))
        .with_context(|| format!("read staged notes {}", staging.join(NOTES_FILE).display()))?;
    Ok(StagedCandidate {
        packet,
        archive,
        checksum,
        notes,
    })
}

/// Decision entry point shared by the CLI and the recording-publisher tests.
/// Reads the staged candidate as data, admits it against the trusted facts,
/// and publishes the captured bytes exactly once when `publish` is set.
/// Any admission refusal or publisher stop exits nonzero with zero
/// create/upload side effects.
pub fn run_with_publisher(
    root: &Path,
    options: ValidateReleaseOptions,
    publisher: &mut dyn ReleasePublisher,
) -> Result<()> {
    let staging = resolve_staging_path(root, &options.staging);
    let staged = read_staged_candidate(&staging)?;
    let facts = TrustedFacts {
        repository: options.repository.clone(),
        workflow: options.workflow.clone(),
        head_sha: options.head_sha.clone(),
        tree_sha: options.tree_sha.clone(),
        producer_ref: options.producer_ref.clone(),
        run_id: options.run_id,
        run_attempt: options.run_attempt,
        artifact_id: options.artifact_id,
        artifact_name: options.artifact_name.clone(),
        retained_until: options.retained_until,
        proposed_tag: options.proposed_tag.clone(),
        package_version: options.package_version.clone(),
        authorization_record: options.authorization_record.clone(),
        authorization_verifier: options.authorization_verifier.clone(),
        packet_sha256: options.packet_sha256.clone(),
        authorization_expires_at: options.authorization_expires_at,
        now: options.now,
    };
    decide_and_publish(staged, facts, options.publish, publisher)
}

/// Admission decision plus single-shot publication over the already-staged
/// candidate. The request is built exclusively from the validator's privately
/// captured byte slices, so no caller can substitute different bytes between
/// validation and upload.
pub fn decide_and_publish(
    staged: StagedCandidate,
    facts: TrustedFacts,
    publish: bool,
    publisher: &mut dyn ReleasePublisher,
) -> Result<()> {
    let verified = match resolve_publication(&staged, &facts) {
        Ok(verified) => verified,
        Err(reason) => {
            println!(
                "{}",
                json!({
                    "schema": DECISION_SCHEMA,
                    "decision": "refuse",
                    "reason": format!("{reason:?}"),
                    "side_effects": [],
                })
            );
            bail!(
                "publish path refused admission ({reason:?}); no create/upload side effect was performed"
            );
        }
    };
    let receipt = json!({
        "schema": DECISION_SCHEMA,
        "decision": "publish",
        "tag": verified.packet().proposed_tag,
        "target_commit": verified.packet().head_sha,
        "archive": verified.packet().asset,
        "archive_sha256": rb::sha256(verified.archive()),
        "checksum_asset": verified.packet().checksum_asset,
        "checksum_sha256": rb::sha256(verified.checksum()),
        "packet_sha256": verified.packet_sha256(),
        "run_id": verified.packet().run_id,
        "run_attempt": verified.packet().run_attempt,
        "artifact_id": verified.packet().artifact_id,
        "valid_until": verified.valid_until(),
        "side_effects": if publish {
            json!(["create-tag", "create-release", "upload-archive", "upload-checksum"])
        } else {
            json!([])
        },
    });
    println!("{receipt}");
    if !publish {
        return Ok(());
    }
    let request = ReleaseRequest {
        repository: verified.packet().repository.clone(),
        tag: verified.packet().proposed_tag.clone(),
        target_commit: verified.packet().head_sha.clone(),
        title: verified.packet().proposed_tag.clone(),
        notes: staged.notes,
        archive_name: verified.packet().asset.clone(),
        archive: verified.archive().to_vec(),
        checksum_name: verified.packet().checksum_asset.clone(),
        checksum: verified.checksum().to_vec(),
    };
    publisher
        .publish(&request)
        .map_err(|stop| anyhow::anyhow!("publish stopped without overwrite: {stop:?}"))?;
    println!(
        "{}",
        json!({
            "schema": DECISION_SCHEMA,
            "decision": "published",
            "tag": request.tag,
            "archive": request.archive_name,
            "checksum_asset": request.checksum_name,
        })
    );
    Ok(())
}

/// `cargo xtask release-packet-digest` options: the offline digest derivation
/// the release authority uses to authorize an exact candidate packet.
#[derive(Clone, Debug)]
pub struct PacketDigestOptions {
    pub packet: PathBuf,
}

impl PacketDigestOptions {
    pub fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mut packet: Option<PathBuf> = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--packet" => packet = Some(PathBuf::from(take_value(&mut args, "--packet")?)),
                other => bail!("unexpected release-packet-digest argument `{other}`"),
            }
        }
        Ok(Self {
            packet: packet.ok_or_else(|| anyhow::anyhow!("missing required --packet <path>"))?,
        })
    }
}

/// Offline, non-publishing digest of an exact candidate packet. The release
/// authority independently derives this value before naming it in an
/// authorization; the publish path re-derives and binds it at admission.
pub fn run_packet_digest(root: &Path, options: PacketDigestOptions) -> Result<()> {
    let path = resolve_staging_path(root, &options.packet);
    let bytes = fs::read(&path).with_context(|| format!("read {}", path.display()))?;
    let packet = rb::parse_packet(&bytes)
        .map_err(|reason| anyhow::anyhow!("packet {} rejected: {reason:?}", path.display()))?;
    let digest =
        rb::packet_digest(&packet).map_err(|reason| anyhow::anyhow!("digest: {reason:?}"))?;
    println!(
        "{}",
        json!({
            "schema": PACKET_DIGEST_SCHEMA,
            "packet": path.display().to_string(),
            "packet_sha256": digest,
            "proposed_tag": packet.proposed_tag,
            "head_sha": packet.head_sha,
            "archive_sha256": packet.archive_sha256,
        })
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write as IoWrite;

    const RECORD: &str = "release-authority/817/record-7";
    const VERIFIER: &str = "trusted-release-verifier/v1";

    fn gzip(tar: &[u8]) -> Result<Vec<u8>> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(tar)?;
        encoder.finish().map_err(Into::into)
    }
    fn header(name: &[u8], kind: u8, prefix: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0; 512];
        bytes[..name.len()].copy_from_slice(name);
        bytes[100..108].copy_from_slice(b"0000755\0");
        bytes[124..136].copy_from_slice(b"00000000004\0");
        bytes[148..156].fill(b' ');
        bytes[156] = kind;
        bytes[257..263].copy_from_slice(b"ustar\0");
        bytes[263..265].copy_from_slice(b"00");
        bytes[345..345 + prefix.len()].copy_from_slice(prefix);
        seal_header(&mut bytes);
        bytes
    }
    fn seal_header(bytes: &mut [u8]) {
        bytes[148..156].fill(b' ');
        let sum: u64 = bytes[..512].iter().map(|b| u64::from(*b)).sum();
        bytes[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    }
    fn tar_bytes(name: &[u8], kind: u8, prefix: &[u8]) -> Vec<u8> {
        let mut bytes = header(name, kind, prefix);
        bytes.extend(b"ELF!");
        bytes.resize(2048, 0);
        bytes
    }

    struct Fixture {
        packet: rb::Packet,
        archive: Vec<u8>,
        checksum: Vec<u8>,
        notes: String,
        facts: TrustedFacts,
    }

    impl Fixture {
        fn new() -> Result<Self> {
            Self::with_packet(|_| {})
        }
        /// `mutate_packet` runs after digest/length fields are computed, so a
        /// mutation isolates one field (the trusted digest fact is refreshed
        /// for the mutated packet; only the mutation's own proof changes).
        fn with_packet(mutate: impl FnOnce(&mut rb::Packet)) -> Result<Self> {
            let archive = gzip(&tar_bytes(b"ub-review", b'0', b""))?;
            let checksum = format!("{}  dist/{}\n", rb::sha256(&archive), rb::ASSET).into_bytes();
            let mut packet = rb::Packet {
                schema: rb::SCHEMA.into(),
                repository: "EffortlessMetrics/ub-review".into(),
                head_sha: "a".repeat(40),
                tree_sha: "b".repeat(40),
                workflow: ".github/workflows/release-binary.yml".into(),
                producer_ref: "refs/heads/prepared".into(),
                run_id: 100,
                run_attempt: 2,
                artifact_id: 200,
                artifact_name: "ub-review-linux-x64-release".into(),
                proposed_tag: "v0.1.3".into(),
                peeled_commit: "a".repeat(40),
                asset: rb::ASSET.into(),
                checksum_asset: rb::CHECKSUM_ASSET.into(),
                archive_length: archive.len() as u64,
                checksum_length: checksum.len() as u64,
                archive_sha256: rb::sha256(&archive),
                checksum_sha256: rb::sha256(&checksum),
                binary_version: "0.1.3".into(),
                package_version: "0.1.3".into(),
                target: "x86_64-unknown-linux-gnu".into(),
                toolchain: "1.95.0".into(),
                retained_until: 500,
                authorization_record: RECORD.into(),
                verifier: VERIFIER.into(),
            };
            mutate(&mut packet);
            let facts = TrustedFacts {
                repository: packet.repository.clone(),
                workflow: packet.workflow.clone(),
                head_sha: packet.head_sha.clone(),
                tree_sha: packet.tree_sha.clone(),
                producer_ref: packet.producer_ref.clone(),
                run_id: packet.run_id,
                run_attempt: packet.run_attempt,
                artifact_id: packet.artifact_id,
                artifact_name: packet.artifact_name.clone(),
                retained_until: 500,
                proposed_tag: packet.proposed_tag.clone(),
                package_version: packet.package_version.clone(),
                authorization_record: packet.authorization_record.clone(),
                authorization_verifier: packet.verifier.clone(),
                packet_sha256: rb::packet_digest(&packet)
                    .map_err(|reason| anyhow::anyhow!("digest: {reason:?}"))?,
                authorization_expires_at: 600,
                now: 100,
            };
            Ok(Self {
                packet,
                archive,
                checksum,
                notes: "# ub-review v0.1.3\n\nRendered offline during preparation.\n".into(),
                facts,
            })
        }
        fn packet_bytes(&self) -> Result<Vec<u8>> {
            serde_json::to_vec(&self.packet).map_err(Into::into)
        }
        fn staged(&self) -> Result<StagedCandidate> {
            Ok(StagedCandidate {
                packet: self.packet_bytes()?,
                archive: self.archive.clone(),
                checksum: self.checksum.clone(),
                notes: self.notes.clone(),
            })
        }
    }

    #[derive(Default)]
    struct RecordingPublisher {
        api_up: bool,
        /// Simulates a tag ref created concurrently between the survey and
        /// the atomic create: the survey still reports a clean state.
        race_tag_created: bool,
        /// Simulates the created ref not resolving to the admitted commit.
        wrong_tag_target: bool,
        tag_refs: Vec<String>,
        release_assets: Option<Vec<String>>,
        surveys: Vec<String>,
        create_tags: Vec<(String, String)>,
        tag_verifies: Vec<(String, String)>,
        release_creates: Vec<ReleaseRequest>,
    }

    impl RecordingPublisher {
        fn authorized() -> Self {
            Self {
                api_up: true,
                ..Self::default()
            }
        }
        fn external_snapshot(&self) -> (Vec<String>, Option<Vec<String>>) {
            (self.tag_refs.clone(), self.release_assets.clone())
        }
    }

    impl ReleasePublisher for RecordingPublisher {
        fn survey(&mut self, repository: &str, tag: &str) -> Result<ExternalState, PublishStop> {
            if !self.api_up {
                return Err(PublishStop::ApiOutage {
                    detail: "fixture API outage".into(),
                });
            }
            self.surveys.push(format!("{repository}#{tag}"));
            Ok(ExternalState {
                tag_refs: self.tag_refs.clone(),
                release_assets: self.release_assets.clone(),
            })
        }
        fn create_tag(
            &mut self,
            repository: &str,
            tag: &str,
            target: &str,
        ) -> Result<(), PublishStop> {
            self.create_tags
                .push((repository.to_owned(), format!("{tag}@{target}")));
            if self.race_tag_created {
                return Err(PublishStop::OccupiedTag {
                    refs: vec![format!("refs/tags/{tag}")],
                    detail: "fixture: ref appeared concurrently".into(),
                });
            }
            Ok(())
        }
        fn verify_tag(
            &mut self,
            repository: &str,
            tag: &str,
            target: &str,
        ) -> Result<(), PublishStop> {
            self.tag_verifies
                .push((repository.to_owned(), format!("{tag}@{target}")));
            if self.wrong_tag_target {
                return Err(PublishStop::TagTargetMismatch {
                    expected: target.to_owned(),
                    observed: "d".repeat(40),
                });
            }
            Ok(())
        }
        fn create_release(&mut self, request: &ReleaseRequest) -> Result<(), PublishStop> {
            self.release_creates.push(request.clone());
            self.release_assets = Some(vec![
                request.archive_name.clone(),
                request.checksum_name.clone(),
            ]);
            Ok(())
        }
    }

    fn assert_zero_side_effects(publisher: &RecordingPublisher) {
        assert!(
            publisher.release_creates.is_empty(),
            "expected zero release-create calls, got {:?}",
            publisher.release_creates
        );
        assert!(
            publisher.create_tags.is_empty(),
            "expected zero tag-create calls, got {:?}",
            publisher.create_tags
        );
    }

    #[test]
    fn authorized_candidate_produces_exactly_one_byte_identical_publication() -> Result<()> {
        // Acceptance #1344 item 2: valid frozen candidate + trusted exact
        // authorization produce one recording-publisher invocation with
        // byte-identical archive/checksum digests.
        let fixture = Fixture::new()?;
        let staged = fixture.staged()?;
        let facts = clone_facts(&fixture.facts);
        let mut publisher = RecordingPublisher::authorized();
        decide_and_publish(staged, facts, true, &mut publisher)?;
        assert_eq!(publisher.surveys.len(), 1);
        assert_eq!(publisher.surveys[0], "EffortlessMetrics/ub-review#v0.1.3");
        assert_eq!(publisher.create_tags.len(), 1);
        assert_eq!(
            publisher.create_tags[0],
            (
                "EffortlessMetrics/ub-review".to_owned(),
                format!("v0.1.3@{}", fixture.packet.head_sha)
            )
        );
        assert_eq!(publisher.tag_verifies.len(), 1);
        assert_eq!(publisher.release_creates.len(), 1);
        let created = &publisher.release_creates[0];
        assert_eq!(created.tag, "v0.1.3");
        assert_eq!(created.title, "v0.1.3");
        assert_eq!(created.target_commit, fixture.packet.head_sha);
        assert_eq!(created.repository, fixture.packet.repository);
        assert_eq!(created.archive_name, rb::ASSET);
        assert_eq!(created.checksum_name, rb::CHECKSUM_ASSET);
        assert_eq!(created.archive, fixture.archive);
        assert_eq!(created.checksum, fixture.checksum);
        assert_eq!(created.notes, fixture.notes);
        // Unauthorized replay of the same authorization after success: the
        // release now exists, so the second attempt stops with zero creates.
        let replay_request = publisher.release_creates[0].clone();
        let replay = match publisher.publish(&replay_request) {
            Ok(()) => bail!("unauthorized replay must stop on the occupied release"),
            Err(stop) => stop,
        };
        assert_eq!(
            replay,
            PublishStop::OccupiedRelease {
                assets: vec![rb::ASSET.to_owned(), rb::CHECKSUM_ASSET.to_owned()]
            }
        );
        assert_eq!(publisher.release_creates.len(), 1);
        assert_eq!(publisher.create_tags.len(), 1);
        Ok(())
    }

    fn clone_facts(facts: &TrustedFacts) -> TrustedFacts {
        TrustedFacts {
            repository: facts.repository.clone(),
            workflow: facts.workflow.clone(),
            head_sha: facts.head_sha.clone(),
            tree_sha: facts.tree_sha.clone(),
            producer_ref: facts.producer_ref.clone(),
            run_id: facts.run_id,
            run_attempt: facts.run_attempt,
            artifact_id: facts.artifact_id,
            artifact_name: facts.artifact_name.clone(),
            retained_until: facts.retained_until,
            proposed_tag: facts.proposed_tag.clone(),
            package_version: facts.package_version.clone(),
            authorization_record: facts.authorization_record.clone(),
            authorization_verifier: facts.authorization_verifier.clone(),
            packet_sha256: facts.packet_sha256.clone(),
            authorization_expires_at: facts.authorization_expires_at,
            now: facts.now,
        }
    }

    #[test]
    fn every_negative_admission_produces_zero_create_calls() -> Result<()> {
        // Acceptance #1344 item 3: wrong producer/tag/version facts, expired
        // artifact or authorization, missing or untrusted authorization and
        // tampering all refuse admission with zero create/upload calls.
        type FactMutation = fn(&mut TrustedFacts);
        type PacketMutation = fn(&mut rb::Packet);
        type FactCase = (&'static str, FactMutation, rb::Rejection);
        type PacketCase = (&'static str, PacketMutation, rb::Rejection);
        let fact_cases: &[FactCase] = &[
            (
                "wrong head sha",
                |facts| facts.head_sha = "c".repeat(40),
                rb::Rejection::Identity("head_sha"),
            ),
            (
                "wrong tree sha",
                |facts| facts.tree_sha = "c".repeat(40),
                rb::Rejection::Identity("tree_sha"),
            ),
            (
                "wrong producer ref",
                |facts| facts.producer_ref = "refs/heads/other".into(),
                rb::Rejection::Identity("producer_ref"),
            ),
            (
                "wrong run id",
                |facts| facts.run_id += 1,
                rb::Rejection::Identity("run_id"),
            ),
            (
                "wrong run attempt",
                |facts| facts.run_attempt += 1,
                rb::Rejection::Identity("run_attempt"),
            ),
            (
                "wrong artifact id",
                |facts| facts.artifact_id += 1,
                rb::Rejection::Identity("artifact_id"),
            ),
            (
                "wrong artifact name",
                |facts| facts.artifact_name = "other".into(),
                rb::Rejection::Identity("artifact_name"),
            ),
            (
                "artifact horizon below the claimed validity window",
                |facts| facts.retained_until = 499,
                rb::Rejection::Identity("retained_until"),
            ),
            (
                "expired candidate packet",
                |facts| facts.now = 500,
                rb::Rejection::Expired,
            ),
            (
                "expired authorization",
                |facts| facts.authorization_expires_at = 100,
                rb::Rejection::Expired,
            ),
            (
                "authorization digest mismatch",
                |facts| facts.packet_sha256 = "0".repeat(64),
                rb::Rejection::AuthorizationMismatch,
            ),
            (
                "authorization record mismatch",
                |facts| facts.authorization_record = "other-record".into(),
                rb::Rejection::AuthorizationMismatch,
            ),
            (
                "authorization verifier mismatch",
                |facts| facts.authorization_verifier = "other-verifier".into(),
                rb::Rejection::AuthorizationMismatch,
            ),
            (
                "missing authorization inputs",
                |facts| {
                    facts.authorization_record = String::new();
                    facts.authorization_verifier = String::new();
                    facts.packet_sha256 = String::new();
                },
                rb::Rejection::AuthorizationMismatch,
            ),
            (
                "wrong independently observed package version",
                |facts| facts.package_version = "0.1.4".into(),
                rb::Rejection::Identity("package_version"),
            ),
            (
                "wrong proposed tag",
                |facts| facts.proposed_tag = "v0.1.4".into(),
                rb::Rejection::Identity("unused_tag"),
            ),
        ];
        for (name, mutate, expected) in fact_cases {
            let fixture = Fixture::new()?;
            let staged = fixture.staged()?;
            let mut facts = clone_facts(&fixture.facts);
            mutate(&mut facts);
            let mut publisher = RecordingPublisher::authorized();
            let error = match decide_and_publish(staged, facts, true, &mut publisher) {
                Ok(()) => bail!("negative case `{name}` unexpectedly published"),
                Err(error) => error,
            };
            assert_zero_side_effects(&publisher);
            let message = format!("{error:#}");
            assert!(
                message.contains(&format!("{expected:?}")),
                "case `{name}` refused with `{message}`, expected {expected:?}"
            );
        }
        let packet_cases: &[PacketCase] = &[
            (
                "unsupported candidate packet schema",
                |packet| packet.schema = "ub-review.release_candidate.v1".into(),
                rb::Rejection::UnsupportedSchema,
            ),
            (
                "tampered binary version claim",
                |packet| packet.binary_version = "0.1.4".into(),
                rb::Rejection::Identity("version"),
            ),
            (
                "tampered archive digest claim",
                |packet| packet.archive_sha256 = "c".repeat(64),
                rb::Rejection::ArchiveDigest,
            ),
            (
                "tampered checksum digest claim",
                |packet| packet.checksum_sha256 = "c".repeat(64),
                rb::Rejection::ChecksumDigest,
            ),
            (
                "validity window past the observed artifact horizon",
                |packet| packet.retained_until = 501,
                rb::Rejection::Identity("retained_until"),
            ),
        ];
        for (name, mutate, expected) in packet_cases {
            let fixture = Fixture::with_packet(*mutate)?;
            let staged = fixture.staged()?;
            let facts = clone_facts(&fixture.facts);
            let mut publisher = RecordingPublisher::authorized();
            let error = match decide_and_publish(staged, facts, true, &mut publisher) {
                Ok(()) => bail!("packet case `{name}` unexpectedly published"),
                Err(error) => error,
            };
            assert_zero_side_effects(&publisher);
            let message = format!("{error:#}");
            assert!(
                message.contains(&format!("{expected:?}")),
                "packet case `{name}` refused with `{message}`, expected {expected:?}"
            );
        }
        // Tampering with the staged bytes after packaging: the packet still
        // binds the original digests, so admission rejects the mutation.
        let fixture = Fixture::new()?;
        let mut staged = fixture.staged()?;
        staged.archive[0] ^= 1;
        let mut publisher = RecordingPublisher::authorized();
        let error =
            match decide_and_publish(staged, clone_facts(&fixture.facts), true, &mut publisher) {
                Ok(()) => bail!("tampered archive unexpectedly published"),
                Err(error) => error,
            };
        assert_zero_side_effects(&publisher);
        assert!(
            format!("{error:#}").contains(&format!("{:?}", rb::Rejection::ArchiveDigest)),
            "tampered archive refused with `{error:#}`"
        );
        let fixture = Fixture::new()?;
        let mut staged = fixture.staged()?;
        staged.checksum[0] ^= 1;
        let mut publisher = RecordingPublisher::authorized();
        let error =
            match decide_and_publish(staged, clone_facts(&fixture.facts), true, &mut publisher) {
                Ok(()) => bail!("tampered checksum unexpectedly published"),
                Err(error) => error,
            };
        assert_zero_side_effects(&publisher);
        assert!(
            format!("{error:#}").contains(&format!("{:?}", rb::Rejection::ChecksumDigest)),
            "tampered checksum refused with `{error:#}`"
        );
        Ok(())
    }

    #[test]
    fn publisher_stops_on_collision_outage_and_replays_without_mutation() -> Result<()> {
        // Acceptance #1344 item 5: historical tag/release/asset collision, API
        // outage and unauthorized replay stop with zero create calls and an
        // unchanged external state snapshot.
        let fixture = Fixture::new()?;
        let staged = fixture.staged()?;
        let facts = clone_facts(&fixture.facts);
        let mut publisher = RecordingPublisher::authorized();
        publisher.tag_refs = vec!["refs/tags/v0.1.3".to_owned()];
        let error =
            match decide_and_publish(staged.clone(), clone_facts(&facts), true, &mut publisher) {
                Ok(()) => bail!("occupied tag must stop"),
                Err(error) => error,
            };
        assert_eq!(
            error.to_string(),
            "publish stopped without overwrite: OccupiedTag { refs: [\"refs/tags/v0.1.3\"], detail: \"survey observed an occupied tag before any create\" }"
        );
        assert_zero_side_effects(&publisher);
        assert_eq!(publisher.surveys.len(), 1);

        let mut publisher = RecordingPublisher::authorized();
        publisher.release_assets = Some(vec![
            rb::ASSET.to_owned(),
            rb::CHECKSUM_ASSET.to_owned(),
            "legacy-asset".to_owned(),
        ]);
        let error =
            match decide_and_publish(staged.clone(), clone_facts(&facts), true, &mut publisher) {
                Ok(()) => bail!("occupied release must stop"),
                Err(error) => error,
            };
        assert!(
            error.to_string().contains("OccupiedRelease"),
            "occupied release stopped with `{error:#}`"
        );
        assert_zero_side_effects(&publisher);

        let mut publisher = RecordingPublisher::authorized();
        publisher.api_up = false;
        let error =
            match decide_and_publish(staged.clone(), clone_facts(&facts), true, &mut publisher) {
                Ok(()) => bail!("API outage must stop"),
                Err(error) => error,
            };
        assert!(
            error.to_string().contains("ApiOutage"),
            "API outage stopped with `{error:#}`"
        );
        assert_zero_side_effects(&publisher);
        Ok(())
    }

    #[test]
    fn concurrent_tag_creation_is_never_adopted() -> Result<()> {
        // The survey can be clean while a historical tag ref appears before
        // the atomic create. The tag-ref create must fail on the existing ref
        // so nothing is adopted, and no release or asset may exist yet.
        let fixture = Fixture::new()?;
        let staged = fixture.staged()?;
        let facts = clone_facts(&fixture.facts);
        let mut publisher = RecordingPublisher::authorized();
        publisher.race_tag_created = true;
        let error = match decide_and_publish(staged, facts, true, &mut publisher) {
            Ok(()) => bail!("concurrent tag must stop the publish path"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("OccupiedTag"),
            "concurrent tag stopped with `{error:#}`"
        );
        assert_eq!(publisher.surveys.len(), 1);
        assert_eq!(publisher.create_tags.len(), 1);
        assert!(publisher.tag_verifies.is_empty());
        assert!(publisher.release_creates.is_empty());
        assert_eq!(publisher.release_assets, None);
        Ok(())
    }

    #[test]
    fn wrong_tag_target_stops_before_any_release_exists() -> Result<()> {
        // The created ref must resolve to the admitted commit before the
        // release is created; a mismatch stops with zero release/asset
        // effects and the mismatch reported for explicit recovery.
        let fixture = Fixture::new()?;
        let staged = fixture.staged()?;
        let facts = clone_facts(&fixture.facts);
        let mut publisher = RecordingPublisher::authorized();
        publisher.wrong_tag_target = true;
        let error = match decide_and_publish(staged, facts, true, &mut publisher) {
            Ok(()) => bail!("wrong tag target must stop the publish path"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("TagTargetMismatch"),
            "wrong target stopped with `{error:#}`"
        );
        assert_eq!(publisher.create_tags.len(), 1);
        assert_eq!(publisher.tag_verifies.len(), 1);
        assert!(publisher.release_creates.is_empty());
        assert_eq!(publisher.release_assets, None);
        Ok(())
    }

    #[test]
    fn partial_publication_requires_explicit_recovery_and_never_clobbers() -> Result<()> {
        // Acceptance #1344 item 7: a partially uploaded release retains exact
        // external state across retries; the publish path refuses and no
        // recovery (upload/delete/clobber) is attempted by the publisher.
        let fixture = Fixture::new()?;
        let staged = fixture.staged()?;
        let facts = clone_facts(&fixture.facts);
        let mut publisher = RecordingPublisher::authorized();
        publisher.release_assets = Some(vec![rb::ASSET.to_owned()]);
        let before = publisher.external_snapshot();
        for attempt in 0..2 {
            let error =
                match decide_and_publish(staged.clone(), clone_facts(&facts), true, &mut publisher)
                {
                    Ok(()) => bail!("partial retry {attempt} unexpectedly published"),
                    Err(error) => error,
                };
            assert!(
                error.to_string().contains("PartialPublication"),
                "attempt {attempt} stopped with `{error:#}`"
            );
            assert_zero_side_effects(&publisher);
            assert_eq!(publisher.external_snapshot(), before);
            assert_eq!(publisher.surveys.len(), attempt + 1);
        }
        Ok(())
    }

    #[test]
    fn captured_bytes_cannot_change_after_validation() -> Result<()> {
        // Acceptance #1344 item 4: publication consumes the validator's
        // privately captured slices; mutating the staged input afterwards
        // cannot change what would be uploaded, and mutated staging refuses.
        let fixture = Fixture::new()?;
        let staged = fixture.staged()?;
        let verified = resolve_publication(&staged, &fixture.facts)
            .map_err(|reason| anyhow::anyhow!("fixture must admit, got {reason:?}"))?;
        let captured_archive = verified.archive().to_vec();
        let captured_checksum = verified.checksum().to_vec();
        let mut mutated = fixture.staged()?;
        mutated.archive[0] ^= 1;
        mutated.checksum[0] ^= 1;
        assert_ne!(mutated.archive, captured_archive);
        assert_ne!(mutated.checksum, captured_checksum);
        assert_eq!(verified.archive(), &captured_archive[..]);
        assert_eq!(verified.checksum(), &captured_checksum[..]);
        // The mutated staging bytes are no longer the admitted bytes: the same
        // facts refuse the re-read staging with zero effects.
        let mut publisher = RecordingPublisher::authorized();
        let error =
            match decide_and_publish(mutated, clone_facts(&fixture.facts), true, &mut publisher) {
                Ok(()) => bail!("mutated staging unexpectedly published"),
                Err(error) => error,
            };
        assert!(
            format!("{error:#}").contains(&format!("{:?}", rb::Rejection::ArchiveDigest)),
            "mutated staging refused with `{error:#}`"
        );
        assert_zero_side_effects(&publisher);
        Ok(())
    }

    #[test]
    fn dry_run_decision_performs_no_publish_call() -> Result<()> {
        let fixture = Fixture::new()?;
        let staged = fixture.staged()?;
        let mut publisher = RecordingPublisher::authorized();
        decide_and_publish(staged, clone_facts(&fixture.facts), false, &mut publisher)?;
        assert!(publisher.surveys.is_empty());
        assert_zero_side_effects(&publisher);
        Ok(())
    }

    #[test]
    fn run_with_publisher_reads_staging_files_and_refuses_tampering() -> Result<()> {
        let fixture = Fixture::new()?;
        let staging = write_staging_dir(&fixture)?;
        let options = ValidateReleaseOptions {
            staging,
            repository: fixture.facts.repository.clone(),
            workflow: fixture.facts.workflow.clone(),
            head_sha: fixture.facts.head_sha.clone(),
            tree_sha: fixture.facts.tree_sha.clone(),
            producer_ref: fixture.facts.producer_ref.clone(),
            run_id: fixture.facts.run_id,
            run_attempt: fixture.facts.run_attempt,
            artifact_id: fixture.facts.artifact_id,
            artifact_name: fixture.facts.artifact_name.clone(),
            retained_until: fixture.facts.retained_until,
            proposed_tag: fixture.facts.proposed_tag.clone(),
            package_version: fixture.facts.package_version.clone(),
            authorization_record: fixture.facts.authorization_record.clone(),
            authorization_verifier: fixture.facts.authorization_verifier.clone(),
            packet_sha256: fixture.facts.packet_sha256.clone(),
            authorization_expires_at: fixture.facts.authorization_expires_at,
            now: fixture.facts.now,
            publish: false,
        };
        let mut publisher = RecordingPublisher::authorized();
        run_with_publisher(Path::new("/"), options.clone(), &mut publisher)?;
        assert_zero_side_effects(&publisher);
        // A wrong trusted fact refuses through the same file-backed path.
        let mut wrong = options.clone();
        wrong.head_sha = "c".repeat(40);
        let error = match run_with_publisher(Path::new("/"), wrong, &mut publisher) {
            Ok(()) => bail!("wrong head sha must refuse"),
            Err(error) => error,
        };
        assert!(
            format!("{error:#}").contains(&format!("{:?}", rb::Rejection::Identity("head_sha"))),
            "refusal carried `{error:#}`"
        );
        assert_zero_side_effects(&publisher);
        let staging = &options.staging;
        let _ = fs::remove_dir_all(staging);
        Ok(())
    }

    fn write_staging_dir(fixture: &Fixture) -> Result<PathBuf> {
        let nanos = epoch_nanos()?;
        let dir = std::env::temp_dir().join(format!(
            "ub-review-validate-release-test-{}-{nanos}",
            std::process::id()
        ));
        fs::create_dir_all(&dir)?;
        fs::write(dir.join(PACKET_FILE), fixture.packet_bytes()?)?;
        fs::write(dir.join(rb::ASSET), &fixture.archive)?;
        fs::write(dir.join(rb::CHECKSUM_ASSET), &fixture.checksum)?;
        fs::write(dir.join(NOTES_FILE), &fixture.notes)?;
        Ok(dir)
    }

    #[test]
    fn validate_release_options_parse_requires_every_trusted_fact() -> Result<()> {
        let valid: Vec<String> = vec![
            "--staging".to_owned(),
            "staging".to_owned(),
            "--repository".to_owned(),
            "EffortlessMetrics/ub-review".to_owned(),
            "--workflow".to_owned(),
            ".github/workflows/release-binary.yml".to_owned(),
            "--head-sha".to_owned(),
            "a".repeat(40),
            "--tree-sha".to_owned(),
            "b".repeat(40),
            "--producer-ref".to_owned(),
            "refs/heads/prepared".to_owned(),
            "--run-id".to_owned(),
            "100".to_owned(),
            "--run-attempt".to_owned(),
            "2".to_owned(),
            "--artifact-id".to_owned(),
            "200".to_owned(),
            "--artifact-name".to_owned(),
            "ub-review-linux-x64-release".to_owned(),
            "--retained-until".to_owned(),
            "500".to_owned(),
            "--proposed-tag".to_owned(),
            "v0.1.3".to_owned(),
            "--package-version".to_owned(),
            "0.1.3".to_owned(),
            "--authorization-record".to_owned(),
            RECORD.to_owned(),
            "--authorization-verifier".to_owned(),
            VERIFIER.to_owned(),
            "--packet-sha256".to_owned(),
            "e".repeat(64),
            "--authorization-expires-at".to_owned(),
            "600".to_owned(),
            "--now".to_owned(),
            "100".to_owned(),
        ];
        let parsed = ValidateReleaseOptions::parse(valid.iter().cloned())?;
        assert!(!parsed.publish);
        assert_eq!(parsed.run_id, 100);
        let with_publish = {
            let mut args = valid.clone();
            args.push("--publish".to_owned());
            ValidateReleaseOptions::parse(args.into_iter())?
        };
        assert!(with_publish.publish);
        for truncate in [2usize, 10, 20] {
            let error = match ValidateReleaseOptions::parse(valid[..truncate].iter().cloned()) {
                Ok(_) => bail!("truncated input unexpectedly parsed"),
                Err(error) => error,
            };
            assert!(
                format!("{error:#}").contains("missing required"),
                "truncated input reported `{error:#}`"
            );
        }
        let error = match ValidateReleaseOptions::parse(
            valid.iter().cloned().chain(["--unknown-flag".to_owned()]),
        ) {
            Ok(_) => bail!("unknown flag unexpectedly parsed"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("--unknown-flag"));
        let bad_number = {
            let mut args = valid.clone();
            args.truncate(args.len() - 2);
            args.push("--run-id".to_owned());
            args.push("not-a-number".to_owned());
            args
        };
        let error = match ValidateReleaseOptions::parse(bad_number.into_iter()) {
            Ok(_) => bail!("non-numeric id unexpectedly parsed"),
            Err(error) => error,
        };
        assert!(format!("{error:#}").contains("non-negative integer"));
        Ok(())
    }

    #[test]
    fn packet_digest_command_derives_the_authorization_digest() -> Result<()> {
        let fixture = Fixture::new()?;
        let dir = write_staging_dir(&fixture)?;
        let options = PacketDigestOptions {
            packet: dir.join(PACKET_FILE),
        };
        run_packet_digest(Path::new("/"), options)?;
        let packet = rb::parse_packet(&fixture.packet_bytes()?)
            .map_err(|reason| anyhow::anyhow!("fixture packet rejected: {reason:?}"))?;
        let digest =
            rb::packet_digest(&packet).map_err(|reason| anyhow::anyhow!("digest: {reason:?}"))?;
        assert_eq!(digest, fixture.facts.packet_sha256);
        // A tampered packet is rejected before any digest is printed.
        let tampered_path = dir.join("tampered-packet.json");
        let mut tampered = fixture.packet_bytes()?;
        tampered.truncate(tampered.len() - 2);
        fs::write(&tampered_path, tampered)?;
        let error = match run_packet_digest(
            Path::new("/"),
            PacketDigestOptions {
                packet: tampered_path,
            },
        ) {
            Ok(()) => bail!("tampered packet unexpectedly digested"),
            Err(error) => error,
        };
        assert!(
            format!("{error:#}").contains("rejected"),
            "tampered packet reported `{error:#}`"
        );
        fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn stage_private_bytes_writes_exactly_the_captured_bytes() -> Result<()> {
        let request = ReleaseRequest {
            repository: "EffortlessMetrics/ub-review".into(),
            tag: "v0.1.3".into(),
            target_commit: "a".repeat(40),
            title: "v0.1.3".into(),
            notes: "notes".into(),
            archive_name: rb::ASSET.into(),
            archive: vec![1, 2, 3, 4],
            checksum_name: rb::CHECKSUM_ASSET.into(),
            checksum: vec![5, 6, 7],
        };
        let staging = stage_private_bytes(&request)
            .map_err(|stop| anyhow::anyhow!("private staging failed: {stop:?}"))?;
        assert_eq!(fs::read(&staging.archive)?, request.archive);
        assert_eq!(fs::read(&staging.checksum)?, request.checksum);
        assert_eq!(fs::read_to_string(&staging.notes)?, request.notes);
        fs::remove_dir_all(&staging.dir)?;
        Ok(())
    }
}
