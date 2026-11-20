//! Offline comparison of observations emitted by two ordinary run packets.
//! Packet directories are immutable local inputs. No Git, producer or gate is invoked.
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{ErrorKind, Read};
use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

use crate::revision_admission::RevisionAdmission;
use crate::revision_identity::{RevisionIdentity, RevisionRef};
use crate::{ProofReceipt, sha256_hex};

const ADMISSION: &str = "input/revision-admission.json";
const PROOF: &str = "review/proof_receipts.json";
const VERDICTS: &str = "review/gate_outcome.json";
const COST: &str = "review/ub-review-cost.json";
const PATCH: &str = "input/diff.patch";
const PATHS: &str = "input/changed-files.txt";
// Exact bytes of the emitted review inputs, excluding machine timing and outputs.
const INPUTS: &[&str] = &[
    PATCH,
    PATHS,
    "input/diff-context.json",
    "input/pr.md",
    "input/claims.md",
    "review/pr_thread_context.json",
    "review/shared_context.md",
    "effective-config.json",
    "plan.json",
    "resolved-plan.json",
    "resolved-profile.json",
    "resolved-tools.json",
];
const FILE_LIMIT: u64 = 8 * 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum Gap {
    Missing,
    Unreadable,
    Invalid,
    Oversize,
    UnsupportedSchema,
    LegacyUnbound,
    StaleRevision,
    DirtyWorktree,
    InputUnavailable,
}

type Observation<T> = std::result::Result<T, Gap>;

fn io_gap(e: std::io::Error) -> Gap {
    if e.kind() == ErrorKind::NotFound {
        Gap::Missing
    } else {
        Gap::Unreadable
    }
}

fn read(root: &Path, relative: &str) -> Observation<Vec<u8>> {
    let target = root.join(relative);
    let mut path = root.to_owned();
    for component in relative.split('/') {
        path.push(component);
        let meta = fs::symlink_metadata(&path).map_err(io_gap)?;
        if meta.file_type().is_symlink() {
            return Err(Gap::Invalid);
        }
        if path == target && !meta.is_file() {
            return Err(Gap::Invalid);
        }
    }
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(io_gap)?
        .take(FILE_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(io_gap)?;
    if bytes.len() as u64 > FILE_LIMIT {
        return Err(Gap::Oversize);
    }
    Ok(bytes)
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Observation<T> {
    serde_json::from_slice(bytes).map_err(|_| Gap::Invalid)
}

#[derive(Serialize, PartialEq)]
struct RecordedInput {
    revision: RevisionRef,
    run: RunOptions,
    started: Started,
    sha256: BTreeMap<&'static str, String>,
}

#[derive(Deserialize, Serialize, PartialEq)]
struct RunOptions {
    shared_context_id: String,
    mode: String,
    model_mode: String,
    model_provider_policy: String,
    provider_policy: String,
    model_timeout_sec: u64,
    ledger_path: String,
    ledger_max_bytes: usize,
}

#[derive(Deserialize, Serialize, PartialEq)]
struct Started {
    dry_run: bool,
    sensor_phases: String,
}

fn run_start(root: &Path) -> Observation<Started> {
    #[derive(Deserialize)]
    struct Event<'a> {
        kind: &'a str,
        payload: Value,
    }
    let bytes = read(root, "events.ndjson")?;
    let mut start = None;
    for line in bytes.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let event: Event = serde_json::from_slice(line).map_err(|_| Gap::Invalid)?;
        if event.kind == "run_started" {
            if start.is_some() {
                return Err(Gap::Invalid);
            }
            start = Some(serde_json::from_value(event.payload).map_err(|_| Gap::Invalid)?);
        }
    }
    start.ok_or(Gap::Missing)
}

