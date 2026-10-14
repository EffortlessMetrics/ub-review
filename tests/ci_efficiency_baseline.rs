//! Fixed, offline #1270 before-state joins. This is neither a runtime verifier
//! nor authority for reuse, scheduling, costs, gate promotion or issue closure.
//! Inventory/privacy/digest helpers adapt the historical Rust corpus at
//! c6c89c949c23ee9d20ee0f65ac40457ee0a329e3; no second Python verifier is added.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use chrono::DateTime;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_BYTES: u64 = 65_536;
const MAIN: &str = "3dbcc2be9768b082c0896dc2360e950ffbd56a65";
const HEAD: &str = "0ce3c632cbd19e102dc49c24a59f30f48bb5935d";
const MERGE: &str = "4fd2812df756e609ce350c27a62149af633516a3";
const PACKET: &str = "d8095fe5f58c6fdc65cc3b931176e9094e723a91a1b668d41df56a6aa8b34ca7";
// A fixed projection-byte lock, not a signature or verification of raw sources
// or uninspected producer artifacts. Semantic tests below bypass this lock so
// rejection must come from the substantive join under test.
const PROJECTIONS: &str = "5f9dde16010ce8c099de9ad11d4325fbc7915942e44cac438313ce89694aa910";
const CANDIDATE_RUN: u64 = 38_520_204_127;
const BASELINE_RUN: u64 = 38_520_201_117;
const GATE_JOB: u64 = 115_707_925_898;

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: String,
    max_total_bytes: u64,
    fixed_fixture_only: bool,
    repository: String,
    issue: u64,
    pull_request: u64,
    frozen_main: String,
    raw_head: String,
    merge_result: String,
    source_packet_sha256: String,
    scope: String,
    unknowns: Vec<String>,
    measurements: BTreeMap<String, Option<u64>>,
    files: Vec<FileReceipt>,
    observations: Vec<Observation>,
    configured_rules: Vec<Rule>,
    overlaps: Vec<Overlap>,
    workflows: BTreeMap<String, Workflow>,
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
struct Workflow {
    definition_commit: String,
    authority: String,
    tested_commit: String,
    semantics: String,
    target_root: String,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Observation {
    id: String,
    kind: String,
    run_id: u64,
    job_id: u64,
    step_number: u64,
    workflow: String,
    tested_commit: Option<String>,
    revision_semantics: String,
    observed_argv: Option<Vec<String>>,
    task_id: Option<String>,
    result: Option<String>,
    receipt_satisfied: Option<bool>,
    process_started: bool,
    source: String,
    source_pointer: String,
    process_time_ms: Option<u64>,
    command_exit_code: Option<u64>,
    workflow_continue_on_error: Option<bool>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Rule {
    id: String,
    configured_argv: Vec<String>,
    configuration_commit: String,
    execution_observed: bool,
    observed_argv: Option<Vec<String>>,
    runtime_request_identity: Option<String>,
    required: bool,
    timeout_sec: u64,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Overlap {
    id: String,
    kind: String,
    left: String,
    right: String,
    status: String,
    avoided_work: Option<u64>,
    differences: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Sources {
    schema: String,
    captures: BTreeMap<String, Capture>,
    projections: BTreeMap<String, Projection>,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Capture {
    capture_basis: String,
    url: String,
    bytes: u64,
    sha256: String,
    complete_bytes_retained: bool,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Projection {
    source: String,
    selector: String,
    value: Value,
}

#[derive(Clone, Deserialize, Serialize)]
struct Corpus {
    manifest: Manifest,
    sources: Sources,
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
        !path.is_empty()
            && !path.contains('\\')
            && Path::new(path)
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
        "x-amz-",
        "sig=",
        "se=",
        "sv=",
        "oaiusercontent.com",
        "blob.core.windows.net",
        "ub_review_action_github_token",
    ] {
        ensure!(
            !text.contains(marker),
            "private or credential-like fixture content"
        );
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
                        "providerrequest",
                        "downloadurl",
                        "archiveurl",
                        "transfermetadata"
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
                path.components().count() <= 4,
                "fixture nesting exceeds budget"
            );
            inventory(&entry.path(), &path, files, total)?;
        } else {
            ensure!(
                metadata.is_file() && metadata.len() <= MAX_BYTES,
                "unbounded or special fixture file"
            );
            *total = total
                .checked_add(metadata.len())
                .context("fixture byte overflow")?;
            ensure!(*total <= MAX_BYTES, "fixture corpus exceeds byte budget");
            ensure!(
                files.insert(path.to_string_lossy().replace('\\', "/")) && files.len() <= 8,
                "excessive fixture inventory"
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
    let bytes = fs::read(directory.join("manifest.json"))?;
    let value: Value = serde_json::from_slice(&bytes)?;
    private_keys(&value)?;
    // Nullable unknowns must be explicit, not silently defaulted by serde.
    for row in rows(&value, "/observations")? {
        for key in [
            "tested_commit",
            "observed_argv",
            "task_id",
            "result",
            "receipt_satisfied",
            "process_time_ms",
            "command_exit_code",
            "workflow_continue_on_error",
        ] {
            ensure!(
                row.get(key).is_some(),
                "missing explicit observation unknown"
            );
        }
    }
    for row in rows(&value, "/configured_rules")? {
        for key in ["observed_argv", "runtime_request_identity"] {
            ensure!(
                row.get(key).is_some(),
                "missing explicit configured unknown"
            );
        }
    }
    let manifest: Manifest = serde_json::from_value(value)?;
    ensure!(
        manifest.files.len() == 1,
        "fixed projection inventory changed"
    );
    let mut expected = BTreeSet::from(["README.md".to_owned(), "manifest.json".to_owned()]);
    for receipt in &manifest.files {
        confined(&receipt.path)?;
        ensure!(
            expected.insert(receipt.path.clone()),
            "duplicate retained file"
        );
        let bytes = fs::read(directory.join(&receipt.path))?;
        ensure!(
            receipt.path == "source-projections.json"
                && receipt.sha256 == PROJECTIONS
                && receipt.bytes == u64::try_from(bytes.len())?
                && digest(&bytes) == receipt.sha256,
            "retained projection byte receipt mismatch"
        );
    }
    ensure!(actual == expected, "unlisted or missing corpus file");
    let value: Value =
        serde_json::from_slice(&fs::read(directory.join("source-projections.json"))?)?;
    private_keys(&value)?;
    let sources = serde_json::from_value(value)?;
    let corpus = Corpus { manifest, sources };
    validate(&corpus)?;
    Ok(corpus)
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

fn projection<'a>(corpus: &'a Corpus, name: &str) -> Result<&'a Value> {
    Ok(&corpus
        .sources
        .projections
        .get(name)
        .with_context(|| format!("missing projection {name}"))?
        .value)
}

fn texts<'a>(corpus: &'a Corpus, name: &str) -> Result<Vec<&'a str>> {
    rows(projection(corpus, name)?, "")?
        .iter()
        .map(|line| string(line, "/text"))
        .collect()
}

fn contains(corpus: &Corpus, name: &str, needle: &str) -> Result<bool> {
    Ok(texts(corpus, name)?
        .iter()
        .any(|text| text.contains(needle)))
}

// Fixed log projections contain either a GitHub Run group heading or the
// corresponding colored shell line. Compare every argv token, not a prefix
// or substring. This is deliberately not a general shell/argv parser.
fn logged_argv(corpus: &Corpus, name: &str, argv: &[String]) -> Result<bool> {
    Ok(texts(corpus, name)?.iter().any(|line| {
        let Some((_, body)) = line.split_once(' ') else {
            return false;
        };
        let command = body.strip_prefix("##[group]Run ").or_else(|| {
            body.strip_prefix("\u{1b}[36;1m")
                .and_then(|text| text.strip_suffix("\u{1b}[0m"))
        });
        command.is_some_and(|command| {
            command
                .split_whitespace()
                .eq(argv.iter().map(String::as_str))
        })
    }))
}

fn named<'a>(corpus: &'a Corpus, name: &str) -> Result<&'a Observation> {
    corpus
        .manifest
        .observations
        .iter()
        .find(|row| row.id == name)
        .with_context(|| format!("missing observation {name}"))
}

fn job<'a>(corpus: &'a Corpus, row: &Observation) -> Result<&'a Value> {
    rows(projection(corpus, &format!("{}-jobs", row.workflow))?, "")?
        .iter()
        .find(|job| job.get("id").and_then(Value::as_u64) == Some(row.job_id))
        .context("observation job not in workflow source")
}

