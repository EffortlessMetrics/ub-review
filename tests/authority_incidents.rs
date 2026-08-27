//! Offline integrity and contradiction checks for the retained authority incidents (#961).

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

const CORPUS_SCHEMA: &str = "ub-review.authority_incident_corpus.v1";
const MANIFEST_NAME: &str = "manifest.json";
const SECRET_MARKERS: &[&str] = &[
    "authorization:",
    "bearer ",
    "x-api-key",
    "factory_api_key",
    "github_token",
    "minimax_api_key",
    "opencode_api_key",
    "ub_review_github_token",
    "ghp_",
    "gho_",
    "ghu_",
    "ghs_",
    "ghr_",
    "github_pat_",
    "sk-",
];
const PRIVATE_PAYLOAD_KEYS: &[&str] = &[
    "\"content\":",
    "\"prompt\":",
    "\"messages\":",
    "\"shared_context\":",
    "\"provider_request\":",
];

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CorpusManifest {
    schema: String,
    max_total_bytes: u64,
    cases: Vec<IncidentCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentCase {
    id: String,
    source: IncidentSource,
    files: Vec<FileReceipt>,
    redactions: Vec<String>,
    omissions: Vec<String>,
    expected_violations: Vec<ExpectedViolation>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IncidentSource {
    repository: String,
    pull_request: u64,
    workflow_run: u64,
    artifact_id: u64,
    artifact_name: String,
    artifact_digest: String,
    base_revision: String,
    head_revision: String,
    extracted_on: String,
    extraction_tool: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct FileReceipt {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ExpectedViolation {
    code: String,
    evidence: Vec<EvidencePointer>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidencePointer {
    file: String,
    pointer: String,
}

struct LoadedCorpus {
    manifest: CorpusManifest,
    documents: BTreeMap<String, Value>,
}

impl LoadedCorpus {
    fn incident(&self, id: &str) -> Result<&IncidentCase> {
        self.manifest
            .cases
            .iter()
            .find(|case| case.id == id)
            .with_context(|| format!("missing incident {id}"))
    }

    fn document(&self, path: &str) -> Result<&Value> {
        self.documents
            .get(path)
            .with_context(|| format!("missing retained document {path}"))
    }
}

fn corpus_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/authority-incidents")
}

fn load_corpus(root: &Path) -> Result<LoadedCorpus> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("canonicalize corpus root {}", root.display()))?;
    let manifest_bytes =
        fs::read(canonical_root.join(MANIFEST_NAME)).context("read authority-incident manifest")?;
    let manifest: CorpusManifest = serde_json::from_slice(&manifest_bytes)
        .context("parse strict authority-incident manifest")?;
    ensure!(
        manifest.schema == CORPUS_SCHEMA,
        "unsupported corpus schema"
    );
    ensure!(
        manifest.max_total_bytes > 0,
        "corpus size budget must be positive"
    );
    ensure!(!manifest.cases.is_empty(), "corpus must contain incidents");

    let mut case_ids = BTreeSet::new();
    let mut declared_files = BTreeSet::new();
    let mut documents = BTreeMap::new();
    let mut total_bytes = 0_u64;

    for case in &manifest.cases {
        validate_nonempty(&case.id, "incident id")?;
        ensure!(
            case_ids.insert(case.id.clone()),
            "duplicate incident id {}",
            case.id
        );
        validate_source(case)?;
        ensure!(
            case.id == case.source.pull_request.to_string(),
            "incident id must match its source pull request"
        );
        ensure!(
            !case.files.is_empty(),
            "incident {} has no retained files",
            case.id
        );
        ensure!(
            !case.expected_violations.is_empty(),
            "incident {} has no expected violations",
            case.id
        );
        ensure!(
            !case.omissions.is_empty(),
            "incident {} must explain omissions",
            case.id
        );
        for redaction in &case.redactions {
            validate_nonempty(redaction, "redaction note")?;
        }
        for omission in &case.omissions {
            validate_nonempty(omission, "omission note")?;
        }

        let mut case_files = BTreeSet::new();
        for receipt in &case.files {
            let relative = validate_relative_path(&receipt.path)?;
            ensure!(
                receipt.path.starts_with(&format!("{}/", case.id)),
                "retained file {} escapes incident {}",
                receipt.path,
                case.id
            );
            ensure!(
                case_files.insert(receipt.path.clone()),
                "duplicate case file {}",
                receipt.path
            );
            ensure!(
                declared_files.insert(receipt.path.clone()),
                "duplicate corpus file {}",
                receipt.path
            );
            ensure!(receipt.bytes > 0, "retained file {} is empty", receipt.path);
            validate_sha256(&receipt.sha256, "retained file digest")?;

            let path = canonical_root.join(relative);
            let canonical_path = path
                .canonicalize()
                .with_context(|| format!("missing retained file {}", receipt.path))?;
            ensure!(
                canonical_path.starts_with(&canonical_root),
                "retained file {} resolves outside the corpus",
                receipt.path
            );
            ensure!(
                !fs::symlink_metadata(&path)?.file_type().is_symlink(),
                "retained file {} must not be a symlink",
                receipt.path
            );
            let bytes = fs::read(&canonical_path)
                .with_context(|| format!("read retained file {}", receipt.path))?;
            let actual_bytes =
                u64::try_from(bytes.len()).context("retained file size exceeds u64")?;
            ensure!(
                actual_bytes == receipt.bytes,
                "size mismatch for {}",
                receipt.path
            );
            let text = std::str::from_utf8(&bytes)
                .with_context(|| format!("retained file {} is not UTF-8", receipt.path))?;
            reject_sensitive_payload(text, &receipt.path)?;
            let actual_digest = format!("{:x}", Sha256::digest(&bytes));
            ensure!(
                actual_digest == receipt.sha256,
                "digest mismatch for {}",
                receipt.path
            );
            let document: Value = serde_json::from_slice(&bytes)
                .with_context(|| format!("retained file {} is not JSON", receipt.path))?;
            total_bytes = total_bytes
                .checked_add(actual_bytes)
                .context("corpus byte count overflow")?;
            documents.insert(receipt.path.clone(), document);
        }

        let mut violation_codes = BTreeSet::new();
        for violation in &case.expected_violations {
            validate_nonempty(&violation.code, "expected violation code")?;
            ensure!(
                violation_codes.insert(violation.code.clone()),
                "duplicate expected violation {} in incident {}",
                violation.code,
                case.id
            );
            ensure!(
                !violation.evidence.is_empty(),
                "expected violation {} has no evidence",
                violation.code
            );
            for evidence in &violation.evidence {
                ensure!(
                    case_files.contains(&evidence.file),
                    "expected violation {} references undeclared file {}",
                    violation.code,
                    evidence.file
                );
                ensure!(
                    evidence.pointer.starts_with('/'),
                    "expected violation {} has a non-pointer evidence path",
                    violation.code
                );
                let document = documents
                    .get(&evidence.file)
                    .with_context(|| format!("missing evidence file {}", evidence.file))?;
                ensure!(
                    document.pointer(&evidence.pointer).is_some(),
                    "expected violation {} has missing evidence pointer {} in {}",
                    violation.code,
                    evidence.pointer,
                    evidence.file
                );
            }
        }
    }

    ensure!(
        total_bytes <= manifest.max_total_bytes,
        "retained corpus exceeds its byte budget"
    );
    let actual_files = collect_corpus_files(&canonical_root)?;
    ensure!(
        actual_files == declared_files,
        "corpus inventory mismatch; declared={declared_files:?}, actual={actual_files:?}"
    );
    Ok(LoadedCorpus {
        manifest,
        documents,
    })
}

fn validate_source(case: &IncidentCase) -> Result<()> {
    let source = &case.source;
    validate_nonempty(&source.repository, "source repository")?;
    ensure!(
        source.pull_request > 0,
        "source pull request must be positive"
    );
    ensure!(
        source.workflow_run > 0,
        "source workflow run must be positive"
    );
    ensure!(
        source.artifact_id > 0,
        "source artifact id must be positive"
    );
    validate_nonempty(&source.artifact_name, "source artifact name")?;
    let artifact_digest = source
        .artifact_digest
        .strip_prefix("sha256:")
        .context("source artifact digest must use sha256")?;
    validate_sha256(artifact_digest, "source artifact digest")?;
    validate_git_sha(&source.base_revision, "source base revision")?;
    validate_git_sha(&source.head_revision, "source head revision")?;
    ensure!(
        source.extracted_on.len() == 10
            && source.extracted_on.as_bytes().get(4) == Some(&b'-')
            && source.extracted_on.as_bytes().get(7) == Some(&b'-')
            && source
                .extracted_on
                .bytes()
                .enumerate()
                .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit()),
        "source extraction date must be YYYY-MM-DD"
    );
    validate_nonempty(&source.extraction_tool, "source extraction tool")?;
    Ok(())
}

fn validate_nonempty(value: &str, label: &str) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.trim() == value && !value.chars().any(char::is_control),
        "{label} must be canonical and non-empty"
    );
    Ok(())
}

