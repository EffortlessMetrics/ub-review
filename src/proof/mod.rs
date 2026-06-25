//! Proof surface types and policy matching: requests, receipts, leases,
//! budgets, and the [[proof.required]] selectors (cleanup train step 8,
//! pure code motion).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::*;

pub(crate) mod command_parse;
pub(crate) use command_parse::*;
pub(crate) mod broker;
pub(crate) use broker::*;

pub(crate) mod budget;
pub(crate) use budget::*;

pub(crate) mod command;
pub(crate) use command::*;

pub(crate) mod tasks;
pub(crate) use tasks::*;

pub(crate) mod build;
pub(crate) use build::*;

pub(crate) mod planner;
pub(crate) use planner::*;

pub(crate) mod red_green;
pub(crate) use red_green::*;

pub(crate) mod worktree;
pub(crate) use worktree::*;

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

/// Typed proof intent (Order 2 of epic #655). Replaces the command-string +
/// cost-classification model with a semantic kind that the executor maps to
/// allowlisted repository-approved commands. Models submit typed intents;
/// the broker resolves them to commands. A model cannot submit arbitrary shell.
//
// `rename_all = "kebab-case"` keeps the on-disk serialization identical to
// `ProofKind::key()` (`focused-test`, `sanitizer-witness`, ...). Without it the
// derived serializer would emit Rust variant names (`SanitizerWitness`), which
// the `worker` subcommand (which reads serialized v2 request files) cannot
// parse. This is the on-the-wire contract for distributed proof execution.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProofKind {
    FocusedTest,
    FocusedBuild,
    BasePlusTests,
    SanitizerWitness,
    MutationWitness,
    MiriWitness,
    SourceRouteProbe,
    ExternalCheck,
}

impl ProofKind {
    pub(crate) fn key(&self) -> &'static str {
        match self {
            ProofKind::FocusedTest => "focused-test",
            ProofKind::FocusedBuild => "focused-build",
            ProofKind::BasePlusTests => "base-plus-tests",
            ProofKind::SanitizerWitness => "sanitizer-witness",
            ProofKind::MutationWitness => "mutation-witness",
            ProofKind::MiriWitness => "miri-witness",
            ProofKind::SourceRouteProbe => "source-route-probe",
            ProofKind::ExternalCheck => "external-check",
        }
    }
}

/// A typed proof request (v2). Carries semantic intent instead of a raw
/// command string. The executor adapter (Order 2 PR 2) maps the kind + target
/// to a repository-approved command template.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct ProofRequestV2 {
    pub(crate) schema: String,
    pub(crate) id: String,
    pub(crate) kind: ProofKind,
    /// The test target, module, or symbol the proof targets.
    pub(crate) target: String,
    /// Claim IDs this proof would confirm or refute.
    pub(crate) claim_ids: Vec<String>,
    /// Lane IDs that requested this proof.
    pub(crate) requested_by: Vec<String>,
    /// What outcome would confirm vs refute the associated claims.
    pub(crate) expected_interpretation: String,
    pub(crate) priority: String,
    pub(crate) timeout_sec: u64,
    pub(crate) status: String,
    /// Base commit SHA the proof is evaluated against (for red/green, the
    /// base the test patch is applied to). Empty when the proof is head-only.
    /// Carried on the request so the worker can stamp a canonical `ProofReceipt`
    /// with the same `base`/`head` identity the planner emitted.
    #[serde(default)]
    pub(crate) base: String,
    /// Head commit SHA the proof is evaluated against (the PR HEAD).
    #[serde(default)]
    pub(crate) head: String,
}

/// Classify an existing v1 ProofRequest into a ProofKind for the v2 shadow.
/// This is a best-effort mapping from cost + command pattern to typed intent.
pub(crate) fn classify_proof_kind(cost: &str, command: &str) -> ProofKind {
    match cost {
        "focused-test" => {
            if command.contains("base") || command.contains("red-green") {
                ProofKind::BasePlusTests
            } else {
                ProofKind::FocusedTest
            }
        }
        "focused-build" => ProofKind::FocusedBuild,
        "manual" => {
            if command.contains("asan") || command.contains("sanitizer") {
                ProofKind::SanitizerWitness
            } else if command.contains("mutants") || command.contains("mutation") {
                ProofKind::MutationWitness
            } else if command.contains("miri") {
                ProofKind::MiriWitness
            } else {
                ProofKind::ExternalCheck
            }
        }
        _ => ProofKind::ExternalCheck,
    }
}