fn step<'a>(job: &'a Value, row: &Observation) -> Result<&'a Value> {
    rows(job, "/steps")?
        .iter()
        .find(|step| step.get("number").and_then(Value::as_u64) == Some(row.step_number))
        .context("observation step not in source job")
}

fn selected_command(corpus: &Corpus, check: &str) -> Result<Vec<String>> {
    let lines = texts(corpus, "baseline-definition")?;
    let arm = format!("{check})");
    let index = lines
        .iter()
        .position(|text| text.trim() == arm)
        .context("missing selected baseline arm")?;
    let command = lines
        .get(index + 1)
        .context("missing selected baseline command")?;
    Ok(command.split_whitespace().map(str::to_owned).collect())
}

fn validate_sources(corpus: &Corpus) -> Result<()> {
    ensure!(
        corpus.sources.schema == "ub-review.ci_efficiency_source_projections.v1",
        "unsupported source schema"
    );
    let mut used = BTreeSet::new();
    for (name, source) in &corpus.sources.captures {
        let expected_basis = if name == "candidate-shadow-projection" {
            "derived_log_projection_bytes"
        } else if [
            "candidate-run",
            "baseline-run",
            "candidate-jobs-full",
            "baseline-jobs-full",
        ]
        .contains(&name.as_str())
        {
            "connector_response_bytes"
        } else {
            "decoded_public_source_bytes"
        };
        ensure!(
            source.capture_basis == expected_basis,
            "capture/source byte basis collapsed"
        );
        ensure!(
            ((source
                .url
                .starts_with("https://github.com/EffortlessMetrics/ub-review/")
                && !source.url.contains('?'))
                || (source
                    .url
                    .starts_with("https://api.github.com/repos/EffortlessMetrics/ub-review/")
                    && (!source.url.contains('?') || source.url.ends_with("/jobs?per_page=100"))))
                && source.bytes > 0
                && hex(&source.sha256, 64)
                && !source.complete_bytes_retained,
            "invalid public source receipt or source/projection collapse"
        );
    }
    for projection in corpus.sources.projections.values() {
        ensure!(
            corpus.sources.captures.contains_key(&projection.source)
                && !projection.selector.trim().is_empty(),
            "projection source identity missing"
        );
        used.insert(&projection.source);
        private_keys(&projection.value)?;
        if projection.selector == "selected_original_lines" {
            let mut previous = 0;
            for line in rows(&projection.value, "")? {
                let current = number(line, "/number")?;
                ensure!(
                    current > previous && string(line, "/text")?.len() <= 1024,
                    "invalid bounded original line selection"
                );
                previous = current;
            }
            ensure!(previous > 0, "empty source line selection");
        }
    }
    ensure!(
        used.len() == corpus.sources.captures.len(),
        "orphan source receipt"
    );
    for (role, run_id, event, conclusion, path) in [
        (
            "candidate",
            CANDIDATE_RUN,
            "pull_request",
            "failure",
            ".github/workflows/ub-review-gate.yml",
        ),
        (
            "baseline",
            BASELINE_RUN,
            "pull_request_target",
            "success",
            ".github/workflows/independent-baseline.yml",
        ),
    ] {
        let run = projection(corpus, &format!("{role}-run"))?;
        ensure!(
            number(run, "/id")? == run_id
                && number(run, "/run_attempt")? == 1
                && string(run, "/head_sha")? == HEAD
                && string(run, "/event")? == event
                && string(run, "/conclusion")? == conclusion
                && string(run, "/status")? == "completed"
                && string(run, "/path")? == path
                && string(run, "/html_url")?
                    == format!(
                        "https://github.com/EffortlessMetrics/ub-review/actions/runs/{run_id}"
                    ),
            "fixed run identity or authority drift"
        );
        let jobs = rows(projection(corpus, &format!("{role}-jobs"))?, "")?;
        ensure!(
            jobs.len() == if role == "candidate" { 2 } else { 8 },
            "fixed job inventory changed"
        );
        let mut ids = BTreeSet::new();
        for job in jobs {
            ensure!(
                ids.insert(number(job, "/id")?)
                    && number(job, "/run_id")? == run_id
                    && number(job, "/run_attempt")? == 1
                    && string(job, "/head_sha")? == HEAD
                    && string(job, "/status")? == "completed",
                "source job identity drift"
            );
            let mut steps = BTreeSet::new();
            for step in rows(job, "/steps")? {
                ensure!(
                    steps.insert(number(step, "/number")?),
                    "duplicate source step"
                );
                let from = DateTime::parse_from_rfc3339(string(step, "/started_at")?)?;
                let to = DateTime::parse_from_rfc3339(string(step, "/completed_at")?)?;
                ensure!(to >= from, "negative step wall window");
                ensure!(
                    string(step, "/status")? == "completed",
                    "nonterminal source step"
                );
            }
        }
    }
    ensure!(
        contains(corpus, "candidate-checkout", "git log -1 --format=%H")?
            && contains(corpus, "candidate-checkout", MERGE)?,
        "candidate checkout evidence missing"
    );
    Ok(())
}