fn validate_sha256(value: &str, label: &str) -> Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "{label} must be 64 lowercase hexadecimal characters"
    );
    Ok(())
}

fn validate_git_sha(value: &str, label: &str) -> Result<()> {
    ensure!(
        value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit()),
        "{label} must be a 40-character hexadecimal object id"
    );
    Ok(())
}

fn validate_relative_path(value: &str) -> Result<PathBuf> {
    validate_nonempty(value, "retained file path")?;
    ensure!(
        !value.contains('\\'),
        "retained file path must use forward slashes"
    );
    let path = Path::new(value);
    ensure!(!path.is_absolute(), "retained file path must be relative");
    ensure!(
        path.components()
            .all(|component| matches!(component, Component::Normal(_))),
        "retained file path must contain only normal components"
    );
    Ok(path.to_path_buf())
}

fn reject_sensitive_payload(text: &str, path: &str) -> Result<()> {
    let lowered = text.to_ascii_lowercase();
    for marker in SECRET_MARKERS {
        ensure!(
            !lowered.contains(marker),
            "secret marker {marker:?} found in {path}"
        );
    }
    for key in PRIVATE_PAYLOAD_KEYS {
        ensure!(
            !lowered.contains(key),
            "private payload key {key:?} found in {path}"
        );
    }
    Ok(())
}

