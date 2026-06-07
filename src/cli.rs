use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::{STANDARD_LANE_WIDTH, STANDARD_MAX_MODEL_CALLS, STANDARD_MODEL_CONCURRENCY};

#[derive(Debug, Parser)]
#[command(name = "ub-review")]
#[command(version)]
#[command(about = "Build box-aware evidence packets for UB-focused PR review")]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
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
    /// Write a read-only CI right-sizing report under `<out>/ci-audit/`.
    AuditCi(AuditCiArgs),
    /// Render the CI migration PR contents from a prior audit-ci run.
    /// v0 is --print-pr only: no repo writes, no network, no GitHub calls.
    SetupCi(SetupCiArgs),
    /// Enforce a recorded gate outcome: exit non-zero when enforcement
    /// resolves on and review/gate_outcome.json records a `fail` conclusion.
    GateCheck(GateCheckArgs),
}

#[derive(Clone, Debug, ValueEnum)]
pub(crate) enum ProfileArg {
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
    pub(crate) fn key(&self) -> &'static str {
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
pub(crate) enum PostingMode {
    ArtifactOnly,
    Review,
}

impl PostingMode {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::ArtifactOnly => "artifact-only",
            Self::Review => "review",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ModelMode {
    Auto,
    Off,
}

impl ModelMode {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Off => "off",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ReviewDepth {
    Quick,
    Standard,
    Deep,
}

impl ReviewDepth {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Quick => "quick",
            Self::Standard => "standard",
            Self::Deep => "deep",
        }
    }

    pub(crate) fn lane_width(self) -> usize {
        match self {
            Self::Quick => 6,
            Self::Standard => STANDARD_LANE_WIDTH,
            Self::Deep => 20,
        }
    }

    pub(crate) fn model_concurrency(self) -> usize {
        match self {
            Self::Quick => 4,
            Self::Standard | Self::Deep => STANDARD_MODEL_CONCURRENCY,
        }
    }

