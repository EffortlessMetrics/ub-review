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
    /// One-command adoption: write a safe GitHub Actions workflow + minimal
    /// `.ub-review.toml` for the chosen review posture. Prints the exact
    /// secret to add. The Droid/Factory-style first-run path.
    Enable(EnableArgs),
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
    /// Render or open the CI migration PR from a prior audit-ci run.
    /// `--print-pr` is local-only; `--open-pr` creates the migration branch and PR.
    SetupCi(SetupCiArgs),
    /// Aggregate prior gate artifacts and GitHub reviewer-state receipts into
    /// rolling quality telemetry. Artifact-only; never posts or edits GitHub.
    QualityBackfill(QualityBackfillArgs),
    /// Normalize raw GitHub review-thread receipts for quality-backfill.
    /// Artifact-only; never posts or edits GitHub.
    QualityGithubOutcomes(QualityGithubOutcomesArgs),
    /// Collect raw GitHub review-thread receipts for quality-backfill.
    /// Artifact-only; never posts or edits GitHub.
    QualityGithubCollect(QualityGithubCollectArgs),
    /// Enforce a recorded gate outcome: exit non-zero when enforcement
    /// resolves on and review/gate_outcome.json records a `fail` conclusion.
    GateCheck(GateCheckArgs),
    /// Execute a single proof request and write its receipt. Designed for
    /// distributed execution: a `plan` job emits proof requests, `worker`
    /// jobs execute them (locally or remotely), and a `finalize` job
    /// collects receipts and produces the gate verdict.
    /// (Order 8 of epic #655.)
    Worker(WorkerArgs),
}

/// Arguments for the `worker` subcommand (Order 8 of epic #655).
#[derive(Clone, Debug, clap::Args)]
pub(crate) struct WorkerArgs {
    /// Path to the proof request JSON file to execute.
    #[arg(long)]
    pub(crate) proof_request: String,
    /// Output directory for the receipt.
    #[arg(long)]
    pub(crate) out: String,
    /// Repository root for command execution.
    #[arg(long, default_value = ".")]
    pub(crate) root: String,
    /// Timeout in seconds for the proof command.
    #[arg(long, default_value_t = 300)]
    pub(crate) timeout_sec: u64,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MinimaxPromptCache {
    ExplicitAnthropic,
    Off,
}

impl MinimaxPromptCache {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::ExplicitAnthropic => "explicit-anthropic",
            Self::Off => "off",
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
    #[value(hide = true)]
    AgentInvestigate,
    #[value(hide = true)]
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

/// User-facing review posture (the simple-mode UX). Each preset resolves to
/// the existing `{mode, fail-on-gate, review_forward}` triple so normal users
/// never have to compose those internal knobs. The legacy knobs remain as
/// backwards-compatible escape hatches; when a preset is set it wins (with a
/// per-knob warning). See ADOPTION_MODES.md and #719.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub(crate) enum ReviewModePreset {
    Advisory,
    Gate,
    Strict,
}

impl ReviewModePreset {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Advisory => "advisory",
            Self::Gate => "gate",
            Self::Strict => "strict",
        }
    }

    /// The full resolution table — explicit and boring.
    ///
    /// | preset    | mode            | fail-on-gate | review_forward |
    /// |-----------|-----------------|--------------|----------------|
    /// | advisory  | review-byok     | false        | false          |
    /// | gate      | intelligent-ci  | true         | false          |
    /// | strict    | intelligent-ci  | true         | true           |
    pub(crate) fn resolve(self) -> ResolvedReviewMode {
        match self {
            Self::Advisory => ResolvedReviewMode {
                mode: RunMode::ReviewByok,
                fail_on_gate: FailOnGate::False,
                review_forward: false,
            },
            Self::Gate => ResolvedReviewMode {
                mode: RunMode::IntelligentCi,
                fail_on_gate: FailOnGate::True,
                review_forward: false,
            },
            Self::Strict => ResolvedReviewMode {
                mode: RunMode::IntelligentCi,
                fail_on_gate: FailOnGate::True,
                review_forward: true,
            },
        }
    }
}

/// The concrete `{mode, fail-on-gate, review_forward}` triple a preset (or the
/// legacy knobs) resolves to. One object, not scattered conditionals (#719).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedReviewMode {
    pub(crate) mode: RunMode,
    pub(crate) fail_on_gate: FailOnGate,
    pub(crate) review_forward: bool,
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
    /// Repository root to inspect for the file-driven init guide.
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    pub(crate) root: PathBuf,
    /// Markdown handoff guide for agents or maintainers finishing setup.
    #[arg(long = "guide-out", default_value = "ub-review-init.md")]
    pub(crate) guide_out: PathBuf,
    /// Write only the starter config, without the file-driven setup guide.
    #[arg(long = "no-guide")]
    pub(crate) no_guide: bool,
    /// Profile to write into the config.
    #[arg(long, value_enum, default_value = "gh-runner")]
    pub(crate) profile: ProfileArg,
    /// Overwrite existing config.
    #[arg(long)]
    pub(crate) force: bool,
}

