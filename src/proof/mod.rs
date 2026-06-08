//! Proof surface types and policy matching: requests, receipts, leases,
//! budgets, and the [[proof.required]] selectors (cleanup train step 8,
//! pure code motion).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::*;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ProofRequest {
    pub(crate) schema: String,
    pub(crate) id: String,
    pub(crate) lane: String,
    pub(crate) requested_by: Vec<String>,
    pub(crate) command: String,
    pub(crate) reason: String,
    pub(crate) cost: String,
    pub(crate) timeout_sec: u64,
    pub(crate) required: bool,
    pub(crate) status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ProofRequestGroup {
    pub(crate) schema: String,
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) cost: String,
    pub(crate) timeout_sec: u64,
    pub(crate) required: bool,
    pub(crate) status: String,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
    pub(crate) reasons: Vec<String>,
    pub(crate) duplicate_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ProofReceipt {
    pub(crate) schema: String,
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) test_patch_mode: String,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
    pub(crate) commands: Vec<ProofCommandReceipt>,
    pub(crate) result: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ProofCommandReceipt {
    pub(crate) side: String,
    pub(crate) command: String,
    #[serde(default)]
    pub(crate) env: BTreeMap<String, String>,
    pub(crate) status: String,
    pub(crate) exit_code: Option<i32>,
    pub(crate) timed_out: bool,
    pub(crate) timeout_sec: u64,
    pub(crate) duration_ms: u128,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ResourceLease {
    pub(crate) schema: String,
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) consumer: String,
    pub(crate) status: String,
    pub(crate) reason: String,
    pub(crate) cpu: u32,
    pub(crate) memory_mb: u64,
    pub(crate) disk_mb: u64,
    pub(crate) timeout_sec: u64,
    pub(crate) network: bool,
    pub(crate) scratch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) worktree: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) command: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProofPlannerInput<'a> {
    pub(crate) schema: &'static str,
    pub(crate) diff_class: &'static str,
    pub(crate) changed_files: &'a [String],
    pub(crate) pr_thread_context_status: &'a str,
    pub(crate) proof_requests: &'a [ProofRequest],
    pub(crate) runtime_budget: ProofPlannerRuntimeBudget,
    pub(crate) box_shape: &'a BoxState,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProofPlannerOutput {
    pub(crate) schema: &'static str,
    pub(crate) lane: &'static str,
    pub(crate) proof_tasks: Vec<ProofTaskArtifact>,
    pub(crate) skip: Vec<ProofPlannerSkip>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProofTaskArtifact {
    pub(crate) schema: &'static str,
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) source: String,
    pub(crate) priority: String,
    pub(crate) packet_policy: String,
    pub(crate) deadline_sec: u64,
    pub(crate) gate_policy: String,
    pub(crate) status: String,
    pub(crate) command: String,
    pub(crate) head_command: String,
    pub(crate) base_plus_tests_command: Option<String>,
    pub(crate) purpose: String,
    pub(crate) consumers: Vec<String>,
    pub(crate) value: String,
    pub(crate) cost: String,
    pub(crate) timeout_sec: u64,
    pub(crate) lease: ProofTaskLease,
    pub(crate) test_file: String,
    pub(crate) test_name: Option<String>,
    pub(crate) mode: String,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProofTaskLease {
    pub(crate) cpu: u32,
    pub(crate) memory_mb: u64,
    pub(crate) disk_mb: u64,
    pub(crate) network: bool,
    pub(crate) timeout_sec: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProofBudget {
    pub(crate) max_focused_test_files: usize,
    pub(crate) max_focused_tests: usize,
    pub(crate) per_command_timeout_sec: u64,
    pub(crate) max_total_seconds: u64,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ProofLeaseBudget {
    pub(crate) cpu: u32,
    pub(crate) memory_mb: u64,
    pub(crate) disk_mb: u64,
    pub(crate) network: bool,
    pub(crate) scratch: bool,
}

pub(crate) fn resolved_proof_policy_artifact(
    config: &Config,
    diff: &DiffContext,
    language_mix: &LanguageMix,
) -> serde_json::Value {
    let matched_required = config
        .proof
        .required
        .iter()
        .filter(|policy| required_proof_policy_matches_diff(policy, diff, language_mix))
        .collect::<Vec<_>>();
    serde_json::json!({
        "schema": PROOF_POLICY_RESOLUTION_SCHEMA,
        "required": &config.proof.required,
        "matched_required": matched_required,
    })
}

pub(crate) fn required_proof_policy_matches_diff(
    policy: &RequiredProofPolicy,
    diff: &DiffContext,
    language_mix: &LanguageMix,
) -> bool {
    policy.enabled
        && proof_policy_language_matches(&policy.languages, language_mix)
        && proof_policy_diff_class_matches(&policy.diff_classes, diff.diff_class.key())
}

pub(crate) fn proof_policy_language_matches(
    languages: &[String],
    language_mix: &LanguageMix,
) -> bool {
    if languages.is_empty() {
        return true;
    }
    languages.iter().any(|language| {
        let language = normalize_policy_selector(language);
        matches!(language.as_str(), "*" | "any" | "all")
            || (language == "mixed" && language_mix.mixed_language)
            || language_mix
                .languages
                .iter()
                .any(|candidate| candidate == &language)
            || language_mix.primary_language.as_deref() == Some(language.as_str())
    })
}

pub(crate) fn proof_policy_diff_class_matches(diff_classes: &[String], diff_class: &str) -> bool {
    if diff_classes.is_empty() {
        return true;
    }
    let diff_class = normalize_policy_selector(diff_class);
    diff_classes.iter().any(|candidate| {
        let candidate = normalize_policy_selector(candidate);
        matches!(candidate.as_str(), "*" | "any" | "all") || candidate == diff_class
    })
}