    pub(crate) fn max_model_calls(self) -> usize {
        match self {
            Self::Quick => 6,
            Self::Standard => STANDARD_MAX_MODEL_CALLS,
            Self::Deep => 24,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ModelProviderPolicy {
    Auto,
    MinimaxPrimary,
    PrimaryWithFallback,
    MinimaxOnly,
    OpencodeGoCanary,
    OpencodeGoWide,
}

impl ModelProviderPolicy {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::MinimaxPrimary => "minimax-primary",
            Self::PrimaryWithFallback => "primary-with-fallback",
            Self::MinimaxOnly => "minimax-only",
            Self::OpencodeGoCanary => "opencode-go-canary",
            Self::OpencodeGoWide => "opencode-go-wide",
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub(crate) enum ProviderKindArg {
    Openai,
    Anthropic,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum OpenCodeEndpointKindArg {
    Auto,
    OpenaiChat,
    AnthropicMessages,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum RunMode {
    #[value(alias = "review-direct")]
    ReviewByok,
    IntelligentCi,
    AgentInvestigate,
    AgentPatch,
}

impl RunMode {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::ReviewByok => "review-byok",
            Self::IntelligentCi => "intelligent-ci",
            Self::AgentInvestigate => "agent-investigate",
            Self::AgentPatch => "agent-patch",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum FailOnGate {
    Auto,
    True,
    False,
}

impl FailOnGate {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::True => "true",
            Self::False => "false",
        }
    }

    pub(crate) fn resolved(self, mode: RunMode) -> bool {
        match self {
            Self::True => true,
            Self::False => false,
            Self::Auto => matches!(mode, RunMode::IntelligentCi),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RunPass {
    Auto,
    Opened,
    Reopened,
    ReadyForReview,
    Synchronize,
    PullRequestOther,
    Manual,
}

impl RunPass {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Opened => "opened",
            Self::Reopened => "reopened",
            Self::ReadyForReview => "ready_for_review",
            Self::Synchronize => "synchronize",
            Self::PullRequestOther => "pull_request_other",
            Self::Manual => "manual",
        }
    }

    /// The `pull_request` event action this pass corresponds to, matching the
    /// `[gate].post_review_on` vocabulary. Catch-all and non-PR passes have no
    /// single event action.
    pub(crate) fn event_action(self) -> Option<&'static str> {
        match self {
            Self::Opened => Some("opened"),
            Self::Reopened => Some("reopened"),
            Self::ReadyForReview => Some("ready_for_review"),
            Self::Synchronize => Some("synchronize"),
            Self::Auto | Self::PullRequestOther | Self::Manual => None,
        }
    }
}

pub(crate) fn parse_run_pass(value: &str) -> std::result::Result<RunPass, String> {
    match value.trim() {
        "auto" => Ok(RunPass::Auto),
        "opened" => Ok(RunPass::Opened),
        "reopened" => Ok(RunPass::Reopened),
        "ready_for_review" | "ready-for-review" => Ok(RunPass::ReadyForReview),
        "synchronize" => Ok(RunPass::Synchronize),
        "pull_request_other" | "pull-request-other" => Ok(RunPass::PullRequestOther),
        "manual" => Ok(RunPass::Manual),
        other => Err(format!(
            "unsupported run pass `{other}`; expected auto, opened, reopened, ready_for_review, synchronize, pull_request_other, or manual"
        )),
    }
}

#[derive(Clone, Debug, Args)]
pub(crate) struct ReviewArgs {
    /// Repository root.
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    pub(crate) root: PathBuf,
    /// Base ref.
    #[arg(long, default_value = "origin/main", env = "UB_REVIEW_BASE")]
    pub(crate) base: String,
    /// Head ref.
    #[arg(long, default_value = "HEAD", env = "UB_REVIEW_HEAD")]
    pub(crate) head: String,
    /// Config path.
    #[arg(long, default_value = ".ub-review.toml", env = "UB_REVIEW_CONFIG")]
    pub(crate) config: PathBuf,
    /// Output run directory.
    #[arg(long, default_value = "target/ub-review", env = "UB_REVIEW_OUT")]
    pub(crate) out: PathBuf,
    /// Box profile override.
    #[arg(long, value_enum, env = "UB_REVIEW_PROFILE")]
    pub(crate) profile: Option<ProfileArg>,
    /// Runtime profile override. Prefer this over --profile for box budgets.
    #[arg(
        long = "runtime-profile",
        value_enum,
        env = "UB_REVIEW_RUNTIME_PROFILE"
    )]
    pub(crate) runtime_profile: Option<ProfileArg>,
}

#[derive(Debug, Args)]
pub(crate) struct InitArgs {
    /// Config file path to write.
    #[arg(long, default_value = ".ub-review.toml")]
    pub(crate) path: PathBuf,
    /// Profile to write into the config.
    #[arg(long, value_enum, default_value = "gh-runner")]
    pub(crate) profile: ProfileArg,
    /// Overwrite existing config.
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct DoctorArgs {
    #[arg(long, default_value = ".ub-review.toml", env = "UB_REVIEW_CONFIG")]
    pub(crate) config: PathBuf,
    #[arg(long, value_enum, env = "UB_REVIEW_PROFILE")]
    pub(crate) profile: Option<ProfileArg>,
    #[arg(
        long = "runtime-profile",
        value_enum,
        env = "UB_REVIEW_RUNTIME_PROFILE"
    )]
    pub(crate) runtime_profile: Option<ProfileArg>,
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    pub(crate) root: PathBuf,
    #[arg(long, env = "UB_REVIEW_BASE")]
    pub(crate) base: Option<String>,
    #[arg(long, env = "UB_REVIEW_CACHE_DIR")]
    pub(crate) cache_dir: Option<PathBuf>,
    #[arg(long, env = "UB_REVIEW_REQUIRE_CORE_TOOLS")]
    pub(crate) require_core_tools: bool,
}

#[derive(Debug, Args)]
pub(crate) struct CacheArgs {
    #[command(subcommand)]
    pub(crate) command: CacheCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CacheCommand {
    /// Create the cache directory skeleton and base-tree manifest.
    Warm(CacheWarmArgs),
}

#[derive(Debug, Args)]
pub(crate) struct CacheWarmArgs {
    #[arg(long, default_value = ".ub-review.toml", env = "UB_REVIEW_CONFIG")]
    pub(crate) config: PathBuf,
    #[arg(long, value_enum, env = "UB_REVIEW_PROFILE")]
    pub(crate) profile: Option<ProfileArg>,
    #[arg(
        long = "runtime-profile",
        value_enum,
        env = "UB_REVIEW_RUNTIME_PROFILE"
    )]
    pub(crate) runtime_profile: Option<ProfileArg>,
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    pub(crate) root: PathBuf,
    #[arg(long, default_value = "origin/main", env = "UB_REVIEW_BASE")]
    pub(crate) base: String,
    #[arg(long = "out", env = "UB_REVIEW_CACHE_DIR")]
    pub(crate) cache_dir: Option<PathBuf>,
}

#[derive(Debug, Args)]
pub(crate) struct PlanArgs {
    #[command(flatten)]
    pub(crate) review: ReviewArgs,
    #[command(flatten)]
    pub(crate) selectors: SelectorArgs,
    /// Write plan artifacts under the run directory.
    #[arg(long)]
    pub(crate) write: bool,
    /// Allow heavy/manual witnesses in the plan.
    #[arg(long)]
    pub(crate) allow_heavy: bool,
}

#[derive(Debug, Args)]
pub(crate) struct RunArgs {
    #[command(flatten)]
    pub(crate) review: ReviewArgs,
    /// Do not execute external sensors.
    #[arg(long)]
    pub(crate) dry_run: bool,
    /// Allow heavy/manual witnesses.
    #[arg(long)]
    pub(crate) allow_heavy: bool,
    /// Do not append running-summary.md to $GITHUB_STEP_SUMMARY.
    #[arg(long)]
    pub(crate) no_github_summary: bool,
    /// Review posting intent. `run` only prepares artifacts; `post` submits.
    #[arg(
        long,
        value_enum,
        default_value = "artifact-only",
        env = "UB_REVIEW_POSTING"
    )]
    pub(crate) posting: PostingMode,
    /// Review execution mode. Default uses BYOK model lanes.
    #[arg(
        long,
        value_enum,
        default_value = "review-byok",
        env = "UB_REVIEW_MODE"
    )]
    pub(crate) mode: RunMode,
    /// Review pass identity. `auto` maps pull_request opened/ready_for_review to the matching pass.
    #[arg(long = "run-pass", value_parser = parse_run_pass, default_value = "auto", env = "UB_REVIEW_RUN_PASS")]
    pub(crate) run_pass: RunPass,
    /// Model execution mode.
    #[arg(long, value_enum, default_value = "auto", env = "UB_REVIEW_MODEL_MODE")]
    pub(crate) model_mode: ModelMode,
    /// Gate enforcement. When this resolves to true and review/gate_outcome.json
    /// records a `fail` conclusion, `run` exits non-zero (exit code 1) after all
    /// artifacts are written. `auto` resolves to true for --mode intelligent-ci
    /// and false otherwise.
    #[arg(
        long = "fail-on-gate",
        value_enum,
        default_value = "auto",
        env = "UB_REVIEW_FAIL_ON_GATE"
    )]
    pub(crate) fail_on_gate: FailOnGate,
    #[command(flatten)]
    pub(crate) selectors: SelectorArgs,
    /// Review depth selector. Nonstandard depths expand to lane/model budgets.
    #[arg(long, value_enum, default_value = "standard", env = "UB_REVIEW_DEPTH")]
    pub(crate) depth: ReviewDepth,
    /// Maximum inline comments to include in github-review.json.
    #[arg(long, default_value_t = 8, env = "UB_REVIEW_MAX_INLINE_COMMENTS")]
    pub(crate) max_inline_comments: usize,
    /// Planned model concurrency for model lane packets.
    #[arg(
        long,
        default_value_t = STANDARD_MODEL_CONCURRENCY,
        env = "UB_REVIEW_MODEL_CONCURRENCY"
    )]
    pub(crate) model_concurrency: usize,
    /// Maximum planned model calls.
    #[arg(
        long,
        default_value_t = STANDARD_MAX_MODEL_CALLS,
        env = "UB_REVIEW_MAX_MODEL_CALLS"
    )]
    pub(crate) max_model_calls: usize,
    /// Provider policy. `auto` (the default) defers to `[providers].policy`
    /// in the repo config when set, else behaves as `minimax-primary`; an
    /// explicit flag or env value overrides config (D2: config wins, CLI
    /// overrides).
    #[arg(
        long = "provider-policy",
        alias = "model-provider-policy",
        value_enum,
        default_value = "auto",
        env = "UB_REVIEW_PROVIDER_POLICY"
    )]
    pub(crate) provider_policy: ModelProviderPolicy,
    /// Number of Bun review lanes to prepare: 6, 10, or 20.
    #[arg(long, default_value_t = STANDARD_LANE_WIDTH, env = "UB_REVIEW_LANE_WIDTH")]
    pub(crate) lane_width: usize,
    /// Per-model-call timeout in seconds.
    #[arg(long, default_value_t = 300, env = "UB_REVIEW_MODEL_TIMEOUT_SEC")]
    pub(crate) model_timeout_sec: u64,
    /// Optional read-only UB ledger path.
    #[arg(long, default_value = "", env = "UB_REVIEW_LEDGER_PATH")]
    pub(crate) ledger_path: String,
    /// Maximum bytes of UB ledger context.
    #[arg(long, default_value_t = 65_536, env = "UB_REVIEW_LEDGER_MAX_BYTES")]
    pub(crate) ledger_max_bytes: usize,
    /// Optional PR thread context file with prior replies, receipts, or resolved comments.
    #[arg(long, default_value = "", env = "UB_REVIEW_PR_THREAD_CONTEXT")]
    pub(crate) pr_thread_context: String,
    /// Maximum bytes of PR thread context to seed into shared_context.md.
    #[arg(
        long,
        default_value_t = 65_536,
        env = "UB_REVIEW_PR_THREAD_CONTEXT_MAX_BYTES"
    )]
    pub(crate) pr_thread_context_max_bytes: usize,
    /// GitHub credential used only to fetch bounded PR-thread context during `run`.
    #[arg(long = "github-token", env = "UB_REVIEW_PR_THREAD_AUTH")]
    pub(crate) pr_thread_auth: Option<String>,
    /// owner/repo used to fetch bounded PR-thread context. Defaults to GITHUB_REPOSITORY.
    #[arg(long = "github-repo", env = "GITHUB_REPOSITORY")]
    pub(crate) github_repo: Option<String>,
    /// Pull request number used to fetch bounded PR-thread context.
    #[arg(long = "github-pull-number", env = "UB_REVIEW_PULL_NUMBER")]
    pub(crate) github_pull_number: Option<u64>,
    /// GitHub API base URL used to fetch bounded PR-thread context.
    #[arg(
        long = "github-api-url",
        default_value = "https://api.github.com",
        env = "UB_REVIEW_GITHUB_API_URL"
    )]
    pub(crate) github_api_url: String,
    /// MiniMax provider request/response family.
    #[arg(
        long,
        value_enum,
        default_value = "anthropic",
        env = "UB_REVIEW_MINIMAX_PROVIDER_KIND"
    )]
    pub(crate) minimax_provider_kind: ProviderKindArg,
    /// MiniMax model name.
    #[arg(long, default_value = "MiniMax-M3", env = "UB_REVIEW_MINIMAX_MODEL")]
    pub(crate) minimax_model: String,
    /// OpenCode Go model name for canary lanes.
    #[arg(long, default_value = "minimax-m3", env = "UB_REVIEW_OPENCODE_MODEL")]
    pub(crate) opencode_model: String,
    /// OpenCode Go endpoint family.
    #[arg(
        long,
        value_enum,
        default_value = "auto",
        env = "UB_REVIEW_OPENCODE_ENDPOINT_KIND"
    )]
    pub(crate) opencode_endpoint_kind: OpenCodeEndpointKindArg,
    /// Maximum bytes in the GitHub review body.
    #[arg(
        long,
        default_value_t = 60_000,
        env = "UB_REVIEW_REVIEW_BODY_MAX_BYTES"
    )]
    pub(crate) review_body_max_bytes: usize,
}

