//! A parseable answer cannot override an explicitly unfinished provider turn.

use anyhow::{Result, bail};
use serde_json::Value;

pub(super) fn validate_model_completion(response: &Value) -> Result<()> {
    // Older compatible envelopes omit completion metadata. Preserve that
    // contract, but never treat an explicit unknown/null ending as completion.
    // Check the same first OpenAI choice that content extraction consumes.
    for (pointer, field, completed, incomplete) in [
        (
            "/choices/0/finish_reason",
            "finish_reason",
            &["stop"][..],
            &["length", "content_filter", "tool_calls", "function_call"][..],
        ),
        (
            "/stop_reason",
            "stop_reason",
            &["end_turn", "stop_sequence"][..],
            &[
                "max_tokens",
                "pause_turn",
                "tool_use",
                "refusal",
                "model_context_window_exceeded",
            ][..],
        ),
    ] {
        let Some(value) = response.pointer(pointer) else {
            continue;
        };
        let diagnosis = match value.as_str() {
            Some(reason) if completed.contains(&reason) => continue,
            Some(reason) if incomplete.contains(&reason) => reason,
            Some(_) => "unsupported",
            None if value.is_null() => "null",
            None => "non-string",
        };
        // Only fixed protocol vocabulary reaches the error receipt. The raw
        // response was already retained; arbitrary provider text stays there.
        bail!(
            "model response completion incomplete: {field}={diagnosis}; retry with a complete response"
        );
    }
    Ok(())
}
