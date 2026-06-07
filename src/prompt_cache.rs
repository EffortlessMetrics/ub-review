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

pub(crate) fn model_cache_mode(provider: &str, endpoint_kind: &str) -> &'static str {
    if provider == ModelProvider::MiniMaxDirect.key()
        && endpoint_kind == ProviderEndpointKind::AnthropicMessages.key()
    {
        "explicit-anthropic-cache-control"
    } else {
        "not-supported"
    }
}