fn recorded_input(root: &Path) -> Observation<RecordedInput> {
    let admission: RevisionAdmission = decode(&read(root, ADMISSION)?)?;
    admission.validate().map_err(|_| Gap::Invalid)?;
    let identity = RevisionIdentity::from_canonical(&admission.identity_canonical)
        .map_err(|_| Gap::Invalid)?;
    let revision = identity.revision_ref();
    if revision != RevisionRef::from_admission(&admission)
        || admission
            .pr_head_commit
            .as_deref()
            .is_some_and(|oid| oid != identity.candidate_head_commit())
    {
        return Err(Gap::StaleRevision);
    }
    if admission.worktree_dirty {
        return Err(Gap::DirtyWorktree);
    }
    let mut sha256 = BTreeMap::new();
    for &name in INPUTS {
        let bytes = read(root, name)?;
        if name == PATCH && !identity.matches_diff_bytes(&bytes) {
            return Err(Gap::StaleRevision);
        }
        if name == PATHS {
            let text = std::str::from_utf8(&bytes).map_err(|_| Gap::Invalid)?;
            let paths = text.lines().map(str::to_owned).collect::<Vec<_>>();
            if !identity.matches_changed_paths(&paths) {
                return Err(Gap::StaleRevision);
            }
        }
        if name == "input/diff-context.json" {
            let diff: crate::DiffContext = decode(&bytes)?;
            if !identity.matches_diff_bytes(diff.patch.as_bytes())
                || !identity.matches_changed_paths(&diff.changed_files)
            {
                return Err(Gap::StaleRevision);
            }
        } else if name == "plan.json" {
            let plan: crate::Plan = decode(&bytes)?;
            if !identity.matches_changed_paths(&plan.changed_files) {
                return Err(Gap::StaleRevision);
            }
        } else if name.ends_with(".json") {
            let value: Value = decode(&bytes)?;
            if !value.is_object() {
                return Err(Gap::Invalid);
            }
            let schema = match name {
                "resolved-tools.json" => Some(crate::artifacts::RESOLVED_TOOLS_SCHEMA),
                "resolved-plan.json" => Some(crate::artifacts::RESOLVED_PLAN_SCHEMA),
                "resolved-profile.json" => Some(crate::artifacts::RESOLVED_PROFILE_SCHEMA),
                "review/pr_thread_context.json" => Some(crate::artifacts::PR_THREAD_CONTEXT_SCHEMA),
                _ => None,
            };
            if schema.is_some() && value["schema"].as_str() != schema {
                return Err(Gap::UnsupportedSchema);
            }
        }
        sha256.insert(name, sha256_hex(&bytes));
    }
    let run: RunOptions = decode(&read(root, "review/review.json")?)?;
    if sha256.get("review/shared_context.md") != Some(&run.shared_context_id) {
        return Err(Gap::Invalid);
    }
    Ok(RecordedInput {
        revision,
        sha256,
        run,
        started: run_start(root)?,
    })
}

fn joined(revision: Option<&RevisionRef>, input: &Observation<RecordedInput>) -> Observation<()> {
    let revision = revision.ok_or(Gap::LegacyUnbound)?;
    revision.validate().map_err(|_| Gap::Invalid)?;
    if revision != &input.as_ref().map_err(|_| Gap::InputUnavailable)?.revision {
        return Err(Gap::StaleRevision);
    }
    Ok(())
}

fn proof(bytes: &[u8], input: &Observation<RecordedInput>) -> Observation<Value> {
    let receipts: Vec<ProofReceipt> = decode(bytes)?;
    let mut rows = BTreeMap::new();
    for receipt in receipts {
        if receipt.schema != crate::artifacts::PROOF_RECEIPT_SCHEMA {
            return Err(Gap::UnsupportedSchema);
        }
        joined(receipt.revision.as_ref(), input)?;
        if !matches!(
            receipt.result.as_str(),
            "head_passed"
                | "head_failed"
                | "timed_out"
                | "discriminating"
                | "non_discriminating"
                | "skipped_profile"
                | "skipped_budget"
                | "skipped_unresolved"
                | "base_patch_failed"
                | "terminated_without_exit_code"
        ) {
            return Err(Gap::Invalid);
        }
        if receipt.id.is_empty()
            || rows.contains_key(&receipt.id)
            || receipt.commands.is_empty()
            || crate::proof::proof_receipt_has_contradictory_completion(&receipt)
        {
            return Err(Gap::Invalid);
        }
        // Keep command identity without exposing commands, environment or private output.
        let commands: Vec<_> = receipt
            .commands
            .iter()
            .map(|cmd| {
                let env = serde_json::to_vec(&cmd.env).map_err(|_| Gap::Invalid)?;
                Ok(json!({
                    "side": cmd.side, "command_sha256": sha256_hex(cmd.command.as_bytes()),
                    "env_sha256": sha256_hex(&env),
                    "status": cmd.status, "exit_code": cmd.exit_code, "timed_out": cmd.timed_out,
                }))
            })
            .collect::<Observation<_>>()?;
        rows.insert(
            receipt.id,
            json!({"reported_result": receipt.result, "kind": receipt.kind,
            "test_patch_mode": receipt.test_patch_mode, "request_ids": receipt.request_ids,
            "requested_by": receipt.requested_by, "commands": commands}),
        );
    }
    Ok(json!(rows))
}

