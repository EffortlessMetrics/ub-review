//! Offline source joins for the historical CI corpus; no cost or gate authority.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_BYTES: u64 = 1_048_576;
const MAX_FILE: u64 = 262_144;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    max_total_bytes: u64,
    incident_reference: IncidentReference,
    gaps: BTreeMap<String, String>,
    cases: Vec<Case>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct IncidentReference {
    path: String,
    sha256: String,
    git_blob: String,
    cases: Vec<Incident>,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct Incident {
    pull_request: u64,
    workflow_run: u64,
    artifact_id: u64,
    artifact_digest: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Case {
    id: String,
    shapes: Vec<String>,
    repository: String,
    pull_request: u64,
    head_revision: String,
    reviewed_revision: String,
    semantics: String,
    run_id: u64,
    attempt: u64,
    artifact_id: u64,
    archive_sha256: String,
    independent_baseline: Option<Baseline>,
    files: Vec<FileReceipt>,
    measurements: Vec<Measurement>,
    duplicate_relationship_status: String,
    omissions: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct FileReceipt {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Baseline {
    run_id: u64,
    artifact_id: u64,
    artifact_digest: String,
    receipt_status: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Measurement {
    name: String,
    unit: String,
    status: String,
    value: Option<u64>,
    basis: String,
    file: Option<String>,
    pointer: Option<String>,
    reason: String,
}

struct Corpus {
    manifest: Manifest,
    documents: BTreeMap<String, Value>,
}

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/ci-efficiency-baseline")
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn hex(text: &str, width: usize) -> bool {
    text.len() == width
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn confined(path: &str) -> Result<()> {
    ensure!(
        !path.is_empty() && !path.contains('\\'),
        "empty or non-portable fixture path"
    );
    ensure!(
        Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_))),
        "fixture path escapes its root"
    );
    Ok(())
}

fn ordinary(path: &Path) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    ensure!(!metadata.file_type().is_symlink(), "linked fixture member");
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        ensure!(
            metadata.file_attributes() & 0x400 == 0,
            "reparse fixture member"
        );
    }
    Ok(metadata)
}

fn privacy(bytes: &[u8]) -> Result<()> {
    let text = std::str::from_utf8(bytes)?.to_ascii_lowercase();
    for marker in [
        "bearer ",
        "authorization:",
        "-----begin private key",
        "github_pat_",
        "ghp_",
        "gho_",
        "ghu_",
        "ghs_",
        "ghr_",
    ] {
        ensure!(!text.contains(marker), "credential-like fixture content");
    }
    Ok(())
}

fn private_keys(value: &Value) -> Result<()> {
    match value {
        Value::Object(object) => {
            for (key, child) in object {
                let key = key.to_ascii_lowercase().replace(['_', '-'], "");
                ensure!(
                    ![
                        "accesstoken",
                        "apikey",
                        "authorization",
                        "password",
                        "privatekey",
                        "clientsecret",
                        "messages",
                        "prompt",
                        "providerrequest"
                    ]
                    .contains(&key.as_str()),
                    "private payload field in corpus"
                );
                private_keys(child)?;
            }
        }
        Value::Array(rows) => {
            for row in rows {
                private_keys(row)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn inventory(
    directory: &Path,
    relative: &Path,
    files: &mut BTreeSet<String>,
    total: &mut u64,
) -> Result<()> {
    ensure!(
        ordinary(directory)?.is_dir(),
        "fixture root is not a directory"
    );
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let metadata = ordinary(&entry.path())?;
        let path = relative.join(entry.file_name());
        if metadata.is_dir() {
            ensure!(
                path.components().count() <= 8,
                "fixture nesting exceeds budget"
            );
            inventory(&entry.path(), &path, files, total)?;
        } else {
            ensure!(
                metadata.is_file() && metadata.len() <= MAX_FILE,
                "unbounded or special fixture file"
            );
            *total = total
                .checked_add(metadata.len())
                .context("fixture byte overflow")?;
            ensure!(*total <= MAX_BYTES, "fixture corpus exceeds byte budget");
            ensure!(
                files.insert(path.to_string_lossy().replace('\\', "/")) && files.len() <= 256,
                "duplicate or excessive fixture inventory"
            );
            privacy(&fs::read(entry.path())?)?;
        }
    }
    Ok(())
}

fn load(directory: &Path) -> Result<Corpus> {
    let mut actual = BTreeSet::new();
    let mut total = 0;
    inventory(directory, Path::new(""), &mut actual, &mut total)?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    ensure!(
        manifest.schema == "ub-review.ci_efficiency_baseline.v1"
            && manifest.max_total_bytes == MAX_BYTES,
        "unsupported fixture contract"
    );
    let mut expected = BTreeSet::from(["README.md".to_owned(), "manifest.json".to_owned()]);
    let mut documents = BTreeMap::new();
    for case in &manifest.cases {
        for file in &case.files {
            confined(&file.path)?;
            ensure!(
                file.path.starts_with(&format!("{}/", case.id)),
                "fixture crosses case ownership"
            );
            ensure!(
                expected.insert(file.path.clone()),
                "duplicate retained file"
            );
            let bytes = fs::read(directory.join(&file.path))?;
            ensure!(
                u64::try_from(bytes.len())? == file.bytes
                    && hex(&file.sha256, 64)
                    && digest(&bytes) == file.sha256,
                "retained byte receipt mismatch: {}",
                file.path
            );
            let document = if file.path.ends_with(".ndjson") {
                let rows = std::str::from_utf8(&bytes)?
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .map(serde_json::from_str)
                    .collect::<std::result::Result<Vec<Value>, _>>()?;
                Value::Array(rows)
            } else {
                ensure!(
                    file.path.ends_with(".json"),
                    "unrecognized retained content"
                );
                serde_json::from_slice(&bytes)?
            };
            private_keys(&document)?;
            documents.insert(file.path.clone(), document);
        }
    }
    ensure!(actual == expected, "unlisted or missing corpus file");
    let corpus = Corpus {
        manifest,
        documents,
    };
    validate(&corpus)?;
    Ok(corpus)
}

fn document<'a>(corpus: &'a Corpus, case: &Case, name: &str) -> Result<&'a Value> {
    corpus
        .documents
        .get(&format!("{}/{name}", case.id))
        .with_context(|| format!("missing {name}"))
}

fn number(value: &Value, pointer: &str) -> Result<u64> {
    value
        .pointer(pointer)
        .and_then(Value::as_u64)
        .with_context(|| format!("missing unsigned {pointer}"))
}

fn string<'a>(value: &'a Value, pointer: &str) -> Result<&'a str> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .with_context(|| format!("missing text {pointer}"))
}