fn validate_workflows(corpus: &Corpus) -> Result<()> {
    ensure!(
        corpus.manifest.workflows.len() == 2,
        "fixed workflow inventory changed"
    );
    for (role, definition, authority, subject, semantics, target) in [
        (
            "candidate",
            MERGE,
            "candidate-controlled-contained",
            MERGE,
            "merge_result",
            "target/ub-review",
        ),
        (
            "baseline",
            MAIN,
            "protected-base-definition",
            HEAD,
            "exact_pr_head",
            "$RUNNER_TEMP/ub-review-independent-${CHECK}-target",
        ),
    ] {
        let workflow = corpus
            .manifest
            .workflows
            .get(role)
            .context("workflow missing")?;
        ensure!(
            workflow.definition_commit == definition
                && workflow.authority == authority
                && workflow.tested_commit == subject
                && workflow.semantics == semantics
                && workflow.target_root == target,
            "workflow definition, authority or subject collapsed"
        );
        let finalizer = texts(corpus, "baseline-finalizer-consumer")?.join("\n");
        ensure!(
            finalizer.contains(MAIN) && finalizer.contains(HEAD),
            "protected workflow/raw-head log binding missing"
        );
    }
    ensure!(
        contains(
            corpus,
            "baseline-definition",
            "ref: ${{ github.event.pull_request.head.sha }}"
        )? && contains(
            corpus,
            "baseline-definition",
            "$RUNNER_TEMP/ub-review-independent-${CHECK}-target"
        )?,
        "baseline checkout or target-root authority missing"
    );
    Ok(())
}

fn validate_observation(corpus: &Corpus, row: &Observation) -> Result<()> {
    let workflow = corpus
        .manifest
        .workflows
        .get(&row.workflow)
        .context("unknown observation workflow")?;
    let run = projection(corpus, &format!("{}-run", row.workflow))?;
    let job = job(corpus, row)?;
    let step = step(job, row)?;
    ensure!(
        row.run_id == number(run, "/id")? && row.run_id == number(job, "/run_id")?,
        "observation run/job identity mismatch"
    );
    ensure!(
        row.process_time_ms.is_none(),
        "unknown process time fabricated"
    );
    let proof = row.id.starts_with("candidate-proof-head-");
    if proof
        || row.id.starts_with("candidate-required-")
        || row.id == "candidate-coverage-publication-consumer"
    {
        ensure!(
            row.tested_commit.is_none()
                && row.revision_semantics == "individual_command_subject_unavailable",
            "proof-head label promoted to tested SHA"
        );
    } else {
        ensure!(
            row.tested_commit.as_deref() == Some(workflow.tested_commit.as_str())
                && row.revision_semantics == workflow.semantics,
            "observation checkout subjects collapsed"
        );
    }
    if row.source == row.id || row.source == "shadow" {
        let capture = corpus
            .sources
            .projections
            .get(&row.source)
            .and_then(|projection| corpus.sources.captures.get(&projection.source))
            .context("observation capture missing")?;
        let expected_job = if row.id == "candidate-coverage-publication-consumer" {
            GATE_JOB
        } else {
            row.job_id
        };
        ensure!(
            capture.url
                == format!(
                    "https://github.com/EffortlessMetrics/ub-review/actions/runs/{}/job/{expected_job}",
                    row.run_id
                ),
            "observation public log source/job mismatch"
        );
    }
    let source = projection(corpus, &row.source)?;
    let selected = source
        .pointer(&row.source_pointer)
        .context("invalid observation source pointer")?;
    match row.kind.as_str() {
        "executed_command" => {
            if row.workflow == "candidate" {
                let expected_step = match row.id.as_str() {
                    "candidate-test-projections" => 3,
                    "candidate-test-publication" | "candidate-test-transactions" => 4,
                    "candidate-test-bootstrap" => 7,
                    "candidate-source-bootstrap" | "candidate-review-run" => 9,
                    "candidate-artifact-verifier" => 10,
                    "candidate-reconcile-tasks" => 11,
                    "candidate-reconcile-publication" => 12,
                    _ => anyhow::bail!("unknown fixed wrapper identity"),
                };
                ensure!(
                    row.job_id == GATE_JOB && row.step_number == expected_step,
                    "wrapper command/step identity mismatch"
                );
            }
            ensure!(
                row.process_started
                    && row.task_id.is_none()
                    && row.receipt_satisfied.is_none()
                    && row.source == row.id
                    && row.source_pointer.is_empty(),
                "command/source identity changed"
            );
            if let Some(check) = row.id.strip_prefix("baseline-") {
                ensure!(
                    row.observed_argv.as_ref() == Some(&selected_command(corpus, check)?),
                    "baseline argv differs from selected arm"
                );
                ensure!(
                    contains(corpus, &row.source, &format!("CHECK: {check}"))?
                        && contains(corpus, &row.source, HEAD)?
                        && string(job, "/name")? == format!("independent evidence / {check}")
                        && row.step_number == 6
                        && string(step, "/conclusion")? == "success"
                        && string(job, "/conclusion")? == "success"
                        && row.result.as_deref() == Some("success"),
                    "selected baseline command/job binding changed"
                );
                let identity_step = rows(job, "/steps")?
                    .iter()
                    .find(|s| s.get("number").and_then(Value::as_u64) == Some(3))
                    .context("baseline identity step missing")?;
                ensure!(
                    string(identity_step, "/conclusion")? == "success",
                    "baseline identity check failed"
                );
            } else if ["candidate-source-bootstrap", "candidate-review-run"]
                .contains(&row.id.as_str())
            {
                ensure!(
                    row.observed_argv.is_none()
                        && row.result.as_deref() == Some("success")
                        && contains(corpus, &row.source, "outcome=success;conclusion=success")?,
                    "wrapper argv or outcome inferred"
                );
                if row.id == "candidate-source-bootstrap" {
                    ensure!(
                        contains(corpus, &row.source, "cargo build --manifest-path")?
                            && contains(corpus, &row.source, "`release` profile")?,
                        "source bootstrap execution evidence missing"
                    );
                } else {
                    ensure!(
                        contains(corpus, &row.source, "--fail-on-gate false")?,
                        "review wrapper gate tolerance missing"
                    );
                }
            } else {
                let argv = row
                    .observed_argv
                    .as_ref()
                    .context("missing observed wrapper argv")?;
                ensure!(
                    !argv.is_empty() && logged_argv(corpus, &row.source, argv)?,
                    "wrapper argv not in command log projection"
                );
                ensure!(
                    string(step, "/conclusion")? == "success",
                    "successful wrapper step changed"
                );
                let expected_result = if row.id == "candidate-reconcile-tasks" {
                    "command_failure_tolerated_by_workflow"
                } else if row.id == "candidate-reconcile-publication" {
                    let report = projection(corpus, "publication-shadow")?;
                    ensure!(
                        string(report, "/schema")?
                            == "ub-review.publication_boundary_reconciliation.v1"
                            && string(report, "/authority")? == "shadow-only"
                            && string(report, "/status")? == "coherent"
                            && report.get("input_unavailable").and_then(Value::as_bool)
                                == Some(false)
                            && string(report, "/revision/reviewed_commit")? == MERGE
                            && string(report, "/revision/pr_head_commit")? == HEAD,
                        "publication shadow outcome source changed"
                    );
                    "command_success_with_shadow_report"
                } else {
                    "success"
                };
                ensure!(
                    row.result.as_deref() == Some(expected_result),
                    "wrapper outcome differs from source step/report"
                );
            }
        }
        "reported_executed_task" => {
            let task = row
                .task_id
                .as_deref()
                .context("missing reported task identity")?;
            ensure!(
                row.id == format!("candidate-{task}")
                    && row.workflow == "candidate"
                    && row.job_id == GATE_JOB
                    && row.step_number == 9
                    && row.process_started
                    && row.observed_argv.is_none()
                    && row.receipt_satisfied.is_none()
                    && row.source == "shadow"
                    && string(selected, "/identity")? == task,
                "reported task identity or argv inference"
            );
            let expected_code = if proof {
                "executed_proof_missing_from_queue"
            } else if task == "sensor-coverage" {
                "terminal_task_left_planned"
            } else {
                "successful_sensor_left_planned"
            };
            ensure!(
                string(selected, "/code")? == expected_code,
                "reported task outcome/code mismatch"
            );
            ensure!(
                row.result.as_deref()
                    == if proof {
                        None
                    } else if task == "sensor-coverage" {
                        Some("reported_terminal_not_succeeded")
                    } else {
                        Some("reported_succeeded")
                    },
                "reported task outcome fabricated"
            );
        }
        "consumer_observation" => {
            ensure!(
                !row.process_started && row.observed_argv.is_none() && row.task_id.is_none(),
                "consumer counted as execution"
            );
            if let Some(request) = row.id.strip_prefix("candidate-required-") {
                ensure!(
                    row.source == "shadow"
                        && row.job_id == GATE_JOB
                        && row.step_number == 9
                        && row.result.as_deref() == Some("required_unproven")
                        && row.receipt_satisfied == Some(false)
                        && string(selected, "/identity")? == request
                        && string(selected, "/code")?
                            == "required_request_without_satisfying_receipt",
                    "Required starvation consumer or source erased"
                );
            } else {
                ensure!(
                    row.source == row.id
                        && row.source_pointer.is_empty()
                        && row.receipt_satisfied.is_none(),
                    "consumer source identity changed"
                );
            }
        }
        _ => anyhow::bail!("configured-only or unknown row counted as observation"),
    }
    if row.id != "candidate-reconcile-tasks" {
        ensure!(
            row.command_exit_code.is_none() && row.workflow_continue_on_error.is_none(),
            "unobserved command exit fabricated"
        );
    }
    Ok(())
}