#[derive(Deserialize)]
struct Cost {
    runner_kind: String,
    fallback_used: bool,
    cap_minutes: u64,
    target_minutes: u64,
    schema: String,
    revision: Option<RevisionRef>,
    required_floor_wall_seconds: Option<f64>,
    estimated_cost_usd: Option<f64>,
    llm_seconds: f64,
    tokens: Tokens,
    cache: Cache,
    cost_basis: Basis,
}

#[derive(Deserialize, Serialize)]
struct Tokens {
    fresh_input: u64,
    cached_input: u64,
    output: u64,
}
#[derive(Deserialize)]
struct Cache {
    cargo: String,
    model_prefix: String,
}
#[derive(Deserialize)]
struct Basis {
    runner_minutes: f64,
    linux_minute_rate_usd: Option<f64>,
    token_pricing: String,
}

fn economics(bytes: &[u8], input: &Observation<RecordedInput>) -> Observation<Value> {
    let cost: Cost = decode(bytes)?;
    if cost.schema != crate::artifacts::COST_RECEIPT_SCHEMA {
        return Err(Gap::UnsupportedSchema);
    }
    joined(cost.revision.as_ref(), input)?;
    if [
        Some(cost.llm_seconds),
        Some(cost.cost_basis.runner_minutes),
        cost.required_floor_wall_seconds,
        cost.estimated_cost_usd,
        cost.cost_basis.linux_minute_rate_usd,
    ]
    .into_iter()
    .flatten()
    .any(|v| v < 0.0)
        || cost.cost_basis.token_pricing != "excluded_v1"
    {
        return Err(Gap::Invalid);
    }
    if !matches!(
        cost.runner_kind.as_str(),
        "unknown" | "github-hosted" | "self-hosted" | "local"
    ) {
        return Err(Gap::Invalid);
    }
    let cache = |s: &str| match s {
        "hit" | "partial" | "miss" => Ok(json!(s)),
        "unknown" => Ok(Value::Null),
        _ => Err(Gap::Invalid),
    };
    Ok(json!({
        "runner_kind": (cost.runner_kind != "unknown").then_some(cost.runner_kind), "fallback_used": cost.fallback_used,
        "cap_minutes": cost.cap_minutes, "target_minutes": cost.target_minutes,
        "runner_minutes": cost.cost_basis.runner_minutes, "llm_seconds": cost.llm_seconds,
        "required_floor_wall_seconds": cost.required_floor_wall_seconds,
        "estimated_cost_usd": cost.estimated_cost_usd,
        "linux_minute_rate_usd": cost.cost_basis.linux_minute_rate_usd,
        "token_pricing": cost.cost_basis.token_pricing,
        "tokens": cost.tokens, "cargo_cache": cache(&cost.cache.cargo)?,
        "model_prefix_cache": cache(&cost.cache.model_prefix)?,
    }))
}