fn rows<'a>(value: &'a Value, pointer: &str) -> Result<&'a [Value]> {
    Ok(value
        .pointer(pointer)
        .and_then(Value::as_array)
        .context("missing rows")?
        .as_slice())
}

fn elapsed(value: &Value, start: &str, end: &str) -> Result<u64> {
    let from = DateTime::parse_from_rfc3339(string(value, start)?)?;
    let to = DateTime::parse_from_rfc3339(string(value, end)?)?;
    u64::try_from((to - from).num_milliseconds()).context("negative source duration")
}

fn validate_jobs(jobs: &Value, case: &Case) -> Result<u64> {
    let entries = rows(jobs, "/jobs")?;
    ensure!(
        number(jobs, "/total_count")? == u64::try_from(entries.len())? && !entries.is_empty(),
        "incomplete jobs metadata"
    );
    ensure!(
        hex(string(jobs, "/source_json_sha256")?, 64),
        "missing pre-projection jobs digest"
    );
    let mut ids = BTreeSet::new();
    let mut job_sum = 0_u64;
    for job in entries {
        ensure!(ids.insert(number(job, "/id")?), "duplicate source job");
        ensure!(
            number(job, "/run_id")? == case.run_id
                && number(job, "/run_attempt")? == case.attempt
                && string(job, "/head_sha")? == case.head_revision,
            "forged job source"
        );
        ensure!(string(job, "/status")? == "completed", "nonterminal job");
        ensure!(
            number(job, "/runner_id")? > 0 && !rows(job, "/labels")?.is_empty(),
            "missing runner observation"
        );
        job_sum = job_sum
            .checked_add(elapsed(job, "/started_at", "/completed_at")?)
            .context("job sum overflow")?;
        let mut steps = BTreeSet::new();
        for step in rows(job, "/steps")? {
            ensure!(
                steps.insert(number(step, "/number")?),
                "duplicate step identity"
            );
            elapsed(step, "/started_at", "/completed_at")?;
        }
    }
    Ok(job_sum)
}