#[derive(Clone, Debug, Default, Args)]
pub(crate) struct SelectorArgs {
    /// Comma-separated lane IDs to run. Empty means the profile default.
    #[arg(long, default_value = "", env = "UB_REVIEW_LANES")]
    pub(crate) lanes: String,
    /// Comma-separated lane IDs to skip after applying --lanes.
    #[arg(
        long = "except-lanes",
        default_value = "",
        env = "UB_REVIEW_EXCEPT_LANES"
    )]
    pub(crate) except_lanes: String,
    /// Comma-separated sensor/tool IDs to plan. Empty means the profile default.
    #[arg(long, default_value = "", env = "UB_REVIEW_TOOLS")]
    pub(crate) tools: String,
    /// Comma-separated sensor/tool IDs to skip after applying --tools.
    #[arg(
        long = "except-tools",
        default_value = "",
        env = "UB_REVIEW_EXCEPT_TOOLS"
    )]
    pub(crate) except_tools: String,
}

#[derive(Debug, Args)]
pub(crate) struct SummaryArgs {
    #[arg(long, default_value = "target/ub-review")]
    pub(crate) run_dir: PathBuf,
}

#[derive(Debug, Args)]
pub(crate) struct GateCheckArgs {
    /// Gate outcome artifact written by `ub-review run`.
    #[arg(
        long = "gate-outcome",
        default_value = "target/ub-review/review/gate_outcome.json",
        env = "UB_REVIEW_GATE_OUTCOME_PATH"
    )]
    pub(crate) gate_outcome: PathBuf,
    /// Gate enforcement. `auto` resolves to true only for --mode intelligent-ci.
    #[arg(
        long = "fail-on-gate",
        value_enum,
        default_value = "auto",
        env = "UB_REVIEW_FAIL_ON_GATE"
    )]
    pub(crate) fail_on_gate: FailOnGate,
    /// Run mode used to resolve `auto` enforcement.
    #[arg(
        long,
        value_enum,
        default_value = "review-byok",
        env = "UB_REVIEW_MODE"
    )]
    pub(crate) mode: RunMode,
}

