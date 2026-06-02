//! Box-aware evidence packet runner for UB-focused PR review.
//!
//! The binary prepares deterministic receipts, model-review artifacts, and lane
//! packets. Posting is a separate command that submits one grouped pull request
//! review when explicitly configured.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Command as ProcessCommand, ExitStatus, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wait_timeout::ChildExt;

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => cmd_init(args),
        Command::Doctor(args) => cmd_doctor(args),
        Command::Plan(args) => cmd_plan(args),
        Command::Run(args) => cmd_run(args),
        Command::Summary(args) => cmd_summary(args),
        Command::Post(args) => cmd_post(args),
    }
}

#[derive(Debug, Parser)]
#[command(name = "ub-review")]
#[command(version)]
#[command(about = "Build box-aware evidence packets for UB-focused PR review")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Write a starter .ub-review.toml.
    Init(InitArgs),
    /// Print box detection and tool availability.
    Doctor(DoctorArgs),
    /// Build and print a run plan without executing sensors.
    Plan(PlanArgs),
    /// Build packets, run eligible sensors, and render lane packets.
    Run(RunArgs),
    /// Re-render a running summary from an existing run directory.
    Summary(SummaryArgs),
    /// Submit a prepared GitHub pull request review.
    Post(PostArgs),
}

#[derive(Clone, Debug, ValueEnum)]
enum ProfileArg {
    GhRunner,
    Cx23,
    Cx33,
    Cx43,
    Auto,
    Custom,
}

impl ProfileArg {
    fn key(&self) -> &'static str {
        match self {
            Self::GhRunner => "gh-runner",
            Self::Cx23 => "cx23",
            Self::Cx33 => "cx33",
            Self::Cx43 => "cx43",
            Self::Auto => "auto",
            Self::Custom => "custom",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PostingMode {
    ArtifactOnly,
    Review,
}

impl PostingMode {
    fn key(self) -> &'static str {
        match self {
            Self::ArtifactOnly => "artifact-only",
            Self::Review => "review",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ModelMode {
    Auto,
    Off,
}

impl ModelMode {
    fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ModelProviderPolicy {
    Auto,
    MinimaxPrimary,
    MinimaxOnly,
    OpencodeGoCanary,
    OpencodeGoWide,
}

impl ModelProviderPolicy {
    fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::MinimaxPrimary => "minimax-primary",
            Self::MinimaxOnly => "minimax-only",
            Self::OpencodeGoCanary => "opencode-go-canary",
            Self::OpencodeGoWide => "opencode-go-wide",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProviderKindArg {
    Openai,
    Anthropic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum OpenCodeEndpointKindArg {
    Auto,
    OpenaiChat,
    AnthropicMessages,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RunMode {
    ReviewDirect,
    AgentInvestigate,
    AgentPatch,
}

impl RunMode {
    fn key(self) -> &'static str {
        match self {
            Self::ReviewDirect => "review-direct",
            Self::AgentInvestigate => "agent-investigate",
            Self::AgentPatch => "agent-patch",
        }
    }
}

#[derive(Clone, Debug, Args)]
struct ReviewArgs {
    /// Repository root.
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    root: PathBuf,
    /// Base ref.
    #[arg(long, default_value = "origin/main", env = "UB_REVIEW_BASE")]
    base: String,
    /// Head ref.
    #[arg(long, default_value = "HEAD", env = "UB_REVIEW_HEAD")]
    head: String,
    /// Config path.
    #[arg(long, default_value = ".ub-review.toml", env = "UB_REVIEW_CONFIG")]
    config: PathBuf,
    /// Output run directory.
    #[arg(long, default_value = "target/ub-review", env = "UB_REVIEW_OUT")]
    out: PathBuf,
    /// Box profile override.
    #[arg(long, value_enum, env = "UB_REVIEW_PROFILE")]
    profile: Option<ProfileArg>,
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Config file path to write.
    #[arg(long, default_value = ".ub-review.toml")]
    path: PathBuf,
    /// Profile to write into the config.
    #[arg(long, value_enum, default_value = "gh-runner")]
    profile: ProfileArg,
    /// Overwrite existing config.
    #[arg(long)]
    force: bool,
}

#[derive(Debug, Args)]
struct DoctorArgs {
    #[arg(long, default_value = ".ub-review.toml", env = "UB_REVIEW_CONFIG")]
    config: PathBuf,
    #[arg(long, value_enum, env = "UB_REVIEW_PROFILE")]
    profile: Option<ProfileArg>,
}

#[derive(Debug, Args)]
struct PlanArgs {
    #[command(flatten)]
    review: ReviewArgs,
    /// Write plan artifacts under the run directory.
    #[arg(long)]
    write: bool,
    /// Allow heavy/manual witnesses in the plan.
    #[arg(long)]
    allow_heavy: bool,
}

#[derive(Debug, Args)]
struct RunArgs {
    #[command(flatten)]
    review: ReviewArgs,
    /// Do not execute external sensors.
    #[arg(long)]
    dry_run: bool,
    /// Allow heavy/manual witnesses.
    #[arg(long)]
    allow_heavy: bool,
    /// Do not append running-summary.md to $GITHUB_STEP_SUMMARY.
    #[arg(long)]
    no_github_summary: bool,
    /// Review posting intent. `run` only prepares artifacts; `post` submits.
    #[arg(
        long,
        value_enum,
        default_value = "artifact-only",
        env = "UB_REVIEW_POSTING"
    )]
    posting: PostingMode,
    /// Review execution mode. Default uses direct BYOK MiniMax fanout.
    #[arg(
        long,
        value_enum,
        default_value = "review-direct",
        env = "UB_REVIEW_MODE"
    )]
    mode: RunMode,
    /// Model execution mode.
    #[arg(long, value_enum, default_value = "auto", env = "UB_REVIEW_MODEL_MODE")]
    model_mode: ModelMode,
    /// Maximum inline comments to include in github-review.json.
    #[arg(long, default_value_t = 8, env = "UB_REVIEW_MAX_INLINE_COMMENTS")]
    max_inline_comments: usize,
    /// Planned model concurrency for model lane packets.
    #[arg(long, default_value_t = 8, env = "UB_REVIEW_MODEL_CONCURRENCY")]
    model_concurrency: usize,
    /// Maximum planned model calls.
    #[arg(long, default_value_t = 14, env = "UB_REVIEW_MAX_MODEL_CALLS")]
    max_model_calls: usize,
    /// Provider policy.
    #[arg(
        long = "provider-policy",
        alias = "model-provider-policy",
        value_enum,
        default_value = "minimax-primary",
        env = "UB_REVIEW_PROVIDER_POLICY"
    )]
    provider_policy: ModelProviderPolicy,
    /// Number of Bun review lanes to prepare: 6, 10, or 20.
    #[arg(long, default_value_t = 10, env = "UB_REVIEW_LANE_WIDTH")]
    lane_width: usize,
    /// Per-model-call timeout in seconds.
    #[arg(long, default_value_t = 180, env = "UB_REVIEW_MODEL_TIMEOUT_SEC")]
    model_timeout_sec: u64,
    /// Optional read-only UB ledger path.
    #[arg(long, default_value = "", env = "UB_REVIEW_LEDGER_PATH")]
    ledger_path: String,
    /// Maximum bytes of UB ledger context.
    #[arg(long, default_value_t = 65_536, env = "UB_REVIEW_LEDGER_MAX_BYTES")]
    ledger_max_bytes: usize,
    /// MiniMax provider request/response family.
    #[arg(
        long,
        value_enum,
        default_value = "openai",
        env = "UB_REVIEW_MINIMAX_PROVIDER_KIND"
    )]
    minimax_provider_kind: ProviderKindArg,
    /// MiniMax model name.
    #[arg(long, default_value = "MiniMax-M3", env = "UB_REVIEW_MINIMAX_MODEL")]
    minimax_model: String,
    /// OpenCode Go model name for canary lanes.
    #[arg(long, default_value = "minimax-m3", env = "UB_REVIEW_OPENCODE_MODEL")]
    opencode_model: String,
    /// OpenCode Go endpoint family.
    #[arg(
        long,
        value_enum,
        default_value = "auto",
        env = "UB_REVIEW_OPENCODE_ENDPOINT_KIND"
    )]
    opencode_endpoint_kind: OpenCodeEndpointKindArg,
    /// Maximum bytes in the GitHub review body.
    #[arg(
        long,
        default_value_t = 60_000,
        env = "UB_REVIEW_REVIEW_BODY_MAX_BYTES"
    )]
    review_body_max_bytes: usize,
}

#[derive(Debug, Args)]
struct SummaryArgs {
    #[arg(long, default_value = "target/ub-review")]
    run_dir: PathBuf,
}