fn validate_zip(zip: &Value, artifact: &Value, case: &Case) -> Result<()> {
    ensure!(
        string(zip, "/archive_sha256")? == case.archive_sha256
            && number(zip, "/archive_bytes")? == number(artifact, "/size_in_bytes")?,
        "ZIP source identity mismatch"
    );
    let mut names = BTreeSet::new();
    let mut expanded = 0_u64;
    let mut packet_expanded = 0_u64;
    let mut compressed = 0_u64;
    for entry in rows(zip, "/entries")? {
        let path = string(entry, "/path")?;
        confined(path)?;
        ensure!(names.insert(path), "duplicate ZIP member");
        ensure!(
            hex(string(entry, "/crc32")?, 8),
            "missing ZIP CRC observation"
        );
        expanded = expanded
            .checked_add(number(entry, "/bytes")?)
            .context("ZIP expansion sum overflow")?;
        if path.starts_with("target/ub-review/") {
            packet_expanded = packet_expanded
                .checked_add(number(entry, "/bytes")?)
                .context("packet expansion sum overflow")?;
        }
        compressed = compressed
            .checked_add(number(entry, "/compressed_bytes")?)
            .context("ZIP compression sum overflow")?;
    }
    ensure!(
        expanded == number(zip, "/expanded_bytes")?
            && compressed == number(zip, "/entry_compressed_bytes")?,
        "ZIP inventory sums disagree"
    );
    ensure!(
        packet_expanded == number(zip, "/packet_expanded_bytes")?,
        "packet-only expansion disagrees"
    );
    ensure!(
        compressed <= number(zip, "/archive_bytes")?,
        "entry bytes exceed ZIP container bytes"
    );
    for file in &case.files {
        if let Some(path) = file.path.strip_prefix(&format!("{}/packet/", case.id)) {
            let source = format!("target/ub-review/{path}");
            let entry = rows(zip, "/entries")?
                .iter()
                .find(|entry| entry.get("path").and_then(Value::as_str) == Some(source.as_str()))
                .context("retained packet file absent from ZIP inventory")?;
            ensure!(
                number(entry, "/bytes")? == file.bytes,
                "retained source size differs from archive inventory"
            );
        }
    }
    Ok(())
}