/// Convert v1 proof requests into v2 typed-intent shadow requests.
/// Writes review/proof_requests_v2.json as a shadow artifact (Order 2 PR 1).
pub(crate) fn build_v2_shadow_requests(v1_requests: &[ProofRequest]) -> Vec<ProofRequestV2> {
    v1_requests
        .iter()
        .map(|req| {
            let kind = classify_proof_kind(&req.cost, &req.command);
            let kind_key = kind.key();
            // Shadow-resolve the typed intent to show what command the executor
            // adapter WOULD produce (Order 2 PR 2). Nightly is assumed available
            // for shadow purposes; actual availability checked at execution time.
            let shadow_resolution = resolve_proof_command(&kind, &req.command, true);
            let resolution_note = shadow_resolution
                .as_ref()
                .map(|r| r.resolution_note.clone())
                .unwrap_or_else(|| "unresolved (kind+target not resolvable)".to_owned());
            ProofRequestV2 {
                schema: crate::artifacts::PROOF_REQUEST_V2_SCHEMA.to_owned(),
                id: format!("{}-v2", req.id),
                kind,
                target: format!("[{}] {} → {}", kind_key, req.command, resolution_note),
                claim_ids: Vec::new(),
                requested_by: req.requested_by.clone(),
                expected_interpretation: String::new(),
                priority: if req.required { "high" } else { "medium" }.to_owned(),
                timeout_sec: req.timeout_sec,
                status: req.status.clone(),
                // v1 ProofRequest carries no commit identity; the shadow
                // stamps empty base/head. The production producer (planner)
                // fills these from the diff so the worker's canonical
                // receipt matches local execution.
                base: String::new(),
                head: String::new(),
            }
        })
        .collect()
}

/// The result of resolving a typed proof intent to an executable command.
/// (Order 2 PR 2 of epic #655.)
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ResolvedProofCommand {
    /// The argv to execute.
    pub(crate) argv: Vec<String>,
    /// Environment variables to set (e.g., RUSTFLAGS for sanitizer).
    pub(crate) env: Vec<(String, String)>,
    /// Whether this command requires nightly Rust.
    pub(crate) requires_nightly: bool,
    /// Human-readable summary of what was resolved.
    pub(crate) resolution_note: String,
}