fn validate_counterexample(corpus: &Corpus) -> Result<()> {
    let shadow = projection(corpus, "shadow")?;
    ensure!(
        string(shadow, "/schema")? == "ub-review.task_projection_reconciliation.v1"
            && string(shadow, "/authority")? == "shadow-only"
            && string(shadow, "/status")? == "contradictory"
            && shadow.get("input_unavailable").and_then(Value::as_bool) == Some(true)
            && string(shadow, "/revision/reviewed_commit")? == MERGE
            && string(shadow, "/revision/semantics")? == "merge_result",
        "shadow contradiction or unavailable input normalized"
    );
    let sensors = corpus
        .manifest
        .observations
        .iter()
        .filter(|row| {
            row.task_id
                .as_deref()
                .is_some_and(|id| id.starts_with("sensor-"))
        })
        .count();
    let proofs = corpus
        .manifest
        .observations
        .iter()
        .filter(|row| {
            row.task_id
                .as_deref()
                .is_some_and(|id| id.starts_with("proof-head-"))
        })
        .count();
    ensure!(
        sensors == 11
            && proofs == 3
            && number(shadow, "/counts/executed_sensor_tasks")? == u64::try_from(sensors)?
            && number(shadow, "/counts/executed_proof_command_tasks")? == u64::try_from(proofs)?,
        "shadow task count differs from observations"
    );
    let required: BTreeSet<_> = rows(shadow, "/observations")?
        .iter()
        .filter(|r| {
            r.get("code").and_then(Value::as_str)
                == Some("required_request_without_satisfying_receipt")
        })
        .map(|r| string(r, "/identity"))
        .collect::<Result<_>>()?;
    ensure!(
        required
            == BTreeSet::from([
                "proof-0000-291419f6f509",
                "proof-0001-d97c389baa65",
                "proof-0002-d2ae3e742e84"
            ]),
        "fixed unsatisfied Required identities changed"
    );
    let observed: BTreeSet<_> = corpus
        .manifest
        .observations
        .iter()
        .filter_map(|r| r.id.strip_prefix("candidate-required-"))
        .collect();
    ensure!(
        observed == required,
        "unsatisfied Required request set differs from source"
    );
    ensure!(
        rows(shadow, "/issues")?
            .iter()
            .any(|issue| issue.get("code").and_then(Value::as_str)
                == Some("timeout_ceilings_exhausted_budget_before_actual_deadline")),
        "Required-starvation timeout diagnostic erased"
    );
    ensure!(
        contains(
            corpus,
            "sensor-interpretation",
            "executed = timing.get(\"process_started_at\") is not None"
        )? && contains(
            corpus,
            "sensor-interpretation",
            "successful_sensor_left_planned"
        )? && contains(corpus, "proof-interpretation", "required: false")?
            && contains(corpus, "proof-interpretation", "impact-planner")?,
        "reported task interpretation source missing"
    );
    let reconcile = named(corpus, "candidate-reconcile-tasks")?;
    ensure!(
        reconcile.command_exit_code == Some(2)
            && reconcile.workflow_continue_on_error == Some(true)
            && reconcile.result.as_deref() == Some("command_failure_tolerated_by_workflow")
            && contains(
                corpus,
                &reconcile.source,
                "Process completed with exit code 2."
            )?
            && contains(corpus, "candidate-tolerance", "continue-on-error: true")?
            && string(step(job(corpus, reconcile)?, reconcile)?, "/conclusion")? == "success",
        "tolerated command failure normalized to success"
    );
    let gate = named(corpus, "candidate-gate-consumer")?;
    ensure!(
        gate.result.as_deref() == Some("failure")
            && contains(corpus, &gate.source, "blocking reasons: ripr")?
            && string(step(job(corpus, gate)?, gate)?, "/conclusion")? == "failure",
        "product gate outcome normalized to sensor success"
    );
    let baseline = named(corpus, "baseline-finalizer-consumer")?;
    ensure!(
        baseline.result.as_deref() == Some("success")
            && contains(corpus, &baseline.source, "EVIDENCE_RESULT: success")?
            && string(step(job(corpus, baseline)?, baseline)?, "/conclusion")? == "success",
        "baseline finalizer outcome changed"
    );
    let coverage = named(corpus, "candidate-coverage-publication-consumer")?;
    ensure!(
        coverage.result.as_deref() == Some("skipped")
            && contains(
                corpus,
                &coverage.source,
                "No files were found with the provided path: target/ub-review/sensors/coverage/lcov.info"
            )?
            && string(step(job(corpus, coverage)?, coverage)?, "/conclusion")? == "skipped",
        "coverage publication success fabricated"
    );
    for artifact in rows(shadow, "/source_artifacts")? {
        confined(string(artifact, "/path")?)?;
        ensure!(
            number(artifact, "/bytes")? > 0 && hex(string(artifact, "/sha256")?, 64),
            "invalid reported producer artifact reference"
        );
    }
    Ok(())
}

