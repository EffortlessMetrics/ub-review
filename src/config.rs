use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::{BoxState, DEFAULT_REVIEW_PROFILE, builtin_profiles, builtin_tools};
use clap::ValueEnum as _;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Config {
    /// ECHO-ONLY: propagated to resolved-plan/profile artifacts but never
    /// branched on. Selects no behavior; the `profile` field is the live one.
    pub(crate) review_profile: String,
    pub(crate) profile: String,
    pub(crate) repo: RepoConfig,
    pub(crate) review: ReviewConfig,
    pub(crate) review_body: ReviewBodyPolicy,
    pub(crate) gate: GateConfig,
    pub(crate) proof: ProofPolicyConfig,
    pub(crate) profiles: BTreeMap<String, Profile>,
    pub(crate) tools: BTreeMap<String, ToolPolicy>,
    pub(crate) lanes: Vec<RepoLane>,
    pub(crate) issues: IssuesConfig,
    pub(crate) providers: ProvidersConfig,
    pub(crate) impact: ImpactConfig,
    /// Malformed gate-policy sections recorded at load time. Serialized into
    /// `effective-config.json` so the gate's `policy` fail reasons point at a
    /// receipt that names the parse error (roadmap #24: policy parse errors
    /// become receipted gate failures, never silent defaults).
    #[serde(skip_deserializing, skip_serializing_if = "Vec::is_empty")]
    pub(crate) policy_errors: Vec<PolicyError>,
}

/// The `[providers]` section (D1/D2, spec 0006). `policy` selects provider
/// routing when the CLI flag is `auto` (D2 precedence: explicit
/// `--provider-policy`/env value overrides config; config overrides the
/// built-in default). Per-provider `max_concurrency` caps live model-lane
/// waves for that provider. `[providers.minimax].prompt_cache` is consumed
/// as strict vocabulary for the current MiniMax cache modes. Invalid
/// consumed values are stripped at load with `PolicyError` receipts.
/// Descriptive fields such as `env`, `role`, and `models` remain
/// documentation of intent.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct ProvidersConfig {
    pub(crate) policy: String,
    #[serde(skip_serializing_if = "ProviderRuntimeConfig::is_empty")]
    pub(crate) minimax: ProviderRuntimeConfig,
    #[serde(skip_serializing_if = "ProviderRuntimeConfig::is_empty")]
    pub(crate) opencode: ProviderRuntimeConfig,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct ProviderRuntimeConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_concurrency: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) prompt_cache: Option<String>,
}

impl ProviderRuntimeConfig {
    fn is_empty(&self) -> bool {
        self.max_concurrency.is_none() && self.prompt_cache.is_none()
    }
}

/// The `[impact]` section (Order 3 of epic #655). Both modes compute and write
/// the full plan artifact. `shadow` (the default) keeps candidates artifact-only;
/// only explicit `active` feeds them to model proof selection. Invalid values
/// fall back to `shadow` and are recorded as a policy error so the gate reports
/// the misconfiguration rather than silently defaulting.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct ImpactConfig {
    /// "shadow" (default) or "active".
    pub(crate) mode: String,
}

impl ImpactConfig {
    /// Resolved mode after default/invalid handling. Anything other than the
    /// literal "active" resolves to "shadow", so an unknown value can never
    /// accidentally promote execution.
    pub(crate) fn resolved_mode(&self) -> &str {
        if self.mode == "active" {
            "active"
        } else {
            "shadow"
        }
    }
}

/// Follow-up issue-capture posture (`[issues]`). `mode = "off"` keeps every
/// candidate artifact-only; `mode = "suggest"` (the default) lets valid
/// high-confidence do-not-block candidates render as a suggested follow-up
/// in the PR body. `mode = "open-high-confidence"` additionally lets the
/// issue broker open GitHub issues at `post` time for suggested candidates
/// whose target repo appears in `open_in` (explicit `owner/repo` slugs, no
/// wildcards), after a remote fingerprint duplicate search, capped at
/// `open_cap` opens per post; every decision lands in the broker plan and
/// results artifacts. `run` never opens issues.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct IssuesConfig {
    pub(crate) enabled: bool,
    pub(crate) mode: String,
    pub(crate) open_in: Vec<String>,
    pub(crate) open_cap: u32,
}

impl Default for IssuesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mode: "suggest".to_owned(),
            open_in: Vec::new(),
            open_cap: 3,
        }
    }
}

/// A repo-declared review lane (`[[lanes]]` in `.ub-review.toml`). The
/// minimal authoring surface is `id` + `role` + `focus`; `receives` defaults
/// to the common sensor trio at plan time, `model` defaults to the lane
/// default, and `diff_classes` defaults to every diff class. What makes a
/// good lane - the evidence a finding must cite, the failure mode, the
/// never-do set - is doctrine, not schema: see
/// docs/specs/UB-REVIEW-SPEC-0011-lane-doctrine.md.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct RepoLane {
    pub(crate) id: String,
    pub(crate) role: String,
    pub(crate) focus: String,
    pub(crate) receives: Vec<String>,
    pub(crate) model: String,
    pub(crate) diff_classes: Vec<String>,
}