fn validate_measurements(corpus: &Corpus, case: &Case) -> Result<()> {
    let required = BTreeMap::from([
        ("packet_elapsed_wall", "ms"),
        ("proof_process_sum_reported", "ms"),
        ("model_call_sum_reported", "ms"),
        ("legacy_phase_wall", "ms"),
        ("artifact_zip", "bytes"),
        ("artifact_expanded", "bytes"),
        ("packet_expanded", "bytes"),
        ("artifact_entry_compressed", "bytes"),
        ("billable_estimate", "usd"),
        ("linux_equivalent", "linux_runner_minutes"),
        ("critical_path", "ms"),
        ("queue_split", "ms"),
        ("cache_hits", "count"),
        ("avoided_executions", "count"),
        ("full_process_sum", "ms"),
    ]);
    let mut seen = BTreeSet::new();
    for measurement in &case.measurements {
        ensure!(
            seen.insert(measurement.name.as_str()),
            "duplicate measurement"
        );
        ensure!(
            required.get(measurement.name.as_str()).copied() == Some(measurement.unit.as_str()),
            "unknown measurement or unit mismatch"
        );
        if measurement.status == "not_measured" {
            ensure!(
                measurement.value.is_none()
                    && measurement.file.is_none()
                    && measurement.pointer.is_none()
                    && measurement.basis == "unavailable"
                    && !measurement.reason.trim().is_empty(),
                "unknown measurement is not explicit"
            );
        } else {
            ensure!(
                measurement.reason.is_empty()
                    && matches!(
                        (measurement.status.as_str(), measurement.basis.as_str()),
                        ("reported", "json_pointer") | ("derived", "zip_inventory")
                    ),
                "invalid measurement basis"
            );
            let file = measurement
                .file
                .as_ref()
                .context("missing measurement source")?;
            ensure!(
                case.files.iter().any(|row| &row.path == file),
                "measurement crosses source ownership"
            );
            let source = corpus
                .documents
                .get(file)
                .context("measurement source missing")?;
            let pointer = measurement
                .pointer
                .as_deref()
                .context("measurement pointer missing")?;
            ensure!(
                measurement.value == Some(number(source, pointer)?),
                "measurement differs from its source"
            );
        }
    }
    ensure!(
        seen == required.keys().copied().collect(),
        "missing required measurement or unknown"
    );
    ensure!(
        case.duplicate_relationship_status == "unproven_equivalence",
        "candidate equivalence promoted into authority"
    );
    for name in [
        "billable_estimate",
        "linux_equivalent",
        "critical_path",
        "queue_split",
        "cache_hits",
        "avoided_executions",
        "full_process_sum",
    ] {
        ensure!(
            case.measurements
                .iter()
                .any(|row| row.name == name && row.status == "not_measured"),
            "unmeasured economics promoted to measured value"
        );
    }
    Ok(())
}