fn validate_rules(corpus: &Corpus) -> Result<()> {
    let text = texts(corpus, "required-config")?.join("\n");
    let mut source_rules = BTreeMap::new();
    for section in text.split("[[proof.required]]").skip(1) {
        let table: toml::Table = toml::from_str(section)?;
        let id = table
            .get("id")
            .and_then(toml::Value::as_str)
            .context("configured source rule id missing")?
            .to_owned();
        ensure!(
            source_rules.insert(id, table).is_none(),
            "duplicate configured source rule id"
        );
    }
    ensure!(
        source_rules.len() == 3 && corpus.manifest.configured_rules.len() == 3,
        "fixed configured rule inventory changed"
    );
    let mut ids = BTreeSet::new();
    let mut joined = BTreeSet::new();
    for rule in &corpus.manifest.configured_rules {
        ensure!(
            ids.insert(rule.id.as_str())
                && rule.configuration_commit == MERGE
                && !rule.execution_observed
                && rule.observed_argv.is_none()
                && rule.runtime_request_identity.is_none()
                && rule.required,
            "configured rule promoted to execution or runtime mapping"
        );
        let source_id = rule
            .id
            .strip_prefix("configured-required-")
            .context("configured row source id missing")?;
        let table = source_rules
            .get(source_id)
            .context("configured row source rule missing")?;
        ensure!(
            joined.insert(source_id)
                && table
                    .get("command")
                    .and_then(toml::Value::as_str)
                    .is_some_and(|command| {
                        command
                            .split_whitespace()
                            .eq(rule.configured_argv.iter().map(String::as_str))
                    })
                && table.get("enabled").and_then(toml::Value::as_bool) == Some(true)
                && table.get("required").and_then(toml::Value::as_bool) == Some(true)
                && table.get("timeout_sec").and_then(toml::Value::as_integer)
                    == i64::try_from(rule.timeout_sec).ok(),
            "configured argv/timeout not joined to its named source rule"
        );
    }
    ensure!(
        joined == source_rules.keys().map(String::as_str).collect(),
        "configured source rule inventory not consumed exactly once"
    );
    ensure!(
        ids == BTreeSet::from([
            "configured-required-cargo-check",
            "configured-required-cargo-doc",
            "configured-required-policy-check"
        ]),
        "configured rule identities changed"
    );
    Ok(())
}

fn validate_overlaps(corpus: &Corpus) -> Result<()> {
    ensure!(
        corpus.manifest.overlaps.len() == 9,
        "fixed overlap inventory changed"
    );
    let mut ids = BTreeSet::new();
    for overlap in &corpus.manifest.overlaps {
        ensure!(
            ids.insert(overlap.id.as_str())
                && overlap.status == "unproven_equivalence"
                && overlap.avoided_work.is_none(),
            "overlap promoted to reuse or savings"
        );
        let left = named(corpus, &overlap.left)?;
        if overlap.id.starts_with("cross-workflow-") {
            let right = named(corpus, &overlap.right)?;
            let check = overlap.id.trim_start_matches("cross-workflow-");
            let task = if check == "verifier" {
                "artifact-verifier".to_owned()
            } else {
                format!("cargo-{check}")
            };
            ensure!(
                overlap.kind == "source_recipe_name_overlap"
                    && left.id == format!("candidate-sensor-{task}")
                    && right.id == format!("baseline-{check}")
                    && left.tested_commit != right.tested_commit
                    && left.workflow != right.workflow
                    && [
                        "tested_commit",
                        "revision_semantics",
                        "workflow_authority",
                        "target_root",
                        "candidate_actual_argv_unavailable",
                        "canonical_receipt_identity_unavailable"
                    ]
                    .iter()
                    .all(|required| overlap.differences.iter().any(|name| name == required)),
                "cross-workflow subject/authority distinction erased"
            );
        } else if overlap.id.starts_with("configured-vs-sensor-") {
            let check = overlap.id.trim_start_matches("configured-vs-sensor-");
            ensure!(
                ["check", "doc"].contains(&check)
                    && overlap.kind == "configured_recipe_overlap_only"
                    && left.id == format!("candidate-sensor-cargo-{check}")
                    && overlap.right == format!("configured-required-cargo-{check}")
                    && corpus
                        .manifest
                        .configured_rules
                        .iter()
                        .any(|rule| rule.id == overlap.right)
                    && overlap
                        .differences
                        .iter()
                        .any(|name| name == "configured_request_not_execution")
                    && overlap
                        .differences
                        .iter()
                        .any(|name| name == "Required_request_receipt_not_satisfied"),
                "configured/task overlap inferred as receipt reuse"
            );
        } else {
            ensure!(
                overlap.id == "coverage-vs-test"
                    && overlap.kind == "different_witness_semantics"
                    && left.id == "candidate-sensor-coverage"
                    && overlap.right == "candidate-sensor-cargo-test"
                    && [
                        "coverage_instrumentation",
                        "all_features_vs_ordinary_all_targets",
                        "coverage_not_succeeded",
                        "no_named_substitution_rule"
                    ]
                    .iter()
                    .all(|required| overlap.differences.iter().any(|name| name == required)),
                "coverage/test witness semantics collapsed"
            );
        }
    }
    Ok(())
}

