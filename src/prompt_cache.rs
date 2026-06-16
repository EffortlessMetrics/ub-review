//! Prompt-cache mode selection and cache usage accounting (cleanup train
//! step 10, pure code motion). Cache artifacts are written by the run flow;
//! the mode contract lives here.

use serde::{Deserialize, Serialize};

use crate::*;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct ModelCacheUsage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cache_creation_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) cache_read_input_tokens: Option<u64>,
}

pub(crate) fn model_cache_mode_for_args(
    args: &RunArgs,
    provider: &str,
    endpoint_kind: &str,
) -> &'static str {
    if provider == ModelProvider::MiniMaxDirect.key()
        && endpoint_kind == ProviderEndpointKind::AnthropicMessages.key()
        && args.minimax_prompt_cache == MinimaxPromptCache::Off
    {
        return "not-supported";
    }
    model_cache_mode(provider, endpoint_kind)
}

pub(crate) fn model_cache_mode(provider: &str, endpoint_kind: &str) -> &'static str {
    if provider == ModelProvider::MiniMaxDirect.key()
        && endpoint_kind == ProviderEndpointKind::AnthropicMessages.key()
    {
        "explicit-anthropic-cache-control"
    } else {
        "not-supported"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_cache_usage_fields_round_trip_and_discriminate() {
        // Each field is independent; assert the actual values (not just
        // Some/None) so ripr has a same-module discriminating path.
        let usage = ModelCacheUsage {
            input_tokens: Some(100),
            output_tokens: Some(20),
            cache_creation_input_tokens: Some(80),
            cache_read_input_tokens: Some(40),
        };
        assert_eq!(usage.input_tokens, Some(100));
        assert_eq!(usage.output_tokens, Some(20));
        assert_eq!(usage.cache_creation_input_tokens, Some(80));
        assert_eq!(usage.cache_read_input_tokens, Some(40));

        let none = ModelCacheUsage::default();
        assert_eq!(none.input_tokens, None);
        assert_eq!(none.output_tokens, None);
        assert_eq!(none.cache_creation_input_tokens, None);
        assert_eq!(none.cache_read_input_tokens, None);
    }

    #[test]
    fn model_cache_mode_for_args_respects_off_override() {
        // The args.minimax_prompt_cache branch must produce a different
        // result than the default, giving ripr a discriminating path
        // through the args parameter.
        let mut args = crate::tests::test_run_args(std::path::PathBuf::from("out"));
        args.minimax_provider_kind = ProviderKindArg::Anthropic;

        args.minimax_prompt_cache = MinimaxPromptCache::ExplicitAnthropic;
        assert_eq!(
            model_cache_mode_for_args(
                &args,
                ModelProvider::MiniMaxDirect.key(),
                ProviderEndpointKind::AnthropicMessages.key()
            ),
            "explicit-anthropic-cache-control"
        );

        args.minimax_prompt_cache = MinimaxPromptCache::Off;
        assert_eq!(
            model_cache_mode_for_args(
                &args,
                ModelProvider::MiniMaxDirect.key(),
                ProviderEndpointKind::AnthropicMessages.key()
            ),
            "not-supported"
        );
    }

    #[test]
    fn model_cache_mode_non_minimax_is_not_supported() {
        assert_eq!(
            model_cache_mode(
                ModelProvider::OpenCodeGo.key(),
                ProviderEndpointKind::OpenAiChat.key()
            ),
            "not-supported"
        );
        assert_eq!(
            model_cache_mode(
                ModelProvider::MiniMaxDirect.key(),
                ProviderEndpointKind::OpenAiChat.key()
            ),
            "not-supported"
        );
    }
}