/// One malformed policy key or section (an unknown top-level key, a `[gate]`
/// key, a `[review_body]` key, a `[tools.<id>]` key, a `[proof]` key, or a
/// `[[proof.required]]` entry) downgraded from a hard parse failure or a
/// silent drop into a recorded, gate-failing receipt.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct PolicyError {
    pub(crate) section: String,
    pub(crate) detail: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct RepoConfig {
    /// ECHO-ONLY: free-text propagated to the `repo_kind` artifact key; not
    /// validated against any allowlist and never branched on (#609).
    pub(crate) kind: String,
    pub(crate) ledger: String,
    /// ECHO-ONLY: diff context comes from DiffContext, not these fields.
    pub(crate) base: String,
    pub(crate) head: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct ReviewConfig {
    /// ECHO-ONLY: propagated to resolved-plan artifacts but never branched on.
    pub(crate) posting_engine: String,
    /// INERT: parsed and stored but no non-test code consumes it (#609).
    pub(crate) custom_poster: bool,
    /// INERT: the name implies security posture but enforces nothing (#609).
    /// Do NOT rely on this to block standalone-approval findings.
    pub(crate) ban_standalone_approval: bool,
    /// INERT: the name implies security posture but enforces nothing (#609).
    /// Do NOT rely on this to require a zero-finding audit.
    pub(crate) require_zero_finding_audit: bool,
    pub(crate) enable_default_lanes: bool,
    pub(crate) github_summary: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct GateConfig {
    pub(crate) required_check: String,
    pub(crate) target_minutes: u64,
    pub(crate) hard_timeout_minutes: u64,
    pub(crate) post_review_on: Vec<String>,
    pub(crate) blocking: GateBlockingPolicy,
    /// Review-forward gate policy (Order 11 of #678). When true, the reporter's
    /// verdict (changes_requested / uncertain) may produce a gate `fail` reason.
    /// Default false: model output never feeds the gate unless the repo
    /// explicitly opts in. Individual lanes never block; only the final
    /// reporter verdict may, and only under this flag.
    #[serde(default)]
    pub(crate) review_forward: bool,
}

/// Repo-policy blocking markers for deterministic evidence classes
/// (ADR 0002: `blocking = true` comes from repo policy receipts, never from
/// model output or model confidence). Model-produced finding classes have no
/// deterministic per-class receipt yet, so candidate/finding blocking markers
/// stay deferred; these flags cover the evidence classes the gate already
/// computes deterministically. Both default to `false`, preserving the
/// pre-policy gate behavior.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct GateBlockingPolicy {
    /// Block when a matched `[[proof.required]]` policy produced no passing
    /// receipt (missing receipt, `skipped_budget`, `skipped_profile`,
    /// `non_discriminating`, `base_patch_failed`).
    pub(crate) required_proof_unproven: bool,
    /// Block when a configured `[tools.*.gate]` threshold could not be
    /// evaluated for a required tool: the sensor failed or timed out without
    /// producing a verdict, sensor evidence is missing, or the gate-decision
    /// receipt is missing or malformed. Without this opt-in those gaps stay
    /// advisory; only an actually-evaluated, actually-exceeded threshold
    /// blocks unconditionally.
    pub(crate) tool_gate_missing_evidence: bool,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct ProofPolicyConfig {
    pub(crate) required: Vec<RequiredProofPolicy>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct RequiredProofPolicy {
    pub(crate) id: String,
    pub(crate) languages: Vec<String>,
    pub(crate) diff_classes: Vec<String>,
    pub(crate) command: String,
    pub(crate) reason: String,
    pub(crate) cost: Option<String>,
    pub(crate) timeout_sec: u64,
    pub(crate) required: bool,
    pub(crate) enabled: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ReviewBodyPolicy {
    pub(crate) include_successful_lane_table: bool,
    pub(crate) include_provider_table: ReviewBodyTablePolicy,
    pub(crate) include_sensor_table: ReviewBodyTablePolicy,
    pub(crate) include_execution_summary: ReviewBodyExecutionSummaryPolicy,
    /// Posture for the boilerplate suppressor: what to do when reviewer-value
    /// content survived compilation but the rendered PR body tripped a
    /// suppressible no-value classification and only summary-only findings
    /// carry the review's content. `suppress` (the consumer default) withholds
    /// the PR post and keeps diagnostics in artifacts.
    pub(crate) summary_only_body: SummaryOnlyBodyPolicy,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewBodyTablePolicy {
    Never,
    #[default]
    OnFailure,
    Always,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ReviewBodyExecutionSummaryPolicy {
    #[default]
    None,
    OnFailure,
    Always,
}

/// `[review_body].summary_only_body`: posting posture for a PR review body
/// that the suppressor would otherwise withhold as no-value boilerplate while
/// summary-only findings exist. Values follow the snake_case `[review_body]`
/// vocabulary; kebab-case spellings are accepted as aliases.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SummaryOnlyBodyPolicy {
    /// Withhold the PR-facing body (today's behavior; consumer default).
    #[default]
    Suppress,
    /// Post when at least one summary-only finding is substantive: severity
    /// medium or higher, or confidence medium-high or higher, excluding pure
    /// lane-status notes.
    #[serde(alias = "post-substantive")]
    PostSubstantive,
    /// Post whenever any summary-only finding exists.
    #[serde(alias = "post-all")]
    PostAll,
}

impl SummaryOnlyBodyPolicy {
    /// Canonical config spelling, used by truthful skip receipts.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Suppress => "suppress",
            Self::PostSubstantive => "post_substantive",
            Self::PostAll => "post_all",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Profile {
    pub(crate) name: String,
    pub(crate) limits: Limits,
    pub(crate) guards: Guards,
    pub(crate) budgets: Budgets,
    pub(crate) trusted_repo: TrustedRepo,
    /// Per-tool sensor lease overrides keyed by tool id, in seconds. Repo
    /// config remains the per-repo override surface: an explicit
    /// `[tools.<id>] timeout_sec` wins over this profile table.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) tool_timeouts: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeProfileFile {
    pub(crate) name: String,
    pub(crate) limits: RuntimeLimitsFile,
    pub(crate) guards: RuntimeGuardsFile,
    pub(crate) budgets: RuntimeBudgetsFile,
    pub(crate) trusted_repo: TrustedRepo,
    #[serde(default)]
    pub(crate) tool_timeouts: BTreeMap<String, u64>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeLimitsFile {
    pub(crate) logical_lanes: usize,
    pub(crate) llm_in_flight: usize,
    pub(crate) sensor_jobs: usize,
    pub(crate) repo_read: usize,
    pub(crate) raw_file_reads: usize,
    pub(crate) grep: usize,
    pub(crate) ast_grep: usize,
    pub(crate) git: usize,
    pub(crate) tests: usize,
    pub(crate) builds: usize,
    pub(crate) rust_analyzer: usize,
    pub(crate) summary_writers: usize,
    pub(crate) patch_writers: usize,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeGuardsFile {
    pub(crate) min_free_mem_mb: u64,
    pub(crate) min_free_disk_mb: u64,
    pub(crate) max_load_1m: f32,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RuntimeBudgetsFile {
    pub(crate) artifact_budget_mb: u64,
    pub(crate) scratch_budget_mb: u64,
    pub(crate) default_timeout_sec: u64,
    pub(crate) hard_timeout_sec: u64,
    pub(crate) proof_max_focused_test_files: usize,
    pub(crate) proof_max_focused_tests: usize,
    pub(crate) proof_command_timeout_sec: u64,
    pub(crate) proof_total_timeout_sec: u64,
    pub(crate) proof_cpu: u32,
    pub(crate) proof_memory_mb: u64,
    pub(crate) proof_disk_mb: u64,
    pub(crate) proof_network: bool,
    pub(crate) proof_scratch: bool,
    pub(crate) mutation: bool,
    pub(crate) sanitizer: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
/// INERT: all three fields are parsed and echoed to artifacts but drive no
/// behavior (#609). The doc comments suggest these should gate the trusted-repo
/// path, but they currently do not. See SPEC-0013 for the full liveness table.
pub(crate) struct TrustedRepo {
    pub(crate) pass_triggers: Vec<String>,
    pub(crate) synchronize: bool,
    pub(crate) proof_lanes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct Limits {
    pub(crate) logical_lanes: usize,
    pub(crate) llm_in_flight: usize,
    pub(crate) sensor_jobs: usize,
    pub(crate) repo_read: usize,
    pub(crate) raw_file_reads: usize,
    pub(crate) grep: usize,
    pub(crate) ast_grep: usize,
    pub(crate) git: usize,
    pub(crate) tests: usize,
    pub(crate) builds: usize,
    pub(crate) rust_analyzer: usize,
    pub(crate) summary_writers: usize,
    pub(crate) patch_writers: usize,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct Guards {
    pub(crate) min_free_mem_mb: u64,
    pub(crate) min_free_disk_mb: u64,
    pub(crate) max_load_1m: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub(crate) struct Budgets {
    pub(crate) artifact_budget_mb: u64,
    pub(crate) scratch_budget_mb: u64,
    pub(crate) default_timeout_sec: u64,
    pub(crate) hard_timeout_sec: u64,
    pub(crate) proof_max_focused_test_files: usize,
    pub(crate) proof_max_focused_tests: usize,
    pub(crate) proof_command_timeout_sec: u64,
    pub(crate) proof_total_timeout_sec: u64,
    pub(crate) proof_cpu: u32,
    pub(crate) proof_memory_mb: u64,
    pub(crate) proof_disk_mb: u64,
    pub(crate) proof_network: bool,
    pub(crate) proof_scratch: bool,
    pub(crate) mutation: bool,
    pub(crate) sanitizer: bool,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ToolPolicy {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) class: ToolClass,
    pub(crate) weight: u32,
    pub(crate) default: Trigger,
    pub(crate) required: bool,
    pub(crate) timeout_sec: u64,
    pub(crate) artifact_budget_mb: u64,
    pub(crate) requires_lease: bool,
    pub(crate) enabled: bool,
    /// Evidence-phase override for the pipelined scheduler (#325). Unset means
    /// the phase is derived from `class`/`requires_lease` at plan time via
    /// `default_sensor_phase`; `fast` forces the sensor into the pre-lane
    /// evidence window, `late` defers it behind lane launch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) phase: Option<SensorPhase>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) gate: Option<ToolGatePolicy>,
    #[serde(skip)]
    pub(crate) provided: ToolPolicyProvided,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ToolPolicyProvided {
    pub(crate) id: bool,
    pub(crate) command: bool,
    pub(crate) class: bool,
    pub(crate) weight: bool,
    pub(crate) default: bool,
    pub(crate) required: bool,
    pub(crate) timeout_sec: bool,
    pub(crate) artifact_budget_mb: bool,
    pub(crate) requires_lease: bool,
    pub(crate) enabled: bool,
    pub(crate) phase: bool,
    pub(crate) gate: bool,
}

#[derive(Default, Deserialize)]
pub(crate) struct ToolPolicyInput {
    pub(crate) id: Option<String>,
    pub(crate) command: Option<String>,
    pub(crate) class: Option<ToolClass>,
    pub(crate) weight: Option<u32>,
    pub(crate) default: Option<Trigger>,
    pub(crate) required: Option<bool>,
    pub(crate) timeout_sec: Option<u64>,
    pub(crate) artifact_budget_mb: Option<u64>,
    pub(crate) requires_lease: Option<bool>,
    pub(crate) enabled: Option<bool>,
    pub(crate) phase: Option<SensorPhase>,
    pub(crate) gate: Option<ToolGatePolicy>,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct ToolGatePolicy {
    /// Gate scope. `"on-diff"` (thresholds apply to findings the diff
    /// introduces) is the only scope semantics that exist; repo-wide scoping
    /// does not exist yet. Values outside `KNOWN_TOOL_GATE_SCOPES` are
    /// stripped at load time and recorded as `PolicyError` receipts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) max_new_unsuppressed: Option<u64>,
}

impl<'de> Deserialize<'de> for ToolPolicy {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let input = ToolPolicyInput::deserialize(deserializer)?;
        let defaults = ToolPolicy::default();
        let provided = ToolPolicyProvided {
            id: input.id.is_some(),
            command: input.command.is_some(),
            class: input.class.is_some(),
            weight: input.weight.is_some(),
            default: input.default.is_some(),
            required: input.required.is_some(),
            timeout_sec: input.timeout_sec.is_some(),
            artifact_budget_mb: input.artifact_budget_mb.is_some(),
            requires_lease: input.requires_lease.is_some(),
            enabled: input.enabled.is_some(),
            phase: input.phase.is_some(),
            gate: input.gate.is_some(),
        };
        Ok(Self {
            id: input.id.unwrap_or(defaults.id),
            command: input.command.unwrap_or(defaults.command),
            class: input.class.unwrap_or(defaults.class),
            weight: input.weight.unwrap_or(defaults.weight),
            default: input.default.unwrap_or(defaults.default),
            required: input.required.unwrap_or(defaults.required),
            timeout_sec: input.timeout_sec.unwrap_or(defaults.timeout_sec),
            artifact_budget_mb: input
                .artifact_budget_mb
                .unwrap_or(defaults.artifact_budget_mb),
            requires_lease: input.requires_lease.unwrap_or(defaults.requires_lease),
            enabled: input.enabled.unwrap_or(defaults.enabled),
            phase: input.phase.or(defaults.phase),
            gate: input.gate.or(defaults.gate),
            provided,
        })
    }
}

/// Evidence phase for the pipelined intelligent-ci scheduler (#325).
///
/// `fast` sensors run to completion before the shared context is rendered and
/// model lanes launch (the "first evidence window"). `late` sensors are
/// spawned concurrently with the model wave and joined before the reporter,
/// the review compile, and the gate — so the gate always evaluates complete
/// sensor evidence, while lane launch never waits on the slow suite. A late
/// sensor that produced no receipt at join time stays missing evidence, never
/// clean evidence.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum SensorPhase {
    Fast,
    Late,
}

impl SensorPhase {
    pub(crate) fn key(self) -> &'static str {
        match self {
            SensorPhase::Fast => "fast",
            SensorPhase::Late => "late",
        }
    }
}

/// Default evidence phase for a tool when `[tools.<id>].phase` is unset:
/// lease-gated witnesses and compile/test-requiring classes are late; the
/// non-compiling static signal (static, search, packet, workflow, security)
/// is fast. A repo can override per tool (for example `cargo-fmt` is class
/// `build` but needs no compile, so this repo pins it `phase = "fast"`).
pub(crate) fn default_sensor_phase(class: ToolClass, requires_lease: bool) -> SensorPhase {
    if requires_lease {
        return SensorPhase::Late;
    }
    match class {
        ToolClass::Test | ToolClass::Build | ToolClass::Coverage | ToolClass::HeavyWitness => {
            SensorPhase::Late
        }
        ToolClass::Packet
        | ToolClass::Static
        | ToolClass::Search
        | ToolClass::Workflow
        | ToolClass::Security => SensorPhase::Fast,
    }
}

impl ToolPolicy {
    /// The phase this tool's sensor executes in: the explicit `phase` key when
    /// provided, else the class/lease-derived default.
    pub(crate) fn effective_phase(&self) -> SensorPhase {
        self.phase
            .unwrap_or_else(|| default_sensor_phase(self.class, self.requires_lease))
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ToolClass {
    Packet,
    #[default]
    Static,
    Search,
    Workflow,
    Security,
    Coverage,
    Test,
    Build,
    HeavyWitness,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Trigger {
    Always,
    SourceChanged,
    SourceExceptionChanged,
    RustBehaviorOrTestsChanged,
    UnsafeOrNativeRiskChanged,
    WorkflowChanged,
    DependencyChanged,
    ShellChanged,
    CppChanged,
    Diff,
    Manual,
    #[default]
    Never,
}

impl Default for Config {
    fn default() -> Self {
        let profiles = builtin_profiles()
            .into_iter()
            .map(|profile| (profile.name.clone(), profile))
            .collect();
        let tools = builtin_tools()
            .into_iter()
            .map(|tool| (tool.id.clone(), tool))
            .collect();
        Self {
            review_profile: DEFAULT_REVIEW_PROFILE.to_owned(),
            profile: "gh-runner".to_owned(),
            repo: RepoConfig::default(),
            review: ReviewConfig::default(),
            review_body: ReviewBodyPolicy::default(),
            gate: GateConfig::default(),
            proof: ProofPolicyConfig::default(),
            profiles,
            tools,
            lanes: Vec::new(),
            issues: IssuesConfig::default(),
            providers: ProvidersConfig::default(),
            impact: ImpactConfig::default(),
            policy_errors: Vec::new(),
        }
    }
}

impl Default for RepoConfig {
    fn default() -> Self {
        Self {
            kind: "bun".to_owned(),
            ledger: "/home/steven/code/bun-ub-ledger".to_owned(),
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
        }
    }
}

impl Default for ReviewConfig {
    fn default() -> Self {
        Self {
            posting_engine: "github-step-summary".to_owned(),
            custom_poster: false,
            ban_standalone_approval: true,
            require_zero_finding_audit: true,
            enable_default_lanes: true,
            github_summary: true,
        }
    }
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            required_check: "ub-review/gate".to_owned(),
            target_minutes: 30,
            hard_timeout_minutes: 60,
            post_review_on: vec!["opened".to_owned(), "ready_for_review".to_owned()],
            blocking: GateBlockingPolicy::default(),
            review_forward: false,
        }
    }
}

impl Default for ReviewBodyPolicy {
    fn default() -> Self {
        Self {
            include_successful_lane_table: false,
            include_provider_table: ReviewBodyTablePolicy::OnFailure,
            include_sensor_table: ReviewBodyTablePolicy::OnFailure,
            include_execution_summary: ReviewBodyExecutionSummaryPolicy::None,
            summary_only_body: SummaryOnlyBodyPolicy::Suppress,
        }
    }
}

impl Default for RequiredProofPolicy {
    fn default() -> Self {
        Self {
            id: String::new(),
            languages: Vec::new(),
            diff_classes: Vec::new(),
            command: String::new(),
            reason: String::new(),
            cost: None,
            timeout_sec: 300,
            required: true,
            enabled: true,
        }
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self {
            name: "gh-runner".to_owned(),
            limits: Limits::default(),
            guards: Guards::default(),
            budgets: Budgets::default(),
            trusted_repo: TrustedRepo::default(),
            tool_timeouts: BTreeMap::new(),
        }
    }
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            logical_lanes: 20,
            llm_in_flight: 16,
            sensor_jobs: 4,
            repo_read: 6,
            raw_file_reads: 6,
            grep: 3,
            ast_grep: 2,
            git: 2,
            tests: 2,
            builds: 0,
            rust_analyzer: 0,
            summary_writers: 1,
            patch_writers: 0,
        }
    }
}

impl Default for Guards {
    fn default() -> Self {
        Self {
            min_free_mem_mb: 1_500,
            min_free_disk_mb: 4_000,
            max_load_1m: 6.0,
        }
    }
}

impl Default for Budgets {
    fn default() -> Self {
        Self {
            artifact_budget_mb: 750,
            scratch_budget_mb: 4_000,
            default_timeout_sec: 1_800,
            hard_timeout_sec: 3_600,
            proof_max_focused_test_files: 3,
            proof_max_focused_tests: 1,
            proof_command_timeout_sec: 300,
            proof_total_timeout_sec: 600,
            proof_cpu: 2,
            proof_memory_mb: 2_048,
            proof_disk_mb: 1_024,
            proof_network: false,
            proof_scratch: true,
            mutation: false,
            sanitizer: false,
        }
    }
}

impl Default for TrustedRepo {
    fn default() -> Self {
        Self {
            pass_triggers: vec!["opened".to_owned(), "ready_for_review".to_owned()],
            synchronize: false,
            proof_lanes: vec![
                "focused-tests".to_owned(),
                "base-tests-red-green".to_owned(),
                "actionlint".to_owned(),
                "scoped-source-route-checks".to_owned(),
            ],
        }
    }
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            id: String::new(),
            command: String::new(),
            class: ToolClass::Static,
            weight: 1,
            default: Trigger::Never,
            required: false,
            timeout_sec: 120,
            artifact_budget_mb: 64,
            requires_lease: false,
            enabled: true,
            phase: None,
            gate: None,
            provided: ToolPolicyProvided::default(),
        }
    }
}

/// Diff-class selector values accepted by `[[proof.required]].diff_classes`.
/// Mirrors `DiffClass::key` in `main.rs`; a test there pins the two lists
/// together so an unknown selector can never silently de-fang a policy.
pub(crate) const KNOWN_POLICY_DIFF_CLASSES: &[&str] = &[
    "source-ub",
    "source-general",
    "tests-only",
    "workflow/tooling",
    "docs-only",
    "artifact-only-smoke",
];

/// Language selector values accepted by `[[proof.required]].languages`.
/// Mirrors `language_for_path` in `main.rs` plus the `mixed` marker; a test
/// there pins the two lists together.
pub(crate) const KNOWN_POLICY_LANGUAGES: &[&str] = &[
    "rust",
    "typescript",
    "javascript",
    "c-cpp",
    "zig",
    "go",
    "python",
    "shell",
    "yaml",
    "toml",
    "json",
    "markdown",
    "mixed",
];

/// Wildcard selector values that match any diff class or language.
const POLICY_SELECTOR_WILDCARDS: &[&str] = &["*", "any", "all"];

pub(crate) fn normalize_policy_selector(value: &str) -> String {
    value.trim().to_ascii_lowercase().replace('_', "-")
}

fn known_policy_selector(value: &str, known: &[&str]) -> bool {
    let normalized = normalize_policy_selector(value);
    POLICY_SELECTOR_WILDCARDS.contains(&normalized.as_str()) || known.contains(&normalized.as_str())
}

impl Config {
    pub(crate) fn load_or_default(path: &Path, profile_override: Option<&str>) -> Result<Self> {
        let mut config = if path.exists() {
            let text =
                fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
            Self::from_toml_with_policy_receipts(&text)
                .with_context(|| format!("parse {}", path.display()))?
        } else {
            Self::default()
        };
        if let Some(profile) = profile_override {
            config.profile = profile.to_owned();
        }
        config.merge_defaults();
        if config.profile == "auto" {
            config.profile = BoxState::detect()?.suggested_profile();
        }
        // Surface an unknown profile name as a receipt rather than letting the
        // silent gh-runner fallback in selected_profile() hide a typo. A
        // misspelled profile (e.g. "gh-runner-fuill") would otherwise silently
        // downgrade a gh-runner-full repo to gh-runner budgets with no signal.
        if !config.profiles.contains_key(&config.profile)
            && config.profile != "gh-runner"
            && config.profiles.contains_key("gh-runner")
        {
            config.policy_errors.push(PolicyError {
                section: "profile".to_owned(),
                detail: format!(
                    "unknown profile `{}` not in the profiles map; falling back to `gh-runner` \
                     silently. Correct the profile name or declare [profiles.{}] to remove this \
                     receipt.",
                    config.profile, config.profile
                ),
            });
        }
        Ok(config)
    }

    /// Parse config TOML while downgrading malformed policy keys (unknown
    /// top-level keys, `[gate]` keys, `[review_body]` keys, `[tools.<id>]`
    /// keys, `[proof]` keys, and `[[proof.required]]` entries) from hard
    /// parse failures or silent drops into recorded `policy_errors`. The run proceeds with only the
    /// offending keys stripped — valid siblings keep working — and
    /// `build_gate_outcome` turns every recorded error into a `policy` fail
    /// reason pointing at `effective-config.json`, never a silent default.
    /// TOML syntax errors and malformed non-policy sections stay hard errors:
    /// with no parsable document there is no trustworthy config to record a
    /// receipt against.
    pub(crate) fn from_toml_with_policy_receipts(text: &str) -> Result<Self> {
        let mut value: toml::Value = toml::from_str(text)?;
        let policy_errors = sanitize_policy_sections(&mut value);
        let mut config: Self = value.try_into()?;
        config.policy_errors = policy_errors;
        Ok(config)
    }

    pub(crate) fn merge_defaults(&mut self) {
        let defaults = Self::default();
        for (key, profile) in defaults.profiles {
            self.profiles.entry(key).or_insert(profile);
        }
        for (key, default_tool) in defaults.tools {
            match self.tools.get_mut(&key) {
                Some(tool) => {
                    if !tool.provided.id || tool.id.is_empty() {
                        tool.id = default_tool.id;
                    }
                    if !tool.provided.command || tool.command.is_empty() {
                        tool.command = default_tool.command;
                    }
                    if !tool.provided.class {
                        tool.class = default_tool.class;
                    }
                    if !tool.provided.weight || tool.weight == 0 {
                        tool.weight = default_tool.weight;
                    }
                    if !tool.provided.default {
                        tool.default = default_tool.default;
                    }
                    if !tool.provided.required {
                        tool.required = default_tool.required;
                    }
                    if !tool.provided.timeout_sec || tool.timeout_sec == 0 {
                        tool.timeout_sec = default_tool.timeout_sec;
                    }
                    if !tool.provided.artifact_budget_mb || tool.artifact_budget_mb == 0 {
                        tool.artifact_budget_mb = default_tool.artifact_budget_mb;
                    }
                    if !tool.provided.requires_lease {
                        tool.requires_lease = default_tool.requires_lease;
                    }
                    if !tool.provided.enabled {
                        tool.enabled = default_tool.enabled;
                    }
                    if !tool.provided.phase {
                        tool.phase = default_tool.phase;
                    }
                    if !tool.provided.gate {
                        tool.gate = default_tool.gate;
                    }
                }
                None => {
                    self.tools.insert(key, default_tool);
                }
            }
        }
    }

    pub(crate) fn selected_profile(&self) -> Result<&Profile> {
        self.profiles
            .get(&self.profile)
            .or_else(|| self.profiles.get("gh-runner"))
            .ok_or_else(|| anyhow::anyhow!("no selected profile and no gh-runner fallback"))
    }
}

/// Top-level keys `Config` deserializes. Unknown top-level keys are stripped
/// and recorded as `PolicyError` receipts so a misspelled section (for
/// example `[gatee]` or `[prooof]`) can never silently de-fang repo policy.
/// `[providers]` consumes `policy` plus `[providers.minimax]` and
/// `[providers.opencode]` `max_concurrency`, validated by
/// `sanitize_providers_section` (D2: config wins when the CLI flag is
/// `auto`). Other per-provider keys remain documentation of intent tolerated
/// without receipts.
const KNOWN_TOP_LEVEL_KEYS: &[&str] = &[
    "review_profile",
    "profile",
    "repo",
    "review",
    "review_body",
    "gate",
    "proof",
    "profiles",
    "tools",
    "lanes",
    "issues",
    "providers",
    "impact",
];

/// Keys `ToolPolicyInput` deserializes for a `[tools.<id>]` table. A test in
/// `main.rs` (`tool_policy_known_keys_match_serialized_fields`) pins this
/// list to the `ToolPolicy` field set so the two can never drift apart.
pub(crate) const KNOWN_TOOL_POLICY_KEYS: &[&str] = &[
    "id",
    "command",
    "class",
    "weight",
    "default",
    "required",
    "timeout_sec",
    "artifact_budget_mb",
    "requires_lease",
    "enabled",
    "phase",
    "gate",
];

/// `[tools.<id>.gate]` scope values with defined semantics. `on-diff` is the
/// only one: repo-wide tool-gate scoping does not exist yet.
pub(crate) const KNOWN_TOOL_GATE_SCOPES: &[&str] = &["on-diff"];

/// Validate the receipted policy surfaces (`[gate]`, `[review_body]`,
/// `[tools.<id>]`, `[proof]`, and their containers) inside a parsed TOML
/// document. The strategy is strip-and-receipt per key: each
/// unknown or malformed key is removed so the remaining document still
/// deserializes, valid siblings survive, and one `PolicyError` is recorded
/// per removed key. Shape mismatches (a non-table `[gate]`, a non-table
/// `tools.<id>`, `[proof.required]` written as a single table) take the same
/// receipt path instead of hard parse errors; hard errors stay scoped to TOML
/// syntax and malformed non-policy sections.
fn sanitize_policy_sections(value: &mut toml::Value) -> Vec<PolicyError> {
    let mut errors = Vec::new();
    let Some(table) = value.as_table_mut() else {
        return errors;
    };
    let unknown_top_level = table
        .keys()
        .filter(|key| !KNOWN_TOP_LEVEL_KEYS.contains(&key.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    for key in unknown_top_level {
        errors.push(PolicyError {
            section: key.clone(),
            detail: format!(
                "unknown top-level config key `{key}`; expected one of: {}",
                KNOWN_TOP_LEVEL_KEYS.join(", ")
            ),
        });
        table.remove(&key);
    }
    sanitize_gate_section(table, &mut errors);
    sanitize_review_body_section(table, &mut errors);
    sanitize_tools_section(table, &mut errors);
    sanitize_proof_section(table, &mut errors);
    sanitize_providers_section(table, &mut errors);
    errors
}

/// Validate consumed `[providers]` keys. `policy` must name a provider policy
/// the CLI enum knows (`auto`, `minimax-primary`, `primary-with-fallback`,
/// `minimax-only`, `opencode-go-canary`, `opencode-go-wide`).
/// `max_concurrency` in provider sub-tables must be a positive integer.
/// `[providers.minimax].prompt_cache`, when present, must name a supported
/// cache mode (`explicit-anthropic` or `off`). `[providers.opencode]` has no
/// cache mode yet, so any `prompt_cache` value there is rejected. Bad values
/// are stripped with `PolicyError` receipts so the run cannot silently fall
/// back while looking configured.
fn sanitize_providers_section(table: &mut toml::value::Table, errors: &mut Vec<PolicyError>) {
    let Some(providers) = table
        .get_mut("providers")
        .and_then(toml::Value::as_table_mut)
    else {
        return;
    };
    if let Some(policy) = providers.get("policy") {
        let valid = policy
            .as_str()
            .is_some_and(|value| crate::cli::ModelProviderPolicy::from_str(value, false).is_ok());
        if !valid {
            errors.push(PolicyError {
                section: "providers".to_owned(),
                detail: format!(
                    "invalid [providers] policy value {policy}; expected one of: auto, \
                     minimax-primary, primary-with-fallback, minimax-only, \
                     opencode-go-canary, opencode-go-wide"
                ),
            });
            providers.remove("policy");
        }
    }
    for provider in ["minimax", "opencode"] {
        let Some(provider_table) = providers
            .get_mut(provider)
            .and_then(toml::Value::as_table_mut)
        else {
            continue;
        };
        if let Some(max_concurrency) = provider_table.get("max_concurrency") {
            let valid = max_concurrency.as_integer().is_some_and(|value| value > 0);
            if !valid {
                errors.push(PolicyError {
                    section: format!("providers.{provider}.max_concurrency"),
                    detail: format!(
                        "invalid [providers.{provider}] max_concurrency value {max_concurrency}; \
                         expected a positive integer"
                    ),
                });
                provider_table.remove("max_concurrency");
            }
        }
        let Some(prompt_cache) = provider_table.get("prompt_cache") else {
            continue;
        };
        let valid = provider == "minimax"
            && prompt_cache.as_str().is_some_and(|value| {
                value == crate::cli::MinimaxPromptCache::ExplicitAnthropic.key()
                    || value == crate::cli::MinimaxPromptCache::Off.key()
            });
        if !valid {
            let detail = if provider == "minimax" {
                format!(
                    "invalid [providers.minimax] prompt_cache value {prompt_cache}; \
                     expected one of: explicit-anthropic, off"
                )
            } else {
                format!(
                    "unsupported [providers.opencode] prompt_cache value {prompt_cache}; \
                     OpenCode prompt caching is not implemented"
                )
            };
            errors.push(PolicyError {
                section: format!("providers.{provider}.prompt_cache"),
                detail,
            });
            provider_table.remove("prompt_cache");
        }
    }
}

/// Per-key validation for `[review_body]`, mirroring `[gate]`: each key is
/// probed as a single-key table against `ReviewBodyPolicy` (which carries
/// `deny_unknown_fields`), so an unknown key or an unknown policy value (for
/// example a misspelled `summary_only_body`) strips only the offending key,
/// records a `PolicyError` receipt, and leaves valid siblings working under
/// the conservative defaults instead of silently de-fanging the policy.
fn sanitize_review_body_section(table: &mut toml::value::Table, errors: &mut Vec<PolicyError>) {
    let Some(review_body) = table.get_mut("review_body") else {
        return;
    };
    let Some(review_body_table) = review_body.as_table_mut() else {
        errors.push(PolicyError {
            section: "review_body".to_owned(),
            detail: format!(
                "invalid [review_body]: expected a table, found {}",
                review_body.type_str()
            ),
        });
        table.remove("review_body");
        return;
    };
    let entries = review_body_table
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    for (key, entry) in entries {
        let mut probe = toml::value::Table::new();
        probe.insert(key.clone(), entry);
        if let Err(err) = toml::Value::Table(probe).try_into::<ReviewBodyPolicy>() {
            errors.push(PolicyError {
                section: format!("review_body.{key}"),
                detail: format!("invalid [review_body] key `{key}`: {err}"),
            });
            review_body_table.remove(&key);
        }
    }
}

const SYNCHRONIZE_MODE_DEPRECATION_DETAIL: &str = "`[gate].synchronize_mode` was removed because it never controlled review posting; use `[gate].post_review_on` to list the pull_request passes that may post reviews (#306)";

/// Per-key validation for `[gate]`: each key is probed as a single-key table
/// against `GateConfig` (which carries `deny_unknown_fields`), so an unknown
/// key, a wrong value type, or a malformed `[gate.blocking]` sub-table strips
/// only the offending key while valid siblings keep working.
fn sanitize_gate_section(table: &mut toml::value::Table, errors: &mut Vec<PolicyError>) {
    let Some(gate) = table.get_mut("gate") else {
        return;
    };
    let Some(gate_table) = gate.as_table_mut() else {
        errors.push(PolicyError {
            section: "gate".to_owned(),
            detail: format!(
                "invalid [gate]: expected a table, found {}",
                gate.type_str()
            ),
        });
        table.remove("gate");
        return;
    };
    let entries = gate_table
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    for (key, entry) in entries {
        if key == "synchronize_mode" {
            errors.push(PolicyError {
                section: "gate.synchronize_mode".to_owned(),
                detail: SYNCHRONIZE_MODE_DEPRECATION_DETAIL.to_owned(),
            });
            gate_table.remove(&key);
            continue;
        }
        let mut probe = toml::value::Table::new();
        probe.insert(key.clone(), entry);
        if let Err(err) = toml::Value::Table(probe).try_into::<GateConfig>() {
            errors.push(PolicyError {
                section: format!("gate.{key}"),
                detail: format!("invalid [gate] key `{key}`: {err}"),
            });
            gate_table.remove(&key);
        }
    }
}

/// Per-key validation for every `[tools.<id>]` table: unknown keys (for
/// example `gates`), wrong value types, non-table tool entries, and a
/// non-table `[tools]` all become `PolicyError` receipts. The `gate` key is
/// validated against `ToolGatePolicy` plus the scope allowlist; an invalid
/// scope strips only `scope` so the threshold siblings keep working under the
/// only semantics that exist (`on-diff`).
fn sanitize_tools_section(table: &mut toml::value::Table, errors: &mut Vec<PolicyError>) {
    let Some(tools) = table.get_mut("tools") else {
        return;
    };
    let Some(tools_table) = tools.as_table_mut() else {
        errors.push(PolicyError {
            section: "tools".to_owned(),
            detail: format!(
                "invalid [tools]: expected a table of [tools.<id>] tables, found {}",
                tools.type_str()
            ),
        });
        table.remove("tools");
        return;
    };
    let ids = tools_table.keys().cloned().collect::<Vec<_>>();
    for id in ids {
        let Some(tool) = tools_table.get_mut(&id) else {
            continue;
        };
        let Some(tool_table) = tool.as_table_mut() else {
            errors.push(PolicyError {
                section: format!("tools.{id}"),
                detail: format!(
                    "invalid [tools.{id}]: expected a table, found {}",
                    tool.type_str()
                ),
            });
            tools_table.remove(&id);
            continue;
        };
        let entries = tool_table
            .iter()
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect::<Vec<_>>();
        for (key, entry) in entries {
            if key == "gate" {
                sanitize_tool_gate_value(&id, tool_table, entry, errors);
                continue;
            }
            if !KNOWN_TOOL_POLICY_KEYS.contains(&key.as_str()) {
                errors.push(PolicyError {
                    section: format!("tools.{id}.{key}"),
                    detail: format!(
                        "unknown [tools.{id}] key `{key}`; expected one of: {}",
                        KNOWN_TOOL_POLICY_KEYS.join(", ")
                    ),
                });
                tool_table.remove(&key);
                continue;
            }
            let mut probe = toml::value::Table::new();
            probe.insert(key.clone(), entry);
            if let Err(err) = toml::Value::Table(probe).try_into::<ToolPolicyInput>() {
                errors.push(PolicyError {
                    section: format!("tools.{id}.{key}"),
                    detail: format!("invalid [tools.{id}] key `{key}`: {err}"),
                });
                tool_table.remove(&key);
            }
        }
    }
}

fn sanitize_tool_gate_value(
    id: &str,
    tool_table: &mut toml::value::Table,
    gate: toml::Value,
    errors: &mut Vec<PolicyError>,
) {
    match gate.try_into::<ToolGatePolicy>() {
        Err(err) => {
            errors.push(PolicyError {
                section: format!("tools.{id}.gate"),
                detail: format!("invalid [tools.{id}.gate] table: {err}"),
            });
            tool_table.remove("gate");
        }
        Ok(policy) => {
            if let Some(scope) = policy.scope.as_deref()
                && !KNOWN_TOOL_GATE_SCOPES.contains(&scope)
            {
                errors.push(PolicyError {
                    section: format!("tools.{id}.gate.scope"),
                    detail: format!(
                        "unknown [tools.{id}.gate] scope `{scope}`; expected one of: {} \
                         (repo-wide tool-gate scoping does not exist yet)",
                        KNOWN_TOOL_GATE_SCOPES.join(", ")
                    ),
                });
                if let Some(gate_table) = tool_table
                    .get_mut("gate")
                    .and_then(toml::Value::as_table_mut)
                {
                    gate_table.remove("scope");
                }
            }
        }
    }
}

/// Per-key validation for `[proof]`: unknown keys (for example a misspelled
/// `[[proof.requierd]]`), a non-table `[proof]`, and `[proof.required]`
/// written as a single table instead of an array of tables all become
/// `PolicyError` receipts; well-formed `[[proof.required]]` entries are then
/// validated individually.
fn sanitize_proof_section(table: &mut toml::value::Table, errors: &mut Vec<PolicyError>) {
    let Some(proof) = table.get_mut("proof") else {
        return;
    };
    let Some(proof_table) = proof.as_table_mut() else {
        errors.push(PolicyError {
            section: "proof".to_owned(),
            detail: format!(
                "invalid [proof]: expected a table, found {}",
                proof.type_str()
            ),
        });
        table.remove("proof");
        return;
    };
    let unknown_keys = proof_table
        .keys()
        .filter(|key| key.as_str() != "required")
        .cloned()
        .collect::<Vec<_>>();
    for key in unknown_keys {
        errors.push(PolicyError {
            section: format!("proof.{key}"),
            detail: format!(
                "unknown [proof] key `{key}`; expected `required` ([[proof.required]] entries)"
            ),
        });
        proof_table.remove(&key);
    }
    let Some(required) = proof_table.get_mut("required") else {
        return;
    };
    let Some(required_array) = required.as_array_mut() else {
        errors.push(PolicyError {
            section: "proof.required".to_owned(),
            detail: format!(
                "invalid [proof.required]: expected an array of tables ([[proof.required]]), \
                 found {}",
                required.type_str()
            ),
        });
        proof_table.remove("required");
        return;
    };
    let entries = std::mem::take(required_array);
    for (index, entry) in entries.into_iter().enumerate() {
        match validate_required_proof_policy(index, &entry) {
            Ok(()) => required_array.push(entry),
            Err(error) => errors.push(error),
        }
    }
}

/// Strict per-entry validation for `[[proof.required]]`: unknown keys, empty
/// commands, and unknown selector values are policy errors instead of
/// silently inert policies.
fn validate_required_proof_policy(
    index: usize,
    entry: &toml::Value,
) -> std::result::Result<(), PolicyError> {
    let section = required_proof_policy_section(index, entry);
    let policy: RequiredProofPolicy = entry.clone().try_into().map_err(|err| PolicyError {
        section: section.clone(),
        detail: format!("invalid [[proof.required]] entry: {err}"),
    })?;
    if policy.command.trim().is_empty() {
        return Err(PolicyError {
            section,
            detail: "[[proof.required]] entry has an empty `command`; a required proof policy \
                     must name the command it proves"
                .to_owned(),
        });
    }
    for diff_class in &policy.diff_classes {
        if !known_policy_selector(diff_class, KNOWN_POLICY_DIFF_CLASSES) {
            return Err(PolicyError {
                section,
                detail: format!(
                    "unknown diff_classes selector `{diff_class}`; expected a wildcard (*, any, \
                     all) or one of: {}",
                    KNOWN_POLICY_DIFF_CLASSES.join(", ")
                ),
            });
        }
    }
    for language in &policy.languages {
        if !known_policy_selector(language, KNOWN_POLICY_LANGUAGES) {
            return Err(PolicyError {
                section,
                detail: format!(
                    "unknown languages selector `{language}`; expected a wildcard (*, any, all) \
                     or one of: {}",
                    KNOWN_POLICY_LANGUAGES.join(", ")
                ),
            });
        }
    }
    Ok(())
}

fn required_proof_policy_section(index: usize, entry: &toml::Value) -> String {
    match entry.get("id").and_then(toml::Value::as_str) {
        Some(id) if !id.trim().is_empty() => format!("proof.required.{}", id.trim()),
        _ => format!("proof.required[{index}]"),
    }
}

impl Limits {
    pub(crate) fn summary_line(&self) -> String {
        format!(
            "logical_lanes={} llm={} sensor_jobs={} grep={} tests={} builds={}",
            self.logical_lanes,
            self.llm_in_flight,
            self.sensor_jobs,
            self.grep,
            self.tests,
            self.builds
        )
    }
}

pub(crate) fn runtime_profile_from_toml(text: &str) -> Result<Profile> {
    let profile: RuntimeProfileFile = toml::from_str(text)?;
    Ok(Profile {
        name: profile.name,
        limits: Limits {
            logical_lanes: profile.limits.logical_lanes,
            llm_in_flight: profile.limits.llm_in_flight,
            sensor_jobs: profile.limits.sensor_jobs,
            repo_read: profile.limits.repo_read,
            raw_file_reads: profile.limits.raw_file_reads,
            grep: profile.limits.grep,
            ast_grep: profile.limits.ast_grep,
            git: profile.limits.git,
            tests: profile.limits.tests,
            builds: profile.limits.builds,
            rust_analyzer: profile.limits.rust_analyzer,
            summary_writers: profile.limits.summary_writers,
            patch_writers: profile.limits.patch_writers,
        },
        guards: Guards {
            min_free_mem_mb: profile.guards.min_free_mem_mb,
            min_free_disk_mb: profile.guards.min_free_disk_mb,
            max_load_1m: profile.guards.max_load_1m,
        },
        budgets: Budgets {
            artifact_budget_mb: profile.budgets.artifact_budget_mb,
            scratch_budget_mb: profile.budgets.scratch_budget_mb,
            default_timeout_sec: profile.budgets.default_timeout_sec,
            hard_timeout_sec: profile.budgets.hard_timeout_sec,
            proof_max_focused_test_files: profile.budgets.proof_max_focused_test_files,
            proof_max_focused_tests: profile.budgets.proof_max_focused_tests,
            proof_command_timeout_sec: profile.budgets.proof_command_timeout_sec,
            proof_total_timeout_sec: profile.budgets.proof_total_timeout_sec,
            proof_cpu: profile.budgets.proof_cpu,
            proof_memory_mb: profile.budgets.proof_memory_mb,
            proof_disk_mb: profile.budgets.proof_disk_mb,
            proof_network: profile.budgets.proof_network,
            proof_scratch: profile.budgets.proof_scratch,
            mutation: profile.budgets.mutation,
            sanitizer: profile.budgets.sanitizer,
        },
        trusted_repo: profile.trusted_repo,
        tool_timeouts: profile.tool_timeouts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_sensor_phase_classifies_by_class_and_lease() {
        // Compile/test-requiring classes and leased witnesses are late.
        assert_eq!(
            default_sensor_phase(ToolClass::Test, false),
            SensorPhase::Late
        );
        assert_eq!(
            default_sensor_phase(ToolClass::Build, false),
            SensorPhase::Late
        );
        assert_eq!(
            default_sensor_phase(ToolClass::Coverage, false),
            SensorPhase::Late
        );
        assert_eq!(
            default_sensor_phase(ToolClass::HeavyWitness, false),
            SensorPhase::Late
        );
        // A lease requirement forces late regardless of class.
        assert_eq!(
            default_sensor_phase(ToolClass::Static, true),
            SensorPhase::Late
        );
        // Non-compiling static signal is fast.
        for class in [
            ToolClass::Packet,
            ToolClass::Static,
            ToolClass::Search,
            ToolClass::Workflow,
            ToolClass::Security,
        ] {
            assert_eq!(default_sensor_phase(class, false), SensorPhase::Fast);
        }
    }

    #[test]
    fn tool_policy_phase_key_overrides_class_default() -> anyhow::Result<()> {
        let mut config = Config::from_toml_with_policy_receipts(
            r#"
[tools.cargo-fmt]
enabled = true
class = "build"
phase = "fast"

[tools.cargo-test]
enabled = true
class = "test"
"#,
        )?;
        assert!(
            config.policy_errors.is_empty(),
            "valid phase values must not receipt: {:?}",
            config.policy_errors
        );
        config.merge_defaults();
        let fmt = config
            .tools
            .get("cargo-fmt")
            .ok_or_else(|| anyhow::anyhow!("cargo-fmt tool missing"))?;
        assert_eq!(fmt.phase, Some(SensorPhase::Fast));
        assert_eq!(fmt.effective_phase(), SensorPhase::Fast);
        let test = config
            .tools
            .get("cargo-test")
            .ok_or_else(|| anyhow::anyhow!("cargo-test tool missing"))?;
        assert_eq!(test.phase, None);
        assert_eq!(test.effective_phase(), SensorPhase::Late);
        Ok(())
    }

    #[test]
    fn invalid_tool_phase_value_is_stripped_with_policy_error() -> anyhow::Result<()> {
        let config = Config::from_toml_with_policy_receipts(
            r#"
[tools.cargo-fmt]
enabled = true
class = "build"
phase = "sometime"
"#,
        )?;
        assert!(
            config
                .policy_errors
                .iter()
                .any(|error| error.section == "tools.cargo-fmt.phase"),
            "expected a policy error for the invalid phase value: {:?}",
            config.policy_errors
        );
        let fmt = config
            .tools
            .get("cargo-fmt")
            .ok_or_else(|| anyhow::anyhow!("cargo-fmt tool missing"))?;
        assert_eq!(fmt.phase, None, "invalid phase value must be stripped");
        Ok(())
    }

    fn provider_max_concurrency_value<'a>(
        value: &'a toml::Value,
        provider: &str,
    ) -> Option<&'a toml::Value> {
        value
            .get("providers")
            .and_then(toml::Value::as_table)
            .and_then(|providers| providers.get(provider))
            .and_then(toml::Value::as_table)
            .and_then(|table| table.get("max_concurrency"))
    }

    fn provider_prompt_cache_value<'a>(
        value: &'a toml::Value,
        provider: &str,
    ) -> Option<&'a toml::Value> {
        value
            .get("providers")
            .and_then(toml::Value::as_table)
            .and_then(|providers| providers.get(provider))
            .and_then(toml::Value::as_table)
            .and_then(|table| table.get("prompt_cache"))
    }

    fn providers_max_concurrency_document(
        minimax: toml::Value,
        opencode: toml::Value,
    ) -> toml::Value {
        let mut minimax_table = toml::value::Table::new();
        minimax_table.insert("max_concurrency".to_owned(), minimax);
        let mut opencode_table = toml::value::Table::new();
        opencode_table.insert("max_concurrency".to_owned(), opencode);
        let mut providers = toml::value::Table::new();
        providers.insert("minimax".to_owned(), toml::Value::Table(minimax_table));
        providers.insert("opencode".to_owned(), toml::Value::Table(opencode_table));
        let mut root = toml::value::Table::new();
        root.insert("providers".to_owned(), toml::Value::Table(providers));
        toml::Value::Table(root)
    }

    fn providers_prompt_cache_document(minimax: toml::Value, opencode: toml::Value) -> toml::Value {
        let mut minimax_table = toml::value::Table::new();
        minimax_table.insert("prompt_cache".to_owned(), minimax);
        let mut opencode_table = toml::value::Table::new();
        opencode_table.insert("prompt_cache".to_owned(), opencode);
        let mut providers = toml::value::Table::new();
        providers.insert("minimax".to_owned(), toml::Value::Table(minimax_table));
        providers.insert("opencode".to_owned(), toml::Value::Table(opencode_table));
        let mut root = toml::value::Table::new();
        root.insert("providers".to_owned(), toml::Value::Table(providers));
        toml::Value::Table(root)
    }

    // Config-parse oracles live in this module, next to the structs they
    // pin, so reach analysis connects them without crossing files (the
    // main.rs test mod's cross-file reach is unreliable upstream,
    // ripr-swarm#1054, and line-keyed suppressions rot, ripr-swarm#1053).
    // The behavior-level twins (D2 precedence, broker planning, lane
    // merging) stay in main.rs where those functions live.

    #[test]
    fn impact_resolved_mode_defaults_to_shadow_and_clamps_invalid() {
        // Default (no [impact] section) -> shadow.
        let default = ImpactConfig::default();
        assert_eq!(default.mode, "");
        assert_eq!(default.resolved_mode(), "shadow");
        // Explicit active -> active.
        let active = ImpactConfig {
            mode: "active".to_owned(),
        };
        assert_eq!(active.resolved_mode(), "active");
        // Garbage must NEVER accidentally promote execution -> shadow.
        let bogus = ImpactConfig {
            mode: "production".to_owned(),
        };
        assert_eq!(bogus.resolved_mode(), "shadow");
    }

    #[test]
    fn impact_section_parses_and_is_known_top_level() -> anyhow::Result<()> {
        // An [impact] section must be accepted (not an unknown-key policy
        // error) since `impact` is registered in KNOWN_TOP_LEVEL_KEYS.
        let cfg: Config = toml::from_str("[impact]\nmode = \"active\"\n")?;
        assert_eq!(cfg.impact.mode, "active");
        assert_eq!(cfg.impact.resolved_mode(), "active");
        Ok(())
    }

    #[test]
    fn providers_section_parses_policy_and_receipts_invalid_values() -> anyhow::Result<()> {
        let absent: Config = toml::from_str("")?;
        assert_eq!(absent.providers.policy, "");

        let explicit: Config = toml::from_str(
            "[providers]\npolicy = \"primary-with-fallback\"\n\n[providers.minimax]\nenabled = true\nmax_concurrency = 12\nprompt_cache = \"explicit-anthropic\"\n",
        )?;
        assert_eq!(explicit.providers.policy, "primary-with-fallback");
        assert_eq!(explicit.providers.minimax.max_concurrency, Some(12));
        assert_eq!(
            explicit.providers.minimax.prompt_cache.as_deref(),
            Some("explicit-anthropic")
        );
        assert_eq!(explicit.providers.opencode.max_concurrency, None);

        // sanitize_providers_section strips an invalid policy value with a
        // receipt: parse the raw document the way load_or_default does.
        let mut value: toml::Value = toml::from_str("[providers]\npolicy = \"minimax-primry\"\n")?;
        let errors = sanitize_policy_sections(&mut value);
        // Exact oracle on the receipt push: exactly one error, exact
        // section, and a detail that names both the rejected value and the
        // accepted vocabulary.
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].section, "providers");
        assert_eq!(
            errors[0].detail,
            "invalid [providers] policy value \"minimax-primry\"; expected one of: auto, \
             minimax-primary, primary-with-fallback, minimax-only, \
             opencode-go-canary, opencode-go-wide"
        );
        assert!(
            !value
                .get("providers")
                .and_then(toml::Value::as_table)
                .is_some_and(|providers| providers.contains_key("policy"))
        );
        let sanitized: Config = value.try_into()?;
        assert_eq!(sanitized.providers.policy, "");

        // A valid policy survives sanitization untouched.
        let mut valid: toml::Value = toml::from_str("[providers]\npolicy = \"minimax-only\"\n")?;
        assert!(sanitize_policy_sections(&mut valid).is_empty());
        let kept: Config = valid.try_into()?;
        assert_eq!(kept.providers.policy, "minimax-only");
        Ok(())
    }

    #[test]
    fn providers_section_receipts_invalid_max_concurrency() -> anyhow::Result<()> {
        let mut value = providers_max_concurrency_document(
            toml::Value::Integer(0),
            toml::Value::String("many".to_owned()),
        );
        assert_eq!(
            provider_max_concurrency_value(&value, "minimax").and_then(toml::Value::as_integer),
            Some(0)
        );
        assert_eq!(
            provider_max_concurrency_value(&value, "opencode").and_then(toml::Value::as_str),
            Some("many")
        );
        let errors = sanitize_policy_sections(&mut value);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].section, "providers.minimax.max_concurrency");
        assert_eq!(
            errors[0].detail,
            "invalid [providers.minimax] max_concurrency value 0; expected a positive integer"
        );
        assert_eq!(errors[1].section, "providers.opencode.max_concurrency");
        assert_eq!(
            errors[1].detail,
            "invalid [providers.opencode] max_concurrency value \"many\"; expected a positive integer"
        );
        let Some(providers) = value.get("providers").and_then(toml::Value::as_table) else {
            anyhow::bail!("providers table should remain after sanitization");
        };
        for provider in ["minimax", "opencode"] {
            assert!(
                !providers
                    .get(provider)
                    .and_then(toml::Value::as_table)
                    .is_some_and(|table| table.contains_key("max_concurrency")),
                "{provider} max_concurrency should be stripped from the raw TOML table"
            );
        }

        let sanitized: Config = value.try_into()?;
        assert_eq!(sanitized.providers.minimax.max_concurrency, None);
        assert_eq!(sanitized.providers.opencode.max_concurrency, None);

        let mut valid =
            providers_max_concurrency_document(toml::Value::Integer(1), toml::Value::Integer(2));
        assert_eq!(
            provider_max_concurrency_value(&valid, "minimax").and_then(toml::Value::as_integer),
            Some(1)
        );
        assert_eq!(
            provider_max_concurrency_value(&valid, "opencode").and_then(toml::Value::as_integer),
            Some(2)
        );
        assert!(sanitize_policy_sections(&mut valid).is_empty());
        let Some(providers) = valid.get("providers").and_then(toml::Value::as_table) else {
            anyhow::bail!("providers table should survive valid sanitization");
        };
        assert_eq!(
            providers
                .get("minimax")
                .and_then(toml::Value::as_table)
                .and_then(|table| table.get("max_concurrency"))
                .and_then(toml::Value::as_integer),
            Some(1)
        );
        assert_eq!(
            providers
                .get("opencode")
                .and_then(toml::Value::as_table)
                .and_then(|table| table.get("max_concurrency"))
                .and_then(toml::Value::as_integer),
            Some(2)
        );
        let kept: Config = valid.try_into()?;
        assert_eq!(kept.providers.minimax.max_concurrency, Some(1));
        assert_eq!(kept.providers.opencode.max_concurrency, Some(2));
        Ok(())
    }

    #[test]
    fn providers_section_receipts_invalid_prompt_cache() -> anyhow::Result<()> {
        let mut value = providers_prompt_cache_document(
            toml::Value::String("anthropic-explicit".to_owned()),
            toml::Value::String("explicit-anthropic".to_owned()),
        );
        assert_eq!(
            provider_prompt_cache_value(&value, "minimax").and_then(toml::Value::as_str),
            Some("anthropic-explicit")
        );
        assert_eq!(
            provider_prompt_cache_value(&value, "opencode").and_then(toml::Value::as_str),
            Some("explicit-anthropic")
        );
        let errors = sanitize_policy_sections(&mut value);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].section, "providers.minimax.prompt_cache");
        assert_eq!(
            errors[0].detail,
            "invalid [providers.minimax] prompt_cache value \"anthropic-explicit\"; \
             expected one of: explicit-anthropic, off"
        );
        assert_eq!(errors[1].section, "providers.opencode.prompt_cache");
        assert_eq!(
            errors[1].detail,
            "unsupported [providers.opencode] prompt_cache value \"explicit-anthropic\"; \
             OpenCode prompt caching is not implemented"
        );
        let Some(providers) = value.get("providers").and_then(toml::Value::as_table) else {
            anyhow::bail!("providers table should remain after sanitization");
        };
        for provider in ["minimax", "opencode"] {
            assert!(
                !providers
                    .get(provider)
                    .and_then(toml::Value::as_table)
                    .is_some_and(|table| table.contains_key("prompt_cache")),
                "{provider} prompt_cache should be stripped from the raw TOML table"
            );
        }

        let sanitized: Config = value.try_into()?;
        assert_eq!(sanitized.providers.minimax.prompt_cache, None);
        assert_eq!(sanitized.providers.opencode.prompt_cache, None);

        let mut valid = providers_prompt_cache_document(
            toml::Value::String("off".to_owned()),
            toml::Value::String("unsupported value gets removed before valid parse".to_owned()),
        );
        let Some(providers) = valid
            .get_mut("providers")
            .and_then(toml::Value::as_table_mut)
        else {
            anyhow::bail!("providers table should exist");
        };
        providers.remove("opencode");
        assert!(sanitize_policy_sections(&mut valid).is_empty());
        let kept: Config = valid.try_into()?;
        assert_eq!(kept.providers.minimax.prompt_cache.as_deref(), Some("off"));
        assert_eq!(kept.providers.opencode.prompt_cache, None);
        Ok(())
    }

    #[test]
    fn synchronize_mode_records_deprecation_receipt_and_keeps_gate_siblings() -> anyhow::Result<()>
    {
        let mut gate = toml::value::Table::new();
        gate.insert(
            "required_check".to_owned(),
            toml::Value::String("ub-review/gate".to_owned()),
        );
        gate.insert(
            "post_review_on".to_owned(),
            toml::Value::Array(vec![
                toml::Value::String("opened".to_owned()),
                toml::Value::String("synchronize".to_owned()),
            ]),
        );
        gate.insert(
            "synchronize_mode".to_owned(),
            toml::Value::String("review".to_owned()),
        );
        let mut root = toml::value::Table::new();
        root.insert("gate".to_owned(), toml::Value::Table(gate));
        let mut value = toml::Value::Table(root);
        let errors = sanitize_policy_sections(&mut value);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].section, "gate.synchronize_mode");
        assert_eq!(errors[0].detail, SYNCHRONIZE_MODE_DEPRECATION_DETAIL);
        let Some(gate) = value.get("gate").and_then(toml::Value::as_table) else {
            anyhow::bail!("gate table should survive synchronize_mode sanitization");
        };
        assert!(!gate.contains_key("synchronize_mode"));
        assert!(gate.contains_key("required_check"));
        assert!(gate.contains_key("post_review_on"));

        let sanitized: Config = value.try_into()?;
        assert_eq!(sanitized.gate.required_check, "ub-review/gate");
        assert_eq!(
            sanitized.gate.post_review_on,
            vec!["opened".to_owned(), "synchronize".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn issues_section_parses_exact_field_defaults() -> anyhow::Result<()> {
        let absent: Config = toml::from_str("")?;
        assert!(absent.issues.enabled);
        assert_eq!(absent.issues.mode, "suggest");
        assert_eq!(absent.issues.open_in, Vec::<String>::new());
        assert_eq!(absent.issues.open_cap, 3);

        let explicit: Config = toml::from_str(
            "[issues]\nenabled = false\nmode = \"off\"\nopen_in = [\"EffortlessMetrics/ripr-swarm\"]\nopen_cap = 1\n",
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
    fn unknown_profile_override_records_policy_error_not_silent_fallback() -> anyhow::Result<()> {
        // A typoed profile name (e.g. "gh-runner-fuill") must surface as a
        // PolicyError receipt, not silently downgrade to gh-runner budgets.
        // See issue #608 / tracker UB-25.
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "profile = \"auto\"\n")?;
        let config = Config::load_or_default(&path, Some("gh-runner-fuill"))?;
        assert_eq!(
            config.profile, "gh-runner-fuill",
            "the typoed name is retained for the receipt"
        );
        let profile_receipts: Vec<&PolicyError> = config
            .policy_errors
            .iter()
            .filter(|error| error.section == "profile")
            .collect();
        assert_eq!(
            profile_receipts.len(),
            1,
            "exactly one profile-fallback receipt expected"
        );
        let detail = &profile_receipts[0].detail;
        assert!(
            detail.contains("gh-runner-fuill"),
            "receipt must name the unknown profile: {detail}"
        );
        assert!(
            detail.contains("gh-runner"),
            "receipt must name the fallback: {detail}"
        );
        Ok(())
    }

    #[test]
    fn known_profile_override_records_no_fallback_receipt() -> anyhow::Result<()> {
        // A valid profile name produces no fallback receipt.
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("config.toml");
        std::fs::write(&path, "profile = \"auto\"\n")?;
        let config = Config::load_or_default(&path, Some("gh-runner-full"))?;
        assert_eq!(config.profile, "gh-runner-full");
        let profile_receipts: Vec<&PolicyError> = config
            .policy_errors
            .iter()
            .filter(|error| error.section == "profile")
            .collect();
        assert!(
            profile_receipts.is_empty(),
            "known profile must not produce a fallback receipt"
        );
        Ok(())
    }
}
