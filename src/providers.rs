//! Model provider routing: MiniMax primary, OpenCode fallback, the D2
//! policy resolution, preflight selection, and the bounded runtime
//! fallback retry (cleanup train step 10, pure code motion).

use anyhow::Result;

use crate::*;

#[derive(Clone, Debug)]
pub(crate) struct ModelAssignment {
    pub(crate) lane: LanePlan,
    pub(crate) spec: ProviderSpec,
    pub(crate) fallback: Option<ProviderSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ModelProvider {
    MiniMaxDirect,
    OpenCodeGo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProviderEndpointKind {
    OpenAiChat,
    AnthropicMessages,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProviderSpec {
    pub(crate) provider: ModelProvider,
    pub(crate) model: String,
    pub(crate) endpoint_kind: ProviderEndpointKind,
}

pub(crate) fn model_assignments(plan: &Plan, args: &RunArgs) -> Result<Vec<ModelAssignment>> {
    model_assignments_with_key_state(plan, args, model_api_key_present(ModelProvider::OpenCodeGo))
}

pub(crate) fn model_assignments_with_key_state(
    plan: &Plan,
    args: &RunArgs,
    opencode_key_present: bool,
) -> Result<Vec<ModelAssignment>> {
    let lanes = selected_review_lanes_for_args(plan, args)?;
    let assignments = lanes
        .into_iter()
        .map(|lane| {
            let spec = provider_spec_for_lane(&lane, args);
            let fallback =
                fallback_provider_spec_for_lane(&lane, &spec, args, opencode_key_present);
            ModelAssignment {
                lane,
                spec,
                fallback,
            }
        })
        .collect::<Vec<_>>();
    Ok(assignments)
}

pub(crate) fn provider_spec_for_lane(lane: &LanePlan, args: &RunArgs) -> ProviderSpec {
    provider_spec_for_lane_with_key_state(
        lane,
        args,
        model_api_key_present(ModelProvider::OpenCodeGo),
    )
}

/// D2 precedence for the provider policy (spec 0006, decision D2: config
/// wins, CLI overrides). An explicit CLI/env value (anything but `auto`)
/// wins outright; `auto` defers to a configured `[providers].policy`
/// (already validated at load - an invalid value was stripped with a
/// `PolicyError` receipt and reads as unset here); with neither, `auto`
/// keeps its built-in minimax-primary semantics in the dispatch functions.
pub(crate) fn resolved_provider_policy(
    config: &Config,
    cli_policy: ModelProviderPolicy,
) -> ModelProviderPolicy {
    if cli_policy != ModelProviderPolicy::Auto {
        return cli_policy;
    }
    let configured = config.providers.policy.trim();
    if configured.is_empty() {
        return ModelProviderPolicy::Auto;
    }
    <ModelProviderPolicy as clap::ValueEnum>::from_str(configured, false)
        .unwrap_or(ModelProviderPolicy::Auto)
}

pub(crate) fn provider_spec_for_lane_with_key_state(
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
        | ModelProviderPolicy::PrimaryWithFallback
        | ModelProviderPolicy::OpencodeGoCanary
        | ModelProviderPolicy::OpencodeGoWide => direct_minimax_spec(args),
    }
}

pub(crate) fn fallback_provider_spec_for_lane(
    lane: &LanePlan,
    spec: &ProviderSpec,
    args: &RunArgs,
    opencode_key_present: bool,
) -> Option<ProviderSpec> {
    if spec.provider == ModelProvider::OpenCodeGo && lane.id == "opposition" {
        return Some(direct_minimax_spec(args));
    }
    if spec.provider == ModelProvider::MiniMaxDirect
        && matches!(
            args.provider_policy,
            ModelProviderPolicy::PrimaryWithFallback
        )
    {
        if !opencode_key_present {
            return None;
        }
        return Some(if is_opencode_fast_lane(&lane.id) {
            opencode_flash_spec(args)
        } else {
            opencode_canary_spec(args)
        });
    }
    None
}

pub(crate) fn direct_minimax_spec(args: &RunArgs) -> ProviderSpec {
    ProviderSpec {
        provider: ModelProvider::MiniMaxDirect,
        model: args.minimax_model.clone(),
        endpoint_kind: match args.minimax_provider_kind {
            ProviderKindArg::Openai => ProviderEndpointKind::OpenAiChat,
            ProviderKindArg::Anthropic => ProviderEndpointKind::AnthropicMessages,
        },
    }
}

pub(crate) fn opencode_canary_spec(args: &RunArgs) -> ProviderSpec {
    let model = args.opencode_model.clone();
    ProviderSpec {
        provider: ModelProvider::OpenCodeGo,
        endpoint_kind: resolve_opencode_endpoint_kind(args.opencode_endpoint_kind, &model),
        model,
    }
}

pub(crate) fn opencode_flash_spec(args: &RunArgs) -> ProviderSpec {
    let model = "deepseek-v4-flash".to_owned();
    ProviderSpec {
        provider: ModelProvider::OpenCodeGo,
        endpoint_kind: resolve_opencode_endpoint_kind(args.opencode_endpoint_kind, &model),
        model,
    }
}

pub(crate) fn is_opencode_fast_lane(lane_id: &str) -> bool {
    lane_id.ends_with("-fast")
        || lane_id.starts_with("refute-finding-")
        || matches!(lane_id, "summary-pressure" | "duplicate-noise-filter")
}

/// Decide whether a failed lane call earns one runtime fallback retry.
///
/// Preflight-time fallback (`selected_provider_spec`) covers a primary that
/// is already down before the run; this covers the temporal gap after a
/// passing preflight — concurrent lanes consume quota, so later lanes can be
/// rate limited mid-run. Retryable classes are transient provider failures
/// only: rate limiting, timeouts, and HTTP 5xx. Auth failures, parse
/// failures, and bad envelopes are deterministic and never retried. A lane
/// already running on its fallback (preflight fallback set `fallback_from`,
/// or a prior runtime retry) is terminal: there is no second fallback.
pub(crate) fn runtime_fallback_retry_spec(
    assignment: &ModelAssignment,
    receipt: &ModelLaneReceipt,
    already_retried: bool,
    status: &str,
    http_status: Option<u16>,
    key_present: fn(&str) -> bool,
) -> Option<ProviderSpec> {
    if already_retried || receipt.fallback_from.is_some() {
        return None;
    }
    let retryable = matches!(status, "rate_limited" | "timed_out")
        || (status == "failed" && http_status.is_some_and(|code| code >= 500));
    if !retryable {
        return None;
    }
    let fallback = assignment.fallback.as_ref()?;
    if !key_present(model_api_key_env(fallback.provider)) {
        return None;
    }
    Some(fallback.clone())
}

pub(crate) fn selected_provider_spec(
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

pub(crate) fn provider_preflight_ok(
    spec: &ProviderSpec,
    preflights: &[ProviderPreflightReceipt],
) -> bool {
    preflights
        .iter()
        .any(|receipt| preflight_matches_spec(receipt, spec) && receipt.status == "ok")
}

/// Per-wave provider concurrency caps resolved from `[providers.<id>]`
/// config (#310). 0 = uncapped. A provider that returned a rate limit this
/// run is shed to one in-flight lane regardless of its configured cap, so a
/// degraded provider stops receiving full waves instead of failing them.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct ProviderCaps {
    pub(crate) minimax: usize,
    pub(crate) opencode: usize,
}

impl ProviderCaps {
    pub(crate) fn from_config(providers: &ProvidersConfig) -> Self {
        Self {
            minimax: providers.minimax.max_concurrency,
            opencode: providers.opencode.max_concurrency,
        }
    }

    pub(crate) fn cap_for(self, provider: ModelProvider) -> usize {
        match provider {
            ModelProvider::MiniMaxDirect => self.minimax,
            ModelProvider::OpenCodeGo => self.opencode,
        }
    }
}

/// True when the wave has a slot open for `provider` under its effective
/// cap. Shedding floors the cap at one: an empty wave always admits the
/// provider, so progress is guaranteed even fully shed.
pub(crate) fn provider_slot_open(in_wave: usize, cap: usize, shed: bool) -> bool {
    let effective = if shed { 1 } else { cap };
    effective == 0 || in_wave < effective
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProviderEntryConfig;

    #[test]
    fn provider_caps_resolve_from_config_and_bound_waves() {
        let providers = ProvidersConfig {
            policy: String::new(),
            minimax: ProviderEntryConfig { max_concurrency: 2 },
            opencode: ProviderEntryConfig { max_concurrency: 0 },
        };
        let caps = ProviderCaps::from_config(&providers);
        assert_eq!(caps.cap_for(ModelProvider::MiniMaxDirect), 2);
        assert_eq!(caps.cap_for(ModelProvider::OpenCodeGo), 0);
        // Cap bounds the wave; 0 means uncapped.
        assert!(provider_slot_open(1, 2, false));
        assert!(!provider_slot_open(2, 2, false));
        assert!(provider_slot_open(100, 0, false));
        // Shedding floors at one regardless of configured cap, and an empty
        // wave always admits the provider - no starvation.
        assert!(provider_slot_open(0, 0, true));
        assert!(!provider_slot_open(1, 0, true));
        assert!(!provider_slot_open(1, 4, true));
    }
}