fn validate(corpus: &Corpus) -> Result<()> {
    let manifest = &corpus.manifest;
    ensure!(
        manifest.schema == "ub-review.ci_efficiency_required_starvation.v1"
            && manifest.max_total_bytes == MAX_BYTES
            && manifest.fixed_fixture_only
            && manifest.repository == "EffortlessMetrics/ub-review"
            && manifest.issue == 1270
            && manifest.pull_request == 1458
            && manifest.frozen_main == MAIN
            && manifest.raw_head == HEAD
            && manifest.merge_result == MERGE
            && manifest.source_packet_sha256 == PACKET
            && !manifest.scope.trim().is_empty()
            && manifest.unknowns.len() >= 5,
        "unsupported fixed fixture identity or scope"
    );
    ensure!(
        manifest
            .measurements
            .keys()
            .map(String::as_str)
            .collect::<BTreeSet<_>>()
            == BTreeSet::from([
                "billing",
                "savings",
                "summed_process_time",
                "critical_path",
                "artifact_expanded_bytes",
                "artifact_compressed_bytes",
                "avoided_executions",
                "independent_execution_count"
            ])
            && manifest.measurements.values().all(Option::is_none),
        "unsupported economics or independent execution total"
    );
    validate_sources(corpus)?;
    validate_workflows(corpus)?;
    let mut ids = BTreeSet::new();
    let mut kinds = BTreeMap::<&str, usize>::new();
    ensure!(
        manifest.observations.len() == 36,
        "fixed observation inventory changed"
    );
    for row in &manifest.observations {
        ensure!(
            !row.id.is_empty() && ids.insert(row.id.as_str()),
            "duplicate or missing observation identity"
        );
        *kinds.entry(row.kind.as_str()).or_default() += 1;
        validate_observation(corpus, row)?;
    }
    ensure!(
        kinds
            == BTreeMap::from([
                ("executed_command", 16),
                ("reported_executed_task", 14),
                ("consumer_observation", 6)
            ]),
        "nested tasks or consumers counted as independent executions"
    );
    validate_counterexample(corpus)?;
    validate_rules(corpus)?;
    validate_overlaps(corpus)?;
    Ok(())
}

// These fixed-fixture controls mutate decoded evidence independently of the
// byte lock, then require the particular substantive join to reject the claim.
fn rejected_value(corpus: &Corpus, pointer: &str, value: Value, reason: &str) -> Result<()> {
    let mut data = serde_json::to_value(corpus)?;
    *data
        .pointer_mut(pointer)
        .context("mutation pointer missing")? = value;
    let changed: Corpus = serde_json::from_value(data)?;
    let error = validate(&changed)
        .err()
        .context("negative control unexpectedly accepted")?;
    ensure!(
        error.to_string().contains(reason),
        "negative control {pointer} rejected at wrong join: {error}"
    );
    Ok(())
}

fn copied_fixture() -> Result<tempfile::TempDir> {
    let directory = tempfile::tempdir()?;
    for entry in fs::read_dir(root())? {
        let entry = entry?;
        fs::copy(entry.path(), directory.path().join(entry.file_name()))?;
    }
    Ok(directory)
}

#[test]
fn fixed_required_starvation_fixture_joins_sources() -> Result<()> {
    let corpus = load(&root())?;
    ensure!(
        corpus.manifest.observations.len() == 36,
        "missing fixed observations"
    );
    ensure!(
        named(&corpus, "candidate-reconcile-tasks")?.command_exit_code == Some(2),
        "command exit 2 lost"
    );
    ensure!(
        named(&corpus, "candidate-sensor-ripr")?.result.as_deref() == Some("reported_succeeded"),
        "sensor disposition lost"
    );
    ensure!(
        named(&corpus, "candidate-gate-consumer")?.result.as_deref() == Some("failure"),
        "gate contradiction lost"
    );
    Ok(())
}

#[test]
fn semantic_row_controls_reject_authority_and_execution_overclaims() -> Result<()> {
    use serde_json::json;
    let corpus = load(&root())?;
    for (identity, field, value, reason) in [
        (
            "baseline-check",
            "job_id",
            json!(115_707_918_657_u64),
            "public log source/job mismatch",
        ),
        (
            "baseline-test",
            "observed_argv",
            json!(["cargo", "test", "--all-features"]),
            "baseline argv differs",
        ),
        (
            "baseline-policy",
            "run_id",
            json!(CANDIDATE_RUN),
            "run/job identity mismatch",
        ),
        (
            "baseline-test",
            "source",
            json!("baseline-doc"),
            "command/source identity changed",
        ),
        (
            "candidate-sensor-cargo-check",
            "tested_commit",
            json!(HEAD),
            "checkout subjects collapsed",
        ),
        (
            "candidate-proof-head-1e1c35087a81",
            "tested_commit",
            json!(HEAD),
            "proof-head label promoted",
        ),
        (
            "candidate-proof-head-1e1c35087a81",
            "observed_argv",
            json!(["cargo", "test"]),
            "reported task identity or argv inference",
        ),
        (
            "candidate-proof-head-1e1c35087a81",
            "result",
            json!("success"),
            "reported task outcome fabricated",
        ),
        (
            "candidate-sensor-cargo-doc",
            "observed_argv",
            json!(["cargo", "doc"]),
            "reported task identity or argv inference",
        ),
        (
            "candidate-sensor-cargo-check",
            "source_pointer",
            json!("/missing"),
            "invalid observation source pointer",
        ),
        (
            "candidate-sensor-cargo-check",
            "source_pointer",
            json!("/issues/21"),
            "reported task identity or argv inference",
        ),
        (
            "candidate-sensor-coverage",
            "result",
            json!("reported_succeeded"),
            "reported task outcome fabricated",
        ),
        (
            "candidate-required-proof-0000-291419f6f509",
            "receipt_satisfied",
            json!(true),
            "Required starvation consumer or source erased",
        ),
        (
            "candidate-required-proof-0000-291419f6f509",
            "process_started",
            json!(true),
            "consumer counted as execution",
        ),
        (
            "candidate-required-proof-0000-291419f6f509",
            "source_pointer",
            json!("/observations/4"),
            "Required starvation consumer or source erased",
        ),
        (
            "candidate-required-proof-0000-291419f6f509",
            "kind",
            json!("executed_command"),
            "unknown fixed wrapper identity",
        ),
        (
            "candidate-reconcile-tasks",
            "command_exit_code",
            json!(0),
            "tolerated command failure normalized",
        ),
        (
            "candidate-reconcile-tasks",
            "workflow_continue_on_error",
            json!(false),
            "tolerated command failure normalized",
        ),
        (
            "candidate-reconcile-tasks",
            "result",
            json!("success"),
            "wrapper outcome differs from source step/report",
        ),
        (
            "candidate-gate-consumer",
            "result",
            json!("success"),
            "product gate outcome normalized",
        ),
        (
            "candidate-coverage-publication-consumer",
            "result",
            json!("success"),
            "coverage publication success fabricated",
        ),
        (
            "baseline-test",
            "process_time_ms",
            json!(0),
            "unknown process time fabricated",
        ),
    ] {
        let index = corpus
            .manifest
            .observations
            .iter()
            .position(|row| row.id == identity)
            .context("mutation row missing")?;
        rejected_value(
            &corpus,
            &format!("/manifest/observations/{index}/{field}"),
            value,
            reason,
        )?;
    }
    Ok(())
}

