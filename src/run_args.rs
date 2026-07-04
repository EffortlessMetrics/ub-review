//! Run-args normalization, validation, and runtime profile resolution
//! (cleanup train step 57, pure code motion).

use crate::*;

pub(crate) fn normalize_run_args(mut args: RunArgs) -> Result<RunArgs> {
    apply_depth_defaults(&mut args)?;
    validate_run_args(&args)?;
    Ok(args)
}

/// Apply the user-facing `review-mode` preset (advisory / gate / strict) when
/// set. The preset overrides the legacy `--mode`, `--fail-on-gate`, and
/// `[gate].review_forward` knobs, emitting one warning per overridden knob so
/// contradictory intent is never silently merged. When the preset is unset
/// (the default), nothing changes — the legacy knobs flow through unchanged.
/// See ADOPTION_MODES.md and #719.
pub(crate) fn apply_review_mode_preset(
    args: &mut RunArgs,
    config: &mut Config,
) -> Option<ResolvedReviewMode> {
    let preset = args.review_mode?;
    let resolved = preset.resolve();
    let preset_name = preset.key();
    if args.mode != resolved.mode {
        eprintln!(
            "warning: --review-mode {preset_name} overrides --mode {} -> {}",
            args.mode.key(),
            resolved.mode.key()
        );
        args.mode = resolved.mode;
    }
    if args.fail_on_gate != resolved.fail_on_gate {
        eprintln!(
            "warning: --review-mode {preset_name} overrides --fail-on-gate {} -> {}",
            args.fail_on_gate.key(),
            resolved.fail_on_gate.key()
        );
        args.fail_on_gate = resolved.fail_on_gate;
    }
    if config.gate.review_forward != resolved.review_forward {
        eprintln!(
            "warning: --review-mode {preset_name} overrides [gate].review_forward {} -> {}",
            config.gate.review_forward, resolved.review_forward
        );
        config.gate.review_forward = resolved.review_forward;
    }
    Some(resolved)
}

pub(crate) fn apply_depth_defaults(args: &mut RunArgs) -> Result<()> {
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

pub(crate) fn ensure_supported_mode(mode: RunMode) -> Result<()> {
    match mode {
        RunMode::ReviewByok | RunMode::IntelligentCi => Ok(()),
        RunMode::AgentInvestigate | RunMode::AgentPatch => bail!(
            "{} is reserved for optional leased workers and is not implemented in v0",
            mode.key()
        ),
    }
}

pub(crate) fn resolved_run_pass(run_pass: RunPass) -> RunPass {
    if run_pass != RunPass::Auto {
        return run_pass;
    }
    resolve_run_pass_from_event(
        std::env::var("GITHUB_EVENT_NAME").ok().as_deref(),
        github_event_action().as_deref(),
    )
}

pub(crate) fn resolve_run_pass_from_event(
    event_name: Option<&str>,
    event_action: Option<&str>,
) -> RunPass {
    match event_name {
        Some("pull_request" | "pull_request_target") => match event_action {
            Some("opened") => RunPass::Opened,
            Some("reopened") => RunPass::Reopened,
            Some("ready_for_review") => RunPass::ReadyForReview,
            Some("synchronize") => RunPass::Synchronize,
            _ => RunPass::PullRequestOther,
        },
        _ => RunPass::Manual,
    }
}

pub(crate) fn github_event_action() -> Option<String> {
    for name in ["UB_REVIEW_GITHUB_EVENT_ACTION", "GITHUB_EVENT_ACTION"] {
        if let Ok(value) = std::env::var(name) {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    github_event_action_from_path()
}

pub(crate) fn github_event_action_from_path() -> Option<String> {
    let path = std::env::var_os("GITHUB_EVENT_PATH")?;
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("action")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

pub(crate) fn validate_run_args(args: &RunArgs) -> Result<()> {
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

pub(crate) fn apply_runtime_profile_limits(args: &mut RunArgs, profile: &Profile) -> Result<()> {
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