#[derive(Debug, Args)]
pub(crate) struct EnableArgs {
    /// Review posture: advisory (comment only), gate (recommended required
    /// check), or strict (+ reporter verdict can block). See #720.
    #[arg(long, value_enum, env = "UB_REVIEW_REVIEW_MODE")]
    pub(crate) mode: ReviewModePreset,
    /// Model backend. Only `minimax` is supported in v0.
    #[arg(long, default_value = "minimax")]
    pub(crate) model: String,
    /// Full 40-hex ub-review commit SHA to pin in the generated workflow.
    /// Required: the generator refuses to invent a pin (matches the
    /// `setup-ci` SHA-pin safety contract).
    #[arg(long, env = "UB_REVIEW_ACTION_SHA")]
    pub(crate) action_sha: String,
    /// Repository root to write into.
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    pub(crate) root: PathBuf,
    /// Overwrite an existing `.ub-review.toml` or ub-review workflow.
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
    /// User-facing review posture. When set, overrides `--mode`,
    /// `--fail-on-gate`, and `[gate].review_forward` (with a per-knob
    /// warning). `advisory` = comment only; `gate` = deterministic-floor
    /// required check (recommended); `strict` = + reporter verdict can block.
    /// Unset (the default) uses the legacy knobs unchanged.
    #[arg(long = "review-mode", value_enum, env = "UB_REVIEW_REVIEW_MODE")]
    pub(crate) review_mode: Option<ReviewModePreset>,
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
    /// Resolved from [providers.minimax].prompt_cache. Hidden from CLI so the
    /// repository config remains the source of truth for provider cache mode.
    #[arg(skip = MinimaxPromptCache::ExplicitAnthropic)]
    pub(crate) minimax_prompt_cache: MinimaxPromptCache,
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
    /// Optional prior review/resolved_candidates.json receipt from an earlier pass.
    #[arg(
        long = "prior-resolved-candidates",
        default_value = "",
        env = "UB_REVIEW_PRIOR_RESOLVED_CANDIDATES"
    )]
    pub(crate) prior_resolved_candidates: String,
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
    /// User-facing review posture. When set, overrides `--fail-on-gate` and
    /// `--mode` here, and `[gate].review_forward` at run time (with a
    /// per-knob warning). See `RunArgs::review_mode`.
    #[arg(long = "review-mode", value_enum, env = "UB_REVIEW_REVIEW_MODE")]
    pub(crate) review_mode: Option<ReviewModePreset>,
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
    /// Count of matching audit-log cancellation events from an external
    /// read-only audit-log check. When supplied, audit-ci can separate
    /// user/API cancellation from suspected runner eviction without querying
    /// audit logs itself.
    #[arg(long = "audit-cancel-events")]
    pub(crate) audit_cancel_events: Option<usize>,
}

#[derive(Debug, Args)]
pub(crate) struct SetupCiArgs {
    /// Run directory containing a prior audit-ci run's `ci-audit/` receipts.
    #[arg(long, default_value = "target/ub-review", env = "UB_REVIEW_OUT")]
    pub(crate) out: PathBuf,
    /// Render the migration PR body to stdout and
    /// `<out>/ci-audit/migration-plan.md` without repo writes, network calls,
    /// GitHub calls, or branch-protection changes. With accepted jobs plus
    /// --action-sha, also writes no-network preview files under
    /// `<out>/ci-audit/preview/`.
    #[arg(long = "print-pr")]
    pub(crate) print_pr: bool,
    /// Accept an audited job into the generated gate policy, as
    /// `<job>=<command>`. Repeatable. The audit receipts record triggers and
    /// timings, never the runnable command, so the maintainer supplies it -
    /// the generator must not invent one. Only `adaptive` and
    /// `move-to-ub-review-required` recommendations are acceptable.
    #[arg(long = "accept")]
    pub(crate) accept: Vec<String>,
    /// Existing repo config, consulted only for \[gate].required_check.
    #[arg(long, default_value = ".ub-review.toml", env = "UB_REVIEW_CONFIG")]
    pub(crate) config: PathBuf,
    /// Open the migration PR on GitHub: create a branch, add the generated
    /// files (.ub-review.toml, the gate workflow, docs/ci/ub-review-migration.md,
    /// docs/ci/branch-protection-change.md), and open one PR whose body is
    /// the migration plan. Never touches branch protection. Requires
    /// --action-sha plus a token, and refuses to edit a repo that already has
    /// a .ub-review.toml.
    #[arg(long = "open-pr")]
    pub(crate) open_pr: bool,
    /// Target owner/repo for --open-pr.
    #[arg(long, env = "GITHUB_REPOSITORY")]
    pub(crate) repo: Option<String>,
    /// GitHub token for --open-pr (ambient GITHUB_TOKEN, matching audit-ci's
    /// zero-setup posture).
    #[arg(long = "github-token", env = "GITHUB_TOKEN")]
    pub(crate) github_token: Option<String>,
    /// GitHub API base URL.
    #[arg(
        long = "github-api-url",
        default_value = "https://api.github.com",
        env = "UB_REVIEW_GITHUB_API_URL"
    )]
    pub(crate) github_api_url: String,
    /// Full 40-hex commit SHA of EffortlessMetrics/ub-review to pin in the
    /// generated gate workflow. Required for --open-pr and for print-pr
    /// preview files: the generator refuses to invent a pin.
    #[arg(long = "action-sha")]
    pub(crate) action_sha: Option<String>,
    /// Branch name the migration PR is opened from.
    #[arg(long, default_value = "ub-review/setup-ci-migration")]
    pub(crate) branch: String,
}