fn collect_corpus_files(root: &Path) -> Result<BTreeSet<String>> {
    let mut files = BTreeSet::new();
    collect_corpus_files_from(root, root, &mut files)?;
    files.remove(MANIFEST_NAME);
    Ok(files)
}

fn collect_corpus_files_from(
    root: &Path,
    current: &Path,
    files: &mut BTreeSet<String>,
) -> Result<()> {
    for entry in fs::read_dir(current).with_context(|| format!("read {}", current.display()))? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        ensure!(!file_type.is_symlink(), "corpus must not contain symlinks");
        if file_type.is_dir() {
            collect_corpus_files_from(root, &entry.path(), files)?;
        } else if file_type.is_file() {
            let relative = entry
                .path()
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative);
        } else {
            bail!(
                "corpus contains unsupported entry {}",
                entry.path().display()
            );
        }
    }
    Ok(())
}

fn text_at<'a>(document: &'a Value, pointer: &str) -> Result<&'a str> {
    document
        .pointer(pointer)
        .and_then(Value::as_str)
        .with_context(|| format!("{pointer} must be a string"))
}

fn u64_at(document: &Value, pointer: &str) -> Result<u64> {
    document
        .pointer(pointer)
        .and_then(Value::as_u64)
        .with_context(|| format!("{pointer} must be a u64"))
}

fn array_at<'a>(document: &'a Value, pointer: &str) -> Result<&'a [Value]> {
    document
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .with_context(|| format!("{pointer} must be an array"))
}