fn validate(corpus: &Corpus) -> Result<()> {
    let mut ids = BTreeSet::new();
    let mut runs = BTreeSet::new();
    for case in &corpus.manifest.cases {
        ensure!(
            ids.insert(case.id.as_str()) && runs.insert(case.run_id),
            "duplicate case or source run"
        );
        ensure!(
            case.id == case.pull_request.to_string()
                && case.repository == "EffortlessMetrics/ub-review"
                && case.attempt > 0,
            "invalid case identity"
        );
        ensure!(
            hex(&case.head_revision, 40)
                && hex(&case.reviewed_revision, 40)
                && hex(&case.archive_sha256, 64),
            "invalid source hash"
        );
        ensure!(
            !case.shapes.is_empty() && !case.omissions.is_empty(),
            "missing shape or omission boundary"
        );
        let run = document(corpus, case, "github-run.json")?;
        ensure!(
            number(run, "/id")? == case.run_id
                && number(run, "/run_attempt")? == case.attempt
                && string(run, "/head_sha")? == case.head_revision
                && string(run, "/repository")? == case.repository,
            "forged run source"
        );
        ensure!(
            number(run, "/workflow_id")? == 289600864
                && string(run, "/path")? == ".github/workflows/ub-review-gate.yml"
                && hex(string(run, "/source_json_sha256")?, 64),
            "missing workflow source"
        );
        let jobs = document(corpus, case, "github-jobs.json")?;
        let job_sum = validate_jobs(jobs, case)?;
        let artifact = document(corpus, case, "github-artifact.json")?;
        ensure!(
            number(artifact, "/id")? == case.artifact_id
                && number(artifact, "/workflow_run/id")? == case.run_id
                && string(artifact, "/workflow_run/head_sha")? == case.head_revision
                && string(artifact, "/digest")? == format!("sha256:{}", case.archive_sha256),
            "forged artifact source"
        );
        let admission = document(corpus, case, "packet/input/revision-admission.json")?;
        ensure!(
            string(admission, "/pr_head_commit")? == case.head_revision
                && string(admission, "/reviewed_commit_oid")? == case.reviewed_revision
                && string(admission, "/semantics")? == case.semantics,
            "source admission disagrees"
        );
        ensure!(
            matches!(case.semantics.as_str(), "candidate_head" | "merge_result"),
            "unknown revision semantics"
        );
        let canonical = string(admission, "/identity_canonical")?;
        let identity = digest(
            &[
                b"ub-review.revision-identity.digest.v1\0".as_slice(),
                canonical.as_bytes(),
            ]
            .concat(),
        );
        ensure!(
            string(admission, "/identity_digest")? == identity,
            "admission digest mismatch"
        );
        let gate = document(corpus, case, "packet/review/gate_outcome.json")?;
        ensure!(
            string(gate, "/revision/digest")? == identity
                && string(gate, "/revision/reviewed_commit")? == case.reviewed_revision,
            "gate not bound to source admission"
        );
        let metrics = document(corpus, case, "packet/review/metrics.json")?;
        ensure!(
            string(metrics, "/model_mode")? == "off",
            "model-on source in model-off corpus"
        );
        if case.id == "1264" {
            ensure!(
                metrics
                    .pointer("/diff_flags/docs_only")
                    .and_then(Value::as_bool)
                    == Some(true),
                "doc-only shape is not source supported"
            );
        }
        if case.id == "1261" {
            ensure!(
                metrics
                    .pointer("/diff_flags/rust_tests_changed")
                    .and_then(Value::as_bool)
                    == Some(true),
                "changed-test shape is not source supported"
            );
        }
        if case.id == "1263" {
            let baseline = case
                .independent_baseline
                .as_ref()
                .context("missing historical baseline status")?;
            ensure!(
                baseline.run_id == 33140109246
                    && baseline.artifact_id == 9673611284
                    && baseline.artifact_digest
                        == "sha256:eb06357f83531ad426f3d511496c22e5bd400e090b065ebd6dcaf865da9a49fb"
                    && baseline.receipt_status == "unavailable_expired_http_410",
                "expired baseline receipt promoted or misidentified"
            );
            let checks = document(corpus, case, "baseline-checks.json")?;
            let mut names = BTreeSet::new();
            for check in rows(checks, "/checks")? {
                ensure!(
                    string(check, "/head_sha")? == case.head_revision
                        && string(check, "/conclusion")? == "success"
                        && string(check, "/status")? == "completed",
                    "baseline check identity or conclusion drift"
                );
                ensure!(
                    string(check, "/details_url")?.starts_with(&format!(
                        "https://github.com/{}/actions/runs/{}/job/",
                        case.repository, baseline.run_id
                    )),
                    "baseline check belongs to another run"
                );
                ensure!(
                    names.insert(string(check, "/name")?),
                    "duplicate baseline check"
                );
            }
            ensure!(
                names
                    == BTreeSet::from([
                        "ub-review/independent-baseline",
                        "independent evidence / fmt",
                        "independent evidence / check",
                        "independent evidence / clippy",
                        "independent evidence / test",
                        "independent evidence / doc",
                        "independent evidence / policy",
                        "independent evidence / verifier"
                    ]),
                "missing baseline check observation"
            );
            let coverage = document(
                corpus,
                case,
                "packet/sensors/coverage/ub-review-sensor-status.json",
            )?;
            ensure!(
                string(coverage, "/status")? == "failed"
                    && number(coverage, "/exit_code")? == 101
                    && coverage.get("required").and_then(Value::as_bool) == Some(false),
                "reported optional sensor failure was erased"
            );
        }
        validate_zip(
            document(corpus, case, "zip-inventory.json")?,
            artifact,
            case,
        )?;
        validate_measurements(corpus, case)?;
        println!(
            "PR {}: job elapsed sum {} ms (not billable, critical path, or process time)",
            case.id, job_sum
        );
    }
    ensure!(
        ids == BTreeSet::from(["1261", "1263", "1264", "1266"]),
        "required representative source is missing"
    );
    ensure!(
        corpus
            .manifest
            .gaps
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            == BTreeSet::from(["multi_package", "second_repository", "cost_authority"]),
        "missing corpus gap"
    );
    ensure!(
        corpus.manifest.gaps.values().all(
            |value| value.starts_with("not_measured:") || value.starts_with("not_established:")
        ),
        "gap promoted into evidence"
    );
    Ok(())
}