/// Map a typed proof intent (ProofKind + target) to a repository-approved
/// command template. This is the executor adapter (Order 2 PR 2).
///
/// Models submit ProofKind + target; this function resolves them to concrete
/// argv. A model cannot submit arbitrary shell — only typed intents that map
/// to pre-approved templates.
///
/// Returns `None` if the kind+target combination is not resolvable (e.g.,
/// sanitizer witness on a non-Rust repo, or Miri without nightly available).
pub(crate) fn resolve_proof_command(
    kind: &ProofKind,
    target: &str,
    nightly_available: bool,
) -> Option<ResolvedProofCommand> {
    match kind {
        ProofKind::FocusedTest => {
            // Delegate to the existing v1 command parser for cargo test.
            // The allowlist is the ONLY resolution path: a focused-test
            // intent that the cargo-test parser rejects (no `--locked`, no
            // focus token, disallowed args) is unresolved, full stop.
            //
            // There is deliberately NO `split_whitespace` fallback. A
            // previous fallback turned any nonempty target without shell
            // metacharacters into arbitrary argv (e.g. a `focused-test`
            // request with `target: "rm -rf some-directory"` resolved to
            // `["rm","-rf","some-directory"]` and executed `rm`). The
            // executor adapter is a security boundary: typed intent must map
            // to an approved command template, or it must not execute.
            let spec = crate::focused_cargo_test_command_spec(target)?;
            Some(ResolvedProofCommand {
                argv: spec.argv,
                env: Vec::new(),
                requires_nightly: false,
                resolution_note: "focused-test resolved via cargo-test allowlist".to_owned(),
            })
        }
        ProofKind::FocusedBuild => {
            let spec = crate::focused_build_command_spec(target)?;
            Some(ResolvedProofCommand {
                argv: spec.argv,
                env: Vec::new(),
                requires_nightly: false,
                resolution_note: "focused-build resolved via cargo-build allowlist".to_owned(),
            })
        }
        ProofKind::BasePlusTests => {
            // Base+tests red/green is orchestrated by the broker, not a single command.
            // The target should be a test name; the broker constructs HEAD and base+tests variants.
            Some(ResolvedProofCommand {
                argv: vec![
                    "cargo".to_owned(),
                    "test".to_owned(),
                    "--locked".to_owned(),
                    target.to_owned(),
                ],
                env: Vec::new(),
                requires_nightly: false,
                resolution_note: format!(
                    "base-plus-tests: broker orchestrates HEAD + base+tests for target `{target}`"
                ),
            })
        }
        ProofKind::SanitizerWitness => {
            if !nightly_available {
                return None;
            }
            // AddressSanitizer: requires nightly + RUSTFLAGS=-Zsanitizer=address.
            // The target should be a test name or --test target.
            Some(ResolvedProofCommand {
                argv: vec![
                    "cargo".to_owned(),
                    "+nightly".to_owned(),
                    "test".to_owned(),
                    "--locked".to_owned(),
                    target.to_owned(),
                ],
                env: vec![("RUSTFLAGS".to_owned(), "-Zsanitizer=address".to_owned())],
                requires_nightly: true,
                resolution_note: format!(
                    "sanitizer-witness: cargo +nightly test {target} with ASAN"
                ),
            })
        }
        ProofKind::MutationWitness => {
            // cargo-mutants: requires the cargo-mutants binary on PATH.
            // The target should be a file or module path.
            Some(ResolvedProofCommand {
                argv: vec![
                    "cargo-mutants".to_owned(),
                    "--in-place".to_owned(),
                    "--no-shuffle".to_owned(),
                    target.to_owned(),
                ],
                env: Vec::new(),
                requires_nightly: false,
                resolution_note: format!("mutation-witness: cargo-mutants on `{target}`"),
            })
        }
        ProofKind::MiriWitness => {
            if !nightly_available {
                return None;
            }
            // Miri: requires nightly + cargo-miri component.
            Some(ResolvedProofCommand {
                argv: vec![
                    "cargo".to_owned(),
                    "+nightly".to_owned(),
                    "miri".to_owned(),
                    "test".to_owned(),
                    target.to_owned(),
                ],
                env: Vec::new(),
                requires_nightly: true,
                resolution_note: format!("miri-witness: cargo +nightly miri test {target}"),
            })
        }
        ProofKind::SourceRouteProbe => {
            // Source-route probes are informational (does the route exist?),
            // resolved via ast-grep or grep rather than cargo. Not a proof command.
            None
        }
        ProofKind::ExternalCheck => {
            // External checks (e.g., cargo xtask policy-check) are resolved
            // via the existing v1 focused-build allowlist.
            let spec = crate::focused_build_command_spec(target)?;
            Some(ResolvedProofCommand {
                argv: spec.argv,
                env: Vec::new(),
                requires_nightly: false,
                resolution_note: "external-check resolved via focused-build allowlist".to_owned(),
            })
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proof_kind_keys_are_stable() {
        assert_eq!(ProofKind::FocusedTest.key(), "focused-test");
        assert_eq!(ProofKind::FocusedBuild.key(), "focused-build");
        assert_eq!(ProofKind::BasePlusTests.key(), "base-plus-tests");
        assert_eq!(ProofKind::SanitizerWitness.key(), "sanitizer-witness");
        assert_eq!(ProofKind::MutationWitness.key(), "mutation-witness");
        assert_eq!(ProofKind::MiriWitness.key(), "miri-witness");
        assert_eq!(ProofKind::SourceRouteProbe.key(), "source-route-probe");
        assert_eq!(ProofKind::ExternalCheck.key(), "external-check");
    }

    #[test]
    fn classify_proof_kind_maps_focused_test() {
        assert_eq!(
            classify_proof_kind("focused-test", "cargo test --locked"),
            ProofKind::FocusedTest
        );
        assert_eq!(
            classify_proof_kind("focused-test", "bun test base+tests"),
            ProofKind::BasePlusTests
        );
    }

    #[test]
    fn classify_proof_kind_maps_heavy_witnesses() {
        assert_eq!(
            classify_proof_kind("manual", "cargo-mutants --in-place"),
            ProofKind::MutationWitness
        );
        assert_eq!(
            classify_proof_kind("manual", "RUSTFLAGS=-Zsanitizer=address cargo test"),
            ProofKind::SanitizerWitness
        );
        assert_eq!(
            classify_proof_kind("manual", "cargo +nightly miri test"),
            ProofKind::MiriWitness
        );
    }

    #[test]
    fn classify_proof_kind_defaults_to_external_check() {
        assert_eq!(
            classify_proof_kind("unknown", "some random command"),
            ProofKind::ExternalCheck
        );
    }

    #[test]
    fn v2_shadow_preserves_id_and_requesters() {
        let v1 = ProofRequest {
            schema: "ub-review.proof_request.v1".to_owned(),
            id: "req-42".to_owned(),
            lane: "tests-oracle".to_owned(),
            requested_by: vec!["lane-a".to_owned()],
            command: "cargo test --locked".to_owned(),
            reason: "test".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: true,
            status: "requested".to_owned(),
        };
        let v2_list = build_v2_shadow_requests(&[v1]);
        assert_eq!(v2_list.len(), 1);
        let v2 = &v2_list[0];
        assert_eq!(v2.id, "req-42-v2");
        assert_eq!(v2.kind, ProofKind::FocusedTest);
        assert_eq!(v2.requested_by, vec!["lane-a".to_owned()]);
        assert_eq!(v2.priority, "high");
        assert_eq!(v2.schema, "ub-review.proof_request.v2");
    }

    #[test]
    fn v2_shadow_handles_empty_requests() {
        assert!(build_v2_shadow_requests(&[]).is_empty());
    }

    #[test]
    fn resolve_focused_test_delegates_to_allowlist() {
        // focused_cargo_test_command_spec requires --locked and a focus token.
        // `--test config_tests` is a genuinely allowlisted focus (the allowlist
        // accepts `--test <value>`); this resolves without any argv fallback.
        let resolved = resolve_proof_command(
            &ProofKind::FocusedTest,
            "cargo test --locked --test config_tests",
            false,
        );
        assert!(
            resolved.is_some(),
            "focused-test should resolve with valid, allowlisted cargo test command"
        );
        if let Some(cmd) = resolved {
            assert_eq!(cmd.argv[0], "cargo");
            assert!(cmd.argv.iter().any(|a| a == "--locked"));
            assert!(!cmd.requires_nightly);
        }
    }

    /// Security regression: a focused-test intent whose target is NOT an
    /// allowlisted cargo test command must be unresolved. The previous
    /// implementation fell back to `split_whitespace` for any nonempty,
    /// shell-meta-free target, turning e.g. `rm -rf some-directory` into the
    /// argv `["rm","-rf","some-directory"]` and executing `rm`. The executor
    /// adapter is a security boundary: typed intent maps to an approved
    /// template, or it does not execute.
    #[test]
    fn resolve_focused_test_rejects_arbitrary_argv_fallback() {
        // An arbitrary program that contains no shell metacharacters. Under the
        // old fallback this became executable argv. It must now be unresolved.
        let malicious = "rm -rf some-directory";
        assert!(
            !has_shell_control_token(malicious),
            "test fixture premise: target has no shell-control tokens"
        );
        let resolved = resolve_proof_command(&ProofKind::FocusedTest, malicious, false);
        assert!(
            resolved.is_none(),
            "focused-test must not fall back to split_whitespace for non-allowlisted targets; \
             got {resolved:?}"
        );
    }

    #[test]
    fn resolve_focused_test_rejects_cargo_test_without_locked() {
        // cargo test without --locked is rejected by the allowlist and must not
        // resolve via any fallback.
        let resolved = resolve_proof_command(
            &ProofKind::FocusedTest,
            "cargo test --test config_tests",
            false,
        );
        assert!(
            resolved.is_none(),
            "focused-test without --locked must be unresolved"
        );
    }

    #[test]
    fn resolve_focused_test_rejects_empty_target() {
        let resolved = resolve_proof_command(&ProofKind::FocusedTest, "", false);
        assert!(
            resolved.is_none(),
            "empty focused-test target must be unresolved"
        );
    }

    /// Serialization contract: `ProofKind` must serialize to the kebab-case
    /// names the `worker` subcommand parses (`sanitizer-witness`, ...). The
    /// derived serializer would otherwise emit Rust variant names
    /// (`SanitizerWitness`), breaking the v2 request file the planner emits
    /// and the worker consumes.
    #[test]
    fn proof_kind_serializes_to_kebab_case() -> Result<()> {
        // Round-trip every variant and confirm the on-wire name matches key().
        for (variant, expected) in [
            (ProofKind::FocusedTest, "focused-test"),
            (ProofKind::FocusedBuild, "focused-build"),
            (ProofKind::BasePlusTests, "base-plus-tests"),
            (ProofKind::SanitizerWitness, "sanitizer-witness"),
            (ProofKind::MutationWitness, "mutation-witness"),
            (ProofKind::MiriWitness, "miri-witness"),
            (ProofKind::SourceRouteProbe, "source-route-probe"),
            (ProofKind::ExternalCheck, "external-check"),
        ] {
            let wire = serde_json::to_string(&variant)?;
            assert_eq!(
                wire.trim_matches('"'),
                expected,
                "ProofKind::{variant:?} serializes to {wire}, expected \"{expected}\""
            );
            let back: ProofKind = serde_json::from_str(&wire)?;
            assert_eq!(back, variant, "round-trip failed for {variant:?}");
            assert_eq!(variant.key(), expected, "key() must match wire name");
        }
        Ok(())
    }

    /// The full ProofRequestV2 struct round-trips through JSON with the
    /// kebab-case kind on the wire — this is the exact contract between the
    /// planner (emitter) and the worker (consumer).
    #[test]
    fn proof_request_v2_round_trips_with_kebab_kind() -> Result<()> {
        let req = ProofRequestV2 {
            schema: crate::artifacts::PROOF_REQUEST_V2_SCHEMA.to_owned(),
            id: "req-1-v2".to_owned(),
            kind: ProofKind::SanitizerWitness,
            target: "config_rejects_unknown_fields".to_owned(),
            claim_ids: vec!["claim-7".to_owned()],
            requested_by: vec!["tests-oracle".to_owned()],
            expected_interpretation: "asan abort => UB present".to_owned(),
            priority: "high".to_owned(),
            timeout_sec: 600,
            status: "requested".to_owned(),
            base: "abc1234".to_owned(),
            head: "def5678".to_owned(),
        };
        let json = serde_json::to_string(&req)?;
        assert!(
            json.contains("\"kind\":\"sanitizer-witness\""),
            "serialized request must carry kebab-case kind: {json}"
        );
        let back: ProofRequestV2 = serde_json::from_str(&json)?;
        assert_eq!(back.kind, ProofKind::SanitizerWitness);
        assert_eq!(back.id, "req-1-v2");
        assert_eq!(back.target, "config_rejects_unknown_fields");
        assert_eq!(back.base, "abc1234");
        assert_eq!(back.head, "def5678");
        Ok(())
    }

    /// Negative serialization: an unknown kind string must fail to deserialize.
    #[test]
    fn proof_kind_rejects_unknown_wire_name() {
        let bad = serde_json::from_str::<ProofKind>("\"not-a-real-kind\"");
        assert!(
            bad.is_err(),
            "unknown kind wire name must be rejected at deserialization"
        );
    }

    /// Negative serialization: a Rust-style variant name must now be rejected
    /// (regression guard for the rename_all change).
    #[test]
    fn proof_kind_rejects_rust_variant_name() {
        let bad = serde_json::from_str::<ProofKind>("\"SanitizerWitness\"");
        assert!(
            bad.is_err(),
            "Rust variant name must no longer be accepted; worker expects kebab-case"
        );
    }

    #[test]
    fn resolve_sanitizer_requires_nightly() {
        let without_nightly =
            resolve_proof_command(&ProofKind::SanitizerWitness, "test_target", false);
        assert!(
            without_nightly.is_none(),
            "sanitizer without nightly should be None"
        );

        let with_nightly = resolve_proof_command(&ProofKind::SanitizerWitness, "test_target", true);
        assert!(
            with_nightly.is_some(),
            "sanitizer with nightly should resolve"
        );
        if let Some(cmd) = with_nightly {
            assert!(cmd.requires_nightly);
            assert!(
                cmd.argv.contains(&"+nightly".to_owned()),
                "argv should include +nightly"
            );
            assert!(
                cmd.env
                    .iter()
                    .any(|(k, v)| k == "RUSTFLAGS" && v.contains("sanitizer=address")),
                "env should set RUSTFLAGS with ASAN"
            );
        }
    }

    #[test]
    fn resolve_miri_requires_nightly() {
        let without_nightly = resolve_proof_command(&ProofKind::MiriWitness, "test_target", false);
        assert!(
            without_nightly.is_none(),
            "miri without nightly should be None"
        );

        let with_nightly = resolve_proof_command(&ProofKind::MiriWitness, "test_target", true);
        assert!(with_nightly.is_some(), "miri with nightly should resolve");
        if let Some(cmd) = with_nightly {
            assert!(cmd.requires_nightly);
            assert!(
                cmd.argv.contains(&"miri".to_owned()),
                "argv should include miri"
            );
        }
    }

    #[test]
    fn resolve_mutation_uses_cargo_mutants() {
        let resolved = resolve_proof_command(&ProofKind::MutationWitness, "src/config.rs", false);
        assert!(resolved.is_some(), "mutation should resolve");
        if let Some(cmd) = resolved {
            assert!(!cmd.requires_nightly);
            assert!(
                cmd.argv.first().map(|s| s.as_str()) == Some("cargo-mutants"),
                "argv should start with cargo-mutants"
            );
        }
    }

    #[test]
    fn resolve_source_route_probe_returns_none() {
        let resolved = resolve_proof_command(&ProofKind::SourceRouteProbe, "src/main.rs", true);
        assert!(resolved.is_none(), "source-route-probe is not a command");
    }
}