fn copy_corpus(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_corpus(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[test]
fn corpus_is_complete_sanitized_and_digest_bound() -> Result<()> {
    let corpus = load_corpus(&corpus_root())?;
    ensure!(corpus.manifest.cases.len() == 3);
    ensure!(
        corpus
            .manifest
            .cases
            .iter()
            .all(|case| case.redactions.is_empty())
    );
    Ok(())
}

#[test]
fn pr_915_preserves_legacy_pass_with_required_proof_unproven() -> Result<()> {
    let corpus = load_corpus(&corpus_root())?;
    let incident = corpus.incident("915")?;
    ensure!(
        incident
            .expected_violations
            .iter()
            .any(|item| item.code == "legacy_pass_with_required_proof_unproven")
    );
    let gate = corpus.document("915/review/gate_outcome.json")?;
    ensure!(text_at(gate, "/conclusion")? == "pass");
    ensure!(text_at(gate, "/gate_result")? == "pass");
    ensure!(u64_at(gate, "/required_proof/matched")? == 3);
    ensure!(u64_at(gate, "/required_proof/passed")? == 0);
    ensure!(u64_at(gate, "/required_proof/skipped")? == 3);
    ensure!(text_at(gate, "/not_proven_reasons/0")?.contains("produced no passing receipt"));
    Ok(())
}

#[test]
fn pr_916_preserves_legacy_pass_with_truthful_not_proven() -> Result<()> {
    let corpus = load_corpus(&corpus_root())?;
    let incident = corpus.incident("916")?;
    ensure!(
        incident
            .expected_violations
            .iter()
            .any(|item| item.code == "legacy_pass_with_truthful_not_proven")
    );
    let gate = corpus.document("916/review/gate_outcome.json")?;
    ensure!(text_at(gate, "/conclusion")? == "pass");
    ensure!(text_at(gate, "/gate_result")? == "not_proven");
    ensure!(u64_at(gate, "/required_proof/passed")? == 0);
    ensure!(u64_at(gate, "/required_proof/skipped")? == 3);
    ensure!(text_at(gate, "/not_proven_reasons/0")?.contains("produced no passing receipt"));
    Ok(())
}

#[test]
fn pr_921_preserves_cross_projection_contradictions() -> Result<()> {
    let corpus = load_corpus(&corpus_root())?;
    let incident = corpus.incident("921")?;
    ensure!(incident.expected_violations.len() == 5);
    let gate = corpus.document("921/review/gate_outcome.json")?;
    let queue = corpus.document("921/work_queue.json")?;
    let portfolio = corpus.document("921/review/proof_portfolio.json")?;
    let receipts = corpus.document("921/review/proof_receipts.json")?;
    let sensor = corpus.document("921/sensors/cargo-allow/ub-review-sensor-status.json")?;

    ensure!(text_at(gate, "/conclusion")? == "pass");
    ensure!(text_at(gate, "/gate_result")? == "not_proven");
    ensure!(u64_at(gate, "/required_proof/passed")? == 0);
    ensure!(u64_at(gate, "/required_proof/skipped")? == 3);

    let queue_tasks = array_at(queue, "/tasks")?;
    let queue_ids = queue_tasks
        .iter()
        .filter_map(|task| task.get("id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    let candidate_ids = array_at(portfolio, "/candidate_tasks")?
        .iter()
        .filter_map(|task| task.get("id").and_then(Value::as_str))
        .collect::<BTreeSet<_>>();
    for receipt in array_at(receipts, "")? {
        let receipt_id = text_at(receipt, "/id")?;
        ensure!(text_at(receipt, "/requested_by/0")? == "impact-planner");
        ensure!(!queue_ids.contains(receipt_id));
        ensure!(!candidate_ids.contains(receipt_id));
        ensure!(text_at(receipt, "/head")? == "HEAD");
    }

    let cargo_allow_task = queue_tasks
        .iter()
        .find(|task| task.get("id").and_then(Value::as_str) == Some("sensor-cargo-allow"))
        .context("missing cargo-allow queue task")?;
    ensure!(text_at(cargo_allow_task, "/status")? == "planned");
    ensure!(text_at(sensor, "/status")? == "ok");
    ensure!(text_at(sensor, "/reason")? == "completed");

    ensure!(text_at(portfolio, "/head")? == "HEAD");
    ensure!(array_at(portfolio, "/selected_task_ids")?.is_empty());
    let required_candidates = array_at(portfolio, "/candidate_tasks")?
        .iter()
        .filter(|task| task.get("required").and_then(Value::as_bool) == Some(true))
        .count();
    ensure!(required_candidates == 3);
    for decision in array_at(portfolio, "/decisions")? {
        if decision.get("required").and_then(Value::as_bool) == Some(true) {
            ensure!(array_at(decision, "/receipt_ids")?.is_empty());
        }
    }

    ensure!(u64_at(portfolio, "/budget_seconds")? == 0);
    ensure!(u64_at(portfolio, "/runtime/deadline_remaining_seconds")? > 0);
    let mut actual_ms = 0_u64;
    let mut timeout_ms = 0_u64;
    for receipt in array_at(receipts, "")? {
        for command in array_at(receipt, "/commands")? {
            actual_ms = actual_ms
                .checked_add(u64_at(command, "/duration_ms")?)
                .context("actual duration overflow")?;
            timeout_ms = timeout_ms
                .checked_add(
                    u64_at(command, "/timeout_sec")?
                        .checked_mul(1_000)
                        .context("timeout conversion overflow")?,
                )
                .context("timeout duration overflow")?;
        }
    }
    ensure!(actual_ms < timeout_ms);
    Ok(())
}

#[test]
fn corpus_rejects_mutation_missing_files_secrets_and_bad_evidence() -> Result<()> {
    for (mutation, expected_error) in [
        ("digest", "digest mismatch"),
        ("missing", "missing retained file"),
        ("secret", "secret marker"),
        ("private", "private payload key"),
        ("pointer", "missing evidence pointer"),
        ("budget", "exceeds its byte budget"),
        ("path", "only normal components"),
        ("unknown", "unknown field"),
        ("extra", "corpus inventory mismatch"),
    ] {
        let temp = tempfile::tempdir()?;
        copy_corpus(&corpus_root(), temp.path())?;
        match mutation {
            "digest" => {
                let path = temp.path().join("915/review/gate_outcome.json");
                let mut bytes = fs::read(&path)?;
                let final_byte = bytes.last_mut().context("empty retained fixture")?;
                *final_byte = b' ';
                fs::write(path, bytes)?;
            }
            "missing" => fs::remove_file(temp.path().join("916/review/proof_receipts.json"))?,
            "secret" | "private" => {
                let payload: &[u8] = if mutation == "secret" {
                    b"{\"GITHUB_TOKEN\":\"ghp_1234567890abcdef\"}\n"
                } else {
                    b"{\"prompt\":\"private provider input\"}\n"
                };
                fs::write(temp.path().join("921/review/gate_outcome.json"), payload)?;
                let manifest_path = temp.path().join(MANIFEST_NAME);
                let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
                let bytes = manifest
                    .pointer_mut("/cases/2/files/0/bytes")
                    .context("missing retained file byte receipt")?;
                *bytes = Value::from(u64::try_from(payload.len())?);
                fs::write(manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
            }
            "pointer" | "budget" | "path" | "unknown" => {
                let manifest_path = temp.path().join(MANIFEST_NAME);
                let mut manifest: Value = serde_json::from_slice(&fs::read(&manifest_path)?)?;
                match mutation {
                    "pointer" => {
                        let evidence = manifest
                            .pointer_mut("/cases/0/expected_violations/0/evidence/0/pointer")
                            .context("missing manifest evidence pointer")?;
                        *evidence = Value::String("/absent".to_owned());
                    }
                    "budget" => {
                        let budget = manifest
                            .pointer_mut("/max_total_bytes")
                            .context("missing manifest size budget")?;
                        *budget = Value::from(1_u64);
                    }
                    "path" => {
                        let path = manifest
                            .pointer_mut("/cases/0/files/0/path")
                            .context("missing retained file path")?;
                        *path = Value::String("../escape.json".to_owned());
                    }
                    "unknown" => {
                        let object = manifest
                            .as_object_mut()
                            .context("manifest must be an object")?;
                        object.insert("future".to_owned(), Value::Bool(true));
                    }
                    other => bail!("unknown manifest mutation {other}"),
                }
                fs::write(manifest_path, serde_json::to_vec_pretty(&manifest)?)?;
            }
            "extra" => fs::write(temp.path().join("921/undeclared.json"), b"{}\n")?,
            other => bail!("unknown corpus mutation {other}"),
        }
        let error = load_corpus(temp.path())
            .err()
            .context("corpus mutation unexpectedly passed")?;
        let message = format!("{error:#}");
        ensure!(
            message.contains(expected_error),
            "corpus mutation {mutation} failed for the wrong reason: expected {expected_error:?}, got {message:?}"
        );
    }
    Ok(())
}