fn semantic_digest(manifest: &Manifest) -> Result<String> {
    let mut canonical = manifest.clone();
    canonical
        .cases
        .sort_by(|left, right| left.id.cmp(&right.id));
    canonical
        .incident_reference
        .cases
        .sort_by_key(|row| row.pull_request);
    for case in &mut canonical.cases {
        case.files.sort_by(|left, right| left.path.cmp(&right.path));
        case.measurements
            .sort_by(|left, right| left.name.cmp(&right.name));
        case.shapes.sort();
        case.omissions.sort();
    }
    Ok(digest(&serde_json::to_vec(&canonical)?))
}

#[test]
fn baseline_sources_are_bound_bounded_and_honestly_measured() -> Result<()> {
    let corpus = load(&root())?;
    println!(
        "semantic corpus digest: {}",
        semantic_digest(&corpus.manifest)?
    );
    let reference = &corpus.manifest.incident_reference;
    ensure!(
        reference.path == "fixtures/authority-incidents/manifest.json"
            && hex(&reference.git_blob, 40),
        "invalid incident reference"
    );
    let bytes = fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join(&reference.path))?;
    ensure!(
        digest(&bytes) == reference.sha256,
        "retained incident source changed"
    );
    let incident: Value = serde_json::from_slice(&bytes)?;
    for reference in &reference.cases {
        let source = rows(&incident, "/cases")?
            .iter()
            .find_map(|row| {
                row.get("source").filter(|source| {
                    source.get("pull_request").and_then(Value::as_u64)
                        == Some(reference.pull_request)
                })
            })
            .context("referenced incident missing")?;
        ensure!(
            number(source, "/workflow_run")? == reference.workflow_run
                && number(source, "/artifact_id")? == reference.artifact_id
                && string(source, "/artifact_digest")? == reference.artifact_digest,
            "incident source identity drift"
        );
    }
    ensure!(
        reference
            .cases
            .iter()
            .map(|row| row.pull_request)
            .collect::<BTreeSet<_>>()
            == BTreeSet::from([915, 916, 921]),
        "incident coverage incomplete"
    );
    Ok(())
}

#[test]
fn baseline_ordering_is_not_semantic_identity() -> Result<()> {
    let corpus = load(&root())?;
    let before = semantic_digest(&corpus.manifest)?;
    let mut reordered = corpus.manifest.clone();
    reordered.cases.reverse();
    reordered.incident_reference.cases.reverse();
    for case in &mut reordered.cases {
        case.files.reverse();
        case.measurements.reverse();
        case.shapes.reverse();
        case.omissions.reverse();
    }
    ensure!(
        semantic_digest(&reordered)? == before,
        "row ordering changed semantic digest"
    );
    let round_trip: Manifest = serde_json::from_slice(&serde_json::to_vec_pretty(&reordered)?)?;
    ensure!(
        semantic_digest(&round_trip)? == before,
        "manifest round trip changed semantic digest"
    );
    Ok(())
}