#[derive(Debug, Args)]
pub(crate) struct QualityBackfillArgs {
    /// Output run directory. The artifact lands at
    /// `<out>/review/quality-backfill.json`.
    #[arg(long, default_value = "target/ub-review", env = "UB_REVIEW_OUT")]
    pub(crate) out: PathBuf,
    /// Extracted ub-review gate artifact root containing
    /// `review/quality-receipt.json`. Repeat for each run in the window.
    #[arg(long = "run-dir")]
    pub(crate) run_dirs: Vec<PathBuf>,
    /// Normalized GitHub reviewer-state receipt. The receipt may name raw API
    /// query receipts in `source_artifacts`; they are copied into the output
    /// tree so the backfill artifact is self-contained.
    #[arg(long = "github-outcomes", env = "UB_REVIEW_GITHUB_QUALITY_OUTCOMES")]
    pub(crate) github_outcomes: Option<PathBuf>,
    /// Previous `quality-backfill.json` to compute deltas against.
    #[arg(long = "previous", env = "UB_REVIEW_PREVIOUS_QUALITY_BACKFILL")]
    pub(crate) previous: Option<PathBuf>,
    /// Rolling window in days.
    #[arg(long = "window-days", default_value_t = 30)]
    pub(crate) window_days: u32,
}

#[derive(Debug, Args)]
pub(crate) struct QualityGithubOutcomesArgs {
    /// Directory containing raw GitHub API receipts such as `pr-state.json` and
    /// `review-threads-<number>.json`.
    #[arg(
        long = "source-dir",
        default_value = "target/ub-review-quality/source/github"
    )]
    pub(crate) source_dir: PathBuf,
    /// Output normalized github_quality_outcomes receipt.
    #[arg(
        long,
        default_value = "target/ub-review-quality/source/github/github-quality-outcomes.json"
    )]
    pub(crate) out: PathBuf,
    /// Review-thread author login to treat as ub-review output. Repeatable.
    /// Defaults to the GitHub Actions bot logins used by the gate workflow.
    #[arg(long = "author-login")]
    pub(crate) author_logins: Vec<String>,
}

#[derive(Debug, Args)]
pub(crate) struct QualityGithubCollectArgs {
    /// Repository root used to derive owner/repo from git remote when --repo is
    /// omitted.
    #[arg(long, default_value = ".", env = "UB_REVIEW_ROOT")]
    pub(crate) root: PathBuf,
    /// Directory where raw GitHub API receipts are written for
    /// `quality-github-outcomes`.
    #[arg(
        long = "source-dir",
        default_value = "target/ub-review-quality/source/github"
    )]
    pub(crate) source_dir: PathBuf,
    /// owner/repo. Defaults to GITHUB_REPOSITORY, else the git origin remote.
    #[arg(long, env = "GITHUB_REPOSITORY")]
    pub(crate) repo: Option<String>,
    /// Pull request number to collect. Repeatable.
    #[arg(long = "pull-number")]
    pub(crate) pull_numbers: Vec<u64>,
    /// File containing pull request numbers, one per line. Blank lines and
    /// `#` comments are ignored.
    #[arg(long = "pull-numbers-file")]
    pub(crate) pull_numbers_file: Option<PathBuf>,
    /// GitHub token for read-only API calls.
    #[arg(long = "github-token", env = "GITHUB_TOKEN")]
    pub(crate) github_token: Option<String>,
    /// GitHub REST API base URL, used to derive the GraphQL URL when
    /// --github-graphql-url is omitted.
    #[arg(
        long = "github-api-url",
        default_value = "https://api.github.com",
        env = "UB_REVIEW_GITHUB_API_URL"
    )]
    pub(crate) github_api_url: String,
    /// GitHub GraphQL API URL. Defaults to `<github-api-url>/graphql`, with
    /// GitHub Enterprise /api/v3 mapped to /api/graphql.
    #[arg(long = "github-graphql-url", env = "UB_REVIEW_GITHUB_GRAPHQL_URL")]
    pub(crate) github_graphql_url: Option<String>,
    /// Per-request timeout in seconds.
    #[arg(long = "timeout-sec", default_value_t = 60)]
    pub(crate) timeout_sec: u64,
}

#[cfg(test)]
mod tests {
    use super::MinimaxPromptCache;

    #[test]
    fn minimax_prompt_cache_keys_match_config_vocabulary() {
        assert_eq!(
            MinimaxPromptCache::ExplicitAnthropic.key(),
            "explicit-anthropic"
        );
        assert_eq!(MinimaxPromptCache::Off.key(), "off");
    }
}