#[test]
fn semantic_source_and_claim_controls_reject_unsupported_joins() -> Result<()> {
    use serde_json::json;
    let corpus = load(&root())?;
    for (pointer, value, reason) in [
        (
            "/manifest/workflows/baseline/definition_commit",
            json!("a".repeat(40)),
            "definition, authority or subject collapsed",
        ),
        (
            "/manifest/workflows/candidate/authority",
            json!("protected-base-definition"),
            "definition, authority or subject collapsed",
        ),
        (
            "/manifest/workflows/baseline/target_root",
            json!("target/ub-review"),
            "definition, authority or subject collapsed",
        ),
        (
            "/manifest/configured_rules/0/runtime_request_identity",
            json!("proof-0000-291419f6f509"),
            "configured rule promoted",
        ),
        (
            "/manifest/configured_rules/0/execution_observed",
            json!(true),
            "configured rule promoted",
        ),
        (
            "/manifest/configured_rules/0/configured_argv",
            json!(["cargo", "check", "--all-features"]),
            "configured argv/timeout not joined",
        ),
        (
            "/manifest/configured_rules/0/timeout_sec",
            json!(1),
            "configured argv/timeout not joined",
        ),
        (
            "/sources/projections/shadow/value/input_unavailable",
            json!(false),
            "unavailable input normalized",
        ),
        (
            "/sources/projections/shadow/value/authority",
            json!("enforcing"),
            "shadow contradiction",
        ),
        (
            "/sources/projections/shadow/value/counts/executed_sensor_tasks",
            json!(999),
            "shadow task count differs",
        ),
        (
            "/manifest/overlaps/0/status",
            json!("equivalent"),
            "overlap promoted",
        ),
        (
            "/manifest/overlaps/0/avoided_work",
            json!(0),
            "overlap promoted",
        ),
        (
            "/manifest/measurements/independent_execution_count",
            json!(36),
            "unsupported economics or independent execution total",
        ),
        (
            "/manifest/measurements/savings",
            json!(0),
            "unsupported economics",
        ),
        (
            "/sources/projections/baseline-test/source",
            json!("unregistered"),
            "projection source identity missing",
        ),
        (
            "/sources/captures/candidate-job-log/complete_bytes_retained",
            json!(true),
            "source/projection collapse",
        ),
        (
            "/sources/captures/candidate-shadow-projection/capture_basis",
            json!("decoded_public_source_bytes"),
            "capture/source byte basis collapsed",
        ),
        (
            "/manifest/observations/1",
            serde_json::to_value(&corpus.manifest.observations[0])?,
            "duplicate or missing observation identity",
        ),
    ] {
        rejected_value(&corpus, pointer, value, reason)?;
    }
    Ok(())
}

#[test]
fn semantic_set_controls_keep_required_timeout_and_witness_boundaries() -> Result<()> {
    let corpus = load(&root())?;
    let mut data = serde_json::to_value(&corpus)?;
    data["manifest"]["observations"]
        .as_array_mut()
        .context("observations missing")?
        .retain(|row| {
            row.get("id").and_then(Value::as_str)
                != Some("candidate-required-proof-0000-291419f6f509")
        });
    rejected_value(
        &corpus,
        "/manifest/observations",
        data["manifest"]["observations"].clone(),
        "fixed observation inventory changed",
    )?;
    let mut data = serde_json::to_value(&corpus)?;
    data["sources"]["projections"]["shadow"]["value"]["issues"]
        .as_array_mut()
        .context("issues missing")?
        .retain(|row| {
            row.get("code").and_then(Value::as_str)
                != Some("timeout_ceilings_exhausted_budget_before_actual_deadline")
        });
    rejected_value(
        &corpus,
        "/sources/projections/shadow/value/issues",
        data["sources"]["projections"]["shadow"]["value"]["issues"].clone(),
        "timeout diagnostic erased",
    )?;
    let mut data = serde_json::to_value(&corpus)?;
    data["manifest"]["overlaps"]
        .as_array_mut()
        .context("overlaps missing")?
        .pop()
        .context("overlap missing")?;
    rejected_value(
        &corpus,
        "/manifest/overlaps",
        data["manifest"]["overlaps"].clone(),
        "fixed overlap inventory changed",
    )?;
    for (identity, removed, reason) in [
        (
            "cross-workflow-check",
            "revision_semantics",
            "subject/authority distinction erased",
        ),
        (
            "coverage-vs-test",
            "coverage_instrumentation",
            "coverage/test witness semantics collapsed",
        ),
    ] {
        let index = corpus
            .manifest
            .overlaps
            .iter()
            .position(|row| row.id == identity)
            .context("overlap missing")?;
        let differences: Vec<_> = corpus.manifest.overlaps[index]
            .differences
            .iter()
            .filter(|name| name.as_str() != removed)
            .collect();
        rejected_value(
            &corpus,
            &format!("/manifest/overlaps/{index}/differences"),
            serde_json::to_value(differences)?,
            reason,
        )?;
    }
    Ok(())
}