#[test]
fn baseline_rejects_forged_missing_duplicate_or_mislabelled_measurements() -> Result<()> {
    let corpus = load(&root())?;
    for mutation in 0..7 {
        let mut changed = Corpus {
            manifest: corpus.manifest.clone(),
            documents: corpus.documents.clone(),
        };
        let case = changed.manifest.cases.first_mut().context("fixture case")?;
        match mutation {
            0 => case.run_id += 1,
            1 => {
                case.measurements.pop();
            }
            2 => {
                case.measurements
                    .first_mut()
                    .context("fixture measure")?
                    .unit = "seconds".to_owned()
            }
            3 => {
                case.measurements
                    .iter_mut()
                    .find(|row| row.name == "billable_estimate")
                    .context("unknown cost")?
                    .value = Some(0)
            }
            4 => {
                let duplicate = case.clone();
                changed.manifest.cases.push(duplicate);
            }
            5 => {
                changed.documents.remove("1261/github-jobs.json");
            }
            _ => case.archive_sha256 = "a".repeat(64),
        }
        ensure!(
            validate(&changed).is_err(),
            "accepted bad fixture mutation {mutation}"
        );
    }
    ensure!(
        serde_json::from_str::<Manifest>(r#"{"schema":"wrong","anonymous_extra":true}"#).is_err()
    );
    Ok(())
}

#[test]
fn baseline_retains_large_packet_and_legacy_disagreement() -> Result<()> {
    let corpus = load(&root())?;
    let case = corpus
        .manifest
        .cases
        .iter()
        .find(|case| case.id == "1266")
        .context("large packet case")?;
    let zip = document(&corpus, case, "zip-inventory.json")?;
    ensure!(
        number(zip, "/archive_bytes")? == 148_138_955
            && number(zip, "/expanded_bytes")? == 1_537_034_279,
        "before-state changed"
    );
    let dominant = rows(zip, "/entries")?
        .iter()
        .max_by_key(|row| row.get("bytes").and_then(Value::as_u64))
        .context("dominant stream")?;
    ensure!(
        number(dominant, "/bytes")? == 1_509_857_324
            && string(dominant, "/path")?.ends_with("exposure-gaps.ripr.stdout"),
        "large raw-stream condition lost"
    );
    let gate = document(&corpus, case, "packet/review/gate_outcome.json")?;
    ensure!(
        string(gate, "/conclusion")? == "pass"
            && string(gate, "/gate_result")? == "not_proven"
            && number(gate, "/required_proof/skipped")? == 3,
        "historical authority disagreement normalized away"
    );
    Ok(())
}

#[test]
fn baseline_rejects_privacy_and_path_escapes() -> Result<()> {
    ensure!(privacy(&synthetic_bearer_header()).is_err());
    ensure!(private_keys(&serde_json::json!({"provider_request": "fixture"})).is_err());
    for path in ["../outside", "/outside", "C:\\outside", ""] {
        ensure!(confined(path).is_err());
    }
    let temporary = tempfile::tempdir()?;
    fs::write(temporary.path().join("extra.json"), "{}")?;
    ensure!(
        load(temporary.path()).is_err(),
        "anonymous extra file accepted"
    );
    Ok(())
}

fn synthetic_bearer_header() -> Vec<u8> {
    // Construct the negative input at runtime so source-review packets do
    // not themselves contain a credential-shaped header.
    [b"Bearer ".as_slice(), b"fixture-not-a-real-credential"].concat()
}

#[test]
fn baseline_file_inventory_rejects_mutation_omission_and_extra_payloads() -> Result<()> {
    let corpus = load(&root())?;
    let temporary = tempfile::tempdir()?;
    for name in ["README.md", "manifest.json"] {
        fs::copy(root().join(name), temporary.path().join(name))?;
    }
    for case in &corpus.manifest.cases {
        for file in &case.files {
            let destination = temporary.path().join(&file.path);
            fs::create_dir_all(destination.parent().context("fixture destination parent")?)?;
            fs::copy(root().join(&file.path), destination)?;
        }
    }
    load(temporary.path())?;
    let file = corpus
        .manifest
        .cases
        .first()
        .context("fixture case")?
        .files
        .first()
        .context("fixture receipt")?;
    let target = temporary.path().join(&file.path);
    let original = fs::read(&target)?;
    fs::write(&target, b"{}")?;
    ensure!(load(temporary.path()).is_err(), "changed bytes accepted");
    fs::remove_file(&target)?;
    ensure!(load(temporary.path()).is_err(), "missing file accepted");
    fs::write(&target, original)?;
    let extra = temporary.path().join("extra.json");
    fs::write(&extra, b"{}")?;
    ensure!(load(temporary.path()).is_err(), "unlisted file accepted");
    fs::write(&extra, synthetic_bearer_header())?;
    ensure!(
        load(temporary.path()).is_err(),
        "credential-shaped extra accepted"
    );
    fs::write(&extra, vec![b'x'; usize::try_from(MAX_FILE + 1)?])?;
    ensure!(load(temporary.path()).is_err(), "oversized file accepted");
    Ok(())
}