#[derive(Debug, Args)]
pub(crate) struct PostArgs {
    /// Prepared GitHub review payload.
    #[arg(long, default_value = "target/ub-review/review/github-review.json")]
    pub(crate) review_json: PathBuf,
    /// Diff patch used to validate RIGHT-side inline comment lines.
    #[arg(long, env = "UB_REVIEW_DIFF_PATCH")]
    pub(crate) diff_patch: Option<PathBuf>,
    /// Directory for post-result.json or post-error.json.
    #[arg(long, default_value = "target/ub-review/review")]
    pub(crate) out: PathBuf,
    /// GitHub token with pull-request write permission.
    #[arg(long, env = "UB_REVIEW_GITHUB_TOKEN")]
    pub(crate) github_token: Option<String>,
    /// owner/repo. Defaults to GITHUB_REPOSITORY.
    #[arg(long, env = "GITHUB_REPOSITORY")]
    pub(crate) repo: Option<String>,
    /// Pull request number. Defaults to GITHUB_EVENT_PATH pull_request.number.
    #[arg(long, env = "UB_REVIEW_PULL_NUMBER")]
    pub(crate) pull_number: Option<u64>,
    /// GitHub API base URL.
    #[arg(
        long,
        default_value = "https://api.github.com",
        env = "UB_REVIEW_GITHUB_API_URL"
    )]
    pub(crate) github_api_url: String,
    /// Return a failing exit code when posting fails.
    #[arg(long)]
    pub(crate) fail_on_post_error: bool,
}

