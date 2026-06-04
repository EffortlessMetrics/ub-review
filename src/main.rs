//! Box-aware evidence packet runner for UB-focused PR review.
//!
//! The binary prepares deterministic receipts, model-review artifacts, and lane
//! packets. Posting is a separate command that submits one grouped pull request
//! review when explicitly configured.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as ProcessCommand, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wait_timeout::ChildExt;

const STANDARD_LANE_WIDTH: usize = 10;
const STANDARD_MODEL_CONCURRENCY: usize = 8;
const STANDARD_MAX_MODEL_CALLS: usize = 14;
const DEFAULT_REVIEW_PROFILE: &str = "bun-ub-v0";

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Init(args) => cmd_init(args),
        Command::Doctor(args) => cmd_doctor(args),
        Command::Cache(args) => cmd_cache(args),
        Command::Plan(args) => cmd_plan(args),
        Command::Run(args) => cmd_run(*args),
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
    /// Inspect or prepare ub-review caches.
    Cache(CacheArgs),
    /// Build and print a run plan without executing sensors.
    Plan(PlanArgs),
    /// Build packets, run eligible sensors, and render lane packets.
    Run(Box<RunArgs>),
    /// Re-render a running summary from an existing run directory.
    Summary(SummaryArgs),
    /// Submit a prepared GitHub pull request review.
    Post(PostArgs),
}

#[derive(Clone, Debug, ValueEnum)]
enum ProfileArg {
    GhRunner,
    GhRunnerStandard,
    GhRunnerFull,
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
            Self::GhRunnerStandard => "gh-runner-standard",
            Self::GhRunnerFull => "gh-runner-full",
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

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum ReviewDepth {
    Quick,
    Standard,
    Deep,
}

impl ReviewDepth {
    fn key(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Standard => "standard",
            Self::Deep => "deep",
        }
    }

    fn lane_width(self) -> usize {
        match self {
            Self::Quick => 6,
            Self::Standard => STANDARD_LANE_WIDTH,
            Self::Deep => 20,
        }
    }

    fn model_concurrency(self) -> usize {
        match self {
            Self::Quick => 4,
            Self::Standard | Self::Deep => STANDARD_MODEL_CONCURRENCY,
        }
    }

    fn max_model_calls(self) -> usize {
        match self {
            Self::Quick => 6,
            Self::Standard => STANDARD_MAX_MODEL_CALLS,
            Self::Deep => 24,
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
    /// Runtime profile override. Prefer this over --profile for box budgets.
    #[arg(
        long = "runtime-profile",
        value_enum,
        env = "UB_REVIEW_RUNTIME_PROFILE"
    )]
    runtime_profile: Option<ProfileArg>,
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
    #[arg(
        long = "runtime-profile",
        value_enum,
        env = "UB_REVIEW_RUNTIME_PROFILE"
    )]
    runtime_profile: Option<ProfileArg>,
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    root: PathBuf,
    #[arg(long, env = "UB_REVIEW_BASE")]
    base: Option<String>,
    #[arg(long, env = "UB_REVIEW_CACHE_DIR")]
    cache_dir: Option<PathBuf>,
    #[arg(long, env = "UB_REVIEW_REQUIRE_CORE_TOOLS")]
    require_core_tools: bool,
}

#[derive(Debug, Args)]
struct CacheArgs {
    #[command(subcommand)]
    command: CacheCommand,
}

#[derive(Debug, Subcommand)]
enum CacheCommand {
    /// Create the cache directory skeleton and base-tree manifest.
    Warm(CacheWarmArgs),
}

#[derive(Debug, Args)]
struct CacheWarmArgs {
    #[arg(long, default_value = ".ub-review.toml", env = "UB_REVIEW_CONFIG")]
    config: PathBuf,
    #[arg(long, value_enum, env = "UB_REVIEW_PROFILE")]
    profile: Option<ProfileArg>,
    #[arg(
        long = "runtime-profile",
        value_enum,
        env = "UB_REVIEW_RUNTIME_PROFILE"
    )]
    runtime_profile: Option<ProfileArg>,
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    root: PathBuf,
    #[arg(long, default_value = "origin/main", env = "UB_REVIEW_BASE")]
    base: String,
    #[arg(long = "out", env = "UB_REVIEW_CACHE_DIR")]
    cache_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
struct PlanArgs {
    #[command(flatten)]
    review: ReviewArgs,
    #[command(flatten)]
    selectors: SelectorArgs,
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
    #[command(flatten)]
    selectors: SelectorArgs,
    /// Review depth selector. Nonstandard depths expand to lane/model budgets.
    #[arg(long, value_enum, default_value = "standard", env = "UB_REVIEW_DEPTH")]
    depth: ReviewDepth,
    /// Maximum inline comments to include in github-review.json.
    #[arg(long, default_value_t = 8, env = "UB_REVIEW_MAX_INLINE_COMMENTS")]
    max_inline_comments: usize,
    /// Planned model concurrency for model lane packets.
    #[arg(
        long,
        default_value_t = STANDARD_MODEL_CONCURRENCY,
        env = "UB_REVIEW_MODEL_CONCURRENCY"
    )]
    model_concurrency: usize,
    /// Maximum planned model calls.
    #[arg(
        long,
        default_value_t = STANDARD_MAX_MODEL_CALLS,
        env = "UB_REVIEW_MAX_MODEL_CALLS"
    )]
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
    #[arg(long, default_value_t = STANDARD_LANE_WIDTH, env = "UB_REVIEW_LANE_WIDTH")]
    lane_width: usize,
    /// Per-model-call timeout in seconds.
    #[arg(long, default_value_t = 300, env = "UB_REVIEW_MODEL_TIMEOUT_SEC")]
    model_timeout_sec: u64,
    /// Optional read-only UB ledger path.
    #[arg(long, default_value = "", env = "UB_REVIEW_LEDGER_PATH")]
    ledger_path: String,
    /// Maximum bytes of UB ledger context.
    #[arg(long, default_value_t = 65_536, env = "UB_REVIEW_LEDGER_MAX_BYTES")]
    ledger_max_bytes: usize,
    /// Optional PR thread context file with prior replies, receipts, or resolved comments.
    #[arg(long, default_value = "", env = "UB_REVIEW_PR_THREAD_CONTEXT")]
    pr_thread_context: String,
    /// Maximum bytes of PR thread context to seed into shared_context.md.
    #[arg(
        long,
        default_value_t = 65_536,
        env = "UB_REVIEW_PR_THREAD_CONTEXT_MAX_BYTES"
    )]
    pr_thread_context_max_bytes: usize,
    /// GitHub credential used only to fetch bounded PR-thread context during `run`.
    #[arg(long = "github-token", env = "UB_REVIEW_PR_THREAD_AUTH")]
    pr_thread_auth: Option<String>,
    /// owner/repo used to fetch bounded PR-thread context. Defaults to GITHUB_REPOSITORY.
    #[arg(long = "github-repo", env = "GITHUB_REPOSITORY")]
    github_repo: Option<String>,
    /// Pull request number used to fetch bounded PR-thread context.
    #[arg(long = "github-pull-number", env = "UB_REVIEW_PULL_NUMBER")]
    github_pull_number: Option<u64>,
    /// GitHub API base URL used to fetch bounded PR-thread context.
    #[arg(
        long = "github-api-url",
        default_value = "https://api.github.com",
        env = "UB_REVIEW_GITHUB_API_URL"
    )]
    github_api_url: String,
    /// MiniMax provider request/response family.
    #[arg(
        long,
        value_enum,
        default_value = "anthropic",
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

#[derive(Clone, Debug, Default, Args)]
struct SelectorArgs {
    /// Comma-separated lane IDs to run. Empty means the profile default.
    #[arg(long, default_value = "", env = "UB_REVIEW_LANES")]
    lanes: String,
    /// Comma-separated lane IDs to skip after applying --lanes.
    #[arg(
        long = "except-lanes",
        default_value = "",
        env = "UB_REVIEW_EXCEPT_LANES"
    )]
    except_lanes: String,
    /// Comma-separated sensor/tool IDs to plan. Empty means the profile default.
    #[arg(long, default_value = "", env = "UB_REVIEW_TOOLS")]
    tools: String,
    /// Comma-separated sensor/tool IDs to skip after applying --tools.
    #[arg(
        long = "except-tools",
        default_value = "",
        env = "UB_REVIEW_EXCEPT_TOOLS"
    )]
    except_tools: String,
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
    /// Diff patch used to validate RIGHT-side inline comment lines.
    #[arg(long, env = "UB_REVIEW_DIFF_PATCH")]
    diff_patch: Option<PathBuf>,
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
    github_review_json: String,
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
#[serde(default)]
struct Config {
    review_profile: String,
    profile: String,
    repo: RepoConfig,
    review: ReviewConfig,
    review_body: ReviewBodyPolicy,
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
struct ReviewBodyPolicy {
    include_successful_lane_table: bool,
    include_provider_table: ReviewBodyTablePolicy,
    include_sensor_table: ReviewBodyTablePolicy,
    include_execution_summary: ReviewBodyExecutionSummaryPolicy,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReviewBodyTablePolicy {
    Never,
    #[default]
    OnFailure,
    Always,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ReviewBodyExecutionSummaryPolicy {
    #[default]
    None,
    OnFailure,
    Always,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default)]
struct Profile {
    name: String,
    limits: Limits,
    guards: Guards,
    budgets: Budgets,
    trusted_repo: TrustedRepo,
}

#[derive(Debug, Deserialize)]
struct RuntimeProfileFile {
    name: String,
    limits: RuntimeLimitsFile,
    guards: RuntimeGuardsFile,
    budgets: RuntimeBudgetsFile,
    trusted_repo: TrustedRepo,
}

#[derive(Debug, Deserialize)]
struct RuntimeLimitsFile {
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

#[derive(Debug, Deserialize)]
struct RuntimeGuardsFile {
    min_free_mem_mb: u64,
    min_free_disk_mb: u64,
    max_load_1m: f32,
}

#[derive(Debug, Deserialize)]
struct RuntimeBudgetsFile {
    artifact_budget_mb: u64,
    scratch_budget_mb: u64,
    default_timeout_sec: u64,
    hard_timeout_sec: u64,
    proof_max_focused_test_files: usize,
    proof_max_focused_tests: usize,
    proof_command_timeout_sec: u64,
    proof_total_timeout_sec: u64,
    proof_cpu: u32,
    proof_memory_mb: u64,
    proof_disk_mb: u64,
    proof_network: bool,
    proof_scratch: bool,
    mutation: bool,
    sanitizer: bool,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
struct TrustedRepo {
    pass_triggers: Vec<String>,
    synchronize: bool,
    proof_lanes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
struct Guards {
    min_free_mem_mb: u64,
    min_free_disk_mb: u64,
    max_load_1m: f32,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
struct Budgets {
    artifact_budget_mb: u64,
    scratch_budget_mb: u64,
    default_timeout_sec: u64,
    hard_timeout_sec: u64,
    proof_max_focused_test_files: usize,
    proof_max_focused_tests: usize,
    proof_command_timeout_sec: u64,
    proof_total_timeout_sec: u64,
    proof_cpu: u32,
    proof_memory_mb: u64,
    proof_disk_mb: u64,
    proof_network: bool,
    proof_scratch: bool,
    mutation: bool,
    sanitizer: bool,
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
    required_reason: String,
    runtime_profile: String,
    enabled: bool,
    planned_run: bool,
    plan_reason: String,
    timeout_sec: u64,
    artifact_budget_mb: u64,
    requires_lease: bool,
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
    required_reason: String,
    runtime_profile: String,
    planned_run: bool,
    status: String,
    reason: String,
    exit_code: Option<i32>,
    timed_out: bool,
    artifact_paths: Vec<String>,
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
    inline_comments: usize,
    summary_only_findings: usize,
}

#[derive(Clone, Debug, Serialize)]
struct ReviewArtifacts {
    shared_context_id: String,
    review_profile: String,
    mode: String,
    posting: String,
    runtime_profile: String,
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
    proof_receipts: Vec<ProofReceipt>,
    resource_leases: Vec<ResourceLease>,
    body: String,
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
    summary_only_findings: usize,
    observations: usize,
    follow_up_results: FollowUpResultMetrics,
    proof_requests: usize,
    proof_receipts: usize,
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
    streams: RunStreamTimings,
    loops: RunLoopTimings,
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
struct ProofRequest {
    schema: String,
    id: String,
    lane: String,
    requested_by: Vec<String>,
    command: String,
    reason: String,
    cost: String,
    timeout_sec: u64,
    required: bool,
    status: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProofRequestGroup {
    schema: String,
    id: String,
    command: String,
    cost: String,
    timeout_sec: u64,
    required: bool,
    status: String,
    requested_by: Vec<String>,
    request_ids: Vec<String>,
    reasons: Vec<String>,
    duplicate_count: usize,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProofReceipt {
    schema: String,
    id: String,
    kind: String,
    base: String,
    head: String,
    test_patch_mode: String,
    requested_by: Vec<String>,
    request_ids: Vec<String>,
    commands: Vec<ProofCommandReceipt>,
    result: String,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ProofCommandReceipt {
    side: String,
    command: String,
    #[serde(default)]
    env: BTreeMap<String, String>,
    status: String,
    exit_code: Option<i32>,
    timed_out: bool,
    timeout_sec: u64,
    duration_ms: u128,
    stdout: String,
    stderr: String,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ResourceLease {
    schema: String,
    id: String,
    kind: String,
    consumer: String,
    status: String,
    reason: String,
    cpu: u32,
    memory_mb: u64,
    disk_mb: u64,
    timeout_sec: u64,
    network: bool,
    scratch: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    worktree: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    command: Option<String>,
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

#[derive(Clone, Debug)]
struct FocusedTestTask {
    id: String,
    file: String,
    test_name: Option<String>,
    mode: FocusedProofMode,
    requested_by: Vec<String>,
    request_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FocusedProofMode {
    HeadOnly,
    RedGreen,
}

impl FocusedProofMode {
    fn key(self) -> &'static str {
        match self {
            Self::HeadOnly => "head-only",
            Self::RedGreen => "red-green",
        }
    }

    fn command_count(self) -> u64 {
        match self {
            Self::HeadOnly => 1,
            Self::RedGreen => 2,
        }
    }
}

#[derive(Clone, Debug)]
struct FocusedProofPlan {
    id: String,
    test_file: String,
    test_name: Option<String>,
    mode: FocusedProofMode,
    head_command: String,
    base_plus_tests_command: String,
    requested_by: Vec<String>,
    request_ids: Vec<String>,
    status: String,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct ProofPlannerInput<'a> {
    schema: &'static str,
    diff_class: &'static str,
    changed_files: &'a [String],
    pr_thread_context_status: &'a str,
    proof_requests: &'a [ProofRequest],
    runtime_budget: ProofPlannerRuntimeBudget,
    box_shape: &'a BoxState,
}

#[derive(Clone, Debug, Serialize)]
struct ProofPlannerRuntimeBudget {
    target_timeout_sec: u64,
    hard_timeout_sec: u64,
    max_focused_tests: usize,
    per_command_timeout_sec: u64,
    total_proof_timeout_sec: u64,
}

#[derive(Clone, Debug, Serialize)]
struct ProofPlannerOutput {
    schema: &'static str,
    lane: &'static str,
    proof_tasks: Vec<ProofTaskArtifact>,
    skip: Vec<ProofPlannerSkip>,
}

#[derive(Clone, Debug, Serialize)]
struct ProofTaskArtifact {
    schema: &'static str,
    id: String,
    kind: String,
    command: String,
    head_command: String,
    base_plus_tests_command: Option<String>,
    purpose: String,
    consumers: Vec<String>,
    value: String,
    cost: String,
    timeout_sec: u64,
    lease: ProofTaskLease,
    test_file: String,
    test_name: Option<String>,
    mode: String,
    requested_by: Vec<String>,
    request_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
struct ProofTaskLease {
    cpu: u32,
    memory_mb: u64,
    disk_mb: u64,
    network: bool,
}

#[derive(Clone, Debug, Serialize)]
struct ProofPlannerSkip {
    kind: String,
    reason: String,
}

#[derive(Clone, Copy, Debug)]
struct ProofBudget {
    max_focused_test_files: usize,
    max_focused_tests: usize,
    per_command_timeout_sec: u64,
    max_total_seconds: u64,
}

#[derive(Clone, Copy, Debug)]
struct ProofLeaseBudget {
    cpu: u32,
    memory_mb: u64,
    disk_mb: u64,
    network: bool,
    scratch: bool,
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
    packet_path: String,
    model_lane: String,
    status: String,
    reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    duration_ms: Option<u128>,
    #[serde(skip_serializing_if = "Option::is_none")]
    http_status: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    response_shape: Option<String>,
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
    model_lane: String,
    status: String,
    reason: String,
    inline_comments: Vec<ReviewInlineComment>,
    summary_only_findings: Vec<SummaryOnlyFinding>,
    observations: Vec<Observation>,
    proof_requests: Vec<ProofRequest>,
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

const LANE_MODEL_ARRAY_FIELDS: &[&str] = &[
    "inline_comments",
    "candidate_findings",
    "summary_only_findings",
    "observations",
    "failed_objections",
    "proof_requests",
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
}

impl RunLoopTracker {
    fn new() -> Self {
        Self {
            evidence: LoopAccumulator::default(),
            model: LoopAccumulator::default(),
            proof: LoopAccumulator::default(),
            compiler: LoopAccumulator::default(),
        }
    }

    fn record(&mut self, loop_id: &str, interval: LoopInterval) {
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
    let mut wall_ms = 0_u128;
    for accumulator in accumulators {
        if let Some(started) = accumulator.started_at_offset_ms {
            started_at_offset_ms =
                Some(started_at_offset_ms.map_or(started, |existing| existing.min(started)));
        }
        if let Some(finished) = accumulator.finished_at_offset_ms {
            finished_at_offset_ms =
                Some(finished_at_offset_ms.map_or(finished, |existing| existing.max(finished)));
        }
        wall_ms = wall_ms.saturating_add(accumulator.wall_ms);
    }
    LoopTiming {
        started_at_offset_ms: started_at_offset_ms.unwrap_or(0),
        finished_at_offset_ms: finished_at_offset_ms.unwrap_or(0),
        wall_ms,
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
        self.wall_ms = self.wall_ms.saturating_add(interval.duration_ms);
        self.intervals.push(interval);
    }

    fn timing(&self) -> LoopTiming {
        LoopTiming {
            started_at_offset_ms: self.started_at_offset_ms.unwrap_or(0),
            finished_at_offset_ms: self.finished_at_offset_ms.unwrap_or(0),
            wall_ms: self.wall_ms,
        }
    }
}

#[derive(Clone, Copy)]
struct LoopInterval {
    started_at_offset_ms: u128,
    finished_at_offset_ms: u128,
    duration_ms: u128,
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

struct FollowUpRunContext<'a> {
    root: &'a Path,
    out: &'a Path,
    review_dir: &'a Path,
    provider_preflights: &'a [ProviderPreflightReceipt],
    args: &'a RunArgs,
    model_calls_used: usize,
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
    degraded: bool,
}

#[derive(Clone, Debug, Deserialize)]
struct SensorReceipt {
    status: String,
    reason: String,
    #[serde(default)]
    exit_code: Option<i32>,
    #[serde(default)]
    timed_out: bool,
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

impl Default for ReviewBodyPolicy {
    fn default() -> Self {
        Self {
            include_successful_lane_table: false,
            include_provider_table: ReviewBodyTablePolicy::OnFailure,
            include_sensor_table: ReviewBodyTablePolicy::OnFailure,
            include_execution_summary: ReviewBodyExecutionSummaryPolicy::None,
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

fn runtime_profile_override<'a>(
    legacy_profile: Option<&'a ProfileArg>,
    runtime_profile: Option<&'a ProfileArg>,
) -> Option<&'a str> {
    runtime_profile.or(legacy_profile).map(ProfileArg::key)
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
    tracker.record(
        active.loop_id,
        LoopInterval {
            started_at_offset_ms: active.started_at_offset_ms,
            finished_at_offset_ms,
            duration_ms,
        },
    );
    Ok(())
}

fn overlap_ms(left: &[LoopInterval], right: &[LoopInterval]) -> u128 {
    let mut total = 0_u128;
    for left_interval in left {
        for right_interval in right {
            let started = left_interval
                .started_at_offset_ms
                .max(right_interval.started_at_offset_ms);
            let finished = left_interval
                .finished_at_offset_ms
                .min(right_interval.finished_at_offset_ms);
            total = total.saturating_add(finished.saturating_sub(started));
        }
    }
    total
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
    println!("Profile: {}", profile.name);
    println!("Box: {}", box_state.summary_line());
    println!("Limits: {}", profile.limits.summary_line());
    println!("Cache root: {}", cache_root.display());
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
        let status = if command_on_path(&tool.command) {
            "found"
        } else {
            "missing"
        };
        let version = command_version(&tool.command).unwrap_or_else(|| "-".to_owned());
        let rule_hit = cache_root
            .join("rules")
            .join(&tool.id)
            .join("manifest.json")
            .exists();
        println!(
            "  {:<16} {:<8} {:<24} version={} rule-cache={}",
            tool.id,
            status,
            tool.command,
            version,
            if rule_hit { "hit" } else { "miss" }
        );
        if require_core_tools && is_core_review_tool(&tool.id) && status == "missing" {
            missing_required.push(tool.id.clone());
        }
    }
    if !missing_required.is_empty() {
        bail!(
            "required core review tools missing from standard image: {}",
            missing_required.join(", ")
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

fn cmd_run(args: RunArgs) -> Result<()> {
    let run_started = Instant::now();
    let mut args = normalize_run_args(args)?;
    let (config, diff, box_state, plan) =
        prepare_plan(&args.review, args.allow_heavy, &args.selectors)?;
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

    let event_log = EventLog::open(&args.review.out.join("events.ndjson"))?;
    let mut run_loop_tracker = RunLoopTracker::new();
    event_log.append(
        "run_started",
        serde_json::json!({"base": args.review.base, "head": args.review.head, "profile": plan.profile_name, "dry_run": args.dry_run}),
    )?;

    let evidence_loop = start_run_loop(
        &event_log,
        &run_started,
        "evidence",
        "coordination",
        "sensors-and-packet",
    )?;
    if args.dry_run {
        write_dry_run_sensor_receipts(&args.review.root, &args.review.out, &plan, &event_log)?;
        event_log.append("run_dry", serde_json::json!({"reason": "--dry-run"}))?;
    } else {
        write_skipped_sensor_receipts(&args.review.root, &args.review.out, &plan, &event_log)?;
        run_sensors(
            &args.review.root,
            &args.review.out,
            &plan,
            profile,
            &event_log,
        )?;
    }
    write_tool_status_artifacts(&args.review.out, &config, profile, &plan)?;

    write_lane_packets(&args.review.out, &diff, &plan, &event_log)?;
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
    write_review_artifacts(
        &args.review.root,
        &args.review.out,
        &config,
        &diff,
        &box_state,
        &plan,
        &preliminary_summary,
        &args,
        &event_log,
        &run_started,
        &mut run_loop_tracker,
        run_started.elapsed(),
    )?;
    let summary = render_summary(&args.review.out, &plan, &diff)?;
    fs::write(args.review.out.join("running-summary.md"), &summary)?;
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

fn normalize_run_args(mut args: RunArgs) -> Result<RunArgs> {
    apply_depth_defaults(&mut args)?;
    validate_run_args(&args)?;
    Ok(args)
}

fn apply_depth_defaults(args: &mut RunArgs) -> Result<()> {
    if args.depth == ReviewDepth::Standard {
        return Ok(());
    }
    if args.lane_width != STANDARD_LANE_WIDTH
        || args.model_concurrency != STANDARD_MODEL_CONCURRENCY
        || args.max_model_calls != STANDARD_MAX_MODEL_CALLS
    {
        bail!(
            "--depth {} cannot be combined with --lane-width, --model-concurrency, or --max-model-calls overrides; use --depth standard for custom raw budgets",
            args.depth.key()
        );
    }
    args.lane_width = args.depth.lane_width();
    args.model_concurrency = args.depth.model_concurrency();
    args.max_model_calls = args.depth.max_model_calls();
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
    validate_selector_syntax(&args.selectors)?;
    if !matches!(args.lane_width, 6 | 10 | 20) {
        bail!("--lane-width must be one of 6, 10, or 20");
    }
    if args.model_timeout_sec == 0 {
        bail!("--model-timeout-sec must be greater than zero");
    }
    if args.model_concurrency == 0 {
        bail!("--model-concurrency must be greater than zero");
    }
    if args.review_body_max_bytes < 1_000 {
        bail!("--review-body-max-bytes must be at least 1000");
    }
    Ok(())
}

fn apply_runtime_profile_limits(args: &mut RunArgs, profile: &Profile) -> Result<()> {
    let llm_in_flight = profile.limits.llm_in_flight;
    if llm_in_flight == 0 {
        bail!(
            "runtime profile {} has llm_in_flight=0; model concurrency cannot be scheduled",
            profile.name
        );
    }
    args.model_concurrency = args.model_concurrency.min(llm_in_flight);
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
        return Ok(());
    }
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
    }
}

fn read_github_review_skip_receipt(review_json: &Path) -> Option<serde_json::Value> {
    let skip_path = github_review_skip_path(review_json);
    let text = fs::read_to_string(skip_path).ok()?;
    serde_json::from_str(&text).ok()
}

fn build_post_error_receipt(args: &PostArgs, err: &anyhow::Error) -> PostErrorReceipt {
    let review_metadata = read_github_review_metadata(args);
    let repo_valid = args.repo.as_deref().is_some_and(is_valid_repo_slug);
    let pull_number = args.pull_number.or_else(detect_pull_number_from_event);
    let token_present = args
        .github_token
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    let review_json_valid = review_metadata
        .as_ref()
        .is_some_and(|metadata| metadata.valid);
    let http_status = http_status_from_error(err);
    let (error_kind, failure_stage) = classify_post_error(
        args,
        err,
        repo_valid,
        pull_number,
        review_json_valid,
        http_status,
    );
    let would_post = token_present && repo_valid && pull_number.is_some() && review_json_valid;
    let payload_written = failure_stage == "network_post"
        && args.out.join("github-review-post-payload.json").exists();
    PostErrorReceipt {
        schema_version: 1,
        status: "failed".to_owned(),
        error_kind,
        failure_stage,
        reason: format!("{err:#}"),
        review_json: args.review_json.display().to_string(),
        review_json_exists: args.review_json.exists(),
        review_json_valid,
        review_event: review_metadata.as_ref().map(|review| review.event.clone()),
        review_body_bytes: review_metadata.as_ref().map(|review| review.body_bytes),
        review_comment_count: review_metadata.as_ref().map(|review| review.comments),
        diff_patch: review_metadata
            .as_ref()
            .map(|review| review.diff_patch.display().to_string())
            .unwrap_or_else(|| post_diff_patch_path(args).display().to_string()),
        diff_patch_exists: review_metadata
            .as_ref()
            .is_some_and(|review| review.diff_patch_exists),
        diff_patch_valid: review_metadata
            .as_ref()
            .is_some_and(|review| review.diff_patch_valid),
        diff_line_count: review_metadata
            .as_ref()
            .and_then(|review| review.diff_line_count),
        off_diff_comment_count: review_metadata
            .as_ref()
            .and_then(|review| review.off_diff_comment_count),
        repo: args.repo.clone(),
        repo_valid,
        pull_number,
        comments: review_metadata.as_ref().map(|review| review.comments),
        http_status,
        token_present,
        payload_written,
        would_post,
        failure_tolerated: !args.fail_on_post_error,
        fail_on_post_error: args.fail_on_post_error,
    }
}

fn classify_post_error(
    args: &PostArgs,
    err: &anyhow::Error,
    repo_valid: bool,
    pull_number: Option<u64>,
    review_json_valid: bool,
    http_status: Option<u16>,
) -> (String, String) {
    let token_present = args
        .github_token
        .as_ref()
        .is_some_and(|value| !value.trim().is_empty());
    if !token_present {
        return ("missing_token".to_owned(), "preflight".to_owned());
    }
    if !repo_valid {
        return ("invalid_repo".to_owned(), "preflight".to_owned());
    }
    if pull_number.is_none() {
        return ("missing_pull_number".to_owned(), "preflight".to_owned());
    }
    if !review_json_valid {
        return (
            "invalid_review_payload".to_owned(),
            "payload_validation".to_owned(),
        );
    }
    if http_status.is_some() {
        return ("post_http_error".to_owned(), "network_post".to_owned());
    }
    let text = model_error_chain_text(err).to_ascii_lowercase();
    if text.contains("curl") || text.contains("github review post failed") {
        return ("post_failed".to_owned(), "network_post".to_owned());
    }
    ("failed".to_owned(), "unknown".to_owned())
}

struct GitHubReviewMetadata {
    valid: bool,
    comments: usize,
    event: String,
    body_bytes: usize,
    diff_patch: PathBuf,
    diff_patch_exists: bool,
    diff_patch_valid: bool,
    diff_line_count: Option<usize>,
    off_diff_comment_count: Option<usize>,
}

fn read_github_review_metadata(args: &PostArgs) -> Option<GitHubReviewMetadata> {
    let review: GitHubReview = serde_json::from_slice(&fs::read(&args.review_json).ok()?).ok()?;
    let diff_patch = post_diff_patch_path(args);
    let diff_metadata = review_diff_metadata(&diff_patch, &review);
    let diff_valid = review.comments.is_empty()
        || diff_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.off_diff_comment_count == 0);
    let valid = validate_github_review_payload(&review).is_ok() && diff_valid;
    Some(GitHubReviewMetadata {
        valid,
        comments: review.comments.len(),
        event: review.event,
        body_bytes: review.body.len(),
        diff_patch,
        diff_patch_exists: diff_metadata.is_some(),
        diff_patch_valid: diff_metadata.is_some(),
        diff_line_count: diff_metadata
            .as_ref()
            .map(|metadata| metadata.diff_line_count),
        off_diff_comment_count: diff_metadata.map(|metadata| metadata.off_diff_comment_count),
    })
}

struct ReviewDiffMetadata {
    diff_line_count: usize,
    off_diff_comment_count: usize,
}

fn review_diff_metadata(diff_patch: &Path, review: &GitHubReview) -> Option<ReviewDiffMetadata> {
    let patch = fs::read_to_string(diff_patch).ok()?;
    let right_lines = right_side_diff_lines(&patch);
    Some(ReviewDiffMetadata {
        diff_line_count: right_lines.len(),
        off_diff_comment_count: off_diff_comment_count(review, &right_lines),
    })
}

fn off_diff_comment_count(review: &GitHubReview, right_lines: &BTreeSet<(String, u32)>) -> usize {
    review
        .comments
        .iter()
        .filter(|comment| {
            let path = normalize_repo_path(&comment.path);
            !right_lines.contains(&(path, comment.line))
        })
        .count()
}

fn prepare_plan(
    args: &ReviewArgs,
    allow_heavy: bool,
    selectors: &SelectorArgs,
) -> Result<(Config, DiffContext, BoxState, Plan)> {
    validate_selector_syntax(selectors)?;
    let config = Config::load_or_default(
        &args.config,
        runtime_profile_override(args.profile.as_ref(), args.runtime_profile.as_ref()),
    )?;
    let profile = config.selected_profile()?;
    let box_state = BoxState::detect()?;
    let diff = DiffContext::from_git(&args.root, &args.base, &args.head)?;
    let mut plan = build_plan(&config, profile, &box_state, &diff, allow_heavy);
    apply_plan_selectors(&mut plan, selectors)?;
    Ok((config, diff, box_state, plan))
}

fn write_plan_artifacts(
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    box_state: &BoxState,
    plan: &Plan,
    selectors: PlanArtifactSelectors<'_>,
) -> Result<()> {
    fs::create_dir_all(out.join("input"))?;
    let profile = config.selected_profile()?;
    fs::write(out.join("plan.json"), serde_json::to_vec_pretty(plan)?)?;
    fs::write(
        out.join("effective-config.json"),
        serde_json::to_vec_pretty(config)?,
    )?;
    fs::write(
        out.join("resolved-profile.json"),
        serde_json::to_vec_pretty(&resolved_profile_artifact(config, profile))?,
    )?;
    fs::write(
        out.join("resolved-plan.json"),
        serde_json::to_vec_pretty(&resolved_plan_artifact(
            config,
            profile,
            diff,
            plan,
            selectors.run_args,
            selectors.selectors,
            selectors.effective_model_lanes,
        ))?,
    )?;
    write_resolved_tools_artifacts(out, config, profile, plan)?;
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

struct PlanArtifactSelectors<'a> {
    run_args: Option<&'a RunArgs>,
    selectors: &'a SelectorArgs,
    effective_model_lanes: Option<&'a [LanePlan]>,
}

fn resolved_profile_artifact(config: &Config, profile: &Profile) -> serde_json::Value {
    serde_json::json!({
        "schema": "ub-review.resolved_profile.v1",
        "selected_profile": &profile.name,
        "selected_review_profile": &config.review_profile,
        "selected_runtime_profile": &profile.name,
        "repo": &config.repo,
        "review": &config.review,
        "review_body": &config.review_body,
        "review_profile": {
            "name": &config.review_profile,
            "repo_kind": &config.repo.kind,
            "default_lanes_enabled": config.review.enable_default_lanes,
            "posting_engine": &config.review.posting_engine,
        },
        "profile": profile,
        "tools": &config.tools,
    })
}

fn write_resolved_tools_artifacts(
    out: &Path,
    config: &Config,
    profile: &Profile,
    plan: &Plan,
) -> Result<()> {
    let artifact = resolved_tools_artifact(config, profile, plan);
    let bytes = serde_json::to_vec_pretty(&artifact)?;
    fs::write(out.join("resolved-tools.json"), &bytes)?;
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    fs::write(review_dir.join("resolved-tools.json"), bytes)?;
    Ok(())
}

fn resolved_tools_artifact(
    config: &Config,
    profile: &Profile,
    plan: &Plan,
) -> ResolvedToolArtifact {
    let plan_by_id = plan
        .sensors
        .iter()
        .map(|sensor| (sensor.id.as_str(), sensor))
        .collect::<BTreeMap<_, _>>();
    let tools = config
        .tools
        .values()
        .map(|tool| {
            let planned = plan_by_id.get(tool.id.as_str());
            ResolvedToolEntry {
                id: tool.id.clone(),
                class: tool.class,
                command: tool.command.clone(),
                required_if: tool.default,
                required_reason: trigger_description(tool.default).to_owned(),
                runtime_profile: profile.name.clone(),
                enabled: tool.enabled,
                planned_run: planned.is_some_and(|sensor| sensor.run),
                plan_reason: planned
                    .map(|sensor| sensor.reason.clone())
                    .unwrap_or_else(|| "not present in resolved plan".to_owned()),
                timeout_sec: planned
                    .map(|sensor| sensor.timeout_sec)
                    .unwrap_or(tool.timeout_sec),
                artifact_budget_mb: tool.artifact_budget_mb,
                requires_lease: tool.requires_lease,
                artifact_paths: tool_artifact_paths(&tool.id),
            }
        })
        .collect();
    ResolvedToolArtifact {
        schema: "ub-review.resolved_tools.v1",
        runtime_profile: profile.name.clone(),
        tools,
    }
}

fn write_tool_status_artifacts(
    out: &Path,
    config: &Config,
    profile: &Profile,
    plan: &Plan,
) -> Result<()> {
    let artifact = tool_status_artifact(out, config, profile, plan);
    let bytes = serde_json::to_vec_pretty(&artifact)?;
    fs::write(out.join("tool-status.json"), &bytes)?;
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    fs::write(review_dir.join("tool-status.json"), bytes)?;
    Ok(())
}

fn tool_status_artifact(
    out: &Path,
    config: &Config,
    profile: &Profile,
    plan: &Plan,
) -> ToolStatusArtifact {
    let plan_by_id = plan
        .sensors
        .iter()
        .map(|sensor| (sensor.id.as_str(), sensor))
        .collect::<BTreeMap<_, _>>();
    let tools = config
        .tools
        .values()
        .map(|tool| {
            let planned = plan_by_id.get(tool.id.as_str());
            let receipt = planned.and_then(|sensor| {
                let receipt_path = out
                    .join("sensors")
                    .join(&sensor.id)
                    .join("ub-review-sensor-status.json");
                read_sensor_receipt(&receipt_path)
            });
            ToolStatusEntry {
                id: tool.id.clone(),
                class: planned.map(|sensor| sensor.class).unwrap_or(tool.class),
                command: tool.command.clone(),
                required_if: tool.default,
                required_reason: trigger_description(tool.default).to_owned(),
                runtime_profile: profile.name.clone(),
                planned_run: planned.is_some_and(|sensor| sensor.run),
                status: receipt
                    .as_ref()
                    .map(|receipt| receipt.status.clone())
                    .unwrap_or_else(|| {
                        if planned.is_some() {
                            "receipt_absent".to_owned()
                        } else {
                            "not_planned".to_owned()
                        }
                    }),
                reason: receipt
                    .as_ref()
                    .map(|receipt| receipt.reason.clone())
                    .or_else(|| planned.map(|sensor| sensor.reason.clone()))
                    .unwrap_or_else(|| "not present in resolved plan".to_owned()),
                exit_code: receipt.as_ref().and_then(|receipt| receipt.exit_code),
                timed_out: receipt.as_ref().is_some_and(|receipt| receipt.timed_out),
                artifact_paths: tool_artifact_paths(&tool.id),
            }
        })
        .collect();
    ToolStatusArtifact {
        schema: "ub-review.tool_status.v1",
        runtime_profile: profile.name.clone(),
        tools,
    }
}

fn tool_artifact_paths(id: &str) -> Vec<String> {
    let sensor = SensorPlan {
        id: id.to_owned(),
        command: id.to_owned(),
        run: false,
        reason: String::new(),
        timeout_sec: 0,
        class: ToolClass::Static,
        weight: 0,
        requires_lease: false,
    };
    let mut paths = vec![format!("sensors/{id}/ub-review-sensor-status.json")];
    paths.extend(
        sensor_outputs(&sensor)
            .into_iter()
            .map(|output| format!("sensors/{id}/{output}")),
    );
    paths
}

fn trigger_description(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Always => "every review run",
        Trigger::SourceChanged => "source file changed",
        Trigger::RustBehaviorOrTestsChanged => "Rust behavior or tests changed",
        Trigger::UnsafeOrNativeRiskChanged => "unsafe/native-risk surface changed",
        Trigger::WorkflowChanged => "workflow or action file changed",
        Trigger::DependencyChanged => "dependency manifest or lockfile changed",
        Trigger::ShellChanged => "shell or script file changed",
        Trigger::CppChanged => "C/C++ file changed",
        Trigger::Diff => "diff-scoped advisory scan",
        Trigger::Manual => "manual proof request",
        Trigger::Never => "disabled unless explicitly selected",
    }
}

fn resolved_plan_artifact(
    config: &Config,
    profile: &Profile,
    diff: &DiffContext,
    plan: &Plan,
    run_args: Option<&RunArgs>,
    selectors: &SelectorArgs,
    effective_model_lanes: Option<&[LanePlan]>,
) -> serde_json::Value {
    serde_json::json!({
        "schema": "ub-review.resolved_plan.v1",
        "base": &plan.base,
        "head": &plan.head,
        "diff_class": diff.diff_class.key(),
        "review_profile": &config.review_profile,
        "profile_name": &plan.profile_name,
        "runtime_profile": &profile.name,
        "budgets": &profile.budgets,
        "trusted_repo": &profile.trusted_repo,
        "guards": &profile.guards,
        "limits": &profile.limits,
        "posting": &config.review,
        "review_body": &config.review_body,
        "selectors": resolved_selector_artifact(run_args, selectors, effective_model_lanes),
        "sensors": &plan.sensors,
        "lanes": &plan.lanes,
        "notes": &plan.notes,
    })
}

fn resolved_selector_artifact(
    run_args: Option<&RunArgs>,
    selectors: &SelectorArgs,
    effective_model_lanes: Option<&[LanePlan]>,
) -> serde_json::Value {
    let lane_include = selector_values_or_empty(&selectors.lanes);
    let lane_exclude = selector_values_or_empty(&selectors.except_lanes);
    let tool_include = selector_values_or_empty(&selectors.tools);
    let tool_exclude = selector_values_or_empty(&selectors.except_tools);
    let effective_lanes = effective_model_lanes
        .map(|lanes| lanes.iter().map(|lane| lane.id.clone()).collect::<Vec<_>>())
        .unwrap_or_default();
    if let Some(args) = run_args {
        serde_json::json!({
            "depth": args.depth.key(),
            "lane_width": args.lane_width,
            "model_concurrency": args.model_concurrency,
            "max_model_calls": args.max_model_calls,
            "max_inline_comments": args.max_inline_comments,
            "lanes": lane_include,
            "except_lanes": lane_exclude,
            "tools": tool_include,
            "except_tools": tool_exclude,
            "effective_model_lanes": effective_lanes,
        })
    } else {
        serde_json::json!({
            "depth": ReviewDepth::Standard.key(),
            "lane_width": STANDARD_LANE_WIDTH,
            "model_concurrency": STANDARD_MODEL_CONCURRENCY,
            "max_model_calls": STANDARD_MAX_MODEL_CALLS,
            "max_inline_comments": 8,
            "lanes": lane_include,
            "except_lanes": lane_exclude,
            "tools": tool_include,
            "except_tools": tool_exclude,
            "effective_model_lanes": effective_lanes,
            "source": "plan-default",
        })
    }
}

fn validate_selector_syntax(selectors: &SelectorArgs) -> Result<()> {
    parse_selector_set(&selectors.lanes, "--lanes")?;
    parse_selector_set(&selectors.except_lanes, "--except-lanes")?;
    parse_selector_set(&selectors.tools, "--tools")?;
    parse_selector_set(&selectors.except_tools, "--except-tools")?;
    Ok(())
}

fn selector_values_or_empty(value: &str) -> Vec<String> {
    let mut values = value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

fn parse_selector_set(value: &str, flag: &str) -> Result<BTreeSet<String>> {
    let mut selected = BTreeSet::new();
    for item in value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        if !is_selector_id(item) {
            bail!("{flag} contains invalid selector id `{item}`");
        }
        selected.insert(item.to_owned());
    }
    Ok(selected)
}

fn is_selector_id(value: &str) -> bool {
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

fn apply_plan_selectors(plan: &mut Plan, selectors: &SelectorArgs) -> Result<()> {
    let tool_include = parse_selector_set(&selectors.tools, "--tools")?;
    let tool_exclude = parse_selector_set(&selectors.except_tools, "--except-tools")?;
    if !tool_include.is_empty() || !tool_exclude.is_empty() {
        plan.sensors = filter_sensor_plans(
            std::mem::take(&mut plan.sensors),
            &tool_include,
            &tool_exclude,
        )?;
        plan.notes.push(format!(
            "tool selectors applied: tools=[{}] except-tools=[{}]",
            tool_include
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(","),
            tool_exclude
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    let lane_include = parse_selector_set(&selectors.lanes, "--lanes")?;
    let lane_exclude = parse_selector_set(&selectors.except_lanes, "--except-lanes")?;
    if !lane_include.is_empty() || !lane_exclude.is_empty() {
        plan.notes.push(format!(
            "lane selectors will filter model assignments: lanes=[{}] except-lanes=[{}]",
            lane_include
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(","),
            lane_exclude
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    Ok(())
}

fn filter_sensor_plans(
    sensors: Vec<SensorPlan>,
    include: &BTreeSet<String>,
    exclude: &BTreeSet<String>,
) -> Result<Vec<SensorPlan>> {
    validate_known_selectors(
        "tool",
        sensors.iter().map(|sensor| sensor.id.as_str()),
        include,
    )?;
    validate_known_selectors(
        "tool",
        sensors.iter().map(|sensor| sensor.id.as_str()),
        exclude,
    )?;
    Ok(sensors
        .into_iter()
        .filter(|sensor| include.is_empty() || include.contains(&sensor.id))
        .filter(|sensor| !exclude.contains(&sensor.id))
        .collect())
}

fn filter_lane_plans(
    lanes: Vec<LanePlan>,
    include: &BTreeSet<String>,
    exclude: &BTreeSet<String>,
) -> Result<Vec<LanePlan>> {
    validate_known_selectors("lane", lanes.iter().map(|lane| lane.id.as_str()), include)?;
    validate_known_selectors("lane", lanes.iter().map(|lane| lane.id.as_str()), exclude)?;
    Ok(lanes
        .into_iter()
        .filter(|lane| include.is_empty() || include.contains(&lane.id))
        .filter(|lane| !exclude.contains(&lane.id))
        .collect())
}

fn validate_known_selectors<'a>(
    kind: &str,
    available: impl Iterator<Item = &'a str>,
    selected: &BTreeSet<String>,
) -> Result<()> {
    if selected.is_empty() {
        return Ok(());
    }
    let available = available.collect::<BTreeSet<_>>();
    let unknown = selected
        .iter()
        .filter(|item| !available.contains(item.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        bail!(
            "unknown {kind} selector(s): {}; available: {}",
            unknown.join(","),
            available.into_iter().collect::<Vec<_>>().join(",")
        );
    }
    Ok(())
}

fn print_plan(plan: &Plan, box_state: &BoxState) {
    println!("Profile: {}", plan.profile_name);
    println!("Diff class: {}", plan.diff_class.key());
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
        let diff_class = classify_diff_class(&changed_files, &flags);
        Ok(Self {
            base: base.to_owned(),
            head: head.to_owned(),
            changed_files,
            patch,
            flags,
            diff_class,
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

fn git_text_owned(root: &Path, args: &[String]) -> Result<String> {
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
        flags.unsafe_or_native_risk |= is_native_risk_path(&lower);
    }
    if patch_tokens_can_promote_native_risk(files, &flags)
        && patch_contains_native_risk_token(patch)
    {
        flags.unsafe_or_native_risk = true;
    }
    flags
}

fn classify_diff_class(files: &[String], flags: &DiffFlags) -> DiffClass {
    if files.is_empty() {
        return DiffClass::ArtifactOnlySmoke;
    }
    if flags.docs_only {
        return DiffClass::DocsOnly;
    }
    if files.iter().all(|path| is_workflow_tooling_path(path)) {
        return DiffClass::WorkflowTooling;
    }
    if files.iter().all(|path| is_test_or_fixture_path(path)) {
        return DiffClass::TestsOnly;
    }
    if flags.unsafe_or_native_risk {
        return DiffClass::SourceUb;
    }
    DiffClass::SourceGeneral
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

fn is_zig_path(path: &str) -> bool {
    path.ends_with(".zig")
}

fn is_native_risk_path(path: &str) -> bool {
    path.contains("ffi")
        || path.contains("jsc")
        || path.contains("arraybuffer")
        || path.contains("typedarray")
        || path.contains("worker")
        || path.contains("crypto")
        || path.contains("zstd")
        || path.contains("src/runtime/")
        || path.contains("src/bun.js/bindings/")
}

fn patch_tokens_can_promote_native_risk(files: &[String], flags: &DiffFlags) -> bool {
    flags.rust_changed
        || flags.cpp_changed
        || files.iter().any(|path| {
            let lower = path.to_ascii_lowercase();
            is_zig_path(&lower) || is_native_risk_path(&lower)
        })
}

fn patch_contains_native_risk_token(patch: &str) -> bool {
    let lower_patch = patch.to_ascii_lowercase();
    [
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
    ]
    .iter()
    .any(|token| lower_patch.contains(token))
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

fn is_workflow_tooling_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.starts_with(".github/workflows/")
        || lower.starts_with(".github/actions/")
        || lower.ends_with("action.yml")
        || lower.ends_with("action.yaml")
        || lower.starts_with("scripts/")
        || lower.starts_with("configs/")
        || matches!(
            lower.as_str(),
            "justfile"
                | "makefile"
                | "dockerfile"
                | ".github/dependabot.yml"
                | ".github/dependabot.yaml"
        )
}

fn is_test_or_fixture_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.contains("/test/")
        || lower.contains("/tests/")
        || lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.contains("/fixtures/")
        || lower.starts_with("fixtures/")
        || lower.ends_with(".snap")
        || lower.ends_with(".test.ts")
        || lower.ends_with(".test.js")
        || lower.ends_with("_test.rs")
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
    if matches!(
        profile.name.as_str(),
        "gh-runner" | "gh-runner-standard" | "gh-runner-full"
    ) {
        notes.push(format!(
            "{} profile: trusted repos get opened and ready_for_review evidence passes, 30m target, 60m hard timeout",
            profile.name
        ));
    }
    Plan {
        base: diff.base.clone(),
        head: diff.head.clone(),
        profile_name: profile.name.clone(),
        diff_class: diff.diff_class,
        sensors,
        lanes: if config.review.enable_default_lanes {
            default_lanes_for_diff_class(diff.diff_class)
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

fn run_sensors(
    root: &Path,
    out: &Path,
    plan: &Plan,
    profile: &Profile,
    event_log: &EventLog,
) -> Result<()> {
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
    let jobs = sensor_job_count(profile, runnable.len())?;
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

fn sensor_job_count(profile: &Profile, runnable_len: usize) -> Result<usize> {
    if profile.limits.sensor_jobs == 0 {
        bail!(
            "runtime profile {} has sensor_jobs=0; sensors cannot be scheduled",
            profile.name
        );
    }
    Ok(profile.limits.sensor_jobs.min(runnable_len))
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
    if sensor.id == "tokmd" {
        return run_tokmd_sensor(root, out, &dir, sensor, event_log, plan, &argv);
    }
    let stdout_path = dir.join("stdout.txt");
    let stderr_path = dir.join("stderr.txt");
    let result = run_command_to_files(
        root,
        &argv,
        &BTreeMap::new(),
        sensor.timeout_sec,
        &stdout_path,
        &stderr_path,
    );
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

fn run_tokmd_sensor(
    root: &Path,
    out: &Path,
    dir: &Path,
    sensor: &SensorPlan,
    event_log: &EventLog,
    plan: &Plan,
    aggregate_argv: &[String],
) -> Result<()> {
    let commands = build_tokmd_sensor_commands(root, dir, plan);
    fs::write(
        dir.join("commands.json"),
        serde_json::to_vec_pretty(&commands_json(&commands))?,
    )?;
    let aggregate_stdout_path = dir.join("stdout.txt");
    let aggregate_stderr_path = dir.join("stderr.txt");
    fs::write(&aggregate_stdout_path, b"")?;
    fs::write(&aggregate_stderr_path, b"")?;

    let started = Instant::now();
    let mut failures = Vec::new();
    let mut timed_out = false;
    let mut exit_code = Some(0);
    for command in &commands {
        event_log.append(
            "sensor_subcommand_started",
            serde_json::json!({"sensor": sensor.id, "label": command.label, "argv": command.argv}),
        )?;
        let result = run_command_to_files(
            root,
            &command.argv,
            &BTreeMap::new(),
            sensor.timeout_sec,
            &command.stdout_path,
            &command.stderr_path,
        );
        match result {
            Ok(result) => {
                append_file(
                    &aggregate_stdout_path,
                    &format!(
                        "$ {}\nstatus={} duration_ms={}\n\n",
                        display_command(&command.argv),
                        result.reason,
                        result.duration_ms
                    ),
                )?;
                append_existing_file(&aggregate_stdout_path, &command.stdout_path)?;
                append_file(&aggregate_stdout_path, "\n")?;
                append_existing_file(&aggregate_stderr_path, &command.stderr_path)?;
                if result.timed_out {
                    timed_out = true;
                }
                if !result.success {
                    if exit_code == Some(0) {
                        exit_code = result.exit_code;
                    }
                    failures.push(format!("{} {}", command.label, result.reason));
                }
                event_log.append(
                    if result.success {
                        "sensor_subcommand_completed"
                    } else {
                        "sensor_subcommand_failed"
                    },
                    serde_json::json!({"sensor": sensor.id, "label": command.label, "exit_code": result.exit_code, "timed_out": result.timed_out, "reason": result.reason}),
                )?;
            }
            Err(err) => {
                let reason = format!("{err:#}");
                failures.push(format!("{} {reason}", command.label));
                if exit_code == Some(0) {
                    exit_code = None;
                }
                event_log.append(
                    "sensor_subcommand_failed",
                    serde_json::json!({"sensor": sensor.id, "label": command.label, "reason": reason}),
                )?;
            }
        }
    }

    let duration_ms = started.elapsed().as_millis();
    let context_path = dir.join("context.md");
    if !context_path.exists() {
        fs::write(
            &context_path,
            "No existing changed paths were available for bounded tokmd context.\n",
        )?;
    }

    let (status, reason) = if failures.is_empty() {
        ("ok", format!("{} tokmd receipts completed", commands.len()))
    } else if timed_out {
        (
            "timed_out",
            format!(
                "tokmd subcommands timed out or failed: {}",
                failures.join("; ")
            ),
        )
    } else {
        (
            "failed",
            format!("tokmd subcommands failed: {}", failures.join("; ")),
        )
    };
    write_sensor_status(
        out,
        sensor,
        SensorStatusWrite {
            status,
            argv: aggregate_argv,
            duration_ms,
            reason: &reason,
            exit_code,
            timed_out,
        },
    )?;
    event_log.append(
        if failures.is_empty() {
            "sensor_completed"
        } else {
            "sensor_failed"
        },
        serde_json::json!({"sensor": sensor.id, "reason": reason}),
    )?;
    Ok(())
}

fn build_sensor_argv(root: &Path, dir: &Path, sensor: &SensorPlan, plan: &Plan) -> Vec<String> {
    match sensor.id.as_str() {
        "tokmd" => vec![
            "tokmd".to_owned(),
            "bundle".to_owned(),
            "analyze".to_owned(),
            "cockpit".to_owned(),
            "context".to_owned(),
            "--base".to_owned(),
            plan.base.clone(),
            "--head".to_owned(),
            plan.head.clone(),
            "--out".to_owned(),
            dir.display().to_string(),
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

fn build_tokmd_sensor_commands(root: &Path, dir: &Path, plan: &Plan) -> Vec<SensorSubcommand> {
    let absolute_dir = absolute_path(dir);
    let mut commands = vec![
        SensorSubcommand {
            label: "analyze-md".to_owned(),
            argv: vec![
                "tokmd".to_owned(),
                "analyze".to_owned(),
                "--preset".to_owned(),
                "estimate".to_owned(),
                "--effort-base-ref".to_owned(),
                plan.base.clone(),
                "--effort-head-ref".to_owned(),
                plan.head.clone(),
                "--format".to_owned(),
                "md".to_owned(),
                "--no-progress".to_owned(),
                ".".to_owned(),
            ],
            stdout_path: dir.join("analyze.md"),
            stderr_path: dir.join("analyze.stderr.txt"),
        },
        SensorSubcommand {
            label: "analyze-json".to_owned(),
            argv: vec![
                "tokmd".to_owned(),
                "analyze".to_owned(),
                "--preset".to_owned(),
                "estimate".to_owned(),
                "--effort-base-ref".to_owned(),
                plan.base.clone(),
                "--effort-head-ref".to_owned(),
                plan.head.clone(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-progress".to_owned(),
                ".".to_owned(),
            ],
            stdout_path: dir.join("analyze.json"),
            stderr_path: dir.join("analyze-json.stderr.txt"),
        },
        SensorSubcommand {
            label: "cockpit-md".to_owned(),
            argv: vec![
                "tokmd".to_owned(),
                "cockpit".to_owned(),
                "--base".to_owned(),
                plan.base.clone(),
                "--head".to_owned(),
                plan.head.clone(),
                "--format".to_owned(),
                "md".to_owned(),
                "--no-progress".to_owned(),
            ],
            stdout_path: dir.join("cockpit.md"),
            stderr_path: dir.join("cockpit.stderr.txt"),
        },
        SensorSubcommand {
            label: "cockpit-json".to_owned(),
            argv: vec![
                "tokmd".to_owned(),
                "cockpit".to_owned(),
                "--base".to_owned(),
                plan.base.clone(),
                "--head".to_owned(),
                plan.head.clone(),
                "--format".to_owned(),
                "json".to_owned(),
                "--no-progress".to_owned(),
            ],
            stdout_path: dir.join("cockpit.json"),
            stderr_path: dir.join("cockpit-json.stderr.txt"),
        },
    ];
    let context_paths = changed_paths_for_tokmd_context(root, plan);
    if !context_paths.is_empty() {
        let mut argv = vec![
            "tokmd".to_owned(),
            "context".to_owned(),
            "--budget".to_owned(),
            "64k".to_owned(),
            "--mode".to_owned(),
            "bundle".to_owned(),
            "--output".to_owned(),
            absolute_dir.join("context.md").display().to_string(),
            "--force".to_owned(),
            "--no-progress".to_owned(),
        ];
        argv.extend(context_paths);
        commands.push(SensorSubcommand {
            label: "context-md".to_owned(),
            argv,
            stdout_path: dir.join("context.stdout.txt"),
            stderr_path: dir.join("context.stderr.txt"),
        });
    }
    commands
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn changed_paths_for_tokmd_context(root: &Path, plan: &Plan) -> Vec<String> {
    git_lines(
        root,
        &[
            "diff",
            "--name-only",
            &format!("{}...{}", plan.base, plan.head),
        ],
    )
    .or_else(|_| git_lines(root, &["diff", "--name-only", &plan.base, &plan.head]))
    .unwrap_or_default()
    .into_iter()
    .filter(|path| root.join(path).is_file())
    .take(40)
    .collect()
}

fn commands_json(commands: &[SensorSubcommand]) -> serde_json::Value {
    serde_json::Value::Array(
        commands
            .iter()
            .map(|command| {
                serde_json::json!({
                    "label": command.label,
                    "command": display_command(&command.argv),
                    "stdout": command.stdout_path.display().to_string(),
                    "stderr": command.stderr_path.display().to_string(),
                })
            })
            .collect(),
    )
}

fn append_file(path: &Path, text: &str) -> Result<()> {
    use std::io::Write as _;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("open {}", path.display()))?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn append_existing_file(target: &Path, source: &Path) -> Result<()> {
    if !source.exists() {
        return Ok(());
    }
    let text = fs::read_to_string(source).unwrap_or_else(|_| String::new());
    if text.is_empty() {
        return Ok(());
    }
    append_file(target, &text)
}

fn run_command_to_files(
    root: &Path,
    argv: &[String],
    env: &BTreeMap<String, String>,
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
        .envs(env)
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
        "tokmd" => outputs.extend([
            "commands.json".to_owned(),
            "analyze.md".to_owned(),
            "analyze.json".to_owned(),
            "cockpit.md".to_owned(),
            "cockpit.json".to_owned(),
            "context.md".to_owned(),
        ]),
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
        text.push_str(review_posture_for_diff_class(diff.diff_class));
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
    args: &RunArgs,
    event_log: &EventLog,
    run_started: &Instant,
    run_loop_tracker: &mut RunLoopTracker,
    elapsed: Duration,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    let pr_thread_context = collect_pr_thread_context(root, args)?;
    let shared_context = render_shared_context(
        root,
        out,
        config,
        diff,
        plan,
        running_summary,
        args,
        &pr_thread_context,
    )?;
    fs::write(review_dir.join("shared_context.md"), &shared_context)?;
    fs::write(
        review_dir.join("pr_thread_context.json"),
        serde_json::to_vec_pretty(&pr_thread_context)?,
    )?;
    let shared_context_id = sha256_hex(shared_context.as_bytes());
    let line_map = right_side_diff_lines(&diff.patch);
    let assignments = model_assignments(plan, args)?;
    let mut provider_preflights = build_provider_preflight_receipts(&assignments, args);
    let mut model_lanes = build_model_lane_receipts(&assignments, args);
    let missing_or_failed_sensor_evidence = collect_sensor_evidence_issues(out, plan);
    let mut missing_or_failed_model_evidence = model_lanes
        .iter()
        .filter(|receipt| is_model_receipt_evidence_issue(receipt))
        .map(model_issue_from_receipt)
        .collect::<Vec<_>>();
    let mut summary_only_findings = Vec::new();
    let mut inline_comments = Vec::new();
    let mut model_observations = Vec::new();
    let mut proof_requests = Vec::new();
    let mut model_calls_used = 0usize;

    let model_loop = start_run_loop(event_log, run_started, "model", "investigation", "primary")?;
    if matches!(args.model_mode, ModelMode::Auto) {
        run_provider_preflights(root, &review_dir, &mut provider_preflights, args)?;
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
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
        )?;
        dedupe_inline_comments(&mut inline_comments, &mut summary_only_findings);
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
    }
    let model_loop_status = if matches!(args.model_mode, ModelMode::Auto) {
        "completed"
    } else {
        "skipped_model_mode_off"
    };
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        model_loop,
        model_loop_status,
    )?;

    let profile = config.selected_profile()?;
    let proof_loop = start_run_loop(
        event_log,
        run_started,
        "proof",
        "proof",
        "planner-and-broker",
    )?;
    write_proof_planner_artifacts(
        out,
        diff,
        profile,
        box_state,
        &pr_thread_context,
        &proof_requests,
    )?;
    let proof_result = run_proof_broker_v0(root, out, diff, profile, &proof_requests, args)?;
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        proof_loop,
        "completed",
    )?;
    let proof_receipts = proof_result.proof_receipts;
    let resource_leases = proof_result.resource_leases;
    let compiler_loop = start_run_loop(
        event_log,
        run_started,
        "compiler",
        "coordination",
        "preliminary",
    )?;
    let candidates = build_candidate_records(&inline_comments, &summary_only_findings);
    write_candidate_artifacts(out, &candidates)?;
    let candidates = read_candidate_records(out)?;
    let (inline_comments, summary_only_findings) = read_candidate_review_surfaces(out)?;

    let preliminary_surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: &shared_context_id,
        review_body_policy: &config.review_body,
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
    })?;
    let mut review = ReviewArtifacts {
        shared_context_id,
        review_profile: config.review_profile.clone(),
        mode: args.mode.key().to_owned(),
        posting: args.posting.key().to_owned(),
        runtime_profile: profile.name.clone(),
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
    );
    write_observation_artifacts(out, &observations)?;
    write_orchestrator_artifacts(out, &orchestrator_plan)?;
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
    run_follow_up_model_pass(
        FollowUpRunContext {
            root,
            out,
            review_dir: &review_dir,
            provider_preflights: &review.provider_preflights,
            args,
            model_calls_used,
            tasks: &orchestrator_plan.follow_up_tasks,
            line_map: &line_map,
        },
        &mut follow_up_results,
        &mut follow_up_outputs,
    )?;
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        follow_up_model_loop,
        "completed",
    )?;
    write_follow_up_result_artifacts(out, &follow_up_results)?;
    write_follow_up_output_artifacts(out, &follow_up_outputs)?;
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
    )?;
    review
        .proof_receipts
        .extend(follow_up_proof_result.proof_receipts);
    review
        .resource_leases
        .extend(follow_up_proof_result.resource_leases);
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        follow_up_proof_loop,
        "completed",
    )?;
    let final_compiler_loop =
        start_run_loop(event_log, run_started, "compiler", "coordination", "final")?;
    let mut compiler_summary_only_findings = review.summary_only_findings.clone();
    compiler_summary_only_findings.extend(follow_up_evidence.summary_only_findings.clone());
    let mut compiler_observations = review.observations.clone();
    compiler_observations.extend(follow_up_evidence.observations.clone());
    let final_surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: &review.shared_context_id,
        review_body_policy: &config.review_body,
        args,
        plan,
        diff,
        model_lanes: &review.model_lanes,
        missing_or_failed_sensor_evidence: &review.missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence: &review.missing_or_failed_model_evidence,
        inline_comments: &review.inline_comments,
        summary_only_findings: &compiler_summary_only_findings,
        observations: &compiler_observations,
        proof_receipts: &review.proof_receipts,
    })?;
    let review_payload_status = final_surface.review_payload_status;
    let should_prepare_github_review = final_surface.should_prepare_github_review;
    let github_review = final_surface.github_review;
    let artifact_body = final_surface.artifact_body;
    let terminal_state = final_surface.terminal_state;
    review.terminal_state = terminal_state.clone();
    review.body = artifact_body.clone();
    let mut witnesses = build_witness_records(
        &review.inline_comments,
        &review.summary_only_findings,
        &observations,
        &review.proof_receipts,
    );
    append_follow_up_evidence_witnesses(&mut witnesses, &follow_up_evidence);
    write_witness_artifacts(out, &witnesses)?;
    write_proof_receipt_artifacts(out, &review.proof_receipts)?;
    write_resource_lease_artifacts(out, &review.resource_leases)?;
    write_proof_request_artifacts(
        out,
        diff,
        profile,
        &review.proof_requests,
        &review.proof_receipts,
    )?;
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        final_compiler_loop,
        "completed",
    )?;
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
        run: run_loop_metrics,
        elapsed,
    });

    fs::write(
        review_dir.join("review.json"),
        serde_json::to_vec_pretty(&review)?,
    )?;
    fs::write(
        review_dir.join("metrics.json"),
        serde_json::to_vec_pretty(&metrics)?,
    )?;
    fs::write(
        review_dir.join("terminal_state.json"),
        serde_json::to_vec_pretty(&review.terminal_state)?,
    )?;
    fs::write(
        review_dir.join("provider-preflight-status.json"),
        serde_json::to_vec_pretty(&review.provider_preflights)?,
    )?;
    fs::write(review_dir.join("review.md"), artifact_body)?;
    if should_prepare_github_review {
        write_github_review_payload(&review_dir, &github_review, &line_map, &config.review_body)?;
    } else {
        write_github_review_skip_receipt(
            &review_dir,
            build_github_review_skip_receipt(args, &review),
        )?;
    }
    Ok(())
}

fn should_prepare_github_review_payload(
    args: &RunArgs,
    inline_comments: &[ReviewInlineComment],
    _summary_only_findings: &[SummaryOnlyFinding],
    proof_receipts: &[ProofReceipt],
    pr_body: &str,
) -> bool {
    if matches!(args.model_mode, ModelMode::Off) {
        return false;
    }
    if has_reviewer_value(inline_comments, pr_body) {
        return true;
    }
    if proof_receipts
        .iter()
        .any(proof_receipt_changes_review_value)
    {
        return true;
    }
    pr_body_has_reviewer_value(pr_body)
}

fn pr_body_has_reviewer_value(body: &str) -> bool {
    [
        "## Confirmed findings",
        "## Findings",
        "## Verification questions",
        "## Test proof",
        "## Proof results",
        "## Refuted",
        "## Residual risk",
        "## Parked follow-ups",
        "## Evidence gaps",
        "## Missing evidence",
    ]
    .iter()
    .any(|heading| body.contains(heading))
}

struct ReviewCompilerInput<'a> {
    shared_context_id: &'a str,
    review_body_policy: &'a ReviewBodyPolicy,
    args: &'a RunArgs,
    plan: &'a Plan,
    diff: &'a DiffContext,
    model_lanes: &'a [ModelLaneReceipt],
    missing_or_failed_sensor_evidence: &'a [SensorEvidenceIssue],
    missing_or_failed_model_evidence: &'a [ModelEvidenceIssue],
    inline_comments: &'a [ReviewInlineComment],
    summary_only_findings: &'a [SummaryOnlyFinding],
    observations: &'a [Observation],
    proof_receipts: &'a [ProofReceipt],
}

struct CompiledReviewSurface {
    artifact_body: String,
    github_review: GitHubReview,
    should_prepare_github_review: bool,
    review_payload_status: &'static str,
    terminal_state: ReviewTerminalState,
}

fn compile_review_surface(input: ReviewCompilerInput<'_>) -> Result<CompiledReviewSurface> {
    let artifact_body = render_review_body(
        input.shared_context_id,
        input.plan,
        input.diff,
        input.model_lanes,
        input.missing_or_failed_sensor_evidence,
        input.missing_or_failed_model_evidence,
        input.inline_comments,
        input.summary_only_findings,
        input.observations,
        input.proof_receipts,
        input.args.review_body_max_bytes,
        ReviewBodyAudience::Artifact,
    );
    let pr_body = render_review_body(
        input.shared_context_id,
        input.plan,
        input.diff,
        input.model_lanes,
        input.missing_or_failed_sensor_evidence,
        input.missing_or_failed_model_evidence,
        input.inline_comments,
        input.summary_only_findings,
        input.observations,
        input.proof_receipts,
        input.args.review_body_max_bytes,
        ReviewBodyAudience::PullRequest,
    );
    validate_pr_review_body_policy(&pr_body, input.review_body_policy)
        .with_context(|| "validate pull request review body policy")?;
    let github_review = GitHubReview {
        event: "COMMENT".to_owned(),
        body: pr_body.clone(),
        comments: input
            .inline_comments
            .iter()
            .map(|comment| GitHubReviewComment {
                path: comment.path.clone(),
                line: comment.line,
                side: comment.side.clone(),
                body: comment.body.clone(),
            })
            .collect(),
    };
    let should_prepare_github_review = should_prepare_github_review_payload(
        input.args,
        input.inline_comments,
        input.summary_only_findings,
        input.proof_receipts,
        &pr_body,
    );
    let review_payload_status = if should_prepare_github_review {
        "prepared"
    } else {
        "skipped_empty_smoke"
    };
    let terminal_state = build_review_terminal_state(TerminalStateInput {
        args: input.args,
        plan: input.plan,
        review_payload_status,
        should_prepare_github_review,
        pr_body: &pr_body,
        inline_comments: input.inline_comments,
        summary_only_findings: input.summary_only_findings,
        model_lanes: input.model_lanes,
        missing_or_failed_sensor_evidence: input.missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence: input.missing_or_failed_model_evidence,
        proof_receipts: input.proof_receipts,
    });
    Ok(CompiledReviewSurface {
        artifact_body,
        github_review,
        should_prepare_github_review,
        review_payload_status,
        terminal_state,
    })
}

fn validate_pr_review_body_policy(body: &str, policy: &ReviewBodyPolicy) -> Result<()> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if has_forbidden_pr_review_boilerplate(trimmed) {
        bail!("github review body contains artifact-only boilerplate");
    }
    if !policy.include_successful_lane_table && contains_successful_lane_table(trimmed) {
        bail!("github review body contains successful lane table");
    }
    match policy.include_provider_table {
        ReviewBodyTablePolicy::Always => {}
        ReviewBodyTablePolicy::Never | ReviewBodyTablePolicy::OnFailure => {
            if contains_provider_status_table(trimmed) {
                bail!("github review body contains provider status table");
            }
        }
    }
    match policy.include_sensor_table {
        ReviewBodyTablePolicy::Always => {}
        ReviewBodyTablePolicy::Never | ReviewBodyTablePolicy::OnFailure => {
            if contains_sensor_status_table(trimmed) {
                bail!("github review body contains sensor status table");
            }
        }
    }
    match policy.include_execution_summary {
        ReviewBodyExecutionSummaryPolicy::Always => {}
        ReviewBodyExecutionSummaryPolicy::None => {
            if contains_execution_summary(trimmed) {
                bail!("github review body contains execution summary");
            }
        }
        ReviewBodyExecutionSummaryPolicy::OnFailure => {
            if contains_execution_summary(trimmed) && !pr_body_has_failure_context(trimmed) {
                bail!("github review body contains success execution summary");
            }
        }
    }
    Ok(())
}

fn has_forbidden_pr_review_boilerplate(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    [
        "no blocking finding after",
        "no blocking ub finding",
        "no actionable findings",
        "a human should still inspect",
        "lane transcript",
        "raw observations",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn contains_successful_lane_table(body: &str) -> bool {
    [
        "## Model lanes",
        "## Model lane status",
        "## Lane status",
        "## Lane roster",
    ]
    .iter()
    .any(|needle| body.contains(needle))
}

fn contains_provider_status_table(body: &str) -> bool {
    [
        "## Provider preflights",
        "## Provider status",
        "## Model provider status",
    ]
    .iter()
    .any(|needle| body.contains(needle))
}

fn contains_sensor_status_table(body: &str) -> bool {
    ["## Sensors", "## Sensor status", "## Sensor receipts"]
        .iter()
        .any(|needle| body.contains(needle))
}

fn contains_execution_summary(body: &str) -> bool {
    [
        "- Shared context:",
        "- Profile:",
        "- Base:",
        "- Head:",
        "- Changed files:",
        "- Inline comments:",
        "## Review efficiency",
        "Runtime:",
        "Terminal state:",
        "Review payload:",
        "Follow-up results:",
    ]
    .iter()
    .any(|needle| body.contains(needle))
}

fn pr_body_has_failure_context(body: &str) -> bool {
    [
        "## Decision",
        "## Evidence gaps",
        "## Missing evidence",
        "## Missing or failed evidence",
        "Needs ",
        "failed",
        "timed out",
        "unavailable",
    ]
    .iter()
    .any(|needle| body.contains(needle))
}

struct TerminalStateInput<'a> {
    args: &'a RunArgs,
    plan: &'a Plan,
    review_payload_status: &'a str,
    should_prepare_github_review: bool,
    pr_body: &'a str,
    inline_comments: &'a [ReviewInlineComment],
    summary_only_findings: &'a [SummaryOnlyFinding],
    model_lanes: &'a [ModelLaneReceipt],
    missing_or_failed_sensor_evidence: &'a [SensorEvidenceIssue],
    missing_or_failed_model_evidence: &'a [ModelEvidenceIssue],
    proof_receipts: &'a [ProofReceipt],
}

fn build_review_terminal_state(input: TerminalStateInput<'_>) -> ReviewTerminalState {
    let usable_model_lanes = input
        .model_lanes
        .iter()
        .filter(|receipt| model_lane_is_usable_for_terminal_state(receipt))
        .count();
    let evidence_gaps = input.missing_or_failed_sensor_evidence.len()
        + input.missing_or_failed_model_evidence.len();
    let reviewer_value_present = input.should_prepare_github_review
        || has_reviewer_value(input.inline_comments, input.pr_body)
        || input
            .proof_receipts
            .iter()
            .any(proof_receipt_changes_review_value);

    let (status, reason) = if reviewer_value_present {
        (
            "needs-reviewer-attention",
            "Reviewer-value content survived compilation; a grouped PR review was prepared.",
        )
    } else if input.args.dry_run {
        (
            "artifact-only",
            "Dry run requested; this run produced artifacts but no reviewer-facing review.",
        )
    } else if matches!(input.args.model_mode, ModelMode::Off) {
        (
            "artifact-only",
            "Model mode was off; this run produced artifacts but no reviewer-facing review.",
        )
    } else if input.plan.diff_class == DiffClass::ArtifactOnlySmoke {
        (
            "artifact-only",
            "Artifact-only smoke diff; diagnostics remain in artifacts and no PR review was prepared.",
        )
    } else if usable_model_lanes == 0 && input.proof_receipts.is_empty() {
        (
            "failed-to-review",
            "No usable model lane or proof receipt was available, so the run did not reach a sufficient review state.",
        )
    } else {
        (
            "sufficient",
            "No reviewer-value content survived compilation; the run reached a sufficient terminal state and stayed artifact-only.",
        )
    };

    ReviewTerminalState {
        schema: "ub-review.terminal_state.v1".to_owned(),
        status: status.to_owned(),
        reason: reason.to_owned(),
        review_payload_status: input.review_payload_status.to_owned(),
        reviewer_value_present,
        diff_class: input.plan.diff_class.key().to_owned(),
        model_mode: input.args.model_mode.key().to_owned(),
        usable_model_lanes,
        model_lanes: input.model_lanes.len(),
        evidence_gaps,
        proof_receipts: input.proof_receipts.len(),
        inline_comments: input.inline_comments.len(),
        summary_only_findings: input.summary_only_findings.len(),
    }
}

fn model_lane_is_usable_for_terminal_state(receipt: &ModelLaneReceipt) -> bool {
    matches!(receipt.status.as_str(), "ok" | "degraded")
}

fn write_github_review_payload(
    review_dir: &Path,
    github_review: &GitHubReview,
    right_lines: &BTreeSet<(String, u32)>,
    review_body_policy: &ReviewBodyPolicy,
) -> Result<()> {
    validate_github_review_payload_for_right_lines(
        github_review,
        right_lines,
        "generated diff context",
        review_body_policy,
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
    run: RunLoopMetrics,
    elapsed: Duration,
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
        mut run,
        elapsed,
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
        model_mode: review.model_mode.clone(),
        depth: review.depth.clone(),
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
        github_review_comments: github_review.map_or(0, |review| review.comments.len()),
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
        proof_requests: review.proof_requests.len(),
        proof_receipts: review.proof_receipts.len(),
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

fn model_call_duration_ms_sum(
    review: &ReviewArtifacts,
    follow_up_results: &[FollowUpResult],
) -> u128 {
    review
        .provider_preflights
        .iter()
        .filter_map(|receipt| receipt.duration_ms)
        .chain(
            review
                .model_lanes
                .iter()
                .filter_map(|receipt| receipt.duration_ms),
        )
        .chain(
            follow_up_results
                .iter()
                .filter_map(|result| result.duration_ms),
        )
        .sum()
}

fn proof_command_duration_ms_sum(proof_receipts: &[ProofReceipt]) -> u128 {
    proof_receipts
        .iter()
        .flat_map(|receipt| receipt.commands.iter())
        .map(|command| command.duration_ms)
        .sum()
}

fn combined_observations(review: &ReviewArtifacts) -> Vec<Observation> {
    let mut observations = review.observations.clone();
    observations.extend(build_observations(review));
    for (index, observation) in observations.iter_mut().enumerate() {
        let short = observation
            .fingerprint
            .get(..12)
            .unwrap_or(&observation.fingerprint);
        observation.id = format!("obs-{index:04}-{short}");
    }
    observations
}

fn build_observations(review: &ReviewArtifacts) -> Vec<Observation> {
    let mut observations = Vec::new();
    for comment in &review.inline_comments {
        observations.push(make_observation(ObservationInput {
            index: observations.len(),
            lane: &comment.lane,
            question: &comment.lane,
            claim: &comment.body,
            kind: infer_observation_kind(&comment.lane, &comment.body, &comment.evidence),
            status: "confirmed",
            severity: &comment.severity,
            confidence: &comment.confidence,
            path: Some(&comment.path),
            line: Some(comment.line),
            evidence: vec![comment.evidence.clone()],
            dedupe_key: None,
            source: "inline-comment",
        }));
    }
    for finding in &review.summary_only_findings {
        let parked = is_parked_follow_up(finding);
        let kind = if parked {
            "parked-follow-up"
        } else {
            infer_observation_kind(&finding.lane, &finding.reason, &finding.evidence)
        };
        let status = if parked { "parked" } else { "open" };
        observations.push(make_observation(ObservationInput {
            index: observations.len(),
            lane: &finding.lane,
            question: &finding.lane,
            claim: &finding.reason,
            kind,
            status,
            severity: &finding.severity,
            confidence: &finding.confidence,
            path: None,
            line: None,
            evidence: vec![finding.evidence.clone()],
            dedupe_key: None,
            source: "summary-only-finding",
        }));
    }
    for issue in &review.missing_or_failed_sensor_evidence {
        let claim = format!(
            "Sensor `{}` evidence is `{}`: {}",
            issue.sensor, issue.status, issue.reason
        );
        observations.push(make_observation(ObservationInput {
            index: observations.len(),
            lane: &format!("sensor-{}", issue.sensor),
            question: "missing-sensor-evidence",
            claim: &claim,
            kind: "missing-evidence",
            status: "open",
            severity: "medium",
            confidence: "high",
            path: None,
            line: None,
            evidence: vec![issue.reason.clone()],
            dedupe_key: None,
            source: "missing-sensor-evidence",
        }));
    }
    for issue in &review.missing_or_failed_model_evidence {
        let claim = format!(
            "Lane `{}` via `{}` model `{}` endpoint `{}` is `{}`: {}",
            issue.lane,
            issue.provider,
            issue.model,
            issue.endpoint_kind,
            issue.status,
            issue.reason
        );
        observations.push(make_observation(ObservationInput {
            index: observations.len(),
            lane: &issue.lane,
            question: "missing-model-evidence",
            claim: &claim,
            kind: "missing-evidence",
            status: "open",
            severity: "medium",
            confidence: "high",
            path: None,
            line: None,
            evidence: vec![issue.reason.clone()],
            dedupe_key: None,
            source: "missing-model-evidence",
        }));
    }
    observations
}

struct ObservationInput<'a> {
    index: usize,
    lane: &'a str,
    question: &'a str,
    claim: &'a str,
    kind: &'a str,
    status: &'a str,
    severity: &'a str,
    confidence: &'a str,
    path: Option<&'a String>,
    line: Option<u32>,
    evidence: Vec<String>,
    dedupe_key: Option<&'a str>,
    source: &'a str,
}

fn make_observation(input: ObservationInput<'_>) -> Observation {
    let path = input.path.cloned();
    let fingerprint_input = format!(
        "{}\n{}\n{}\n{}\n{:?}\n{:?}\n{}",
        input.lane,
        input.kind,
        input.status,
        input.claim,
        path,
        input.line,
        input.evidence.join("\n")
    );
    let fingerprint = sha256_hex(fingerprint_input.as_bytes());
    let short = &fingerprint[..12];
    Observation {
        schema: "ub-review.observation.v1".to_owned(),
        id: format!("obs-{index:04}-{short}", index = input.index),
        lane: input.lane.to_owned(),
        question: input.question.to_owned(),
        claim: input.claim.to_owned(),
        kind: input.kind.to_owned(),
        status: input.status.to_owned(),
        severity: input.severity.to_owned(),
        confidence: input.confidence.to_owned(),
        path: path.clone(),
        line: input.line,
        fingerprint,
        evidence: input.evidence,
        dedupe_key: input
            .dedupe_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                observation_dedupe_key(input.lane, input.kind, path.as_deref(), input.line)
            }),
        source: input.source.to_owned(),
    }
}

fn observation_dedupe_key(lane: &str, kind: &str, path: Option<&str>, line: Option<u32>) -> String {
    match (path, line) {
        (Some(path), Some(line)) => format!("{kind}:{path}:{line}"),
        _ => format!("{kind}:{}", sanitize_artifact_name(lane)),
    }
}

fn infer_observation_kind(lane: &str, claim: &str, evidence: &str) -> &'static str {
    let lane = lane.to_ascii_lowercase();
    let text = format!("{claim}\n{evidence}").to_ascii_lowercase();
    if text.contains("missing") || text.contains("unavailable") || text.contains("skipped") {
        "missing-evidence"
    } else if text.contains("parked") || text.contains("follow-up") {
        "parked-follow-up"
    } else if lane.contains("test") || text.contains("test") || text.contains("oracle") {
        "test-gap"
    } else if lane.contains("source-route") || lane.contains("sibling") || text.contains("route") {
        "source-route-gap"
    } else if lane.contains("security") || text.contains("exploit") || text.contains("secret") {
        "security-risk"
    } else if text.contains("verify") || text.contains("confirm") || text.contains("question") {
        "verification-question"
    } else {
        "bug"
    }
}

fn write_observation_artifacts(out: &Path, observations: &[Observation]) -> Result<()> {
    let observations_dir = out.join("observations");
    if observations_dir.exists() {
        fs::remove_dir_all(&observations_dir)
            .with_context(|| format!("remove {}", observations_dir.display()))?;
    }
    fs::create_dir_all(&observations_dir)
        .with_context(|| format!("create {}", observations_dir.display()))?;

    let questions_dir = out.join("questions");
    if questions_dir.exists() {
        fs::remove_dir_all(&questions_dir)
            .with_context(|| format!("remove {}", questions_dir.display()))?;
    }
    fs::create_dir_all(&questions_dir)
        .with_context(|| format!("create {}", questions_dir.display()))?;

    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("observations.json"),
        serde_json::to_vec_pretty(observations)?,
    )?;
    let observation_summary = observation_summary_artifacts(observations);
    fs::write(
        review_dir.join("unique_observations.json"),
        serde_json::to_vec_pretty(&observation_summary.unique)?,
    )?;
    fs::write(
        review_dir.join("merged_observations.json"),
        serde_json::to_vec_pretty(&observation_summary.merged)?,
    )?;
    fs::write(
        review_dir.join("dropped_observations.json"),
        serde_json::to_vec_pretty(&observation_summary.dropped)?,
    )?;

    let mut by_lane: BTreeMap<&str, Vec<&Observation>> = BTreeMap::new();
    let mut by_question: BTreeMap<(String, String), QuestionObservationArtifact<'_>> =
        BTreeMap::new();
    for observation in observations {
        by_lane
            .entry(observation.lane.as_str())
            .or_default()
            .push(observation);
        let lane_name = sanitize_artifact_name(&observation.lane);
        let question_name = sanitize_artifact_name(&observation.question);
        let artifact = by_question
            .entry((lane_name, question_name))
            .or_insert_with(|| QuestionObservationArtifact {
                schema: "ub-review.question_observations.v1",
                lane: &observation.lane,
                question: &observation.question,
                observations: Vec::new(),
            });
        if artifact.lane != observation.lane || artifact.question != observation.question {
            bail!(
                "questions artifact path collision for {}/{}",
                observation.lane,
                observation.question
            );
        }
        artifact.observations.push(observation);
    }
    for (lane, lane_observations) in by_lane {
        let path = observations_dir.join(format!("{}.ndjson", sanitize_artifact_name(lane)));
        let mut text = String::new();
        for observation in lane_observations {
            text.push_str(&serde_json::to_string(observation)?);
            text.push('\n');
        }
        fs::write(path, text)?;
    }
    for ((lane_name, question_name), artifact) in by_question {
        let lane_dir = questions_dir.join(lane_name);
        fs::create_dir_all(&lane_dir).with_context(|| format!("create {}", lane_dir.display()))?;
        fs::write(
            lane_dir.join(format!("{question_name}.json")),
            serde_json::to_vec_pretty(&artifact)?,
        )?;
    }
    Ok(())
}

fn build_witness_records(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    observations: &[Observation],
    proof_receipts: &[ProofReceipt],
) -> Vec<WitnessRecord> {
    let mut witnesses = Vec::new();
    for comment in inline_comments {
        witnesses.push(witness_record(WitnessRecordInput {
            status: "needs-witness",
            kind: "inline-finding",
            source: "inline-comment",
            claim: &comment.body,
            dedupe_key: &format!(
                "inline:{}:{}:{}",
                comment.path,
                comment.line,
                sha256_hex(comment.body.as_bytes())
            ),
            evidence: vec![comment.evidence.clone()],
            lane: Some(comment.lane.clone()),
            path: Some(comment.path.clone()),
            line: Some(comment.line),
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for finding in summary_only_findings {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_summary_finding(finding),
            kind: "summary-finding",
            source: "summary-only-finding",
            claim: &finding.reason,
            dedupe_key: &format!(
                "summary:{}:{}",
                finding.lane,
                sha256_hex(format!("{}\n{}", finding.reason, finding.evidence).as_bytes())
            ),
            evidence: vec![finding.evidence.clone()],
            lane: Some(finding.lane.clone()),
            path: None,
            line: None,
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for observation in observations {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_observation(observation),
            kind: &observation.kind,
            source: &observation.source,
            claim: &observation.claim,
            dedupe_key: &observation.dedupe_key,
            evidence: observation.evidence.clone(),
            lane: Some(observation.lane.clone()),
            path: observation.path.clone(),
            line: observation.line,
            observation_id: Some(observation.id.clone()),
            proof_receipt_id: None,
        }));
    }
    for receipt in proof_receipts {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_proof_receipt(receipt),
            kind: &receipt.kind,
            source: "proof-receipt",
            claim: &receipt.reason,
            dedupe_key: &receipt.id,
            evidence: proof_receipt_witness_evidence(receipt),
            lane: None,
            path: None,
            line: None,
            observation_id: None,
            proof_receipt_id: Some(receipt.id.clone()),
        }));
    }
    witnesses
}

fn append_follow_up_evidence_witnesses(
    witnesses: &mut Vec<WitnessRecord>,
    evidence: &FollowUpEvidenceArtifact,
) {
    for comment in &evidence.inline_comments {
        witnesses.push(witness_record(WitnessRecordInput {
            status: "needs-witness",
            kind: "inline-finding",
            source: "follow-up-inline-comment",
            claim: &comment.body,
            dedupe_key: &format!(
                "follow-up-inline:{}:{}:{}",
                comment.path,
                comment.line,
                sha256_hex(comment.body.as_bytes())
            ),
            evidence: vec![comment.evidence.clone()],
            lane: Some(comment.lane.clone()),
            path: Some(comment.path.clone()),
            line: Some(comment.line),
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for finding in &evidence.summary_only_findings {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_summary_finding(finding),
            kind: "summary-finding",
            source: "follow-up-summary-only-finding",
            claim: &finding.reason,
            dedupe_key: &format!(
                "follow-up-summary:{}:{}",
                finding.lane,
                sha256_hex(format!("{}\n{}", finding.reason, finding.evidence).as_bytes())
            ),
            evidence: vec![finding.evidence.clone()],
            lane: Some(finding.lane.clone()),
            path: None,
            line: None,
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for observation in &evidence.observations {
        witnesses.push(witness_record(WitnessRecordInput {
            status: witness_status_for_observation(observation),
            kind: &observation.kind,
            source: &format!("follow-up-{}", observation.source),
            claim: &observation.claim,
            dedupe_key: &format!("follow-up-observation:{}", observation.dedupe_key),
            evidence: observation.evidence.clone(),
            lane: Some(observation.lane.clone()),
            path: observation.path.clone(),
            line: observation.line,
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
    for request in &evidence.proof_requests {
        witnesses.push(witness_record(WitnessRecordInput {
            status: "needs-witness",
            kind: "proof-request",
            source: "follow-up-proof-request",
            claim: &request.reason,
            dedupe_key: &format!("follow-up-proof-request:{}", request.id),
            evidence: vec![request.command.clone()],
            lane: Some(request.lane.clone()),
            path: None,
            line: None,
            observation_id: None,
            proof_receipt_id: None,
        }));
    }
}

fn build_candidate_records(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
) -> Vec<CandidateRecord> {
    let mut candidates = Vec::new();
    for comment in inline_comments {
        let fingerprint = sha256_hex(
            format!(
                "inline-comment\n{}\n{}\n{}\n{}\n{}",
                comment.lane, comment.path, comment.line, comment.body, comment.evidence
            )
            .as_bytes(),
        );
        candidates.push(CandidateRecord {
            schema: "ub-review.candidate.v1".to_owned(),
            id: format!(
                "candidate-{index:04}-{short}",
                index = candidates.len(),
                short = &fingerprint[..12]
            ),
            lane: comment.lane.clone(),
            source: "inline-comment".to_owned(),
            status: "accepted-inline".to_owned(),
            disposition: "inline".to_owned(),
            severity: comment.severity.clone(),
            confidence: comment.confidence.clone(),
            claim: comment.body.clone(),
            evidence: comment.evidence.clone(),
            path: Some(comment.path.clone()),
            line: Some(comment.line),
            side: Some(comment.side.clone()),
        });
    }
    for finding in summary_only_findings {
        let fingerprint = sha256_hex(
            format!(
                "summary-only-finding\n{}\n{}\n{}",
                finding.lane, finding.reason, finding.evidence
            )
            .as_bytes(),
        );
        candidates.push(CandidateRecord {
            schema: "ub-review.candidate.v1".to_owned(),
            id: format!(
                "candidate-{index:04}-{short}",
                index = candidates.len(),
                short = &fingerprint[..12]
            ),
            lane: finding.lane.clone(),
            source: "summary-only-finding".to_owned(),
            status: "summary-only".to_owned(),
            disposition: candidate_disposition_for_summary_finding(finding).to_owned(),
            severity: finding.severity.clone(),
            confidence: finding.confidence.clone(),
            claim: finding.reason.clone(),
            evidence: finding.evidence.clone(),
            path: None,
            line: None,
            side: None,
        });
    }
    candidates
}

fn candidate_disposition_for_summary_finding(finding: &SummaryOnlyFinding) -> &'static str {
    let reason = finding.reason.to_ascii_lowercase();
    let evidence = finding.evidence.to_ascii_lowercase();
    if is_parked_follow_up(finding) {
        "parked-follow-up"
    } else if reason.contains("false premise")
        || reason.contains("refuted")
        || evidence.contains("false premise")
        || evidence.contains("refuted")
    {
        "refuted"
    } else if reason.contains("duplicate inline candidate merged")
        || reason.contains("summary-only guard rejected candidate")
    {
        "dropped"
    } else {
        "summary-only"
    }
}

fn write_candidate_artifacts(out: &Path, candidates: &[CandidateRecord]) -> Result<()> {
    let candidates_dir = out.join("candidates");
    if candidates_dir.exists() {
        fs::remove_dir_all(&candidates_dir)
            .with_context(|| format!("remove {}", candidates_dir.display()))?;
    }
    fs::create_dir_all(&candidates_dir)
        .with_context(|| format!("create {}", candidates_dir.display()))?;

    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("candidates.json"),
        serde_json::to_vec_pretty(candidates)?,
    )?;

    let mut ndjson = String::new();
    for candidate in candidates {
        ndjson.push_str(&serde_json::to_string(candidate)?);
        ndjson.push('\n');
        fs::write(
            candidates_dir.join(format!("{}.json", sanitize_artifact_name(&candidate.id))),
            serde_json::to_vec_pretty(candidate)?,
        )?;
    }
    fs::write(out.join("candidates.ndjson"), ndjson)?;
    Ok(())
}

fn read_candidate_review_surfaces(
    out: &Path,
) -> Result<(Vec<ReviewInlineComment>, Vec<SummaryOnlyFinding>)> {
    let candidates = read_candidate_records(out)?;
    candidate_review_surfaces(&candidates)
}

fn read_candidate_records(out: &Path) -> Result<Vec<CandidateRecord>> {
    let path = out.join("review/candidates.json");
    serde_json::from_slice(&fs::read(&path).with_context(|| format!("read {}", path.display()))?)
        .with_context(|| format!("parse {}", path.display()))
}

fn candidate_review_surfaces(
    candidates: &[CandidateRecord],
) -> Result<(Vec<ReviewInlineComment>, Vec<SummaryOnlyFinding>)> {
    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    for candidate in candidates {
        if candidate.schema != "ub-review.candidate.v1" {
            bail!("candidate {} has unsupported schema", candidate.id);
        }
        if !matches!(
            candidate.disposition.as_str(),
            "inline" | "summary-only" | "parked-follow-up" | "refuted" | "dropped"
        ) {
            bail!(
                "candidate {} has unsupported disposition {}",
                candidate.id,
                candidate.disposition
            );
        }
        match (candidate.source.as_str(), candidate.status.as_str()) {
            ("inline-comment", "accepted-inline") => {
                if candidate.disposition != "inline" {
                    bail!(
                        "inline candidate {} disposition must be inline",
                        candidate.id
                    );
                }
                let path = candidate
                    .path
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("candidate {} missing path", candidate.id))?;
                let line = candidate
                    .line
                    .ok_or_else(|| anyhow::anyhow!("candidate {} missing line", candidate.id))?;
                let side = candidate
                    .side
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("candidate {} missing side", candidate.id))?;
                if side != "RIGHT" {
                    bail!("candidate {} side must be RIGHT", candidate.id);
                }
                inline_comments.push(ReviewInlineComment {
                    lane: candidate.lane.clone(),
                    severity: candidate.severity.clone(),
                    confidence: candidate.confidence.clone(),
                    path,
                    line,
                    side,
                    body: candidate.claim.clone(),
                    evidence: candidate.evidence.clone(),
                });
            }
            ("summary-only-finding", "summary-only") => {
                if candidate.disposition == "inline" {
                    bail!(
                        "summary-only candidate {} disposition cannot be inline",
                        candidate.id
                    );
                }
                if candidate.path.is_some() || candidate.line.is_some() || candidate.side.is_some()
                {
                    bail!("summary-only candidate {} has inline fields", candidate.id);
                }
                summary_only_findings.push(SummaryOnlyFinding {
                    lane: candidate.lane.clone(),
                    severity: candidate.severity.clone(),
                    confidence: candidate.confidence.clone(),
                    reason: candidate.claim.clone(),
                    evidence: candidate.evidence.clone(),
                });
            }
            _ => bail!(
                "candidate {} has unsupported source/status {}/{}",
                candidate.id,
                candidate.source,
                candidate.status
            ),
        }
    }
    Ok((inline_comments, summary_only_findings))
}

fn build_orchestrator_plan(
    candidates: &[CandidateRecord],
    observations: &[ObservationGroup],
    proof_receipts: &[ProofReceipt],
    resource_leases: &[ResourceLease],
) -> OrchestratorPlanArtifact {
    let mut grouped: BTreeMap<(String, String), Vec<&CandidateRecord>> = BTreeMap::new();
    for candidate in candidates {
        let evidence_need = candidate_evidence_need(candidate);
        grouped
            .entry((candidate.disposition.clone(), evidence_need))
            .or_default()
            .push(candidate);
    }

    let mut evidence_groups = Vec::new();
    let mut follow_up_tasks = Vec::new();
    for ((disposition, evidence_need), group_candidates) in grouped {
        let candidate_ids = group_candidates
            .iter()
            .map(|candidate| candidate.id.clone())
            .collect::<Vec<_>>();
        let lanes = unique_sorted(
            group_candidates
                .iter()
                .map(|candidate| candidate.lane.clone())
                .collect(),
        );
        let routed_evidence =
            routed_evidence_for_group(&evidence_need, &lanes, proof_receipts, resource_leases);
        let fingerprint = sha256_hex(format!("{disposition}\n{evidence_need}").as_bytes());
        let group_id = format!("evidence-group-{}", &fingerprint[..12]);
        let group = OrchestratorEvidenceGroup {
            schema: "ub-review.orchestrator_evidence_group.v1".to_owned(),
            id: group_id.clone(),
            evidence_need: evidence_need.clone(),
            disposition: disposition.clone(),
            candidate_ids: candidate_ids.clone(),
            lanes,
            routed_evidence: routed_evidence.clone(),
            duplicate_count: candidate_ids.len().saturating_sub(1),
            reason: orchestrator_group_reason(&disposition, &evidence_need),
        };
        if let Some(task) = follow_up_task_for_group(
            &group_id,
            &disposition,
            &evidence_need,
            &candidate_ids,
            &routed_evidence,
        ) {
            follow_up_tasks.push(task);
        }
        evidence_groups.push(group);
    }

    let mut observation_groups = Vec::new();
    for observation in observations {
        let evidence_need = observation_evidence_need(observation);
        let routed_evidence = routed_evidence_for_group(
            &evidence_need,
            &observation.lanes,
            proof_receipts,
            resource_leases,
        );
        let group_id = format!("orchestrator-{}", observation.id);
        let group = OrchestratorObservationGroup {
            schema: "ub-review.orchestrator_observation_group.v1".to_owned(),
            id: group_id.clone(),
            observation_group_id: observation.id.clone(),
            dedupe_key: observation.dedupe_key.clone(),
            evidence_need: evidence_need.clone(),
            claim: observation.claim.clone(),
            kind: observation.kind.clone(),
            status: observation.status.clone(),
            lanes: observation.lanes.clone(),
            sources: observation.sources.clone(),
            observation_ids: observation.observation_ids.clone(),
            duplicate_count: observation.duplicate_count,
            routed_evidence: routed_evidence.clone(),
            reason: format!(
                "routed unique observation group `{}` under evidence need `{evidence_need}`",
                observation.id
            ),
        };
        if let Some(task) =
            follow_up_task_for_observation_group(observation, &group, &routed_evidence)
        {
            follow_up_tasks.push(task);
        }
        observation_groups.push(group);
    }

    OrchestratorPlanArtifact {
        schema: "ub-review.orchestrator_plan.v1".to_owned(),
        candidates: candidates.len(),
        observations: observations.len(),
        evidence_groups,
        observation_groups,
        follow_up_tasks,
    }
}

fn write_orchestrator_artifacts(out: &Path, plan: &OrchestratorPlanArtifact) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("orchestrator_plan.json"),
        serde_json::to_vec_pretty(plan)?,
    )?;

    let mut ndjson = String::new();
    for task in &plan.follow_up_tasks {
        ndjson.push_str(&serde_json::to_string(task)?);
        ndjson.push('\n');
    }
    fs::write(out.join("follow_up_questions.ndjson"), ndjson)?;
    write_follow_up_question_packets(out, &plan.follow_up_tasks)?;
    Ok(())
}

fn write_follow_up_result_artifacts(out: &Path, results: &[FollowUpResult]) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("follow_up_results.json"),
        serde_json::to_vec_pretty(results)?,
    )?;
    let mut ndjson = String::new();
    for result in results {
        ndjson.push_str(&serde_json::to_string(result)?);
        ndjson.push('\n');
    }
    fs::write(out.join("follow_up_results.ndjson"), ndjson)?;
    Ok(())
}

fn write_follow_up_output_artifacts(out: &Path, outputs: &[FollowUpOutputRecord]) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("follow_up_outputs.json"),
        serde_json::to_vec_pretty(outputs)?,
    )?;
    let mut ndjson = String::new();
    for output in outputs {
        ndjson.push_str(&serde_json::to_string(output)?);
        ndjson.push('\n');
    }
    fs::write(out.join("follow_up_outputs.ndjson"), ndjson)?;
    Ok(())
}

fn follow_up_evidence_from_outputs(outputs: &[FollowUpOutputRecord]) -> FollowUpEvidenceArtifact {
    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    let mut observations = Vec::new();
    let mut proof_requests = Vec::new();
    for output in outputs {
        inline_comments.extend(output.inline_comments.iter().cloned());
        summary_only_findings.extend(output.summary_only_findings.iter().cloned());
        observations.extend(output.observations.iter().cloned());
        proof_requests.extend(output.proof_requests.iter().cloned());
    }
    FollowUpEvidenceArtifact {
        schema: "ub-review.follow_up_evidence.v1".to_owned(),
        follow_up_outputs: outputs.len(),
        inline_comments,
        summary_only_findings,
        observations,
        proof_requests,
    }
}

fn append_follow_up_proof_requests(
    proof_requests: &mut Vec<ProofRequest>,
    evidence: &FollowUpEvidenceArtifact,
) {
    let mut seen_ids = proof_requests
        .iter()
        .map(|request| request.id.clone())
        .collect::<BTreeSet<_>>();
    for request in &evidence.proof_requests {
        if !seen_ids.insert(request.id.clone()) {
            continue;
        }
        let mut request = request.clone();
        request.reason = post_broker_follow_up_proof_reason(&request.reason);
        proof_requests.push(request);
    }
}

fn post_broker_follow_up_proof_reason(reason: &str) -> String {
    const NOTE: &str = "Follow-up proof request arrived after proof broker v0 execution; retained for next broker scheduling pass.";
    if reason.contains(NOTE) {
        return reason.to_owned();
    }
    let reason = reason.trim();
    if reason.is_empty() {
        NOTE.to_owned()
    } else if reason.ends_with('.') || reason.ends_with('!') || reason.ends_with('?') {
        format!("{reason} {NOTE}")
    } else {
        format!("{reason}. {NOTE}")
    }
}

fn write_follow_up_evidence_artifact(
    out: &Path,
    evidence: &FollowUpEvidenceArtifact,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("follow_up_evidence.json"),
        serde_json::to_vec_pretty(evidence)?,
    )?;
    Ok(())
}

fn write_follow_up_question_packets(out: &Path, tasks: &[FollowUpQuestionTask]) -> Result<()> {
    let follow_up_dir = out.join("questions").join("orchestrator-follow-up");
    if follow_up_dir.exists() {
        fs::remove_dir_all(&follow_up_dir)
            .with_context(|| format!("remove {}", follow_up_dir.display()))?;
    }
    if tasks.is_empty() {
        return Ok(());
    }
    fs::create_dir_all(&follow_up_dir)
        .with_context(|| format!("create {}", follow_up_dir.display()))?;
    for task in tasks {
        let packet = follow_up_question_packet(task);
        fs::write(
            follow_up_dir.join(format!("{}.json", sanitize_artifact_name(&task.id))),
            serde_json::to_vec_pretty(&packet)?,
        )?;
    }
    Ok(())
}

fn follow_up_question_packet(task: &FollowUpQuestionTask) -> FollowUpQuestionPacket<'_> {
    FollowUpQuestionPacket {
        schema: "ub-review.follow_up_question_packet.v1",
        id: task.id.as_str(),
        task_id: task.id.as_str(),
        group_id: task.group_id.as_str(),
        stage: task.stage.as_str(),
        stage_reason: task.stage_reason.as_str(),
        evidence_need: task.evidence_need.as_str(),
        disposition: task.disposition.as_str(),
        candidate_ids: &task.candidate_ids,
        observation_group_ids: &task.observation_group_ids,
        routed_evidence: &task.routed_evidence,
        question: task.question.as_str(),
        status: task.status.as_str(),
        source_artifact: "review/orchestrator_plan.json",
        prompt: render_follow_up_question_prompt(task),
    }
}

fn render_follow_up_question_prompt(task: &FollowUpQuestionTask) -> String {
    let mut prompt = String::new();
    prompt.push_str("Follow-up question task\n\n");
    prompt.push_str(&format!("- Task: `{}`\n", task.id));
    prompt.push_str(&format!("- Group: `{}`\n", task.group_id));
    prompt.push_str(&format!(
        "- Stage: `{}` - {}\n",
        task.stage, task.stage_reason
    ));
    prompt.push_str(&format!("- Evidence need: `{}`\n", task.evidence_need));
    prompt.push_str(&format!("- Disposition: `{}`\n", task.disposition));
    if !task.candidate_ids.is_empty() {
        prompt.push_str(&format!(
            "- Candidate ids: `{}`\n",
            task.candidate_ids.join("`, `")
        ));
    }
    if !task.observation_group_ids.is_empty() {
        prompt.push_str(&format!(
            "- Observation group ids: `{}`\n",
            task.observation_group_ids.join("`, `")
        ));
    }
    prompt.push_str(&format!("\nQuestion: {}\n\n", task.question));
    if task.routed_evidence.is_empty() {
        prompt.push_str("Routed evidence: none.\n\n");
    } else {
        prompt.push_str("Routed evidence:\n");
        for evidence in &task.routed_evidence {
            prompt.push_str(&format!(
                "- `{}` kind=`{}` status=`{}` result=`{}` artifact=`{}` reason={}\n",
                evidence.id,
                evidence.kind,
                evidence.status,
                evidence.result,
                evidence.artifact,
                evidence.reason
            ));
        }
        prompt.push('\n');
    }
    match task.stage.as_str() {
        "tertiary" => prompt.push_str(
            "Stage instruction: use routed evidence to refine, refute, drop, or park the concern; do not repeat an already-resolved question.\n",
        ),
        _ => prompt.push_str(
            "Stage instruction: identify the smallest remaining evidence or proof request needed before promotion.\n",
        ),
    }
    prompt.push_str(
        &format!(
            "Return strict JSON with observations, summary_only_findings, failed_objections, and proof_requests. Use question `{}` for observations. Do not emit candidate_findings or inline_comments. Do not post, mutate, or run shell commands.\n",
            task.id
        ),
    );
    prompt
}

fn candidate_evidence_need(candidate: &CandidateRecord) -> String {
    match candidate.disposition.as_str() {
        "inline" => "accepted-inline-review".to_owned(),
        "parked-follow-up" => "parked-follow-up-confirmation".to_owned(),
        "refuted" => "refutation-confirmation".to_owned(),
        "dropped" => "dropped-candidate-audit".to_owned(),
        _ => {
            let text = format!("{}\n{}", candidate.claim, candidate.evidence).to_ascii_lowercase();
            if text.contains("proof") || text.contains("red") || text.contains("green") {
                "proof-confirmation".to_owned()
            } else if text.contains("route") || text.contains("sibling") {
                "source-route-confirmation".to_owned()
            } else if text.contains("test") || text.contains("oracle") {
                "test-oracle-confirmation".to_owned()
            } else {
                "summary-confirmation".to_owned()
            }
        }
    }
}

fn observation_evidence_need(observation: &ObservationGroup) -> String {
    if is_refutation_confirmation_observation(observation) {
        return "refutation-confirmation".to_owned();
    }
    if is_parked_observation(observation) {
        return "parked-follow-up-confirmation".to_owned();
    }
    if observation.kind == "test-gap" {
        return "test-oracle-confirmation".to_owned();
    }
    if observation.kind == "source-route-gap" {
        return "source-route-confirmation".to_owned();
    }

    let text =
        format!("{}\n{}", observation.claim, observation.evidence.join("\n")).to_ascii_lowercase();
    if text.contains("proof")
        || text.contains("red")
        || text.contains("green")
        || text.contains("base+tests")
    {
        "proof-confirmation".to_owned()
    } else if text.contains("route") || text.contains("sibling") {
        "source-route-confirmation".to_owned()
    } else if text.contains("test") || text.contains("oracle") {
        "test-oracle-confirmation".to_owned()
    } else if is_missing_evidence_observation(observation) {
        "evidence-gap-confirmation".to_owned()
    } else if is_residual_risk_observation(observation) {
        "residual-risk-confirmation".to_owned()
    } else {
        "observation-confirmation".to_owned()
    }
}

fn follow_up_task_for_group(
    group_id: &str,
    disposition: &str,
    evidence_need: &str,
    candidate_ids: &[String],
    routed_evidence: &[OrchestratorRoutedEvidence],
) -> Option<FollowUpQuestionTask> {
    if matches!(disposition, "inline" | "dropped") {
        return None;
    }
    let fingerprint = sha256_hex(format!("{group_id}\n{evidence_need}").as_bytes());
    let stage = follow_up_stage(disposition, evidence_need, routed_evidence);
    Some(FollowUpQuestionTask {
        schema: "ub-review.follow_up_question.v1".to_owned(),
        id: format!("follow-up-{}", &fingerprint[..12]),
        group_id: group_id.to_owned(),
        stage: stage.to_owned(),
        stage_reason: follow_up_stage_reason(stage).to_owned(),
        evidence_need: evidence_need.to_owned(),
        disposition: disposition.to_owned(),
        candidate_ids: candidate_ids.to_vec(),
        observation_group_ids: Vec::new(),
        routed_evidence: routed_evidence.to_vec(),
        question: follow_up_question_text(disposition, evidence_need),
        status: "planned".to_owned(),
        reason: "deterministic orchestrator skeleton; no shell commands or posting side effects"
            .to_owned(),
    })
}

fn follow_up_task_for_observation_group(
    observation: &ObservationGroup,
    group: &OrchestratorObservationGroup,
    routed_evidence: &[OrchestratorRoutedEvidence],
) -> Option<FollowUpQuestionTask> {
    if is_pr_body_artifact_only_observation(observation)
        || matches!(observation.status.as_str(), "covered" | "duplicate")
    {
        return None;
    }
    let fingerprint = sha256_hex(format!("{}\n{}", group.id, group.evidence_need).as_bytes());
    let stage = follow_up_stage("observation", &group.evidence_need, routed_evidence);
    Some(FollowUpQuestionTask {
        schema: "ub-review.follow_up_question.v1".to_owned(),
        id: format!("follow-up-{}", &fingerprint[..12]),
        group_id: group.id.clone(),
        stage: stage.to_owned(),
        stage_reason: follow_up_stage_reason(stage).to_owned(),
        evidence_need: group.evidence_need.clone(),
        disposition: "observation".to_owned(),
        candidate_ids: Vec::new(),
        observation_group_ids: vec![observation.id.clone()],
        routed_evidence: routed_evidence.to_vec(),
        question: observation_follow_up_question_text(&group.evidence_need),
        status: "planned".to_owned(),
        reason: "deterministic observation follow-up; no shell commands or posting side effects"
            .to_owned(),
    })
}

fn follow_up_stage(
    disposition: &str,
    evidence_need: &str,
    routed_evidence: &[OrchestratorRoutedEvidence],
) -> &'static str {
    if !routed_evidence.is_empty()
        || matches!(disposition, "refuted" | "parked-follow-up")
        || matches!(
            evidence_need,
            "refutation-confirmation" | "parked-follow-up-confirmation"
        )
    {
        "tertiary"
    } else {
        "secondary"
    }
}

fn follow_up_stage_reason(stage: &str) -> &'static str {
    match stage {
        "tertiary" => {
            "routed evidence or prior disposition is available; refine, refute, drop, or park instead of restating the concern"
        }
        _ => {
            "no routed proof receipt is available; ask for the smallest remaining evidence or proof request"
        }
    }
}

fn routed_evidence_for_group(
    evidence_need: &str,
    lanes: &[String],
    proof_receipts: &[ProofReceipt],
    resource_leases: &[ResourceLease],
) -> Vec<OrchestratorRoutedEvidence> {
    if !matches!(
        evidence_need,
        "proof-confirmation" | "test-oracle-confirmation"
    ) {
        return Vec::new();
    }
    let mut routed = Vec::new();
    for receipt in proof_receipts {
        if !proof_receipt_routes_to_lanes(receipt, lanes) {
            continue;
        }
        routed.push(proof_receipt_routed_evidence(receipt));
        for lease in resource_leases
            .iter()
            .filter(|lease| lease.consumer == receipt.id)
        {
            routed.push(resource_lease_routed_evidence(lease));
        }
    }
    routed
}

fn proof_receipt_routes_to_lanes(receipt: &ProofReceipt, lanes: &[String]) -> bool {
    receipt
        .requested_by
        .iter()
        .any(|lane| lane == "proof-broker")
        || receipt
            .requested_by
            .iter()
            .any(|lane| lanes.iter().any(|group_lane| group_lane == lane))
}

fn proof_receipt_routed_evidence(receipt: &ProofReceipt) -> OrchestratorRoutedEvidence {
    OrchestratorRoutedEvidence {
        schema: "ub-review.orchestrator_routed_evidence.v1".to_owned(),
        id: receipt.id.clone(),
        kind: "proof-receipt".to_owned(),
        artifact: "review/proof_receipts.json".to_owned(),
        status: routed_status_for_proof_receipt(receipt).to_owned(),
        result: receipt.result.clone(),
        reason: receipt.reason.clone(),
    }
}

fn resource_lease_routed_evidence(lease: &ResourceLease) -> OrchestratorRoutedEvidence {
    OrchestratorRoutedEvidence {
        schema: "ub-review.orchestrator_routed_evidence.v1".to_owned(),
        id: lease.id.clone(),
        kind: "resource-lease".to_owned(),
        artifact: "review/resource_leases.json".to_owned(),
        status: lease.status.clone(),
        result: lease.status.clone(),
        reason: lease.reason.clone(),
    }
}

fn routed_status_for_proof_receipt(receipt: &ProofReceipt) -> &'static str {
    if proof_receipt_is_test_proof_result(receipt) {
        "tool-confirmed"
    } else if proof_receipt_is_residual_risk(receipt) {
        "residual-risk"
    } else if proof_receipt_is_missing_evidence(receipt) {
        "missing-evidence"
    } else {
        "recorded"
    }
}

fn observation_follow_up_question_text(evidence_need: &str) -> String {
    match evidence_need {
        "proof-confirmation" => {
            "Confirm whether routed proof evidence resolves this observation.".to_owned()
        }
        "source-route-confirmation" => {
            "Confirm the changed source route or sibling path before promoting this observation."
                .to_owned()
        }
        "test-oracle-confirmation" => {
            "Confirm the test oracle strength before promoting this observation.".to_owned()
        }
        "refutation-confirmation" => {
            "Confirm the observation refutation still matches current PR evidence.".to_owned()
        }
        "parked-follow-up-confirmation" => {
            "Confirm whether this observation remains parked outside current PR scope.".to_owned()
        }
        "evidence-gap-confirmation" => {
            "Confirm whether this observation is still trust-affecting missing evidence.".to_owned()
        }
        "residual-risk-confirmation" => {
            "Confirm whether this observation remains specific residual risk.".to_owned()
        }
        _ => "Confirm whether this observation needs promotion, refutation, or parking.".to_owned(),
    }
}

fn follow_up_question_text(disposition: &str, evidence_need: &str) -> String {
    match (disposition, evidence_need) {
        ("refuted", _) => "Confirm the refutation still matches the current PR evidence.".to_owned(),
        ("parked-follow-up", _) => {
            "Confirm whether this parked follow-up should remain outside current PR scope.".to_owned()
        }
        (_, "proof-confirmation") => {
            "Confirm whether focused proof can resolve this summary-only candidate.".to_owned()
        }
        (_, "source-route-confirmation") => {
            "Confirm the changed source route or sibling path before promoting this candidate."
                .to_owned()
        }
        (_, "test-oracle-confirmation") => {
            "Confirm the test oracle strength before promoting this candidate.".to_owned()
        }
        _ => "Confirm whether additional evidence should promote or keep this candidate summary-only."
            .to_owned(),
    }
}

fn orchestrator_group_reason(disposition: &str, evidence_need: &str) -> String {
    format!("grouped candidate disposition `{disposition}` under evidence need `{evidence_need}`")
}

fn unique_sorted(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

struct WitnessRecordInput<'a> {
    status: &'a str,
    kind: &'a str,
    source: &'a str,
    claim: &'a str,
    dedupe_key: &'a str,
    evidence: Vec<String>,
    lane: Option<String>,
    path: Option<String>,
    line: Option<u32>,
    observation_id: Option<String>,
    proof_receipt_id: Option<String>,
}

fn witness_record(input: WitnessRecordInput<'_>) -> WitnessRecord {
    let fingerprint = sha256_hex(
        format!(
            "{}\n{}\n{}\n{}",
            input.status, input.kind, input.source, input.dedupe_key
        )
        .as_bytes(),
    );
    WitnessRecord {
        schema: "ub-review.witness.v1".to_owned(),
        id: format!("witness-{}", &fingerprint[..12]),
        status: input.status.to_owned(),
        kind: input.kind.to_owned(),
        source: input.source.to_owned(),
        claim: input.claim.to_owned(),
        dedupe_key: input.dedupe_key.to_owned(),
        evidence: non_empty_evidence(input.evidence, "witness registry source artifact"),
        lane: input.lane,
        path: input.path,
        line: input.line,
        observation_id: input.observation_id,
        proof_receipt_id: input.proof_receipt_id,
    }
}

fn witness_status_for_summary_finding(finding: &SummaryOnlyFinding) -> &'static str {
    if is_parked_follow_up(finding) {
        "parked"
    } else {
        "needs-witness"
    }
}

fn witness_status_for_observation(observation: &Observation) -> &'static str {
    if observation.status == "refuted"
        || matches!(
            observation.kind.as_str(),
            "false-premise" | "resolved-check"
        )
    {
        "refuted"
    } else if observation.status == "parked" || observation.kind == "parked-follow-up" {
        "parked"
    } else if observation.status == "confirmed"
        || matches!(observation.kind.as_str(), "bug" | "security-risk")
    {
        "tool-confirmed"
    } else {
        "needs-witness"
    }
}

fn witness_status_for_proof_receipt(receipt: &ProofReceipt) -> &'static str {
    match receipt.result.as_str() {
        "discriminating" | "head_passed" | "head_failed" => "tool-confirmed",
        _ => "needs-witness",
    }
}

fn proof_receipt_witness_evidence(receipt: &ProofReceipt) -> Vec<String> {
    let mut evidence = Vec::new();
    for command in &receipt.commands {
        evidence.push(format!(
            "{} `{}` status=`{}` reason=`{}` stdout=`{}` stderr=`{}`",
            command.side,
            command.command,
            command.status,
            command.reason,
            command.stdout,
            command.stderr
        ));
    }
    if evidence.is_empty() {
        evidence.push(receipt.reason.clone());
    }
    evidence
}

fn witness_registry_artifact(witnesses: &[WitnessRecord]) -> WitnessRegistryArtifact {
    let mut status_counts = BTreeMap::new();
    let mut kind_counts = BTreeMap::new();
    let mut source_counts = BTreeMap::new();
    let mut follow_up_status_counts = BTreeMap::new();
    let mut witness_ids_by_status: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut follow_up_witness_ids_by_status: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut follow_up_total = 0;

    for witness in witnesses {
        *status_counts.entry(witness.status.clone()).or_insert(0) += 1;
        *kind_counts.entry(witness.kind.clone()).or_insert(0) += 1;
        *source_counts.entry(witness.source.clone()).or_insert(0) += 1;
        witness_ids_by_status
            .entry(witness.status.clone())
            .or_default()
            .push(witness.id.clone());

        if witness.source.starts_with("follow-up-") {
            follow_up_total += 1;
            *follow_up_status_counts
                .entry(witness.status.clone())
                .or_insert(0) += 1;
            follow_up_witness_ids_by_status
                .entry(witness.status.clone())
                .or_default()
                .push(witness.id.clone());
        }
    }

    WitnessRegistryArtifact {
        schema: "ub-review.witness_registry.v1".to_owned(),
        total: witnesses.len(),
        status_counts,
        kind_counts,
        source_counts,
        follow_up_total,
        follow_up_status_counts,
        witness_ids_by_status,
        follow_up_witness_ids_by_status,
    }
}

fn write_witness_artifacts(out: &Path, witnesses: &[WitnessRecord]) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let registry = witness_registry_artifact(witnesses);
    fs::write(
        review_dir.join("witnesses.json"),
        serde_json::to_vec_pretty(witnesses)?,
    )?;
    fs::write(
        review_dir.join("witness_registry.json"),
        serde_json::to_vec_pretty(&registry)?,
    )?;
    let mut ndjson = String::new();
    for witness in witnesses {
        ndjson.push_str(&serde_json::to_string(witness)?);
        ndjson.push('\n');
    }
    fs::write(out.join("witnesses.ndjson"), ndjson)?;
    Ok(())
}

fn write_proof_planner_artifacts(
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    box_state: &BoxState,
    pr_thread_context: &PrThreadContext,
    proof_requests: &[ProofRequest],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let budget = proof_budget(profile)?;
    let lease_budget = proof_lease_budget(profile)?;
    let input = ProofPlannerInput {
        schema: "ub-review.proof_planner_input.v1",
        diff_class: diff.diff_class.key(),
        changed_files: &diff.changed_files,
        pr_thread_context_status: &pr_thread_context.status,
        proof_requests,
        runtime_budget: ProofPlannerRuntimeBudget {
            target_timeout_sec: profile.budgets.default_timeout_sec,
            hard_timeout_sec: profile.budgets.hard_timeout_sec,
            max_focused_tests: budget.max_focused_tests,
            per_command_timeout_sec: budget.per_command_timeout_sec,
            total_proof_timeout_sec: budget.max_total_seconds,
        },
        box_shape: box_state,
    };
    let plans = focused_proof_plans_from_diff(diff, proof_requests, budget);
    let proof_tasks = plans
        .into_iter()
        .map(|plan| proof_task_artifact(plan, budget, lease_budget))
        .collect::<Vec<_>>();
    let skip = proof_planner_skips(diff);
    let output = ProofPlannerOutput {
        schema: "ub-review.proof_planner_output.v1",
        lane: "proof-planner",
        proof_tasks,
        skip,
    };
    fs::write(
        review_dir.join("proof_planner_input.json"),
        serde_json::to_vec_pretty(&input)?,
    )?;
    fs::write(
        review_dir.join("proof_planner_output.json"),
        serde_json::to_vec_pretty(&output)?,
    )?;
    let mut ndjson = String::new();
    for task in &output.proof_tasks {
        ndjson.push_str(&serde_json::to_string(task)?);
        ndjson.push('\n');
    }
    fs::write(out.join("proof_tasks.ndjson"), ndjson)?;
    Ok(())
}

fn proof_task_artifact(
    plan: FocusedProofPlan,
    budget: ProofBudget,
    lease_budget: ProofLeaseBudget,
) -> ProofTaskArtifact {
    let base_plus_tests_command =
        (plan.mode == FocusedProofMode::RedGreen).then(|| plan.base_plus_tests_command.clone());
    let command = match &base_plus_tests_command {
        Some(base_command) => format!("{} && {}", plan.head_command, base_command),
        None => plan.head_command.clone(),
    };
    let purpose = focused_proof_task_purpose(&plan);
    ProofTaskArtifact {
        schema: "ub-review.proof_task.v1",
        id: plan.id,
        kind: "focused-test".to_owned(),
        command,
        head_command: plan.head_command,
        base_plus_tests_command,
        purpose,
        consumers: vec![
            "tests-oracle".to_owned(),
            "opposition".to_owned(),
            "compiler".to_owned(),
        ],
        value: "high".to_owned(),
        cost: "low".to_owned(),
        timeout_sec: budget
            .per_command_timeout_sec
            .saturating_mul(plan.mode.command_count())
            .min(budget.max_total_seconds),
        lease: ProofTaskLease {
            cpu: lease_budget.cpu,
            memory_mb: lease_budget.memory_mb,
            disk_mb: lease_budget.disk_mb,
            network: lease_budget.network,
        },
        test_file: plan.test_file,
        test_name: plan.test_name,
        mode: plan.mode.key().to_owned(),
        requested_by: plan.requested_by,
        request_ids: plan.request_ids,
    }
}

fn focused_proof_task_purpose(plan: &FocusedProofPlan) -> String {
    match plan.mode {
        FocusedProofMode::HeadOnly => {
            format!(
                "Prove the focused test target `{}` passes on HEAD.",
                plan.test_file
            )
        }
        FocusedProofMode::RedGreen => format!(
            "Prove the focused test target `{}` fails on base+tests and passes on HEAD.",
            plan.test_file
        ),
    }
}

fn proof_planner_skips(diff: &DiffContext) -> Vec<ProofPlannerSkip> {
    let mut skip = Vec::new();
    if !diff.flags.unsafe_or_native_risk {
        skip.push(ProofPlannerSkip {
            kind: "miri".to_owned(),
            reason: "No new unsafe/native aliasing surface was detected; cheaper focused proof is preferred when available.".to_owned(),
        });
    }
    if !diff.flags.workflow_changed {
        skip.push(ProofPlannerSkip {
            kind: "actionlint".to_owned(),
            reason: "No workflow files changed.".to_owned(),
        });
    }
    skip
}

fn write_proof_request_artifacts(
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    proof_requests: &[ProofRequest],
    proof_receipts: &[ProofReceipt],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let proof_groups = proof_request_groups(proof_requests);
    let focused_plans = focused_proof_plans_from_diff(diff, proof_requests, proof_budget(profile)?);
    fs::write(
        review_dir.join("proof_requests.json"),
        serde_json::to_vec_pretty(proof_requests)?,
    )?;
    fs::write(
        review_dir.join("proof_request_groups.json"),
        serde_json::to_vec_pretty(&proof_groups)?,
    )?;

    let proof_request_dir = out.join("proof_requests");
    if proof_request_dir.exists() {
        fs::remove_dir_all(&proof_request_dir)
            .with_context(|| format!("remove {}", proof_request_dir.display()))?;
    }
    fs::create_dir_all(&proof_request_dir)
        .with_context(|| format!("create {}", proof_request_dir.display()))?;

    let mut ndjson = String::new();
    for request in proof_requests {
        ndjson.push_str(&serde_json::to_string(request)?);
        ndjson.push('\n');
        fs::write(
            proof_request_dir.join(format!("{}.json", sanitize_artifact_name(&request.id))),
            serde_json::to_vec_pretty(request)?,
        )?;
    }
    fs::write(out.join("proof_requests.ndjson"), ndjson)?;

    let mut plan = String::new();
    plan.push_str("# Proof request plan\n\n");
    if proof_requests.is_empty() && focused_plans.is_empty() {
        plan.push_str("No proof requests were emitted by model lanes.\n");
    } else {
        if proof_requests.is_empty() {
            plan.push_str("No model-lane proof requests were emitted.\n\n");
        } else {
            plan.push_str(&format!(
                "Grouped proof broker tasks: {} unique from {} request(s).\n\n",
                proof_groups.len(),
                proof_requests.len()
            ));
            for group in &proof_groups {
                plan.push_str(&format!(
                    "- `{}` requested by `{}`: `{}` ({}, timeout {}s, required={}, status={}, merged_requests={})\n",
                    group.id,
                    group.requested_by.join(", "),
                    group.command,
                    group.cost,
                    group.timeout_sec,
                    group.required,
                    group.status,
                    group.duplicate_count
                ));
                for reason in &group.reasons {
                    plan.push_str(&format!("  - Reason: {}\n", escape_md(reason)));
                }
            }
            plan.push('\n');
        }
        if focused_plans.is_empty() {
            plan.push_str(
                "No focused proof targets were planned from the diff or proof requests.\n",
            );
        } else {
            plan.push_str("## Focused proof plan\n\n");
            if proof_receipts.is_empty() {
                plan.push_str(
                    "No proof broker commands were executed in this planner-only pass.\n\n",
                );
            } else {
                plan.push_str(
                    "Proof broker v0 executed focused proof under the runtime budget.\n\n",
                );
                for receipt in proof_receipts {
                    plan.push_str(&format!(
                        "- Receipt `{}`: kind=`{}`, test_patch_mode=`{}`, result=`{}`, commands=`{}`.\n",
                        receipt.id,
                        receipt.kind,
                        receipt.test_patch_mode,
                        receipt.result,
                        receipt.commands.len()
                    ));
                }
                plan.push('\n');
            }
            for plan_item in focused_plans {
                plan.push_str(&format!(
                    "- `{}` `{}`{} requested by `{}`: mode=`{}`, status=`{}`, cost=`focused-test`, head=`{}`, base+tests=`{}`. {}\n",
                    plan_item.id,
                    plan_item.test_file,
                    plan_item
                        .test_name
                        .as_ref()
                        .map(|name| format!(" - `{}`", escape_md(name)))
                        .unwrap_or_default(),
                    plan_item.requested_by.join(", "),
                    plan_item.mode.key(),
                    plan_item.status,
                    escape_md(&plan_item.head_command),
                    escape_md(&plan_item.base_plus_tests_command),
                    escape_md(&plan_item.reason)
                ));
                if !plan_item.request_ids.is_empty() {
                    plan.push_str(&format!(
                        "  - Merged requests: `{}`\n",
                        plan_item.request_ids.join("`, `")
                    ));
                }
            }
        }
    }
    fs::write(review_dir.join("proof_plan.md"), plan)?;
    Ok(())
}

fn write_proof_receipt_artifacts(out: &Path, proof_receipts: &[ProofReceipt]) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("proof_receipts.json"),
        serde_json::to_vec_pretty(proof_receipts)?,
    )?;
    let mut ndjson = String::new();
    for receipt in proof_receipts {
        ndjson.push_str(&serde_json::to_string(receipt)?);
        ndjson.push('\n');
    }
    fs::write(out.join("proof_receipts.ndjson"), ndjson)?;
    Ok(())
}

fn write_resource_lease_artifacts(out: &Path, resource_leases: &[ResourceLease]) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("resource_leases.json"),
        serde_json::to_vec_pretty(resource_leases)?,
    )?;

    let mut ndjson = String::new();
    for lease in resource_leases {
        ndjson.push_str(&serde_json::to_string(lease)?);
        ndjson.push('\n');
    }
    fs::write(out.join("resource_leases.ndjson"), ndjson)?;

    let mut plan = String::new();
    plan.push_str("# Resource lease plan\n\n");
    if resource_leases.is_empty() {
        plan.push_str("No local proof leases were requested in this packet.\n");
    } else {
        plan.push_str("## Focused proof leases\n\n");
        for lease in resource_leases {
            plan.push_str(&format!(
                "- `{}` kind=`{}` consumer=`{}` status=`{}` cpu=`{}` memory_mb=`{}` disk_mb=`{}` timeout_sec=`{}` network=`{}` scratch=`{}`",
                lease.id,
                lease.kind,
                lease.consumer,
                lease.status,
                lease.cpu,
                lease.memory_mb,
                lease.disk_mb,
                lease.timeout_sec,
                lease.network,
                lease.scratch
            ));
            if let Some(worktree) = &lease.worktree {
                plan.push_str(&format!(" worktree=`{}`", escape_md(worktree)));
            }
            if let Some(command) = &lease.command {
                plan.push_str(&format!(" command=`{}`", escape_md(command)));
            }
            plan.push_str(&format!(". {}\n", escape_md(&lease.reason)));
        }
    }
    fs::write(review_dir.join("resource_plan.md"), plan)?;
    Ok(())
}

fn proof_request_groups(proof_requests: &[ProofRequest]) -> Vec<ProofRequestGroup> {
    let mut groups = BTreeMap::<(String, String, u64), ProofRequestGroup>::new();
    for request in proof_requests {
        let key = (
            request.command.clone(),
            request.cost.clone(),
            request.timeout_sec,
        );
        let fingerprint = sha256_hex(
            format!(
                "{}\n{}\n{}",
                request.command, request.cost, request.timeout_sec
            )
            .as_bytes(),
        );
        let group = groups.entry(key).or_insert_with(|| ProofRequestGroup {
            schema: "ub-review.proof_request_group.v1".to_owned(),
            id: format!("proof-group-{}", &fingerprint[..12]),
            command: request.command.clone(),
            cost: request.cost.clone(),
            timeout_sec: request.timeout_sec,
            required: false,
            status: "invalid".to_owned(),
            requested_by: Vec::new(),
            request_ids: Vec::new(),
            reasons: Vec::new(),
            duplicate_count: 0,
        });
        group.required |= request.required;
        match request.status.as_str() {
            "requested" => group.status = "requested".to_owned(),
            "unsupported" if group.status != "requested" => {
                group.status = "unsupported".to_owned();
            }
            _ => {}
        }
        push_unique(&mut group.requested_by, &request.lane);
        for lane in &request.requested_by {
            push_unique(&mut group.requested_by, lane);
        }
        push_unique(&mut group.request_ids, &request.id);
        push_unique(&mut group.reasons, &request.reason);
        group.duplicate_count += 1;
    }
    groups.into_values().collect()
}

fn proof_budget(profile: &Profile) -> Result<ProofBudget> {
    let budget = ProofBudget {
        max_focused_test_files: profile.budgets.proof_max_focused_test_files,
        max_focused_tests: profile.budgets.proof_max_focused_tests,
        per_command_timeout_sec: profile.budgets.proof_command_timeout_sec,
        max_total_seconds: profile.budgets.proof_total_timeout_sec,
    };
    if budget.max_focused_tests > 0 && budget.per_command_timeout_sec == 0 {
        bail!(
            "runtime profile {} has proof_command_timeout_sec=0 with focused proof enabled",
            profile.name
        );
    }
    if budget.max_focused_tests > 0 && budget.max_total_seconds == 0 {
        bail!(
            "runtime profile {} has proof_total_timeout_sec=0 with focused proof enabled",
            profile.name
        );
    }
    Ok(budget)
}

fn proof_lease_budget(profile: &Profile) -> Result<ProofLeaseBudget> {
    let budget = ProofLeaseBudget {
        cpu: profile.budgets.proof_cpu,
        memory_mb: profile.budgets.proof_memory_mb,
        disk_mb: profile.budgets.proof_disk_mb,
        network: profile.budgets.proof_network,
        scratch: profile.budgets.proof_scratch,
    };
    if profile.limits.tests > 0 && profile.budgets.proof_max_focused_tests > 0 {
        if budget.cpu == 0 {
            bail!(
                "runtime profile {} has proof_cpu=0 with focused proof enabled",
                profile.name
            );
        }
        if budget.memory_mb == 0 {
            bail!(
                "runtime profile {} has proof_memory_mb=0 with focused proof enabled",
                profile.name
            );
        }
        if budget.disk_mb == 0 {
            bail!(
                "runtime profile {} has proof_disk_mb=0 with focused proof enabled",
                profile.name
            );
        }
    }
    Ok(budget)
}

fn focused_proof_plans_from_diff(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
    budget: ProofBudget,
) -> Vec<FocusedProofPlan> {
    focused_test_tasks_from_diff(diff, proof_requests, budget)
        .into_iter()
        .map(|task| {
            let head_command = proof_task_plan_command(&task, "head", "head");
            let base_plus_tests_command = if task.mode == FocusedProofMode::RedGreen {
                proof_task_plan_command(&task, "base-plus-tests", "base-plus-tests")
            } else {
                "not planned for head-only proof".to_owned()
            };
            FocusedProofPlan {
                id: task.id,
                test_file: task.file,
                test_name: task.test_name,
                mode: task.mode,
                head_command,
                base_plus_tests_command,
                requested_by: task.requested_by,
                request_ids: task.request_ids,
                status: "planned".to_owned(),
                reason: format!(
                    "planner-only focused test target under budget: max {} file(s), {} test(s), {}s per command, {}s total",
                    budget.max_focused_test_files,
                    budget.max_focused_tests,
                    budget.per_command_timeout_sec,
                    budget.max_total_seconds
                ),
            }
        })
        .collect()
}

fn run_proof_broker_v0(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    proof_requests: &[ProofRequest],
    args: &RunArgs,
) -> Result<ProofBrokerResult> {
    let budget = proof_budget(profile)?;
    let tasks = focused_test_candidates_from_diff(diff, proof_requests);
    run_focused_red_green_proof_tasks_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        tasks,
        run_command_to_files,
        prepare_base_plus_tests_worktree,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
fn run_follow_up_proof_broker_v0(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    proof_requests: &[ProofRequest],
    existing_receipts: &[ProofReceipt],
    existing_leases: &[ResourceLease],
    args: &RunArgs,
) -> Result<ProofBrokerResult> {
    let budget = remaining_focused_proof_budget(proof_budget(profile)?, existing_leases);
    let tasks = unreceipted_focused_test_tasks(
        focused_test_candidates_from_requests(proof_requests),
        existing_receipts,
    );
    run_follow_up_proof_broker_v0_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        tasks,
        run_command_to_files,
        prepare_base_plus_tests_worktree,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
fn run_follow_up_proof_broker_v0_with_runner<F, G>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
    budget: ProofBudget,
    tasks: Vec<FocusedTestTask>,
    runner: F,
    prepare_base_plus_tests: G,
) -> Result<ProofBrokerResult>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
    ) -> Result<CommandStatus>,
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
{
    run_focused_red_green_proof_tasks_with_runner(
        root,
        out,
        diff,
        profile,
        args,
        budget,
        tasks,
        runner,
        prepare_base_plus_tests,
    )
}

fn remaining_focused_proof_budget(
    mut budget: ProofBudget,
    existing_leases: &[ResourceLease],
) -> ProofBudget {
    let focused_leases = existing_leases
        .iter()
        .filter(|lease| lease.kind == "focused-test")
        .collect::<Vec<_>>();
    if focused_leases
        .iter()
        .any(|lease| lease.status == "exhausted")
    {
        budget.max_focused_test_files = 0;
        budget.max_focused_tests = 0;
        budget.max_total_seconds = 0;
        return budget;
    }

    let granted = focused_leases
        .iter()
        .filter(|lease| lease.status == "granted")
        .count();
    let granted_seconds = focused_leases
        .iter()
        .filter(|lease| lease.status == "granted")
        .map(|lease| lease.timeout_sec)
        .sum::<u64>();
    budget.max_focused_tests = budget.max_focused_tests.saturating_sub(granted);
    budget.max_focused_test_files = budget.max_focused_test_files.saturating_sub(granted);
    budget.max_total_seconds = budget.max_total_seconds.saturating_sub(granted_seconds);
    budget
}

fn unreceipted_focused_test_tasks(
    tasks: Vec<FocusedTestTask>,
    existing_receipts: &[ProofReceipt],
) -> Vec<FocusedTestTask> {
    let existing_ids = existing_receipts
        .iter()
        .map(|receipt| receipt.id.clone())
        .collect::<BTreeSet<_>>();
    tasks
        .into_iter()
        .filter(|task| !existing_ids.contains(&task.id))
        .collect()
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
fn run_focused_red_green_proof_tasks_with_runner<F, G>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
    budget: ProofBudget,
    tasks: Vec<FocusedTestTask>,
    mut runner: F,
    mut prepare_base_plus_tests: G,
) -> Result<ProofBrokerResult>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
    ) -> Result<CommandStatus>,
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
{
    let mut receipts = Vec::new();
    let mut leases = Vec::new();
    let mut executed_tasks = 0_usize;
    let mut executed_files = BTreeSet::new();
    let mut estimated_seconds = 0_u64;
    let lease_budget = proof_lease_budget(profile)?;
    for task in tasks {
        if args.dry_run {
            leases.push(focused_test_resource_lease(
                &task,
                budget,
                lease_budget,
                "skipped_profile",
                "dry-run; resource broker did not grant a proof lease",
            ));
            receipts.push(skipped_focused_proof_receipt(
                out,
                diff,
                &task,
                "skipped_profile",
                "dry-run; proof broker did not execute focused tests",
            )?);
            continue;
        }
        if profile.limits.tests == 0 {
            leases.push(focused_test_resource_lease(
                &task,
                budget,
                lease_budget,
                "skipped_profile",
                "profile allows zero focused test leases",
            ));
            receipts.push(skipped_focused_proof_receipt(
                out,
                diff,
                &task,
                "skipped_profile",
                "profile allows zero focused test leases",
            )?);
            continue;
        }
        if !focused_proof_budget_allows_next(
            executed_tasks,
            &executed_files,
            &task.file,
            estimated_seconds,
            task.mode.command_count(),
            budget,
        ) {
            leases.push(focused_test_resource_lease(
                &task,
                budget,
                lease_budget,
                "exhausted",
                "focused red/green proof lease budget exhausted by runtime profile",
            ));
            receipts.push(skipped_focused_proof_receipt(
                out,
                diff,
                &task,
                "skipped_budget",
                "focused red/green proof lease budget exhausted by runtime profile",
            )?);
            continue;
        }
        executed_files.insert(task.file.clone());
        leases.push(focused_test_resource_lease(
            &task,
            budget,
            lease_budget,
            "granted",
            "focused red/green proof lease granted by runtime profile",
        ));
        let receipt = match task.mode {
            FocusedProofMode::HeadOnly => run_focused_head_proof_task(
                root,
                out,
                diff,
                &task,
                budget.per_command_timeout_sec,
                &mut runner,
            )?,
            FocusedProofMode::RedGreen => run_focused_red_green_proof_task(
                root,
                out,
                diff,
                &task,
                budget.per_command_timeout_sec,
                &mut runner,
                &mut prepare_base_plus_tests,
            )?,
        };
        receipts.push(receipt);
        executed_tasks += 1;
        estimated_seconds = estimated_seconds.saturating_add(
            budget
                .per_command_timeout_sec
                .saturating_mul(task.mode.command_count()),
        );
    }
    Ok(ProofBrokerResult {
        proof_receipts: receipts,
        resource_leases: leases,
    })
}

fn run_focused_head_proof_task<F>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    task: &FocusedTestTask,
    timeout_sec: u64,
    runner: &mut F,
) -> Result<ProofReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
    ) -> Result<CommandStatus>,
{
    let head_spec = proof_task_command_spec(task, "head");
    let head = run_proof_command_receipt(root, out, task, "head", &head_spec, timeout_sec, runner)?;
    let result = match head.status.as_str() {
        "passed" => "head_passed",
        "failed" => "head_failed",
        "timed_out" => "timed_out",
        _ => "skipped_profile",
    };
    let reason = format!("HEAD proof {}: {}", head.status, head.reason);
    Ok(focused_head_receipt(
        diff,
        task,
        vec![head],
        result.to_owned(),
        reason,
    ))
}

fn run_focused_red_green_proof_task<F, G>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    task: &FocusedTestTask,
    timeout_sec: u64,
    runner: &mut F,
    prepare_base_plus_tests: &mut G,
) -> Result<ProofReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
    ) -> Result<CommandStatus>,
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
{
    let head_spec = proof_task_command_spec(task, "head");
    let head = run_proof_command_receipt(root, out, task, "head", &head_spec, timeout_sec, runner)?;
    let head_status = head.status.clone();
    if head_status != "passed" {
        let result = match head_status.as_str() {
            "timed_out" => "timed_out",
            "failed" => "head_failed",
            _ => "skipped_profile",
        };
        let reason = format!("HEAD proof {}: {}", head.status, head.reason);
        return Ok(focused_red_green_receipt(
            diff,
            task,
            vec![head],
            result.to_owned(),
            reason,
        ));
    }

    let base_root = match prepare_base_plus_tests(root, out, diff) {
        Ok(path) => path,
        Err(error) => {
            let mut commands = vec![head];
            let base_spec = proof_task_command_spec(task, "base-plus-tests");
            commands.push(skipped_proof_command_receipt(
                out,
                task,
                "base-plus-tests",
                &base_spec,
                "skipped",
                format!("base+tests patch failed: {error:#}"),
            )?);
            return Ok(focused_red_green_receipt(
                diff,
                task,
                commands,
                "base_patch_failed".to_owned(),
                "base+tests patch failed".to_owned(),
            ));
        }
    };
    let base_spec = proof_task_command_spec(task, "base-plus-tests");
    let base = run_proof_command_receipt(
        &base_root,
        out,
        task,
        "base-plus-tests",
        &base_spec,
        timeout_sec,
        runner,
    )?;
    let (result, reason) = match base.status.as_str() {
        "failed" => (
            "discriminating".to_owned(),
            format!("HEAD passed; base+tests failed: {}", base.reason),
        ),
        "passed" => (
            "non_discriminating".to_owned(),
            "HEAD and base+tests both passed".to_owned(),
        ),
        "timed_out" => (
            "timed_out".to_owned(),
            format!("base+tests timed out: {}", base.reason),
        ),
        _ => (
            "skipped_profile".to_owned(),
            format!("base+tests proof unavailable: {}", base.reason),
        ),
    };
    let _ = cleanup_base_plus_tests_worktree(root, &base_root);
    Ok(focused_red_green_receipt(
        diff,
        task,
        vec![head, base],
        result,
        reason,
    ))
}

fn run_proof_command_receipt<F>(
    command_root: &Path,
    out: &Path,
    task: &FocusedTestTask,
    side: &str,
    spec: &ProofCommandSpec,
    timeout_sec: u64,
    runner: &mut F,
) -> Result<ProofCommandReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
    ) -> Result<CommandStatus>,
{
    let paths = proof_command_paths(out, &task.id, side)?;
    let command = command_display_with_env(&spec.env, &spec.argv);
    let status = runner(
        command_root,
        &spec.argv,
        &spec.env,
        timeout_sec,
        &paths.stdout_path,
        &paths.stderr_path,
    );
    let (command_status, reason, exit_code, timed_out, duration_ms) = match status {
        Ok(status) if status.timed_out => (
            "timed_out".to_owned(),
            status.reason,
            status.exit_code,
            true,
            status.duration_ms,
        ),
        Ok(status) if status.success => (
            "passed".to_owned(),
            status.reason,
            status.exit_code,
            false,
            status.duration_ms,
        ),
        Ok(status) => (
            "failed".to_owned(),
            status.reason,
            status.exit_code,
            false,
            status.duration_ms,
        ),
        Err(error) => (
            "skipped".to_owned(),
            format!("focused proof command unavailable: {error:#}"),
            None,
            false,
            0,
        ),
    };
    Ok(ProofCommandReceipt {
        side: side.to_owned(),
        command,
        env: spec.env.clone(),
        status: command_status,
        exit_code,
        timed_out,
        timeout_sec,
        duration_ms,
        stdout: paths.stdout_rel,
        stderr: paths.stderr_rel,
        reason,
    })
}

fn skipped_proof_command_receipt(
    out: &Path,
    task: &FocusedTestTask,
    side: &str,
    spec: &ProofCommandSpec,
    status: &str,
    reason: String,
) -> Result<ProofCommandReceipt> {
    let paths = proof_command_paths(out, &task.id, side)?;
    Ok(ProofCommandReceipt {
        side: side.to_owned(),
        command: command_display_with_env(&spec.env, &spec.argv),
        env: spec.env.clone(),
        status: status.to_owned(),
        exit_code: None,
        timed_out: false,
        timeout_sec: 0,
        duration_ms: 0,
        stdout: paths.stdout_rel,
        stderr: paths.stderr_rel,
        reason,
    })
}

fn skipped_focused_proof_receipt(
    out: &Path,
    diff: &DiffContext,
    task: &FocusedTestTask,
    result: &str,
    reason: &str,
) -> Result<ProofReceipt> {
    let spec = proof_task_command_spec(task, "head");
    let command =
        skipped_proof_command_receipt(out, task, "head", &spec, "skipped", reason.to_owned())?;
    Ok(focused_receipt(
        diff,
        task,
        vec![command],
        result.to_owned(),
        reason.to_owned(),
    ))
}

fn focused_receipt(
    diff: &DiffContext,
    task: &FocusedTestTask,
    commands: Vec<ProofCommandReceipt>,
    result: String,
    reason: String,
) -> ProofReceipt {
    match task.mode {
        FocusedProofMode::HeadOnly => focused_head_receipt(diff, task, commands, result, reason),
        FocusedProofMode::RedGreen => {
            focused_red_green_receipt(diff, task, commands, result, reason)
        }
    }
}

fn focused_head_receipt(
    diff: &DiffContext,
    task: &FocusedTestTask,
    commands: Vec<ProofCommandReceipt>,
    result: String,
    reason: String,
) -> ProofReceipt {
    ProofReceipt {
        schema: "ub-review.proof_receipt.v1".to_owned(),
        id: task.id.clone(),
        kind: "focused-head".to_owned(),
        base: diff.base.clone(),
        head: diff.head.clone(),
        test_patch_mode: "head-only".to_owned(),
        requested_by: task.requested_by.clone(),
        request_ids: task.request_ids.clone(),
        commands,
        result,
        reason,
    }
}

fn focused_red_green_receipt(
    diff: &DiffContext,
    task: &FocusedTestTask,
    commands: Vec<ProofCommandReceipt>,
    result: String,
    reason: String,
) -> ProofReceipt {
    ProofReceipt {
        schema: "ub-review.proof_receipt.v1".to_owned(),
        id: task.id.clone(),
        kind: "focused-red-green".to_owned(),
        base: diff.base.clone(),
        head: diff.head.clone(),
        test_patch_mode: "base-plus-tests".to_owned(),
        requested_by: task.requested_by.clone(),
        request_ids: task.request_ids.clone(),
        commands,
        result,
        reason,
    }
}

struct ProofCommandPaths {
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    stdout_rel: String,
    stderr_rel: String,
}

struct ProofCommandSpec {
    argv: Vec<String>,
    env: BTreeMap<String, String>,
}

struct ProofBrokerResult {
    proof_receipts: Vec<ProofReceipt>,
    resource_leases: Vec<ResourceLease>,
}

fn proof_command_paths(out: &Path, receipt_id: &str, side: &str) -> Result<ProofCommandPaths> {
    let rel_dir = format!("proof/{receipt_id}/{side}");
    let dir = out.join(&rel_dir);
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let stdout_path = dir.join("stdout.txt");
    let stderr_path = dir.join("stderr.txt");
    if !stdout_path.exists() {
        fs::write(&stdout_path, b"")?;
    }
    if !stderr_path.exists() {
        fs::write(&stderr_path, b"")?;
    }
    Ok(ProofCommandPaths {
        stdout_path,
        stderr_path,
        stdout_rel: format!("{rel_dir}/stdout.txt"),
        stderr_rel: format!("{rel_dir}/stderr.txt"),
    })
}

fn focused_test_tasks_from_diff(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
    budget: ProofBudget,
) -> Vec<FocusedTestTask> {
    let candidates = focused_test_candidates_from_diff(diff, proof_requests);
    let mut tasks = Vec::new();
    let mut files = BTreeSet::new();
    let mut estimated_seconds = 0_u64;
    for task in candidates {
        if !focused_proof_budget_allows_next(
            tasks.len(),
            &files,
            &task.file,
            estimated_seconds,
            task.mode.command_count(),
            budget,
        ) {
            return tasks;
        }
        files.insert(task.file.clone());
        estimated_seconds = estimated_seconds.saturating_add(
            budget
                .per_command_timeout_sec
                .saturating_mul(task.mode.command_count()),
        );
        tasks.push(task);
    }
    tasks
}

fn focused_test_candidates_from_diff(
    diff: &DiffContext,
    proof_requests: &[ProofRequest],
) -> Vec<FocusedTestTask> {
    let request_groups = proof_request_groups(proof_requests);
    let mut tasks = Vec::new();
    for file in diff
        .changed_files
        .iter()
        .filter(|path| is_bun_focused_test_file(path))
    {
        let names = focused_test_names_for_file(&diff.patch, file);
        if names.is_empty() {
            merge_focused_test_task(
                &mut tasks,
                focused_test_task_with_mode(
                    file,
                    None,
                    FocusedProofMode::RedGreen,
                    &request_groups,
                ),
            );
        } else {
            for name in names {
                merge_focused_test_task(
                    &mut tasks,
                    focused_test_task_with_mode(
                        file,
                        Some(name),
                        FocusedProofMode::RedGreen,
                        &request_groups,
                    ),
                );
            }
        }
    }
    merge_focused_test_request_group_tasks(&mut tasks, &request_groups);
    tasks
}

fn focused_test_candidates_from_requests(proof_requests: &[ProofRequest]) -> Vec<FocusedTestTask> {
    let request_groups = proof_request_groups(proof_requests);
    let mut tasks = Vec::new();
    merge_focused_test_request_group_tasks(&mut tasks, &request_groups);
    tasks
}

fn merge_focused_test_request_group_tasks(
    tasks: &mut Vec<FocusedTestTask>,
    request_groups: &[ProofRequestGroup],
) {
    for group in request_groups {
        let Some(target) = focused_test_request_target(group) else {
            continue;
        };
        merge_focused_test_task(
            tasks,
            FocusedTestTask {
                id: focused_test_task_id(
                    &target.file,
                    target.test_name.as_deref(),
                    FocusedProofMode::RedGreen,
                ),
                file: target.file,
                test_name: target.test_name,
                mode: FocusedProofMode::RedGreen,
                requested_by: group.requested_by.clone(),
                request_ids: group.request_ids.clone(),
            },
        );
    }
}

fn focused_proof_budget_allows_next(
    current_tasks: usize,
    current_files: &BTreeSet<String>,
    next_file: &str,
    estimated_seconds: u64,
    next_command_count: u64,
    budget: ProofBudget,
) -> bool {
    current_tasks < budget.max_focused_tests
        && (current_files.contains(next_file)
            || current_files.len() < budget.max_focused_test_files)
        && estimated_seconds
            .saturating_add(budget.per_command_timeout_sec)
            .saturating_add(
                budget
                    .per_command_timeout_sec
                    .saturating_mul(next_command_count.saturating_sub(1)),
            )
            <= budget.max_total_seconds
}

#[cfg(test)]
fn focused_test_task(
    file: &str,
    test_name: Option<String>,
    request_groups: &[ProofRequestGroup],
) -> FocusedTestTask {
    focused_test_task_with_mode(file, test_name, FocusedProofMode::RedGreen, request_groups)
}

fn focused_test_task_with_mode(
    file: &str,
    test_name: Option<String>,
    mode: FocusedProofMode,
    request_groups: &[ProofRequestGroup],
) -> FocusedTestTask {
    let mut requested_by = Vec::new();
    let mut request_ids = Vec::new();
    for group in request_groups {
        if group.status == "requested"
            && group.command.contains(file)
            && test_name
                .as_ref()
                .is_none_or(|name| group.command.contains(name))
        {
            for lane in &group.requested_by {
                push_unique(&mut requested_by, lane);
            }
            for id in &group.request_ids {
                push_unique(&mut request_ids, id);
            }
        }
    }
    if requested_by.is_empty() {
        requested_by.push("proof-broker".to_owned());
    }
    FocusedTestTask {
        id: focused_test_task_id(file, test_name.as_deref(), mode),
        file: file.to_owned(),
        test_name,
        mode,
        requested_by,
        request_ids,
    }
}

fn focused_test_task_id(file: &str, test_name: Option<&str>, mode: FocusedProofMode) -> String {
    let fingerprint = sha256_hex(format!("{file}\n{}", test_name.unwrap_or("")).as_bytes());
    let prefix = match mode {
        FocusedProofMode::HeadOnly => "proof-head",
        FocusedProofMode::RedGreen => "proof-red-green",
    };
    format!("{prefix}-{}", &fingerprint[..12])
}

fn merge_focused_test_task(tasks: &mut Vec<FocusedTestTask>, mut task: FocusedTestTask) {
    if let Some(existing) = tasks
        .iter_mut()
        .find(|existing| existing.file == task.file && existing.test_name == task.test_name)
    {
        if existing.mode == FocusedProofMode::HeadOnly && task.mode == FocusedProofMode::RedGreen {
            existing.mode = FocusedProofMode::RedGreen;
            existing.id =
                focused_test_task_id(&existing.file, existing.test_name.as_deref(), existing.mode);
        }
        for lane in task.requested_by.drain(..) {
            push_unique(&mut existing.requested_by, &lane);
        }
        for request_id in task.request_ids.drain(..) {
            push_unique(&mut existing.request_ids, &request_id);
        }
        return;
    }
    tasks.push(task);
}

#[derive(Clone, Debug)]
struct FocusedTestRequestTarget {
    file: String,
    test_name: Option<String>,
}

fn focused_test_request_target(group: &ProofRequestGroup) -> Option<FocusedTestRequestTarget> {
    if group.status != "requested" || group.cost != "focused-test" {
        return None;
    }
    let parts = group.command.split_whitespace().collect::<Vec<_>>();
    let (file, args) = match parts.as_slice() {
        ["bun", "test", file, args @ ..] => (*file, args),
        ["bun", "bd", "test", file, args @ ..] => (*file, args),
        _ => return None,
    };
    if !is_bun_focused_test_file(file) {
        return None;
    }
    Some(FocusedTestRequestTarget {
        file: normalize_repo_path(file),
        test_name: focused_test_name_arg(args),
    })
}

fn focused_test_name_arg(args: &[&str]) -> Option<String> {
    let index = args
        .iter()
        .position(|arg| matches!(*arg, "-t" | "--test-name-pattern"))?;
    let mut tokens = Vec::new();
    for token in &args[index + 1..] {
        if token.starts_with('-') {
            break;
        }
        tokens.push(*token);
    }
    let joined = tokens.join(" ");
    let value = strip_matching_quotes(joined.trim());
    (!value.is_empty()).then(|| value.to_owned())
}

fn strip_matching_quotes(value: &str) -> &str {
    if value.len() < 2 {
        return value;
    }
    let bytes = value.as_bytes();
    if matches!(
        (bytes.first(), bytes.last()),
        (Some(b'\''), Some(b'\'')) | (Some(b'"'), Some(b'"'))
    ) {
        &value[1..value.len() - 1]
    } else {
        value
    }
}

fn focused_test_resource_lease(
    task: &FocusedTestTask,
    budget: ProofBudget,
    lease_budget: ProofLeaseBudget,
    status: &str,
    reason: &str,
) -> ResourceLease {
    ResourceLease {
        schema: "ub-review.resource_lease.v1".to_owned(),
        id: format!("lease-{}", task.id),
        kind: "focused-test".to_owned(),
        consumer: task.id.clone(),
        status: status.to_owned(),
        reason: reason.to_owned(),
        cpu: lease_budget.cpu,
        memory_mb: lease_budget.memory_mb,
        disk_mb: lease_budget.disk_mb,
        timeout_sec: budget
            .per_command_timeout_sec
            .saturating_mul(task.mode.command_count())
            .min(budget.max_total_seconds),
        network: lease_budget.network,
        scratch: lease_budget.scratch,
        worktree: if task.mode == FocusedProofMode::RedGreen {
            Some("base-plus-tests".to_owned())
        } else {
            None
        },
        command: Some(match task.mode {
            FocusedProofMode::HeadOnly => {
                format!("head: {}", proof_task_plan_command(task, "head", "head"))
            }
            FocusedProofMode::RedGreen => format!(
                "head: {}; base+tests: {}",
                proof_task_plan_command(task, "head", "head"),
                proof_task_plan_command(task, "base-plus-tests", "base-plus-tests")
            ),
        }),
    }
}

fn focused_test_names_for_file(patch: &str, file: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut current_path = String::new();
    for line in patch.lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            current_path = normalize_repo_path(path);
            continue;
        }
        if current_path != file || !line.starts_with('+') || line.starts_with("+++") {
            continue;
        }
        if let Some(name) = extract_focused_test_name(&line[1..]) {
            push_unique(&mut names, &name);
        }
    }
    names
}

fn extract_focused_test_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    for prefix in ["test(", "it(", "describe("] {
        if let Some(rest) = trimmed.strip_prefix(prefix)
            && let Some(name) = parse_js_string_literal(rest.trim_start())
        {
            return Some(name);
        }
    }
    None
}

fn parse_js_string_literal(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let quote = chars.next()?;
    if !matches!(quote, '"' | '\'') {
        return None;
    }
    let mut escaped = false;
    let mut out = String::new();
    for ch in chars {
        if escaped {
            out.push(ch);
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == quote {
            return Some(out.trim().to_owned()).filter(|value| !value.is_empty());
        } else {
            out.push(ch);
        }
    }
    None
}

fn is_bun_focused_test_file(path: &str) -> bool {
    let path = normalize_repo_path(path);
    if !is_repo_relative_path(&path) {
        return false;
    }
    let lower = path.to_ascii_lowercase();
    (lower.starts_with("test/") || lower.starts_with("tests/"))
        && [
            ".test.ts",
            ".test.tsx",
            ".test.js",
            ".test.jsx",
            ".test.mjs",
            ".test.cjs",
        ]
        .iter()
        .any(|suffix| lower.ends_with(suffix))
}

fn proof_task_command_spec(task: &FocusedTestTask, side: &str) -> ProofCommandSpec {
    let mut env = BTreeMap::new();
    let mut argv = if side == "head" {
        vec![
            "bun".to_owned(),
            "bd".to_owned(),
            "test".to_owned(),
            task.file.clone(),
        ]
    } else {
        env.insert("USE_SYSTEM_BUN".to_owned(), "1".to_owned());
        vec!["bun".to_owned(), "test".to_owned(), task.file.clone()]
    };
    if let Some(name) = &task.test_name {
        argv.push("-t".to_owned());
        argv.push(name.clone());
    }
    ProofCommandSpec { argv, env }
}

fn proof_task_plan_command(task: &FocusedTestTask, side: &str, worktree: &str) -> String {
    let spec = proof_task_command_spec(task, side);
    format!(
        "cwd=target/ub-review/proof-worktrees/{worktree} {}",
        command_display_with_env(&spec.env, &spec.argv)
    )
}

fn prepare_base_plus_tests_worktree(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
) -> Result<PathBuf> {
    let patch_files = base_plus_tests_patch_files(diff);
    let worktrees_dir = out.join("proof-worktrees");
    fs::create_dir_all(&worktrees_dir)
        .with_context(|| format!("create {}", worktrees_dir.display()))?;
    let worktree = worktrees_dir.join("base-plus-tests");
    if worktree.exists() {
        let _ = cleanup_base_plus_tests_worktree(root, &worktree);
        if worktree.exists() {
            safe_remove_dir_all_under(&worktrees_dir, &worktree)?;
        }
    }

    let add_args = vec![
        "worktree".to_owned(),
        "add".to_owned(),
        "--detach".to_owned(),
        worktree.to_string_lossy().to_string(),
        diff.base.clone(),
    ];
    git_text_owned(root, &add_args).with_context(|| {
        format!(
            "create base+tests worktree at {} from {}",
            worktree.display(),
            diff.base
        )
    })?;

    if !patch_files.is_empty() {
        let patch = base_plus_tests_patch(root, diff, &patch_files)?;
        let proof_dir = out.join("proof");
        fs::create_dir_all(&proof_dir)
            .with_context(|| format!("create {}", proof_dir.display()))?;
        let patch_path = proof_dir.join("base-plus-tests.patch");
        fs::write(&patch_path, patch).with_context(|| format!("write {}", patch_path.display()))?;

        let apply_args = vec![
            "apply".to_owned(),
            "--whitespace=nowarn".to_owned(),
            patch_path.to_string_lossy().to_string(),
        ];
        if let Err(error) = git_text_owned(&worktree, &apply_args)
            .with_context(|| format!("apply test-only patch in {}", worktree.display()))
        {
            let _ = cleanup_base_plus_tests_worktree(root, &worktree);
            return Err(error);
        }
    }

    Ok(worktree)
}

fn base_plus_tests_patch(root: &Path, diff: &DiffContext, files: &[String]) -> Result<String> {
    let mut args = vec![
        "diff".to_owned(),
        "--patch".to_owned(),
        format!("{}...{}", diff.base, diff.head),
        "--".to_owned(),
    ];
    args.extend(files.iter().cloned());
    let patch = git_text_owned(root, &args).or_else(|_| {
        let mut fallback = vec![
            "diff".to_owned(),
            "--patch".to_owned(),
            diff.base.clone(),
            diff.head.clone(),
            "--".to_owned(),
        ];
        fallback.extend(files.iter().cloned());
        git_text_owned(root, &fallback)
    })?;
    if patch.trim().is_empty() {
        bail!("test-only diff for base+tests worktree was empty");
    }
    Ok(patch)
}

fn base_plus_tests_patch_files(diff: &DiffContext) -> Vec<String> {
    diff.changed_files
        .iter()
        .filter(|path| is_base_plus_tests_patch_file(path))
        .cloned()
        .collect()
}

fn is_base_plus_tests_patch_file(path: &str) -> bool {
    let path = normalize_repo_path(path);
    if !is_repo_relative_path(&path) {
        return false;
    }
    if is_bun_focused_test_file(&path) {
        return true;
    }
    let lower = path.to_ascii_lowercase();
    lower.starts_with("test/")
        || lower.starts_with("tests/")
        || lower.starts_with("fixtures/")
        || lower.contains("/fixtures/")
        || lower.contains("/fixture/")
        || lower.contains("doc-test")
        || lower.contains("doctest")
}

fn cleanup_base_plus_tests_worktree(root: &Path, worktree: &Path) -> Result<()> {
    let worktree_arg = worktree.to_string_lossy().to_string();
    let remove_args = vec![
        "worktree".to_owned(),
        "remove".to_owned(),
        "--force".to_owned(),
        worktree_arg,
    ];
    let _ = git_text_owned(root, &remove_args);
    if worktree.exists() {
        let parent = worktree
            .parent()
            .context("base+tests worktree had no parent directory")?;
        safe_remove_dir_all_under(parent, worktree)?;
    }
    let prune_args = vec!["worktree".to_owned(), "prune".to_owned()];
    let _ = git_text_owned(root, &prune_args);
    Ok(())
}

fn safe_remove_dir_all_under(parent: &Path, target: &Path) -> Result<()> {
    let parent_abs = parent
        .canonicalize()
        .with_context(|| format!("resolve {}", parent.display()))?;
    let target_abs = target
        .canonicalize()
        .with_context(|| format!("resolve {}", target.display()))?;
    if !target_abs.starts_with(&parent_abs) {
        bail!(
            "refusing to remove {} outside {}",
            target_abs.display(),
            parent_abs.display()
        );
    }
    fs::remove_dir_all(&target_abs).with_context(|| format!("remove {}", target_abs.display()))?;
    Ok(())
}

fn command_display(argv: &[String]) -> String {
    argv.iter()
        .map(|part| {
            if part
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '.' | '_' | '-' | ':'))
            {
                part.clone()
            } else {
                format!("'{}'", part.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn command_display_with_env(env: &BTreeMap<String, String>, argv: &[String]) -> String {
    if env.is_empty() {
        return command_display(argv);
    }
    let mut parts = env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    parts.push(command_display(argv));
    parts.join(" ")
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_owned());
    }
}

#[derive(Clone, Debug, Serialize)]
struct ObservationGroup {
    schema: String,
    id: String,
    dedupe_key: String,
    claim: String,
    kind: String,
    status: String,
    severity: String,
    confidence: String,
    path: Option<String>,
    line: Option<u32>,
    evidence: Vec<String>,
    lanes: Vec<String>,
    sources: Vec<String>,
    observation_ids: Vec<String>,
    duplicate_count: usize,
}

#[derive(Clone, Debug, Serialize)]
struct MergedObservationRecord {
    schema: String,
    group_id: String,
    dedupe_key: String,
    kept_observation_id: String,
    merged_observation_ids: Vec<String>,
    lanes: Vec<String>,
    reason: String,
}

#[derive(Clone, Debug, Serialize)]
struct DroppedObservationRecord {
    schema: String,
    observation_id: String,
    group_id: String,
    dedupe_key: String,
    lane: String,
    reason: String,
}

struct ObservationSummaryArtifacts {
    unique: Vec<ObservationGroup>,
    merged: Vec<MergedObservationRecord>,
    dropped: Vec<DroppedObservationRecord>,
}

fn observation_summary_artifacts(observations: &[Observation]) -> ObservationSummaryArtifacts {
    let mut indexes = BTreeMap::new();
    let mut groups = Vec::<ObservationGroup>::new();
    for observation in observations {
        let key = observation_group_key(observation);
        if let Some(index) = indexes.get(&key).copied() {
            merge_review_observation(&mut groups[index], observation);
        } else {
            let group_id = observation_group_id(groups.len(), &key);
            indexes.insert(key.clone(), groups.len());
            groups.push(ObservationGroup {
                schema: "ub-review.observation_group.v1".to_owned(),
                id: group_id,
                dedupe_key: key,
                claim: observation.claim.clone(),
                kind: observation.kind.clone(),
                status: observation.status.clone(),
                severity: observation.severity.clone(),
                confidence: observation.confidence.clone(),
                path: observation.path.clone(),
                line: observation.line,
                evidence: observation.evidence.iter().take(3).cloned().collect(),
                lanes: vec![observation.lane.clone()],
                sources: vec![observation.source.clone()],
                observation_ids: vec![observation.id.clone()],
                duplicate_count: 0,
            });
        }
    }
    let merged = groups
        .iter()
        .filter(|group| group.observation_ids.len() > 1)
        .map(|group| MergedObservationRecord {
            schema: "ub-review.merged_observation.v1".to_owned(),
            group_id: group.id.clone(),
            dedupe_key: group.dedupe_key.clone(),
            kept_observation_id: group.observation_ids[0].clone(),
            merged_observation_ids: group.observation_ids[1..].to_vec(),
            lanes: group.lanes.clone(),
            reason: "merged_duplicate_dedupe_key".to_owned(),
        })
        .collect::<Vec<_>>();
    let observation_lanes = observations
        .iter()
        .map(|observation| (observation.id.as_str(), observation.lane.as_str()))
        .collect::<BTreeMap<_, _>>();
    let dropped = groups
        .iter()
        .flat_map(|group| {
            group
                .observation_ids
                .iter()
                .skip(1)
                .map(|observation_id| DroppedObservationRecord {
                    schema: "ub-review.dropped_observation.v1".to_owned(),
                    observation_id: observation_id.clone(),
                    group_id: group.id.clone(),
                    dedupe_key: group.dedupe_key.clone(),
                    lane: observation_lanes
                        .get(observation_id.as_str())
                        .copied()
                        .unwrap_or("unknown")
                        .to_owned(),
                    reason: "merged_into_unique_observation".to_owned(),
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    ObservationSummaryArtifacts {
        unique: groups,
        merged,
        dropped,
    }
}

fn unique_review_observations(observations: &[Observation]) -> Vec<ObservationGroup> {
    observation_summary_artifacts(observations).unique
}

fn observation_group_key(observation: &Observation) -> String {
    if observation.dedupe_key.trim().is_empty() {
        observation.fingerprint.clone()
    } else {
        observation.dedupe_key.clone()
    }
}

fn observation_group_id(index: usize, dedupe_key: &str) -> String {
    let digest = sha256_hex(dedupe_key.as_bytes());
    format!("obsgrp-{index:04}-{}", &digest[..12])
}

fn merge_review_observation(group: &mut ObservationGroup, observation: &Observation) {
    if severity_rank(&observation.severity) > severity_rank(&group.severity) {
        group.severity = observation.severity.clone();
    }
    if confidence_rank(&observation.confidence) > confidence_rank(&group.confidence) {
        group.confidence = observation.confidence.clone();
    }
    if observation_status_rank(&observation.status) > observation_status_rank(&group.status) {
        group.status = observation.status.clone();
    }
    if group.path.is_none() {
        group.path = observation.path.clone();
    }
    if group.line.is_none() {
        group.line = observation.line;
    }
    if !group.lanes.contains(&observation.lane) {
        group.lanes.push(observation.lane.clone());
    }
    if !group.sources.contains(&observation.source) {
        group.sources.push(observation.source.clone());
    }
    group.observation_ids.push(observation.id.clone());
    group.duplicate_count = group.observation_ids.len().saturating_sub(1);
    for evidence in &observation.evidence {
        if group.evidence.len() >= 3 {
            break;
        }
        if !group.evidence.contains(evidence) {
            group.evidence.push(evidence.clone());
        }
    }
}

fn observation_status_rank(value: &str) -> u8 {
    match value {
        "refuted" => 7,
        "confirmed" => 6,
        "parked" => 5,
        "demoted" => 4,
        "open" => 3,
        "covered" => 2,
        "duplicate" => 1,
        _ => 0,
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
            | "degraded"
            | "invalid_json"
            | "timed_out"
            | "rate_limited"
            | "auth_failed"
            | "bad_envelope"
    )
}

fn write_github_review_skip_receipt(
    review_dir: &Path,
    receipt: GitHubReviewSkipReceipt,
) -> Result<()> {
    let review_json = review_dir.join("github-review.json");
    if review_json.exists() {
        fs::remove_file(&review_json)?;
    }
    fs::write(
        github_review_skip_path(&review_json),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(())
}

fn build_github_review_skip_receipt(
    args: &RunArgs,
    review: &ReviewArtifacts,
) -> GitHubReviewSkipReceipt {
    GitHubReviewSkipReceipt {
        schema_version: 1,
        status: "skipped".to_owned(),
        reason: review.terminal_state.reason.clone(),
        review_payload_status: review.terminal_state.review_payload_status.clone(),
        terminal_state: review.terminal_state.status.clone(),
        github_review_json: "review/github-review.json".to_owned(),
        model_mode: args.model_mode.key().to_owned(),
        inline_comments: review.inline_comments.len(),
        summary_only_findings: review.summary_only_findings.len(),
        missing_or_failed_sensor_evidence: review.missing_or_failed_sensor_evidence.len(),
        missing_or_failed_model_evidence: review.missing_or_failed_model_evidence.len(),
    }
}

fn github_review_skip_path(review_json: &Path) -> PathBuf {
    review_json
        .parent()
        .map(|dir| dir.join("github-review-skip.json"))
        .unwrap_or_else(|| PathBuf::from("github-review-skip.json"))
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
fn render_shared_context(
    root: &Path,
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    plan: &Plan,
    running_summary: &str,
    args: &RunArgs,
    pr_thread_context: &PrThreadContext,
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
    text.push_str(&format!("- Diff class: `{}`\n", diff.diff_class.key()));
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
    text.push_str(&format!(
        "\n## {} Review Posture\n\n",
        diff_class_posture_heading(diff.diff_class)
    ));
    text.push_str(review_posture_for_diff_class(diff.diff_class));
    text.push_str("\n\n## PR Thread Context\n\n");
    text.push_str(&render_pr_thread_context(pr_thread_context));
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

fn collect_pr_thread_context(root: &Path, args: &RunArgs) -> Result<PrThreadContext> {
    let mut context = PrThreadContext {
        schema: "ub-review.pr_thread_context.v1".to_owned(),
        status: "absent".to_owned(),
        max_bytes: args.pr_thread_context_max_bytes,
        sources: Vec::new(),
        warnings: Vec::new(),
        pull_number: None,
        title: None,
        body: None,
        body_truncated: false,
        thread_context_path: None,
        thread_context: None,
        thread_context_truncated: false,
    };

    if let Some(event_path) = std::env::var_os("GITHUB_EVENT_PATH") {
        let event_path = PathBuf::from(event_path);
        context
            .sources
            .push(format!("github-event:{}", event_path.display()));
        match read_github_event_pr_context(&event_path, args.pr_thread_context_max_bytes) {
            Ok(event_context) => {
                context.pull_number = event_context.pull_number;
                context.title = event_context.title;
                context.body = event_context.body;
                context.body_truncated = event_context.body_truncated;
            }
            Err(err) => context
                .warnings
                .push(format!("github-event unavailable: {err}")),
        }
    }
    context.pull_number = args.github_pull_number.or(context.pull_number);

    let configured_thread_path = args.pr_thread_context.trim();
    if !configured_thread_path.is_empty() {
        let configured_path = PathBuf::from(configured_thread_path);
        let path = if configured_path.is_absolute() {
            configured_path
        } else {
            root.join(configured_path)
        };
        context
            .sources
            .push(format!("thread-context-file:{}", path.display()));
        context.thread_context_path = Some(path.display().to_string());
        match read_bounded_text_with_status(&path, args.pr_thread_context_max_bytes) {
            Ok(text) => {
                context.thread_context = Some(text.text);
                context.thread_context_truncated = text.truncated;
            }
            Err(err) => context
                .warnings
                .push(format!("thread-context-file unavailable: {err}")),
        }
    }

    match github_thread_api_request(args, context.pull_number) {
        None => {}
        Some(Err(err)) => context
            .warnings
            .push(format!("github-api thread context unavailable: {err}")),
        Some(Ok(request)) => {
            match read_github_pr_thread_context(root, &request, args.pr_thread_context_max_bytes) {
                Ok(api_context) => {
                    context.sources.extend(api_context.sources);
                    append_thread_context(
                        &mut context,
                        &api_context.thread_context,
                        args.pr_thread_context_max_bytes,
                    );
                }
                Err(err) => context
                    .warnings
                    .push(format!("github-api thread context unavailable: {err}")),
            }
        }
    }

    context.status =
        if context.title.is_some() || context.body.is_some() || context.thread_context.is_some() {
            "seeded".to_owned()
        } else if context.warnings.is_empty() {
            "absent".to_owned()
        } else {
            "unavailable".to_owned()
        };

    Ok(context)
}

struct GitHubThreadApiRequest<'a> {
    auth: &'a str,
    repo: &'a str,
    pull_number: u64,
    api_url: &'a str,
}

struct GitHubThreadApiContext {
    sources: Vec<String>,
    thread_context: String,
}

fn github_thread_api_request<'a>(
    args: &'a RunArgs,
    event_pull_number: Option<u64>,
) -> Option<Result<GitHubThreadApiRequest<'a>>> {
    let auth = args
        .pr_thread_auth
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    let Some(repo) = args
        .github_repo
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return Some(Err(anyhow::anyhow!(
            "GitHub repository slug is unavailable"
        )));
    };
    if !is_valid_repo_slug(repo) {
        return Some(Err(anyhow::anyhow!(
            "GitHub repository slug is invalid: {repo}"
        )));
    }
    let Some(pull_number) = args.github_pull_number.or(event_pull_number) else {
        return Some(Err(anyhow::anyhow!("pull request number is unavailable")));
    };
    Some(Ok(GitHubThreadApiRequest {
        auth,
        repo,
        pull_number,
        api_url: args.github_api_url.trim_end_matches('/'),
    }))
}

fn read_github_pr_thread_context(
    root: &Path,
    request: &GitHubThreadApiRequest<'_>,
    max_bytes: usize,
) -> Result<GitHubThreadApiContext> {
    let endpoints = [
        (
            "issue-comments",
            format!(
                "{}/repos/{}/issues/{}/comments?per_page=30",
                request.api_url, request.repo, request.pull_number
            ),
        ),
        (
            "review-summaries",
            format!(
                "{}/repos/{}/pulls/{}/reviews?per_page=30",
                request.api_url, request.repo, request.pull_number
            ),
        ),
        (
            "review-comments",
            format!(
                "{}/repos/{}/pulls/{}/comments?per_page=50",
                request.api_url, request.repo, request.pull_number
            ),
        ),
    ];
    let mut sections = Vec::new();
    let mut sources = Vec::new();
    for (kind, url) in endpoints {
        let value = run_github_api_get(root, &url, request.auth)
            .with_context(|| format!("fetch GitHub PR thread {kind}"))?;
        sources.push(format!(
            "github-api:{}/{}/{}",
            request.repo, request.pull_number, kind
        ));
        sections.push(render_github_pr_thread_section(kind, &value, max_bytes));
    }

    let mut text = String::new();
    text.push_str("## GitHub PR Thread Snapshot\n\n");
    text.push_str(&format!(
        "Source: `{}` PR `#{}`. Bounded to lane context; full GitHub thread remains source of truth.\n\n",
        escape_md(request.repo),
        request.pull_number
    ));
    text.push_str(&sections.join("\n"));
    let bounded = bounded_string(&text, max_bytes);
    Ok(GitHubThreadApiContext {
        sources,
        thread_context: bounded.text,
    })
}

fn run_github_api_get(root: &Path, url: &str, auth: &str) -> Result<serde_json::Value> {
    let mut command = ProcessCommand::new("curl");
    command
        .arg("-sS")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg("30")
        .arg("-w")
        .arg("\nUB_REVIEW_HTTP_STATUS:%{http_code}\n")
        .arg("-K")
        .arg("-")
        .arg(url)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().with_context(|| "spawn GitHub API curl")?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("curl stdin unavailable"))?;
        use std::io::Write as _;
        const AUTH_HEADER_NAME: &str = "Authorization";
        let auth_scheme = ["Bear", "er"].concat();
        for header in [
            "Accept: application/vnd.github+json",
            "X-GitHub-Api-Version: 2022-11-28",
            &format!("{AUTH_HEADER_NAME}: {auth_scheme} {auth}"),
        ] {
            writeln!(stdin, "header = \"{}\"", curl_config_quote(header))?;
        }
    }
    let output = child
        .wait_with_output()
        .with_context(|| "wait for GitHub API curl")?;
    let (stdout, http_status) = split_curl_http_status(output.stdout);
    if !output.status.success() {
        bail!(
            "GitHub API curl exited {:?} with http status {:?}: stderr: {}; stdout: {}",
            output.status.code(),
            http_status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&stdout)
        );
    }
    serde_json::from_slice(&stdout).with_context(|| "parse GitHub API response")
}

fn render_github_pr_thread_section(
    kind: &str,
    value: &serde_json::Value,
    max_bytes: usize,
) -> String {
    let title = match kind {
        "issue-comments" => "Issue Comments",
        "review-summaries" => "Review Summaries",
        "review-comments" => "Review Comments",
        _ => "Thread Items",
    };
    let mut text = format!("### {title}\n\n");
    let Some(items) = value.as_array() else {
        text.push_str("- GitHub response was not an array.\n");
        return text;
    };
    if items.is_empty() {
        text.push_str("- None found.\n");
        return text;
    }
    for item in items {
        text.push_str(&render_github_pr_thread_item(kind, item, max_bytes));
    }
    text
}

fn render_github_pr_thread_item(kind: &str, item: &serde_json::Value, max_bytes: usize) -> String {
    let author = item
        .pointer("/user/login")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let created_at = item
        .get("created_at")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown-time");
    let state = item
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let path = item
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let line = item
        .get("line")
        .or_else(|| item.get("original_line"))
        .and_then(serde_json::Value::as_u64);
    let body = item
        .get("body")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let bounded_body = bounded_string(body.trim(), max_bytes.min(1200));
    let location = if !path.is_empty() {
        match line {
            Some(line) => format!(" `{}`:`{line}`", escape_md(path)),
            None => format!(" `{}`", escape_md(path)),
        }
    } else {
        String::new()
    };
    let state = if state.is_empty() {
        String::new()
    } else {
        format!(" `{}`", escape_md(state))
    };
    let item_kind = match kind {
        "issue-comments" => "issue-comment",
        "review-summaries" => "review",
        "review-comments" => "review-comment",
        _ => "thread-item",
    };
    let mut text = format!(
        "- `{}` `{}` by `{}`{}{}\n",
        item_kind,
        escape_md(created_at),
        escape_md(author),
        state,
        location
    );
    if !bounded_body.text.is_empty() {
        text.push_str("  ```text\n");
        text.push_str(&bounded_body.text);
        if !bounded_body.text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("  ```\n");
    }
    text
}

fn append_thread_context(context: &mut PrThreadContext, addition: &str, max_bytes: usize) {
    if addition.trim().is_empty() {
        return;
    }
    let mut merged = String::new();
    if let Some(existing) = context.thread_context.as_deref() {
        merged.push_str(existing);
        if !existing.ends_with('\n') {
            merged.push('\n');
        }
        merged.push('\n');
    }
    merged.push_str(addition);
    let bounded = bounded_string(&merged, max_bytes);
    context.thread_context = Some(bounded.text);
    context.thread_context_truncated |= bounded.truncated;
}

struct GitHubEventPrContext {
    pull_number: Option<u64>,
    title: Option<String>,
    body: Option<String>,
    body_truncated: bool,
}

fn read_github_event_pr_context(path: &Path, max_bytes: usize) -> Result<GitHubEventPrContext> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let Some(pull_request) = value.get("pull_request") else {
        return Ok(GitHubEventPrContext {
            pull_number: None,
            title: None,
            body: None,
            body_truncated: false,
        });
    };
    let body = pull_request
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(|body| bounded_string(body, max_bytes));
    Ok(GitHubEventPrContext {
        pull_number: pull_request
            .get("number")
            .and_then(serde_json::Value::as_u64),
        title: pull_request
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        body: body.as_ref().map(|body| body.text.clone()),
        body_truncated: body.as_ref().is_some_and(|body| body.truncated),
    })
}

fn render_pr_thread_context(context: &PrThreadContext) -> String {
    let mut text = String::new();
    text.push_str(&format!("- Status: `{}`\n", context.status));
    if context.sources.is_empty() {
        text.push_str("- Sources: none\n");
    } else {
        text.push_str("- Sources:\n");
        for source in &context.sources {
            text.push_str(&format!("  - `{}`\n", escape_md(source)));
        }
    }
    if !context.warnings.is_empty() {
        text.push_str("- Warnings:\n");
        for warning in &context.warnings {
            text.push_str(&format!("  - {}\n", escape_md(warning)));
        }
    }
    if let Some(number) = context.pull_number {
        text.push_str(&format!("- Pull request: `#{number}`\n"));
    }
    if let Some(title) = context.title.as_deref() {
        text.push_str(&format!("- Title: {}\n", escape_md(title)));
    }
    if let Some(guidance) = pr_thread_reuse_guidance(context) {
        text.push('\n');
        text.push_str(guidance);
    }
    if let Some(body) = context.body.as_deref() {
        text.push_str("\n### PR Body\n\n```text\n");
        text.push_str(body);
        if !body.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```\n");
    }
    if let Some(thread_context) = context.thread_context.as_deref() {
        text.push_str("\n### Prior Review Thread\n\n```text\n");
        text.push_str(thread_context);
        if !thread_context.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```\n");
    }
    if context.status == "absent" {
        text.push_str("- No PR thread context was provided for this run.\n");
    }
    text
}

fn pr_thread_reuse_guidance(context: &PrThreadContext) -> Option<&'static str> {
    if context.status != "seeded" {
        return None;
    }
    Some(
        "### Seeded Thread Reuse Rules\n\n\
- Treat PR body claims, author replies, prior ub-review comments, resolved/dismissed discussion notes, and proof receipts in this context as lane evidence.\n\
- Before emitting a verification question or proof request, compare it with the seeded thread. If the same concern is already answered and the current diff does not reopen it, emit a `resolved-check` observation or `failed_objection` instead of a fresh candidate.\n\
- If the current diff reopens an answered concern, cite the changed file/line or proof receipt that makes the prior answer stale.\n",
    )
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

struct BoundedText {
    text: String,
    truncated: bool,
}

fn read_bounded_text(path: &Path, max_bytes: usize) -> Result<String> {
    read_bounded_text_with_status(path, max_bytes).map(|bounded| bounded.text)
}

fn read_bounded_text_with_status(path: &Path, max_bytes: usize) -> Result<BoundedText> {
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
    Ok(BoundedText {
        text,
        truncated: count > max_bytes,
    })
}

fn bounded_string(value: &str, max_bytes: usize) -> BoundedText {
    if value.len() <= max_bytes {
        return BoundedText {
            text: value.to_owned(),
            truncated: false,
        };
    }
    let mut end = 0;
    for (index, ch) in value.char_indices() {
        let next = index + ch.len_utf8();
        if next > max_bytes {
            break;
        }
        end = next;
    }
    let mut text = value[..end].to_owned();
    text.push_str("\n[truncated]\n");
    BoundedText {
        text,
        truncated: true,
    }
}

fn is_ledger_excerpt_candidate(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|extension| extension.to_str()),
        Some("md" | "txt" | "toml" | "json")
    )
}

fn review_lanes_for_args(plan: &Plan, args: &RunArgs) -> Vec<LanePlan> {
    match (args.lane_width, args.provider_policy) {
        (20, ModelProviderPolicy::OpencodeGoWide) if plan.diff_class == DiffClass::SourceUb => {
            opencode_go_wide_lanes()
        }
        (width, _) => review_lanes_for_width(width, plan),
    }
}

fn selected_review_lanes_for_args(plan: &Plan, args: &RunArgs) -> Result<Vec<LanePlan>> {
    let include = parse_selector_set(&args.selectors.lanes, "--lanes")?;
    let exclude = parse_selector_set(&args.selectors.except_lanes, "--except-lanes")?;
    filter_lane_plans(review_lanes_for_args(plan, args), &include, &exclude)
}

fn review_lanes_for_width(width: usize, plan: &Plan) -> Vec<LanePlan> {
    match plan.diff_class {
        DiffClass::SourceUb => match width {
            6 => plan.lanes.clone(),
            10 => standard_minimax_lanes(),
            20 => deep_minimax_lanes(),
            _ => plan.lanes.clone(),
        },
        DiffClass::SourceGeneral => source_general_lanes(),
        DiffClass::TestsOnly => tests_only_lanes(),
        DiffClass::WorkflowTooling => workflow_tooling_lanes(),
        DiffClass::DocsOnly | DiffClass::ArtifactOnlySmoke => Vec::new(),
    }
}

fn source_general_lanes() -> Vec<LanePlan> {
    vec![
        model_lane(
            "correctness",
            "Changed behavior correctness review",
            &["tokmd", "ripr", "ast-grep"],
            "Review changed behavior, public API route truth, regression risk, and overclaim/underclaim without source-UB assumptions.",
        ),
        model_lane(
            "tests-red-green",
            "Red/green changed-behavior proof review",
            &["tokmd", "ripr"],
            "Check whether tests distinguish old from new behavior and prove the PR claim.",
        ),
        model_lane(
            "source-route",
            "Public API source-route review",
            &["tokmd", "ast-grep", "ripr"],
            "Trace public API routes, changed helper callers, sibling paths, and PR claim truth.",
        ),
        model_lane(
            "architecture",
            "Boundary and smallest-complete-fix review",
            &["tokmd", "ast-grep"],
            "Check boundary placement, helper shape, scope control, duplication risk, and smallest complete fix.",
        ),
        model_lane(
            "opposition",
            "Strongest substantiated objection review",
            &["tokmd", "ripr", "ast-grep"],
            "Try to disprove the PR across correctness, proof, portability, performance, route truth, and overclaim risk.",
        ),
    ]
}

fn tests_only_lanes() -> Vec<LanePlan> {
    vec![
        model_lane(
            "tests-red-green",
            "Red/green test proof review",
            &["tokmd", "ripr"],
            "Check whether added or changed tests fail on unpatched code and pass on patched code.",
        ),
        model_lane(
            "tests-oracle",
            "Test oracle strength review",
            &["tokmd", "ripr"],
            "Look for smoke-only, tautological, reach-only, flaky, or non-discriminating assertions.",
        ),
        model_lane(
            "proof-request",
            "Focused proof request review",
            &["tokmd", "ripr"],
            "Request only cheap focused proof that would change reviewer confidence.",
        ),
        model_lane(
            "opposition",
            "Strongest test-suite objection review",
            &["tokmd", "ripr"],
            "Try to disprove whether the test change proves the claimed behavior.",
        ),
    ]
}

fn workflow_tooling_lanes() -> Vec<LanePlan> {
    vec![
        model_lane(
            "workflow-permissions",
            "Workflow permissions and token-scope review",
            &["tokmd", "actionlint", "zizmor"],
            "Check workflow permissions, fork safety, pull_request_target absence, checkout credential persistence, and non-blocking auxiliary behavior.",
        ),
        model_lane(
            "workflow-pinning",
            "Action pinning and runner setup review",
            &["tokmd", "actionlint", "zizmor"],
            "Check action pinning, trusted setup boundaries, tool installation posture, and runner assumptions.",
        ),
        model_lane(
            "workflow-proof",
            "Workflow lint and smoke proof review",
            &["tokmd", "actionlint"],
            "Check whether actionlint or focused smoke proof is available and whether missing proof affects trust.",
        ),
        model_lane(
            "workflow-opposition",
            "Strongest workflow/tooling objection review",
            &["tokmd", "actionlint", "zizmor"],
            "Try to disprove the workflow/tooling change across permissions, triggers, pinning, checkout, fork-only behavior, and reviewer-value claims.",
        ),
    ]
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

fn model_assignments(plan: &Plan, args: &RunArgs) -> Result<Vec<ModelAssignment>> {
    let lanes = selected_review_lanes_for_args(plan, args)?;
    let assignments = lanes
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
        .collect::<Vec<_>>();
    Ok(assignments)
}

fn provider_spec_for_lane(lane: &LanePlan, args: &RunArgs) -> ProviderSpec {
    provider_spec_for_lane_with_key_state(
        lane,
        args,
        model_api_key_present(ModelProvider::OpenCodeGo),
    )
}

fn provider_spec_for_lane_with_key_state(
    lane: &LanePlan,
    args: &RunArgs,
    opencode_key_present: bool,
) -> ProviderSpec {
    match args.provider_policy {
        ModelProviderPolicy::MinimaxOnly => direct_minimax_spec(args),
        ModelProviderPolicy::Auto | ModelProviderPolicy::MinimaxPrimary
            if lane.id == "opposition" && opencode_key_present =>
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
                    let primary_label = model_api_key_label(spec.provider);
                    if env_value_present(primary_env) {
                        (
                            "planned",
                            format!(
                                "{primary_label} present; lane eligible for {} call",
                                spec.provider.key()
                            ),
                        )
                    } else if let Some(fallback) = &assignment.fallback {
                        let fallback_env = model_api_key_env(fallback.provider);
                        let fallback_label = model_api_key_label(fallback.provider);
                        if env_value_present(fallback_env) {
                            (
                                "planned",
                                format!(
                                    "{primary_label} not provided; fallback {fallback_label} present"
                                ),
                            )
                        } else {
                            (
                                "missing_key",
                                format!(
                                    "{primary_label} and fallback {fallback_label} not provided; lane output unavailable"
                                ),
                            )
                        }
                    } else {
                        (
                            "missing_key",
                            format!(
                                "{primary_label} not provided; {} lane output unavailable",
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
                    let key_label = model_api_key_label(spec.provider);
                    if env_value_present(env_name) {
                        ("planned", format!("{key_label} present; preflight planned"))
                    } else {
                        (
                            "missing_key",
                            format!("{key_label} not provided; provider unavailable"),
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

fn is_model_receipt_evidence_issue(receipt: &ModelLaneReceipt) -> bool {
    is_model_evidence_issue(&receipt.status) || is_model_skipped_evidence_issue(receipt)
}

fn is_model_skipped_evidence_issue(receipt: &ModelLaneReceipt) -> bool {
    receipt.status == "skipped"
        && matches!(
            receipt.reason.as_str(),
            "model-mode off"
                | "model call budget or inline comment cap reached before lane execution"
                | "model call budget exhausted before refuter pass"
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
            let reason = receipt
                .map(|receipt| receipt.reason)
                .unwrap_or_else(|| sensor.reason.clone());
            if !is_sensor_evidence_issue(sensor, &status, &reason) {
                return None;
            }
            Some(SensorEvidenceIssue {
                sensor: sensor.id.clone(),
                status,
                reason,
            })
        })
        .collect()
}

fn is_sensor_evidence_issue(sensor: &SensorPlan, status: &str, reason: &str) -> bool {
    match status {
        "ok" => false,
        "skipped" => is_sensor_skipped_evidence_issue(sensor, reason),
        _ => true,
    }
}

fn is_sensor_skipped_evidence_issue(sensor: &SensorPlan, reason: &str) -> bool {
    sensor.run || reason == "dry-run; sensor not executed" || reason.starts_with("box guard failed")
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
    model_observations: &mut Vec<Observation>,
    proof_requests: &mut Vec<ProofRequest>,
) -> Result<usize> {
    let model_dir = context.review_dir.join("model");
    fs::create_dir_all(&model_dir)?;
    let mut calls = 0usize;
    let mut next_assignment = 0usize;
    loop {
        if calls >= context.args.max_model_calls || next_assignment >= context.assignments.len() {
            break;
        }

        let mut wave = Vec::new();
        while wave.len() < context.args.model_concurrency
            && calls + wave.len() < context.args.max_model_calls
            && next_assignment < context.assignments.len()
        {
            let index = next_assignment;
            next_assignment += 1;
            let assignment = &context.assignments[index];
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
            if !env_value_present(env_name) {
                let key_label = model_api_key_label(spec.provider);
                receipt.status = "missing_key".to_owned();
                receipt.reason = format!(
                    "{key_label} not provided; {} lane output unavailable",
                    spec.provider.key()
                );
                missing_or_failed_model_evidence.push(model_issue_from_receipt(receipt));
                continue;
            }
            receipt.status = "running".to_owned();
            wave.push(ModelLaneTask {
                index,
                lane: lane.clone(),
                spec,
            });
        }

        if wave.is_empty() {
            continue;
        }

        calls += wave.len();
        let mut results = run_model_lane_tasks(&context, &model_dir, wave)?;
        results.sort_by_key(|result| result.index);
        for task_result in results {
            let receipt = &mut model_lanes[task_result.index];
            let lane = &context.assignments[task_result.index].lane;
            match task_result.result {
                Ok(outcome) => {
                    if outcome.degraded {
                        receipt.status = "degraded".to_owned();
                        receipt.reason =
                            "contentful lane output was preserved as degraded evidence".to_owned();
                    } else {
                        receipt.status = "ok".to_owned();
                        receipt.reason = "completed".to_owned();
                    }
                    receipt.duration_ms = Some(outcome.duration_ms);
                    receipt.http_status = outcome.http_status;
                    receipt.response_shape = Some(outcome.response_shape.clone());
                    apply_model_output(
                        lane,
                        outcome.output,
                        context.line_map,
                        context.args.max_inline_comments,
                        ModelOutputSinks {
                            inline_comments,
                            summary_only_findings,
                            model_observations,
                            proof_requests,
                        },
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
    }
    for receipt in model_lanes {
        if receipt.status == "planned" {
            receipt.status = "skipped".to_owned();
            receipt.reason = "model call budget reached before lane execution".to_owned();
            if is_model_receipt_evidence_issue(receipt) {
                missing_or_failed_model_evidence.push(model_issue_from_receipt(receipt));
            }
        }
    }
    Ok(calls)
}

fn run_follow_up_model_pass(
    context: FollowUpRunContext<'_>,
    follow_up_results: &mut Vec<FollowUpResult>,
    follow_up_outputs: &mut Vec<FollowUpOutputRecord>,
) -> Result<usize> {
    let spec = direct_minimax_spec(context.args);
    let available = context
        .args
        .max_model_calls
        .saturating_sub(context.model_calls_used);
    let model_mode_enabled = matches!(context.args.model_mode, ModelMode::Auto);
    let preflight_ready = provider_preflight_ok(&spec, context.provider_preflights);
    let key_present = env_value_present(model_api_key_env(spec.provider));
    let mut calls = 0usize;
    for task in context.tasks {
        let model_lane = follow_up_model_lane_id(task);
        let packet_path = follow_up_packet_artifact_path(task);
        let packet = read_follow_up_packet(context.out, task)?;
        if !model_mode_enabled {
            let result = follow_up_result(
                task,
                &packet_path,
                &model_lane,
                "skipped",
                "model-mode off; follow-up task remains artifact-only",
                FollowUpResultArtifacts::default(),
                FollowUpOutputCounts::default(),
            );
            follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
            follow_up_results.push(result);
            continue;
        }
        if !preflight_ready {
            let reason = provider_preflight_reason(&spec, context.provider_preflights)
                .unwrap_or_else(|| "MiniMax preflight did not succeed".to_owned());
            let result = follow_up_result(
                task,
                &packet_path,
                &model_lane,
                "preflight_failed",
                &reason,
                FollowUpResultArtifacts::default(),
                FollowUpOutputCounts::default(),
            );
            follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
            follow_up_results.push(result);
            continue;
        }
        if !key_present {
            let reason = format!(
                "{} not provided; follow-up task remains artifact-only",
                model_api_key_env(spec.provider)
            );
            let result = follow_up_result(
                task,
                &packet_path,
                &model_lane,
                "missing_key",
                &reason,
                FollowUpResultArtifacts::default(),
                FollowUpOutputCounts::default(),
            );
            follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
            follow_up_results.push(result);
            continue;
        }
        if calls >= available {
            let result = follow_up_result(
                task,
                &packet_path,
                &model_lane,
                "skipped_budget",
                "follow-up model call budget exhausted before task execution",
                FollowUpResultArtifacts::default(),
                FollowUpOutputCounts::default(),
            );
            follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
            follow_up_results.push(result);
            continue;
        }

        let task_dir = context.review_dir.join("model").join(&model_lane);
        fs::create_dir_all(&task_dir)?;
        calls += 1;
        match call_model_prompt(context.root, &task_dir, &spec, &packet.prompt, context.args) {
            Ok(outcome) => {
                let status = if outcome.degraded { "degraded" } else { "ok" };
                let reason = if outcome.degraded {
                    "contentful follow-up output was preserved as degraded evidence"
                } else {
                    "completed"
                };
                let output_counts = follow_up_output_counts(&outcome.output);
                let output_record = follow_up_output_record(
                    task,
                    &model_lane,
                    status,
                    reason,
                    outcome.output,
                    context.line_map,
                    context.args.max_inline_comments,
                );
                let mut result = follow_up_result(
                    task,
                    &packet_path,
                    &model_lane,
                    status,
                    reason,
                    follow_up_result_artifacts(&model_lane, &task_dir),
                    output_counts,
                );
                result.duration_ms = Some(outcome.duration_ms);
                result.http_status = outcome.http_status;
                result.response_shape = Some(outcome.response_shape);
                follow_up_outputs.push(output_record);
                follow_up_results.push(result);
            }
            Err(err) => {
                let status = classify_model_error(&err);
                let mut result = follow_up_result(
                    task,
                    &packet_path,
                    &model_lane,
                    &status,
                    &format!("{err:#}"),
                    follow_up_result_artifacts(&model_lane, &task_dir),
                    FollowUpOutputCounts::default(),
                );
                result.http_status = http_status_from_error(&err);
                follow_up_outputs.push(empty_follow_up_output_record(task, &model_lane, &result));
                follow_up_results.push(result);
            }
        }
    }
    Ok(calls)
}

fn read_follow_up_packet(
    out: &Path,
    task: &FollowUpQuestionTask,
) -> Result<FollowUpQuestionPacketArtifact> {
    let path = out.join(follow_up_packet_artifact_path(task));
    let packet: FollowUpQuestionPacketArtifact = serde_json::from_slice(&fs::read(&path)?)?;
    if packet.schema != "ub-review.follow_up_question_packet.v1"
        || packet.task_id != task.id
        || packet.group_id != task.group_id
        || packet.id != task.id
        || packet.stage != task.stage
        || packet.stage_reason != task.stage_reason
    {
        bail!(
            "follow-up packet {} does not match task {}",
            path.display(),
            task.id
        );
    }
    Ok(packet)
}

fn follow_up_packet_artifact_path(task: &FollowUpQuestionTask) -> String {
    format!(
        "questions/orchestrator-follow-up/{}.json",
        sanitize_artifact_name(&task.id)
    )
}

fn follow_up_model_lane_id(task: &FollowUpQuestionTask) -> String {
    format!(
        "orchestrator-follow-up-{}",
        sanitize_artifact_name(&task.id)
    )
}

#[derive(Default)]
struct FollowUpResultArtifacts {
    request_path: Option<String>,
    response_path: Option<String>,
    content_path: Option<String>,
    normalized_content_path: Option<String>,
    stderr_path: Option<String>,
}

fn follow_up_result_artifacts(model_lane: &str, task_dir: &Path) -> FollowUpResultArtifacts {
    FollowUpResultArtifacts {
        request_path: follow_up_result_artifact_path(model_lane, task_dir, "request.json"),
        response_path: follow_up_result_artifact_path(model_lane, task_dir, "response.json"),
        content_path: follow_up_result_artifact_path(model_lane, task_dir, "content.json"),
        normalized_content_path: follow_up_result_artifact_path(
            model_lane,
            task_dir,
            "content-normalized.json",
        ),
        stderr_path: follow_up_result_artifact_path(model_lane, task_dir, "stderr.txt"),
    }
}

fn follow_up_result_artifact_path(
    model_lane: &str,
    task_dir: &Path,
    file_name: &str,
) -> Option<String> {
    task_dir
        .join(file_name)
        .exists()
        .then(|| format!("review/model/{model_lane}/{file_name}"))
}

fn follow_up_output_counts(output: &LaneModelOutput) -> FollowUpOutputCounts {
    FollowUpOutputCounts {
        observations: output.observations.len(),
        candidate_findings: output.candidate_findings.len() + output.inline_comments.len(),
        summary_only_findings: output.summary_only_findings.len()
            + usize::from(output.summary.is_some()),
        failed_objections: output.failed_objections.len(),
        proof_requests: output.proof_requests.len(),
    }
}

fn follow_up_output_record(
    task: &FollowUpQuestionTask,
    model_lane: &str,
    status: &str,
    reason: &str,
    output: LaneModelOutput,
    line_map: &BTreeSet<(String, u32)>,
    max_inline: usize,
) -> FollowUpOutputRecord {
    let lane = follow_up_lane(task, model_lane);
    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    let mut observations = Vec::new();
    let mut proof_requests = Vec::new();
    apply_model_output(
        &lane,
        output,
        line_map,
        max_inline,
        ModelOutputSinks {
            inline_comments: &mut inline_comments,
            summary_only_findings: &mut summary_only_findings,
            model_observations: &mut observations,
            proof_requests: &mut proof_requests,
        },
    );
    FollowUpOutputRecord {
        schema: "ub-review.follow_up_output.v1".to_owned(),
        task_id: task.id.clone(),
        group_id: task.group_id.clone(),
        stage: task.stage.clone(),
        model_lane: model_lane.to_owned(),
        status: status.to_owned(),
        reason: reason.to_owned(),
        inline_comments,
        summary_only_findings,
        observations,
        proof_requests,
    }
}

fn empty_follow_up_output_record(
    task: &FollowUpQuestionTask,
    model_lane: &str,
    result: &FollowUpResult,
) -> FollowUpOutputRecord {
    FollowUpOutputRecord {
        schema: "ub-review.follow_up_output.v1".to_owned(),
        task_id: task.id.clone(),
        group_id: task.group_id.clone(),
        stage: task.stage.clone(),
        model_lane: model_lane.to_owned(),
        status: result.status.clone(),
        reason: result.reason.clone(),
        inline_comments: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: Vec::new(),
        proof_requests: Vec::new(),
    }
}

fn follow_up_lane(task: &FollowUpQuestionTask, model_lane: &str) -> LanePlan {
    LanePlan {
        id: model_lane.to_owned(),
        role: "Orchestrator follow-up".to_owned(),
        model: "custom:MiniMax-M3-3".to_owned(),
        model_display: "MiniMax-M3".to_owned(),
        receives: vec![
            "orchestrator-plan".to_owned(),
            "routed-evidence".to_owned(),
            "follow-up-question".to_owned(),
        ],
        focus: task.question.clone(),
    }
}

fn follow_up_result(
    task: &FollowUpQuestionTask,
    packet_path: &str,
    model_lane: &str,
    status: &str,
    reason: &str,
    artifacts: FollowUpResultArtifacts,
    output_counts: FollowUpOutputCounts,
) -> FollowUpResult {
    FollowUpResult {
        schema: "ub-review.follow_up_result.v1".to_owned(),
        task_id: task.id.clone(),
        group_id: task.group_id.clone(),
        stage: task.stage.clone(),
        packet_path: packet_path.to_owned(),
        model_lane: model_lane.to_owned(),
        status: status.to_owned(),
        reason: reason.to_owned(),
        duration_ms: None,
        http_status: None,
        response_shape: None,
        request_path: artifacts.request_path,
        response_path: artifacts.response_path,
        content_path: artifacts.content_path,
        normalized_content_path: artifacts.normalized_content_path,
        stderr_path: artifacts.stderr_path,
        output_counts,
    }
}

fn run_model_lane_tasks(
    context: &ModelRunContext<'_>,
    model_dir: &Path,
    tasks: Vec<ModelLaneTask>,
) -> Result<Vec<ModelLaneTaskResult>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = context.args.model_concurrency.max(1).min(tasks.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
    let (tx, rx) = mpsc::channel();
    let results = thread::scope(|scope| {
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            scope.spawn(move || {
                loop {
                    let task = match queue.lock() {
                        Ok(mut queue) => queue.pop_front(),
                        Err(_) => None,
                    };
                    let Some(task) = task else {
                        break;
                    };
                    let lane_dir = model_dir.join(&task.lane.id);
                    let result = fs::create_dir_all(&lane_dir)
                        .with_context(|| format!("create {}", lane_dir.display()))
                        .and_then(|()| {
                            call_model_lane(
                                context.root,
                                &lane_dir,
                                &task.lane,
                                &task.spec,
                                context.shared_context,
                                context.args,
                            )
                        });
                    let _ = tx.send(ModelLaneTaskResult {
                        index: task.index,
                        result,
                    });
                }
            });
        }
        drop(tx);
        rx.into_iter().collect::<Vec<_>>()
    });
    Ok(results)
}

fn run_refuter_pass(
    context: RefuterRunContext<'_>,
    model_lanes: &mut Vec<ModelLaneReceipt>,
    missing_or_failed_model_evidence: &mut Vec<ModelEvidenceIssue>,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) -> Result<usize> {
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
        return Ok(0);
    }
    if context.model_calls_used >= context.args.max_model_calls {
        receipt.status = "skipped".to_owned();
        receipt.reason = "model call budget exhausted before refuter pass".to_owned();
        if is_model_receipt_evidence_issue(&receipt) {
            missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        }
        demote_inline_candidates_for_refuter_unavailable(
            &receipt.reason,
            inline_comments,
            summary_only_findings,
        );
        model_lanes.push(receipt);
        return Ok(0);
    }
    if !provider_preflight_ok(&spec, context.provider_preflights) {
        receipt.status = "preflight_failed".to_owned();
        receipt.reason = provider_preflight_reason(&spec, context.provider_preflights)
            .unwrap_or_else(|| "MiniMax preflight did not succeed".to_owned());
        missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        demote_inline_candidates_for_refuter_unavailable(
            &receipt.reason,
            inline_comments,
            summary_only_findings,
        );
        model_lanes.push(receipt);
        return Ok(0);
    }
    let env_name = model_api_key_env(spec.provider);
    if !env_value_present(env_name) {
        let key_label = model_api_key_label(spec.provider);
        receipt.status = "missing_key".to_owned();
        receipt.reason = format!("{key_label} not provided; refuter output unavailable");
        missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        demote_inline_candidates_for_refuter_unavailable(
            &receipt.reason,
            inline_comments,
            summary_only_findings,
        );
        model_lanes.push(receipt);
        return Ok(0);
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
            demote_inline_candidates_for_refuter_unavailable(
                &receipt.reason,
                inline_comments,
                summary_only_findings,
            );
        }
    }
    model_lanes.push(receipt);
    Ok(1)
}

fn demote_inline_candidates_for_refuter_unavailable(
    reason: &str,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) {
    for comment in std::mem::take(inline_comments) {
        summary_only_findings.push(summary_from_refuted_inline(
            comment,
            &format!("refuter unavailable; candidate kept summary-only: {reason}"),
        ));
    }
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
    let content = call_model_prompt_content(root, lane_dir, spec, prompt, args)?;
    let (output, degraded) =
        parse_lane_model_output_or_degrade(&content.json_payload, &content.parse_path)?;
    Ok(ModelCallOutcome {
        output,
        duration_ms: content.duration_ms,
        http_status: content.http_status,
        response_shape: content.response_shape,
        degraded,
    })
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
    let content = call_model_prompt_content(root, lane_dir, spec, prompt, args)?;
    let parsed_output = serde_json::from_str(&content.json_payload)
        .with_context(|| format!("parse {}", content.parse_path.display()))?;
    Ok(ModelCallOutcome {
        output: parsed_output,
        duration_ms: content.duration_ms,
        http_status: content.http_status,
        response_shape: content.response_shape,
        degraded: false,
    })
}

struct ModelPromptContent {
    json_payload: String,
    parse_path: PathBuf,
    duration_ms: u128,
    http_status: Option<u16>,
    response_shape: String,
}

fn call_model_prompt_content(
    root: &Path,
    lane_dir: &Path,
    spec: &ProviderSpec,
    prompt: &str,
    args: &RunArgs,
) -> Result<ModelPromptContent> {
    let env_name = model_api_key_env(spec.provider);
    let token = env_value(env_name).with_context(|| format!("{env_name} missing"))?;
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
    Ok(ModelPromptContent {
        json_payload,
        parse_path,
        duration_ms,
        http_status: process_output.http_status,
        response_shape,
    })
}

fn parse_lane_model_output_or_degrade(
    json_payload: &str,
    parse_path: &Path,
) -> Result<(LaneModelOutput, bool)> {
    match serde_json::from_str::<LaneModelOutput>(json_payload) {
        Ok(output) => {
            let degraded = output.degraded;
            if degraded || lane_model_output_has_value(&output) {
                Ok((output, degraded))
            } else if lane_model_json_payload_has_content(json_payload) {
                Ok((
                    degraded_lane_model_output(
                        json_payload,
                        "Output parsed as JSON but did not contain recognized lane evidence.",
                        parse_path,
                    ),
                    true,
                ))
            } else {
                Err(anyhow::anyhow!("lane model output was empty or unusable"))
                    .with_context(|| format!("parse {}", parse_path.display()))
            }
        }
        Err(err) if lane_model_raw_content_is_usable(json_payload) => Ok((
            degraded_lane_model_output(json_payload, &format!("Parse error: {err}"), parse_path),
            true,
        )),
        Err(err) => {
            Err(anyhow::Error::new(err)).with_context(|| format!("parse {}", parse_path.display()))
        }
    }
}

fn lane_model_output_has_value(output: &LaneModelOutput) -> bool {
    output
        .summary
        .as_deref()
        .is_some_and(|summary| !summary.trim().is_empty())
        || !output.inline_comments.is_empty()
        || !output.candidate_findings.is_empty()
        || !output.summary_only_findings.is_empty()
        || !output.observations.is_empty()
        || !output.failed_objections.is_empty()
        || !output.proof_requests.is_empty()
}

fn lane_model_json_payload_has_content(json_payload: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(json_payload)
        .ok()
        .is_some_and(|value| lane_model_json_value_has_content(&value))
}

fn lane_model_json_value_has_content(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => true,
        serde_json::Value::String(raw) => !raw.trim().is_empty(),
        serde_json::Value::Array(items) => items.iter().any(lane_model_json_value_has_content),
        serde_json::Value::Object(fields) => fields.values().any(lane_model_json_value_has_content),
    }
}

fn lane_model_raw_content_is_usable(value: &str) -> bool {
    let trimmed = value.trim();
    !trimmed.is_empty() && trimmed.chars().any(char::is_alphabetic)
}

fn degraded_lane_model_output(raw: &str, reason: &str, parse_path: &Path) -> LaneModelOutput {
    LaneModelOutput {
        summary: None,
        inline_comments: Vec::new(),
        candidate_findings: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: vec![lane_output_malformed_content_observation(
            raw, reason, parse_path,
        )],
        failed_objections: Vec::new(),
        proof_requests: Vec::new(),
        degraded: true,
    }
}

fn lane_output_malformed_content_observation(
    raw: &str,
    reason: &str,
    parse_path: &Path,
) -> ModelCandidateObservation {
    let raw = truncate_chars(raw.trim(), 240);
    ModelCandidateObservation {
        claim: truncate_chars(
            &format!(
                "Lane output was contentful but not valid JSON; preserved degraded text: {raw}"
            ),
            320,
        ),
        question: Some("lane-output-shape".to_owned()),
        kind: Some("missing-evidence".to_owned()),
        status: Some("open".to_owned()),
        severity: Some("low".to_owned()),
        confidence: Some("medium".to_owned()),
        path: None,
        line: None,
        evidence: vec![
            reason.to_owned(),
            format!("Raw content artifact: {}", parse_path.display()),
        ],
        dedupe_key: Some("lane-output-malformed-content".to_owned()),
    }
}

fn render_lane_model_prompt(lane: &LanePlan, spec: &ProviderSpec, shared_context: &str) -> String {
    let lane_guidance = lane_specific_prompt_guidance(lane);
    format!(
        r#"Lane: {lane}
Provider: {provider}
Model: {model}
Endpoint kind: {endpoint_kind}
Role: {role}
Focus: {focus}
{lane_guidance}

Use the shared context below. Return only one strict JSON object:
{{
  "summary": "short lane summary, 300 chars max",
  "observations": [
    {{
      "claim": "terse unique observation, 300 chars max",
      "question": "{lane}",
      "kind": "bug|verification-question|missing-evidence|test-gap|source-route-gap|security-risk|false-premise|parked-follow-up|residual-risk|resolved-check",
      "status": "open|covered|confirmed|refuted|demoted|parked|duplicate",
      "severity": "blocker|high|medium|low",
      "confidence": "high|medium-high|medium|low",
      "path": "optional repo-relative/path.rs",
      "line": 123,
      "evidence": ["artifact, diff, or invariant, 240 chars max"],
      "dedupe_key": "stable coordination key when known"
    }}
  ],
  "candidate_findings": [
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
  ],
  "failed_objections": [
    {{
      "claim": "objection tested by this lane",
      "reason": "why it did not hold",
      "confidence": "high|medium-high|medium|low",
      "kind": "resolved-check|false-premise",
      "evidence": ["artifact, diff, or invariant"]
    }}
  ],
  "proof_requests": [
    {{
      "command": "focused command requested from central proof broker",
      "reason": "why this proof would matter",
      "cost": "focused-test|focused-build|manual",
      "timeout_sec": 300,
      "required": false
    }}
  ]
}}

Hard caps: at most 3 observations, 2 candidate_findings, 1 summary_only_findings item, 2 failed_objections, and 1 proof_request.
If there is no blocker/high/medium actionable issue, use empty arrays and put the failed-objection audit in summary.
Only propose candidate_findings for valid RIGHT-side changed or context lines in the PR diff.
Legacy `inline_comments` is accepted as an alias for `candidate_findings`, but prefer `candidate_findings`.
Do not guess line numbers. Do not use deletion-side comments. Do not output a standalone approval.
Calibration: `Box::from(slice)` / `Box::<[u8]>::from(slice)` allocation failure does not return `None`, an empty box, or a recoverable fallback. If that objection arises, return it as a refuted false-premise failed_objection, not as a candidate finding.

{shared_context}"#,
        lane = lane.id,
        provider = spec.provider.key(),
        model = spec.model,
        endpoint_kind = spec.endpoint_kind.key(),
        role = lane.role,
        focus = lane.focus,
        lane_guidance = lane_guidance,
        shared_context = shared_context
    )
}

fn lane_specific_prompt_guidance(lane: &LanePlan) -> &'static str {
    if lane.id == "tests" || lane.id.starts_with("tests-") {
        "Convergence calibration: batch every material test-oracle weakness you can substantiate in this pass; classify correctness/oracle gaps as blocker/high/medium and submaterial polish as low advisory or parked-follow-up. If the test is red/green-correct or proof receipts answer the concern, emit a resolved-check or failed_objection instead of a fresh candidate finding. Do not drip-feed one nit per pass."
    } else {
        ""
    }
}

struct ModelOutputSinks<'a> {
    inline_comments: &'a mut Vec<ReviewInlineComment>,
    summary_only_findings: &'a mut Vec<SummaryOnlyFinding>,
    model_observations: &'a mut Vec<Observation>,
    proof_requests: &'a mut Vec<ProofRequest>,
}

fn apply_model_output(
    lane: &LanePlan,
    output: LaneModelOutput,
    line_map: &BTreeSet<(String, u32)>,
    max_inline: usize,
    sinks: ModelOutputSinks<'_>,
) {
    let ModelOutputSinks {
        inline_comments,
        summary_only_findings,
        model_observations,
        proof_requests,
    } = sinks;
    if let Some(summary) = output.summary {
        if let Some(observation) = box_from_allocation_false_premise_observation_from_text(
            lane,
            &summary,
            vec!["lane model summary".to_owned()],
            None,
            None,
            model_observations.len(),
            "model-false-premise-guard",
        ) {
            model_observations.push(observation);
        } else {
            summary_only_findings.push(validate_lane_model_summary(lane, &summary));
        }
    }
    for candidate in output.summary_only_findings {
        if let Some(observation) = box_from_allocation_false_premise_observation_from_summary_only(
            lane,
            &candidate,
            model_observations.len(),
        ) {
            model_observations.push(observation);
        } else {
            summary_only_findings.push(validate_summary_only_candidate(lane, candidate));
        }
    }
    for observation in output.observations {
        model_observations.push(validate_model_observation(
            lane,
            observation,
            model_observations.len(),
        ));
    }
    for objection in output.failed_objections {
        model_observations.push(validate_failed_objection(
            lane,
            objection,
            model_observations.len(),
        ));
    }
    for request in output.proof_requests {
        proof_requests.push(validate_proof_request(lane, request, proof_requests.len()));
    }
    for candidate in output
        .candidate_findings
        .into_iter()
        .chain(output.inline_comments)
    {
        if let Some(observation) = box_from_allocation_false_premise_observation_from_candidate(
            lane,
            &candidate,
            model_observations.len(),
        ) {
            model_observations.push(observation);
            continue;
        }
        if is_candidate_only_lane(&lane.id) {
            summary_only_findings.push(SummaryOnlyFinding {
                lane: lane.id.clone(),
                severity: candidate.severity,
                confidence: candidate.confidence,
                reason: format!(
                    "candidate-only lane emitted inline candidate for {}:{}; kept summary-only",
                    candidate.path, candidate.line
                ),
                evidence: candidate.evidence,
            });
            continue;
        }
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

fn validate_model_observation(
    lane: &LanePlan,
    candidate: ModelCandidateObservation,
    index: usize,
) -> Observation {
    let claim = non_empty_or(
        candidate.claim.trim(),
        "model observation guard rejected empty claim",
    );
    let evidence = non_empty_evidence(candidate.evidence, "model observation");
    let kind = candidate
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|kind| allowed_observation_kind(kind))
        .unwrap_or_else(|| infer_observation_kind(&lane.id, &claim, &evidence.join("\n")));
    let status = candidate
        .status
        .as_deref()
        .map(str::trim)
        .filter(|status| allowed_observation_status(status))
        .unwrap_or("open");
    let severity = candidate
        .severity
        .as_deref()
        .map(str::trim)
        .filter(|severity| matches!(*severity, "blocker" | "high" | "medium" | "low"))
        .unwrap_or("low");
    let confidence = candidate
        .confidence
        .as_deref()
        .map(str::trim)
        .filter(|confidence| matches!(*confidence, "high" | "medium-high" | "medium" | "low"))
        .unwrap_or("medium");
    let path = candidate
        .path
        .as_deref()
        .map(normalize_repo_path)
        .filter(|path| !path.is_empty());
    if let Some(observation) = box_from_allocation_false_premise_observation_from_text(
        lane,
        &format!("{claim}\n{}", evidence.join("\n")),
        evidence.clone(),
        path.as_ref(),
        candidate.line,
        index,
        "model-false-premise-guard",
    ) {
        return observation;
    }
    make_observation(ObservationInput {
        index,
        lane: &lane.id,
        question: candidate.question.as_deref().unwrap_or(lane.id.as_str()),
        claim: &claim,
        kind,
        status,
        severity,
        confidence,
        path: path.as_ref(),
        line: candidate.line,
        evidence,
        dedupe_key: candidate.dedupe_key.as_deref(),
        source: "model-observation",
    })
}

fn validate_failed_objection(
    lane: &LanePlan,
    objection: ModelFailedObjection,
    index: usize,
) -> Observation {
    let claim = non_empty_or(
        objection.claim.trim(),
        "model failed objection missing claim",
    );
    let reason = non_empty_or(
        objection.reason.trim(),
        "model failed objection missing reason",
    );
    let full_claim = format!("{claim}; refuted because: {reason}");
    let evidence = non_empty_evidence(objection.evidence, "failed objection audit");
    if let Some(observation) = box_from_allocation_false_premise_observation_from_text(
        lane,
        &format!("{full_claim}\n{}", evidence.join("\n")),
        evidence.clone(),
        None,
        None,
        index,
        "model-failed-objection",
    ) {
        return observation;
    }
    let kind = objection
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|kind| allowed_observation_kind(kind))
        .unwrap_or_else(|| {
            if reason.to_ascii_lowercase().contains("false premise") {
                "false-premise"
            } else {
                "resolved-check"
            }
        });
    let confidence = objection
        .confidence
        .as_deref()
        .map(str::trim)
        .filter(|confidence| matches!(*confidence, "high" | "medium-high" | "medium" | "low"))
        .unwrap_or("medium");
    make_observation(ObservationInput {
        index,
        lane: &lane.id,
        question: "failed-objection",
        claim: &full_claim,
        kind,
        status: "refuted",
        severity: "low",
        confidence,
        path: None,
        line: None,
        evidence,
        dedupe_key: None,
        source: "model-failed-objection",
    })
}

const BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY: &str = "rust-box-from-allocation-failure";
const BOX_FROM_ALLOCATION_FALSE_PREMISE_CLAIM: &str = "`Box::from(slice)` allocation failure does not return `None`; recoverable fallback claims are dropped.";

fn box_from_allocation_false_premise_observation_from_candidate(
    lane: &LanePlan,
    candidate: &ModelCandidateComment,
    index: usize,
) -> Option<Observation> {
    let text = format!("{}\n{}", candidate.body, candidate.evidence);
    let path = normalize_repo_path(&candidate.path);
    let path = if path.is_empty() { None } else { Some(path) };
    box_from_allocation_false_premise_observation_from_text(
        lane,
        &text,
        vec![candidate.evidence.clone()],
        path.as_ref(),
        Some(candidate.line),
        index,
        "model-false-premise-guard",
    )
}

fn box_from_allocation_false_premise_observation_from_summary_only(
    lane: &LanePlan,
    candidate: &ModelCandidateFinding,
    index: usize,
) -> Option<Observation> {
    box_from_allocation_false_premise_observation_from_text(
        lane,
        &format!("{}\n{}", candidate.reason, candidate.evidence),
        vec![candidate.evidence.clone()],
        None,
        None,
        index,
        "model-false-premise-guard",
    )
}

fn box_from_allocation_false_premise_observation_from_text(
    lane: &LanePlan,
    text: &str,
    evidence: Vec<String>,
    path: Option<&String>,
    line: Option<u32>,
    index: usize,
    source: &str,
) -> Option<Observation> {
    if !is_box_from_allocation_false_premise(text) {
        return None;
    }
    let mut evidence = non_empty_evidence(evidence, "model false-premise guard");
    let invariant =
        "Rust allocation semantics: Box::from(&[u8]) does not return None on allocation failure.";
    if !evidence.iter().any(|item| item == invariant) {
        evidence.push(invariant.to_owned());
    }
    Some(make_observation(ObservationInput {
        index,
        lane: &lane.id,
        question: "false-premise",
        claim: BOX_FROM_ALLOCATION_FALSE_PREMISE_CLAIM,
        kind: "false-premise",
        status: "refuted",
        severity: "low",
        confidence: "high",
        path,
        line,
        evidence,
        dedupe_key: Some(BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY),
        source,
    }))
}

fn is_box_from_allocation_false_premise(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let compact = lower
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '`')
        .collect::<String>();
    let mentions_box_from =
        compact.contains("box::from(") || compact.contains("box::<[u8]>::from(");
    let mentions_allocation = lower.contains("allocation failure")
        || lower.contains("allocation fails")
        || lower.contains("alloc failure")
        || lower.contains("out of memory")
        || lower.contains("oom");
    let mentions_recoverable_result = lower.contains("none")
        || lower.contains("empty box")
        || lower.contains("fallback")
        || lower.contains("fall through")
        || lower.contains("fallthrough");
    mentions_box_from && mentions_allocation && mentions_recoverable_result
}

fn validate_proof_request(
    lane: &LanePlan,
    request: ModelProofRequest,
    index: usize,
) -> ProofRequest {
    let command = request.command.trim().replace(['\r', '\n'], " ");
    let reason = non_empty_or(request.reason.trim(), "model proof request missing reason");
    let command = non_empty_or(&command, "<missing command>");
    let cost = classify_proof_cost(request.cost.as_deref(), &command);
    let status = proof_request_status(&command, &cost);
    let timeout_sec = request.timeout_sec.unwrap_or(300).clamp(1, 900);
    let fingerprint = sha256_hex(
        format!(
            "{}\n{}\n{}\n{}\n{}",
            lane.id, command, reason, cost, timeout_sec
        )
        .as_bytes(),
    );
    let short = &fingerprint[..12];
    ProofRequest {
        schema: "ub-review.proof_request.v1".to_owned(),
        id: format!("proof-{index:04}-{short}"),
        lane: lane.id.clone(),
        requested_by: vec![lane.id.clone()],
        command,
        reason,
        cost,
        timeout_sec,
        required: request.required.unwrap_or(false),
        status: status.to_owned(),
    }
}

fn proof_request_status(command: &str, cost: &str) -> &'static str {
    if command == "<missing command>" {
        return "invalid";
    }
    if proof_request_allowed_v0(command, cost) {
        "requested"
    } else {
        "unsupported"
    }
}

fn proof_request_allowed_v0(command: &str, cost: &str) -> bool {
    if cost != "focused-test" || has_shell_control_token(command) {
        return false;
    }
    let parts = command.split_whitespace().collect::<Vec<_>>();
    match parts.as_slice() {
        ["bun", "test", file, ..] => is_bun_focused_test_file(file),
        ["bun", "bd", "test", file, ..] => is_bun_focused_test_file(file),
        _ => false,
    }
}

fn has_shell_control_token(command: &str) -> bool {
    command
        .chars()
        .any(|ch| matches!(ch, '&' | '|' | ';' | '`' | '>' | '<' | '$'))
}

fn classify_proof_cost(cost: Option<&str>, command: &str) -> String {
    let supplied = cost.unwrap_or("").trim().to_ascii_lowercase();
    if matches!(
        supplied.as_str(),
        "focused-test" | "focused-build" | "manual"
    ) {
        return supplied;
    }
    let command = command.to_ascii_lowercase();
    if supplied.contains("test")
        || command.contains(" test ")
        || command.starts_with("bun test")
        || command.starts_with("cargo test")
        || command.starts_with("npm test")
    {
        return "focused-test".to_owned();
    }
    if supplied.contains("build")
        || command.contains(" build")
        || command.starts_with("cargo build")
        || command.starts_with("bun build")
        || command.starts_with("ninja")
        || command.starts_with("cmake")
    {
        return "focused-build".to_owned();
    }
    "manual".to_owned()
}

fn non_empty_or(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

fn non_empty_evidence(values: Vec<String>, fallback: &str) -> Vec<String> {
    let cleaned = values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if cleaned.is_empty() {
        vec![fallback.to_owned()]
    } else {
        cleaned
    }
}

fn allowed_observation_kind(value: &str) -> bool {
    matches!(
        value,
        "bug"
            | "verification-question"
            | "missing-evidence"
            | "test-gap"
            | "source-route-gap"
            | "security-risk"
            | "false-premise"
            | "parked-follow-up"
            | "residual-risk"
            | "resolved-check"
    )
}

fn allowed_observation_status(value: &str) -> bool {
    matches!(
        value,
        "open" | "covered" | "confirmed" | "refuted" | "demoted" | "parked" | "duplicate"
    )
}

fn is_candidate_only_lane(lane_id: &str) -> bool {
    is_opencode_fast_lane(lane_id)
}

fn validate_lane_model_summary(lane: &LanePlan, summary: &str) -> SummaryOnlyFinding {
    let reason = summary.trim().to_owned();
    let reason_present = !reason.is_empty();
    let concise = reason.chars().count() <= 1_200;
    let no_standalone_approval = !has_standalone_approval_line(&reason);

    if reason_present && concise && no_standalone_approval {
        SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason,
            evidence: "lane model summary".to_owned(),
        }
    } else {
        SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason: format!(
                "lane model summary guard rejected summary; reason_present={} concise={} no_standalone_approval={}",
                reason_present, concise, no_standalone_approval
            ),
            evidence: "lane model summary guardrail".to_owned(),
        }
    }
}

fn validate_summary_only_candidate(
    lane: &LanePlan,
    candidate: ModelCandidateFinding,
) -> SummaryOnlyFinding {
    let severity = candidate.severity.trim().to_owned();
    let confidence = candidate.confidence.trim().to_owned();
    let reason = candidate.reason.trim().to_owned();
    let evidence = candidate.evidence.trim().to_owned();
    let severity_allowed = matches!(severity.as_str(), "blocker" | "high" | "medium" | "low");
    let confidence_allowed = matches!(confidence.as_str(), "high" | "medium-high" | "medium");
    let reason_present = !reason.is_empty();
    let evidence_present = !evidence.is_empty();
    let concise = reason.chars().count() <= 1_200 && evidence.chars().count() <= 1_200;

    if severity_allowed && confidence_allowed && reason_present && evidence_present && concise {
        SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity,
            confidence,
            reason,
            evidence,
        }
    } else {
        SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason: format!(
                "summary-only guard rejected candidate; severity_allowed={} confidence_allowed={} reason_present={} evidence_present={} concise={}",
                severity_allowed, confidence_allowed, reason_present, evidence_present, concise
            ),
            evidence: "model summary-only candidate guardrail".to_owned(),
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
    let body_text = candidate.body.trim();
    let evidence = candidate.evidence.trim().to_owned();
    let body = ensure_lane_prefix(&lane.id, body_text);
    let concise = body.chars().count() <= 1_200;
    let body_present = !body_text.is_empty();
    let evidence_present = !evidence.is_empty();
    let repo_relative = is_repo_relative_path(&path);

    if allowed_severity
        && allowed_confidence
        && line_valid
        && concise
        && body_present
        && evidence_present
        && repo_relative
    {
        Ok(ReviewInlineComment {
            lane: lane.id.clone(),
            severity: candidate.severity,
            confidence: candidate.confidence,
            path,
            line: candidate.line,
            side: "RIGHT".to_owned(),
            body,
            evidence,
        })
    } else {
        Err(SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: candidate.severity,
            confidence: candidate.confidence,
            reason: format!(
                "inline guard rejected {}:{}; severity_allowed={} confidence_allowed={} line_valid={} concise={} body_present={} evidence_present={} repo_relative={}",
                path,
                candidate.line,
                allowed_severity,
                allowed_confidence,
                line_valid,
                concise,
                body_present,
                evidence_present,
                repo_relative
            ),
            evidence,
        })
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
fn render_review_body(
    shared_context_id: &str,
    plan: &Plan,
    diff: &DiffContext,
    model_lanes: &[ModelLaneReceipt],
    missing_or_failed_sensor_evidence: &[SensorEvidenceIssue],
    missing_or_failed_model_evidence: &[ModelEvidenceIssue],
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    observations: &[Observation],
    proof_receipts: &[ProofReceipt],
    review_body_max_bytes: usize,
    audience: ReviewBodyAudience,
) -> String {
    if matches!(audience, ReviewBodyAudience::PullRequest) {
        return render_pull_request_review_body(
            shared_context_id,
            plan,
            diff,
            missing_or_failed_sensor_evidence,
            missing_or_failed_model_evidence,
            inline_comments,
            summary_only_findings,
            observations,
            proof_receipts,
            review_body_max_bytes,
        );
    }

    let mut text = String::new();
    text.push_str(&format!(
        "# {}\n\n",
        pr_review_heading_for_diff_class(plan.diff_class)
    ));
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
    text.push_str(&format!(
        "- {}\n",
        residual_risk_for_diff_class(plan.diff_class)
    ));

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

    if audience.include_successful_lane_table() {
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
    }
    cap_review_body(text, review_body_max_bytes)
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
fn render_pull_request_review_body(
    _shared_context_id: &str,
    _plan: &Plan,
    _diff: &DiffContext,
    _missing_or_failed_sensor_evidence: &[SensorEvidenceIssue],
    _missing_or_failed_model_evidence: &[ModelEvidenceIssue],
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    observations: &[Observation],
    proof_receipts: &[ProofReceipt],
    review_body_max_bytes: usize,
) -> String {
    let mut text = String::new();
    let observation_items = unique_review_observations(observations);
    let pr_observation_items = observation_items
        .iter()
        .filter(|observation| !is_pr_body_artifact_only_observation(observation))
        .collect::<Vec<_>>();
    let refuted_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| is_pr_body_refuted_observation(observation))
        .collect::<Vec<_>>();
    let missing_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| is_missing_evidence_observation(observation))
        .collect::<Vec<_>>();
    let parked_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| is_parked_observation(observation))
        .collect::<Vec<_>>();
    let residual_risk_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| is_residual_risk_observation(observation))
        .collect::<Vec<_>>();
    let verification_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| {
            !is_refuted_observation(observation)
                && !is_missing_evidence_observation(observation)
                && !is_parked_observation(observation)
                && !is_residual_risk_observation(observation)
                && is_verification_observation(observation)
        })
        .collect::<Vec<_>>();
    let concern_observations = pr_observation_items
        .iter()
        .copied()
        .filter(|observation| {
            !is_refuted_observation(observation)
                && !is_missing_evidence_observation(observation)
                && !is_parked_observation(observation)
                && !is_residual_risk_observation(observation)
                && !is_verification_observation(observation)
        })
        .collect::<Vec<_>>();
    let parked = summary_only_findings
        .iter()
        .filter(|finding| {
            !is_pr_body_artifact_only_finding(finding)
                && is_parked_follow_up(finding)
                && !summary_finding_matches_observations(finding, &observation_items)
        })
        .collect::<Vec<_>>();
    let verification_questions = summary_only_findings
        .iter()
        .filter(|finding| {
            !is_pr_body_artifact_only_finding(finding)
                && !is_parked_follow_up(finding)
                && is_verification_question(finding)
                && !summary_finding_matches_observations(finding, &observation_items)
        })
        .collect::<Vec<_>>();
    let summary_concerns = summary_only_findings
        .iter()
        .filter(|finding| {
            !is_pr_body_artifact_only_finding(finding)
                && !is_parked_follow_up(finding)
                && !is_verification_question(finding)
                && !summary_finding_matches_observations(finding, &observation_items)
        })
        .collect::<Vec<_>>();
    let has_specific_missing_evidence = !missing_observations.is_empty()
        || proof_receipts.iter().any(proof_receipt_is_missing_evidence);
    let residual_risk_receipts = proof_receipts
        .iter()
        .filter(|receipt| proof_receipt_is_residual_risk(receipt))
        .collect::<Vec<_>>();
    let proof_result_receipts = proof_receipts
        .iter()
        .filter(|receipt| proof_receipt_is_test_proof_result(receipt))
        .collect::<Vec<_>>();
    let current_proof_failure = proof_receipts
        .iter()
        .any(|receipt| receipt.result == "head_failed");
    let finding_count = inline_comments.len() + summary_concerns.len() + concern_observations.len();
    let verification_count = verification_questions.len() + verification_observations.len();
    let has_test_proof_verification = verification_questions
        .iter()
        .any(|finding| text_is_test_proof_review_question(&finding.reason))
        || verification_observations
            .iter()
            .any(|observation| text_is_test_proof_review_question(&observation.claim));
    let decision_sentence = pr_decision_sentence(PrDecisionContext {
        finding_count,
        verification_count,
        has_test_proof_verification,
        current_proof_failure,
    });
    let has_decision_item = decision_sentence.is_some();
    let has_reviewer_value_item = has_decision_item
        || !refuted_observations.is_empty()
        || !proof_result_receipts.is_empty()
        || !parked.is_empty()
        || !parked_observations.is_empty()
        || !residual_risk_observations.is_empty()
        || !residual_risk_receipts.is_empty()
        || has_specific_missing_evidence;
    if !has_reviewer_value_item {
        return String::new();
    }

    if let Some(decision_sentence) = decision_sentence {
        text.push_str("## Decision\n\n");
        text.push_str("- ");
        text.push_str(decision_sentence);
        text.push('\n');
    }

    if !inline_comments.is_empty()
        || !summary_concerns.is_empty()
        || !concern_observations.is_empty()
    {
        text.push_str("\n## Confirmed findings\n\n");
        for comment in inline_comments {
            render_pr_model_signal(&mut text, &comment.body);
        }
        for observation in &concern_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
        for finding in summary_concerns {
            render_pr_model_signal(&mut text, &finding.reason);
        }
    }

    if !verification_questions.is_empty() {
        text.push_str("\n## Verification questions\n\n");
        for observation in &verification_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Verification);
        }
        for finding in verification_questions {
            render_pr_model_verification(&mut text, &finding.reason);
        }
    } else if !verification_observations.is_empty() {
        text.push_str("\n## Verification questions\n\n");
        for observation in &verification_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Verification);
        }
    }

    if !refuted_observations.is_empty() {
        text.push_str("\n## Refuted\n\n");
        for observation in &refuted_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
    }

    if !proof_result_receipts.is_empty() {
        text.push_str("\n## Test proof\n\n");
        for receipt in proof_result_receipts {
            render_proof_receipt_summary(&mut text, receipt);
        }
    }

    if !parked.is_empty() {
        text.push_str("\n## Parked follow-ups\n\n");
        for observation in &parked_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
        for finding in parked {
            render_pr_model_signal(&mut text, &finding.reason);
        }
    } else if !parked_observations.is_empty() {
        text.push_str("\n## Parked follow-ups\n\n");
        for observation in &parked_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
    }

    if !residual_risk_observations.is_empty() || !residual_risk_receipts.is_empty() {
        text.push_str("\n## Residual risk\n\n");
        for observation in &residual_risk_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
        for receipt in residual_risk_receipts {
            render_residual_risk_proof_receipt_summary(&mut text, receipt);
        }
    }

    if has_specific_missing_evidence {
        text.push_str("\n## Evidence gaps\n\n");
        for observation in &missing_observations {
            render_review_observation(&mut text, observation, PrObservationTone::Signal);
        }
        for receipt in proof_receipts
            .iter()
            .filter(|receipt| proof_receipt_is_missing_evidence(receipt))
        {
            render_missing_proof_receipt_summary(&mut text, receipt);
        }
    }

    cap_review_body(text, review_body_max_bytes)
}

struct PrDecisionContext {
    finding_count: usize,
    verification_count: usize,
    has_test_proof_verification: bool,
    current_proof_failure: bool,
}

fn pr_decision_sentence(context: PrDecisionContext) -> Option<&'static str> {
    if context.finding_count > 0 {
        return Some("Needs reviewer attention before upstream: findings remain.");
    }
    if context.current_proof_failure {
        return Some("Needs focused proof failure resolved before upstream.");
    }
    if context.verification_count == 1 {
        if context.has_test_proof_verification {
            return Some("Needs one test-proof clarification before upstream.");
        }
        return Some("Needs one verification check before upstream.");
    }
    if context.verification_count > 1 {
        return Some("Needs verification checks before upstream.");
    }
    None
}

fn proof_receipt_changes_review_value(receipt: &ProofReceipt) -> bool {
    matches!(
        receipt.result.as_str(),
        "discriminating" | "non_discriminating" | "head_passed" | "head_failed"
    )
}

fn proof_receipt_is_test_proof_result(receipt: &ProofReceipt) -> bool {
    matches!(
        receipt.result.as_str(),
        "discriminating" | "head_passed" | "head_failed"
    )
}

fn proof_receipt_is_residual_risk(receipt: &ProofReceipt) -> bool {
    matches!(receipt.result.as_str(), "non_discriminating")
}

fn proof_receipt_is_missing_evidence(receipt: &ProofReceipt) -> bool {
    matches!(
        receipt.result.as_str(),
        "base_patch_failed" | "timed_out" | "skipped_budget" | "skipped_profile"
    )
}

fn is_pr_body_artifact_only_finding(finding: &SummaryOnlyFinding) -> bool {
    let reason = finding.reason.to_ascii_lowercase();
    reason.starts_with("inline guard rejected ")
        || reason.contains("severity_allowed=")
        || reason.contains("confidence_allowed=")
        || reason.contains("line_valid=")
        || reason.contains("body_present=")
        || reason.contains("evidence_present=")
        || reason.contains("repo_relative=")
}

fn is_verification_question(finding: &SummaryOnlyFinding) -> bool {
    let text = format!("{} {}", finding.reason, finding.evidence).to_ascii_lowercase();
    text.contains("verify")
        || text.contains("verification")
        || text.contains("confirm")
        || text.contains("question")
        || text.contains("witness")
        || text.contains("red/green")
        || text.contains("proof")
        || text.contains("prove")
        || text.contains("proves")
        || text.contains("proven")
}

fn is_verification_observation(observation: &ObservationGroup) -> bool {
    observation.kind == "verification-question"
        || matches!(observation.kind.as_str(), "test-gap" | "source-route-gap")
        || text_is_verification_question(&observation.claim)
}

fn is_refuted_observation(observation: &ObservationGroup) -> bool {
    observation.status == "refuted"
        || matches!(
            observation.kind.as_str(),
            "false-premise" | "resolved-check"
        )
}

fn is_pr_body_refuted_observation(observation: &ObservationGroup) -> bool {
    if observation.kind == "resolved-check" {
        return false;
    }
    is_refuted_observation(observation) && !is_global_calibration_refutation(observation)
}

fn is_refutation_confirmation_observation(observation: &ObservationGroup) -> bool {
    is_refuted_observation(observation) && !is_global_calibration_refutation(observation)
}

fn is_pr_body_artifact_only_observation(observation: &ObservationGroup) -> bool {
    observation.dedupe_key.starts_with("lane-output-shape")
        || observation
            .dedupe_key
            .starts_with("lane-output-malformed-content")
}

fn is_global_calibration_refutation(observation: &ObservationGroup) -> bool {
    observation.dedupe_key == BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY
        && observation.path.is_none()
        && observation
            .sources
            .iter()
            .any(|source| source == "model-false-premise-guard")
}

fn is_missing_evidence_observation(observation: &ObservationGroup) -> bool {
    observation.kind == "missing-evidence"
}

fn is_residual_risk_observation(observation: &ObservationGroup) -> bool {
    observation.kind == "residual-risk"
}

fn pr_review_heading_for_diff_class(diff_class: DiffClass) -> &'static str {
    match diff_class {
        DiffClass::SourceUb => "UB Review",
        DiffClass::SourceGeneral => "Source Review",
        DiffClass::TestsOnly => "Test Review",
        DiffClass::WorkflowTooling => "Workflow Review",
        DiffClass::DocsOnly => "Docs Review",
        DiffClass::ArtifactOnlySmoke => "Review Packet",
    }
}

fn residual_risk_for_diff_class(diff_class: DiffClass) -> &'static str {
    match diff_class {
        DiffClass::SourceUb => {
            "Artifact audit note: unsafe/native route truth, oracle strength, and unavailable evidence remain tracked in artifacts."
        }
        DiffClass::SourceGeneral => {
            "Artifact audit note: changed behavior, route truth, test strength, and unavailable evidence remain tracked in artifacts."
        }
        DiffClass::TestsOnly => {
            "Artifact audit note: red/green discrimination, oracle strength, flake risk, and unavailable proof evidence remain tracked in artifacts."
        }
        DiffClass::WorkflowTooling => {
            "Artifact audit note: workflow permissions, trigger safety, action pinning, checkout credentials, fork behavior, and actionlint/zizmor evidence remain tracked in artifacts."
        }
        DiffClass::DocsOnly => {
            "Artifact audit note: claim accuracy, links, examples, and unavailable evidence remain tracked in artifacts."
        }
        DiffClass::ArtifactOnlySmoke => {
            "Artifact audit note: packet completeness and unavailable evidence remain tracked in artifacts."
        }
    }
}

fn is_parked_observation(observation: &ObservationGroup) -> bool {
    observation.status == "parked" || observation.kind == "parked-follow-up"
}

fn text_is_verification_question(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("verify")
        || text.contains("verification")
        || text.contains("confirm")
        || text.contains("question")
        || text.contains("witness")
        || text.contains("red/green")
        || text.contains("proof")
        || text.contains("prove")
        || text.contains("proves")
        || text.contains("proven")
}

fn text_is_test_proof_review_question(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("proof")
        || text.contains("prove")
        || text.contains("proves")
        || text.contains("proven")
        || text.contains("red/green")
        || text.contains("base+tests")
        || text.contains("asan")
        || text.contains("bad-free")
}

fn summary_finding_matches_observations(
    finding: &SummaryOnlyFinding,
    observations: &[ObservationGroup],
) -> bool {
    let summary = normalized_review_text(&format!("{} {}", finding.reason, finding.evidence));
    observations.iter().any(|observation| {
        let claim = normalized_review_text(&observation.claim);
        claim.len() >= 24 && (summary.contains(&claim) || claim.contains(&summary))
    })
}

fn normalized_review_text(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy)]
enum PrObservationTone {
    Signal,
    Verification,
}

fn render_review_observation(
    text: &mut String,
    observation: &ObservationGroup,
    tone: PrObservationTone,
) {
    match tone {
        PrObservationTone::Signal => render_pr_model_signal(text, &observation.claim),
        PrObservationTone::Verification => render_pr_model_verification(text, &observation.claim),
    }
}

fn render_pr_signal(text: &mut String, value: &str) {
    let sentence = pr_sentence(value);
    text.push_str(&format!("- {}\n", escape_md(&sentence)));
}

fn render_pr_verification(text: &mut String, value: &str) {
    let sentence = verification_sentence(value);
    text.push_str(&format!("- {}\n", escape_md(&sentence)));
}

fn render_pr_model_signal(text: &mut String, value: &str) {
    render_pr_signal(text, &reviewer_facing_pr_text(value));
}

fn render_pr_model_verification(text: &mut String, value: &str) {
    render_pr_verification(text, &reviewer_facing_pr_text(value));
}

fn reviewer_facing_pr_text(value: &str) -> String {
    let mut text = value.trim();
    if let Some(stripped) = strip_bracketed_lane_prefix(text) {
        text = stripped;
    }
    if let Some(stripped) = strip_raw_lane_metadata_prefix(text) {
        text = stripped;
    }
    strip_embedded_evidence_label(text).trim().to_owned()
}

fn strip_bracketed_lane_prefix(value: &str) -> Option<&str> {
    let trimmed = value.trim_start();
    if !trimmed.starts_with('[') {
        return None;
    }
    let end = trimmed.find(']')?;
    if end > 80 {
        return None;
    }
    Some(trimmed[end + 1..].trim_start())
}

fn strip_raw_lane_metadata_prefix(value: &str) -> Option<&str> {
    let lower = value.to_ascii_lowercase();
    let at_index = lower.find(" at ")?;
    let prefix = lower[..at_index].trim();
    if !prefix
        .split_whitespace()
        .all(|token| matches!(token, "blocker" | "high" | "medium" | "low" | "medium-high"))
    {
        return None;
    }
    let after_at = &value[at_index + 4..];
    let body_index = after_at.find(": ")?;
    Some(after_at[body_index + 2..].trim_start())
}

fn strip_embedded_evidence_label(value: &str) -> &str {
    for marker in [" Evidence:", " evidence:"] {
        if let Some(index) = value.find(marker) {
            return value[..index].trim_end();
        }
    }
    value
}

fn render_proof_receipt_summary(text: &mut String, receipt: &ProofReceipt) {
    let command = receipt
        .commands
        .first()
        .map(|command| command.command.as_str())
        .unwrap_or("focused test");
    let head_status =
        proof_command_outcome(receipt, "head").unwrap_or_else(|| "HEAD status unknown".to_owned());
    let head_only_status = proof_command_status_for_side(receipt, "head")
        .unwrap_or_else(|| "status unknown".to_owned());
    let base_plus_tests_status = proof_command_outcome(receipt, "base-plus-tests")
        .unwrap_or_else(|| "base+tests status unknown".to_owned());
    let summary = match receipt.result.as_str() {
        "discriminating" => format!(
            "Focused red/green proof discriminates the patch: {head_status} and {base_plus_tests_status} for `{command}`."
        ),
        "non_discriminating" => format!(
            "Focused red/green proof did not discriminate the patch: {head_status} and {base_plus_tests_status} for `{command}`."
        ),
        "head_passed" => format!(
            "Focused HEAD proof {head_only_status}: `{command}`. Base+tests red/green was not run in this v0 proof."
        ),
        "head_failed" => format!(
            "Focused HEAD proof {head_only_status}: `{command}`. This is a current failure, not a red/green witness."
        ),
        _ => format!(
            "Focused proof result `{}` for `{}` is recorded in artifacts.",
            receipt.result, command
        ),
    };
    render_pr_signal(text, &summary);
}

fn render_residual_risk_proof_receipt_summary(text: &mut String, receipt: &ProofReceipt) {
    let command = receipt
        .commands
        .first()
        .map(|command| command.command.as_str())
        .unwrap_or("focused test");
    let head_status =
        proof_command_outcome(receipt, "head").unwrap_or_else(|| "HEAD status unknown".to_owned());
    let base_plus_tests_status = proof_command_outcome(receipt, "base-plus-tests")
        .unwrap_or_else(|| "base+tests status unknown".to_owned());
    let summary = match receipt.result.as_str() {
        "non_discriminating" => format!(
            "Focused red/green proof did not discriminate the patch: {head_status} and {base_plus_tests_status} for `{command}`."
        ),
        _ => format!(
            "Focused proof result `{}` leaves residual risk for `{}`.",
            receipt.result, command
        ),
    };
    render_pr_signal(text, &summary);
}

fn proof_command_outcome(receipt: &ProofReceipt, side: &str) -> Option<String> {
    let command = receipt
        .commands
        .iter()
        .find(|command| command.side == side)?;
    let side_label = match side {
        "head" => "HEAD",
        "base-plus-tests" => "base+tests",
        other => other,
    };
    let outcome = format!("{side_label} {}", proof_command_status(command));
    Some(outcome)
}

fn proof_command_status_for_side(receipt: &ProofReceipt, side: &str) -> Option<String> {
    receipt
        .commands
        .iter()
        .find(|command| command.side == side)
        .map(proof_command_status)
}

fn proof_command_status(command: &ProofCommandReceipt) -> String {
    let mut outcome = command.status.clone();
    if let Some(exit_code) = command.exit_code {
        outcome.push_str(&format!(" (exit {exit_code})"));
    }
    if command.timed_out && !outcome.contains("timed_out") {
        outcome.push_str(" (timed out)");
    }
    outcome
}

fn render_missing_proof_receipt_summary(text: &mut String, receipt: &ProofReceipt) {
    let command = receipt
        .commands
        .first()
        .map(|command| command.command.as_str())
        .unwrap_or("focused test");
    let summary = match receipt.result.as_str() {
        "base_patch_failed" => format!(
            "Base+tests proof was unavailable for `{command}` because the test-only patch did not apply cleanly."
        ),
        "timed_out" => format!("Focused proof timed out for `{command}`; logs are in artifacts."),
        "skipped_budget" => {
            format!(
                "Focused proof was skipped by budget for `{command}`; plan details are in artifacts."
            )
        }
        "skipped_profile" => {
            format!(
                "Focused proof was unavailable for `{command}`; profile/tool details are in artifacts."
            )
        }
        _ => format!(
            "Focused proof result `{}` for `{}` needs artifact review.",
            receipt.result, command
        ),
    };
    render_pr_signal(text, &summary);
}

fn verification_sentence(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "Confirm the unresolved review question.".to_owned();
    }
    let without_trailing = trimmed.trim_end_matches(&['.', '!', '?'][..]).trim();
    if without_trailing.is_empty() {
        return "Confirm the unresolved review question.".to_owned();
    }
    if is_actionable_verification_sentence(trimmed) {
        return pr_sentence(trimmed);
    }
    format!("Confirm {}.", lower_first_ascii(without_trailing))
}

fn is_actionable_verification_sentence(value: &str) -> bool {
    let normalized = value.trim_start().to_ascii_lowercase();
    value.trim_end().ends_with('?')
        || [
            "confirm ", "verify ", "check ", "ensure ", "run ", "add ", "can ", "does ", "do ",
            "is ", "are ", "will ", "should ", "could ", "did ",
        ]
        .iter()
        .any(|prefix| normalized.starts_with(prefix))
}

fn pr_sentence(value: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return "See review artifacts for the recorded evidence.".to_owned();
    }
    if trimmed
        .chars()
        .next_back()
        .is_some_and(|ch| matches!(ch, '.' | '!' | '?'))
    {
        trimmed.to_owned()
    } else {
        format!("{trimmed}.")
    }
}

fn lower_first_ascii(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_uppercase() => {
            format!("{}{}", first.to_ascii_lowercase(), chars.as_str())
        }
        Some(_) => value.to_owned(),
        None => String::new(),
    }
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

fn has_reviewer_value(inline_comments: &[ReviewInlineComment], pr_body: &str) -> bool {
    !inline_comments.is_empty() || pr_body_has_reviewer_value(pr_body)
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

fn model_api_key_label(provider: ModelProvider) -> &'static str {
    match provider {
        ModelProvider::MiniMaxDirect => "minimax API key",
        ModelProvider::OpenCodeGo => "opencode-go API key",
    }
}

fn model_api_key_present(provider: ModelProvider) -> bool {
    env_value_present(model_api_key_env(provider))
}

fn env_value_present(name: &str) -> bool {
    env_value(name).is_some()
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
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
        ProviderEndpointKind::AnthropicMessages => {
            let thinking_type = if spec.provider == ModelProvider::MiniMaxDirect {
                "disabled"
            } else {
                "adaptive"
            };
            serde_json::json!({
                "model": spec.model,
                "max_tokens": model_max_tokens(spec),
                "system": "Return one compact JSON object in the final text block. Do not include markdown fences or prose outside JSON.",
                "thinking": {"type": thinking_type},
                "temperature": 0.1,
                "messages": [
                    {"role": "user", "content": prompt}
                ],
            })
        }
        ProviderEndpointKind::OpenAiChat if spec.provider == ModelProvider::MiniMaxDirect => {
            serde_json::json!({
                "model": spec.model,
                "messages": [
                    {"role": "system", "content": "Return strict JSON only. Do not include markdown fences or prose outside JSON."},
                    {"role": "user", "content": prompt}
                ],
                "max_completion_tokens": model_max_tokens(spec),
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
    let text = model_error_chain_text(err).to_ascii_lowercase();
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

fn model_error_chain_text(err: &anyhow::Error) -> String {
    format!("{err:#}")
}

fn run_curl_json_post(
    root: &Path,
    url: &str,
    auth_header: &str,
    request_path: &Path,
    headers: &[&str],
    timeout_sec: u64,
) -> Result<HttpPostOutput> {
    let (stdout_path, stderr_path) = curl_temp_output_paths(request_path);
    let stdout =
        File::create(&stdout_path).with_context(|| format!("create {}", stdout_path.display()))?;
    let stderr =
        File::create(&stderr_path).with_context(|| format!("create {}", stderr_path.display()))?;
    let data_binary_arg = curl_data_binary_arg(request_path)?;
    let mut command = ProcessCommand::new("curl");
    command
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
        .arg(data_binary_arg)
        .arg(url)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            remove_output_files(&stdout_path, &stderr_path);
            return Err(err).with_context(|| "spawn curl");
        }
    };
    let write_config_result = (|| -> Result<()> {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("curl stdin unavailable"))?;
        use std::io::Write as _;
        for header in headers {
            writeln!(stdin, "header = \"{}\"", curl_config_quote(header))?;
        }
        writeln!(stdin, "header = \"{}\"", curl_config_quote(auth_header))?;
        Ok(())
    })();
    if let Err(err) = write_config_result {
        let _ = child.kill();
        let _ = child.wait();
        remove_output_files(&stdout_path, &stderr_path);
        return Err(err);
    }
    let output = wait_for_child_output_files(child, &stdout_path, &stderr_path, timeout_sec)
        .with_context(|| "wait for curl")?;
    let (stdout, http_status) = split_curl_http_status(output.stdout);
    Ok(HttpPostOutput {
        status: output.status,
        stdout,
        stderr: output.stderr,
        http_status,
    })
}

fn curl_temp_output_paths(request_path: &Path) -> (PathBuf, PathBuf) {
    let dir = request_path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = request_path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("request.json");
    (
        dir.join(format!("{file_name}.curl.stdout.tmp")),
        dir.join(format!("{file_name}.curl.stderr.tmp")),
    )
}

fn curl_data_binary_arg(request_path: &Path) -> Result<String> {
    let absolute = fs::canonicalize(request_path)
        .with_context(|| format!("canonicalize {}", request_path.display()))?;
    let path = absolute.to_string_lossy().replace('\\', "/");
    Ok(format!("@{path}"))
}

fn wait_for_child_output_files(
    mut child: Child,
    stdout_path: &Path,
    stderr_path: &Path,
    timeout_sec: u64,
) -> Result<FileCommandOutput> {
    let status = match child.wait_timeout(Duration::from_secs(timeout_sec))? {
        Some(status) => status,
        None => {
            let _ = child.kill();
            let _ = child.wait();
            let stdout = read_and_remove_output_file(stdout_path)?;
            let stderr = read_and_remove_output_file(stderr_path)?;
            bail!(
                "process timed out after {timeout_sec}s: stderr: {}; stdout: {}",
                String::from_utf8_lossy(&stderr),
                String::from_utf8_lossy(&stdout)
            );
        }
    };
    let stdout = read_and_remove_output_file(stdout_path)?;
    let stderr = read_and_remove_output_file(stderr_path)?;
    Ok(FileCommandOutput {
        status,
        stdout,
        stderr,
    })
}

fn read_and_remove_output_file(path: &Path) -> Result<Vec<u8>> {
    let bytes = fs::read(path).with_context(|| format!("read {}", path.display()))?;
    let _ = fs::remove_file(path);
    Ok(bytes)
}

fn remove_output_files(stdout_path: &Path, stderr_path: &Path) {
    let _ = fs::remove_file(stdout_path);
    let _ = fs::remove_file(stderr_path);
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
    let text = model_error_chain_text(err);
    let needle = "http status Some(";
    let start = text.find(needle)? + needle.len();
    let end = text[start..].find(')')? + start;
    text[start..end].parse::<u16>().ok()
}

fn post_github_review(args: &PostArgs) -> Result<PostResultReceipt> {
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
    validate_github_review_payload_for_post(args, &review)?;
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
    let review_metadata = read_github_review_metadata(args);
    Ok(PostResultReceipt {
        schema_version: 1,
        status: "ok".to_owned(),
        repo: repo.clone(),
        repo_valid: true,
        pull_number,
        comments: review.comments.len(),
        review_json: args.review_json.display().to_string(),
        review_json_exists: args.review_json.exists(),
        review_json_valid: review_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.valid),
        review_event: review_metadata.as_ref().map(|review| review.event.clone()),
        review_body_bytes: review_metadata.as_ref().map(|review| review.body_bytes),
        review_comment_count: review_metadata.as_ref().map(|review| review.comments),
        diff_patch: review_metadata
            .as_ref()
            .map(|review| review.diff_patch.display().to_string())
            .unwrap_or_else(|| post_diff_patch_path(args).display().to_string()),
        diff_patch_exists: review_metadata
            .as_ref()
            .is_some_and(|review| review.diff_patch_exists),
        diff_patch_valid: review_metadata
            .as_ref()
            .is_some_and(|review| review.diff_patch_valid),
        diff_line_count: review_metadata
            .as_ref()
            .and_then(|review| review.diff_line_count),
        off_diff_comment_count: review_metadata
            .as_ref()
            .and_then(|review| review.off_diff_comment_count),
        http_status: output.http_status,
        token_present: true,
        payload_written: post_payload.exists(),
        post_stdout_written: args.out.join("post-stdout.json").exists(),
        post_stderr_written: args.out.join("post-stderr.txt").exists(),
        response,
    })
}

fn validate_github_review_payload(review: &GitHubReview) -> Result<()> {
    validate_github_review_payload_with_policy(review, &ReviewBodyPolicy::default())
}

fn validate_github_review_payload_with_policy(
    review: &GitHubReview,
    policy: &ReviewBodyPolicy,
) -> Result<()> {
    if review.event != "COMMENT" {
        bail!("github review event must be COMMENT");
    }
    validate_pr_review_body_policy(&review.body, policy)?;
    if review.comments.is_empty() && !pr_body_has_reviewer_value(&review.body) {
        bail!("github review body is missing reviewer-value content");
    }
    if has_standalone_approval_line(&review.body) {
        bail!("github review body contains standalone approval language");
    }
    for comment in &review.comments {
        if comment.side != "RIGHT" {
            bail!("github review comments must use side=RIGHT");
        }
        if !is_repo_relative_path(&comment.path) {
            bail!("github review comment path must be repo-relative");
        }
        if comment.line == 0 {
            bail!("github review comment line must be positive");
        }
        if comment.body.trim().is_empty() {
            bail!("github review comment body must not be empty");
        }
        if comment.body.chars().count() > 1_200 {
            bail!("github review comment body must be 1200 chars or fewer");
        }
        if !has_lane_prefix(&comment.body) {
            bail!("github review comment body must start with a lane prefix");
        }
        if has_standalone_approval_line(&comment.body) {
            bail!("github review comment contains standalone approval language");
        }
    }
    Ok(())
}

fn validate_github_review_payload_for_post(args: &PostArgs, review: &GitHubReview) -> Result<()> {
    let review_body_policy = ReviewBodyPolicy::default();
    validate_github_review_payload_with_policy(review, &review_body_policy)?;
    let diff_patch = post_diff_patch_path(args);
    if review.comments.is_empty() {
        return Ok(());
    }
    let patch = fs::read_to_string(&diff_patch)
        .with_context(|| format!("read {}", diff_patch.display()))?;
    let right_lines = right_side_diff_lines(&patch);
    validate_github_review_payload_for_right_lines(
        review,
        &right_lines,
        &diff_patch.display().to_string(),
        &review_body_policy,
    )
}

fn validate_github_review_payload_for_right_lines(
    review: &GitHubReview,
    right_lines: &BTreeSet<(String, u32)>,
    source: &str,
    review_body_policy: &ReviewBodyPolicy,
) -> Result<()> {
    validate_github_review_payload_with_policy(review, review_body_policy)?;
    for comment in &review.comments {
        let path = normalize_repo_path(&comment.path);
        if !right_lines.contains(&(path.clone(), comment.line)) {
            bail!(
                "github review comment {}:{} is not a valid RIGHT-side diff line in {}",
                path,
                comment.line,
                source
            );
        }
    }
    Ok(())
}

fn post_diff_patch_path(args: &PostArgs) -> PathBuf {
    if let Some(path) = &args.diff_patch {
        return path.clone();
    }
    if let Some(review_dir) = args.review_json.parent()
        && review_dir
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name == "review")
        && let Some(run_dir) = review_dir.parent()
    {
        return run_dir.join("input").join("diff.patch");
    }
    args.out
        .parent()
        .map(|run_dir| run_dir.join("input").join("diff.patch"))
        .unwrap_or_else(|| PathBuf::from("target/ub-review/input/diff.patch"))
}

fn is_repo_relative_path(path: &str) -> bool {
    let path = normalize_repo_path(path);
    !path.is_empty()
        && !Path::new(&path).is_absolute()
        && !path.split('/').any(|part| part.is_empty() || part == "..")
}

fn has_lane_prefix(body: &str) -> bool {
    let trimmed = body.trim_start();
    trimmed.starts_with('[')
        && trimmed
            .find(']')
            .is_some_and(|position| position > 1 && position <= 32)
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
    text.push_str(&format!("- Diff class: `{}`\n", diff.diff_class.key()));
    render_review_efficiency_section(&mut text, out);
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
    render_model_status_sections(&mut text, out);
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

fn render_review_efficiency_section(text: &mut String, out: &Path) {
    let Some(metrics) = read_review_metrics(out) else {
        return;
    };
    let runtime = metrics
        .get("wall_clock_seconds")
        .and_then(serde_json::Value::as_u64)
        .map(format_seconds)
        .unwrap_or_else(|| "unknown".to_owned());
    let total_lanes = metrics
        .pointer("/models/model_lanes")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let ok_lanes = metrics
        .pointer("/models/model_lane_status_counts/ok")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let degraded_lanes = metrics
        .pointer("/models/model_lane_status_counts/degraded")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let usable_lanes = ok_lanes.saturating_add(degraded_lanes);
    let inline_comments = metrics
        .get("github_review_comments")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let max_inline_comments = metrics
        .get("max_inline_comments")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let off_diff_rejected = metrics
        .get("off_diff_candidates_rejected")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let provider_failures = metrics
        .get("provider_evidence_failures")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let follow_up_results = metrics
        .pointer("/follow_up_results/total")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let follow_up_attempted = metrics
        .pointer("/follow_up_results/calls_attempted")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    let follow_up_statuses = metrics
        .pointer("/follow_up_results/status_counts")
        .and_then(serde_json::Value::as_object)
        .map(format_json_status_counts)
        .unwrap_or_else(|| "none".to_owned());
    let payload_status = metrics
        .get("review_payload_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let post_status = metrics
        .get("post_status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let terminal_state = metrics
        .get("terminal_state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let coordination_wall = metrics
        .pointer("/run/coordination_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let investigation_wall = metrics
        .pointer("/run/investigation_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let proof_stream_wall = metrics
        .pointer("/run/proof_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let model_wall = metrics
        .pointer("/run/model_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let proof_wall = metrics
        .pointer("/run/local_proof_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let compiler_wall = metrics
        .pointer("/run/compiler_wall_ms")
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let overlap = metrics
        .pointer("/run/investigation_proof_overlap_ms")
        .or_else(|| metrics.pointer("/run/model_proof_overlap_ms"))
        .and_then(serde_json::Value::as_u64)
        .map(format_millis)
        .unwrap_or_else(|| "unknown".to_owned());
    let concurrency_model = metrics
        .pointer("/run/concurrency_model")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let scheduler_profile = metrics
        .pointer("/run/scheduler_profile")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");

    text.push_str("\n## Review efficiency\n\n");
    text.push_str(&format!("- Runtime: `{runtime}`\n"));
    text.push_str(&format!(
        "- Run streams: coordination `{coordination_wall}`, investigation `{investigation_wall}`, proof `{proof_stream_wall}`, investigation/proof overlap `{overlap}` (`{scheduler_profile}` via `{concurrency_model}`)\n"
    ));
    text.push_str(&format!(
        "- Loop detail: model `{model_wall}`, local proof `{proof_wall}`, compiler `{compiler_wall}`\n"
    ));
    text.push_str(&format!("- Terminal state: `{terminal_state}`\n"));
    text.push_str(&format!(
        "- Model lanes: `{usable_lanes}/{total_lanes}` usable (`{ok_lanes}` ok, `{degraded_lanes}` degraded)\n"
    ));
    text.push_str(&format!(
        "- Inline comments: `{inline_comments}/{max_inline_comments}`\n"
    ));
    text.push_str(&format!(
        "- Off-diff candidates rejected: `{off_diff_rejected}`\n"
    ));
    text.push_str(&format!(
        "- Provider evidence failures: `{provider_failures}`\n"
    ));
    text.push_str(&format!(
        "- Follow-up results: `{follow_up_results}` total, `{follow_up_attempted}` attempted ({follow_up_statuses})\n"
    ));
    text.push_str(&format!(
        "- Review payload: `{payload_status}`; post: `{post_status}`\n"
    ));
}

fn format_millis(ms: u64) -> String {
    if ms < 1_000 {
        format!("{ms}ms")
    } else {
        format_seconds(ms / 1_000)
    }
}

fn format_json_status_counts(counts: &serde_json::Map<String, serde_json::Value>) -> String {
    let parts = counts
        .iter()
        .filter_map(|(status, count)| count.as_u64().map(|count| format!("{status}={count}")))
        .collect::<Vec<_>>();
    if parts.is_empty() {
        "none".to_owned()
    } else {
        parts.join(", ")
    }
}

fn read_review_metrics(out: &Path) -> Option<serde_json::Value> {
    let text = fs::read_to_string(out.join("review/metrics.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn format_seconds(seconds: u64) -> String {
    let minutes = seconds / 60;
    let seconds = seconds % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m{seconds:02}s")
    }
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
            "skipped" if is_sensor_evidence_issue(sensor, &receipt.status, &receipt.reason) => {
                missing.push(format!(
                    "{} skipped; {} unavailable; reason: {}.",
                    sensor.id,
                    evidence_label(&sensor.id),
                    receipt.reason
                ));
            }
            status if is_sensor_evidence_issue(sensor, status, &receipt.reason) => {
                missing.push(format!(
                    "{} {}; {} unavailable; reason: {}.",
                    sensor.id,
                    status,
                    evidence_label(&sensor.id),
                    receipt.reason
                ))
            }
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

fn render_model_status_sections(text: &mut String, out: &Path) {
    let Some(review) = read_review_summary_receipt(out) else {
        return;
    };

    text.push_str("\n## Provider preflights\n\n");
    text.push_str(&format!(
        "- Model mode: `{}`\n- Depth: `{}`\n- Provider policy: `{}`\n- Lane width: `{}`\n\n",
        review.model_mode,
        if review.depth.is_empty() {
            ReviewDepth::Standard.key()
        } else {
            review.depth.as_str()
        },
        review.provider_policy,
        review.lane_width
    ));
    if review.provider_preflights.is_empty() {
        text.push_str("- No provider preflight receipts were produced.\n");
    } else {
        text.push_str("| Provider | Model | Endpoint | Status | HTTP | Response | Reason |\n");
        text.push_str("|---|---|---|---|---|---|---|\n");
        for receipt in &review.provider_preflights {
            text.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` | {} | {} | {} |\n",
                receipt.provider,
                receipt.model,
                receipt.endpoint_kind,
                receipt.status,
                optional_u16_cell(receipt.http_status),
                optional_str_cell(receipt.response_shape.as_deref()),
                escape_md(&receipt.reason)
            ));
        }
    }

    text.push_str("\n## Model lane status\n\n");
    if review.model_lanes.is_empty() {
        text.push_str("- No model lane receipts were produced.\n");
    } else {
        text.push_str(
            "| Lane | Provider | Model | Endpoint | Status | Fallback | HTTP | Reason |\n",
        );
        text.push_str("|---|---|---|---|---|---|---|---|\n");
        for receipt in &review.model_lanes {
            text.push_str(&format!(
                "| `{}` | `{}` | `{}` | `{}` | `{}` | {} | {} | {} |\n",
                receipt.lane,
                receipt.provider,
                receipt.model,
                receipt.endpoint_kind,
                receipt.status,
                optional_str_cell(receipt.fallback_from.as_deref()),
                optional_u16_cell(receipt.http_status),
                escape_md(&receipt.reason)
            ));
        }
    }

    let missing_or_failed = model_status_evidence_issues(&review);
    text.push_str("\n## Missing or failed model evidence\n\n");
    if missing_or_failed.is_empty() {
        text.push_str("- No planned model evidence is currently missing or failed.\n");
    } else {
        for item in missing_or_failed {
            text.push_str(&format!("- {}\n", escape_md(&item)));
        }
    }
}

fn model_status_evidence_issues(review: &ReviewSummaryReceipt) -> Vec<String> {
    let mut issues = Vec::new();
    for receipt in &review.provider_preflights {
        if is_model_evidence_issue(&receipt.status) {
            issues.push(format!(
                "Provider preflight `{}` model `{}` endpoint `{}`: `{}` - {}",
                receipt.provider,
                receipt.model,
                receipt.endpoint_kind,
                receipt.status,
                receipt.reason
            ));
        }
    }
    for receipt in &review.model_lanes {
        if is_model_receipt_evidence_issue(receipt) {
            issues.push(format!(
                "Lane `{}` via `{}` model `{}` endpoint `{}`: `{}` - {}",
                receipt.lane,
                receipt.provider,
                receipt.model,
                receipt.endpoint_kind,
                receipt.status,
                receipt.reason
            ));
        }
    }
    issues
}

fn read_review_summary_receipt(out: &Path) -> Option<ReviewSummaryReceipt> {
    let text = fs::read_to_string(out.join("review/review.json")).ok()?;
    serde_json::from_str(&text).ok()
}

fn optional_u16_cell(value: Option<u16>) -> String {
    value
        .map(|value| format!("`{value}`"))
        .unwrap_or_else(|| "-".to_owned())
}

fn optional_str_cell(value: Option<&str>) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .map(|value| format!("`{}`", escape_md(value)))
        .unwrap_or_else(|| "-".to_owned())
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

const CORE_REVIEW_TOOLS: [&str; 5] = ["tokmd", "ripr", "unsafe-review", "ast-grep", "actionlint"];

fn is_core_review_tool(tool_id: &str) -> bool {
    CORE_REVIEW_TOOLS.contains(&tool_id)
}

fn cache_root_path(value: Option<&PathBuf>) -> PathBuf {
    value
        .cloned()
        .or_else(|| std::env::var_os("UB_REVIEW_CACHE_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(".cache/ub-review"))
}

fn base_cache_dir(cache_root: &Path, base_tree_sha: &str) -> PathBuf {
    cache_root.join("bases").join(base_tree_sha)
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .ok()
        .map(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}

fn profile_config_hash(config: &Config) -> Result<String> {
    Ok(sha256_hex(&serde_json::to_vec(config)?))
}

fn git_tree_sha(root: &Path, rev: &str) -> Result<String> {
    let output = ProcessCommand::new("git")
        .arg("rev-parse")
        .arg(format!("{rev}^{{tree}}"))
        .current_dir(root)
        .output()
        .with_context(|| format!("run git rev-parse for {rev}"))?;
    if !output.status.success() {
        bail!(
            "git rev-parse failed for {} in {}: {}",
            rev,
            root.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let tree = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if tree.is_empty() {
        bail!("git rev-parse returned an empty tree sha for {rev}");
    }
    Ok(tree)
}

fn command_version(command: &str) -> Option<String> {
    if !command_on_path(command) {
        return None;
    }
    let output = ProcessCommand::new(command)
        .arg("--version")
        .output()
        .ok()?;
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).to_string()
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(160).collect())
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
    [
        include_str!("../runtime/gh-runner.toml"),
        include_str!("../configs/runtime/gh-runner-standard.toml"),
        include_str!("../configs/runtime/gh-runner-full.toml"),
        include_str!("../runtime/cx23.toml"),
        include_str!("../runtime/cx33.toml"),
        include_str!("../runtime/cx43.toml"),
    ]
    .into_iter()
    .map(|profile| match runtime_profile_from_toml(profile) {
        Ok(profile) => profile,
        Err(err) => {
            eprintln!("fatal: parse builtin runtime profile: {err}");
            std::process::exit(2);
        }
    })
    .collect()
}

fn runtime_profile_from_toml(text: &str) -> Result<Profile> {
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
    })
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

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
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

fn default_lanes_for_diff_class(diff_class: DiffClass) -> Vec<LanePlan> {
    match diff_class {
        DiffClass::SourceUb => default_lanes(),
        DiffClass::SourceGeneral => source_general_lanes(),
        DiffClass::TestsOnly => tests_only_lanes(),
        DiffClass::WorkflowTooling => workflow_tooling_lanes(),
        DiffClass::DocsOnly | DiffClass::ArtifactOnlySmoke => Vec::new(),
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

const WORKFLOW_TOOLING_POSTURE: &str = r#"Standalone approval language is banned.

Return workflow/tooling reviewer value only: findings, verification questions, actionlint/zizmor proof results, refutations, residual workflow risk, parked follow-ups, and trust-affecting missing workflow evidence.

Check permissions, trigger safety, action pinning, checkout credential persistence, fork-only behavior, pull_request_target absence, auxiliary/non-blocking semantics, and actionlint availability.

Do not add ArrayBuffer, worker-handoff, native UB, or source-route narrative unless the diff actually touches those paths.
"#;

const SOURCE_GENERAL_POSTURE: &str = r#"Standalone approval language is banned.

Return changed-behavior reviewer value only: findings, verification questions, proof results, refutations, residual risk, parked follow-ups, and trust-affecting missing evidence.

Check route truth, test proof, overclaims, behavior regressions, performance risk, and smallest-complete-fix boundaries.

Do not add native UB, ArrayBuffer, or worker-handoff narrative unless the diff actually touches those paths.
"#;

const TESTS_ONLY_POSTURE: &str = r#"Standalone approval language is banned.

Return test-review value only: test-oracle gaps, red/green proof questions, proof results, refutations, residual test risk, parked follow-ups, and trust-affecting missing proof evidence.

Check whether tests discriminate the patch, whether assertions are non-tautological, whether focused proof is cheap, and whether missing base+tests evidence affects trust.

Do not add source UB, ArrayBuffer, worker-handoff, or source-route narrative unless the diff actually touches those paths.
"#;

const DOCS_ONLY_POSTURE: &str = r#"Standalone approval language is banned.

Return documentation reviewer value only: factual issues, verification questions, refutations, residual documentation risk, parked follow-ups, and trust-affecting missing evidence.

Check claim accuracy, links, examples, release/process promises, and whether docs overstate unproven behavior.

Do not add source UB, ArrayBuffer, worker-handoff, workflow, or test-proof narrative unless the diff actually touches those paths.
"#;

fn diff_class_posture_heading(diff_class: DiffClass) -> &'static str {
    match diff_class {
        DiffClass::SourceUb => "Bun UB",
        DiffClass::SourceGeneral => "Source-general",
        DiffClass::TestsOnly => "Tests-only",
        DiffClass::WorkflowTooling => "Workflow/tooling",
        DiffClass::DocsOnly => "Docs-only",
        DiffClass::ArtifactOnlySmoke => "Artifact-only smoke",
    }
}

fn review_posture_for_diff_class(diff_class: DiffClass) -> &'static str {
    match diff_class {
        DiffClass::SourceUb => NO_LGTM_POSTURE,
        DiffClass::SourceGeneral => SOURCE_GENERAL_POSTURE,
        DiffClass::TestsOnly => TESTS_ONLY_POSTURE,
        DiffClass::WorkflowTooling => WORKFLOW_TOOLING_POSTURE,
        DiffClass::DocsOnly | DiffClass::ArtifactOnlySmoke => DOCS_ONLY_POSTURE,
    }
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

    use super::{
        BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY, BoxState, Budgets, CommandStatus, Config,
        DEFAULT_REVIEW_PROFILE, DiffClass, DiffContext, DiffFlags, EventLog, FollowUpQuestionTask,
        GitHubReview, GitHubReviewComment, LaneModelOutput, LanePlan, Limits, ModelAssignment,
        ModelCandidateComment, ModelCandidateFinding, ModelEvidenceIssue, ModelLaneReceipt,
        ModelMode, ModelOutputSinks, ModelProvider, ModelProviderPolicy, ModelRunContext,
        NO_LGTM_POSTURE, Observation, OpenCodeEndpointKindArg, Plan, PostArgs, PostingMode,
        PrDecisionContext, PrThreadContext, Profile, ProfileArg, ProofBudget, ProofCommandReceipt,
        ProofReceipt, ProofRequest, ProofRequestGroup, ProviderKindArg, RefuterDecision,
        RefuterOutput, RefuterRunContext, ResourceLease, ReviewArgs, ReviewBodyAudience,
        ReviewBodyExecutionSummaryPolicy, ReviewBodyPolicy, ReviewCompilerInput, ReviewDepth,
        ReviewInlineComment, ReviewMetricsInput, ReviewTerminalState, RunArgs, RunMode,
        STANDARD_LANE_WIDTH, STANDARD_MAX_MODEL_CALLS, STANDARD_MODEL_CONCURRENCY, SelectorArgs,
        SensorEvidenceIssue, SensorPlan, SensorStatusWrite, SummaryOnlyFinding, TerminalStateInput,
        ToolClass, append_follow_up_evidence_witnesses, append_follow_up_proof_requests,
        apply_model_output, apply_plan_selectors, apply_refuter_output,
        apply_runtime_profile_limits, build_candidate_records, build_orchestrator_plan,
        build_review_metrics, build_review_terminal_state, build_tokmd_sensor_commands,
        build_witness_records, builtin_profiles, cap_review_body, classify_diff,
        classify_diff_class, classify_proof_cost, cmd_post, collect_pr_thread_context,
        collect_sensor_evidence_issues, combined_observations, command_display,
        compile_review_surface, dedupe_inline_comments, deep_minimax_lanes, default_lanes,
        direct_minimax_spec, extract_model_content, focused_test_tasks_from_diff,
        follow_up_evidence_from_outputs, follow_up_model_lane_id, follow_up_output_record,
        github_review_skip_path, http_status_from_error, is_model_receipt_evidence_issue,
        model_api_url, model_assignments, model_auth_header, model_json_payload, model_lane,
        model_request_payload, model_response_shape, normalize_run_args,
        observation_summary_artifacts, opencode_canary_spec, pr_decision_sentence, proof_budget,
        proof_lease_budget, provider_spec_for_lane_with_key_state, read_candidate_review_surfaces,
        read_github_event_pr_context, render_lane_model_prompt, render_ledger_context,
        render_pr_thread_context, render_review_body, render_summary, review_lanes_for_args,
        right_side_diff_lines, run_available_model_lanes, run_command_to_files, run_refuter_pass,
        run_sensor, runtime_profile_from_toml, runtime_profile_override, sensor_job_count,
        sha256_hex, split_curl_http_status, standard_minimax_lanes, validate_github_review_payload,
        validate_github_review_payload_for_post, validate_inline_candidate,
        validate_pr_review_body_policy, validate_run_args, validate_summary_only_candidate,
        wait_for_child_output_files, write_candidate_artifacts, write_follow_up_evidence_artifact,
        write_follow_up_output_artifacts, write_github_review_payload, write_observation_artifacts,
        write_orchestrator_artifacts, write_proof_receipt_artifacts, write_proof_request_artifacts,
        write_resource_lease_artifacts, write_review_artifacts, write_sensor_status,
        write_witness_artifacts,
    };

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
    fn unsafe_tokens_trigger_native_risk() {
        let flags = classify_diff(&["src/lib.rs".to_owned()], "+ let p = bytes.as_ptr();");
        assert!(flags.rust_changed);
        assert!(flags.unsafe_or_native_risk);
        assert_eq!(
            classify_diff_class(&["src/lib.rs".to_owned()], &flags),
            DiffClass::SourceUb
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
        plan.lanes = super::default_lanes_for_diff_class(DiffClass::WorkflowTooling);
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
            runtime_profile_from_toml(include_str!("../configs/runtime/gh-runner-standard.toml"))?,
            runtime_profile_from_toml(include_str!("../configs/runtime/gh-runner-full.toml"))?,
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
        Ok(())
    }

    #[test]
    fn runtime_config_presets_match_embedded_profiles_for_shared_names() -> Result<()> {
        let shared = [
            (
                include_str!("../runtime/cx23.toml"),
                include_str!("../configs/runtime/cx23.toml"),
            ),
            (
                include_str!("../runtime/cx33.toml"),
                include_str!("../configs/runtime/cx33.toml"),
            ),
            (
                include_str!("../runtime/cx43.toml"),
                include_str!("../configs/runtime/cx43.toml"),
            ),
        ];
        for (embedded, config) in shared {
            assert_eq!(
                serde_json::to_value(runtime_profile_from_toml(embedded)?)?,
                serde_json::to_value(runtime_profile_from_toml(config)?)?
            );
        }
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
        Ok(())
    }

    #[test]
    fn tokmd_sensor_commands_use_on_diff_analyze_cockpit_and_context() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("repo");
        fs::create_dir_all(repo.join("src"))?;
        fs::write(repo.join("src/lib.rs"), "pub fn value() -> usize { 1 }\n")?;
        run_test_command(&repo, "git", &["init"])?;
        run_test_command(
            &repo,
            "git",
            &["config", "user.email", "ub-review@example.invalid"],
        )?;
        run_test_command(&repo, "git", &["config", "user.name", "UB Review Test"])?;
        run_test_command(&repo, "git", &["add", "."])?;
        run_test_command(&repo, "git", &["commit", "-m", "baseline"])?;
        fs::write(repo.join("src/lib.rs"), "pub fn value() -> usize { 2 }\n")?;
        run_test_command(&repo, "git", &["add", "."])?;
        run_test_command(&repo, "git", &["commit", "-m", "touch source"])?;

        let plan = test_plan(vec![sensor_plan("tokmd", "tokmd", true)]);
        let dir = temp.path().join("out/sensors/tokmd");
        let commands = build_tokmd_sensor_commands(&repo, &dir, &plan);
        let command_texts = commands
            .iter()
            .map(|command| command.argv.join(" "))
            .collect::<Vec<_>>();

        assert!(command_texts.iter().any(|command| command.contains(
            "tokmd analyze --preset estimate --effort-base-ref HEAD~1 --effort-head-ref HEAD"
        )));
        assert!(
            command_texts
                .iter()
                .any(|command| command.contains("tokmd cockpit --base HEAD~1 --head HEAD"))
        );
        assert!(
            command_texts
                .iter()
                .any(|command| command.contains("tokmd context"))
        );
        assert!(
            command_texts
                .iter()
                .any(|command| command.contains("src/lib.rs"))
        );
        assert!(
            commands
                .iter()
                .any(|command| command.stdout_path.ends_with("analyze.md"))
        );
        assert!(
            commands
                .iter()
                .any(|command| command.stdout_path.ends_with("cockpit.json"))
        );
        assert!(
            commands
                .iter()
                .any(|command| command.argv.iter().any(|arg| arg.ends_with("context.md")))
        );
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
    fn sensor_receipt_defaults_missing_exit_fields() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let status_path = temp
            .path()
            .join("sensors/ripr/ub-review-sensor-status.json");
        let parent = status_path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("status path missing parent"))?;
        fs::create_dir_all(parent)?;
        fs::write(
            &status_path,
            serde_json::to_vec_pretty(&serde_json::json!({
                "sensor": "ripr",
                "status": "missing",
                "reason": "command not found"
            }))?,
        )?;

        let receipt = super::read_sensor_receipt(&status_path)
            .ok_or_else(|| anyhow::anyhow!("sensor receipt missing"))?;

        assert_eq!(receipt.status, "missing");
        assert_eq!(receipt.reason, "command not found");
        assert_eq!(receipt.exit_code, None);
        assert!(!receipt.timed_out);
        Ok(())
    }

    #[test]
    fn sensor_timeout_status_is_returned() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let stdout_path = temp.path().join("stdout.txt");
        let stderr_path = temp.path().join("stderr.txt");
        let argv = sleeper_argv();

        let status = run_command_to_files(
            temp.path(),
            &argv,
            &BTreeMap::new(),
            1,
            &stdout_path,
            &stderr_path,
        )?;

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
    fn running_summary_reports_planned_skipped_sensor_evidence() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let planned_dry_run = sensor_plan("tokmd", "tokmd", true);
        let trigger_skipped = sensor_plan("ripr", "ripr", false);
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
        let plan = test_plan(vec![planned_dry_run, trigger_skipped]);

        let summary = render_summary(&out, &plan, &test_diff())?;
        let missing = summary_section(&summary, "## Missing evidence", "## Lane packets")
            .ok_or_else(|| anyhow::anyhow!("missing evidence section not found"))?;

        assert!(missing.contains("tokmd skipped; deterministic repository/diff packet unavailable; reason: dry-run; sensor not executed."));
        assert!(!missing.contains("ripr"));
        assert!(!missing.contains("No planned sensor evidence is currently missing."));
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
        let missing_evidence = validate_inline_candidate(
            &lane,
            ModelCandidateComment {
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/lib.rs".to_owned(),
                line: 2,
                body: "[tests] line-valid but unsupported claim".to_owned(),
                evidence: "".to_owned(),
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
            }],
            candidate_findings: Vec::new(),
            summary_only_findings: Vec::new(),
            observations: Vec::new(),
            failed_objections: Vec::new(),
            proof_requests: Vec::new(),
            degraded: false,
        };
        let mut inline_comments = Vec::new();
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();

        apply_model_output(
            &lane,
            output,
            &line_map,
            8,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut model_observations,
                proof_requests: &mut proof_requests,
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

        apply_model_output(
            &lane,
            output,
            &line_map,
            8,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
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
        apply_model_output(
            &lane,
            output,
            &BTreeSet::new(),
            8,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
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
        apply_model_output(
            &lane,
            output,
            &BTreeSet::new(),
            8,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
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
    fn proof_request_artifacts_group_duplicate_commands_once() -> Result<()> {
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts".to_owned(),
                reason: "Need old-main red witness.".to_owned(),
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
                command: "bun test test/js/bun/md/md-edge-cases.test.ts".to_owned(),
                reason: "Confirm the same focused test before posting.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: true,
                status: "requested".to_owned(),
            },
        ];

        let temp = tempfile::tempdir()?;
        write_proof_request_artifacts(
            temp.path(),
            &test_diff(),
            &Profile::default(),
            &proof_requests,
            &[] as &[ProofReceipt],
        )?;

        let proof_json: Vec<super::ProofRequest> =
            serde_json::from_slice(&fs::read(temp.path().join("review/proof_requests.json"))?)?;
        let proof_groups: Vec<ProofRequestGroup> = serde_json::from_slice(&fs::read(
            temp.path().join("review/proof_request_groups.json"),
        )?)?;
        let proof_plan = fs::read_to_string(temp.path().join("review/proof_plan.md"))?;
        let proof_ndjson = fs::read_to_string(temp.path().join("proof_requests.ndjson"))?;

        assert_eq!(proof_json.len(), 2);
        assert_eq!(proof_ndjson.lines().count(), 2);
        assert_eq!(proof_groups.len(), 1);
        let group = &proof_groups[0];
        assert_eq!(group.schema, "ub-review.proof_request_group.v1");
        assert_eq!(
            group.command,
            "bun test test/js/bun/md/md-edge-cases.test.ts"
        );
        assert_eq!(
            group.requested_by,
            vec!["tests-oracle".to_owned(), "opposition".to_owned()]
        );
        assert_eq!(
            group.request_ids,
            vec![
                "proof-tests-001".to_owned(),
                "proof-opposition-001".to_owned()
            ]
        );
        assert_eq!(group.reasons.len(), 2);
        assert_eq!(group.duplicate_count, 2);
        assert!(group.required);
        assert_eq!(group.status, "requested");
        assert!(proof_plan.contains("Grouped proof broker tasks: 1 unique from 2 request(s)."));
        assert!(proof_plan.contains("merged_requests=2"));
        Ok(())
    }

    #[test]
    fn focused_proof_tasks_detect_changed_test_names_and_merge_lane_requests() -> Result<()> {
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
                max_focused_tests: 6,
                per_command_timeout_sec: 300,
                max_total_seconds: 1_200,
            },
        );

        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].file, "test/js/bun/md/md-edge-cases.test.ts");
        assert_eq!(tasks[0].mode, super::FocusedProofMode::RedGreen);
        assert_eq!(
            tasks[0].test_name.as_deref(),
            Some("snapshots resizable ArrayBuffer input")
        );
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
        assert_eq!(
            tasks[1].test_name.as_deref(),
            Some("keeps stable bytes after getter reentry")
        );
        assert_eq!(tasks[1].mode, super::FocusedProofMode::RedGreen);
        let time_capped_tasks = focused_test_tasks_from_diff(
            &diff,
            &proof_requests,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 6,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
        );
        assert_eq!(time_capped_tasks.len(), 1);
        assert_eq!(proof_budget(&Profile::default())?.max_focused_tests, 1);
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
                command: "bun test test/js/bun/ffi/ffi.test.js -t 'ffi toBuffer bad free'"
                    .to_owned(),
                reason: "Run the requested focused Bun proof.".to_owned(),
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
                command:
                    "bun bd test test/js/bun/ffi/ffi.test.js --test-name-pattern \"ffi toBuffer bad free\""
                        .to_owned(),
                reason: "Confirm the same requested focused Bun proof.".to_owned(),
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
                max_total_seconds: 600,
            },
        );
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].mode, super::FocusedProofMode::RedGreen);
        assert_eq!(tasks[0].file, "test/js/bun/ffi/ffi.test.js");
        assert_eq!(tasks[0].test_name.as_deref(), Some("ffi toBuffer bad free"));
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
                commands.push(super::command_display_with_env(env, argv));
                let is_base = stdout.to_string_lossy().contains("base-plus-tests");
                assert_eq!(env.contains_key("USE_SYSTEM_BUN"), is_base);
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
        assert_eq!(lease.timeout_sec, 600);
        assert_eq!(lease.worktree, Some("base-plus-tests".to_owned()));
        assert!(lease.command.as_deref().is_some_and(
            |command| command.contains("head: cwd=") && command.contains("base+tests:")
        ));
        Ok(())
    }

    #[test]
    fn proof_budget_comes_from_runtime_profile_budgets() -> Result<()> {
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;
        let cx23 = profiles
            .iter()
            .find(|profile| profile.name == "cx23")
            .ok_or_else(|| anyhow::anyhow!("missing cx23 profile"))?;
        let cx43 = profiles
            .iter()
            .find(|profile| profile.name == "cx43")
            .ok_or_else(|| anyhow::anyhow!("missing cx43 profile"))?;

        assert_eq!(proof_budget(gh_runner)?.max_focused_tests, 1);
        assert_eq!(proof_budget(cx23)?.max_focused_tests, 2);
        assert_eq!(proof_budget(cx43)?.max_focused_tests, 6);
        assert_eq!(proof_budget(cx43)?.per_command_timeout_sec, 600);
        assert_eq!(proof_budget(cx43)?.max_total_seconds, 1_800);
        Ok(())
    }

    #[test]
    fn invalid_enabled_proof_budget_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_command_timeout_sec: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let err = proof_budget(&profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid proof budget unexpectedly passed"))?;

        assert!(
            err.to_string()
                .contains("runtime profile broken has proof_command_timeout_sec=0")
        );
        Ok(())
    }

    #[test]
    fn proof_lease_budget_comes_from_runtime_profile_budgets() -> Result<()> {
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;
        let cx23 = profiles
            .iter()
            .find(|profile| profile.name == "cx23")
            .ok_or_else(|| anyhow::anyhow!("missing cx23 profile"))?;
        let cx43 = profiles
            .iter()
            .find(|profile| profile.name == "cx43")
            .ok_or_else(|| anyhow::anyhow!("missing cx43 profile"))?;

        assert_eq!(proof_lease_budget(gh_runner)?.cpu, 2);
        assert_eq!(proof_lease_budget(gh_runner)?.memory_mb, 2_048);
        assert_eq!(proof_lease_budget(gh_runner)?.disk_mb, 1_024);
        assert_eq!(proof_lease_budget(cx23)?.cpu, 1);
        assert_eq!(proof_lease_budget(cx23)?.memory_mb, 1_024);
        assert_eq!(proof_lease_budget(cx43)?.cpu, 4);
        assert_eq!(proof_lease_budget(cx43)?.disk_mb, 2_048);
        Ok(())
    }

    #[test]
    fn proof_planner_skips_actionlint_when_workflows_are_unchanged() {
        let mut diff = test_diff();
        diff.flags.workflow_changed = false;

        let skips = super::proof_planner_skips(&diff);

        assert!(skips.iter().any(|skip| {
            skip.kind == "actionlint" && skip.reason == "No workflow files changed."
        }));
    }

    #[test]
    fn proof_planner_keeps_actionlint_relevant_for_workflow_changes() {
        let mut diff = test_diff();
        diff.flags.workflow_changed = true;
        diff.changed_files = vec![".github/workflows/ci.yml".to_owned()];

        let skips = super::proof_planner_skips(&diff);

        assert!(!skips.iter().any(|skip| skip.kind == "actionlint"));
    }

    #[test]
    fn invalid_enabled_proof_lease_budget_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_cpu: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let err = proof_lease_budget(&profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid proof lease budget unexpectedly passed"))?;

        assert!(
            err.to_string()
                .contains("runtime profile broken has proof_cpu=0")
        );
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
    fn proof_request_status_enforces_v0_bun_test_allowlist() {
        let lane = model_lane(
            "tests-oracle",
            "Tests oracle",
            &["tokmd"],
            "Check focused proof requests.",
        );
        let requests = vec![
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots input'"
                        .to_owned(),
                    reason: "Run the focused Bun test.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(true),
                },
                0,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command:
                        "bun bd test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots input'"
                            .to_owned(),
                    reason: "Confirm the patched Bun development binary.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                1,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "cargo build --workspace".to_owned(),
                    reason: "Compile the workspace.".to_owned(),
                    cost: Some("focused-build".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                2,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "bun test test/js/bun/md/md-edge-cases.test.ts && rm -rf target"
                        .to_owned(),
                    reason: "Shell-shaped command should not be brokered.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                3,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: String::new(),
                    reason: "Missing command.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                4,
            ),
        ];

        assert_eq!(requests[0].status, "requested");
        assert_eq!(requests[0].cost, "focused-test");
        assert_eq!(requests[1].status, "requested");
        assert_eq!(requests[1].cost, "focused-test");
        assert_eq!(requests[2].status, "unsupported");
        assert_eq!(requests[2].cost, "focused-build");
        assert_eq!(requests[3].status, "unsupported");
        assert_eq!(requests[4].status, "invalid");
        assert_eq!(requests[4].command, "<missing command>");

        let groups = super::proof_request_groups(&requests);
        assert_eq!(groups.len(), 5);
        assert!(groups.iter().any(|group| group.status == "requested"));
        assert!(groups.iter().any(|group| group.status == "unsupported"));
        assert!(groups.iter().any(|group| group.status == "invalid"));

        let task = super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &groups,
        );
        assert_eq!(task.requested_by, vec!["tests-oracle".to_owned()]);
        assert_eq!(task.request_ids.len(), 2);
        assert!(task.request_ids.contains(&requests[0].id));
        assert!(task.request_ids.contains(&requests[1].id));
    }

    #[test]
    fn proof_request_artifacts_write_focused_planner_without_execution() -> Result<()> {
        let temp = tempfile::tempdir()?;
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
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Need focused red/green witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-build-001".to_owned(),
                lane: "architecture".to_owned(),
                requested_by: vec!["architecture".to_owned()],
                command: "cargo build --workspace".to_owned(),
                reason: "Compile proof is outside proof broker v0.".to_owned(),
                cost: "focused-build".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "unsupported".to_owned(),
            },
        ];

        write_proof_request_artifacts(
            temp.path(),
            &diff,
            &Profile::default(),
            &proof_requests,
            &[] as &[ProofReceipt],
        )?;

        let proof_plan = fs::read_to_string(temp.path().join("review/proof_plan.md"))?;
        let proof_groups: Vec<ProofRequestGroup> = serde_json::from_slice(&fs::read(
            temp.path().join("review/proof_request_groups.json"),
        )?)?;

        assert!(proof_plan.contains("## Focused proof plan"));
        assert!(proof_plan.contains("mode=`red-green`"));
        assert!(proof_plan.contains("status=unsupported"));
        assert!(proof_plan.contains("No proof broker commands were executed"));
        assert!(
            proof_groups
                .iter()
                .any(|group| group.status == "unsupported")
        );
        assert!(proof_plan.contains(
            "head=`cwd=target/ub-review/proof-worktrees/head bun bd test test/js/bun/md/md-edge-cases.test.ts -t"
        ));
        assert!(
            proof_plan.contains(
                "base+tests=`cwd=target/ub-review/proof-worktrees/base-plus-tests USE_SYSTEM_BUN=1 bun test test/js/bun/md/md-edge-cases.test.ts -t"
            )
        );
        assert!(!temp.path().join("review/proof_receipts.json").exists());
        assert!(!temp.path().join("proof_receipts.ndjson").exists());
        Ok(())
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
                unreachable!("base+tests worktree should not be prepared after budget is spent")
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
            Err(anyhow::anyhow!("patch did not apply"))
        };

        let receipt = super::run_focused_red_green_proof_task(
            temp.path(),
            &out,
            &diff,
            &task,
            300,
            &mut runner,
            &mut prepare,
        )?;

        assert_eq!(receipt.result, "base_patch_failed");
        assert_eq!(receipt.commands.len(), 2);
        assert_eq!(receipt.commands[0].status, "passed");
        assert_eq!(receipt.commands[1].side, "base-plus-tests");
        assert_eq!(receipt.commands[1].status, "skipped");
        assert!(super::proof_receipt_is_missing_evidence(&receipt));
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
                unreachable!("proof command should not run without a lease")
            },
            |_root, _out, _diff| {
                unreachable!("base+tests worktree should not be prepared without a lease")
            },
        )?;

        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.proof_receipts[0].result, "skipped_profile");
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.resource_leases[0].status, "skipped_profile");
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
                unreachable!("proof command should not run when proof budget is zero")
            },
            |_root, _out, _diff| {
                unreachable!("base+tests worktree should not be prepared when proof budget is zero")
            },
        )?;

        assert_eq!(proof_result.proof_receipts.len(), 1);
        assert_eq!(proof_result.proof_receipts[0].result, "skipped_budget");
        assert_eq!(proof_result.resource_leases.len(), 1);
        assert_eq!(proof_result.resource_leases[0].status, "exhausted");
        Ok(())
    }

    #[test]
    fn prepare_base_plus_tests_worktree_allows_source_only_request_without_test_patch() -> Result<()>
    {
        let repo = tempfile::tempdir()?;
        fs::create_dir_all(repo.path().join("src"))?;
        fs::write(repo.path().join("src/lib.rs"), "pub fn current() {}\n")?;
        run_test_command(repo.path(), "git", &["init"])?;
        run_test_command(
            repo.path(),
            "git",
            &["config", "user.email", "ub-review@example.invalid"],
        )?;
        run_test_command(
            repo.path(),
            "git",
            &["config", "user.name", "UB Review Test"],
        )?;
        run_test_command(repo.path(), "git", &["add", "."])?;
        run_test_command(
            repo.path(),
            "git",
            &["-c", "commit.gpgsign=false", "commit", "-m", "initial"],
        )?;

        let out = tempfile::tempdir()?;
        let diff = DiffContext {
            base: "HEAD".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["src/lib.rs".to_owned()],
            patch: "+pub fn changed() {}\n".to_owned(),
            flags: DiffFlags {
                source_changed: true,
                rust_changed: true,
                rust_tests_changed: false,
                workflow_changed: false,
                dependency_changed: false,
                shell_changed: false,
                cpp_changed: false,
                docs_only: false,
                unsafe_or_native_risk: true,
            },
            diff_class: DiffClass::SourceUb,
        };
        assert!(super::base_plus_tests_patch_files(&diff).is_empty());

        let worktree = super::prepare_base_plus_tests_worktree(repo.path(), out.path(), &diff)?;

        assert!(worktree.join("src/lib.rs").exists());
        assert!(!out.path().join("proof/base-plus-tests.patch").exists());
        super::cleanup_base_plus_tests_worktree(repo.path(), &worktree)?;
        Ok(())
    }

    #[test]
    fn base_plus_tests_patch_files_excludes_source_fix_files() {
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec![
                "src/native/write.rs".to_owned(),
                "test/js/node/fs/write.test.ts".to_owned(),
                "test/fixtures/fs/write.bin".to_owned(),
                "docs/usage.md".to_owned(),
                "tests/doctest/bytea.md".to_owned(),
            ],
            patch: String::new(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::SourceUb,
        };

        let files = super::base_plus_tests_patch_files(&diff);

        assert_eq!(
            files,
            vec![
                "test/js/node/fs/write.test.ts".to_owned(),
                "test/fixtures/fs/write.bin".to_owned(),
                "tests/doctest/bytea.md".to_owned(),
            ]
        );
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

        apply_model_output(
            &lane,
            output,
            &line_map,
            8,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
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

        apply_model_output(
            &lane,
            output,
            &BTreeSet::new(),
            8,
            ModelOutputSinks {
                inline_comments: &mut inline_comments,
                summary_only_findings: &mut summary_only_findings,
                model_observations: &mut observations,
                proof_requests: &mut proof_requests,
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
                degraded: false,
            };
            let mut inline_comments = Vec::new();
            let mut summary_only_findings = Vec::new();
            let mut model_observations = Vec::new();
            let mut proof_requests = Vec::new();

            apply_model_output(
                &lane,
                output,
                &BTreeSet::new(),
                8,
                ModelOutputSinks {
                    inline_comments: &mut inline_comments,
                    summary_only_findings: &mut summary_only_findings,
                    model_observations: &mut model_observations,
                    proof_requests: &mut proof_requests,
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
        }];
        let mut summary_only_findings = Vec::new();
        let mut model_observations = Vec::new();
        let mut proof_requests = Vec::new();
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
            },
            &mut model_lanes,
            &mut missing_or_failed_model_evidence,
            &mut inline_comments,
            &mut summary_only_findings,
            &mut model_observations,
            &mut proof_requests,
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
            }],
        };
        validate_github_review_payload(&ok)?;

        let temp = tempfile::tempdir()?;
        write_github_review_payload(temp.path(), &ok, &line_map, &ReviewBodyPolicy::default())?;
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
                &ReviewBodyPolicy::default()
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
                &ReviewBodyPolicy::default()
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
    fn sensor_jobs_use_runtime_profile_limit() -> Result<()> {
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;
        let cx23 = profiles
            .iter()
            .find(|profile| profile.name == "cx23")
            .ok_or_else(|| anyhow::anyhow!("missing cx23 profile"))?;
        let cx43 = profiles
            .iter()
            .find(|profile| profile.name == "cx43")
            .ok_or_else(|| anyhow::anyhow!("missing cx43 profile"))?;

        assert_eq!(sensor_job_count(gh_runner, 10)?, 4);
        assert_eq!(sensor_job_count(cx23, 10)?, 2);
        assert_eq!(sensor_job_count(cx43, 10)?, 6);
        Ok(())
    }

    #[test]
    fn sensor_jobs_cap_to_runnable_sensors() -> Result<()> {
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;

        assert_eq!(sensor_job_count(gh_runner, 2)?, 2);
        Ok(())
    }

    #[test]
    fn zero_sensor_runtime_limit_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            limits: Limits {
                sensor_jobs: 0,
                ..Limits::default()
            },
            ..Profile::default()
        };

        let err = sensor_job_count(&profile, 2)
            .err()
            .ok_or_else(|| anyhow::anyhow!("zero sensor limit unexpectedly passed"))?;

        assert!(
            err.to_string()
                .contains("runtime profile broken has sensor_jobs=0")
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
    fn tool_selectors_filter_planned_sensors() -> Result<()> {
        let mut plan = test_plan(vec![
            sensor_plan("tokmd", "tokmd", true),
            sensor_plan("ripr", "ripr", true),
            sensor_plan("ast-grep", "ast-grep", true),
        ]);
        let selectors = SelectorArgs {
            tools: "tokmd,ripr".to_owned(),
            except_tools: "ripr".to_owned(),
            ..SelectorArgs::default()
        };

        apply_plan_selectors(&mut plan, &selectors)?;

        assert_eq!(
            plan.sensors
                .iter()
                .map(|sensor| sensor.id.as_str())
                .collect::<Vec<_>>(),
            vec!["tokmd"]
        );
        assert!(
            plan.notes
                .iter()
                .any(|note| note.contains("tool selectors"))
        );
        Ok(())
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
    fn minimax_anthropic_payload_uses_messages_shape() {
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
    fn pr_review_body_renders_specific_residual_risk_only() {
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

        assert!(body.contains("## Residual risk"));
        assert!(body.contains("FileHandle.write test was not proven"));
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
            plan: &plan,
            review_payload_status: "skipped_empty_smoke",
            should_prepare_github_review: false,
            pr_body: "",
            inline_comments: &[],
            summary_only_findings: &[],
            model_lanes: &model_lanes,
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            proof_receipts: &[],
        });

        assert_eq!(state.status, "sufficient");
        assert_eq!(state.usable_model_lanes, 1);
        assert!(!state.reviewer_value_present);
    }

    #[test]
    fn terminal_state_keeps_model_off_runs_artifact_only() {
        let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
        args.model_mode = ModelMode::Off;
        let plan = test_plan(Vec::new());
        let state = build_review_terminal_state(TerminalStateInput {
            args: &args,
            plan: &plan,
            review_payload_status: "skipped_empty_smoke",
            should_prepare_github_review: false,
            pr_body: "",
            inline_comments: &[],
            summary_only_findings: &[],
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            proof_receipts: &[],
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
            plan: &plan,
            review_payload_status: "skipped_empty_smoke",
            should_prepare_github_review: false,
            pr_body: "",
            inline_comments: &[],
            summary_only_findings: &[],
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &missing_model,
            proof_receipts: &[],
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
            plan: &plan,
            review_payload_status: "prepared",
            should_prepare_github_review: true,
            pr_body: "## Verification questions\n\n- Confirm the focused proof.",
            inline_comments: &[],
            summary_only_findings: &[],
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            proof_receipts: &[],
        });

        assert_eq!(state.status, "needs-reviewer-attention");
        assert!(state.reviewer_value_present);
    }

    #[test]
    fn compiler_surface_promotes_follow_up_observation_to_final_review() -> Result<()> {
        let args = test_run_args(Path::new("target/ub-review").to_path_buf());
        let plan = test_plan(Vec::new());
        let diff = test_diff();
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
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[],
            summary_only_findings: &[],
            observations: &[follow_up_observation],
            proof_receipts: &[],
        })?;

        assert!(surface.should_prepare_github_review);
        assert_eq!(surface.review_payload_status, "prepared");
        assert_eq!(surface.terminal_state.status, "needs-reviewer-attention");
        assert!(surface.github_review.body.contains("## Refuted"));
        assert!(
            surface
                .github_review
                .body
                .contains("source-route concern was refuted")
        );
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
        })?;

        assert!(!surface.should_prepare_github_review);
        assert_eq!(surface.review_payload_status, "skipped_empty_smoke");
        assert_eq!(surface.terminal_state.status, "sufficient");
        assert!(surface.github_review.body.is_empty());
        assert!(surface.github_review.comments.is_empty());
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
        plan.lanes = super::default_lanes_for_diff_class(DiffClass::WorkflowTooling);
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
    fn pr_review_body_renders_non_discriminating_proof_as_residual_risk() {
        let receipt = test_red_green_proof_receipt("non_discriminating", "passed");
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

        assert!(!body.contains("## Decision"));
        assert!(!body.contains("No blocking UB finding from this pass."));
        assert!(body.contains("## Residual risk"));
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
        let receipt = test_proof_receipt("timed_out", "timed_out");
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
            &args,
            &event_log,
            &run_started,
            &mut run_loop_tracker,
            std::time::Duration::from_secs(73),
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
    fn review_metrics_count_efficiency_facts() {
        let review = super::ReviewArtifacts {
            shared_context_id: "abc123".to_owned(),
            review_profile: DEFAULT_REVIEW_PROFILE.to_owned(),
            mode: "review-direct".to_owned(),
            posting: "review".to_owned(),
            runtime_profile: "gh-runner".to_owned(),
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
                },
            ],
            model_lanes: vec![model_lane_receipt("tests-oracle", "ok")],
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
            run: test_run_loop_metrics(),
            elapsed: std::time::Duration::from_secs(601),
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
        assert_eq!(metrics.terminal_state, "needs-reviewer-attention");
        assert_eq!(metrics.observations, 0);
        assert_eq!(metrics.follow_up_results.total, 2);
        assert_eq!(metrics.follow_up_results.status_counts["ok"], 1);
        assert_eq!(metrics.follow_up_results.status_counts["skipped_budget"], 1);
        assert_eq!(metrics.follow_up_results.calls_attempted, 1);
        assert_eq!(metrics.proof_requests, 0);
        assert_eq!(metrics.proof_receipts, 0);
        assert_eq!(metrics.resource_leases, 0);
        assert_eq!(metrics.github_review_body_bytes, "pr body".len());
        assert_eq!(metrics.artifact_review_body_bytes, "artifact body".len());
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
            build_orchestrator_plan(&candidates, &observation_summary.unique, &[], &[]);
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
        );
        write_orchestrator_artifacts(temp.path(), &plan)?;

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
                args: &args,
                model_calls_used: 0,
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
            follow_up_output_record(&task, &model_lane, "ok", "completed", output, &line_map, 4);

        assert_eq!(record.schema, "ub-review.follow_up_output.v1");
        assert_eq!(record.task_id, task.id);
        assert_eq!(record.stage, "secondary");
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
                .contains("Follow-up proof request arrived after proof broker v0 execution")
        );
        append_follow_up_proof_requests(&mut canonical_proof_requests, &evidence);
        assert_eq!(canonical_proof_requests.len(), 1);
        let mut witnesses = Vec::new();
        append_follow_up_evidence_witnesses(&mut witnesses, &evidence);
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

        write_follow_up_output_artifacts(temp.path(), &outputs)?;
        write_follow_up_evidence_artifact(temp.path(), &evidence)?;
        write_witness_artifacts(temp.path(), &witnesses)?;
        write_proof_request_artifacts(
            temp.path(),
            &test_diff(),
            &Profile::default(),
            &canonical_proof_requests,
            &[] as &[ProofReceipt],
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
            serde_json::to_value(&canonical_proof_requests)?
        );
        let proof_request_file: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path()
                .join("proof_requests")
                .join(format!("{}.json", canonical_proof_requests[0].id)),
        )?)?;
        assert_eq!(proof_request_file, serde_json::to_value(&proof_json[0])?);
        let proof_ndjson = fs::read_to_string(temp.path().join("proof_requests.ndjson"))?;
        assert_eq!(proof_ndjson.lines().count(), 1);
        assert!(
            proof_ndjson
                .contains("Follow-up proof request arrived after proof broker v0 execution")
        );
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
    fn observation_artifacts_include_aggregate_and_lane_ndjson() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review = super::ReviewArtifacts {
            shared_context_id: "abc123".to_owned(),
            review_profile: DEFAULT_REVIEW_PROFILE.to_owned(),
            mode: "review-direct".to_owned(),
            posting: "review".to_owned(),
            runtime_profile: "gh-runner".to_owned(),
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

    fn run_test_command(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
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
        }
    }

    fn test_follow_up_result(task_id: &str, group_id: &str, status: &str) -> super::FollowUpResult {
        super::FollowUpResult {
            schema: "ub-review.follow_up_result.v1".to_owned(),
            task_id: task_id.to_owned(),
            group_id: group_id.to_owned(),
            stage: "secondary".to_owned(),
            packet_path: format!("questions/orchestrator-follow-up/{task_id}.json"),
            model_lane: format!("orchestrator-follow-up-{task_id}"),
            status: status.to_owned(),
            reason: "test follow-up result".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
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
        }
    }

    fn test_proof_receipt(result: &str, command_status: &str) -> ProofReceipt {
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

    fn test_plan(sensors: Vec<SensorPlan>) -> Plan {
        Plan {
            base: "HEAD~1".to_owned(),
            head: "HEAD".to_owned(),
            profile_name: "gh-runner".to_owned(),
            diff_class: DiffClass::SourceUb,
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
        }
    }

    fn test_terminal_state(status: &str) -> ReviewTerminalState {
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
            inline_comments: 0,
            summary_only_findings: 0,
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
                runtime_profile: None,
            },
            dry_run: false,
            allow_heavy: false,
            no_github_summary: true,
            posting: PostingMode::ArtifactOnly,
            mode: RunMode::ReviewDirect,
            model_mode: ModelMode::Auto,
            selectors: SelectorArgs::default(),
            depth: ReviewDepth::Standard,
            max_inline_comments: 8,
            model_concurrency: STANDARD_MODEL_CONCURRENCY,
            max_model_calls: STANDARD_MAX_MODEL_CALLS,
            provider_policy: ModelProviderPolicy::MinimaxPrimary,
            lane_width: STANDARD_LANE_WIDTH,
            model_timeout_sec: 300,
            ledger_path: String::new(),
            ledger_max_bytes: 65_536,
            pr_thread_context: String::new(),
            pr_thread_context_max_bytes: 65_536,
            pr_thread_auth: None,
            github_repo: None,
            github_pull_number: None,
            github_api_url: "https://api.github.com".to_owned(),
            minimax_provider_kind: ProviderKindArg::Anthropic,
            minimax_model: "MiniMax-M3".to_owned(),
            opencode_model: "minimax-m3".to_owned(),
            opencode_endpoint_kind: OpenCodeEndpointKindArg::Auto,
            review_body_max_bytes: 60_000,
        }
    }

    fn spawn_fake_github_thread_api(
        expected_requests: usize,
    ) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || -> Result<Vec<String>> {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut requests = Vec::new();
            while requests.len() < expected_requests {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        requests.push(handle_fake_github_thread_request(stream)?);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            bail!(
                                "fake GitHub thread API received {} of {} requests",
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
                    "line": 12,
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