#[derive(Debug, Args)]
struct PostArgs {
    /// Prepared GitHub review payload.
    #[arg(long, default_value = "target/ub-review/review/github-review.json")]
    review_json: PathBuf,
    /// Directory for post-result.json or post-error.json.
    #[arg(long, default_value = "target/ub-review/review")]
    out: PathBuf,
    /// GitHub token with pull-request write permission.
    #[arg(long, env = "UB_REVIEW_GITHUB_TOKEN")]
    github_token: Option<String>,
    /// owner/repo. Defaults to GITHUB_REPOSITORY.
    #[arg(long, env = "GITHUB_REPOSITORY")]
    repo: Option<String>,
    /// Pull request number. Defaults to GITHUB_EVENT_PATH pull_request.number.
    #[arg(long, env = "UB_REVIEW_PULL_NUMBER")]
    pull_number: Option<u64>,
    /// GitHub API base URL.
    #[arg(
        long,
        default_value = "https://api.github.com",
        env = "UB_REVIEW_GITHUB_API_URL"
    )]
    github_api_url: String,
    /// Return a failing exit code when posting fails.
    #[arg(long)]
    fail_on_post_error: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct Config {
    profile: String,
    repo: RepoConfig,
    review: ReviewConfig,
    profiles: BTreeMap<String, Profile>,
    tools: BTreeMap<String, ToolPolicy>,
    lanes: Vec<LanePlan>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct RepoConfig {
    kind: String,
    ledger: String,
    base: String,
    head: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct ReviewConfig {
    posting_engine: String,
    custom_poster: bool,
    ban_standalone_approval: bool,
    require_zero_finding_audit: bool,
    enable_default_lanes: bool,
    github_summary: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct Profile {
    name: String,
    limits: Limits,
    guards: Guards,
    budgets: Budgets,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct Limits {
    logical_lanes: usize,
    llm_in_flight: usize,
    sensor_jobs: usize,
    repo_read: usize,
    raw_file_reads: usize,
    grep: usize,
    ast_grep: usize,
    git: usize,
    tests: usize,
    builds: usize,
    rust_analyzer: usize,
    summary_writers: usize,
    patch_writers: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct Guards {
    min_free_mem_mb: u64,
    min_free_disk_mb: u64,
    max_load_1m: f32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct Budgets {
    artifact_budget_mb: u64,
    scratch_budget_mb: u64,
    default_timeout_sec: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct ToolPolicy {
    id: String,
    command: String,
    class: ToolClass,
    weight: u32,
    default: Trigger,
    timeout_sec: u64,
    artifact_budget_mb: u64,
    requires_lease: bool,
    enabled: bool,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum ToolClass {
    Packet,
    #[default]
    Static,
    Search,
    Workflow,
    Security,
    Test,
    Build,
    HeavyWitness,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
enum Trigger {
    Always,
    SourceChanged,
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

#[derive(Clone, Debug, Deserialize, Serialize)]
struct Plan {
    base: String,
    head: String,
    profile_name: String,
    sensors: Vec<SensorPlan>,
    lanes: Vec<LanePlan>,
    docs_only: bool,
    notes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct SensorPlan {
    id: String,
    command: String,
    run: bool,
    reason: String,
    timeout_sec: u64,
    class: ToolClass,
    weight: u32,
    requires_lease: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct LanePlan {
    id: String,
    role: String,
    model: String,
    model_display: String,
    receives: Vec<String>,
    focus: String,
}

#[derive(Clone, Debug, Serialize)]
struct ReviewArtifacts {
    shared_context_id: String,
    mode: String,
    posting: String,
    model_mode: String,
    provider_policy: String,
    model_provider_policy: String,
    lane_width: usize,
    model_concurrency: usize,
    max_model_calls: usize,
    max_inline_comments: usize,
    model_timeout_sec: u64,
    ledger_path: String,
    ledger_max_bytes: usize,
    provider_preflights: Vec<ProviderPreflightReceipt>,
    model_lanes: Vec<ModelLaneReceipt>,
    missing_or_failed_sensor_evidence: Vec<SensorEvidenceIssue>,
    missing_or_failed_model_evidence: Vec<ModelEvidenceIssue>,
    inline_comments: Vec<ReviewInlineComment>,
    summary_only_findings: Vec<SummaryOnlyFinding>,
    body: String,
}

#[derive(Clone, Debug, Serialize)]
struct ReviewMetrics {
    schema_version: u32,
    shared_context_id: String,
    base: String,
    head: String,
    profile_name: String,
    mode: String,
    posting: String,
    model_mode: String,
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
    summary_only_findings: usize,
    missing_or_failed_sensor_evidence: usize,
    missing_or_failed_model_evidence: usize,
    review_body_bytes: usize,
    review_body_truncated: bool,
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
}

#[derive(Debug, Deserialize)]
struct LaneModelOutput {
    summary: Option<String>,
    #[serde(default)]
    inline_comments: Vec<ModelCandidateComment>,
    #[serde(default)]
    summary_only_findings: Vec<ModelCandidateFinding>,
}

#[derive(Debug, Deserialize)]
struct ModelCandidateComment {
    severity: String,
    confidence: String,
    path: String,
    line: u32,
    body: String,
    evidence: String,
}

#[derive(Debug, Deserialize)]
struct ModelCandidateFinding {
    severity: String,
    confidence: String,
    reason: String,
    evidence: String,
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

struct SensorStatusWrite<'a> {
    status: &'a str,
    argv: &'a [String],
    duration_ms: u128,
    reason: &'a str,
    exit_code: Option<i32>,
    timed_out: bool,
}

struct ModelRunContext<'a> {
    root: &'a Path,
    review_dir: &'a Path,
    assignments: &'a [ModelAssignment],
    provider_preflights: &'a [ProviderPreflightReceipt],
    shared_context: &'a str,
    args: &'a RunArgs,
    line_map: &'a BTreeSet<(String, u32)>,
}

struct RefuterRunContext<'a> {
    root: &'a Path,
    review_dir: &'a Path,
    provider_preflights: &'a [ProviderPreflightReceipt],
    shared_context: &'a str,
    args: &'a RunArgs,
    model_calls_used: usize,
}

#[derive(Clone, Debug)]
struct ModelAssignment {
    lane: LanePlan,
    spec: ProviderSpec,
    fallback: Option<ProviderSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ModelProvider {
    MiniMaxDirect,
    OpenCodeGo,
}

impl ModelProvider {
    fn key(self) -> &'static str {
        match self {
            Self::MiniMaxDirect => "minimax",
            Self::OpenCodeGo => "opencode-go",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ProviderEndpointKind {
    OpenAiChat,
    AnthropicMessages,
}

impl ProviderEndpointKind {
    fn key(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai-chat",
            Self::AnthropicMessages => "anthropic-messages",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ProviderSpec {
    provider: ModelProvider,
    model: String,
    endpoint_kind: ProviderEndpointKind,
}

impl ProviderSpec {
    fn label(&self) -> String {
        format!(
            "{}:{}:{}",
            self.provider.key(),
            self.model,
            self.endpoint_kind.key()
        )
    }
}

struct ModelCallOutcome<T> {
    output: T,
    duration_ms: u128,
    http_status: Option<u16>,
    response_shape: String,
}

#[derive(Clone, Debug, Deserialize)]
struct SensorReceipt {
    status: String,
    reason: String,
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
            profile: "gh-runner".to_owned(),
            repo: RepoConfig::default(),
            review: ReviewConfig::default(),
            profiles,
            tools,
            lanes: Vec::new(),
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

impl Default for Profile {
    fn default() -> Self {
        profile(
            "gh-runner",
            20,
            16,
            3,
            6,
            3,
            2,
            2,
            0,
            0,
            1_500,
            4_000,
            6.0,
            750,
            4_000,
            900,
        )
    }
}

impl Default for Limits {
    fn default() -> Self {
        Profile::default().limits
    }
}

impl Default for Guards {
    fn default() -> Self {
        Profile::default().guards
    }
}

impl Default for Budgets {
    fn default() -> Self {
        Profile::default().budgets
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
            timeout_sec: 120,
            artifact_budget_mb: 64,
            requires_lease: false,
            enabled: true,
        }
    }
}

impl Config {
    fn load_or_default(path: &Path, profile_override: Option<&str>) -> Result<Self> {
        let mut config = if path.exists() {
            let text =
                fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
            toml::from_str(&text).with_context(|| format!("parse {}", path.display()))?
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
        Ok(config)
    }

    fn merge_defaults(&mut self) {
        let defaults = Self::default();
        for (key, profile) in defaults.profiles {
            self.profiles.entry(key).or_insert(profile);
        }
        for (key, default_tool) in defaults.tools {
            match self.tools.get_mut(&key) {
                Some(tool) => {
                    if tool.id.is_empty() {
                        tool.id = default_tool.id;
                    }
                    if tool.command.is_empty() {
                        tool.command = default_tool.command;
                    }
                    if tool.timeout_sec == 0 {
                        tool.timeout_sec = default_tool.timeout_sec;
                    }
                    if tool.artifact_budget_mb == 0 {
                        tool.artifact_budget_mb = default_tool.artifact_budget_mb;
                    }
                    if tool.weight == 0 {
                        tool.weight = default_tool.weight;
                    }
                }
                None => {
                    self.tools.insert(key, default_tool);
                }
            }
        }
    }

    fn selected_profile(&self) -> Result<&Profile> {
        self.profiles
            .get(&self.profile)
            .or_else(|| self.profiles.get("gh-runner"))
            .ok_or_else(|| anyhow::anyhow!("no selected profile and no gh-runner fallback"))
    }
}

impl Limits {
    fn summary_line(&self) -> String {
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
}

fn cmd_init(args: InitArgs) -> Result<()> {
    if args.path.exists() && !args.force {
        bail!(
            "{} already exists; pass --force to overwrite",
            args.path.display()
        );
    }
    let config = Config {
        profile: args.profile.key().to_owned(),
        ..Config::default()
    };
    fs::write(&args.path, toml::to_string_pretty(&config)?)?;
    println!("wrote {}", args.path.display());
    Ok(())
}

fn cmd_doctor(args: DoctorArgs) -> Result<()> {
    let config = Config::load_or_default(&args.config, args.profile.as_ref().map(ProfileArg::key))?;
    let profile = config.selected_profile()?;
    let box_state = BoxState::detect()?;
    println!("Profile: {}", profile.name);
    println!("Box: {}", box_state.summary_line());
    println!("Limits: {}", profile.limits.summary_line());
    println!();
    println!("Tools:");
    for tool in config.tools.values() {
        let status = if command_on_path(&tool.command) {
            "found"
        } else {
            "missing"
        };
        println!("  {:<16} {:<8} {}", tool.id, status, tool.command);
    }
    Ok(())
}

fn cmd_plan(args: PlanArgs) -> Result<()> {
    let (config, diff, box_state, plan) = prepare_plan(&args.review, args.allow_heavy)?;
    print_plan(&plan, &box_state);
    if args.write {
        write_plan_artifacts(&args.review.out, &config, &diff, &box_state, &plan)?;
    }
    Ok(())
}

fn cmd_run(args: RunArgs) -> Result<()> {
    validate_run_args(&args)?;
    let (config, diff, box_state, plan) = prepare_plan(&args.review, args.allow_heavy)?;
    print_plan(&plan, &box_state);
    write_plan_artifacts(&args.review.out, &config, &diff, &box_state, &plan)?;

    let event_log = EventLog::open(&args.review.out.join("events.ndjson"))?;
    event_log.append(
        "run_started",
        serde_json::json!({"base": args.review.base, "head": args.review.head, "profile": plan.profile_name, "dry_run": args.dry_run}),
    )?;

    if args.dry_run {
        write_dry_run_sensor_receipts(&args.review.root, &args.review.out, &plan, &event_log)?;
        event_log.append("run_dry", serde_json::json!({"reason": "--dry-run"}))?;
    } else {
        write_skipped_sensor_receipts(&args.review.root, &args.review.out, &plan, &event_log)?;
        run_sensors(&args.review.root, &args.review.out, &plan, &event_log)?;
    }

    write_lane_packets(&args.review.out, &diff, &plan, &event_log)?;
    let summary = render_summary(&args.review.out, &plan, &diff)?;
    fs::write(args.review.out.join("running-summary.md"), &summary)?;
    write_review_artifacts(
        &args.review.root,
        &args.review.out,
        &config,
        &diff,
        &plan,
        &summary,
        &args,
    )?;
    if config.review.github_summary && !args.no_github_summary {
        append_github_step_summary(&summary)?;
    }
    event_log.append(
        "run_finished",
        serde_json::json!({"run_dir": args.review.out}),
    )?;
    println!("wrote {}", args.review.out.display());
    println!("open {}/running-summary.md", args.review.out.display());
    Ok(())
}

fn ensure_supported_mode(mode: RunMode) -> Result<()> {
    match mode {
        RunMode::ReviewDirect => Ok(()),
        RunMode::AgentInvestigate | RunMode::AgentPatch => bail!(
            "{} is reserved for optional leased workers and is not implemented in v0",
            mode.key()
        ),
    }
}

fn validate_run_args(args: &RunArgs) -> Result<()> {
    ensure_supported_mode(args.mode)?;
    if !matches!(args.lane_width, 6 | 10 | 20) {
        bail!("--lane-width must be one of 6, 10, or 20");
    }
    if args.model_timeout_sec == 0 {
        bail!("--model-timeout-sec must be greater than zero");
    }
    if args.review_body_max_bytes < 1_000 {
        bail!("--review-body-max-bytes must be at least 1000");
    }
    Ok(())
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
    match post_github_review(&args) {
        Ok(value) => {
            fs::write(
                args.out.join("post-result.json"),
                serde_json::to_vec_pretty(&value)?,
            )?;
            println!("wrote {}/post-result.json", args.out.display());
            Ok(())
        }
        Err(err) => {
            let value = serde_json::json!({
                "status": "failed",
                "reason": err.to_string(),
            });
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
    }
}

fn prepare_plan(
    args: &ReviewArgs,
    allow_heavy: bool,
) -> Result<(Config, DiffContext, BoxState, Plan)> {
    let config = Config::load_or_default(&args.config, args.profile.as_ref().map(ProfileArg::key))?;
    let profile = config.selected_profile()?;
    let box_state = BoxState::detect()?;
    let diff = DiffContext::from_git(&args.root, &args.base, &args.head)?;
    let plan = build_plan(&config, profile, &box_state, &diff, allow_heavy);
    Ok((config, diff, box_state, plan))
}

fn write_plan_artifacts(
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    box_state: &BoxState,
    plan: &Plan,
) -> Result<()> {
    fs::create_dir_all(out.join("input"))?;
    fs::write(out.join("plan.json"), serde_json::to_vec_pretty(plan)?)?;
    fs::write(
        out.join("effective-config.json"),
        serde_json::to_vec_pretty(config)?,
    )?;
    fs::write(
        out.join("box-state.json"),
        serde_json::to_vec_pretty(box_state)?,
    )?;
    fs::write(
        out.join("input/diff-context.json"),
        serde_json::to_vec_pretty(diff)?,
    )?;
    fs::write(
        out.join("input/changed-files.txt"),
        diff.changed_files.join("\n"),
    )?;
    fs::write(out.join("input/diff.patch"), &diff.patch)?;
    fs::write(out.join("input/pr.md"), render_pr_packet(diff))?;
    fs::write(out.join("input/claims.md"), render_claim_prompt(diff))?;
    Ok(())
}

fn print_plan(plan: &Plan, box_state: &BoxState) {
    println!("Profile: {}", plan.profile_name);
    println!("Box: {}", box_state.summary_line());
    println!("Sensors:");
    for sensor in &plan.sensors {
        let marker = if sensor.run { "run" } else { "skip" };
        println!("  {:<5} {:<16} {}", marker, sensor.id, sensor.reason);
    }
    println!("Lanes:");
    for lane in &plan.lanes {
        println!("  {:<13} {}", lane.id, lane.model_display);
    }
}

impl DiffContext {
    fn from_git(root: &Path, base: &str, head: &str) -> Result<Self> {
        let range = format!("{base}...{head}");
        let changed_files = git_lines(root, &["diff", "--name-only", &range])
            .or_else(|_| git_lines(root, &["diff", "--name-only", base, head]))
            .with_context(|| format!("git diff --name-only {range}"))?;
        let patch = git_text(root, &["diff", "--patch", &range])
            .or_else(|_| git_text(root, &["diff", "--patch", base, head]))
            .unwrap_or_else(|_| String::new());
        let flags = classify_diff(&changed_files, &patch);
        Ok(Self {
            base: base.to_owned(),
            head: head.to_owned(),
            changed_files,
            patch,
            flags,
        })
    }
}

fn git_lines(root: &Path, args: &[&str]) -> Result<Vec<String>> {
    Ok(git_text(root, args)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| "run git")?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn classify_diff(files: &[String], patch: &str) -> DiffFlags {
    let mut flags = DiffFlags {
        docs_only: !files.is_empty(),
        ..DiffFlags::default()
    };
    for path in files {
        let lower = path.to_ascii_lowercase();
        let is_doc = lower.ends_with(".md") || lower.starts_with("docs/");
        flags.docs_only &= is_doc;
        flags.source_changed |= is_source_path(&lower);
        flags.rust_changed |= lower.ends_with(".rs");
        flags.rust_tests_changed |=
            lower.ends_with(".rs") && (lower.contains("test") || lower.contains("tests/"));
        flags.workflow_changed |= lower.starts_with(".github/workflows/")
            || lower.ends_with("action.yml")
            || lower.ends_with("action.yaml");
        flags.dependency_changed |= is_dependency_path(&lower);
        flags.shell_changed |= lower.ends_with(".sh") || lower.starts_with("scripts/");
        flags.cpp_changed |= is_cpp_path(&lower);
        flags.unsafe_or_native_risk |= lower.contains("ffi")
            || lower.contains("jsc")
            || lower.contains("arraybuffer")
            || lower.contains("typedarray")
            || lower.contains("worker")
            || lower.contains("crypto")
            || lower.contains("zstd")
            || lower.contains("src/runtime/")
            || lower.contains("src/bun.js/bindings/");
    }
    let lower_patch = patch.to_ascii_lowercase();
    for token in [
        "unsafe",
        "extern",
        "from_raw_parts",
        "as_ptr",
        "as_mut_ptr",
        "maybeuninit",
        "nonnull",
        "arraybuffer",
        "typedarray",
        "detach",
        "resize",
        "transfer",
        "protect",
        "unprotect",
        "worker",
        "ffi",
        "jsc",
        "stringorbuffer",
        "sharedarraybuffer",
    ] {
        if lower_patch.contains(token) {
            flags.unsafe_or_native_risk = true;
        }
    }
    flags
}

fn is_source_path(path: &str) -> bool {
    [
        ".rs", ".zig", ".cpp", ".cc", ".c", ".h", ".hpp", ".ts", ".tsx", ".js", ".jsx",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
}

fn is_cpp_path(path: &str) -> bool {
    [".c", ".cc", ".cpp", ".cxx", ".h", ".hpp"]
        .iter()
        .any(|suffix| path.ends_with(suffix))
}

fn is_dependency_path(path: &str) -> bool {
    matches!(
        path,
        "cargo.lock"
            | "cargo.toml"
            | "package.json"
            | "bun.lock"
            | "bun.lockb"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "package-lock.json"
    ) || path.ends_with("/cargo.toml")
        || path.ends_with("/cargo.lock")
}

fn build_plan(
    config: &Config,
    profile: &Profile,
    box_state: &BoxState,
    diff: &DiffContext,
    allow_heavy: bool,
) -> Plan {
    let mut notes = Vec::new();
    let guard_ok = guard_ok(profile, box_state, &mut notes);
    let mut sensors = config
        .tools
        .values()
        .map(|tool| plan_tool(tool, profile, diff, guard_ok, allow_heavy))
        .collect::<Vec<_>>();
    sensors.sort_by_key(|sensor| sensor_order(&sensor.id));
    if diff.flags.docs_only {
        notes.push(
            "docs-only diff detected; workflow paths-ignore should normally skip this run"
                .to_owned(),
        );
    }
    if !allow_heavy {
        notes.push("heavy witnesses are disabled unless --allow-heavy is passed".to_owned());
    }
    if profile.name == "gh-runner" {
        notes.push("gh-runner profile: one evidence pass, bounded model review, optional grouped PR Review posting".to_owned());
    }
    Plan {
        base: diff.base.clone(),
        head: diff.head.clone(),
        profile_name: profile.name.clone(),
        sensors,
        lanes: if config.review.enable_default_lanes {
            default_lanes()
        } else {
            Vec::new()
        },
        docs_only: diff.flags.docs_only,
        notes,
    }
}

fn plan_tool(
    tool: &ToolPolicy,
    profile: &Profile,
    diff: &DiffContext,
    guard_ok: bool,
    allow_heavy: bool,
) -> SensorPlan {
    if !tool.enabled {
        return skipped(tool, "disabled by config");
    }
    if tool.requires_lease && !allow_heavy {
        return skipped(tool, "heavy/manual witness requires --allow-heavy");
    }
    if matches!(tool.class, ToolClass::Test) && profile.limits.tests == 0 {
        return skipped(tool, "profile disables test leases");
    }
    if matches!(tool.class, ToolClass::Build) && profile.limits.builds == 0 {
        return skipped(tool, "profile disables build leases");
    }
    if !guard_ok && !matches!(tool.class, ToolClass::Packet) {
        return skipped(tool, "box guard failed; only packet generation is allowed");
    }
    match trigger_match(tool.default, &diff.flags) {
        Some(reason) => SensorPlan {
            id: tool.id.clone(),
            command: tool.command.clone(),
            run: true,
            reason,
            timeout_sec: tool.timeout_sec.min(profile.budgets.default_timeout_sec),
            class: tool.class,
            weight: tool.weight,
            requires_lease: tool.requires_lease,
        },
        None => skipped(tool, "trigger did not match this diff"),
    }
}

fn skipped(tool: &ToolPolicy, reason: &str) -> SensorPlan {
    SensorPlan {
        id: tool.id.clone(),
        command: tool.command.clone(),
        run: false,
        reason: reason.to_owned(),
        timeout_sec: tool.timeout_sec,
        class: tool.class,
        weight: tool.weight,
        requires_lease: tool.requires_lease,
    }
}

fn trigger_match(trigger: Trigger, flags: &DiffFlags) -> Option<String> {
    match trigger {
        Trigger::Always => Some("always-on base packet".to_owned()),
        Trigger::SourceChanged if flags.source_changed => Some("source file changed".to_owned()),
        Trigger::RustBehaviorOrTestsChanged if flags.rust_changed || flags.rust_tests_changed => {
            Some("Rust behavior or tests changed".to_owned())
        }
        Trigger::UnsafeOrNativeRiskChanged if flags.unsafe_or_native_risk => {
            Some("unsafe/native-risk pattern detected".to_owned())
        }
        Trigger::WorkflowChanged if flags.workflow_changed => {
            Some("workflow/action file changed".to_owned())
        }
        Trigger::DependencyChanged if flags.dependency_changed => {
            Some("dependency manifest or lockfile changed".to_owned())
        }
        Trigger::ShellChanged if flags.shell_changed => {
            Some("shell/script file changed".to_owned())
        }
        Trigger::CppChanged if flags.cpp_changed => Some("C/C++ file changed".to_owned()),
        Trigger::Diff => Some("diff-scoped advisory scan".to_owned()),
        Trigger::Manual | Trigger::Never => None,
        _ => None,
    }
}

fn guard_ok(profile: &Profile, box_state: &BoxState, notes: &mut Vec<String>) -> bool {
    let mut ok = true;
    if let Some(mem) = box_state.free_mem_mb
        && mem < profile.guards.min_free_mem_mb
    {
        ok = false;
        notes.push(format!(
            "free memory {mem}MiB is below profile floor {}MiB",
            profile.guards.min_free_mem_mb
        ));
    }
    if let Some(disk) = box_state.free_disk_mb
        && disk < profile.guards.min_free_disk_mb
    {
        ok = false;
        notes.push(format!(
            "free disk {disk}MiB is below profile floor {}MiB",
            profile.guards.min_free_disk_mb
        ));
    }
    if let Some(load) = box_state.load_1m
        && load > profile.guards.max_load_1m
    {
        ok = false;
        notes.push(format!(
            "load average {load:.2} exceeds profile ceiling {:.2}",
            profile.guards.max_load_1m
        ));
    }
    ok
}

fn sensor_order(id: &str) -> u8 {
    match id {
        "tokmd" => 0,
        "ripr" => 1,
        "unsafe-review" => 2,
        "ast-grep" => 3,
        "semgrep" => 4,
        "actionlint" => 5,
        "zizmor" => 6,
        "gitleaks" => 7,
        "osv-scanner" => 8,
        "cargo-audit" => 9,
        "cargo-deny" => 10,
        _ => 50,
    }
}

fn write_skipped_sensor_receipts(
    root: &Path,
    out: &Path,
    plan: &Plan,
    event_log: &EventLog,
) -> Result<()> {
    for sensor in plan.sensors.iter().filter(|sensor| !sensor.run) {
        let dir = out.join("sensors").join(&sensor.id);
        let argv = build_sensor_argv(root, &dir, sensor, plan);
        write_sensor_status(
            out,
            sensor,
            SensorStatusWrite {
                status: "skipped",
                argv: &argv,
                duration_ms: 0,
                reason: &sensor.reason,
                exit_code: None,
                timed_out: false,
            },
        )?;
        event_log.append(
            "sensor_skipped",
            serde_json::json!({"sensor": sensor.id, "reason": sensor.reason}),
        )?;
    }
    Ok(())
}

fn write_dry_run_sensor_receipts(
    root: &Path,
    out: &Path,
    plan: &Plan,
    event_log: &EventLog,
) -> Result<()> {
    for sensor in &plan.sensors {
        let dir = out.join("sensors").join(&sensor.id);
        let argv = build_sensor_argv(root, &dir, sensor, plan);
        let reason = if sensor.run {
            "dry-run; sensor not executed"
        } else {
            &sensor.reason
        };
        write_sensor_status(
            out,
            sensor,
            SensorStatusWrite {
                status: "skipped",
                argv: &argv,
                duration_ms: 0,
                reason,
                exit_code: None,
                timed_out: false,
            },
        )?;
        event_log.append(
            "sensor_skipped",
            serde_json::json!({"sensor": sensor.id, "reason": reason}),
        )?;
    }
    Ok(())
}

fn run_sensors(root: &Path, out: &Path, plan: &Plan, event_log: &EventLog) -> Result<()> {
    let runnable = plan
        .sensors
        .iter()
        .filter(|sensor| sensor.run)
        .cloned()
        .collect::<VecDeque<_>>();
    if runnable.is_empty() {
        event_log.append("sensors_empty", serde_json::json!({}))?;
        return Ok(());
    }
    let jobs = sensor_jobs_for_profile(&plan.profile_name)
        .max(1)
        .min(runnable.len());
    let queue = Arc::new(Mutex::new(runnable));
    let failures = Arc::new(Mutex::new(Vec::<String>::new()));

    thread::scope(|scope| {
        for _ in 0..jobs {
            let queue = Arc::clone(&queue);
            let failures = Arc::clone(&failures);
            scope.spawn(move || {
                loop {
                    let sensor = match queue.lock() {
                        Ok(mut queue) => queue.pop_front(),
                        Err(_) => None,
                    };
                    let Some(sensor) = sensor else {
                        break;
                    };
                    if let Err(err) = run_sensor(root, out, &sensor, event_log, plan)
                        && let Ok(mut failures) = failures.lock()
                    {
                        failures.push(format!("{}: {err:#}", sensor.id));
                    }
                }
            });
        }
    });

    let failures = failures
        .lock()
        .map_err(|_| anyhow::anyhow!("failure list mutex poisoned"))?;
    if !failures.is_empty() {
        event_log.append(
            "sensor_degraded",
            serde_json::json!({"failures": &*failures}),
        )?;
    }
    Ok(())
}

fn sensor_jobs_for_profile(profile: &str) -> usize {
    match profile {
        "cx43" => 6,
        "cx33" => 3,
        "gh-runner" => 4,
        _ => 2,
    }
}

fn run_sensor(
    root: &Path,
    out: &Path,
    sensor: &SensorPlan,
    event_log: &EventLog,
    plan: &Plan,
) -> Result<()> {
    let dir = out.join("sensors").join(&sensor.id);
    fs::create_dir_all(&dir)?;
    let argv = build_sensor_argv(root, &dir, sensor, plan);
    if !command_on_path(&sensor.command) {
        write_sensor_status(
            out,
            sensor,
            SensorStatusWrite {
                status: "missing",
                argv: &argv,
                duration_ms: 0,
                reason: "command not found",
                exit_code: None,
                timed_out: false,
            },
        )?;
        event_log.append(
            "sensor_missing_command",
            serde_json::json!({"sensor": sensor.id, "command": sensor.command}),
        )?;
        return Ok(());
    }
    event_log.append(
        "sensor_started",
        serde_json::json!({"sensor": sensor.id, "argv": argv}),
    )?;
    let stdout_path = dir.join("stdout.txt");
    let stderr_path = dir.join("stderr.txt");
    let result = run_command_to_files(root, &argv, sensor.timeout_sec, &stdout_path, &stderr_path);
    match result {
        Ok(result) => {
            let status = if result.timed_out {
                "timed_out"
            } else if result.success {
                "ok"
            } else {
                "failed"
            };
            write_sensor_status(
                out,
                sensor,
                SensorStatusWrite {
                    status,
                    argv: &argv,
                    duration_ms: result.duration_ms,
                    reason: &result.reason,
                    exit_code: result.exit_code,
                    timed_out: result.timed_out,
                },
            )?;
            event_log.append(
                if result.success {
                    "sensor_completed"
                } else {
                    "sensor_failed"
                },
                serde_json::json!({"sensor": sensor.id, "exit_code": result.exit_code, "timed_out": result.timed_out, "reason": result.reason}),
            )?;
        }
        Err(err) => {
            let reason = format!("{err:#}");
            write_sensor_status(
                out,
                sensor,
                SensorStatusWrite {
                    status: "failed",
                    argv: &argv,
                    duration_ms: 0,
                    reason: &reason,
                    exit_code: None,
                    timed_out: false,
                },
            )?;
            event_log.append(
                "sensor_failed",
                serde_json::json!({"sensor": sensor.id, "reason": reason}),
            )?;
        }
    }
    Ok(())
}

fn build_sensor_argv(root: &Path, dir: &Path, sensor: &SensorPlan, plan: &Plan) -> Vec<String> {
    match sensor.id.as_str() {
        "tokmd" => vec![
            "tokmd".to_owned(),
            "cockpit".to_owned(),
            "--base".to_owned(),
            plan.base.clone(),
            "--head".to_owned(),
            plan.head.clone(),
            "--review-packet-dir".to_owned(),
            dir.join("review").display().to_string(),
        ],
        "ripr" => vec![
            "ripr".to_owned(),
            "first-pr".to_owned(),
            "--root".to_owned(),
            root.display().to_string(),
            "--base".to_owned(),
            plan.base.clone(),
            "--head".to_owned(),
            plan.head.clone(),
        ],
        "unsafe-review" => vec![
            "unsafe-review".to_owned(),
            "first-pr".to_owned(),
            "--root".to_owned(),
            root.display().to_string(),
            "--base".to_owned(),
            plan.base.clone(),
        ],
        "ast-grep" => {
            let config = root.join("tools/ub-rules/sgconfig.yml");
            if config.exists() {
                vec![
                    "ast-grep".to_owned(),
                    "scan".to_owned(),
                    "--config".to_owned(),
                    config.display().to_string(),
                    root.display().to_string(),
                ]
            } else {
                vec!["ast-grep".to_owned(), "--version".to_owned()]
            }
        }
        "semgrep" => vec![
            "semgrep".to_owned(),
            "scan".to_owned(),
            "--config".to_owned(),
            "auto".to_owned(),
            "--json".to_owned(),
            "--output".to_owned(),
            dir.join("report.json").display().to_string(),
        ],
        "actionlint" => vec![
            "actionlint".to_owned(),
            "-format".to_owned(),
            "json".to_owned(),
        ],
        "zizmor" => vec![
            "zizmor".to_owned(),
            ".github/workflows".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
        ],
        "gitleaks" => vec![
            "gitleaks".to_owned(),
            "detect".to_owned(),
            "--redact".to_owned(),
            "--source".to_owned(),
            root.display().to_string(),
            "--report-format".to_owned(),
            "json".to_owned(),
            "--report-path".to_owned(),
            dir.join("report.json").display().to_string(),
        ],
        "osv-scanner" => vec![
            "osv-scanner".to_owned(),
            "scan".to_owned(),
            "--recursive".to_owned(),
            "--format".to_owned(),
            "json".to_owned(),
            ".".to_owned(),
        ],
        "cargo-audit" => vec!["cargo".to_owned(), "audit".to_owned(), "--json".to_owned()],
        "cargo-deny" => vec!["cargo".to_owned(), "deny".to_owned(), "check".to_owned()],
        "shellcheck" => vec!["shellcheck".to_owned(), "--version".to_owned()],
        "cppcheck" => vec!["cppcheck".to_owned(), "--version".to_owned()],
        other => vec![other.to_owned(), "--version".to_owned()],
    }
}

fn run_command_to_files(
    root: &Path,
    argv: &[String],
    timeout_sec: u64,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<CommandStatus> {
    let Some((program, args)) = argv.split_first() else {
        bail!("empty command");
    };
    let stdout =
        File::create(stdout_path).with_context(|| format!("create {}", stdout_path.display()))?;
    let stderr =
        File::create(stderr_path).with_context(|| format!("create {}", stderr_path.display()))?;
    let started = Instant::now();
    let mut child = ProcessCommand::new(program)
        .args(args)
        .current_dir(root)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("spawn {program}"))?;
    let status = match child.wait_timeout(Duration::from_secs(timeout_sec))? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(CommandStatus {
                exit_code: None,
                timed_out: true,
                success: false,
                reason: format!("timed out after {timeout_sec}s"),
                duration_ms: started.elapsed().as_millis(),
            });
        }
    };
    Ok(CommandStatus {
        exit_code: status.code(),
        timed_out: false,
        success: status.success(),
        reason: if status.success() {
            "completed".to_owned()
        } else {
            format!("exit code {:?}", status.code())
        },
        duration_ms: started.elapsed().as_millis(),
    })
}

fn write_sensor_status(
    out: &Path,
    sensor: &SensorPlan,
    fields: SensorStatusWrite<'_>,
) -> Result<()> {
    let dir = out.join("sensors").join(&sensor.id);
    fs::create_dir_all(&dir)?;
    ensure_sensor_text_receipts(&dir)?;
    let value = serde_json::json!({
        "sensor": sensor.id,
        "status": fields.status,
        "command": display_command(fields.argv),
        "duration_ms": fields.duration_ms,
        "reason": fields.reason,
        "outputs": sensor_outputs(sensor),
        "exit_code": fields.exit_code,
        "timed_out": fields.timed_out,
        "timeout_sec": sensor.timeout_sec,
        "class": sensor.class,
        "requires_lease": sensor.requires_lease,
    });
    fs::write(
        dir.join("ub-review-sensor-status.json"),
        serde_json::to_vec_pretty(&value)?,
    )?;
    Ok(())
}

fn ensure_sensor_text_receipts(dir: &Path) -> Result<()> {
    for name in ["stdout.txt", "stderr.txt"] {
        let path = dir.join(name);
        if !path.exists() {
            fs::write(path, b"")?;
        }
    }
    Ok(())
}

fn sensor_outputs(sensor: &SensorPlan) -> Vec<String> {
    let mut outputs = vec!["stdout.txt".to_owned(), "stderr.txt".to_owned()];
    match sensor.id.as_str() {
        "tokmd" => outputs.push("review/".to_owned()),
        "ast-grep" | "semgrep" | "gitleaks" => outputs.push("report.json".to_owned()),
        _ => {}
    }
    outputs
}

fn display_command(argv: &[String]) -> String {
    argv.iter()
        .map(|arg| {
            if arg.chars().any(char::is_whitespace) {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn write_lane_packets(
    out: &Path,
    diff: &DiffContext,
    plan: &Plan,
    event_log: &EventLog,
) -> Result<()> {
    let lane_dir = out.join("lanes");
    fs::create_dir_all(&lane_dir)?;
    for lane in &plan.lanes {
        let mut text = String::new();
        text.push_str(&format!("# Lane: `{}`\n\n", lane.id));
        text.push_str(&format!("Model: `{}`\n\n", lane.model_display));
        text.push_str(&format!("Role: {}\n\n", lane.role));
        text.push_str("## Focus\n\n");
        text.push_str(&lane.focus);
        text.push_str("\n\n## Shared diff\n\n");
        text.push_str(&format!(
            "Base: `{}`\n\nHead: `{}`\n\n",
            diff.base, diff.head
        ));
        text.push_str("Changed files:\n\n");
        for file in &diff.changed_files {
            text.push_str(&format!("- `{file}`\n"));
        }
        text.push_str("\n## Routed sensor evidence\n\n");
        for sensor_id in &lane.receives {
            let status_path = out
                .join("sensors")
                .join(sensor_id)
                .join("ub-review-sensor-status.json");
            let status = read_sensor_receipt(&status_path)
                .map(|receipt| receipt.status)
                .unwrap_or_else(|| "receipt-absent".to_owned());
            text.push_str(&format!("- `{sensor_id}`: `{status}`\n"));
        }
        text.push_str("\n## Review posture\n\n");
        text.push_str(NO_LGTM_POSTURE);
        text.push_str("\n\n## Required output shape\n\n");
        text.push_str(&format!(
            "Start inline comments for this lane with `[{}]`. If no blocking finding exists, write an audit trail: what you checked, strongest failed objection, and residual risk. Do not infer safety from missing sensor receipts.\n",
            lane.id
        ));
        fs::write(lane_dir.join(format!("{}.md", lane.id)), text)?;
        event_log.append("lane_packet_written", serde_json::json!({"lane": lane.id}))?;
    }
    Ok(())
}

fn render_pr_packet(diff: &DiffContext) -> String {
    let mut text = String::new();
    text.push_str("# PR evidence packet\n\n");
    text.push_str(&format!(
        "Base: `{}`\n\nHead: `{}`\n\n",
        diff.base, diff.head
    ));
    text.push_str("## Changed files\n\n");
    for file in &diff.changed_files {
        text.push_str(&format!("- `{file}`\n"));
    }
    text.push_str("\n## Diff flags\n\n");
    text.push_str(&format!(
        "```json\n{}\n```\n",
        serde_json::to_string_pretty(&diff.flags).unwrap_or_else(|_| "{}".to_owned())
    ));
    text
}

fn render_claim_prompt(diff: &DiffContext) -> String {
    let mut text = String::new();
    text.push_str("# PR claims extraction prompt\n\n");
    text.push_str("No PR body is available in no-token GH runner mode. Treat claims as absent unless supplied by a separate artifact.\n\n");
    text.push_str(
        "Reviewers should verify only claims grounded in the diff and available artifacts.\n\n",
    );
    text.push_str("Changed files:\n\n");
    for file in &diff.changed_files {
        text.push_str(&format!("- `{file}`\n"));
    }
    text
}

fn write_review_artifacts(
    root: &Path,
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    plan: &Plan,
    running_summary: &str,
    args: &RunArgs,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    let shared_context =
        render_shared_context(root, out, config, diff, plan, running_summary, args)?;
    fs::write(review_dir.join("shared_context.md"), &shared_context)?;
    let shared_context_id = sha256_hex(shared_context.as_bytes());
    let line_map = right_side_diff_lines(&diff.patch);
    let assignments = model_assignments(plan, args);
    let mut provider_preflights = build_provider_preflight_receipts(&assignments, args);
    let mut model_lanes = build_model_lane_receipts(&assignments, args);
    let missing_or_failed_sensor_evidence = collect_sensor_evidence_issues(out, plan);
    let mut missing_or_failed_model_evidence = model_lanes
        .iter()
        .filter(|receipt| is_model_evidence_issue(&receipt.status))
        .map(model_issue_from_receipt)
        .collect::<Vec<_>>();
    let mut summary_only_findings = Vec::new();
    let mut inline_comments = Vec::new();

    if matches!(args.model_mode, ModelMode::Auto) {
        run_provider_preflights(root, &review_dir, &mut provider_preflights, args)?;
        append_preflight_evidence_issues(
            &provider_preflights,
            &mut missing_or_failed_model_evidence,
        );
        let model_calls_used = run_available_model_lanes(
            ModelRunContext {
                root,
                review_dir: &review_dir,
                assignments: &assignments,
                provider_preflights: &provider_preflights,
                shared_context: &shared_context,
                args,
                line_map: &line_map,
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
        )?;
        dedupe_inline_comments(&mut inline_comments, &mut summary_only_findings);
        run_refuter_pass(
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
    }

    let body = render_review_body(
        &shared_context_id,
        plan,
        diff,
        &model_lanes,
        &missing_or_failed_sensor_evidence,
        &missing_or_failed_model_evidence,
        &inline_comments,
        &summary_only_findings,
        args.review_body_max_bytes,
    );
    let github_review = GitHubReview {
        event: "COMMENT".to_owned(),
        body: body.clone(),
        comments: inline_comments
            .iter()
            .map(|comment| GitHubReviewComment {
                path: comment.path.clone(),
                line: comment.line,
                side: comment.side.clone(),
                body: comment.body.clone(),
            })
            .collect(),
    };
    let review = ReviewArtifacts {
        shared_context_id,
        mode: args.mode.key().to_owned(),
        posting: args.posting.key().to_owned(),
        model_mode: args.model_mode.key().to_owned(),
        provider_policy: args.provider_policy.key().to_owned(),
        model_provider_policy: args.provider_policy.key().to_owned(),
        lane_width: args.lane_width,
        model_concurrency: args.model_concurrency,
        max_model_calls: args.max_model_calls,
        max_inline_comments: args.max_inline_comments,
        model_timeout_sec: args.model_timeout_sec,
        ledger_path: effective_ledger_path(config, args),
        ledger_max_bytes: args.ledger_max_bytes,
        provider_preflights,
        model_lanes,
        missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence,
        inline_comments,
        summary_only_findings,
        body: body.clone(),
    };
    let metrics = build_review_metrics(out, diff, plan, &review, &github_review);

    fs::write(
        review_dir.join("review.json"),
        serde_json::to_vec_pretty(&review)?,
    )?;
    fs::write(
        review_dir.join("metrics.json"),
        serde_json::to_vec_pretty(&metrics)?,
    )?;
    fs::write(
        review_dir.join("provider-preflight-status.json"),
        serde_json::to_vec_pretty(&review.provider_preflights)?,
    )?;
    fs::write(review_dir.join("review.md"), body)?;
    fs::write(
        review_dir.join("github-review.json"),
        serde_json::to_vec_pretty(&github_review)?,
    )?;
    Ok(())
}

fn build_review_metrics(
    out: &Path,
    diff: &DiffContext,
    plan: &Plan,
    review: &ReviewArtifacts,
    github_review: &GitHubReview,
) -> ReviewMetrics {
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

    ReviewMetrics {
        schema_version: 1,
        shared_context_id: review.shared_context_id.clone(),
        base: diff.base.clone(),
        head: diff.head.clone(),
        profile_name: plan.profile_name.clone(),
        mode: review.mode.clone(),
        posting: review.posting.clone(),
        model_mode: review.model_mode.clone(),
        provider_policy: review.provider_policy.clone(),
        lane_width: review.lane_width,
        model_concurrency: review.model_concurrency,
        max_model_calls: review.max_model_calls,
        max_inline_comments: review.max_inline_comments,
        changed_files: diff.changed_files.len(),
        diff_flags: diff.flags.clone(),
        lane_packets: plan.lanes.len(),
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
        },
        inline_comments: review.inline_comments.len(),
        github_review_comments: github_review.comments.len(),
        summary_only_findings: review.summary_only_findings.len(),
        missing_or_failed_sensor_evidence: review.missing_or_failed_sensor_evidence.len(),
        missing_or_failed_model_evidence: review.missing_or_failed_model_evidence.len(),
        review_body_bytes: review.body.len(),
        review_body_truncated: review.body.contains(REVIEW_BODY_TRUNCATED_SUFFIX.trim()),
    }
}

fn sensor_status_for_metrics(out: &Path, sensor: &SensorPlan) -> String {
    let status_path = out
        .join("sensors")
        .join(&sensor.id)
        .join("ub-review-sensor-status.json");
    read_sensor_receipt(&status_path)
        .map(|receipt| receipt.status)
        .unwrap_or_else(|| {
            if sensor.run {
                "receipt-absent".to_owned()
            } else {
                "skipped".to_owned()
            }
        })
}

fn status_counts<'a>(statuses: impl Iterator<Item = &'a str>) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for status in statuses {
        *counts.entry(status.to_owned()).or_insert(0) += 1;
    }
    counts
}

fn model_call_attempted_status(status: &str) -> bool {
    matches!(
        status,
        "ok" | "failed"
            | "invalid_json"
            | "timed_out"
            | "rate_limited"
            | "auth_failed"
            | "bad_envelope"
    )
}

fn render_shared_context(
    root: &Path,
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    plan: &Plan,
    running_summary: &str,
    args: &RunArgs,
) -> Result<String> {
    let mut text = String::new();
    text.push_str("# Shared UB Review Context\n\n");
    text.push_str("This stable prefix is intended for lane model calls and future provider-side context caching.\n\n");
    text.push_str("## PR Summary\n\n");
    text.push_str(running_summary);
    text.push_str("\n\n## Diff Summary\n\n");
    text.push_str(&format!("- Base: `{}`\n", diff.base));
    text.push_str(&format!("- Head: `{}`\n", diff.head));
    text.push_str(&format!(
        "- Changed files: `{}`\n",
        diff.changed_files.len()
    ));
    text.push_str(&format!(
        "- Unsafe/native risk touched: `{}`\n",
        diff.flags.unsafe_or_native_risk
    ));
    text.push_str("\n## Changed Files\n\n");
    for file in &diff.changed_files {
        text.push_str(&format!("- `{file}`\n"));
    }
    text.push_str("\n## Sensor Statuses\n\n");
    for sensor in &plan.sensors {
        let status_path = out
            .join("sensors")
            .join(&sensor.id)
            .join("ub-review-sensor-status.json");
        let receipt = read_sensor_receipt(&status_path);
        let status = receipt
            .as_ref()
            .map(|receipt| receipt.status.as_str())
            .unwrap_or("receipt-absent");
        let reason = receipt
            .as_ref()
            .map(|receipt| receipt.reason.as_str())
            .unwrap_or(&sensor.reason);
        text.push_str(&format!(
            "- `{}`: `{}` - {}\n",
            sensor.id,
            status,
            escape_md(reason)
        ));
    }
    text.push_str("\n## Bun UB Review Posture\n\n");
    text.push_str(NO_LGTM_POSTURE);
    text.push_str("\n\n## UB Ledger Context\n\n");
    text.push_str(&render_ledger_context(root, config, args)?);
    text.push_str("\n\n## Diff Patch\n\n```diff\n");
    text.push_str(&diff.patch);
    if !diff.patch.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("```\n");
    Ok(text)
}

fn render_ledger_context(root: &Path, config: &Config, args: &RunArgs) -> Result<String> {
    let ledger = effective_ledger_path(config, args);
    let ledger = ledger.trim();
    if ledger.is_empty() {
        return Ok("- No UB ledger configured for this run.\n".to_owned());
    }
    let configured_path = PathBuf::from(ledger);
    let path = if configured_path.is_absolute() {
        configured_path
    } else {
        root.join(configured_path)
    };
    if !path.exists() {
        return Ok(format!(
            "- UB ledger configured but unavailable at `{}`.\n",
            path.display()
        ));
    }
    if path.is_file() {
        let text = read_bounded_text(&path, args.ledger_max_bytes)?;
        return Ok(format!(
            "Source: `{}`\n\n```text\n{}\n```\n",
            path.display(),
            text
        ));
    }
    if path.is_dir() {
        let mut entries = fs::read_dir(&path)?
            .filter_map(std::result::Result::ok)
            .filter(|entry| entry.path().is_file())
            .map(|entry| entry.path())
            .filter(|path| is_ledger_excerpt_candidate(path))
            .collect::<Vec<_>>();
        entries.sort();
        entries.truncate(8);
        if entries.is_empty() {
            return Ok(format!(
                "- UB ledger directory `{}` has no supported excerpt files.\n",
                path.display()
            ));
        }
        let mut text = format!("Source directory: `{}`\n\n", path.display());
        let per_entry_limit = args.ledger_max_bytes.saturating_div(entries.len().max(1));
        let per_entry_limit = per_entry_limit.max(1024);
        let mut remaining = args.ledger_max_bytes;
        for entry in entries {
            if remaining == 0 {
                text.push_str("[ledger byte budget exhausted]\n");
                break;
            }
            let limit = per_entry_limit.min(remaining);
            text.push_str(&format!("### `{}`\n\n```text\n", entry.display()));
            let excerpt = read_bounded_text(&entry, limit)?;
            remaining = remaining.saturating_sub(excerpt.len());
            text.push_str(&excerpt);
            text.push_str("\n```\n\n");
        }
        return Ok(text);
    }
    Ok(format!(
        "- UB ledger path `{}` is not a regular file or directory.\n",
        path.display()
    ))
}

fn effective_ledger_path(config: &Config, args: &RunArgs) -> String {
    let cli_path = args.ledger_path.trim();
    if cli_path.is_empty() {
        config.repo.ledger.clone()
    } else {
        cli_path.to_owned()
    }
}

fn read_bounded_text(path: &Path, max_bytes: usize) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut buffer = vec![0; max_bytes.saturating_add(1)];
    use std::io::Read as _;
    let count = file
        .read(&mut buffer)
        .with_context(|| format!("read {}", path.display()))?;
    buffer.truncate(count.min(max_bytes));
    let mut text = String::from_utf8_lossy(&buffer).to_string();
    if count > max_bytes {
        text.push_str("\n[truncated]\n");
    }
    Ok(text)
}

fn is_ledger_excerpt_candidate(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("md" | "txt" | "toml" | "json")
    )
}

fn review_lanes_for_args(plan: &Plan, args: &RunArgs) -> Vec<LanePlan> {
    match (args.lane_width, args.provider_policy) {
        (20, ModelProviderPolicy::OpencodeGoWide) => opencode_go_wide_lanes(),
        (width, _) => review_lanes_for_width(width, plan),
    }
}

fn review_lanes_for_width(width: usize, plan: &Plan) -> Vec<LanePlan> {
    match width {
        6 => plan.lanes.clone(),
        10 => standard_minimax_lanes(),
        20 => deep_minimax_lanes(),
        _ => plan.lanes.clone(),
    }
}

fn standard_minimax_lanes() -> Vec<LanePlan> {
    vec![
        model_lane(
            "ub-memory-lifetime",
            "Memory lifetime and pointer ownership review",
            &["tokmd", "unsafe-review", "ast-grep"],
            "Review lifetime, pointer, aliasing, ownership, and safety-contract risks at changed native seams.",
        ),
        model_lane(
            "ub-active-view",
            "Resizable buffer and active-view review",
            &["tokmd", "unsafe-review", "ast-grep"],
            "Check active view region vs whole backing store, ArrayBuffer/TypedArray resize/detach/transfer, and stale length snapshots.",
        ),
        model_lane(
            "ub-worker-handoff",
            "Worker handoff and async capture review",
            &["tokmd", "unsafe-review"],
            "Check JS-backed memory crossing worker, async, GC, detach, and transfer boundaries.",
        ),
        model_lane(
            "tests-red-green",
            "Red/green changed-behavior proof review",
            &["tokmd", "ripr"],
            "Check whether tests distinguish old from new behavior and prove the PR claim.",
        ),
        model_lane(
            "tests-oracle",
            "Test oracle strength review",
            &["tokmd", "ripr"],
            "Look for smoke-only, tautological, reach-only, or non-discriminating assertions.",
        ),
        model_lane(
            "source-route",
            "Public API source-route review",
            &["tokmd", "ast-grep", "ripr"],
            "Trace public API routes, changed helper callers, sibling paths, and PR claim truth.",
        ),
        model_lane(
            "sibling-paths",
            "Sibling helper and parked follow-up review",
            &["tokmd", "ast-grep"],
            "Find related crypto/compression/runtime helper paths and identify what should be parked rather than broadened.",
        ),
        model_lane(
            "architecture",
            "Boundary and smallest-complete-fix review",
            &["tokmd", "unsafe-review"],
            "Check boundary placement, helper shape, scope control, duplication risk, and smallest complete fix.",
        ),
        model_lane(
            "security",
            "UB as exploit primitive review",
            &["tokmd", "unsafe-review"],
            "Assess OOB, UAF, type confusion, info disclosure, DoS, secret material, and exploitability framing.",
        ),
        model_lane(
            "opposition",
            "Strongest substantiated objection review",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Try to disprove the PR across correctness, proof, portability, performance, route truth, and overclaim risk.",
        ),
    ]
}

fn deep_minimax_lanes() -> Vec<LanePlan> {
    let specs = [
        (
            "ub-memory-lifetime",
            "Memory lifetime review",
            "Focus on object lifetime, borrows, ownership transfer, and stale references.",
        ),
        (
            "ub-pointer-length",
            "Pointer and length coupling review",
            "Focus on stale pointer/length pairs, integer truncation, and offset math.",
        ),
        (
            "ub-active-view",
            "Active view region review",
            "Focus on active view vs backing store boundaries after resize/detach/transfer.",
        ),
        (
            "ub-backing-store",
            "Backing store review",
            "Focus on backing-store aliases, snapshots, and mutations after capture.",
        ),
        (
            "ub-worker-handoff",
            "Worker handoff review",
            "Focus on async worker handoff and JS-backed memory crossing threads.",
        ),
        (
            "ub-gc-detach-transfer",
            "GC detach transfer review",
            "Focus on GC, detach, transfer, and protect/unprotect lifetime hazards.",
        ),
        (
            "tests-red-green",
            "Red green proof review",
            "Focus on whether old main fails and patched code passes for the claimed behavior.",
        ),
        (
            "tests-oracle-strength",
            "Oracle strength review",
            "Focus on revealability, propagation, and non-tautological assertions.",
        ),
        (
            "tests-flake-race",
            "Flake and race review",
            "Focus on timing, async, worker, and platform-dependent proof gaps.",
        ),
        (
            "tests-ci-cost",
            "CI cost review",
            "Focus on whether the proof is cheap enough and placed in the right suite.",
        ),
        (
            "source-route-public-api",
            "Public API route review",
            "Focus on public API entrypoints reaching the changed helper.",
        ),
        (
            "source-route-helper-callers",
            "Helper caller review",
            "Focus on all helper callers and variants affected by the change.",
        ),
        (
            "sibling-paths-crypto",
            "Crypto sibling path review",
            "Focus on PBKDF2, scrypt, hashInto, and key material siblings.",
        ),
        (
            "sibling-paths-compression",
            "Compression sibling path review",
            "Focus on zstd/compression helpers and package-second style siblings.",
        ),
        (
            "sibling-paths-runtime",
            "Runtime sibling path review",
            "Focus on runtime string/buffer helpers and related JS/native boundaries.",
        ),
        (
            "architecture-boundary",
            "Boundary architecture review",
            "Focus on boundary placement, helper shape, and invariants.",
        ),
        (
            "architecture-scope",
            "Scope review",
            "Focus on smallest complete fix and what must not broaden in this PR.",
        ),
        (
            "security-exploitability",
            "Exploitability review",
            "Focus on UB as exploit primitive, OOB, UAF, type confusion, and info leak.",
        ),
        (
            "security-secret-material",
            "Secret material review",
            "Focus on crypto/key/secret material exposure and lifetime risks.",
        ),
        (
            "opposition-overclaim",
            "Overclaim opposition review",
            "Focus on the strongest reason the PR is wrong, incomplete, or overclaimed.",
        ),
    ];
    specs
        .into_iter()
        .map(|(id, role, focus)| {
            model_lane(
                id,
                role,
                &["tokmd", "ripr", "unsafe-review", "ast-grep"],
                focus,
            )
        })
        .collect()
}

fn opencode_go_wide_lanes() -> Vec<LanePlan> {
    let mut lanes = standard_minimax_lanes();
    lanes.extend([
        model_lane(
            "sibling-paths-fast",
            "Fast sibling-path candidate generation",
            &["tokmd", "ast-grep"],
            "Generate candidate-only sibling path gaps and parked follow-up risks. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "source-route-fast",
            "Fast source-route candidate generation",
            &["tokmd", "ast-grep"],
            "Generate candidate-only public API route and helper caller gaps. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "test-gap-fast",
            "Fast test-gap candidate generation",
            &["tokmd", "ripr"],
            "Generate candidate-only weak oracle, red/green, and revealability gaps. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "overclaim-fast",
            "Fast overclaim candidate generation",
            &["tokmd", "ripr", "unsafe-review"],
            "Generate candidate-only overclaim and scope risks. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "security-fast",
            "Fast security candidate generation",
            &["tokmd", "unsafe-review"],
            "Generate candidate-only exploitability, info leak, DoS, and secret material risks. Mark every concern as requiring confirmation.",
        ),
        model_lane(
            "refute-finding-1",
            "Fast refutation draft",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Draft candidate-only refutations for top suspected findings. Do not request inline posting.",
        ),
        model_lane(
            "refute-finding-2",
            "Fast refutation draft",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Draft candidate-only refutations for alternative suspected findings. Do not request inline posting.",
        ),
        model_lane(
            "refute-finding-3",
            "Fast refutation draft",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Draft candidate-only refutations for weaker suspected findings. Do not request inline posting.",
        ),
        model_lane(
            "summary-pressure",
            "Fast summary pressure test",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Generate candidate-only pressure on the summary decision, residual risk, and parked follow-ups.",
        ),
        model_lane(
            "duplicate-noise-filter",
            "Fast duplicate and noise filter",
            &["tokmd", "ripr", "unsafe-review", "ast-grep"],
            "Identify likely duplicates, off-diff concerns, and low-confidence noise. Do not request inline posting.",
        ),
    ]);
    lanes
}

fn model_lane(id: &str, role: &str, receives: &[&str], focus: &str) -> LanePlan {
    lane(
        id,
        role,
        "custom:MiniMax-M3-3",
        "MiniMax-M3",
        receives,
        focus,
    )
}

fn model_assignments(plan: &Plan, args: &RunArgs) -> Vec<ModelAssignment> {
    let lanes = review_lanes_for_args(plan, args);
    lanes
        .into_iter()
        .map(|lane| {
            let spec = provider_spec_for_lane(&lane, args);
            let fallback = fallback_provider_spec_for_lane(&lane, &spec, args);
            ModelAssignment {
                lane,
                spec,
                fallback,
            }
        })
        .collect()
}

fn provider_spec_for_lane(lane: &LanePlan, args: &RunArgs) -> ProviderSpec {
    match args.provider_policy {
        ModelProviderPolicy::MinimaxOnly => direct_minimax_spec(args),
        ModelProviderPolicy::Auto | ModelProviderPolicy::MinimaxPrimary
            if lane.id == "opposition"
                && std::env::var_os("UB_REVIEW_OPENCODE_API_KEY").is_some() =>
        {
            opencode_canary_spec(args)
        }
        ModelProviderPolicy::OpencodeGoCanary if lane.id == "opposition" => {
            opencode_canary_spec(args)
        }
        ModelProviderPolicy::OpencodeGoWide if is_opencode_fast_lane(&lane.id) => {
            opencode_flash_spec(args)
        }
        ModelProviderPolicy::Auto
        | ModelProviderPolicy::MinimaxPrimary
        | ModelProviderPolicy::OpencodeGoCanary
        | ModelProviderPolicy::OpencodeGoWide => direct_minimax_spec(args),
    }
}

fn fallback_provider_spec_for_lane(
    lane: &LanePlan,
    spec: &ProviderSpec,
    args: &RunArgs,
) -> Option<ProviderSpec> {
    if spec.provider == ModelProvider::OpenCodeGo && lane.id == "opposition" {
        Some(direct_minimax_spec(args))
    } else {
        None
    }
}

fn direct_minimax_spec(args: &RunArgs) -> ProviderSpec {
    ProviderSpec {
        provider: ModelProvider::MiniMaxDirect,
        model: args.minimax_model.clone(),
        endpoint_kind: match args.minimax_provider_kind {
            ProviderKindArg::Openai => ProviderEndpointKind::OpenAiChat,
            ProviderKindArg::Anthropic => ProviderEndpointKind::AnthropicMessages,
        },
    }
}

fn opencode_canary_spec(args: &RunArgs) -> ProviderSpec {
    let model = args.opencode_model.clone();
    ProviderSpec {
        provider: ModelProvider::OpenCodeGo,
        endpoint_kind: resolve_opencode_endpoint_kind(args.opencode_endpoint_kind, &model),
        model,
    }
}

fn opencode_flash_spec(args: &RunArgs) -> ProviderSpec {
    let model = "deepseek-v4-flash".to_owned();
    ProviderSpec {
        provider: ModelProvider::OpenCodeGo,
        endpoint_kind: resolve_opencode_endpoint_kind(args.opencode_endpoint_kind, &model),
        model,
    }
}

fn resolve_opencode_endpoint_kind(
    configured: OpenCodeEndpointKindArg,
    model: &str,
) -> ProviderEndpointKind {
    match configured {
        OpenCodeEndpointKindArg::OpenaiChat => ProviderEndpointKind::OpenAiChat,
        OpenCodeEndpointKindArg::AnthropicMessages => ProviderEndpointKind::AnthropicMessages,
        OpenCodeEndpointKindArg::Auto if is_opencode_openai_chat_model(model) => {
            ProviderEndpointKind::OpenAiChat
        }
        OpenCodeEndpointKindArg::Auto => ProviderEndpointKind::AnthropicMessages,
    }
}

fn is_opencode_openai_chat_model(model: &str) -> bool {
    model.starts_with("deepseek-") || model.starts_with("kimi-") || model.starts_with("mimo-")
}

fn is_opencode_fast_lane(lane_id: &str) -> bool {
    lane_id.ends_with("-fast")
        || lane_id.starts_with("refute-finding-")
        || matches!(lane_id, "summary-pressure" | "duplicate-noise-filter")
}

fn build_model_lane_receipts(
    assignments: &[ModelAssignment],
    args: &RunArgs,
) -> Vec<ModelLaneReceipt> {
    assignments
        .iter()
        .map(|assignment| {
            let spec = &assignment.spec;
            let (status, reason) = match args.model_mode {
                ModelMode::Off => ("skipped", "model-mode off".to_owned()),
                ModelMode::Auto => {
                    let primary_env = model_api_key_env(spec.provider);
                    if std::env::var_os(primary_env).is_some() {
                        (
                            "planned",
                            format!(
                                "{primary_env} present; lane eligible for {} call",
                                spec.provider.key()
                            ),
                        )
                    } else if let Some(fallback) = &assignment.fallback {
                        let fallback_env = model_api_key_env(fallback.provider);
                        if std::env::var_os(fallback_env).is_some() {
                            (
                                "planned",
                                format!(
                                    "{primary_env} not provided; fallback {fallback_env} present"
                                ),
                            )
                        } else {
                            (
                                "missing_key",
                                format!(
                                    "{primary_env} and fallback {fallback_env} not provided; lane output unavailable"
                                ),
                            )
                        }
                    } else {
                        (
                            "missing_key",
                            format!(
                                "{primary_env} not provided; {} lane output unavailable",
                                spec.provider.key()
                            ),
                        )
                    }
                }
            };
            ModelLaneReceipt {
                lane: assignment.lane.id.clone(),
                provider: spec.provider.key().to_owned(),
                model: spec.model.clone(),
                endpoint_kind: spec.endpoint_kind.key().to_owned(),
                status: status.to_owned(),
                reason,
                duration_ms: None,
                http_status: None,
                response_shape: None,
                fallback_from: None,
            }
        })
        .collect()
}

fn build_provider_preflight_receipts(
    assignments: &[ModelAssignment],
    args: &RunArgs,
) -> Vec<ProviderPreflightReceipt> {
    let mut specs = BTreeSet::new();
    for assignment in assignments {
        specs.insert(assignment.spec.clone());
        if let Some(fallback) = &assignment.fallback {
            specs.insert(fallback.clone());
        }
    }
    specs
        .into_iter()
        .map(|spec| {
            let (status, reason) = match args.model_mode {
                ModelMode::Off => ("skipped", "model-mode off".to_owned()),
                ModelMode::Auto => {
                    let env_name = model_api_key_env(spec.provider);
                    if std::env::var_os(env_name).is_some() {
                        ("planned", format!("{env_name} present; preflight planned"))
                    } else {
                        (
                            "missing_key",
                            format!("{env_name} not provided; provider unavailable"),
                        )
                    }
                }
            };
            ProviderPreflightReceipt {
                provider: spec.provider.key().to_owned(),
                model: spec.model,
                endpoint_kind: spec.endpoint_kind.key().to_owned(),
                status: status.to_owned(),
                reason,
                duration_ms: None,
                http_status: None,
                response_shape: None,
            }
        })
        .collect()
}

fn is_model_evidence_issue(status: &str) -> bool {
    matches!(
        status,
        "missing_key"
            | "failed"
            | "invalid_json"
            | "timed_out"
            | "rate_limited"
            | "auth_failed"
            | "bad_envelope"
            | "preflight_failed"
    )
}

fn collect_sensor_evidence_issues(out: &Path, plan: &Plan) -> Vec<SensorEvidenceIssue> {
    plan.sensors
        .iter()
        .filter_map(|sensor| {
            let status_path = out
                .join("sensors")
                .join(&sensor.id)
                .join("ub-review-sensor-status.json");
            let receipt = read_sensor_receipt(&status_path);
            let status = receipt
                .as_ref()
                .map(|receipt| receipt.status.clone())
                .unwrap_or_else(|| "receipt-absent".to_owned());
            if !is_sensor_evidence_issue(&status) {
                return None;
            }
            let reason = receipt
                .map(|receipt| receipt.reason)
                .unwrap_or_else(|| sensor.reason.clone());
            Some(SensorEvidenceIssue {
                sensor: sensor.id.clone(),
                status,
                reason,
            })
        })
        .collect()
}

fn is_sensor_evidence_issue(status: &str) -> bool {
    !matches!(status, "ok")
}

fn model_issue_from_receipt(receipt: &ModelLaneReceipt) -> ModelEvidenceIssue {
    ModelEvidenceIssue {
        lane: receipt.lane.clone(),
        provider: receipt.provider.clone(),
        model: receipt.model.clone(),
        endpoint_kind: receipt.endpoint_kind.clone(),
        status: receipt.status.clone(),
        reason: receipt.reason.clone(),
    }
}

fn append_preflight_evidence_issues(
    provider_preflights: &[ProviderPreflightReceipt],
    missing_or_failed_model_evidence: &mut Vec<ModelEvidenceIssue>,
) {
    for receipt in provider_preflights {
        if is_model_evidence_issue(&receipt.status) {
            missing_or_failed_model_evidence.push(ModelEvidenceIssue {
                lane: "provider-preflight".to_owned(),
                provider: receipt.provider.clone(),
                model: receipt.model.clone(),
                endpoint_kind: receipt.endpoint_kind.clone(),
                status: receipt.status.clone(),
                reason: receipt.reason.clone(),
            });
        }
    }
}

fn run_provider_preflights(
    root: &Path,
    review_dir: &Path,
    provider_preflights: &mut [ProviderPreflightReceipt],
    args: &RunArgs,
) -> Result<()> {
    let preflight_dir = review_dir.join("provider-preflight");
    fs::create_dir_all(&preflight_dir)?;
    for receipt in provider_preflights {
        if receipt.status != "planned" {
            continue;
        }
        let spec = provider_spec_from_preflight(receipt)?;
        let lane_dir = preflight_dir.join(sanitize_artifact_name(&spec.label()));
        fs::create_dir_all(&lane_dir)?;
        let prompt = "Return strict JSON only: {\"summary\":\"preflight ok\",\"inline_comments\":[],\"summary_only_findings\":[]}";
        match call_model_prompt(root, &lane_dir, &spec, prompt, args) {
            Ok(outcome) => {
                receipt.status = "ok".to_owned();
                receipt.reason = "completed".to_owned();
                receipt.duration_ms = Some(outcome.duration_ms);
                receipt.http_status = outcome.http_status;
                receipt.response_shape = Some(outcome.response_shape);
            }
            Err(err) => {
                receipt.status = classify_model_error(&err);
                receipt.reason = format!("{err:#}");
                receipt.http_status = http_status_from_error(&err);
            }
        }
    }
    Ok(())
}

fn provider_spec_from_preflight(receipt: &ProviderPreflightReceipt) -> Result<ProviderSpec> {
    let provider = match receipt.provider.as_str() {
        "minimax" => ModelProvider::MiniMaxDirect,
        "opencode-go" => ModelProvider::OpenCodeGo,
        other => bail!("unknown provider in preflight receipt: {other}"),
    };
    let endpoint_kind = match receipt.endpoint_kind.as_str() {
        "openai-chat" => ProviderEndpointKind::OpenAiChat,
        "anthropic-messages" => ProviderEndpointKind::AnthropicMessages,
        other => bail!("unknown endpoint kind in preflight receipt: {other}"),
    };
    Ok(ProviderSpec {
        provider,
        model: receipt.model.clone(),
        endpoint_kind,
    })
}

fn sanitize_artifact_name(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn run_available_model_lanes(
    context: ModelRunContext<'_>,
    model_lanes: &mut [ModelLaneReceipt],
    missing_or_failed_model_evidence: &mut Vec<ModelEvidenceIssue>,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) -> Result<usize> {
    let model_dir = context.review_dir.join("model");
    fs::create_dir_all(&model_dir)?;
    let mut calls = 0usize;
    for (index, assignment) in context.assignments.iter().enumerate() {
        if calls >= context.args.max_model_calls
            || inline_comments.len() >= context.args.max_inline_comments
        {
            break;
        }
        let receipt = &mut model_lanes[index];
        if receipt.status != "planned" {
            continue;
        }
        let lane = &assignment.lane;
        let Some((spec, fallback_from, preflight_reason)) =
            selected_provider_spec(assignment, context.provider_preflights)
        else {
            receipt.status = "preflight_failed".to_owned();
            receipt.reason =
                "provider preflight did not succeed and no fallback was available".to_owned();
            missing_or_failed_model_evidence.push(model_issue_from_receipt(receipt));
            continue;
        };
        if let Some(reason) = preflight_reason {
            receipt.reason = reason;
        }
        if let Some(original) = fallback_from {
            receipt.fallback_from = Some(original);
        }
        receipt.provider = spec.provider.key().to_owned();
        receipt.model = spec.model.clone();
        receipt.endpoint_kind = spec.endpoint_kind.key().to_owned();
        let env_name = model_api_key_env(spec.provider);
        if std::env::var_os(env_name).is_none() {
            receipt.status = "missing_key".to_owned();
            receipt.reason = format!(
                "{env_name} not provided; {} lane output unavailable",
                spec.provider.key()
            );
            missing_or_failed_model_evidence.push(model_issue_from_receipt(receipt));
            continue;
        }
        calls += 1;
        let lane_dir = model_dir.join(&lane.id);
        fs::create_dir_all(&lane_dir)?;
        receipt.status = "running".to_owned();
        match call_model_lane(
            context.root,
            &lane_dir,
            lane,
            &spec,
            context.shared_context,
            context.args,
        ) {
            Ok(outcome) => {
                receipt.status = "ok".to_owned();
                receipt.reason = "completed".to_owned();
                receipt.duration_ms = Some(outcome.duration_ms);
                receipt.http_status = outcome.http_status;
                receipt.response_shape = Some(outcome.response_shape.clone());
                apply_model_output(
                    lane,
                    outcome.output,
                    context.line_map,
                    context.args.max_inline_comments,
                    inline_comments,
                    summary_only_findings,
                );
            }
            Err(err) => {
                receipt.status = classify_model_error(&err);
                receipt.reason = format!("{err:#}");
                receipt.http_status = http_status_from_error(&err);
                missing_or_failed_model_evidence.push(model_issue_from_receipt(receipt));
            }
        }
    }
    for receipt in model_lanes {
        if receipt.status == "planned" {
            receipt.status = "skipped".to_owned();
            receipt.reason =
                "model call budget or inline comment cap reached before lane execution".to_owned();
        }
    }
    Ok(calls)
}

fn run_refuter_pass(
    context: RefuterRunContext<'_>,
    model_lanes: &mut Vec<ModelLaneReceipt>,
    missing_or_failed_model_evidence: &mut Vec<ModelEvidenceIssue>,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) -> Result<()> {
    let spec = direct_minimax_spec(context.args);
    let mut receipt = ModelLaneReceipt {
        lane: "refuter".to_owned(),
        provider: spec.provider.key().to_owned(),
        model: spec.model.clone(),
        endpoint_kind: spec.endpoint_kind.key().to_owned(),
        status: "planned".to_owned(),
        reason: "planned M3 refuter pass for validated inline candidates".to_owned(),
        duration_ms: None,
        http_status: None,
        response_shape: None,
        fallback_from: None,
    };

    if inline_comments.is_empty() {
        receipt.status = "skipped".to_owned();
        receipt.reason = "no inline candidates passed guardrails before refuter".to_owned();
        model_lanes.push(receipt);
        return Ok(());
    }
    if context.model_calls_used >= context.args.max_model_calls {
        receipt.status = "skipped".to_owned();
        receipt.reason = "model call budget exhausted before refuter pass".to_owned();
        model_lanes.push(receipt);
        return Ok(());
    }
    if !provider_preflight_ok(&spec, context.provider_preflights) {
        receipt.status = "preflight_failed".to_owned();
        receipt.reason = provider_preflight_reason(&spec, context.provider_preflights)
            .unwrap_or_else(|| "MiniMax preflight did not succeed".to_owned());
        missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        model_lanes.push(receipt);
        return Ok(());
    }
    let env_name = model_api_key_env(spec.provider);
    if std::env::var_os(env_name).is_none() {
        receipt.status = "missing_key".to_owned();
        receipt.reason = format!("{env_name} not provided; refuter output unavailable");
        missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        model_lanes.push(receipt);
        return Ok(());
    }

    let refuter_dir = context.review_dir.join("model").join("refuter");
    fs::create_dir_all(&refuter_dir)?;
    receipt.status = "running".to_owned();
    match call_model_refuter(
        context.root,
        &refuter_dir,
        &spec,
        context.shared_context,
        inline_comments,
        context.args,
    ) {
        Ok(outcome) => {
            receipt.status = "ok".to_owned();
            receipt.reason = "completed".to_owned();
            receipt.duration_ms = Some(outcome.duration_ms);
            receipt.http_status = outcome.http_status;
            receipt.response_shape = Some(outcome.response_shape);
            apply_refuter_output(outcome.output, inline_comments, summary_only_findings);
        }
        Err(err) => {
            receipt.status = classify_model_error(&err);
            receipt.reason = format!("{err:#}");
            receipt.http_status = http_status_from_error(&err);
            missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        }
    }
    model_lanes.push(receipt);
    Ok(())
}

fn call_model_refuter(
    root: &Path,
    lane_dir: &Path,
    spec: &ProviderSpec,
    shared_context: &str,
    inline_comments: &[ReviewInlineComment],
    args: &RunArgs,
) -> Result<ModelCallOutcome<RefuterOutput>> {
    let prompt = render_refuter_prompt(shared_context, inline_comments)?;
    call_model_prompt_typed(root, lane_dir, spec, &prompt, args)
}

fn render_refuter_prompt(
    shared_context: &str,
    inline_comments: &[ReviewInlineComment],
) -> Result<String> {
    let candidates = serde_json::to_string_pretty(inline_comments)?;
    Ok(format!(
        r#"You are the final refuter for a Bun UB PR review.

Use only the shared context and candidate inline comments below.
Do not browse. Do not infer safety from missing evidence.
Return strict JSON only:
{{
  "decisions": [
    {{
      "path": "repo-relative/path.rs",
      "line": 123,
      "disposition": "inline|summary|drop",
      "confidence": "high|medium-high|medium|low",
      "reason": "why this candidate should remain inline, move to summary, or be dropped"
    }}
  ]
}}

Rules:
- `inline` only when the candidate is grounded, actionable, and not contradicted.
- `summary` for plausible but uncertain, broad, off-proof, or needs-human-verification concerns.
- `drop` only for high-confidence false positives or duplicates.
- If uncertain, use `summary`.
- Do not approve the PR and do not output LGTM language.

Candidate inline comments:
{candidates}

Shared context:
{shared_context}"#
    ))
}

fn apply_refuter_output(
    output: RefuterOutput,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) {
    let mut decisions = BTreeMap::new();
    for decision in output.decisions {
        decisions.insert(
            (normalize_repo_path(&decision.path), decision.line),
            decision,
        );
    }

    let mut kept = Vec::new();
    for comment in std::mem::take(inline_comments) {
        let key = (comment.path.clone(), comment.line);
        let Some(decision) = decisions.remove(&key) else {
            summary_only_findings.push(summary_from_refuted_inline(
                comment,
                "refuter returned no decision for this candidate; kept as summary-only",
            ));
            continue;
        };
        let confidence = decision
            .confidence
            .as_deref()
            .unwrap_or("medium")
            .trim()
            .to_ascii_lowercase();
        let confident = matches!(confidence.as_str(), "high" | "medium-high");
        let disposition = decision.disposition.trim().to_ascii_lowercase();
        match disposition.as_str() {
            "inline" if confident => kept.push(comment),
            "drop" if confident => {}
            "summary" | "summary-only" => {
                summary_only_findings.push(summary_from_refuted_inline(comment, &decision.reason));
            }
            "drop" | "inline" => {
                let reason = format!(
                    "refuter returned `{}` with `{}` confidence; kept as summary-only: {}",
                    disposition, confidence, decision.reason
                );
                summary_only_findings.push(summary_from_refuted_inline(comment, &reason));
            }
            _ => {
                let reason = format!(
                    "refuter returned unknown disposition `{}`; kept as summary-only: {}",
                    decision.disposition, decision.reason
                );
                summary_only_findings.push(summary_from_refuted_inline(comment, &reason));
            }
        }
    }
    inline_comments.extend(kept);
}

fn summary_from_refuted_inline(comment: ReviewInlineComment, reason: &str) -> SummaryOnlyFinding {
    SummaryOnlyFinding {
        lane: comment.lane,
        severity: comment.severity,
        confidence: comment.confidence,
        reason: format!(
            "refuter demoted inline candidate at {}:{}: {}",
            comment.path, comment.line, reason
        ),
        evidence: comment.evidence,
    }
}

fn selected_provider_spec(
    assignment: &ModelAssignment,
    preflights: &[ProviderPreflightReceipt],
) -> Option<(ProviderSpec, Option<String>, Option<String>)> {
    if provider_preflight_ok(&assignment.spec, preflights) {
        return Some((assignment.spec.clone(), None, None));
    }
    let primary_status = provider_preflight_reason(&assignment.spec, preflights);
    let fallback = assignment.fallback.as_ref()?;
    if provider_preflight_ok(fallback, preflights) {
        return Some((
            fallback.clone(),
            Some(assignment.spec.label()),
            primary_status
                .map(|reason| format!("primary provider unavailable; fallback used: {reason}")),
        ));
    }
    None
}

fn provider_preflight_ok(spec: &ProviderSpec, preflights: &[ProviderPreflightReceipt]) -> bool {
    preflights
        .iter()
        .any(|receipt| preflight_matches_spec(receipt, spec) && receipt.status == "ok")
}

fn provider_preflight_reason(
    spec: &ProviderSpec,
    preflights: &[ProviderPreflightReceipt],
) -> Option<String> {
    preflights
        .iter()
        .find(|receipt| preflight_matches_spec(receipt, spec))
        .map(|receipt| {
            format!(
                "{} `{}` - {}",
                receipt.provider, receipt.status, receipt.reason
            )
        })
}

fn preflight_matches_spec(receipt: &ProviderPreflightReceipt, spec: &ProviderSpec) -> bool {
    receipt.provider == spec.provider.key()
        && receipt.model == spec.model
        && receipt.endpoint_kind == spec.endpoint_kind.key()
}

fn call_model_lane(
    root: &Path,
    lane_dir: &Path,
    lane: &LanePlan,
    spec: &ProviderSpec,
    shared_context: &str,
    args: &RunArgs,
) -> Result<ModelCallOutcome<LaneModelOutput>> {
    let prompt = render_lane_model_prompt(lane, spec, shared_context);
    call_model_prompt(root, lane_dir, spec, &prompt, args)
}

fn call_model_prompt(
    root: &Path,
    lane_dir: &Path,
    spec: &ProviderSpec,
    prompt: &str,
    args: &RunArgs,
) -> Result<ModelCallOutcome<LaneModelOutput>> {
    call_model_prompt_typed(root, lane_dir, spec, prompt, args)
}

fn call_model_prompt_typed<T>(
    root: &Path,
    lane_dir: &Path,
    spec: &ProviderSpec,
    prompt: &str,
    args: &RunArgs,
) -> Result<ModelCallOutcome<T>>
where
    T: DeserializeOwned,
{
    let env_name = model_api_key_env(spec.provider);
    let token = std::env::var(env_name).with_context(|| format!("{env_name} missing"))?;
    let url = model_api_url(spec);
    let auth_header = model_auth_header(spec, &token);
    let payload = model_request_payload(spec, prompt);
    let request_path = lane_dir.join("request.json");
    let response_path = lane_dir.join("response.json");
    let stderr_path = lane_dir.join("stderr.txt");
    fs::write(&request_path, serde_json::to_vec_pretty(&payload)?)?;
    let started = Instant::now();
    let process_output = run_curl_json_post(
        root,
        &url,
        &auth_header,
        &request_path,
        &["Accept: application/json", "Content-Type: application/json"],
        args.model_timeout_sec,
    )
    .with_context(|| "run model curl")?;
    let duration_ms = started.elapsed().as_millis();
    fs::write(&response_path, &process_output.stdout)?;
    fs::write(&stderr_path, &process_output.stderr)?;
    if !process_output.status.success() {
        let response_text = String::from_utf8_lossy(&process_output.stdout);
        bail!(
            "model curl exited {:?} with http status {:?}: stderr: {}; stdout: {}",
            process_output.status.code(),
            process_output.http_status,
            String::from_utf8_lossy(&process_output.stderr),
            response_text
        );
    }
    let response: serde_json::Value = serde_json::from_slice(&process_output.stdout)
        .with_context(|| format!("parse {}", response_path.display()))?;
    let response_shape = model_response_shape(&response).to_owned();
    let content = extract_model_content(&response)
        .ok_or_else(|| anyhow::anyhow!("model response did not contain assistant content"))?;
    let content_path = lane_dir.join("content.json");
    fs::write(&content_path, content.as_bytes())?;
    let json_payload = model_json_payload(content);
    let parse_path = if json_payload == content {
        content_path
    } else {
        let normalized_path = lane_dir.join("content-normalized.json");
        fs::write(&normalized_path, json_payload.as_bytes())?;
        normalized_path
    };
    let parsed_output = serde_json::from_str(&json_payload)
        .with_context(|| format!("parse {}", parse_path.display()))?;
    Ok(ModelCallOutcome {
        output: parsed_output,
        duration_ms,
        http_status: process_output.http_status,
        response_shape,
    })
}

fn render_lane_model_prompt(lane: &LanePlan, spec: &ProviderSpec, shared_context: &str) -> String {
    format!(
        r#"Lane: {lane}
Provider: {provider}
Model: {model}
Endpoint kind: {endpoint_kind}
Role: {role}
Focus: {focus}

Use the shared context below. Return only one strict JSON object:
{{
  "summary": "short lane summary, 300 chars max",
  "inline_comments": [
    {{
      "severity": "blocker|high|medium",
      "confidence": "high|medium-high",
      "path": "repo-relative/path.rs",
      "line": 123,
      "body": "[{lane}] concise actionable finding, 400 chars max",
      "evidence": "artifact, diff, or invariant, 240 chars max"
    }}
  ],
  "summary_only_findings": [
    {{
      "severity": "blocker|high|medium|low",
      "confidence": "high|medium-high|medium",
      "reason": "summary-only issue, 400 chars max",
      "evidence": "artifact, diff, or invariant, 240 chars max"
    }}
  ]
}}

Hard caps: at most 2 inline_comments and at most 1 summary_only_findings item.
If there is no blocker/high/medium actionable issue, use empty arrays and put the failed-objection audit in summary.
Only propose inline comments for valid RIGHT-side changed or context lines in the PR diff.
Do not guess line numbers. Do not use deletion-side comments. Do not output a standalone approval.

{shared_context}"#,
        lane = lane.id,
        provider = spec.provider.key(),
        model = spec.model,
        endpoint_kind = spec.endpoint_kind.key(),
        role = lane.role,
        focus = lane.focus,
        shared_context = shared_context
    )
}

fn apply_model_output(
    lane: &LanePlan,
    output: LaneModelOutput,
    line_map: &BTreeSet<(String, u32)>,
    max_inline: usize,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) {
    if let Some(summary) = output.summary {
        summary_only_findings.push(SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason: summary,
            evidence: "lane model summary".to_owned(),
        });
    }
    for candidate in output.summary_only_findings {
        summary_only_findings.push(SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: candidate.severity,
            confidence: candidate.confidence,
            reason: candidate.reason,
            evidence: candidate.evidence,
        });
    }
    for candidate in output.inline_comments {
        if inline_comments.len() >= max_inline {
            summary_only_findings.push(SummaryOnlyFinding {
                lane: lane.id.clone(),
                severity: candidate.severity,
                confidence: candidate.confidence,
                reason: format!(
                    "inline budget exhausted for {}:{}; kept as summary-only",
                    candidate.path, candidate.line
                ),
                evidence: candidate.evidence,
            });
            continue;
        }
        match validate_inline_candidate(lane, candidate, line_map) {
            Ok(comment) => inline_comments.push(comment),
            Err(finding) => summary_only_findings.push(finding),
        }
    }
}

fn dedupe_inline_comments(
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) {
    let mut deduped = BTreeMap::new();
    for comment in std::mem::take(inline_comments) {
        let key = (comment.path.clone(), comment.line);
        if let Some(existing) = deduped.get_mut(&key) {
            let dropped = if inline_comment_rank(&comment) > inline_comment_rank(existing) {
                std::mem::replace(existing, comment)
            } else {
                comment
            };
            merge_duplicate_inline_evidence(existing, &dropped);
            summary_only_findings.push(SummaryOnlyFinding {
                lane: dropped.lane,
                severity: dropped.severity,
                confidence: dropped.confidence,
                reason: format!(
                    "duplicate inline candidate merged into {}:{}",
                    dropped.path, dropped.line
                ),
                evidence: dropped.evidence,
            });
        } else {
            deduped.insert(key, comment);
        }
    }
    inline_comments.extend(deduped.into_values());
}

fn inline_comment_rank(comment: &ReviewInlineComment) -> (u8, u8) {
    (
        severity_rank(&comment.severity),
        confidence_rank(&comment.confidence),
    )
}

fn severity_rank(value: &str) -> u8 {
    match value {
        "blocker" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

fn confidence_rank(value: &str) -> u8 {
    match value {
        "high" => 3,
        "medium-high" => 2,
        "medium" => 1,
        "low" => 0,
        _ => 0,
    }
}

fn merge_duplicate_inline_evidence(kept: &mut ReviewInlineComment, dropped: &ReviewInlineComment) {
    if dropped.evidence.is_empty() || kept.evidence.contains(&dropped.evidence) {
        return;
    }
    let merged = format!(
        "{} Additional duplicate evidence from lane `{}`: {}",
        kept.evidence, dropped.lane, dropped.evidence
    );
    kept.evidence = truncate_chars(&merged, 2_000);
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 3 {
        return value.chars().take(max_chars).collect();
    }
    let mut truncated = value.chars().take(max_chars - 3).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn validate_inline_candidate(
    lane: &LanePlan,
    candidate: ModelCandidateComment,
    line_map: &BTreeSet<(String, u32)>,
) -> std::result::Result<ReviewInlineComment, SummaryOnlyFinding> {
    let path = normalize_repo_path(&candidate.path);
    let allowed_severity = matches!(candidate.severity.as_str(), "blocker" | "high" | "medium");
    let allowed_confidence = matches!(candidate.confidence.as_str(), "high" | "medium-high");
    let line_valid = line_map.contains(&(path.clone(), candidate.line));
    let body = ensure_lane_prefix(&lane.id, candidate.body.trim());
    let concise = body.chars().count() <= 1_200;
    let repo_relative = !path.is_empty()
        && !Path::new(&path).is_absolute()
        && !path.split('/').any(|part| part == "..");

    if allowed_severity && allowed_confidence && line_valid && concise && repo_relative {
        Ok(ReviewInlineComment {
            lane: lane.id.clone(),
            severity: candidate.severity,
            confidence: candidate.confidence,
            path,
            line: candidate.line,
            side: "RIGHT".to_owned(),
            body,
            evidence: candidate.evidence,
        })
    } else {
        Err(SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: candidate.severity,
            confidence: candidate.confidence,
            reason: format!(
                "inline guard rejected {}:{}; severity_allowed={} confidence_allowed={} line_valid={} concise={} repo_relative={}",
                path,
                candidate.line,
                allowed_severity,
                allowed_confidence,
                line_valid,
                concise,
                repo_relative
            ),
            evidence: candidate.evidence,
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn render_review_body(
    shared_context_id: &str,
    plan: &Plan,
    diff: &DiffContext,
    model_lanes: &[ModelLaneReceipt],
    missing_or_failed_sensor_evidence: &[SensorEvidenceIssue],
    missing_or_failed_model_evidence: &[ModelEvidenceIssue],
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    review_body_max_bytes: usize,
) -> String {
    let mut text = String::new();
    text.push_str("# UB Review\n\n");
    text.push_str(&format!("- Shared context: `{shared_context_id}`\n"));
    text.push_str(&format!("- Profile: `{}`\n", plan.profile_name));
    text.push_str(&format!("- Base: `{}`\n", plan.base));
    text.push_str(&format!("- Head: `{}`\n", plan.head));
    text.push_str(&format!(
        "- Changed files: `{}`\n",
        diff.changed_files.len()
    ));
    text.push_str(&format!("- Inline comments: `{}`\n", inline_comments.len()));
    text.push_str("\n## Decision\n\n");
    text.push_str(&format!(
        "- {}\n",
        review_decision(
            missing_or_failed_sensor_evidence,
            missing_or_failed_model_evidence,
            inline_comments,
            summary_only_findings
        )
    ));

    if !has_actionable_review_finding(inline_comments, summary_only_findings) {
        text.push_str("\n## No blocking finding after checking\n\n");
        text.push_str("- Shared diff packet, changed-file list, diff flags, sensor receipts, model lane receipts, and Bun lane packet prompts.\n");
        text.push_str(
            "- Inline-comment guardrails found no validated candidate comments to post.\n",
        );
    }

    text.push_str("\n## Confirmed findings\n\n");
    if inline_comments.is_empty() {
        text.push_str("- None validated for inline posting.\n");
    } else {
        for comment in inline_comments {
            text.push_str(&format!(
                "- `[{}]` `{}` `{}` at `{}`:{}: {} Evidence: {}\n",
                comment.lane,
                comment.severity,
                comment.confidence,
                comment.path,
                comment.line,
                escape_md(&comment.body),
                escape_md(&comment.evidence)
            ));
        }
    }

    text.push_str("\n## Summary-only findings\n\n");
    if summary_only_findings.is_empty() {
        text.push_str("- None.\n");
    } else {
        for finding in summary_only_findings {
            text.push_str(&format!(
                "- `[{}]` `{}` `{}`: {} Evidence: {}\n",
                finding.lane,
                finding.severity,
                finding.confidence,
                escape_md(&finding.reason),
                escape_md(&finding.evidence)
            ));
        }
    }

    text.push_str("\n## Failed objections\n\n");
    if has_actionable_review_finding(inline_comments, summary_only_findings) {
        text.push_str("- Refuter and diff-line guardrails kept uncertain, duplicate, low-confidence, and off-diff objections out of inline comments.\n");
    } else {
        text.push_str("- Strongest failed objection: a model or sensor may have found a real issue, but no blocker/high/medium candidate survived the bounded lane run, diff-line validation, and refuter path.\n");
    }
    text.push_str("- Missing evidence is not a failed objection; it is listed separately below.\n");

    text.push_str("\n## Residual risk\n\n");
    text.push_str("- A human should inspect unsafe/native seams, test-oracle strength, and any unavailable sensor/model evidence before relying on this review.\n");

    text.push_str("\n## Parked follow-ups\n\n");
    let parked = summary_only_findings
        .iter()
        .filter(|finding| is_parked_follow_up(finding))
        .collect::<Vec<_>>();
    if parked.is_empty() {
        text.push_str(
            "- No parked follow-up was promoted from ledger or lane evidence in this run.\n",
        );
    } else {
        for finding in parked {
            text.push_str(&format!(
                "- `[{}]` {} Evidence: {}\n",
                finding.lane,
                escape_md(&finding.reason),
                escape_md(&finding.evidence)
            ));
        }
    }

    text.push_str("\n## Missing or failed evidence\n\n");
    if missing_or_failed_sensor_evidence.is_empty() && missing_or_failed_model_evidence.is_empty() {
        text.push_str("- None recorded.\n");
    } else {
        for issue in missing_or_failed_sensor_evidence {
            text.push_str(&format!(
                "- Sensor `{}`: `{}` - {}\n",
                issue.sensor,
                issue.status,
                escape_md(&issue.reason)
            ));
        }
        for issue in missing_or_failed_model_evidence {
            text.push_str(&format!(
                "- Lane `{}` via `{}` model `{}` endpoint `{}`: `{}` - {}\n",
                issue.lane,
                issue.provider,
                issue.model,
                issue.endpoint_kind,
                issue.status,
                escape_md(&issue.reason)
            ));
        }
    }

    text.push_str("\n## Model lanes\n\n");
    for lane in model_lanes {
        text.push_str(&format!(
            "- Lane: `{}`\n  Provider: `{}`\n  Model: `{}`\n  Status: `{}` - {}\n",
            lane.lane,
            lane.provider,
            lane.model,
            lane.status,
            escape_md(&lane.reason)
        ));
    }
    cap_review_body(text, review_body_max_bytes)
}

fn review_decision(
    missing_or_failed_sensor_evidence: &[SensorEvidenceIssue],
    missing_or_failed_model_evidence: &[ModelEvidenceIssue],
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
) -> &'static str {
    if has_actionable_review_finding(inline_comments, summary_only_findings) {
        "Needs reviewer attention before upstream: grounded findings or summary-only concerns remain."
    } else if !missing_or_failed_sensor_evidence.is_empty()
        || !missing_or_failed_model_evidence.is_empty()
    {
        "No blocking finding after bounded review; evidence is incomplete."
    } else {
        "No blocking finding after bounded review; residual risk remains for human review."
    }
}

fn has_actionable_review_finding(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
) -> bool {
    inline_comments
        .iter()
        .any(|comment| matches!(comment.severity.as_str(), "blocker" | "high" | "medium"))
        || summary_only_findings
            .iter()
            .any(|finding| matches!(finding.severity.as_str(), "blocker" | "high" | "medium"))
}

fn is_parked_follow_up(finding: &SummaryOnlyFinding) -> bool {
    let reason = finding.reason.to_ascii_lowercase();
    let evidence = finding.evidence.to_ascii_lowercase();
    reason.contains("parked")
        || reason.contains("follow-up")
        || evidence.contains("parked")
        || evidence.contains("follow-up")
}

const REVIEW_BODY_TRUNCATED_SUFFIX: &str = "\n\n[review body truncated; see review artifacts]\n";
const REVIEW_BODY_REQUIRED_HEADINGS: [&str; 7] = [
    "## Decision",
    "## Confirmed findings",
    "## Summary-only findings",
    "## Failed objections",
    "## Residual risk",
    "## Parked follow-ups",
    "## Missing or failed evidence",
];

fn cap_review_body(text: String, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text;
    }
    if REVIEW_BODY_REQUIRED_HEADINGS
        .iter()
        .all(|heading| text.contains(heading))
        && let Some(compact) = compact_review_body_sections(&text, max_bytes)
    {
        return compact;
    }
    cap_text_prefix(text, max_bytes)
}

fn compact_review_body_sections(text: &str, max_bytes: usize) -> Option<String> {
    for section_budget in [180, 120, 80, 48, 0] {
        let mut compact = String::new();
        if let Some(first_heading) = first_required_heading_index(text) {
            let prefix = text[..first_heading].trim_end();
            append_review_excerpt(&mut compact, prefix, 220);
            compact.push('\n');
        }
        for (index, heading) in REVIEW_BODY_REQUIRED_HEADINGS.iter().enumerate() {
            compact.push('\n');
            compact.push_str(heading);
            compact.push_str("\n\n");
            let next_heading = REVIEW_BODY_REQUIRED_HEADINGS.get(index + 1).copied();
            let section = review_body_section(text, heading, next_heading)?;
            append_review_excerpt(&mut compact, section, section_budget);
        }
        compact.push_str(REVIEW_BODY_TRUNCATED_SUFFIX);
        if compact.len() <= max_bytes {
            return Some(compact);
        }
    }
    None
}

fn first_required_heading_index(text: &str) -> Option<usize> {
    REVIEW_BODY_REQUIRED_HEADINGS
        .iter()
        .filter_map(|heading| text.find(heading))
        .min()
}

fn review_body_section<'a>(
    text: &'a str,
    heading: &str,
    next_heading: Option<&str>,
) -> Option<&'a str> {
    let start = text.find(heading)? + heading.len();
    let rest = &text[start..];
    let end = next_heading.and_then(|heading| rest.find(heading));
    Some(end.map_or(rest, |end| &rest[..end]))
}

fn append_review_excerpt(out: &mut String, text: &str, max_bytes: usize) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        out.push_str("- See review artifacts for full section.\n");
        return;
    }
    let line = trimmed
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("- See review artifacts for full section.");
    if max_bytes == 0 {
        out.push_str("- See review artifacts for full section.\n");
        return;
    }
    let mut excerpt = utf8_prefix(line, max_bytes);
    if excerpt.is_empty() {
        excerpt = "- See review artifacts for full section.".to_owned();
    }
    out.push_str(&excerpt);
    if line.len() > excerpt.len() {
        out.push_str(" ...");
    }
    out.push('\n');
}

fn utf8_prefix(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut boundary = max_bytes.min(text.len());
    while !text.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    text[..boundary].to_owned()
}

fn cap_text_prefix(mut text: String, max_bytes: usize) -> String {
    let keep = max_bytes
        .saturating_sub(REVIEW_BODY_TRUNCATED_SUFFIX.len())
        .max(1);
    let mut boundary = keep.min(text.len());
    while !text.is_char_boundary(boundary) {
        boundary = boundary.saturating_sub(1);
    }
    text.truncate(boundary);
    text.push_str(REVIEW_BODY_TRUNCATED_SUFFIX);
    text
}

fn right_side_diff_lines(patch: &str) -> BTreeSet<(String, u32)> {
    let mut lines = BTreeSet::new();
    let mut current_path = String::new();
    let mut new_line: Option<u32> = None;
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            current_path = normalize_repo_path(path);
            continue;
        }
        if line.starts_with("@@") {
            new_line = parse_hunk_new_start(line);
            continue;
        }
        let Some(line_no) = new_line else {
            continue;
        };
        if current_path.is_empty() {
            continue;
        }
        let is_right_line =
            (line.starts_with('+') && !line.starts_with("+++")) || line.starts_with(' ');
        if is_right_line {
            lines.insert((current_path.clone(), line_no));
            new_line = line_no.checked_add(1);
        } else if line.starts_with('-') && !line.starts_with("---") {
            new_line = Some(line_no);
        } else if !line.starts_with('\\') {
            new_line = line_no.checked_add(1);
        }
    }
    lines
}

fn parse_hunk_new_start(line: &str) -> Option<u32> {
    let plus = line.split_whitespace().find(|part| part.starts_with('+'))?;
    let start = plus
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse::<u32>()
        .ok()?;
    Some(start)
}

fn normalize_repo_path(path: &str) -> String {
    path.trim().trim_start_matches("b/").replace('\\', "/")
}

fn ensure_lane_prefix(lane: &str, body: &str) -> String {
    let prefix = format!("[{lane}]");
    if body.starts_with(&prefix) {
        body.to_owned()
    } else {
        format!("{prefix} {body}")
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn model_api_key_env(provider: ModelProvider) -> &'static str {
    match provider {
        ModelProvider::MiniMaxDirect => "UB_REVIEW_MINIMAX_API_KEY",
        ModelProvider::OpenCodeGo => "UB_REVIEW_OPENCODE_API_KEY",
    }
}

fn model_api_url(spec: &ProviderSpec) -> String {
    match spec.provider {
        ModelProvider::MiniMaxDirect => {
            if let Ok(value) = std::env::var("UB_REVIEW_MINIMAX_API_URL")
                && !value.trim().is_empty()
            {
                return value;
            }
            match spec.endpoint_kind {
                ProviderEndpointKind::AnthropicMessages => {
                    "https://api.minimax.io/anthropic/v1/messages".to_owned()
                }
                ProviderEndpointKind::OpenAiChat => {
                    "https://api.minimax.io/v1/chat/completions".to_owned()
                }
            }
        }
        ModelProvider::OpenCodeGo => {
            if let Ok(value) = std::env::var("UB_REVIEW_OPENCODE_API_URL")
                && !value.trim().is_empty()
            {
                return value;
            }
            match spec.endpoint_kind {
                ProviderEndpointKind::AnthropicMessages => {
                    "https://opencode.ai/zen/go/v1/messages".to_owned()
                }
                ProviderEndpointKind::OpenAiChat => {
                    "https://opencode.ai/zen/go/v1/chat/completions".to_owned()
                }
            }
        }
    }
}

fn model_auth_header(spec: &ProviderSpec, token: &str) -> String {
    match spec.provider {
        ModelProvider::MiniMaxDirect
            if spec.endpoint_kind == ProviderEndpointKind::AnthropicMessages =>
        {
            format!("X-Api-Key: {token}")
        }
        ModelProvider::MiniMaxDirect => format!("Authorization: Bearer {token}"),
        ModelProvider::OpenCodeGo => format!("Authorization: Bearer {token}"),
    }
}

fn model_request_payload(spec: &ProviderSpec, prompt: &str) -> serde_json::Value {
    match spec.endpoint_kind {
        ProviderEndpointKind::AnthropicMessages => serde_json::json!({
            "model": spec.model,
            "max_tokens": model_max_tokens(spec),
            "system": "Return one compact JSON object in the final text block. Do not include markdown fences or prose outside JSON.",
            "thinking": {"type": "adaptive"},
            "temperature": 0.1,
            "messages": [
                {"role": "user", "content": prompt}
            ],
        }),
        ProviderEndpointKind::OpenAiChat if spec.provider == ModelProvider::MiniMaxDirect => {
            serde_json::json!({
                "model": spec.model,
                "messages": [
                    {"role": "system", "content": "Return strict JSON only. Do not include markdown fences or prose outside JSON."},
                    {"role": "user", "content": prompt}
                ],
                "max_completion_tokens": 4096,
                "reasoning_split": true,
                "response_format": {"type": "json_object"},
                "temperature": 0.1,
                "stream": false
            })
        }
        ProviderEndpointKind::OpenAiChat => serde_json::json!({
            "model": spec.model,
            "messages": [
                {"role": "system", "content": "Return strict JSON only. Do not include markdown fences."},
                {"role": "user", "content": prompt}
            ],
            "temperature": 0.1,
            "stream": false
        }),
    }
}

fn model_max_tokens(spec: &ProviderSpec) -> u32 {
    match spec.endpoint_kind {
        ProviderEndpointKind::AnthropicMessages => 4096,
        ProviderEndpointKind::OpenAiChat => 4096,
    }
}

fn extract_model_content(response: &serde_json::Value) -> Option<&str> {
    response
        .pointer("/choices/0/message/content")
        .and_then(serde_json::Value::as_str)
        .or_else(|| anthropic_content_text(response))
        .or_else(|| response.get("text").and_then(serde_json::Value::as_str))
        .or_else(|| response.get("reply").and_then(serde_json::Value::as_str))
        .or_else(|| response.get("content").and_then(serde_json::Value::as_str))
}

fn anthropic_content_text(response: &serde_json::Value) -> Option<&str> {
    response
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|item| item.get("text").and_then(serde_json::Value::as_str))
}

fn model_response_shape(response: &serde_json::Value) -> &'static str {
    if response.pointer("/choices/0/message/content").is_some() {
        "openai"
    } else if anthropic_content_text(response).is_some() {
        "anthropic"
    } else if response.get("reply").is_some() || response.get("content").is_some() {
        "provider-flat"
    } else {
        "unknown"
    }
}

fn model_json_payload(content: &str) -> String {
    let trimmed = content.trim();
    strip_markdown_json_fence(trimmed)
        .map(str::trim)
        .unwrap_or(content)
        .to_owned()
}

fn strip_markdown_json_fence(trimmed: &str) -> Option<&str> {
    let body = trimmed.strip_prefix("```")?;
    let newline = body.find('\n')?;
    let (info, rest_with_newline) = body.split_at(newline);
    let info = info.trim();
    if !info.is_empty() && !info.eq_ignore_ascii_case("json") {
        return None;
    }
    let rest = rest_with_newline.strip_prefix('\n')?.trim_end();
    rest.strip_suffix("```")
}

fn classify_model_error(err: &anyhow::Error) -> String {
    let text = err.to_string().to_ascii_lowercase();
    if text.contains("401") || text.contains("403") || text.contains("auth") {
        "auth_failed".to_owned()
    } else if text.contains("429") || text.contains("rate") {
        "rate_limited".to_owned()
    } else if text.contains("timed out")
        || text.contains("timeout")
        || text.contains("operation timed out")
    {
        "timed_out".to_owned()
    } else if text.contains("parse") {
        "invalid_json".to_owned()
    } else if text.contains("assistant content") {
        "bad_envelope".to_owned()
    } else {
        "failed".to_owned()
    }
}

fn run_curl_json_post(
    root: &Path,
    url: &str,
    auth_header: &str,
    request_path: &Path,
    headers: &[&str],
    timeout_sec: u64,
) -> Result<HttpPostOutput> {
    let mut child = ProcessCommand::new("curl")
        .arg("-sS")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg(timeout_sec.to_string())
        .arg("-w")
        .arg("\nUB_REVIEW_HTTP_STATUS:%{http_code}\n")
        .arg("-X")
        .arg("POST")
        .arg("-K")
        .arg("-")
        .arg("--data-binary")
        .arg(format!("@{}", request_path.display()))
        .arg(url)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| "spawn curl")?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("curl stdin unavailable"))?;
        use std::io::Write as _;
        for header in headers {
            writeln!(stdin, "header = \"{}\"", curl_config_quote(header))?;
        }
        writeln!(stdin, "header = \"{}\"", curl_config_quote(auth_header))?;
    }
    let output = child.wait_with_output().with_context(|| "wait for curl")?;
    let (stdout, http_status) = split_curl_http_status(output.stdout);
    Ok(HttpPostOutput {
        status: output.status,
        stdout,
        stderr: output.stderr,
        http_status,
    })
}

fn curl_config_quote(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn split_curl_http_status(stdout: Vec<u8>) -> (Vec<u8>, Option<u16>) {
    const MARKER: &[u8] = b"\nUB_REVIEW_HTTP_STATUS:";
    let Some(position) = stdout
        .windows(MARKER.len())
        .rposition(|window| window == MARKER)
    else {
        return (stdout, None);
    };
    let status_bytes = &stdout[position + MARKER.len()..];
    let Ok(status_text) = std::str::from_utf8(status_bytes) else {
        return (stdout, None);
    };
    let Ok(status) = status_text.trim().parse::<u16>() else {
        return (stdout, None);
    };
    let mut body = stdout;
    body.truncate(position);
    (body, Some(status))
}

fn http_status_from_error(err: &anyhow::Error) -> Option<u16> {
    let text = format!("{err:#}");
    let needle = "http status Some(";
    let start = text.find(needle)? + needle.len();
    let end = text[start..].find(')')? + start;
    text[start..end].parse::<u16>().ok()
}

fn post_github_review(args: &PostArgs) -> Result<serde_json::Value> {
    let token = args
        .github_token
        .as_ref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("github token is required for posting"))?;
    let repo = args
        .repo
        .as_ref()
        .filter(|value| is_valid_repo_slug(value))
        .ok_or_else(|| anyhow::anyhow!("valid GitHub repository slug is required"))?;
    let pull_number = match args.pull_number {
        Some(number) => number,
        None => detect_pull_number_from_event()
            .ok_or_else(|| anyhow::anyhow!("pull request number is required for posting"))?,
    };
    let review: GitHubReview = serde_json::from_slice(
        &fs::read(&args.review_json)
            .with_context(|| format!("read {}", args.review_json.display()))?,
    )
    .with_context(|| format!("parse {}", args.review_json.display()))?;
    validate_github_review_payload(&review)?;
    let post_payload = args.out.join("github-review-post-payload.json");
    fs::write(&post_payload, serde_json::to_vec_pretty(&review)?)?;
    let url = format!(
        "{}/repos/{}/pulls/{}/reviews",
        args.github_api_url.trim_end_matches('/'),
        repo,
        pull_number
    );
    let output = run_curl_json_post(
        Path::new("."),
        &url,
        &format!("Authorization: Bearer {token}"),
        &post_payload,
        &[
            "Accept: application/vnd.github+json",
            "Content-Type: application/json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    )
    .with_context(|| "run GitHub review curl")?;
    let response_text = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr_text = String::from_utf8_lossy(&output.stderr).to_string();
    fs::write(args.out.join("post-stdout.json"), &response_text)?;
    fs::write(args.out.join("post-stderr.txt"), &stderr_text)?;
    let response = serde_json::from_str(&response_text).unwrap_or_else(|_| {
        serde_json::json!({
            "raw": response_text,
        })
    });
    if !output.status.success() {
        bail!(
            "GitHub review post failed with exit code {:?} and http status {:?}: {}",
            output.status.code(),
            output.http_status,
            stderr_text
        );
    }
    Ok(serde_json::json!({
        "status": "ok",
        "repo": repo,
        "pull_number": pull_number,
        "comments": review.comments.len(),
        "http_status": output.http_status,
        "response": response,
    }))
}

fn validate_github_review_payload(review: &GitHubReview) -> Result<()> {
    if review.event != "COMMENT" {
        bail!("github review event must be COMMENT");
    }
    if has_standalone_approval_line(&review.body) {
        bail!("github review body contains standalone approval language");
    }
    for comment in &review.comments {
        if comment.side != "RIGHT" {
            bail!("github review comments must use side=RIGHT");
        }
        if comment.path.trim().is_empty() || Path::new(&comment.path).is_absolute() {
            bail!("github review comment path must be repo-relative");
        }
        if comment.line == 0 {
            bail!("github review comment line must be positive");
        }
        if has_standalone_approval_line(&comment.body) {
            bail!("github review comment contains standalone approval language");
        }
    }
    Ok(())
}

fn is_valid_repo_slug(value: &str) -> bool {
    let mut parts = value.split('/');
    let Some(owner) = parts.next() else {
        return false;
    };
    let Some(repo) = parts.next() else {
        return false;
    };
    parts.next().is_none()
        && !owner.is_empty()
        && !repo.is_empty()
        && owner.chars().all(is_repo_slug_char)
        && repo.chars().all(is_repo_slug_char)
}

fn is_repo_slug_char(value: char) -> bool {
    value.is_ascii_alphanumeric() || matches!(value, '-' | '_' | '.')
}

fn detect_pull_number_from_event() -> Option<u64> {
    let path = std::env::var_os("GITHUB_EVENT_PATH")?;
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .pointer("/pull_request/number")
        .and_then(serde_json::Value::as_u64)
}

fn render_summary(out: &Path, plan: &Plan, diff: &DiffContext) -> Result<String> {
    let mut text = String::new();
    text.push_str("# UB Review Packet\n\n");
    text.push_str("This is an advisory evidence packet plus review compiler. Posting is a separate grouped Pull Request Review step.\n\n");
    text.push_str(&format!("- Profile: `{}`\n", plan.profile_name));
    text.push_str(&format!("- Base: `{}`\n", plan.base));
    text.push_str(&format!("- Head: `{}`\n", plan.head));
    text.push_str(&format!(
        "- Changed files: `{}`\n",
        diff.changed_files.len()
    ));
    text.push_str("\n## Sensors\n\n");
    text.push_str("| Sensor | Planned | Status | Reason | Receipt |\n");
    text.push_str("|---|---:|---|---|---|\n");
    for sensor in &plan.sensors {
        let status_path = out
            .join("sensors")
            .join(&sensor.id)
            .join("ub-review-sensor-status.json");
        let receipt = read_sensor_receipt(&status_path);
        let status = receipt
            .as_ref()
            .map(|receipt| receipt.status.clone())
            .unwrap_or_else(|| {
                if sensor.run {
                    "receipt-absent".to_owned()
                } else {
                    "skipped".to_owned()
                }
            });
        let reason = receipt
            .as_ref()
            .map(|receipt| receipt.reason.as_str())
            .unwrap_or(&sensor.reason);
        let planned = if sensor.run { "yes" } else { "no" };
        let receipt = format!("`sensors/{}/`", sensor.id);
        text.push_str(&format!(
            "| `{}` | {} | `{}` | {} | {} |\n",
            sensor.id,
            planned,
            status,
            escape_md(reason),
            receipt
        ));
    }
    render_evidence_sections(&mut text, out, plan);
    text.push_str("\n## Lane packets\n\n");
    text.push_str("| Lane | Model | Packet |\n");
    text.push_str("|---|---|---|\n");
    for lane in &plan.lanes {
        text.push_str(&format!(
            "| `{}` | `{}` | `lanes/{}.md` |\n",
            lane.id, lane.model_display, lane.id
        ));
    }
    text.push_str("\n## Diff flags\n\n");
    text.push_str(&format!(
        "- Unsafe/native risk touched: `{}`\n",
        diff.flags.unsafe_or_native_risk
    ));
    text.push_str(&format!(
        "- Rust behavior or tests touched: `{}`\n",
        diff.flags.rust_changed || diff.flags.rust_tests_changed
    ));
    text.push_str(&format!(
        "- Source changed: `{}`\n",
        diff.flags.source_changed
    ));
    text.push_str("\n## Changed files\n\n");
    if diff.changed_files.is_empty() {
        text.push_str("- No changed files detected. Check checkout/base configuration.\n");
    } else {
        for file in &diff.changed_files {
            text.push_str(&format!("- `{file}`\n"));
        }
    }
    text.push_str("\n## Notes\n\n");
    if plan.notes.is_empty() {
        text.push_str("- No planner notes.\n");
    } else {
        for note in &plan.notes {
            text.push_str(&format!("- {}\n", escape_md(note)));
        }
    }
    text.push_str("\n## Review posture\n\n");
    text.push_str("A one-line approval shortcut is a failure mode. A no-finding lane must include what it checked, its strongest failed objection, and residual risk. Missing sensor evidence is not proof of safety.\n");
    Ok(text)
}

fn render_evidence_sections(text: &mut String, out: &Path, plan: &Plan) {
    let mut available = Vec::new();
    let mut missing = Vec::new();
    let mut failed = Vec::new();

    for sensor in &plan.sensors {
        let status_path = out
            .join("sensors")
            .join(&sensor.id)
            .join("ub-review-sensor-status.json");
        let Some(receipt) = read_sensor_receipt(&status_path) else {
            if sensor.run {
                missing.push(format!(
                    "{} receipt absent; {} unavailable.",
                    sensor.id,
                    evidence_label(&sensor.id)
                ));
            }
            continue;
        };
        match receipt.status.as_str() {
            "ok" => available.push(format!(
                "{} ran; {} available.",
                sensor.id,
                evidence_label(&sensor.id)
            )),
            "missing" => missing.push(format!(
                "{} not installed; {} unavailable.",
                sensor.id,
                evidence_label(&sensor.id)
            )),
            "failed" | "timed_out" => failed.push(format!(
                "{} {}; reason: {}.",
                sensor.id, receipt.status, receipt.reason
            )),
            _ => {}
        }
    }

    text.push_str("\n## Available evidence\n\n");
    if available.is_empty() {
        text.push_str("- No sensor evidence completed successfully.\n");
    } else {
        for item in available {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }

    text.push_str("\n## Missing evidence\n\n");
    if missing.is_empty() {
        text.push_str("- No planned sensor evidence is currently missing.\n");
    } else {
        for item in missing {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }

    if !failed.is_empty() {
        text.push_str("\n## Failed evidence\n\n");
        for item in failed {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }
}

fn evidence_label(sensor_id: &str) -> &'static str {
    match sensor_id {
        "tokmd" => "deterministic repository/diff packet",
        "ripr" => "Rust test-oracle packet",
        "unsafe-review" => "unsafe/native reviewability packet",
        "ast-grep" => "structural route scan",
        "semgrep" => "semantic security scan",
        "actionlint" => "workflow lint packet",
        "zizmor" => "workflow hardening packet",
        "gitleaks" => "secret-scan packet",
        "osv-scanner" => "dependency advisory packet",
        "cargo-audit" => "Cargo advisory packet",
        "cargo-deny" => "Cargo policy packet",
        _ => "sensor packet",
    }
}

fn read_sensor_receipt(path: &Path) -> Option<SensorReceipt> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn append_github_step_summary(summary: &str) -> Result<()> {
    let Some(path) = std::env::var_os("GITHUB_STEP_SUMMARY") else {
        return Ok(());
    };
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    use std::io::Write as _;
    writeln!(file, "\n{summary}")?;
    Ok(())
}

fn escape_md(value: &str) -> String {
    value.replace('|', "\\|")
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

fn command_on_path(command: &str) -> bool {
    if command.contains('/') || command.contains('\\') {
        return Path::new(command).exists();
    }
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(command);
        if candidate.is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            let pathext = std::env::var_os("PATHEXT")
                .map(|value| {
                    value
                        .to_string_lossy()
                        .split(';')
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec![".EXE".to_owned(), ".CMD".to_owned(), ".BAT".to_owned()]);
            pathext
                .iter()
                .any(|ext| dir.join(format!("{command}{ext}")).is_file())
        }
        #[cfg(not(windows))]
        {
            false
        }
    })
}

fn detect_mem_available_mb() -> Option<u64> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

fn detect_load_1m() -> Option<f32> {
    let text = fs::read_to_string("/proc/loadavg").ok()?;
    text.split_whitespace().next()?.parse::<f32>().ok()
}

fn detect_disk_free_mb() -> Option<u64> {
    let output = ProcessCommand::new("df")
        .arg("-Pk")
        .arg(".")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let line = stdout.lines().nth(1)?;
    let available_kb = line.split_whitespace().nth(3)?.parse::<u64>().ok()?;
    Some(available_kb / 1024)
}

fn builtin_profiles() -> Vec<Profile> {
    vec![
        profile(
            "gh-runner",
            20,
            16,
            3,
            6,
            3,
            2,
            2,
            0,
            0,
            1_500,
            4_000,
            6.0,
            750,
            4_000,
            900,
        ),
        profile(
            "cx23", 20, 12, 2, 4, 2, 1, 1, 0, 0, 900, 8_000, 3.0, 500, 8_000, 900,
        ),
        profile(
            "cx33", 24, 16, 3, 6, 3, 2, 2, 1, 0, 1_400, 12_000, 5.0, 1_000, 16_000, 1_200,
        ),
        profile(
            "cx43", 32, 24, 6, 10, 6, 4, 3, 2, 1, 2_500, 20_000, 9.0, 2_000, 40_000, 1_800,
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn profile(
    name: &str,
    logical_lanes: usize,
    llm: usize,
    sensor_jobs: usize,
    repo_read: usize,
    grep: usize,
    ast_grep: usize,
    git: usize,
    tests: usize,
    builds: usize,
    min_mem: u64,
    min_disk: u64,
    max_load: f32,
    artifact_budget: u64,
    scratch_budget: u64,
    timeout: u64,
) -> Profile {
    Profile {
        name: name.to_owned(),
        limits: Limits {
            logical_lanes,
            llm_in_flight: llm,
            sensor_jobs,
            repo_read,
            raw_file_reads: repo_read,
            grep,
            ast_grep,
            git,
            tests,
            builds,
            rust_analyzer: usize::from(builds > 0),
            summary_writers: 1,
            patch_writers: usize::from(name != "gh-runner"),
        },
        guards: Guards {
            min_free_mem_mb: min_mem,
            min_free_disk_mb: min_disk,
            max_load_1m: max_load,
        },
        budgets: Budgets {
            artifact_budget_mb: artifact_budget,
            scratch_budget_mb: scratch_budget,
            default_timeout_sec: timeout,
        },
    }
}

fn builtin_tools() -> Vec<ToolPolicy> {
    vec![
        tool(
            "tokmd",
            "tokmd",
            ToolClass::Packet,
            2,
            Trigger::Always,
            180,
            128,
            false,
            true,
        ),
        tool(
            "ast-grep",
            "ast-grep",
            ToolClass::Search,
            1,
            Trigger::SourceChanged,
            60,
            64,
            false,
            true,
        ),
        tool(
            "ripr",
            "ripr",
            ToolClass::Static,
            3,
            Trigger::RustBehaviorOrTestsChanged,
            240,
            128,
            false,
            true,
        ),
        tool(
            "unsafe-review",
            "unsafe-review",
            ToolClass::Static,
            3,
            Trigger::UnsafeOrNativeRiskChanged,
            240,
            128,
            false,
            true,
        ),
        tool(
            "semgrep",
            "semgrep",
            ToolClass::Security,
            3,
            Trigger::SourceChanged,
            180,
            128,
            false,
            false,
        ),
        tool(
            "actionlint",
            "actionlint",
            ToolClass::Workflow,
            1,
            Trigger::WorkflowChanged,
            60,
            32,
            false,
            true,
        ),
        tool(
            "zizmor",
            "zizmor",
            ToolClass::Workflow,
            2,
            Trigger::WorkflowChanged,
            120,
            64,
            false,
            true,
        ),
        tool(
            "gitleaks",
            "gitleaks",
            ToolClass::Security,
            2,
            Trigger::Diff,
            120,
            128,
            false,
            false,
        ),
        tool(
            "osv-scanner",
            "osv-scanner",
            ToolClass::Security,
            3,
            Trigger::DependencyChanged,
            180,
            128,
            false,
            false,
        ),
        tool(
            "cargo-audit",
            "cargo",
            ToolClass::Security,
            2,
            Trigger::DependencyChanged,
            120,
            64,
            false,
            false,
        ),
        tool(
            "cargo-deny",
            "cargo",
            ToolClass::Security,
            3,
            Trigger::DependencyChanged,
            180,
            128,
            false,
            false,
        ),
        tool(
            "shellcheck",
            "shellcheck",
            ToolClass::Static,
            1,
            Trigger::ShellChanged,
            60,
            32,
            false,
            true,
        ),
        tool(
            "cppcheck",
            "cppcheck",
            ToolClass::Static,
            3,
            Trigger::CppChanged,
            240,
            128,
            false,
            false,
        ),
        tool(
            "test",
            "cargo",
            ToolClass::Test,
            8,
            Trigger::Manual,
            900,
            256,
            true,
            true,
        ),
        tool(
            "build",
            "cargo",
            ToolClass::Build,
            12,
            Trigger::Manual,
            1_200,
            256,
            true,
            true,
        ),
        tool(
            "miri",
            "cargo",
            ToolClass::HeavyWitness,
            99,
            Trigger::Manual,
            1_800,
            256,
            true,
            true,
        ),
        tool(
            "cargo-mutants",
            "cargo",
            ToolClass::HeavyWitness,
            99,
            Trigger::Manual,
            1_800,
            256,
            true,
            true,
        ),
    ]
}

#[allow(clippy::too_many_arguments)]
fn tool(
    id: &str,
    command: &str,
    class: ToolClass,
    weight: u32,
    trigger: Trigger,
    timeout_sec: u64,
    artifact_budget_mb: u64,
    requires_lease: bool,
    enabled: bool,
) -> ToolPolicy {
    ToolPolicy {
        id: id.to_owned(),
        command: command.to_owned(),
        class,
        weight,
        default: trigger,
        timeout_sec,
        artifact_budget_mb,
        requires_lease,
        enabled,
    }
}

fn default_lanes() -> Vec<LanePlan> {
    vec![
        lane(
            "ub",
            "Native-boundary undefined-behavior review",
            "custom:MiniMax-M3-3",
            "MiniMax-M3",
            &["tokmd", "unsafe-review", "ast-grep", "ripr"],
            "Find stale pointer/length, RAB resize/detach/transfer/GC, worker handoff, active-view-region, JSC lifetime, FFI ownership, allocator, truncation, signedness, and overflow risks.",
        ),
        lane(
            "source-route",
            "Source route, sibling path, and claim verification",
            "custom:MiniMax-M3-3",
            "MiniMax-M3",
            &["tokmd", "ast-grep", "ripr"],
            "Trace public API to native path, sibling paths, helper callers, route variants, and PR claims. Call out overclaims and underclaims.",
        ),
        lane(
            "tests",
            "Red/green proof and oracle review",
            "custom:MiniMax-M3-3",
            "MiniMax-M3",
            &["tokmd", "ripr", "unsafe-review"],
            "Check whether tests fail on unpatched main, prove the changed behavior, observe post-capture mutations, and avoid smoke-only or tautological assertions.",
        ),
        lane(
            "arch",
            "Architecture, boundary, and smallest-complete-fix review",
            "custom:MiniMax-M3-3",
            "MiniMax-M3",
            &["tokmd", "unsafe-review", "ast-grep"],
            "Check boundary placement, helper shape, scope control, duplication risk, performance cost, and whether a smaller complete fix exists.",
        ),
        lane(
            "opposition",
            "Strongest substantiated objection review",
            "custom:MiniMax-M3-3",
            "MiniMax-M3",
            &["tokmd", "ripr", "unsafe-review", "ast-grep", "semgrep"],
            "Try to disprove the PR across correctness, test proof, performance, portability, source-route, and claim truth. Report serious issues outside focus too.",
        ),
        lane(
            "security",
            "Security and UB-as-exploit-primitive review",
            "custom:MiniMax-M3-3",
            "MiniMax-M3",
            &[
                "tokmd",
                "unsafe-review",
                "semgrep",
                "zizmor",
                "gitleaks",
                "osv-scanner",
                "cargo-audit",
                "cargo-deny",
            ],
            "Review STRIDE/OWASP and native-boundary exploitability: OOB, UAF, type confusion, size/offset overflow, information disclosure, DoS, secrets, crypto misuse, and workflow-token risk when present.",
        ),
    ]
}

fn lane(
    id: &str,
    role: &str,
    model: &str,
    model_display: &str,
    receives: &[&str],
    focus: &str,
) -> LanePlan {
    LanePlan {
        id: id.to_owned(),
        role: role.to_owned(),
        model: model.to_owned(),
        model_display: model_display.to_owned(),
        receives: receives.iter().map(|value| (*value).to_owned()).collect(),
        focus: focus.to_owned(),
    }
}

const NO_LGTM_POSTURE: &str = r#"Standalone approval language is banned.

Do not answer with only a one-word approval, a generic quality adjective, or a zero-actionable shorthand.

A zero-finding review is not approval. It must report:
1. what concrete paths, invariants, tests, or claims were checked;
2. the strongest failed objection;
3. why that objection did not hold;
4. residual risk for a human to verify.
"#;

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use anyhow::Result;

    use super::{
        BoxState, Config, DiffContext, DiffFlags, EventLog, GitHubReview, GitHubReviewComment,
        LaneModelOutput, LanePlan, ModelCandidateComment, ModelEvidenceIssue, ModelMode,
        ModelProvider, ModelProviderPolicy, NO_LGTM_POSTURE, OpenCodeEndpointKindArg, Plan,
        PostingMode, ProviderKindArg, RefuterDecision, RefuterOutput, ReviewArgs,
        ReviewInlineComment, RunArgs, RunMode, SensorEvidenceIssue, SensorPlan, SensorStatusWrite,
        SummaryOnlyFinding, ToolClass, apply_refuter_output, cap_review_body, classify_diff,
        dedupe_inline_comments, default_lanes, direct_minimax_spec, extract_model_content,
        http_status_from_error, model_api_url, model_assignments, model_auth_header,
        model_json_payload, model_request_payload, model_response_shape, opencode_canary_spec,
        render_ledger_context, render_review_body, render_summary, right_side_diff_lines,
        run_command_to_files, run_sensor, split_curl_http_status, validate_github_review_payload,
        validate_inline_candidate, write_sensor_status,
    };

    #[test]
    fn docs_only_diff_is_detected() {
        let flags = classify_diff(&["docs/readme.md".to_owned()], "");
        assert!(flags.docs_only);
        assert!(!flags.source_changed);
    }

    #[test]
    fn unsafe_tokens_trigger_native_risk() {
        let flags = classify_diff(&["src/lib.rs".to_owned()], "+ let p = bytes.as_ptr();");
        assert!(flags.rust_changed);
        assert!(flags.unsafe_or_native_risk);
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

    #[test]
    fn bun_config_loads_with_default_lanes_enabled() -> Result<()> {
        let mut config: Config = toml::from_str(include_str!("../configs/bun-gh-runner.toml"))?;
        config.merge_defaults();
        assert_eq!(config.profile, "gh-runner");
        assert!(config.review.enable_default_lanes);
        let profile = config.selected_profile()?;
        assert_eq!(profile.name, "gh-runner");
        let ripr = config
            .tools
            .get("ripr")
            .ok_or_else(|| anyhow::anyhow!("ripr tool policy missing"))?;
        assert!(ripr.enabled);
        Ok(())
    }

    #[test]
    fn missing_tool_status_is_recorded_as_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        let out = root.join("out");
        let event_log = EventLog::open(&out.join("events.ndjson"))?;
        let sensor = sensor_plan("ripr", "ub-review-test-tool-that-does-not-exist", true);
        let plan = test_plan(vec![sensor.clone()]);

        run_sensor(root, &out, &sensor, &event_log, &plan)?;

        let status_path = out.join("sensors/ripr/ub-review-sensor-status.json");
        let value: serde_json::Value = serde_json::from_slice(&fs::read(status_path)?)?;
        assert_eq!(value["sensor"], "ripr");
        assert_eq!(value["status"], "missing");
        assert_eq!(value["reason"], "command not found");
        assert!(out.join("sensors/ripr/stdout.txt").exists());
        assert!(out.join("sensors/ripr/stderr.txt").exists());
        Ok(())
    }

    #[test]
    fn sensor_timeout_status_is_returned() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let stdout_path = temp.path().join("stdout.txt");
        let stderr_path = temp.path().join("stderr.txt");
        let argv = sleeper_argv();

        let status = run_command_to_files(temp.path(), &argv, 1, &stdout_path, &stderr_path)?;

        assert!(status.timed_out);
        assert!(!status.success);
        assert_eq!(status.exit_code, None);
        assert!(status.duration_ms >= 1);
        Ok(())
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
            },
            &line_map,
        )
        .map_err(|finding| anyhow::anyhow!("unexpected rejection: {}", finding.reason))?;
        assert_eq!(accepted.side, "RIGHT");
        assert!(accepted.body.starts_with("[tests]"));

        let rejected = validate_inline_candidate(
            &lane,
            ModelCandidateComment {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 50,
                body: "[tests] guessed stale line".to_owned(),
                evidence: "none".to_owned(),
            },
            &line_map,
        );
        assert!(rejected.is_err());
        Ok(())
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
        let ok = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Decision\n\n- No blocking finding after bounded review; evidence is incomplete.\n\n## No blocking finding after checking\n\n- packet\n\n## Failed objections\n\n- missing evidence is listed separately\n\n## Residual risk\n\n- human review".to_owned(),
            comments: vec![GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 2,
                side: "RIGHT".to_owned(),
                body: "[tests] This test reaches the helper but does not assert the boundary.".to_owned(),
            }],
        };
        validate_github_review_payload(&ok)?;

        let bad_side = GitHubReview {
            comments: vec![GitHubReviewComment {
                side: "LEFT".to_owned(),
                ..ok.comments[0].clone()
            }],
            ..ok.clone()
        };
        assert!(validate_github_review_payload(&bad_side).is_err());
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
    fn provider_policy_minimax_only_uses_direct_m3() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::MinimaxOnly;
        args.lane_width = 6;
        let assignments = model_assignments(&test_plan(Vec::new()), &args);

        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].spec.provider, ModelProvider::MiniMaxDirect);
        assert_eq!(assignments[0].spec.model, "MiniMax-M3");
        assert!(assignments[0].fallback.is_none());
    }

    #[test]
    fn direct_minimax_openai_uses_chat_endpoint_and_bearer_header() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
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
        let assignments = model_assignments(&test_plan(Vec::new()), &args);

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
    fn provider_policy_opencode_wide_uses_flash_for_fast_lanes() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.provider_policy = ModelProviderPolicy::OpencodeGoWide;
        args.lane_width = 20;
        let assignments = model_assignments(&test_plan(Vec::new()), &args);

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
        let minimax_thinking: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/providers/minimax-m3-thinking-then-text.json"
        ))?;
        let malformed: serde_json::Value = serde_json::from_str(include_str!(
            "../fixtures/providers/malformed-no-content.json"
        ))?;

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
        assert_eq!(model_response_shape(&minimax_thinking), "anthropic");
        assert_eq!(
            extract_model_content(&minimax_thinking),
            Some(
                "{\"summary\":\"preflight ok\",\"inline_comments\":[],\"summary_only_findings\":[]}"
            )
        );
        assert_eq!(model_response_shape(&malformed), "unknown");
        assert!(extract_model_content(&malformed).is_none());
        Ok(())
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
        assert!(parsed.summary_only_findings.is_empty());
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
    fn model_error_exposes_http_status_for_receipts() {
        let err = anyhow::anyhow!(
            "model curl exited Some(22) with http status Some(401): stderr: unauthorized"
        );

        assert_eq!(http_status_from_error(&err), Some(401));
        assert_eq!(super::classify_model_error(&err), "auth_failed");
    }

    #[test]
    fn minimax_openai_payload_uses_chat_shape() {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
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
    fn minimax_anthropic_payload_uses_messages_shape() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.minimax_provider_kind = ProviderKindArg::Anthropic;
        let spec = direct_minimax_spec(&args);
        let payload = model_request_payload(&spec, "packet");

        assert_eq!(payload["model"], "MiniMax-M3");
        assert_eq!(payload["max_tokens"], 4096);
        assert_eq!(payload["thinking"]["type"], "adaptive");
        assert!(
            payload["system"]
                .as_str()
                .is_some_and(|system| system.contains("final text block"))
        );
        assert!(payload["messages"].is_array());
        assert!(payload["messages"][0]["content"].as_str().is_some());
        assert!(payload.get("stream").is_none());
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
            60_000,
        );

        assert!(body.contains("## Decision"));
        assert!(body.contains("evidence is incomplete"));
        assert!(body.contains("## Confirmed findings"));
        assert!(body.contains("## Summary-only findings"));
        assert!(body.contains("## Failed objections"));
        assert!(body.contains("## Residual risk"));
        assert!(body.contains("## Parked follow-ups"));
        assert!(body.contains("## Missing or failed evidence"));
        assert!(body.contains("Sensor `ripr`: `missing` - command not found"));
        assert!(body.contains("rate_limited"));
        assert!(body.contains("## No blocking finding after checking"));
        assert!(!has_standalone_approval_line(&body));
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
            60_000,
        );

        assert!(body.contains("## Parked follow-ups"));
        assert!(body.contains("PBKDF2 sibling path is parked as follow-up"));
        assert!(body.contains("## Summary-only findings"));
        assert!(!has_standalone_approval_line(&body));
    }

    #[test]
    fn review_body_cap_preserves_utf8_boundary() {
        let capped = cap_review_body("a🙂b🙂c".repeat(100), 64);

        assert!(capped.ends_with("[review body truncated; see review artifacts]\n"));
        assert!(capped.is_char_boundary(capped.len()));
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
            1_000,
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

    fn sensor_plan(id: &str, command: &str, run: bool) -> SensorPlan {
        SensorPlan {
            id: id.to_owned(),
            command: command.to_owned(),
            run,
            reason: "test reason".to_owned(),
            timeout_sec: 1,
            class: ToolClass::Static,
            weight: 1,
            requires_lease: false,
        }
    }

    fn test_plan(sensors: Vec<SensorPlan>) -> Plan {
        Plan {
            base: "HEAD~1".to_owned(),
            head: "HEAD".to_owned(),
            profile_name: "gh-runner".to_owned(),
            sensors,
            lanes: vec![LanePlan {
                id: "tests".to_owned(),
                role: "Test oracle review".to_owned(),
                model: "custom:MiniMax-M3-3".to_owned(),
                model_display: "MiniMax-M3".to_owned(),
                receives: vec!["ripr".to_owned()],
                focus: "Check test proof.".to_owned(),
            }],
            docs_only: false,
            notes: Vec::new(),
        }
    }

    fn test_diff() -> DiffContext {
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
        }
    }

    fn test_run_args(out: std::path::PathBuf) -> RunArgs {
        RunArgs {
            review: ReviewArgs {
                root: Path::new(".").to_path_buf(),
                base: "HEAD~1".to_owned(),
                head: "HEAD".to_owned(),
                config: Path::new(".ub-review.toml").to_path_buf(),
                out,
                profile: None,
            },
            dry_run: false,
            allow_heavy: false,
            no_github_summary: true,
            posting: PostingMode::ArtifactOnly,
            mode: RunMode::ReviewDirect,
            model_mode: ModelMode::Auto,
            max_inline_comments: 8,
            model_concurrency: 8,
            max_model_calls: 14,
            provider_policy: ModelProviderPolicy::MinimaxPrimary,
            lane_width: 10,
            model_timeout_sec: 180,
            ledger_path: String::new(),
            ledger_max_bytes: 65_536,
            minimax_provider_kind: ProviderKindArg::Openai,
            minimax_model: "MiniMax-M3".to_owned(),
            opencode_model: "minimax-m3".to_owned(),
            opencode_endpoint_kind: OpenCodeEndpointKindArg::Auto,
            review_body_max_bytes: 60_000,
        }
    }

    fn sleeper_argv() -> Vec<String> {
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
}