#[derive(Debug, Args)]
pub(crate) struct AuditCiArgs {
    /// Repository root containing .github/workflows.
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    pub(crate) root: PathBuf,
    /// Output run directory. Artifacts land under `<out>/ci-audit/`.
    #[arg(long, default_value = "target/ub-review", env = "UB_REVIEW_OUT")]
    pub(crate) out: PathBuf,
    /// owner/repo. Defaults to GITHUB_REPOSITORY, else the git origin remote.
    #[arg(long, env = "GITHUB_REPOSITORY")]
    pub(crate) repo: Option<String>,
    /// GitHub token for read-only API calls. Tokenless runs degrade to inventory-only.
    // Ambient GITHUB_TOKEN (not UB_REVIEW_GITHUB_TOKEN) is intentional here:
    // audit-ci is the adoption wedge and must work with the token a runner or
    // `gh` shell already exports, with zero ub-review-specific setup.
    #[arg(long = "github-token", env = "GITHUB_TOKEN")]
    pub(crate) github_token: Option<String>,
    /// GitHub API base URL.
    #[arg(
        long = "github-api-url",
        default_value = "https://api.github.com",
        env = "UB_REVIEW_GITHUB_API_URL"
    )]
    pub(crate) github_api_url: String,
    /// History window in days.
    #[arg(long = "window-days", default_value_t = 90)]
    pub(crate) window_days: u32,
}

#[derive(Debug, Args)]
pub(crate) struct SetupCiArgs {
    /// Run directory containing a prior audit-ci run's `ci-audit/` receipts.
    #[arg(long, default_value = "target/ub-review", env = "UB_REVIEW_OUT")]
    pub(crate) out: PathBuf,
    /// Render the full migration PR contents (file blocks + PR body) to
    /// stdout and `<out>/ci-audit/migration-plan.md`. Required in v0: PR
    /// opening is a later slice and bare `setup-ci` says so instead of
    /// guessing.
    #[arg(long = "print-pr")]
    pub(crate) print_pr: bool,
    /// Accept an audited job into the generated gate policy, as
    /// `<job>=<command>`. Repeatable. The audit receipts record triggers and
    /// timings, never the runnable command, so the maintainer supplies it -
    /// the generator must not invent one. Only `adaptive`-tier
    /// recommendations are acceptable.
    #[arg(long = "accept")]
    pub(crate) accept: Vec<String>,
    /// Existing repo config, consulted only for [gate].required_check.
    #[arg(long, default_value = ".ub-review.toml", env = "UB_REVIEW_CONFIG")]
    pub(crate) config: PathBuf,
}
