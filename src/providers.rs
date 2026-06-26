//! Model provider identity, spec selection, and preflight routing (cleanup
//! train step 11, pure code motion). Provider specs name which model serves
//! a lane and over which endpoint; the routing functions here resolve a lane
//! to its primary spec and optional fallback under the configured provider
//! policy. Receipt construction (ModelLaneReceipt, ProviderPreflightReceipt
//! writers) and runtime backpressure stay with the run flow in main.rs; this
//! module owns only the identity and selection.

use std::collections::BTreeMap;

use anyhow::{Result, bail};

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

impl ModelProvider {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::MiniMaxDirect => "minimax",
            Self::OpenCodeGo => "opencode-go",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum ProviderEndpointKind {
    OpenAiChat,
    AnthropicMessages,
}

impl ProviderEndpointKind {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::OpenAiChat => "openai-chat",
            Self::AnthropicMessages => "anthropic-messages",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct ProviderSpec {
    pub(crate) provider: ModelProvider,
    pub(crate) model: String,
    pub(crate) endpoint_kind: ProviderEndpointKind,
}

impl ProviderSpec {
    pub(crate) fn label(&self) -> String {
        format!(
            "{}:{}:{}",
            self.provider.key(),
            self.model,
            self.endpoint_kind.key()
        )
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ProviderConcurrencyLimits {
    pub(crate) minimax: Option<usize>,
    pub(crate) opencode_go: Option<usize>,
}

impl ProviderConcurrencyLimits {
    pub(crate) fn limit_for(self, provider: ModelProvider, global_limit: usize) -> usize {
        let configured = match provider {
            ModelProvider::MiniMaxDirect => self.minimax,
            ModelProvider::OpenCodeGo => self.opencode_go,
        };
        configured.unwrap_or(global_limit).min(global_limit).max(1)
    }
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
        .map(|lane| model_assignment_for_lane_with_key_state(lane, args, opencode_key_present))
        .collect::<Vec<_>>();
    Ok(assignments)
}

pub(crate) fn proof_planner_assignment(args: &RunArgs) -> ModelAssignment {
    proof_planner_assignment_with_key_state(args, model_api_key_present(ModelProvider::OpenCodeGo))
}

pub(crate) fn proof_planner_assignment_with_key_state(
    args: &RunArgs,
    opencode_key_present: bool,
) -> ModelAssignment {
    model_assignment_for_lane_with_key_state(proof_planner_lane(), args, opencode_key_present)
}

pub(crate) fn follow_up_provider_assignment_with_key_state(
    args: &RunArgs,
    opencode_key_present: bool,
) -> ModelAssignment {
    model_assignment_for_lane_with_key_state(follow_up_provider_lane(), args, opencode_key_present)
}

pub(crate) fn model_assignment_for_lane_with_key_state(
    lane: LanePlan,
    args: &RunArgs,
    opencode_key_present: bool,
) -> ModelAssignment {
    let spec = provider_spec_for_lane_with_key_state(&lane, args, opencode_key_present);
    let fallback = fallback_provider_spec_for_lane(&lane, &spec, args, opencode_key_present);
    ModelAssignment {
        lane,
        spec,
        fallback,
    }
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

pub(crate) fn provider_concurrency_limits(config: &Config) -> ProviderConcurrencyLimits {
    ProviderConcurrencyLimits {
        minimax: config.providers.minimax.max_concurrency,
        opencode_go: config.providers.opencode.max_concurrency,
    }
}

pub(crate) fn resolved_minimax_prompt_cache(config: &Config) -> MinimaxPromptCache {
    match config.providers.minimax.prompt_cache.as_deref() {
        Some(value) if value == MinimaxPromptCache::Off.key() => MinimaxPromptCache::Off,
        _ => MinimaxPromptCache::ExplicitAnthropic,
    }
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

pub(crate) fn resolve_opencode_endpoint_kind(
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

pub(crate) fn is_opencode_openai_chat_model(model: &str) -> bool {
    model.starts_with("deepseek-") || model.starts_with("kimi-") || model.starts_with("mimo-")
}

pub(crate) fn is_opencode_fast_lane(lane_id: &str) -> bool {
    lane_id.ends_with("-fast")
        || lane_id.starts_with("refute-finding-")
        || matches!(lane_id, "summary-pressure" | "duplicate-noise-filter")
}

pub(crate) fn provider_spec_from_preflight(
    receipt: &ProviderPreflightReceipt,
) -> Result<ProviderSpec> {
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

pub(crate) fn provider_assignment_preflight_failed_reason(
    assignment: &ModelAssignment,
    preflights: &[ProviderPreflightReceipt],
) -> String {
    let primary = provider_preflight_reason(&assignment.spec, preflights)
        .unwrap_or_else(|| format!("{} preflight receipt missing", assignment.spec.label()));
    if let Some(fallback) = &assignment.fallback {
        let fallback_reason = provider_preflight_reason(fallback, preflights)
            .unwrap_or_else(|| format!("{} preflight receipt missing", fallback.label()));
        format!("{primary}; fallback unavailable: {fallback_reason}")
    } else {
        primary
    }
}

pub(crate) fn selected_provider_spec_with_backpressure(
    assignment: &ModelAssignment,
    preflights: &[ProviderPreflightReceipt],
    provider_backpressure: &BTreeMap<ModelProvider, ProviderBackpressure>,
) -> Option<(ProviderSpec, Option<String>, Option<String>)> {
    let (spec, fallback_from, preflight_reason) = selected_provider_spec(assignment, preflights)?;
    let Some(backpressure) = provider_backpressure.get(&spec.provider) else {
        return Some((spec, fallback_from, preflight_reason));
    };
    let Some(fallback) = assignment.fallback.as_ref() else {
        return Some((spec, fallback_from, preflight_reason));
    };
    if fallback.provider == spec.provider
        || provider_backpressure.contains_key(&fallback.provider)
        || !provider_preflight_ok(fallback, preflights)
    {
        return Some((spec, fallback_from, preflight_reason));
    }
    Some((
        fallback.clone(),
        Some(assignment.spec.label()),
        Some(format!(
            "provider backed off after {}; fallback used: {}",
            provider_backpressure_label(backpressure),
            fallback.label()
        )),
    ))
}

pub(crate) fn provider_preflight_ok(
    spec: &ProviderSpec,
    preflights: &[ProviderPreflightReceipt],
) -> bool {
    preflights
        .iter()
        .any(|receipt| preflight_matches_spec(receipt, spec) && receipt.status == "ok")
}

pub(crate) fn provider_preflight_reason(
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

pub(crate) fn preflight_matches_spec(
    receipt: &ProviderPreflightReceipt,
    spec: &ProviderSpec,
) -> bool {
    receipt.provider == spec.provider.key()
        && receipt.model == spec.model
        && receipt.endpoint_kind == spec.endpoint_kind.key()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_preflight(spec: &ProviderSpec) -> ProviderPreflightReceipt {
        ProviderPreflightReceipt {
            provider: spec.provider.key().to_owned(),
            model: spec.model.clone(),
            endpoint_kind: spec.endpoint_kind.key().to_owned(),
            status: "ok".to_owned(),
            reason: "ready".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            cache_usage: ModelCacheUsage::default(),
        }
    }

    fn bad_preflight(spec: &ProviderSpec) -> ProviderPreflightReceipt {
        ProviderPreflightReceipt {
            provider: spec.provider.key().to_owned(),
            model: spec.model.clone(),
            endpoint_kind: spec.endpoint_kind.key().to_owned(),
            status: "unavailable".to_owned(),
            reason: "key missing".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            cache_usage: ModelCacheUsage::default(),
        }
    }

    #[test]
    fn provider_keys_and_labels_are_stable() {
        assert_eq!(ModelProvider::MiniMaxDirect.key(), "minimax");
        assert_eq!(ModelProvider::OpenCodeGo.key(), "opencode-go");
        assert_eq!(ProviderEndpointKind::OpenAiChat.key(), "openai-chat");
        assert_eq!(
            ProviderEndpointKind::AnthropicMessages.key(),
            "anthropic-messages"
        );
        let spec = ProviderSpec {
            provider: ModelProvider::MiniMaxDirect,
            model: "test".to_owned(),
            endpoint_kind: ProviderEndpointKind::AnthropicMessages,
        };
        assert_eq!(spec.label(), "minimax:test:anthropic-messages");
    }

    #[test]
    fn resolve_opencode_endpoint_kind_matches_each_arm() {
        assert_eq!(
            resolve_opencode_endpoint_kind(OpenCodeEndpointKindArg::OpenaiChat, "deepseek-v4"),
            ProviderEndpointKind::OpenAiChat
        );
        assert_eq!(
            resolve_opencode_endpoint_kind(
                OpenCodeEndpointKindArg::AnthropicMessages,
                "deepseek-v4"
            ),
            ProviderEndpointKind::AnthropicMessages
        );
        assert_eq!(
            resolve_opencode_endpoint_kind(OpenCodeEndpointKindArg::Auto, "deepseek-v4"),
            ProviderEndpointKind::OpenAiChat
        );
        assert_eq!(
            resolve_opencode_endpoint_kind(OpenCodeEndpointKindArg::Auto, "gpt-4o"),
            ProviderEndpointKind::AnthropicMessages
        );
    }

    #[test]
    fn provider_spec_from_preflight_round_trips() -> Result<()> {
        let receipt = ProviderPreflightReceipt {
            provider: "minimax".to_owned(),
            model: "M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: "ready".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            cache_usage: ModelCacheUsage::default(),
        };
        let spec = provider_spec_from_preflight(&receipt)?;
        assert_eq!(spec.provider, ModelProvider::MiniMaxDirect);
        assert_eq!(spec.model, "M3");
        assert_eq!(spec.endpoint_kind, ProviderEndpointKind::AnthropicMessages);
        Ok(())
    }

    #[test]
    fn provider_spec_from_preflight_rejects_unknown() {
        let bad = ProviderPreflightReceipt {
            provider: "unknown".to_owned(),
            model: "x".to_owned(),
            endpoint_kind: "openai-chat".to_owned(),
            status: "ok".to_owned(),
            reason: String::new(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            cache_usage: ModelCacheUsage::default(),
        };
        assert!(provider_spec_from_preflight(&bad).is_err());
    }

    #[test]
    fn preflight_matches_spec_discriminates_fields() {
        let spec = ProviderSpec {
            provider: ModelProvider::MiniMaxDirect,
            model: "M3".to_owned(),
            endpoint_kind: ProviderEndpointKind::AnthropicMessages,
        };
        assert!(preflight_matches_spec(&ok_preflight(&spec), &spec));
        let wrong_provider = ProviderPreflightReceipt {
            provider: "opencode-go".to_owned(),
            ..ok_preflight(&spec)
        };
        assert!(!preflight_matches_spec(&wrong_provider, &spec));
        let wrong_model = ProviderPreflightReceipt {
            model: "other".to_owned(),
            ..ok_preflight(&spec)
        };
        assert!(!preflight_matches_spec(&wrong_model, &spec));
        let wrong_endpoint = ProviderPreflightReceipt {
            endpoint_kind: "openai-chat".to_owned(),
            ..ok_preflight(&spec)
        };
        assert!(!preflight_matches_spec(&wrong_endpoint, &spec));
    }

    #[test]
    fn selected_provider_spec_prefers_primary_then_fallback() -> Result<()> {
        let primary = ProviderSpec {
            provider: ModelProvider::MiniMaxDirect,
            model: "M3".to_owned(),
            endpoint_kind: ProviderEndpointKind::AnthropicMessages,
        };
        let fallback = ProviderSpec {
            provider: ModelProvider::OpenCodeGo,
            model: "canary".to_owned(),
            endpoint_kind: ProviderEndpointKind::AnthropicMessages,
        };
        let assignment = ModelAssignment {
            lane: LanePlan {
                id: "opposition".to_owned(),
                role: "skeptical".to_owned(),
                model: "M3".to_owned(),
                model_display: "M3".to_owned(),
                receives: vec![],
                focus: "find defects".to_owned(),
            },
            spec: primary.clone(),
            fallback: Some(fallback.clone()),
        };
        let (spec, from, reason) =
            selected_provider_spec(&assignment, &[ok_preflight(&primary)])
                .ok_or_else(|| anyhow::anyhow!("primary ok should return spec"))?;
        assert_eq!(spec, primary);
        assert!(from.is_none());
        assert!(reason.is_none());
        let (spec, from, reason) = selected_provider_spec(
            &assignment,
            &[bad_preflight(&primary), ok_preflight(&fallback)],
        )
        .ok_or_else(|| anyhow::anyhow!("fallback ok should return spec"))?;
        assert_eq!(spec, fallback);
        assert_eq!(from.as_deref(), Some("minimax:M3:anthropic-messages"));
        assert!(reason.is_some_and(|r| r.contains("fallback used")));
        assert!(
            selected_provider_spec(
                &assignment,
                &[bad_preflight(&primary), bad_preflight(&fallback)]
            )
            .is_none()
        );
        Ok(())
    }

    #[test]
    fn provider_preflight_reason_formats_status_and_reason() -> Result<()> {
        let spec = ProviderSpec {
            provider: ModelProvider::MiniMaxDirect,
            model: "M3".to_owned(),
            endpoint_kind: ProviderEndpointKind::AnthropicMessages,
        };
        let reason = provider_preflight_reason(&spec, &[bad_preflight(&spec)])
            .ok_or_else(|| anyhow::anyhow!("bad preflight should have reason"))?;
        assert!(reason.contains("minimax"));
        assert!(reason.contains("unavailable"));
        assert!(reason.contains("key missing"));
        Ok(())
    }

    #[test]
    fn provider_assignment_preflight_failed_reason_includes_both() -> Result<()> {
        let primary = ProviderSpec {
            provider: ModelProvider::MiniMaxDirect,
            model: "M3".to_owned(),
            endpoint_kind: ProviderEndpointKind::AnthropicMessages,
        };
        let fallback_spec = ProviderSpec {
            provider: ModelProvider::OpenCodeGo,
            model: "canary".to_owned(),
            endpoint_kind: ProviderEndpointKind::AnthropicMessages,
        };
        let assignment = ModelAssignment {
            lane: LanePlan {
                id: "opposition".to_owned(),
                role: "skeptical".to_owned(),
                model: "M3".to_owned(),
                model_display: "M3".to_owned(),
                receives: vec![],
                focus: "find defects".to_owned(),
            },
            spec: primary,
            fallback: Some(fallback_spec),
        };
        let fallback = assignment
            .fallback
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("assignment should have fallback"))?;
        let msg = provider_assignment_preflight_failed_reason(
            &assignment,
            &[bad_preflight(&assignment.spec), bad_preflight(fallback)],
        );
        assert!(msg.contains("fallback unavailable"));
        Ok(())
    }

    fn test_lane_receipt() -> ModelLaneReceipt {
        ModelLaneReceipt {
            lane: "security".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "openai-chat".to_owned(),
            status: "running".to_owned(),
            reason: "test".to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: ModelCacheUsage::default(),
            cohort_id: String::new(),
            shared_prefix_hash: String::new(),
            thread_id: String::new(),
            turn: 0,
            cohort_broken: false,
        }
    }

    #[test]
    fn runtime_fallback_retry_spec_only_retries_transient_primary_failures() {
        let primary = ProviderSpec {
            provider: ModelProvider::MiniMaxDirect,
            model: "M3".to_owned(),
            endpoint_kind: ProviderEndpointKind::AnthropicMessages,
        };
        let fallback = ProviderSpec {
            provider: ModelProvider::OpenCodeGo,
            model: "canary".to_owned(),
            endpoint_kind: ProviderEndpointKind::AnthropicMessages,
        };
        let assignment = ModelAssignment {
            lane: LanePlan {
                id: "security".to_owned(),
                role: "skeptical".to_owned(),
                model: "M3".to_owned(),
                model_display: "M3".to_owned(),
                receives: vec![],
                focus: "find defects".to_owned(),
            },
            spec: primary,
            fallback: Some(fallback),
        };
        let receipt = test_lane_receipt();
        let key_present: fn(&str) -> bool = |_| true;
        // Transient classes retry on the fallback spec.
        for (status, http) in [
            ("rate_limited", Some(429)),
            ("timed_out", None),
            ("failed", Some(500)),
            ("failed", Some(503)),
        ] {
            let spec = runtime_fallback_retry_spec(
                &assignment,
                &receipt,
                false,
                status,
                http,
                key_present,
            );
            assert_eq!(
                spec.as_ref().map(|s| s.provider),
                Some(ModelProvider::OpenCodeGo),
                "{status} {http:?} should retry on the fallback",
            );
        }
        // Deterministic failures never retry.
        for (status, http) in [
            ("auth_failed", Some(401)),
            ("invalid_json", Some(200)),
            ("failed", Some(404)),
            ("failed", None),
        ] {
            assert!(
                runtime_fallback_retry_spec(
                    &assignment,
                    &receipt,
                    false,
                    status,
                    http,
                    key_present,
                )
                .is_none(),
                "{status} {http:?} must not retry",
            );
        }
        // Already retried or already on fallback -> no retry.
        assert!(
            runtime_fallback_retry_spec(
                &assignment,
                &receipt,
                true,
                "rate_limited",
                Some(429),
                key_present,
            )
            .is_none(),
        );
    }
}
