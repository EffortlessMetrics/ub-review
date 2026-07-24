//! Box-aware evidence packet runner for UB-focused PR review.
//!
//! The binary prepares deterministic receipts, model-review artifacts, and lane
//! packets. Posting is a separate command that submits one grouped pull request
//! review when explicitly configured.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::Parser;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wait_timeout::ChildExt;

mod cli;
use cli::*;
mod config;
use config::*;
mod builtin;
use builtin::*;
mod gate;
use gate::*;
mod artifacts;
use artifacts::*;
mod proof;
pub(crate) use proof::*;
mod tools;
pub(crate) use tools::*;
mod lanes;
pub(crate) use lanes::*;
mod observations;
pub(crate) use observations::*;

mod sensors;
pub(crate) use sensors::*;
mod prompt_cache;
pub(crate) use prompt_cache::*;
mod providers;
pub(crate) use providers::*;
mod init;
pub(crate) use init::*;
mod enable;
pub(crate) use enable::*;
mod promotion;
pub(crate) use promotion::*;
mod claim_graph;
pub(crate) use claim_graph::*;
mod review_topics;
pub(crate) use review_topics::*;
mod impact_plan;
pub(crate) use impact_plan::*;
mod model_api;
pub(crate) use model_api::*;
mod model_exec;
pub(crate) use model_exec::*;
mod validate;
pub(crate) use validate::*;
mod render;
pub(crate) use render::*;
mod decision_core;
pub(crate) use decision_core::*;
mod noise;
pub(crate) use noise::*;
mod diff_class;
pub(crate) use diff_class::*;
mod observation_build;
pub(crate) use observation_build::*;
mod candidate;
pub(crate) use candidate::*;
mod issue_broker;
pub(crate) use issue_broker::*;
mod quality_backfill_build;
pub(crate) use quality_backfill_build::*;
mod fill_ledger;
pub(crate) use fill_ledger::*;
mod follow_up_routing;
pub(crate) use follow_up_routing::*;
mod plan_build;
pub(crate) use plan_build::*;
mod lane_packets;
pub(crate) use lane_packets::*;
mod lane_threads;
pub(crate) use lane_threads::*;
mod cross_lane_messages;
pub(crate) use cross_lane_messages::*;
mod reporter;
pub(crate) use reporter::*;
mod calibration;
pub(crate) use calibration::{
    CalibrationArtifact, read_messages_ndjson, write_calibration_artifact,
};
mod review_compiler;
pub(crate) use review_compiler::*;
mod cost_artifact;
#[cfg(test)]
mod review_experience;
pub(crate) use cost_artifact::*;
mod quality_artifact;
use quality_artifact::{write_quality_receipt_artifact, write_quality_trend_artifact};
mod quality_github;
pub(crate) use quality_github::*;
mod artifact_writers;
pub(crate) use artifact_writers::*;
mod witness;
pub(crate) use witness::*;
mod work_queue;
pub(crate) use work_queue::*;
mod observation_merge;
mod test_parse;
pub(crate) use observation_merge::*;
mod shared_context_render;
pub(crate) use shared_context_render::*;
mod pr_thread_context;
pub(crate) use pr_thread_context::*;
mod receipt_builders;
pub(crate) use receipt_builders::*;
mod model_orchestration;
pub(crate) use model_orchestration::*;
mod proof_planner_lane;
pub(crate) use proof_planner_lane::*;
mod review_text;
pub(crate) use review_text::*;
mod github_validation;
pub(crate) use github_validation::*;
mod summary_render;
pub(crate) use summary_render::*;
mod post_run_utils;
pub(crate) use post_run_utils::*;
mod system_detect;
use system_detect::{
    command_path, command_version, detect_disk_free_mb, detect_load_1m, detect_mem_available_mb,
    doctor_binary_install_status, git_tree_sha, profile_config_hash,
};
mod diff_posture;
mod run_args;
pub(crate) use run_args::*;
mod post_command;
pub(crate) use post_command::*;
mod plan_artifacts;
pub(crate) use plan_artifacts::*;
mod ci_audit;
pub(crate) use ci_audit::*;
mod gate_watchdog;
pub(crate) use gate_watchdog::*;

const STANDARD_LANE_WIDTH: usize = 10;
const STANDARD_MODEL_CONCURRENCY: usize = 8;
const STANDARD_MAX_MODEL_CALLS: usize = 14;
const DEFAULT_REVIEW_PROFILE: &str = "bun-ub-v0";
const TOKMD_ANALYZE_PRESET: &str = "bun-ub";
const DEFAULT_INITIAL_PACKET_DEADLINE_SEC: u64 = 60;
const DEFAULT_FOLLOW_UP_PACKET_DEADLINE_SEC: u64 = 300;
const PRIOR_RESOLVED_CANDIDATES_ARTIFACT: &str = "review/prior_resolved_candidates.json";

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => cmd_init(args),
        Command::Enable(args) => cmd_enable(args),
        Command::Status(args) => cmd_status(args),
        Command::Recommend(args) => cmd_recommend(args),
        Command::Promote(args) => cmd_promote(args),
        Command::Doctor(args) => cmd_doctor(args),
        Command::Cache(args) => cmd_cache(args),
        Command::Plan(args) => cmd_plan(args),
        Command::Run(args) => {
            let completion = cmd_run(*args)?;
            if let Some(message) = run_gate_failure_message(&completion) {
                bail!("{message}");
            }
            Ok(())
        }
        Command::Summary(args) => cmd_summary(args),
        Command::Post(args) => cmd_post(args),
        Command::AuditCi(args) => cmd_audit_ci(args),
        Command::SetupCi(args) => cmd_setup_ci(args),
        Command::QualityBackfill(args) => cmd_quality_backfill(args),
        Command::QualityGithubOutcomes(args) => cmd_quality_github_outcomes(args),
        Command::QualityGithubCollect(args) => cmd_quality_github_collect(args),
        Command::GateCheck(args) => cmd_gate_check(args),
        Command::GateWatchdog(args) => cmd_gate_watchdog(args),
        Command::Worker(args) => cmd_worker(args),
    }
}

/// Execute a single proof request and write its receipt.
/// (Order 8 of epic #655 — the execution-plane worker command.)
///
/// Reads a typed `proof_request.v2` JSON, deserializes it into a
/// `ProofRequestV2`, validates the schema tag, resolves it via the executor
/// adapter, executes the approved command, and writes the receipt to the
/// output directory. This enables distributed proof execution: a `plan` job
/// emits proof requests, `worker` jobs execute them (locally or remotely),
/// and a `finalize` job (currently `run`) collects receipts and produces
/// the gate verdict.
///
/// Security contract: the worker only executes commands produced by
/// `resolve_proof_command` from a typed, allowlisted intent. It never parses
/// a free-form `target` string into argv. An intent that does not resolve to
/// an approved command template is written as a `skipped_unresolved` receipt
/// and never executed. This is the boundary that keeps distributed workers
/// safe to run on untrusted-but-typed requests: the type system + allowlist
/// are the trust root, not the request string.
fn cmd_worker(args: WorkerArgs) -> Result<()> {
    let request_path = Path::new(&args.proof_request);
    let out = Path::new(&args.out);
    let root = Path::new(&args.root);

    // Read and deserialize the typed proof request. A `serde_json::Value`
    // grab of `kind`/`target` would bypass schema validation and is
    // deliberately not used: the worker must consume the same `ProofRequestV2`
    // struct the planner emits, so a malformed or foreign-shaped request is
    // rejected at deserialization rather than reaching the executor adapter.
    let request_json = fs::read_to_string(request_path)
        .with_context(|| format!("read proof request: {}", request_path.display()))?;
    let request: ProofRequestV2 =
        serde_json::from_str(&request_json).context("parse proof_request.v2 JSON")?;

    // Enforce the schema tag so a v1 request (or arbitrary JSON) cannot be
    // processed as v2. `deny_unknown_fields` is intentionally NOT set on the
    // struct (it serializes extra metadata for the planner); the schema field
    // is the explicit version gate.
    anyhow::ensure!(
        request.schema == crate::artifacts::PROOF_REQUEST_V2_SCHEMA,
        "worker requires {}, got `{}`",
        crate::artifacts::PROOF_REQUEST_V2_SCHEMA,
        request.schema,
    );

    let kind = request.kind;
    let kind_str = kind.key().to_owned();
    let target = request.target.as_str();

    // Check nightly availability.
    let nightly = std::process::Command::new("cargo")
        .args(["+nightly", "--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    // Resolve via the executor adapter. `None` means the typed intent does
    // not map to an approved command template — this is the only path to an
    // executable argv. There is no fallback that turns a raw target string
    // into argv.
    let resolved = resolve_proof_command(&kind, target, nightly);
    let Some(cmd) = resolved else {
        let receipt = serde_json::json!({
            "schema": "ub-review.proof_receipt.v1",
            "kind": kind_str,
            "result": "skipped_unresolved",
            "reason": format!("executor adapter could not resolve {kind_str} intent"),
        });
        let receipt_path = out.join("proof_receipt.json");
        if let Some(dir) = receipt_path.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&receipt_path, serde_json::to_string_pretty(&receipt)?)?;
        println!(
            "worker: proof request {kind_str} unresolved, receipt written to {}",
            receipt_path.display()
        );
        return Ok(());
    };

    // Execute via the SAME canonical path local proof uses, so worker and
    // local execution emit equivalent receipts. This gives the worker the
    // broker's lease admission, wall-clock timeout (WorkerArgs.timeout_sec),
    // bounded stdout/stderr artifact files, and the canonical
    // ProofCommandReceipt — closing the Order 8 "equivalent receipts" gap.
    //
    // The runner (`run_command_to_files`) is the production focused-proof
    // runner: it spawns the process, applies `wait_timeout`, kills on timeout,
    // and writes stdout/stderr to the paths `run_proof_command_receipt`
    // allocates under proof/<id>/head/.
    let timeout_sec = args.timeout_sec.max(request.timeout_sec);
    let env_map: BTreeMap<String, String> = cmd.env.iter().cloned().collect();
    let spec = ProofCommandSpec {
        argv: cmd.argv.clone(),
        env: env_map,
    };
    let lease = ResourceLease {
        schema: crate::artifacts::RESOURCE_LEASE_SCHEMA.to_owned(),
        id: format!("worker-lease-{}", request.id),
        kind: kind_str.clone(),
        consumer: request.id.clone(),
        status: "granted".to_owned(),
        reason: "worker proof lease granted".to_owned(),
        cpu: 1,
        memory_mb: 512,
        disk_mb: 64,
        timeout_sec,
        network: false,
        scratch: true,
        worktree: None,
        command: Some(format!("head: {}", cmd.argv.join(" "))),
    };
    let mut runner = crate::run_command_to_files;
    let command_receipt = run_proof_command_receipt(
        ProofCommandInvocation {
            command_root: root,
            out,
            receipt_id: &request.id,
            side: "head",
            spec: &spec,
            timeout_sec,
            lease: &lease,
        },
        &mut runner,
    )?;

    // Per-ProofKind result classification. The old worker labeled ANY failure
    // of a nightly-requiring command as `sanitizer_ub_detected`, conflating
    // sanitizer UB with Miri findings, compile errors, missing components, and
    // ordinary test failures. Classification now keys off ProofKind + the
    // command receipt's status/timed_out, so each witness kind gets its own
    // outcome string.
    let result = classify_worker_proof_result(&kind, &command_receipt);
    let reason = format!("{kind_str}: {result}");

    // Canonical ProofReceipt — same schema and fields the broker's
    // focused_head_receipt / focused_build_receipt produce, stamped with the
    // base/head identity and requesters from the v2 request.
    let receipt = ProofReceipt {
        schema: crate::artifacts::PROOF_RECEIPT_SCHEMA.to_owned(),
        id: request.id.clone(),
        kind: kind_str.clone(),
        base: request.base.clone(),
        head: request.head.clone(),
        test_patch_mode: "head-only".to_owned(),
        requested_by: request.requested_by.clone(),
        request_ids: request.claim_ids.clone(),
        commands: vec![command_receipt],
        result,
        reason,
    };

    let lease_path = out.join("resource_lease.json");
    if let Some(dir) = lease_path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&lease_path, serde_json::to_string_pretty(&lease)?)?;
    let receipt_path = out.join("proof_receipt.json");
    if let Some(dir) = receipt_path.parent() {
        fs::create_dir_all(dir)?;
    }
    fs::write(&receipt_path, serde_json::to_string_pretty(&receipt)?)?;
    println!(
        "worker: proof request {} ({kind_str}) result '{}'; receipt written to {}",
        request.id,
        receipt.result,
        receipt_path.display()
    );
    Ok(())
}

/// Classify a worker proof result from the ProofKind and the executed command
/// receipt. Replaces the previous `requires_nightly → sanitizer_ub_detected`
/// heuristic, which over-claimed UB for any nightly failure (Miri, compile
/// errors, missing components, ordinary test failures).
///
/// Outcome vocabulary:
/// - `passed` / `head_passed`: command exited 0.
/// - `timed_out`: wall-clock timeout exceeded (lease/runner killed the process).
/// - `sanitizer_ub_detected`: sanitizer-witness command failed (potential UB or
///   sanitizer runtime abort). Still a `failed`-class signal, but scoped to
///   SanitizerWitness so Miri/compile failures are not mislabeled.
/// - `failed`: any other nonzero exit (build break, test failure, etc.).
/// - `skipped`: the command could not run (lease not granted, spawn error).
fn classify_worker_proof_result(kind: &ProofKind, command: &ProofCommandReceipt) -> String {
    match command.status.as_str() {
        "passed" => "passed".to_owned(),
        "timed_out" => "timed_out".to_owned(),
        "skipped" => "skipped".to_owned(),
        "failed" => match kind {
            ProofKind::SanitizerWitness => "sanitizer_ub_detected".to_owned(),
            _ => "failed".to_owned(),
        },
        other => other.to_owned(),
    }
}

#[cfg(test)]
mod worker_proof_tests {
    use super::*;
    use crate::proof::ProofCommandSpec;

    fn receipt_with_status(status: &str) -> ProofCommandReceipt {
        ProofCommandReceipt {
            side: "head".to_owned(),
            command: "cargo test --locked --test x".to_owned(),
            env: BTreeMap::new(),
            status: status.to_owned(),
            exit_code: Some(1),
            timed_out: false,
            timeout_sec: 300,
            duration_ms: 100,
            stdout: String::new(),
            stderr: String::new(),
            reason: "stub".to_owned(),
        }
    }

    /// The old worker labeled ANY nightly failure as `sanitizer_ub_detected`.
    /// A Miri failure must NOT be mislabeled as sanitizer UB — it is a generic
    /// `failed` under per-kind classification. Only SanitizerWitness failures
    /// carry the UB-detected result.
    #[test]
    fn classify_scopes_sanitizer_ub_to_sanitizer_kind_only() {
        assert_eq!(
            classify_worker_proof_result(
                &ProofKind::SanitizerWitness,
                &receipt_with_status("failed")
            ),
            "sanitizer_ub_detected",
            "sanitizer-witness failure is UB-class"
        );
        assert_eq!(
            classify_worker_proof_result(&ProofKind::MiriWitness, &receipt_with_status("failed")),
            "failed",
            "miri failure must not be mislabeled as sanitizer UB"
        );
        assert_eq!(
            classify_worker_proof_result(&ProofKind::FocusedTest, &receipt_with_status("failed")),
            "failed",
            "focused-test failure is a generic failure"
        );
        assert_eq!(
            classify_worker_proof_result(&ProofKind::FocusedBuild, &receipt_with_status("failed")),
            "failed",
            "focused-build failure is a generic failure"
        );
    }

    #[test]
    fn classify_passes_through_passed_timed_out_skipped() {
        assert_eq!(
            classify_worker_proof_result(&ProofKind::FocusedTest, &receipt_with_status("passed")),
            "passed"
        );
        assert_eq!(
            classify_worker_proof_result(
                &ProofKind::SanitizerWitness,
                &receipt_with_status("timed_out")
            ),
            "timed_out",
            "timeout is timeout regardless of kind"
        );
        assert_eq!(
            classify_worker_proof_result(&ProofKind::MiriWitness, &receipt_with_status("skipped")),
            "skipped"
        );
    }

    /// Worker and local execution must emit the SAME canonical ProofReceipt
    /// schema and core fields. This constructs a worker receipt through the
    /// same `run_proof_command_receipt` path the broker uses, then asserts the
    /// resulting ProofReceipt matches the canonical shape (schema, base/head,
    /// requested_by, request_ids, commands[].stdout artifact path) that local
    /// focused-proof receipts produce. This is the Order 8 "shared
    /// local/remote receipts (same schema)" end-state criterion.
    #[test]
    fn worker_canonical_receipt_matches_local_schema_shape() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let spec = ProofCommandSpec {
            argv: vec!["cargo".to_owned(), "test".to_owned(), "--locked".to_owned()],
            env: BTreeMap::new(),
        };
        let lease = ResourceLease {
            schema: crate::artifacts::RESOURCE_LEASE_SCHEMA.to_owned(),
            id: "worker-lease-req-1".to_owned(),
            kind: "focused-test".to_owned(),
            consumer: "req-1".to_owned(),
            status: "granted".to_owned(),
            reason: "test lease".to_owned(),
            cpu: 1,
            memory_mb: 512,
            disk_mb: 64,
            timeout_sec: 30,
            network: false,
            scratch: true,
            worktree: None,
            command: None,
        };
        // Use the real production runner against an inert command (echo on the
        // runner's argv is not allowlisted; instead exercise the path with a
        // no-op runner that writes the stream files, proving the canonical
        // receipt + artifact-path contract).
        let mut runner = |_root: &Path,
                          _argv: &[String],
                          _env: &BTreeMap<String, String>,
                          _timeout: u64,
                          stdout: &Path,
                          stderr: &Path| {
            fs::write(stdout, b"ok\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(0),
                timed_out: false,
                success: true,
                reason: "completed".to_owned(),
                duration_ms: 5,
            })
        };
        let command_receipt = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: "req-1",
                side: "head",
                spec: &spec,
                timeout_sec: 30,
                lease: &lease,
            },
            &mut runner,
        )?;

        // Assemble the canonical ProofReceipt exactly as cmd_worker does.
        let receipt = ProofReceipt {
            schema: crate::artifacts::PROOF_RECEIPT_SCHEMA.to_owned(),
            id: "req-1".to_owned(),
            kind: "focused-test".to_owned(),
            base: "abc1234".to_owned(),
            head: "def5678".to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids: vec!["claim-7".to_owned()],
            commands: vec![command_receipt.clone()],
            result: "passed".to_owned(),
            reason: "focused-test: passed".to_owned(),
        };

        // Canonical schema + identity fields present and non-empty.
        assert_eq!(receipt.schema, "ub-review.proof_receipt.v1");
        assert_eq!(receipt.base, "abc1234");
        assert_eq!(receipt.head, "def5678");
        assert_eq!(receipt.requested_by, vec!["tests-oracle".to_owned()]);
        assert_eq!(receipt.request_ids, vec!["claim-7".to_owned()]);
        assert_eq!(receipt.commands.len(), 1);
        // stdout/stderr are artifact PATHS (not inline content) and the files
        // exist on disk — the bounding + artifact contract the old ad-hoc
        // worker receipt lacked.
        let stdout_rel = &receipt.commands[0].stdout;
        assert!(
            stdout_rel.starts_with("proof/req-1/head/"),
            "stdout must be an artifact path under proof/<id>/head/, got {stdout_rel}"
        );
        assert!(
            out.join(stdout_rel).exists(),
            "stdout artifact must exist on disk"
        );
        assert!(receipt.commands[0].stdout != receipt.commands[0].stderr);
        // Serialize the whole receipt — it must round-trip as the canonical
        // schema (the same JSON shape the gate and finalize consume).
        let json = serde_json::to_string(&receipt)?;
        let back: ProofReceipt = serde_json::from_str(&json)?;
        assert_eq!(back.id, "req-1");
        assert_eq!(back.commands.len(), 1);
        assert_eq!(back.commands[0].status, "passed");
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct PostErrorReceipt {
    schema_version: u32,
    status: String,
    error_kind: String,
    failure_stage: String,
    reason: String,
    review_json: String,
    review_json_exists: bool,
    review_json_valid: bool,
    review_event: Option<String>,
    review_body_bytes: Option<usize>,
    review_comment_count: Option<usize>,
    diff_patch: String,
    diff_patch_exists: bool,
    diff_patch_valid: bool,
    diff_line_count: Option<usize>,
    off_diff_comment_count: Option<usize>,
    repo: Option<String>,
    repo_valid: bool,
    pull_number: Option<u64>,
    comments: Option<usize>,
    http_status: Option<u16>,
    token_present: bool,
    payload_written: bool,
    would_post: bool,
    failure_tolerated: bool,
    fail_on_post_error: bool,
}

#[derive(Debug, Serialize)]
struct PostResultReceipt {
    schema_version: u32,
    status: String,
    repo: String,
    repo_valid: bool,
    pull_number: u64,
    comments: usize,
    review_json: String,
    review_json_exists: bool,
    review_json_valid: bool,
    review_event: Option<String>,
    review_body_bytes: Option<usize>,
    review_comment_count: Option<usize>,
    diff_patch: String,
    diff_patch_exists: bool,
    diff_patch_valid: bool,
    diff_line_count: Option<usize>,
    off_diff_comment_count: Option<usize>,
    http_status: Option<u16>,
    token_present: bool,
    payload_written: bool,
    post_stdout_written: bool,
    post_stderr_written: bool,
    response: serde_json::Value,
}

#[derive(Debug, Serialize)]
struct GitHubReviewSkipReceipt {
    schema_version: u32,
    status: String,
    reason: String,
    review_payload_status: String,
    terminal_state: String,
    github_review_json: Option<String>,
    run_pass: String,
    model_mode: String,
    inline_comments: usize,
    summary_only_findings: usize,
    missing_or_failed_sensor_evidence: usize,
    missing_or_failed_model_evidence: usize,
}

#[derive(Clone, Debug, Serialize)]
struct CacheWarmManifest {
    schema_version: u32,
    profile: String,
    profile_hash: String,
    base: String,
    base_tree_sha: String,
    cache_root: String,
    base_cache_dir: String,
    rules_cache_dir: String,
    tools: Vec<ToolCacheReceipt>,
}

#[derive(Clone, Debug, Serialize)]
struct ToolCacheReceipt {
    tool: String,
    command: String,
    status: String,
    version: Option<String>,
    rule_cache_dir: String,
    base_cache_dir: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct BoxState {
    cpus: usize,
    free_mem_mb: Option<u64>,
    free_disk_mb: Option<u64>,
    load_1m: Option<f32>,
    github_actions: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct DiffContext {
    base: String,
    head: String,
    changed_files: Vec<String>,
    patch: String,
    flags: DiffFlags,
    #[serde(default = "default_diff_class")]
    diff_class: DiffClass,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
struct DiffFlags {
    source_changed: bool,
    rust_changed: bool,
    rust_tests_changed: bool,
    workflow_changed: bool,
    dependency_changed: bool,
    shell_changed: bool,
    cpp_changed: bool,
    docs_only: bool,
    unsafe_or_native_risk: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
struct LanguageMix {
    languages: Vec<String>,
    primary_language: Option<String>,
    mixed_language: bool,
    surfaces: Vec<String>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
enum DiffClass {
    #[serde(rename = "source-ub")]
    SourceUb,
    #[serde(rename = "source-general")]
    SourceGeneral,
    #[serde(rename = "tests-only")]
    TestsOnly,
    #[serde(rename = "workflow/tooling")]
    WorkflowTooling,
    #[serde(rename = "docs-only")]
    DocsOnly,
    #[serde(rename = "artifact-only-smoke")]
    ArtifactOnlySmoke,
}

fn default_diff_class() -> DiffClass {
    DiffClass::SourceUb
}

impl DiffClass {
    fn key(self) -> &'static str {
        match self {
            Self::SourceUb => "source-ub",
            Self::SourceGeneral => "source-general",
            Self::TestsOnly => "tests-only",
            Self::WorkflowTooling => "workflow/tooling",
            Self::DocsOnly => "docs-only",
            Self::ArtifactOnlySmoke => "artifact-only-smoke",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Plan {
    base: String,
    head: String,
    profile_name: String,
    #[serde(default = "default_diff_class")]
    diff_class: DiffClass,
    changed_files: Vec<String>,
    #[serde(default)]
    language_mix: LanguageMix,
    sensors: Vec<SensorPlan>,
    lanes: Vec<LanePlan>,
    /// Repo-declared `[[lanes]]` whose diff_classes matched this run,
    /// converted at plan time. Kept separate from `lanes` (the builtin
    /// planning view) so lane-width selection can merge them into the
    /// EXECUTED set at every width and diff class - the first wiring pass
    /// only reached the plan artifact, not execution.
    #[serde(default)]
    repo_lanes: Vec<LanePlan>,
    docs_only: bool,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SensorPlan {
    id: String,
    command: String,
    run: bool,
    reason: String,
    required: bool,
    timeout_sec: u64,
    artifact_budget_mb: u64,
    class: ToolClass,
    weight: u32,
    requires_lease: bool,
    /// Evidence phase under the pipelined scheduler (#325): `fast` sensors
    /// complete before the shared context renders and lanes launch; `late`
    /// sensors overlap the model wave and join before the reporter/compile/
    /// gate. Defaults to `fast` so plan artifacts from older runs still load.
    #[serde(default = "default_plan_sensor_phase")]
    phase: SensorPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    gate: Option<ToolGatePolicy>,
}

fn default_plan_sensor_phase() -> SensorPhase {
    SensorPhase::Fast
}

#[derive(Clone, Debug, Serialize)]
struct ResolvedToolArtifact {
    schema: &'static str,
    runtime_profile: String,
    tools: Vec<ResolvedToolEntry>,
}

#[derive(Clone, Debug, Serialize)]
struct ResolvedToolEntry {
    id: String,
    class: ToolClass,
    command: String,
    required_if: Trigger,
    required: bool,
    required_reason: String,
    runtime_profile: String,
    enabled: bool,
    planned_run: bool,
    plan_reason: String,
    timeout_sec: u64,
    artifact_budget_mb: u64,
    requires_lease: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    gate: Option<ToolGatePolicy>,
    artifact_paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct ToolStatusArtifact {
    schema: &'static str,
    runtime_profile: String,
    tools: Vec<ToolStatusEntry>,
}

#[derive(Clone, Debug, Serialize)]
struct ToolStatusEntry {
    id: String,
    class: ToolClass,
    command: String,
    required_if: Trigger,
    required: bool,
    required_reason: String,
    runtime_profile: String,
    planned_run: bool,
    timeout_sec: u64,
    artifact_budget_mb: u64,
    requires_lease: bool,
    /// Evidence phase under the pipelined scheduler (#325). The artifact
    /// verifier mirrors this to recompute the work-queue initial-packet
    /// status: a late-phase planned sensor is never part of the initial
    /// packet.
    phase: SensorPhase,
    status: String,
    reason: String,
    exit_code: Option<i32>,
    timed_out: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    gate: Option<ToolGatePolicy>,
    artifact_paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct ToolGateOutcomeArtifact {
    schema: &'static str,
    runtime_profile: String,
    outcomes: Vec<ToolGateOutcomeEntry>,
}

#[derive(Clone, Debug, Serialize)]
struct ToolGateOutcomeEntry {
    schema: &'static str,
    tool: String,
    policy: ToolGatePolicy,
    required: bool,
    planned_run: bool,
    sensor_status: String,
    sensor_reason: String,
    sensor_receipt_path: String,
    status_source: &'static str,
    outcome: String,
    evaluated: bool,
    reason: String,
    metrics: ToolGateOutcomeMetrics,
    source_artifacts: Vec<String>,
    packet_policy: &'static str,
    gate_policy: &'static str,
}

#[derive(Clone, Debug, Serialize)]
struct ToolGateOutcomeMetrics {
    new_unsuppressed: Option<u64>,
}

#[derive(Clone, Debug, Serialize)]
struct PrThreadContext {
    schema: String,
    status: String,
    max_bytes: usize,
    sources: Vec<String>,
    warnings: Vec<String>,
    pull_number: Option<u64>,
    title: Option<String>,
    body: Option<String>,
    body_truncated: bool,
    thread_context_path: Option<String>,
    thread_context: Option<String>,
    thread_context_truncated: bool,
    #[serde(default)]
    threads: Vec<ReviewThreadRecord>,
}

#[derive(Clone, Debug, Serialize)]
struct ReviewTerminalState {
    schema: String,
    status: String,
    reason: String,
    review_payload_status: String,
    reviewer_value_present: bool,
    diff_class: String,
    model_mode: String,
    usable_model_lanes: usize,
    model_lanes: usize,
    evidence_gaps: usize,
    proof_receipts: usize,
    final_follow_up_tasks: usize,
    inline_comments: usize,
    summary_only_findings: usize,
    /// Summary-only findings that carry reviewer-relevant weight on their own
    /// (severity medium+ or confidence medium-high+, excluding pure
    /// lane-status notes); `[review_body].summary_only_body` receipts cite
    /// this count.
    substantive_summary_only_findings: usize,
}

#[derive(Clone, Debug, Serialize)]
struct ReviewArtifacts {
    shared_context_id: String,
    review_profile: String,
    mode: String,
    posting: String,
    runtime_profile: String,
    run_pass: String,
    model_mode: String,
    depth: String,
    provider_policy: String,
    model_provider_policy: String,
    lane_width: usize,
    model_concurrency: usize,
    max_model_calls: usize,
    max_inline_comments: usize,
    model_timeout_sec: u64,
    ledger_path: String,
    ledger_max_bytes: usize,
    pr_thread_context: PrThreadContext,
    terminal_state: ReviewTerminalState,
    provider_preflights: Vec<ProviderPreflightReceipt>,
    model_lanes: Vec<ModelLaneReceipt>,
    missing_or_failed_sensor_evidence: Vec<SensorEvidenceIssue>,
    missing_or_failed_model_evidence: Vec<ModelEvidenceIssue>,
    inline_comments: Vec<ReviewInlineComment>,
    summary_only_findings: Vec<SummaryOnlyFinding>,
    observations: Vec<Observation>,
    proof_requests: Vec<ProofRequest>,
    proof_intents: Vec<ProofIntent>,
    proof_receipts: Vec<ProofReceipt>,
    resource_leases: Vec<ResourceLease>,
    body: String,
}

#[derive(Clone, Debug, Serialize)]
struct SharedContextCacheManifest {
    schema: &'static str,
    shared_context_hash: String,
    shared_context_bytes: usize,
    cache_block_path: &'static str,
    hash_path: &'static str,
    events_path: &'static str,
    explicit_cache_provider: &'static str,
    explicit_cache_endpoint: &'static str,
    cache_lifetime: &'static str,
    lanes: Vec<SharedContextCacheLane>,
}

#[derive(Clone, Debug, Serialize)]
struct SharedContextCacheLane {
    lane: String,
    provider: String,
    model: String,
    endpoint_kind: String,
    cache_mode: String,
    shared_context_hash: String,
}

/// One ordered section of the shared PR/repo prefix, with its byte range.
/// Recorded as the prefix is built (not re-parsed) so the manifest's view of
/// what the cohort shares is exact and drift-free. (Order 6 of #678.)
#[derive(Clone, Debug, Deserialize, Serialize)]
struct PrefixSection {
    name: String,
    byte_start: usize,
    byte_end: usize,
}

/// `review/shared-prefix-manifest.json` — the byte-stable shared-prefix
/// contract for a cohort. Records the hash, byte length, base/head identity,
/// the ordered source sections composing the prefix (with byte ranges), the
/// cache policy, and truncations. Makes the cohort's cache-coherence claim
/// (one immutable prefix, byte-stable across lanes) inspectable rather than
/// assumed. (Order 6 of #678.)
#[derive(Clone, Debug, Deserialize, Serialize)]
struct SharedPrefixManifest {
    schema: String,
    hash: String,
    byte_length: usize,
    base: String,
    head: String,
    ordered_source_sections: Vec<PrefixSection>,
    cache_policy: SharedPrefixCachePolicy,
    /// Truncations applied while building the prefix. Empty today; future
    /// slices record any capped section (e.g. an oversized diff patch).
    truncations: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SharedPrefixCachePolicy {
    provider: String,
    endpoint_kind: String,
    mode: String,
    lifetime: String,
}

#[derive(Clone, Debug, Serialize)]
struct SharedContextCacheEvent {
    schema: &'static str,
    kind: String,
    shared_context_hash: String,
    lane: Option<String>,
    provider: Option<String>,
    endpoint_kind: Option<String>,
    cache_mode: String,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[derive(Debug, Deserialize)]
struct ReviewSummaryReceipt {
    #[serde(default)]
    model_mode: String,
    #[serde(default)]
    depth: String,
    #[serde(default)]
    provider_policy: String,
    #[serde(default)]
    lane_width: usize,
    #[serde(default)]
    provider_preflights: Vec<ProviderPreflightReceipt>,
    #[serde(default)]
    model_lanes: Vec<ModelLaneReceipt>,
}

#[derive(Clone, Debug, Serialize)]
struct ReviewMetrics {
    schema_version: u32,
    wall_clock_ms: u128,
    wall_clock_seconds: u64,
    run: RunLoopMetrics,
    shared_context_id: String,
    base: String,
    head: String,
    review_profile: String,
    profile_name: String,
    runtime_profile: String,
    mode: String,
    posting: String,
    run_pass: String,
    model_mode: String,
    depth: String,
    provider_policy: String,
    lane_width: usize,
    model_concurrency: usize,
    max_model_calls: usize,
    max_inline_comments: usize,
    changed_files: usize,
    diff_flags: DiffFlags,
    lane_packets: usize,
    sensors: SensorMetrics,
    models: ModelMetrics,
    inline_comments: usize,
    github_review_comments: usize,
    prepared_inline_comments: usize,
    prepared_review_body: bool,
    summary_only_findings: usize,
    observations: usize,
    follow_up_results: FollowUpResultMetrics,
    final_follow_up_tasks: usize,
    proof_requests: usize,
    proof_request_status_counts: BTreeMap<String, usize>,
    proof_requests_terminal: usize,
    proof_request_terminal_rate: Option<f64>,
    proof_receipts: usize,
    proof_receipts_current_head: usize,
    proof_receipts_stale_head: usize,
    proof_receipts_with_request_links: usize,
    proof_changed_conclusions: usize,
    resource_leases: usize,
    off_diff_candidates_rejected: usize,
    missing_or_failed_sensor_evidence: usize,
    missing_or_failed_model_evidence: usize,
    provider_evidence_failures: usize,
    terminal_state: String,
    review_payload_status: String,
    post_status: String,
    review_body_bytes: usize,
    artifact_review_body_bytes: usize,
    github_review_body_bytes: usize,
    review_body_truncated: bool,
    github_review_body_truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
struct CostReceipt {
    schema: &'static str,
    run_id: String,
    runner_kind: String,
    target_minutes: u64,
    cap_minutes: u64,
    fallback_used: bool,
    required_floor_wall_seconds: Option<f64>,
    llm_seconds: f64,
    cache: CostCacheReceipt,
    tokens: CostTokenReceipt,
    estimated_cost_usd: Option<f64>,
    cost_basis: CostBasisReceipt,
    source_artifacts: Vec<String>,
    missing: Vec<CostMissingInput>,
}

#[derive(Clone, Debug, Serialize)]
struct FloorTrendArtifact {
    schema: &'static str,
    run_id: String,
    as_of: String,
    window_scope: &'static str,
    window_runs: usize,
    source_artifacts: Vec<String>,
    releases: Vec<FloorTrendRelease>,
    trend: FloorTrendSummary,
    missing: Vec<FloorTrendMissingInput>,
}

#[derive(Clone, Debug, Serialize)]
struct FloorTrendRelease {
    version: String,
    sample_runs: usize,
    floor_wall_seconds_p50: Option<f64>,
    floor_wall_seconds_p95: Option<f64>,
    cargo_cache_hit_rate: Option<f64>,
    model_prefix_cache_hit_rate: Option<f64>,
    fallback_used_rate: f64,
    avg_cost_usd: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
struct FloorTrendSummary {
    floor_creep_detected: Option<bool>,
    floor_budget_pressure_detected: Option<bool>,
    cache_hit_rate_delta: Option<f64>,
    avg_cost_delta_usd: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
struct FloorTrendMissingInput {
    field: String,
    reason: String,
    source_artifact: String,
}

#[derive(Clone, Debug, Serialize)]
struct CostCacheReceipt {
    cargo: String,
    model_prefix: String,
}

#[derive(Clone, Debug, Default, Serialize)]
struct CostTokenReceipt {
    fresh_input: u64,
    cached_input: u64,
    output: u64,
}

#[derive(Clone, Debug, Serialize)]
struct CostBasisReceipt {
    runner_minutes: f64,
    linux_minute_rate_usd: Option<f64>,
    token_pricing: String,
}

#[derive(Clone, Debug, Serialize)]
struct CostMissingInput {
    field: String,
    reason: String,
    source_artifact: String,
}

#[derive(Clone, Debug, Serialize)]
struct FillLedger {
    schema: &'static str,
    run_id: String,
    catalog_scope: &'static str,
    source_artifacts: Vec<String>,
    entries: Vec<FillLedgerEntry>,
}

#[derive(Clone, Debug, Serialize)]
struct FillLedgerEntry {
    check_id: String,
    kind: String,
    selected: bool,
    selection_reason: String,
    cost: String,
    expected_signal: Option<String>,
    actual_signal: Option<String>,
    time_spent_sec: f64,
    artifact_path: Option<String>,
    affected_merge: Option<bool>,
    source_artifacts: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct QualityReceipt {
    schema: &'static str,
    run_id: String,
    source_artifacts: Vec<String>,
    review_payload_status: String,
    comments_prepared: usize,
    comments_posted: Option<usize>,
    comments_accepted: Option<usize>,
    comments_resolved: Option<usize>,
    comments_off_diff_rejected: usize,
    fills_with_signal: usize,
    fills_total: usize,
    llm_unavailable_events: usize,
    fallback_used_lanes: usize,
    reviewer_overrides: Option<usize>,
    adopted_generated_tests: Option<usize>,
    missing: Vec<QualityMissingInput>,
}

#[derive(Clone, Debug, Serialize)]
struct QualityTrendArtifact {
    schema: &'static str,
    run_id: String,
    as_of: String,
    window_scope: &'static str,
    window_runs: usize,
    source_artifacts: Vec<String>,
    comments_prepared: usize,
    comments_posted: Option<usize>,
    comment_acceptance_rate: Option<f64>,
    comment_resolution_rate: Option<f64>,
    fills_signal_rate: Option<f64>,
    llm_unavailable_rate: f64,
    reviewer_override_rate: Option<f64>,
    adopted_generated_tests: Option<usize>,
    trend: QualityTrendSummary,
    missing: Vec<QualityTrendMissingInput>,
}

#[derive(Clone, Debug, Serialize)]
struct QualityTrendSummary {
    comment_acceptance_rate_delta: Option<f64>,
    fills_signal_rate_delta: Option<f64>,
    llm_unavailable_rate_delta: Option<f64>,
    reviewer_override_rate_delta: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
struct QualityBackfillArtifact {
    schema: &'static str,
    as_of: String,
    window_scope: &'static str,
    window_days: u32,
    window_runs: usize,
    source_artifacts: Vec<String>,
    comments_prepared: usize,
    comments_posted: Option<usize>,
    comments_accepted: Option<usize>,
    comments_resolved: Option<usize>,
    comment_acceptance_rate: Option<f64>,
    comment_resolution_rate: Option<f64>,
    fills_signal_rate: Option<f64>,
    llm_unavailable_rate: Option<f64>,
    reviewer_overrides: Option<usize>,
    reviewer_override_rate: Option<f64>,
    adopted_generated_tests: Option<usize>,
    trend: QualityBackfillTrendSummary,
    missing: Vec<QualityBackfillMissingInput>,
}

#[derive(Clone, Debug, Serialize)]
struct QualityBackfillTrendSummary {
    comment_acceptance_rate_delta: Option<f64>,
    fills_signal_rate_delta: Option<f64>,
    llm_unavailable_rate_delta: Option<f64>,
    reviewer_override_rate_delta: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
struct QualityTrendMissingInput {
    field: String,
    reason: String,
    source_artifact: String,
}

#[derive(Clone, Debug, Serialize)]
struct QualityBackfillMissingInput {
    field: String,
    reason: String,
    source_artifact: String,
}

#[derive(Clone, Debug, Serialize)]
struct QualityMissingInput {
    field: String,
    reason: String,
    source_artifact: String,
}

#[derive(Clone, Debug, Deserialize)]
struct QualityReceiptSeed {
    schema: String,
    run_id: String,
    comments_prepared: usize,
    fills_with_signal: usize,
    fills_total: usize,
    llm_unavailable_events: usize,
}

#[derive(Clone, Debug, Deserialize)]
struct QualityTrendSeed {
    schema: String,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubQualityOutcomes {
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    source_artifacts: Vec<String>,
    #[serde(default)]
    comments: Vec<GithubQualityCommentOutcome>,
    #[serde(default)]
    adopted_generated_tests: Vec<serde_json::Value>,
}

#[derive(Clone, Debug, Deserialize)]
struct GithubQualityCommentOutcome {
    #[serde(default)]
    posted: Option<bool>,
    #[serde(default)]
    accepted: Option<bool>,
    #[serde(default)]
    resolved: Option<bool>,
    #[serde(default)]
    reviewer_override: Option<bool>,
}

struct LoadedGithubQualityOutcomes {
    outcomes: GithubQualityOutcomes,
    has_comments: bool,
    has_adopted_generated_tests: bool,
    source_artifact: String,
    raw_source_artifacts: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct GithubQualityOutcomesArtifact {
    schema: &'static str,
    collection_status: &'static str,
    source_artifacts: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    collection_warnings: Vec<GithubQualityCollectionWarning>,
    #[serde(skip_serializing_if = "Option::is_none")]
    comments: Option<Vec<GithubQualityNormalizedComment>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    adopted_generated_tests: Option<Vec<GithubQualityGeneratedTestAdoption>>,
}

#[derive(Clone, Debug, Serialize)]
struct GithubQualityCollectionWarning {
    source_artifact: String,
    reason: String,
    detail: String,
}

#[derive(Clone, Debug, Serialize)]
struct GithubQualityNormalizedComment {
    posted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    accepted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    resolved: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    reviewer_override: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source_pull_number: Option<u64>,
    source_thread_id: String,
    source_comment_id: String,
    source_author_login: String,
    source_url: String,
    outcome_source: &'static str,
}

#[derive(Clone, Debug)]
struct GithubQualityResolvedThreadContext {
    source_pull_number: u64,
    source_thread_id: String,
    source_comment_id: String,
    source_author_login: String,
    source_url: String,
}

#[derive(Clone, Debug, Serialize)]
struct GithubQualityGeneratedTestAdoption {
    source_pull_number: u64,
    path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    additions: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deletions: Option<u64>,
    source_thread_id: String,
    source_comment_id: String,
    source_author_login: String,
    source_url: String,
    outcome_source: &'static str,
}

#[derive(Clone, Debug)]
struct GithubQualityChangedFile {
    path: String,
    status: Option<String>,
    additions: Option<u64>,
    deletions: Option<u64>,
}

#[derive(Clone)]
struct QualityBackfillRun {
    receipt: QualityReceiptSeed,
    receipt_source: String,
    trend_source: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct PreviousQualityBackfill {
    schema: String,
    comment_acceptance_rate: Option<f64>,
    fills_signal_rate: Option<f64>,
    llm_unavailable_rate: Option<f64>,
    reviewer_override_rate: Option<f64>,
}

struct LoadedPreviousQualityBackfill {
    artifact: PreviousQualityBackfill,
    source_artifact: String,
}

struct FillLedgerInput<'a> {
    out: &'a Path,
    diff: &'a DiffContext,
    profile: &'a Profile,
    plan: &'a Plan,
    tool_gate_outcomes: &'a [ToolGateOutcomeEntry],
    gate_outcome: &'a GateOutcome,
    review: &'a ReviewArtifacts,
    metrics: &'a ReviewMetrics,
}

#[derive(Clone, Debug, Serialize)]
struct RunLoopMetrics {
    concurrency_model: String,
    scheduler_profile: String,
    local_proof_wall_excludes_model_wait: bool,
    elapsed_wall_ms: u128,
    coordination_wall_ms: u128,
    investigation_wall_ms: u128,
    proof_wall_ms: u128,
    evidence_wall_ms: u128,
    model_wall_ms: u128,
    local_proof_wall_ms: u128,
    compiler_wall_ms: u128,
    model_call_duration_ms_sum: u128,
    proof_command_duration_ms_sum: u128,
    investigation_proof_overlap_ms: u128,
    model_proof_overlap_ms: u128,
    proof_overlap_ms: u128,
    scheduler_roles: SchedulerRoleTimings,
    streams: RunStreamTimings,
    loops: RunLoopTimings,
    phases: Vec<RunLoopPhase>,
}

#[derive(Clone, Debug, Serialize)]
struct SchedulerRoleTimings {
    evidence: LoopTiming,
    model: LoopTiming,
    proof: LoopTiming,
}

#[derive(Clone, Debug, Serialize)]
struct RunStreamTimings {
    coordination: LoopTiming,
    investigation: LoopTiming,
    proof: LoopTiming,
}

#[derive(Clone, Debug, Serialize)]
struct RunLoopTimings {
    evidence: LoopTiming,
    model: LoopTiming,
    proof: LoopTiming,
    compiler: LoopTiming,
}

#[derive(Clone, Debug, Serialize)]
struct LoopTiming {
    started_at_offset_ms: u128,
    finished_at_offset_ms: u128,
    wall_ms: u128,
}

#[derive(Clone, Debug, Serialize)]
struct RunLoopPhase {
    loop_id: String,
    stream_id: String,
    stage: String,
    status: String,
    started_at_offset_ms: u128,
    finished_at_offset_ms: u128,
    duration_ms: u128,
}

impl RunLoopPhase {
    fn interval(&self) -> LoopInterval {
        LoopInterval {
            started_at_offset_ms: self.started_at_offset_ms,
            finished_at_offset_ms: self.finished_at_offset_ms,
        }
    }
}

#[derive(Serialize)]
struct SchedulerArtifact<'a> {
    schema: &'static str,
    concurrency_model: &'a str,
    scheduler_profile: &'a str,
    local_proof_wall_excludes_model_wait: bool,
    elapsed_wall_ms: u128,
    scheduler_roles: &'a SchedulerRoleTimings,
    streams: &'a RunStreamTimings,
    loops: &'a RunLoopTimings,
    overlaps: SchedulerOverlapArtifact,
    phases: &'a [RunLoopPhase],
}

#[derive(Serialize)]
struct SchedulerOverlapArtifact {
    investigation_proof_overlap_ms: u128,
    model_proof_overlap_ms: u128,
    proof_overlap_ms: u128,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Observation {
    schema: String,
    id: String,
    lane: String,
    question: String,
    claim: String,
    kind: String,
    status: String,
    severity: String,
    confidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    fingerprint: String,
    evidence: Vec<String>,
    dedupe_key: String,
    source: String,
}

#[derive(Debug, Serialize)]
struct QuestionObservationArtifact<'a> {
    schema: &'static str,
    lane: &'a str,
    question: &'a str,
    observations: Vec<&'a Observation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WitnessRecord {
    schema: String,
    id: String,
    status: String,
    kind: String,
    source: String,
    claim: String,
    dedupe_key: String,
    evidence: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lane: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    observation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    proof_receipt_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct WitnessRegistryArtifact {
    schema: String,
    total: usize,
    status_counts: BTreeMap<String, usize>,
    kind_counts: BTreeMap<String, usize>,
    source_counts: BTreeMap<String, usize>,
    follow_up_total: usize,
    follow_up_status_counts: BTreeMap<String, usize>,
    witness_ids_by_status: BTreeMap<String, Vec<String>>,
    follow_up_witness_ids_by_status: BTreeMap<String, Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
struct WorkQueueArtifact<'a> {
    schema: &'static str,
    initial_packet_deadline_sec: u64,
    follow_up_deadline_sec: u64,
    tasks: &'a [WorkQueueTaskArtifact],
}

#[derive(Clone, Debug, Serialize)]
struct WorkQueueTaskArtifact {
    schema: &'static str,
    id: String,
    kind: String,
    source: String,
    priority: String,
    packet_policy: String,
    deadline_sec: u64,
    consumers: Vec<String>,
    gate_policy: String,
    dedupe_key: String,
    lease: ProofTaskLease,
    receipt_path: String,
    status: String,
    initial_packet_status: String,
    task_path: String,
}

#[derive(Clone, Debug, Serialize)]
struct WorkEventArtifact {
    schema: &'static str,
    kind: &'static str,
    task_id: String,
    task_kind: String,
    source: String,
    packet_policy: String,
    deadline_sec: u64,
    consumers: Vec<String>,
    gate_policy: String,
    status: String,
    initial_packet_status: String,
    receipt_path: String,
}

#[derive(Clone, Debug, Serialize)]
struct ReceiptRoutesArtifact<'a> {
    schema: &'static str,
    source_artifacts: Vec<&'static str>,
    routes: &'a [ReceiptRouteArtifact],
}

#[derive(Clone, Debug, Serialize)]
struct ReceiptRouteArtifact {
    schema: &'static str,
    id: String,
    receipt_id: String,
    phase: String,
    receipt_kind: String,
    result: String,
    status: String,
    requested_by: Vec<String>,
    request_ids: Vec<String>,
    consumers: Vec<String>,
    lease_ids: Vec<String>,
    source_artifacts: Vec<String>,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct ProofPlannerSkip {
    kind: String,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct SensorMetrics {
    total: usize,
    planned: usize,
    skipped_by_plan: usize,
    status_counts: BTreeMap<String, usize>,
}

#[derive(Clone, Debug, Serialize)]
struct ModelMetrics {
    provider_preflights: usize,
    provider_preflight_status_counts: BTreeMap<String, usize>,
    provider_preflight_calls_attempted: usize,
    model_lanes: usize,
    model_lane_status_counts: BTreeMap<String, usize>,
    model_lane_calls_attempted: usize,
    model_fallbacks_used: usize,
    prompt_cache_creation_input_tokens: u64,
    prompt_cache_read_input_tokens: u64,
    prompt_cache_lane_hits: usize,
    prompt_cache_lane_misses: usize,
    prompt_cache_lane_unknown: usize,
}

#[derive(Clone, Debug, Serialize)]
struct FollowUpResultMetrics {
    total: usize,
    status_counts: BTreeMap<String, usize>,
    calls_attempted: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ModelLaneReceipt {
    lane: String,
    provider: String,
    model: String,
    endpoint_kind: String,
    status: String,
    reason: String,
    duration_ms: Option<u128>,
    http_status: Option<u16>,
    response_shape: Option<String>,
    fallback_from: Option<String>,
    #[serde(default)]
    cache_usage: ModelCacheUsage,
    /// Run-level cohort identity: one value per strict-cohort run, derived
    /// from provider + model + a prefix of the shared_prefix_hash. Every
    /// same-model lane in a strict cohort carries the same cohort_id; a
    /// mixed-provider run records the cohort break on `cohort_broken` rather
    /// than silently presenting as cache-coherent. (Order 5 of #678.)
    #[serde(default)]
    cohort_id: String,
    /// sha256 of the immutable shared PR/repo context prefix the cohort
    /// shares. Proves cache coherence: every lane in a strict cohort reads
    /// the same prefix and can reuse the provider's prompt cache. Sourced
    /// from the run's `shared_context_id`. (Order 5 of #678.)
    #[serde(default)]
    shared_prefix_hash: String,
    /// Logical lane thread identity (`<cohort_id>:<lane>`). Stateless today
    /// (one turn per lane); becomes a persistent multi-turn thread id in
    /// Order 7. (Order 5 of #678.)
    #[serde(default)]
    thread_id: String,
    /// Execution turn within the lane's logical thread. 0 for the first
    /// investigation wave; follow-up and reporter turns increment it once
    /// Order 7 lands persistent sessions. (Order 5 of #678.)
    #[serde(default)]
    turn: u32,
    /// True only when this lane left the cohort — i.e. its provider/model
    /// differs from the cohort's primary (a cross-provider fallback under
    /// `PrimaryWithFallback`, or mixed routing under `Auto`/`OpencodeGoWide`).
    /// Default false. This makes a cache-coherence break observable on the
    /// receipt rather than silent. (Order 5 of #678.)
    #[serde(default)]
    cohort_broken: bool,
}

/// Deterministic run-level cohort identity. One value per strict-cohort run,
/// derived from provider + model + a short prefix of the shared-prefix hash.
/// Every same-model lane in a strict cohort carries the same cohort_id; a
/// receipt whose provider/model differ from the cohort's primary records the
/// break on `cohort_broken` rather than presenting as cache-coherent.
/// (Order 5 of #678.)
fn cohort_id_for(provider: &str, model: &str, shared_prefix_hash: &str) -> String {
    let prefix = &shared_prefix_hash.chars().take(12).collect::<String>();
    format!("cohort:{provider}:{model}:{prefix}")
}

/// Build the cohort stamp (cohort_id, shared_prefix_hash, thread_id, turn,
/// cohort_broken) that every lane receipt carries. Centralized so all
/// construction sites stamp identically. `fallback_spec` is the provider+model
/// the lane actually used if it fell back (None = primary); when the fallback
/// is a different provider, that is a cohort break.
fn cohort_stamp(
    cohort_provider: &str,
    cohort_model: &str,
    shared_prefix_hash: &str,
    lane: &str,
    turn: u32,
    fallback_spec: Option<(&str, &str)>,
) -> (String, String, String, u32, bool) {
    let cohort_id = cohort_id_for(cohort_provider, cohort_model, shared_prefix_hash);
    let cohort_broken = match fallback_spec {
        Some((fp, fm)) => fp != cohort_provider || fm != cohort_model,
        None => false,
    };
    let thread_id = format!("{cohort_id}:{lane}");
    (
        cohort_id,
        shared_prefix_hash.to_owned(),
        thread_id,
        turn,
        cohort_broken,
    )
}

#[cfg(test)]
mod cohort_contract_tests {
    use super::*;

    #[test]
    fn cohort_id_is_deterministic_and_stable() {
        let a = cohort_id_for("minimax", "MiniMax-M3", "abcdef0123456789");
        let b = cohort_id_for("minimax", "MiniMax-M3", "abcdef0123456789");
        assert_eq!(a, b, "same inputs => same cohort id");
        // Only the first 12 chars of the prefix hash contribute, matching the
        // focused-proof id convention.
        assert!(a.starts_with("cohort:minimax:MiniMax-M3:abcdef012345"));
        // Different provider or model => different cohort.
        assert_ne!(
            a,
            cohort_id_for("opencode-go", "MiniMax-M3", "abcdef0123456789")
        );
        assert_ne!(
            a,
            cohort_id_for("minimax", "OtherModel", "abcdef0123456789")
        );
    }

    /// A strict cohort (same provider+model+prefix, no fallback) stamps
    /// cohort_broken = false and identical cohort_id/thread_id per lane.
    #[test]
    fn strict_cohort_stamp_is_unbroken_and_consistent() {
        let prefix = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        let (id_a, hash_a, thread_a, turn_a, broken_a) =
            cohort_stamp("minimax", "MiniMax-M3", prefix, "tests-oracle", 0, None);
        let (id_b, _hash_b, thread_b, turn_b, broken_b) =
            cohort_stamp("minimax", "MiniMax-M3", prefix, "opposition", 0, None);
        assert_eq!(id_a, id_b, "strict cohort: same cohort_id across lanes");
        assert_eq!(hash_a, prefix);
        assert!(!broken_a && !broken_b, "strict cohort: never broken");
        assert_ne!(thread_a, thread_b, "thread_id is lane-distinct");
        assert!(thread_a.ends_with(":tests-oracle"));
        assert_eq!(turn_a, 0);
        assert_eq!(turn_b, 0);
    }

    /// A cross-provider fallback (the lane ran on a different provider than the
    /// cohort primary) is a cohort break: cohort_broken = true. This is the
    /// observable provenance for a cache-coherence break under
    /// PrimaryWithFallback / Auto / OpencodeGoWide.
    #[test]
    fn cross_provider_fallback_is_a_cohort_break() {
        let (.., broken) = cohort_stamp(
            "minimax", // cohort primary
            "MiniMax-M3",
            "deadbeef",
            "tests-oracle",
            0,
            Some(("opencode-go", "mimo-v2.5")), // lane actually ran elsewhere
        );
        assert!(broken, "a cross-provider fallback must mark cohort_broken");
    }

    /// A same-provider fallback (same provider+model as primary) is NOT a
    /// cohort break even though fallback_from is set.
    #[test]
    fn same_provider_fallback_is_not_a_cohort_break() {
        let (.., broken) = cohort_stamp(
            "minimax",
            "MiniMax-M3",
            "deadbeef",
            "tests-oracle",
            0,
            Some(("minimax", "MiniMax-M3")), // same provider+model
        );
        assert!(
            !broken,
            "a same-provider/model fallback is not a cohort break"
        );
    }

    /// New receipt fields round-trip through JSON; old receipts (without the
    /// fields) deserialize via #[serde(default)] (backward compat).
    #[test]
    fn model_lane_receipt_round_trips_and_back_compat() -> Result<()> {
        let receipt = ModelLaneReceipt {
            lane: "tests-oracle".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "completed".to_owned(),
            duration_ms: Some(1234),
            http_status: Some(200),
            response_shape: None,
            fallback_from: None,
            cache_usage: ModelCacheUsage::default(),
            cohort_id: "cohort:minimax:MiniMax-M3:abcdef012345".to_owned(),
            shared_prefix_hash: "abcdef0123456789".to_owned(),
            thread_id: "cohort:minimax:MiniMax-M3:abcdef012345:tests-oracle".to_owned(),
            turn: 0,
            cohort_broken: false,
        };
        let json = serde_json::to_string(&receipt)?;
        assert!(json.contains("\"cohort_id\":\"cohort:minimax:MiniMax-M3:abcdef012345\""));
        assert!(json.contains("\"shared_prefix_hash\":\"abcdef0123456789\""));
        assert!(json.contains("\"cohort_broken\":false"));
        let back: ModelLaneReceipt = serde_json::from_str(&json)?;
        assert_eq!(back.cohort_id, receipt.cohort_id);
        assert_eq!(back.thread_id, receipt.thread_id);

        // Backward compat: an old receipt JSON (no cohort fields) parses with
        // defaults — existing artifacts on disk stay readable.
        let old_json = r#"{
            "lane":"x","provider":"minimax","model":"M3","endpoint_kind":"openai-chat",
            "status":"ok","reason":"done","cache_usage":{}
        }"#;
        let legacy: ModelLaneReceipt = serde_json::from_str(old_json)?;
        assert_eq!(legacy.cohort_id, "");
        assert_eq!(legacy.shared_prefix_hash, "");
        assert!(!legacy.cohort_broken);
        assert_eq!(legacy.turn, 0);
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProviderPreflightReceipt {
    provider: String,
    model: String,
    endpoint_kind: String,
    status: String,
    reason: String,
    duration_ms: Option<u128>,
    http_status: Option<u16>,
    response_shape: Option<String>,
    #[serde(default)]
    cache_usage: ModelCacheUsage,
}

#[derive(Clone, Debug, Serialize)]
struct ModelEvidenceIssue {
    lane: String,
    provider: String,
    model: String,
    endpoint_kind: String,
    status: String,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct SensorEvidenceIssue {
    sensor: String,
    status: String,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReviewInlineComment {
    lane: String,
    severity: String,
    confidence: String,
    path: String,
    line: u32,
    side: String,
    body: String,
    evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestion: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct SummaryOnlyFinding {
    lane: String,
    severity: String,
    confidence: String,
    reason: String,
    evidence: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct CandidateRecord {
    schema: String,
    id: String,
    lane: String,
    source: String,
    status: String,
    disposition: String,
    severity: String,
    confidence: String,
    claim: String,
    evidence: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    side: Option<String>,
}

/// A follow-up too broad for the current PR, preserved as structured work
/// instead of blocking the PR or vanishing (release lane step 4). Lanes may
/// emit these; they never open issues - in v0 every valid candidate is
/// artifact-only and the only side-effect surface (the issue broker) does
/// not exist yet.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct IssueCandidate {
    schema: String,
    id: String,
    source: String,
    target_repo: String,
    kind: String,
    confidence: String,
    current_pr_disposition: String,
    title: String,
    problem: String,
    why_not_this_pr: String,
    evidence: Vec<IssueCandidateEvidence>,
    implementation_plan: Vec<String>,
    acceptance: Vec<String>,
    labels: Vec<String>,
}

impl Default for IssueCandidate {
    fn default() -> Self {
        Self {
            schema: ISSUE_CANDIDATE_SCHEMA.to_owned(),
            id: String::new(),
            source: String::new(),
            target_repo: String::new(),
            kind: String::new(),
            confidence: String::new(),
            current_pr_disposition: "do-not-block".to_owned(),
            title: String::new(),
            problem: String::new(),
            why_not_this_pr: String::new(),
            evidence: Vec::new(),
            implementation_plan: Vec::new(),
            acceptance: Vec::new(),
            labels: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct IssueCandidateEvidence {
    #[serde(rename = "type")]
    kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

/// What ub-review did with each issue candidate during `run`. The run-side
/// vocabulary is exactly artifact-only, duplicate, invalid, and suggested -
/// fail-closed forever. Broker outcomes (opened/failed_to_open) never appear
/// here: opening issues is a `post`-time side effect recorded in the
/// separate broker plan and results artifacts.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct IssueAction {
    schema: String,
    candidate_id: String,
    action: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    existing: Option<String>,
}

/// One issue-broker decision made at `run` time (ub-review.issue_broker_plan.v1).
/// The plan is pure data: `run` decides (allowlist, cap, slug validity) and
/// renders the full issue title/body; `post` only executes attempts. The
/// rendered body carries the `ub-review-fingerprint: <sha256>` marker the
/// broker's remote duplicate search keys on.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct IssueBrokerPlanEntry {
    schema: String,
    candidate_id: String,
    fingerprint: String,
    target_repo: String,
    /// `attempt` or `skip`.
    decision: String,
    reason: String,
    title: String,
    body: String,
    labels: Vec<String>,
}

/// One issue-broker outcome recorded at `post` time
/// (ub-review.issue_broker_result.v1). Vocabulary: `opened` (url required),
/// `duplicate` (existing issue url required), `failed_to_open` (error
/// required), `skipped` (mirrors a plan skip). Broker outcomes never affect
/// the gate or the post exit code.
#[derive(Clone, Debug, Deserialize, Serialize)]
struct IssueBrokerResult {
    schema: String,
    candidate_id: String,
    target_repo: String,
    action: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ResolvedCandidateRecord {
    schema: String,
    candidate_id: String,
    lane: String,
    source: String,
    original_status: String,
    original_disposition: String,
    resolved_status: String,
    resolved_disposition: String,
    resolution_source: String,
    source_artifacts: Vec<String>,
    reason: String,
    follow_up_task_ids: Vec<String>,
    follow_up_stages: Vec<String>,
    follow_up_statuses: Vec<String>,
    evidence: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OrchestratorPlanArtifact {
    schema: String,
    candidates: usize,
    observations: usize,
    evidence_groups: Vec<OrchestratorEvidenceGroup>,
    observation_groups: Vec<OrchestratorObservationGroup>,
    follow_up_tasks: Vec<FollowUpQuestionTask>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OrchestratorEvidenceGroup {
    schema: String,
    id: String,
    evidence_need: String,
    disposition: String,
    candidate_ids: Vec<String>,
    lanes: Vec<String>,
    routed_evidence: Vec<OrchestratorRoutedEvidence>,
    duplicate_count: usize,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OrchestratorRoutedEvidence {
    schema: String,
    id: String,
    kind: String,
    artifact: String,
    status: String,
    result: String,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct OrchestratorObservationGroup {
    schema: String,
    id: String,
    observation_group_id: String,
    dedupe_key: String,
    evidence_need: String,
    claim: String,
    kind: String,
    status: String,
    lanes: Vec<String>,
    sources: Vec<String>,
    observation_ids: Vec<String>,
    duplicate_count: usize,
    routed_evidence: Vec<OrchestratorRoutedEvidence>,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct FollowUpQuestionTask {
    schema: String,
    id: String,
    group_id: String,
    stage: String,
    stage_reason: String,
    evidence_need: String,
    disposition: String,
    candidate_ids: Vec<String>,
    observation_group_ids: Vec<String>,
    routed_evidence: Vec<OrchestratorRoutedEvidence>,
    question: String,
    status: String,
    reason: String,
}

#[derive(Debug, Serialize)]
struct FollowUpQuestionPacket<'a> {
    schema: &'static str,
    id: &'a str,
    task_id: &'a str,
    group_id: &'a str,
    stage: &'a str,
    stage_reason: &'a str,
    evidence_need: &'a str,
    disposition: &'a str,
    candidate_ids: &'a [String],
    observation_group_ids: &'a [String],
    routed_evidence: &'a [OrchestratorRoutedEvidence],
    question: &'a str,
    status: &'a str,
    source_artifact: &'static str,
    prompt: String,
}

#[derive(Debug, Deserialize)]
struct FollowUpQuestionPacketArtifact {
    schema: String,
    id: String,
    task_id: String,
    group_id: String,
    stage: String,
    stage_reason: String,
    prompt: String,
}

#[derive(Debug, Serialize)]
struct FollowUpResult {
    schema: String,
    task_id: String,
    group_id: String,
    stage: String,
    disposition: String,
    evidence_need: String,
    candidate_ids: Vec<String>,
    observation_group_ids: Vec<String>,
    packet_path: String,
    model_lane: String,
    status: String,
    reason: String,
    provider: String,
    model: String,
    endpoint_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    fallback_from: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_shape: Option<String>,
    #[serde(default)]
    cache_usage: ModelCacheUsage,
    #[serde(skip_serializing_if = "Option::is_none")]
    request_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    content_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    normalized_content_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    stderr_path: Option<String>,
    output_counts: FollowUpOutputCounts,
}

#[derive(Clone, Debug, Default, Serialize)]
struct FollowUpOutputCounts {
    observations: usize,
    candidate_findings: usize,
    summary_only_findings: usize,
    failed_objections: usize,
    proof_requests: usize,
}

#[derive(Debug, Serialize)]
struct FollowUpOutputRecord {
    schema: String,
    task_id: String,
    group_id: String,
    stage: String,
    disposition: String,
    evidence_need: String,
    candidate_ids: Vec<String>,
    observation_group_ids: Vec<String>,
    model_lane: String,
    status: String,
    reason: String,
    inline_comments: Vec<ReviewInlineComment>,
    summary_only_findings: Vec<SummaryOnlyFinding>,
    observations: Vec<Observation>,
    proof_requests: Vec<ProofRequest>,
}

#[derive(Debug, Serialize)]
struct ModelStageRecord {
    schema: String,
    lane: String,
    source: String,
    stage: String,
    stage_reason: String,
    status: String,
    reason: String,
    provider: String,
    model: String,
    endpoint_kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    task_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    packet_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_shape: Option<String>,
    #[serde(default)]
    cache_usage: ModelCacheUsage,
}

#[derive(Debug, Serialize)]
struct FollowUpEvidenceArtifact {
    schema: String,
    follow_up_outputs: usize,
    inline_comments: Vec<ReviewInlineComment>,
    summary_only_findings: Vec<SummaryOnlyFinding>,
    observations: Vec<Observation>,
    proof_requests: Vec<ProofRequest>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GitHubReview {
    event: String,
    body: String,
    comments: Vec<GitHubReviewComment>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct GitHubReviewComment {
    path: String,
    line: u32,
    side: String,
    body: String,
    /// GitHub suggestion block content (the text between the fenced
    /// ` ```suggestion ` markers). Present only when unsafe-review's
    /// repair-queue supplies a concrete applicable edit for this site.
    /// Serialised as a JSON string when set; omitted entirely when absent
    /// (advisory comment without a one-click edit).
    #[serde(skip_serializing_if = "Option::is_none")]
    suggestion: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
struct GitHubReviewPostPayload {
    event: String,
    body: String,
    comments: Vec<GitHubReviewPostComment>,
}

#[derive(Clone, Debug, Serialize)]
struct GitHubReviewPostComment {
    path: String,
    line: u32,
    side: String,
    body: String,
}

#[derive(Debug)]
struct LaneModelOutput {
    summary: Option<String>,
    inline_comments: Vec<ModelCandidateComment>,
    candidate_findings: Vec<ModelCandidateComment>,
    summary_only_findings: Vec<ModelCandidateFinding>,
    observations: Vec<ModelCandidateObservation>,
    failed_objections: Vec<ModelFailedObjection>,
    proof_requests: Vec<ModelProofRequest>,
    proof_intents: Vec<ModelProofIntent>,
    issue_candidates: Vec<IssueCandidate>,
    degraded: bool,
}

#[derive(Debug, Deserialize)]
struct LaneModelOutputWire {
    summary: Option<String>,
    #[serde(default)]
    inline_comments: Vec<ModelCandidateComment>,
    #[serde(default)]
    candidate_findings: Vec<ModelCandidateComment>,
    #[serde(default)]
    summary_only_findings: Vec<ModelCandidateFinding>,
    #[serde(default)]
    observations: Vec<ModelCandidateObservation>,
    #[serde(default)]
    failed_objections: Vec<ModelFailedObjection>,
    #[serde(default)]
    proof_requests: Vec<ModelProofRequest>,
    #[serde(default)]
    proof_intents: Vec<ModelProofIntent>,
    #[serde(default)]
    issue_candidates: Vec<IssueCandidate>,
}

impl<'de> Deserialize<'de> for LaneModelOutput {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let mut value = serde_json::Value::deserialize(deserializer)?;
        let normalization = normalize_lane_model_output_value(&mut value);
        let wire: LaneModelOutputWire =
            serde_json::from_value(value).map_err(serde::de::Error::custom)?;
        let mut observations = wire.observations;
        observations.extend(normalization.degraded_observations);
        Ok(Self {
            summary: wire.summary,
            inline_comments: wire.inline_comments,
            candidate_findings: wire.candidate_findings,
            summary_only_findings: wire.summary_only_findings,
            observations,
            failed_objections: wire.failed_objections,
            proof_requests: wire.proof_requests,
            proof_intents: wire.proof_intents,
            issue_candidates: wire.issue_candidates,
            degraded: normalization.degraded,
        })
    }
}

#[derive(Debug, Deserialize)]
struct ModelCandidateComment {
    severity: String,
    confidence: String,
    path: String,
    line: u32,
    body: String,
    evidence: String,
    #[serde(default)]
    suggestion: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelCandidateFinding {
    severity: String,
    confidence: String,
    reason: String,
    evidence: String,
}

#[derive(Debug, Deserialize)]
struct ModelCandidateObservation {
    claim: String,
    #[serde(default)]
    question: Option<String>,
    #[serde(default)]
    kind: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default)]
    confidence: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    dedupe_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelFailedObjection {
    claim: String,
    reason: String,
    #[serde(default)]
    evidence: Vec<String>,
    #[serde(default)]
    confidence: Option<String>,
    #[serde(default)]
    kind: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ModelProofRequest {
    command: String,
    reason: String,
    #[serde(default)]
    cost: Option<String>,
    #[serde(default)]
    timeout_sec: Option<u64>,
    #[serde(default)]
    required: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ModelProofIntent {
    claim_id: String,
    question: String,
    expected_answer_shape: String,
    proof_kind: ProofKind,
    target: String,
    #[serde(default)]
    estimated_value: Option<String>,
}

const LANE_MODEL_ARRAY_FIELDS: &[&str] = &[
    "inline_comments",
    "candidate_findings",
    "summary_only_findings",
    "observations",
    "failed_objections",
    "proof_requests",
    "proof_intents",
];

struct LaneModelNormalization {
    degraded_observations: Vec<ModelCandidateObservation>,
    degraded: bool,
}

fn normalize_lane_model_output_value(value: &mut serde_json::Value) -> LaneModelNormalization {
    let Some(object) = value.as_object_mut() else {
        return LaneModelNormalization {
            degraded_observations: Vec::new(),
            degraded: false,
        };
    };
    let mut normalization = LaneModelNormalization {
        degraded_observations: Vec::new(),
        degraded: false,
    };
    for field in LANE_MODEL_ARRAY_FIELDS {
        if let Some(field_value) = object.get_mut(*field) {
            normalize_lane_model_array_field(field, field_value, &mut normalization);
        }
    }
    normalization
}

fn normalize_lane_model_array_field(
    field: &str,
    value: &mut serde_json::Value,
    normalization: &mut LaneModelNormalization,
) {
    match value {
        serde_json::Value::Array(items) => {
            for item in items {
                normalization.degraded |= normalize_lane_model_array_item(field, item);
            }
        }
        serde_json::Value::Object(_) => {
            let mut item = std::mem::replace(value, serde_json::Value::Null);
            normalize_lane_model_array_item(field, &mut item);
            *value = serde_json::Value::Array(vec![item]);
            normalization.degraded = true;
        }
        serde_json::Value::String(raw) => {
            if let Some(observation) = lane_output_scalar_field_observation(field, raw) {
                normalization.degraded_observations.push(observation);
                normalization.degraded = true;
            }
            *value = serde_json::Value::Array(Vec::new());
        }
        serde_json::Value::Null => {
            *value = serde_json::Value::Array(Vec::new());
        }
        other => {
            let raw = other.to_string();
            if let Some(observation) = lane_output_scalar_field_observation(field, &raw) {
                normalization.degraded_observations.push(observation);
                normalization.degraded = true;
            }
            *other = serde_json::Value::Array(Vec::new());
        }
    }
}

fn normalize_lane_model_array_item(field: &str, value: &mut serde_json::Value) -> bool {
    if !matches!(field, "observations" | "failed_objections") {
        return false;
    }
    let Some(object) = value.as_object_mut() else {
        return false;
    };
    if let Some(evidence) = object.get_mut("evidence") {
        return normalize_string_array_field(evidence);
    }
    false
}

fn normalize_string_array_field(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::String(raw) => {
            let raw = raw.trim().to_owned();
            let degraded = !raw.is_empty();
            *value = if raw.is_empty() {
                serde_json::Value::Array(Vec::new())
            } else {
                serde_json::Value::Array(vec![serde_json::Value::String(raw.to_owned())])
            };
            degraded
        }
        serde_json::Value::Null => {
            *value = serde_json::Value::Array(Vec::new());
            false
        }
        _ => false,
    }
}

fn lane_output_scalar_field_observation(
    field: &str,
    raw: &str,
) -> Option<ModelCandidateObservation> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let raw_claim = truncate_chars(raw, 180);
    let claim = truncate_chars(
        &format!(
            "Lane output field `{field}` used scalar text where an array was expected: {raw_claim}"
        ),
        300,
    );
    Some(ModelCandidateObservation {
        claim,
        question: Some("lane-output-shape".to_owned()),
        kind: Some("missing-evidence".to_owned()),
        status: Some("open".to_owned()),
        severity: Some("low".to_owned()),
        confidence: Some("high".to_owned()),
        path: None,
        line: None,
        evidence: vec![format!(
            "Schema expected `{field}` as an array; raw scalar: {}",
            truncate_chars(raw, 220)
        )],
        dedupe_key: Some(format!("lane-output-shape-{field}")),
    })
}

#[derive(Debug, Deserialize)]
struct RefuterOutput {
    #[serde(default)]
    decisions: Vec<RefuterDecision>,
}

#[derive(Debug, Deserialize)]
struct RefuterDecision {
    path: String,
    line: u32,
    disposition: String,
    confidence: Option<String>,
    reason: String,
}

#[derive(Serialize)]
struct Event<'a, T> {
    ts: DateTime<Utc>,
    kind: &'a str,
    payload: T,
}

struct EventLog {
    file: Mutex<File>,
}

struct RunLoopTracker {
    evidence: LoopAccumulator,
    model: LoopAccumulator,
    proof: LoopAccumulator,
    compiler: LoopAccumulator,
    phases: Vec<RunLoopPhase>,
}

impl RunLoopTracker {
    fn new() -> Self {
        Self {
            evidence: LoopAccumulator::default(),
            model: LoopAccumulator::default(),
            proof: LoopAccumulator::default(),
            compiler: LoopAccumulator::default(),
            phases: Vec::new(),
        }
    }

    fn record(&mut self, phase: RunLoopPhase) {
        let loop_id = phase.loop_id.clone();
        self.record_interval(&loop_id, phase.interval());
        self.phases.push(phase);
    }

    fn record_interval(&mut self, loop_id: &str, interval: LoopInterval) {
        match loop_id {
            "evidence" => self.evidence.record(interval),
            "model" => self.model.record(interval),
            "proof" => self.proof.record(interval),
            "compiler" => self.compiler.record(interval),
            _ => {}
        }
    }

    fn metrics(&self) -> RunLoopMetrics {
        let coordination_timing = combined_timing(&[&self.evidence, &self.compiler]);
        let investigation_timing = self.model.timing();
        let proof_timing = self.proof.timing();
        let investigation_proof_overlap_ms =
            overlap_ms(&self.model.intervals, &self.proof.intervals);
        RunLoopMetrics {
            concurrency_model: "profiled-stream-scheduler-v0".to_owned(),
            scheduler_profile: "default-three-stream-v0".to_owned(),
            local_proof_wall_excludes_model_wait: true,
            elapsed_wall_ms: self.elapsed_wall_ms(),
            coordination_wall_ms: coordination_timing.wall_ms,
            investigation_wall_ms: investigation_timing.wall_ms,
            proof_wall_ms: proof_timing.wall_ms,
            evidence_wall_ms: self.evidence.wall_ms,
            model_wall_ms: self.model.wall_ms,
            local_proof_wall_ms: self.proof.wall_ms,
            compiler_wall_ms: self.compiler.wall_ms,
            model_call_duration_ms_sum: 0,
            proof_command_duration_ms_sum: 0,
            investigation_proof_overlap_ms,
            model_proof_overlap_ms: investigation_proof_overlap_ms,
            proof_overlap_ms: investigation_proof_overlap_ms,
            scheduler_roles: SchedulerRoleTimings {
                evidence: self.evidence.timing(),
                model: investigation_timing.clone(),
                proof: proof_timing.clone(),
            },
            streams: RunStreamTimings {
                coordination: coordination_timing,
                investigation: investigation_timing,
                proof: proof_timing,
            },
            loops: RunLoopTimings {
                evidence: self.evidence.timing(),
                model: self.model.timing(),
                proof: self.proof.timing(),
                compiler: self.compiler.timing(),
            },
            phases: self.phases.clone(),
        }
    }

    fn elapsed_wall_ms(&self) -> u128 {
        let mut started_at_offset_ms = None::<u128>;
        let mut finished_at_offset_ms = None::<u128>;
        for accumulator in [&self.evidence, &self.model, &self.proof, &self.compiler] {
            if let Some(started) = accumulator.started_at_offset_ms {
                started_at_offset_ms =
                    Some(started_at_offset_ms.map_or(started, |existing| existing.min(started)));
            }
            if let Some(finished) = accumulator.finished_at_offset_ms {
                finished_at_offset_ms =
                    Some(finished_at_offset_ms.map_or(finished, |existing| existing.max(finished)));
            }
        }
        finished_at_offset_ms
            .unwrap_or(0)
            .saturating_sub(started_at_offset_ms.unwrap_or(0))
    }
}

fn combined_timing(accumulators: &[&LoopAccumulator]) -> LoopTiming {
    let mut started_at_offset_ms = None::<u128>;
    let mut finished_at_offset_ms = None::<u128>;
    let mut intervals = Vec::new();
    for accumulator in accumulators {
        if let Some(started) = accumulator.started_at_offset_ms {
            started_at_offset_ms =
                Some(started_at_offset_ms.map_or(started, |existing| existing.min(started)));
        }
        if let Some(finished) = accumulator.finished_at_offset_ms {
            finished_at_offset_ms =
                Some(finished_at_offset_ms.map_or(finished, |existing| existing.max(finished)));
        }
        intervals.extend(accumulator.intervals.iter().copied());
    }
    LoopTiming {
        started_at_offset_ms: started_at_offset_ms.unwrap_or(0),
        finished_at_offset_ms: finished_at_offset_ms.unwrap_or(0),
        // Union across the combined loops for the same reason as
        // LoopAccumulator::record: overlapping phases must not double-count
        // wall time (the verifier enforces wall <= observed span).
        wall_ms: union_wall_ms(&intervals),
    }
}

#[derive(Default)]
struct LoopAccumulator {
    started_at_offset_ms: Option<u128>,
    finished_at_offset_ms: Option<u128>,
    wall_ms: u128,
    intervals: Vec<LoopInterval>,
}

impl LoopAccumulator {
    fn record(&mut self, interval: LoopInterval) {
        self.started_at_offset_ms = Some(
            self.started_at_offset_ms
                .map_or(interval.started_at_offset_ms, |existing| {
                    existing.min(interval.started_at_offset_ms)
                }),
        );
        self.finished_at_offset_ms = Some(
            self.finished_at_offset_ms
                .map_or(interval.finished_at_offset_ms, |existing| {
                    existing.max(interval.finished_at_offset_ms)
                }),
        );
        self.intervals.push(interval);
        // Wall time is the union of the recorded intervals, not their sum:
        // under the pipelined scheduler (#325) phases within one loop can
        // overlap (the late evidence phase runs behind lane launch while the
        // sensors-and-packet phase is still open), and summing durations
        // would double-count that time — the artifact verifier enforces
        // wall <= observed span per stream.
        self.wall_ms = union_wall_ms(&self.intervals);
    }

    fn timing(&self) -> LoopTiming {
        LoopTiming {
            started_at_offset_ms: self.started_at_offset_ms.unwrap_or(0),
            finished_at_offset_ms: self.finished_at_offset_ms.unwrap_or(0),
            wall_ms: self.wall_ms,
        }
    }
}

/// Busy wall time of a set of possibly-overlapping intervals: the summed
/// length of the union of their `[start, finish)` offset spans.
fn union_wall_ms(intervals: &[LoopInterval]) -> u128 {
    let mut spans: Vec<(u128, u128)> = intervals
        .iter()
        .map(|interval| {
            (
                interval.started_at_offset_ms,
                interval
                    .finished_at_offset_ms
                    .max(interval.started_at_offset_ms),
            )
        })
        .collect();
    spans.sort_unstable();
    let mut total = 0_u128;
    let mut current: Option<(u128, u128)> = None;
    for (start, finish) in spans {
        current = match current {
            Some((open_start, open_finish)) if start <= open_finish => {
                Some((open_start, open_finish.max(finish)))
            }
            Some((open_start, open_finish)) => {
                total = total.saturating_add(open_finish.saturating_sub(open_start));
                Some((start, finish))
            }
            None => Some((start, finish)),
        };
    }
    if let Some((open_start, open_finish)) = current {
        total = total.saturating_add(open_finish.saturating_sub(open_start));
    }
    total
}

#[derive(Clone, Copy)]
struct LoopInterval {
    started_at_offset_ms: u128,
    finished_at_offset_ms: u128,
}

struct ActiveRunLoop {
    loop_id: &'static str,
    stream_id: &'static str,
    stage: &'static str,
    started_at: Instant,
    started_at_offset_ms: u128,
}

struct CommandStatus {
    exit_code: Option<i32>,
    timed_out: bool,
    success: bool,
    reason: String,
    duration_ms: u128,
}

struct HttpPostOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    http_status: Option<u16>,
}

#[derive(Debug)]
struct FileCommandOutput {
    status: ExitStatus,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

struct SensorStatusWrite<'a> {
    status: &'a str,
    argv: &'a [String],
    duration_ms: u128,
    reason: &'a str,
    exit_code: Option<i32>,
    timed_out: bool,
}

struct SensorSubcommand {
    label: String,
    argv: Vec<String>,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
}

pub(crate) struct ModelRunContext<'a> {
    pub(crate) root: &'a Path,
    pub(crate) review_dir: &'a Path,
    pub(crate) assignments: &'a [ModelAssignment],
    pub(crate) provider_preflights: &'a [ProviderPreflightReceipt],
    pub(crate) shared_context: &'a str,
    pub(crate) args: &'a RunArgs,
    pub(crate) line_map: &'a BTreeSet<(String, u32)>,
    /// Provider API-key presence check. Production passes
    /// `env_value_present`; tests inject a constant so lane scheduling and
    /// runtime fallback can be exercised without mutating process env.
    pub(crate) key_present: fn(&str) -> bool,
    pub(crate) provider_concurrency: ProviderConcurrencyLimits,
}

struct RefuterRunContext<'a> {
    root: &'a Path,
    review_dir: &'a Path,
    provider_preflights: &'a [ProviderPreflightReceipt],
    shared_context: &'a str,
    args: &'a RunArgs,
    model_calls_used: usize,
}

struct ProofPlannerRunContext<'a> {
    root: &'a Path,
    review_dir: &'a Path,
    provider_preflights: &'a [ProviderPreflightReceipt],
    shared_context: &'a str,
    args: &'a RunArgs,
    diff: &'a DiffContext,
    profile: &'a Profile,
    box_state: &'a BoxState,
    pr_thread_context: &'a PrThreadContext,
    model_calls_used: usize,
    key_present: fn(&str) -> bool,
    line_map: &'a BTreeSet<(String, u32)>,
    /// Impact-plan candidate tasks (implementation step 6 of #678) so the
    /// proof-planner model lane can make model-selected proof choices.
    impact_candidates: &'a [ImpactCandidateTask],
}

struct FollowUpRunContext<'a> {
    root: &'a Path,
    out: &'a Path,
    review_dir: &'a Path,
    provider_preflights: &'a [ProviderPreflightReceipt],
    shared_context: &'a str,
    args: &'a RunArgs,
    model_calls_used: usize,
    key_present: fn(&str) -> bool,
    tasks: &'a [FollowUpQuestionTask],
    line_map: &'a BTreeSet<(String, u32)>,
}

struct ModelLaneTask {
    index: usize,
    lane: LanePlan,
    spec: ProviderSpec,
}

struct ModelLaneTaskResult {
    index: usize,
    result: Result<ModelCallOutcome<LaneModelOutput>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReviewBodyAudience {
    PullRequest,
    Artifact,
}

impl ReviewBodyAudience {
    fn include_successful_lane_table(self) -> bool {
        matches!(self, Self::Artifact)
    }
}

struct ModelCallOutcome<T> {
    output: T,
    duration_ms: u128,
    http_status: Option<u16>,
    response_shape: String,
    cache_usage: ModelCacheUsage,
    degraded: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct SensorReceipt {
    status: String,
    reason: String,
    #[serde(default)]
    duration_ms: Option<u128>,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    timed_out: bool,
}

fn runtime_profile_override<'a>(
    legacy_profile: Option<&'a ProfileArg>,
    runtime_profile: Option<&'a ProfileArg>,
) -> Option<&'a str> {
    runtime_profile.or(legacy_profile).map(ProfileArg::key)
}

impl BoxState {
    fn detect() -> Result<Self> {
        Ok(Self {
            cpus: thread::available_parallelism()
                .map(usize::from)
                .unwrap_or(1),
            free_mem_mb: detect_mem_available_mb(),
            free_disk_mb: detect_disk_free_mb(),
            load_1m: detect_load_1m(),
            github_actions: std::env::var_os("GITHUB_ACTIONS").is_some(),
        })
    }

    fn suggested_profile(&self) -> String {
        if self.github_actions {
            return "gh-runner".to_owned();
        }
        match (self.cpus, self.free_mem_mb.unwrap_or(0)) {
            (0..=2, _) | (_, 0..=5_999) => "cx23".to_owned(),
            (3..=4, 6_000..=11_999) => "cx33".to_owned(),
            (5.., 12_000..) => "cx43".to_owned(),
            _ => "cx23".to_owned(),
        }
    }

    fn summary_line(&self) -> String {
        let mem = self
            .free_mem_mb
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        let disk = self
            .free_disk_mb
            .map(|value| value.to_string())
            .unwrap_or_else(|| "unknown".to_owned());
        let load = self
            .load_1m
            .map(|value| format!("{value:.2}"))
            .unwrap_or_else(|| "unknown".to_owned());
        format!(
            "cpus={} mem_free={}MiB disk_free={}MiB load_1m={} github_actions={}",
            self.cpus, mem, disk, load, self.github_actions
        )
    }
}

impl EventLog {
    fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("open event log {}", path.display()))?;
        Ok(Self {
            file: Mutex::new(file),
        })
    }

    fn append<T: Serialize>(&self, kind: &str, payload: T) -> Result<()> {
        let event = Event {
            ts: Utc::now(),
            kind,
            payload,
        };
        let mut file = self
            .file
            .lock()
            .map_err(|_| anyhow::anyhow!("event log mutex poisoned"))?;
        serde_json::to_writer(&mut *file, &event)?;
        use std::io::Write as _;
        writeln!(&mut *file)?;
        Ok(())
    }

    fn sync(&self) -> Result<()> {
        let file = self
            .file
            .lock()
            .map_err(|_| anyhow::anyhow!("event log mutex poisoned"))?;
        file.sync_data().context("sync event log")
    }
}

fn start_run_loop(
    event_log: &EventLog,
    run_started: &Instant,
    loop_id: &'static str,
    stream_id: &'static str,
    stage: &'static str,
) -> Result<ActiveRunLoop> {
    let started_at_offset_ms = run_started.elapsed().as_millis();
    let payload = serde_json::json!({
        "loop_id": loop_id,
        "stream_id": stream_id,
        "stage": stage,
        "started_at_offset_ms": started_at_offset_ms,
    });
    event_log.append(&format!("{loop_id}_loop_started"), payload.clone())?;
    event_log.append(&format!("{stream_id}_stream_started"), payload)?;
    if let Some(role_stream_id) = scheduler_role_stream_id(loop_id)
        && role_stream_id != stream_id
    {
        let role_payload = serde_json::json!({
            "loop_id": loop_id,
            "stream_id": role_stream_id,
            "legacy_stream_id": stream_id,
            "stage": stage,
            "started_at_offset_ms": started_at_offset_ms,
        });
        event_log.append(&format!("{role_stream_id}_stream_started"), role_payload)?;
    }
    Ok(ActiveRunLoop {
        loop_id,
        stream_id,
        stage,
        started_at: Instant::now(),
        started_at_offset_ms,
    })
}

fn finish_run_loop(
    event_log: &EventLog,
    run_started: &Instant,
    tracker: &mut RunLoopTracker,
    active: ActiveRunLoop,
    status: &str,
) -> Result<()> {
    let phase = finish_run_loop_phase(event_log, run_started, active, status)?;
    tracker.record(phase);
    Ok(())
}

fn finish_run_loop_phase(
    event_log: &EventLog,
    run_started: &Instant,
    active: ActiveRunLoop,
    status: &str,
) -> Result<RunLoopPhase> {
    let finished_at_offset_ms = run_started.elapsed().as_millis();
    let duration_ms = active.started_at.elapsed().as_millis();
    let payload = serde_json::json!({
        "loop_id": active.loop_id,
        "stream_id": active.stream_id,
        "stage": active.stage,
        "started_at_offset_ms": active.started_at_offset_ms,
        "finished_at_offset_ms": finished_at_offset_ms,
        "duration_ms": duration_ms,
        "status": status,
    });
    event_log.append(
        &format!("{}_loop_finished", active.loop_id),
        payload.clone(),
    )?;
    event_log.append(&format!("{}_stream_completed", active.stream_id), payload)?;
    if let Some(role_stream_id) = scheduler_role_stream_id(active.loop_id)
        && role_stream_id != active.stream_id
    {
        let role_payload = serde_json::json!({
            "loop_id": active.loop_id,
            "stream_id": role_stream_id,
            "legacy_stream_id": active.stream_id,
            "stage": active.stage,
            "started_at_offset_ms": active.started_at_offset_ms,
            "finished_at_offset_ms": finished_at_offset_ms,
            "duration_ms": duration_ms,
            "status": status,
        });
        event_log.append(&format!("{role_stream_id}_stream_completed"), role_payload)?;
    }
    Ok(RunLoopPhase {
        loop_id: active.loop_id.to_owned(),
        stream_id: active.stream_id.to_owned(),
        stage: active.stage.to_owned(),
        status: status.to_owned(),
        started_at_offset_ms: active.started_at_offset_ms,
        finished_at_offset_ms,
        duration_ms,
    })
}

fn scheduler_role_stream_id(loop_id: &str) -> Option<&str> {
    match loop_id {
        "evidence" | "model" | "proof" => Some(loop_id),
        _ => None,
    }
}

// Computed via inclusion-exclusion (|A| + |B| - |A ∪ B|) rather than a naive
// pairwise-interval sum: if either side has already accumulated multiple
// intervals that overlap each other (the same hazard union_wall_ms exists
// for, e.g. pipelined sub-phases within one loop), a pairwise sum would
// count the shared wall time once per overlapping pair on that side and
// inflate the total. Unioning each side first keeps this exact regardless
// of self-overlap.
fn overlap_ms(left: &[LoopInterval], right: &[LoopInterval]) -> u128 {
    let left_wall = union_wall_ms(left);
    let right_wall = union_wall_ms(right);
    let combined: Vec<LoopInterval> = left.iter().chain(right).copied().collect();
    let combined_wall = union_wall_ms(&combined);
    left_wall
        .saturating_add(right_wall)
        .saturating_sub(combined_wall)
}

fn cmd_doctor(args: DoctorArgs) -> Result<()> {
    let config = Config::load_or_default(
        &args.config,
        runtime_profile_override(args.profile.as_ref(), args.runtime_profile.as_ref()),
    )?;
    let profile = config.selected_profile()?;
    let box_state = BoxState::detect()?;
    let cache_root = cache_root_path(args.cache_dir.as_ref());
    let profile_hash = profile_config_hash(&config)?;
    let require_core_tools = args.require_core_tools || env_flag("UB_REVIEW_STANDARD_IMAGE");
    let mut missing_required = Vec::new();
    let mut version_mismatches = Vec::new();
    let mut fixes = Vec::new();
    println!("Profile: {}", profile.name);
    println!("Box: {}", box_state.summary_line());
    println!("Limits: {}", profile.limits.summary_line());
    println!("Cache root: {}", cache_root.display());
    let current_exe = std::env::current_exe();
    match &current_exe {
        Ok(path) => println!("Binary path: {}", path.display()),
        Err(err) => println!("Binary path: unknown ({err})"),
    }
    println!(
        "Install status: {}",
        doctor_binary_install_status(current_exe.as_deref().ok())
    );
    println!("Profile hash: {}", profile_hash);
    if let Some(base) = args.base.as_deref() {
        match git_tree_sha(&args.root, base) {
            Ok(tree) => {
                let base_dir = base_cache_dir(&cache_root, &tree);
                let hit = base_dir.join("manifest.json").exists();
                println!(
                    "Base cache: {} {} ({})",
                    if hit { "hit" } else { "miss" },
                    tree,
                    base_dir.display()
                );
            }
            Err(err) => println!("Base cache: unknown ({err:#})"),
        }
    }
    println!();
    println!("Tools:");
    for tool in config.tools.values() {
        let command_path = command_path(&tool.command);
        let found = command_path.is_some();
        let status = if found { "found" } else { "missing" };
        let version = if found {
            command_version(&tool.command)
        } else {
            None
        };
        let version_text = version.as_deref().unwrap_or("-");
        let path_text = command_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_else(|| "-".to_owned());
        let rule_hit = cache_root
            .join("rules")
            .join(&tool.id)
            .join("manifest.json")
            .exists();
        let expected = expected_standard_image_tool_version(&tool.id);
        let fix_entry = if !found {
            Some(format!(
                "{} missing: {}",
                tool.id,
                doctor_tool_install_hint(&tool.id)
            ))
        } else {
            expected.and_then(|expected| match version.as_deref() {
                Some(actual) if command_version_matches(actual, expected) => None,
                Some(_) => Some(format!(
                    "{} version drift: {}",
                    tool.id,
                    doctor_tool_version_fix(&tool.id, expected)
                )),
                None => Some(format!(
                    "{} version unknown: {}",
                    tool.id,
                    doctor_tool_version_fix(&tool.id, expected)
                )),
            })
        };
        println!(
            "  {:<16} {:<8} {:<24} path={} version={} expected={} rule-cache={}",
            tool.id,
            status,
            tool.command,
            path_text,
            version_text,
            expected.unwrap_or("-"),
            if rule_hit { "hit" } else { "miss" }
        );
        if let Some(fix) = fix_entry {
            fixes.push(fix);
        }
        if require_core_tools && is_core_review_tool(&tool.id) {
            if !found {
                missing_required.push(tool.id.clone());
            } else if let Some(expected) = expected_standard_image_tool_version(&tool.id) {
                match version.as_deref() {
                    Some(actual) if command_version_matches(actual, expected) => {}
                    Some(actual) => {
                        version_mismatches
                            .push(format!("{} expected {}, got {}", tool.id, expected, actual));
                    }
                    None => {
                        version_mismatches.push(format!(
                            "{} expected {}, got no --version output",
                            tool.id, expected
                        ));
                    }
                }
            }
        }
    }
    println!();
    println!("Providers:");
    for provider in [ModelProvider::MiniMaxDirect, ModelProvider::OpenCodeGo] {
        let env_name = model_api_key_env(provider);
        let status = if env_value_present(env_name) {
            "present"
        } else {
            "missing"
        };
        println!("  {:<16} {:<8} env={}", provider.key(), status, env_name);
    }
    if !fixes.is_empty() {
        println!();
        println!("Fixes:");
        for fix in &fixes {
            println!("  - {fix}");
        }
    }
    if !missing_required.is_empty() {
        bail!(
            "required core review tools missing from standard image: {}; see Fixes above",
            missing_required.join(", ")
        );
    }
    if !version_mismatches.is_empty() {
        bail!(
            "required core review tool versions drifted from standard image pins: {}; see Fixes above",
            version_mismatches.join(", ")
        );
    }
    Ok(())
}

fn cmd_cache(args: CacheArgs) -> Result<()> {
    match args.command {
        CacheCommand::Warm(args) => cmd_cache_warm(args),
    }
}

fn cmd_cache_warm(args: CacheWarmArgs) -> Result<()> {
    let config = Config::load_or_default(
        &args.config,
        runtime_profile_override(args.profile.as_ref(), args.runtime_profile.as_ref()),
    )?;
    let profile = config.selected_profile()?;
    let cache_root = cache_root_path(args.cache_dir.as_ref());
    let profile_hash = profile_config_hash(&config)?;
    let base_tree_sha = git_tree_sha(&args.root, &args.base)?;
    let base_dir = base_cache_dir(&cache_root, &base_tree_sha);
    let rules_dir = cache_root.join("rules");
    fs::create_dir_all(&base_dir)?;
    fs::create_dir_all(&rules_dir)?;

    let mut tools = Vec::new();
    for tool_id in CORE_REVIEW_TOOLS {
        let Some(tool) = config.tools.get(tool_id) else {
            continue;
        };
        let rule_dir = rules_dir.join(&tool.id);
        let tool_base_dir = base_dir.join(&tool.id);
        fs::create_dir_all(&rule_dir)?;
        fs::create_dir_all(&tool_base_dir)?;
        let version = command_version(&tool.command);
        let tool_manifest = serde_json::json!({
            "schema_version": 1,
            "tool": tool.id,
            "command": tool.command,
            "version": version.clone(),
            "profile_hash": profile_hash,
            "base_tree_sha": base_tree_sha,
        });
        fs::write(
            rule_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&tool_manifest)?,
        )?;
        fs::write(
            tool_base_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&tool_manifest)?,
        )?;
        tools.push(ToolCacheReceipt {
            tool: tool.id.clone(),
            command: tool.command.clone(),
            status: if version.is_some() {
                "found".to_owned()
            } else {
                "missing".to_owned()
            },
            version,
            rule_cache_dir: rule_dir.display().to_string(),
            base_cache_dir: tool_base_dir.display().to_string(),
        });
    }
    let manifest = CacheWarmManifest {
        schema_version: 1,
        profile: profile.name.clone(),
        profile_hash,
        base: args.base,
        base_tree_sha: base_tree_sha.clone(),
        cache_root: cache_root.display().to_string(),
        base_cache_dir: base_dir.display().to_string(),
        rules_cache_dir: rules_dir.display().to_string(),
        tools,
    };
    fs::write(
        base_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    fs::write(
        cache_root.join("latest-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    println!("warmed cache {}", base_dir.display());
    println!("base tree {}", base_tree_sha);
    Ok(())
}

fn cmd_plan(args: PlanArgs) -> Result<()> {
    let (config, diff, box_state, plan) =
        prepare_plan(&args.review, args.allow_heavy, &args.selectors)?;
    print_plan(&plan, &box_state);
    if args.write {
        write_plan_artifacts(
            &args.review.out,
            &config,
            &diff,
            &box_state,
            &plan,
            PlanArtifactSelectors {
                run_args: None,
                selectors: &args.selectors,
                effective_model_lanes: None,
            },
        )?;
    }
    Ok(())
}

/// Result of a completed `run` invocation. All artifacts are written before
/// this value is returned; `main` turns it into the process exit decision so a
/// failing gate can never truncate artifacts.
struct RunCompletion {
    gate_conclusion: String,
    fail_on_gate: bool,
    run_dir: PathBuf,
}

fn cmd_run(args: RunArgs) -> Result<RunCompletion> {
    let run_started = Instant::now();
    let mut args = normalize_run_args(args)?;
    let run_pass = resolved_run_pass(args.run_pass);
    let (mut config, diff, box_state, plan) =
        prepare_plan(&args.review, args.allow_heavy, &args.selectors)?;
    // #719: apply the user-facing review-mode preset (advisory/gate/strict)
    // when set. This overrides --mode, --fail-on-gate, and
    // [gate].review_forward before any downstream resolution reads them, so
    // the resolved triple flows into the gate outcome unchanged.
    apply_review_mode_preset(&mut args, &mut config);
    // D2 precedence (spec 0006): an explicit --provider-policy / env value
    // wins; `auto` defers to the repo's [providers].policy; with neither,
    // `auto` keeps its built-in minimax-primary semantics.
    args.provider_policy = resolved_provider_policy(&config, args.provider_policy);
    args.minimax_prompt_cache = resolved_minimax_prompt_cache(&config);
    let profile = config.selected_profile()?;
    apply_runtime_profile_limits(&mut args, profile)?;
    let selected_model_lanes = selected_review_lanes_for_args(&plan, &args)?;
    print_plan(&plan, &box_state);
    write_plan_artifacts(
        &args.review.out,
        &config,
        &diff,
        &box_state,
        &plan,
        PlanArtifactSelectors {
            run_args: Some(&args),
            selectors: &args.selectors,
            effective_model_lanes: Some(&selected_model_lanes),
        },
    )?;

    let event_log = Arc::new(EventLog::open(&args.review.out.join("events.ndjson"))?);
    let mut run_loop_tracker = RunLoopTracker::new();
    event_log.append(
        "run_started",
        serde_json::json!({"base": args.review.base, "head": args.review.head, "profile": plan.profile_name, "dry_run": args.dry_run, "run_pass": run_pass.key(), "sensor_phases": args.sensor_phases.key()}),
    )?;

    let evidence_loop = start_run_loop(
        &event_log,
        &run_started,
        "evidence",
        "coordination",
        "sensors-and-packet",
    )?;
    // #325 pipelined scheduler: fast sensors block here because they feed the
    // shared precontext the lanes launch from; late-phase sensors (test,
    // build, coverage, lease-gated witnesses) overlap the model wave on a
    // background pool and are joined inside write_review_artifacts before the
    // reporter, tool-status/gate-outcome computation, the review compiles,
    // and the gate — so the gate always evaluates complete sensor evidence.
    let mut late_phase: Option<LateSensorPhase> = None;
    if args.dry_run {
        write_dry_run_sensor_receipts(&args.review.root, &args.review.out, &plan, &event_log)?;
        event_log.append("run_dry", serde_json::json!({"reason": "--dry-run"}))?;
    } else {
        write_skipped_sensor_receipts(&args.review.root, &args.review.out, &plan, &event_log)?;
        let runnable = plan
            .sensors
            .iter()
            .filter(|sensor| sensor.run)
            .cloned()
            .collect::<Vec<SensorPlan>>();
        let (fast, late): (Vec<SensorPlan>, Vec<SensorPlan>) = match args.sensor_phases {
            SensorPhasesMode::Serial => (runnable, Vec::new()),
            SensorPhasesMode::Pipelined => runnable
                .into_iter()
                .partition(|sensor| matches!(sensor.phase, SensorPhase::Fast)),
        };
        if fast.is_empty() && late.is_empty() {
            event_log.append("sensors_empty", serde_json::json!({}))?;
        } else {
            if !fast.is_empty() {
                let jobs = sensor_job_count(profile, fast.len())?;
                run_sensor_pool(
                    &args.review.root,
                    &args.review.out,
                    &plan,
                    jobs,
                    fast.into(),
                    &event_log,
                )?;
            }
            if !late.is_empty() {
                late_phase = Some(spawn_late_sensor_phase(
                    &args.review.root,
                    &args.review.out,
                    &plan,
                    profile,
                    &event_log,
                    &run_started,
                    late,
                )?);
            }
        }
    }
    let pr_thread_context = collect_pr_thread_context(&args.review.root, &args)?;

    write_lane_packets(
        &args.review.out,
        &diff,
        &plan,
        &selected_model_lanes,
        &pr_thread_context,
        &event_log,
    )?;
    finish_run_loop(
        &event_log,
        &run_started,
        &mut run_loop_tracker,
        evidence_loop,
        "completed",
    )?;
    let preliminary_summary = render_summary(&args.review.out, &plan, &diff)?;
    fs::write(
        args.review.out.join("running-summary.md"),
        &preliminary_summary,
    )?;
    let gate_outcome = write_review_artifacts(
        &args.review.root,
        &args.review.out,
        &config,
        &diff,
        &box_state,
        &plan,
        &preliminary_summary,
        pr_thread_context,
        &args,
        &event_log,
        &run_started,
        &mut run_loop_tracker,
        run_started.elapsed(),
        late_phase,
    )?;
    let summary = render_summary(&args.review.out, &plan, &diff)?;
    fs::write(args.review.out.join("running-summary.md"), &summary)?;
    if config.review.github_summary && !args.no_github_summary {
        append_github_step_summary(&summary)?;
    }
    println!("wrote {}", args.review.out.display());
    println!("open {}/running-summary.md", args.review.out.display());
    Ok(RunCompletion {
        gate_conclusion: gate_outcome.conclusion,
        fail_on_gate: args.fail_on_gate.resolved(args.mode),
        run_dir: args.review.out,
    })
}

fn cmd_summary(args: SummaryArgs) -> Result<()> {
    let plan: Plan = serde_json::from_slice(&fs::read(args.run_dir.join("plan.json"))?)?;
    let diff: DiffContext =
        serde_json::from_slice(&fs::read(args.run_dir.join("input/diff-context.json"))?)?;
    let summary = render_summary(&args.run_dir, &plan, &diff)?;
    fs::write(args.run_dir.join("running-summary.md"), summary)?;
    println!("wrote {}/running-summary.md", args.run_dir.display());
    Ok(())
}

fn cmd_post(args: PostArgs) -> Result<()> {
    fs::create_dir_all(&args.out)?;
    if !args.review_json.exists()
        && let Some(skip) = read_github_review_skip_receipt(&args.review_json)
    {
        fs::write(
            args.out.join("post-result.json"),
            serde_json::to_vec_pretty(&skip)?,
        )?;
        println!(
            "skipped GitHub review post; wrote {}/post-result.json",
            args.out.display()
        );
        run_issue_broker_step(&args);
        return Ok(());
    }
    let post_outcome = match post_github_review(&args) {
        Ok(value) => {
            fs::write(
                args.out.join("post-result.json"),
                serde_json::to_vec_pretty(&value)?,
            )?;
            println!("wrote {}/post-result.json", args.out.display());
            Ok(())
        }
        Err(err) => {
            let value = build_post_error_receipt(&args, &err);
            fs::write(
                args.out.join("post-error.json"),
                serde_json::to_vec_pretty(&value)?,
            )?;
            if args.fail_on_post_error {
                Err(err)
            } else {
                eprintln!(
                    "ub-review post failed; wrote {}/post-error.json",
                    args.out.display()
                );
                Ok(())
            }
        }
    };
    // The issue broker runs after the review submission attempt on every
    // path: it has its own receipts (issue_broker_results.json), the
    // fingerprint duplicate search makes it idempotent across passes, and
    // its failures never change the post exit code.
    run_issue_broker_step(&args);
    post_outcome
}

/// Execute the run-written broker plan, never fatally: read
/// review/issue_broker_plan.json next to the review payload, perform the
/// remote duplicate search and opens for `attempt` entries, and write
/// issue_broker_results artifacts. Absent plan means the broker was not
/// opted in; any whole-step error is reported to stderr and swallowed
/// (broker outcomes never affect the gate or the post exit code).
fn read_repair_queue(
    sensor_dir: &Path,
    artifacts: &UnsafeReviewGate,
) -> std::collections::BTreeMap<String, RepairQueueEntry> {
    let out_dir = sensor_dir.join(UNSAFE_REVIEW_OUTPUT_SUBDIR);
    let rq_path = artifacts
        .artifacts
        .get("repair_queue")
        .map(|rel| out_dir.join(rel))
        .unwrap_or_else(|| out_dir.join("repair-queue.json"));
    let text = match fs::read_to_string(&rq_path) {
        Ok(t) => t,
        Err(_) => return std::collections::BTreeMap::new(),
    };
    let rq: RepairQueueFile = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(_) => return std::collections::BTreeMap::new(),
    };
    let mut by_card: std::collections::BTreeMap<String, RepairQueueEntry> =
        std::collections::BTreeMap::new();
    for entries in rq.buckets.into_values() {
        for entry in entries {
            by_card.entry(entry.card_id.clone()).or_insert(entry);
        }
    }
    by_card
}

/// Build `GitHubReviewComment` entries from unsafe-review `comment-plan.json`
/// candidates for inline posting on the PR diff.
///
/// # Selection rules
/// - Only candidates with `changed_line: true` are eligible; anchoring to an
///   unchanged line would be rejected by the GitHub review API.
/// - Capped at `min(comment_plan.len(), max_inline_budget)` (the comment plan
///   is already bounded to ≤3 by unsafe-review itself).
/// - Deduplication against `existing_paths_lines` prevents double-posting if a
///   model lane already proposed a comment on the same `(path, line)` pair.
///
/// # Suggestion blocks
/// Each comment body names the `coverage_gap`, the next reviewer action
/// (`confirmation_state`), and the per-entry `trust_boundary`. A GitHub
/// `suggestion` block is emitted ONLY when unsafe-review's repair-queue
/// provides a concrete applicable code edit for the site. As of
/// `repair-queue/0.1`, the queue provides bucket classification and guidance
/// (missing evidence, do-not-do constraints) but NO replacement text — so
/// suggestion blocks are NOT emitted from this source. The field remains ready
/// on `GitHubReviewComment` for a future repair-queue version that adds
/// `replacement` / `applicable_edit` output. This is an honest capability gap
/// reported upstream as a narrow follow-up issue.
#[cfg(test)]
fn build_unsafe_review_inline_comments(
    sensor_dir: &Path,
    existing_paths_lines: &std::collections::BTreeSet<(String, u32)>,
    right_side_lines: &std::collections::BTreeSet<(String, u32)>,
    max_inline_budget: usize,
) -> Vec<GitHubReviewComment> {
    let artifacts = match read_unsafe_review_artifacts(sensor_dir) {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };
    let repair_queue = read_repair_queue(sensor_dir, &artifacts.gate);
    let trust = artifacts
        .gate
        .trust_boundary
        .as_deref()
        .unwrap_or("advisory");
    let mut comments: Vec<GitHubReviewComment> = Vec::new();
    for entry in &artifacts.comment_plan {
        if comments.len() >= max_inline_budget {
            break;
        }
        // Only anchor to changed lines — GitHub review API requires it.
        if entry.changed_line != Some(true) {
            continue;
        }
        let (Some(path), Some(line)) = (entry.path.as_deref(), entry.line) else {
            continue;
        };
        // Dedup: skip if a model lane already claimed this (path, line).
        let norm_path = normalize_repo_path(path);
        if !right_side_lines.contains(&(norm_path.clone(), line)) {
            continue;
        }
        if existing_paths_lines.contains(&(norm_path.clone(), line)) {
            continue;
        }
        let gap = entry
            .coverage_gap
            .as_deref()
            .unwrap_or("unsafe coverage gap");
        let action = entry
            .confirmation_state
            .as_deref()
            .unwrap_or("reviewer confirmation required");
        let card_label = entry
            .card_id
            .as_deref()
            .map(|id| format!(" (`{id}`)"))
            .unwrap_or_default();
        // Optional: if the repair queue has an entry for this card, surface the
        // bucket reason, operation, and missing evidence as additional context
        // (guidance only, not a suggestion block).
        let rq_entry = entry.card_id.as_deref().and_then(|id| repair_queue.get(id));
        let rq_context = rq_entry
            .map(|rq_entry| {
                let bucket = rq_entry
                    .bucket_reason
                    .as_deref()
                    .unwrap_or("see repair-queue.json");
                let operation_line = rq_entry
                    .operation
                    .as_deref()
                    .map(|op| format!("\n\n**Operation**: `{op}`"))
                    .unwrap_or_default();
                let evidence_lines = rq_entry
                    .missing_evidence
                    .iter()
                    .map(|e| format!("  - {e}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                if evidence_lines.is_empty() {
                    format!("\n\n**Repair class**: {bucket}{operation_line}")
                } else {
                    format!(
                        "\n\n**Repair class**: {bucket}{operation_line}\n\n**Missing evidence**:\n{evidence_lines}"
                    )
                }
            })
            .unwrap_or_default();
        let suggestion = rq_entry.and_then(RepairQueueEntry::suggestion);
        let body = format!(
            "[unsafe-review]{card_label} **{gap}**\n\n\
             **Next action**: {action}\n\n\
             **Trust boundary** (advisory): {trust}{rq_context}\n\n\
             _Suggestion sourced from unsafe-review advisory output. \
             Apply only after reviewer verification. \
             Inline comments are advisory — they do not change the merge decision._"
        );
        // No suggestion block: repair-queue/0.1 provides guidance (missing
        // evidence, bucket classification, do-not-do constraints) but no
        // concrete replacement text. suggestion = None until a future
        // repair-queue version adds an applicable edit field.
        comments.push(GitHubReviewComment {
            path: norm_path,
            line,
            side: "RIGHT".to_owned(),
            body,
            suggestion,
        });
    }
    comments
}

fn unsafe_review_comment_plan_candidates(
    sensor_dir: &Path,
) -> (Vec<ModelCandidateComment>, Vec<SummaryOnlyFinding>) {
    let mut candidates = Vec::new();
    let mut skips = Vec::new();
    let artifacts = match read_unsafe_review_artifacts(sensor_dir) {
        Ok(artifacts) => artifacts,
        Err(_) => return (candidates, skips),
    };
    let repair_queue = read_repair_queue(sensor_dir, &artifacts.gate);
    let gate_trust = artifacts
        .gate
        .trust_boundary
        .as_deref()
        .unwrap_or("advisory");
    for entry in &artifacts.comment_plan {
        if entry.changed_line != Some(true) {
            let label = entry
                .card_id
                .as_deref()
                .map(|id| format!(" `{id}`"))
                .unwrap_or_default();
            skips.push(SummaryOnlyFinding {
                lane: "unsafe-review".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: format!(
                    "unsafe-review comment-plan{label} did not target a changed RIGHT-side line; kept artifact-only"
                ),
                evidence: "unsafe-review comment-plan changed_line guard".to_owned(),
            });
            continue;
        }
        let (Some(path), Some(line)) = (entry.path.as_deref(), entry.line) else {
            let label = entry
                .card_id
                .as_deref()
                .map(|id| format!(" `{id}`"))
                .unwrap_or_default();
            skips.push(SummaryOnlyFinding {
                lane: "unsafe-review".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: format!(
                    "unsafe-review comment-plan{label} lacked path or line; kept artifact-only"
                ),
                evidence: "unsafe-review comment-plan anchor guard".to_owned(),
            });
            continue;
        };
        let gap = entry
            .coverage_gap
            .as_deref()
            .unwrap_or("unsafe coverage gap");
        let action = entry
            .confirmation_state
            .as_deref()
            .unwrap_or("reviewer confirmation required");
        let card_label = entry
            .card_id
            .as_deref()
            .map(|id| format!(" (`{id}`)"))
            .unwrap_or_default();
        let trust = entry.trust_boundary.as_deref().unwrap_or(gate_trust);
        let rq_entry = entry.card_id.as_deref().and_then(|id| repair_queue.get(id));
        let rq_context = rq_entry
            .map(|rq_entry| {
                let bucket = rq_entry
                    .bucket_reason
                    .as_deref()
                    .unwrap_or("see repair-queue.json");
                let operation_line = rq_entry
                    .operation
                    .as_deref()
                    .map(|op| format!("\n\n**Operation**: `{op}`"))
                    .unwrap_or_default();
                let evidence_lines = rq_entry
                    .missing_evidence
                    .iter()
                    .map(|e| format!("  - {e}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                if evidence_lines.is_empty() {
                    format!("\n\n**Repair class**: {bucket}{operation_line}")
                } else {
                    format!(
                        "\n\n**Repair class**: {bucket}{operation_line}\n\n**Missing evidence**:\n{evidence_lines}"
                    )
                }
            })
            .unwrap_or_default();
        let suggestion = rq_entry.and_then(RepairQueueEntry::suggestion);
        let body = truncate_chars(
            &format!(
                "**{gap}**{card_label}\n\n\
                 **Next action**: {action}\n\n\
                 **Trust boundary** (advisory): {trust}{rq_context}\n\n\
                 _Sourced from unsafe-review advisory output. \
                 Apply only after reviewer verification. \
                 Inline comments are advisory - they do not change the merge decision._"
            ),
            1_100,
        );
        let selection = entry
            .selection_reason
            .as_deref()
            .unwrap_or("deterministic comment-plan candidate");
        candidates.push(ModelCandidateComment {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: path.to_owned(),
            line,
            body,
            evidence: format!(
                "unsafe-review comment-plan{card_label}: {selection}; confirmation_state: {action}"
            ),
            suggestion,
        });
    }
    (candidates, skips)
}

fn apply_unsafe_review_comment_plan_candidates(
    sensor_dir: &Path,
    line_map: &BTreeSet<(String, u32)>,
    sinks: ModelOutputSinks<'_>,
) {
    let (candidates, skips) = unsafe_review_comment_plan_candidates(sensor_dir);
    sinks.summary_only_findings.extend(skips);
    if candidates.is_empty() {
        return;
    }
    let lane = unsafe_review_sensor_lane();
    let output = LaneModelOutput {
        summary: None,
        inline_comments: Vec::new(),
        candidate_findings: candidates,
        summary_only_findings: Vec::new(),
        observations: Vec::new(),
        failed_objections: Vec::new(),
        proof_requests: Vec::new(),
        proof_intents: Vec::new(),
        issue_candidates: Vec::new(),
        degraded: false,
    };
    apply_model_output(&lane, output, line_map, sinks);
}

fn route_follow_up_proof_receipts(
    message_log: &MessageLog,
    event_log: &EventLog,
    receipts: &[ProofReceipt],
) {
    for receipt in receipts {
        let references = vec![format!("review/proof_receipts.json#{}", receipt.id)];
        for consumer in receipt_route_consumers(receipt) {
            if let Err(error) = message_log.append(
                CrossLaneMessageKind::EvidenceRouted,
                "proof-broker",
                &consumer,
                0,
                references.clone(),
                serde_json::json!({
                    "receipt_id": &receipt.id,
                    "result": &receipt.result,
                    "reason": &receipt.reason,
                    "phase": "follow-up",
                    "reconsider": true,
                }),
            ) {
                let _ = event_log.append(
                    "message_log_error",
                    serde_json::json!({
                        "receipt": &receipt.id,
                        "kind": "evidence_routed",
                        "error": format!("{error:#}"),
                    }),
                );
            }
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
fn write_review_artifacts(
    root: &Path,
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    box_state: &BoxState,
    plan: &Plan,
    running_summary: &str,
    pr_thread_context: PrThreadContext,
    args: &RunArgs,
    event_log: &EventLog,
    run_started: &Instant,
    run_loop_tracker: &mut RunLoopTracker,
    elapsed: Duration,
    late_phase: Option<LateSensorPhase>,
) -> Result<GateOutcome> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    // Order 8 (#678): cross-lane message queue — the lossless working-memory
    // surface the reporter (Order 9) reads from. Sibling to the event log.
    let message_log = MessageLog::open(&review_dir.join("messages.ndjson"))?;
    let profile = config.selected_profile()?;
    let provider_concurrency = provider_concurrency_limits(config);
    let mut proof_requests = Vec::new();
    append_configured_required_proof_requests(config, diff, args, &mut proof_requests);
    let (shared_context, prefix_sections) = render_shared_context(
        root,
        out,
        config,
        diff,
        plan,
        running_summary,
        args,
        &pr_thread_context,
        profile,
        &proof_requests,
    )?;
    fs::write(review_dir.join("shared_context.md"), &shared_context)?;
    // Shared-prefix manifest (Order 6 of #678): records the byte-stable
    // shared-prefix contract — hash, byte length, base/head, ordered source
    // sections with byte ranges, cache policy, truncations — so the cohort's
    // cache-coherence claim is inspectable.
    let shared_context_id = sha256_hex(shared_context.as_bytes());
    write_shared_prefix_manifest(
        &review_dir,
        &shared_context_id,
        shared_context.len(),
        diff,
        &prefix_sections,
        args,
    )?;
    fs::write(
        review_dir.join("pr_thread_context.json"),
        serde_json::to_vec_pretty(&pr_thread_context)?,
    )?;
    let prior_resolved_candidates = load_prior_resolved_candidates(root, out, args)?;
    let line_map = right_side_diff_lines(&diff.patch);
    let assignments = model_assignments(plan, args)?;
    let mut provider_preflights = build_provider_preflight_receipts(&assignments, args);
    if should_run_proof_planner_model_lane(args, diff) {
        ensure_provider_preflight_receipts_for_assignment(
            &mut provider_preflights,
            &proof_planner_assignment(args),
            args,
        );
    }
    write_shared_context_cache_artifacts(
        out,
        &shared_context,
        &assignments,
        &provider_preflights,
        &[],
        &[],
        args,
    )?;
    let mut model_lanes = build_model_lane_receipts(&assignments, args);
    // Build the impact plan before the model wave and write the complete
    // artifact in every mode. Only explicit active mode may expose its ranked
    // candidates to model proof planning (implementation step 6 of #678).
    let impact_mode = config.impact.resolved_mode();
    let impact_plan = build_impact_plan(root, &diff.changed_files, impact_mode);
    let impact_plan_candidate_tasks = impact_plan.proof_planner_candidates().to_vec();
    write_impact_plan(out, &impact_plan)?;
    // Sensor evidence issues are collected after the late-phase join below
    // (#325): collecting here would misread still-running late sensors as
    // missing evidence.
    let mut missing_or_failed_model_evidence = model_lanes
        .iter()
        .filter(|receipt| is_model_receipt_evidence_issue(receipt))
        .map(model_issue_from_receipt)
        .collect::<Vec<_>>();
    let mut summary_only_findings = Vec::new();
    let mut inline_comments = Vec::new();
    let mut model_observations = Vec::new();
    let mut proof_intents = Vec::new();
    let mut issue_candidates: Vec<IssueCandidate> = Vec::new();
    let mut model_calls_used = 0usize;
    let seeded_proof_requests = proof_requests.clone();

    let mut proof_result = ProofBrokerResult::default();
    if matches!(args.model_mode, ModelMode::Auto) {
        let initial_proof_loop = start_run_loop(
            event_log,
            run_started,
            "proof",
            "proof",
            "initial-diff-broker",
        )?;
        thread::scope(|scope| -> Result<()> {
            let proof_handle = scope.spawn(move || {
                run_seeded_proof_stream_v0(
                    root,
                    out,
                    diff,
                    profile,
                    args,
                    &seeded_proof_requests,
                    initial_proof_loop,
                    event_log,
                    run_started,
                    box_state,
                )
            });

            let model_loop =
                start_run_loop(event_log, run_started, "model", "investigation", "primary")?;
            run_provider_preflights(
                root,
                &review_dir,
                &mut provider_preflights,
                &shared_context,
                args,
            )?;
            append_preflight_evidence_issues(
                &provider_preflights,
                &mut missing_or_failed_model_evidence,
            );
            model_calls_used = run_available_model_lanes(
                ModelRunContext {
                    root,
                    review_dir: &review_dir,
                    assignments: &assignments,
                    provider_preflights: &provider_preflights,
                    shared_context: &shared_context,
                    args,
                    line_map: &line_map,
                    key_present: env_value_present,
                    provider_concurrency,
                },
                &mut model_lanes,
                &mut missing_or_failed_model_evidence,
                &mut inline_comments,
                &mut summary_only_findings,
                &mut model_observations,
                &mut proof_requests,
                &mut proof_intents,
                &mut issue_candidates,
            )?;
            // Order 7 (#678): record each executed lane's primary-wave turn as a
            // persistent thread artifact (review/threads/<lane>/turn-000.json +
            // thread.json). This makes a lane's logical history inspectable and
            // gives the reporter (Order 9) a thread to address. Written from the
            // executed receipts; lanes that only reached preflight/planned
            // (empty thread_id) are skipped.
            for receipt in &model_lanes {
                if receipt.thread_id.is_empty() {
                    continue;
                }
                let routed: Vec<String> = assignments
                    .iter()
                    .find(|a| a.lane.id == receipt.lane)
                    .map(|a| a.lane.receives.clone())
                    .unwrap_or_default();
                let receipt_ref = format!("review/model/{}/content.json", receipt.lane);
                let turn = primary_turn(
                    &receipt.thread_id,
                    &receipt.lane,
                    &receipt.reason,
                    routed,
                    &receipt_ref,
                );
                let _ = write_lane_thread_turn(
                    &review_dir,
                    &receipt.lane,
                    &turn,
                    &receipt.cohort_id,
                    &receipt.status,
                );
                // Order 8 (#678): emit a lane_report message to the cross-lane
                // queue so the reporter (Order 9) can consume the lane's
                // conclusion. Also emit thread_terminal when the lane is done.
                // Message-log errors are logged to the event log, not
                // propagated (the queue is observability, not a run dependency).
                let turn_ref = format!("review/threads/{}/turn-000.json", receipt.lane);
                if let Err(e) = message_log.append(
                    CrossLaneMessageKind::LaneReport,
                    &receipt.lane,
                    "reporter",
                    0,
                    vec![turn_ref.clone()],
                    serde_json::json!({
                        "status": receipt.status,
                        "conclusion": receipt.reason,
                        "cohort_id": receipt.cohort_id,
                    }),
                ) {
                    let _ = event_log.append(
                        "message_log_error",
                        serde_json::json!({"lane": receipt.lane, "kind": "lane_report", "error": format!("{e:#}")}),
                    );
                }
                if matches!(
                    receipt.status.as_str(),
                    "ok" | "degraded" | "failed" | "skipped_budget" | "preflight_failed"
                ) && let Err(e) = message_log.append(
                    CrossLaneMessageKind::ThreadTerminal,
                    &receipt.lane,
                    "reporter",
                    0,
                    vec![turn_ref],
                    serde_json::json!({"terminal_reason": receipt.status}),
                ) {
                    let _ = event_log.append(
                        "message_log_error",
                        serde_json::json!({"lane": receipt.lane, "kind": "thread_terminal", "error": format!("{e:#}")}),
                    );
                }
            }
            dedupe_inline_comments(&mut inline_comments, &mut summary_only_findings);
            apply_unsafe_review_comment_plan_candidates(
                &out.join("sensors").join("unsafe-review"),
                &line_map,
                ModelOutputSinks {
                    inline_comments: &mut inline_comments,
                    summary_only_findings: &mut summary_only_findings,
                    model_observations: &mut model_observations,
                    proof_requests: &mut proof_requests,
                    proof_intents: &mut proof_intents,
                    issue_candidates: &mut issue_candidates,
                },
            );
            dedupe_inline_comments(&mut inline_comments, &mut summary_only_findings);
            model_calls_used += run_proof_planner_model_lane(
                ProofPlannerRunContext {
                    root,
                    review_dir: &review_dir,
                    provider_preflights: &provider_preflights,
                    shared_context: &shared_context,
                    args,
                    diff,
                    profile,
                    box_state,
                    pr_thread_context: &pr_thread_context,
                    model_calls_used,
                    key_present: env_value_present,
                    line_map: &line_map,
                    impact_candidates: &impact_plan_candidate_tasks,
                },
                &mut model_lanes,
                &mut missing_or_failed_model_evidence,
                &mut model_observations,
                &mut proof_requests,
                &mut proof_intents,
            )?;
            model_calls_used += run_refuter_pass(
                RefuterRunContext {
                    root,
                    review_dir: &review_dir,
                    provider_preflights: &provider_preflights,
                    shared_context: &shared_context,
                    args,
                    model_calls_used,
                },
                &mut model_lanes,
                &mut missing_or_failed_model_evidence,
                &mut inline_comments,
                &mut summary_only_findings,
            )?;
            append_cross_lane_conflict_observations(
                &inline_comments,
                &summary_only_findings,
                &mut model_observations,
            );
            finish_run_loop(
                event_log,
                run_started,
                run_loop_tracker,
                model_loop,
                "completed",
            )?;

            let (seeded_result, proof_phases) = proof_handle
                .join()
                .map_err(|_| anyhow::anyhow!("seeded proof stream thread panicked"))??;
            for phase in proof_phases {
                run_loop_tracker.record(phase);
            }
            proof_result = seeded_result;
            Ok(())
        })?;
    } else {
        let model_loop =
            start_run_loop(event_log, run_started, "model", "investigation", "primary")?;
        finish_run_loop(
            event_log,
            run_started,
            run_loop_tracker,
            model_loop,
            "skipped_model_mode_off",
        )?;
        let initial_proof_loop = start_run_loop(
            event_log,
            run_started,
            "proof",
            "proof",
            "initial-diff-broker",
        )?;
        proof_result = run_initial_diff_proof_broker_v0(
            root,
            out,
            diff,
            profile,
            args,
            box_state,
            run_started,
        )?;
        finish_run_loop(
            event_log,
            run_started,
            run_loop_tracker,
            initial_proof_loop,
            "completed",
        )?;
    }
    // #325 stream-as-it-lands join point: the late evidence phase lands here,
    // before anything downstream reads sensor state — the model-request proof
    // broker, the reporter (which routes late receipts into lane continuation
    // turns), tool-status/tool-gate artifacts, sensor evidence-issue
    // collection, both review compiles, and the gate. A late sensor that
    // failed or never wrote a receipt surfaces as missing evidence through
    // the same collectors as before — never as clean evidence.
    let late_sensor_ids: Vec<String> = late_phase
        .as_ref()
        .map(|phase| phase.sensor_ids.clone())
        .unwrap_or_default();
    if let Some(phase) = late_phase {
        phase.join(event_log, run_started, run_loop_tracker)?;
    }
    let tool_gate_outcome_artifact = write_tool_status_artifacts(out, config, profile, plan)?;
    let tool_gate_outcomes: &[ToolGateOutcomeEntry] = &tool_gate_outcome_artifact.outcomes;
    let missing_or_failed_sensor_evidence = collect_sensor_evidence_issues(out, plan);
    let late_sensor_evidence = late_sensor_receipt_digests(out, &late_sensor_ids);
    for digest in &late_sensor_evidence {
        if let Err(e) = message_log.append(
            CrossLaneMessageKind::EvidenceRouted,
            "runner",
            "reporter",
            0,
            vec![digest.receipt_path.clone()],
            serde_json::json!({
                "sensor": digest.sensor,
                "status": digest.status,
                "reason": digest.reason,
                "phase": "late",
            }),
        ) {
            let _ = event_log.append(
                "message_log_error",
                serde_json::json!({"sensor": digest.sensor, "kind": "evidence_routed", "error": format!("{e:#}")}),
            );
        }
    }
    // Run the model-request proof broker BEFORE the reporter so proof receipts
    // are available for routing to lanes in the multi-turn continuation.
    // Previously this ran after the reporter, meaning continuation prompts
    // never saw proof evidence. (Order 9c fix of #678.)
    attach_request_metadata_to_focused_receipts(
        diff,
        &proof_requests,
        &mut proof_result.proof_receipts,
    );
    write_proof_planner_artifacts(ProofPlannerArtifactContext {
        out,
        diff,
        plan,
        profile,
        box_state,
        pr_thread_context: &pr_thread_context,
        proof_requests: &proof_requests,
        additional_intents: &proof_intents,
    })?;
    if has_unreceipted_proof_request_tasks(&proof_requests, &proof_result.proof_receipts) {
        let request_proof_loop = start_run_loop(
            event_log,
            run_started,
            "proof",
            "proof",
            "model-request-broker",
        )?;
        let request_proof_result = run_request_proof_broker_v0(
            root,
            out,
            diff,
            profile,
            &proof_requests,
            &proof_result.proof_receipts,
            &proof_result.resource_leases,
            args,
            box_state,
            run_started,
        )?;
        proof_result
            .proof_receipts
            .extend(request_proof_result.proof_receipts);
        proof_result
            .resource_leases
            .extend(request_proof_result.resource_leases);
        finish_run_loop(
            event_log,
            run_started,
            run_loop_tracker,
            request_proof_loop,
            "completed",
        )?;
    }
    // Order 9 (#678): the live reporter — the same-model coordinator. Runs
    // after the primary wave + proof broker, reads lane digests, makes one
    // same-model distillation call (same cohort, same cached prefix), and
    // records its conclusion as a reporter thread artifact + messages. Advisory:
    // feeds the compiler, does not post or gate. Runs AFTER the proof broker
    // so proof receipts are available for routing to lanes in the multi-turn
    // continuation (Order 9c proof-routing).
    let reporter_loop =
        start_run_loop(event_log, run_started, "model", "investigation", "reporter")?;
    let reporter_status = match run_reporter_coordination(
        root,
        &review_dir,
        &shared_context,
        &model_lanes,
        &proof_result.proof_receipts,
        &late_sensor_evidence,
        args,
        model_calls_used,
        event_log,
        &message_log,
    ) {
        Ok(calls) => {
            model_calls_used = model_calls_used.saturating_add(calls);
            "completed"
        }
        Err(e) => {
            let _ = event_log.append(
                "reporter_error",
                serde_json::json!({"error": format!("{e:#}")}),
            );
            "failed"
        }
    };
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        reporter_loop,
        reporter_status,
    )?;
    // Impact plan built earlier (before the model wave) so its candidate
    // tasks are available to the proof-planner model lane. See line ~3955.
    // Shadow-mode v2 typed proof requests (Order 2 of epic #655). Converts
    // existing v1 requests to typed intents. Emitted but not consumed for
    // execution — the broker still uses v1 command-string requests.
    let v2_shadow_requests = build_v2_shadow_requests(&proof_requests);
    if !v2_shadow_requests.is_empty() {
        let v2_path = out.join("review").join("proof_requests_v2.json");
        std::fs::write(&v2_path, serde_json::to_string_pretty(&v2_shadow_requests)?)?;
    }
    // Legacy shadow-mode claim graph (Order 3 of epic #655). It is overwritten
    // before the final compiler consumes the review surface with claims,
    // evidence, conflicts, and current-head delivery state.
    let shadow_claim_graph = build_shadow_claim_graph();
    write_claim_graph(out, &shadow_claim_graph)?;
    let proof_receipts = proof_result.proof_receipts;
    let resource_leases = proof_result.resource_leases;
    let compiler_loop = start_run_loop(
        event_log,
        run_started,
        "compiler",
        "coordination",
        "preliminary",
    )?;
    // Invariant (#314): candidates are built from the same inline-comment
    // and summary-finding values the final compile later filters through
    // `candidate_matches_inline_comment` / `candidate_matches_summary_finding`.
    // No dedupe, trim, or other normalization may run between this point and
    // that filter - the matchers compare exact fields, and a normalized
    // surface would fail the match OPEN, letting a refuted candidate post.
    // The verifier's negative self-test pins the closed loop from the other
    // side: a leaked refuted surface in final_compiler_input.json reds the
    // gate's verifier step.
    let candidates = build_candidate_records(&inline_comments, &summary_only_findings);
    write_candidate_artifacts(out, &candidates)?;
    let candidates = read_candidate_records(out)?;
    let (inline_comments, summary_only_findings) = read_candidate_review_surfaces(out)?;

    let run_pass = resolved_run_pass(args.run_pass);
    let preliminary_surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: &shared_context_id,
        review_body_policy: &config.review_body,
        run_pass,
        post_review_on: &config.gate.post_review_on,
        args,
        plan,
        diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence: &missing_or_failed_model_evidence,
        inline_comments: &inline_comments,
        summary_only_findings: &summary_only_findings,
        observations: &model_observations,
        proof_receipts: &proof_receipts,
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;
    let mut review = ReviewArtifacts {
        shared_context_id,
        review_profile: config.review_profile.clone(),
        mode: args.mode.key().to_owned(),
        posting: args.posting.key().to_owned(),
        runtime_profile: profile.name.clone(),
        run_pass: run_pass.key().to_owned(),
        model_mode: args.model_mode.key().to_owned(),
        depth: args.depth.key().to_owned(),
        provider_policy: args.provider_policy.key().to_owned(),
        model_provider_policy: args.provider_policy.key().to_owned(),
        lane_width: args.lane_width,
        model_concurrency: args.model_concurrency,
        max_model_calls: args.max_model_calls,
        max_inline_comments: args.max_inline_comments,
        model_timeout_sec: args.model_timeout_sec,
        ledger_path: effective_ledger_path(config, args),
        ledger_max_bytes: args.ledger_max_bytes,
        pr_thread_context,
        terminal_state: preliminary_surface.terminal_state,
        provider_preflights,
        model_lanes,
        missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence,
        inline_comments,
        summary_only_findings,
        observations: model_observations,
        proof_requests,
        proof_intents,
        proof_receipts,
        resource_leases,
        body: preliminary_surface.artifact_body,
    };
    let observations = combined_observations(&review);
    let observation_summary = observation_summary_artifacts(&observations);
    let orchestrator_plan = build_orchestrator_plan(
        &candidates,
        &observation_summary.unique,
        &review.proof_receipts,
        &review.resource_leases,
        &[],
    );
    write_observation_artifacts(out, &observations)?;
    write_orchestrator_artifacts(out, &orchestrator_plan, &review.proof_receipts)?;
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        compiler_loop,
        "completed",
    )?;
    let mut follow_up_results = Vec::new();
    let mut follow_up_outputs = Vec::new();
    let follow_up_model_loop = start_run_loop(
        event_log,
        run_started,
        "model",
        "investigation",
        "follow-up",
    )?;
    let follow_up_model_calls = run_follow_up_model_pass(
        FollowUpRunContext {
            root,
            out,
            review_dir: &review_dir,
            provider_preflights: &review.provider_preflights,
            shared_context: &shared_context,
            args,
            model_calls_used,
            key_present: env_value_present,
            tasks: &orchestrator_plan.follow_up_tasks,
            line_map: &line_map,
        },
        &mut follow_up_results,
        &mut follow_up_outputs,
    )?;
    model_calls_used = model_calls_used.saturating_add(follow_up_model_calls);
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        follow_up_model_loop,
        "completed",
    )?;
    write_follow_up_result_artifacts(out, &follow_up_results)?;
    write_follow_up_output_artifacts(out, &follow_up_outputs)?;
    let resolved_candidates = resolved_candidate_records(
        &candidates,
        &follow_up_results,
        &follow_up_outputs,
        &prior_resolved_candidates,
    );
    write_resolved_candidate_artifacts(out, &resolved_candidates)?;
    // The late receipt turn exists to change candidate dispositions, so the
    // final compiler must honor those changes: candidates the follow-up pass
    // resolved to `refuted` or `dropped` lose their review surface here.
    // The full audit trail stays in candidates.json, resolved_candidates.json,
    // and the follow-up artifacts. Candidates resolved to `parked-follow-up`
    // keep their surface — parked items render in the dedicated parked
    // section instead of disappearing.
    let resolved_away_candidate_ids = follow_up_resolved_away_candidate_ids(&resolved_candidates);
    let resolved_away_candidates = candidates
        .iter()
        .filter(|candidate| resolved_away_candidate_ids.contains(&candidate.id))
        .collect::<Vec<_>>();
    write_model_stage_artifacts(out, &review.model_lanes, &follow_up_results, args)?;
    write_shared_context_cache_artifacts(
        out,
        &shared_context,
        &assignments,
        &review.provider_preflights,
        &review.model_lanes,
        &follow_up_results,
        args,
    )?;
    let follow_up_evidence = follow_up_evidence_from_outputs(&follow_up_outputs);
    write_follow_up_evidence_artifact(out, &follow_up_evidence)?;
    append_follow_up_proof_requests(&mut review.proof_requests, &follow_up_evidence);
    let follow_up_proof_loop =
        start_run_loop(event_log, run_started, "proof", "proof", "follow-up-broker")?;
    let follow_up_proof_result = run_follow_up_proof_broker_v0(
        root,
        out,
        diff,
        profile,
        &follow_up_evidence.proof_requests,
        &review.proof_receipts,
        &review.resource_leases,
        args,
        box_state,
        run_started,
    )?;
    let follow_up_proof_receipts = follow_up_proof_result.proof_receipts;
    review
        .resource_leases
        .extend(follow_up_proof_result.resource_leases);
    route_follow_up_proof_receipts(&message_log, event_log, &follow_up_proof_receipts);
    let reconsideration_result = run_receipt_reconsiderations(
        root,
        &review_dir,
        &shared_context,
        &mut review.model_lanes,
        &follow_up_proof_receipts,
        args,
        model_calls_used,
        event_log,
        &message_log,
    )?;
    review.proof_receipts.extend(follow_up_proof_receipts);
    apply_receipt_reconsiderations(
        &mut review.observations,
        &reconsideration_result.reconsiderations,
    );
    write_receipt_reconsideration_artifact(out, &reconsideration_result)?;
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        follow_up_proof_loop,
        "completed",
    )?;
    let receipt_routes = receipt_route_artifacts(&review.proof_receipts, &review.resource_leases);
    write_receipt_route_artifacts(out, &receipt_routes)?;
    // Release lane step 4: lane-emitted follow-up candidates become the
    // issue-capture artifacts. v0 is artifact-only - no PR-body rendering,
    // no GitHub side effects; classification is the whole pipeline.
    let (issue_capture_candidates, issue_capture_actions) =
        classify_issue_candidates(&config.issues, std::mem::take(&mut issue_candidates));
    write_issue_capture_artifacts(out, &issue_capture_candidates, &issue_capture_actions)?;
    // Step 6: under open-high-confidence, `run` decides what the post-time
    // broker may attempt and renders the full issue text into a pure plan
    // artifact. `run` itself never opens issues.
    if config.issues.enabled && config.issues.mode == "open-high-confidence" {
        let broker_plan = build_issue_broker_plan(
            &config.issues,
            &issue_capture_candidates,
            &issue_capture_actions,
            args.github_repo.as_deref(),
            args.github_pull_number
                .or_else(detect_pull_number_from_event),
        );
        write_issue_broker_plan(out, &broker_plan)?;
    }
    let suggested_issue_ids = issue_capture_actions
        .iter()
        .filter(|action| action.action == "suggested")
        .map(|action| action.candidate_id.as_str())
        .collect::<BTreeSet<_>>();
    let suggested_issues = issue_capture_candidates
        .iter()
        .filter(|candidate| suggested_issue_ids.contains(candidate.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let final_orchestrator_plan = build_final_orchestrator_plan(
        &candidates,
        &observation_summary.unique,
        &review.proof_receipts,
        &review.resource_leases,
        tool_gate_outcomes,
    );
    write_final_orchestrator_artifact(out, &final_orchestrator_plan)?;
    let final_compiler_loop =
        start_run_loop(event_log, run_started, "compiler", "coordination", "final")?;
    let mut compiler_inline_comments = review
        .inline_comments
        .iter()
        .filter(|comment| {
            !resolved_away_candidates
                .iter()
                .any(|candidate| candidate_matches_inline_comment(candidate, comment))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut compiler_summary_only_findings = review.summary_only_findings.clone();
    compiler_summary_only_findings.retain(|finding| {
        !resolved_away_candidates
            .iter()
            .any(|candidate| candidate_matches_summary_finding(candidate, finding))
    });
    compiler_summary_only_findings.extend(follow_up_evidence.summary_only_findings.clone());
    let mut compiler_observations = review.observations.clone();
    compiler_observations.extend(follow_up_evidence.observations.clone());
    let pre_compile_claim_graph = build_active_claim_graph(
        &diff.head,
        &compiler_observations,
        &compiler_inline_comments,
        &compiler_summary_only_findings,
        &review.proof_requests,
        &review.proof_receipts,
        &review.pr_thread_context,
    );
    compiler_inline_comments =
        reconcile_inline_comments(&pre_compile_claim_graph, &compiler_inline_comments);
    compiler_summary_only_findings =
        reconcile_summary_only_findings(&pre_compile_claim_graph, &compiler_summary_only_findings);
    write_claim_graph(out, &pre_compile_claim_graph)?;
    write_final_compiler_input_artifact(
        out,
        FinalCompilerInputArtifact {
            schema: FINAL_COMPILER_INPUT_V2_SCHEMA,
            phase: "final",
            source_artifacts: &[
                "review/review.json",
                "review/follow_up_evidence.json",
                "review/resolved_candidates.json",
                PRIOR_RESOLVED_CANDIDATES_ARTIFACT,
                "review/proof_receipts.json",
                "review/tool-gate-outcomes.json",
                "review/receipt_routes.json",
                "review/receipt_reconsiderations.json",
                "review/final_orchestrator_plan.json",
                "review/claim_graph.json",
                "review/pr_thread_context.json",
                "review/proof_intents.json",
            ],
            model_lanes: &review.model_lanes,
            missing_or_failed_sensor_evidence: &review.missing_or_failed_sensor_evidence,
            missing_or_failed_model_evidence: &review.missing_or_failed_model_evidence,
            follow_up_resolved_candidate_ids: &resolved_away_candidate_ids,
            inline_comments: &compiler_inline_comments,
            summary_only_findings: &compiler_summary_only_findings,
            observations: &compiler_observations,
            proof_receipts: &review.proof_receipts,
        },
    )?;
    // Order 10 (#678): read the reporter's distillation (Order 9 #696) to pass
    // into the compiler as the review body's editorial summary. The compiler
    // renders it verbatim (firewall, not truth reducer).
    let reporter_distillation = read_reporter_distillation(&review_dir);
    let final_surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: &review.shared_context_id,
        review_body_policy: &config.review_body,
        run_pass,
        post_review_on: &config.gate.post_review_on,
        args,
        plan,
        diff,
        model_lanes: &review.model_lanes,
        missing_or_failed_sensor_evidence: &review.missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence: &review.missing_or_failed_model_evidence,
        inline_comments: &compiler_inline_comments,
        summary_only_findings: &compiler_summary_only_findings,
        observations: &compiler_observations,
        proof_receipts: &review.proof_receipts,
        suggested_issues: &suggested_issues,
        final_follow_up_tasks: final_orchestrator_plan.follow_up_tasks.len(),
        reporter_distillation: reporter_distillation.as_deref(),
    })?;
    let mut review_payload_status = final_surface.review_payload_status;
    let should_prepare_github_review = final_surface.should_prepare_github_review;
    let summary_only_policy_posted = final_surface.summary_only_policy_posted;
    let github_review = final_surface.github_review;
    let artifact_body = final_surface.artifact_body;
    // unsafe-review comment-plan candidates entered the compiler intake before
    // candidate records were built. No post-compile comment injection happens
    // here; appending here would bypass the ledger, cap, dedupe, and refuter.
    let terminal_state = final_surface.terminal_state;
    review.terminal_state = terminal_state.clone();
    review.body = artifact_body.clone();
    let mut witnesses = build_witness_records(
        &review.inline_comments,
        &review.summary_only_findings,
        &observations,
        &review.proof_receipts,
    );
    append_follow_up_evidence_witnesses(
        &mut witnesses,
        &follow_up_evidence,
        &review.proof_receipts,
    );
    write_witness_artifacts(out, &witnesses)?;
    write_proof_receipt_artifacts(out, &review.proof_receipts)?;
    write_resource_lease_artifacts(out, &review.resource_leases)?;
    review.proof_requests =
        terminalize_proof_requests(&review.proof_requests, &review.proof_receipts);
    let mut active_claim_graph = build_active_claim_graph(
        &diff.head,
        &compiler_observations,
        &compiler_inline_comments,
        &compiler_summary_only_findings,
        &review.proof_requests,
        &review.proof_receipts,
        &review.pr_thread_context,
    );
    add_resolved_candidate_topics(
        &mut active_claim_graph,
        &diff.head,
        &resolved_away_candidates,
        &review.pr_thread_context,
    );
    write_claim_graph(out, &active_claim_graph)?;
    write_proof_request_artifacts(
        out,
        diff,
        profile,
        &review.proof_requests,
        &review.proof_receipts,
        &review.resource_leases,
    )?;
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        final_compiler_loop,
        "completed",
    )?;
    // Order 11 (#678): read the reporter's verdict for review-forward gate
    // policy. Only affects the gate when config.gate.review_forward == true.
    let reporter_verdict = read_reporter_verdict(&review_dir);
    let gate_outcome = build_gate_outcome(GateOutcomeInput {
        args,
        config,
        plan,
        terminal_state: &review.terminal_state,
        proof_requests: &review.proof_requests,
        proof_receipts: &review.proof_receipts,
        tool_gate_outcomes,
        missing_or_failed_sensor_evidence: &review.missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence: &review.missing_or_failed_model_evidence,
        reporter_verdict,
    });
    if (gate_outcome.conclusion == "fail" || gate_outcome.conclusion == "inconclusive")
        && review_payload_status == "skipped_empty_smoke"
    {
        review_payload_status = "skipped_gate_failure_artifact_only";
        review.terminal_state.review_payload_status = review_payload_status.to_owned();
    }
    event_log.append(
        "terminal_state",
        serde_json::json!({
            "status": review.terminal_state.status,
            "review_payload_status": review.terminal_state.review_payload_status,
        }),
    )?;
    let run_loop_metrics = run_loop_tracker.metrics();
    let metrics = build_review_metrics(ReviewMetricsInput {
        out,
        diff,
        plan,
        review: &review,
        github_review: if should_prepare_github_review {
            Some(&github_review)
        } else {
            None
        },
        review_payload_status,
        observations_count: observations.len(),
        follow_up_results: &follow_up_results,
        final_follow_up_tasks: final_orchestrator_plan.follow_up_tasks.len(),
        run: run_loop_metrics,
        elapsed,
        args,
    });

    fs::write(
        review_dir.join("review.json"),
        serde_json::to_vec_pretty(&review)?,
    )?;
    fs::write(
        review_dir.join("metrics.json"),
        serde_json::to_vec_pretty(&metrics)?,
    )?;
    let cost_receipt =
        write_cost_receipt_artifact(root, out, config, &metrics, &review, &follow_up_results)?;
    write_floor_trend_artifact(out, &cost_receipt)?;
    let fill_ledger = write_fill_ledger_artifact(FillLedgerInput {
        out,
        diff,
        profile,
        plan,
        tool_gate_outcomes,
        gate_outcome: &gate_outcome,
        review: &review,
        metrics: &metrics,
    })?;
    let quality_receipt = write_quality_receipt_artifact(out, &metrics, &review, &fill_ledger)?;
    write_quality_trend_artifact(out, &quality_receipt)?;
    write_scheduler_artifact(&review_dir, &metrics.run)?;
    fs::write(
        review_dir.join("terminal_state.json"),
        serde_json::to_vec_pretty(&review.terminal_state)?,
    )?;
    fs::write(
        review_dir.join("gate_outcome.json"),
        serde_json::to_vec_pretty(&gate_outcome)?,
    )?;
    event_log.append(
        "gate_outcome",
        serde_json::json!({
            "conclusion": gate_outcome.conclusion,
            "terminal_status": gate_outcome.terminal_status,
            "reasons": gate_outcome.reasons.len(),
            "fail_on_gate": args.fail_on_gate.key(),
            "fail_on_gate_resolved": args.fail_on_gate.resolved(args.mode),
        }),
    )?;
    fs::write(
        review_dir.join("provider-preflight-status.json"),
        serde_json::to_vec_pretty(&review.provider_preflights)?,
    )?;
    fs::write(review_dir.join("review.md"), artifact_body)?;
    // Calibration artifact (calibration of #678): a run-level measurable
    // signal computed from existing receipts/messages. Makes the product
    // tunable: proof_changed_conclusion_count and useful comment counts.
    let cal_messages = read_messages_ndjson(&review_dir);
    let _ = write_calibration_artifact(
        &review_dir,
        &review.model_lanes,
        &review.proof_requests,
        &review.proof_receipts,
        &cal_messages,
        github_review.comments.len(),
        if github_review.body.is_empty() { 0 } else { 1 },
        &gate_outcome.conclusion,
        args.mode.key(),
        &diff.base,
        &diff.head,
    );
    if should_prepare_github_review {
        write_github_review_payload(
            &review_dir,
            &github_review,
            &line_map,
            &config.review_body,
            summary_only_policy_posted,
        )?;
    } else {
        write_github_review_skip_receipt(
            &review_dir,
            build_github_review_skip_receipt(args, &review, config.review_body.summary_only_body),
        )?;
    }
    event_log.append("run_finished", serde_json::json!({"run_dir": out}))?;
    event_log.sync()?;
    Ok(gate_outcome)
}

fn build_review_terminal_state(input: TerminalStateInput<'_>) -> ReviewTerminalState {
    let substantive_summary_only_findings =
        count_substantive_summary_only_findings(input.summary_only_findings);
    let usable_model_lanes = input
        .model_lanes
        .iter()
        .filter(|receipt| model_lane_is_usable_for_terminal_state(receipt))
        .count();
    let evidence_gaps = input.missing_or_failed_sensor_evidence.len()
        + input.missing_or_failed_model_evidence.len();
    let reviewer_content_present = input.should_prepare_github_review
        || has_reviewer_value(input.inline_comments, input.pr_body);
    let reviewer_value_present = reviewer_content_present
        || input
            .proof_receipts
            .iter()
            .any(proof_receipt_is_test_proof_result);

    let (status, reason) = if usable_model_lanes == 0
        && !input.args.dry_run
        && !matches!(input.args.model_mode, ModelMode::Off)
        && input.plan.diff_class != DiffClass::ArtifactOnlySmoke
        && !reviewer_content_present
    {
        (
            "failed-to-review",
            "No usable model lane or proof receipt was available, so the run did not reach a sufficient review state.".to_owned(),
        )
    } else if reviewer_value_present {
        let reason = if input.should_prepare_github_review {
            "Reviewer-value content survived compilation; a grouped PR review was prepared."
                .to_owned()
        } else if input.review_payload_status == "skipped_pass_policy" {
            format!(
                "Reviewer-value content survived compilation, but pass `{}` is not in [gate].post_review_on; diagnostics remain in artifacts.",
                input.run_pass.key()
            )
        } else {
            format!(
                "Reviewer-value content survived compilation, but summary_only_body = `{}` withheld the PR-facing payload as no-value boilerplate: {} summary-only findings, {} substantive; diagnostics remain in artifacts.",
                input.summary_only_body.key(),
                input.summary_only_findings.len(),
                substantive_summary_only_findings
            )
        };
        ("needs-reviewer-attention", reason)
    } else if input.args.dry_run {
        (
            "artifact-only",
            "Dry run requested; this run produced artifacts but no reviewer-facing review."
                .to_owned(),
        )
    } else if matches!(input.args.mode, RunMode::IntelligentCi)
        && has_required_sensor_evidence_gap(input.plan, input.missing_or_failed_sensor_evidence)
    {
        (
            "failed-to-review",
            "A required intelligent-ci sensor was missing, skipped, failed, or timed out, so the gate did not reach a sufficient review state.".to_owned(),
        )
    } else if matches!(input.args.model_mode, ModelMode::Off) {
        (
            "artifact-only",
            "Model mode was off; this run produced artifacts but no reviewer-facing review."
                .to_owned(),
        )
    } else if input.plan.diff_class == DiffClass::ArtifactOnlySmoke {
        (
            "artifact-only",
            "Artifact-only smoke diff; diagnostics remain in artifacts and no PR review was prepared.".to_owned(),
        )
    } else {
        (
            "sufficient",
            "No reviewer-value content survived compilation; the run reached a sufficient terminal state and stayed artifact-only.".to_owned(),
        )
    };

    ReviewTerminalState {
        schema: TERMINAL_STATE_SCHEMA.to_owned(),
        status: status.to_owned(),
        reason,
        review_payload_status: input.review_payload_status.to_owned(),
        reviewer_value_present,
        diff_class: input.plan.diff_class.key().to_owned(),
        model_mode: input.args.model_mode.key().to_owned(),
        usable_model_lanes,
        model_lanes: input.model_lanes.len(),
        evidence_gaps,
        proof_receipts: input.proof_receipts.len(),
        final_follow_up_tasks: input.final_follow_up_tasks,
        inline_comments: input.inline_comments.len(),
        summary_only_findings: input.summary_only_findings.len(),
        substantive_summary_only_findings,
    }
}

/// Shared predicate for gate blocking and terminal-state routing: a sensor
/// evidence issue is blocking material only when the plan marks that sensor as
/// required. Keeping one helper prevents the two call sites from drifting.
fn sensor_issue_is_required(plan: &Plan, issue: &SensorEvidenceIssue) -> bool {
    plan.sensors
        .iter()
        .any(|sensor| sensor.id == issue.sensor && sensor.required)
}

fn has_required_sensor_evidence_gap(plan: &Plan, issues: &[SensorEvidenceIssue]) -> bool {
    issues
        .iter()
        .any(|issue| sensor_issue_is_required(plan, issue))
}

fn model_lane_is_usable_for_terminal_state(receipt: &ModelLaneReceipt) -> bool {
    matches!(receipt.status.as_str(), "ok" | "degraded")
}

fn write_github_review_payload(
    review_dir: &Path,
    github_review: &GitHubReview,
    right_lines: &BTreeSet<(String, u32)>,
    review_body_policy: &ReviewBodyPolicy,
    waive_suppressible_body_policy: bool,
) -> Result<()> {
    validate_github_review_payload_for_right_lines(
        github_review,
        right_lines,
        "generated diff context",
        review_body_policy,
        waive_suppressible_body_policy,
    )?;
    fs::write(
        review_dir.join("github-review.json"),
        serde_json::to_vec_pretty(github_review)?,
    )?;
    Ok(())
}

struct ReviewMetricsInput<'a> {
    out: &'a Path,
    diff: &'a DiffContext,
    plan: &'a Plan,
    review: &'a ReviewArtifacts,
    github_review: Option<&'a GitHubReview>,
    review_payload_status: &'a str,
    observations_count: usize,
    follow_up_results: &'a [FollowUpResult],
    final_follow_up_tasks: usize,
    run: RunLoopMetrics,
    elapsed: Duration,
    args: &'a RunArgs,
}

fn build_review_metrics(input: ReviewMetricsInput<'_>) -> ReviewMetrics {
    let ReviewMetricsInput {
        out,
        diff,
        plan,
        review,
        github_review,
        review_payload_status,
        observations_count,
        follow_up_results,
        final_follow_up_tasks,
        mut run,
        elapsed,
        args,
    } = input;
    let sensor_statuses = plan
        .sensors
        .iter()
        .map(|sensor| sensor_status_for_metrics(out, sensor))
        .collect::<Vec<_>>();
    let preflight_statuses = review
        .provider_preflights
        .iter()
        .map(|receipt| receipt.status.as_str())
        .collect::<Vec<_>>();
    let model_lane_statuses = review
        .model_lanes
        .iter()
        .map(|receipt| receipt.status.as_str())
        .collect::<Vec<_>>();
    let follow_up_result_statuses = follow_up_results
        .iter()
        .map(|result| result.status.as_str())
        .collect::<Vec<_>>();
    run.model_call_duration_ms_sum = model_call_duration_ms_sum(review, follow_up_results);
    run.proof_command_duration_ms_sum = proof_command_duration_ms_sum(&review.proof_receipts);
    let prompt_cache = model_prompt_cache_metrics(review, follow_up_results, args);
    let proof_request_status_counts = status_counts(
        review
            .proof_requests
            .iter()
            .map(|request| request.status.as_str()),
    );
    let proof_requests_terminal = review
        .proof_requests
        .iter()
        .filter(|request| request.status != "requested")
        .count();
    let proof_request_terminal_rate = (!review.proof_requests.is_empty())
        .then(|| proof_requests_terminal as f64 / review.proof_requests.len() as f64);
    let proof_receipts_current_head = review
        .proof_receipts
        .iter()
        .filter(|receipt| receipt.head.eq_ignore_ascii_case(&diff.head))
        .count();
    let proof_receipts_with_request_links = review
        .proof_receipts
        .iter()
        .filter(|receipt| !receipt.request_ids.is_empty())
        .count();
    let (proof_changed_conclusions, _) = crate::calibration::detect_proof_changed_conclusions(
        &out.join("review"),
        &review.proof_receipts,
    );

    ReviewMetrics {
        schema_version: 1,
        wall_clock_ms: elapsed.as_millis(),
        wall_clock_seconds: elapsed.as_secs(),
        run,
        shared_context_id: review.shared_context_id.clone(),
        base: diff.base.clone(),
        head: diff.head.clone(),
        review_profile: review.review_profile.clone(),
        profile_name: plan.profile_name.clone(),
        runtime_profile: review.runtime_profile.clone(),
        mode: review.mode.clone(),
        posting: review.posting.clone(),
        run_pass: review.run_pass.clone(),
        model_mode: review.model_mode.clone(),
        depth: review.depth.clone(),
        provider_policy: review.provider_policy.clone(),
        lane_width: review.lane_width,
        model_concurrency: review.model_concurrency,
        max_model_calls: review.max_model_calls,
        max_inline_comments: review.max_inline_comments,
        changed_files: diff.changed_files.len(),
        diff_flags: diff.flags.clone(),
        lane_packets: lane_packet_count(out),
        sensors: SensorMetrics {
            total: plan.sensors.len(),
            planned: plan.sensors.iter().filter(|sensor| sensor.run).count(),
            skipped_by_plan: plan.sensors.iter().filter(|sensor| !sensor.run).count(),
            status_counts: status_counts(sensor_statuses.iter().map(String::as_str)),
        },
        models: ModelMetrics {
            provider_preflights: review.provider_preflights.len(),
            provider_preflight_status_counts: status_counts(preflight_statuses.iter().copied()),
            provider_preflight_calls_attempted: review
                .provider_preflights
                .iter()
                .filter(|receipt| model_call_attempted_status(&receipt.status))
                .count(),
            model_lanes: review.model_lanes.len(),
            model_lane_status_counts: status_counts(model_lane_statuses.iter().copied()),
            model_lane_calls_attempted: review
                .model_lanes
                .iter()
                .filter(|receipt| model_call_attempted_status(&receipt.status))
                .count(),
            model_fallbacks_used: review
                .model_lanes
                .iter()
                .filter(|receipt| receipt.fallback_from.is_some())
                .count(),
            prompt_cache_creation_input_tokens: prompt_cache.creation_input_tokens,
            prompt_cache_read_input_tokens: prompt_cache.read_input_tokens,
            prompt_cache_lane_hits: prompt_cache.lane_hits,
            prompt_cache_lane_misses: prompt_cache.lane_misses,
            prompt_cache_lane_unknown: prompt_cache.lane_unknown,
        },
        inline_comments: review.inline_comments.len(),
        github_review_comments: github_review.map_or(0, |review| review.comments.len()),
        prepared_inline_comments: github_review.map_or(0, |review| review.comments.len()),
        prepared_review_body: github_review.is_some_and(|review| !review.body.trim().is_empty()),
        summary_only_findings: review.summary_only_findings.len(),
        observations: observations_count,
        follow_up_results: FollowUpResultMetrics {
            total: follow_up_results.len(),
            status_counts: status_counts(follow_up_result_statuses.iter().copied()),
            calls_attempted: follow_up_results
                .iter()
                .filter(|result| model_call_attempted_status(&result.status))
                .count(),
        },
        final_follow_up_tasks,
        proof_requests: review.proof_requests.len(),
        proof_request_status_counts,
        proof_requests_terminal,
        proof_request_terminal_rate,
        proof_receipts: review.proof_receipts.len(),
        proof_receipts_current_head,
        proof_receipts_stale_head: review
            .proof_receipts
            .len()
            .saturating_sub(proof_receipts_current_head),
        proof_receipts_with_request_links,
        proof_changed_conclusions,
        resource_leases: review.resource_leases.len(),
        off_diff_candidates_rejected: review
            .summary_only_findings
            .iter()
            .filter(|finding| finding.reason.contains("line_valid=false"))
            .count(),
        missing_or_failed_sensor_evidence: review.missing_or_failed_sensor_evidence.len(),
        missing_or_failed_model_evidence: review.missing_or_failed_model_evidence.len(),
        provider_evidence_failures: review
            .provider_preflights
            .iter()
            .filter(|receipt| is_model_evidence_issue(&receipt.status))
            .count(),
        terminal_state: review.terminal_state.status.clone(),
        review_payload_status: review_payload_status.to_owned(),
        post_status: "not_attempted_by_run".to_owned(),
        review_body_bytes: review.body.len(),
        artifact_review_body_bytes: review.body.len(),
        github_review_body_bytes: github_review.map_or(0, |review| review.body.len()),
        review_body_truncated: review.body.contains(REVIEW_BODY_TRUNCATED_SUFFIX.trim()),
        github_review_body_truncated: github_review
            .is_some_and(|review| review.body.contains(REVIEW_BODY_TRUNCATED_SUFFIX.trim())),
    }
}

fn write_fill_ledger_artifact(input: FillLedgerInput<'_>) -> Result<FillLedger> {
    let out = input.out;
    let ledger = build_fill_ledger(input)?;
    fs::write(
        out.join("review").join("fill-ledger.json"),
        serde_json::to_vec_pretty(&ledger)?,
    )?;
    Ok(ledger)
}

fn build_fill_ledger(input: FillLedgerInput<'_>) -> Result<FillLedger> {
    let proof_output =
        build_proof_planner_output(input.diff, input.profile, &input.review.proof_requests)?;
    let mut entries = Vec::new();
    entries.extend(
        input
            .plan
            .sensors
            .iter()
            .filter(|sensor| !sensor.required)
            .map(|sensor| {
                fill_sensor_entry(
                    input.out,
                    sensor,
                    input.tool_gate_outcomes,
                    input.gate_outcome,
                )
            }),
    );
    entries.extend(
        input
            .review
            .proof_requests
            .iter()
            .filter(|request| !proof_request_is_gate_required(request))
            .map(|request| {
                fill_proof_request_entry(
                    request,
                    &proof_output.proof_tasks,
                    &input.review.proof_receipts,
                    &input.review.resource_leases,
                    input.gate_outcome,
                )
            }),
    );
    entries.extend(
        proof_output
            .skip
            .into_iter()
            .map(fill_proof_planner_skip_entry),
    );

    Ok(FillLedger {
        schema: FILL_LEDGER_SCHEMA,
        run_id: cost_run_id(input.metrics),
        catalog_scope: "executed_work_queue_v1",
        source_artifacts: vec![
            "work_queue.json".to_owned(),
            "review/proof_requests.json".to_owned(),
            "review/proof_planner_output.json".to_owned(),
            "review/proof_receipts.json".to_owned(),
            "review/resource_leases.json".to_owned(),
            "review/tool-gate-outcomes.json".to_owned(),
            "review/gate_outcome.json".to_owned(),
            "review/metrics.json".to_owned(),
        ],
        entries,
    })
}

const DEFAULT_GITHUB_QUALITY_AUTHOR_LOGINS: &[&str] = &["github-actions", "github-actions[bot]"];
const GITHUB_QUALITY_REVIEW_THREADS_QUERY: &str = r#"query UbReviewQualityReviewThreads($owner: String!, $name: String!, $number: Int!) {
  repository(owner: $owner, name: $name) {
    pullRequest(number: $number) {
      number
      mergedAt
      files(first: 100) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          path
          additions
          deletions
          changeType
        }
      }
      reviewThreads(first: 100) {
        pageInfo {
          hasNextPage
          endCursor
        }
        nodes {
          id
          isResolved
          comments(first: 100) {
            pageInfo {
              hasNextPage
              endCursor
            }
            nodes {
              id
              body
              createdAt
              url
              author {
                login
              }
            }
          }
        }
      }
    }
  }
}"#;

fn cmd_quality_backfill(args: QualityBackfillArgs) -> Result<()> {
    let review_dir = args.out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let runs = load_quality_backfill_runs(&args)?;
    let outcomes = load_github_quality_outcomes(&args)?;
    let previous = load_previous_quality_backfill(&args)?;
    let artifact = build_quality_backfill_artifact(
        args.window_days,
        &runs,
        outcomes.as_ref(),
        previous.as_ref(),
    );
    fs::write(
        review_dir.join("quality-backfill.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    println!(
        "quality-backfill: wrote {}/quality-backfill.json ({} runs)",
        review_dir.display(),
        artifact.window_runs
    );
    Ok(())
}

fn load_quality_backfill_runs(args: &QualityBackfillArgs) -> Result<Vec<QualityBackfillRun>> {
    let mut runs = Vec::new();
    let mut seen = BTreeSet::new();
    for run_dir in &args.run_dirs {
        let receipt_path = run_dir.join("review").join("quality-receipt.json");
        let trend_path = run_dir.join("review").join("quality-trend.json");
        let receipt: QualityReceiptSeed = read_json_file(&receipt_path)?;
        if receipt.schema != QUALITY_RECEIPT_SCHEMA {
            bail!(
                "{} schema invalid: expected {}, got {}",
                receipt_path.display(),
                QUALITY_RECEIPT_SCHEMA,
                receipt.schema
            );
        }
        if !seen.insert(receipt.run_id.clone()) {
            bail!(
                "duplicate quality run_id `{}` in {}",
                receipt.run_id,
                run_dir.display()
            );
        }
        let label = sanitize_artifact_name(&format!("run-{}", receipt.run_id));
        let receipt_source =
            copy_quality_backfill_source(&args.out, &receipt_path, &format!("{label}-receipt"))?;
        let trend_source = if trend_path.exists() {
            let trend: QualityTrendSeed = read_json_file(&trend_path)?;
            if trend.schema != QUALITY_TREND_SCHEMA {
                bail!(
                    "{} schema invalid: expected {}, got {}",
                    trend_path.display(),
                    QUALITY_TREND_SCHEMA,
                    trend.schema
                );
            }
            Some(copy_quality_backfill_source(
                &args.out,
                &trend_path,
                &format!("{label}-trend"),
            )?)
        } else {
            None
        };
        runs.push(QualityBackfillRun {
            receipt,
            receipt_source,
            trend_source,
        });
    }
    Ok(runs)
}

fn load_github_quality_outcomes(
    args: &QualityBackfillArgs,
) -> Result<Option<LoadedGithubQualityOutcomes>> {
    let Some(path) = &args.github_outcomes else {
        return Ok(None);
    };
    let value: serde_json::Value = read_json_file(path)?;
    let has_comments = value.get("comments").is_some();
    let has_adopted_generated_tests = value.get("adopted_generated_tests").is_some();
    let outcomes: GithubQualityOutcomes =
        serde_json::from_value(value).with_context(|| format!("parse {}", path.display()))?;
    let schema = outcomes.schema.as_deref().unwrap_or("");
    if schema != GITHUB_QUALITY_OUTCOMES_SCHEMA {
        bail!(
            "{} schema invalid: expected {}, got {}",
            path.display(),
            GITHUB_QUALITY_OUTCOMES_SCHEMA,
            schema
        );
    }
    let source_artifact = copy_quality_backfill_source(&args.out, path, "github-quality-outcomes")?;
    let base = path.parent().unwrap_or_else(|| Path::new("."));
    let mut raw_source_artifacts = Vec::new();
    for source in &outcomes.source_artifacts {
        let source_path = if Path::new(source).is_absolute() {
            PathBuf::from(source)
        } else {
            base.join(source)
        };
        raw_source_artifacts.push(copy_quality_backfill_source(
            &args.out,
            &source_path,
            source,
        )?);
    }
    Ok(Some(LoadedGithubQualityOutcomes {
        outcomes,
        has_comments,
        has_adopted_generated_tests,
        source_artifact,
        raw_source_artifacts,
    }))
}

fn load_previous_quality_backfill(
    args: &QualityBackfillArgs,
) -> Result<Option<LoadedPreviousQualityBackfill>> {
    let Some(path) = &args.previous else {
        return Ok(None);
    };
    let artifact: PreviousQualityBackfill = read_json_file(path)?;
    if artifact.schema != QUALITY_BACKFILL_SCHEMA {
        bail!(
            "{} schema invalid: expected {}, got {}",
            path.display(),
            QUALITY_BACKFILL_SCHEMA,
            artifact.schema
        );
    }
    let source_artifact =
        copy_quality_backfill_source(&args.out, path, "previous-quality-backfill")?;
    Ok(Some(LoadedPreviousQualityBackfill {
        artifact,
        source_artifact,
    }))
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::io::{BufRead, BufReader, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::process::{Command as ProcessCommand, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};

    use anyhow::{Context as _, Result, bail};

    use super::diff_posture::{NO_LGTM_POSTURE, default_lanes_for_diff_context};
    use super::quality_artifact::{build_quality_receipt, build_quality_trend_artifact};
    use super::test_parse::command_display;
    use super::{
        BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY, BoxState, CandidateRecord, CommandStatus,
        Config, DEFAULT_REVIEW_PROFILE, DiffClass, DiffContext, DiffFlags, EventLog, FailOnGate,
        FollowUpOutputRecord, FollowUpQuestionTask, GateCheckArgs, GitHubReview,
        GitHubReviewComment, IssueBrokerPlanEntry, IssueCandidate, IssueCandidateEvidence,
        LaneModelOutput, LanePlan, LanguageMix, Limits, MinimaxPromptCache, ModelAssignment,
        ModelCacheUsage, ModelCallOutcome, ModelEvidenceIssue, ModelLaneReceipt,
        ModelLaneTaskResult, ModelMode, ModelOutputSinks, ModelProvider, ModelProviderPolicy,
        ModelRunContext, Observation, ObservationInput, OpenCodeEndpointKindArg, Plan, PostArgs,
        PostingMode, PrDecisionContext, PrThreadContext, Profile, ProfileArg, ProofBudget,
        ProofCommandReceipt, ProofLeaseBudget, ProofReceipt, ProofRequest, ProofRequestGroup,
        ProviderConcurrencyLimits, ProviderKindArg, RefuterDecision, RefuterOutput,
        RefuterRunContext, ResolvedCandidateRecord, ResourceLease, ReviewArgs, ReviewBodyAudience,
        ReviewBodyExecutionSummaryPolicy, ReviewBodyPolicy, ReviewCompilerInput, ReviewDepth,
        ReviewInlineComment, ReviewMetricsInput, ReviewTerminalState, RunArgs, RunCompletion,
        RunMode, STANDARD_LANE_WIDTH, STANDARD_MAX_MODEL_CALLS, STANDARD_MODEL_CONCURRENCY,
        SelectorArgs, SensorEvidenceIssue, SensorPlan, SensorStatusWrite, SummaryOnlyBodyPolicy,
        SummaryOnlyFinding, TOOL_GATE_OUTCOME_SCHEMA, TerminalStateInput, ToolClass,
        ToolGateOutcomeEntry, ToolGateOutcomeMetrics, ToolGatePolicy,
        append_follow_up_evidence_witnesses, append_follow_up_proof_requests, apply_model_output,
        apply_refuter_output, apply_runtime_profile_limits, build_candidate_records,
        build_cost_receipt, build_final_orchestrator_plan, build_issue_broker_plan,
        build_orchestrator_plan, build_review_metrics, build_review_terminal_state,
        build_witness_records, builtin_profiles, candidate_matches_inline_comment,
        candidate_matches_summary_finding, cap_review_body, cap_review_body_bullets, classify_diff,
        classify_diff_class, classify_issue_candidates, classify_proof_cost, cmd_gate_check,
        cmd_post, collect_pr_thread_context, combined_observations, compile_review_surface,
        dedupe_inline_comments, deep_minimax_lanes, default_lanes, direct_minimax_spec,
        execute_issue_broker, extract_model_content, fallback_provider_spec_for_lane,
        focused_test_tasks_from_diff, follow_up_evidence_from_outputs, follow_up_model_lane_id,
        follow_up_output_record, follow_up_provider_assignment_with_key_state,
        follow_up_resolved_away_candidate_ids, github_review_skip_path, http_status_from_error,
        is_model_receipt_evidence_issue, make_observation, model_api_url, model_assignments,
        model_assignments_with_key_state, model_auth_header, model_json_payload, model_lane,
        model_request_payload, model_response_shape, normalize_run_args,
        observation_summary_artifacts, opencode_canary_spec, pr_decision_sentence,
        proof_planner_assignment_with_key_state, provider_concurrency_limits,
        provider_spec_for_lane_with_key_state, read_candidate_review_surfaces,
        read_github_event_pr_context, render_lane_model_prompt, render_ledger_context,
        render_pr_thread_context, render_refuter_prompt, render_review_body, render_summary,
        resolved_candidate_records, resolved_minimax_prompt_cache, resolved_provider_policy,
        review_lanes_for_args, right_side_diff_lines, run_available_model_lanes,
        run_available_model_lanes_with_runner, run_gate_failure_message, run_refuter_pass,
        runtime_fallback_retry_spec, runtime_profile_from_toml, runtime_profile_override,
        selected_provider_spec, sha256_hex, split_curl_http_status, standard_minimax_lanes,
        terminalize_proof_requests, validate_github_review_payload,
        validate_github_review_payload_for_post, validate_pr_review_body_policy, validate_run_args,
        wait_for_child_output_files, write_candidate_artifacts, write_final_orchestrator_artifact,
        write_follow_up_evidence_artifact, write_follow_up_output_artifacts,
        write_github_review_payload, write_issue_broker_results, write_issue_capture_artifacts,
        write_observation_artifacts, write_orchestrator_artifacts, write_proof_receipt_artifacts,
        write_proof_request_artifacts, write_resolved_candidate_artifacts,
        write_resource_lease_artifacts, write_review_artifacts, write_sensor_status,
        write_witness_artifacts,
    };
    use crate::{
        ModelCandidateComment, ModelCandidateFinding, ModelCandidateObservation,
        ModelFailedObjection, collect_sensor_evidence_issues, validate_failed_objection,
        validate_inline_candidate, validate_model_observation, validate_summary_only_candidate,
    };

    #[test]
    fn doctor_tool_install_hints_name_exact_core_tool_fixes() {
        assert_eq!(
            super::doctor_tool_install_hint("tokmd"),
            "cargo install tokmd --locked --version 1.12.0 --force"
        );
        assert_eq!(
            super::doctor_tool_install_hint("cargo-allow"),
            "cargo install cargo-allow --locked --version 0.1.8 --force"
        );
        assert_eq!(
            super::doctor_tool_install_hint("ripr"),
            "cargo install ripr --locked --version 0.10.0 --force"
        );
        assert_eq!(
            super::doctor_tool_install_hint("unsafe-review"),
            "cargo install unsafe-review --locked --version 0.3.4 --force"
        );
        assert_eq!(
            super::doctor_tool_install_hint("actionlint"),
            "go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.12; add $(go env GOPATH)/bin to PATH"
        );
    }

    #[test]
    fn doctor_version_fix_reinstalls_pinned_standard_image_tools() {
        assert_eq!(
            super::doctor_tool_version_fix("tokmd", "1.12.0"),
            "cargo install tokmd --locked --version 1.12.0 --force"
        );
        assert_eq!(
            super::doctor_tool_version_fix("cargo-allow", "0.1.8"),
            "cargo install cargo-allow --locked --version 0.1.8 --force"
        );
        assert_eq!(
            super::doctor_tool_version_fix("unsafe-review", "0.3.4"),
            "cargo install unsafe-review --locked --version 0.3.4 --force"
        );
        assert_eq!(
            super::doctor_tool_version_fix("actionlint", "1.7.12"),
            "go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.12; add $(go env GOPATH)/bin to PATH"
        );
        assert!(super::command_version_matches("ripr 0.10.0", "0.10.0"));
        assert!(super::command_version_matches(
            "actionlint version v1.7.12",
            "1.7.12"
        ));
        assert!(!super::command_version_matches("ripr 0.9.9", "0.10.0"));
    }

    #[test]
    fn doctor_binary_install_status_reports_path_state_and_fix() {
        let current = PathBuf::from("/opt/ub-review/bin/ub-review");
        let on_path = super::system_detect::doctor_binary_install_status_from_paths(
            Some(&current),
            Some(&current),
        );
        assert_eq!(on_path, "on PATH as /opt/ub-review/bin/ub-review");

        let missing_path =
            super::system_detect::doctor_binary_install_status_from_paths(Some(&current), None);
        assert!(missing_path.contains("not on PATH"));
        assert!(missing_path.contains("add /opt/ub-review/bin to PATH"));
        assert!(missing_path.contains("install-mode=path"));
        assert!(missing_path.contains("binary-path=/opt/ub-review/bin/ub-review"));

        let shadowed = super::system_detect::doctor_binary_install_status_from_paths(
            Some(&current),
            Some(Path::new("/usr/local/bin/ub-review")),
        );
        assert!(shadowed.contains("running /opt/ub-review/bin/ub-review"));
        assert!(shadowed.contains("PATH resolves ub-review to /usr/local/bin/ub-review"));
    }

    #[test]
    fn docs_only_diff_is_detected() {
        let flags = classify_diff(&["docs/readme.md".to_owned()], "");
        assert!(flags.docs_only);
        assert!(!flags.source_changed);
        assert_eq!(
            classify_diff_class(&["docs/readme.md".to_owned()], &flags),
            DiffClass::DocsOnly
        );
    }

    #[test]
    fn generic_typescript_diff_stays_source_general_despite_native_words() {
        let files = vec!["packages/bun-plugin/src/options.ts".to_owned()];
        let flags = classify_diff(
            &files,
            "+ const message = 'unsafe fallback should not route to UB lanes';",
        );

        assert!(flags.source_changed);
        assert!(!flags.rust_changed);
        assert!(!flags.cpp_changed);
        assert!(!flags.unsafe_or_native_risk);
        assert_eq!(
            classify_diff_class(&files, &flags),
            DiffClass::SourceGeneral
        );

        let mut plan = test_plan(Vec::new());
        plan.diff_class = DiffClass::SourceGeneral;
        let lanes = review_lanes_for_args(&plan, &test_run_args(PathBuf::from("out")));
        assert!(lanes.iter().all(|lane| !lane.id.starts_with("ub-")));
        assert!(!lanes.iter().any(|lane| lane.id == "ub"));
    }

    #[test]
    fn mixed_language_diff_records_language_mix_without_ub_routing() {
        let files = vec![
            "cmd/server/main.go".to_owned(),
            "scripts/score.py".to_owned(),
            "web/routes.ts".to_owned(),
            "tests/routes.test.ts".to_owned(),
        ];
        let flags = classify_diff(&files, "+ handler();\n");
        let language_mix = super::classify_language_mix(&files);

        assert!(flags.source_changed);
        assert!(!flags.unsafe_or_native_risk);
        assert_eq!(
            classify_diff_class(&files, &flags),
            DiffClass::SourceGeneral
        );
        assert_eq!(
            language_mix.languages,
            vec![
                "go".to_owned(),
                "python".to_owned(),
                "typescript".to_owned()
            ]
        );
        assert_eq!(language_mix.primary_language.as_deref(), Some("typescript"));
        assert!(language_mix.mixed_language);
        assert!(language_mix.surfaces.contains(&"source".to_owned()));
        assert!(language_mix.surfaces.contains(&"scripts".to_owned()));
        assert!(language_mix.surfaces.contains(&"tests".to_owned()));
    }

    #[test]
    fn native_surface_typescript_path_can_still_route_source_ub() {
        let files = vec!["src/bun.js/bindings/arraybuffer.ts".to_owned()];
        let flags = classify_diff(&files, "+ const length = view.byteLength;");

        assert!(flags.source_changed);
        assert!(flags.unsafe_or_native_risk);
        assert_eq!(classify_diff_class(&files, &flags), DiffClass::SourceUb);
    }

    #[test]
    fn workflow_only_diff_routes_to_workflow_lanes() {
        let files = vec![".github/workflows/review.yml".to_owned()];
        let flags = classify_diff(&files, "+permissions:\n+  contents: read\n");
        assert!(flags.workflow_changed);
        assert_eq!(
            classify_diff_class(&files, &flags),
            DiffClass::WorkflowTooling
        );

        let mut plan = test_plan(Vec::new());
        plan.diff_class = DiffClass::WorkflowTooling;
        plan.changed_files = files.clone();
        plan.language_mix = super::classify_language_mix(&files);
        plan.lanes = default_lanes_for_diff_context(DiffClass::WorkflowTooling, &plan.language_mix);
        let mut args = test_run_args(std::path::PathBuf::from("out"));
        args.lane_width = 10;
        let lanes = review_lanes_for_args(&plan, &args);

        assert!(!lanes.is_empty());
        assert!(lanes.iter().all(|lane| lane.id.starts_with("workflow-")));
        assert!(lanes.iter().any(|lane| {
            lane.focus.contains("pull_request_target") && lane.focus.contains("checkout")
        }));
        assert!(!lanes.iter().any(|lane| {
            lane.focus.contains("ArrayBuffer")
                || lane.focus.contains("worker handoff")
                || lane.role.contains("undefined-behavior")
        }));
    }

    #[test]
    fn scripts_only_tooling_diff_routes_to_tooling_lanes_not_workflow_lanes() {
        let files = vec!["scripts/verify-bun-review-artifacts.py".to_owned()];
        let flags = classify_diff(&files, "+import tempfile\n");
        assert!(flags.source_changed);
        assert!(flags.shell_changed);
        assert_eq!(
            classify_diff_class(&files, &flags),
            DiffClass::WorkflowTooling
        );

        let mut plan = test_plan(Vec::new());
        plan.diff_class = DiffClass::WorkflowTooling;
        plan.changed_files = files.clone();
        plan.language_mix = super::classify_language_mix(&files);
        plan.lanes = default_lanes_for_diff_context(DiffClass::WorkflowTooling, &plan.language_mix);
        let mut args = test_run_args(std::path::PathBuf::from("out"));
        args.lane_width = 10;
        let lanes = review_lanes_for_args(&plan, &args);
        let lane_ids = lanes
            .iter()
            .map(|lane| lane.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            lane_ids,
            vec![
                "tooling-script-proof",
                "tooling-policy",
                "tooling-opposition"
            ]
        );
        assert!(lanes.iter().all(|lane| {
            !lane.id.starts_with("workflow-")
                && !lane.focus.contains("pull_request_target")
                && !lane.focus.contains("action pinning")
        }));
        assert!(lanes.iter().any(|lane| {
            lane.focus.contains("scripts") && lane.focus.contains("focused self-test proof")
        }));
    }

    #[test]
    fn tests_oracle_prompt_batches_oracle_critique_and_convergence() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let spec = direct_minimax_spec(&args);
        for lane in [
            standard_minimax_lanes()
                .into_iter()
                .find(|lane| lane.id == "tests-oracle")
                .ok_or_else(|| anyhow::anyhow!("tests-oracle lane missing"))?,
            deep_minimax_lanes()
                .into_iter()
                .find(|lane| lane.id == "tests-oracle-strength")
                .ok_or_else(|| anyhow::anyhow!("tests-oracle-strength lane missing"))?,
        ] {
            let prompt = render_lane_model_prompt(&lane, &spec, "shared context");

            assert!(prompt.contains("batch every material test-oracle weakness"));
            assert!(prompt.contains("submaterial polish as low advisory or parked-follow-up"));
            assert!(prompt.contains("red/green-correct or proof receipts answer the concern"));
            assert!(prompt.contains("resolved-check or failed_objection"));
            assert!(prompt.contains("Do not drip-feed one nit per pass"));
        }
        Ok(())
    }

    #[test]
    fn lane_prompt_does_not_seed_global_box_refutations() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let spec = direct_minimax_spec(&args);
        let lane = lane_plan("tests-red-green");

        let prompt = render_lane_model_prompt(&lane, &spec, "shared context");

        assert!(prompt.contains("do not introduce `Box::from(slice)`"));
        assert!(prompt.contains("unless the current PR diff, seeded thread, or a candidate"));
        assert!(prompt.contains("return it as a refuted false-premise failed_objection"));
        assert!(!prompt.contains("If that objection arises"));
    }

    #[test]
    fn lane_prompt_routes_execution_through_typed_or_legacy_requests() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let spec = direct_minimax_spec(&args);
        let lane = lane_plan("tests-red-green");

        let prompt = render_lane_model_prompt(&lane, &spec, "shared context");

        assert!(prompt.contains("Do not post, mutate files, or run shell commands"));
        assert!(
            prompt.contains("Request executable proof through `proof_requests` or `proof_intents`")
        );
        assert!(prompt.contains("at most 3 observations, 2 candidate_findings"));
        assert!(prompt.contains("2 proof_intents"));
        assert!(prompt.contains("focused command requested from central proof broker"));
    }

    #[test]
    fn lane_model_identity_is_split() {
        let lanes = default_lanes();
        let security = lanes.iter().find(|lane| lane.id == "security");
        assert!(security.is_some());
        if let Some(security) = security {
            assert_eq!(security.id, "security");
            assert_eq!(security.model_display, "MiniMax-M3");
            assert_ne!(security.id, security.model_display);
        }
    }

    #[test]
    fn profile_selection_prefers_gh_runner_on_actions() {
        let box_state = BoxState {
            cpus: 2,
            free_mem_mb: Some(7_000),
            free_disk_mb: Some(10_000),
            load_1m: Some(0.5),
            github_actions: true,
        };
        assert_eq!(box_state.suggested_profile(), "gh-runner");
    }

    fn test_box_state() -> BoxState {
        BoxState {
            cpus: 2,
            free_mem_mb: Some(7_000),
            free_disk_mb: Some(10_000),
            load_1m: Some(0.5),
            github_actions: false,
        }
    }

    #[test]
    fn runtime_profile_override_takes_precedence_over_legacy_profile() {
        assert_eq!(
            runtime_profile_override(Some(&ProfileArg::Cx23), Some(&ProfileArg::Cx43)),
            Some("cx43")
        );
        assert_eq!(
            runtime_profile_override(Some(&ProfileArg::GhRunner), Some(&ProfileArg::GhRunnerFull)),
            Some("gh-runner-full")
        );
        assert_eq!(
            runtime_profile_override(Some(&ProfileArg::Cx23), None),
            Some("cx23")
        );
    }

    #[test]
    fn builtin_gh_runner_profile_matches_default_test_lease() {
        let builtin = builtin_profiles()
            .into_iter()
            .find(|profile| profile.name == "gh-runner");
        assert!(builtin.is_some());
        if let Some(builtin) = builtin {
            assert_eq!(builtin.limits.tests, Profile::default().limits.tests);
            assert_eq!(builtin.limits.tests, 2);
        }
    }

    #[test]
    fn builtin_runtime_profiles_match_runtime_files() -> Result<()> {
        let profiles = builtin_profiles();
        assert_eq!(
            profiles
                .iter()
                .map(|profile| profile.name.as_str())
                .collect::<Vec<_>>(),
            vec![
                "gh-runner",
                "gh-runner-standard",
                "gh-runner-full",
                "cx23",
                "cx33",
                "cx43",
            ]
        );
        let from_files = vec![
            runtime_profile_from_toml(include_str!("../runtime/gh-runner.toml"))?,
            runtime_profile_from_toml(include_str!("../runtime/gh-runner-standard.toml"))?,
            runtime_profile_from_toml(include_str!("../runtime/gh-runner-full.toml"))?,
            runtime_profile_from_toml(include_str!("../runtime/cx23.toml"))?,
            runtime_profile_from_toml(include_str!("../runtime/cx33.toml"))?,
            runtime_profile_from_toml(include_str!("../runtime/cx43.toml"))?,
        ];
        assert_eq!(
            serde_json::to_value(&profiles)?,
            serde_json::to_value(&from_files)?
        );
        Ok(())
    }

    #[test]
    fn gh_runner_alias_matches_standard_profile_except_name() -> Result<()> {
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;
        let standard = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner-standard")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner-standard profile"))?;

        assert_eq!(gh_runner.limits, standard.limits);
        assert_eq!(gh_runner.guards, standard.guards);
        assert_eq!(gh_runner.budgets, standard.budgets);
        assert_eq!(gh_runner.trusted_repo, standard.trusted_repo);
        assert_eq!(gh_runner.tool_timeouts, standard.tool_timeouts);
        Ok(())
    }

    #[test]
    fn builtin_runtime_profiles_encode_trusted_repo_gate_defaults() {
        for profile in builtin_profiles() {
            assert_eq!(
                profile.trusted_repo.pass_triggers,
                vec!["opened".to_owned(), "ready_for_review".to_owned()],
                "{} pass triggers",
                profile.name
            );
            assert!(
                !profile.trusted_repo.synchronize,
                "{} should not run full passes on synchronize by default",
                profile.name
            );
            assert_eq!(
                profile.budgets.default_timeout_sec, 1_800,
                "{} target timeout",
                profile.name
            );
            assert_eq!(
                profile.budgets.hard_timeout_sec, 3_600,
                "{} hard timeout",
                profile.name
            );
            assert!(
                profile.budgets.default_timeout_sec <= profile.budgets.hard_timeout_sec,
                "{} target timeout exceeds hard timeout",
                profile.name
            );
        }
    }

    #[test]
    fn action_smoke_workflow_is_a_manual_debug_lane() {
        // The self gate exercises `uses: ./` plus live model lanes on every
        // PR pass, so the smoke must not spend PR passes of its own; it stays
        // available as a workflow_dispatch debugging lane.
        let workflow = include_str!("../.github/workflows/action-smoke.yml");
        let action = include_str!("../action.yml");
        assert!(
            workflow.contains("workflow_dispatch:"),
            "action smoke should stay available as a manual debugging lane"
        );
        assert!(
            !workflow.contains("pull_request"),
            "action smoke must not trigger on or condition over pull_request events"
        );
        assert!(
            workflow.contains("if: inputs.run_model_smoke == true"),
            "the model smoke job should run only on explicit dispatch opt-in"
        );
        assert!(
            action.contains("run-pass:")
                && action.contains("UB_REVIEW_GITHUB_EVENT_ACTION: ${{ github.event.action }}")
                && action.contains("--run-pass \"${{ inputs['run-pass'] }}\""),
            "the action should pass event action and resolved pass input into ub-review run"
        );
    }

    #[test]
    fn ub_review_gate_workflow_matches_self_profile_post_policy() {
        // Dogfood posture: the self gate runs on every PR pass so a required
        // check exists for every head SHA, but review posting is reserved for
        // opened/ready_for_review; synchronize and reopened passes stay
        // gate-only so push storms do not become an inbox tax.
        let workflow = include_str!("../.github/workflows/ub-review-gate.yml");
        let profile = include_str!("../.ub-review.toml");
        assert!(
            workflow.contains("types: [opened, reopened, ready_for_review, synchronize]"),
            "self gate should run on every PR pass, including synchronize"
        );
        assert!(
            profile.contains("post_review_on = [\"opened\", \"ready_for_review\"]"),
            "self config should post reviews on opened/ready_for_review only"
        );
        assert!(
            workflow.contains(
                "posting: ${{ github.event_name == 'pull_request' && 'review' || 'artifact-only' }}"
            ),
            "posting expression should post the grouped review on every PR pass"
        );
        assert!(
            workflow.contains("run-pass: auto"),
            "run-pass must stay auto so synchronize/reopened resolve to first-class passes that the [gate].post_review_on policy can admit"
        );
        assert!(
            workflow.contains(
                "base: ${{ github.event_name == 'pull_request' && format('origin/{0}', github.event.pull_request.base.ref) || 'origin/main' }}"
            ),
            "self gate must diff against the PR's declared base branch so stacked PRs measure only their layer"
        );
        assert!(
            workflow.contains("cancel-in-progress: true"),
            "synchronize passes must collapse push storms via concurrency"
        );
        assert!(
            workflow.contains("model-mode: auto"),
            "every PR pass should keep normal model routing"
        );
    }

    #[test]
    fn ub_review_self_profile_loads_baseline_gate_tools() -> Result<()> {
        let mut config: Config = toml::from_str(include_str!("../.ub-review.toml"))?;
        config.merge_defaults();
        assert_eq!(config.review_profile, "ub-review-self");
        assert_eq!(config.profile, "gh-runner-full");
        assert_eq!(
            config.review_body.summary_only_body,
            SummaryOnlyBodyPolicy::PostSubstantive,
            "dogfood profile posts substantive summary-only bodies for calibration"
        );
        assert_eq!(config.gate.required_check, "ub-review/gate");
        assert_eq!(config.gate.target_minutes, 30);
        assert_eq!(config.gate.hard_timeout_minutes, 60);
        assert_eq!(
            config.gate.post_review_on,
            vec!["opened".to_owned(), "ready_for_review".to_owned()]
        );
        // The three doctrine lanes the corpus mandated
        // (UB-REVIEW-SPEC-0011): the builtin lanes were blind to this
        // repo's mirror drifts and spec overclaims.
        assert_eq!(
            config
                .lanes
                .iter()
                .map(|lane| lane.id.as_str())
                .collect::<Vec<_>>(),
            vec!["contract-mirror", "gate-semantics", "spec-honesty"]
        );
        assert_eq!(config.repo.ledger, "docs/REVIEW_LEDGER.md");
        for id in [
            "cargo-fmt",
            "cargo-check",
            "cargo-test",
            "cargo-clippy",
            "cargo-doc",
            "artifact-verifier",
            "ripr",
            "unsafe-review",
            "ast-grep",
            "cargo-allow",
            "actionlint",
        ] {
            let tool = config
                .tools
                .get(id)
                .ok_or_else(|| anyhow::anyhow!("missing self-profile tool {id}"))?;
            assert!(tool.enabled, "{id} should be enabled");
        }
        let coverage = config
            .tools
            .get("coverage")
            .ok_or_else(|| anyhow::anyhow!("missing self-profile coverage tool"))?;
        assert!(
            coverage.enabled,
            "coverage folds the standalone coverage workflow into the self gate"
        );
        assert!(
            coverage.requires_lease,
            "coverage stays a leased heavy witness behind allow-heavy"
        );
        assert!(
            !coverage.required,
            "coverage is execution-surface telemetry, not a blocking gate sensor"
        );
        for policy in &config.proof.required {
            assert!(
                policy.enabled && policy.required,
                "self-profile proof policy {} should be required and enabled",
                policy.id
            );
            assert_eq!(
                super::proof_request_status(
                    &policy.command,
                    policy.cost.as_deref().unwrap_or_default()
                ),
                "requested",
                "self-profile required proof {} must be brokerable, not silently unsupported",
                policy.id
            );
        }
        assert!(
            config
                .proof
                .required
                .iter()
                .any(|policy| policy.id == "policy-check"
                    && policy.command == "cargo xtask policy-check"),
            "policy-check from the folded CI workflow must stay a required gate proof"
        );
        let ripr = config
            .tools
            .get("ripr")
            .ok_or_else(|| anyhow::anyhow!("missing self-profile ripr tool"))?;
        let ripr_gate = ripr
            .gate
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("missing self-profile ripr gate policy"))?;
        assert_eq!(ripr_gate.scope.as_deref(), Some("on-diff"));
        assert_eq!(ripr_gate.max_new_unsuppressed, Some(0));
        let plan = super::build_plan(
            &config,
            config.selected_profile()?,
            &BoxState {
                cpus: 4,
                free_mem_mb: Some(8_000),
                free_disk_mb: Some(20_000),
                load_1m: Some(0.5),
                github_actions: true,
            },
            &test_diff(),
            Path::new("."),
            true,
        );
        for id in [
            "cargo-fmt",
            "cargo-check",
            "cargo-test",
            "cargo-clippy",
            "cargo-doc",
            "artifact-verifier",
        ] {
            let sensor = plan
                .sensors
                .iter()
                .find(|sensor| sensor.id == id)
                .ok_or_else(|| anyhow::anyhow!("missing planned sensor {id}"))?;
            assert!(sensor.run, "{id} should run in every self gate");
            assert!(
                sensor.required,
                "{id} should be a required self gate sensor"
            );
        }
        let coverage_sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "coverage")
            .ok_or_else(|| anyhow::anyhow!("missing planned coverage sensor"))?;
        assert!(
            coverage_sensor.run,
            "coverage should run in the self gate when heavy witnesses are leased"
        );
        assert!(
            !coverage_sensor.required,
            "coverage stays advisory in the self gate"
        );
        let ripr_sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "ripr")
            .ok_or_else(|| anyhow::anyhow!("missing planned ripr sensor"))?;
        assert_eq!(ripr_sensor.gate, ripr.gate);
        let resolved_tools =
            super::resolved_tools_artifact(&config, config.selected_profile()?, &plan);
        let resolved_ripr = resolved_tools
            .tools
            .iter()
            .find(|tool| tool.id == "ripr")
            .ok_or_else(|| anyhow::anyhow!("missing resolved ripr tool"))?;
        assert_eq!(resolved_ripr.gate, ripr.gate);
        let temp = tempfile::tempdir()?;
        let tool_status =
            super::tool_status_artifact(temp.path(), &config, config.selected_profile()?, &plan);
        let status_ripr = tool_status
            .tools
            .iter()
            .find(|tool| tool.id == "ripr")
            .ok_or_else(|| anyhow::anyhow!("missing status ripr tool"))?;
        assert_eq!(status_ripr.gate, ripr.gate);
        let resolved_profile =
            super::resolved_profile_artifact(&config, config.selected_profile()?);
        assert_eq!(resolved_profile["gate"]["required_check"], "ub-review/gate");
        let resolved_plan = super::resolved_plan_artifact(
            &config,
            config.selected_profile()?,
            &test_diff(),
            &plan,
            None,
            &SelectorArgs::default(),
            None,
        );
        assert_eq!(resolved_plan["gate"], resolved_profile["gate"]);
        Ok(())
    }

    #[test]
    fn ub_review_example_config_loads_clean_and_demonstrates_schema() -> Result<()> {
        // The nominal consumer example (configs/ub-review.example.toml) must
        // parse with zero policy_errors, like the test-pinned full-feature
        // reference (unsafe_review_swarm). See #607 / tracker UB-24.
        let mut config = Config::from_toml_with_policy_receipts(include_str!(
            "../configs/ub-review.example.toml"
        ))?;
        config.merge_defaults();
        assert!(
            config.policy_errors.is_empty(),
            "example config should not rely on stripped or deprecated keys: {:?}",
            config.policy_errors
        );
        assert_eq!(config.review_profile, "bun-ub-v0");
        assert_eq!(config.profile, "gh-runner");
        assert_eq!(config.gate.required_check, "ub-review/gate");
        assert_eq!(
            config.gate.post_review_on,
            vec!["opened".to_owned(), "ready_for_review".to_owned()]
        );
        // The example demonstrates the ripr gate threshold and the disabled-
        // tool set; the three richest surfaces ([[lanes]], [[proof.required]],
        // [providers]) are shown as commented documentation, so this config
        // exercises none of them by value — that is intentional (lean first
        // pass) and the comments point at unsafe-review-swarm for the full
        // feature set.
        assert!(config.lanes.is_empty(), "example keeps lanes commented");
        assert!(
            config.proof.required.is_empty(),
            "example keeps required proof commented"
        );
        assert!(
            config.providers.policy.is_empty(),
            "example keeps provider policy commented"
        );
        Ok(())
    }

    #[test]
    fn work_queue_includes_baseline_sensor_packet_policies() -> Result<()> {
        let mut config: Config = toml::from_str(include_str!("../.ub-review.toml"))?;
        config.merge_defaults();
        let plan = super::build_plan(
            &config,
            config.selected_profile()?,
            &BoxState {
                cpus: 4,
                free_mem_mb: Some(8_000),
                free_disk_mb: Some(20_000),
                load_1m: Some(0.5),
                github_actions: true,
            },
            &test_diff(),
            Path::new("."),
            true,
        );
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("sensors/cargo-fmt"))?;
        fs::write(
            temp.path()
                .join("sensors/cargo-fmt/ub-review-sensor-status.json"),
            "{}",
        )?;
        super::write_work_queue_artifacts(temp.path(), &plan, &[])?;
        let queue: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("work_queue.json"))?)?;
        let tasks = queue["tasks"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("work queue tasks missing"))?;
        assert_eq!(tasks.len(), plan.sensors.len());

        let cargo_fmt = tasks
            .iter()
            .find(|task| task["id"] == "sensor-cargo-fmt")
            .ok_or_else(|| anyhow::anyhow!("cargo-fmt queue task missing"))?;
        assert_eq!(cargo_fmt["kind"], "sensor");
        assert_eq!(cargo_fmt["source"], "tool-registry");
        assert_eq!(cargo_fmt["packet_policy"], "must-run");
        assert_eq!(cargo_fmt["gate_policy"], "gate-required");
        assert_eq!(cargo_fmt["status"], "planned");
        assert_eq!(
            cargo_fmt["receipt_path"],
            "sensors/cargo-fmt/ub-review-sensor-status.json"
        );
        assert_eq!(
            cargo_fmt["initial_packet_status"],
            "ready_for_initial_packet"
        );

        let tokmd = tasks
            .iter()
            .find(|task| task["id"] == "sensor-tokmd")
            .ok_or_else(|| anyhow::anyhow!("tokmd queue task missing"))?;
        assert_eq!(tokmd["packet_policy"], "include-if-ready");
        assert_eq!(tokmd["gate_policy"], "review-context");
        assert_eq!(tokmd["initial_packet_status"], "pending_initial_packet");

        let semgrep = tasks
            .iter()
            .find(|task| task["id"] == "sensor-semgrep")
            .ok_or_else(|| anyhow::anyhow!("semgrep queue task missing"))?;
        assert_eq!(semgrep["packet_policy"], "artifact-only");
        assert_eq!(semgrep["status"], "skipped");
        assert_eq!(semgrep["deadline_sec"], 0);
        assert_eq!(semgrep["lease"]["timeout_sec"], semgrep["deadline_sec"]);
        assert_eq!(semgrep["initial_packet_status"], "not_initial_packet");

        let work_events = fs::read_to_string(temp.path().join("work_events.ndjson"))?;
        let cargo_fmt_event = work_events
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|event| event["task_id"] == "sensor-cargo-fmt")
            .ok_or_else(|| anyhow::anyhow!("cargo-fmt work event missing"))?;
        assert_eq!(
            cargo_fmt_event["initial_packet_status"],
            cargo_fmt["initial_packet_status"]
        );
        Ok(())
    }

    #[test]
    fn work_queue_late_phase_sensor_stays_pending_initial_packet() -> Result<()> {
        // #325: a late-phase sensor was never part of the initial packet, so
        // its initial-packet status is deterministically pending even when
        // its receipt has landed by the time the queue artifact is written.
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("sensors/cargo-test"))?;
        fs::write(
            temp.path()
                .join("sensors/cargo-test/ub-review-sensor-status.json"),
            "{}",
        )?;
        let mut late = sensor_plan("cargo-test", "cargo", true);
        late.required = true;
        late.phase = super::SensorPhase::Late;
        let task = super::work_queue_task_from_sensor(temp.path(), &late);
        assert_eq!(task.packet_policy, "must-run");
        assert_eq!(task.initial_packet_status, "pending_initial_packet");

        let mut fast = sensor_plan("cargo-test", "cargo", true);
        fast.required = true;
        let ready = super::work_queue_task_from_sensor(temp.path(), &fast);
        assert_eq!(ready.initial_packet_status, "ready_for_initial_packet");
        Ok(())
    }

    #[test]
    fn lane_packets_render_late_phase_sensors_as_scheduled() -> Result<()> {
        // #325: packets must not read a racing late receipt — a routed late
        // sensor renders as scheduled work with the late-is-not-missing rule.
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        fs::create_dir_all(out.join("sensors/ast-grep"))?;
        fs::write(
            out.join("sensors/ast-grep/ub-review-sensor-status.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "sensor": "ast-grep",
                "status": "ok",
                "reason": "completed",
            }))?,
        )?;
        let mut coverage = sensor_plan("coverage", "cargo", true);
        coverage.phase = super::SensorPhase::Late;
        let plan = test_plan(vec![coverage, sensor_plan("ast-grep", "ast-grep", true)]);
        let lane = LanePlan {
            id: "tests-oracle".to_owned(),
            role: "test oracle".to_owned(),
            model: "custom:test".to_owned(),
            model_display: "test model".to_owned(),
            receives: vec!["coverage".to_owned(), "ast-grep".to_owned()],
            focus: "test focus".to_owned(),
        };
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        super::write_lane_packets(
            &out,
            &test_diff(),
            &plan,
            std::slice::from_ref(&lane),
            &test_pr_thread_context(),
            &event_log,
        )?;
        let packet = fs::read_to_string(out.join("lanes/tests-oracle.md"))?;
        assert!(
            packet.contains("- `coverage`: `scheduled-late`"),
            "late routed sensor must render as scheduled: {packet}"
        );
        assert!(packet.contains("late is not missing"));
        assert!(
            packet.contains("- `ast-grep`: `ok`"),
            "fast routed sensor still renders its receipt status: {packet}"
        );
        Ok(())
    }

    #[test]
    fn evidence_sections_surface_unevaluated_required_tool_gates() -> Result<()> {
        // #316's alarm class: repo policy configured a required tool gate and
        // the run produced no verdict for it. The running summary must say so
        // under Missing evidence, not let the gap idle in artifacts.
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("tool-gate-outcomes.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "ub-review.tool_gate_outcomes.v1",
                "outcomes": [{
                    "schema": "ub-review.tool_gate_outcome.v1",
                    "tool": "ripr",
                    "required": true,
                    "evaluated": false,
                    "outcome": "missing_evidence",
                    "reason": "`ripr` ran ok, but no machine-readable gate-decision receipt was available"
                }]
            }))?,
        )?;
        let plan = test_plan(Vec::new());
        let mut text = String::new();
        super::render_evidence_sections(&mut text, temp.path(), &plan);
        assert!(
            text.contains("ripr gate threshold configured but not evaluated"),
            "missing-evidence section must name the unevaluated gate: {text}"
        );
        assert!(
            text.contains("no machine-readable gate-decision receipt"),
            "the entry carries the outcome reason: {text}"
        );
        Ok(())
    }

    #[test]
    fn repo_lane_toml_parse_pins_exact_field_defaults() -> Result<()> {
        // Exact-value oracle for the RepoLane authoring contract: a minimal
        // [[lanes]] entry deserializes with every optional field at its
        // documented default (empty until plan-time defaulting), and a full
        // entry round-trips field-for-field. Pins the serde surface the
        // doctrine documents (UB-REVIEW-SPEC-0011).
        let config: Config = toml::from_str(
            r#"
[[lanes]]
id = "contract-mirror"
role = "Cross-language mirror parity review"
focus = "Check both sides of mirrored contracts moved together."
"#,
        )?;
        let lane = &config.lanes[0];
        assert_eq!(lane.id, "contract-mirror");
        assert_eq!(lane.role, "Cross-language mirror parity review");
        assert_eq!(
            lane.focus,
            "Check both sides of mirrored contracts moved together."
        );
        assert_eq!(lane.receives, Vec::<String>::new());
        assert_eq!(lane.model, "");
        assert_eq!(lane.diff_classes, Vec::<String>::new());

        let full: Config = toml::from_str(
            r#"
[[lanes]]
id = "x"
role = "r"
focus = "f"
receives = ["tokmd"]
model = "custom:m"
diff_classes = ["docs-only"]
"#,
        )?;
        let lane = &full.lanes[0];
        assert_eq!(
            (
                lane.id.as_str(),
                lane.role.as_str(),
                lane.focus.as_str(),
                lane.receives.clone(),
                lane.model.as_str(),
                lane.diff_classes.clone(),
            ),
            (
                "x",
                "r",
                "f",
                vec!["tokmd".to_owned()],
                "custom:m",
                vec!["docs-only".to_owned()],
            )
        );
        Ok(())
    }

    #[test]
    fn issues_toml_parse_pins_exact_field_defaults() -> Result<()> {
        // Exact-value oracle for the [issues] authoring contract: an absent
        // section deserializes to the documented defaults (enabled, suggest),
        // and an explicit section round-trips field-for-field. Pins the serde
        // surface the issue-capture posture documents.
        let absent: Config = toml::from_str("")?;
        assert!(absent.issues.enabled);
        assert_eq!(absent.issues.mode, "suggest");
        assert_eq!(absent.issues.open_in, Vec::<String>::new());
        assert_eq!(absent.issues.open_cap, 3);

        let explicit: Config = toml::from_str(
            r#"
[issues]
enabled = false
mode = "off"
open_in = ["EffortlessMetrics/ripr-swarm"]
open_cap = 1
"#,
        )?;
        assert!(!explicit.issues.enabled);
        assert_eq!(explicit.issues.mode, "off");
        assert_eq!(
            explicit.issues.open_in,
            vec!["EffortlessMetrics/ripr-swarm".to_owned()]
        );
        assert_eq!(explicit.issues.open_cap, 1);
        Ok(())
    }

    #[test]
    fn providers_policy_config_parses_resolves_and_receipts_invalid_values() -> Result<()> {
        use super::ModelProviderPolicy;
        // Exact-value oracle for the consumed [providers] surface: absent
        // section reads as unset; an explicit policy round-trips; descriptive
        // sub-table keys parse without effect, while max_concurrency maps
        // into the scheduler's provider limit surface.
        let absent: Config = toml::from_str("")?;
        assert_eq!(absent.providers.policy, "");

        let explicit: Config = toml::from_str(
            r#"
[providers]
policy = "primary-with-fallback"

[providers.minimax]
enabled = true
max_concurrency = 12
prompt_cache = "off"

[providers.opencode]
role = "fallback"
max_concurrency = 8
"#,
        )?;
        assert_eq!(explicit.providers.policy, "primary-with-fallback");
        let limits = provider_concurrency_limits(&explicit);
        assert_eq!(
            limits,
            ProviderConcurrencyLimits {
                minimax: Some(12),
                opencode_go: Some(8),
            }
        );
        assert_eq!(
            resolved_minimax_prompt_cache(&explicit),
            MinimaxPromptCache::Off
        );

        // D2 precedence matrix: explicit CLI wins; auto defers to config;
        // auto with no config stays auto (built-in minimax-primary
        // semantics in the dispatch functions).
        assert_eq!(
            resolved_provider_policy(&explicit, ModelProviderPolicy::MinimaxOnly),
            ModelProviderPolicy::MinimaxOnly,
        );
        assert_eq!(
            resolved_provider_policy(&explicit, ModelProviderPolicy::Auto),
            ModelProviderPolicy::PrimaryWithFallback,
        );
        assert_eq!(
            resolved_provider_policy(&absent, ModelProviderPolicy::Auto),
            ModelProviderPolicy::Auto,
        );

        // An invalid policy value is stripped with a PolicyError receipt -
        // a typo can never silently fall back while looking configured.
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(".ub-review.toml");
        fs::write(&config_path, "[providers]\npolicy = \"minimax-primry\"\n")?;
        let loaded = Config::load_or_default(&config_path, None)?;
        assert_eq!(loaded.providers.policy, "");
        assert!(
            loaded.policy_errors.iter().any(|error| {
                error.section == "providers"
                    && error.detail.contains("invalid [providers] policy value")
            }),
            "expected a providers PolicyError receipt: {:?}",
            loaded.policy_errors
        );
        assert_eq!(
            resolved_provider_policy(&loaded, ModelProviderPolicy::Auto),
            ModelProviderPolicy::Auto,
        );
        Ok(())
    }

    #[test]
    fn review_byok_advisory_contract_pins_resolution_alias_and_posture_defaults() -> Result<()> {
        // SPEC-0002's enforcement pivot, pinned as a full matrix: auto
        // enforces for intelligent-ci alone; review-byok (and the agent
        // modes) never enforce unless fail-on-gate is explicitly true.
        use super::{FailOnGate, RunMode};
        let modes = [
            (RunMode::ReviewByok, false),
            (RunMode::IntelligentCi, true),
            (RunMode::AgentInvestigate, false),
            (RunMode::AgentPatch, false),
        ];
        for (mode, auto_enforces) in modes {
            assert_eq!(
                FailOnGate::Auto.resolved(mode),
                auto_enforces,
                "auto resolution for {}",
                mode.key()
            );
            assert!(FailOnGate::True.resolved(mode));
            assert!(!FailOnGate::False.resolved(mode));
        }

        // `review-direct` is a legacy alias of review-byok: same variant,
        // same never-enforcing resolution. The alias must never grow its own
        // semantics.
        let direct = <RunMode as clap::ValueEnum>::from_str("review-direct", false)
            .map_err(|err| anyhow::anyhow!("parse review-direct: {err}"))?;
        assert_eq!(direct, RunMode::ReviewByok);
        assert_eq!(direct.key(), "review-byok");
        assert!(!FailOnGate::Auto.resolved(direct));

        // The byok posting posture defaults the spec documents: standalone
        // approvals banned, zero-finding audits required, no custom poster.
        let config: Config = toml::from_str("")?;
        assert!(config.review.ban_standalone_approval);
        assert!(config.review.require_zero_finding_audit);
        assert!(!config.review.custom_poster);
        assert_eq!(config.review.posting_engine, "github-step-summary");
        Ok(())
    }

    #[test]
    fn repo_lanes_merge_with_defaults_replacement_and_diff_class_gating() -> Result<()> {
        let mut config: Config = toml::from_str(
            r#"
[[lanes]]
id = "contract-mirror"
role = "Cross-language mirror parity review"
focus = "Check both sides of mirrored contracts moved together."

[[lanes]]
id = "opposition"
role = "Repo-tuned opposition"
focus = "Route checkable objections to proof requests, never confident refutations."
model = "custom:test-model"

[[lanes]]
id = ""
role = "invalid"
focus = "missing id"

[[lanes]]
id = "docs-claims"
role = "Docs claims review"
focus = "Spec claims versus code."
diff_classes = ["docs-only"]
"#,
        )?;
        config.merge_defaults();
        let plan = super::build_plan(
            &config,
            config.selected_profile()?,
            &BoxState {
                cpus: 4,
                free_mem_mb: Some(8_000),
                free_disk_mb: Some(20_000),
                load_1m: Some(0.5),
                github_actions: true,
            },
            &test_diff(),
            Path::new("."),
            true,
        );

        // Conversion: a new repo lane registers with the default sensor trio
        // and the default lane model; the diff-class-gated and invalid
        // entries do not.
        let mirror = plan
            .repo_lanes
            .iter()
            .find(|lane| lane.id == "contract-mirror")
            .ok_or_else(|| anyhow::anyhow!("repo lane contract-mirror missing"))?;
        assert_eq!(mirror.receives, vec!["tokmd", "ripr", "ast-grep"]);
        assert_eq!(mirror.model_display, "MiniMax-M3");
        assert_eq!(
            mirror.focus,
            "Check both sides of mirrored contracts moved together."
        );
        assert!(
            !plan.repo_lanes.iter().any(|lane| lane.id == "docs-claims"),
            "docs-only lane must not register on a {} diff",
            plan.diff_class.key()
        );
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("repo lane skipped: id and focus are required")),
            "invalid lane entries surface in plan notes: {:?}",
            plan.notes
        );
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("repo lane `contract-mirror` registered")),
        );

        // Execution: the EXECUTED lane set carries the repo lanes at every
        // lane width - PR #344's first live run proved plan-only wiring is
        // not execution (plan notes recorded the lanes, the executed set did
        // not contain them).
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let executed = super::review_lanes_for_args(&plan, &args);
        let executed_mirror = executed
            .iter()
            .find(|lane| lane.id == "contract-mirror")
            .ok_or_else(|| anyhow::anyhow!("contract-mirror missing from executed set"))?;
        assert_eq!(
            executed_mirror.focus,
            "Check both sides of mirrored contracts moved together."
        );
        let executed_oppositions = executed
            .iter()
            .filter(|lane| lane.id == "opposition")
            .collect::<Vec<_>>();
        assert_eq!(
            executed_oppositions.len(),
            1,
            "replacement must not duplicate in the executed set"
        );
        assert_eq!(executed_oppositions[0].model, "custom:test-model");
        assert_eq!(executed_oppositions[0].role, "Repo-tuned opposition");
        assert!(
            !executed.iter().any(|lane| lane.id == "docs-claims"),
            "diff-class gating holds through execution selection"
        );
        Ok(())
    }

    #[test]
    fn action_config_input_overrides_bundled_preset_selection() {
        let action = include_str!("../action.yml");
        assert!(
            action.contains("config:")
                && action.contains("config_path=\"${{ inputs.config }}\"")
                && action.contains("if [[ -z \"$config_path\" ]]; then")
                && action.contains("config_path=\"$GITHUB_ACTION_PATH/profiles/bun-ub-v0.toml\"")
                && action.contains("--config \"$config_path\""),
            "the action should let repo-local config paths override bundled preset selection"
        );
    }

    #[test]
    fn release_binary_workflow_dispatch_is_packaging_dry_run() {
        let workflow = include_str!("../.github/workflows/release-binary.yml");
        assert!(
            workflow.contains("workflow_dispatch:"),
            "release packaging should be manually dry-runnable before a tag is pushed"
        );
        assert!(
            workflow.contains("package:\n    name: Build release asset"),
            "the manual-runnable job should describe packaging, not release publishing"
        );
        assert!(
            workflow.contains("package:\n    name: Build release asset")
                && workflow.contains("permissions:\n      contents: read"),
            "the manual-runnable packaging job must not receive release-write permission"
        );
        assert!(
            workflow.contains("name: Upload workflow artifact")
                && workflow.contains("dist/${{ env.ASSET_NAME }}")
                && workflow.contains("dist/${{ env.ASSET_NAME }}.sha256"),
            "manual dry-runs must leave the archive and .sha256 as workflow artifacts"
        );
        let tag_only_guard =
            "if: github.event_name == 'push' && startsWith(github.ref, 'refs/tags/')";
        assert!(
            workflow.contains(&format!(
                "publish:\n    name: Publish release asset\n    needs: package\n    {tag_only_guard}"
            )),
            "the publish job must exist only on tag pushes"
        );
        assert!(
            workflow.contains("publish:\n    name: Publish release asset")
                && workflow.contains("permissions:\n      contents: write")
                && workflow.contains("uses: actions/download-artifact@v7")
                && workflow.contains("name: Publish GitHub release asset"),
            "release write permission and GitHub release mutation must stay inside the tag-only publish job"
        );
        let publish_section = workflow
            .split_once("\n  publish:\n")
            .map(|(_, section)| section)
            .unwrap_or_default();
        let checkout_index = publish_section.find("uses: actions/checkout@v5");
        let notes_index = publish_section.find("--notes-file .github/release-notes.md");
        assert!(
            checkout_index
                .zip(notes_index)
                .is_some_and(|(checkout, notes)| checkout < notes),
            "the publish job must check out the tagged repository before reading release notes"
        );
    }

    #[test]
    fn release_binary_workflow_records_and_validates_immutable_candidate() {
        let workflow = include_str!("../.github/workflows/release-binary.yml");
        for required in [
            "name: Write immutable release candidate receipt",
            "ub-review.release_candidate.v1",
            "candidate_sha=\"$(git rev-parse HEAD)\"",
            "dist/release-candidate.json",
            "name: Validate immutable release candidate receipt",
            "$(jq -r '.head_sha' \"$manifest\")\" = \"$GITHUB_SHA\"",
            "$(jq -r '.archive_sha256' \"$manifest\")\" = \"$archive_sha256\"",
            "$(jq -r '.toolchain' \"$manifest\")\" = \"1.95.0\"",
        ] {
            assert!(
                workflow.contains(required),
                "release candidate contract missing: {required}"
            );
        }
    }

    #[test]
    fn action_release_download_verifies_sha256_receipt() {
        let action = include_str!("../action.yml");
        assert!(
            action.contains("checksum_url=\"$url.sha256\"")
                && action.contains(
                    "curl -fL --retry 3 --retry-delay 2 -o \"$checksum\" \"$checksum_url\""
                )
                && action
                    .contains("expected_sha=\"$(awk 'NF >= 1 {print $1; exit}' \"$checksum\")\"")
                && action.contains("[[ ! \"$expected_sha\" =~ ^[A-Fa-f0-9]{64}$ ]]")
                && action.contains("actual_sha=\"$(sha256sum \"$archive\" | awk '{print $1}')\"")
                && action.contains("[[ \"$actual_sha\" != \"$expected_sha\" ]]")
                && action.contains(
                    "report_release_unavailable warning \"release binary checksum mismatch\""
                ),
            "release download must verify the .sha256 receipt before accepting the binary"
        );
    }

    #[test]
    fn action_release_install_mode_fails_closed_instead_of_source_fallback() {
        let action = include_str!("../action.yml");
        assert!(
            action.contains("if [[ \"$mode\" == \"release\" ]]; then")
                && action.contains(
                    "install-mode=release failed; use install-mode=auto for source fallback"
                )
                && action.contains(
                    "report_release_unavailable warning \"release binary download failed\""
                ),
            "explicit install-mode=release should fail when release receipts are unavailable; only auto may source-build fallback"
        );
    }

    #[test]
    fn bun_config_loads_with_default_lanes_enabled() -> Result<()> {
        let mut config: Config = toml::from_str(include_str!("../profiles/bun-ub-v0.toml"))?;
        config.merge_defaults();
        assert_eq!(config.review_profile, DEFAULT_REVIEW_PROFILE);
        assert_eq!(config.profile, "gh-runner");
        assert!(config.review.enable_default_lanes);
        let profile = config.selected_profile()?;
        assert_eq!(profile.name, "gh-runner");
        let ripr = config
            .tools
            .get("ripr")
            .ok_or_else(|| anyhow::anyhow!("ripr tool policy missing"))?;
        assert!(ripr.enabled);
        let cargo_allow = config
            .tools
            .get("cargo-allow")
            .ok_or_else(|| anyhow::anyhow!("cargo-allow tool policy missing"))?;
        assert!(cargo_allow.enabled);
        assert_eq!(cargo_allow.default, super::Trigger::SourceExceptionChanged);
        Ok(())
    }

    #[test]
    fn required_tool_policy_applies_only_when_trigger_matches() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut config = Config::default();
        let actionlint = config
            .tools
            .get_mut("actionlint")
            .ok_or_else(|| anyhow::anyhow!("actionlint tool missing"))?;
        actionlint.required = true;
        config
            .tools
            .get_mut("ripr")
            .ok_or_else(|| anyhow::anyhow!("ripr tool missing"))?
            .required = true;
        config
            .tools
            .get_mut("unsafe-review")
            .ok_or_else(|| anyhow::anyhow!("unsafe-review tool missing"))?
            .required = true;

        let workflow_files = vec![".github/workflows/ci.yml".to_owned()];
        let workflow_flags = classify_diff(&workflow_files, "+name: ci");
        let workflow_diff = DiffContext {
            base: "HEAD~1".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: workflow_files,
            patch: "+name: ci".to_owned(),
            diff_class: classify_diff_class(
                &[".github/workflows/ci.yml".to_owned()],
                &workflow_flags,
            ),
            flags: workflow_flags,
        };

        let workflow_plan = super::build_plan(
            &config,
            &Profile::default(),
            &BoxState {
                cpus: 4,
                free_mem_mb: None,
                free_disk_mb: None,
                load_1m: None,
                github_actions: false,
            },
            &workflow_diff,
            temp.path(),
            false,
        );
        let workflow_actionlint = workflow_plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "actionlint")
            .ok_or_else(|| anyhow::anyhow!("actionlint not planned"))?;
        assert!(workflow_actionlint.required);
        assert!(workflow_actionlint.run);
        let resolved_tools =
            super::resolved_tools_artifact(&config, &Profile::default(), &workflow_plan);
        let resolved_actionlint = resolved_tools
            .tools
            .iter()
            .find(|tool| tool.id == "actionlint")
            .ok_or_else(|| anyhow::anyhow!("resolved actionlint tool missing"))?;
        assert!(resolved_actionlint.required);
        assert!(resolved_actionlint.planned_run);

        let source_diff = test_diff();
        let source_plan = super::build_plan(
            &config,
            &Profile::default(),
            &BoxState {
                cpus: 4,
                free_mem_mb: None,
                free_disk_mb: None,
                load_1m: None,
                github_actions: false,
            },
            &source_diff,
            temp.path(),
            false,
        );
        let source_actionlint = source_plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "actionlint")
            .ok_or_else(|| anyhow::anyhow!("source actionlint not planned"))?;
        assert!(!source_actionlint.required);
        assert!(!source_actionlint.run);
        assert_eq!(source_actionlint.reason, "trigger did not match this diff");
        let source_ripr = source_plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "ripr")
            .ok_or_else(|| anyhow::anyhow!("source ripr not planned"))?;
        assert!(source_ripr.required);
        assert!(source_ripr.run);
        let source_unsafe_review = source_plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "unsafe-review")
            .ok_or_else(|| anyhow::anyhow!("source unsafe-review not planned"))?;
        assert!(source_unsafe_review.required);
        assert!(source_unsafe_review.run);
        let source_resolved =
            super::resolved_tools_artifact(&config, &Profile::default(), &source_plan);
        let source_resolved_actionlint = source_resolved
            .tools
            .iter()
            .find(|tool| tool.id == "actionlint")
            .ok_or_else(|| anyhow::anyhow!("source resolved actionlint missing"))?;
        assert!(!source_resolved_actionlint.required);
        Ok(())
    }

    #[test]
    fn partial_tool_config_inherits_builtin_routing_defaults() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut config: Config = toml::from_str(
            r#"
[tools.actionlint]
required = true
"#,
        )?;
        config.merge_defaults();
        let actionlint = config
            .tools
            .get("actionlint")
            .ok_or_else(|| anyhow::anyhow!("actionlint tool missing"))?;
        assert_eq!(actionlint.command, "actionlint");
        assert_eq!(actionlint.class, ToolClass::Workflow);
        assert_eq!(actionlint.default, super::Trigger::WorkflowChanged);
        assert_eq!(actionlint.weight, 1);
        assert_eq!(actionlint.timeout_sec, 60);
        assert_eq!(actionlint.artifact_budget_mb, 32);
        assert!(actionlint.enabled);
        assert!(actionlint.required);

        let workflow_files = vec![".github/workflows/ci.yml".to_owned()];
        let workflow_flags = classify_diff(&workflow_files, "+name: ci");
        let workflow_diff = DiffContext {
            base: "HEAD~1".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: workflow_files,
            patch: "+name: ci".to_owned(),
            diff_class: DiffClass::WorkflowTooling,
            flags: workflow_flags,
        };
        let workflow_plan = super::build_plan(
            &config,
            &Profile::default(),
            &BoxState {
                cpus: 4,
                free_mem_mb: None,
                free_disk_mb: None,
                load_1m: None,
                github_actions: false,
            },
            &workflow_diff,
            temp.path(),
            false,
        );
        let workflow_actionlint = workflow_plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "actionlint")
            .ok_or_else(|| anyhow::anyhow!("workflow actionlint not planned"))?;
        assert!(workflow_actionlint.required);
        assert!(workflow_actionlint.run);
        assert_eq!(workflow_actionlint.class, ToolClass::Workflow);

        let resolved_tools =
            super::resolved_tools_artifact(&config, &Profile::default(), &workflow_plan);
        let resolved_actionlint = resolved_tools
            .tools
            .iter()
            .find(|tool| tool.id == "actionlint")
            .ok_or_else(|| anyhow::anyhow!("resolved actionlint missing"))?;
        assert_eq!(resolved_actionlint.class, ToolClass::Workflow);
        assert_eq!(
            resolved_actionlint.required_if,
            super::Trigger::WorkflowChanged
        );
        assert!(resolved_actionlint.required);
        assert!(resolved_actionlint.planned_run);

        let source_diff = test_diff();
        let source_plan = super::build_plan(
            &config,
            &Profile::default(),
            &BoxState {
                cpus: 4,
                free_mem_mb: None,
                free_disk_mb: None,
                load_1m: None,
                github_actions: false,
            },
            &source_diff,
            temp.path(),
            false,
        );
        let source_actionlint = source_plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "actionlint")
            .ok_or_else(|| anyhow::anyhow!("source actionlint not planned"))?;
        assert!(!source_actionlint.required);
        assert!(!source_actionlint.run);
        assert_eq!(source_actionlint.reason, "trigger did not match this diff");
        Ok(())
    }

    #[test]
    fn partial_tool_config_preserves_explicit_overrides() -> Result<()> {
        let mut config: Config = toml::from_str(
            r#"
[tools.actionlint]
command = "custom-actionlint"
class = "static"
default = "always"
required = true
weight = 7
timeout_sec = 123
artifact_budget_mb = 45
requires_lease = true
enabled = false
"#,
        )?;
        config.merge_defaults();
        let actionlint = config
            .tools
            .get("actionlint")
            .ok_or_else(|| anyhow::anyhow!("actionlint tool missing"))?;
        assert_eq!(actionlint.command, "custom-actionlint");
        assert_eq!(actionlint.class, ToolClass::Static);
        assert_eq!(actionlint.default, super::Trigger::Always);
        assert!(actionlint.required);
        assert_eq!(actionlint.weight, 7);
        assert_eq!(actionlint.timeout_sec, 123);
        assert_eq!(actionlint.artifact_budget_mb, 45);
        assert!(actionlint.requires_lease);
        assert!(!actionlint.enabled);
        Ok(())
    }

    #[test]
    fn example_config_preserves_gate_policy() -> Result<()> {
        let mut config: Config = toml::from_str(include_str!("../configs/ub-review.example.toml"))?;
        config.merge_defaults();
        assert_eq!(config.gate.required_check, "ub-review/gate");
        assert_eq!(config.gate.target_minutes, 30);
        assert_eq!(config.gate.hard_timeout_minutes, 60);
        assert_eq!(
            config.gate.post_review_on,
            vec!["opened".to_owned(), "ready_for_review".to_owned()]
        );
        assert!(!config.gate.blocking.required_proof_unproven);
        assert!(!config.gate.blocking.tool_gate_missing_evidence);
        assert_eq!(
            config.review_body.summary_only_body,
            SummaryOnlyBodyPolicy::Suppress,
            "consumer example must document the conservative suppress default"
        );

        let ripr = config
            .tools
            .get("ripr")
            .ok_or_else(|| anyhow::anyhow!("example config missing ripr tool"))?;
        let ripr_gate = ripr
            .gate
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("example config missing ripr gate policy"))?;
        assert_eq!(ripr_gate.scope.as_deref(), Some("on-diff"));
        // The EXAMPLE config demonstrates the recommended strict-zero posture
        // for consumers (0), which is deliberately distinct from this repo's
        // own temporary epic ceiling (.ub-review.toml = 200 for #678 Orders
        // 5-9; see policy/allow.toml#ripr-epic-ceiling-678). Consumers should
        // adopt strict zero, not the epic ceiling.
        assert_eq!(ripr_gate.max_new_unsuppressed, Some(0));
        Ok(())
    }

    #[test]
    fn gate_config_rejects_unknown_fields() {
        let top_level = toml::from_str::<Config>(
            r#"
[gate]
target_minutez = 30
"#,
        );
        assert!(top_level.is_err());

        let tool_gate = toml::from_str::<Config>(
            r#"
[tools.ripr.gate]
max_new_unsuppressed_findings = 0
"#,
        );
        assert!(tool_gate.is_err());
    }

    #[test]
    fn no_standalone_approval_lines_in_generated_templates() {
        for text in [
            NO_LGTM_POSTURE,
            include_str!("../templates/no-lgtm.md"),
            include_str!("../templates/bun/lane-prompt.md"),
        ] {
            assert!(!has_standalone_approval_line(text));
        }
    }

    #[test]
    fn events_ndjson_appends_across_reopen() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("events.ndjson");
        let log = EventLog::open(&path)?;
        log.append("first", serde_json::json!({"n": 1}))?;
        drop(log);
        let log = EventLog::open(&path)?;
        log.append("second", serde_json::json!({"n": 2}))?;

        let text = fs::read_to_string(path)?;
        assert_eq!(text.lines().count(), 2);
        assert!(text.lines().any(|line| line.contains("\"kind\":\"first\"")));
        assert!(
            text.lines()
                .any(|line| line.contains("\"kind\":\"second\""))
        );
        Ok(())
    }

    #[test]
    fn running_summary_uses_missing_evidence_wording() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let ripr = sensor_plan("ripr", "ripr", true);
        let unsafe_review = sensor_plan("unsafe-review", "unsafe-review", true);
        write_sensor_status(
            &out,
            &ripr,
            SensorStatusWrite {
                status: "missing",
                argv: &["ripr".to_owned(), "first-pr".to_owned()],
                duration_ms: 0,
                reason: "command not found",
                exit_code: None,
                timed_out: false,
            },
        )?;
        write_sensor_status(
            &out,
            &unsafe_review,
            SensorStatusWrite {
                status: "missing",
                argv: &["unsafe-review".to_owned(), "first-pr".to_owned()],
                duration_ms: 0,
                reason: "command not found",
                exit_code: None,
                timed_out: false,
            },
        )?;
        let plan = test_plan(vec![ripr, unsafe_review]);
        let diff = test_diff();

        let summary = render_summary(&out, &plan, &diff)?;

        assert!(summary.contains("- ripr not installed; Rust test-oracle packet unavailable."));
        assert!(summary.contains(
            "- unsafe-review not installed; unsafe/native reviewability packet unavailable."
        ));
        assert!(!summary.contains("No ripr findings"));
        Ok(())
    }

    #[test]
    fn running_summary_renders_model_receipt_status() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        fs::create_dir_all(out.join("review"))?;
        fs::write(
            out.join("review/review.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "model_mode": "auto",
                "provider_policy": "minimax-only",
                "lane_width": 10,
                "provider_preflights": [
                    {
                        "provider": "minimax",
                        "model": "MiniMax-M3",
                        "endpoint_kind": "openai-chat",
                        "status": "missing_key",
                        "reason": "UB_REVIEW_MINIMAX_API_KEY not provided; provider unavailable",
                        "duration_ms": null,
                        "http_status": null,
                        "response_shape": null
                    }
                ],
                "model_lanes": [
                    {
                        "lane": "ub-memory-lifetime",
                        "provider": "minimax",
                        "model": "MiniMax-M3",
                        "endpoint_kind": "openai-chat",
                        "status": "missing_key",
                        "reason": "UB_REVIEW_MINIMAX_API_KEY not provided; minimax lane output unavailable",
                        "duration_ms": null,
                        "http_status": null,
                        "response_shape": null,
                        "fallback_from": null
                    }
                ]
            }))?,
        )?;

        let summary = render_summary(&out, &test_plan(Vec::new()), &test_diff())?;

        assert!(summary.contains("## Provider preflights"));
        assert!(summary.contains("- Provider policy: `minimax-only`"));
        assert!(summary.contains("## Model lane status"));
        assert!(summary.contains("`ub-memory-lifetime`"));
        assert!(summary.contains("## Missing or failed model evidence"));
        assert!(summary.contains("Provider preflight `minimax` model `MiniMax-M3`"));
        assert!(summary.contains("Lane `ub-memory-lifetime` via `minimax` model `MiniMax-M3`"));
        assert!(!summary.contains("No planned model evidence is currently missing or failed."));
        assert!(!has_standalone_approval_line(&summary));
        Ok(())
    }

    #[test]
    fn skipped_out_of_scope_sensors_are_not_missing_review_evidence() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let planned_dry_run = sensor_plan("tokmd", "tokmd", true);
        let trigger_skipped = sensor_plan("ripr", "ripr", false);
        let disabled = sensor_plan("semgrep", "semgrep", false);
        let heavy = sensor_plan("miri", "cargo", false);

        write_sensor_status(
            &out,
            &planned_dry_run,
            SensorStatusWrite {
                status: "skipped",
                argv: &["tokmd".to_owned()],
                duration_ms: 0,
                reason: "dry-run; sensor not executed",
                exit_code: None,
                timed_out: false,
            },
        )?;
        write_sensor_status(
            &out,
            &trigger_skipped,
            SensorStatusWrite {
                status: "skipped",
                argv: &["ripr".to_owned()],
                duration_ms: 0,
                reason: "trigger did not match this diff",
                exit_code: None,
                timed_out: false,
            },
        )?;
        write_sensor_status(
            &out,
            &disabled,
            SensorStatusWrite {
                status: "skipped",
                argv: &["semgrep".to_owned()],
                duration_ms: 0,
                reason: "disabled by config",
                exit_code: None,
                timed_out: false,
            },
        )?;
        write_sensor_status(
            &out,
            &heavy,
            SensorStatusWrite {
                status: "skipped",
                argv: &["cargo".to_owned(), "miri".to_owned()],
                duration_ms: 0,
                reason: "heavy/manual witness requires --allow-heavy",
                exit_code: None,
                timed_out: false,
            },
        )?;
        let plan = test_plan(vec![planned_dry_run, trigger_skipped, disabled, heavy]);

        let issues = collect_sensor_evidence_issues(&out, &plan);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].sensor, "tokmd");
        assert_eq!(issues[0].status, "skipped");
        assert_eq!(issues[0].reason, "dry-run; sensor not executed");
        Ok(())
    }

    #[test]
    fn skipped_required_sensor_is_missing_review_evidence() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let mut required_actionlint = sensor_plan("actionlint", "actionlint", false);
        required_actionlint.required = true;
        required_actionlint.reason = "disabled by config".to_owned();
        let mut trigger_skipped = sensor_plan("ripr", "ripr", false);
        trigger_skipped.reason = "trigger did not match this diff".to_owned();

        write_sensor_status(
            &out,
            &required_actionlint,
            SensorStatusWrite {
                status: "skipped",
                argv: &["actionlint".to_owned()],
                duration_ms: 0,
                reason: "disabled by config",
                exit_code: None,
                timed_out: false,
            },
        )?;
        let required_status: serde_json::Value = serde_json::from_slice(&fs::read(
            out.join("sensors/actionlint/ub-review-sensor-status.json"),
        )?)?;
        assert_eq!(required_status["required"], true);
        write_sensor_status(
            &out,
            &trigger_skipped,
            SensorStatusWrite {
                status: "skipped",
                argv: &["ripr".to_owned()],
                duration_ms: 0,
                reason: "trigger did not match this diff",
                exit_code: None,
                timed_out: false,
            },
        )?;
        let plan = test_plan(vec![required_actionlint, trigger_skipped]);

        let issues = collect_sensor_evidence_issues(&out, &plan);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].sensor, "actionlint");
        assert_eq!(issues[0].status, "skipped");
        assert_eq!(issues[0].reason, "disabled by config");
        Ok(())
    }

    #[test]
    fn unsafe_review_ok_without_gate_artifact_is_sensor_artifact_gap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let mut sensor = sensor_plan("unsafe-review", "unsafe-review", true);
        sensor.required = true;
        write_sensor_status(
            &out,
            &sensor,
            SensorStatusWrite {
                status: "ok",
                argv: &["unsafe-review".to_owned(), "first-pr".to_owned()],
                duration_ms: 10,
                reason: "completed",
                exit_code: Some(0),
                timed_out: false,
            },
        )?;
        let plan = test_plan(vec![sensor.clone()]);

        let issues = collect_sensor_evidence_issues(&out, &plan);

        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].sensor, "unsafe-review");
        assert_eq!(issues[0].status, "artifact-gap");
        assert_eq!(
            issues[0].reason,
            "unsafe-review-gate.json absent; structured evidence unavailable"
        );

        let gate_dir = out
            .join("sensors")
            .join("unsafe-review")
            .join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&gate_dir)?;
        fs::write(
            gate_dir.join("unsafe-review-gate.json"),
            r#"{"schema_version":"unsafe-review-gate/v1","status":"advisory"}"#,
        )?;

        let issues = collect_sensor_evidence_issues(&out, &plan);
        assert!(
            issues.is_empty(),
            "valid structured unsafe-review evidence should clear the gap: {issues:?}"
        );
        Ok(())
    }

    #[test]
    fn summary_only_guard_rejects_unsupported_model_findings() -> Result<()> {
        let lane = default_lanes()
            .into_iter()
            .find(|lane| lane.id == "tests")
            .ok_or_else(|| anyhow::anyhow!("tests lane missing"))?;

        let accepted = validate_summary_only_candidate(
            &lane,
            ModelCandidateFinding {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "The test reaches the helper but does not reveal the changed behavior."
                    .to_owned(),
                evidence: "ripr summary excerpt".to_owned(),
            },
        );
        assert_eq!(accepted.severity, "medium");
        assert_eq!(accepted.confidence, "medium-high");
        assert_eq!(accepted.evidence, "ripr summary excerpt");

        let rejected = validate_summary_only_candidate(
            &lane,
            ModelCandidateFinding {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: " ".to_owned(),
                evidence: "".to_owned(),
            },
        );
        assert_eq!(rejected.severity, "low");
        assert_eq!(rejected.confidence, "medium");
        assert!(rejected.reason.contains("reason_present=false"));
        assert!(rejected.reason.contains("evidence_present=false"));
        assert_eq!(rejected.evidence, "model summary-only candidate guardrail");
        Ok(())
    }

    #[test]
    fn sibling_source_route_prompt_requires_scan_boundaries() -> Result<()> {
        let lane = default_lanes()
            .into_iter()
            .find(|lane| lane.id == "source-route")
            .ok_or_else(|| anyhow::anyhow!("source-route lane missing"))?;

        let guidance = super::lane_specific_prompt_guidance(&lane);

        assert!(guidance.contains("no-match scan"));
        assert!(guidance.contains("not proof that no sibling paths exist"));
        assert!(guidance.contains("checked pattern/scope"));
        assert!(guidance.contains("unscanned variants"));
        Ok(())
    }

    #[test]
    fn sibling_summary_completeness_claim_becomes_verification_observation() -> Result<()> {
        let lane = lane_plan("sibling-paths");
        let output = LaneModelOutput {
            summary: Some(
                "No analogous sibling panic paths were found, so the fix is correctly scoped and need not be broadened."
                    .to_owned(),
            ),
            inline_comments: Vec::new(),
            candidate_findings: Vec::new(),
            summary_only_findings: Vec::new(),
            observations: Vec::new(),
            failed_objections: Vec::new(),
            proof_requests: Vec::new(),
            proof_intents: Vec::new(),
            issue_candidates: Vec::new(),
            degraded: false,
        };
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();

        apply_model_output(
            &lane,
            output,
            &BTreeSet::new(),
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert!(inline_comments.is_empty());
        assert!(summary_only_findings.is_empty());
        assert!(proof_requests.is_empty());
        assert_eq!(observations.len(), 1);
        let observation = &observations[0];
        assert_eq!(observation.question, "sibling-path-coverage");
        assert_eq!(observation.kind, "source-route-gap");
        assert_eq!(observation.status, "open");
        assert_eq!(observation.severity, "medium");
        assert_eq!(observation.confidence, "high");
        assert_eq!(
            observation.dedupe_key,
            super::SIBLING_COMPLETENESS_OVERCLAIM_DEDUPE_KEY
        );
        assert!(
            observation
                .evidence
                .iter()
                .any(|item| item.contains("narrow no-match scans"))
        );

        let pr_body = render_review_body(
            "shared-context-test",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[],
            &[],
            &[],
            &[],
            &observations,
            &[],
            16_384,
            ReviewBodyAudience::PullRequest,
        );

        assert!(pr_body.contains("## Decision"));
        assert!(pr_body.contains("## Verification questions"));
        assert!(pr_body.contains("Check sibling-path scan coverage"));
        assert!(!pr_body.contains("## Refuted"));
        assert!(!pr_body.contains("correctly scoped"));
        assert!(!pr_body.contains("No analogous"));
        Ok(())
    }

    #[test]
    fn sibling_failed_objection_completeness_claim_is_not_refuted() {
        let lane = lane_plan("source-route");

        let observation = validate_failed_objection(
            &lane,
            ModelFailedObjection {
                claim: "No analogous sibling panic paths were found.".to_owned(),
                reason: "The fix is correctly scoped and need not be broadened.".to_owned(),
                confidence: Some("high".to_owned()),
                kind: Some("resolved-check".to_owned()),
                evidence: vec!["single-pattern write/dispose scan".to_owned()],
            },
            0,
        );

        assert_eq!(observation.question, "sibling-path-coverage");
        assert_eq!(observation.kind, "source-route-gap");
        assert_eq!(observation.status, "open");
        assert_ne!(observation.status, "refuted");
        assert_eq!(
            observation.dedupe_key,
            super::SIBLING_COMPLETENESS_OVERCLAIM_DEDUPE_KEY
        );
    }

    #[test]
    fn scoped_sibling_scan_limit_remains_coverage_limited() {
        let lane = lane_plan("sibling-paths");

        let observation = validate_model_observation(
            &lane,
            ModelCandidateObservation {
                claim: "Checked write/dispose only; did not scan ptr/toBuffer or to_int64 paths."
                    .to_owned(),
                question: Some("sibling-paths".to_owned()),
                kind: Some("source-route-gap".to_owned()),
                status: Some("open".to_owned()),
                severity: Some("medium".to_owned()),
                confidence: Some("medium".to_owned()),
                path: None,
                line: None,
                evidence: vec!["coverage-limited sibling scan".to_owned()],
                dedupe_key: Some("coverage-limited-sibling-scan".to_owned()),
            },
            0,
        );

        assert_eq!(
            observation.claim,
            "Checked write/dispose only; did not scan ptr/toBuffer or to_int64 paths."
        );
        assert_eq!(observation.kind, "source-route-gap");
        assert_eq!(observation.status, "open");
        assert_eq!(observation.dedupe_key, "coverage-limited-sibling-scan");
        assert_eq!(observation.source, "model-observation");
    }

    #[test]
    fn inline_guard_accepts_only_right_side_diff_lines() -> Result<()> {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
";
        let line_map = right_side_diff_lines(patch);
        let lane = default_lanes()
            .into_iter()
            .find(|lane| lane.id == "tests")
            .ok_or_else(|| anyhow::anyhow!("tests lane missing"))?;
        let accepted = validate_inline_candidate(
            &lane,
            ModelCandidateComment {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 2,
                body: "This reaches the helper but does not assert the changed boundary."
                    .to_owned(),
                evidence: "diff hunk".to_owned(),
                suggestion: None,
            },
            &line_map,
        )
        .map_err(|finding| anyhow::anyhow!("unexpected rejection: {}", finding.reason))?;
        assert_eq!(accepted.side, "RIGHT");
        assert!(accepted.body.starts_with("[tests]"));
        assert!(accepted.suggestion.is_none());

        let model_suggestion = validate_inline_candidate(
            &lane,
            ModelCandidateComment {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 2,
                body: "[tests] model-proposed edit must remain advisory".to_owned(),
                evidence: "diff hunk".to_owned(),
                suggestion: Some("assert!(proved);".to_owned()),
            },
            &line_map,
        )
        .map_err(|finding| anyhow::anyhow!("unexpected rejection: {}", finding.reason))?;
        assert!(
            model_suggestion.suggestion.is_none(),
            "non-unsafe-review lanes must not smuggle suggestion blocks"
        );

        let rejected = validate_inline_candidate(
            &lane,
            ModelCandidateComment {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 50,
                body: "[tests] guessed stale line".to_owned(),
                evidence: "none".to_owned(),
                suggestion: None,
            },
            &line_map,
        );
        assert!(rejected.is_err());
        let missing_evidence = validate_inline_candidate(
            &lane,
            ModelCandidateComment {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 2,
                body: "[tests] line-valid but unsupported claim".to_owned(),
                evidence: "".to_owned(),
                suggestion: None,
            },
            &line_map,
        );
        assert!(
            missing_evidence
                .is_err_and(|finding| { finding.reason.contains("evidence_present=false") })
        );

        let empty_body = validate_inline_candidate(
            &lane,
            ModelCandidateComment {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 2,
                body: "   ".to_owned(),
                evidence: "diff hunk".to_owned(),
                suggestion: None,
            },
            &line_map,
        );
        assert!(empty_body.is_err_and(|finding| { finding.reason.contains("body_present=false") }));
        Ok(())
    }

    #[test]
    fn candidate_only_lanes_cannot_emit_inline_comments() {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
";
        let line_map = right_side_diff_lines(patch);
        let lane = model_lane(
            "source-route-fast",
            "Fast source-route candidate generation",
            &["tokmd", "ast-grep"],
            "Generate candidate-only public API route and helper caller gaps.",
        );
        let output = LaneModelOutput {
            summary: None,
            inline_comments: vec![ModelCandidateComment {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 2,
                body: "[source-route-fast] This is line-valid but must stay candidate-only."
                    .to_owned(),
                evidence: "diff hunk".to_owned(),
                suggestion: None,
            }],
            candidate_findings: Vec::new(),
            summary_only_findings: Vec::new(),
            observations: Vec::new(),
            failed_objections: Vec::new(),
            proof_requests: Vec::new(),
            proof_intents: Vec::new(),
            issue_candidates: Vec::new(),
            degraded: false,
        };
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();

        apply_model_output(
            &lane,
            output,
            &line_map,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut model_observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert!(inline_comments.is_empty());
        assert_eq!(summary_only_findings.len(), 1);
        assert_eq!(summary_only_findings[0].lane, "source-route-fast");
        assert!(
            summary_only_findings[0]
                .reason
                .contains("candidate-only lane emitted inline candidate")
        );
        assert_eq!(summary_only_findings[0].evidence, "diff hunk");
    }

    #[test]
    fn lane_output_split_accepts_observations_candidates_and_proof_requests() -> Result<()> {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
";
        let line_map = right_side_diff_lines(patch);
        let lane = model_lane(
            "tests-oracle",
            "Test oracle review",
            &["tokmd", "ripr"],
            "Check test proof.",
        );
        let json = r#"{
  "summary": "Checked red/green and route proof.",
  "observations": [
    {
      "claim": "The new test needs a witnessed old-main red run.",
      "question": "red-green",
      "kind": "missing-evidence",
      "status": "open",
      "severity": "medium",
      "confidence": "high",
      "evidence": ["PR body claims old code fails"],
      "dedupe_key": "markdown-red-green-witness"
    }
  ],
  "candidate_findings": [
    {
      "severity": "medium",
      "confidence": "medium-high",
      "path": "src/lib.rs",
      "line": 2,
      "body": "[tests-oracle] The changed pointer path needs a test oracle.",
      "evidence": "diff hunk"
    }
  ],
  "failed_objections": [
    {
      "claim": "Box::from(slice) can return None on allocation failure",
      "reason": "false premise: allocation failure does not return None",
      "confidence": "high",
      "kind": "false-premise",
      "evidence": ["Rust allocation semantics"]
    }
  ],
  "proof_requests": [
    {
      "command": "bun test test/js/bun/md/md-edge-cases.test.ts",
      "reason": "Need a focused green witness on HEAD",
      "cost": "focused-test",
      "timeout_sec": 300,
      "required": false
    }
  ]
}"#;
        let output: LaneModelOutput = serde_json::from_str(json)?;
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();

        apply_model_output(
            &lane,
            output,
            &line_map,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert_eq!(inline_comments.len(), 1);
        assert_eq!(inline_comments[0].lane, "tests-oracle");
        assert_eq!(summary_only_findings.len(), 1);
        assert_eq!(observations.len(), 2);
        assert!(observations.iter().any(|observation| {
            observation.kind == "missing-evidence"
                && observation.dedupe_key == "markdown-red-green-witness"
                && observation.source == "model-observation"
        }));
        assert!(observations.iter().any(|observation| {
            observation.kind == "false-premise"
                && observation.status == "refuted"
                && observation.source == "model-failed-objection"
        }));
        assert_eq!(proof_requests.len(), 1);
        assert_eq!(proof_requests[0].schema, "ub-review.proof_request.v1");
        assert_eq!(proof_requests[0].status, "requested");
        assert_eq!(
            proof_requests[0].requested_by,
            vec!["tests-oracle".to_owned()]
        );

        let temp = tempfile::tempdir()?;
        write_proof_request_artifacts(
            temp.path(),
            &test_diff(),
            &Profile::default(),
            &proof_requests,
            &[] as &[ProofReceipt],
            &[] as &[ResourceLease],
        )?;
        let proof_json: Vec<super::ProofRequest> =
            serde_json::from_slice(&fs::read(temp.path().join("review/proof_requests.json"))?)?;
        let proof_groups: Vec<ProofRequestGroup> = serde_json::from_slice(&fs::read(
            temp.path().join("review/proof_request_groups.json"),
        )?)?;
        let proof_request_file: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path()
                .join("proof_requests")
                .join(format!("{}.json", proof_requests[0].id)),
        )?)?;
        let proof_plan = fs::read_to_string(temp.path().join("review/proof_plan.md"))?;
        let proof_ndjson = fs::read_to_string(temp.path().join("proof_requests.ndjson"))?;
        assert_eq!(proof_json.len(), 1);
        assert_eq!(proof_request_file, serde_json::to_value(&proof_json[0])?);
        assert_eq!(proof_groups.len(), 1);
        assert_eq!(proof_groups[0].duplicate_count, 1);
        assert!(proof_plan.contains("## Focused proof plan"));
        assert!(proof_plan.contains("mode=`red-green`"));
        assert!(proof_plan.contains("base+tests=`cwd=target/ub-review/proof-worktrees/base-plus-tests USE_SYSTEM_BUN=1 bun test test/js/bun/md/md-edge-cases.test.ts`"));
        assert!(proof_ndjson.contains("bun test test/js/bun/md/md-edge-cases.test.ts"));
        Ok(())
    }

    #[test]
    fn lane_output_split_accepts_scalar_evidence_strings() -> Result<()> {
        let lane = model_lane(
            "source-route",
            "Source route review",
            &["tokmd", "ast-grep"],
            "Check public API route proof.",
        );
        let json = r#"{
  "observations": [
    {
      "claim": "FileHandle.write route still needs proof.",
      "kind": "source-route-gap",
      "status": "open",
      "evidence": "route excerpt was scalar text"
    }
  ],
  "failed_objections": [
    {
      "claim": "writev uses the patched scalar branch",
      "reason": "sibling route still calls a separate helper",
      "evidence": "sibling-path scan was scalar text"
    }
  ]
}"#;
        let output: LaneModelOutput = serde_json::from_str(json)?;
        assert!(output.degraded);
        assert_eq!(
            output.observations[0].evidence,
            vec!["route excerpt was scalar text".to_owned()]
        );
        assert_eq!(
            output.failed_objections[0].evidence,
            vec!["sibling-path scan was scalar text".to_owned()]
        );

        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        apply_model_output(
            &lane,
            output,
            &BTreeSet::new(),
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert_eq!(observations.len(), 2);
        assert!(observations.iter().any(|observation| {
            observation.source == "model-observation"
                && observation.evidence == vec!["route excerpt was scalar text".to_owned()]
        }));
        assert!(observations.iter().any(|observation| {
            observation.source == "model-failed-objection"
                && observation.evidence == vec!["sibling-path scan was scalar text".to_owned()]
        }));
        Ok(())
    }

    #[test]
    fn lane_output_accepts_typed_proof_intents_without_command() -> Result<()> {
        let lane = model_lane(
            "tests-oracle",
            "Test oracle review",
            &["tokmd", "ripr"],
            "Check test proof.",
        );
        let output: LaneModelOutput = serde_json::from_str(
            r#"{
  "proof_intents": [
    {
      "claim_id": "parser:list-item-postfix",
      "question": "Does the second declaration preserve a direct subscript?",
      "expected_answer_shape": "AST contains indexed expression",
      "proof_kind": "focused-test",
      "target": "parser_list_item_postfix",
      "estimated_value": "high"
    },
    {
      "claim_id": "unsafe-intent",
      "question": "must be rejected",
      "expected_answer_shape": "no execution",
      "proof_kind": "focused-test",
      "target": "cargo;rm",
      "estimated_value": "high"
    }
  ]
}"#,
        )?;
        anyhow::ensure!(output.proof_requests.is_empty());
        anyhow::ensure!(output.proof_intents.len() == 2);

        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut proof_intents = Vec::new();
        let mut issue_candidates = Vec::new();
        apply_model_output(
            &lane,
            output,
            &BTreeSet::new(),
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut proof_intents,
                issue_candidates: &mut issue_candidates,
            },
        );

        anyhow::ensure!(proof_requests.is_empty());
        anyhow::ensure!(proof_intents.len() == 1);
        anyhow::ensure!(proof_intents[0].claim_id == "parser:list-item-postfix");
        anyhow::ensure!(proof_intents[0].target == "parser_list_item_postfix");
        let artifact = serde_json::to_value(&proof_intents[0])?;
        anyhow::ensure!(artifact.get("command").is_none());
        anyhow::ensure!(artifact["proof_kind"] == "focused-test");
        Ok(())
    }

    #[test]
    fn lane_output_split_degrades_scalar_sequence_fields() -> Result<()> {
        let lane = model_lane(
            "tests-oracle",
            "Test oracle review",
            &["tokmd", "ripr"],
            "Check test proof.",
        );
        let json = r#"{
  "observations": "The added regression test still needs base+tests red/green proof.",
  "candidate_findings": "Malformed inline finding text should not erase the whole lane."
}"#;
        let (output, degraded) = super::parse_lane_model_output_or_degrade(
            json,
            Path::new("target/ub-review/review/model/tests-oracle/content.json"),
        )?;
        assert!(degraded);
        assert!(output.degraded);
        assert!(output.candidate_findings.is_empty());
        assert_eq!(output.observations.len(), 2);

        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        apply_model_output(
            &lane,
            output,
            &BTreeSet::new(),
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert!(inline_comments.is_empty());
        assert!(summary_only_findings.is_empty());
        assert_eq!(observations.len(), 2);
        assert!(observations.iter().any(|observation| {
            observation.source == "model-observation"
                && observation.kind == "missing-evidence"
                && observation.question == "lane-output-shape"
                && observation.dedupe_key == "lane-output-shape-observations"
                && observation.claim.contains("base+tests red/green proof")
        }));
        assert!(observations.iter().any(|observation| {
            observation.source == "model-observation"
                && observation.kind == "missing-evidence"
                && observation.dedupe_key == "lane-output-shape-candidate_findings"
                && observation
                    .evidence
                    .iter()
                    .any(|item| item.contains("Malformed inline finding text"))
        }));
        Ok(())
    }

    #[test]
    fn lane_output_split_degrades_contentful_malformed_output() -> Result<()> {
        let raw = "args.buffer = StringOrBuffer::EncodedSlice(ZigStringSlice::init_owned(owned)); runs synchronously pre-schedule";
        let parse_path = Path::new("target/ub-review/review/model/ub-worker-handoff/content.json");

        let (output, degraded) = super::parse_lane_model_output_or_degrade(raw, parse_path)?;

        assert!(degraded);
        assert!(output.degraded);
        assert!(output.inline_comments.is_empty());
        assert!(output.candidate_findings.is_empty());
        assert!(output.summary_only_findings.is_empty());
        assert_eq!(output.observations.len(), 1);
        assert_eq!(
            output.observations[0].question.as_deref(),
            Some("lane-output-shape")
        );
        assert_eq!(
            output.observations[0].kind.as_deref(),
            Some("missing-evidence")
        );
        assert!(output.observations[0].claim.contains("EncodedSlice"));
        assert!(
            output.observations[0]
                .evidence
                .iter()
                .any(|item| item.contains("content.json"))
        );
        Ok(())
    }

    #[test]
    fn lane_output_split_degrades_contentful_schema_wrong_json() -> Result<()> {
        let raw = r#"{"findings":"EncodedSlice route excerpt survived as text"}"#;
        let parse_path = Path::new("target/ub-review/review/model/ub-worker-handoff/content.json");

        let (output, degraded) = super::parse_lane_model_output_or_degrade(raw, parse_path)?;

        assert!(degraded);
        assert!(output.degraded);
        assert!(output.inline_comments.is_empty());
        assert!(output.candidate_findings.is_empty());
        assert!(output.summary_only_findings.is_empty());
        assert_eq!(output.observations.len(), 1);
        assert!(
            output.observations[0]
                .claim
                .contains("EncodedSlice route excerpt")
        );
        assert!(
            output.observations[0]
                .evidence
                .iter()
                .any(|item| item.contains("recognized lane evidence"))
        );
        Ok(())
    }

    #[test]
    fn lane_output_split_rejects_empty_unusable_output() -> Result<()> {
        let parse_path = Path::new("target/ub-review/review/model/ub-active-view/content.json");

        for raw in ["{}", r#"{"observations": ""}"#] {
            let err = super::parse_lane_model_output_or_degrade(raw, parse_path)
                .err()
                .ok_or_else(|| anyhow::anyhow!("empty lane output unexpectedly parsed"))?;
            assert_eq!(super::classify_model_error(&err), "invalid_json");
            assert!(format!("{err:#}").contains("empty or unusable"));
        }
        Ok(())
    }

    #[test]
    fn degraded_model_lane_is_attempted_but_not_missing_evidence() {
        let mut degraded = model_lane_receipt("ub-worker-handoff", "degraded");
        degraded.reason = "contentful lane output was preserved as degraded evidence".to_owned();

        assert!(super::model_call_attempted_status("degraded"));
        assert!(!super::is_model_receipt_evidence_issue(&degraded));
    }

    #[test]
    fn unreceipted_proof_request_tasks_skip_already_receipted_seeded_request() -> Result<()> {
        let proof_requests = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-policy-001".to_owned(),
            lane: "intelligent-ci-policy".to_owned(),
            requested_by: vec![
                "intelligent-ci-policy".to_owned(),
                "proof-policy:required-smoke".to_owned(),
            ],
            command: "cargo test --locked required_proof_smoke".to_owned(),
            reason: "Required Rust focused smoke for intelligent CI.".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: true,
            status: "requested".to_owned(),
        }];

        assert!(super::has_unreceipted_proof_request_tasks(
            &proof_requests,
            &[]
        ));

        let task = super::focused_test_candidates_from_requests(&proof_requests)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("focused request should produce a task"))?;
        let mut receipt = test_proof_receipt("head_passed", "passed");
        receipt.id = task.id;
        receipt.requested_by = task.requested_by;
        receipt.request_ids = task.request_ids;

        assert!(!super::has_unreceipted_proof_request_tasks(
            &proof_requests,
            &[receipt]
        ));
        Ok(())
    }

    #[test]
    fn proof_broker_v0_executes_allowlisted_request_as_red_green_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "USE_SYSTEM_BUN=1 bun test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'"
                    .to_owned(),
                reason: "Run the requested focused Bun proof.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 120,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-opposition-001".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command:
                    "bun bd test test/js/bun/ffi/ffi.test.js --test-name-pattern \"ffi toBuffer bad free\""
                        .to_owned(),
                reason: "Confirm the same requested focused Bun proof.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 180,
                required: true,
                status: "requested".to_owned(),
            },
        ];
        let tasks = focused_test_tasks_from_diff(
            &diff,
            &proof_requests,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].mode, super::FocusedProofMode::RedGreen);
        assert_eq!(tasks[0].file, "test/js/bun/ffi/ffi.test.js");
        assert_eq!(tasks[0].test_name.as_deref(), Some("ffi toBuffer bad free"));
        assert_eq!(tasks[0].timeout_sec, Some(180));
        assert_eq!(tasks[0].requested_by.len(), 2);
        assert!(
            tasks[0]
                .requested_by
                .iter()
                .any(|lane| lane == "tests-oracle")
        );
        assert!(
            tasks[0]
                .requested_by
                .iter()
                .any(|lane| lane == "opposition")
        );
        assert_eq!(tasks[0].request_ids.len(), 2);
        assert!(
            tasks[0]
                .request_ids
                .iter()
                .any(|request_id| request_id == "proof-tests-001")
        );
        assert!(
            tasks[0]
                .request_ids
                .iter()
                .any(|request_id| request_id == "proof-opposition-001")
        );

        let args = test_run_args(out.clone());
        let mut commands = Vec::<String>::new();
        let prepared_base_root = base_root.clone();
        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, timeout, stdout, stderr| {
                commands.push(super::test_parse::command_display_with_env(env, argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                assert_eq!(timeout, 180);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 21,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(
            commands,
            vec![
                "bun bd test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'",
                "USE_SYSTEM_BUN=1 bun test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'",
            ]
        );
        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        let receipt = &proof_result.proof_receipts[0];
        assert_eq!(receipt.kind, "focused-red-green");
        assert_eq!(receipt.test_patch_mode, "base-plus-tests");
        assert_eq!(receipt.result, "discriminating");
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].side, "head");
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].side, "base-plus-tests");
        assert_eq!(receipt.commands[1].status, "failed");
        assert!(out.join(&receipt.commands[0].stdout).exists());
        assert!(out.join(&receipt.commands[1].stdout).exists());
        let lease = &proof_result.resource_leases[0];
        assert_eq!(lease.status, "granted");
        assert_eq!(lease.timeout_sec, 360);
        assert_eq!(lease.worktree, Some("base-plus-tests".to_owned()));
        assert!(lease.command.as_deref().is_some_and(
            |command| command.contains("head: cwd=") && command.contains("base+tests:")
        ));
        Ok(())
    }

    #[test]
    fn proof_broker_v0_executes_allowlisted_cargo_test_request_as_red_green_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let command =
            "cargo test --locked -p ub-review cargo_focused_red_green -- --exact".to_owned();
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: command.clone(),
                reason: "Run the requested focused Cargo proof.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-opposition-001".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command: command.clone(),
                reason: "Confirm the same requested focused Cargo proof.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: true,
                status: "requested".to_owned(),
            },
        ];
        let tasks = super::focused_test_candidates_from_requests(&proof_requests);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].mode, super::FocusedProofMode::RedGreen);
        assert_eq!(tasks[0].file, "cargo-package:ub-review");
        assert_eq!(
            tasks[0].test_name.as_deref(),
            Some("cargo_focused_red_green")
        );
        assert!(tasks[0].command_specs.is_some());
        assert_eq!(tasks[0].requested_by.len(), 2);
        assert_eq!(tasks[0].request_ids.len(), 2);

        let args = test_run_args(out.clone());
        let mut commands = Vec::<String>::new();
        let prepared_base_root = base_root.clone();
        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, timeout, stdout, stderr| {
                commands.push(super::test_parse::command_display_with_env(env, argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert!(env.is_empty());
                assert_eq!(timeout, 300);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 21,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(commands, vec![command.clone(), command]);
        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        let receipt = &proof_result.proof_receipts[0];
        assert_eq!(receipt.kind, "focused-red-green");
        assert_eq!(receipt.test_patch_mode, "base-plus-tests");
        assert_eq!(receipt.result, "discriminating");
        assert_eq!(
            receipt.requested_by,
            vec!["tests-oracle".to_owned(), "opposition".to_owned()]
        );
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].side, "head");
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].side, "base-plus-tests");
        assert_eq!(receipt.commands[1].status, "failed");
        assert!(out.join(&receipt.commands[0].stdout).exists());
        assert!(out.join(&receipt.commands[1].stdout).exists());
        let lease = &proof_result.resource_leases[0];
        assert_eq!(lease.kind, "focused-test");
        assert_eq!(lease.status, "granted");
        assert_eq!(lease.timeout_sec, 600);
        assert_eq!(lease.worktree, Some("base-plus-tests".to_owned()));
        Ok(())
    }

    #[test]
    fn proof_cost_is_normalized_to_known_broker_classes() {
        assert_eq!(
            classify_proof_cost(Some("focused-test"), "bun test test/js/node/fs/fs.test.ts"),
            "focused-test"
        );
        assert_eq!(
            classify_proof_cost(
                None,
                "USE_SYSTEM_BUN=1 bun test test/js/node/fs/fs.test.ts -t route"
            ),
            "focused-test"
        );
        assert_eq!(
            classify_proof_cost(
                Some("slow integration test"),
                "bun test test/js/node/fs/fs.test.ts"
            ),
            "focused-test"
        );
        assert_eq!(
            classify_proof_cost(Some("compile"), "cargo build --workspace"),
            "focused-build"
        );
        assert_eq!(
            classify_proof_cost(Some("expensive mutation"), "cargo mutants"),
            "manual"
        );
    }

    #[test]
    fn focused_build_allowlist_brokers_exact_repo_policy_check_only() {
        let spec = super::focused_build_command_spec("cargo xtask policy-check");
        assert!(
            spec.is_some_and(|spec| spec.argv == ["cargo", "xtask", "policy-check"]),
            "the exact policy receipt validation command should be brokered"
        );
        assert_eq!(
            super::proof_request_status("cargo xtask policy-check", "focused-build"),
            "requested"
        );
        for rejected in [
            "cargo xtask",
            "cargo xtask precommit",
            "cargo xtask policy-check --fix",
            "cargo xtask policy-check && rm -rf target",
        ] {
            assert!(
                super::focused_build_command_spec(rejected).is_none(),
                "{rejected} must not be brokered"
            );
        }
    }

    #[test]
    fn intelligent_ci_synthesizes_matching_required_proof_requests() {
        let mut config = Config::default();
        config.proof.required = vec![
            super::RequiredProofPolicy {
                id: "cargo-check".to_owned(),
                languages: vec!["rust".to_owned()],
                diff_classes: vec!["source-ub".to_owned()],
                command: "cargo check --workspace --locked".to_owned(),
                reason: "Required Rust workspace check for intelligent CI.".to_owned(),
                cost: Some("focused-build".to_owned()),
                timeout_sec: 300,
                required: true,
                enabled: true,
            },
            super::RequiredProofPolicy {
                id: "workflow-lint".to_owned(),
                languages: vec!["yaml".to_owned()],
                diff_classes: vec!["workflow/tooling".to_owned()],
                command: "actionlint".to_owned(),
                reason: "Only workflow diffs should request actionlint.".to_owned(),
                cost: Some("focused-build".to_owned()),
                timeout_sec: 120,
                required: true,
                enabled: true,
            },
        ];
        let diff = test_diff();
        let mut args = test_run_args(PathBuf::from("out"));
        args.mode = RunMode::IntelligentCi;

        let requests = super::configured_required_proof_requests(&config, &diff, &args, 0);

        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].lane, "intelligent-ci-policy");
        assert_eq!(
            requests[0].requested_by,
            vec![
                "intelligent-ci-policy".to_owned(),
                "proof-policy:cargo-check".to_owned()
            ]
        );
        assert_eq!(requests[0].command, "cargo check --workspace --locked");
        assert_eq!(
            requests[0].reason,
            "Required Rust workspace check for intelligent CI."
        );
        assert_eq!(requests[0].cost, "focused-build");
        assert_eq!(requests[0].status, "requested");
        assert!(requests[0].required);

        let language_mix = super::classify_language_mix(&diff.changed_files);
        let proof_policy = super::resolved_proof_policy_artifact(&config, &diff, &language_mix);
        assert_eq!(
            proof_policy["matched_required"].as_array().map(Vec::len),
            Some(1)
        );
        assert_eq!(proof_policy["matched_required"][0]["id"], "cargo-check");
    }

    #[test]
    fn review_byok_does_not_synthesize_required_proof_requests() {
        let mut config = Config::default();
        config.proof.required = vec![super::RequiredProofPolicy {
            id: "cargo-check".to_owned(),
            languages: vec!["rust".to_owned()],
            diff_classes: vec!["source-ub".to_owned()],
            command: "cargo check --workspace --locked".to_owned(),
            reason: "Required Rust workspace check for intelligent CI.".to_owned(),
            cost: Some("focused-build".to_owned()),
            timeout_sec: 300,
            required: true,
            enabled: true,
        }];
        let diff = test_diff();
        let args = test_run_args(PathBuf::from("out"));

        let requests = super::configured_required_proof_requests(&config, &diff, &args, 0);

        assert!(requests.is_empty());
    }

    #[test]
    fn proof_broker_v0_runs_budgeted_focused_red_green_targets_and_writes_receipts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let patch = "\
diff --git a/test/js/bun/md/md-edge-cases.test.ts b/test/js/bun/md/md-edge-cases.test.ts
index 1111111..2222222 100644
--- a/test/js/bun/md/md-edge-cases.test.ts
+++ b/test/js/bun/md/md-edge-cases.test.ts
@@ -1,2 +1,4 @@
 import { test } from 'bun:test';
+test(\"snapshots resizable ArrayBuffer input\", () => {});
+it('keeps stable bytes after getter reentry', () => {});
";
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["test/js/bun/md/md-edge-cases.test.ts".to_owned()],
            patch: patch.to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        };
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Need green witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-opposition-001".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Confirm the same focused test.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: true,
                status: "requested".to_owned(),
            },
        ];
        let tasks = focused_test_tasks_from_diff(
            &diff,
            &proof_requests,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 1_200,
            },
        );
        assert_eq!(tasks.len(), 2);
        let args = test_run_args(out.clone());
        let mut commands = Vec::<String>::new();
        let prepared_base_root = base_root.clone();
        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 1_200,
            },
            tasks,
            |_root, argv, env, timeout, stdout, stderr| {
                commands.push(command_display(argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: format!("completed with timeout {timeout}s"),
                    duration_ms: 42,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;
        let receipts = proof_result.proof_receipts;
        let resource_leases = proof_result.resource_leases;

        assert_eq!(commands.len(), 4);
        assert_eq!(receipts.len(), 2);
        assert_eq!(resource_leases.len(), 2);
        assert_eq!(resource_leases[0].schema, "ub-review.resource_lease.v1");
        assert_eq!(resource_leases[0].kind, "focused-test");
        assert_eq!(resource_leases[0].consumer, receipts[0].id);
        assert_eq!(resource_leases[0].status, "granted");
        assert_eq!(resource_leases[1].consumer, receipts[1].id);
        assert_eq!(resource_leases[1].status, "granted");
        assert_eq!(receipts[0].schema, "ub-review.proof_receipt.v1");
        assert_eq!(receipts[0].kind, "focused-red-green");
        assert_eq!(receipts[0].test_patch_mode, "base-plus-tests");
        assert_eq!(receipts[0].result, "discriminating");
        assert_eq!(
            receipts[0].requested_by,
            vec!["tests-oracle".to_owned(), "opposition".to_owned()]
        );
        assert_eq!(
            receipts[0].request_ids,
            vec![
                "proof-tests-001".to_owned(),
                "proof-opposition-001".to_owned()
            ]
        );
        assert_eq!(receipts[0].commands[0].status, "passed");
        assert_eq!(receipts[0].commands[1].side, "base-plus-tests");
        assert_eq!(receipts[0].commands[1].status, "failed");
        assert!(out.join(&receipts[0].commands[0].stdout).exists());
        assert!(out.join(&receipts[0].commands[1].stdout).exists());
        assert_eq!(receipts[1].result, "discriminating");
        assert_eq!(receipts[1].commands.len(), 2);
        assert_eq!(receipts[1].commands[0].status, "passed");
        assert_eq!(receipts[1].commands[1].status, "failed");

        write_proof_receipt_artifacts(&out, &receipts)?;
        write_resource_lease_artifacts(&out, &resource_leases)?;
        write_proof_request_artifacts(
            &out,
            &diff,
            &Profile::default(),
            &proof_requests,
            &receipts,
            &resource_leases,
        )?;
        let receipt_json: Vec<ProofReceipt> =
            serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
        let receipt_ndjson = fs::read_to_string(out.join("proof_receipts.ndjson"))?;
        let lease_json: Vec<ResourceLease> =
            serde_json::from_slice(&fs::read(out.join("review/resource_leases.json"))?)?;
        let lease_ndjson = fs::read_to_string(out.join("resource_leases.ndjson"))?;
        let resource_plan = fs::read_to_string(out.join("review/resource_plan.md"))?;
        let proof_plan = fs::read_to_string(out.join("review/proof_plan.md"))?;
        assert_eq!(receipt_json.len(), 2);
        assert_eq!(receipt_ndjson.lines().count(), 2);
        assert_eq!(lease_json.len(), 2);
        assert_eq!(lease_ndjson.lines().count(), 2);
        assert!(resource_plan.contains("# Resource lease plan"));
        assert!(resource_plan.contains("status=`granted`"));
        assert!(!resource_plan.contains("status=`exhausted`"));
        assert!(
            proof_plan.contains("Proof broker v0 executed focused proof under the runtime budget")
        );
        assert!(proof_plan.contains("result=`discriminating`"));
        assert!(!proof_plan.contains("No proof broker commands were executed"));
        Ok(())
    }

    #[test]
    fn follow_up_proof_broker_executes_request_only_focused_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["src/lib.rs".to_owned()],
            patch: "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 pub fn route() {}
+pub fn patched_route() {}
"
            .to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::SourceGeneral,
        };
        let proof_requests = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-follow-up-001".to_owned(),
            lane: "orchestrator-follow-up-route-proof".to_owned(),
            requested_by: vec!["orchestrator-follow-up-route-proof".to_owned()],
            command: "bun test test/js/bun/fs/fs.write.test.ts -t route".to_owned(),
            reason: "Follow-up asked for a route witness.".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "requested".to_owned(),
        }];
        let tasks = super::focused_test_candidates_from_requests(&proof_requests);
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].file, "test/js/bun/fs/fs.write.test.ts");
        assert_eq!(tasks[0].request_ids, vec!["proof-follow-up-001"]);

        let args = test_run_args(out.clone());
        let prepared_base_root = base_root.clone();
        let mut commands = Vec::<String>::new();
        let proof_result = super::run_follow_up_proof_broker_v0_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, _timeout, stdout, stderr| {
                commands.push(command_display(argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 42,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(commands.len(), 2);
        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.resource_leases[0].status, "granted");
        let receipt = &proof_result.proof_receipts[0];
        assert_eq!(receipt.kind, "focused-red-green");
        assert_eq!(receipt.result, "discriminating");
        assert_eq!(
            receipt.requested_by,
            vec!["orchestrator-follow-up-route-proof".to_owned()]
        );
        assert_eq!(receipt.request_ids, vec!["proof-follow-up-001"]);
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].status, "failed");
        Ok(())
    }

    #[test]
    fn follow_up_proof_broker_uses_remaining_focused_proof_budget() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let proof_requests = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-follow-up-002".to_owned(),
            lane: "orchestrator-follow-up-tests-proof".to_owned(),
            requested_by: vec!["orchestrator-follow-up-tests-proof".to_owned()],
            command: "bun test test/js/bun/md/md-edge-cases.test.ts -t snapshots".to_owned(),
            reason: "Follow-up asked for a second proof.".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "requested".to_owned(),
        }];
        let existing_leases = vec![ResourceLease {
            schema: "ub-review.resource_lease.v1".to_owned(),
            id: "lease-proof-red-green-existing".to_owned(),
            kind: "focused-test".to_owned(),
            consumer: "proof-red-green-existing".to_owned(),
            status: "granted".to_owned(),
            reason: "focused red/green proof lease granted by runtime profile".to_owned(),
            cpu: 2,
            memory_mb: 2048,
            disk_mb: 1024,
            timeout_sec: 600,
            network: false,
            scratch: true,
            worktree: Some("base-plus-tests".to_owned()),
            command: Some("head: bun bd test existing; base+tests: bun test existing".to_owned()),
        }];
        let remaining = super::remaining_focused_proof_budget(
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            &existing_leases,
        );
        assert_eq!(remaining.max_focused_tests, 0);
        assert_eq!(remaining.max_focused_test_files, 2);
        assert_eq!(remaining.max_total_seconds, 0);

        let args = test_run_args(out.clone());
        let tasks = super::focused_test_candidates_from_requests(&proof_requests);
        let mut commands = Vec::<String>::new();
        let proof_result = super::run_follow_up_proof_broker_v0_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            remaining,
            tasks,
            |_root, argv, _env, _timeout, _stdout, _stderr| {
                commands.push(command_display(argv));
                Ok(CommandStatus {
                    exit_code: Some(0),
                    timed_out: false,
                    success: true,
                    reason: "should not run".to_owned(),
                    duration_ms: 0,
                })
            },
            |_root, _out, _diff| {
                bail!("base+tests worktree should not be prepared after budget is spent")
            },
        )?;

        assert!(commands.is_empty());
        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.proof_receipts[0].result, "skipped_budget");
        assert_eq!(
            proof_result.proof_receipts[0].request_ids,
            vec!["proof-follow-up-002"]
        );
        assert_eq!(proof_result.resource_leases[0].status, "exhausted");
        assert_eq!(
            proof_result.resource_leases[0].reason,
            "focused red/green proof lease budget exhausted by runtime profile"
        );
        Ok(())
    }

    #[test]
    fn initial_diff_receipt_absorbs_matching_lane_request_metadata() {
        let patch = "\
diff --git a/test/js/bun/md/md-edge-cases.test.ts b/test/js/bun/md/md-edge-cases.test.ts
index 1111111..2222222 100644
--- a/test/js/bun/md/md-edge-cases.test.ts
+++ b/test/js/bun/md/md-edge-cases.test.ts
@@ -1,2 +1,3 @@
 import { test } from 'bun:test';
+test(\"snapshots resizable ArrayBuffer input\", () => {});
";
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["test/js/bun/md/md-edge-cases.test.ts".to_owned()],
            patch: patch.to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        };
        let initial_tasks = super::focused_test_candidates_from_diff(&diff, &[]);
        assert_eq!(initial_tasks.len(), 1);
        let mut receipts = vec![super::focused_red_green_receipt(
            &diff,
            &initial_tasks[0],
            Vec::new(),
            "discriminating".to_owned(),
            "HEAD passed; base+tests failed".to_owned(),
        )];
        assert_eq!(receipts[0].requested_by, vec!["proof-broker"]);
        assert!(receipts[0].request_ids.is_empty());

        let proof_requests = vec![ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-md-001".to_owned(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["tests-oracle".to_owned(), "opposition".to_owned()],
            command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
            reason: "Need md red/green witness.".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "requested".to_owned(),
        }];

        super::attach_request_metadata_to_focused_receipts(&diff, &proof_requests, &mut receipts);
        assert_eq!(
            receipts[0].requested_by,
            vec![
                "proof-broker".to_owned(),
                "tests-oracle".to_owned(),
                "opposition".to_owned()
            ]
        );
        assert_eq!(receipts[0].request_ids, vec!["proof-md-001"]);
        let remaining_tasks = super::unreceipted_focused_test_tasks(
            super::focused_test_candidates_from_requests(&proof_requests),
            &receipts,
        );
        assert!(
            remaining_tasks.is_empty(),
            "matching model request should not rerun initial diff proof"
        );
    }

    #[test]
    fn run_loop_metrics_count_actual_model_proof_overlap() {
        let mut tracker = super::RunLoopTracker::new();
        tracker.record_interval(
            "model",
            super::LoopInterval {
                started_at_offset_ms: 10,
                finished_at_offset_ms: 110,
            },
        );
        tracker.record_interval(
            "proof",
            super::LoopInterval {
                started_at_offset_ms: 50,
                finished_at_offset_ms: 80,
            },
        );

        let metrics = tracker.metrics();
        assert_eq!(metrics.model_wall_ms, 100);
        assert_eq!(metrics.local_proof_wall_ms, 30);
        assert_eq!(metrics.scheduler_roles.model.wall_ms, 100);
        assert_eq!(metrics.scheduler_roles.proof.wall_ms, 30);
        assert_eq!(metrics.investigation_proof_overlap_ms, 30);
        assert_eq!(metrics.model_proof_overlap_ms, 30);
        assert_eq!(metrics.proof_overlap_ms, 30);
    }

    #[test]
    fn run_loop_overlap_ms_does_not_double_count_self_overlapping_side() {
        // Regression: overlap_ms used to sum overlap across every
        // (left, right) interval pair, so two overlapping sub-phases on one
        // side (e.g. pipelined model turns) got the shared proof window
        // counted twice. The union-based fix must report the true union
        // overlap even when a side's own intervals overlap each other.
        //
        // The three intermediate quantities the fix combines (model's union,
        // proof's union, and their combined union) are chosen pairwise
        // distinct from each other and from the expected overlap, so a
        // wrong-operand or wrong-operation mutation of the formula changes
        // the asserted result instead of hiding behind a coincidental tie.
        let mut tracker = super::RunLoopTracker::new();
        tracker.record_interval(
            "model",
            super::LoopInterval {
                started_at_offset_ms: 0,
                finished_at_offset_ms: 100,
            },
        );
        tracker.record_interval(
            "model",
            super::LoopInterval {
                started_at_offset_ms: 50,
                finished_at_offset_ms: 150,
            },
        );
        tracker.record_interval(
            "proof",
            super::LoopInterval {
                started_at_offset_ms: 30,
                finished_at_offset_ms: 200,
            },
        );

        let metrics = tracker.metrics();
        assert_eq!(metrics.model_wall_ms, 150, "union of [0,100) and [50,150)");
        assert_eq!(metrics.local_proof_wall_ms, 170, "union of [30,200)");
        assert_eq!(
            metrics.investigation_proof_overlap_ms, 120,
            "true overlap of [0,150) and [30,200) is [30,150); the old pairwise \
             sum double-counted the shared [50,100) sub-range across both \
             overlapping model intervals and reported 170 instead"
        );
        assert_eq!(metrics.model_proof_overlap_ms, 120);
        assert_eq!(metrics.proof_overlap_ms, 120);
    }

    #[test]
    fn overlapping_evidence_phases_never_exceed_observed_span() {
        // #325 regression (caught by the artifact verifier on the first
        // pipelined dogfood run): the late evidence phase overlaps the
        // sensors-and-packet phase inside the same evidence loop. Wall time
        // must be the union of the intervals, never their sum — the verifier
        // enforces wall <= observed span per stream.
        let mut tracker = super::RunLoopTracker::new();
        tracker.record_interval(
            "evidence",
            super::LoopInterval {
                started_at_offset_ms: 0,
                finished_at_offset_ms: 100,
            },
        );
        // Late-sensors phase: opened while sensors-and-packet was still
        // running, finished long after it (under the model wave).
        tracker.record_interval(
            "evidence",
            super::LoopInterval {
                started_at_offset_ms: 40,
                finished_at_offset_ms: 400,
            },
        );

        let metrics = tracker.metrics();
        let span = metrics.streams.coordination.finished_at_offset_ms
            - metrics.streams.coordination.started_at_offset_ms;
        assert_eq!(
            metrics.evidence_wall_ms, 400,
            "union of [0,100) and [40,400)"
        );
        assert_eq!(metrics.streams.coordination.wall_ms, 400);
        assert!(
            metrics.streams.coordination.wall_ms <= span,
            "coordination wall {} must not exceed observed span {span}",
            metrics.streams.coordination.wall_ms
        );
        assert_eq!(metrics.scheduler_roles.evidence.wall_ms, 400);

        // Disjoint follow-up phase still adds its full length.
        tracker.record_interval(
            "evidence",
            super::LoopInterval {
                started_at_offset_ms: 500,
                finished_at_offset_ms: 550,
            },
        );
        assert_eq!(tracker.metrics().evidence_wall_ms, 450);
    }

    #[test]
    fn proof_broker_v0_exhausts_focused_tests_after_runtime_budget() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let tasks = vec![
            super::focused_test_task(
                "test/js/bun/md/md-edge-cases.test.ts",
                Some("snapshots input".to_owned()),
                &[] as &[ProofRequestGroup],
            ),
            super::focused_test_task(
                "test/js/bun/md/md-edge-cases.test.ts",
                Some("getter reentry".to_owned()),
                &[] as &[ProofRequestGroup],
            ),
        ];
        let args = test_run_args(out.clone());
        let prepared_base_root = base_root.clone();
        let mut commands = Vec::<String>::new();

        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, _timeout, stdout, stderr| {
                commands.push(command_display(argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 42,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(commands.len(), 2);
        assert_eq!(proof_result.proof_receipts.len(), 2);
        assert_eq!(proof_result.proof_receipts[0].result, "discriminating");
        assert_eq!(proof_result.proof_receipts[1].result, "skipped_budget");
        assert_eq!(proof_result.proof_receipts[1].commands[0].status, "skipped");
        assert_eq!(proof_result.resource_leases.len(), 2);
        assert_eq!(proof_result.resource_leases[0].status, "granted");
        assert_eq!(proof_result.resource_leases[1].status, "exhausted");
        assert_eq!(
            proof_result.resource_leases[1].reason,
            "focused red/green proof lease budget exhausted by runtime profile"
        );
        Ok(())
    }

    #[test]
    fn proof_broker_v0_records_candidates_beyond_focused_file_budget() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let patch = "\
diff --git a/test/js/bun/md/md-edge-cases.test.ts b/test/js/bun/md/md-edge-cases.test.ts
index 1111111..2222222 100644
--- a/test/js/bun/md/md-edge-cases.test.ts
+++ b/test/js/bun/md/md-edge-cases.test.ts
@@ -1,2 +1,3 @@
 import { test } from 'bun:test';
+test(\"snapshots resizable ArrayBuffer input\", () => {});
diff --git a/test/js/bun/ffi/ffi.test.js b/test/js/bun/ffi/ffi.test.js
index 3333333..4444444 100644
--- a/test/js/bun/ffi/ffi.test.js
+++ b/test/js/bun/ffi/ffi.test.js
@@ -1,2 +1,3 @@
 import { test } from 'bun:test';
+test(\"no-finalizer toBuffer keeps caller memory alive\", () => {});
";
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec![
                "test/js/bun/md/md-edge-cases.test.ts".to_owned(),
                "test/js/bun/ffi/ffi.test.js".to_owned(),
            ],
            patch: patch.to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        };
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-md-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Need md red/green witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-ffi-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/ffi/ffi.test.js -t 'no-finalizer toBuffer keeps caller memory alive'".to_owned(),
                reason: "Need ffi red/green witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
        ];
        let tasks = super::focused_test_candidates_from_diff(&diff, &proof_requests);
        assert_eq!(tasks.len(), 2);

        let args = test_run_args(out.clone());
        let prepared_base_root = base_root.clone();
        let mut commands = Vec::<String>::new();
        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 1,
                max_focused_tests: 6,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, argv, env, _timeout, stdout, stderr| {
                commands.push(command_display(argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
                fs::write(
                    stdout,
                    if is_base {
                        b"base failed\n".as_slice()
                    } else {
                        b"head ok\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(if is_base { 1 } else { 0 }),
                    timed_out: false,
                    success: !is_base,
                    reason: "completed".to_owned(),
                    duration_ms: 42,
                })
            },
            move |_root, _out, _diff| Ok(prepared_base_root.clone()),
        )?;

        assert_eq!(commands.len(), 2);
        assert_eq!(proof_result.proof_receipts.len(), 2);
        assert_eq!(proof_result.resource_leases.len(), 2);
        assert_eq!(proof_result.proof_receipts[0].result, "discriminating");
        assert_eq!(
            proof_result.proof_receipts[0].request_ids,
            vec!["proof-md-001"]
        );
        assert_eq!(proof_result.resource_leases[0].status, "granted");
        assert_eq!(
            proof_result.resource_leases[0].consumer,
            proof_result.proof_receipts[0].id
        );
        assert_eq!(proof_result.proof_receipts[1].result, "skipped_budget");
        assert_eq!(
            proof_result.proof_receipts[1].request_ids,
            vec!["proof-ffi-001"]
        );
        assert_eq!(proof_result.proof_receipts[1].commands[0].status, "skipped");
        assert_eq!(proof_result.resource_leases[1].status, "exhausted");
        assert_eq!(
            proof_result.resource_leases[1].consumer,
            proof_result.proof_receipts[1].id
        );
        assert!(
            proof_result.proof_receipts[1]
                .reason
                .contains("lease budget exhausted"),
            "unexpected skipped-budget reason: {}",
            proof_result.proof_receipts[1].reason
        );
        Ok(())
    }

    fn test_focused_test_lease(task: &super::FocusedTestTask) -> ResourceLease {
        super::focused_test_resource_lease(
            task,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 3,
                per_command_timeout_sec: 300,
                max_total_seconds: 900,
            },
            ProofLeaseBudget {
                cpu: 1,
                memory_mb: 512,
                disk_mb: 64,
                network: false,
                scratch: true,
            },
            "granted",
            "focused proof lease granted by test fixture",
        )
    }

    #[test]
    fn proof_broker_v0_marks_base_plus_tests_pass_as_non_discriminating() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let task = super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        );
        let lease = test_focused_test_lease(&task);
        let mut runner_calls = 0;
        let mut prepare_calls = 0;
        let mut runner = |_root: &Path,
                          _argv: &[String],
                          _env: &BTreeMap<String, String>,
                          _timeout: u64,
                          stdout: &Path,
                          stderr: &Path|
         -> Result<CommandStatus> {
            runner_calls += 1;
            fs::write(stdout, b"ok\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(0),
                timed_out: false,
                success: true,
                reason: "completed".to_owned(),
                duration_ms: 7,
            })
        };
        let prepared_base_root = base_root.clone();
        let mut prepare = |_root: &Path, _out: &Path, _diff: &DiffContext| -> Result<_> {
            prepare_calls += 1;
            Ok(prepared_base_root.clone())
        };

        let receipt = super::run_focused_red_green_proof_task(
            temp.path(),
            &out,
            &diff,
            &task,
            300,
            &lease,
            &mut runner,
            &mut prepare,
        )?;

        assert_eq!(runner_calls, 2);
        assert_eq!(prepare_calls, 1);
        assert_eq!(receipt.kind, "focused-red-green");
        assert_eq!(receipt.result, "non_discriminating");
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].status, "passed");
        Ok(())
    }

    #[test]
    fn proof_broker_v0_skips_base_plus_tests_when_head_fails() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let task = super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        );
        let lease = test_focused_test_lease(&task);
        let mut prepare_calls = 0;
        let mut runner = |_root: &Path,
                          _argv: &[String],
                          _env: &BTreeMap<String, String>,
                          _timeout: u64,
                          stdout: &Path,
                          stderr: &Path|
         -> Result<CommandStatus> {
            fs::write(stdout, b"failed\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(1),
                timed_out: false,
                success: false,
                reason: "exit code Some(1)".to_owned(),
                duration_ms: 7,
            })
        };
        let mut prepare = |_root: &Path, _out: &Path, _diff: &DiffContext| -> Result<_> {
            prepare_calls += 1;
            Ok(temp.path().join("base-plus-tests"))
        };

        let receipt = super::run_focused_red_green_proof_task(
            temp.path(),
            &out,
            &diff,
            &task,
            300,
            &lease,
            &mut runner,
            &mut prepare,
        )?;

        assert_eq!(prepare_calls, 0);
        assert_eq!(receipt.result, "head_failed");
        assert_eq!(receipt.commands.len(), 1);
        assert_eq!(receipt.commands[0].status, "failed");
        Ok(())
    }

    #[test]
    fn proof_broker_v0_records_base_patch_failed_as_missing_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let task = super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        );
        let lease = test_focused_test_lease(&task);
        let mut runner = |_root: &Path,
                          _argv: &[String],
                          _env: &BTreeMap<String, String>,
                          _timeout: u64,
                          stdout: &Path,
                          stderr: &Path|
         -> Result<CommandStatus> {
            fs::write(stdout, b"head ok\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(0),
                timed_out: false,
                success: true,
                reason: "completed".to_owned(),
                duration_ms: 7,
            })
        };
        let mut prepare = |_root: &Path, _out: &Path, _diff: &DiffContext| -> Result<PathBuf> {
            Err(anyhow::anyhow!("patch hunk #1 rejected for tests/cli.rs"))
        };

        let mut receipt = super::run_focused_red_green_proof_task(
            temp.path(),
            &out,
            &diff,
            &task,
            300,
            &lease,
            &mut runner,
            &mut prepare,
        )?;

        assert_eq!(receipt.result, "base_patch_failed");
        assert_eq!(
            receipt.reason,
            "base+tests patch failed: patch hunk #1 rejected for tests/cli.rs"
        );
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].side, "base-plus-tests");
        assert_eq!(receipt.commands[1].status, "skipped");
        assert_eq!(receipt.commands[1].reason, receipt.reason);
        assert!(super::proof_receipt_is_missing_evidence(&receipt));

        receipt.requested_by = vec!["tests-oracle".to_owned()];
        let routed = super::routed_evidence_for_group(
            "proof-confirmation",
            &["tests-oracle".to_owned()],
            std::slice::from_ref(&receipt),
            &[],
            &[],
        );
        assert_eq!(routed.len(), 1);
        assert_eq!(routed[0].result, "base_patch_failed");
        assert_eq!(routed[0].status, "missing-evidence");
        assert_eq!(routed[0].reason, receipt.reason);

        let follow_up = FollowUpQuestionTask {
            schema: "test".to_owned(),
            id: "fu-base-patch-failed".to_owned(),
            group_id: "group-base-patch-failed".to_owned(),
            stage: "tertiary".to_owned(),
            stage_reason: "routed proof".to_owned(),
            evidence_need: "proof-confirmation".to_owned(),
            disposition: "summary-only".to_owned(),
            candidate_ids: Vec::new(),
            observation_group_ids: Vec::new(),
            routed_evidence: routed,
            question: "Confirm whether routed proof evidence resolves this observation.".to_owned(),
            status: "pending".to_owned(),
            reason: "test".to_owned(),
        };
        let prompt = super::render_follow_up_question_prompt(&follow_up, &BTreeMap::new());
        assert!(prompt.contains("base_patch_failed"));
        assert!(prompt.contains("patch hunk #1 rejected for tests/cli.rs"));
        Ok(())
    }

    #[test]
    fn proof_broker_v0_cleans_base_plus_tests_worktree_when_base_receipt_errors() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let base_root = temp.path().join("base-plus-tests");
        fs::create_dir_all(&base_root)?;
        let diff = test_diff();
        let task = super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        );
        let lease = test_focused_test_lease(&task);
        let base_receipt_dir = out.join("proof").join(&task.id).join("base-plus-tests");
        fs::create_dir_all(base_receipt_dir.parent().context("base receipt parent")?)?;
        fs::write(&base_receipt_dir, b"not a directory")?;

        let mut runner_calls = 0;
        let mut prepare_calls = 0;
        let mut runner = |_root: &Path,
                          _argv: &[String],
                          _env: &BTreeMap<String, String>,
                          _timeout: u64,
                          stdout: &Path,
                          stderr: &Path|
         -> Result<CommandStatus> {
            runner_calls += 1;
            fs::write(stdout, b"head ok\n")?;
            fs::write(stderr, b"")?;
            Ok(CommandStatus {
                exit_code: Some(0),
                timed_out: false,
                success: true,
                reason: "completed".to_owned(),
                duration_ms: 7,
            })
        };
        let prepared_base_root = base_root.clone();
        let mut prepare = |_root: &Path, _out: &Path, _diff: &DiffContext| -> Result<PathBuf> {
            prepare_calls += 1;
            Ok(prepared_base_root.clone())
        };

        let result = super::run_focused_red_green_proof_task(
            temp.path(),
            &out,
            &diff,
            &task,
            300,
            &lease,
            &mut runner,
            &mut prepare,
        );

        let error = match result {
            Ok(_) => bail!("base receipt path error should have failed the proof task"),
            Err(error) => error,
        };
        assert_eq!(runner_calls, 1);
        assert_eq!(prepare_calls, 1);
        assert!(
            format!("{error:#}").contains("base-plus-tests"),
            "error should name the failed base receipt path: {error:#}"
        );
        assert!(
            !base_root.exists(),
            "base+tests worktree must be cleaned after receipt-path errors"
        );
        Ok(())
    }

    #[test]
    fn proof_broker_v0_does_not_execute_without_focused_test_lease() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let tasks = vec![super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        )];
        let args = test_run_args(out.clone());
        let mut profile = Profile::default();
        profile.limits.tests = 0;

        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &profile,
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 1,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, _argv, _env, _timeout, _stdout, _stderr| {
                bail!("proof command should not run without a lease")
            },
            |_root, _out, _diff| {
                bail!("base+tests worktree should not be prepared without a lease")
            },
        )?;

        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.proof_receipts[0].result, "skipped_profile");
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.resource_leases[0].status, "absent");
        assert_eq!(
            proof_result.resource_leases[0].reason,
            "profile allows zero focused test leases"
        );
        Ok(())
    }

    #[test]
    fn proof_broker_v0_does_not_execute_when_focused_test_budget_is_zero() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let tasks = vec![super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &[] as &[ProofRequestGroup],
        )];
        let args = test_run_args(out.clone());

        let proof_result = super::run_focused_red_green_proof_tasks_with_runner(
            temp.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 0,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            tasks,
            |_root, _argv, _env, _timeout, _stdout, _stderr| {
                bail!("proof command should not run when proof budget is zero")
            },
            |_root, _out, _diff| {
                bail!("base+tests worktree should not be prepared when proof budget is zero")
            },
        )?;

        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.proof_receipts[0].result, "skipped_budget");
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.resource_leases[0].status, "exhausted");
        Ok(())
    }

    #[test]
    fn box_from_allocation_false_premise_candidate_is_refuted_not_inline() -> Result<()> {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn snapshot() {
+    let bytes = Box::<[u8]>::from(slice());
 }
";
        let line_map = right_side_diff_lines(patch);
        let lane = model_lane(
            "ub-active-view",
            "Active view review",
            &["tokmd", "unsafe-review"],
            "Check active-view/backing-store safety.",
        );
        let json = r#"{
  "candidate_findings": [
    {
      "severity": "high",
      "confidence": "high",
      "path": "src/lib.rs",
      "line": 2,
      "body": "[ub-active-view] If Box::<[u8]>::from(pinned.slice()) returns None on allocation failure, this can fall back to a borrowed live slice.",
      "evidence": "diff hunk claims allocation failure can fall through"
    }
  ]
}"#;
        let output: LaneModelOutput = serde_json::from_str(json)?;
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();

        apply_model_output(
            &lane,
            output,
            &line_map,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert!(inline_comments.is_empty());
        assert!(summary_only_findings.is_empty());
        assert_eq!(observations.len(), 1);
        let observation = &observations[0];
        assert_eq!(observation.kind, "false-premise");
        assert_eq!(observation.status, "refuted");
        assert_eq!(observation.severity, "low");
        assert_eq!(observation.confidence, "high");
        assert_eq!(observation.dedupe_key, "rust-box-from-allocation-failure");
        assert_eq!(observation.source, "model-false-premise-guard");
        assert_eq!(observation.path.as_deref(), Some("src/lib.rs"));
        assert_eq!(observation.line, Some(2));
        assert!(observation.claim.contains("does not return `None`"));
        Ok(())
    }

    #[test]
    fn box_from_allocation_false_premise_observation_is_forced_refuted() -> Result<()> {
        let lane = model_lane(
            "ub-active-view",
            "Active view review",
            &["tokmd", "unsafe-review"],
            "Check active-view/backing-store safety.",
        );
        let json = r#"{
  "observations": [
    {
      "claim": "Box::<[u8]>::from(pinned.slice()) can return None on allocation failure and then fall back to borrowed bytes.",
      "question": "fallback-path",
      "kind": "bug",
      "status": "open",
      "severity": "high",
      "confidence": "medium-high",
      "path": "src/lib.rs",
      "line": 2,
      "evidence": ["model premise"]
    }
  ]
}"#;
        let output: LaneModelOutput = serde_json::from_str(json)?;
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();

        apply_model_output(
            &lane,
            output,
            &BTreeSet::new(),
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert!(inline_comments.is_empty());
        assert!(summary_only_findings.is_empty());
        assert_eq!(observations.len(), 1);
        let observation = &observations[0];
        assert_eq!(observation.kind, "false-premise");
        assert_eq!(observation.status, "refuted");
        assert_eq!(observation.severity, "low");
        assert_eq!(observation.confidence, "high");
        assert_eq!(observation.dedupe_key, "rust-box-from-allocation-failure");
        assert_eq!(observation.source, "model-false-premise-guard");
        assert_eq!(observation.question, "false-premise");
        assert!(
            observation
                .evidence
                .iter()
                .any(|item| item.contains("does not return None"))
        );
        Ok(())
    }

    #[test]
    fn lane_model_summary_rejects_standalone_approval_language() -> Result<()> {
        let lane = default_lanes()
            .into_iter()
            .find(|lane| lane.id == "tests")
            .ok_or_else(|| anyhow::anyhow!("tests lane missing"))?;
        for summary in ["LGTM", "no actionable findings", "no actionable"] {
            let output = LaneModelOutput {
                summary: Some(summary.to_owned()),
                inline_comments: Vec::new(),
                candidate_findings: Vec::new(),
                summary_only_findings: Vec::new(),
                observations: Vec::new(),
                failed_objections: Vec::new(),
                proof_requests: Vec::new(),
                proof_intents: Vec::new(),
                issue_candidates: Vec::new(),
                degraded: false,
            };
            let mut inline_comments = Vec::new();
            let mut summary_only_findings = Vec::new();
            let mut model_observations = Vec::new();
            let mut proof_requests = Vec::new();
            let mut issue_candidates = Vec::new();

            apply_model_output(
                &lane,
                output,
                &BTreeSet::new(),
                ModelOutputSinks {
                    inline_comments: &mut inline_comments,
                    summary_only_findings: &mut summary_only_findings,
                    model_observations: &mut model_observations,
                    proof_requests: &mut proof_requests,
                    proof_intents: &mut Vec::new(),
                    issue_candidates: &mut issue_candidates,
                },
            );

            assert!(inline_comments.is_empty());
            assert_eq!(summary_only_findings.len(), 1);
            assert_eq!(
                summary_only_findings[0].evidence,
                "lane model summary guardrail"
            );
            assert!(
                summary_only_findings[0]
                    .reason
                    .contains("no_standalone_approval=false")
            );
            assert!(!has_standalone_approval_line(
                &summary_only_findings[0].reason
            ));
        }
        Ok(())
    }

    #[test]
    fn inline_findings_order_best_first_after_dedupe() {
        // #178 value ranking: severity then confidence descending, with
        // path:line as the deterministic tiebreak - the PR body and the
        // inline list lead with the finding most worth the reviewer's
        // first look, regardless of lane arrival order.
        let comment = |path: &str, severity: &str, confidence: &str| ReviewInlineComment {
            lane: "ub".to_owned(),
            path: path.to_owned(),
            line: 10,
            side: "RIGHT".to_owned(),
            severity: severity.to_owned(),
            confidence: confidence.to_owned(),
            body: format!("finding at {path}"),
            evidence: "input/diff.patch".to_owned(),
            suggestion: None,
        };
        let mut inline = vec![
            comment("src/low.rs", "low", "high"),
            comment("src/blocker.rs", "blocker", "medium"),
            comment("src/high_med.rs", "high", "medium"),
            comment("src/high_hi.rs", "high", "high"),
        ];
        let mut summary = Vec::new();
        dedupe_inline_comments(&mut inline, &mut summary);
        let order: Vec<&str> = inline.iter().map(|c| c.path.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "src/blocker.rs",
                "src/high_hi.rs",
                "src/high_med.rs",
                "src/low.rs"
            ],
            "best finding leads"
        );
        assert!(summary.is_empty(), "no duplicates were merged");
    }

    #[test]
    fn inline_findings_rank_executed_evidence_before_model_confidence() {
        let comment = |severity: &str, confidence: &str, evidence: &str| ReviewInlineComment {
            lane: "correctness".to_owned(),
            path: format!("src/{severity}.rs"),
            line: 10,
            side: "RIGHT".to_owned(),
            severity: severity.to_owned(),
            confidence: confidence.to_owned(),
            body: "the changed branch has a behavioral claim".to_owned(),
            evidence: evidence.to_owned(),
            suggestion: None,
        };
        let mut inline = vec![
            comment("blocker", "high", "model observation"),
            comment("low", "medium", "executed focused test receipt"),
        ];
        let mut summary = Vec::new();
        dedupe_inline_comments(&mut inline, &mut summary);
        assert_eq!(inline.len(), 2);
        assert_eq!(inline[0].severity, "low");
        assert_eq!(inline[0].evidence, "executed focused test receipt");
    }

    #[test]
    fn inline_dedupe_keeps_distinct_structural_claims_with_shared_vocabulary() {
        let comment = |line: u32, body: &str| ReviewInlineComment {
            lane: "parser".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            path: "src/parser.rs".to_owned(),
            line,
            side: "RIGHT".to_owned(),
            body: body.to_owned(),
            evidence: "source diff observation".to_owned(),
            suggestion: None,
        };
        let mut inline = vec![
            comment(
                12,
                "The declaration list parser drops postfix subscript handling for `$x[0]`; add a focused test for the list behavior.",
            ),
            comment(
                30,
                "The declaration list lookahead omits the percent sigil for `%h`; add a focused test for the list behavior.",
            ),
        ];
        let mut summary = Vec::new();
        dedupe_inline_comments(&mut inline, &mut summary);
        assert_eq!(inline.len(), 2);
        assert!(summary.is_empty());
    }

    #[test]
    fn inline_dedupe_keeps_strongest_same_location_candidate() {
        let mut inline_comments = vec![
            ReviewInlineComment {
                lane: "tests-oracle".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests-oracle] This test reaches the helper but not the boundary."
                    .to_owned(),
                evidence: "ripr excerpt".to_owned(),
                suggestion: None,
            },
            ReviewInlineComment {
                lane: "ub-active-view".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[ub-active-view] The view length can diverge from backing storage."
                    .to_owned(),
                evidence: "unsafe-review card".to_owned(),
                suggestion: None,
            },
        ];
        let mut summary_only_findings = Vec::new();

        dedupe_inline_comments(&mut inline_comments, &mut summary_only_findings);

        assert_eq!(inline_comments.len(), 1);
        assert_eq!(inline_comments[0].lane, "ub-active-view");
        assert_eq!(inline_comments[0].severity, "high");
        assert!(inline_comments[0].evidence.contains("unsafe-review card"));
        assert!(inline_comments[0].evidence.contains("ripr excerpt"));
        assert_eq!(summary_only_findings.len(), 1);
        assert_eq!(summary_only_findings[0].lane, "tests-oracle");
        assert!(
            summary_only_findings[0]
                .reason
                .contains("duplicate inline candidate merged into src/lib.rs:2")
        );
    }

    #[test]
    fn inline_dedupe_merges_same_claim_different_line_candidates() {
        let mut inline_comments = vec![
            ReviewInlineComment {
                lane: "tests-oracle".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "tests/boundary.test.ts".to_owned(),
                line: 12,
                side: "RIGHT".to_owned(),
                body: "[tests-oracle] Bare `.toThrow()` assertion is non-discriminating; assert the error type or message."
                    .to_owned(),
                evidence: "tests-oracle saw a bare throw assertion on line 12".to_owned(),
                suggestion: None,
            },
            ReviewInlineComment {
                lane: "tests-red-green".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: "tests/boundary.test.ts".to_owned(),
                line: 30,
                side: "RIGHT".to_owned(),
                body: "[tests-red-green] The `.toThrow()` check does not discriminate the thrown error; assert type or message."
                    .to_owned(),
                evidence: "red/green lane saw the same weak oracle on line 30".to_owned(),
                suggestion: None,
            },
        ];
        let mut summary_only_findings = Vec::new();

        dedupe_inline_comments(&mut inline_comments, &mut summary_only_findings);

        assert_eq!(inline_comments.len(), 1);
        assert_eq!(inline_comments[0].lane, "tests-red-green");
        assert_eq!(inline_comments[0].line, 30);
        assert!(inline_comments[0].evidence.contains("line 30"));
        assert!(inline_comments[0].evidence.contains("line 12"));
        assert_eq!(summary_only_findings.len(), 1);
        assert_eq!(summary_only_findings[0].lane, "tests-oracle");
        assert!(
            summary_only_findings[0]
                .reason
                .contains("same-claim inline candidate at tests/boundary.test.ts:12 merged into tests/boundary.test.ts:30")
        );
    }

    #[test]
    fn inline_dedupe_keeps_suggestions_line_specific() {
        let mut inline_comments = vec![
            ReviewInlineComment {
                lane: "unsafe-review".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: "src/native.rs".to_owned(),
                line: 10,
                side: "RIGHT".to_owned(),
                body: "[unsafe-review] The unsafe precondition is missing the caller-owned length invariant."
                    .to_owned(),
                evidence: "unsafe-review comment-plan candidate 1".to_owned(),
                suggestion: Some("// SAFETY: len is caller-owned and bounded.".to_owned()),
            },
            ReviewInlineComment {
                lane: "unsafe-review".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: "src/native.rs".to_owned(),
                line: 18,
                side: "RIGHT".to_owned(),
                body: "[unsafe-review] The unsafe precondition is missing the caller-owned length invariant."
                    .to_owned(),
                evidence: "unsafe-review comment-plan candidate 2".to_owned(),
                suggestion: Some("// SAFETY: len is caller-owned for this slice.".to_owned()),
            },
        ];
        let mut summary_only_findings = Vec::new();

        dedupe_inline_comments(&mut inline_comments, &mut summary_only_findings);

        assert_eq!(inline_comments.len(), 2);
        assert!(summary_only_findings.is_empty());
        assert_eq!(inline_comments[0].line, 10);
        assert_eq!(inline_comments[1].line, 18);
    }

    #[test]
    fn refuter_prompt_keeps_execution_out_of_model_turn() -> Result<()> {
        let inline_comments = vec![ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            side: "RIGHT".to_owned(),
            body: "[tests-oracle] This test does not prove the changed boundary.".to_owned(),
            evidence: "ripr excerpt".to_owned(),
            suggestion: None,
        }];

        let prompt = render_refuter_prompt(&inline_comments)?;

        assert!(prompt.contains("Do not post, mutate files, or run shell commands"));
        assert!(prompt.contains("The refuter only classifies candidates"));
        assert!(prompt.contains("Use only the cached shared context"));
        Ok(())
    }

    #[test]
    fn refuter_demotes_uncertain_or_unmatched_inline_candidates() {
        let mut inline_comments = vec![
            ReviewInlineComment {
                lane: "tests-oracle".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests-oracle] This test does not prove the changed boundary.".to_owned(),
                evidence: "ripr excerpt".to_owned(),
                suggestion: None,
            },
            ReviewInlineComment {
                lane: "source-route".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 4,
                side: "RIGHT".to_owned(),
                body: "[source-route] A sibling path may share the helper.".to_owned(),
                evidence: "route map".to_owned(),
                suggestion: None,
            },
        ];
        let mut summary_only_findings = Vec::new();
        let output = RefuterOutput {
            decisions: vec![RefuterDecision {
                path: "src/lib.rs".to_owned(),
                line: 2,
                disposition: "summary".to_owned(),
                confidence: Some("high".to_owned()),
                reason: "plausible but not line-local enough".to_owned(),
            }],
        };

        apply_refuter_output(output, &mut inline_comments, &mut summary_only_findings);

        assert!(inline_comments.is_empty());
        assert_eq!(summary_only_findings.len(), 2);
        assert!(summary_only_findings.iter().any(|finding| {
            finding
                .reason
                .contains("plausible but not line-local enough")
        }));
        assert!(
            summary_only_findings
                .iter()
                .any(|finding| finding.reason.contains("returned no decision"))
        );
    }

    #[test]
    fn refuter_unavailable_demotes_pending_inline_candidates() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.max_model_calls = 3;
        let mut model_lanes = Vec::new();
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = vec![ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            side: "RIGHT".to_owned(),
            body: "[tests-oracle] This test does not prove the changed boundary.".to_owned(),
            evidence: "ripr excerpt".to_owned(),
            suggestion: None,
        }];
        let mut summary_only_findings = Vec::new();

        run_refuter_pass(
            RefuterRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                provider_preflights: &[],
                shared_context: "shared context",
                args: &args,
                model_calls_used: 3,
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
        )?;

        assert!(inline_comments.is_empty());
        assert_eq!(summary_only_findings.len(), 1);
        assert!(
            summary_only_findings[0]
                .reason
                .contains("refuter unavailable")
        );
        assert!(
            summary_only_findings[0]
                .reason
                .contains("model call budget exhausted before refuter pass")
        );
        assert_eq!(model_lanes.len(), 1);
        assert_eq!(model_lanes[0].lane, "refuter");
        assert_eq!(model_lanes[0].status, "skipped");
        assert_eq!(missing_or_failed_model_evidence.len(), 1);
        assert_eq!(
            missing_or_failed_model_evidence[0].reason,
            "model call budget exhausted before refuter pass"
        );
        Ok(())
    }

    #[test]
    fn model_lane_scheduling_ignores_inline_comment_cap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.max_inline_comments = 1;
        args.max_model_calls = 2;
        args.model_concurrency = 2;
        let spec = direct_minimax_spec(&args);
        let assignments = vec![
            ModelAssignment {
                lane: lane_plan("security"),
                spec: spec.clone(),
                fallback: None,
            },
            ModelAssignment {
                lane: lane_plan("opposition"),
                spec,
                fallback: None,
            },
        ];
        let mut model_lanes = vec![
            model_lane_receipt("security", "planned"),
            model_lane_receipt("opposition", "planned"),
        ];
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = vec![ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            side: "RIGHT".to_owned(),
            body: "[tests-oracle] Existing inline candidate fills the post cap.".to_owned(),
            evidence: "test setup".to_owned(),
            suggestion: None,
        }];
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut proof_intents = Vec::new();
        let mut issue_candidates = Vec::new();
        let line_map = BTreeSet::new();

        let calls = run_available_model_lanes(
            ModelRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                assignments: &assignments,
                provider_preflights: &[],
                shared_context: "shared context",
                args: &args,
                line_map: &line_map,
                key_present: |_| true,
                provider_concurrency: ProviderConcurrencyLimits::default(),
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
            &mut proof_intents,
            &mut issue_candidates,
        )?;

        assert_eq!(calls, 0);
        assert_eq!(inline_comments.len(), 1);
        assert!(summary_only_findings.is_empty());
        assert!(model_observations.is_empty());
        assert!(proof_requests.is_empty());
        assert_eq!(
            model_lanes
                .iter()
                .map(|receipt| receipt.status.as_str())
                .collect::<Vec<_>>(),
            vec!["preflight_failed", "preflight_failed"]
        );
        assert_eq!(missing_or_failed_model_evidence.len(), 2);
        assert!(
            missing_or_failed_model_evidence
                .iter()
                .all(|issue| !issue.reason.contains("inline comment cap"))
        );
        Ok(())
    }

    #[test]
    fn model_lane_scheduler_honors_provider_max_concurrency() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.max_model_calls = 4;
        args.model_concurrency = 3;
        let spec = direct_minimax_spec(&args);
        let assignments = ["security", "opposition", "tests-oracle"]
            .into_iter()
            .map(|lane| ModelAssignment {
                lane: lane_plan(lane),
                spec: spec.clone(),
                fallback: None,
            })
            .collect::<Vec<_>>();
        let preflights = vec![preflight_ok_receipt(&spec)];
        let mut model_lanes = assignments
            .iter()
            .map(|assignment| model_lane_receipt(&assignment.lane.id, "planned"))
            .collect::<Vec<_>>();
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        let line_map = BTreeSet::new();
        let waves = std::cell::RefCell::new(Vec::<Vec<ModelProvider>>::new());

        let calls = run_available_model_lanes_with_runner(
            ModelRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                assignments: &assignments,
                provider_preflights: &preflights,
                shared_context: "shared context",
                args: &args,
                line_map: &line_map,
                key_present: |_| true,
                provider_concurrency: ProviderConcurrencyLimits {
                    minimax: Some(1),
                    opencode_go: None,
                },
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
            &mut Vec::new(),
            &mut issue_candidates,
            |_context, _model_dir, tasks| {
                waves.borrow_mut().push(
                    tasks
                        .iter()
                        .map(|task| task.spec.provider)
                        .collect::<Vec<_>>(),
                );
                Ok(tasks
                    .into_iter()
                    .map(|task| ModelLaneTaskResult {
                        index: task.index,
                        result: Ok(ModelCallOutcome {
                            output: empty_lane_output(),
                            duration_ms: 5,
                            http_status: Some(200),
                            response_shape: "anthropic-messages".to_owned(),
                            cache_usage: ModelCacheUsage::default(),
                            degraded: false,
                        }),
                    })
                    .collect())
            },
        )?;

        assert_eq!(calls, 3);
        assert_eq!(
            waves.into_inner(),
            vec![
                vec![ModelProvider::MiniMaxDirect],
                vec![ModelProvider::MiniMaxDirect],
                vec![ModelProvider::MiniMaxDirect],
            ]
        );
        assert!(model_lanes.iter().all(|receipt| receipt.status == "ok"));
        assert!(missing_or_failed_model_evidence.is_empty());
        Ok(())
    }

    #[test]
    fn model_lane_scheduler_fills_wave_across_provider_limits() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.max_model_calls = 4;
        args.model_concurrency = 3;
        let minimax = direct_minimax_spec(&args);
        let opencode = opencode_canary_spec(&args);
        let assignments = vec![
            ModelAssignment {
                lane: lane_plan("security"),
                spec: minimax.clone(),
                fallback: None,
            },
            ModelAssignment {
                lane: lane_plan("opposition"),
                spec: opencode.clone(),
                fallback: None,
            },
            ModelAssignment {
                lane: lane_plan("spec-honesty"),
                spec: opencode.clone(),
                fallback: None,
            },
            ModelAssignment {
                lane: lane_plan("tests-oracle"),
                spec: minimax.clone(),
                fallback: None,
            },
        ];
        let preflights = vec![
            preflight_ok_receipt(&minimax),
            preflight_ok_receipt(&opencode),
        ];
        let mut model_lanes = assignments
            .iter()
            .map(|assignment| model_lane_receipt(&assignment.lane.id, "planned"))
            .collect::<Vec<_>>();
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        let line_map = BTreeSet::new();
        let waves = std::cell::RefCell::new(Vec::<Vec<ModelProvider>>::new());

        let calls = run_available_model_lanes_with_runner(
            ModelRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                assignments: &assignments,
                provider_preflights: &preflights,
                shared_context: "shared context",
                args: &args,
                line_map: &line_map,
                key_present: |_| true,
                provider_concurrency: ProviderConcurrencyLimits {
                    minimax: Some(1),
                    opencode_go: Some(2),
                },
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
            &mut Vec::new(),
            &mut issue_candidates,
            |_context, _model_dir, tasks| {
                waves.borrow_mut().push(
                    tasks
                        .iter()
                        .map(|task| task.spec.provider)
                        .collect::<Vec<_>>(),
                );
                Ok(tasks
                    .into_iter()
                    .map(|task| ModelLaneTaskResult {
                        index: task.index,
                        result: Ok(ModelCallOutcome {
                            output: empty_lane_output(),
                            duration_ms: 5,
                            http_status: Some(200),
                            response_shape: "anthropic-messages".to_owned(),
                            cache_usage: ModelCacheUsage::default(),
                            degraded: false,
                        }),
                    })
                    .collect())
            },
        )?;

        assert_eq!(calls, 4);
        assert_eq!(
            waves.into_inner(),
            vec![
                vec![
                    ModelProvider::MiniMaxDirect,
                    ModelProvider::OpenCodeGo,
                    ModelProvider::OpenCodeGo,
                ],
                vec![ModelProvider::MiniMaxDirect],
            ]
        );
        assert!(model_lanes.iter().all(|receipt| receipt.status == "ok"));
        assert!(missing_or_failed_model_evidence.is_empty());
        Ok(())
    }

    fn empty_lane_output() -> LaneModelOutput {
        LaneModelOutput {
            summary: None,
            inline_comments: Vec::new(),
            candidate_findings: Vec::new(),
            summary_only_findings: Vec::new(),
            observations: Vec::new(),
            failed_objections: Vec::new(),
            proof_requests: Vec::new(),
            proof_intents: Vec::new(),
            issue_candidates: Vec::new(),
            degraded: false,
        }
    }

    fn preflight_ok_receipt(spec: &super::ProviderSpec) -> super::ProviderPreflightReceipt {
        super::ProviderPreflightReceipt {
            provider: spec.provider.key().to_owned(),
            model: spec.model.clone(),
            endpoint_kind: spec.endpoint_kind.key().to_owned(),
            status: "ok".to_owned(),
            reason: "preflight ok".to_owned(),
            duration_ms: Some(1),
            http_status: Some(200),
            response_shape: Some("anthropic-messages".to_owned()),
            cache_usage: ModelCacheUsage::default(),
        }
    }

    fn preflight_failed_receipt(
        spec: &super::ProviderSpec,
        status: &str,
        reason: &str,
    ) -> super::ProviderPreflightReceipt {
        super::ProviderPreflightReceipt {
            provider: spec.provider.key().to_owned(),
            model: spec.model.clone(),
            endpoint_kind: spec.endpoint_kind.key().to_owned(),
            status: status.to_owned(),
            reason: reason.to_owned(),
            duration_ms: Some(1),
            http_status: None,
            response_shape: None,
            cache_usage: ModelCacheUsage::default(),
        }
    }

    #[test]
    fn model_pass_provider_selection_uses_fallback_preflight() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::PrimaryWithFallback;
        args.opencode_model = "mimo-v2.5".to_owned();
        let assignments = [
            proof_planner_assignment_with_key_state(&args, true),
            follow_up_provider_assignment_with_key_state(&args, true),
        ];

        for assignment in assignments {
            let fallback = assignment
                .fallback
                .clone()
                .ok_or_else(|| anyhow::anyhow!("fallback missing for {}", assignment.lane.id))?;
            let preflights = vec![
                preflight_failed_receipt(&assignment.spec, "missing_key", "primary unavailable"),
                preflight_ok_receipt(&fallback),
            ];

            let (selected, fallback_from, reason) =
                selected_provider_spec(&assignment, &preflights).ok_or_else(|| {
                    anyhow::anyhow!("provider selection failed for {}", assignment.lane.id)
                })?;

            assert_eq!(selected.provider, ModelProvider::OpenCodeGo);
            assert_eq!(selected.model, "mimo-v2.5");
            assert_eq!(fallback_from, Some(assignment.spec.label()));
            assert!(
                reason
                    .as_deref()
                    .is_some_and(|reason| reason.contains("primary provider unavailable"))
            );
        }
        Ok(())
    }

    #[test]
    fn runtime_fallback_retry_spec_only_retries_transient_primary_failures() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let primary = direct_minimax_spec(&args);
        let fallback = opencode_canary_spec(&args);
        let assignment = ModelAssignment {
            lane: lane_plan("security"),
            spec: primary.clone(),
            fallback: Some(fallback.clone()),
        };
        let receipt = model_lane_receipt("security", "running");
        let key_present: fn(&str) -> bool = |_| true;
        let key_absent: fn(&str) -> bool = |_| false;

        // Transient classes retry on the fallback spec.
        for (status, http) in [
            ("rate_limited", Some(429)),
            ("timed_out", None),
            ("failed", Some(500)),
            ("failed", Some(503)),
        ] {
            let spec = runtime_fallback_retry_spec(
                &assignment,
                &receipt,
                false,
                status,
                http,
                key_present,
            );
            assert_eq!(
                spec.as_ref().map(|spec| spec.provider),
                Some(ModelProvider::OpenCodeGo),
                "{status} {http:?} should retry on the fallback"
            );
        }

        // Deterministic failures never retry.
        for (status, http) in [
            ("auth_failed", Some(401)),
            ("invalid_json", Some(200)),
            ("bad_envelope", Some(200)),
            ("failed", Some(404)),
            ("failed", None),
        ] {
            assert!(
                runtime_fallback_retry_spec(
                    &assignment,
                    &receipt,
                    false,
                    status,
                    http,
                    key_present
                )
                .is_none(),
                "{status} {http:?} must not retry"
            );
        }

        // One retry per lane; a lane already on its fallback is terminal.
        assert!(
            runtime_fallback_retry_spec(
                &assignment,
                &receipt,
                true,
                "rate_limited",
                Some(429),
                key_present
            )
            .is_none()
        );
        let mut fallback_receipt = model_lane_receipt("security", "running");
        fallback_receipt.fallback_from = Some(primary.label());
        assert!(
            runtime_fallback_retry_spec(
                &assignment,
                &fallback_receipt,
                false,
                "rate_limited",
                Some(429),
                key_present
            )
            .is_none()
        );

        // No fallback key, or no fallback at all: terminal.
        assert!(
            runtime_fallback_retry_spec(
                &assignment,
                &receipt,
                false,
                "rate_limited",
                Some(429),
                key_absent
            )
            .is_none()
        );
        let no_fallback = ModelAssignment {
            lane: lane_plan("security"),
            spec: primary,
            fallback: None,
        };
        assert!(
            runtime_fallback_retry_spec(
                &no_fallback,
                &receipt,
                false,
                "rate_limited",
                Some(429),
                key_present
            )
            .is_none()
        );
    }

    #[test]
    fn model_lane_rate_limited_primary_retries_once_on_fallback() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.max_model_calls = 4;
        args.model_concurrency = 2;
        let primary = direct_minimax_spec(&args);
        let fallback = opencode_canary_spec(&args);
        let assignments = vec![ModelAssignment {
            lane: lane_plan("security"),
            spec: primary.clone(),
            fallback: Some(fallback.clone()),
        }];
        let preflights = vec![preflight_ok_receipt(&primary)];
        let mut model_lanes = vec![model_lane_receipt("security", "planned")];
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        let line_map = BTreeSet::new();

        let calls = run_available_model_lanes_with_runner(
            ModelRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                assignments: &assignments,
                provider_preflights: &preflights,
                shared_context: "shared context",
                args: &args,
                line_map: &line_map,
                key_present: |_| true,
                provider_concurrency: ProviderConcurrencyLimits::default(),
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
            &mut Vec::new(),
            &mut issue_candidates,
            |_context, _model_dir, tasks| {
                Ok(tasks
                    .into_iter()
                    .map(|task| {
                        let result = match task.spec.provider {
                            ModelProvider::MiniMaxDirect => Err(anyhow::anyhow!(
                                "model curl: http status Some(429) too many requests"
                            )),
                            ModelProvider::OpenCodeGo => Ok(ModelCallOutcome {
                                output: empty_lane_output(),
                                duration_ms: 5,
                                http_status: Some(200),
                                response_shape: "anthropic-messages".to_owned(),
                                cache_usage: ModelCacheUsage::default(),
                                degraded: false,
                            }),
                        };
                        ModelLaneTaskResult {
                            index: task.index,
                            result,
                        }
                    })
                    .collect())
            },
        )?;

        assert_eq!(calls, 2, "primary attempt plus one fallback retry");
        assert_eq!(model_lanes[0].status, "ok");
        assert_eq!(
            model_lanes[0].reason,
            "completed after runtime fallback retry"
        );
        assert_eq!(model_lanes[0].provider, fallback.provider.key());
        assert_eq!(model_lanes[0].fallback_from, Some(primary.label()));
        assert!(
            missing_or_failed_model_evidence.is_empty(),
            "a recovered lane is not a model evidence gap: {missing_or_failed_model_evidence:?}"
        );
        Ok(())
    }

    #[test]
    fn model_lane_rate_limited_provider_sheds_next_wave_to_fallback() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.max_model_calls = 6;
        args.model_concurrency = 3;
        let primary = direct_minimax_spec(&args);
        let fallback = opencode_canary_spec(&args);
        let assignments = ["security", "opposition", "tests-oracle"]
            .into_iter()
            .map(|lane| ModelAssignment {
                lane: lane_plan(lane),
                spec: primary.clone(),
                fallback: Some(fallback.clone()),
            })
            .collect::<Vec<_>>();
        let preflights = vec![
            preflight_ok_receipt(&primary),
            preflight_ok_receipt(&fallback),
        ];
        let mut model_lanes = assignments
            .iter()
            .map(|assignment| model_lane_receipt(&assignment.lane.id, "planned"))
            .collect::<Vec<_>>();
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        let line_map = BTreeSet::new();
        let waves = std::cell::RefCell::new(Vec::<Vec<ModelProvider>>::new());

        let calls = run_available_model_lanes_with_runner(
            ModelRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                assignments: &assignments,
                provider_preflights: &preflights,
                shared_context: "shared context",
                args: &args,
                line_map: &line_map,
                key_present: |_| true,
                provider_concurrency: ProviderConcurrencyLimits {
                    minimax: Some(1),
                    opencode_go: Some(3),
                },
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
            &mut Vec::new(),
            &mut issue_candidates,
            |_context, _model_dir, tasks| {
                waves.borrow_mut().push(
                    tasks
                        .iter()
                        .map(|task| task.spec.provider)
                        .collect::<Vec<_>>(),
                );
                Ok(tasks
                    .into_iter()
                    .map(|task| {
                        let result = match task.spec.provider {
                            ModelProvider::MiniMaxDirect => Err(anyhow::anyhow!(
                                "model curl: http status Some(429) too many requests"
                            )),
                            ModelProvider::OpenCodeGo => Ok(ModelCallOutcome {
                                output: empty_lane_output(),
                                duration_ms: 5,
                                http_status: Some(200),
                                response_shape: "anthropic-messages".to_owned(),
                                cache_usage: ModelCacheUsage::default(),
                                degraded: false,
                            }),
                        };
                        ModelLaneTaskResult {
                            index: task.index,
                            result,
                        }
                    })
                    .collect())
            },
        )?;

        assert_eq!(
            waves.into_inner(),
            vec![
                vec![ModelProvider::MiniMaxDirect],
                vec![
                    ModelProvider::OpenCodeGo,
                    ModelProvider::OpenCodeGo,
                    ModelProvider::OpenCodeGo,
                ],
            ]
        );
        assert_eq!(
            calls, 4,
            "one primary failure, one queued retry, and two backpressure fallbacks"
        );
        assert!(model_lanes.iter().all(|receipt| receipt.status == "ok"));
        assert!(
            model_lanes
                .iter()
                .all(|receipt| receipt.provider == fallback.provider.key())
        );
        assert!(
            model_lanes
                .iter()
                .all(|receipt| receipt.fallback_from == Some(primary.label()))
        );
        assert_eq!(
            model_lanes[0].reason,
            "completed after runtime fallback retry"
        );
        assert!(
            model_lanes[1..]
                .iter()
                .all(|receipt| receipt.reason == "completed after provider backpressure fallback")
        );
        assert!(missing_or_failed_model_evidence.is_empty());
        Ok(())
    }

    #[test]
    fn model_lane_rate_limited_retry_provider_sheds_next_wave_to_fallback() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.max_model_calls = 4;
        args.model_concurrency = 1;
        let minimax = direct_minimax_spec(&args);
        let opencode = opencode_canary_spec(&args);
        let assignments = vec![
            ModelAssignment {
                lane: lane_plan("security"),
                spec: minimax.clone(),
                fallback: Some(opencode.clone()),
            },
            ModelAssignment {
                lane: lane_plan("opencode-canary"),
                spec: opencode.clone(),
                fallback: Some(minimax.clone()),
            },
        ];
        let preflights = vec![
            preflight_ok_receipt(&minimax),
            preflight_ok_receipt(&opencode),
        ];
        let mut model_lanes = assignments
            .iter()
            .map(|assignment| model_lane_receipt(&assignment.lane.id, "planned"))
            .collect::<Vec<_>>();
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        let line_map = BTreeSet::new();
        let waves = std::cell::RefCell::new(Vec::<Vec<ModelProvider>>::new());

        let calls = run_available_model_lanes_with_runner(
            ModelRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                assignments: &assignments,
                provider_preflights: &preflights,
                shared_context: "shared context",
                args: &args,
                line_map: &line_map,
                key_present: |_| true,
                provider_concurrency: ProviderConcurrencyLimits::default(),
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
            &mut Vec::new(),
            &mut issue_candidates,
            |_context, _model_dir, tasks| {
                let mut waves = waves.borrow_mut();
                let wave_index = waves.len();
                waves.push(
                    tasks
                        .iter()
                        .map(|task| task.spec.provider)
                        .collect::<Vec<_>>(),
                );
                Ok(tasks
                    .into_iter()
                    .map(|task| {
                        let result = match (wave_index, task.spec.provider) {
                            (0, ModelProvider::MiniMaxDirect) => Err(anyhow::anyhow!(
                                "model curl: http status Some(429) too many requests"
                            )),
                            (1, ModelProvider::OpenCodeGo) => Err(anyhow::anyhow!(
                                "model curl: http status Some(429) too many requests"
                            )),
                            (_, ModelProvider::MiniMaxDirect) => Ok(ModelCallOutcome {
                                output: empty_lane_output(),
                                duration_ms: 5,
                                http_status: Some(200),
                                response_shape: "anthropic-messages".to_owned(),
                                cache_usage: ModelCacheUsage::default(),
                                degraded: false,
                            }),
                            (_, ModelProvider::OpenCodeGo) => Err(anyhow::anyhow!(
                                "unexpected OpenCode call after retry-provider backpressure"
                            )),
                        };
                        ModelLaneTaskResult {
                            index: task.index,
                            result,
                        }
                    })
                    .collect())
            },
        )?;

        assert_eq!(
            waves.into_inner(),
            vec![
                vec![ModelProvider::MiniMaxDirect],
                vec![ModelProvider::OpenCodeGo],
                vec![ModelProvider::MiniMaxDirect],
            ]
        );
        assert_eq!(calls, 3);
        assert_eq!(model_lanes[0].status, "rate_limited");
        assert_eq!(model_lanes[0].provider, opencode.provider.key());
        assert_eq!(model_lanes[0].fallback_from, Some(minimax.label()));
        assert_eq!(model_lanes[1].status, "ok");
        assert_eq!(model_lanes[1].provider, minimax.provider.key());
        assert_eq!(model_lanes[1].fallback_from, Some(opencode.label()));
        assert_eq!(
            model_lanes[1].reason,
            "completed after provider backpressure fallback"
        );
        assert_eq!(missing_or_failed_model_evidence.len(), 1);
        Ok(())
    }

    #[test]
    fn provider_backpressure_only_tracks_transient_provider_failures() {
        for (status, http_status) in [
            ("rate_limited", Some(429)),
            ("timed_out", None),
            ("failed", Some(500)),
            ("failed", Some(503)),
        ] {
            assert!(
                super::model_error_triggers_provider_backpressure(status, http_status),
                "{status} {http_status:?} should shed the provider"
            );
        }
        for (status, http_status) in [
            ("auth_failed", Some(401)),
            ("invalid_json", Some(200)),
            ("bad_envelope", Some(200)),
            ("failed", Some(404)),
            ("failed", None),
        ] {
            assert!(
                !super::model_error_triggers_provider_backpressure(status, http_status),
                "{status} {http_status:?} should not shed the provider"
            );
        }
    }

    #[test]
    fn model_lane_queued_retry_starved_by_budget_is_skipped_not_leaked() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        // Exactly one call of budget: the primary attempt consumes it, the
        // queued fallback retry can never run. The loop must terminate and
        // the lane must end terminal (skipped/budget), never silently
        // `planned`.
        args.max_model_calls = 1;
        args.model_concurrency = 2;
        let primary = direct_minimax_spec(&args);
        let fallback = opencode_canary_spec(&args);
        let assignments = vec![ModelAssignment {
            lane: lane_plan("security"),
            spec: primary.clone(),
            fallback: Some(fallback),
        }];
        let preflights = vec![preflight_ok_receipt(&primary)];
        let mut model_lanes = vec![model_lane_receipt("security", "planned")];
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        let line_map = BTreeSet::new();

        let calls = run_available_model_lanes_with_runner(
            ModelRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                assignments: &assignments,
                provider_preflights: &preflights,
                shared_context: "shared context",
                args: &args,
                line_map: &line_map,
                key_present: |_| true,
                provider_concurrency: ProviderConcurrencyLimits::default(),
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
            &mut Vec::new(),
            &mut issue_candidates,
            |_context, _model_dir, tasks| {
                Ok(tasks
                    .into_iter()
                    .map(|task| ModelLaneTaskResult {
                        index: task.index,
                        result: Err(anyhow::anyhow!(
                            "model curl: http status Some(429) too many requests"
                        )),
                    })
                    .collect())
            },
        )?;

        assert_eq!(
            calls, 1,
            "budget caps the retry; calls never exceed max_model_calls"
        );
        assert_eq!(
            model_lanes[0].status, "skipped",
            "a starved retry is terminal, not leaked as planned"
        );
        assert_eq!(missing_or_failed_model_evidence.len(), 1);
        Ok(())
    }

    #[test]
    fn model_lane_retryable_failure_without_fallback_is_terminal() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.max_model_calls = 4;
        args.model_concurrency = 2;
        let primary = direct_minimax_spec(&args);
        let assignments = vec![ModelAssignment {
            lane: lane_plan("security"),
            spec: primary.clone(),
            fallback: None,
        }];
        let preflights = vec![preflight_ok_receipt(&primary)];
        let mut model_lanes = vec![model_lane_receipt("security", "planned")];
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        let line_map = BTreeSet::new();

        let calls = run_available_model_lanes_with_runner(
            ModelRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                assignments: &assignments,
                provider_preflights: &preflights,
                shared_context: "shared context",
                args: &args,
                line_map: &line_map,
                key_present: |_| true,
                provider_concurrency: ProviderConcurrencyLimits::default(),
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
            &mut Vec::new(),
            &mut issue_candidates,
            |_context, _model_dir, tasks| {
                Ok(tasks
                    .into_iter()
                    .map(|task| ModelLaneTaskResult {
                        index: task.index,
                        result: Err(anyhow::anyhow!(
                            "model curl: http status Some(429) too many requests"
                        )),
                    })
                    .collect())
            },
        )?;

        assert_eq!(calls, 1, "no fallback means no retry");
        assert_eq!(model_lanes[0].status, "rate_limited");
        assert_eq!(model_lanes[0].fallback_from, None);
        assert_eq!(missing_or_failed_model_evidence.len(), 1);
        Ok(())
    }

    #[test]
    fn model_lane_failed_fallback_retry_is_terminal() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.max_model_calls = 4;
        args.model_concurrency = 2;
        let primary = direct_minimax_spec(&args);
        let fallback = opencode_canary_spec(&args);
        let assignments = vec![ModelAssignment {
            lane: lane_plan("security"),
            spec: primary.clone(),
            fallback: Some(fallback),
        }];
        let preflights = vec![preflight_ok_receipt(&primary)];
        let mut model_lanes = vec![model_lane_receipt("security", "planned")];
        let mut missing_or_failed_model_evidence = Vec::new();
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();
        let line_map = BTreeSet::new();

        let calls = run_available_model_lanes_with_runner(
            ModelRunContext {
                root: temp.path(),
                review_dir: temp.path(),
                assignments: &assignments,
                provider_preflights: &preflights,
                shared_context: "shared context",
                args: &args,
                line_map: &line_map,
                key_present: |_| true,
                provider_concurrency: ProviderConcurrencyLimits::default(),
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
            &mut Vec::new(),
            &mut issue_candidates,
            |_context, _model_dir, tasks| {
                Ok(tasks
                    .into_iter()
                    .map(|task| ModelLaneTaskResult {
                        index: task.index,
                        result: Err(anyhow::anyhow!(
                            "model curl: http status Some(429) too many requests"
                        )),
                    })
                    .collect())
            },
        )?;

        assert_eq!(calls, 2, "exactly one retry; the loop must terminate");
        assert_eq!(model_lanes[0].status, "rate_limited");
        assert_eq!(model_lanes[0].fallback_from, Some(primary.label()));
        assert_eq!(
            missing_or_failed_model_evidence.len(),
            1,
            "the terminal failure is recorded once as a model evidence gap"
        );
        Ok(())
    }

    #[test]
    fn refuter_drops_high_confidence_false_positive() {
        let mut inline_comments = vec![ReviewInlineComment {
            lane: "ub-active-view".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            side: "RIGHT".to_owned(),
            body: "[ub-active-view] The view length can diverge from backing storage.".to_owned(),
            evidence: "candidate evidence".to_owned(),
            suggestion: None,
        }];
        let mut summary_only_findings = Vec::new();
        let output = RefuterOutput {
            decisions: vec![RefuterDecision {
                path: "src/lib.rs".to_owned(),
                line: 2,
                disposition: "drop".to_owned(),
                confidence: Some("high".to_owned()),
                reason: "contradicted by the diff context".to_owned(),
            }],
        };

        apply_refuter_output(output, &mut inline_comments, &mut summary_only_findings);

        assert!(inline_comments.is_empty());
        assert!(summary_only_findings.is_empty());
    }

    #[test]
    fn github_review_payload_requires_comment_event_and_right_side() -> Result<()> {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
";
        let line_map = right_side_diff_lines(patch);
        let ok = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Verification questions\n\n- Confirm the added regression test fails on base+tests, not only that it passes on HEAD.".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests] This test reaches the helper but does not assert the boundary.".to_owned(),
                suggestion: None,
            }],
        };
        validate_github_review_payload(&ok)?;

        let temp = tempfile::tempdir()?;
        write_github_review_payload(
            temp.path(),
            &ok,
            &line_map,
            &ReviewBodyPolicy::default(),
            false,
        )?;
        assert!(temp.path().join("github-review.json").exists());

        let stale_line = GitHubReview {
            comments: vec![GitHubReviewComment {
                line: 99,
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        let stale_line_out = tempfile::tempdir()?;
        let err = write_github_review_payload(
            stale_line_out.path(),
            &stale_line,
            &line_map,
            &ReviewBodyPolicy::default(),
            false,
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("stale line unexpectedly wrote github-review.json"))?;
        assert!(err.to_string().contains("not a valid RIGHT-side diff line"));
        assert!(!stale_line_out.path().join("github-review.json").exists());

        let bad_event = GitHubReview {
            event: "APPROVE".to_owned(),
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&bad_event).is_err());
        let bad_event_out = tempfile::tempdir()?;
        assert!(
            write_github_review_payload(
                bad_event_out.path(),
                &bad_event,
                &line_map,
                &ReviewBodyPolicy::default(),
                false
            )
            .is_err()
        );
        assert!(!bad_event_out.path().join("github-review.json").exists());

        let bad_side = GitHubReview {
            comments: vec![GitHubReviewComment {
                side: "LEFT".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&bad_side).is_err());
        let bad_out = tempfile::tempdir()?;
        assert!(
            write_github_review_payload(
                bad_out.path(),
                &bad_side,
                &line_map,
                &ReviewBodyPolicy::default(),
                false
            )
            .is_err()
        );
        assert!(!bad_out.path().join("github-review.json").exists());

        let parent_path = GitHubReview {
            comments: vec![GitHubReviewComment {
                path: "../src/lib.rs".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&parent_path).is_err());

        let empty_body = GitHubReview {
            comments: vec![GitHubReviewComment {
                body: " ".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&empty_body).is_err());

        let missing_prefix = GitHubReview {
            comments: vec![GitHubReviewComment {
                body: "This test reaches the helper but does not assert the boundary.".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&missing_prefix).is_err());

        let overlong_body = GitHubReview {
            comments: vec![GitHubReviewComment {
                body: format!("[tests] {}", "x".repeat(1_201)),
                ..ok.comments[0].clone()
            }],
            ..ok
        };
        assert!(validate_github_review_payload(&overlong_body).is_err());
        Ok(())
    }

    #[test]
    fn github_review_payload_rejects_pr_body_boilerplate() -> Result<()> {
        let mut review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Model lanes\n\n- Lane: `ub`\n  Provider: `minimax`\n  Model: `m3`\n  Status: `ok` - completed".to_owned(),
            comments: Vec::new(),
        };

        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("model lane table unexpectedly passed"))?;
        assert!(err.to_string().contains("successful lane table"), "{err:#}");

        review.body = "## Decision\n\n- No blocking finding after bounded review; residual risk remains for human review.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("no-finding boilerplate unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "- Profile: `gh-runner`\n- Base: `origin/main`\n- Head: `HEAD`".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("execution summary unexpectedly passed"))?;
        assert!(err.to_string().contains("execution summary"), "{err:#}");

        review.body =
            "## Test proof\n\n- Provider status was ok and the model lane roster completed."
                .to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("status boilerplate unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        assert!(super::contains_successful_lane_table(
            "## model lanes\n\n- tests: ok"
        ));
        assert!(super::contains_provider_status_table(
            "## provider preflights\n\n- minimax: ok"
        ));
        assert!(super::contains_sensor_status_table(
            "## sensor receipts\n\n- ripr: ok"
        ));
        assert!(super::contains_execution_summary("runtime: 31s"));
        assert!(!super::contains_execution_summary(
            "## Evidence gaps\n\n- Check the `runtime:` field in the proof receipt."
        ));

        review.body = "## Decision\n\n- Needs proof.\n\n## model lanes\n\n- tests: ok".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("lowercase model lane table unexpectedly passed"))?;
        assert!(err.to_string().contains("successful lane table"), "{err:#}");

        review.body =
            "## Decision\n\n- Needs proof.\n\n## provider preflights\n\n- minimax: ok".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("lowercase provider table unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body =
            "## Decision\n\n- Needs proof.\n\n## sensor receipts\n\n- ripr: ok".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("lowercase sensor table unexpectedly passed"))?;
        assert!(err.to_string().contains("sensor status table"), "{err:#}");

        review.body = "## Evidence gaps\n\nruntime: 31s".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("lowercase runtime summary unexpectedly passed"))?;
        assert!(err.to_string().contains("execution summary"), "{err:#}");

        review.body =
            "## Evidence gaps\n\n- Check the `runtime:` field in the proof receipt.".to_owned();
        validate_github_review_payload(&review)?;

        review.body = "## Residual risk\n\n- External trust risk remains.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("residual-risk section unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Verification questions\n\n- Confirm the cached prior observation still matches; the refuter demoted inline candidate because Gate proof is pending.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("meta review prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Confirmed findings\n\n- Ub-review action receives secrets.MINIMAX_API_KEY and github.token at runtime; a malicious or compromised dad0f23 would exfiltrate these. Pinning to SHA is correct posture but does not eliminate upstream trust.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("standing workflow trust prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Confirmed findings\n\n- No pinning defect introduced. The only standing concern is upstream SHA trust for EffortlessMetrics/ub-review@e76ccbc, which is identical in posture to the prior pin and is a repo-level policy item, not a diff finding.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("no-defect pinning prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Confirmed findings\n\n- The diff is a 4-line mechanical SHA bump at the three expected sites: cache `key`, `restore-keys` prefix, and action `uses:`. No permission, trigger, or `with:` block change; net new secret/permission surface relative to the prior pin is zero.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("mechanical pin no-change prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Decision\n\n- Needs one verification check before upstream.\n\n## Verification questions\n\n- Confirm checkout credential persistence: workflows using pull_request from forks receive a read-only GITHUB_TOKEN; this lane did not change checkout config, so no new persistence vector is introduced. Actionlint receipt 'ok' supports no syntactic regression.\n\n## Refuted\n\n- Adding a workflow file to paths-ignore could grant implicit permission expansion; refuted because: paths-ignore only filters trigger activation; it does not alter token scopes, permissions blocks, or any job-level security context.\n\n## Parked follow-ups\n\n- paths-ignore is a literal substring/glob match; a future rename of ub-review-packet.yml silently re-enables Droid noise.\n\n## Evidence gaps\n\n- zizmor, gitleaks, osv-scanner, cargo-audit, cargo-deny, shellcheck, semgrep, coverage all disabled by config or trigger-mismatched. No security/pinning tool independently re-validated this workflow file.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("paths-ignore no-posture review unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Decision\n\n- Needs one verification check before upstream.\n\n## Verification questions\n\n- Confirm no focused smoke proof (workflow_run on a fork-PR dry-run, or a temporary pull_request_target guard test) was executed for the paths-ignore change. Trust rests on actionlint parse only; semantic skip behavior on the droid lane is not proven by sensors.\n\n## Refuted\n\n- adding ub-review-packet.yml to paths-ignore could mask future unpinned uses: additions in that file from Droid lane coverage; refuted because: paths-ignore lift is per-PR: any future PR that also touches ub-review-packet.yml (i.e. adds/changes uses:) will change the changed-files set and re-trigger Droid. Droid lanes are non-blocking/auxiliary by design; UB gate is the authoritative review.\n\n## Evidence gaps\n\n- PR body states actionlint is not installed locally, so the 'ok' receipt must come from the ub-review gate's own tooling rather than a local pre-push run; trust depends on that gate having actually executed actionlint v1 against this ref.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| {
                anyhow::anyhow!("paths-ignore smoke-proof review unexpectedly passed")
            })?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Decision\n\n- Needs one verification check before upstream.\n\n## Verification questions\n\n- Confirm actionlint receipt 'ok' confirms syntactic validity, but no semantic proof of skip behavior on the droid lane is available; trust rests on actionlint parse plus per-PR trigger semantics - the droid lane is auxiliary/non-blocking and the UB gate is authoritative, so residual workflow risk is bounded.\n\n## Refuted\n\n- paths-ignore addition could mask future unpinned uses: additions in ub-review-packet.yml from Droid lane coverage; refuted because: paths-ignore lift is per-PR: any future PR that also touches ub-review-packet.yml (adds/changes uses:) will change the changed-files set and re-trigger Droid. UB gate is the authoritative review surface and runs on the new pin.\n\n## Parked follow-ups\n\n- Residual workflow risk: cache key/restore-keys prefix is coupled to action SHA. Any future repin must update all three sites; a partial update silently mismatches cache restore. Not actionable in this PR (current state is consistent) - parked for follow-up lint rule or script.\n\n## Evidence gaps\n\n- trust gap: no focused smoke proof (workflow_run on fork-PR dry-run or pull_request_target guard) executed for the paths-ignore change; semantic skip behavior on Droid lane unproven beyond actionlint parse.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| {
                anyhow::anyhow!("paths-ignore actionlint skip-proof review unexpectedly passed")
            })?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Decision\n\n- Needs one verification check before upstream.\n\n## Verification questions\n\n- Workflow-pinning lane for PR #49. Two workflow YAML files touched. Pin lockstep verified across 3 sites, old pin absent, cache key/restore-keys prefix match, no other third-party actions changed.\n\n## Parked follow-ups\n\n- Cache key/restore-keys prefix is coupled to action SHA; any future partial repin silently mismatches restore. Current state consistent, parked for lint-rule follow-up.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| {
                anyhow::anyhow!("workflow lockstep summary review unexpectedly passed")
            })?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = format!(
            "## Decision\n\n- Needs focused cleanup before merge.\n\n## Verification questions\n\n{}",
            (1..=13)
                .map(|index| format!("- Confirm decision-relevant proof item {index}."))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("overlong bullet list unexpectedly passed"))?;
        assert!(err.to_string().contains("not concise enough"), "{err:#}");

        review.body = format!(
            "## Decision\n\n- Needs focused cleanup before merge.\n\n## Evidence gaps\n\n- {}",
            "proof gap ".repeat(800)
        );
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("oversized body unexpectedly passed"))?;
        assert!(err.to_string().contains("not concise enough"), "{err:#}");

        review.body = "## Confirmed findings\n\n- CodeRabbit's review-comment at ub-review-packet.yml:58 asserts the PR gate target SHA is 892e1bb44b7cb24753b7701b405d078f4ef11ee1, not be524219e33ff37edeab61ddc28c01250a08b492 used in the diff. If that claim is correct the workflow pin does not match the upstream gate.\n\n## Evidence gaps\n\n- CodeRabbit review-comment on .github/workflows/ub-review-packet.yml:58, scripted check showing 0 references to 892e1bb44b... in the file; PR body and droid-ub/droid-tests receipts only confirm internal lockstep, not match to gate target.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("stale bot target-SHA prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Refuted\n\n- cursor[bot] and coderabbitai[bot] comments claim target is e76ccbcb... and demand swap back; PR body, diff, and head tree all show ec8f890 as the actual target. Their objection is a false positive against the current diff and reopens nothing.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("stale bot refutation prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body =
            "## Refuted\n\n- A prior objection was false, and no finding remains.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("refuted-only body unexpectedly passed"))?;
        assert!(
            err.to_string().contains("refuted-only artifact note"),
            "{err:#}"
        );

        review.body = "## Evidence gaps\n\n- actionlint receipt is 'ok' per sensor table; no per-line output inlined into this lane packet, so re-verification of lint findings depends on the central proof broker artifact.\n- No fresh PR-build smoke run is available (build/test skipped, --allow-heavy required); only tokmd/actionlint receipts are present for this 4-line workflow pin.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("workflow tool-status gap prose unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        review.body = "## Evidence gaps\n\n- Shared context hash and terminal state are available in artifacts.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("artifact pointer boilerplate unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        for phrase in [
            "pre-existing, not a diff target",
            "identical to prior pin",
            "no widened attack surface",
            "standing-repo concern",
        ] {
            review.body =
                format!("## Evidence gaps\n\n- This is {phrase}; leave it out of the PR body.");
            let err = validate_github_review_payload(&review)
                .err()
                .ok_or_else(|| anyhow::anyhow!("{phrase:?} boilerplate unexpectedly passed"))?;
            assert!(
                err.to_string().contains("artifact-only boilerplate"),
                "{phrase}: {err:#}"
            );
        }

        review.body = "## Evidence gaps\n\n- Command log: cargo test focused_case exited 0; stdout says focused_case passed.".to_owned();
        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("command-log boilerplate unexpectedly passed"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        for phrase in [
            "All checks passed.",
            "Looks good after we ran the focused proof.",
            "No issues found in the changed files.",
            "LGTM because the tests passed.",
            "We ran cargo test and it passed.",
            "I ran clippy and it passed.",
        ] {
            review.body = format!("## Decision\n\n- {phrase}");
            let err = validate_github_review_payload(&review)
                .err()
                .ok_or_else(|| anyhow::anyhow!("{phrase:?} approval filler unexpectedly passed"))?;
            assert!(
                err.to_string().contains("artifact-only boilerplate"),
                "{phrase}: {err:#}"
            );
        }

        review.body = "## Verification questions\n\n- Confirm `CI ran cargo test` stays valid evidence for the literal `\"i ran \"` needle.".to_owned();
        validate_github_review_payload(&review)?;
        Ok(())
    }

    #[test]
    fn github_review_payload_rejects_inline_comment_boilerplate() -> Result<()> {
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Verification questions\n\n- Confirm the focused proof.".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests] No actionable findings after checking this path.".to_owned(),
                suggestion: None,
            }],
        };

        let err = validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("inline boilerplate unexpectedly passed"))?;
        assert!(
            err.to_string()
                .contains("comment contains artifact-only boilerplate"),
            "{err:#}"
        );

        let allowed = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Verification questions\n\n- Confirm the focused proof.".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[contract-mirror] Raw `\"i ran \"` must not reject CI evidence such as `CI ran cargo test`.".to_owned(),
                suggestion: None,
            }],
        };
        validate_github_review_payload(&allowed)?;
        Ok(())
    }

    #[test]
    fn review_body_policy_allows_configured_execution_summary_on_failure() -> Result<()> {
        let policy = ReviewBodyPolicy {
            include_execution_summary: ReviewBodyExecutionSummaryPolicy::OnFailure,
            ..ReviewBodyPolicy::default()
        };
        validate_pr_review_body_policy(
            "## Evidence gaps\n\n- Focused proof timed out.\n\nRuntime: `31s`",
            &policy,
        )?;
        let err = validate_pr_review_body_policy("Runtime: `31s`", &policy)
            .err()
            .ok_or_else(|| anyhow::anyhow!("success execution summary unexpectedly passed"))?;
        assert!(
            err.to_string().contains("success execution summary"),
            "{err:#}"
        );
        Ok(())
    }

    #[test]
    fn github_review_post_payload_requires_recorded_right_diff_line() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let diff_patch = temp.path().join("diff.patch");
        fs::write(
            &diff_patch,
            "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
",
        )?;
        let args = PostArgs {
            review_json: temp.path().join("github-review.json"),
            diff_patch: Some(diff_patch),
            out: temp.path().join("post"),
            github_token: Some("token".to_owned()),
            repo: Some("EffortlessMetrics/ub-review".to_owned()),
            pull_number: Some(1),
            github_api_url: "https://api.github.com".to_owned(),
            fail_on_post_error: false,
        };
        let ok = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "Review body".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests] This test reaches the helper but not the boundary.".to_owned(),
                suggestion: None,
            }],
        };
        validate_github_review_payload_for_post(&args, &ok)?;

        let stale_line = GitHubReview {
            comments: vec![GitHubReviewComment {
                line: 99,
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        let err = validate_github_review_payload_for_post(&args, &stale_line)
            .err()
            .ok_or_else(|| anyhow::anyhow!("stale line unexpectedly passed diff validation"))?;
        assert!(err.to_string().contains("not a valid RIGHT-side diff line"));

        let wrong_file = GitHubReview {
            comments: vec![GitHubReviewComment {
                path: "src/other.rs".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok
        };
        let err = validate_github_review_payload_for_post(&args, &wrong_file)
            .err()
            .ok_or_else(|| anyhow::anyhow!("wrong file unexpectedly passed diff validation"))?;
        assert!(err.to_string().contains("not a valid RIGHT-side diff line"));
        Ok(())
    }

    #[test]
    fn post_command_accepts_explicit_skip_receipt_without_token() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_json = temp.path().join("review").join("github-review.json");
        let review_dir = review_json
            .parent()
            .ok_or_else(|| anyhow::anyhow!("review json parent missing"))?;
        fs::create_dir_all(review_dir)?;
        fs::write(
            github_review_skip_path(&review_json),
            serde_json::json!({
                "schema_version": 1,
                "status": "skipped",
                "reason": "empty smoke review",
                "review_payload_status": "skipped_empty_smoke"
            })
            .to_string(),
        )?;
        let args = PostArgs {
            review_json,
            diff_patch: None,
            out: temp.path().join("post"),
            github_token: None,
            repo: None,
            pull_number: None,
            github_api_url: "https://api.github.com".to_owned(),
            fail_on_post_error: true,
        };

        cmd_post(args)?;

        let result: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("post/post-result.json"))?)?;
        assert_eq!(result["status"], "skipped");
        assert_eq!(result["review_payload_status"], "skipped_empty_smoke");
        assert!(!temp.path().join("post/post-error.json").exists());
        Ok(())
    }

    #[test]
    fn ledger_context_reads_configured_file_bounded() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("ub-ledger.md"),
            "RAB resize follow-up: verify post-capture mutation.",
        )?;
        let mut config = Config::default();
        config.repo.ledger = "ub-ledger.md".to_owned();

        let args = test_run_args(temp.path().join("out"));
        let context = render_ledger_context(temp.path(), &config, &args)?;

        assert!(context.contains("RAB resize follow-up"));
        assert!(context.contains("Source:"));
        Ok(())
    }

    #[test]
    fn pr_thread_context_reads_configured_file_bounded() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("thread.md"),
            "Author reply: ASAN bad-free receipt attached.\nThis tail should be truncated.",
        )?;
        let mut args = test_run_args(temp.path().join("out"));
        args.pr_thread_context = "thread.md".to_owned();
        args.pr_thread_context_max_bytes = 40;

        let context = collect_pr_thread_context(temp.path(), &args)?;
        let rendered = render_pr_thread_context(&context);

        assert_eq!(context.status, "seeded");
        assert!(
            context
                .thread_context_path
                .as_deref()
                .is_some_and(|path| path.ends_with("thread.md"))
        );
        assert!(
            context
                .thread_context
                .as_deref()
                .is_some_and(|text| text.contains("ASAN bad-free"))
        );
        assert!(context.thread_context_truncated);
        assert!(rendered.contains("### Prior Review Thread"));
        assert!(rendered.contains("[truncated]"));
        assert!(!rendered.contains("tail should be truncated"));
        Ok(())
    }

    #[test]
    fn seeded_pr_thread_context_tells_lanes_not_to_reask_answered_questions() {
        let mut context = test_pr_thread_context();
        context.status = "seeded".to_owned();
        context.title = Some("Harden bad-free proof".to_owned());
        context.body = Some(
            "PR body: base+tests receipt shows the new focused test fails on base and passes on HEAD."
                .to_owned(),
        );
        context.thread_context = Some(
            "Author reply: ASAN receipt attached; prior ub-review verification question is answered."
                .to_owned(),
        );

        let rendered = render_pr_thread_context(&context);

        assert!(rendered.contains("### Seeded Thread Reuse Rules"));
        assert!(rendered.contains("Treat PR body claims, author replies"));
        assert!(rendered.contains("proof receipts in this context as lane evidence"));
        assert!(rendered.contains("already answered and the current diff does not reopen it"));
        assert!(rendered.contains("`resolved-check` observation or `failed_objection`"));
        assert!(rendered.contains("makes the prior answer stale"));
    }

    #[test]
    fn absent_pr_thread_context_omits_reuse_rules() {
        let rendered = render_pr_thread_context(&test_pr_thread_context());

        assert!(!rendered.contains("### Seeded Thread Reuse Rules"));
        assert!(rendered.contains("- No PR thread context was provided for this run."));
    }

    #[test]
    fn pr_thread_context_reads_github_event_pr_metadata() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let event_path = temp.path().join("event.json");
        fs::write(
            &event_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "pull_request": {
                    "number": 37,
                    "title": "Harden FFI bad-free tests",
                    "body": "The ASAN receipt proves the old no-finalizer path fails on base+tests and passes on HEAD."
                }
            }))?,
        )?;

        let context = read_github_event_pr_context(&event_path, 48)?;

        assert_eq!(context.pull_number, Some(37));
        assert_eq!(context.title.as_deref(), Some("Harden FFI bad-free tests"));
        assert!(
            context
                .body
                .as_deref()
                .is_some_and(|body| body.contains("ASAN receipt"))
        );
        assert!(context.body_truncated);
        Ok(())
    }

    #[test]
    fn pr_thread_context_treats_non_pr_event_as_absent() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let event_path = temp.path().join("event.json");
        fs::write(
            &event_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "repository": {
                    "full_name": "EffortlessMetrics/ub-review"
                }
            }))?,
        )?;

        let context = read_github_event_pr_context(&event_path, 65_536)?;

        assert_eq!(context.pull_number, None);
        assert_eq!(context.title, None);
        assert_eq!(context.body, None);
        assert!(!context.body_truncated);
        Ok(())
    }

    #[test]
    fn pr_thread_context_truncates_github_event_body_on_utf8_boundary() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let event_path = temp.path().join("event.json");
        fs::write(
            &event_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "pull_request": {
                    "number": 38,
                    "title": "Non-ASCII PR body",
                    "body": "🔥 receipt attached"
                }
            }))?,
        )?;

        let context = read_github_event_pr_context(&event_path, 1)?;

        assert_eq!(context.pull_number, Some(38));
        assert_eq!(context.body.as_deref(), Some("\n[truncated]\n"));
        assert!(context.body_truncated);
        Ok(())
    }

    #[test]
    fn pr_thread_context_fetches_github_thread_snapshot() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (github_api_url, handle) = spawn_fake_github_thread_api(3)?;
        let mut args = test_run_args(temp.path().join("out"));
        args.pr_thread_auth = Some("thread-token-redacted".to_owned());
        args.github_repo = Some("EffortlessMetrics/ub-review".to_owned());
        args.github_pull_number = Some(76);
        args.github_api_url = github_api_url;
        args.pr_thread_context_max_bytes = 8_192;

        let context = collect_pr_thread_context(temp.path(), &args)?;
        let requests = join_fake_github_thread_api(handle)?;
        let rendered = render_pr_thread_context(&context);

        assert_eq!(requests.len(), 3);
        assert!(requests.iter().any(|request| request.contains(
            "GET /repos/EffortlessMetrics/ub-review/issues/76/comments?per_page=30 HTTP/1.1"
        )));
        assert!(requests.iter().any(|request| request.contains(
            "GET /repos/EffortlessMetrics/ub-review/pulls/76/reviews?per_page=30 HTTP/1.1"
        )));
        assert!(requests.iter().any(|request| request.contains(
            "GET /repos/EffortlessMetrics/ub-review/pulls/76/comments?per_page=50 HTTP/1.1"
        )));
        let expected_auth = format!(
            "{}: {} thread-token-redacted",
            "Authorization",
            ["Bear", "er"].concat()
        );
        assert!(
            requests
                .iter()
                .all(|request| request.contains(&expected_auth))
        );
        assert_eq!(context.status, "seeded");
        assert_eq!(context.pull_number, Some(76));
        assert!(context.sources.iter().any(|source| {
            source.contains("github-api:EffortlessMetrics/ub-review/76/issue-comments")
        }));
        assert!(
            context
                .thread_context
                .as_deref()
                .is_some_and(|thread| thread.contains("ASAN receipt attached"))
        );
        assert!(rendered.contains("## GitHub PR Thread Snapshot"));
        assert!(rendered.contains("ub-review previous question resolved"));
        assert!(rendered.contains("`src/lib.rs`:`12`"));
        assert!(!rendered.contains("thread-token-redacted"));
        Ok(())
    }

    #[test]
    fn provider_policy_minimax_only_uses_direct_m3() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::MinimaxOnly;
        args.lane_width = 6;
        let assignments = model_assignments(&test_plan(Vec::new()), &args)?;

        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].spec.provider, ModelProvider::MiniMaxDirect);
        assert_eq!(assignments[0].spec.model, "MiniMax-M3");
        assert!(assignments[0].fallback.is_none());
        Ok(())
    }

    #[test]
    fn auto_run_pass_maps_pull_request_actions() {
        assert_eq!(
            super::resolve_run_pass_from_event(Some("pull_request"), Some("opened")),
            super::RunPass::Opened
        );
        assert_eq!(
            super::resolve_run_pass_from_event(Some("pull_request"), Some("ready_for_review")),
            super::RunPass::ReadyForReview
        );
        assert_eq!(
            super::resolve_run_pass_from_event(Some("pull_request"), Some("reopened")),
            super::RunPass::Reopened
        );
        assert_eq!(
            super::resolve_run_pass_from_event(Some("pull_request"), Some("synchronize")),
            super::RunPass::Synchronize
        );
        assert_eq!(
            super::resolve_run_pass_from_event(Some("pull_request"), Some("labeled")),
            super::RunPass::PullRequestOther
        );
        assert_eq!(
            super::resolve_run_pass_from_event(Some("workflow_dispatch"), None),
            super::RunPass::Manual
        );
    }

    #[test]
    fn run_pass_parser_accepts_github_action_spelling() {
        assert_eq!(super::parse_run_pass("auto"), Ok(super::RunPass::Auto));
        assert_eq!(super::parse_run_pass("opened"), Ok(super::RunPass::Opened));
        assert_eq!(
            super::parse_run_pass("ready_for_review"),
            Ok(super::RunPass::ReadyForReview)
        );
        assert_eq!(
            super::parse_run_pass("ready-for-review"),
            Ok(super::RunPass::ReadyForReview)
        );
        assert_eq!(
            super::parse_run_pass("reopened"),
            Ok(super::RunPass::Reopened)
        );
        assert_eq!(
            super::parse_run_pass("synchronize"),
            Ok(super::RunPass::Synchronize)
        );
        assert_eq!(
            super::parse_run_pass("pull_request_other"),
            Ok(super::RunPass::PullRequestOther)
        );
        assert_eq!(super::parse_run_pass("manual"), Ok(super::RunPass::Manual));
        assert!(super::parse_run_pass("draft").is_err());
    }

    #[test]
    fn pass_policy_decision_matrix_honors_post_review_on() {
        let two_pass: Vec<String> = vec!["opened".to_owned(), "ready_for_review".to_owned()];
        let every_pass: Vec<String> = vec![
            "opened".to_owned(),
            "reopened".to_owned(),
            "ready_for_review".to_owned(),
            "synchronize".to_owned(),
        ];

        // posting=review: the profile list is authoritative per event action.
        for (pass, in_two_pass) in [
            (super::RunPass::Opened, true),
            (super::RunPass::Reopened, false),
            (super::RunPass::ReadyForReview, true),
            (super::RunPass::Synchronize, false),
        ] {
            assert_eq!(
                super::pass_policy_permits_review_post(PostingMode::Review, pass, &two_pass),
                in_two_pass,
                "two-pass default, pass {}",
                pass.key()
            );
            assert!(
                super::pass_policy_permits_review_post(PostingMode::Review, pass, &every_pass),
                "every-pass profile, pass {}",
                pass.key()
            );
        }
        // Catch-all PR passes have no event action in the list and never post.
        assert!(!super::pass_policy_permits_review_post(
            PostingMode::Review,
            super::RunPass::PullRequestOther,
            &every_pass
        ));
        // Manual runs are explicit operator requests; the PR pass policy does
        // not apply to them.
        assert!(super::pass_policy_permits_review_post(
            PostingMode::Review,
            super::RunPass::Manual,
            &two_pass
        ));
        // An unresolved `auto` leaking into compilation must DENY, not post:
        // admitting it would post on every profile regardless of
        // post_review_on (gate inline finding on run 27053765285).
        assert!(!super::pass_policy_permits_review_post(
            PostingMode::Review,
            super::RunPass::Auto,
            &every_pass
        ));
        // posting=artifact-only never posts, so payload preparation stays
        // unrestricted for every pass.
        for pass in [
            super::RunPass::Opened,
            super::RunPass::Reopened,
            super::RunPass::ReadyForReview,
            super::RunPass::Synchronize,
            super::RunPass::PullRequestOther,
            super::RunPass::Manual,
        ] {
            assert!(
                super::pass_policy_permits_review_post(PostingMode::ArtifactOnly, pass, &two_pass),
                "artifact-only, pass {}",
                pass.key()
            );
        }
    }

    #[test]
    fn run_mode_accepts_product_names_and_legacy_review_direct() {
        assert_eq!(super::RunMode::ReviewByok.key(), "review-byok");
        assert_eq!(super::RunMode::IntelligentCi.key(), "intelligent-ci");
        assert_eq!(
            <super::RunMode as clap::ValueEnum>::from_str("review-byok", true),
            Ok(super::RunMode::ReviewByok)
        );
        assert_eq!(
            <super::RunMode as clap::ValueEnum>::from_str("intelligent-ci", true),
            Ok(super::RunMode::IntelligentCi)
        );
        assert_eq!(
            <super::RunMode as clap::ValueEnum>::from_str("review-direct", true),
            Ok(super::RunMode::ReviewByok)
        );
    }

    #[test]
    fn quick_depth_expands_to_small_lane_budget() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.depth = ReviewDepth::Quick;

        let args = normalize_run_args(args)?;

        assert_eq!(args.lane_width, 6);
        assert_eq!(args.model_concurrency, 4);
        assert_eq!(args.max_model_calls, 6);
        assert_eq!(args.max_inline_comments, 8);
        assert_eq!(
            review_lanes_for_args(&test_plan(Vec::new()), &args).len(),
            1
        );
        Ok(())
    }

    #[test]
    fn deep_depth_expands_to_wide_lane_budget() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.depth = ReviewDepth::Deep;

        let args = normalize_run_args(args)?;

        assert_eq!(args.lane_width, 20);
        assert_eq!(args.model_concurrency, 8);
        assert_eq!(args.max_model_calls, 24);
        assert_eq!(args.max_inline_comments, 8);
        assert_eq!(
            review_lanes_for_args(&test_plan(Vec::new()), &args).len(),
            20
        );
        Ok(())
    }

    #[test]
    fn runtime_profile_caps_model_concurrency() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.model_concurrency = 99;
        let profiles = builtin_profiles();
        let cx23 = profiles
            .iter()
            .find(|profile| profile.name == "cx23")
            .ok_or_else(|| anyhow::anyhow!("missing cx23 profile"))?;

        apply_runtime_profile_limits(&mut args, cx23)?;

        assert_eq!(args.model_concurrency, cx23.limits.llm_in_flight);
        assert_eq!(args.model_concurrency, 12);
        Ok(())
    }

    #[test]
    fn gh_runner_runtime_profile_keeps_standard_model_concurrency() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;

        apply_runtime_profile_limits(&mut args, gh_runner)?;

        assert_eq!(args.model_concurrency, STANDARD_MODEL_CONCURRENCY);
        assert_eq!(args.model_concurrency, 8);
        Ok(())
    }

    #[test]
    fn zero_llm_runtime_limit_is_rejected() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let profile = Profile {
            name: "broken".to_owned(),
            limits: Limits {
                llm_in_flight: 0,
                ..Limits::default()
            },
            ..Profile::default()
        };

        let err = apply_runtime_profile_limits(&mut args, &profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("zero llm limit unexpectedly passed"))?;

        assert!(
            err.to_string()
                .contains("runtime profile broken has llm_in_flight=0")
        );
        Ok(())
    }

    #[test]
    fn nonstandard_depth_rejects_raw_budget_override() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.depth = ReviewDepth::Deep;
        args.max_model_calls = 30;

        let err = normalize_run_args(args)
            .err()
            .ok_or_else(|| anyhow::anyhow!("conflicting deep budget unexpectedly passed"))?;

        assert!(err.to_string().contains("--depth deep cannot be combined"));
        Ok(())
    }

    #[test]
    fn lane_selectors_filter_model_assignments() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.selectors.lanes = "tests-oracle,source-route".to_owned();

        let assignments = model_assignments(&test_plan(Vec::new()), &args)?;

        assert_eq!(
            assignments
                .iter()
                .map(|assignment| assignment.lane.id.as_str())
                .collect::<Vec<_>>(),
            vec!["tests-oracle", "source-route"]
        );
        Ok(())
    }

    #[test]
    fn except_lane_selector_filters_model_assignments() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.selectors.except_lanes = "security,opposition".to_owned();

        let assignments = model_assignments(&test_plan(Vec::new()), &args)?;

        assert!(
            !assignments
                .iter()
                .any(|assignment| matches!(assignment.lane.id.as_str(), "security" | "opposition"))
        );
        assert_eq!(assignments.len(), 8);
        Ok(())
    }

    #[test]
    fn unknown_lane_selector_is_rejected() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.selectors.lanes = "missing-lane".to_owned();

        let err = model_assignments(&test_plan(Vec::new()), &args)
            .err()
            .map(|err| err.to_string())
            .unwrap_or_default();

        assert!(err.contains("unknown lane selector"));
    }

    #[test]
    fn direct_minimax_openai_uses_chat_endpoint_and_bearer_header() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.minimax_provider_kind = ProviderKindArg::Openai;
        let spec = direct_minimax_spec(&args);

        assert_eq!(
            model_api_url(&spec),
            "https://api.minimax.io/v1/chat/completions"
        );
        assert_eq!(
            model_auth_header(&spec, "test-token"),
            "Authorization: Bearer test-token"
        );
    }

    #[test]
    fn direct_minimax_anthropic_uses_messages_endpoint_and_api_key_header() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.minimax_provider_kind = ProviderKindArg::Anthropic;
        let spec = direct_minimax_spec(&args);

        assert_eq!(
            model_api_url(&spec),
            "https://api.minimax.io/anthropic/v1/messages"
        );
        assert_eq!(
            model_auth_header(&spec, "test-token"),
            "X-Api-Key: test-token"
        );
    }

    #[test]
    fn provider_policy_opencode_canary_routes_only_opposition() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::OpencodeGoCanary;
        args.lane_width = 10;
        let assignments = model_assignments(&test_plan(Vec::new()), &args)?;

        let opposition = assignments
            .iter()
            .find(|assignment| assignment.lane.id == "opposition")
            .ok_or_else(|| anyhow::anyhow!("opposition lane missing"))?;
        assert_eq!(opposition.spec.provider, ModelProvider::OpenCodeGo);
        assert_eq!(opposition.spec.model, "minimax-m3");
        assert_eq!(
            opposition.spec.endpoint_kind,
            super::ProviderEndpointKind::AnthropicMessages
        );
        assert_eq!(
            opposition.fallback.as_ref().map(|spec| spec.provider),
            Some(ModelProvider::MiniMaxDirect)
        );
        assert!(assignments.iter().any(|assignment| {
            assignment.lane.id == "security"
                && assignment.spec.provider == ModelProvider::MiniMaxDirect
        }));
        Ok(())
    }

    #[test]
    fn minimax_primary_ignores_empty_opencode_key() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::MinimaxPrimary;
        args.lane_width = 10;
        let opposition_lane = review_lanes_for_args(&test_plan(Vec::new()), &args)
            .into_iter()
            .find(|lane| lane.id == "opposition")
            .ok_or_else(|| anyhow::anyhow!("opposition lane missing"))?;

        let spec = provider_spec_for_lane_with_key_state(&opposition_lane, &args, false);

        assert_eq!(spec.provider, ModelProvider::MiniMaxDirect);
        assert_eq!(spec.model, "MiniMax-M3");
        Ok(())
    }

    #[test]
    fn primary_with_fallback_routes_canary_and_fast_fallbacks() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::PrimaryWithFallback;
        args.lane_width = 10;
        args.opencode_model = "mimo-v2.5".to_owned();
        let assignments = model_assignments_with_key_state(&test_plan(Vec::new()), &args, true)?;

        let security = assignments
            .iter()
            .find(|assignment| assignment.lane.id == "security")
            .ok_or_else(|| anyhow::anyhow!("security lane missing"))?;
        assert_eq!(security.spec.provider, ModelProvider::MiniMaxDirect);
        assert_eq!(
            security.fallback.as_ref().map(|spec| spec.provider),
            Some(ModelProvider::OpenCodeGo)
        );
        assert_eq!(
            security.fallback.as_ref().map(|spec| spec.model.as_str()),
            Some("mimo-v2.5")
        );
        let primary = direct_minimax_spec(&args);
        let canary = fallback_provider_spec_for_lane(&lane_plan("security"), &primary, &args, true)
            .ok_or_else(|| anyhow::anyhow!("security fallback missing"))?;
        assert_eq!(canary.provider, ModelProvider::OpenCodeGo);
        assert_eq!(canary.model, "mimo-v2.5");

        let fast =
            fallback_provider_spec_for_lane(&lane_plan("summary-pressure"), &primary, &args, true)
                .ok_or_else(|| anyhow::anyhow!("fast fallback missing"))?;
        assert_eq!(fast.provider, ModelProvider::OpenCodeGo);
        assert_eq!(fast.model, "deepseek-v4-flash");
        Ok(())
    }

    #[test]
    fn primary_with_fallback_routes_model_pass_fallbacks() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::PrimaryWithFallback;
        args.opencode_model = "mimo-v2.5".to_owned();

        let proof_planner = proof_planner_assignment_with_key_state(&args, true);
        assert_eq!(proof_planner.spec.provider, ModelProvider::MiniMaxDirect);
        assert_eq!(
            proof_planner.fallback.as_ref().map(|spec| spec.provider),
            Some(ModelProvider::OpenCodeGo)
        );
        assert_eq!(
            proof_planner
                .fallback
                .as_ref()
                .map(|spec| spec.model.as_str()),
            Some("mimo-v2.5")
        );

        let follow_up = follow_up_provider_assignment_with_key_state(&args, true);
        assert_eq!(follow_up.spec.provider, ModelProvider::MiniMaxDirect);
        assert_eq!(
            follow_up.fallback.as_ref().map(|spec| spec.provider),
            Some(ModelProvider::OpenCodeGo)
        );
        assert_eq!(
            follow_up.fallback.as_ref().map(|spec| spec.model.as_str()),
            Some("mimo-v2.5")
        );

        let unavailable = follow_up_provider_assignment_with_key_state(&args, false);
        assert!(unavailable.fallback.is_none());
    }

    #[test]
    fn primary_with_fallback_does_not_plan_missing_opencode_fallbacks() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::PrimaryWithFallback;
        args.lane_width = 10;
        args.opencode_model = "mimo-v2.5".to_owned();
        let assignments = model_assignments_with_key_state(&test_plan(Vec::new()), &args, false)?;

        let security = assignments
            .iter()
            .find(|assignment| assignment.lane.id == "security")
            .ok_or_else(|| anyhow::anyhow!("security lane missing"))?;
        assert_eq!(security.spec.provider, ModelProvider::MiniMaxDirect);
        assert!(security.fallback.is_none());
        assert!(assignments.iter().all(|assignment| {
            assignment
                .fallback
                .as_ref()
                .map(|fallback| fallback.provider)
                != Some(ModelProvider::OpenCodeGo)
        }));
        Ok(())
    }

    #[test]
    fn provider_policy_opencode_wide_uses_flash_for_fast_lanes() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::OpencodeGoWide;
        args.lane_width = 20;
        let assignments = model_assignments(&test_plan(Vec::new()), &args)?;

        assert_eq!(assignments.len(), 20);
        assert!(assignments.iter().any(|assignment| {
            assignment.lane.id == "ub-memory-lifetime"
                && assignment.spec.provider == ModelProvider::MiniMaxDirect
        }));
        assert!(assignments.iter().any(|assignment| {
            assignment.lane.id == "source-route-fast"
                && assignment.spec.provider == ModelProvider::OpenCodeGo
                && assignment.spec.model == "deepseek-v4-flash"
                && assignment.spec.endpoint_kind == super::ProviderEndpointKind::OpenAiChat
        }));
        Ok(())
    }

    #[test]
    fn zero_model_concurrency_is_rejected() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.model_concurrency = 0;

        let result = validate_run_args(&args);

        assert!(result.is_err());
        assert!(
            result
                .err()
                .is_some_and(|err| err.to_string().contains("--model-concurrency"))
        );
    }

    #[test]
    fn skipped_model_lanes_are_missing_evidence_when_review_work_was_suppressed() {
        let mut model_mode_off = model_lane_receipt("ub-memory-lifetime", "skipped");
        model_mode_off.reason = "model-mode off".to_owned();
        assert!(is_model_receipt_evidence_issue(&model_mode_off));

        let mut budget_skipped = model_lane_receipt("tests-oracle", "skipped");
        budget_skipped.reason =
            "model call budget or inline comment cap reached before lane execution".to_owned();
        assert!(is_model_receipt_evidence_issue(&budget_skipped));

        let mut refuter_budget = model_lane_receipt("refuter", "skipped");
        refuter_budget.reason = "model call budget exhausted before refuter pass".to_owned();
        assert!(is_model_receipt_evidence_issue(&refuter_budget));

        let mut no_inline_refuter = model_lane_receipt("refuter", "skipped");
        no_inline_refuter.reason =
            "no inline candidates passed guardrails before refuter".to_owned();
        assert!(!is_model_receipt_evidence_issue(&no_inline_refuter));

        let mut unknown_skip = model_lane_receipt("opposition", "skipped");
        unknown_skip.reason = "optional lane had no work".to_owned();
        assert!(!is_model_receipt_evidence_issue(&unknown_skip));
    }

    #[test]
    fn model_content_extracts_openai_and_anthropic_envelopes() -> Result<()> {
        let openai: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/providers/openai-chat-completion.json"
        ))?;
        let minimax: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/providers/minimax-chat-completion-anthropic.json"
        ))?;
        let opencode: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/providers/opencode-go-m3-messages.json"
        ))?;
        let opencode_openai: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/providers/opencode-go-openai-chat-completion.json"
        ))?;
        let minimax_thinking: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/providers/minimax-m3-thinking-then-text.json"
        ))?;
        let malformed: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/providers/malformed-no-content.json"
        ))?;
        let non_json: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/providers/non-json-content.json"))?;

        assert_eq!(model_response_shape(&openai), "openai");
        assert_eq!(
            extract_model_content(&openai),
            Some("{\"summary\":\"openai ok\",\"inline_comments\":[],\"summary_only_findings\":[]}")
        );
        assert_eq!(model_response_shape(&minimax), "anthropic");
        assert_eq!(
            extract_model_content(&minimax),
            Some("{\"summary\":\"m3 ok\",\"inline_comments\":[],\"summary_only_findings\":[]}")
        );
        assert_eq!(model_response_shape(&opencode), "anthropic");
        assert_eq!(
            extract_model_content(&opencode),
            Some(
                "{\"summary\":\"opencode go m3 ok\",\"inline_comments\":[],\"summary_only_findings\":[]}"
            )
        );
        assert_eq!(model_response_shape(&opencode_openai), "openai");
        assert_eq!(
            extract_model_content(&opencode_openai),
            Some(
                "{\"summary\":\"opencode go openai ok\",\"inline_comments\":[],\"summary_only_findings\":[]}"
            )
        );
        assert_eq!(model_response_shape(&minimax_thinking), "anthropic");
        assert_eq!(
            extract_model_content(&minimax_thinking),
            Some(
                "{\"summary\":\"preflight ok\",\"inline_comments\":[],\"summary_only_findings\":[]}"
            )
        );
        assert_eq!(model_response_shape(&malformed), "unknown");
        assert!(extract_model_content(&malformed).is_none());
        assert_eq!(model_response_shape(&non_json), "openai");
        let content = extract_model_content(&non_json)
            .ok_or_else(|| anyhow::anyhow!("non-json content fixture missing assistant content"))?;
        let result = serde_json::from_str::<LaneModelOutput>(&model_json_payload(content))
            .map(|_| ())
            .map_err(anyhow::Error::from)
            .context("parse model output fixture");
        let Err(err) = result else {
            return Err(anyhow::anyhow!(
                "non-json provider content passed strict lane JSON parsing"
            ));
        };
        assert_eq!(super::classify_model_error(&err), "invalid_json");
        Ok(())
    }

    #[test]
    fn model_cache_usage_extracts_provider_token_counts() {
        let anthropic: serde_json::Value = serde_json::json!({
            "usage": {
                "input_tokens": 100,
                "output_tokens": 20,
                "cache_creation_input_tokens": 80,
                "cache_read_input_tokens": 40
            }
        });
        let usage = super::model_cache_usage(&anthropic);
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.cache_creation_input_tokens, Some(80));
        assert_eq!(usage.cache_read_input_tokens, Some(40));

        let openai: serde_json::Value = serde_json::json!({
            "usage": {
                "prompt_tokens": 75,
                "completion_tokens": 10,
                "prompt_tokens_details": {
                    "cached_tokens": 55
                }
            }
        });
        let usage = super::model_cache_usage(&openai);
        assert_eq!(usage.input_tokens, Some(75));
        assert_eq!(usage.output_tokens, Some(10));
        assert_eq!(usage.cache_creation_input_tokens, None);
        assert_eq!(usage.cache_read_input_tokens, Some(55));
    }

    #[test]
    fn model_json_payload_accepts_markdown_json_fence() -> Result<()> {
        let fenced = r#"```json
{
  "summary": "fenced ok",
  "inline_comments": [],
  "summary_only_findings": []
}
```"#;

        let parsed: LaneModelOutput = serde_json::from_str(&model_json_payload(fenced))?;

        assert_eq!(parsed.summary.as_deref(), Some("fenced ok"));
        assert!(parsed.inline_comments.is_empty());
        assert!(parsed.candidate_findings.is_empty());
        assert!(parsed.summary_only_findings.is_empty());
        assert!(parsed.observations.is_empty());
        assert!(parsed.failed_objections.is_empty());
        assert!(parsed.proof_requests.is_empty());
        assert!(
            serde_json::from_str::<LaneModelOutput>(&model_json_payload("Here is the JSON:\n{}"))
                .is_err()
        );
        Ok(())
    }

    #[test]
    fn curl_http_status_marker_is_stripped_from_body() {
        let (body, status) = split_curl_http_status(br#"{"error":"rate"}"#.to_vec());
        assert_eq!(body, br#"{"error":"rate"}"#);
        assert_eq!(status, None);

        let (body, status) = split_curl_http_status(
            br#"{"error":"rate"}
UB_REVIEW_HTTP_STATUS:429
"#
            .to_vec(),
        );

        assert_eq!(body, br#"{"error":"rate"}"#);
        assert_eq!(status, Some(429));
    }

    #[test]
    fn child_output_file_wait_reports_timeout_and_cleans_temp_files() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let stdout_path = temp.path().join("stdout.txt");
        let stderr_path = temp.path().join("stderr.txt");
        let stdout =
            fs::File::create(&stdout_path).with_context(|| "create sleeper stdout file")?;
        let stderr =
            fs::File::create(&stderr_path).with_context(|| "create sleeper stderr file")?;
        let argv = sleeper_argv();
        let Some((program, args)) = argv.split_first() else {
            return Err(anyhow::anyhow!("empty sleeper argv"));
        };
        let child = ProcessCommand::new(program)
            .args(args)
            .current_dir(temp.path())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| "spawn sleeper")?;

        let err = match wait_for_child_output_files(child, &stdout_path, &stderr_path, 1) {
            Ok(_) => return Err(anyhow::anyhow!("sleeper completed before timeout")),
            Err(err) => err,
        };

        assert!(err.to_string().contains("timed out after 1s"));
        assert!(!stdout_path.exists());
        assert!(!stderr_path.exists());
        Ok(())
    }

    #[test]
    fn model_error_exposes_http_status_for_receipts() {
        let err = anyhow::anyhow!(
            "model curl exited Some(22) with http status Some(401): stderr: unauthorized"
        );

        assert_eq!(http_status_from_error(&err), Some(401));
        assert_eq!(super::classify_model_error(&err), "auth_failed");

        let wrapped_rate_limit = anyhow::anyhow!(
            "model curl exited Some(22) with http status Some(429): stderr: too many requests"
        )
        .context("run model curl");
        assert_eq!(http_status_from_error(&wrapped_rate_limit), Some(429));
        assert_eq!(
            super::classify_model_error(&wrapped_rate_limit),
            "rate_limited"
        );

        let wrapped_timeout = anyhow::anyhow!("operation timed out").context("run model curl");
        assert_eq!(super::classify_model_error(&wrapped_timeout), "timed_out");

        let wrapped_parse_error =
            anyhow::anyhow!("parse lane model JSON response").context("decode model output");
        assert_eq!(
            super::classify_model_error(&wrapped_parse_error),
            "invalid_json"
        );
    }

    #[test]
    fn post_error_classification_covers_403_429_500_statuses() {
        // Property-style coverage of the HTTP status classification that the
        // post error receipt depends on. Exercises 403/429/500 error responses
        // through the real classification logic without a subprocess or fake
        // server (avoiding the Linux CI timing issue that blocked the
        // integration-test approach). See #612 / tracker UB-29.
        for (status, expected_class) in [
            (403u16, "auth_failed"),
            (429u16, "rate_limited"),
            (500u16, "failed"),
            (502u16, "failed"),
            (503u16, "failed"),
        ] {
            let err = anyhow::anyhow!(
                "GitHub review post failed with exit code Some(22) and http status Some({status}): stderr: error"
            );
            assert_eq!(
                http_status_from_error(&err),
                Some(status),
                "http_status_from_error must extract {status}"
            );
            assert_eq!(
                super::classify_model_error(&err),
                expected_class,
                "classify_model_error must classify {status} as {expected_class}"
            );
        }
    }

    #[test]
    fn minimax_openai_payload_uses_chat_shape() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.minimax_provider_kind = ProviderKindArg::Openai;
        let spec = direct_minimax_spec(&args);
        let payload = model_request_payload(&spec, "packet");

        assert_eq!(payload["model"], "MiniMax-M3");
        assert_eq!(payload["max_completion_tokens"], 4096);
        assert_eq!(payload["reasoning_split"], true);
        assert_eq!(payload["response_format"]["type"], "json_object");
        assert!(
            payload["messages"][0]["content"]
                .as_str()
                .is_some_and(|system| system.contains("strict JSON"))
        );
        assert!(payload["messages"][1]["content"].as_str().is_some());
        assert_eq!(payload["stream"], false);
        assert!(payload.get("max_tokens").is_none());
    }

    #[test]
    fn minimax_anthropic_payload_uses_messages_shape() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.minimax_provider_kind = ProviderKindArg::Anthropic;
        let spec = direct_minimax_spec(&args);
        let payload = model_request_payload(&spec, "packet");

        assert_eq!(payload["model"], "MiniMax-M3");
        assert_eq!(payload["max_tokens"], 4096);
        assert_eq!(payload["thinking"]["type"], "disabled");
        assert!(
            payload["system"]
                .as_str()
                .is_some_and(|system| system.contains("final text block"))
        );
        assert!(payload["messages"].is_array());
        assert!(payload["messages"][0]["content"].as_str().is_some());
        assert!(payload.get("stream").is_none());

        let cached =
            super::model_request_payload_parts(&spec, Some("shared cache block"), "lane task");
        let content = cached["messages"][0]["content"].as_array().ok_or_else(|| {
            anyhow::anyhow!("cached MiniMax Anthropic content should be block array")
        })?;
        assert_eq!(content[0]["type"], "text");
        assert_eq!(content[0]["text"], "shared cache block");
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");
        assert_eq!(content[1]["type"], "text");
        assert_eq!(content[1]["text"], "lane task");
        Ok(())
    }

    #[test]
    fn provider_preflight_cache_selection_is_minimax_anthropic_only() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.minimax_provider_kind = ProviderKindArg::Anthropic;
        let anthropic = direct_minimax_spec(&args);
        assert_eq!(
            super::provider_preflight_cacheable_prefix(&anthropic, "shared context", &args),
            Some("shared context")
        );

        args.minimax_provider_kind = ProviderKindArg::Openai;
        let openai = direct_minimax_spec(&args);
        assert_eq!(
            super::provider_preflight_cacheable_prefix(&openai, "shared context", &args),
            None
        );

        args.minimax_provider_kind = ProviderKindArg::Anthropic;
        args.minimax_prompt_cache = MinimaxPromptCache::Off;
        let cache_disabled = direct_minimax_spec(&args);
        assert_eq!(
            super::provider_preflight_cacheable_prefix(&cache_disabled, "shared context", &args),
            None
        );
    }

    #[test]
    fn minimax_prompt_cache_off_omits_anthropic_cache_control() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.minimax_provider_kind = ProviderKindArg::Anthropic;
        let spec = direct_minimax_spec(&args);
        let cached =
            super::model_request_payload_parts(&spec, Some("shared cache block"), "lane task");
        let content = cached["messages"][0]["content"].as_array().ok_or_else(|| {
            anyhow::anyhow!("default MiniMax Anthropic content should be block array")
        })?;
        assert_eq!(content[0]["cache_control"]["type"], "ephemeral");

        args.minimax_prompt_cache = MinimaxPromptCache::Off;
        let disabled = super::model_request_payload_parts_with_cache_control(
            &spec,
            Some("shared cache block"),
            "lane task",
            super::model_cacheable_prefix(&spec, "shared cache block", &args).is_some(),
        );
        let prompt = disabled["messages"][0]["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("disabled cache payload should use plain prompt"))?;
        assert!(prompt.contains("Cached shared context:"));
        assert!(prompt.contains("shared cache block"));
        assert!(prompt.contains("lane task"));
        assert!(!prompt.contains("cache_control"));
        Ok(())
    }

    #[test]
    fn cache_events_include_provider_preflight_usage() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut args = test_run_args(temp.path().join("out"));
        args.minimax_provider_kind = ProviderKindArg::Anthropic;
        let assignments = model_assignments(&test_plan(Vec::new()), &args)?;
        let preflights = vec![super::ProviderPreflightReceipt {
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "completed".to_owned(),
            duration_ms: Some(25),
            http_status: Some(200),
            response_shape: Some("anthropic".to_owned()),
            cache_usage: super::ModelCacheUsage {
                input_tokens: Some(900),
                output_tokens: Some(30),
                cache_creation_input_tokens: Some(700),
                cache_read_input_tokens: Some(50),
            },
        }];

        super::write_shared_context_cache_artifacts(
            temp.path(),
            "stable shared context",
            &assignments,
            &preflights,
            &[],
            &[],
            &args,
        )?;

        let events = fs::read_to_string(temp.path().join("review/cache_events.ndjson"))?;
        let preflight_event = events
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|event| event["kind"] == "provider_preflight_cache_usage")
            .ok_or_else(|| anyhow::anyhow!("provider preflight cache event missing"))?;
        assert_eq!(preflight_event["lane"], "provider-preflight");
        assert_eq!(preflight_event["provider"], "minimax");
        assert_eq!(preflight_event["endpoint_kind"], "anthropic-messages");
        assert_eq!(
            preflight_event["cache_mode"],
            "explicit-anthropic-cache-control"
        );
        assert_eq!(preflight_event["cache_creation_input_tokens"], 700);
        assert_eq!(preflight_event["cache_read_input_tokens"], 50);
        Ok(())
    }

    #[test]
    fn cache_artifacts_use_follow_up_result_provider() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = test_run_args(temp.path().join("out"));
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
        let mut follow_up = test_follow_up_result("follow-cache", "group-cache", "ok");
        follow_up.provider = "opencode-go".to_owned();
        follow_up.model = "mimo-v2.5".to_owned();
        follow_up.endpoint_kind = "anthropic-messages".to_owned();
        follow_up.cache_usage = super::ModelCacheUsage {
            input_tokens: Some(321),
            output_tokens: Some(12),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: Some(111),
        };

        super::write_shared_context_cache_artifacts(
            temp.path(),
            "stable shared context",
            &[],
            &[],
            &model_lanes,
            std::slice::from_ref(&follow_up),
            &args,
        )?;

        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("review/cache_manifest.json"))?)?;
        let follow_lane = manifest["lanes"]
            .as_array()
            .and_then(|lanes| {
                lanes
                    .iter()
                    .find(|lane| lane["lane"] == "orchestrator-follow-up-follow-cache")
            })
            .ok_or_else(|| anyhow::anyhow!("follow-up cache lane missing"))?;
        assert_eq!(follow_lane["provider"], "opencode-go");
        assert_eq!(follow_lane["model"], "mimo-v2.5");
        assert_eq!(follow_lane["endpoint_kind"], "anthropic-messages");

        let events = fs::read_to_string(temp.path().join("review/cache_events.ndjson"))?;
        let follow_event = events
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .find(|event| event["kind"] == "follow_up_cache_usage")
            .ok_or_else(|| anyhow::anyhow!("follow-up cache event missing"))?;
        assert_eq!(follow_event["provider"], "opencode-go");
        assert_eq!(follow_event["endpoint_kind"], "anthropic-messages");
        assert_eq!(follow_event["cache_mode"], "not-supported");
        assert_eq!(follow_event["cache_read_input_tokens"], 111);
        Ok(())
    }

    #[test]
    fn opencode_go_canary_payload_uses_messages_shape() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let spec = opencode_canary_spec(&args);
        let payload = model_request_payload(&spec, "packet");

        assert_eq!(payload["model"], "minimax-m3");
        assert_eq!(payload["max_tokens"], 4096);
        assert_eq!(payload["thinking"]["type"], "adaptive");
        assert!(
            payload["system"]
                .as_str()
                .is_some_and(|system| system.contains("final text block"))
        );
        assert!(payload["messages"][0]["content"].as_str().is_some());
        assert!(payload.get("stream").is_none());
    }

    #[test]
    fn failed_model_evidence_is_not_rendered_as_summary_finding() {
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[SensorEvidenceIssue {
                sensor: "ripr".to_owned(),
                status: "missing".to_owned(),
                reason: "command not found".to_owned(),
            }],
            &[ModelEvidenceIssue {
                lane: "ub-memory-lifetime".to_owned(),
                provider: "minimax".to_owned(),
                model: "MiniMax-M3".to_owned(),
                endpoint_kind: "anthropic-messages".to_owned(),
                status: "rate_limited".to_owned(),
                reason: "rate limited after retry".to_owned(),
            }],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("## Review result"));
        assert!(!body.contains("## Residual risk"));
        assert!(!body.contains("## Missing evidence"));
        assert!(!body.contains("Some sensor and model evidence was unavailable"));
        assert!(!body.contains("review artifacts"));
        assert!(!body.contains("Sensor `ripr` unavailable"));
        assert!(!body.contains("command not found"));
        assert!(!body.contains("rate_limited"));
        assert!(!body.contains("ub-memory-lifetime"));
        assert!(!body.contains("## Model lanes"));
        assert!(!body.contains("## Confirmed findings"));
        assert!(!body.contains("## Summary-only findings"));
        assert!(!body.contains("## Failed objections"));
        assert!(!body.contains("## No blocking finding after checking"));
        assert!(!body.contains("A human should still inspect"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_omits_residual_risk_only_observations() {
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[] as &[ModelLaneReceipt],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[test_observation(
                "tests-oracle",
                "The added FileHandle.write test was not proven to hit the patched scalar-write branch.",
                "residual-risk",
                "open",
                "medium",
                "high",
                "filehandle-route-proof",
            )],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("## Residual risk"));
        assert!(!body.contains("FileHandle.write test was not proven"));
        assert!(!body.contains("A human should still inspect"));
        assert!(!body.contains("residual risk remains for human review"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_omits_successful_model_lane_roster_and_default_decision() {
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[model_lane_receipt("ub-memory-lifetime", "ok")],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("Shared context"));
        assert!(!body.contains("Profile:"));
        assert!(!body.contains("Changed files:"));
        assert!(!body.contains("Inline comments:"));
        assert!(!body.contains("## Model lanes"));
        assert!(!body.contains("Lane: `ub-memory-lifetime`"));
        assert!(!body.contains("Provider: `minimax`"));
        assert!(!body.contains("Model: `MiniMax-M3`"));
        assert!(!body.contains("## Residual risk"));
        assert!(!body.contains("A human should still inspect"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_decision_has_no_default_no_finding_sentence() {
        let decision = pr_decision_sentence(PrDecisionContext {
            finding_count: 0,
            verification_count: 0,
            has_test_proof_verification: false,
            current_proof_failure: false,
        });

        assert!(decision.is_none());
    }

    #[test]
    fn no_value_pr_body_is_not_prepared_for_posting() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.model_mode = ModelMode::Auto;
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[model_lane_receipt("ub-memory-lifetime", "ok")],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!super::should_prepare_github_review_payload(
            &args,
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[] as &[ProofReceipt],
            &body
        ));
    }

    #[test]
    fn terminal_state_marks_clean_usable_review_sufficient() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
        let state = build_review_terminal_state(TerminalStateInput {
            args: &args,
            run_pass: super::RunPass::Manual,
            plan: &plan,
            review_payload_status: "skipped_empty_smoke",
            should_prepare_github_review: false,
            pr_body: "",
            inline_comments: &[],
            summary_only_findings: &[],
            summary_only_body: SummaryOnlyBodyPolicy::Suppress,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
        });

        assert_eq!(state.status, "sufficient");
        assert_eq!(state.usable_model_lanes, 1);
        assert!(!state.reviewer_value_present);
    }

    #[test]
    fn terminal_state_fails_intelligent_ci_required_sensor_gap() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.mode = RunMode::IntelligentCi;
        let mut required_actionlint = sensor_plan("actionlint", "actionlint", false);
        required_actionlint.required = true;
        let plan = test_plan(vec![required_actionlint]);
        let model_lanes = vec![model_lane_receipt("workflow-permissions", "ok")];
        let missing_sensor = vec![SensorEvidenceIssue {
            sensor: "actionlint".to_owned(),
            status: "skipped".to_owned(),
            reason: "disabled by config".to_owned(),
        }];

        let state = build_review_terminal_state(TerminalStateInput {
            args: &args,
            run_pass: super::RunPass::Manual,
            plan: &plan,
            review_payload_status: "skipped_empty_smoke",
            should_prepare_github_review: false,
            pr_body: "",
            inline_comments: &[],
            summary_only_findings: &[],
            summary_only_body: SummaryOnlyBodyPolicy::Suppress,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &missing_sensor,
            missing_or_failed_model_evidence: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
        });

        assert_eq!(state.status, "failed-to-review");
        assert_eq!(state.evidence_gaps, 1);

        args.mode = RunMode::ReviewByok;
        let review_byok_state = build_review_terminal_state(TerminalStateInput {
            args: &args,
            run_pass: super::RunPass::Manual,
            plan: &plan,
            review_payload_status: "skipped_empty_smoke",
            should_prepare_github_review: false,
            pr_body: "",
            inline_comments: &[],
            summary_only_findings: &[],
            summary_only_body: SummaryOnlyBodyPolicy::Suppress,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &missing_sensor,
            missing_or_failed_model_evidence: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
        });

        assert_eq!(review_byok_state.status, "sufficient");
        assert_eq!(review_byok_state.evidence_gaps, 1);
    }

    #[test]
    fn terminal_state_keeps_model_off_runs_artifact_only() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.model_mode = ModelMode::Off;
        let plan = test_plan(Vec::new());
        let state = build_review_terminal_state(TerminalStateInput {
            args: &args,
            run_pass: super::RunPass::Manual,
            plan: &plan,
            review_payload_status: "skipped_empty_smoke",
            should_prepare_github_review: false,
            pr_body: "",
            inline_comments: &[],
            summary_only_findings: &[],
            summary_only_body: SummaryOnlyBodyPolicy::Suppress,
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
        });

        assert_eq!(state.status, "artifact-only");
        assert!(state.reason.contains("Model mode was off"));
    }

    #[test]
    fn terminal_state_marks_unusable_auto_run_failed_to_review() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let missing_model = vec![ModelEvidenceIssue {
            lane: "tests-oracle".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "timed_out".to_owned(),
            reason: "timed out".to_owned(),
        }];
        let state = build_review_terminal_state(TerminalStateInput {
            args: &args,
            run_pass: super::RunPass::Manual,
            plan: &plan,
            review_payload_status: "skipped_empty_smoke",
            should_prepare_github_review: false,
            pr_body: "",
            inline_comments: &[],
            summary_only_findings: &[],
            summary_only_body: SummaryOnlyBodyPolicy::Suppress,
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &missing_model,
            proof_receipts: &[test_proof_receipt("discriminating", "ok")],
            final_follow_up_tasks: 0,
        });

        assert_eq!(state.status, "failed-to-review");
        assert_eq!(state.evidence_gaps, 1);
    }

    #[test]
    fn terminal_state_marks_surviving_pr_body_as_reviewer_attention() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let state = build_review_terminal_state(TerminalStateInput {
            args: &args,
            run_pass: super::RunPass::Manual,
            plan: &plan,
            review_payload_status: "prepared",
            should_prepare_github_review: true,
            pr_body: "## Verification questions\n\n- Confirm the focused proof.",
            inline_comments: &[],
            summary_only_findings: &[],
            summary_only_body: SummaryOnlyBodyPolicy::Suppress,
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
        });

        assert_eq!(state.status, "needs-reviewer-attention");
        assert!(state.reviewer_value_present);
    }

    #[test]
    fn policy_parse_errors_are_recorded_receipts_not_silent_defaults() -> Result<()> {
        let config = Config::from_toml_with_policy_receipts(
            r#"
[gate]
target_minutes = 45
target_minutez = 30

[tools.ripr.gate]
max_new_unsuppressed_findings = 0

[tools.unsafe-review.gate]
max_new_unsuppressed = 0

[[proof.required]]
id = "cargo-check"
command = "cargo check --workspace"
diff_classes = ["all"]

[[proof.required]]
id = "cargo-clippy"
command = "cargo clippy --workspace"
requird = true

[[proof.required]]
id = "empty-command"
command = "   "

[[proof.required]]
id = "bad-diff-class"
command = "cargo test"
diff_classes = ["sourceish"]

[[proof.required]]
id = "bad-language"
command = "cargo test"
languages = ["cobol"]
"#,
        )?;

        let sections = config
            .policy_errors
            .iter()
            .map(|error| error.section.as_str())
            .collect::<Vec<_>>();
        assert_eq!(
            sections,
            [
                "gate.target_minutez",
                "tools.ripr.gate",
                "proof.required.cargo-clippy",
                "proof.required.empty-command",
                "proof.required.bad-diff-class",
                "proof.required.bad-language",
            ]
        );
        // Only the offending key is stripped; the valid sibling inside the
        // same [gate] table survives. 45 is intentionally NOT the default
        // (30), so this assertion fails if the sibling falls back.
        assert_ne!(super::GateConfig::default().target_minutes, 45);
        assert_eq!(config.gate.target_minutes, 45);
        assert!(
            config
                .tools
                .get("ripr")
                .is_none_or(|tool| tool.gate.is_none())
        );
        // Well-formed sibling tables keep working too.
        let unsafe_review = config
            .tools
            .get("unsafe-review")
            .ok_or_else(|| anyhow::anyhow!("unsafe-review tool missing"))?;
        assert_eq!(
            unsafe_review
                .gate
                .as_ref()
                .and_then(|gate| gate.max_new_unsuppressed),
            Some(0)
        );
        assert_eq!(config.proof.required.len(), 1);
        assert_eq!(config.proof.required[0].id, "cargo-check");
        // The receipt artifact (effective-config.json serializes Config)
        // names each parse error.
        let serialized = serde_json::to_value(&config)?;
        assert_eq!(
            serialized["policy_errors"][0]["section"],
            serde_json::json!("gate.target_minutez")
        );
        assert!(
            serialized["policy_errors"][1]["detail"]
                .as_str()
                .is_some_and(|detail| detail.contains("max_new_unsuppressed_findings")),
        );
        Ok(())
    }

    #[test]
    fn unknown_policy_keys_are_stripped_with_receipts_keeping_valid_siblings() -> Result<()> {
        // Misspelled section and key names must never silently de-fang
        // policy: each unknown key becomes a PolicyError receipt while the
        // correctly-spelled siblings keep working.
        let config = Config::from_toml_with_policy_receipts(
            r#"
[gatee]
target_minutes = 45

[gate]
target_minutes = 45
required_chekc = "ub-review/gate"

[tools.ripr]
required = true
gates = { max_new_unsuppressed = 0 }

[tools.ripr.gate]
scope = "on-diff"
max_new_unsuppressed = 0

[[proof.requierd]]
id = "cargo-check"
command = "cargo check"
"#,
        )?;
        let mut sections = config
            .policy_errors
            .iter()
            .map(|error| error.section.as_str())
            .collect::<Vec<_>>();
        sections.sort_unstable();
        assert_eq!(
            sections,
            [
                "gate.required_chekc",
                "gatee",
                "proof.requierd",
                "tools.ripr.gates"
            ]
        );
        // Valid siblings survive each one-key typo.
        assert_eq!(config.gate.target_minutes, 45);
        let ripr = config
            .tools
            .get("ripr")
            .ok_or_else(|| anyhow::anyhow!("ripr tool missing"))?;
        assert!(ripr.required);
        assert_eq!(
            ripr.gate
                .as_ref()
                .and_then(|gate| gate.max_new_unsuppressed),
            Some(0)
        );
        Ok(())
    }

    #[test]
    fn policy_shape_mismatches_become_receipts_not_hard_errors() -> Result<()> {
        // [proof.required] as a single table (not [[proof.required]]),
        // a non-table tools.<id>, and a non-table [gate] are policy-surface
        // shape mismatches: the doc scopes hard errors to TOML syntax, so all
        // of these must take the PolicyError receipt path.
        let config = Config::from_toml_with_policy_receipts(
            r#"
gate = 5

[proof.required]
id = "cargo-check"
command = "cargo check"

[tools]
ripr = 5

[tools.unsafe-review.gate]
max_new_unsuppressed = 0
"#,
        )?;
        let mut sections = config
            .policy_errors
            .iter()
            .map(|error| error.section.as_str())
            .collect::<Vec<_>>();
        sections.sort_unstable();
        assert_eq!(sections, ["gate", "proof.required", "tools.ripr"]);
        assert!(config.proof.required.is_empty());
        // Valid siblings in the same containers keep working.
        let unsafe_review = config
            .tools
            .get("unsafe-review")
            .ok_or_else(|| anyhow::anyhow!("unsafe-review tool missing"))?;
        assert_eq!(
            unsafe_review
                .gate
                .as_ref()
                .and_then(|gate| gate.max_new_unsuppressed),
            Some(0)
        );
        // A non-table [tools] (the container itself) is also a receipt.
        let container = Config::from_toml_with_policy_receipts("tools = 5\n")?;
        assert_eq!(container.policy_errors.len(), 1);
        assert_eq!(container.policy_errors[0].section, "tools");
        let proof_container = Config::from_toml_with_policy_receipts("proof = 5\n")?;
        assert_eq!(proof_container.policy_errors.len(), 1);
        assert_eq!(proof_container.policy_errors[0].section, "proof");
        Ok(())
    }

    #[test]
    fn tool_policy_known_keys_match_serialized_fields() -> Result<()> {
        // Pin KNOWN_TOOL_POLICY_KEYS to the ToolPolicy field set so the
        // sanitizer's unknown-key receipts can never drift from the struct.
        let tool = super::ToolPolicy {
            gate: Some(super::ToolGatePolicy {
                scope: Some("on-diff".to_owned()),
                max_new_unsuppressed: Some(0),
            }),
            phase: Some(super::SensorPhase::Fast),
            ..super::ToolPolicy::default()
        };
        let serialized = serde_json::to_value(&tool)?;
        let mut serialized_keys = serialized
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("tool policy did not serialize to an object"))?
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>();
        serialized_keys.sort_unstable();
        let mut known = super::KNOWN_TOOL_POLICY_KEYS.to_vec();
        known.sort_unstable();
        assert_eq!(serialized_keys, known);
        Ok(())
    }

    #[test]
    fn well_formed_policy_sections_record_no_policy_errors() -> Result<()> {
        for text in [
            include_str!("../.ub-review.toml"),
            include_str!("../profiles/bun-ub-v0.toml"),
            include_str!("../configs/ub-review.example.toml"),
        ] {
            let config = Config::from_toml_with_policy_receipts(text)?;
            assert!(
                config.policy_errors.is_empty(),
                "unexpected policy errors: {:?}",
                config.policy_errors
            );
        }
        Ok(())
    }

    #[test]
    fn gate_blocking_policy_parses_and_defaults_off() -> Result<()> {
        let config = Config::from_toml_with_policy_receipts(
            r#"
[gate.blocking]
required_proof_unproven = true
tool_gate_missing_evidence = true
"#,
        )?;
        assert!(config.policy_errors.is_empty());
        assert!(config.gate.blocking.required_proof_unproven);
        assert!(config.gate.blocking.tool_gate_missing_evidence);
        assert!(!Config::default().gate.blocking.required_proof_unproven);
        assert!(!Config::default().gate.blocking.tool_gate_missing_evidence);

        let misspelled = Config::from_toml_with_policy_receipts(
            r#"
[gate]
target_minutes = 45

[gate.blocking]
required_proof_unprooven = true
"#,
        )?;
        assert_eq!(misspelled.policy_errors.len(), 1);
        assert_eq!(misspelled.policy_errors[0].section, "gate.blocking");
        assert!(!misspelled.gate.blocking.required_proof_unproven);
        // Only the malformed [gate.blocking] key is stripped; the valid
        // sibling inside [gate] survives.
        assert_eq!(misspelled.gate.target_minutes, 45);
        Ok(())
    }

    #[test]
    fn policy_selector_known_sets_match_classifier_outputs() {
        // Pin the config-side selector allowlists to the classifier outputs
        // so an unknown selector can never silently de-fang a policy.
        for diff_class in [
            DiffClass::SourceUb,
            DiffClass::SourceGeneral,
            DiffClass::TestsOnly,
            DiffClass::WorkflowTooling,
            DiffClass::DocsOnly,
            DiffClass::ArtifactOnlySmoke,
        ] {
            assert!(
                super::KNOWN_POLICY_DIFF_CLASSES.contains(&diff_class.key()),
                "diff class {} missing from KNOWN_POLICY_DIFF_CLASSES",
                diff_class.key()
            );
        }
        for path in [
            "a.rs", "a.ts", "a.js", "a.cc", "a.zig", "a.go", "a.py", "a.sh", "a.yml", "a.toml",
            "a.json", "a.md",
        ] {
            let Some(language) = super::language_for_path(path) else {
                continue;
            };
            assert!(
                super::KNOWN_POLICY_LANGUAGES.contains(&language),
                "language {language} missing from KNOWN_POLICY_LANGUAGES"
            );
        }
    }

    #[test]
    fn gate_check_enforces_fail_outcomes_per_fail_on_gate_resolution() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let pass_path = temp.path().join("gate-pass.json");
        fs::write(
            &pass_path,
            serde_json::json!({
                "schema": "ub-review.gate_outcome.v1",
                "conclusion": "pass",
                "reasons": []
            })
            .to_string(),
        )?;
        let fail_path = temp.path().join("gate-fail.json");
        fs::write(
            &fail_path,
            serde_json::json!({
                "schema": "ub-review.gate_outcome.v1",
                "conclusion": "fail",
                "reasons": [
                    {"kind": "required-proof", "id": "cargo-check", "detail": "exit 101", "receipt": "review/proof_receipts.json#x"},
                    {"kind": "tool-gate", "id": "ripr", "detail": "threshold", "receipt": "review/tool-gate-outcomes.json#ripr"}
                ]
            })
            .to_string(),
        )?;
        let missing_path = temp.path().join("absent/gate_outcome.json");
        let check = |path: &Path, fail_on_gate: FailOnGate, mode: RunMode| {
            cmd_gate_check(GateCheckArgs {
                gate_outcome: path.to_path_buf(),
                fail_on_gate,
                mode,
                review_mode: None,
            })
        };

        // Passing outcomes stay green under enforcement.
        assert!(check(&pass_path, FailOnGate::True, RunMode::ReviewByok).is_ok());
        // Enforcement off tolerates failing outcomes and missing artifacts.
        assert!(check(&fail_path, FailOnGate::False, RunMode::IntelligentCi).is_ok());
        assert!(check(&fail_path, FailOnGate::Auto, RunMode::ReviewByok).is_ok());
        assert!(check(&missing_path, FailOnGate::Auto, RunMode::ReviewByok).is_ok());
        // Enforcement on turns a failing outcome into a non-zero exit naming
        // the blocking reason ids and the artifact path.
        let enforced = check(&fail_path, FailOnGate::True, RunMode::ReviewByok)
            .map_err(|err| err.to_string())
            .err()
            .ok_or_else(|| anyhow::anyhow!("enforced gate-check should fail"))?;
        assert!(enforced.contains("cargo-check, ripr"), "{enforced}");
        assert!(enforced.contains("gate-fail.json"), "{enforced}");
        // `auto` mirrors FailOnGate::resolved: intelligent-ci enforces.
        assert!(check(&fail_path, FailOnGate::Auto, RunMode::IntelligentCi).is_err());
        // Enforcement on with a missing artifact is a hard error.
        assert!(check(&missing_path, FailOnGate::True, RunMode::ReviewByok).is_err());
        assert!(check(&missing_path, FailOnGate::Auto, RunMode::IntelligentCi).is_err());

        // Enforcement fails closed: any conclusion that is not exactly
        // `pass` or `fail` is treated as a failure naming the value and the
        // artifact path.
        for (label, conclusion_json) in [
            ("string `error`", r#""error""#),
            ("cased `Fail`", r#""Fail""#),
            ("null", "null"),
        ] {
            let weird_path = temp.path().join("gate-weird.json");
            fs::write(
                &weird_path,
                format!(
                    r#"{{"schema":"ub-review.gate_outcome.v1","conclusion":{conclusion_json},"reasons":[]}}"#
                ),
            )?;
            let err = check(&weird_path, FailOnGate::True, RunMode::ReviewByok)
                .map_err(|err| err.to_string())
                .err()
                .ok_or_else(|| {
                    anyhow::anyhow!("enforced gate-check should fail closed on {label}")
                })?;
            assert!(err.contains("unrecognized conclusion"), "{label}: {err}");
            assert!(err.contains("gate-weird.json"), "{label}: {err}");
            // Enforcement off tolerates the same artifact.
            assert!(check(&weird_path, FailOnGate::False, RunMode::IntelligentCi).is_ok());
        }
        // A missing conclusion key also fails closed.
        let keyless_path = temp.path().join("gate-keyless.json");
        fs::write(
            &keyless_path,
            r#"{"schema":"ub-review.gate_outcome.v1","reasons":[]}"#,
        )?;
        let keyless = check(&keyless_path, FailOnGate::True, RunMode::ReviewByok)
            .map_err(|err| err.to_string())
            .err()
            .ok_or_else(|| anyhow::anyhow!("enforced gate-check should fail on missing key"))?;
        assert!(keyless.contains("missing"), "{keyless}");
        // A schema mismatch fails closed under enforcement even when the
        // conclusion claims `pass`, and stays informational otherwise.
        let wrong_schema_path = temp.path().join("gate-wrong-schema.json");
        fs::write(
            &wrong_schema_path,
            r#"{"schema":"ub-review.gate_outcome.v2","conclusion":"pass","reasons":[]}"#,
        )?;
        let wrong_schema = check(&wrong_schema_path, FailOnGate::True, RunMode::ReviewByok)
            .map_err(|err| err.to_string())
            .err()
            .ok_or_else(|| anyhow::anyhow!("enforced gate-check should fail on schema drift"))?;
        assert!(
            wrong_schema.contains("ub-review.gate_outcome.v2"),
            "{wrong_schema}"
        );
        assert!(
            check(
                &wrong_schema_path,
                FailOnGate::False,
                RunMode::IntelligentCi
            )
            .is_ok()
        );
        Ok(())
    }

    #[test]
    fn fail_on_gate_resolves_auto_by_mode() {
        assert_eq!(super::FailOnGate::Auto.key(), "auto");
        assert_eq!(super::FailOnGate::True.key(), "true");
        assert_eq!(super::FailOnGate::False.key(), "false");
        assert!(super::FailOnGate::Auto.resolved(RunMode::IntelligentCi));
        assert!(!super::FailOnGate::Auto.resolved(RunMode::ReviewByok));
        assert!(super::FailOnGate::True.resolved(RunMode::ReviewByok));
        assert!(super::FailOnGate::True.resolved(RunMode::IntelligentCi));
        assert!(!super::FailOnGate::False.resolved(RunMode::IntelligentCi));
        assert!(!super::FailOnGate::False.resolved(RunMode::ReviewByok));
        assert_eq!(
            <super::FailOnGate as clap::ValueEnum>::from_str("auto", true),
            Ok(super::FailOnGate::Auto)
        );
        assert_eq!(
            <super::FailOnGate as clap::ValueEnum>::from_str("true", true),
            Ok(super::FailOnGate::True)
        );
        assert_eq!(
            <super::FailOnGate as clap::ValueEnum>::from_str("false", true),
            Ok(super::FailOnGate::False)
        );
    }

    #[test]
    fn review_mode_preset_resolves_to_expected_triple() {
        use crate::ReviewModePreset;
        assert_eq!(super::ReviewModePreset::Advisory.key(), "advisory");
        assert_eq!(super::ReviewModePreset::Gate.key(), "gate");
        assert_eq!(super::ReviewModePreset::Strict.key(), "strict");

        let advisory = ReviewModePreset::Advisory.resolve();
        assert_eq!(advisory.mode, super::RunMode::ReviewByok);
        assert_eq!(advisory.fail_on_gate, super::FailOnGate::False);
        assert!(!advisory.review_forward);

        let gate = ReviewModePreset::Gate.resolve();
        assert_eq!(gate.mode, super::RunMode::IntelligentCi);
        assert_eq!(gate.fail_on_gate, super::FailOnGate::True);
        assert!(!gate.review_forward);

        let strict = ReviewModePreset::Strict.resolve();
        assert_eq!(strict.mode, super::RunMode::IntelligentCi);
        assert_eq!(strict.fail_on_gate, super::FailOnGate::True);
        assert!(strict.review_forward);

        // The preset names parse from clap exactly as the action input declares them.
        assert_eq!(
            <super::ReviewModePreset as clap::ValueEnum>::from_str("advisory", true),
            Ok(super::ReviewModePreset::Advisory)
        );
        assert_eq!(
            <super::ReviewModePreset as clap::ValueEnum>::from_str("gate", true),
            Ok(super::ReviewModePreset::Gate)
        );
        assert_eq!(
            <super::ReviewModePreset as clap::ValueEnum>::from_str("strict", true),
            Ok(super::ReviewModePreset::Strict)
        );
    }

    #[test]
    fn review_mode_preset_overrides_legacy_knobs() -> Result<()> {
        // Start with legacy knobs that disagree with the gate preset.
        let mut args = test_run_args(PathBuf::from("target/ub-review-test"));
        args.mode = super::RunMode::ReviewByok;
        args.fail_on_gate = super::FailOnGate::False;
        args.review_mode = Some(super::ReviewModePreset::Gate);
        let mut config = Config::default();
        // review_forward true in TOML must be overridden to false by gate preset.
        config.gate.review_forward = true;

        let resolved = crate::apply_review_mode_preset(&mut args, &mut config)
            .ok_or_else(|| anyhow::anyhow!("gate preset should resolve to Some"))?;
        assert_eq!(resolved.mode, super::RunMode::IntelligentCi);
        assert_eq!(resolved.fail_on_gate, super::FailOnGate::True);
        assert!(!resolved.review_forward);
        // The args and config are mutated to match the resolution.
        assert_eq!(args.mode, super::RunMode::IntelligentCi);
        assert_eq!(args.fail_on_gate, super::FailOnGate::True);
        assert!(!config.gate.review_forward);

        // strict forces review_forward true even when TOML says false.
        args.review_mode = Some(super::ReviewModePreset::Strict);
        config.gate.review_forward = false;
        let strict = crate::apply_review_mode_preset(&mut args, &mut config)
            .ok_or_else(|| anyhow::anyhow!("strict preset should resolve to Some"))?;
        assert!(strict.review_forward);
        assert!(config.gate.review_forward);
        Ok(())
    }

    #[test]
    fn review_mode_preset_unset_uses_legacy_knobs() {
        // No preset set: legacy knobs flow through unchanged, and the helper
        // returns None so callers know no override was applied.
        let mut args = test_run_args(PathBuf::from("target/ub-review-test"));
        args.mode = super::RunMode::ReviewByok;
        args.fail_on_gate = super::FailOnGate::Auto;
        args.review_mode = None;
        let mut config = Config::default();
        config.gate.review_forward = true;
        let resolved = crate::apply_review_mode_preset(&mut args, &mut config);
        assert!(resolved.is_none(), "unset preset must not override");
        assert_eq!(args.mode, super::RunMode::ReviewByok);
        assert_eq!(args.fail_on_gate, super::FailOnGate::Auto);
        assert!(config.gate.review_forward, "TOML review_forward preserved");
    }

    #[test]
    fn run_gate_failure_message_names_gate_outcome_artifact() {
        let failing = RunCompletion {
            gate_conclusion: "fail".to_owned(),
            fail_on_gate: true,
            run_dir: Path::new("target/ub-review").to_path_buf(),
        };
        let message = run_gate_failure_message(&failing);
        assert!(
            message
                .as_deref()
                .is_some_and(|message| message.contains("review/gate_outcome.json"))
        );

        let tolerated = RunCompletion {
            gate_conclusion: "fail".to_owned(),
            fail_on_gate: false,
            run_dir: Path::new("target/ub-review").to_path_buf(),
        };
        assert!(run_gate_failure_message(&tolerated).is_none());

        let passing = RunCompletion {
            gate_conclusion: "pass".to_owned(),
            fail_on_gate: true,
            run_dir: Path::new("target/ub-review").to_path_buf(),
        };
        assert!(run_gate_failure_message(&passing).is_none());
    }

    #[test]
    fn write_review_artifacts_records_gate_outcome_before_run_exit_decision() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let config = Config::default();
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let mut args = test_run_args(out.clone());
        args.model_mode = ModelMode::Off;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let run_started = Instant::now();
        let mut run_loop_tracker = super::RunLoopTracker::new();

        let gate_outcome = write_review_artifacts(
            temp.path(),
            &out,
            &config,
            &diff,
            &test_box_state(),
            &plan,
            "running summary",
            test_pr_thread_context(),
            &args,
            &event_log,
            &run_started,
            &mut run_loop_tracker,
            std::time::Duration::from_secs(5),
            None,
        )?;

        let written: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/gate_outcome.json"))?)?;
        assert_eq!(written["schema"], "ub-review.gate_outcome.v1");
        assert_eq!(written["conclusion"], "pass");
        assert_eq!(written["terminal_status"], "artifact-only");
        assert_eq!(written["conclusion"], gate_outcome.conclusion.as_str());
        assert_eq!(written["tool_gates"]["evaluated"], 0);
        let events = fs::read_to_string(out.join("events.ndjson"))?;
        assert!(events.contains("\"kind\":\"gate_outcome\""));
        assert!(events.contains("\"conclusion\":\"pass\""));
        let summary = render_summary(&out, &plan, &diff)?;
        assert!(
            summary
                .contains("- Gate: `pass` with `0` blocking reasons (`review/gate_outcome.json`)")
        );
        Ok(())
    }

    #[test]
    fn pipelined_late_sensor_phase_joins_before_gate_and_stays_missing_evidence() -> Result<()> {
        // #325 end-to-end: a late-phase required sensor runs behind the model
        // wave; its receipt must land before the gate evaluates, and a late
        // sensor that could not run stays missing evidence — never clean.
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        fs::create_dir_all(&out)?;
        let config = Config::default();
        let mut late_sensor = sensor_plan(
            "slowtool",
            "ub-review-test-late-tool-that-does-not-exist",
            true,
        );
        late_sensor.required = true;
        late_sensor.phase = super::SensorPhase::Late;
        let plan = test_plan(vec![late_sensor.clone()]);
        let diff = test_diff();
        let mut args = test_run_args(out.clone());
        args.model_mode = ModelMode::Off;
        args.mode = super::RunMode::IntelligentCi;
        let event_log = std::sync::Arc::new(EventLog::open(&out.join("events.ndjson"))?);
        let run_started = Instant::now();
        let mut run_loop_tracker = super::RunLoopTracker::new();
        let profile = config.selected_profile()?;

        let late_phase = super::spawn_late_sensor_phase(
            temp.path(),
            &out,
            &plan,
            profile,
            &event_log,
            &run_started,
            vec![late_sensor],
        )?;
        let gate_outcome = write_review_artifacts(
            temp.path(),
            &out,
            &config,
            &diff,
            &test_box_state(),
            &plan,
            "running summary",
            test_pr_thread_context(),
            &args,
            &event_log,
            &run_started,
            &mut run_loop_tracker,
            std::time::Duration::from_secs(5),
            Some(late_phase),
        )?;

        // The late receipt landed before the gate evaluated, and the missing
        // late required sensor reddens the intelligent-ci gate.
        let receipt: serde_json::Value = serde_json::from_slice(&fs::read(
            out.join("sensors/slowtool/ub-review-sensor-status.json"),
        )?)?;
        assert_eq!(receipt["status"], "missing");
        assert_eq!(receipt["phase"], "late");
        // Three-state gate: an evidence gap is inconclusive (blocking in
        // intelligent-ci), not a demonstrated failure — and never a pass.
        assert_eq!(gate_outcome.conclusion, "inconclusive");
        let written: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/gate_outcome.json"))?)?;
        assert_eq!(written["conclusion"], "inconclusive");

        // The shared prefix renders the late sensor as scheduled work
        // deterministically — it never reads a racing receipt.
        let shared_context = fs::read_to_string(out.join("review/shared_context.md"))?;
        assert!(
            shared_context.contains("`slowtool`: `scheduled-late`"),
            "shared context must render late sensors as scheduled: {shared_context}"
        );
        assert!(shared_context.contains("late is not missing"));

        // Join accounting: lifecycle events and the late-sensors scheduler
        // phase are recorded; tool status is computed after the join.
        let events = fs::read_to_string(out.join("events.ndjson"))?;
        assert!(events.contains("late_sensor_phase_started"));
        assert!(events.contains("late_sensor_phase_joined"));
        assert!(events.contains("late-sensors"));
        assert!(out.join("tool-status.json").is_file());
        Ok(())
    }

    #[test]
    fn compiler_surface_keeps_refuted_only_follow_up_artifact_only() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("workflow-opposition", "ok")];
        let follow_up_observation = test_observation(
            "orchestrator-follow-up-route",
            "The source-route concern was refuted by the routed proof receipt.",
            "false-premise",
            "refuted",
            "medium",
            "high",
            "source-route-refuted",
        );

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[],
            summary_only_findings: &[],
            observations: &[follow_up_observation],
            proof_receipts: &[],
            final_follow_up_tasks: 2,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(!surface.should_prepare_github_review);
        assert_eq!(surface.review_payload_status, "skipped_empty_smoke");
        assert_eq!(surface.terminal_state.status, "sufficient");
        assert_eq!(surface.terminal_state.final_follow_up_tasks, 2);
        assert!(surface.github_review.body.is_empty());
        assert!(surface.github_review.comments.is_empty());
        Ok(())
    }

    #[test]
    fn compiler_surface_keeps_resolved_check_artifact_only() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
        let resolved_observation = test_observation(
            "tests-oracle",
            "Prior author reply already answered the test-proof question.",
            "resolved-check",
            "covered",
            "low",
            "high",
            "prior-test-proof-resolved",
        );

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[],
            summary_only_findings: &[],
            observations: &[resolved_observation],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(!surface.should_prepare_github_review);
        assert_eq!(surface.review_payload_status, "skipped_empty_smoke");
        assert_eq!(surface.terminal_state.status, "sufficient");
        assert!(surface.github_review.body.is_empty());
        assert!(surface.github_review.comments.is_empty());
        Ok(())
    }

    #[test]
    fn compiler_surface_suppresses_policy_rejected_artifact_only_pr_body() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("workflow-opposition", "ok")];
        let summary_only_findings = vec![SummaryOnlyFinding {
            lane: "workflow-opposition".to_owned(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason:
                "No blocking finding after bounded review; residual risk remains for human review."
                    .to_owned(),
            evidence: "bounded lane summary".to_owned(),
        }];

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[],
            summary_only_findings: &summary_only_findings,
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(!surface.should_prepare_github_review);
        assert_eq!(surface.review_payload_status, "skipped_artifact_only_body");
        assert_eq!(surface.terminal_state.status, "sufficient");
        assert!(surface.github_review.body.is_empty());
        assert!(surface.github_review.comments.is_empty());
        Ok(())
    }

    #[test]
    fn compiler_surface_keeps_valid_inline_findings_when_body_is_suppressed() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let inline_comments = [ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/main.rs".to_owned(),
            line: 100,
            side: "RIGHT".to_owned(),
            body: "[tests-oracle] lane roster leaked into otherwise actionable finding.".to_owned(),
            evidence: "focused regression receipt".to_owned(),
            suggestion: None,
        }];

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &inline_comments,
            summary_only_findings: &[],
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(surface.github_review.body.is_empty());
        assert_eq!(surface.github_review.comments.len(), 1);
        assert!(surface.should_prepare_github_review);
        assert_eq!(surface.review_payload_status, "prepared");
        Ok(())
    }

    #[test]
    fn compiler_surface_suppresses_successful_status_tables_without_failing_gate() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = [model_lane_receipt("tests-oracle", "ok")];
        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[],
            summary_only_findings: &[],
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: Some(
                "## Model lanes\n\n- Lane: `tests`\n  Status: `ok` - completed",
            ),
        })?;

        assert!(surface.github_review.body.is_empty());
        assert!(!surface.should_prepare_github_review);
        assert_eq!(surface.review_payload_status, "skipped_artifact_only_body");
        assert_eq!(surface.terminal_state.status, "sufficient");
        Ok(())
    }

    #[test]
    fn compiler_surface_caps_inline_comments_after_value_ranking() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.max_inline_comments = 1;
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
        let inline_comments = vec![
            ReviewInlineComment {
                lane: "tests-oracle".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                path: "src/main.rs".to_owned(),
                line: 100,
                side: "RIGHT".to_owned(),
                body: "[tests-oracle] Low-value candidate arrived first.".to_owned(),
                evidence: "arrival-order fixture".to_owned(),
                suggestion: None,
            },
            ReviewInlineComment {
                lane: "security".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                path: "src/main.rs".to_owned(),
                line: 101,
                side: "RIGHT".to_owned(),
                body: "[security] High-value candidate arrived after the cap would have filled."
                    .to_owned(),
                evidence: "ranking fixture".to_owned(),
                suggestion: None,
            },
        ];

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &inline_comments,
            summary_only_findings: &[],
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(surface.should_prepare_github_review);
        assert_eq!(surface.review_payload_status, "prepared");
        assert_eq!(surface.terminal_state.status, "needs-reviewer-attention");
        assert_eq!(surface.github_review.comments.len(), 1);
        assert_eq!(surface.github_review.comments[0].line, 101);
        assert!(surface.github_review.body.contains("High-value candidate"));
        assert!(!surface.github_review.body.contains("Low-value candidate"));
        assert!(
            surface
                .artifact_body
                .contains("High-value candidate arrived after the cap")
        );
        assert!(
            surface
                .artifact_body
                .contains("Low-value candidate arrived first")
        );
        Ok(())
    }

    /// Substantive summary-only finding (severity medium+) whose reason trips
    /// the boilerplate suppressor when rendered into the PR body.
    fn substantive_summary_finding() -> SummaryOnlyFinding {
        SummaryOnlyFinding {
            lane: "opposition".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            reason: "Residual risk remains for human review in the resize realloc ordering."
                .to_owned(),
            evidence: "diff hunk src/lib.rs:42".to_owned(),
        }
    }

    /// Non-substantive summary-only finding (severity low, confidence medium)
    /// whose reason trips the boilerplate suppressor when rendered.
    fn non_substantive_summary_finding() -> SummaryOnlyFinding {
        SummaryOnlyFinding {
            lane: "workflow-opposition".to_owned(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason:
                "No blocking finding after bounded review; residual risk remains for human review."
                    .to_owned(),
            evidence: "bounded lane summary".to_owned(),
        }
    }

    fn compile_summary_only_surface(
        summary_only_body: SummaryOnlyBodyPolicy,
        summary_only_findings: &[SummaryOnlyFinding],
    ) -> Result<super::CompiledReviewSurface> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("opposition", "ok")];
        let review_body_policy = ReviewBodyPolicy {
            summary_only_body,
            ..ReviewBodyPolicy::default()
        };
        compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &review_body_policy,
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[],
            summary_only_findings,
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })
    }

    #[test]
    fn summary_only_finding_substantive_classification() {
        assert!(super::summary_only_finding_is_substantive(
            &substantive_summary_finding()
        ));
        let mut confidence_only = substantive_summary_finding();
        confidence_only.severity = "low".to_owned();
        assert!(
            super::summary_only_finding_is_substantive(&confidence_only),
            "confidence medium-high alone should qualify"
        );
        assert!(!super::summary_only_finding_is_substantive(
            &non_substantive_summary_finding()
        ));
        let lane_status_note = SummaryOnlyFinding {
            lane: "tests-oracle".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            reason: "Lane reviewed the packet without findings.".to_owned(),
            evidence: "lane model summary".to_owned(),
        };
        assert!(
            !super::summary_only_finding_is_substantive(&lane_status_note),
            "pure lane-status notes never count as substantive"
        );
    }

    #[test]
    fn summary_only_body_suppress_withholds_substantive_findings() -> Result<()> {
        let findings = vec![substantive_summary_finding()];
        let surface = compile_summary_only_surface(SummaryOnlyBodyPolicy::Suppress, &findings)?;
        assert!(!surface.should_prepare_github_review);
        assert!(!surface.summary_only_policy_posted);
        assert_eq!(surface.review_payload_status, "skipped_artifact_only_body");
        assert!(surface.github_review.body.is_empty());
        assert_eq!(surface.terminal_state.summary_only_findings, 1);
        assert_eq!(surface.terminal_state.substantive_summary_only_findings, 1);
        Ok(())
    }

    #[test]
    fn summary_only_body_post_substantive_posts_substantive_findings() -> Result<()> {
        let findings = vec![
            non_substantive_summary_finding(),
            substantive_summary_finding(),
        ];
        let surface =
            compile_summary_only_surface(SummaryOnlyBodyPolicy::PostSubstantive, &findings)?;
        assert!(
            surface.should_prepare_github_review,
            "post_substantive should post a body with a substantive finding: {}",
            surface.terminal_state.reason
        );
        assert!(surface.summary_only_policy_posted);
        assert_eq!(surface.review_payload_status, "prepared");
        assert!(
            surface
                .github_review
                .body
                .contains("Residual risk remains for human review"),
            "posted body should keep the finding content: {}",
            surface.github_review.body
        );
        assert_eq!(surface.terminal_state.status, "needs-reviewer-attention");
        assert_eq!(surface.terminal_state.substantive_summary_only_findings, 1);
        Ok(())
    }

    #[test]
    fn summary_only_body_post_substantive_withholds_non_substantive_findings() -> Result<()> {
        let findings = vec![non_substantive_summary_finding()];
        let surface =
            compile_summary_only_surface(SummaryOnlyBodyPolicy::PostSubstantive, &findings)?;
        assert!(!surface.should_prepare_github_review);
        assert!(!surface.summary_only_policy_posted);
        assert_eq!(surface.review_payload_status, "skipped_artifact_only_body");
        assert!(surface.github_review.body.is_empty());
        assert_eq!(surface.terminal_state.summary_only_findings, 1);
        assert_eq!(surface.terminal_state.substantive_summary_only_findings, 0);
        Ok(())
    }

    #[test]
    fn summary_only_body_post_all_posts_non_substantive_findings() -> Result<()> {
        let findings = vec![non_substantive_summary_finding()];
        let surface = compile_summary_only_surface(SummaryOnlyBodyPolicy::PostAll, &findings)?;
        assert!(
            surface.should_prepare_github_review,
            "post_all should post whenever any summary-only finding exists: {}",
            surface.terminal_state.reason
        );
        assert!(surface.summary_only_policy_posted);
        assert_eq!(surface.review_payload_status, "prepared");
        assert!(!surface.github_review.body.is_empty());
        Ok(())
    }

    #[test]
    fn summary_only_body_post_all_does_not_create_payload_without_summary_findings() -> Result<()> {
        // A zero inline cap leaves no PR-facing comments and no summary-only
        // findings; the summary-only knob must not create reviewer content.
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.max_inline_comments = 0;
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
        let inline_comments = (0..3)
            .map(|index| ReviewInlineComment {
                lane: "tests-oracle".to_owned(),
                severity: "medium".to_owned(),
                confidence: "high".to_owned(),
                path: "src/main.rs".to_owned(),
                line: 100 + index,
                side: "RIGHT".to_owned(),
                body: format!("Confirm concise-review guard boundary {index}."),
                evidence: "generated regression fixture".to_owned(),
                suggestion: None,
            })
            .collect::<Vec<_>>();
        let review_body_policy = ReviewBodyPolicy {
            summary_only_body: SummaryOnlyBodyPolicy::PostAll,
            ..ReviewBodyPolicy::default()
        };

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &review_body_policy,
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &inline_comments,
            summary_only_findings: &[],
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(!surface.should_prepare_github_review);
        assert!(!surface.summary_only_policy_posted);
        assert_eq!(surface.review_payload_status, "skipped_empty_smoke");
        assert!(surface.github_review.body.is_empty());
        assert!(surface.github_review.comments.is_empty());
        Ok(())
    }

    #[test]
    fn summary_only_withheld_terminal_reason_names_policy_and_counts() {
        // A value-changing proof receipt keeps reviewer_value_present true so
        // the terminal state takes the withheld-payload branch.
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let findings = vec![non_substantive_summary_finding()];
        let proof_receipts = vec![test_proof_receipt("discriminating", "ok")];
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
        let state = build_review_terminal_state(TerminalStateInput {
            args: &args,
            run_pass: super::RunPass::Manual,
            plan: &plan,
            review_payload_status: "skipped_artifact_only_body",
            should_prepare_github_review: false,
            pr_body: "",
            inline_comments: &[],
            summary_only_findings: &findings,
            summary_only_body: SummaryOnlyBodyPolicy::PostSubstantive,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            proof_receipts: &proof_receipts,
            final_follow_up_tasks: 0,
        });

        assert_eq!(state.status, "needs-reviewer-attention");
        assert!(
            state
                .reason
                .contains("summary_only_body = `post_substantive`"),
            "terminal reason should name the policy value: {}",
            state.reason
        );
        assert!(
            state
                .reason
                .contains("1 summary-only findings, 0 substantive"),
            "terminal reason should name the counts: {}",
            state.reason
        );
        assert!(
            !state
                .reason
                .contains("withheld as no-value boilerplate; diagnostics"),
            "old unconditional wording should be gone: {}",
            state.reason
        );
        assert_eq!(state.substantive_summary_only_findings, 0);
    }

    #[test]
    fn review_body_waiver_skips_suppressible_checks_only() -> Result<()> {
        let policy = ReviewBodyPolicy::default();
        let boilerplate_body = "## Confirmed findings\n\n- [opposition] Residual risk remains for human review in the resize path.";
        assert!(
            super::validate_pr_review_body_policy(boilerplate_body, &policy).is_err(),
            "boilerplate body must still fail without the waiver"
        );
        super::validate_pr_review_body_policy_with_waiver(boilerplate_body, &policy, true)?;

        let sensor_table_body = "## Confirmed findings\n\n- A finding.\n\n## Sensor status\n\n- ok";
        let err =
            super::validate_pr_review_body_policy_with_waiver(sensor_table_body, &policy, true)
                .err()
                .ok_or_else(|| anyhow::anyhow!("sensor table unexpectedly passed under waiver"))?;
        assert!(err.to_string().contains("sensor status table"), "{err:#}");
        Ok(())
    }

    #[test]
    fn review_body_summary_only_body_parses_known_values() -> Result<()> {
        for (value, expected) in [
            ("suppress", SummaryOnlyBodyPolicy::Suppress),
            ("post_substantive", SummaryOnlyBodyPolicy::PostSubstantive),
            ("post-substantive", SummaryOnlyBodyPolicy::PostSubstantive),
            ("post_all", SummaryOnlyBodyPolicy::PostAll),
            ("post-all", SummaryOnlyBodyPolicy::PostAll),
        ] {
            let config = Config::from_toml_with_policy_receipts(&format!(
                "[review_body]\nsummary_only_body = \"{value}\"\n"
            ))?;
            assert!(
                config.policy_errors.is_empty(),
                "`{value}` should parse without policy errors: {:?}",
                config.policy_errors
            );
            assert_eq!(config.review_body.summary_only_body, expected, "{value}");
        }
        assert_eq!(
            Config::default().review_body.summary_only_body,
            SummaryOnlyBodyPolicy::Suppress,
            "consumer default must stay suppress"
        );
        Ok(())
    }

    #[test]
    fn review_body_unknown_policy_values_are_receipted() -> Result<()> {
        let config = Config::from_toml_with_policy_receipts(
            "[review_body]\ninclude_successful_lane_table = true\nsummary_only_body = \"post-everything\"\n",
        )?;
        assert_eq!(config.policy_errors.len(), 1, "{:?}", config.policy_errors);
        assert_eq!(
            config.policy_errors[0].section,
            "review_body.summary_only_body"
        );
        assert!(
            config.policy_errors[0].detail.contains("summary_only_body"),
            "{:?}",
            config.policy_errors[0]
        );
        // Valid siblings keep working; the bad key falls back to the default.
        assert!(config.review_body.include_successful_lane_table);
        assert_eq!(
            config.review_body.summary_only_body,
            SummaryOnlyBodyPolicy::Suppress
        );

        let misspelled = Config::from_toml_with_policy_receipts(
            "[review_body]\nsummary_only_bodyy = \"suppress\"\n",
        )?;
        assert_eq!(
            misspelled.policy_errors.len(),
            1,
            "{:?}",
            misspelled.policy_errors
        );
        assert_eq!(
            misspelled.policy_errors[0].section,
            "review_body.summary_only_bodyy"
        );
        Ok(())
    }

    #[test]
    fn post_validation_honors_effective_summary_only_body_policy() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let run_dir = temp.path();
        let review_dir = run_dir.join("review");
        fs::create_dir_all(&review_dir)?;
        let args = PostArgs {
            review_json: review_dir.join("github-review.json"),
            diff_patch: None,
            out: review_dir.clone(),
            github_token: Some("token".to_owned()),
            repo: Some("EffortlessMetrics/ub-review".to_owned()),
            pull_number: Some(1),
            github_api_url: "https://api.github.com".to_owned(),
            fail_on_post_error: false,
        };
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Confirmed findings\n\n- [opposition] Residual risk remains for human review in the resize path.".to_owned(),
            comments: Vec::new(),
        };

        // Without an effective config the conservative default rejects the body.
        let err = super::validate_github_review_payload_for_post(&args, &review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("boilerplate body unexpectedly passed post checks"))?;
        assert!(
            err.to_string().contains("artifact-only boilerplate"),
            "{err:#}"
        );

        // With a posting posture in effective-config.json the post step honors
        // the run's compile decision and waives the suppressible classes.
        let mut config = Config::default();
        config.review_body.summary_only_body = SummaryOnlyBodyPolicy::PostSubstantive;
        fs::write(
            run_dir.join("effective-config.json"),
            serde_json::to_vec_pretty(&config)?,
        )?;
        super::validate_github_review_payload_for_post(&args, &review)?;
        Ok(())
    }

    fn test_pass_policy_inline_comment() -> ReviewInlineComment {
        ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            path: "src/main.rs".to_owned(),
            line: 100,
            side: "RIGHT".to_owned(),
            body: "Confirm the resize path cannot alias the detached buffer.".to_owned(),
            evidence: "generated regression fixture".to_owned(),
            suggestion: None,
        }
    }

    #[test]
    fn compiler_surface_skips_pass_excluded_by_post_review_on() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.posting = PostingMode::Review;
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
        let inline_comments = vec![test_pass_policy_inline_comment()];
        let two_pass: Vec<String> = vec!["opened".to_owned(), "ready_for_review".to_owned()];

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Synchronize,
            post_review_on: &two_pass,
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &inline_comments,
            summary_only_findings: &[],
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(!surface.should_prepare_github_review);
        assert_eq!(surface.review_payload_status, "skipped_pass_policy");
        assert_eq!(surface.terminal_state.status, "needs-reviewer-attention");
        assert!(surface.terminal_state.reviewer_value_present);
        assert!(
            surface
                .terminal_state
                .reason
                .contains("pass `synchronize` is not in [gate].post_review_on"),
            "terminal reason should name the pass policy: {}",
            surface.terminal_state.reason
        );
        Ok(())
    }

    #[test]
    fn compiler_surface_preserves_unsafe_review_suggestions() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("unsafe-review", "ok")];
        let inline_comments = vec![ReviewInlineComment {
            lane: "unsafe-review".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/main.rs".to_owned(),
            line: 100,
            side: "RIGHT".to_owned(),
            body: "[unsafe-review] Guard evidence is missing.".to_owned(),
            evidence: "unsafe-review comment-plan card-001".to_owned(),
            suggestion: Some("let header = guarded_header_read(ptr)?;".to_owned()),
        }];

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &inline_comments,
            summary_only_findings: &[],
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(surface.should_prepare_github_review);
        assert_eq!(
            surface.github_review.comments[0].suggestion.as_deref(),
            Some("let header = guarded_header_read(ptr)?;")
        );
        Ok(())
    }

    #[test]
    fn compiler_surface_prepares_review_for_synchronize_pass_in_profile_list() -> Result<()> {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.posting = PostingMode::Review;
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
        let inline_comments = vec![test_pass_policy_inline_comment()];
        let every_pass: Vec<String> = vec![
            "opened".to_owned(),
            "reopened".to_owned(),
            "ready_for_review".to_owned(),
            "synchronize".to_owned(),
        ];

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Synchronize,
            post_review_on: &every_pass,
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &inline_comments,
            summary_only_findings: &[],
            observations: &[],
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(
            surface.should_prepare_github_review,
            "synchronize pass in post_review_on should keep the payload prepared: {}",
            surface.terminal_state.reason
        );
        assert_eq!(surface.review_payload_status, "prepared");
        assert_eq!(surface.terminal_state.status, "needs-reviewer-attention");
        assert_eq!(surface.github_review.comments.len(), 1);
        assert_eq!(
            surface.terminal_state.reason,
            "Reviewer-value content survived compilation; a grouped PR review was prepared."
        );
        Ok(())
    }

    #[test]
    fn artifact_review_body_keeps_model_lane_roster() {
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[model_lane_receipt("ub-memory-lifetime", "ok")],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::Artifact,
        );

        assert!(body.contains("## Model lanes"));
        assert!(body.contains("Lane: `ub-memory-lifetime`"));
        assert!(body.contains("Provider: `minimax`"));
        assert!(body.contains("Model: `MiniMax-M3`"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn workflow_pr_body_uses_workflow_route_language() {
        let mut plan = test_plan(Vec::new());
        plan.diff_class = DiffClass::WorkflowTooling;
        plan.lanes =
            default_lanes_for_diff_context(DiffClass::WorkflowTooling, &LanguageMix::default());
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec![".github/workflows/ub-review.yml".to_owned()],
            patch: "+permissions:\n+  contents: read\n".to_owned(),
            flags: classify_diff(
                &[".github/workflows/ub-review.yml".to_owned()],
                "+permissions:\n+  contents: read\n",
            ),
            diff_class: DiffClass::WorkflowTooling,
        };

        let body = render_review_body(
            "abc123",
            &plan,
            &diff,
            &[model_lane_receipt("workflow-permissions", "ok")],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("ArrayBuffer"));
        assert!(!body.contains("worker handoff"));
        assert!(!body.contains("unsafe/native seams"));
        assert!(!body.contains("test-oracle strength"));
        assert!(!body.contains("actionlint/zizmor"));
        assert!(!body.contains("## Residual risk"));
        assert!(!body.contains("A human should still inspect"));
    }

    #[test]
    fn verifier_script_scope_noise_stays_artifact_only() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let mut plan = test_plan(Vec::new());
        plan.diff_class = DiffClass::WorkflowTooling;
        plan.lanes =
            default_lanes_for_diff_context(DiffClass::WorkflowTooling, &LanguageMix::default());
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["scripts/verify-bun-review-artifacts.py".to_owned()],
            patch: "+import tempfile\n".to_owned(),
            flags: classify_diff(
                &["scripts/verify-bun-review-artifacts.py".to_owned()],
                "+import tempfile\n",
            ),
            diff_class: DiffClass::WorkflowTooling,
        };
        let model_lanes = vec![model_lane_receipt("workflow-proof", "ok")];
        let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "workflow-pinning".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "workflow-pinning lane has no actionable finding: no action versions, runner images, or setup steps were modified by this Python-only diff.".to_owned(),
                evidence: "Changed files: scripts/verify-bun-review-artifacts.py; no workflow YAML changes.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "No workflow lint proof is applicable because the PR diff does not modify any GitHub Actions YAML. Actionlint availability is not a trust gap for this PR.".to_owned(),
                evidence: "Changed files=1, scripts/verify-bun-review-artifacts.py; actionlint skipped because no workflow files changed.".to_owned(),
            },
        ];
        let self_test_coverage = test_observation(
            "workflow-proof",
            "New self-tests use tempfile.TemporaryDirectory and assert both happy-path and failure-path via expect_self_test_failure; this is a focused smoke proof pattern suitable for Python change verification.",
            "test-gap",
            "open",
            "low",
            "medium",
            "self-test-coverage",
        );
        let mut actionlint_skip = test_observation(
            "workflow-pinning",
            "actionlint/zizmor unavailable (skipped) for this diff; no GitHub Actions YAML changed, so absence of proof is not trust-affecting for pinning review.",
            "missing-evidence",
            "open",
            "low",
            "medium",
            "wp.missing.actionlint-zizmor",
        );
        actionlint_skip.path = Some("scripts/verify-bun-review-artifacts.py".to_owned());
        actionlint_skip.evidence = vec![
            "actionlint skipped - trigger did not match; zizmor disabled by config; no YAML in diff."
                .to_owned(),
        ];
        let mut python_only_scope = test_observation(
            "workflow-opposition",
            "Diff is Python-only in scripts/; no GitHub Actions YAML, permissions, triggers, or action pins touched, so workflow/tooling opposition surfaces are limited to the validator script itself.",
            "verification-question",
            "open",
            "medium",
            "medium-high",
            "empty-candidates-dir-acceptance",
        );
        python_only_scope.path = Some("scripts/verify-bun-review-artifacts.py".to_owned());
        let mut trust_language_softening = test_observation(
            "opposition",
            "Observation text for actionlint/zizmor pinning changed from 'unverified by sensors for this run' to 'unavailability is not trust-affecting' - this is a softer trust claim that should be defended by a concrete rule, not just narrative softening.",
            "source-route-gap",
            "open",
            "medium",
            "medium",
            "source-route/trust-language-softening",
        );
        trust_language_softening.path = Some("src/main.rs".to_owned());
        trust_language_softening.evidence = vec![
            "String literal changed to '...absence of proof is not trust-affecting for pinning review' while the new gap-noise clause now matches 'no yaml in diff' and 'no github actions yaml'."
                .to_owned(),
        ];
        let self_test_receipt = test_observation(
            "proof-planner",
            "Self-test wiring is in run_self_tests; if --self-test is not executed in CI, the new branches are unverified at gate time. PR body asserts it ran, but receipt not in seeded thread.",
            "verification-question",
            "open",
            "medium",
            "medium",
            "proof-planner.self-test-evidence",
        );
        let observations = vec![
            self_test_coverage,
            actionlint_skip,
            python_only_scope,
            trust_language_softening,
            self_test_receipt,
        ];

        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[],
            summary_only_findings: &summary_only_findings,
            observations: &observations,
            proof_receipts: &[],
            final_follow_up_tasks: 0,
            suggested_issues: &[],
            reporter_distillation: None,
        })?;

        assert!(
            !surface.should_prepare_github_review,
            "{}",
            surface.github_review.body
        );
        assert_eq!(surface.review_payload_status, "skipped_empty_smoke");
        assert_eq!(surface.terminal_state.status, "sufficient");
        assert!(surface.github_review.body.is_empty());
        assert!(surface.github_review.comments.is_empty());
        assert!(surface.artifact_body.contains("workflow-pinning lane"));
        Ok(())
    }

    #[test]
    fn pr_review_body_hides_machine_metadata_for_findings() {
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[ReviewInlineComment {
                lane: "opposition".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/postgres.rs".to_owned(),
                line: 196,
                side: "RIGHT".to_owned(),
                body: "Confirm the C++ copy cannot race detach or resize between the Rust guard and native read.".to_owned(),
                evidence: "line 196 calls Bun__createArrayBufferForCopy".to_owned(),
                suggestion: None,
            }],
            &[] as &[SummaryOnlyFinding],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Confirmed findings"));
        assert!(body.contains("Confirm the C++ copy cannot race detach or resize"));
        assert!(!body.contains("Shared context"));
        assert!(!body.contains("Profile:"));
        assert!(!body.contains("Changed files:"));
        assert!(!body.contains("Inline comments:"));
        assert!(!body.contains("`[opposition]`"));
        assert!(!body.contains("medium-high"));
        assert!(!body.contains("src/postgres.rs"));
        assert!(!body.contains("Evidence:"));
        assert!(!body.contains("line 196 calls"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_keeps_compiler_residue_artifact_only() {
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "source-route".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "inline guard rejected src/lib.rs:12; severity_allowed=true confidence_allowed=true line_valid=false concise=true body_present=true evidence_present=true repo_relative=true".to_owned(),
                evidence: "compiler guard metadata".to_owned(),
            }],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("inline guard rejected"));
        assert!(!body.contains("severity_allowed"));
        assert!(!body.contains("compiler guard metadata"));
        assert!(!body.contains("## Confirmed findings"));
        assert!(!body.contains("## Verification questions"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_keeps_clean_lane_summary_artifact_only() {
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Workflow-permissions lane: no token-scope, permissions, pull_request_target, or fork-vector change. Lane is clean.".to_owned(),
                evidence: "lane model summary".to_owned(),
            }],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("Lane is clean"));
    }

    #[test]
    fn pr_review_body_drops_stale_workflow_cache_path_summary() {
        let mut diff = test_diff();
        diff.diff_class = DiffClass::WorkflowTooling;
        diff.changed_files = vec![".github/workflows/ub-review-packet.yml".to_owned()];
        diff.patch =
            "+ key: ub-review-gh-runner-v2-c52b0e4403384cc7836e162a54df005cbcab968d\n".to_owned();
        let finding = SummaryOnlyFinding {
            lane: "workflow-proof".to_owned(),
            severity: "high".to_owned(),
            confidence: "medium-high".to_owned(),
            reason: "actionlint sensor ran ok, but the diff adds '~/go/bin/actionlint' to the cache path block and prior seeded reviews failed; attach a fresh actionlint proof.".to_owned(),
            evidence: "cache path '~/go/bin/actionlint' added in hunk with no install step visible.".to_owned(),
        };
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &diff,
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            std::slice::from_ref(&finding),
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("actionlint"));

        diff.patch.push_str("+            ~/go/bin/actionlint\n");
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &diff,
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[finding],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("actionlint"));
    }

    #[test]
    fn pr_review_body_keeps_workflow_status_residue_artifact_only() {
        let mut diff = test_diff();
        diff.diff_class = DiffClass::WorkflowTooling;
        diff.changed_files = vec![".github/workflows/ub-review-packet.yml".to_owned()];
        diff.patch = concat!(
            "+          key: ub-review-gh-runner-v2-4dfbd9d7caeff4f506984c63cc36f248233206e6-${{ runner.os }}-rust-1.95-core\n",
            "+            ub-review-gh-runner-v2-4dfbd9d7caeff4f506984c63cc36f248233206e6-${{ runner.os }}-rust-1.95-\n",
            "+        uses: EffortlessMetrics/ub-review@4dfbd9d7caeff4f506984c63cc36f248233206e6\n",
        )
        .to_owned();
        let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "workflow-permissions lane: no permissions, pull_request_target, token-scope, or fork-vector change in this diff. No new auth surface. actionlint ok in this packet.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "Pin of EffortlessMetrics/ub-review to a specific commit SHA is supply-chain tightening and inherits that repo's default GITHUB_TOKEN scope; no new scope is requested in the workflow itself. Worth a one-line note for future audits that third-party action token scope is not visible here.".to_owned(),
                evidence: "uses: EffortlessMetrics/ub-review@4dfbd9d7caeff4f506984c63cc36f248233206e6 with preset/profile inputs only; no permissions: override.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "refuter demoted inline candidate at .github/workflows/ub-review-packet.yml:56: the PR packet contains no upstream commit-existence/ancestry proof, and the PR body says 'Gate proof is pending this PR's UB evidence packet.' Confirming reachability requires an external API call that the lane cannot perform from cached context.".to_owned(),
                evidence: "uses: EffortlessMetrics/ub-review@4dfbd9d7caeff4f506984c63cc36f248233206e6 - only local diff evidence, no upstream commit proof".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-pinning".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "Ub-review action receives secrets.MINIMAX_API_KEY and github.token at runtime; a malicious or compromised dad0f23 would exfiltrate these. Pinning to SHA is correct posture but does not eliminate upstream trust.".to_owned(),
                evidence: "The diff is an atomic ub-review SHA/cache-key bump; the with: block is unchanged.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-pinning".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "No pinning defect introduced. The only standing concern is upstream SHA trust for EffortlessMetrics/ub-review@e76ccbc, which is identical in posture to the prior pin and is a repo-level policy item, not a diff finding.".to_owned(),
                evidence: "Per-action full-SHA pinning preserved; old pin fully removed; 3 replacement sites consistent; actionlint ok; workflow file diff is 4 lines, no perm/trigger change.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "The diff is a 4-line mechanical SHA bump (da14100 -> 88f3dcc7c344f8b54871caea2122de0b68701925) at the three expected sites: cache `key` (line 56), `restore-keys` prefix (line 58), and action `uses:` (line 62). No permission, trigger, or `with:` block change; net new secret/permission surface relative to the prior pin is zero.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Workflow-proof lane: actionlint receipt ok; no Rust/source surface touched (yaml-only). Pin/uses ref consistent at 3x ec8f890; old da14100 absent. Strongest failed objection: actionlint proof receipt not inlined for me to re-verify, and no fresh PR-build smoke. cursor/coderabbit stale SHA mismatch is a false positive (cites e76ccbc while PR targets ec8f890).".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "actionlint 'ok' status is reported by the sensor table but the underlying lint output is not inlined into the workflow-proof lane packet; trust in 'no workflow lint findings' therefore depends on the central proof broker artifact at sensors/actionlint/. For a 4-line SHA-swap with consistent 40-hex pin and unchanged permissions/trigger, this is a parked follow-up, not a blocker.".to_owned(),
                evidence: "sensor table: actionlint=ok; receipt path sensors/actionlint/; yaml-only diff; pin 40-hex non-zero".to_owned(),
            },
        ];
        let mut tokmd_gap = test_observation(
            "sensor-tokmd",
            "Sensor `tokmd` evidence is `missing`: command not found",
            "missing-evidence",
            "open",
            "low",
            "high",
            "sensor-tokmd-missing",
        );
        tokmd_gap.evidence = vec!["command not found".to_owned()];
        let observations = vec![
            test_observation(
                "workflow-permissions",
                "workflow-permissions lane: no permissions, pull_request_target, token-scope, or fork-vector change in this diff. No new auth surface. actionlint ok in this packet.",
                "bug",
                "open",
                "low",
                "medium",
                "bug:workflow-permissions",
            ),
            test_observation(
                "workflow-permissions",
                "Workflow-level permissions block on the caller workflow was not modified; perms posture remains whatever the base file declares (pre-existing, not a diff target).",
                "verification-question",
                "open",
                "low",
                "high",
                "wf-perms-unchanged",
            ),
            test_observation(
                "workflow-pinning",
                "Ub-review action receives secrets.MINIMAX_API_KEY and github.token at runtime; a malicious or compromised dad0f23 would exfiltrate these. Pinning to SHA is correct posture but does not eliminate upstream trust.",
                "security-risk",
                "open",
                "medium",
                "medium-high",
                "wf-pin-secrets-surface",
            ),
            test_observation(
                "workflow-pinning",
                "Pinning format could be 39-hex or all-zero making the gate unsafe.; refuted because: e76ccbcbe94258fd03cf6ddb4e1536833cad610d is 40 hex characters, non-zero, and matches expected SHA-1 shape; the gate's SHA-pinning control remains effective.",
                "false-premise",
                "refuted",
                "low",
                "high",
                "false-premise:workflow-pinning",
            ),
            test_observation(
                "workflow-permissions",
                "cursor[bot] and coderabbitai[bot] comments claim target is e76ccbcb... and demand swap back; PR body, diff, and head tree all show ec8f890 as the actual target. Their objection is a false positive against the current diff and reopens nothing.",
                "false-premise",
                "refuted",
                "low",
                "high",
                "wf-perms-stale-cursor-coderabbit",
            ),
            test_observation(
                "workflow-proof",
                "actionlint receipt is 'ok' per sensor table; no per-line output inlined into this lane packet, so re-verification of lint findings depends on the central proof broker artifact",
                "missing-evidence",
                "open",
                "low",
                "medium",
                "wf-proof:actionlint-receipt-inlined?",
            ),
            test_observation(
                "workflow-proof",
                "No fresh PR-build smoke run is available (build/test skipped, --allow-heavy required); only tokmd/actionlint receipts are present for this 4-line workflow pin",
                "missing-evidence",
                "open",
                "low",
                "high",
                "missing-evidence:.github/workflows/ub-review-packet.yml:62",
            ),
            test_observation(
                "orchestrator-follow-up",
                "`pull_request` types limited to `opened` and `ready_for_review` cause pushes to skip re-running the UB packet.",
                "bug",
                "open",
                "medium",
                "medium",
                "follow-up-trigger-scope",
            ),
            test_observation(
                "workflow-proof",
                "actionlint install step absent from the changed hunk: a future runner without preinstalled ~/go/bin/actionlint would cache-restore an empty path.",
                "bug",
                "open",
                "medium",
                "medium",
                "workflow-tool-cache-path",
            ),
            test_observation(
                "workflow-pinning",
                "v2-4dfbd9d7 cache key will collide on a future re-pin sharing the same prefix; refuted because the current diff uses the full 40-hex SHA in both key and restore-keys.",
                "false-premise",
                "refuted",
                "low",
                "high",
                "sha-prefix-refuted",
            ),
            test_observation(
                "workflow-permissions",
                "third-party action EffortlessMetrics/ub-review@4fdab02 may inherit broader token scope and widen permissions; refuted because no permissions key changed, and pinning to a SHA is supply-chain tightening not a scope change.",
                "false-premise",
                "refuted",
                "low",
                "high",
                "third-party-token-scope-refuted",
            ),
            test_observation(
                "workflow-pinning",
                "Floating @v0.1 tag is a supply-chain widening risk that this PR fails to close; refuted because pinning to immutable commit SHA is strict supply-chain tightening.",
                "false-premise",
                "refuted",
                "low",
                "high",
                "floating-tag-refuted",
            ),
            test_observation(
                "workflow-pinning",
                "Cache key/restore-keys embedded with full 40-hex SHA prefix; prior 7-char prefix collision objection is now resolved by using the entire SHA.",
                "residual-risk",
                "parked",
                "low",
                "high",
                "full-sha-prefix-resolved",
            ),
            test_observation(
                "workflow-pinning",
                "pull_request trigger scoped to [opened, ready_for_review] means new pushes on an open PR do not re-run the UB packet; reviewer evidence can stale as HEAD advances (cursor bug 40e40af3).",
                "residual-risk",
                "open",
                "low",
                "high",
                "cursor-trigger-stale",
            ),
            test_observation(
                "workflow-proof",
                "PR-body contract hardening claimed for ccae442 is not verifiable from the repo diff itself; trust depends on upstream tag of EffortlessMetrics/ub-review@ccae442.",
                "verification-question",
                "open",
                "medium",
                "medium",
                "pr-body-contract-meta",
            ),
            test_observation(
                "workflow-opposition",
                "Cache key/uses ref must be a single coherent bump; if SHA were 39-hex or all-zero the gate would be unsafe. Ccae442de05bb9330e5d45bbbebfd64c6c39ee93 is 40 hex and non-zero.",
                "verification-question",
                "open",
                "medium",
                "medium",
                "sha-format-meta",
            ),
            test_observation(
                "orchestrator-follow-up",
                "The cached prior observation still matches the current PR evidence and is not reopened by the diff.",
                "bug",
                "open",
                "low",
                "medium",
                "cached-prior-meta",
            ),
            test_observation(
                "orchestrator-follow-up",
                "The refutation claiming this PR is a benign SHA pin bump with no workflow posture impact still matches current evidence.",
                "bug",
                "open",
                "low",
                "medium",
                "refutation-meta",
            ),
            test_observation(
                "workflow-opposition",
                "Workflow posture is unsafe because the new pin dad0f23 receives secrets and is not reproducibly verified in this repo.; refuted because: False-premise at the opposition-lane level: the diff itself adds zero new secret/permission surface relative to the prior pin ccae442; pinning-by-SHA is the established control and is preserved. Trust in upstream tag is a standing-repo concern, not introduced by this 4-line bump.",
                "false-premise",
                "refuted",
                "low",
                "high",
                "false-premise:workflow-opposition",
            ),
            test_observation(
                "workflow-permissions",
                "Actionlint ran ok; combined with disabled zizmor/gitleaks, security-relevant linting for this pin-only diff is partial but the diff class does not require them.",
                "missing-evidence",
                "open",
                "low",
                "medium",
                "tool-status-only-gap",
            ),
            tokmd_gap,
        ];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &diff,
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &summary_only_findings,
            &observations,
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("Needs reviewer attention"));
        assert!(!body.contains("Confirmed findings"));
        assert!(!body.contains("workflow-permissions lane"));
        assert!(!body.contains("ready_for_review"));
        assert!(!body.contains("actionlint install step"));
        assert!(!body.contains("Floating @v0.1"));
        assert!(!body.contains("third-party action"));
        assert!(!body.contains("prefix collision"));
        assert!(!body.contains("tokmd"));
        assert!(!body.contains("cached prior observation"));
        assert!(!body.contains("refuter demoted inline candidate"));
        assert!(!body.contains("Gate proof is pending"));
        assert!(!body.contains("40 hex and non-zero"));
        assert!(!body.contains("Actionlint ran ok"));
        assert!(!body.contains("malicious or compromised"));
        assert!(!body.contains("does not eliminate upstream trust"));
        assert!(!body.contains("pre-existing, not a diff target"));
        assert!(!body.contains("standing-repo concern"));
        assert!(!body.contains("No pinning defect introduced"));
        assert!(!body.contains("repo-level policy item"));
        assert!(!body.contains("4-line mechanical SHA bump"));
        assert!(!body.contains("net new secret/permission surface"));
        assert!(!body.contains("Pinning format could be 39-hex"));
        assert!(!body.contains("SHA-pinning control remains effective"));
        assert!(!body.contains("cursor[bot]"));
        assert!(!body.contains("coderabbit"));
        assert!(!body.contains("stale-bot false positives"));
        assert!(!body.contains("central proof broker artifact"));
        assert!(!body.contains("No fresh PR-build smoke"));
        assert!(!body.contains("--allow-heavy"));
    }

    #[test]
    fn pr_review_body_keeps_paths_ignore_no_posture_review_artifact_only() {
        let mut diff = test_diff();
        diff.diff_class = DiffClass::WorkflowTooling;
        diff.changed_files = vec![".github/workflows/droid-focused-review.yml".to_owned()];
        diff.patch = concat!(
            " pull_request:\n",
            "   paths-ignore:\n",
            "+    - \".github/workflows/ub-review-packet.yml\"\n",
        )
        .to_owned();
        let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Confirm checkout credential persistence: workflows using pull_request from forks receive a read-only GITHUB_TOKEN; this lane did not change checkout config, so no new persistence vector is introduced. Actionlint receipt 'ok' supports no syntactic regression.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Adding a workflow file to paths-ignore could grant implicit permission expansion; refuted because: paths-ignore only filters trigger activation; it does not alter token scopes, permissions blocks, or any job-level security context.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "zizmor, gitleaks, osv-scanner, cargo-audit, cargo-deny, shellcheck, semgrep, coverage all disabled by config or trigger-mismatched. No security/pinning tool independently re-validated this workflow file.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Confirm no focused smoke proof (workflow_run on a fork-PR dry-run, or a temporary pull_request_target guard test) was executed for the paths-ignore change. Trust rests on actionlint parse only; semantic skip behavior on the droid lane is not proven by sensors.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "PR body states actionlint is not installed locally, so the 'ok' receipt must come from the ub-review gate's own tooling rather than a local pre-push run; trust depends on that gate having actually executed actionlint v1 against this ref.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "CodeRabbit's review-comment at ub-review-packet.yml:58 asserts the PR gate target SHA is 892e1bb44b7cb24753b7701b405d078f4ef11ee1, not be524219e33ff37edeab61ddc28c01250a08b492 used in the diff. If that claim is correct the workflow pin does not match the upstream gate and the packet will be filtered/no-posture.".to_owned(),
                evidence: "CodeRabbit review-comment on .github/workflows/ub-review-packet.yml:58, scripted check showing 0 references to 892e1bb44b... in the file; PR body and droid-ub/droid-tests receipts only confirm internal lockstep, not match to gate target.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Confirm actionlint receipt 'ok' confirms syntactic validity, but no semantic proof of skip behavior on the droid lane is available; trust rests on actionlint parse plus per-PR trigger semantics - the droid lane is auxiliary/non-blocking and the UB gate is authoritative, so residual workflow risk is bounded.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Residual workflow risk: cache key/restore-keys prefix is coupled to action SHA. Any future repin must update all three sites; a partial update silently mismatches cache restore. Not actionable in this PR (current state is consistent) - parked for follow-up lint rule or script.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "trust gap: no focused smoke proof (workflow_run on fork-PR dry-run or pull_request_target guard) executed for the paths-ignore change; semantic skip behavior on Droid lane unproven beyond actionlint parse.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
        ];
        let observations = vec![
            test_observation(
                "workflow-permissions",
                "Confirm checkout credential persistence: workflows using pull_request from forks receive a read-only GITHUB_TOKEN; this lane did not change checkout config, so no new persistence vector is introduced. Actionlint receipt 'ok' supports no syntactic regression.",
                "verification-question",
                "open",
                "low",
                "medium",
                "checkout-persistence-no-change",
            ),
            test_observation(
                "workflow-opposition",
                "Adding a workflow file to paths-ignore could grant implicit permission expansion; refuted because: paths-ignore only filters trigger activation; it does not alter token scopes, permissions blocks, or any job-level security context.",
                "false-premise",
                "refuted",
                "low",
                "medium",
                "paths-ignore-permission-refuted",
            ),
            test_observation(
                "workflow-proof",
                "paths-ignore is a literal substring/glob match; a future rename of ub-review-packet.yml silently re-enables Droid noise.",
                "parked-follow-up",
                "parked",
                "low",
                "medium",
                "paths-ignore-future-rename",
            ),
            test_observation(
                "workflow-proof",
                "Confirm no focused smoke proof (workflow_run on a fork-PR dry-run, or a temporary pull_request_target guard test) was executed for the paths-ignore change. Trust rests on actionlint parse only; semantic skip behavior on the droid lane is not proven by sensors.",
                "verification-question",
                "open",
                "low",
                "medium",
                "paths-ignore-smoke-proof-gap",
            ),
            test_observation(
                "workflow-proof",
                "Confirm actionlint receipt 'ok' confirms syntactic validity, but no semantic proof of skip behavior on the droid lane is available; trust rests on actionlint parse plus per-PR trigger semantics - the droid lane is auxiliary/non-blocking and the UB gate is authoritative, so residual workflow risk is bounded.",
                "verification-question",
                "open",
                "low",
                "medium",
                "actionlint-semantic-skip-proof",
            ),
            test_observation(
                "workflow-proof",
                "Residual workflow risk: cache key/restore-keys prefix is coupled to action SHA. Any future repin must update all three sites; a partial update silently mismatches cache restore. Not actionable in this PR (current state is consistent) - parked for follow-up lint rule or script.",
                "parked-follow-up",
                "parked",
                "low",
                "medium",
                "cache-key-current-pin-followup",
            ),
            test_observation(
                "workflow-proof",
                "trust gap: no focused smoke proof (workflow_run on fork-PR dry-run or pull_request_target guard) executed for the paths-ignore change; semantic skip behavior on Droid lane unproven beyond actionlint parse.",
                "missing-evidence",
                "open",
                "low",
                "medium",
                "actionlint-skip-proof-gap",
            ),
        ];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &diff,
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &summary_only_findings,
            &observations,
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("checkout credential persistence"));
        assert!(!body.contains("paths-ignore"));
        assert!(!body.contains("focused smoke proof"));
        assert!(!body.contains("semantic skip behavior"));
        assert!(!body.contains("cache key/restore-keys"));
        assert!(!body.contains("actionlint is not installed locally"));
        assert!(!body.contains("CodeRabbit"));
        assert!(!body.contains("892e1bb"));
        assert!(!body.contains("zizmor"));
        assert!(!body.contains("Droid noise"));
    }

    #[test]
    fn pr_review_body_keeps_refuted_only_observations_artifact_only() {
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[test_observation(
                "workflow-opposition",
                "This diff widens the workflow permission/secret surface; refuted because the changed hunk only updates an already pinned action ref.",
                "false-premise",
                "refuted",
                "low",
                "high",
                "refuted-only-workflow-posture",
            )],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("## Refuted"));
        assert!(!body.contains("widens the workflow permission"));
    }

    #[test]
    fn pr_review_body_keeps_workflow_lockstep_summaries_artifact_only() {
        let mut diff = test_diff();
        diff.diff_class = DiffClass::WorkflowTooling;
        diff.changed_files = vec![
            ".github/workflows/droid-focused-review.yml".to_owned(),
            ".github/workflows/ub-review-packet.yml".to_owned(),
        ];
        diff.patch = concat!(
            " pull_request:\n",
            "   paths-ignore:\n",
            "+    - \".github/workflows/ub-review-packet.yml\"\n",
            "+          key: ub-review-gh-runner-v2-cec5b07457f99e652a500dffc603f98e6082a7f7-${{ runner.os }}-rust-1.95-core\n",
            "+            ub-review-gh-runner-v2-cec5b07457f99e652a500dffc603f98e6082a7f7-${{ runner.os }}-rust-1.95-\n",
            "+        uses: EffortlessMetrics/ub-review@cec5b07457f99e652a500dffc603f98e6082a7f7\n",
        )
        .to_owned();
        let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "workflow-permissions lane: paths-ignore addition cannot alter token scopes/permissions; pin bump is lockstep across cache key, restore-keys prefix, and uses: reference. No new third-party action, no pull_request_target, no checkout credential persistence vector. Residual risk: cache key/pin coupling is a parked follow-up; no blocker.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-pinning".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Workflow-pinning lane for PR #49. Two workflow YAML files touched. Pin lockstep verified for cec5b07457f99e652a500dffc603f98e6082a7f7 across 3 sites, old pin absent, cache key/restore-keys prefix match, no other third-party actions changed.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Workflow lint proof lane: actionlint receipt ok, no focused smoke proof available, no syntactic regression. Pin lockstep and paths-ignore semantics covered by seeded thread; CodeRabbit stale SHA comment is stale (current head re-pinned to cec5b074).".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "Cache key/restore-keys prefix is coupled to action SHA; any future partial repin silently mismatches restore. Current state consistent, parked for lint-rule follow-up.".to_owned(),
                evidence: "diff: cache key, restore-keys prefix, uses: all reference cec5b074 in lockstep".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Opposition lane for workflow/tooling: paths-ignore addition + lockstep SHA pin. Actionlint ok. No source, no permissions, no token, no checkout changes. No blocker.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Residual workflow risk: cache key/restore-keys prefix is manually coupled to action SHA. Partial repin silently breaks cache restore. Parked for follow-up lint rule.".to_owned(),
                evidence: "cache key/restore-keys must be updated in lockstep with uses: pin; no automated guard exists; current PR state is consistent".to_owned(),
            },
        ];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &diff,
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &summary_only_findings,
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty());
        assert!(!body.contains("Workflow-pinning lane"));
        assert!(!body.contains("Pin lockstep verified"));
        assert!(!body.contains("Current state consistent"));
        assert!(!body.contains("No blocker"));
        assert!(!body.contains("cache key/restore-keys"));
    }

    #[test]
    fn pr_review_body_compiles_ffi_test_gap_as_decision_memo() {
        let mut global_box_refutation = test_observation(
            "ub-active-view",
            "Box::from(slice) can return None on allocation failure; refuted because allocation failure does not return None.",
            "false-premise",
            "refuted",
            "low",
            "high",
            BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY,
        );
        global_box_refutation.source = "model-false-premise-guard".to_owned();
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "tests-red-green".to_owned(),
                severity: "medium".to_owned(),
                confidence: "high".to_owned(),
                reason: "[tests-red-green] high high at test/js/bun/ffi/ffi.test.js:985: The no-finalizer `toBuffer(ptr(buffer))` subprocess tests assert process survival after GC, but they do not prove memory remains valid after collection/reuse. Attach the ASAN bad-free witness, or strengthen the subprocess test so it observes a real post-GC memory-validity condition rather than only `exitCode === 0`.".to_owned(),
                evidence: "lane transcript".to_owned(),
            }],
            &[
                test_observation(
                    "tests-oracle",
                    "The explicit-finalizer regression is useful guard coverage, but it is not the red/green proof for this bug. It checks that explicit ownership still works; the no-finalizer path is the actual fix surface.",
                    "parked-follow-up",
                    "parked",
                    "low",
                    "medium-high",
                    "explicit-finalizer-guard",
                ),
                global_box_refutation,
            ],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Decision"));
        assert!(body.contains("Needs one test-proof clarification before upstream."));
        assert!(body.contains("## Verification questions"));
        assert!(body.contains("Confirm the no-finalizer `toBuffer(ptr(buffer))` subprocess tests"));
        assert!(body.contains("## Parked follow-ups"));
        assert!(body.contains("explicit-finalizer regression is useful guard coverage"));
        assert!(!body.contains("Shared context"));
        assert!(!body.contains("Profile:"));
        assert!(!body.contains("Changed files:"));
        assert!(!body.contains("Inline comments:"));
        assert!(!body.contains("[tests-red-green]"));
        assert!(!body.contains("high high at"));
        assert!(!body.contains("test/js/bun/ffi/ffi.test.js:985"));
        assert!(!body.contains("lane transcript"));
        assert!(!body.contains("A human should still inspect"));
        assert!(!body.contains("## Residual risk"));
        assert!(!body.contains("## Refuted"));
        assert!(!body.contains("Box::from(slice)"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_renders_discriminating_proof_receipt_once() {
        let mut receipt = test_red_green_proof_receipt("discriminating", "failed");
        receipt.commands[1].exit_code = Some(132);
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[] as &[Observation],
            &[receipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Test proof"));
        assert!(!body.contains("## Decision"));
        assert!(!body.contains("No blocking UB finding from this pass."));
        assert!(body.contains("Focused red/green proof discriminates the patch"));
        assert!(body.contains("HEAD passed (exit 0) and base+tests failed (exit 132)"));
        assert!(!body.contains("Needs reviewer attention"));
        assert!(!body.contains("## Residual risk"));
        assert!(!body.contains("stdout.txt"));
        assert!(!body.contains("stderr.txt"));
        assert!(!body.contains("## Model lanes"));
        assert!(!body.contains("A human should still inspect"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_uses_proof_receipt_instead_of_duplicate_test_witness_question() {
        let receipt = test_red_green_proof_receipt("discriminating", "failed");
        let observations = vec![
            test_observation(
                "tests-oracle",
                "The new test needs a witnessed old-main red run.",
                "missing-evidence",
                "open",
                "medium",
                "high",
                "markdown-red-green-witness",
            ),
            test_observation(
                "source-route",
                "The red/green proof does not prove FileHandle.write reaches the patched scalar-write path; confirm the route before relying on the test.",
                "verification-question",
                "open",
                "medium",
                "medium-high",
                "filehandle-write-route",
            ),
        ];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "tests-red-green".to_owned(),
                severity: "medium".to_owned(),
                confidence: "high".to_owned(),
                reason: "The new test needs a witnessed old-main red run.".to_owned(),
                evidence: "duplicate lane summary".to_owned(),
            }],
            &observations,
            &[receipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Test proof"));
        assert!(body.contains("Focused red/green proof discriminates the patch"));
        assert!(body.contains("## Verification questions"));
        assert!(body.contains(
            "Confirm the red/green proof does not prove FileHandle.write reaches the patched scalar-write path"
        ));
        assert!(body.contains("Needs one verification check before upstream."));
        assert!(!body.contains("Needs one test-proof clarification before upstream."));
        assert!(!body.contains("witnessed old-main red run"));
        assert!(!body.contains("duplicate lane summary"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_uses_missing_proof_receipt_instead_of_duplicate_test_witness_question() {
        let mut receipt = test_red_green_proof_receipt("timed_out", "timed_out");
        receipt.commands[1].timed_out = true;
        receipt.request_ids = vec!["markdown-red-green-witness".to_owned()];
        let mut unrelated_receipt = test_red_green_proof_receipt("timed_out", "timed_out");
        unrelated_receipt.id = "proof-unrelated".to_owned();
        unrelated_receipt.requested_by = vec!["architecture".to_owned()];
        unrelated_receipt.request_ids = vec!["architecture-unrelated".to_owned()];
        unrelated_receipt.commands[0].command = "cargo test unrelated-question".to_owned();
        let observations = vec![test_observation(
            "tests-oracle",
            "Changed parser route remains unevaluated.",
            "missing-evidence",
            "open",
            "medium",
            "high",
            "markdown-red-green-witness",
        )];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "tests-red-green".to_owned(),
                severity: "medium".to_owned(),
                confidence: "high".to_owned(),
                reason: "The new test needs a witnessed old-main red run.".to_owned(),
                evidence: "duplicate lane summary".to_owned(),
            }],
            &observations,
            &[receipt, unrelated_receipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Evidence gaps"));
        assert!(body.contains("Focused proof timed out"));
        assert!(!body.contains("unrelated-question"));
        assert!(!body.contains("## Verification questions"));
        assert!(!body.contains("Needs one test-proof clarification before upstream."));
        assert!(!body.contains("witnessed old-main red run"));
        assert!(!body.contains("duplicate lane summary"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_dedupes_duplicate_summary_findings() {
        let duplicate_reason = "The bare toThrow assertion is non-discriminating for the changed FFI offset path; strengthen it with the expected TypeError message.";
        let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "tests-oracle".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: duplicate_reason.to_owned(),
                evidence: "oracle lane transcript".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "tests-red-green".to_owned(),
                severity: "medium".to_owned(),
                confidence: "high".to_owned(),
                reason: duplicate_reason.to_owned(),
                evidence: "red/green lane transcript".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "sibling-paths".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "Parked follow-up: sweep sibling FFI offset entry points for the same expect-to-error conversion.".to_owned(),
                evidence: "sibling lane transcript".to_owned(),
            },
        ];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &summary_only_findings,
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert_eq!(body.matches("bare toThrow assertion").count(), 1);
        assert!(body.contains("## Confirmed findings"));
        assert!(body.contains("## Parked follow-ups"));
        assert!(body.contains("sweep sibling FFI offset entry points"));
        assert!(!body.contains("oracle lane transcript"));
        assert!(!body.contains("red/green lane transcript"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_keeps_red_green_question_after_head_only_proof_receipt() {
        let receipt = test_proof_receipt("head_passed", "passed");
        let observations = vec![test_observation(
            "tests-oracle",
            "The new test still needs a base+tests red/green witness.",
            "verification-question",
            "open",
            "medium",
            "high",
            "markdown-red-green-witness",
        )];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "tests-red-green".to_owned(),
                severity: "medium".to_owned(),
                confidence: "high".to_owned(),
                reason: "The new test still needs a base+tests red/green witness.".to_owned(),
                evidence: "head-only proof is not a base+tests witness".to_owned(),
            }],
            &observations,
            &[receipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Test proof"));
        assert!(body.contains("## Verification questions"));
        assert!(body.contains("Confirm the new test still needs a base+tests red/green witness."));
        assert!(body.contains("Needs one test-proof clarification before upstream."));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_drops_structurally_answered_red_green_question() {
        let mut diff = test_diff();
        diff.patch = "\
diff --git a/src/ffi.rs b/src/ffi.rs
index 1111111..2222222 100644
--- a/src/ffi.rs
+++ b/src/ffi.rs
@@ -1,3 +1,3 @@
-let offset = usize::try_from(raw_offset).expect(\"offset must fit\");
+let offset = usize::try_from(raw_offset).map_err(|_| TypeError::new(\"offset must fit\"))?;
"
        .to_owned();
        let summary_only_findings = vec![SummaryOnlyFinding {
            lane: "tests-red-green".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            reason:
                "The added regression still needs base+tests red/green proof; old code was not run."
                    .to_owned(),
            evidence: "lane transcript".to_owned(),
        }];

        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &diff,
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &summary_only_findings,
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.is_empty(), "{body}");
    }

    #[test]
    fn structural_red_green_does_not_answer_source_route_question() {
        let mut diff = test_diff();
        diff.patch = "\
diff --git a/src/ffi.rs b/src/ffi.rs
index 1111111..2222222 100644
--- a/src/ffi.rs
+++ b/src/ffi.rs
@@ -1,3 +1,3 @@
-let offset = usize::try_from(raw_offset).expect(\"offset must fit\");
+let offset = usize::try_from(raw_offset).map_err(|_| TypeError::new(\"offset must fit\"))?;
"
        .to_owned();
        let summary_only_findings = vec![SummaryOnlyFinding {
            lane: "source-route".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            reason: "Confirm the base+tests red/green proof reaches the patched FFI offset route."
                .to_owned(),
            evidence: "route lane transcript".to_owned(),
        }];

        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &diff,
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &summary_only_findings,
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Verification questions"));
        assert!(body.contains("Confirm the base+tests red/green proof reaches"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_renders_non_discriminating_proof_as_evidence_gap() {
        let mut receipt = test_red_green_proof_receipt("non_discriminating", "passed");
        receipt.request_ids = vec!["markdown-red-green-witness".to_owned()];
        let observations = vec![test_observation(
            "tests-oracle",
            "Changed parser route remains unevaluated.",
            "missing-evidence",
            "open",
            "medium",
            "high",
            "markdown-red-green-witness",
        )];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &observations,
            &[receipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(!body.contains("## Decision"));
        assert!(!body.contains("No blocking UB finding from this pass."));
        assert!(body.contains("## Evidence gaps"));
        assert!(!body.contains("## Residual risk"));
        assert!(body.contains("HEAD passed (exit 0) and base+tests passed (exit 0)"));
        assert!(!body.contains("## Test proof"));
        assert!(!body.contains("Needs one residual-risk check"));
        assert!(!body.contains("A human should still inspect"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_renders_head_failed_proof_exit_code() {
        let mut receipt = test_proof_receipt("head_failed", "failed");
        receipt.commands[0].exit_code = Some(132);
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &[] as &[Observation],
            &[receipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Decision"));
        assert!(body.contains("Needs focused proof failure resolved"));
        assert!(body.contains("## Test proof"));
        assert!(body.contains("Focused HEAD proof failed (exit 132)"));
        assert!(!body.contains("stdout.txt"));
        assert!(!body.contains("stderr.txt"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_collapses_timed_out_proof_to_missing_evidence() {
        let mut receipt = test_proof_receipt("timed_out", "timed_out");
        receipt.request_ids = vec!["markdown-red-green-witness".to_owned()];
        let observations = vec![test_observation(
            "tests-oracle",
            "Changed parser route remains unevaluated.",
            "missing-evidence",
            "open",
            "medium",
            "high",
            "markdown-red-green-witness",
        )];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &observations,
            &[receipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(!body.contains("## Decision"));
        assert!(!body.contains("No blocking UB finding from this pass."));
        assert!(body.contains("## Evidence gaps"));
        assert!(body.contains("Focused proof timed out"));
        assert!(!body.contains("## Test proof"));
        assert!(!body.contains("## Residual risk"));
        assert!(!body.contains("Needs one missing evidence item resolved before upstream."));
        assert!(!body.contains("A human should still inspect"));
        assert!(!body.contains("stdout.txt"));
        assert!(!body.contains("stderr.txt"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn model_off_empty_smoke_writes_skip_receipt_instead_of_review_payload() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let config = Config::default();
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let mut args = test_run_args(out.clone());
        args.run_pass = super::RunPass::Manual;
        args.model_mode = ModelMode::Off;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let run_started = Instant::now();
        let mut run_loop_tracker = super::RunLoopTracker::new();

        write_review_artifacts(
            temp.path(),
            &out,
            &config,
            &diff,
            &test_box_state(),
            &plan,
            "running summary",
            test_pr_thread_context(),
            &args,
            &event_log,
            &run_started,
            &mut run_loop_tracker,
            std::time::Duration::from_secs(73),
            None,
        )?;

        let artifact_body = fs::read_to_string(out.join("review/review.md"))?;
        let metrics: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
        let skip: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/github-review-skip.json"))?)?;
        let summary = render_summary(&out, &plan, &diff)?;

        assert!(!out.join("review/github-review.json").exists());
        assert!(artifact_body.contains("## Model lanes"));
        assert!(artifact_body.contains("Lane: `ub-memory-lifetime`"));
        assert_eq!(skip["status"], "skipped");
        assert_eq!(skip["review_payload_status"], "skipped_empty_smoke");
        assert_eq!(skip["terminal_state"], "artifact-only");
        assert!(skip["github_review_json"].is_null());
        assert_eq!(skip["run_pass"], "manual");
        assert_eq!(metrics["wall_clock_seconds"], 73);
        assert_eq!(metrics["wall_clock_ms"], 73_000);
        assert_eq!(metrics["terminal_state"], "artifact-only");
        assert_eq!(metrics["review_payload_status"], "skipped_empty_smoke");
        assert_eq!(metrics["post_status"], "not_attempted_by_run");
        assert_eq!(metrics["github_review_body_bytes"], 0);
        assert_eq!(metrics["github_review_comments"], 0);
        assert_eq!(metrics["artifact_review_body_bytes"], artifact_body.len());
        assert!(summary.contains("## Review efficiency"));
        assert!(summary.contains("Runtime: `1m13s`"));
        assert!(summary.contains("Terminal state: `artifact-only`"));
        assert!(summary.contains("Follow-up results:"));
        assert!(summary.contains("attempted"));
        assert!(
            summary.contains("Review payload: `skipped_empty_smoke`; post: `not_attempted_by_run`")
        );
        assert!(!has_standalone_approval_line(&artifact_body));
        Ok(())
    }

    #[test]
    fn failed_gate_without_post_reports_gate_failure_not_empty_smoke() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let config = Config::default();
        let mut required_ripr = sensor_plan("ripr", "ripr", true);
        required_ripr.required = true;
        let plan = test_plan(vec![required_ripr]);
        let diff = test_diff();
        let mut args = test_run_args(out.clone());
        args.run_pass = super::RunPass::Manual;
        args.model_mode = ModelMode::Off;
        args.mode = RunMode::IntelligentCi;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let run_started = Instant::now();
        let mut run_loop_tracker = super::RunLoopTracker::new();

        let gate_outcome = write_review_artifacts(
            temp.path(),
            &out,
            &config,
            &diff,
            &test_box_state(),
            &plan,
            "running summary",
            test_pr_thread_context(),
            &args,
            &event_log,
            &run_started,
            &mut run_loop_tracker,
            std::time::Duration::from_secs(5),
            None,
        )?;

        assert_eq!(gate_outcome.conclusion, "inconclusive");
        assert!(!out.join("review/github-review.json").exists());
        let skip: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/github-review-skip.json"))?)?;
        let metrics: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/metrics.json"))?)?;
        let terminal: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/terminal_state.json"))?)?;
        for (label, value) in [
            ("skip receipt", &skip["review_payload_status"]),
            ("metrics", &metrics["review_payload_status"]),
            ("terminal state", &terminal["review_payload_status"]),
        ] {
            assert_eq!(
                value, "skipped_gate_failure_artifact_only",
                "{label} must name the gate failure"
            );
        }
        let reason = skip["reason"].as_str().unwrap_or_default();
        assert!(
            reason.contains("gate") && reason.contains("review/gate_outcome.json"),
            "skip reason must name the gate and receipt: {reason}"
        );
        let summary = render_summary(&out, &plan, &diff)?;
        assert!(
            summary.contains(
                "Review payload: `skipped_gate_failure_artifact_only`; \
                 post: `not_attempted_by_run`"
            ),
            "running summary must carry the gate-failure status"
        );
        assert!(
            summary.contains("Gate:")
                && (summary.contains("inconclusive") || summary.contains("fail")),
            "running summary must carry the gate status"
        );
        Ok(())
    }

    #[test]
    fn pass_excluded_by_post_review_on_writes_truthful_skip_receipt() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        // Config::default keeps the two-pass posture: synchronize is not in
        // [gate].post_review_on, so a posting=review synchronize pass must
        // skip with a receipt that names the pass policy.
        let config = Config::default();
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let mut args = test_run_args(out.clone());
        args.run_pass = super::RunPass::Synchronize;
        args.posting = PostingMode::Review;
        args.model_mode = ModelMode::Off;
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let run_started = Instant::now();
        let mut run_loop_tracker = super::RunLoopTracker::new();

        write_review_artifacts(
            temp.path(),
            &out,
            &config,
            &diff,
            &test_box_state(),
            &plan,
            "running summary",
            test_pr_thread_context(),
            &args,
            &event_log,
            &run_started,
            &mut run_loop_tracker,
            std::time::Duration::from_secs(5),
            None,
        )?;

        assert!(!out.join("review/github-review.json").exists());
        let skip: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/github-review-skip.json"))?)?;
        assert_eq!(skip["status"], "skipped");
        assert_eq!(skip["review_payload_status"], "skipped_pass_policy");
        assert_eq!(skip["run_pass"], "synchronize");
        let cost: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/ub-review-cost.json"))?)?;
        assert_eq!(cost["schema"], "ub-review.cost_receipt.v1");
        assert!(cost.get("suggested_fill_seconds").is_none());
        let fill_ledger: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("review/fill-ledger.json"))?)?;
        assert_eq!(fill_ledger["schema"], "ub-review.fill_ledger.v1");
        assert_eq!(fill_ledger["catalog_scope"], "executed_work_queue_v1");
        let reason = skip["reason"].as_str().unwrap_or_default();
        assert!(
            reason.contains("pass `synchronize` is not in [gate].post_review_on"),
            "skip reason should name the pass policy: {reason}"
        );
        assert!(
            !reason.contains("a grouped PR review was prepared"),
            "skip reason must not claim a review was prepared: {reason}"
        );
        Ok(())
    }

    #[test]
    fn suppressed_body_skip_receipt_names_policy_and_counts() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let mut terminal_state = test_terminal_state("needs-reviewer-attention");
        terminal_state.review_payload_status = "skipped_artifact_only_body".to_owned();
        terminal_state.summary_only_findings = 10;
        terminal_state.substantive_summary_only_findings = 0;
        let review = super::ReviewArtifacts {
            shared_context_id: "abc123".to_owned(),
            review_profile: DEFAULT_REVIEW_PROFILE.to_owned(),
            mode: "review-byok".to_owned(),
            posting: "review".to_owned(),
            runtime_profile: "gh-runner".to_owned(),
            run_pass: "opened".to_owned(),
            model_mode: "auto".to_owned(),
            depth: "standard".to_owned(),
            provider_policy: "minimax-only".to_owned(),
            model_provider_policy: "minimax-only".to_owned(),
            lane_width: 10,
            model_concurrency: 8,
            max_model_calls: 18,
            max_inline_comments: 8,
            model_timeout_sec: 300,
            ledger_path: String::new(),
            ledger_max_bytes: 65_536,
            pr_thread_context: test_pr_thread_context(),
            terminal_state,
            provider_preflights: Vec::new(),
            model_lanes: vec![model_lane_receipt("opposition", "ok")],
            missing_or_failed_sensor_evidence: Vec::new(),
            missing_or_failed_model_evidence: Vec::new(),
            inline_comments: Vec::new(),
            summary_only_findings: Vec::new(),
            observations: Vec::new(),
            proof_requests: Vec::new(),
            proof_intents: Vec::new(),
            proof_receipts: Vec::new(),
            resource_leases: Vec::new(),
            body: "artifact body".to_owned(),
        };

        let receipt = super::build_github_review_skip_receipt(
            &args,
            &review,
            SummaryOnlyBodyPolicy::PostSubstantive,
        );
        assert_eq!(receipt.review_payload_status, "skipped_artifact_only_body");
        assert_eq!(
            receipt.reason,
            "summary_only_body = `post_substantive` withheld the PR-facing body as no-value boilerplate: 10 summary-only findings, 0 substantive; diagnostics remain in artifacts."
        );

        let mut suppress_review = review;
        suppress_review.terminal_state.summary_only_findings = 3;
        suppress_review
            .terminal_state
            .substantive_summary_only_findings = 2;
        let suppress_receipt = super::build_github_review_skip_receipt(
            &args,
            &suppress_review,
            SummaryOnlyBodyPolicy::Suppress,
        );
        assert_eq!(
            suppress_receipt.reason,
            "summary_only_body = `suppress` withheld the PR-facing body as no-value boilerplate: 3 summary-only findings, 2 substantive; diagnostics remain in artifacts."
        );
    }

    #[test]
    fn review_metrics_count_efficiency_facts() {
        let mut cached_lane = model_lane_receipt("tests-oracle", "ok");
        cached_lane.endpoint_kind = "anthropic-messages".to_owned();
        cached_lane.cache_usage = super::ModelCacheUsage {
            input_tokens: Some(1_000),
            output_tokens: Some(100),
            cache_creation_input_tokens: Some(800),
            cache_read_input_tokens: Some(400),
        };
        let review = super::ReviewArtifacts {
            shared_context_id: "abc123".to_owned(),
            review_profile: DEFAULT_REVIEW_PROFILE.to_owned(),
            mode: "review-byok".to_owned(),
            posting: "review".to_owned(),
            runtime_profile: "gh-runner".to_owned(),
            run_pass: "opened".to_owned(),
            model_mode: "auto".to_owned(),
            depth: "standard".to_owned(),
            provider_policy: "minimax-only".to_owned(),
            model_provider_policy: "minimax-only".to_owned(),
            lane_width: 10,
            model_concurrency: 8,
            max_model_calls: 18,
            max_inline_comments: 8,
            model_timeout_sec: 300,
            ledger_path: String::new(),
            ledger_max_bytes: 65_536,
            pr_thread_context: test_pr_thread_context(),
            terminal_state: test_terminal_state("needs-reviewer-attention"),
            provider_preflights: vec![
                super::ProviderPreflightReceipt {
                    provider: "minimax".to_owned(),
                    model: "MiniMax-M3".to_owned(),
                    endpoint_kind: "anthropic-messages".to_owned(),
                    status: "ok".to_owned(),
                    reason: "preflight ok".to_owned(),
                    duration_ms: Some(100),
                    http_status: Some(200),
                    response_shape: Some("anthropic".to_owned()),
                    cache_usage: super::ModelCacheUsage {
                        input_tokens: Some(900),
                        output_tokens: Some(30),
                        cache_creation_input_tokens: Some(200),
                        cache_read_input_tokens: Some(50),
                    },
                },
                super::ProviderPreflightReceipt {
                    provider: "opencode-go".to_owned(),
                    model: "minimax-m3".to_owned(),
                    endpoint_kind: "anthropic-messages".to_owned(),
                    status: "missing_key".to_owned(),
                    reason: "optional provider unavailable".to_owned(),
                    duration_ms: None,
                    http_status: None,
                    response_shape: None,
                    cache_usage: super::ModelCacheUsage::default(),
                },
            ],
            model_lanes: vec![cached_lane],
            missing_or_failed_sensor_evidence: Vec::new(),
            missing_or_failed_model_evidence: Vec::new(),
            inline_comments: Vec::new(),
            summary_only_findings: vec![SummaryOnlyFinding {
                lane: "tests-oracle".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "inline guard rejected src/lib.rs:99; severity_allowed=true confidence_allowed=true line_valid=false concise=true body_present=true evidence_present=true repo_relative=true".to_owned(),
                evidence: "line map receipt".to_owned(),
            }],
            observations: Vec::new(),
            proof_requests: Vec::new(),
            proof_intents: Vec::new(),
            proof_receipts: Vec::new(),
            resource_leases: Vec::new(),
            body: "artifact body".to_owned(),
        };
        let github_review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "pr body".to_owned(),
            comments: Vec::new(),
        };

        let diff = test_diff();
        let plan = test_plan(Vec::new());
        let follow_up_results = vec![
            test_follow_up_result("follow-up-a", "group-a", "ok"),
            test_follow_up_result("follow-up-b", "group-b", "skipped_budget"),
        ];
        let metrics = build_review_metrics(ReviewMetricsInput {
            out: Path::new("target/ub-review-test"),
            diff: &diff,
            plan: &plan,
            review: &review,
            github_review: Some(&github_review),
            review_payload_status: "prepared",
            observations_count: 0,
            follow_up_results: &follow_up_results,
            final_follow_up_tasks: 1,
            run: test_run_loop_metrics(),
            elapsed: std::time::Duration::from_secs(601),
            args: &test_run_args(Path::new("target/ub-review").to_path_buf()),
        });

        assert_eq!(metrics.wall_clock_seconds, 601);
        assert_eq!(metrics.wall_clock_ms, 601_000);
        assert_eq!(metrics.run.model_wall_ms, 300);
        assert_eq!(metrics.run.local_proof_wall_ms, 80);
        assert_eq!(metrics.run.compiler_wall_ms, 40);
        assert_eq!(metrics.run.model_call_duration_ms_sum, 100);
        assert_eq!(metrics.run.proof_command_duration_ms_sum, 0);
        assert_eq!(metrics.run.model_proof_overlap_ms, 0);
        assert!(metrics.run.local_proof_wall_excludes_model_wait);
        assert_eq!(metrics.off_diff_candidates_rejected, 1);
        assert_eq!(metrics.provider_evidence_failures, 1);
        assert_eq!(metrics.review_payload_status, "prepared");
        assert_eq!(metrics.post_status, "not_attempted_by_run");
        assert_eq!(metrics.run_pass, "opened");
        assert_eq!(metrics.terminal_state, "needs-reviewer-attention");
        assert_eq!(metrics.observations, 0);
        assert_eq!(metrics.follow_up_results.total, 2);
        assert_eq!(metrics.follow_up_results.status_counts["ok"], 1);
        assert_eq!(metrics.follow_up_results.status_counts["skipped_budget"], 1);
        assert_eq!(metrics.follow_up_results.calls_attempted, 1);
        assert_eq!(metrics.final_follow_up_tasks, 1);
        assert_eq!(metrics.models.prompt_cache_creation_input_tokens, 1_000);
        assert_eq!(metrics.models.prompt_cache_read_input_tokens, 450);
        assert_eq!(metrics.models.prompt_cache_lane_hits, 1);
        assert_eq!(metrics.models.prompt_cache_lane_misses, 0);
        assert_eq!(metrics.models.prompt_cache_lane_unknown, 0);
        assert_eq!(metrics.proof_requests, 0);
        assert_eq!(metrics.proof_receipts, 0);
        assert_eq!(metrics.resource_leases, 0);
        assert_eq!(metrics.github_review_body_bytes, "pr body".len());
        assert_eq!(metrics.artifact_review_body_bytes, "artifact body".len());
    }

    #[test]
    fn review_metrics_separate_prepared_output_and_receipt_freshness() {
        let request = |id: &str, status: &str| ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: id.to_owned(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            command: "cargo test --locked focused_case".to_owned(),
            reason: "focused proof fixture".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 60,
            required: false,
            status: status.to_owned(),
        };
        let mut review = test_review_artifacts();
        review.proof_requests = vec![
            request("proof-executed", "executed"),
            request("proof-deduplicated", "deduplicated"),
            request("proof-requested", "requested"),
        ];

        let mut current = test_proof_receipt("passed", "passed");
        current.id = "proof-current".to_owned();
        current.request_ids = vec!["proof-executed".to_owned()];
        let mut stale = test_proof_receipt("passed", "passed");
        stale.id = "proof-stale".to_owned();
        stale.head = "older-head".to_owned();
        stale.request_ids.clear();
        review.proof_receipts = vec![current, stale];

        let github_review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "Prepared synthesis".to_owned(),
            comments: vec![
                GitHubReviewComment {
                    path: "src/lib.rs".to_owned(),
                    line: 10,
                    side: "RIGHT".to_owned(),
                    body: "First finding".to_owned(),
                    suggestion: None,
                },
                GitHubReviewComment {
                    path: "src/lib.rs".to_owned(),
                    line: 20,
                    side: "RIGHT".to_owned(),
                    body: "Second finding".to_owned(),
                    suggestion: None,
                },
            ],
        };
        let diff = test_diff();
        let metrics = build_review_metrics(ReviewMetricsInput {
            out: Path::new("target/ub-review-metrics"),
            diff: &diff,
            plan: &test_plan(Vec::new()),
            review: &review,
            github_review: Some(&github_review),
            review_payload_status: "prepared",
            observations_count: 0,
            follow_up_results: &[],
            final_follow_up_tasks: 0,
            run: test_run_loop_metrics(),
            elapsed: std::time::Duration::from_secs(1),
            args: &test_run_args(PathBuf::from("target/ub-review-metrics")),
        });

        assert_eq!(metrics.github_review_comments, 2);
        assert_eq!(metrics.prepared_inline_comments, 2);
        assert!(metrics.prepared_review_body);
        assert_eq!(metrics.proof_requests, 3);
        assert_eq!(metrics.proof_requests_terminal, 2);
        assert_eq!(metrics.proof_request_terminal_rate, Some(2.0 / 3.0));
        assert_eq!(metrics.proof_request_status_counts["deduplicated"], 1);
        assert_eq!(metrics.proof_receipts, 2);
        assert_eq!(metrics.proof_receipts_current_head, 1);
        assert_eq!(metrics.proof_receipts_stale_head, 1);
        assert_eq!(metrics.proof_receipts_with_request_links, 1);
        assert_eq!(metrics.proof_changed_conclusions, 0);
        assert_eq!(metrics.post_status, "not_attempted_by_run");
    }

    #[test]
    fn cost_receipt_records_measured_v1_inputs() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("policy"))?;
        fs::write(
            temp.path().join("policy/ci-budget.toml"),
            "[budget]\nlinux_minute_rate_usd = 0.008\n",
        )?;
        let unsafe_review_dir = temp
            .path()
            .join("sensors/unsafe-review/unsafe-review-output");
        fs::create_dir_all(&unsafe_review_dir)?;
        fs::write(
            unsafe_review_dir.join("unsafe-review-gate.json"),
            r#"{
                "schema_version": "unsafe-review-gate/v1",
                "status": "advisory",
                "required_floor_wall_seconds": 45.25
            }"#,
        )?;

        let mut review = test_review_artifacts();
        review.provider_preflights = vec![super::ProviderPreflightReceipt {
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "preflight ok".to_owned(),
            duration_ms: Some(1_000),
            http_status: Some(200),
            response_shape: Some("anthropic".to_owned()),
            cache_usage: super::ModelCacheUsage {
                input_tokens: Some(1_000),
                output_tokens: Some(50),
                cache_creation_input_tokens: Some(700),
                cache_read_input_tokens: Some(200),
            },
        }];
        let mut cached_lane = model_lane_receipt("tests-oracle", "ok");
        cached_lane.endpoint_kind = "anthropic-messages".to_owned();
        cached_lane.duration_ms = Some(2_000);
        cached_lane.cache_usage = super::ModelCacheUsage {
            input_tokens: Some(500),
            output_tokens: Some(20),
            cache_creation_input_tokens: Some(300),
            cache_read_input_tokens: Some(100),
        };
        review.model_lanes = vec![cached_lane];
        let mut follow_up = test_follow_up_result("follow-up-a", "group-a", "ok");
        follow_up.duration_ms = Some(3_000);
        follow_up.cache_usage = super::ModelCacheUsage {
            input_tokens: Some(250),
            output_tokens: Some(10),
            cache_creation_input_tokens: None,
            cache_read_input_tokens: None,
        };
        let follow_up_results = vec![follow_up];

        let mut run = test_run_loop_metrics();
        run.elapsed_wall_ms = 120_000;
        let metrics = build_review_metrics(ReviewMetricsInput {
            out: temp.path(),
            diff: &test_diff(),
            plan: &test_plan(Vec::new()),
            review: &review,
            github_review: None,
            review_payload_status: "prepared",
            observations_count: 0,
            follow_up_results: &follow_up_results,
            final_follow_up_tasks: 1,
            run,
            elapsed: std::time::Duration::from_secs(120),
            args: &test_run_args(temp.path().join("out")),
        });
        let mut config = Config::default();
        config.gate.target_minutes = 25;
        config.gate.hard_timeout_minutes = 50;

        let receipt = build_cost_receipt(
            temp.path(),
            temp.path(),
            &config,
            &metrics,
            &review,
            &follow_up_results,
        );

        assert_eq!(receipt.schema, super::COST_RECEIPT_SCHEMA);
        assert_eq!(receipt.target_minutes, 25);
        assert_eq!(receipt.cap_minutes, 50);
        assert_eq!(receipt.required_floor_wall_seconds, Some(45.25));
        assert_eq!(receipt.llm_seconds, 6.0);
        assert_eq!(receipt.cost_basis.runner_minutes, 2.0);
        assert_eq!(receipt.cost_basis.linux_minute_rate_usd, Some(0.008));
        assert_eq!(receipt.estimated_cost_usd, Some(0.016));
        assert_eq!(receipt.cache.model_prefix, "hit");
        assert_eq!(receipt.tokens.fresh_input, 1_450);
        assert_eq!(receipt.tokens.cached_input, 300);
        assert_eq!(receipt.tokens.output, 80);
        assert!(
            !receipt
                .missing
                .iter()
                .any(|missing| missing.field == "required_floor_wall_seconds")
        );
        assert!(
            receipt
                .missing
                .iter()
                .any(|missing| missing.field == "cache.cargo")
        );
        Ok(())
    }

    #[test]
    fn floor_trend_records_single_run_without_history_overclaim() {
        let cost = super::CostReceipt {
            schema: super::COST_RECEIPT_SCHEMA,
            run_id: "local-abc123".to_owned(),
            runner_kind: "local".to_owned(),
            target_minutes: 30,
            cap_minutes: 60,
            fallback_used: true,
            required_floor_wall_seconds: Some(1_000.0),
            llm_seconds: 6.0,
            cache: super::CostCacheReceipt {
                cargo: "miss".to_owned(),
                model_prefix: "hit".to_owned(),
            },
            tokens: super::CostTokenReceipt {
                fresh_input: 1,
                cached_input: 2,
                output: 3,
            },
            estimated_cost_usd: Some(0.016),
            cost_basis: super::CostBasisReceipt {
                runner_minutes: 2.0,
                linux_minute_rate_usd: Some(0.008),
                token_pricing: "excluded_v1".to_owned(),
            },
            source_artifacts: Vec::new(),
            missing: Vec::new(),
        };

        let trend = super::build_floor_trend_artifact(&cost);
        let release = &trend.releases[0];
        let missing_fields = trend
            .missing
            .iter()
            .map(|entry| entry.field.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(trend.schema, super::FLOOR_TREND_SCHEMA);
        assert_eq!(trend.run_id, cost.run_id);
        assert_eq!(trend.window_scope, "single_run_v1");
        assert_eq!(trend.window_runs, 1);
        assert_eq!(release.sample_runs, 1);
        assert_eq!(release.floor_wall_seconds_p50, Some(1_000.0));
        assert_eq!(release.floor_wall_seconds_p95, Some(1_000.0));
        assert_eq!(release.cargo_cache_hit_rate, Some(0.0));
        assert_eq!(release.model_prefix_cache_hit_rate, Some(1.0));
        assert_eq!(release.fallback_used_rate, 1.0);
        assert_eq!(release.avg_cost_usd, Some(0.016));
        assert_eq!(trend.trend.floor_creep_detected, None);
        assert_eq!(trend.trend.floor_budget_pressure_detected, Some(true));
        assert_eq!(trend.trend.cache_hit_rate_delta, None);
        assert_eq!(trend.trend.avg_cost_delta_usd, None);
        assert!(missing_fields.contains("trend.floor_creep_detected"));
        assert!(missing_fields.contains("trend.cache_hit_rate_delta"));
        assert!(missing_fields.contains("trend.avg_cost_delta_usd"));
        assert!(!missing_fields.contains("releases[].floor_wall_seconds_p50"));
    }

    #[test]
    fn cost_receipt_records_missing_inputs_without_suggested_fill() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review = test_review_artifacts();
        let metrics = build_review_metrics(ReviewMetricsInput {
            out: temp.path(),
            diff: &test_diff(),
            plan: &test_plan(Vec::new()),
            review: &review,
            github_review: None,
            review_payload_status: "prepared",
            observations_count: 0,
            follow_up_results: &[],
            final_follow_up_tasks: 0,
            run: test_run_loop_metrics(),
            elapsed: std::time::Duration::from_secs(1),
            args: &test_run_args(temp.path().join("out")),
        });
        let config = Config::default();

        let receipt = build_cost_receipt(temp.path(), temp.path(), &config, &metrics, &review, &[]);
        let missing_fields = receipt
            .missing
            .iter()
            .map(|missing| missing.field.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(receipt.required_floor_wall_seconds, None);
        assert_eq!(receipt.estimated_cost_usd, None);
        assert!(missing_fields.contains("required_floor_wall_seconds"));
        assert!(missing_fields.contains("cost_basis.linux_minute_rate_usd"));
        assert!(missing_fields.contains("estimated_cost_usd"));
        assert!(missing_fields.contains("cache.cargo"));

        let serialized = serde_json::to_value(&receipt)?;
        assert!(serialized.get("suggested_fill_seconds").is_none());
        Ok(())
    }

    #[test]
    fn quality_receipt_records_run_completion_inputs_without_reviewer_overclaim() {
        let mut review = test_review_artifacts();
        review.provider_preflights = vec![super::ProviderPreflightReceipt {
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "missing_key".to_owned(),
            reason: "provider unavailable".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            cache_usage: super::ModelCacheUsage::default(),
        }];
        let mut fallback_lane = model_lane_receipt("tests-oracle", "ok");
        fallback_lane.fallback_from = Some("minimax/MiniMax-M3".to_owned());
        review.model_lanes = vec![fallback_lane];
        review.summary_only_findings = vec![SummaryOnlyFinding {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            reason: "inline guard rejected src/lib.rs:99; severity_allowed=true confidence_allowed=true line_valid=false concise=true body_present=true evidence_present=true repo_relative=true".to_owned(),
            evidence: "line map receipt".to_owned(),
        }];
        let github_review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Findings\n\n- Focused finding.".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 12,
                side: "RIGHT".to_owned(),
                body: "Focused finding.".to_owned(),
                suggestion: None,
            }],
        };
        let metrics = build_review_metrics(ReviewMetricsInput {
            out: Path::new("target/ub-review-test"),
            diff: &test_diff(),
            plan: &test_plan(Vec::new()),
            review: &review,
            github_review: Some(&github_review),
            review_payload_status: "prepared",
            observations_count: 0,
            follow_up_results: &[],
            final_follow_up_tasks: 0,
            run: test_run_loop_metrics(),
            elapsed: std::time::Duration::from_secs(1),
            args: &test_run_args(Path::new("target/ub-review").to_path_buf()),
        });
        let fill_ledger = super::FillLedger {
            schema: super::FILL_LEDGER_SCHEMA,
            run_id: super::cost_run_id(&metrics),
            catalog_scope: "executed_work_queue_v1",
            source_artifacts: Vec::new(),
            entries: vec![
                super::FillLedgerEntry {
                    check_id: "ripr".to_owned(),
                    kind: "sensor".to_owned(),
                    selected: true,
                    selection_reason: "Rust behavior changed".to_owned(),
                    cost: "static".to_owned(),
                    expected_signal: Some("static mutation-exposure signal".to_owned()),
                    actual_signal: Some("ok: ripr completed".to_owned()),
                    time_spent_sec: 0.1,
                    artifact_path: Some("sensors/ripr/ub-review-sensor-status.json".to_owned()),
                    affected_merge: Some(false),
                    source_artifacts: Vec::new(),
                },
                super::FillLedgerEntry {
                    check_id: "miri".to_owned(),
                    kind: "proof-skip".to_owned(),
                    selected: false,
                    selection_reason: "No unsafe/native risk.".to_owned(),
                    cost: "miri".to_owned(),
                    expected_signal: None,
                    actual_signal: None,
                    time_spent_sec: 0.0,
                    artifact_path: None,
                    affected_merge: None,
                    source_artifacts: Vec::new(),
                },
            ],
        };

        let receipt = build_quality_receipt(&metrics, &review, &fill_ledger);

        assert_eq!(receipt.schema, super::QUALITY_RECEIPT_SCHEMA);
        assert_eq!(receipt.run_id, fill_ledger.run_id);
        assert_eq!(receipt.review_payload_status, "prepared");
        assert_eq!(receipt.comments_prepared, 1);
        assert_eq!(receipt.comments_posted, None);
        assert_eq!(receipt.comments_accepted, None);
        assert_eq!(receipt.comments_resolved, None);
        assert_eq!(receipt.reviewer_overrides, None);
        assert_eq!(receipt.adopted_generated_tests, None);
        assert_eq!(receipt.comments_off_diff_rejected, 1);
        assert_eq!(receipt.fills_total, 1);
        assert_eq!(receipt.fills_with_signal, 1);
        assert_eq!(receipt.llm_unavailable_events, 2);
        assert_eq!(receipt.fallback_used_lanes, 1);
        assert!(
            receipt
                .missing
                .iter()
                .any(|missing| missing.field == "comments_posted")
        );
        assert!(
            receipt
                .source_artifacts
                .contains(&"review/github-review.json".to_owned())
        );
    }

    #[test]
    fn fill_ledger_skip_entries_record_heavy_witness_expected_signal() {
        let mutation = super::fill_proof_planner_skip_entry(super::ProofPlannerSkip {
            kind: "mutation".to_owned(),
            reason: "profile does not lease mutation proof".to_owned(),
        });
        let sanitizer = super::fill_proof_planner_skip_entry(super::ProofPlannerSkip {
            kind: "sanitizer".to_owned(),
            reason: "profile does not lease sanitizer proof".to_owned(),
        });

        assert_eq!(mutation.kind, "proof-skip");
        assert!(!mutation.selected);
        assert_eq!(mutation.cost, "mutation");
        assert_eq!(
            mutation.expected_signal.as_deref(),
            Some("runtime mutation check for targeted test oracle strength")
        );
        assert_eq!(
            sanitizer.expected_signal.as_deref(),
            Some("sanitizer runtime witness for memory-safety regressions")
        );
        assert_eq!(
            mutation.source_artifacts,
            vec!["review/proof_planner_output.json".to_owned()]
        );
    }

    #[test]
    fn fill_ledger_selected_focused_build_request_cites_matching_lease() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        fs::create_dir_all(out.join("review"))?;
        let diff = test_diff();
        let plan = test_plan(Vec::new());
        let request = ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "proof-build-policy-check".to_owned(),
            lane: "proof-planner".to_owned(),
            requested_by: vec!["proof-planner".to_owned()],
            command: "cargo xtask policy-check".to_owned(),
            reason: "Policy receipts should be checked before asking a reviewer.".to_owned(),
            cost: "focused-build".to_owned(),
            timeout_sec: 90,
            required: false,
            status: "requested".to_owned(),
        };
        let mut review = test_review_artifacts();
        review.proof_requests = vec![request.clone()];
        let mut receipt = test_proof_receipt("head_passed", "passed");
        receipt.id = super::focused_build_candidates_from_requests(&review.proof_requests)
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("focused-build request did not plan"))?
            .id;
        receipt.kind = "focused-build".to_owned();
        receipt.requested_by = request.requested_by.clone();
        receipt.request_ids = vec![request.id.clone()];
        receipt.commands[0].command = request.command.clone();
        receipt.commands[0].duration_ms = 1_234;
        let lease = ResourceLease {
            schema: super::RESOURCE_LEASE_SCHEMA.to_owned(),
            id: format!("lease-{}", receipt.id),
            kind: "focused-build".to_owned(),
            consumer: receipt.id.clone(),
            status: "granted".to_owned(),
            reason: "focused build proof lease granted by runtime profile".to_owned(),
            cpu: 1,
            memory_mb: 1_024,
            disk_mb: 512,
            timeout_sec: 90,
            network: false,
            scratch: true,
            worktree: None,
            command: Some(format!("head: {}", request.command)),
        };
        review.proof_receipts = vec![receipt.clone()];
        review.resource_leases = vec![lease.clone()];
        let metrics = build_review_metrics(ReviewMetricsInput {
            out: &out,
            diff: &diff,
            plan: &plan,
            review: &review,
            github_review: None,
            review_payload_status: "prepared",
            observations_count: 0,
            follow_up_results: &[],
            final_follow_up_tasks: 0,
            run: test_run_loop_metrics(),
            elapsed: std::time::Duration::from_secs(1),
            args: &test_run_args(out.clone()),
        });
        let gate_outcome = super::GateOutcome {
            schema: super::GATE_OUTCOME_SCHEMA.to_owned(),
            conclusion: "pass".to_owned(),
            terminal_status: "artifact-only".to_owned(),
            reasons: Vec::new(),
            required_proof: super::GateRequiredProofCounts::default(),
            tool_gates: super::GateToolGateCounts::default(),
            evidence_gaps_blocking: 0,
            evidence_gaps_advisory: 0,
        };

        let ledger = super::build_fill_ledger(super::FillLedgerInput {
            out: &out,
            diff: &diff,
            profile: &Profile::default(),
            plan: &plan,
            tool_gate_outcomes: &[],
            gate_outcome: &gate_outcome,
            review: &review,
            metrics: &metrics,
        })?;

        let entry = ledger
            .entries
            .iter()
            .find(|entry| {
                entry.kind == "proof-request" && entry.check_id == "proof-build-policy-check"
            })
            .ok_or_else(|| anyhow::anyhow!("missing focused-build fill-ledger entry"))?;
        assert!(entry.selected);
        assert_eq!(entry.cost, "focused-build");
        assert_eq!(
            entry.artifact_path.as_deref(),
            Some(format!("review/proof_receipts.json#{}", receipt.id).as_str())
        );
        assert_eq!(entry.time_spent_sec, 1.234);
        assert!(
            entry
                .source_artifacts
                .contains(&"review/proof_receipts.json".to_owned())
        );
        assert!(
            entry
                .source_artifacts
                .contains(&format!("review/resource_leases.json#{}", lease.id)),
            "focused-build fills must cite the exact broker lease anchor: {entry:#?}"
        );
        Ok(())
    }

    #[test]
    fn quality_trend_records_single_run_without_reviewer_or_history_overclaim() {
        let receipt = super::QualityReceipt {
            schema: super::QUALITY_RECEIPT_SCHEMA,
            run_id: "local-abc123".to_owned(),
            source_artifacts: vec![
                "review/metrics.json".to_owned(),
                "review/fill-ledger.json".to_owned(),
                "review/provider-preflight-status.json".to_owned(),
                "review/review.json".to_owned(),
                "review/github-review-skip.json".to_owned(),
            ],
            review_payload_status: "skipped_empty_smoke".to_owned(),
            comments_prepared: 0,
            comments_posted: None,
            comments_accepted: None,
            comments_resolved: None,
            comments_off_diff_rejected: 1,
            fills_with_signal: 1,
            fills_total: 2,
            llm_unavailable_events: 1,
            fallback_used_lanes: 1,
            reviewer_overrides: None,
            adopted_generated_tests: None,
            missing: Vec::new(),
        };

        let trend = build_quality_trend_artifact(&receipt);
        let missing_fields = trend
            .missing
            .iter()
            .map(|entry| entry.field.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(trend.schema, super::QUALITY_TREND_SCHEMA);
        assert_eq!(trend.run_id, receipt.run_id);
        assert_eq!(trend.window_scope, "single_run_v1");
        assert_eq!(trend.window_runs, 1);
        assert_eq!(trend.comments_prepared, 0);
        assert_eq!(trend.comments_posted, None);
        assert_eq!(trend.comment_acceptance_rate, None);
        assert_eq!(trend.comment_resolution_rate, None);
        assert_eq!(trend.fills_signal_rate, Some(0.5));
        assert_eq!(trend.llm_unavailable_rate, 1.0);
        assert_eq!(trend.reviewer_override_rate, None);
        assert_eq!(trend.adopted_generated_tests, None);
        assert_eq!(trend.trend.comment_acceptance_rate_delta, None);
        assert_eq!(trend.trend.fills_signal_rate_delta, None);
        assert_eq!(trend.trend.llm_unavailable_rate_delta, None);
        assert_eq!(trend.trend.reviewer_override_rate_delta, None);
        assert!(
            trend
                .source_artifacts
                .contains(&"review/quality-receipt.json".to_owned())
        );
        assert!(missing_fields.contains("comment_acceptance_rate"));
        assert!(missing_fields.contains("comment_resolution_rate"));
        assert!(missing_fields.contains("reviewer_override_rate"));
        assert!(missing_fields.contains("adopted_generated_tests"));
        assert!(missing_fields.contains("trend.comment_acceptance_rate_delta"));
        assert!(missing_fields.contains("trend.fills_signal_rate_delta"));
        assert!(missing_fields.contains("trend.llm_unavailable_rate_delta"));
        assert!(missing_fields.contains("trend.reviewer_override_rate_delta"));
        assert!(!missing_fields.contains("fills_signal_rate"));
    }

    #[test]
    fn quality_backfill_aggregates_github_outcomes_and_previous_deltas() {
        let runs = vec![
            super::QualityBackfillRun {
                receipt: super::QualityReceiptSeed {
                    schema: super::QUALITY_RECEIPT_SCHEMA.to_owned(),
                    run_id: "run-a".to_owned(),
                    comments_prepared: 2,
                    fills_with_signal: 1,
                    fills_total: 2,
                    llm_unavailable_events: 0,
                },
                receipt_source: "review/quality-backfill-sources/run-a-receipt.json".to_owned(),
                trend_source: Some("review/quality-backfill-sources/run-a-trend.json".to_owned()),
            },
            super::QualityBackfillRun {
                receipt: super::QualityReceiptSeed {
                    schema: super::QUALITY_RECEIPT_SCHEMA.to_owned(),
                    run_id: "run-b".to_owned(),
                    comments_prepared: 1,
                    fills_with_signal: 1,
                    fills_total: 1,
                    llm_unavailable_events: 1,
                },
                receipt_source: "review/quality-backfill-sources/run-b-receipt.json".to_owned(),
                trend_source: Some("review/quality-backfill-sources/run-b-trend.json".to_owned()),
            },
        ];
        let outcomes = super::LoadedGithubQualityOutcomes {
            outcomes: super::GithubQualityOutcomes {
                schema: Some(super::GITHUB_QUALITY_OUTCOMES_SCHEMA.to_owned()),
                source_artifacts: Vec::new(),
                comments: vec![
                    super::GithubQualityCommentOutcome {
                        posted: Some(true),
                        accepted: Some(true),
                        resolved: Some(true),
                        reviewer_override: Some(false),
                    },
                    super::GithubQualityCommentOutcome {
                        posted: Some(true),
                        accepted: Some(false),
                        resolved: Some(true),
                        reviewer_override: Some(true),
                    },
                    super::GithubQualityCommentOutcome {
                        posted: Some(false),
                        accepted: Some(true),
                        resolved: Some(true),
                        reviewer_override: Some(true),
                    },
                ],
                adopted_generated_tests: vec![serde_json::json!({"path": "tests/generated.rs"})],
            },
            has_comments: true,
            has_adopted_generated_tests: true,
            source_artifact: "review/quality-backfill-sources/github-quality-outcomes.json"
                .to_owned(),
            raw_source_artifacts: Vec::new(),
        };
        let previous = super::LoadedPreviousQualityBackfill {
            artifact: super::PreviousQualityBackfill {
                schema: super::QUALITY_BACKFILL_SCHEMA.to_owned(),
                comment_acceptance_rate: Some(0.25),
                fills_signal_rate: Some(0.5),
                llm_unavailable_rate: Some(0.25),
                reviewer_override_rate: Some(0.0),
            },
            source_artifact: "review/quality-backfill-sources/previous-quality-backfill.json"
                .to_owned(),
        };

        let artifact =
            super::build_quality_backfill_artifact(30, &runs, Some(&outcomes), Some(&previous));

        assert_eq!(artifact.schema, super::QUALITY_BACKFILL_SCHEMA);
        assert_eq!(artifact.window_scope, "rolling_v1");
        assert_eq!(artifact.window_runs, 2);
        assert_eq!(artifact.comments_prepared, 3);
        assert_eq!(artifact.comments_posted, Some(2));
        assert_eq!(artifact.comments_accepted, Some(1));
        assert_eq!(artifact.comments_resolved, Some(2));
        assert_eq!(artifact.comment_acceptance_rate, Some(0.5));
        assert_eq!(artifact.comment_resolution_rate, Some(1.0));
        assert_eq!(artifact.fills_signal_rate, Some(2.0 / 3.0));
        assert_eq!(artifact.llm_unavailable_rate, Some(0.5));
        assert_eq!(artifact.reviewer_overrides, Some(1));
        assert_eq!(artifact.reviewer_override_rate, Some(0.5));
        assert_eq!(artifact.adopted_generated_tests, Some(1));
        assert_eq!(artifact.trend.comment_acceptance_rate_delta, Some(0.25));
        assert_eq!(
            artifact.trend.fills_signal_rate_delta,
            Some((2.0 / 3.0) - 0.5)
        );
        assert_eq!(artifact.trend.llm_unavailable_rate_delta, Some(0.25));
        assert_eq!(artifact.trend.reviewer_override_rate_delta, Some(0.5));
        assert!(artifact.missing.is_empty());
        assert!(
            artifact.source_artifacts.contains(
                &"review/quality-backfill-sources/github-quality-outcomes.json".to_owned()
            )
        );
    }

    #[test]
    fn quality_backfill_reviewer_override_rate_uses_posted_comment_denominator() {
        let runs = vec![super::QualityBackfillRun {
            receipt: super::QualityReceiptSeed {
                schema: super::QUALITY_RECEIPT_SCHEMA.to_owned(),
                run_id: "run-a".to_owned(),
                comments_prepared: 2,
                fills_with_signal: 0,
                fills_total: 1,
                llm_unavailable_events: 0,
            },
            receipt_source: "review/quality-backfill-sources/run-a-receipt.json".to_owned(),
            trend_source: Some("review/quality-backfill-sources/run-a-trend.json".to_owned()),
        }];
        let outcomes = super::LoadedGithubQualityOutcomes {
            outcomes: super::GithubQualityOutcomes {
                schema: Some(super::GITHUB_QUALITY_OUTCOMES_SCHEMA.to_owned()),
                source_artifacts: Vec::new(),
                comments: vec![
                    super::GithubQualityCommentOutcome {
                        posted: Some(true),
                        accepted: Some(true),
                        resolved: Some(true),
                        reviewer_override: Some(true),
                    },
                    super::GithubQualityCommentOutcome {
                        posted: Some(true),
                        accepted: Some(false),
                        resolved: Some(false),
                        reviewer_override: Some(true),
                    },
                ],
                adopted_generated_tests: Vec::new(),
            },
            has_comments: true,
            has_adopted_generated_tests: true,
            source_artifact: "review/quality-backfill-sources/github-quality-outcomes.json"
                .to_owned(),
            raw_source_artifacts: Vec::new(),
        };

        let artifact = super::build_quality_backfill_artifact(30, &runs, Some(&outcomes), None);
        let missing_fields = artifact
            .missing
            .iter()
            .map(|entry| entry.field.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(artifact.window_runs, 1);
        assert_eq!(artifact.comments_posted, Some(2));
        assert_eq!(artifact.reviewer_overrides, Some(2));
        assert_eq!(artifact.reviewer_override_rate, Some(1.0));
        assert!(
            !missing_fields.contains("reviewer_override_rate"),
            "posted-comment denominator keeps reviewer_override_rate inside verifier bounds"
        );
    }

    #[test]
    fn quality_backfill_keeps_reviewer_rates_missing_without_github_receipts() {
        let runs = vec![super::QualityBackfillRun {
            receipt: super::QualityReceiptSeed {
                schema: super::QUALITY_RECEIPT_SCHEMA.to_owned(),
                run_id: "run-a".to_owned(),
                comments_prepared: 1,
                fills_with_signal: 0,
                fills_total: 1,
                llm_unavailable_events: 0,
            },
            receipt_source: "review/quality-backfill-sources/run-a-receipt.json".to_owned(),
            trend_source: Some("review/quality-backfill-sources/run-a-trend.json".to_owned()),
        }];

        let artifact = super::build_quality_backfill_artifact(30, &runs, None, None);
        let missing_fields = artifact
            .missing
            .iter()
            .map(|entry| entry.field.as_str())
            .collect::<BTreeSet<_>>();

        assert_eq!(artifact.comments_posted, None);
        assert_eq!(artifact.comment_acceptance_rate, None);
        assert_eq!(artifact.comment_resolution_rate, None);
        assert_eq!(artifact.reviewer_override_rate, None);
        assert_eq!(artifact.adopted_generated_tests, None);
        for field in [
            "comments_posted",
            "comments_accepted",
            "comments_resolved",
            "comment_acceptance_rate",
            "comment_resolution_rate",
            "reviewer_overrides",
            "reviewer_override_rate",
            "adopted_generated_tests",
            "trend.comment_acceptance_rate_delta",
            "trend.fills_signal_rate_delta",
            "trend.llm_unavailable_rate_delta",
            "trend.reviewer_override_rate_delta",
        ] {
            assert!(missing_fields.contains(field), "missing field {field}");
        }
    }

    #[test]
    fn github_quality_outcomes_normalizes_review_thread_state() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("github");
        fs::create_dir_all(&source)?;
        fs::write(source.join("actions-runs.json"), "[]")?;
        fs::write(source.join("pr-state.json"), "[]")?;
        fs::write(source.join("review-threads.graphql"), "query ReviewThreads")?;
        fs::write(source.join("review-threads-request-445.json"), "{}")?;
        fs::write(
            source.join("review-threads-445.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "data": {
                    "repository": {
                        "pullRequest": {
                            "number": 445,
                            "mergedAt": "2026-06-13T01:29:45Z",
                            "files": {
                                "pageInfo": {"hasNextPage": false, "endCursor": null},
                                "nodes": [
                                    {
                                        "path": "tests/generated.rs",
                                        "additions": 12,
                                        "deletions": 0,
                                        "changeType": "ADDED"
                                    },
                                    {
                                        "path": "src/lib.rs",
                                        "additions": 3,
                                        "deletions": 1,
                                        "changeType": "MODIFIED"
                                    }
                                ]
                            },
                            "reviewThreads": {
                                "pageInfo": {"hasNextPage": false, "endCursor": null},
                                "nodes": [
                                    {
                                        "id": "thread-a",
                                        "isResolved": false,
                                        "comments": {
                                            "pageInfo": {"hasNextPage": false, "endCursor": null},
                                            "nodes": [
                                                {
                                                    "id": "comment-a",
                                                    "body": "[source-route] finding",
                                                    "url": "https://github.example/thread-a",
                                                    "author": {"login": "github-actions"}
                                                },
                                                {
                                                    "id": "comment-b",
                                                    "body": "external review",
                                                    "url": "https://github.example/thread-b",
                                                    "author": {"login": "chatgpt-codex-connector"}
                                                }
                                            ]
                                        }
                                    },
                                    {
                                        "id": "thread-b",
                                        "isResolved": true,
                                        "comments": {
                                            "pageInfo": {"hasNextPage": false, "endCursor": null},
                                            "nodes": [
                                                {
                                                    "id": "comment-c",
                                                    "body": "[tests] fixed",
                                                    "url": "https://github.example/thread-c",
                                                    "author": {"login": "github-actions[bot]"}
                                                }
                                            ]
                                        }
                                    }
                                ]
                            }
                        }
                    }
                }
            }))?,
        )?;

        let authors = super::github_quality_author_logins(&[]);
        let artifact = super::build_github_quality_outcomes_artifact(&source, &authors)?;
        let comments = artifact
            .comments
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("comments[] missing"))?;

        assert_eq!(artifact.schema, super::GITHUB_QUALITY_OUTCOMES_SCHEMA);
        assert_eq!(artifact.collection_status, "complete");
        assert!(artifact.collection_warnings.is_empty());
        assert!(
            artifact
                .source_artifacts
                .contains(&"actions-runs.json".to_owned())
        );
        assert!(
            artifact
                .source_artifacts
                .contains(&"pr-state.json".to_owned())
        );
        assert!(
            artifact
                .source_artifacts
                .contains(&"review-threads-445.json".to_owned())
        );
        assert!(
            artifact
                .source_artifacts
                .contains(&"review-threads-request-445.json".to_owned())
        );
        assert_eq!(comments.len(), 2);
        assert_eq!(comments[0].source_comment_id, "comment-a");
        assert_eq!(comments[0].accepted, Some(false));
        assert_eq!(comments[0].resolved, Some(false));
        assert_eq!(comments[0].reviewer_override, Some(true));
        assert_eq!(comments[1].source_comment_id, "comment-c");
        assert_eq!(comments[1].accepted, Some(true));
        assert_eq!(comments[1].resolved, Some(true));
        assert_eq!(comments[1].reviewer_override, Some(false));
        let adopted = artifact
            .adopted_generated_tests
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("adopted_generated_tests missing"))?;
        assert_eq!(adopted.len(), 1);
        assert_eq!(adopted[0].path, "tests/generated.rs");
        assert_eq!(adopted[0].source_comment_id, "comment-c");
        assert_eq!(
            adopted[0].outcome_source,
            "github.mergedResolvedUbReviewThreadChangedTestFile.v1"
        );
        Ok(())
    }

    #[test]
    fn github_quality_outcomes_requires_request_source_for_complete_collection() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("github");
        fs::create_dir_all(&source)?;
        fs::write(source.join("review-threads.graphql"), "query ReviewThreads")?;
        fs::write(
            source.join("review-threads-445.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "data": {
                    "repository": {
                        "pullRequest": {
                            "number": 445,
                            "mergedAt": "2026-06-13T01:29:45Z",
                            "files": {
                                "pageInfo": {"hasNextPage": false, "endCursor": null},
                                "nodes": []
                            },
                            "reviewThreads": {
                                "pageInfo": {"hasNextPage": false, "endCursor": null},
                                "nodes": []
                            }
                        }
                    }
                }
            }))?,
        )?;

        let authors = super::github_quality_author_logins(&[]);
        let artifact = super::build_github_quality_outcomes_artifact(&source, &authors)?;

        assert_eq!(artifact.collection_status, "incomplete");
        assert!(artifact.comments.is_none());
        assert!(
            artifact
                .collection_warnings
                .iter()
                .any(|warning| warning.reason == "missing_review_thread_request")
        );
        Ok(())
    }

    #[test]
    fn github_quality_outcomes_keeps_comments_absent_when_collection_truncated() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("github");
        fs::create_dir_all(&source)?;
        fs::write(source.join("actions-runs.json"), "[]")?;
        fs::write(
            source.join("review-threads-445.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "data": {
                    "repository": {
                        "pullRequest": {
                            "number": 445,
                            "mergedAt": "2026-06-13T01:29:45Z",
                            "files": {
                                "pageInfo": {"hasNextPage": false, "endCursor": null},
                                "nodes": []
                            },
                            "reviewThreads": {
                                "pageInfo": {"hasNextPage": true, "endCursor": "cursor-100"},
                                "nodes": [
                                    {
                                        "id": "thread-a",
                                        "isResolved": false,
                                        "comments": {
                                            "pageInfo": {"hasNextPage": false, "endCursor": null},
                                            "nodes": [
                                                {
                                                    "id": "comment-a",
                                                    "body": "[source-route] finding",
                                                    "url": "https://github.example/thread-a",
                                                    "author": {"login": "github-actions"}
                                                }
                                            ]
                                        }
                                    }
                                ]
                            }
                        }
                    }
                }
            }))?,
        )?;

        let authors = super::github_quality_author_logins(&[]);
        let artifact = super::build_github_quality_outcomes_artifact(&source, &authors)?;

        assert_eq!(artifact.collection_status, "incomplete");
        assert!(artifact.comments.is_none());
        assert!(
            artifact
                .collection_warnings
                .iter()
                .any(|warning| warning.reason == "review_threads_truncated")
        );
        Ok(())
    }

    #[test]
    fn github_quality_outcomes_keeps_comments_absent_when_error_receipt_present() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("github");
        fs::create_dir_all(&source)?;
        fs::write(source.join("actions-runs.json"), "[]")?;
        fs::write(
            source.join("review-thread-error-445.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "ub-review.github_review_threads_error.v1",
                "pull_number": 445,
                "error": "GraphQL rate limit"
            }))?,
        )?;

        let authors = super::github_quality_author_logins(&[]);
        let artifact = super::build_github_quality_outcomes_artifact(&source, &authors)?;

        assert_eq!(artifact.collection_status, "incomplete");
        assert!(artifact.comments.is_none());
        assert!(
            artifact
                .collection_warnings
                .iter()
                .any(|warning| warning.reason == "github_api_error")
        );
        Ok(())
    }

    #[test]
    fn quality_github_collect_writes_raw_receipts_for_outcomes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("github");
        fs::create_dir_all(&source)?;
        fs::write(source.join("review-thread-error-445.json"), "{}")?;
        let (graphql_url, handle) = spawn_fake_quality_github_graphql_api(1)?;

        super::cmd_quality_github_collect(super::QualityGithubCollectArgs {
            root: temp.path().to_path_buf(),
            source_dir: source.clone(),
            repo: Some("acme/widgets".to_owned()),
            pull_numbers: vec![445],
            pull_numbers_file: None,
            github_token: Some("test-token".to_owned()),
            github_api_url: "https://api.github.com".to_owned(),
            github_graphql_url: Some(graphql_url),
            timeout_sec: 10,
        })?;

        let requests = handle
            .join()
            .map_err(|_| anyhow::anyhow!("fake GitHub GraphQL API panicked"))??;
        assert_eq!(requests.len(), 1);
        let request = &requests[0];
        let expected_auth = super::github_graphql_auth_header("test-token");
        assert!(request.contains("POST /graphql HTTP/1.1"));
        assert!(request.contains(&expected_auth));
        assert!(request.contains("UbReviewQualityReviewThreads"));
        assert!(request.contains("files(first: 100)"));
        assert!(request.contains("changeType"));
        assert!(request.contains(r#""owner": "acme""#));
        assert!(request.contains(r#""name": "widgets""#));
        assert!(request.contains(r#""number": 445"#));

        assert!(source.join("review-threads.graphql").is_file());
        assert!(source.join("pr-numbers.txt").is_file());
        assert!(source.join("review-threads-request-445.json").is_file());
        assert!(source.join("review-threads-445.json").is_file());
        assert!(!source.join("review-thread-error-445.json").exists());
        let query = fs::read_to_string(source.join("review-threads.graphql"))?;
        assert!(query.contains("UbReviewQualityReviewThreads"));
        assert!(query.contains("createdAt"));

        let authors = super::github_quality_author_logins(&[]);
        let artifact = super::build_github_quality_outcomes_artifact(&source, &authors)?;
        let comments = artifact
            .comments
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("comments[] missing"))?;

        assert_eq!(artifact.schema, super::GITHUB_QUALITY_OUTCOMES_SCHEMA);
        assert_eq!(artifact.collection_status, "complete");
        assert!(artifact.collection_warnings.is_empty());
        assert!(
            artifact
                .source_artifacts
                .contains(&"review-threads-request-445.json".to_owned())
        );
        assert!(
            artifact
                .source_artifacts
                .contains(&"review-threads-445.json".to_owned())
        );
        assert_eq!(comments.len(), 1);
        assert_eq!(comments[0].source_comment_id, "comment-a");
        assert_eq!(comments[0].accepted, Some(true));
        assert_eq!(comments[0].resolved, Some(true));
        assert_eq!(comments[0].reviewer_override, Some(false));
        let adopted = artifact
            .adopted_generated_tests
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("adopted_generated_tests missing"))?;
        assert_eq!(adopted.len(), 1);
        assert_eq!(adopted[0].path, "tests/generated.rs");
        Ok(())
    }

    #[test]
    fn quality_github_collect_allows_empty_pull_list() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("github");
        let list = temp.path().join("pulls.txt");
        fs::write(&list, "# no pull requests in window\n")?;

        super::cmd_quality_github_collect(super::QualityGithubCollectArgs {
            root: temp.path().to_path_buf(),
            source_dir: source.clone(),
            repo: None,
            pull_numbers: Vec::new(),
            pull_numbers_file: Some(list),
            github_token: None,
            github_api_url: "https://api.github.com".to_owned(),
            github_graphql_url: None,
            timeout_sec: 10,
        })?;

        assert!(source.join("review-threads.graphql").is_file());
        assert_eq!(fs::read_to_string(source.join("pr-numbers.txt"))?, "");
        let authors = super::github_quality_author_logins(&[]);
        let artifact = super::build_github_quality_outcomes_artifact(&source, &authors)?;
        assert_eq!(artifact.collection_status, "missing");
        assert!(artifact.comments.is_none());
        assert_eq!(
            artifact.source_artifacts,
            vec![
                "pr-numbers.txt".to_owned(),
                "review-threads.graphql".to_owned()
            ]
        );
        Ok(())
    }

    #[test]
    fn quality_github_collect_pull_numbers_dedupes_file_and_cli() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let list = temp.path().join("pulls.txt");
        fs::write(&list, "445\n# comment\n446, 445\n")?;
        let args = super::QualityGithubCollectArgs {
            root: temp.path().to_path_buf(),
            source_dir: temp.path().join("github"),
            repo: Some("acme/widgets".to_owned()),
            pull_numbers: vec![444, 445],
            pull_numbers_file: Some(list),
            github_token: Some("test-token".to_owned()),
            github_api_url: "https://api.github.com".to_owned(),
            github_graphql_url: None,
            timeout_sec: 10,
        };

        assert_eq!(
            super::quality_github_collect_pull_numbers(&args)?,
            vec![444, 445, 446]
        );
        Ok(())
    }

    #[test]
    fn github_quality_outcomes_keeps_comments_absent_without_thread_receipts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let source = temp.path().join("github");
        fs::create_dir_all(&source)?;
        fs::write(source.join("actions-runs.json"), "[]")?;
        fs::write(source.join("pr-state.json"), "[]")?;

        let authors = super::github_quality_author_logins(&[]);
        let artifact = super::build_github_quality_outcomes_artifact(&source, &authors)?;

        assert_eq!(artifact.schema, super::GITHUB_QUALITY_OUTCOMES_SCHEMA);
        assert_eq!(artifact.collection_status, "missing");
        assert!(artifact.comments.is_none());
        assert_eq!(
            artifact.source_artifacts,
            vec!["actions-runs.json".to_owned(), "pr-state.json".to_owned()]
        );
        Ok(())
    }

    /// Drives a fake HTTP fixture listener until `expected_requests` requests
    /// have been served (or the deadline expires). See the reliability contract
    /// on the integration-test twin in `tests/common/mod.rs::serve_fake_http`
    /// (issue #760): a per-connection handler error is recorded and serving
    /// continues rather than tearing down the listener, so a retried client
    /// connection is answered instead of refused, and the recorded error is
    /// surfaced on a deadline bail.
    fn serve_fake_http(
        listener: TcpListener,
        expected_requests: usize,
        label: &str,
        deadline: Duration,
        handler: impl Fn(usize, TcpStream) -> Result<String>,
    ) -> Result<Vec<String>> {
        let mut deadline_at = Instant::now() + deadline;
        let mut requests = Vec::new();
        let mut last_error: Option<String> = None;
        while requests.len() < expected_requests {
            match listener.accept() {
                Ok((stream, _addr)) => match handler(requests.len(), stream) {
                    Ok(request) => {
                        requests.push(request);
                        deadline_at = Instant::now() + deadline;
                    }
                    // Recorded errors are advisory only: they surface in the
                    // deadline bail below, but are discarded once
                    // `expected_requests` is reached via later good connections.
                    // This deliberately favors tolerating the transient
                    // per-connection errors #760 targets over failing loud on a
                    // handler error that the caller then retries past.
                    Err(err) => last_error = Some(err.to_string()),
                },
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline_at {
                        if let Some(err) = &last_error {
                            bail!(
                                "fake {label} API received {} of {} requests; last handler error: {err}",
                                requests.len(),
                                expected_requests
                            );
                        }
                        bail!(
                            "fake {label} API received {} of {} requests",
                            requests.len(),
                            expected_requests
                        );
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(requests)
    }

    fn spawn_fake_quality_github_graphql_api(
        expected_requests: usize,
    ) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}/graphql", listener.local_addr()?);
        let handle = thread::spawn(move || {
            serve_fake_http(
                listener,
                expected_requests,
                "GitHub GraphQL",
                Duration::from_secs(20),
                |_idx, stream| handle_fake_quality_github_graphql_request(stream),
            )
        });
        Ok((url, handle))
    }

    fn handle_fake_quality_github_graphql_request(mut stream: TcpStream) -> Result<String> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                bail!("fake GitHub GraphQL request ended before headers finished");
            }
            headers.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .and_then(|value| value.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        let mut body = vec![0; content_length];
        {
            use std::io::Read as _;
            reader.read_exact(&mut body)?;
        }
        let request_body = String::from_utf8_lossy(&body);
        let response_body = serde_json::to_vec(&serde_json::json!({
            "data": {
                "repository": {
                    "pullRequest": {
                        "number": 445,
                        "mergedAt": "2026-06-13T01:29:45Z",
                        "files": {
                            "pageInfo": {"hasNextPage": false, "endCursor": null},
                            "nodes": [
                                {
                                    "path": "tests/generated.rs",
                                    "additions": 4,
                                    "deletions": 0,
                                    "changeType": "ADDED"
                                }
                            ]
                        },
                        "reviewThreads": {
                            "pageInfo": {"hasNextPage": false, "endCursor": null},
                            "nodes": [
                                {
                                    "id": "thread-a",
                                    "isResolved": true,
                                    "comments": {
                                        "pageInfo": {"hasNextPage": false, "endCursor": null},
                                        "nodes": [
                                            {
                                                "id": "comment-a",
                                                "body": "[tests] generated regression adopted",
                                                "createdAt": "2026-06-13T01:35:00Z",
                                                "url": "https://github.example/thread-a",
                                                "author": {"login": "github-actions"}
                                            }
                                        ]
                                    }
                                }
                            ]
                        }
                    }
                }
            }
        }))?;
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        )?;
        stream.write_all(&response_body)?;
        Ok(format!("{headers}\n{request_body}"))
    }

    #[test]
    fn artifact_name_sanitizer_bounds_long_generated_ids() {
        assert_eq!(
            super::sanitize_artifact_name("source-route/question one"),
            "source-route-question-one"
        );
        let raw = format!("candidate-{}", "generated-id-segment-".repeat(24));
        let sanitized = super::sanitize_artifact_name(&raw);
        let digest = sha256_hex(raw.as_bytes());
        assert!(sanitized.len() <= super::ARTIFACT_NAME_MAX_CHARS);
        assert!(sanitized.starts_with("candidate-generated-id-segment-"));
        assert!(sanitized.ends_with(&format!("-{}", &digest[..16])));

        let sibling = format!("{raw}-sibling");
        assert_ne!(sanitized, super::sanitize_artifact_name(&sibling));
    }

    #[test]
    fn long_model_artifact_ids_are_bounded_without_rewriting_receipts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let long_tail = "lane-generated-proof-request-".repeat(20);
        let candidate_id = format!("candidate-{long_tail}");
        let candidate = CandidateRecord {
            schema: "ub-review.candidate.v1".to_owned(),
            id: candidate_id.clone(),
            lane: "tests-oracle".to_owned(),
            source: "summary-only-finding".to_owned(),
            status: "summary-only".to_owned(),
            disposition: "summary-only".to_owned(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            claim: "Long generated candidate id should stay in the receipt.".to_owned(),
            evidence: "Filesystem artifact path is bounded separately.".to_owned(),
            path: None,
            line: None,
            side: None,
        };
        write_candidate_artifacts(temp.path(), std::slice::from_ref(&candidate))?;
        let candidate_file = temp.path().join("candidates").join(format!(
            "{}.json",
            super::sanitize_artifact_name(&candidate_id)
        ));
        assert!(candidate_file.is_file());
        assert!(
            candidate_file
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.len() <= super::ARTIFACT_NAME_MAX_CHARS + ".json".len())
        );
        let written_candidate: CandidateRecord =
            serde_json::from_slice(&fs::read(candidate_file)?)?;
        assert_eq!(written_candidate.id, candidate_id);

        let proof_request_id = format!("proof-request-{long_tail}");
        let proof_request = ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: proof_request_id.clone(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            command: "cargo test focused_case".to_owned(),
            reason: "Exercise long model-generated proof request ids.".to_owned(),
            cost: "low".to_owned(),
            timeout_sec: 60,
            required: false,
            status: "requested".to_owned(),
        };
        write_proof_request_artifacts(
            temp.path(),
            &test_diff(),
            &Profile::default(),
            std::slice::from_ref(&proof_request),
            &[] as &[ProofReceipt],
            &[] as &[ResourceLease],
        )?;
        let proof_request_file = temp.path().join("proof_requests").join(format!(
            "{}.json",
            super::sanitize_artifact_name(&proof_request_id)
        ));
        assert!(proof_request_file.is_file());
        let written_request: ProofRequest = serde_json::from_slice(&fs::read(proof_request_file)?)?;
        assert_eq!(written_request.id, proof_request_id);

        let task_id = format!("follow-up-{long_tail}");
        let task = FollowUpQuestionTask {
            schema: "ub-review.follow_up_question_task.v1".to_owned(),
            id: task_id.clone(),
            group_id: "group-long-generated-id".to_owned(),
            stage: "tertiary".to_owned(),
            stage_reason: "routed evidence arrived".to_owned(),
            evidence_need: "proof-confirmation".to_owned(),
            disposition: "summary-only".to_owned(),
            candidate_ids: vec![candidate.id],
            observation_group_ids: Vec::new(),
            routed_evidence: Vec::new(),
            question: "Does the new proof receipt close this concern?".to_owned(),
            status: "planned".to_owned(),
            reason: "test long follow-up artifact path".to_owned(),
        };
        super::write_follow_up_question_packets(temp.path(), std::slice::from_ref(&task), &[])?;
        let packet_path = temp
            .path()
            .join(super::follow_up_packet_artifact_path(&task));
        assert!(packet_path.is_file());
        let packet: serde_json::Value = serde_json::from_slice(&fs::read(packet_path)?)?;
        assert_eq!(packet["task_id"], task_id);
        assert!(
            follow_up_model_lane_id(&task).len()
                <= "orchestrator-follow-up-".len() + super::ARTIFACT_NAME_MAX_CHARS
        );
        Ok(())
    }

    #[test]
    fn follow_up_packet_prompt_carries_bounded_routed_receipt_content() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let proof_dir = temp
            .path()
            .join("proof")
            .join("proof-build-abc")
            .join("head");
        fs::create_dir_all(&proof_dir)?;
        // stderr larger than the tail cap proves bounding; stdout small and
        // fully included.
        let loud = "assertion failed: the focused proof exposed the gap\n".repeat(60);
        fs::write(proof_dir.join("stderr.txt"), &loud)?;
        fs::write(proof_dir.join("stdout.txt"), "running 1 test\n")?;

        let mut receipt = test_red_green_proof_receipt("head_failed", "failed");
        receipt.id = "proof-build-abc".to_owned();
        receipt.commands = vec![ProofCommandReceipt {
            side: "head".to_owned(),
            command: "cargo test focused_case".to_owned(),
            env: BTreeMap::new(),
            status: "failed".to_owned(),
            exit_code: Some(101),
            timed_out: false,
            timeout_sec: 60,
            duration_ms: 1_200,
            stdout: "proof/proof-build-abc/head/stdout.txt".to_owned(),
            stderr: "proof/proof-build-abc/head/stderr.txt".to_owned(),
            reason: "exit code Some(101)".to_owned(),
        }];

        let task = FollowUpQuestionTask {
            schema: "ub-review.follow_up_question_task.v1".to_owned(),
            id: "follow-up-receipt-content".to_owned(),
            group_id: "group-receipt-content".to_owned(),
            stage: "tertiary".to_owned(),
            stage_reason: "routed evidence arrived".to_owned(),
            evidence_need: "proof-confirmation".to_owned(),
            disposition: "summary-only".to_owned(),
            candidate_ids: Vec::new(),
            observation_group_ids: Vec::new(),
            routed_evidence: vec![super::proof_receipt_routed_evidence(&receipt)],
            question: "Does the failed focused proof confirm the concern?".to_owned(),
            status: "planned".to_owned(),
            reason: "test routed receipt content".to_owned(),
        };
        super::write_follow_up_question_packets(
            temp.path(),
            std::slice::from_ref(&task),
            std::slice::from_ref(&receipt),
        )?;
        let packet_path = temp
            .path()
            .join(super::follow_up_packet_artifact_path(&task));
        let packet: serde_json::Value = serde_json::from_slice(&fs::read(packet_path)?)?;
        let prompt = packet["prompt"].as_str().unwrap_or_default();
        assert!(
            prompt.contains("Receipt content (bounded command-output tails"),
            "prompt should carry the receipt content block: {prompt}"
        );
        assert!(
            prompt.contains("assertion failed: the focused proof exposed the gap"),
            "prompt should carry the stderr tail"
        );
        assert!(
            prompt.contains(&format!(
                "stderr (last {} bytes of {}):",
                super::ROUTED_RECEIPT_STDERR_TAIL_BYTES,
                loud.len()
            )),
            "oversized stderr must be explicitly truncated: {prompt}"
        );
        assert!(
            prompt.contains("running 1 test"),
            "prompt should carry the small stdout in full"
        );
        assert!(
            prompt.contains(
                "command `cargo test focused_case` side=`head` status=`failed` exit=Some(101)"
            ),
            "prompt should carry the command status line: {prompt}"
        );

        // A task with no matching receipt renders metadata only, no content
        // block.
        let mut bare = task.clone();
        bare.id = "follow-up-no-content".to_owned();
        bare.routed_evidence[0].id = "proof-unknown".to_owned();
        super::write_follow_up_question_packets(
            temp.path(),
            std::slice::from_ref(&bare),
            std::slice::from_ref(&receipt),
        )?;
        let bare_packet: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path()
                .join(super::follow_up_packet_artifact_path(&bare)),
        )?)?;
        let bare_prompt = bare_packet["prompt"].as_str().unwrap_or_default();
        assert!(!bare_prompt.contains("Receipt content"));
        Ok(())
    }

    #[test]
    fn candidate_artifacts_track_inline_and_summary_surfaces() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let inline_comments = vec![ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            path: "test/js/bun/md/md-edge-cases.test.ts".to_owned(),
            line: 1145,
            side: "RIGHT".to_owned(),
            body: "[tests-oracle] Added regression needs a red witness.".to_owned(),
            evidence: "RIGHT-side line map and test proof request".to_owned(),
            suggestion: None,
        }];
        let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "source-route".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "inline guard rejected src/lib.rs:99; line_valid=false".to_owned(),
                evidence: "line map receipt".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "source-route".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "PBKDF2 sibling path is parked as follow-up, not current PR scope."
                    .to_owned(),
                evidence: "UB ledger follow-up".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "opposition".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "`Box::from(slice)` allocation fallback claim was refuted.".to_owned(),
                evidence: "false premise calibration".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "tests-oracle".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "duplicate inline candidate merged into src/lib.rs:2".to_owned(),
                evidence: "duplicate evidence".to_owned(),
            },
        ];
        let candidates = build_candidate_records(&inline_comments, &summary_only_findings);

        write_candidate_artifacts(temp.path(), &candidates)?;

        let aggregate: Vec<super::CandidateRecord> =
            serde_json::from_slice(&fs::read(temp.path().join("review/candidates.json"))?)?;
        let first_file: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path()
                .join("candidates")
                .join(format!("{}.json", aggregate[0].id)),
        )?)?;
        let ndjson = fs::read_to_string(temp.path().join("candidates.ndjson"))?;
        let (hydrated_inline, hydrated_summary) = read_candidate_review_surfaces(temp.path())?;

        assert_eq!(aggregate.len(), 5);
        assert_eq!(aggregate[0].schema, "ub-review.candidate.v1");
        assert_eq!(aggregate[0].source, "inline-comment");
        assert_eq!(aggregate[0].status, "accepted-inline");
        assert_eq!(aggregate[0].disposition, "inline");
        assert_eq!(
            aggregate[0].path.as_deref(),
            Some(inline_comments[0].path.as_str())
        );
        assert_eq!(aggregate[0].line, Some(inline_comments[0].line));
        assert_eq!(aggregate[0].side.as_deref(), Some("RIGHT"));
        assert_eq!(aggregate[1].source, "summary-only-finding");
        assert_eq!(aggregate[1].status, "summary-only");
        assert_eq!(aggregate[1].disposition, "summary-only");
        assert!(aggregate[1].path.is_none());
        assert_eq!(aggregate[2].disposition, "parked-follow-up");
        assert_eq!(aggregate[3].disposition, "refuted");
        assert_eq!(aggregate[4].disposition, "dropped");
        assert_eq!(first_file, serde_json::to_value(&aggregate[0])?);
        assert_eq!(ndjson.lines().count(), 5);
        assert!(ndjson.contains("\"schema\":\"ub-review.candidate.v1\""));
        assert_eq!(hydrated_inline.len(), 1);
        assert_eq!(hydrated_inline[0].body, inline_comments[0].body);
        assert_eq!(hydrated_inline[0].side, "RIGHT");
        assert_eq!(hydrated_summary.len(), 4);
        assert_eq!(hydrated_summary[0].reason, summary_only_findings[0].reason);
        Ok(())
    }

    #[test]
    fn candidate_artifact_readback_rejects_malformed_records() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("review"))?;
        fs::write(
            temp.path().join("review/candidates.json"),
            serde_json::to_vec_pretty(&serde_json::json!([
                {
                    "schema": "ub-review.candidate.v1",
                    "id": "candidate-bad",
                    "lane": "tests-oracle",
                    "source": "inline-comment",
                    "status": "accepted-inline",
                    "disposition": "inline",
                    "severity": "medium",
                    "confidence": "high",
                    "claim": "[tests-oracle] Missing path should fail readback.",
                    "evidence": "candidate artifact fixture",
                    "line": 42,
                    "side": "RIGHT"
                }
            ]))?,
        )?;

        let error = match read_candidate_review_surfaces(temp.path()) {
            Ok(_) => return Err(anyhow::anyhow!("malformed candidate was accepted")),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("candidate candidate-bad missing path")
        );
        Ok(())
    }

    #[test]
    fn candidate_artifact_readback_rejects_inconsistent_disposition() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::create_dir_all(temp.path().join("review"))?;
        fs::write(
            temp.path().join("review/candidates.json"),
            serde_json::to_vec_pretty(&serde_json::json!([
                {
                    "schema": "ub-review.candidate.v1",
                    "id": "candidate-bad-disposition",
                    "lane": "tests-oracle",
                    "source": "summary-only-finding",
                    "status": "summary-only",
                    "disposition": "inline",
                    "severity": "medium",
                    "confidence": "high",
                    "claim": "Summary-only record cannot have inline disposition.",
                    "evidence": "candidate artifact fixture"
                }
            ]))?,
        )?;

        let error = match read_candidate_review_surfaces(temp.path()) {
            Ok(_) => return Err(anyhow::anyhow!("inconsistent candidate was accepted")),
            Err(error) => error,
        };

        assert!(error.to_string().contains(
            "summary-only candidate candidate-bad-disposition disposition cannot be inline"
        ));
        Ok(())
    }

    #[test]
    fn orchestrator_plan_groups_evidence_needs_and_tasks() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let candidates = vec![
            super::CandidateRecord {
                schema: "ub-review.candidate.v1".to_owned(),
                id: "candidate-proof-a".to_owned(),
                lane: "tests-oracle".to_owned(),
                source: "summary-only-finding".to_owned(),
                status: "summary-only".to_owned(),
                disposition: "summary-only".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                claim: "Needs red/green proof before upstream.".to_owned(),
                evidence: "proof request from tests lane".to_owned(),
                path: None,
                line: None,
                side: None,
            },
            super::CandidateRecord {
                schema: "ub-review.candidate.v1".to_owned(),
                id: "candidate-proof-b".to_owned(),
                lane: "opposition".to_owned(),
                source: "summary-only-finding".to_owned(),
                status: "summary-only".to_owned(),
                disposition: "summary-only".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium".to_owned(),
                claim: "Red witness is still missing.".to_owned(),
                evidence: "proof concern from opposition".to_owned(),
                path: None,
                line: None,
                side: None,
            },
            super::CandidateRecord {
                schema: "ub-review.candidate.v1".to_owned(),
                id: "candidate-inline".to_owned(),
                lane: "ub-memory-lifetime".to_owned(),
                source: "inline-comment".to_owned(),
                status: "accepted-inline".to_owned(),
                disposition: "inline".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                claim: "Inline comment survives line validation.".to_owned(),
                evidence: "RIGHT-side line map".to_owned(),
                path: Some("src/lib.rs".to_owned()),
                line: Some(42),
                side: Some("RIGHT".to_owned()),
            },
            super::CandidateRecord {
                schema: "ub-review.candidate.v1".to_owned(),
                id: "candidate-dropped".to_owned(),
                lane: "architecture".to_owned(),
                source: "summary-only-finding".to_owned(),
                status: "summary-only".to_owned(),
                disposition: "dropped".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                claim: "Duplicate inline candidate merged into src/lib.rs:42.".to_owned(),
                evidence: "duplicate evidence".to_owned(),
                path: None,
                line: None,
                side: None,
            },
            super::CandidateRecord {
                schema: "ub-review.candidate.v1".to_owned(),
                id: "candidate-parked".to_owned(),
                lane: "sibling-paths".to_owned(),
                source: "summary-only-finding".to_owned(),
                status: "summary-only".to_owned(),
                disposition: "parked-follow-up".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                claim: "Sibling helper needs a later route check.".to_owned(),
                evidence: "parked follow-up evidence".to_owned(),
                path: None,
                line: None,
                side: None,
            },
            super::CandidateRecord {
                schema: "ub-review.candidate.v1".to_owned(),
                id: "candidate-refuted".to_owned(),
                lane: "opposition".to_owned(),
                source: "summary-only-finding".to_owned(),
                status: "summary-only".to_owned(),
                disposition: "refuted".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                claim: "False premise was refuted before posting.".to_owned(),
                evidence: "refuted by deterministic calibration".to_owned(),
                path: None,
                line: None,
                side: None,
            },
        ];

        let observations = vec![
            test_observation(
                "tests-oracle",
                "The new test needs a witnessed old-main red run.",
                "missing-evidence",
                "open",
                "medium",
                "high",
                "markdown-red-green-witness",
            ),
            test_observation(
                "opposition",
                "The new test needs a witnessed old-main red run.",
                "missing-evidence",
                "open",
                "medium",
                "medium-high",
                "markdown-red-green-witness",
            ),
        ];
        let observation_summary = observation_summary_artifacts(&observations);

        let no_evidence_plan =
            build_orchestrator_plan(&candidates, &observation_summary.unique, &[], &[], &[]);
        let no_evidence_proof_group = no_evidence_plan
            .evidence_groups
            .iter()
            .find(|group| group.evidence_need == "proof-confirmation")
            .ok_or_else(|| anyhow::anyhow!("proof group should be present without evidence"))?;
        assert!(no_evidence_proof_group.routed_evidence.is_empty());
        let no_evidence_proof_task = no_evidence_plan
            .follow_up_tasks
            .iter()
            .find(|task| task.group_id == no_evidence_proof_group.id)
            .ok_or_else(|| anyhow::anyhow!("proof task should be present without evidence"))?;
        assert_eq!(no_evidence_proof_task.stage, "secondary");
        assert!(
            no_evidence_proof_task
                .stage_reason
                .contains("no routed proof receipt")
        );
        assert_eq!(
            no_evidence_plan
                .follow_up_tasks
                .iter()
                .find(|task| task.disposition == "parked-follow-up")
                .map(|task| task.stage.as_str()),
            Some("tertiary")
        );
        assert_eq!(
            no_evidence_plan
                .follow_up_tasks
                .iter()
                .find(|task| task.disposition == "refuted")
                .map(|task| task.stage.as_str()),
            Some("tertiary")
        );

        let mut confirmed_receipt = test_red_green_proof_receipt("discriminating", "failed");
        confirmed_receipt.id = "proof-confirmed".to_owned();
        confirmed_receipt.reason =
            "HEAD passed; base+tests failed: discriminating proof".to_owned();
        let mut missing_receipt = test_proof_receipt("timed_out", "timed_out");
        missing_receipt.id = "proof-timeout".to_owned();
        missing_receipt.reason = "Focused proof timed out.".to_owned();
        let proof_receipts = vec![confirmed_receipt, missing_receipt];
        let resource_leases = vec![
            ResourceLease {
                schema: "ub-review.resource_lease.v1".to_owned(),
                id: "lease-proof-confirmed".to_owned(),
                kind: "focused-test".to_owned(),
                consumer: "proof-confirmed".to_owned(),
                status: "granted".to_owned(),
                reason: "focused proof lease granted".to_owned(),
                cpu: 2,
                memory_mb: 2_048,
                disk_mb: 1_024,
                timeout_sec: 600,
                network: false,
                scratch: true,
                worktree: Some("base-plus-tests".to_owned()),
                command: Some("bun test test/js/bun/md/md-edge-cases.test.ts".to_owned()),
            },
            ResourceLease {
                schema: "ub-review.resource_lease.v1".to_owned(),
                id: "lease-proof-timeout".to_owned(),
                kind: "focused-test".to_owned(),
                consumer: "proof-timeout".to_owned(),
                status: "exhausted".to_owned(),
                reason: "focused proof lease budget exhausted".to_owned(),
                cpu: 2,
                memory_mb: 2_048,
                disk_mb: 1_024,
                timeout_sec: 600,
                network: false,
                scratch: true,
                worktree: Some("base-plus-tests".to_owned()),
                command: Some("bun test test/js/bun/md/md-edge-cases.test.ts".to_owned()),
            },
        ];

        let plan = build_orchestrator_plan(
            &candidates,
            &observation_summary.unique,
            &proof_receipts,
            &resource_leases,
            &[],
        );
        write_orchestrator_artifacts(temp.path(), &plan, &proof_receipts)?;

        let aggregate: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/orchestrator_plan.json"),
        )?)?;
        let ndjson = fs::read_to_string(temp.path().join("follow_up_questions.ndjson"))?;
        let proof_group = plan
            .evidence_groups
            .iter()
            .find(|group| group.evidence_need == "proof-confirmation")
            .ok_or_else(|| anyhow::anyhow!("proof group should be present"))?;
        let inline_group = plan
            .evidence_groups
            .iter()
            .find(|group| group.disposition == "inline")
            .ok_or_else(|| anyhow::anyhow!("inline group should be present"))?;
        let dropped_group = plan
            .evidence_groups
            .iter()
            .find(|group| group.disposition == "dropped")
            .ok_or_else(|| anyhow::anyhow!("dropped group should be present"))?;
        let observation_group = plan
            .observation_groups
            .iter()
            .find(|group| group.observation_group_id == observation_summary.unique[0].id)
            .ok_or_else(|| anyhow::anyhow!("observation group should be present"))?;

        assert_eq!(aggregate, serde_json::to_value(&plan)?);
        assert_eq!(plan.schema, "ub-review.orchestrator_plan.v1");
        assert_eq!(plan.candidates, 6);
        assert_eq!(plan.observations, 1);
        assert_eq!(
            proof_group.candidate_ids,
            vec!["candidate-proof-a", "candidate-proof-b"]
        );
        assert_eq!(proof_group.lanes, vec!["opposition", "tests-oracle"]);
        assert_eq!(proof_group.duplicate_count, 1);
        assert_eq!(proof_group.routed_evidence.len(), 4);
        assert!(proof_group.routed_evidence.iter().any(|evidence| {
            evidence.kind == "proof-receipt"
                && evidence.id == "proof-confirmed"
                && evidence.artifact == "review/proof_receipts.json"
                && evidence.status == "tool-confirmed"
                && evidence.result == "discriminating"
        }));
        assert!(proof_group.routed_evidence.iter().any(|evidence| {
            evidence.kind == "proof-receipt"
                && evidence.id == "proof-timeout"
                && evidence.status == "missing-evidence"
                && evidence.result == "timed_out"
        }));
        assert!(proof_group.routed_evidence.iter().any(|evidence| {
            evidence.kind == "resource-lease"
                && evidence.id == "lease-proof-confirmed"
                && evidence.artifact == "review/resource_leases.json"
                && evidence.status == "granted"
        }));
        assert!(proof_group.routed_evidence.iter().any(|evidence| {
            evidence.kind == "resource-lease"
                && evidence.id == "lease-proof-timeout"
                && evidence.status == "exhausted"
        }));
        assert_eq!(inline_group.duplicate_count, 0);
        assert!(inline_group.routed_evidence.is_empty());
        assert_eq!(dropped_group.duplicate_count, 0);
        assert!(dropped_group.routed_evidence.is_empty());
        assert_eq!(plan.observation_groups.len(), 1);
        assert_eq!(
            observation_group.schema,
            "ub-review.orchestrator_observation_group.v1"
        );
        assert_eq!(observation_group.evidence_need, "proof-confirmation");
        assert_eq!(observation_group.duplicate_count, 1);
        assert_eq!(observation_group.lanes, vec!["tests-oracle", "opposition"]);
        assert_eq!(observation_group.sources, vec!["model-observation"]);
        assert_eq!(observation_group.routed_evidence.len(), 4);
        assert_eq!(
            serde_json::to_value(&observation_group.routed_evidence)?,
            serde_json::to_value(&proof_group.routed_evidence)?
        );
        assert!(
            !plan
                .follow_up_tasks
                .iter()
                .any(|task| task.group_id == inline_group.id || task.group_id == dropped_group.id)
        );

        let task_group_ids = plan
            .follow_up_tasks
            .iter()
            .map(|task| task.group_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(plan.follow_up_tasks.len(), 4);
        assert!(task_group_ids.contains(&proof_group.id.as_str()));
        assert!(task_group_ids.contains(&observation_group.id.as_str()));
        let proof_task = plan
            .follow_up_tasks
            .iter()
            .find(|task| task.group_id == proof_group.id)
            .ok_or_else(|| anyhow::anyhow!("proof follow-up task should be present"))?;
        assert_eq!(proof_task.stage, "tertiary");
        assert!(
            proof_task
                .stage_reason
                .contains("routed evidence or prior disposition")
        );
        assert_eq!(
            serde_json::to_value(&proof_task.routed_evidence)?,
            serde_json::to_value(&proof_group.routed_evidence)?
        );
        let observation_task = plan
            .follow_up_tasks
            .iter()
            .find(|task| task.group_id == observation_group.id)
            .ok_or_else(|| anyhow::anyhow!("observation follow-up task should be present"))?;
        assert_eq!(observation_task.disposition, "observation");
        assert_eq!(observation_task.stage, "tertiary");
        assert!(observation_task.candidate_ids.is_empty());
        assert_eq!(
            observation_task.observation_group_ids,
            vec![observation_group.observation_group_id.clone()]
        );
        assert_eq!(
            serde_json::to_value(&observation_task.routed_evidence)?,
            serde_json::to_value(&observation_group.routed_evidence)?
        );
        assert!(
            plan.follow_up_tasks
                .iter()
                .all(|task| task.status == "planned")
        );

        let ndjson_tasks = ndjson
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let expected_tasks = plan
            .follow_up_tasks
            .iter()
            .map(serde_json::to_value)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(ndjson_tasks, expected_tasks);
        let follow_up_dir = temp.path().join("questions/orchestrator-follow-up");
        assert!(follow_up_dir.is_dir());
        let follow_up_files = fs::read_dir(&follow_up_dir)?
            .filter_map(std::result::Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>();
        assert_eq!(follow_up_files.len(), plan.follow_up_tasks.len());
        let proof_packet: serde_json::Value = serde_json::from_slice(&fs::read(
            follow_up_dir.join(format!("{}.json", proof_task.id)),
        )?)?;
        assert_eq!(
            proof_packet["schema"],
            "ub-review.follow_up_question_packet.v1"
        );
        assert_eq!(proof_packet["task_id"], proof_task.id);
        assert_eq!(proof_packet["group_id"], proof_task.group_id);
        assert_eq!(proof_packet["stage"], proof_task.stage);
        assert_eq!(proof_packet["stage_reason"], proof_task.stage_reason);
        assert_eq!(
            proof_packet["routed_evidence"],
            serde_json::to_value(&proof_task.routed_evidence)?
        );
        assert!(proof_packet["prompt"].as_str().is_some_and(|prompt| {
            prompt.contains("Routed evidence:")
                && prompt.contains("proof-confirmed")
                && prompt.contains("- Stage: `tertiary`")
                && prompt.contains("use routed evidence to refine, refute, drop, or park")
                && prompt.contains("Do not post, mutate, or run shell commands")
        }));
        let observation_packet: serde_json::Value = serde_json::from_slice(&fs::read(
            follow_up_dir.join(format!("{}.json", observation_task.id)),
        )?)?;
        assert_eq!(observation_packet["disposition"], "observation");
        assert_eq!(
            observation_packet["observation_group_ids"],
            serde_json::json!([observation_group.observation_group_id])
        );

        let mut args = test_run_args(temp.path().join("out"));
        args.model_mode = ModelMode::Off;
        let review_dir = temp.path().join("review");
        let mut follow_up_results = Vec::new();
        let mut follow_up_outputs = Vec::new();
        let calls = super::run_follow_up_model_pass(
            super::FollowUpRunContext {
                root: Path::new("."),
                out: temp.path(),
                review_dir: &review_dir,
                provider_preflights: &[],
                shared_context: "shared context",
                args: &args,
                model_calls_used: 0,
                key_present: |_| false,
                tasks: &plan.follow_up_tasks,
                line_map: &BTreeSet::new(),
            },
            &mut follow_up_results,
            &mut follow_up_outputs,
        )?;
        assert_eq!(calls, 0);
        assert_eq!(follow_up_results.len(), plan.follow_up_tasks.len());
        assert_eq!(follow_up_outputs.len(), plan.follow_up_tasks.len());
        let proof_result = follow_up_results
            .iter()
            .find(|result| result.task_id == proof_task.id)
            .ok_or_else(|| anyhow::anyhow!("proof follow-up result should be present"))?;
        let proof_output = follow_up_outputs
            .iter()
            .find(|output| output.task_id == proof_task.id)
            .ok_or_else(|| anyhow::anyhow!("proof follow-up output should be present"))?;
        assert_eq!(proof_result.schema, "ub-review.follow_up_result.v1");
        assert_eq!(proof_result.group_id, proof_task.group_id);
        assert_eq!(proof_result.stage, proof_task.stage);
        assert_eq!(proof_result.disposition, proof_task.disposition);
        assert_eq!(proof_result.evidence_need, proof_task.evidence_need);
        assert_eq!(proof_result.candidate_ids, proof_task.candidate_ids);
        assert_eq!(
            proof_result.observation_group_ids,
            proof_task.observation_group_ids
        );
        assert_eq!(
            proof_result.packet_path,
            format!("questions/orchestrator-follow-up/{}.json", proof_task.id)
        );
        assert_eq!(
            proof_result.model_lane,
            format!("orchestrator-follow-up-{}", proof_task.id)
        );
        assert_eq!(proof_result.status, "skipped");
        assert_eq!(
            proof_result.reason,
            "model-mode off; follow-up task remains artifact-only"
        );
        assert_eq!(proof_result.provider, "minimax");
        assert_eq!(proof_result.model, "MiniMax-M3");
        assert_eq!(proof_result.endpoint_kind, "anthropic-messages");
        assert!(proof_result.fallback_from.is_none());
        assert_eq!(proof_result.output_counts.observations, 0);
        assert_eq!(proof_result.output_counts.candidate_findings, 0);
        assert_eq!(proof_result.output_counts.summary_only_findings, 0);
        assert_eq!(proof_result.output_counts.failed_objections, 0);
        assert_eq!(proof_result.output_counts.proof_requests, 0);
        assert!(proof_result.request_path.is_none());
        assert!(proof_result.response_path.is_none());
        assert!(proof_result.content_path.is_none());
        assert!(proof_result.stderr_path.is_none());
        assert_eq!(proof_output.schema, "ub-review.follow_up_output.v1");
        assert_eq!(proof_output.group_id, proof_task.group_id);
        assert_eq!(proof_output.stage, proof_task.stage);
        assert_eq!(proof_output.disposition, proof_task.disposition);
        assert_eq!(proof_output.evidence_need, proof_task.evidence_need);
        assert_eq!(proof_output.candidate_ids, proof_task.candidate_ids);
        assert_eq!(
            proof_output.observation_group_ids,
            proof_task.observation_group_ids
        );
        assert_eq!(proof_output.status, "skipped");
        assert!(proof_output.inline_comments.is_empty());
        assert!(proof_output.summary_only_findings.is_empty());
        assert!(proof_output.observations.is_empty());
        assert!(proof_output.proof_requests.is_empty());

        super::write_follow_up_result_artifacts(temp.path(), &follow_up_results)?;
        super::write_follow_up_output_artifacts(temp.path(), &follow_up_outputs)?;
        let written_results: serde_json::Value =
            serde_json::from_slice(&fs::read(review_dir.join("follow_up_results.json"))?)?;
        let written_outputs: serde_json::Value =
            serde_json::from_slice(&fs::read(review_dir.join("follow_up_outputs.json"))?)?;
        let written_result_lines =
            fs::read_to_string(temp.path().join("follow_up_results.ndjson"))?;
        let written_output_lines =
            fs::read_to_string(temp.path().join("follow_up_outputs.ndjson"))?;
        let written_result_ndjson = written_result_lines
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let written_output_ndjson = written_output_lines
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(
            written_results.as_array().map(Vec::len),
            Some(plan.follow_up_tasks.len())
        );
        assert_eq!(
            written_outputs.as_array().map(Vec::len),
            Some(plan.follow_up_tasks.len())
        );
        assert_eq!(
            written_result_ndjson,
            written_results.as_array().cloned().unwrap_or_default()
        );
        assert_eq!(
            written_output_ndjson,
            written_outputs.as_array().cloned().unwrap_or_default()
        );
        assert!(
            written_results
                .as_array()
                .is_some_and(
                    |results| results.iter().any(|result| result["task_id"].as_str()
                        == Some(proof_task.id.as_str())
                        && result["status"].as_str() == Some("skipped")
                        && result.get("request_path").is_none())
                )
        );
        assert!(
            written_outputs
                .as_array()
                .is_some_and(
                    |outputs| outputs.iter().any(|output| output["task_id"].as_str()
                        == Some(proof_task.id.as_str())
                        && output["status"].as_str() == Some("skipped")
                        && output["proof_requests"]
                            .as_array()
                            .is_some_and(Vec::is_empty))
                )
        );
        Ok(())
    }

    #[test]
    fn model_stage_records_cover_primary_refuter_and_follow_ups() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = test_run_args(temp.path().join("out"));
        let mut planner = model_lane_receipt("proof-planner", "ok");
        planner.reason = "planned advisory proof-planner lane for intelligent-ci".to_owned();
        let mut refuter = model_lane_receipt("refuter", "ok");
        refuter.reason = "completed".to_owned();
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok"), planner, refuter];
        let mut secondary = test_follow_up_result("follow-secondary", "group-secondary", "ok");
        secondary.provider = "opencode-go".to_owned();
        secondary.model = "mimo-v2.5".to_owned();
        secondary.endpoint_kind = "anthropic-messages".to_owned();
        secondary.fallback_from = Some("minimax:MiniMax-M3:anthropic-messages".to_owned());
        secondary.duration_ms = Some(123);
        secondary.http_status = Some(200);
        secondary.response_shape = Some("anthropic".to_owned());
        secondary.cache_usage = super::ModelCacheUsage {
            input_tokens: Some(1000),
            output_tokens: Some(25),
            cache_creation_input_tokens: Some(900),
            cache_read_input_tokens: Some(100),
        };
        let mut tertiary = test_follow_up_result("follow-tertiary", "group-tertiary", "skipped");
        tertiary.stage = "tertiary".to_owned();
        let follow_up_results = vec![secondary, tertiary];

        let records = super::model_stage_records(&model_lanes, &follow_up_results, &args);
        assert_eq!(records.len(), 5);
        assert_eq!(records[0].lane, "tests-oracle");
        assert_eq!(records[0].source, "model-lane");
        assert_eq!(records[0].stage, "primary");
        assert!(records[0].task_id.is_none());
        assert_eq!(records[1].lane, "proof-planner");
        assert_eq!(records[1].source, "proof-planner");
        assert_eq!(records[1].stage, "primary");
        assert_eq!(records[2].lane, "refuter");
        assert_eq!(records[2].source, "refuter");
        assert_eq!(records[2].stage, "tertiary");
        assert_eq!(records[3].source, "orchestrator-follow-up");
        assert_eq!(records[3].stage, "secondary");
        assert_eq!(records[3].task_id.as_deref(), Some("follow-secondary"));
        assert_eq!(records[3].provider, "opencode-go");
        assert_eq!(records[3].model, "mimo-v2.5");
        assert_eq!(records[3].endpoint_kind, "anthropic-messages");
        assert_eq!(records[3].duration_ms, Some(123));
        assert_eq!(records[3].http_status, Some(200));
        assert_eq!(records[3].response_shape.as_deref(), Some("anthropic"));
        assert_eq!(records[3].cache_usage.input_tokens, Some(1000));
        assert_eq!(records[3].cache_usage.output_tokens, Some(25));
        assert_eq!(
            records[3].cache_usage.cache_creation_input_tokens,
            Some(900)
        );
        assert_eq!(records[3].cache_usage.cache_read_input_tokens, Some(100));
        assert_eq!(records[4].source, "orchestrator-follow-up");
        assert_eq!(records[4].stage, "tertiary");
        assert_eq!(records[4].task_id.as_deref(), Some("follow-tertiary"));

        super::write_model_stage_artifacts(temp.path(), &model_lanes, &follow_up_results, &args)?;
        let written: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("review/model_stages.json"))?)?;
        let lines = fs::read_to_string(temp.path().join("model_stages.ndjson"))?;
        let ndjson = lines
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(written.as_array().map(Vec::len), Some(records.len()));
        assert_eq!(written.as_array().cloned().unwrap_or_default(), ndjson);
        Ok(())
    }

    #[test]
    fn final_compiler_input_artifact_records_exact_final_sources() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
        let inline_comments = vec![ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            side: "RIGHT".to_owned(),
            body: "[tests-oracle] Confirm the focused proof covers the changed route.".to_owned(),
            evidence: "test evidence".to_owned(),
            suggestion: None,
        }];
        let summary_only_findings = vec![SummaryOnlyFinding {
            lane: "orchestrator-follow-up-follow-secondary".to_owned(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason: "Follow-up narrowed the remaining proof question.".to_owned(),
            evidence: "follow-up evidence".to_owned(),
        }];
        let proof_receipts = Vec::new();
        super::write_final_compiler_input_artifact(
            temp.path(),
            super::FinalCompilerInputArtifact {
                schema: "ub-review.final_compiler_input.v2",
                phase: "final",
                source_artifacts: &[
                    "review/review.json",
                    "review/follow_up_evidence.json",
                    "review/resolved_candidates.json",
                    "review/prior_resolved_candidates.json",
                    "review/proof_receipts.json",
                    "review/tool-gate-outcomes.json",
                    "review/receipt_routes.json",
                    "review/final_orchestrator_plan.json",
                    "review/claim_graph.json",
                    "review/pr_thread_context.json",
                    "review/proof_intents.json",
                ],
                model_lanes: &model_lanes,
                missing_or_failed_sensor_evidence: &[],
                missing_or_failed_model_evidence: &[],
                follow_up_resolved_candidate_ids: &["candidate-0001-deadbeef0123".to_owned()],
                inline_comments: &inline_comments,
                summary_only_findings: &summary_only_findings,
                observations: &[],
                proof_receipts: &proof_receipts,
            },
        )?;
        let written: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/final_compiler_input.json"),
        )?)?;
        assert_eq!(written["schema"], "ub-review.final_compiler_input.v2");
        assert_eq!(written["phase"], "final");
        assert_eq!(written["model_lanes"][0]["lane"], "tests-oracle");
        assert_eq!(
            written["inline_comments"][0]["body"],
            "[tests-oracle] Confirm the focused proof covers the changed route."
        );
        assert_eq!(
            written["summary_only_findings"][0]["reason"],
            "Follow-up narrowed the remaining proof question."
        );
        assert_eq!(
            written["source_artifacts"],
            serde_json::json!([
                "review/review.json",
                "review/follow_up_evidence.json",
                "review/resolved_candidates.json",
                "review/prior_resolved_candidates.json",
                "review/proof_receipts.json",
                "review/tool-gate-outcomes.json",
                "review/receipt_routes.json",
                "review/final_orchestrator_plan.json",
                "review/claim_graph.json",
                "review/pr_thread_context.json",
                "review/proof_intents.json"
            ])
        );
        assert_eq!(
            written["follow_up_resolved_candidate_ids"],
            serde_json::json!(["candidate-0001-deadbeef0123"])
        );
        Ok(())
    }

    #[test]
    fn final_orchestrator_plan_can_route_late_follow_up_receipts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let candidates = vec![super::CandidateRecord {
            schema: "ub-review.candidate.v1".to_owned(),
            id: "candidate-proof".to_owned(),
            lane: "tests-oracle".to_owned(),
            source: "summary-only-finding".to_owned(),
            status: "summary-only".to_owned(),
            disposition: "summary-only".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium".to_owned(),
            claim: "Needs red/green proof before upstream.".to_owned(),
            evidence: "proof request from tests lane".to_owned(),
            path: None,
            line: None,
            side: None,
        }];
        let observations = Vec::new();
        let initial_plan = build_orchestrator_plan(&candidates, &observations, &[], &[], &[]);
        write_orchestrator_artifacts(temp.path(), &initial_plan, &[])?;
        let mut late_receipt = test_red_green_proof_receipt("discriminating", "failed");
        late_receipt.id = "proof-follow-up-late".to_owned();
        late_receipt.requested_by = vec!["tests-oracle".to_owned()];
        late_receipt.request_ids = vec!["proof-follow-up-1".to_owned()];
        late_receipt.reason = "HEAD passed; base+tests failed after follow-up proof.".to_owned();
        let late_lease = ResourceLease {
            schema: "ub-review.resource_lease.v1".to_owned(),
            id: "lease-proof-follow-up-late".to_owned(),
            kind: "focused-test".to_owned(),
            consumer: "proof-follow-up-late".to_owned(),
            status: "granted".to_owned(),
            reason: "follow-up proof lease granted".to_owned(),
            cpu: 2,
            memory_mb: 2_048,
            disk_mb: 1_024,
            timeout_sec: 600,
            network: false,
            scratch: true,
            worktree: Some("base-plus-tests".to_owned()),
            command: Some("bun test test/js/bun/md/md-edge-cases.test.ts".to_owned()),
        };

        let final_plan = build_final_orchestrator_plan(
            &candidates,
            &observations,
            &[late_receipt],
            &[late_lease],
            &[],
        );
        write_final_orchestrator_artifact(temp.path(), &final_plan)?;

        let initial_written: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/orchestrator_plan.json"),
        )?)?;
        let final_written: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/final_orchestrator_plan.json"),
        )?)?;
        assert!(
            initial_written["evidence_groups"][0]["routed_evidence"]
                .as_array()
                .is_some_and(Vec::is_empty)
        );
        assert_eq!(initial_written["follow_up_tasks"][0]["stage"], "secondary");
        assert_eq!(
            final_written["evidence_groups"][0]["routed_evidence"][0]["id"],
            "proof-follow-up-late"
        );
        assert!(
            final_written["follow_up_tasks"]
                .as_array()
                .is_some_and(Vec::is_empty),
            "final routed proof should resolve the proof-confirmation follow-up task"
        );
        Ok(())
    }

    #[test]
    fn follow_up_proof_receipts_route_reconsideration_messages() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        fs::create_dir_all(&review_dir)?;
        let message_log = crate::MessageLog::open(&review_dir.join("messages.ndjson"))?;
        let event_log = EventLog::open(&temp.path().join("events.ndjson"))?;
        let mut receipt = test_proof_receipt("discriminating", "passed");
        receipt.id = "proof-follow-up-reconsideration".to_owned();
        receipt.kind = "focused-head".to_owned();
        receipt.requested_by = vec!["proof-broker".to_owned(), "tests-oracle".to_owned()];
        receipt.request_ids = vec!["follow-up-proof-1".to_owned()];

        super::route_follow_up_proof_receipts(&message_log, &event_log, &[receipt]);

        let messages = crate::read_messages_ndjson(&review_dir);
        let destinations = messages
            .iter()
            .map(|message| message.to.as_str())
            .collect::<BTreeSet<_>>();
        assert_eq!(messages.len(), 3);
        assert!(destinations.contains("tests-oracle"));
        assert!(destinations.contains("opposition"));
        assert!(destinations.contains("compiler"));
        assert!(messages.iter().all(|message| {
            message.kind == crate::CrossLaneMessageKind::EvidenceRouted
                && message.payload["reconsider"] == serde_json::json!(true)
                && message.references
                    == ["review/proof_receipts.json#proof-follow-up-reconsideration"]
        }));
        Ok(())
    }

    #[test]
    fn final_orchestrator_plan_suppresses_test_oracle_task_when_late_proof_routes() -> Result<()> {
        let candidates = vec![super::CandidateRecord {
            schema: "ub-review.candidate.v1".to_owned(),
            id: "candidate-test-oracle".to_owned(),
            lane: "tests-oracle".to_owned(),
            source: "summary-only-finding".to_owned(),
            status: "summary-only".to_owned(),
            disposition: "summary-only".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium".to_owned(),
            claim: "The changed test oracle may still be too weak for this behavior.".to_owned(),
            evidence: "tests-oracle follow-up candidate".to_owned(),
            path: None,
            line: None,
            side: None,
        }];
        let observations = Vec::new();
        let initial_plan = build_orchestrator_plan(&candidates, &observations, &[], &[], &[]);
        assert_eq!(initial_plan.follow_up_tasks.len(), 1);
        assert_eq!(
            initial_plan.follow_up_tasks[0].evidence_need,
            "test-oracle-confirmation"
        );

        let mut late_receipt = test_red_green_proof_receipt("discriminating", "failed");
        late_receipt.id = "proof-test-oracle-late".to_owned();
        late_receipt.requested_by = vec!["tests-oracle".to_owned()];
        late_receipt.request_ids = vec!["proof-test-oracle-1".to_owned()];
        late_receipt.reason = "Routed proof confirms the changed test oracle.".to_owned();

        let final_plan =
            build_final_orchestrator_plan(&candidates, &observations, &[late_receipt], &[], &[]);

        assert_eq!(
            final_plan.evidence_groups[0].routed_evidence[0].status,
            "tool-confirmed"
        );
        assert!(
            final_plan.follow_up_tasks.is_empty(),
            "late routed proof should resolve test-oracle follow-up tasks"
        );
        Ok(())
    }

    #[test]
    fn final_orchestrator_plan_suppresses_source_route_task_when_late_proof_routes() -> Result<()> {
        let observation = test_observation(
            "source-route",
            "The changed helper route might bypass the scalar write path.",
            "source-route-gap",
            "confirmed",
            "medium",
            "high",
            "filehandle-write-route",
        );
        let observations = observation_summary_artifacts(&[observation]).unique;
        let initial_plan = build_orchestrator_plan(&[], &observations, &[], &[], &[]);
        assert_eq!(initial_plan.follow_up_tasks.len(), 1);
        assert_eq!(
            initial_plan.follow_up_tasks[0].evidence_need,
            "source-route-confirmation"
        );
        assert!(
            initial_plan.observation_groups[0]
                .routed_evidence
                .is_empty()
        );

        let mut late_receipt = test_red_green_proof_receipt("discriminating", "failed");
        late_receipt.id = "proof-source-route-late".to_owned();
        late_receipt.requested_by = vec!["source-route".to_owned()];
        late_receipt.request_ids = vec!["proof-source-route-1".to_owned()];
        late_receipt.reason =
            "Routed proof confirms the changed helper reaches the patched path.".to_owned();

        let final_plan =
            build_final_orchestrator_plan(&[], &observations, &[late_receipt], &[], &[]);

        assert_eq!(
            final_plan.observation_groups[0].routed_evidence[0].id,
            "proof-source-route-late"
        );
        assert_eq!(
            final_plan.observation_groups[0].routed_evidence[0].status,
            "tool-confirmed"
        );
        assert!(
            final_plan.follow_up_tasks.is_empty(),
            "late routed proof should resolve source-route follow-up tasks"
        );
        Ok(())
    }

    #[test]
    fn final_orchestrator_plan_routes_evaluated_tool_gate_to_relevant_lanes() -> Result<()> {
        let candidates = vec![super::CandidateRecord {
            schema: "ub-review.candidate.v1".to_owned(),
            id: "candidate-test-oracle-ripr".to_owned(),
            lane: "tests-oracle".to_owned(),
            source: "summary-only-finding".to_owned(),
            status: "summary-only".to_owned(),
            disposition: "summary-only".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium".to_owned(),
            claim: "The changed test oracle needs the ripr receipt before promotion.".to_owned(),
            evidence: "test oracle lane asked for static mutation-exposure signal".to_owned(),
            path: None,
            line: None,
            side: None,
        }];
        let unrelated_observation = test_observation(
            "security",
            "The changed test oracle still needs a security-only review.",
            "test-gap",
            "open",
            "medium",
            "medium",
            "security-only-test-gap",
        );
        let observations = observation_summary_artifacts(&[unrelated_observation]).unique;
        let ripr_outcome = ToolGateOutcomeEntry {
            schema: TOOL_GATE_OUTCOME_SCHEMA,
            tool: "ripr".to_owned(),
            policy: ToolGatePolicy {
                scope: Some("on-diff".to_owned()),
                max_new_unsuppressed: Some(0),
            },
            required: false,
            planned_run: true,
            sensor_status: "ok".to_owned(),
            sensor_reason: "ripr completed".to_owned(),
            sensor_receipt_path: "sensors/ripr/ub-review-sensor-status.json".to_owned(),
            status_source: "tool-status.json",
            outcome: "passed".to_owned(),
            evaluated: true,
            reason: "ripr threshold passed".to_owned(),
            metrics: ToolGateOutcomeMetrics {
                new_unsuppressed: Some(0),
            },
            source_artifacts: vec![
                "sensors/ripr/ub-review-sensor-status.json".to_owned(),
                "tool-status.json".to_owned(),
                "sensors/ripr/gate-decision.json".to_owned(),
                "sensors/ripr/exposure-gaps.json".to_owned(),
            ],
            packet_policy: "gate-only",
            gate_policy: "trust-affecting",
        };

        let final_plan =
            build_final_orchestrator_plan(&candidates, &observations, &[], &[], &[ripr_outcome]);

        let candidate_group = final_plan
            .evidence_groups
            .iter()
            .find(|group| {
                group
                    .candidate_ids
                    .iter()
                    .map(String::as_str)
                    .eq(["candidate-test-oracle-ripr"])
            })
            .ok_or_else(|| anyhow::anyhow!("candidate group should be present"))?;
        assert_eq!(candidate_group.evidence_need, "test-oracle-confirmation");
        assert_eq!(candidate_group.routed_evidence.len(), 1);
        let routed = &candidate_group.routed_evidence[0];
        assert_eq!(routed.kind, "tool-gate-outcome");
        assert_eq!(routed.id, "ripr");
        assert_eq!(routed.artifact, "review/tool-gate-outcomes.json#ripr");
        assert_eq!(routed.status, "tool-gate-passed");
        assert_eq!(routed.result, "passed");
        assert!(routed.reason.contains("new_unsuppressed=0"));
        assert!(routed.reason.contains("sensors/ripr/exposure-gaps.json"));

        let candidate_task = final_plan
            .follow_up_tasks
            .iter()
            .find(|task| task.group_id == candidate_group.id)
            .ok_or_else(|| anyhow::anyhow!("candidate follow-up task should remain"))?;
        assert_eq!(candidate_task.stage, "tertiary");
        assert_eq!(
            serde_json::to_value(&candidate_task.routed_evidence)?,
            serde_json::to_value(&candidate_group.routed_evidence)?
        );

        let observation_group = final_plan
            .observation_groups
            .iter()
            .find(|group| group.lanes.iter().map(String::as_str).eq(["security"]))
            .ok_or_else(|| anyhow::anyhow!("security observation group should be present"))?;
        assert!(
            observation_group.routed_evidence.is_empty(),
            "ripr should not route to a lane that does not receive ripr"
        );
        let observation_task = final_plan
            .follow_up_tasks
            .iter()
            .find(|task| task.group_id == observation_group.id)
            .ok_or_else(|| anyhow::anyhow!("security observation follow-up should remain"))?;
        assert_eq!(observation_task.stage, "secondary");
        assert!(observation_task.routed_evidence.is_empty());
        Ok(())
    }

    #[test]
    fn follow_up_outputs_preserve_validated_model_content() -> Result<()> {
        let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
";
        let line_map = right_side_diff_lines(patch);
        let task = FollowUpQuestionTask {
            schema: "ub-review.follow_up_question.v1".to_owned(),
            id: "follow-up-route-proof".to_owned(),
            group_id: "orchestrator-observation-0000".to_owned(),
            stage: "secondary".to_owned(),
            stage_reason: "no routed proof receipt is available; ask for the smallest remaining evidence or proof request".to_owned(),
            evidence_need: "proof-confirmation".to_owned(),
            disposition: "observation".to_owned(),
            candidate_ids: Vec::new(),
            observation_group_ids: vec!["observation-group-0000".to_owned()],
            routed_evidence: Vec::new(),
            question: "Confirm whether routed proof resolves the remaining route question."
                .to_owned(),
            status: "planned".to_owned(),
            reason: "test follow-up task".to_owned(),
        };
        let model_lane = follow_up_model_lane_id(&task);
        let output: LaneModelOutput = serde_json::from_str(
            r#"{
  "observations": [
    {
      "claim": "The routed proof confirms the changed helper reaches the scalar write path.",
      "question": "source-route",
      "kind": "source-route-gap",
      "status": "confirmed",
      "severity": "medium",
      "confidence": "high",
      "evidence": ["routed proof receipt"],
      "dedupe_key": "filehandle-write-route"
    }
  ],
  "candidate_findings": [
    {
      "severity": "medium",
      "confidence": "high",
      "path": "src/lib.rs",
      "line": 2,
      "body": "[orchestrator] Follow-up kept a line-valid candidate as structured evidence.",
      "evidence": "RIGHT-side line map"
    }
  ],
  "summary_only_findings": [
    {
      "severity": "low",
      "confidence": "medium",
      "reason": "Routed evidence narrowed the remaining route check to one helper.",
      "evidence": "routed evidence packet"
    }
  ],
  "failed_objections": [
    {
      "claim": "The sibling helper bypasses the patched path.",
      "reason": "routed proof shows the helper reaches the patched path",
      "confidence": "high",
      "kind": "false-premise",
      "evidence": ["source route receipt"]
    }
  ],
  "proof_requests": [
    {
      "command": "bun test test/js/bun/fs/fs.write.test.ts -t route",
      "reason": "Need a focused route witness",
      "cost": "focused-test",
      "timeout_sec": 300,
      "required": false
    }
  ]
}"#,
        )?;

        let record =
            follow_up_output_record(&task, &model_lane, "ok", "completed", output, &line_map);

        assert_eq!(record.schema, "ub-review.follow_up_output.v1");
        assert_eq!(record.task_id, task.id);
        assert_eq!(record.stage, "secondary");
        assert_eq!(record.disposition, task.disposition);
        assert_eq!(record.evidence_need, task.evidence_need);
        assert_eq!(record.candidate_ids, task.candidate_ids);
        assert_eq!(record.observation_group_ids, task.observation_group_ids);
        assert_eq!(record.model_lane, model_lane);
        assert_eq!(record.inline_comments.len(), 1);
        assert_eq!(record.inline_comments[0].lane, record.model_lane);
        assert_eq!(record.inline_comments[0].side, "RIGHT");
        assert_eq!(record.summary_only_findings.len(), 1);
        assert_eq!(record.summary_only_findings[0].lane, record.model_lane);
        assert_eq!(record.observations.len(), 2);
        assert!(record.observations.iter().any(|observation| {
            observation.source == "model-observation"
                && observation.dedupe_key == "filehandle-write-route"
        }));
        assert!(record.observations.iter().any(|observation| {
            observation.source == "model-failed-objection"
                && observation.status == "refuted"
                && observation.kind == "false-premise"
        }));
        assert_eq!(record.proof_requests.len(), 1);
        assert_eq!(
            record.proof_requests[0].requested_by,
            vec![record.model_lane.clone()]
        );

        let temp = tempfile::tempdir()?;
        let outputs = vec![record];
        let evidence = follow_up_evidence_from_outputs(&outputs);
        assert_eq!(evidence.schema, "ub-review.follow_up_evidence.v1");
        assert_eq!(evidence.follow_up_outputs, 1);
        assert_eq!(evidence.inline_comments.len(), 1);
        assert_eq!(evidence.summary_only_findings.len(), 1);
        assert_eq!(evidence.observations.len(), 2);
        assert_eq!(evidence.proof_requests.len(), 1);
        let mut canonical_proof_requests = Vec::new();
        append_follow_up_proof_requests(&mut canonical_proof_requests, &evidence);
        assert_eq!(canonical_proof_requests.len(), 1);
        assert_eq!(canonical_proof_requests[0].status, "requested");
        assert_eq!(
            canonical_proof_requests[0].requested_by,
            vec![model_lane.clone()]
        );
        assert!(
            canonical_proof_requests[0]
                .reason
                .contains("routed to the follow-up broker scheduling pass")
        );
        assert!(
            !canonical_proof_requests[0]
                .reason
                .contains("retained for next broker scheduling pass")
        );
        append_follow_up_proof_requests(&mut canonical_proof_requests, &evidence);
        assert_eq!(canonical_proof_requests.len(), 1);
        let mut witnesses = Vec::new();
        append_follow_up_evidence_witnesses(&mut witnesses, &evidence, &[]);
        assert_eq!(witnesses.len(), 5);
        assert!(witnesses.iter().any(|witness| {
            witness.source == "follow-up-inline-comment"
                && witness.kind == "inline-finding"
                && witness.status == "needs-witness"
        }));
        assert!(witnesses.iter().any(|witness| {
            witness.source == "follow-up-summary-only-finding"
                && witness.kind == "summary-finding"
                && witness.status == "needs-witness"
        }));
        assert!(witnesses.iter().any(|witness| {
            witness.source == "follow-up-model-observation"
                && witness.dedupe_key == "follow-up-observation:filehandle-write-route"
                && witness.status == "tool-confirmed"
        }));
        assert!(witnesses.iter().any(|witness| {
            witness.source == "follow-up-model-failed-objection"
                && witness.kind == "false-premise"
                && witness.status == "refuted"
        }));
        assert!(witnesses.iter().any(|witness| {
            witness.source == "follow-up-proof-request"
                && witness.kind == "proof-request"
                && witness.status == "needs-witness"
                && witness
                    .evidence
                    .iter()
                    .any(|item| item.contains("bun test test/js/bun/fs/fs.write.test.ts"))
        }));

        let mut receipt = test_red_green_proof_receipt("discriminating", "failed");
        receipt.request_ids = vec![evidence.proof_requests[0].id.clone()];
        let mut linked_witnesses = Vec::new();
        append_follow_up_evidence_witnesses(&mut linked_witnesses, &evidence, &[receipt.clone()]);
        let linked_request_witness = linked_witnesses
            .iter()
            .find(|witness| witness.source == "follow-up-proof-request")
            .ok_or_else(|| anyhow::anyhow!("missing linked follow-up proof request witness"))?;
        assert_eq!(linked_request_witness.status, "tool-confirmed");
        assert_eq!(
            linked_request_witness.proof_receipt_id.as_deref(),
            Some(receipt.id.as_str())
        );
        assert!(linked_request_witness.evidence.iter().any(|item| {
            item.contains("Follow-up proof request")
                && item.contains("bun test test/js/bun/fs/fs.write.test.ts")
        }));
        assert!(
            linked_request_witness
                .evidence
                .iter()
                .any(|item| item.contains("base-plus-tests"))
        );

        write_follow_up_output_artifacts(temp.path(), &outputs)?;
        write_follow_up_evidence_artifact(temp.path(), &evidence)?;
        write_witness_artifacts(temp.path(), &witnesses)?;
        write_proof_request_artifacts(
            temp.path(),
            &test_diff(),
            &Profile::default(),
            &canonical_proof_requests,
            &[] as &[ProofReceipt],
            &[] as &[ResourceLease],
        )?;
        let written: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/follow_up_outputs.json"),
        )?)?;
        let written_evidence: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/follow_up_evidence.json"),
        )?)?;
        let lines = fs::read_to_string(temp.path().join("follow_up_outputs.ndjson"))?;
        let ndjson = lines
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(written.as_array().map(Vec::len), Some(1));
        assert_eq!(ndjson, written.as_array().cloned().unwrap_or_default());
        assert_eq!(written_evidence["follow_up_outputs"], 1);
        assert_eq!(
            written_evidence["observations"].as_array().map(Vec::len),
            Some(2)
        );
        assert_eq!(
            written_evidence["proof_requests"].as_array().map(Vec::len),
            Some(1)
        );
        let proof_json: Vec<super::ProofRequest> =
            serde_json::from_slice(&fs::read(temp.path().join("review/proof_requests.json"))?)?;
        assert_eq!(
            serde_json::to_value(&proof_json)?,
            serde_json::to_value(terminalize_proof_requests(&canonical_proof_requests, &[]))?
        );
        let proof_request_file: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path()
                .join("proof_requests")
                .join(format!("{}.json", canonical_proof_requests[0].id)),
        )?)?;
        assert_eq!(proof_request_file, serde_json::to_value(&proof_json[0])?);
        let proof_ndjson = fs::read_to_string(temp.path().join("proof_requests.ndjson"))?;
        assert_eq!(proof_ndjson.lines().count(), 1);
        assert!(proof_ndjson.contains("routed to the follow-up broker scheduling pass"));
        assert!(!proof_ndjson.contains("retained for next broker scheduling pass"));
        let witness_json: Vec<super::WitnessRecord> =
            serde_json::from_slice(&fs::read(temp.path().join("review/witnesses.json"))?)?;
        assert_eq!(witness_json.len(), 5);
        let registry: super::WitnessRegistryArtifact =
            serde_json::from_slice(&fs::read(temp.path().join("review/witness_registry.json"))?)?;
        assert_eq!(registry.schema, "ub-review.witness_registry.v1");
        assert_eq!(registry.total, 5);
        assert_eq!(registry.follow_up_total, 5);
        assert_eq!(registry.follow_up_status_counts["needs-witness"], 3);
        assert_eq!(registry.follow_up_status_counts["tool-confirmed"], 1);
        assert_eq!(registry.follow_up_status_counts["refuted"], 1);
        assert_eq!(
            registry.follow_up_witness_ids_by_status["needs-witness"].len(),
            3
        );
        Ok(())
    }

    #[test]
    fn resolved_candidate_records_capture_follow_up_dispositions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let candidates = vec![
            test_candidate_record("candidate-unchanged"),
            test_candidate_record("candidate-unavailable"),
            test_candidate_record("candidate-unresolved"),
            test_candidate_record("candidate-refuted"),
            test_candidate_record("candidate-parked"),
            test_candidate_record("candidate-dropped"),
            test_candidate_record("candidate-conflicting"),
            test_candidate_record("candidate-resolved-kind-open"),
            test_candidate_record("candidate-parked-kind-open"),
            test_candidate_record("candidate-polish-dropped"),
            test_candidate_record("candidate-covered-dropped"),
        ];
        let outputs = vec![
            test_follow_up_output_for_candidate(
                "follow-unavailable",
                &candidates[1].id,
                "skipped_budget",
                Vec::new(),
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-unresolved",
                &candidates[2].id,
                "ok",
                Vec::new(),
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-refuted",
                &candidates[3].id,
                "ok",
                vec![test_observation(
                    "orchestrator-follow-up-follow-refuted",
                    "The proof receipt resolves this candidate.",
                    "resolved-check",
                    "refuted",
                    "low",
                    "high",
                    "candidate-refuted",
                )],
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-parked",
                &candidates[4].id,
                "ok",
                vec![test_observation(
                    "orchestrator-follow-up-follow-parked",
                    "The sibling route is parked behind a helper.",
                    "parked-follow-up",
                    "parked",
                    "low",
                    "medium",
                    "candidate-parked",
                )],
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-dropped",
                &candidates[5].id,
                "ok",
                Vec::new(),
                vec![SummaryOnlyFinding {
                    lane: "orchestrator-follow-up-follow-dropped".to_owned(),
                    severity: "low".to_owned(),
                    confidence: "high".to_owned(),
                    reason: "duplicate inline candidate merged after tertiary review".to_owned(),
                    evidence: "duplicate evidence".to_owned(),
                }],
            ),
            test_follow_up_output_for_candidate(
                "follow-conflict-refuted",
                &candidates[6].id,
                "ok",
                vec![test_observation(
                    "orchestrator-follow-up-follow-conflict-refuted",
                    "One follow-up refutes this candidate.",
                    "false-premise",
                    "refuted",
                    "low",
                    "high",
                    "candidate-conflict-refuted",
                )],
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-conflict-parked",
                &candidates[6].id,
                "ok",
                vec![test_observation(
                    "orchestrator-follow-up-follow-conflict-parked",
                    "Another follow-up parks this candidate.",
                    "parked-follow-up",
                    "parked",
                    "low",
                    "medium",
                    "candidate-conflict-parked",
                )],
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-resolved-kind-open",
                &candidates[7].id,
                "ok",
                vec![test_observation(
                    "orchestrator-follow-up-follow-resolved-kind-open",
                    "The proof receipt may resolve this candidate, but status stayed open.",
                    "resolved-check",
                    "open",
                    "low",
                    "medium",
                    "candidate-resolved-kind-open",
                )],
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-parked-kind-open",
                &candidates[8].id,
                "ok",
                vec![test_observation(
                    "orchestrator-follow-up-follow-parked-kind-open",
                    "The sibling route may be parked, but status stayed open.",
                    "parked-follow-up",
                    "open",
                    "low",
                    "medium",
                    "candidate-parked-kind-open",
                )],
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-polish-dropped",
                &candidates[9].id,
                "ok",
                Vec::new(),
                vec![SummaryOnlyFinding {
                    lane: "orchestrator-follow-up-follow-polish-dropped".to_owned(),
                    severity: "low".to_owned(),
                    confidence: "medium".to_owned(),
                    reason: "Submaterial polish below materiality: also assert pointer identity."
                        .to_owned(),
                    evidence: "The test is already red/green-correct; this is a polish suggestion."
                        .to_owned(),
                }],
            ),
            test_follow_up_output_for_candidate(
                "follow-covered-dropped",
                &candidates[10].id,
                "ok",
                vec![test_observation(
                    "orchestrator-follow-up-follow-covered-dropped",
                    "The seeded thread and routed proof already cover this concern.",
                    "resolved-check",
                    "covered",
                    "low",
                    "high",
                    "candidate-covered-dropped",
                )],
                Vec::new(),
            ),
        ];
        let results = outputs
            .iter()
            .map(|output| {
                let mut result =
                    test_follow_up_result(&output.task_id, &output.group_id, &output.status);
                result.stage = output.stage.clone();
                result.disposition = output.disposition.clone();
                result.evidence_need = output.evidence_need.clone();
                result.candidate_ids = output.candidate_ids.clone();
                result.observation_group_ids = output.observation_group_ids.clone();
                result
            })
            .collect::<Vec<_>>();

        let records = resolved_candidate_records(&candidates, &results, &outputs, &[]);
        assert_eq!(records.len(), candidates.len());
        assert_eq!(records[0].resolved_status, "unchanged");
        assert_eq!(records[0].resolution_source, "candidate");
        assert_eq!(records[1].resolved_status, "follow-up-unavailable");
        assert_eq!(records[2].resolved_status, "unresolved");
        assert_eq!(records[3].resolved_status, "resolved");
        assert_eq!(records[3].resolved_disposition, "refuted");
        assert_eq!(records[4].resolved_status, "resolved");
        assert_eq!(records[4].resolved_disposition, "parked-follow-up");
        assert_eq!(records[5].resolved_status, "resolved");
        assert_eq!(records[5].resolved_disposition, "dropped");
        assert_eq!(records[6].resolved_status, "conflicting");
        assert_eq!(records[6].resolved_disposition, "summary-only");
        assert_eq!(records[7].resolved_status, "unresolved");
        assert_eq!(records[7].resolved_disposition, "summary-only");
        assert_eq!(records[8].resolved_status, "unresolved");
        assert_eq!(records[8].resolved_disposition, "summary-only");
        assert_eq!(records[9].resolved_status, "resolved");
        assert_eq!(records[9].resolved_disposition, "dropped");
        assert_eq!(records[10].resolved_status, "resolved");
        assert_eq!(records[10].resolved_disposition, "dropped");
        assert_eq!(
            records[6].source_artifacts,
            vec![
                "review/candidates.json".to_owned(),
                "review/follow_up_results.json".to_owned(),
                "review/follow_up_outputs.json".to_owned()
            ]
        );

        write_resolved_candidate_artifacts(temp.path(), &records)?;
        let written: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/resolved_candidates.json"),
        )?)?;
        let lines = fs::read_to_string(temp.path().join("resolved_candidates.ndjson"))?;
        let ndjson = lines
            .lines()
            .map(serde_json::from_str::<serde_json::Value>)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        assert_eq!(written.as_array().map(Vec::len), Some(candidates.len()));
        assert_eq!(written.as_array().cloned().unwrap_or_default(), ndjson);
        Ok(())
    }

    #[test]
    fn suggested_follow_up_renders_in_pr_body_and_never_blocks() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let diff = test_diff();
        let model_lanes = vec![model_lane_receipt("tests-red-green", "ok")];
        let suggested = vec![IssueCandidate {
            id: "issue-candidate-000-abc".to_owned(),
            source: "tests-red-green".to_owned(),
            target_repo: "EffortlessMetrics/ub-review".to_owned(),
            kind: "test-gap".to_owned(),
            confidence: "high".to_owned(),
            title: "Track base+tests red/green for focused proof requests".to_owned(),
            problem: "p".to_owned(),
            why_not_this_pr: "This PR adds HEAD receipts; base+tests needs a separate                               worktree and test-only patch path."
                .to_owned(),
            evidence: vec![IssueCandidateEvidence {
                kind: "artifact".to_owned(),
                path: Some("review/proof_plan.md".to_owned()),
                url: None,
            }],
            implementation_plan: vec!["step".to_owned()],
            acceptance: vec!["done".to_owned()],
            ..IssueCandidate::default()
        }];
        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: super::RunPass::Manual,
            post_review_on: &[],
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[],
            summary_only_findings: &[],
            observations: &[],
            proof_receipts: &[],
            suggested_issues: &suggested,
            final_follow_up_tasks: 0,
            reporter_distillation: None,
        })?;
        assert!(
            surface
                .github_review
                .body
                .contains("## Suggested follow-up"),
            "suggested section must render: {}",
            surface.github_review.body
        );
        assert!(
            surface
                .github_review
                .body
                .contains("Track base+tests red/green")
        );
        // Never blocks: the suggested follow-up changes the body, not the
        // terminal state class.
        assert_ne!(surface.terminal_state.status, "failed-to-review");
        Ok(())
    }

    #[test]
    fn issue_candidates_classify_validate_and_dedupe() -> Result<()> {
        fn valid_candidate(title: &str) -> IssueCandidate {
            IssueCandidate {
                source: "tests-red-green".to_owned(),
                target_repo: "EffortlessMetrics/ub-review".to_owned(),
                kind: "test-gap".to_owned(),
                confidence: "high".to_owned(),
                title: title.to_owned(),
                problem: "base+tests discrimination remains unimplemented".to_owned(),
                why_not_this_pr: "needs worktree setup and separate receipt states".to_owned(),
                evidence: vec![IssueCandidateEvidence {
                    kind: "artifact".to_owned(),
                    path: Some("review/proof_plan.md".to_owned()),
                    url: None,
                }],
                implementation_plan: vec!["create base worktree".to_owned()],
                acceptance: vec!["discriminating receipt renders".to_owned()],
                ..IssueCandidate::default()
            }
        }
        let raw = vec![
            valid_candidate("Run base+tests red/green"),
            // Same repo/kind, title differing only in case/punctuation:
            // a duplicate by fingerprint.
            valid_candidate("run BASE+TESTS red/green!"),
            // Below the bar: no implementation plan.
            IssueCandidate {
                implementation_plan: Vec::new(),
                ..valid_candidate("Different follow-up entirely")
            },
            // Disallowed kind.
            IssueCandidate {
                kind: "security-downgrade".to_owned(),
                ..valid_candidate("Sensitive thing")
            },
        ];
        let issues = super::IssuesConfig::default();
        let (candidates, actions) = classify_issue_candidates(&issues, raw);
        assert_eq!(candidates.len(), 4);
        assert_eq!(actions.len(), 4);
        // Default posture is suggest: a valid high-confidence do-not-block
        // candidate promotes to suggested (and never blocks).
        assert_eq!(actions[0].action, "suggested");
        assert_eq!(actions[1].action, "duplicate");
        assert_eq!(
            actions[1].existing.as_deref(),
            Some(candidates[0].id.as_str())
        );
        assert_eq!(actions[2].action, "invalid");
        assert!(actions[2].reason.contains("implementation_plan"));
        assert_eq!(actions[3].action, "invalid");
        assert!(actions[3].reason.contains("not in the allowed set"));
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.id.starts_with("issue-candidate-"))
        );

        let temp = tempfile::tempdir()?;
        write_issue_capture_artifacts(temp.path(), &candidates, &actions)?;
        let written: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(temp.path().join("review/issue_candidates.json"))?)?;
        assert_eq!(written.len(), 4);
        let ndjson = fs::read_to_string(temp.path().join("issue_actions.ndjson"))?;
        assert_eq!(ndjson.lines().count(), 4);
        let drafts = fs::read_to_string(temp.path().join("review/suggested_issues.md"))?;
        assert!(drafts.contains("Run base+tests red/green"));
        assert!(
            !drafts.contains("Different follow-up entirely"),
            "invalid candidates must not render as drafts"
        );
        assert!(
            drafts.contains("artifact-only"),
            "v0 posture stated in the drafts header"
        );

        // mode = off keeps everything artifact-only.
        let off = super::IssuesConfig {
            mode: "off".to_owned(),
            ..super::IssuesConfig::default()
        };
        let (_, off_actions) =
            classify_issue_candidates(&off, vec![valid_candidate("Run base+tests red/green")]);
        assert_eq!(off_actions[0].action, "artifact-only");

        // Medium confidence stays artifact-only even under suggest.
        let medium = IssueCandidate {
            confidence: "medium".to_owned(),
            ..valid_candidate("Medium confidence follow-up")
        };
        let (_, medium_actions) = classify_issue_candidates(&issues, vec![medium]);
        assert_eq!(medium_actions[0].action, "artifact-only");
        Ok(())
    }

    fn broker_test_candidate(title: &str, target_repo: &str) -> IssueCandidate {
        IssueCandidate {
            source: "tests-red-green".to_owned(),
            target_repo: target_repo.to_owned(),
            kind: "test-gap".to_owned(),
            confidence: "high".to_owned(),
            title: title.to_owned(),
            problem: "base+tests discrimination remains unimplemented".to_owned(),
            why_not_this_pr: "needs worktree setup and separate receipt states".to_owned(),
            evidence: vec![IssueCandidateEvidence {
                kind: "artifact".to_owned(),
                path: Some("review/proof_plan.md".to_owned()),
                url: None,
            }],
            implementation_plan: vec!["create base worktree".to_owned()],
            acceptance: vec!["discriminating receipt renders".to_owned()],
            labels: vec!["ub-review".to_owned()],
            ..IssueCandidate::default()
        }
    }

    #[test]
    fn issue_broker_plan_gates_on_mode_allowlist_and_cap() {
        let issues = super::IssuesConfig {
            mode: "open-high-confidence".to_owned(),
            open_in: vec!["EffortlessMetrics/ripr-swarm".to_owned()],
            open_cap: 1,
            ..super::IssuesConfig::default()
        };
        let raw = vec![
            broker_test_candidate("Allowlisted follow-up", "EffortlessMetrics/ripr-swarm"),
            broker_test_candidate("Not allowlisted", "EffortlessMetrics/tokmd-swarm"),
            broker_test_candidate("Over the cap", "EffortlessMetrics/ripr-swarm"),
            broker_test_candidate("Bad slug", "not-a-slug"),
        ];
        let (candidates, actions) = classify_issue_candidates(&issues, raw);
        let plan = build_issue_broker_plan(
            &issues,
            &candidates,
            &actions,
            Some("EffortlessMetrics/ub-review"),
            Some(346),
        );
        // Every suggested candidate appears in the plan; nothing is silent.
        assert_eq!(plan.len(), 4);
        assert_eq!(plan[0].decision, "attempt");
        assert_eq!(plan[1].decision, "skip");
        assert!(plan[1].reason.contains("open_in allowlist"));
        assert_eq!(plan[2].decision, "skip");
        assert!(plan[2].reason.contains("open_cap=1"));
        assert_eq!(plan[3].decision, "skip");
        assert!(plan[3].reason.contains("valid owner/repo slug"));
        // The rendered body carries the duplicate-search marker, provenance,
        // and the acceptance checklist; post performs zero formatting.
        let body = &plan[0].body;
        assert!(body.contains(&format!("ub-review-fingerprint: {}", plan[0].fingerprint)));
        assert!(body.contains("opened by ub-review from EffortlessMetrics/ub-review#346"));
        assert!(body.contains("- [ ] discriminating receipt renders"));
        assert!(plan[0].fingerprint.len() == 64);
        assert!(
            plan[0]
                .candidate_id
                .ends_with(&plan[0].fingerprint[..12].to_owned())
        );

        // Suggest mode never produces a plan: the broker is opt-in.
        let suggest_only = super::IssuesConfig::default();
        let (candidates, actions) = classify_issue_candidates(
            &suggest_only,
            vec![broker_test_candidate(
                "Allowlisted follow-up",
                "EffortlessMetrics/ripr-swarm",
            )],
        );
        assert!(
            build_issue_broker_plan(&suggest_only, &candidates, &actions, None, None).is_empty()
        );
    }

    #[test]
    fn issue_broker_executes_plan_with_duplicate_search_and_receipts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("post-out");
        fs::create_dir_all(&out)?;
        let review_dir = temp.path().join("review");
        fs::create_dir_all(&review_dir)?;
        let entry = |id: &str, fingerprint: &str, decision: &str| IssueBrokerPlanEntry {
            schema: "ub-review.issue_broker_plan.v1".to_owned(),
            candidate_id: id.to_owned(),
            fingerprint: fingerprint.to_owned(),
            target_repo: "EffortlessMetrics/ripr-swarm".to_owned(),
            decision: decision.to_owned(),
            reason: "test".to_owned(),
            title: "Broker test issue".to_owned(),
            body: format!("body\n\nub-review-fingerprint: {fingerprint}\n"),
            labels: vec!["ub-review".to_owned()],
        };
        let plan = vec![
            entry("issue-candidate-000-aaaaaaaaaaaa", "fresh", "attempt"),
            entry("issue-candidate-001-bbbbbbbbbbbb", "existing", "attempt"),
            entry("issue-candidate-002-cccccccccccc", "skipme", "skip"),
        ];
        let plan_path = review_dir.join("issue_broker_plan.json");
        fs::write(&plan_path, serde_json::to_vec_pretty(&plan)?)?;

        // Fake GitHub API: search for "fresh" finds nothing then accepts the
        // create; search for "existing" returns one hit so no create happens.
        let (api_url, handle) = spawn_fake_issue_broker_api(3)?;
        let args = PostArgs {
            review_json: review_dir.join("github-review.json"),
            diff_patch: None,
            out: out.clone(),
            github_token: Some("test-token".to_owned()),
            repo: Some("EffortlessMetrics/ub-review".to_owned()),
            pull_number: Some(346),
            github_api_url: api_url,
            fail_on_post_error: false,
        };
        let results = execute_issue_broker(&args, &plan_path)?;
        let requests = join_fake_issue_broker_api(handle)?;
        assert_eq!(requests.len(), 3);
        assert!(requests[0].contains("GET /search/issues"));
        assert!(requests[0].contains("fresh"));
        assert!(requests[1].contains("POST /repos/EffortlessMetrics/ripr-swarm/issues"));
        assert!(requests[2].contains("GET /search/issues"));
        assert!(requests[2].contains("existing"));

        assert_eq!(results.len(), 3);
        assert_eq!(results[0].action, "opened");
        assert_eq!(
            results[0].url.as_deref(),
            Some("https://github.com/EffortlessMetrics/ripr-swarm/issues/9001")
        );
        assert_eq!(results[1].action, "duplicate");
        assert_eq!(
            results[1].url.as_deref(),
            Some("https://github.com/EffortlessMetrics/ripr-swarm/issues/1052")
        );
        assert_eq!(results[2].action, "skipped");

        // The create payload receipt is on disk and carries the marker body.
        let payload = fs::read_to_string(out.join("issue-broker-payload-000.json"))?;
        assert!(payload.contains("ub-review-fingerprint: fresh"));

        write_issue_broker_results(&out, &results)?;
        let written: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(out.join("review/issue_broker_results.json"))?)?;
        assert_eq!(written.len(), 3);
        let ndjson = fs::read_to_string(out.join("issue_broker_results.ndjson"))?;
        assert_eq!(ndjson.lines().count(), 3);

        // No token: planned attempts become failed_to_open, never errors.
        let no_token = PostArgs {
            github_token: None,
            ..args
        };
        let results = execute_issue_broker(&no_token, &plan_path)?;
        assert_eq!(results[0].action, "failed_to_open");
        assert_eq!(results[1].action, "failed_to_open");
        assert_eq!(results[2].action, "skipped");
        Ok(())
    }

    fn spawn_fake_issue_broker_api(
        expected_requests: usize,
    ) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || {
            serve_fake_http(
                listener,
                expected_requests,
                "issue broker",
                Duration::from_secs(20),
                |_idx, stream| handle_fake_issue_broker_request(stream),
            )
        });
        Ok((url, handle))
    }

    fn handle_fake_issue_broker_request(mut stream: TcpStream) -> Result<String> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                bail!("fake issue broker request ended before headers finished");
            }
            headers.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        if content_length > 0 {
            let mut body = vec![0u8; content_length];
            use std::io::Read as _;
            reader.read_exact(&mut body)?;
        }
        let request_line = headers.lines().next().unwrap_or_default().to_owned();
        let (status_line, response_body) = if request_line.starts_with("GET /search/issues")
            && request_line.contains("existing")
        {
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({
                    "total_count": 1,
                    "items": [{
                        "html_url": "https://github.com/EffortlessMetrics/ripr-swarm/issues/1052"
                    }]
                }))?,
            )
        } else if request_line.starts_with("GET /search/issues") {
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({"total_count": 0, "items": []}))?,
            )
        } else {
            (
                "HTTP/1.1 201 Created",
                serde_json::to_vec(&serde_json::json!({
                    "html_url": "https://github.com/EffortlessMetrics/ripr-swarm/issues/9001"
                }))?,
            )
        };
        write!(
            stream,
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        )?;
        stream.write_all(&response_body)?;
        Ok(request_line)
    }

    fn join_fake_issue_broker_api(
        handle: thread::JoinHandle<Result<Vec<String>>>,
    ) -> Result<Vec<String>> {
        match handle.join() {
            Ok(result) => result,
            Err(_) => bail!("fake issue broker API thread panicked"),
        }
    }

    #[test]
    fn follow_up_resolved_away_excludes_refuted_and_dropped_but_keeps_parked() {
        let candidates = vec![
            test_candidate_record("candidate-keep"),
            test_candidate_record("candidate-refuted"),
            test_candidate_record("candidate-parked"),
            test_candidate_record("candidate-dropped"),
        ];
        let outputs = vec![
            test_follow_up_output_for_candidate(
                "follow-refuted",
                &candidates[1].id,
                "ok",
                vec![test_observation(
                    "orchestrator-follow-up-follow-refuted",
                    "The proof receipt resolves this candidate.",
                    "resolved-check",
                    "refuted",
                    "low",
                    "high",
                    "candidate-refuted",
                )],
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-parked",
                &candidates[2].id,
                "ok",
                vec![test_observation(
                    "orchestrator-follow-up-follow-parked",
                    "The sibling route is parked behind a helper.",
                    "parked-follow-up",
                    "parked",
                    "low",
                    "medium",
                    "candidate-parked",
                )],
                Vec::new(),
            ),
            test_follow_up_output_for_candidate(
                "follow-dropped",
                &candidates[3].id,
                "ok",
                Vec::new(),
                vec![SummaryOnlyFinding {
                    lane: "orchestrator-follow-up-follow-dropped".to_owned(),
                    severity: "low".to_owned(),
                    confidence: "high".to_owned(),
                    reason: "duplicate inline candidate merged after tertiary review".to_owned(),
                    evidence: "duplicate evidence".to_owned(),
                }],
            ),
        ];
        let results = outputs
            .iter()
            .map(|output| {
                let mut result =
                    test_follow_up_result(&output.task_id, &output.group_id, &output.status);
                result.stage = output.stage.clone();
                result.disposition = output.disposition.clone();
                result.evidence_need = output.evidence_need.clone();
                result.candidate_ids = output.candidate_ids.clone();
                result.observation_group_ids = output.observation_group_ids.clone();
                result
            })
            .collect::<Vec<_>>();

        let records = resolved_candidate_records(&candidates, &results, &outputs, &[]);
        let resolved_away = follow_up_resolved_away_candidate_ids(&records);
        assert_eq!(
            resolved_away,
            vec![candidates[1].id.clone(), candidates[3].id.clone()],
            "only refuted and dropped resolutions lose their review surface"
        );
        assert!(
            !resolved_away.contains(&candidates[2].id),
            "parked-follow-up resolutions keep their surface for the parked section"
        );
        assert!(!resolved_away.contains(&candidates[0].id));
    }

    #[test]
    fn resolved_candidate_record_serialization_round_trips_for_known_dispositions() -> Result<()> {
        // Property-style test (no proptest dep): the four canonical resolved
        // statuses must survive a serialize -> deserialize round-trip
        // unchanged. This guards the schema invariant that
        // prior-resolved-candidates (read back from the previous run's
        // resolved_candidates.json) parse losslessly. See #611 / tracker UB-28.
        let statuses = ["confirmed", "refuted", "dropped", "parked-follow-up"];
        for status in statuses {
            let record = ResolvedCandidateRecord {
                schema: "ub-review.resolved_candidate.v1".to_owned(),
                candidate_id: format!("cand-{status}"),
                lane: "ub-memory-lifetime".to_owned(),
                source: "proof-planner".to_owned(),
                original_status: "open".to_owned(),
                original_disposition: "needs-evidence".to_owned(),
                resolved_status: status.to_owned(),
                resolved_disposition: format!("resolved-{status}"),
                resolution_source: "current-run".to_owned(),
                source_artifacts: vec![
                    "review/candidates.json".to_owned(),
                    "review/follow_up_results.json".to_owned(),
                ],
                reason: format!("round-trip test for {status}"),
                follow_up_task_ids: vec!["task-1".to_owned(), "task-2".to_owned()],
                follow_up_stages: vec!["tertiary".to_owned()],
                follow_up_statuses: vec![status.to_owned()],
                evidence: vec!["proof_receipt_42".to_owned()],
            };
            let json = serde_json::to_string(&record)
                .with_context(|| format!("serialize failed for {status}"))?;
            let parsed: ResolvedCandidateRecord = serde_json::from_str(&json)
                .with_context(|| format!("deserialize failed for {status}"))?;
            assert_eq!(parsed.schema, record.schema, "schema mismatch for {status}");
            assert_eq!(
                parsed.candidate_id, record.candidate_id,
                "candidate_id mismatch for {status}"
            );
            assert_eq!(
                parsed.resolved_status, record.resolved_status,
                "resolved_status mismatch for {status}"
            );
            assert_eq!(
                parsed.resolved_disposition, record.resolved_disposition,
                "resolved_disposition mismatch for {status}"
            );
            assert_eq!(
                parsed.source_artifacts, record.source_artifacts,
                "source_artifacts mismatch for {status}"
            );
            assert_eq!(
                parsed.follow_up_task_ids, record.follow_up_task_ids,
                "follow_up_task_ids mismatch for {status}"
            );
            assert_eq!(
                parsed.follow_up_stages, record.follow_up_stages,
                "follow_up_stages mismatch for {status}"
            );
            assert_eq!(
                parsed.follow_up_statuses, record.follow_up_statuses,
                "follow_up_statuses mismatch for {status}"
            );
            assert_eq!(
                parsed.evidence, record.evidence,
                "evidence mismatch for {status}"
            );
        }
        Ok(())
    }

    #[test]
    fn resolved_candidate_record_round_trips_unicode_and_empty_vectors() -> Result<()> {
        // Edge cases: empty vectors and non-ASCII content must survive the
        // round-trip. Guards against serde renames, skip_serializing_if, or
        // encoding assumptions that would silently drop fields. See #611.
        let record = ResolvedCandidateRecord {
            schema: "ub-review.resolved_candidate.v1".to_owned(),
            candidate_id: "cand-unicode-λ-Ω-日本語".to_owned(),
            lane: String::new(),
            source: String::new(),
            original_status: String::new(),
            original_disposition: String::new(),
            resolved_status: "confirmed".to_owned(),
            resolved_disposition: String::new(),
            resolution_source: "prior-resolved-candidates".to_owned(),
            source_artifacts: Vec::new(),
            reason: "unicode + empty-vec edge case 🎯".to_owned(),
            follow_up_task_ids: Vec::new(),
            follow_up_stages: Vec::new(),
            follow_up_statuses: Vec::new(),
            evidence: Vec::new(),
        };
        let json =
            serde_json::to_string(&record).context("serialize failed for unicode edge case")?;
        let parsed: ResolvedCandidateRecord =
            serde_json::from_str(&json).context("deserialize failed for unicode edge case")?;
        assert_eq!(parsed.candidate_id, record.candidate_id);
        assert_eq!(parsed.resolved_status, record.resolved_status);
        assert_eq!(parsed.reason, record.reason);
        assert!(parsed.source_artifacts.is_empty());
        assert!(parsed.follow_up_task_ids.is_empty());
        assert!(parsed.evidence.is_empty());
        Ok(())
    }

    #[test]
    fn prior_resolved_candidates_drop_matching_candidate_by_hash_suffix() {
        let mut current = test_candidate_record("candidate-0007-deadbeef1234");
        current.claim = "The new test may also pass on base.".to_owned();
        current.evidence = "test proof receipt".to_owned();
        let control = test_candidate_record("candidate-0008-cafebabe1234");
        let prior = super::ResolvedCandidateRecord {
            schema: "ub-review.resolved_candidate.v1".to_owned(),
            candidate_id: "candidate-0001-deadbeef1234".to_owned(),
            lane: current.lane.clone(),
            source: current.source.clone(),
            original_status: current.status.clone(),
            original_disposition: current.disposition.clone(),
            resolved_status: "resolved".to_owned(),
            resolved_disposition: "dropped".to_owned(),
            resolution_source: "orchestrator-follow-up".to_owned(),
            source_artifacts: vec![
                "review/candidates.json".to_owned(),
                "review/follow_up_results.json".to_owned(),
                "review/follow_up_outputs.json".to_owned(),
            ],
            reason: "prior follow-up found this below materiality".to_owned(),
            follow_up_task_ids: vec!["follow-prior".to_owned()],
            follow_up_stages: vec!["tertiary".to_owned()],
            follow_up_statuses: vec!["ok".to_owned()],
            evidence: vec!["Prior pass dropped the same candidate surface.".to_owned()],
        };

        let records =
            resolved_candidate_records(&[current.clone(), control.clone()], &[], &[], &[prior]);

        assert_eq!(records[0].candidate_id, current.id);
        assert_eq!(records[0].resolved_status, "resolved");
        assert_eq!(records[0].resolved_disposition, "dropped");
        assert_eq!(records[0].resolution_source, "prior-resolved-candidates");
        assert!(
            records[0]
                .source_artifacts
                .contains(&"review/prior_resolved_candidates.json".to_owned())
        );
        assert_eq!(records[1].candidate_id, control.id);
        assert_eq!(records[1].resolved_status, "unchanged");
        assert_eq!(
            follow_up_resolved_away_candidate_ids(&records),
            vec![current.id]
        );
    }

    #[test]
    fn resolved_away_candidates_match_their_original_review_surfaces() {
        let inline_comments = vec![ReviewInlineComment {
            lane: "ub-active-view".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 7,
            side: "RIGHT".to_owned(),
            body: "[ub-active-view] The reborrow may alias the active view.".to_owned(),
            evidence: "unsafe block at src/lib.rs:7".to_owned(),
            suggestion: None,
        }];
        let summary_only_findings = vec![SummaryOnlyFinding {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium".to_owned(),
            reason: "The new test may also pass on base.".to_owned(),
            evidence: "test sensor receipt".to_owned(),
        }];
        let candidates = build_candidate_records(&inline_comments, &summary_only_findings);
        assert_eq!(candidates.len(), 2);

        // Each candidate matches exactly the surface it was built from.
        assert!(candidate_matches_inline_comment(
            &candidates[0],
            &inline_comments[0]
        ));
        assert!(!candidate_matches_summary_finding(
            &candidates[0],
            &summary_only_findings[0]
        ));
        assert!(candidate_matches_summary_finding(
            &candidates[1],
            &summary_only_findings[0]
        ));
        assert!(!candidate_matches_inline_comment(
            &candidates[1],
            &inline_comments[0]
        ));

        // A different line on the same path must not match: the filter may
        // only remove the exact surface the follow-up pass resolved away.
        let mut moved = inline_comments[0].clone();
        moved.line = 8;
        assert!(!candidate_matches_inline_comment(&candidates[0], &moved));
        let mut reworded = summary_only_findings[0].clone();
        reworded.reason = "The new test proves the patch.".to_owned();
        assert!(!candidate_matches_summary_finding(
            &candidates[1],
            &reworded
        ));
    }

    #[test]
    fn observation_artifacts_include_aggregate_and_lane_ndjson() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review = super::ReviewArtifacts {
            shared_context_id: "abc123".to_owned(),
            review_profile: DEFAULT_REVIEW_PROFILE.to_owned(),
            mode: "review-byok".to_owned(),
            posting: "review".to_owned(),
            runtime_profile: "gh-runner".to_owned(),
            run_pass: "manual".to_owned(),
            model_mode: "auto".to_owned(),
            depth: "standard".to_owned(),
            provider_policy: "minimax-only".to_owned(),
            model_provider_policy: "minimax-only".to_owned(),
            lane_width: 10,
            model_concurrency: 8,
            max_model_calls: 18,
            max_inline_comments: 8,
            model_timeout_sec: 300,
            ledger_path: String::new(),
            ledger_max_bytes: 65_536,
            pr_thread_context: test_pr_thread_context(),
            terminal_state: test_terminal_state("needs-reviewer-attention"),
            provider_preflights: Vec::new(),
            model_lanes: vec![model_lane_receipt("tests-oracle", "ok")],
            missing_or_failed_sensor_evidence: vec![SensorEvidenceIssue {
                sensor: "ripr".to_owned(),
                status: "missing".to_owned(),
                reason: "command not found".to_owned(),
            }],
            missing_or_failed_model_evidence: vec![
                ModelEvidenceIssue {
                    lane: "opposition".to_owned(),
                    provider: "minimax".to_owned(),
                    model: "MiniMax-M3".to_owned(),
                    endpoint_kind: "anthropic-messages".to_owned(),
                    status: "timed_out".to_owned(),
                    reason: "model call timed out".to_owned(),
                },
                ModelEvidenceIssue {
                    lane: "opposition".to_owned(),
                    provider: "minimax".to_owned(),
                    model: "MiniMax-M3".to_owned(),
                    endpoint_kind: "anthropic-messages".to_owned(),
                    status: "failed".to_owned(),
                    reason: "model returned malformed JSON".to_owned(),
                },
            ],
            inline_comments: vec![ReviewInlineComment {
                lane: "tests-oracle".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "test/js/bun/md/md-edge-cases.test.ts".to_owned(),
                line: 1145,
                side: "RIGHT".to_owned(),
                body: "[tests-oracle] The test reaches the helper but needs a red witness."
                    .to_owned(),
                evidence: "ripr excerpt".to_owned(),
                suggestion: None,
            }],
            summary_only_findings: vec![SummaryOnlyFinding {
                lane: "source-route".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "PBKDF2 sibling path is parked as follow-up, not current PR scope."
                    .to_owned(),
                evidence: "UB ledger excerpt".to_owned(),
            }],
            observations: vec![
                test_observation(
                    "tests-oracle",
                    "The new test needs a witnessed old-main red run.",
                    "missing-evidence",
                    "open",
                    "medium",
                    "high",
                    "markdown-red-green-witness",
                ),
                test_observation(
                    "opposition",
                    "The new test needs a witnessed old-main red run.",
                    "missing-evidence",
                    "open",
                    "medium",
                    "medium-high",
                    "markdown-red-green-witness",
                ),
            ],
            proof_requests: Vec::new(),
            proof_intents: Vec::new(),
            proof_receipts: Vec::new(),
            resource_leases: Vec::new(),
            body: "artifact body".to_owned(),
        };

        let observations = combined_observations(&review);
        write_observation_artifacts(temp.path(), &observations)?;

        let aggregate: Vec<super::Observation> =
            serde_json::from_slice(&fs::read(temp.path().join("review/observations.json"))?)?;
        let unique: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/unique_observations.json"),
        )?)?;
        let merged: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/merged_observations.json"),
        )?)?;
        let dropped: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/dropped_observations.json"),
        )?)?;
        let lane_ndjson = fs::read_to_string(temp.path().join("observations/tests-oracle.ndjson"))?;
        let question_artifact: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path()
                .join("questions/opposition/missing-model-evidence.json"),
        )?)?;

        assert_eq!(aggregate.len(), 7);
        assert_eq!(unique.as_array().map(Vec::len), Some(5));
        assert_eq!(merged.as_array().map(Vec::len), Some(2));
        assert_eq!(dropped.as_array().map(Vec::len), Some(2));
        assert_eq!(unique[0]["schema"], "ub-review.observation_group.v1");
        assert_eq!(unique[0]["duplicate_count"], 1);
        assert_eq!(merged[0]["schema"], "ub-review.merged_observation.v1");
        assert_eq!(dropped[0]["schema"], "ub-review.dropped_observation.v1");
        assert!(lane_ndjson.contains("\"schema\":\"ub-review.observation.v1\""));
        assert!(lane_ndjson.contains("\"kind\":\"test-gap\""));
        assert_eq!(
            question_artifact["schema"],
            "ub-review.question_observations.v1"
        );
        assert_eq!(question_artifact["lane"], "opposition");
        assert_eq!(question_artifact["question"], "missing-model-evidence");
        let expected_question_observations: Vec<_> = aggregate
            .iter()
            .filter(|observation| {
                observation.lane == "opposition" && observation.question == "missing-model-evidence"
            })
            .collect();
        assert_eq!(
            question_artifact["observations"],
            serde_json::to_value(expected_question_observations)?
        );
        assert_eq!(
            question_artifact["observations"].as_array().map(Vec::len),
            Some(2)
        );
        assert!(aggregate.iter().any(|observation| {
            observation.lane == "tests-oracle"
                && observation.status == "confirmed"
                && observation.path.as_deref() == Some("test/js/bun/md/md-edge-cases.test.ts")
                && observation.line == Some(1145)
                && observation.dedupe_key == "test-gap:test/js/bun/md/md-edge-cases.test.ts:1145"
                && observation.evidence == vec!["ripr excerpt".to_owned()]
        }));
        assert!(aggregate.iter().any(|observation| {
            observation.lane == "source-route"
                && observation.kind == "parked-follow-up"
                && observation.status == "parked"
        }));
        assert!(aggregate.iter().any(|observation| {
            observation.lane == "sensor-ripr"
                && observation.kind == "missing-evidence"
                && observation.confidence == "high"
        }));
        assert!(aggregate.iter().any(|observation| {
            observation.lane == "opposition"
                && observation.kind == "missing-evidence"
                && observation.question == "missing-model-evidence"
        }));
        Ok(())
    }

    #[test]
    fn observation_question_artifacts_reject_path_collisions() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut observations = vec![
            test_observation(
                "lane/a",
                "First question observation.",
                "missing-evidence",
                "open",
                "low",
                "medium",
                "question-path-collision-a",
            ),
            test_observation(
                "lane-a",
                "Second question observation.",
                "missing-evidence",
                "open",
                "low",
                "medium",
                "question-path-collision-b",
            ),
        ];
        observations[0].question = "same/question".to_owned();
        observations[1].question = "same-question".to_owned();

        let error = match write_observation_artifacts(temp.path(), &observations) {
            Ok(()) => return Err(anyhow::anyhow!("question path collision was not rejected")),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("questions artifact path collision")
        );
        Ok(())
    }

    #[test]
    fn observation_question_artifacts_bound_model_supplied_filenames() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut observations = vec![test_observation(
            "opposition",
            "The lane emitted a long verification question.",
            "missing-evidence",
            "open",
            "low",
            "medium",
            "long-question-filename",
        )];
        let long_question = format!(
            "{}terminal-proof-question",
            "confirm-the-focused-proof-before-upstream-".repeat(12)
        );
        observations[0].question = long_question.clone();

        write_observation_artifacts(temp.path(), &observations)?;

        let lane_dir = temp.path().join("questions").join("opposition");
        let files = fs::read_dir(&lane_dir)?.collect::<Result<Vec<_>, _>>()?;
        assert_eq!(files.len(), 1);
        let file_name = files[0].file_name().to_string_lossy().to_string();
        let expected_stem = super::sanitize_artifact_name(&long_question);
        assert_eq!(expected_stem.len(), super::ARTIFACT_NAME_MAX_CHARS);
        assert_eq!(file_name, format!("{expected_stem}.json"));
        assert!(file_name.len() <= super::ARTIFACT_NAME_MAX_CHARS + ".json".len());

        let artifact: serde_json::Value = serde_json::from_slice(&fs::read(files[0].path())?)?;
        assert_eq!(artifact["question"], long_question);
        Ok(())
    }

    #[test]
    fn witness_artifacts_track_review_statuses() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let inline_comments = vec![ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "test/js/bun/ffi/ffi.test.js".to_owned(),
            line: 42,
            side: "RIGHT".to_owned(),
            body: "[tests-oracle] The no-finalizer regression still needs a red witness."
                .to_owned(),
            evidence: "diff hunk".to_owned(),
            suggestion: None,
        }];
        let summary_only_findings = vec![SummaryOnlyFinding {
            lane: "source-route".to_owned(),
            severity: "low".to_owned(),
            confidence: "medium-high".to_owned(),
            reason: "PBKDF2 sibling path is parked as follow-up, not current PR scope.".to_owned(),
            evidence: "UB ledger excerpt".to_owned(),
        }];
        let observations = vec![
            test_observation(
                "tests-oracle",
                "The focused test reaches the patched helper.",
                "test-gap",
                "confirmed",
                "medium",
                "high",
                "test-helper-route",
            ),
            test_observation(
                "tests-red-green",
                "The new test still needs a base+tests witness.",
                "missing-evidence",
                "open",
                "medium",
                "high",
                "base-tests-witness",
            ),
            test_observation(
                "ub-active-view",
                "Box::from(slice) allocation failure concern is false.",
                "false-premise",
                "refuted",
                "low",
                "high",
                "box-from-refuted",
            ),
        ];
        let receipts = vec![
            test_red_green_proof_receipt("discriminating", "failed"),
            test_proof_receipt("timed_out", "timed_out"),
        ];

        let witnesses = build_witness_records(
            &inline_comments,
            &summary_only_findings,
            &observations,
            &receipts,
        );
        write_witness_artifacts(temp.path(), &witnesses)?;

        let witness_json: Vec<super::WitnessRecord> =
            serde_json::from_slice(&fs::read(temp.path().join("review/witnesses.json"))?)?;
        let registry: super::WitnessRegistryArtifact =
            serde_json::from_slice(&fs::read(temp.path().join("review/witness_registry.json"))?)?;
        let ndjson = fs::read_to_string(temp.path().join("witnesses.ndjson"))?;
        assert_eq!(witness_json.len(), witnesses.len());
        assert_eq!(ndjson.lines().count(), witness_json.len());
        assert_eq!(registry.schema, "ub-review.witness_registry.v1");
        assert_eq!(registry.total, witness_json.len());
        assert_eq!(registry.status_counts["needs-witness"], 3);
        assert_eq!(registry.status_counts["tool-confirmed"], 2);
        assert_eq!(registry.status_counts["refuted"], 1);
        assert_eq!(registry.status_counts["parked"], 1);
        assert_eq!(registry.source_counts["proof-receipt"], 2);
        assert_eq!(registry.follow_up_total, 0);
        assert!(registry.follow_up_status_counts.is_empty());
        assert!(
            witness_json.iter().all(|witness| {
                witness.schema == "ub-review.witness.v1" && !witness.id.is_empty()
            })
        );
        assert!(witness_json.iter().any(|witness| {
            witness.kind == "inline-finding" && witness.status == "needs-witness"
        }));
        assert!(
            witness_json
                .iter()
                .any(|witness| { witness.kind == "summary-finding" && witness.status == "parked" })
        );
        assert!(witness_json.iter().any(|witness| {
            witness.dedupe_key == "test-helper-route" && witness.status == "tool-confirmed"
        }));
        assert!(witness_json.iter().any(|witness| {
            witness.dedupe_key == "base-tests-witness" && witness.status == "needs-witness"
        }));
        assert!(witness_json.iter().any(|witness| {
            witness.dedupe_key == "box-from-refuted" && witness.status == "refuted"
        }));
        assert!(witness_json.iter().any(|witness| {
            witness.source == "proof-receipt"
                && witness.status == "tool-confirmed"
                && witness.proof_receipt_id.is_some()
        }));
        assert!(witness_json.iter().any(|witness| {
            witness.source == "proof-receipt"
                && witness.status == "needs-witness"
                && witness
                    .evidence
                    .iter()
                    .any(|item| item.contains("stdout.txt"))
        }));
        Ok(())
    }

    #[test]
    fn review_body_routes_parked_followups_to_campaign_section() {
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "source-route".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "PBKDF2 sibling path is parked as follow-up, not current PR scope."
                    .to_owned(),
                evidence: "UB ledger excerpt".to_owned(),
            }],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Parked follow-ups"));
        assert!(body.contains("PBKDF2 sibling path is parked as follow-up"));
        assert!(!body.contains("## Summary-only findings"));
        assert!(!body.contains("## Summary-only concerns"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_dedupes_observations_before_rendering() {
        let mut global_box_refutation = test_observation(
            "ub-active-view",
            "Box::from(slice) can return None on allocation failure; refuted because: allocation failure does not return None",
            "false-premise",
            "refuted",
            "low",
            "high",
            "rust-box-from-allocation-failure",
        );
        global_box_refutation.source = "model-false-premise-guard".to_owned();
        let observations = vec![
            test_observation(
                "tests-oracle",
                "The new test needs a witnessed old-main red run.",
                "missing-evidence",
                "open",
                "medium",
                "high",
                "markdown-red-green-witness",
            ),
            test_observation(
                "opposition",
                "The new test needs a witnessed old-main red run.",
                "missing-evidence",
                "open",
                "medium",
                "medium-high",
                "markdown-red-green-witness",
            ),
            global_box_refutation,
            test_observation(
                "source-route",
                "A typed-array view over a resizable ArrayBuffer carries the resizable flag through PinnedView.",
                "verification-question",
                "open",
                "medium",
                "medium-high",
                "typed-array-rab-resizable-flag",
            ),
        ];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "tests-red-green".to_owned(),
                severity: "medium".to_owned(),
                confidence: "high".to_owned(),
                reason: "The new test needs a witnessed old-main red run.".to_owned(),
                evidence: "duplicate lane summary".to_owned(),
            }],
            &observations,
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert_eq!(
            body.matches("The new test needs a witnessed old-main red run.")
                .count(),
            1
        );
        assert!(!body.contains("`[tests-oracle, opposition]`"));
        assert!(body.contains("## Verification questions"));
        assert!(body.contains("Confirm a typed-array view over a resizable ArrayBuffer"));
        assert!(!body.contains("## Refuted"));
        assert!(!body.contains("Box::from(slice) can return None"));
        assert!(body.contains("## Evidence gaps"));
        assert!(!body.contains("duplicate lane summary"));
        assert!(!body.contains("Evidence:"));
        assert!(!body.contains("medium-high"));
        assert!(!body.contains("## Model lanes"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn cross_lane_conflict_receipt_suppresses_standalone_summary_finding() {
        let finding = SummaryOnlyFinding {
            lane: "correctness".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            reason: "Remove the `?? options[key]` fallback; otherwise top-level symbol specs can silently merge into undefined args.".to_owned(),
            evidence: "Changed TypeScript fallback branch in ffi symbol option handling.".to_owned(),
        };
        let mut observations = vec![test_observation(
            "source-route",
            "The `?? options[key]` fallback is not a valid top-level symbol route; refuted because the cc signature requires symbols nested under the `symbols` key.",
            "false-premise",
            "refuted",
            "low",
            "high",
            "fallback-route-refuted",
        )];

        super::append_cross_lane_conflict_observations(
            &[],
            std::slice::from_ref(&finding),
            &mut observations,
        );

        let conflict = observations
            .iter()
            .find(|observation| observation.source == super::CROSS_LANE_CONFLICT_SOURCE);
        assert!(
            conflict.is_some(),
            "cross-lane conflict observation missing"
        );
        let Some(conflict) = conflict else {
            return;
        };
        assert_eq!(conflict.lane, "orchestrator-conflict");
        assert_eq!(conflict.kind, "verification-question");
        assert_eq!(conflict.status, "open");
        assert!(conflict.dedupe_key.starts_with("cross-lane-conflict:"));
        assert!(conflict.evidence.iter().any(|evidence| {
            evidence.contains(&format!(
                "finding_key={}",
                super::summary_finding_conflict_key(&finding)
            ))
        }));

        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            std::slice::from_ref(&finding),
            &observations,
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(body.contains("## Decision"));
        assert!(body.contains("## Verification questions"));
        assert!(body.contains("cross-lane conflict"));
        assert!(!body.contains("## Confirmed findings"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn pr_review_body_omits_lane_output_shape_artifacts() {
        let observations = vec![test_observation(
            "ub-worker-handoff",
            "Lane output was contentful but not valid JSON; preserved degraded text: EncodedSlice route excerpt",
            "missing-evidence",
            "open",
            "low",
            "medium",
            "lane-output-malformed-content",
        )];
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[] as &[SummaryOnlyFinding],
            &observations,
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(!body.contains("## Evidence gaps"));
        assert!(!body.contains("## Missing evidence"));
        assert!(!body.contains("Lane output was contentful"));
        assert!(!body.contains("EncodedSlice route excerpt"));
        assert!(body.is_empty());
    }

    #[test]
    fn review_body_cap_preserves_utf8_boundary() {
        let capped = cap_review_body("a🙂b🙂c".repeat(100), 64);

        assert!(capped.ends_with("[review body truncated; see review artifacts]\n"));
        assert!(capped.is_char_boundary(capped.len()));
    }

    #[test]
    fn review_body_cap_drops_excess_bullets_without_failing_compilation() {
        let body = (0..20)
            .map(|index| format!("- finding {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let capped = cap_review_body_bullets(body, 12);
        let bullets = capped.lines().filter(|line| line.starts_with("- ")).count();
        assert_eq!(bullets, 12);
        assert!(capped.contains("review body truncated"));
    }

    #[test]
    fn review_body_cap_preserves_required_sections() {
        let long_text = "changed Rust native boundary evidence ".repeat(100);
        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[SensorEvidenceIssue {
                sensor: "unsafe-review".to_owned(),
                status: "missing".to_owned(),
                reason: long_text.clone(),
            }],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "tests-oracle".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: long_text,
                evidence: "RIPR proof gap excerpt".to_owned(),
            }],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            1_000,
            ReviewBodyAudience::Artifact,
        );

        assert!(body.len() <= 1_000);
        for heading in [
            "## Decision",
            "## Confirmed findings",
            "## Summary-only findings",
            "## Failed objections",
            "## Residual risk",
            "## Parked follow-ups",
            "## Missing or failed evidence",
        ] {
            assert!(body.contains(heading), "missing {heading}");
        }
        assert!(body.ends_with("[review body truncated; see review artifacts]\n"));
        assert!(!has_standalone_approval_line(&body));
    }

    pub(crate) fn sensor_plan(id: &str, command: &str, run: bool) -> SensorPlan {
        SensorPlan {
            id: id.to_owned(),
            command: command.to_owned(),
            run,
            reason: "test reason".to_owned(),
            required: false,
            timeout_sec: 1,
            artifact_budget_mb: 1,
            class: ToolClass::Static,
            weight: 1,
            requires_lease: false,
            phase: super::SensorPhase::Fast,
            gate: None,
        }
    }

    pub(crate) fn run_test_command(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
        let output = ProcessCommand::new(program)
            .args(args)
            .current_dir(cwd)
            .output()?;
        if output.status.success() {
            return Ok(());
        }
        Err(anyhow::anyhow!(
            "{} {:?} failed\nstdout:\n{}\nstderr:\n{}",
            program,
            args,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }

    fn model_lane_receipt(lane: &str, status: &str) -> super::ModelLaneReceipt {
        super::ModelLaneReceipt {
            lane: lane.to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "openai-chat".to_owned(),
            status: status.to_owned(),
            reason: "test reason".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: super::ModelCacheUsage::default(),
            cohort_id: String::new(),
            shared_prefix_hash: String::new(),
            thread_id: String::new(),
            turn: 0,
            cohort_broken: false,
        }
    }

    fn test_review_artifacts() -> super::ReviewArtifacts {
        super::ReviewArtifacts {
            shared_context_id: "abc123abc123abc123abc123abc123abc123abc123abc123abc123abc123abcd"
                .to_owned(),
            review_profile: DEFAULT_REVIEW_PROFILE.to_owned(),
            mode: "review-byok".to_owned(),
            posting: "review".to_owned(),
            runtime_profile: "gh-runner".to_owned(),
            run_pass: "opened".to_owned(),
            model_mode: "auto".to_owned(),
            depth: "standard".to_owned(),
            provider_policy: "minimax-only".to_owned(),
            model_provider_policy: "minimax-only".to_owned(),
            lane_width: 10,
            model_concurrency: 8,
            max_model_calls: 18,
            max_inline_comments: 8,
            model_timeout_sec: 300,
            ledger_path: String::new(),
            ledger_max_bytes: 65_536,
            pr_thread_context: test_pr_thread_context(),
            terminal_state: test_terminal_state("needs-reviewer-attention"),
            provider_preflights: Vec::new(),
            model_lanes: Vec::new(),
            missing_or_failed_sensor_evidence: Vec::new(),
            missing_or_failed_model_evidence: Vec::new(),
            inline_comments: Vec::new(),
            summary_only_findings: Vec::new(),
            observations: Vec::new(),
            proof_requests: Vec::new(),
            proof_intents: Vec::new(),
            proof_receipts: Vec::new(),
            resource_leases: Vec::new(),
            body: "artifact body".to_owned(),
        }
    }

    fn test_candidate_record(id: &str) -> CandidateRecord {
        CandidateRecord {
            schema: "ub-review.candidate.v1".to_owned(),
            id: id.to_owned(),
            lane: "tests-oracle".to_owned(),
            source: "summary-only-finding".to_owned(),
            status: "summary-only".to_owned(),
            disposition: "summary-only".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            claim: format!("Candidate {id} needs follow-up classification."),
            evidence: "test candidate evidence".to_owned(),
            path: None,
            line: None,
            side: None,
        }
    }

    fn test_follow_up_output_for_candidate(
        task_id: &str,
        candidate_id: &str,
        status: &str,
        observations: Vec<Observation>,
        summary_only_findings: Vec<SummaryOnlyFinding>,
    ) -> FollowUpOutputRecord {
        FollowUpOutputRecord {
            schema: "ub-review.follow_up_output.v1".to_owned(),
            task_id: task_id.to_owned(),
            group_id: format!("group-{candidate_id}"),
            stage: "tertiary".to_owned(),
            disposition: "summary-only".to_owned(),
            evidence_need: "proof-confirmation".to_owned(),
            candidate_ids: vec![candidate_id.to_owned()],
            observation_group_ids: Vec::new(),
            model_lane: format!("orchestrator-follow-up-{task_id}"),
            status: status.to_owned(),
            reason: "test follow-up output".to_owned(),
            inline_comments: Vec::new(),
            summary_only_findings,
            observations,
            proof_requests: Vec::new(),
        }
    }

    fn test_follow_up_result(task_id: &str, group_id: &str, status: &str) -> super::FollowUpResult {
        super::FollowUpResult {
            schema: "ub-review.follow_up_result.v1".to_owned(),
            task_id: task_id.to_owned(),
            group_id: group_id.to_owned(),
            stage: "secondary".to_owned(),
            disposition: "observation".to_owned(),
            evidence_need: "proof-confirmation".to_owned(),
            candidate_ids: Vec::new(),
            observation_group_ids: Vec::new(),
            packet_path: format!("questions/orchestrator-follow-up/{task_id}.json"),
            model_lane: format!("orchestrator-follow-up-{task_id}"),
            status: status.to_owned(),
            reason: "test follow-up result".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            fallback_from: None,
            duration_ms: None,
            http_status: None,
            response_shape: None,
            cache_usage: super::ModelCacheUsage::default(),
            request_path: None,
            response_path: None,
            content_path: None,
            normalized_content_path: None,
            stderr_path: None,
            output_counts: super::FollowUpOutputCounts::default(),
        }
    }

    fn test_run_loop_metrics() -> super::RunLoopMetrics {
        super::RunLoopMetrics {
            concurrency_model: "profiled-stream-scheduler-v0".to_owned(),
            scheduler_profile: "default-three-stream-v0".to_owned(),
            local_proof_wall_excludes_model_wait: true,
            elapsed_wall_ms: 450,
            coordination_wall_ms: 50,
            investigation_wall_ms: 300,
            proof_wall_ms: 80,
            evidence_wall_ms: 10,
            model_wall_ms: 300,
            local_proof_wall_ms: 80,
            compiler_wall_ms: 40,
            model_call_duration_ms_sum: 0,
            proof_command_duration_ms_sum: 0,
            investigation_proof_overlap_ms: 0,
            model_proof_overlap_ms: 0,
            proof_overlap_ms: 0,
            scheduler_roles: super::SchedulerRoleTimings {
                evidence: super::LoopTiming {
                    started_at_offset_ms: 0,
                    finished_at_offset_ms: 10,
                    wall_ms: 10,
                },
                model: super::LoopTiming {
                    started_at_offset_ms: 10,
                    finished_at_offset_ms: 310,
                    wall_ms: 300,
                },
                proof: super::LoopTiming {
                    started_at_offset_ms: 320,
                    finished_at_offset_ms: 400,
                    wall_ms: 80,
                },
            },
            streams: super::RunStreamTimings {
                coordination: super::LoopTiming {
                    started_at_offset_ms: 0,
                    finished_at_offset_ms: 450,
                    wall_ms: 50,
                },
                investigation: super::LoopTiming {
                    started_at_offset_ms: 10,
                    finished_at_offset_ms: 310,
                    wall_ms: 300,
                },
                proof: super::LoopTiming {
                    started_at_offset_ms: 320,
                    finished_at_offset_ms: 400,
                    wall_ms: 80,
                },
            },
            loops: super::RunLoopTimings {
                evidence: super::LoopTiming {
                    started_at_offset_ms: 0,
                    finished_at_offset_ms: 10,
                    wall_ms: 10,
                },
                model: super::LoopTiming {
                    started_at_offset_ms: 10,
                    finished_at_offset_ms: 310,
                    wall_ms: 300,
                },
                proof: super::LoopTiming {
                    started_at_offset_ms: 320,
                    finished_at_offset_ms: 400,
                    wall_ms: 80,
                },
                compiler: super::LoopTiming {
                    started_at_offset_ms: 410,
                    finished_at_offset_ms: 450,
                    wall_ms: 40,
                },
            },
            phases: vec![
                super::RunLoopPhase {
                    loop_id: "evidence".to_owned(),
                    stream_id: "coordination".to_owned(),
                    stage: "sensors".to_owned(),
                    status: "completed".to_owned(),
                    started_at_offset_ms: 0,
                    finished_at_offset_ms: 10,
                    duration_ms: 10,
                },
                super::RunLoopPhase {
                    loop_id: "model".to_owned(),
                    stream_id: "investigation".to_owned(),
                    stage: "primary".to_owned(),
                    status: "completed".to_owned(),
                    started_at_offset_ms: 10,
                    finished_at_offset_ms: 310,
                    duration_ms: 300,
                },
            ],
        }
    }

    pub(crate) fn test_proof_receipt(result: &str, command_status: &str) -> ProofReceipt {
        ProofReceipt {
            schema: "ub-review.proof_receipt.v1".to_owned(),
            id: "proof-red-green-test".to_owned(),
            kind: "focused-head".to_owned(),
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids: vec!["proof-tests-001".to_owned()],
            commands: vec![ProofCommandReceipt {
                side: "head".to_owned(),
                command: "bun bd test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots input'"
                    .to_owned(),
                env: BTreeMap::new(),
                status: command_status.to_owned(),
                exit_code: Some(0),
                timed_out: result == "timed_out",
                timeout_sec: 300,
                duration_ms: 42,
                stdout: "proof/proof-red-green-test/head/stdout.txt".to_owned(),
                stderr: "proof/proof-red-green-test/head/stderr.txt".to_owned(),
                reason: "test receipt fixture".to_owned(),
            }],
            result: result.to_owned(),
            reason: "test receipt fixture".to_owned(),
        }
    }

    fn test_red_green_proof_receipt(result: &str, base_status: &str) -> ProofReceipt {
        ProofReceipt {
            schema: "ub-review.proof_receipt.v1".to_owned(),
            id: "proof-red-green-test".to_owned(),
            kind: "focused-red-green".to_owned(),
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            test_patch_mode: "base-plus-tests".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids: vec!["proof-tests-001".to_owned()],
            commands: vec![
                ProofCommandReceipt {
                    side: "head".to_owned(),
                    command:
                        "bun bd test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots input'"
                            .to_owned(),
                    env: BTreeMap::new(),
                    status: "passed".to_owned(),
                    exit_code: Some(0),
                    timed_out: false,
                    timeout_sec: 300,
                    duration_ms: 42,
                    stdout: "proof/proof-red-green-test/head/stdout.txt".to_owned(),
                    stderr: "proof/proof-red-green-test/head/stderr.txt".to_owned(),
                    reason: "test receipt fixture".to_owned(),
                },
                ProofCommandReceipt {
                    side: "base-plus-tests".to_owned(),
                    command: "USE_SYSTEM_BUN=1 bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots input'"
                        .to_owned(),
                    env: BTreeMap::from([("USE_SYSTEM_BUN".to_owned(), "1".to_owned())]),
                    status: base_status.to_owned(),
                    exit_code: Some(if base_status == "passed" { 0 } else { 1 }),
                    timed_out: false,
                    timeout_sec: 300,
                    duration_ms: 42,
                    stdout: "proof/proof-red-green-test/base-plus-tests/stdout.txt".to_owned(),
                    stderr: "proof/proof-red-green-test/base-plus-tests/stderr.txt".to_owned(),
                    reason: "test receipt fixture".to_owned(),
                },
            ],
            result: result.to_owned(),
            reason: "test receipt fixture".to_owned(),
        }
    }

    fn test_observation(
        lane: &str,
        claim: &str,
        kind: &str,
        status: &str,
        severity: &str,
        confidence: &str,
        dedupe_key: &str,
    ) -> Observation {
        let fingerprint = sha256_hex(format!("{lane}\n{kind}\n{status}\n{claim}").as_bytes());
        Observation {
            schema: "ub-review.observation.v1".to_owned(),
            id: format!("obs-test-{}", &fingerprint[..12]),
            lane: lane.to_owned(),
            question: lane.to_owned(),
            claim: claim.to_owned(),
            kind: kind.to_owned(),
            status: status.to_owned(),
            severity: severity.to_owned(),
            confidence: confidence.to_owned(),
            path: None,
            line: None,
            fingerprint,
            evidence: vec![format!("{lane} observation evidence")],
            dedupe_key: dedupe_key.to_owned(),
            source: "model-observation".to_owned(),
        }
    }

    #[test]
    fn observation_artifacts_normalize_zero_line_to_absent() {
        let path = "src/main.rs".to_owned();
        let observation = make_observation(ObservationInput {
            index: 0,
            lane: "source-route",
            question: "source-route",
            claim: "model supplied a non-positive observation line",
            kind: "false-premise",
            status: "refuted",
            severity: "low",
            confidence: "high",
            path: Some(&path),
            line: Some(0),
            evidence: vec!["model observation fixture".to_owned()],
            dedupe_key: None,
            source: "model-observation",
        });

        assert_eq!(observation.path.as_deref(), Some("src/main.rs"));
        assert_eq!(observation.line, None);
        assert_eq!(
            observation.dedupe_key,
            "false-premise:source-route".to_owned()
        );
    }

    fn lane_plan(id: &str) -> LanePlan {
        LanePlan {
            id: id.to_owned(),
            role: "Test lane".to_owned(),
            model: "custom:MiniMax-M3".to_owned(),
            model_display: "MiniMax-M3".to_owned(),
            receives: Vec::new(),
            focus: "Check focused review evidence.".to_owned(),
        }
    }

    pub(crate) fn test_plan(sensors: Vec<SensorPlan>) -> Plan {
        Plan {
            base: "HEAD~1".to_owned(),
            head: "HEAD".to_owned(),
            profile_name: "gh-runner".to_owned(),
            diff_class: DiffClass::SourceUb,
            changed_files: vec!["src/lib.rs".to_owned(), "tests/lib.rs".to_owned()],
            language_mix: super::classify_language_mix(&[
                "src/lib.rs".to_owned(),
                "tests/lib.rs".to_owned(),
            ]),
            sensors,
            lanes: vec![LanePlan {
                id: "tests".to_owned(),
                role: "Test oracle review".to_owned(),
                model: "custom:MiniMax-M3-3".to_owned(),
                model_display: "MiniMax-M3".to_owned(),
                receives: vec!["ripr".to_owned()],
                focus: "Check test proof.".to_owned(),
            }],
            repo_lanes: Vec::new(),
            docs_only: false,
            notes: Vec::new(),
        }
    }

    pub(crate) fn test_diff() -> DiffContext {
        DiffContext {
            base: "HEAD~1".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["src/lib.rs".to_owned(), "tests/lib.rs".to_owned()],
            patch: "+ unsafe { core::ptr::read(ptr) }".to_owned(),
            flags: DiffFlags {
                source_changed: true,
                rust_changed: true,
                rust_tests_changed: true,
                workflow_changed: false,
                dependency_changed: false,
                shell_changed: false,
                cpp_changed: false,
                docs_only: false,
                unsafe_or_native_risk: true,
            },
            diff_class: DiffClass::SourceUb,
        }
    }

    fn test_pr_thread_context() -> PrThreadContext {
        PrThreadContext {
            schema: "ub-review.pr_thread_context.v1".to_owned(),
            status: "absent".to_owned(),
            max_bytes: 65_536,
            sources: Vec::new(),
            warnings: Vec::new(),
            pull_number: None,
            title: None,
            body: None,
            body_truncated: false,
            thread_context_path: None,
            thread_context: None,
            thread_context_truncated: false,
            threads: Vec::new(),
        }
    }

    pub(crate) fn test_terminal_state(status: &str) -> ReviewTerminalState {
        ReviewTerminalState {
            schema: "ub-review.terminal_state.v1".to_owned(),
            status: status.to_owned(),
            reason: "test terminal state".to_owned(),
            review_payload_status: if status == "needs-reviewer-attention" {
                "prepared".to_owned()
            } else {
                "skipped_empty_smoke".to_owned()
            },
            reviewer_value_present: status == "needs-reviewer-attention",
            diff_class: "source-ub".to_owned(),
            model_mode: "auto".to_owned(),
            usable_model_lanes: 1,
            model_lanes: 1,
            evidence_gaps: 0,
            proof_receipts: 0,
            final_follow_up_tasks: 0,
            inline_comments: 0,
            summary_only_findings: 0,
            substantive_summary_only_findings: 0,
        }
    }

    pub(crate) fn test_run_args(out: std::path::PathBuf) -> RunArgs {
        RunArgs {
            review: ReviewArgs {
                root: Path::new(".").to_path_buf(),
                base: "HEAD~1".to_owned(),
                head: "HEAD".to_owned(),
                config: Path::new(".ub-review.toml").to_path_buf(),
                out,
                profile: None,
                runtime_profile: None,
            },
            dry_run: false,
            allow_heavy: false,
            no_github_summary: true,
            posting: PostingMode::ArtifactOnly,
            mode: RunMode::ReviewByok,
            run_pass: super::RunPass::Auto,
            model_mode: ModelMode::Auto,
            sensor_phases: super::SensorPhasesMode::Pipelined,
            fail_on_gate: super::FailOnGate::Auto,
            selectors: SelectorArgs::default(),
            depth: ReviewDepth::Standard,
            max_inline_comments: 8,
            model_concurrency: STANDARD_MODEL_CONCURRENCY,
            max_model_calls: STANDARD_MAX_MODEL_CALLS,
            provider_policy: ModelProviderPolicy::MinimaxPrimary,
            minimax_prompt_cache: MinimaxPromptCache::ExplicitAnthropic,
            lane_width: STANDARD_LANE_WIDTH,
            model_timeout_sec: 300,
            ledger_path: String::new(),
            ledger_max_bytes: 65_536,
            pr_thread_context: String::new(),
            pr_thread_context_max_bytes: 65_536,
            prior_resolved_candidates: String::new(),
            pr_thread_auth: None,
            github_repo: None,
            github_pull_number: None,
            github_api_url: "https://api.github.com".to_owned(),
            minimax_provider_kind: ProviderKindArg::Anthropic,
            minimax_model: "MiniMax-M3".to_owned(),
            opencode_model: "minimax-m3".to_owned(),
            opencode_endpoint_kind: OpenCodeEndpointKindArg::Auto,
            review_body_max_bytes: 60_000,
            review_mode: None,
        }
    }

    fn spawn_fake_github_thread_api(
        expected_requests: usize,
    ) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || {
            serve_fake_http(
                listener,
                expected_requests,
                "GitHub thread",
                Duration::from_secs(20),
                |_idx, stream| handle_fake_github_thread_request(stream),
            )
        });
        Ok((url, handle))
    }

    fn handle_fake_github_thread_request(mut stream: TcpStream) -> Result<String> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                bail!("fake GitHub thread request ended before headers finished");
            }
            headers.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let request_line = headers.lines().next().unwrap_or_default();
        let response_body = if request_line.contains("/issues/76/comments?per_page=30") {
            serde_json::to_vec(&serde_json::json!([
                {
                    "created_at": "2026-06-03T10:00:00Z",
                    "user": {"login": "author"},
                    "body": "Author reply: ASAN receipt attached; prior verification question is answered."
                }
            ]))?
        } else if request_line.contains("/pulls/76/reviews?per_page=30") {
            serde_json::to_vec(&serde_json::json!([
                {
                    "created_at": "2026-06-03T10:05:00Z",
                    "user": {"login": "ub-review[bot]"},
                    "state": "COMMENTED",
                    "body": "ub-review previous question resolved by the receipt."
                }
            ]))?
        } else if request_line.contains("/pulls/76/comments?per_page=50") {
            serde_json::to_vec(&serde_json::json!([
                {
                    "created_at": "2026-06-03T10:10:00Z",
                    "user": {"login": "maintainer"},
                    "path": "src/lib.rs",
                    "line": null,
                    "original_line": 12,
                    "body": "Inline thread points at the route proof receipt."
                }
            ]))?
        } else {
            serde_json::to_vec(&serde_json::json!([]))?
        };
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        )?;
        stream.write_all(&response_body)?;
        Ok(headers)
    }

    fn join_fake_github_thread_api(
        handle: thread::JoinHandle<Result<Vec<String>>>,
    ) -> Result<Vec<String>> {
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("fake GitHub thread API thread panicked"))?
    }

    fn summary_section<'a>(text: &'a str, heading: &str, next_heading: &str) -> Option<&'a str> {
        let start = text.find(heading)? + heading.len();
        let rest = &text[start..];
        let end = rest.find(next_heading)?;
        Some(&rest[..end])
    }

    pub(crate) fn sleeper_argv() -> Vec<String> {
        if cfg!(windows) {
            vec![
                "cmd".to_owned(),
                "/C".to_owned(),
                "ping -n 3 127.0.0.1 >NUL".to_owned(),
            ]
        } else {
            vec!["sh".to_owned(), "-c".to_owned(), "sleep 2".to_owned()]
        }
    }

    fn has_standalone_approval_line(text: &str) -> bool {
        text.lines().any(|line| {
            let trimmed = line
                .trim()
                .trim_start_matches("- ")
                .trim_start_matches("* ")
                .trim()
                .to_ascii_lowercase();
            matches!(
                trimmed.as_str(),
                "lgtm"
                    | "looks good"
                    | "clean"
                    | "solid"
                    | "no issues found"
                    | "no actionable findings"
                    | "no actionable"
            )
        })
    }

    // ----------------------------------------------------------------------
    // audit-ci (docs/CI_AUDIT_WIZARD.md): fixture-driven, no network.
    #[path = "sensor_tests.rs"]
    mod sensor_tests;

    #[path = "validate_tests.rs"]
    mod validate_tests;

    #[cfg(test)]
    #[path = "ci_audit_tests.rs"]
    mod ci_audit_tests;

    #[test]
    fn ci_repo_slug_parses_remote_url_variants() {
        assert_eq!(
            super::ci_repo_slug_from_remote_url("git@github.com:acme/widgets.git").as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(
            super::ci_repo_slug_from_remote_url("https://github.com/acme/widgets").as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(
            super::ci_repo_slug_from_remote_url("ssh://git@github.com/acme/widgets.git").as_deref(),
            Some("acme/widgets")
        );
        assert_eq!(super::ci_repo_slug_from_remote_url("not-a-url"), None);
    }

    // --- unsafe-review artifact ingestion (#359) ---

    /// Sensor status ok + v1 gate: lane evidence block includes movement and
    /// comment-plan candidates.
    #[test]
    fn render_unsafe_review_lane_evidence_includes_movement_and_candidates() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;
        // Real-shape manifest: movement nested under `summary`, snake_case
        // `comment_plan` artifacts key. new_gaps=1 lives in the nested block,
        // so a regression to flat top-level reads would render new_gaps=0 here.
        fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{
                "schema_version": "unsafe-review-gate/v1",
                "dialect": "unsafe-review",
                "status": "advisory",
                "summary": {
                    "new_gaps": 1,
                    "worsened_gaps": 0,
                    "resolved_gaps": 0,
                    "inherited_gaps": 0
                },
                "artifacts": {"comment_plan": "comment-plan.json"},
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict",
                "tool": "unsafe-review",
                "tool_version": "0.3.4"
            }"#,
        )?;
        fs::write(
            out_dir.join("comment-plan.json"),
            r#"[{"card_id": "c1", "path": "src/ffi.rs", "line": 99, "changed_line": true, "coverage_gap": "transmute without invariant check", "confirmation_state": "unconfirmed"}]"#,
        )?;
        let block = super::render_unsafe_review_lane_evidence(&sensor_dir, "ok");
        assert!(block.contains("advisory"), "trust boundary must appear");
        assert!(
            block.contains("new_gaps=1"),
            "movement must come through nested summary: {block}"
        );
        assert!(
            block.contains("src/ffi.rs:99"),
            "candidate path:line must appear"
        );
        assert!(block.contains("transmute"), "coverage_gap must appear");
        Ok(())
    }

    /// Unknown schema: lane evidence block explains degradation, does not crash.
    #[test]
    fn render_unsafe_review_lane_evidence_degrades_on_unknown_schema() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;
        fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{"schema_version": "unsafe-review-gate/v99", "status": "advisory"}"#,
        )?;
        let block = super::render_unsafe_review_lane_evidence(&sensor_dir, "ok");
        assert!(
            block.contains("schema_version `unsafe-review-gate/v99` not recognised"),
            "degradation note must appear: {block}"
        );
        Ok(())
    }

    // ── #360 inline comment tests ─────────────────────────────────────────────

    /// Helper: write the canonical v1 gate manifest + a comment-plan with one
    /// `changed_line: true` entry pointing at `src/lib.rs:8`.
    fn write_inline_comment_fixtures(
        out_dir: &Path,
        comment_plan_json: &str,
        repair_queue_json: Option<&str>,
    ) -> Result<()> {
        fs::create_dir_all(out_dir)?;
        fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{
                "schema_version": "unsafe-review-gate/v1",
                "dialect": "unsafe-review",
                "status": "advisory",
                "summary": {"new_gaps": 1, "worsened_gaps": 0, "resolved_gaps": 0, "inherited_gaps": 0},
                "artifacts": {
                    "comment_plan": "comment-plan.json",
                    "repair_queue": "repair-queue.json"
                },
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict",
                "tool": "unsafe-review",
                "tool_version": "0.3.4"
            }"#,
        )?;
        fs::write(out_dir.join("comment-plan.json"), comment_plan_json)?;
        if let Some(rq) = repair_queue_json {
            fs::write(out_dir.join("repair-queue.json"), rq)?;
        }
        Ok(())
    }

    fn unsafe_review_right_side_lines(
        path: &str,
        line: u32,
    ) -> std::collections::BTreeSet<(String, u32)> {
        let mut lines = std::collections::BTreeSet::new();
        lines.insert((super::normalize_repo_path(path), line));
        lines
    }

    /// `changed_line: true` entry → one inline comment is produced at the
    /// correct `path:line`, body contains `coverage_gap`, `confirmation_state`,
    /// and the advisory `trust_boundary`. No suggestion block is set.
    #[test]
    fn build_unsafe_review_inline_comments_produces_comment_for_changed_line() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-001",
                "path": "src/lib.rs",
                "line": 8,
                "changed_line": true,
                "coverage_gap": "raw_pointer_read without alignment guard",
                "selection_reason": "changed line in unsafe block",
                "selection_reason_code": "changed-line-unsafe",
                "confirmation_state": "unconfirmed",
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict"
            }]"#,
            None,
        )?;
        let existing = std::collections::BTreeSet::new();
        let right_side_lines = unsafe_review_right_side_lines("src/lib.rs", 8);
        let comments = super::build_unsafe_review_inline_comments(
            &sensor_dir,
            &existing,
            &right_side_lines,
            8,
        );
        assert_eq!(comments.len(), 1, "expected exactly one inline comment");
        let c = &comments[0];
        assert_eq!(c.path, "src/lib.rs");
        assert_eq!(c.line, 8);
        assert_eq!(c.side, "RIGHT");
        assert!(
            c.body.contains("raw_pointer_read without alignment guard"),
            "body must contain coverage_gap: {}",
            c.body
        );
        assert!(
            c.body.contains("unconfirmed"),
            "body must name the confirmation_state: {}",
            c.body
        );
        assert!(
            c.body.contains("advisory"),
            "body must surface trust_boundary: {}",
            c.body
        );
        // No suggestion block: repair-queue/0.1 provides no replacement text.
        assert!(
            c.suggestion.is_none(),
            "suggestion must be None when repair-queue has no edit"
        );
        Ok(())
    }

    /// `changed_line: false` → the entry is skipped (GitHub API requires
    /// inline comments to anchor to changed diff lines only).
    #[test]
    fn build_unsafe_review_inline_comments_skips_unchanged_line() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-002",
                "path": "src/lib.rs",
                "line": 5,
                "changed_line": false,
                "coverage_gap": "guard_coverage: weak",
                "confirmation_state": "unconfirmed"
            }]"#,
            None,
        )?;
        let existing = std::collections::BTreeSet::new();
        let right_side_lines = unsafe_review_right_side_lines("src/lib.rs", 5);
        let comments = super::build_unsafe_review_inline_comments(
            &sensor_dir,
            &existing,
            &right_side_lines,
            8,
        );
        assert!(
            comments.is_empty(),
            "unchanged-line entry must be skipped, got {comments:?}"
        );
        Ok(())
    }

    /// `changed_line: true` is treated as a tool claim, not proof. The
    /// compiler's diff line map is the authority for whether a candidate can
    /// become a GitHub RIGHT-side inline comment.
    #[test]
    fn build_unsafe_review_inline_comments_skips_non_diff_anchor() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-002b",
                "path": "src/lib.rs",
                "line": 99,
                "changed_line": true,
                "coverage_gap": "guard_coverage: weak",
                "confirmation_state": "unconfirmed"
            }]"#,
            None,
        )?;
        let existing = std::collections::BTreeSet::new();
        let right_side_lines = unsafe_review_right_side_lines("src/lib.rs", 8);
        let comments = super::build_unsafe_review_inline_comments(
            &sensor_dir,
            &existing,
            &right_side_lines,
            8,
        );
        assert!(
            comments.is_empty(),
            "stale changed_line claim must be skipped, got {comments:?}"
        );
        Ok(())
    }

    #[test]
    fn unsafe_review_comment_plan_enters_compiler_intake() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-compiler-001",
                "path": "src/lib.rs",
                "line": 8,
                "changed_line": true,
                "coverage_gap": "raw_pointer_read without alignment guard",
                "selection_reason": "changed line in unsafe block",
                "confirmation_state": "unconfirmed",
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict"
            }]"#,
            None,
        )?;
        let line_map = unsafe_review_right_side_lines("src/lib.rs", 8);
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();

        super::apply_unsafe_review_comment_plan_candidates(
            &sensor_dir,
            &line_map,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut model_observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert_eq!(inline_comments.len(), 1);
        assert!(summary_only_findings.is_empty());
        let comment = &inline_comments[0];
        assert_eq!(comment.lane, "unsafe-review");
        assert_eq!(comment.severity, "medium");
        assert_eq!(comment.confidence, "medium-high");
        assert_eq!(comment.path, "src/lib.rs");
        assert_eq!(comment.line, 8);
        assert_eq!(comment.side, "RIGHT");
        assert!(comment.body.starts_with("[unsafe-review]"));
        assert!(
            comment
                .body
                .contains("raw_pointer_read without alignment guard")
        );
        assert!(comment.evidence.contains("changed line in unsafe block"));
        assert!(comment.suggestion.is_none());
        Ok(())
    }

    #[test]
    fn unsafe_review_comment_plan_suggestion_enters_compiler_intake() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-compiler-suggest",
                "path": "src/lib.rs",
                "line": 8,
                "changed_line": true,
                "coverage_gap": "raw_pointer_read without alignment guard",
                "selection_reason": "changed line in unsafe block",
                "confirmation_state": "unconfirmed"
            }]"#,
            Some(
                r#"{
                "schema_version": "0.2",
                "buckets": {
                    "repairable_by_guard": [{
                        "card_id": "card-compiler-suggest",
                        "bucket_reason": "guard_evidence_missing",
                        "applicable_edit": {
                            "suggestion_text": "let header = guarded_header_read(ptr)?;"
                        }
                    }]
                }
            }"#,
            ),
        )?;
        let line_map = unsafe_review_right_side_lines("src/lib.rs", 8);
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();

        super::apply_unsafe_review_comment_plan_candidates(
            &sensor_dir,
            &line_map,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut model_observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert_eq!(inline_comments.len(), 1);
        assert!(summary_only_findings.is_empty());
        assert_eq!(
            inline_comments[0].suggestion.as_deref(),
            Some("let header = guarded_header_read(ptr)?;")
        );
        Ok(())
    }

    #[test]
    fn unsafe_review_comment_plan_stale_anchor_uses_shared_inline_guard() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-compiler-002",
                "path": "src/lib.rs",
                "line": 99,
                "changed_line": true,
                "coverage_gap": "guard_coverage: weak",
                "selection_reason": "changed line in unsafe block",
                "confirmation_state": "unconfirmed"
            }]"#,
            None,
        )?;
        let line_map = unsafe_review_right_side_lines("src/lib.rs", 8);
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
        let mut issue_candidates = Vec::new();

        super::apply_unsafe_review_comment_plan_candidates(
            &sensor_dir,
            &line_map,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut model_observations,
                proof_requests: &mut proof_requests,
                proof_intents: &mut Vec::new(),
                issue_candidates: &mut issue_candidates,
            },
        );

        assert!(inline_comments.is_empty());
        assert_eq!(summary_only_findings.len(), 1);
        assert_eq!(summary_only_findings[0].lane, "unsafe-review");
        assert!(
            summary_only_findings[0].reason.contains("line_valid=false"),
            "stale anchors must be rejected by validate_inline_candidate: {:?}",
            summary_only_findings[0]
        );
        Ok(())
    }

    /// Dedup: a path:line already claimed by a model lane is skipped so the
    /// same location is not posted twice.
    #[test]
    fn build_unsafe_review_inline_comments_deduplicates_against_existing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-003",
                "path": "src/lib.rs",
                "line": 8,
                "changed_line": true,
                "coverage_gap": "transmute size mismatch",
                "confirmation_state": "unconfirmed"
            }]"#,
            None,
        )?;
        // Simulate a model lane already posting to src/lib.rs:8.
        let mut existing = std::collections::BTreeSet::new();
        existing.insert(("src/lib.rs".to_owned(), 8u32));
        let right_side_lines = unsafe_review_right_side_lines("src/lib.rs", 8);
        let comments = super::build_unsafe_review_inline_comments(
            &sensor_dir,
            &existing,
            &right_side_lines,
            8,
        );
        assert!(
            comments.is_empty(),
            "duplicate path:line must be deduped, got {comments:?}"
        );
        Ok(())
    }

    /// Budget cap: if `max_inline_budget` is 0 no comments are produced, even
    /// when there are eligible candidates.
    #[test]
    fn build_unsafe_review_inline_comments_respects_budget_cap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-004",
                "path": "src/lib.rs",
                "line": 8,
                "changed_line": true,
                "coverage_gap": "pointer cast without validity check",
                "confirmation_state": "unconfirmed"
            }]"#,
            None,
        )?;
        let existing = std::collections::BTreeSet::new();
        let right_side_lines = unsafe_review_right_side_lines("src/lib.rs", 8);
        let comments = super::build_unsafe_review_inline_comments(
            &sensor_dir,
            &existing,
            &right_side_lines,
            0,
        );
        assert!(
            comments.is_empty(),
            "budget=0 must produce no comments, got {comments:?}"
        );
        Ok(())
    }

    /// Repair-queue with a `repairable_by_guard` entry: the bucket context
    /// (bucket_reason, operation, missing_evidence) is surfaced in the body.
    /// Crucially, `suggestion` is still `None` because the repair queue does
    /// NOT provide a concrete replacement text — this is the honest capability
    /// finding for the #360 follow-up issue.
    #[test]
    fn build_unsafe_review_inline_comments_surfaces_repair_queue_context() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        // Repair-queue shape from real unsafe-review 0.3.4 output: guidance and
        // classification are present, but no concrete replacement text exists.
        let repair_queue = r#"{
            "schema_version": "0.1",
            "tool": "unsafe-review",
            "mode": "aggregate_repair_queue",
            "source": "review_card",
            "policy": "advisory",
            "trust_boundary": "static unsafe contract review only",
            "buckets": {
                "repairable_by_guard": [{
                    "card_id": "card-001",
                    "class": "guard_missing",
                    "priority": "high",
                    "confidence": "medium",
                    "operation": "unsafe { ptr.cast::<Header>().read() }",
                    "missing_evidence": ["Missing visible local guard", "No witness receipt"],
                    "bucket_reason": "guard_evidence_missing"
                }],
                "repairable_by_safety_docs": [],
                "repairable_by_test": [],
                "requires_witness_receipt": [],
                "requires_human_review": [],
                "do_not_auto_repair": []
            }
        }"#;
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-001",
                "path": "src/lib.rs",
                "line": 8,
                "changed_line": true,
                "coverage_gap": "raw_pointer_read without alignment guard",
                "confirmation_state": "unconfirmed",
                "trust_boundary": "static unsafe-review coverage evidence; not proof, not a merge verdict"
            }]"#,
            Some(repair_queue),
        )?;
        let existing = std::collections::BTreeSet::new();
        let right_side_lines = unsafe_review_right_side_lines("src/lib.rs", 8);
        let comments = super::build_unsafe_review_inline_comments(
            &sensor_dir,
            &existing,
            &right_side_lines,
            8,
        );
        assert_eq!(comments.len(), 1);
        let c = &comments[0];
        // Repair-queue context surfaces bucket reason, operation, and evidence.
        assert!(
            c.body.contains("guard_evidence_missing"),
            "body must contain bucket_reason: {}",
            c.body
        );
        assert!(
            c.body.contains("ptr.cast"),
            "body must contain operation from repair-queue: {}",
            c.body
        );
        assert!(
            c.body.contains("Missing visible local guard"),
            "body must contain missing_evidence: {}",
            c.body
        );
        // Honest finding: repair-queue/0.1 has no replacement text → no
        // suggestion block can be emitted. This is the evidence for the
        // follow-up issue "repair-queue should emit applicable edits for
        // suggestion blocks".
        assert!(
            c.suggestion.is_none(),
            "suggestion must be None — repair-queue/0.1 provides no replacement text, \
             only guidance; fabricating edits is explicitly prohibited"
        );
        Ok(())
    }

    /// Future repair-queue producers may provide concrete replacement text.
    /// Only then can ub-review prepare a GitHub suggestion block; guidance
    /// fields alone are still never promoted into edits.
    #[test]
    fn build_unsafe_review_inline_comments_uses_concrete_repair_queue_suggestion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        let repair_queue = r#"{
            "schema_version": "0.2",
            "tool": "unsafe-review",
            "mode": "aggregate_repair_queue",
            "buckets": {
                "repairable_by_guard": [{
                    "card_id": "card-suggest-001",
                    "operation": "unsafe { ptr.cast::<Header>().read() }",
                    "missing_evidence": ["Missing visible local guard"],
                    "bucket_reason": "guard_evidence_missing",
                    "replacement_text": "let header = guarded_header_read(ptr)?;"
                }],
                "requires_human_review": []
            }
        }"#;
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-suggest-001",
                "path": "src/lib.rs",
                "line": 8,
                "changed_line": true,
                "coverage_gap": "raw_pointer_read without alignment guard",
                "selection_reason": "changed line in unsafe block",
                "confirmation_state": "unconfirmed"
            }]"#,
            Some(repair_queue),
        )?;
        let existing = std::collections::BTreeSet::new();
        let right_side_lines = unsafe_review_right_side_lines("src/lib.rs", 8);
        let comments = super::build_unsafe_review_inline_comments(
            &sensor_dir,
            &existing,
            &right_side_lines,
            8,
        );
        assert_eq!(comments.len(), 1);
        assert_eq!(
            comments[0].suggestion.as_deref(),
            Some("let header = guarded_header_read(ptr)?;")
        );
        assert!(
            comments[0].body.contains("guard_evidence_missing"),
            "body should still carry repair context: {}",
            comments[0].body
        );
        Ok(())
    }

    /// Absent repair-queue file: `build_unsafe_review_inline_comments` still
    /// produces comments (degrades gracefully — repair-queue context is
    /// optional).
    #[test]
    fn build_unsafe_review_inline_comments_works_without_repair_queue() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        // No repair-queue.json written.
        write_inline_comment_fixtures(
            &out_dir,
            r#"[{
                "card_id": "card-005",
                "path": "src/ffi.rs",
                "line": 12,
                "changed_line": true,
                "coverage_gap": "slice::from_raw_parts without length check",
                "confirmation_state": "unconfirmed"
            }]"#,
            None,
        )?;
        let existing = std::collections::BTreeSet::new();
        let right_side_lines = unsafe_review_right_side_lines("src/ffi.rs", 12);
        let comments = super::build_unsafe_review_inline_comments(
            &sensor_dir,
            &existing,
            &right_side_lines,
            8,
        );
        assert_eq!(
            comments.len(),
            1,
            "comment must be produced without repair-queue"
        );
        assert_eq!(comments[0].path, "src/ffi.rs");
        assert_eq!(comments[0].line, 12);
        assert!(comments[0].suggestion.is_none());
        Ok(())
    }

    /// `suggestion` field is omitted from JSON serialisation when `None`, and
    /// present as a string when set. Validates the serde round-trip contract
    /// so the GitHub API payload stays clean.
    #[test]
    fn github_review_comment_suggestion_serialises_correctly() -> Result<()> {
        let without = super::GitHubReviewComment {
            path: "src/lib.rs".to_owned(),
            line: 8,
            side: "RIGHT".to_owned(),
            body: "[unsafe-review] gap".to_owned(),
            suggestion: None,
        };
        let json = serde_json::to_string(&without)?;
        assert!(
            !json.contains("suggestion"),
            "suggestion must be omitted when None: {json}"
        );

        let with_suggestion = super::GitHubReviewComment {
            suggestion: Some(
                "// SAFETY: alignment verified above\nunsafe { ptr.cast::<Header>().read() }"
                    .to_owned(),
            ),
            ..without
        };
        let json_with = serde_json::to_string(&with_suggestion)?;
        assert!(
            json_with.contains("suggestion"),
            "suggestion must appear when Some: {json_with}"
        );
        assert!(
            json_with.contains("alignment verified above"),
            "suggestion content must be serialised: {json_with}"
        );
        Ok(())
    }

    #[test]
    fn github_review_post_payload_renders_suggestion_without_internal_field() -> Result<()> {
        let review = super::GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Verification questions\n\n- Confirm the unsafe guard proof.".to_owned(),
            comments: vec![super::GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 8,
                side: "RIGHT".to_owned(),
                body: "[unsafe-review] Guard evidence is missing.".to_owned(),
                suggestion: Some("let header = guarded_header_read(ptr)?;".to_owned()),
            }],
        };

        super::validate_github_review_payload(&review)?;
        let payload = super::github_review_post_payload(&review)?;
        let json = serde_json::to_value(&payload)?;
        assert!(
            json["comments"][0].get("suggestion").is_none(),
            "post payload must not leak the internal suggestion field: {json}"
        );
        let body = json["comments"][0]["body"].as_str().unwrap_or_default();
        assert!(
            body.contains("```suggestion\nlet header = guarded_header_read(ptr)?;\n```"),
            "post body must render GitHub suggestion markdown: {body}"
        );
        Ok(())
    }

    #[test]
    fn github_review_payload_rejects_non_unsafe_review_suggestion() -> Result<()> {
        let review = super::GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Verification questions\n\n- Confirm the test proof.".to_owned(),
            comments: vec![super::GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 8,
                side: "RIGHT".to_owned(),
                body: "[tests] A model lane cannot provide one-click edits.".to_owned(),
                suggestion: Some("assert!(proved);".to_owned()),
            }],
        };
        let err = super::validate_github_review_payload(&review)
            .err()
            .ok_or_else(|| anyhow::anyhow!("non-unsafe suggestion unexpectedly passed"))?;
        assert!(
            err.to_string().contains("sourced from unsafe-review"),
            "{err:#}"
        );
        Ok(())
    }

    /// `read_repair_queue` with a real-shape v0.1 file: entries are indexed by
    /// card_id, the first bucket hit wins for a card appearing in multiple
    /// buckets.
    #[test]
    fn read_repair_queue_indexes_entries_by_card_id() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;
        // Minimal v1 gate with a repair_queue pointer.
        fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{
                "schema_version": "unsafe-review-gate/v1",
                "status": "advisory",
                "artifacts": {"repair_queue": "repair-queue.json"}
            }"#,
        )?;
        fs::write(
            out_dir.join("repair-queue.json"),
            r#"{
                "schema_version": "0.1",
                "buckets": {
                    "repairable_by_guard": [{
                        "card_id": "cid-1",
                        "operation": "unsafe { *ptr }",
                        "missing_evidence": ["needs alignment proof"],
                        "bucket_reason": "guard_evidence_missing"
                    }],
                    "requires_witness_receipt": [{
                        "card_id": "cid-1",
                        "operation": "unsafe { *ptr }",
                        "missing_evidence": ["no witness receipt"],
                        "bucket_reason": "witness_receipt_missing"
                    }, {
                        "card_id": "cid-2",
                        "operation": "unsafe { slice::from_raw_parts(p, n) }",
                        "missing_evidence": ["length not verified"],
                        "bucket_reason": "witness_receipt_missing"
                    }],
                    "requires_human_review": [],
                    "do_not_auto_repair": []
                }
            }"#,
        )?;
        let artifacts = super::read_unsafe_review_artifacts(&sensor_dir)
            .map_err(|gap| anyhow::anyhow!("expected ingested artifacts, got gap: {gap:?}"))?;
        let map = super::read_repair_queue(&sensor_dir, &artifacts.gate);
        assert_eq!(map.len(), 2, "two distinct card_ids expected");
        // cid-1 should keep the first bucket hit (repairable_by_guard).
        let e1 = map
            .get("cid-1")
            .ok_or_else(|| anyhow::anyhow!("cid-1 missing"))?;
        assert_eq!(
            e1.bucket_reason.as_deref(),
            Some("guard_evidence_missing"),
            "first bucket hit must win for cid-1"
        );
        assert_eq!(e1.missing_evidence.len(), 1);
        // cid-2 only appears in requires_witness_receipt.
        let e2 = map
            .get("cid-2")
            .ok_or_else(|| anyhow::anyhow!("cid-2 missing"))?;
        assert_eq!(e2.bucket_reason.as_deref(), Some("witness_receipt_missing"));
        Ok(())
    }

    /// Absent repair-queue file: `read_repair_queue` returns an empty map,
    /// never panics or errors.
    #[test]
    fn read_repair_queue_absent_file_returns_empty_map() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let sensor_dir = temp.path().join("sensors/unsafe-review");
        let out_dir = sensor_dir.join(super::UNSAFE_REVIEW_OUTPUT_SUBDIR);
        fs::create_dir_all(&out_dir)?;
        // Gate file exists but no repair-queue.json.
        fs::write(
            out_dir.join("unsafe-review-gate.json"),
            r#"{
                "schema_version": "unsafe-review-gate/v1",
                "status": "advisory",
                "artifacts": {"repair_queue": "repair-queue.json"}
            }"#,
        )?;
        let artifacts = super::read_unsafe_review_artifacts(&sensor_dir)
            .map_err(|gap| anyhow::anyhow!("expected ingested artifacts, got gap: {gap:?}"))?;
        let map = super::read_repair_queue(&sensor_dir, &artifacts.gate);
        assert!(map.is_empty(), "absent repair-queue must return empty map");
        Ok(())
    }
}