#[test]
fn loader_rejects_digest_inventory_paths_and_size_drift() -> Result<()> {
    let directory = copied_fixture()?;
    let path = directory.path().join("source-projections.json");
    let mut bytes = fs::read(&path)?;
    bytes.push(b' ');
    fs::write(&path, bytes)?;
    ensure!(
        load(directory.path())
            .err()
            .context("tampered projection accepted")?
            .to_string()
            .contains("projection byte receipt mismatch"),
        "digest control failed"
    );
    let directory = copied_fixture()?;
    fs::write(directory.path().join("unlisted.json"), b"{}")?;
    ensure!(
        load(directory.path())
            .err()
            .context("unlisted file accepted")?
            .to_string()
            .contains("unlisted or missing"),
        "inventory control failed"
    );
    let directory = copied_fixture()?;
    let path = directory.path().join("manifest.json");
    let mut value: Value = serde_json::from_slice(&fs::read(&path)?)?;
    value["files"][0]["path"] = Value::String("../source-projections.json".to_owned());
    fs::write(path, serde_json::to_vec(&value)?)?;
    ensure!(
        load(directory.path())
            .err()
            .context("escaping path accepted")?
            .to_string()
            .contains("path escapes"),
        "path control failed"
    );
    let directory = copied_fixture()?;
    fs::write(
        directory.path().join("oversize.json"),
        vec![b' '; usize::try_from(MAX_BYTES)?],
    )?;
    ensure!(
        load(directory.path())
            .err()
            .context("oversize corpus accepted")?
            .to_string()
            .contains("exceeds byte budget"),
        "byte budget control failed"
    );
    Ok(())
}

#[test]
fn loader_rejects_private_transfers_payloads_and_missing_explicit_unknowns() -> Result<()> {
    for (field, value, error) in [
        (
            "scope",
            Value::String("https://example.invalid/?X-Amz-Signature=secret".to_owned()),
            "private or credential-like",
        ),
        (
            "transfer_metadata",
            Value::String("private".to_owned()),
            "private payload field",
        ),
    ] {
        let directory = copied_fixture()?;
        let path = directory.path().join("manifest.json");
        let mut data: Value = serde_json::from_slice(&fs::read(&path)?)?;
        data[field] = value;
        fs::write(path, serde_json::to_vec(&data)?)?;
        ensure!(
            load(directory.path())
                .err()
                .context("private fixture accepted")?
                .to_string()
                .contains(error),
            "privacy control failed"
        );
    }
    let directory = copied_fixture()?;
    let path = directory.path().join("manifest.json");
    let mut data: Value = serde_json::from_slice(&fs::read(&path)?)?;
    data["observations"][0]
        .as_object_mut()
        .context("observation missing")?
        .remove("process_time_ms");
    fs::write(path, serde_json::to_vec(&data)?)?;
    ensure!(
        load(directory.path())
            .err()
            .context("missing unknown accepted")?
            .to_string()
            .contains("missing explicit observation unknown"),
        "explicit unknown control failed"
    );
    Ok(())
}

#[test]
fn reordering_preserves_source_joins_without_rewriting_fixed_bytes() -> Result<()> {
    let mut corpus = load(&root())?;
    corpus.manifest.observations.reverse();
    corpus.manifest.configured_rules.reverse();
    corpus.manifest.overlaps.reverse();
    validate(&corpus)
}

#[cfg(unix)]
#[test]
fn loader_rejects_symlinked_fixture_member() -> Result<()> {
    let directory = copied_fixture()?;
    let path = directory.path().join("source-projections.json");
    fs::remove_file(&path)?;
    std::os::unix::fs::symlink(root().join("source-projections.json"), path)?;
    ensure!(
        load(directory.path())
            .err()
            .context("linked member accepted")?
            .to_string()
            .contains("linked fixture member"),
        "symlink control failed"
    );
    Ok(())
}

// Exercise review counterexamples through the real disk loader as well as the
// semantic validator. Only manifest claims change; projection bytes stay fixed.
fn rejected_manifest(
    corpus: &Corpus,
    mutate: impl FnOnce(&mut Value) -> Result<()>,
    reason: &str,
) -> Result<()> {
    let directory = copied_fixture()?;
    let mut manifest = serde_json::to_value(&corpus.manifest)?;
    mutate(&mut manifest)?;
    fs::write(
        directory.path().join("manifest.json"),
        serde_json::to_vec(&manifest)?,
    )?;
    let error = load(directory.path())
        .err()
        .context("manifest-only counterexample accepted")?;
    ensure!(
        error.to_string().contains(reason),
        "manifest-only counterexample rejected at wrong join: {error}"
    );
    Ok(())
}

fn manifest_row<'a>(manifest: &'a mut Value, id: &str) -> Result<&'a mut Value> {
    manifest
        .get_mut("observations")
        .and_then(Value::as_array_mut)
        .context("manifest observations missing")?
        .iter_mut()
        .find(|row| row.get("id").and_then(Value::as_str) == Some(id))
        .context("manifest mutation row missing")
}

#[test]
fn loader_rejects_shortened_wrapper_argv_at_complete_command_join() -> Result<()> {
    let corpus = load(&root())?;
    rejected_manifest(
        &corpus,
        |manifest| {
            manifest_row(manifest, "candidate-test-projections")?["observed_argv"] =
                serde_json::json!(["python"]);
            Ok(())
        },
        "wrapper argv not in command log projection",
    )
}

#[test]
fn loader_rejects_swapped_configured_recipes_at_named_rule_join() -> Result<()> {
    let corpus = load(&root())?;
    rejected_manifest(
        &corpus,
        |manifest| {
            let check = manifest["configured_rules"][0]["configured_argv"].clone();
            let doc = manifest["configured_rules"][1]["configured_argv"].clone();
            manifest["configured_rules"][0]["configured_argv"] = doc;
            manifest["configured_rules"][1]["configured_argv"] = check;
            Ok(())
        },
        "configured argv/timeout not joined to its named source rule",
    )
}

#[test]
fn loader_rejects_forged_ordinary_wrapper_outcome_at_step_report_join() -> Result<()> {
    let corpus = load(&root())?;
    rejected_manifest(
        &corpus,
        |manifest| {
            manifest_row(manifest, "candidate-test-publication")?["result"] =
                Value::String("command_failure_tolerated_by_workflow".to_owned());
            Ok(())
        },
        "wrapper outcome differs from source step/report",
    )
}