// The legacy conclusion and the later split verdicts are observations of the
// original run. A legacy row can lack all three split verdicts.
fn verdicts(bytes: &[u8], input: &Observation<RecordedInput>) -> Observation<Value> {
    #[derive(Deserialize)]
    struct Verdicts {
        schema: String,
        revision: Option<RevisionRef>,
        conclusion: String,
        analysis_result: Option<String>,
        publication_result: Option<String>,
        gate_result: Option<String>,
    }
    let v: Verdicts = decode(bytes)?;
    if v.schema != crate::artifacts::GATE_OUTCOME_SCHEMA {
        return Err(Gap::UnsupportedSchema);
    }
    joined(v.revision.as_ref(), input)?;
    if !matches!(v.conclusion.as_str(), "pass" | "fail" | "inconclusive")
        || !matches!(
            v.analysis_result.as_deref(),
            None | Some("clean" | "findings" | "limited" | "not_proven")
        )
        || !matches!(
            v.publication_result.as_deref(),
            None | Some("posted" | "not_needed" | "failed" | "not_proven")
        )
        || !matches!(
            v.gate_result.as_deref(),
            None | Some("pass" | "finding" | "not_proven")
        )
    {
        return Err(Gap::Invalid);
    }
    Ok(
        json!({"conclusion":v.conclusion, "analysis_result":v.analysis_result,
        "publication_result":v.publication_result, "gate_result":v.gate_result}),
    )
}

fn compare_values(a: Option<&Value>, b: Option<&Value>, matched: bool) -> &'static str {
    if !matched || a.is_none_or(Value::is_null) || b.is_none_or(Value::is_null) {
        "unresolved"
    } else if a == b {
        "agreement"
    } else {
        "different"
    }
}

fn observed<T: Serialize>(result: &Observation<T>) -> Value {
    match result {
        Ok(value) => json!({"status": "available", "value": value}),
        Err(reason) => json!({"status": reason}),
    }
}

fn plane(path: &str, left: Observation<Value>, right: Observation<Value>, matched: bool) -> Value {
    let fields: BTreeMap<_, _> = left
        .iter()
        .chain(right.iter())
        .filter_map(Value::as_object)
        .flat_map(|v| v.keys())
        .map(|key| {
            let a = left.as_ref().ok().and_then(|v| v.get(key));
            let b = right.as_ref().ok().and_then(|v| v.get(key));
            let status = match (a, b) {
                (Some(_), None) if matched && right.is_ok() => "stable_only",
                (None, Some(_)) if matched && left.is_ok() => "candidate_only",
                _ => compare_values(a, b, matched),
            };
            (key, status)
        })
        .collect();
    let status = if !matched || left.is_err() || right.is_err() {
        "unresolved"
    } else if fields
        .values()
        .any(|s| matches!(*s, "different" | "stable_only" | "candidate_only"))
    {
        "different"
    } else if fields.values().any(|s| *s == "unresolved") {
        "unresolved"
    } else {
        "agreement"
    };
    json!({"source":path, "status":status, "fields":fields, "stable":observed(&left), "candidate":observed(&right)})
}

fn compare(stable: &Path, candidate: &Path) -> Value {
    let left = recorded_input(stable);
    let right = recorded_input(candidate);
    let status = match (&left, &right) {
        (Ok(a), Ok(b)) if a == b => "matched",
        (Ok(_), Ok(_)) => "different",
        _ => "unresolved",
    };
    let matched = status == "matched";
    let report = |path, project: fn(&[u8], &Observation<RecordedInput>) -> Observation<Value>| {
        let stable_bytes = read(stable, path);
        let candidate_bytes = read(candidate, path);
        let source_sha256 = json!({
            "stable": stable_bytes.as_ref().ok().map(|b| sha256_hex(b)),
            "candidate": candidate_bytes.as_ref().ok().map(|b| sha256_hex(b)),
        });
        let mut result = plane(
            path,
            stable_bytes.and_then(|b| project(&b, &left)),
            candidate_bytes.and_then(|b| project(&b, &right)),
            matched,
        );
        result["source_sha256"] = source_sha256;
        result
    };
    json!({"schema":"ub-review.offline_packet_comparison.v1",
        "recorded_input":{"status":status,"stable":observed(&left),"candidate":observed(&right)},
        "proof":report(PROOF,proof), "economics":report(COST,economics),
        "verdicts":report(VERDICTS,verdicts)})
}

pub(crate) fn run(stable: &Path, candidate: &Path) -> Result<()> {
    anyhow::ensure!(
        stable.is_dir() && candidate.is_dir(),
        "both packet roots must be directories"
    );
    println!(
        "{}",
        serde_json::to_string_pretty(&compare(stable, candidate))?
    );
    Ok(())
}
