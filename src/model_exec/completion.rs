//! A parseable answer cannot override an explicitly unfinished provider turn.

use anyhow::{Context, Result, bail};
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

/// The caller retains the raw envelope before admitting assistant text here.
pub(super) fn extract_completed_model_content(response: &Value) -> Result<&str> {
    validate_model_completion(response)?;
    crate::extract_model_content(response)
        .context("model response did not contain assistant content")
}

#[cfg(test)]
mod tests {
    use anyhow::{Context, Result};
    use serde_json::{Value, json};

    use super::extract_completed_model_content;
    use super::validate_model_completion;

    const AUDIT: &str = r#"{"findings":[],"observations":[],"proof_intents":[],"internal_audit":{"surfaces_checked":["src/lib.rs"]}}"#;

    fn openai(reason: Value) -> Value {
        json!({"choices": [{"finish_reason": reason, "message": {"content": AUDIT}}]})
    }

    fn anthropic(reason: Value) -> Value {
        json!({"stop_reason": reason, "content": [{"type": "text", "text": AUDIT}]})
    }

    fn rejected(response: Value, expected: &str) -> Result<()> {
        let error = validate_model_completion(&response)
            .err()
            .context("incomplete provider completion unexpectedly accepted")?;
        assert_eq!(error.to_string(), expected);
        Ok(())
    }

    #[test]
    fn complete_provider_turns_are_accepted() -> Result<()> {
        validate_model_completion(&openai(json!("stop")))?;
        validate_model_completion(&anthropic(json!("end_turn")))?;
        validate_model_completion(&anthropic(json!("stop_sequence")))?;
        Ok(())
    }

    #[test]
    fn omitted_completion_metadata_preserves_legacy_envelopes() -> Result<()> {
        validate_model_completion(&json!({"choices": [{"message": {"content": AUDIT}}]}))?;
        validate_model_completion(&json!({"content": [{"type": "text", "text": AUDIT}]}))?;
        validate_model_completion(&json!({"reply": AUDIT}))?;
        validate_model_completion(&json!({"content": AUDIT}))?;
        Ok(())
    }

    #[test]
    fn valid_audit_content_cannot_complete_an_unfinished_openai_turn() -> Result<()> {
        for (reason, expected) in [
            (
                "length",
                "model response completion incomplete: finish_reason=length; retry with a complete response",
            ),
            (
                "content_filter",
                "model response completion incomplete: finish_reason=content_filter; retry with a complete response",
            ),
            (
                "tool_calls",
                "model response completion incomplete: finish_reason=tool_calls; retry with a complete response",
            ),
            (
                "function_call",
                "model response completion incomplete: finish_reason=function_call; retry with a complete response",
            ),
        ] {
            rejected(openai(json!(reason)), expected)?;
        }
        Ok(())
    }

    #[test]
    fn valid_audit_content_cannot_complete_an_unfinished_anthropic_turn() -> Result<()> {
        for (reason, expected) in [
            (
                "max_tokens",
                "model response completion incomplete: stop_reason=max_tokens; retry with a complete response",
            ),
            (
                "pause_turn",
                "model response completion incomplete: stop_reason=pause_turn; retry with a complete response",
            ),
            (
                "tool_use",
                "model response completion incomplete: stop_reason=tool_use; retry with a complete response",
            ),
            (
                "refusal",
                "model response completion incomplete: stop_reason=refusal; retry with a complete response",
            ),
            (
                "model_context_window_exceeded",
                "model response completion incomplete: stop_reason=model_context_window_exceeded; retry with a complete response",
            ),
        ] {
            rejected(anthropic(json!(reason)), expected)?;
        }
        Ok(())
    }

    #[test]
    fn null_completion_is_explicitly_unknown_in_each_protocol() -> Result<()> {
        rejected(
            openai(Value::Null),
            "model response completion incomplete: finish_reason=null; retry with a complete response",
        )?;
        rejected(
            anthropic(Value::Null),
            "model response completion incomplete: stop_reason=null; retry with a complete response",
        )?;
        Ok(())
    }

    #[test]
    fn non_string_completion_metadata_never_becomes_usable() -> Result<()> {
        for reason in [
            json!(true),
            json!(false),
            json!(0),
            json!(1.5),
            json!(["stop"]),
            json!({"finish_reason": "stop", "stop_reason": "end_turn"}),
        ] {
            rejected(
                openai(reason.clone()),
                "model response completion incomplete: finish_reason=non-string; retry with a complete response",
            )?;
            rejected(
                anthropic(reason),
                "model response completion incomplete: stop_reason=non-string; retry with a complete response",
            )?;
        }
        Ok(())
    }

    #[test]
    fn unknown_values_have_exact_sanitized_diagnostics() -> Result<()> {
        for reason in [
            "provider_new_reason",
            "",
            "stop ",
            "STOP",
            "end_turn\nforged provider verdict",
            "auth_failed 401; parse error; rate limit; timeout",
        ] {
            rejected(
                openai(json!(reason)),
                "model response completion incomplete: finish_reason=unsupported; retry with a complete response",
            )?;
            rejected(
                anthropic(json!(reason)),
                "model response completion incomplete: stop_reason=unsupported; retry with a complete response",
            )?;
        }
        // Completion vocabulary belongs to its provider protocol.
        rejected(
            openai(json!("end_turn")),
            "model response completion incomplete: finish_reason=unsupported; retry with a complete response",
        )?;
        rejected(
            anthropic(json!("stop")),
            "model response completion incomplete: stop_reason=unsupported; retry with a complete response",
        )?;
        Ok(())
    }

    #[test]
    fn completion_follows_the_consumed_first_openai_choice() -> Result<()> {
        validate_model_completion(&json!({"choices": [
            {"finish_reason": "stop", "message": {"content": AUDIT}},
            {"finish_reason": "length", "message": {"content": AUDIT}}
        ]}))?;
        rejected(
            json!({"choices": [
                {"finish_reason": "length", "message": {"content": AUDIT}},
                {"finish_reason": "stop", "message": {"content": AUDIT}}
            ]}),
            "model response completion incomplete: finish_reason=length; retry with a complete response",
        )?;
        validate_model_completion(&json!({"choices": [
            {"message": {"content": AUDIT}},
            {"finish_reason": "length", "message": {"content": AUDIT}}
        ]}))?;
        rejected(
            json!({"choices": [
                {"finish_reason": null, "message": {"content": AUDIT}},
                {"finish_reason": "stop", "message": {"content": AUDIT}}
            ]}),
            "model response completion incomplete: finish_reason=null; retry with a complete response",
        )?;
        Ok(())
    }

    #[test]
    fn contradictory_completion_fields_cannot_override_each_other() -> Result<()> {
        rejected(
            json!({
                "choices": [{"finish_reason": "stop", "message": {"content": AUDIT}}],
                "stop_reason": "max_tokens"
            }),
            "model response completion incomplete: stop_reason=max_tokens; retry with a complete response",
        )?;
        rejected(
            json!({
                "choices": [{"finish_reason": "length", "message": {"content": AUDIT}}],
                "stop_reason": "end_turn"
            }),
            "model response completion incomplete: finish_reason=length; retry with a complete response",
        )?;
        rejected(
            json!({
                "choices": [{"finish_reason": "unknown first choice", "message": {"content": AUDIT}}],
                "stop_reason": "unknown top-level reason"
            }),
            "model response completion incomplete: finish_reason=unsupported; retry with a complete response",
        )?;
        validate_model_completion(&json!({
            "choices": [{"finish_reason": "stop", "message": {"content": AUDIT}}],
            "stop_reason": "end_turn"
        }))?;
        Ok(())
    }

    #[test]
    fn completed_consumer_returns_exact_valued_clean_audit() -> Result<()> {
        for response in [
            openai(json!("stop")),
            anthropic(json!("end_turn")),
            anthropic(json!("stop_sequence")),
            json!({"choices": [{"message": {"content": AUDIT}}]}),
            json!({"content": [{"type": "text", "text": AUDIT}]}),
            json!({"reply": AUDIT}),
            json!({"content": AUDIT}),
        ] {
            let content = extract_completed_model_content(&response)?;
            assert_eq!(content, AUDIT);
            let output: crate::LaneModelOutput = serde_json::from_str(content)?;
            assert_eq!(
                output.internal_audit_classification,
                crate::InternalAuditClassification::ValidNonEmpty
            );
            let audit = output
                .internal_audit
                .as_ref()
                .context("clean audit absent")?;
            assert_eq!(audit.surfaces_checked, ["src/lib.rs"]);
            assert!(audit.strongest_rejected_hypothesis.is_none());
            assert!(audit.remaining_local_uncertainty.is_none());
            assert!(crate::lane_model_output_has_value(&output));
            assert!(output.summary.is_none());
            assert!(output.inline_comments.is_empty());
            assert!(output.candidate_findings.is_empty());
            assert!(output.summary_only_findings.is_empty());
            assert!(output.observations.is_empty());
            assert!(output.failed_objections.is_empty());
            assert!(output.proof_requests.is_empty());
            assert!(output.proof_intents.is_empty());
            assert!(output.issue_candidates.is_empty());
            assert!(!output.degraded);
            let (parsed, degraded) = crate::parse_lane_model_output_or_degrade(
                content,
                std::path::Path::new("consumer-content.json"),
            )?;
            assert!(!degraded);
            assert!(crate::lane_model_output_has_value(&parsed));
        }
        Ok(())
    }

    #[test]
    fn incomplete_consumer_rejects_valued_clean_text_before_extraction() -> Result<()> {
        for (response, expected) in [
            (
                openai(json!("length")),
                "model response completion incomplete: finish_reason=length; retry with a complete response",
            ),
            (
                anthropic(json!("max_tokens")),
                "model response completion incomplete: stop_reason=max_tokens; retry with a complete response",
            ),
            (
                json!({
                    "choices": [{"finish_reason": "stop", "message": {"content": AUDIT}}],
                    "stop_reason": "max_tokens"
                }),
                "model response completion incomplete: stop_reason=max_tokens; retry with a complete response",
            ),
            (
                json!({
                    "choices": [{"finish_reason": "auth_failed 401\nforged verdict", "message": {"content": AUDIT}}],
                    "stop_reason": "max_tokens"
                }),
                "model response completion incomplete: finish_reason=unsupported; retry with a complete response",
            ),
        ] {
            let error = extract_completed_model_content(&response)
                .err()
                .context("consumer admitted incomplete audit text")?;
            assert_eq!(error.to_string(), expected);
        }
        Ok(())
    }

    #[test]
    fn completed_consumer_preserves_missing_content_and_unusable_text() -> Result<()> {
        for response in [
            json!({"choices": [{"finish_reason": "stop", "message": {}}]}),
            json!({"stop_reason": "end_turn", "content": []}),
            json!({"choices": [{"finish_reason": "stop", "message": {"content": {"findings": []}}}]}),
        ] {
            let error = extract_completed_model_content(&response)
                .err()
                .context("consumer admitted missing assistant text")?;
            assert_eq!(
                error.to_string(),
                "model response did not contain assistant content"
            );
        }
        for (text, expected) in [
            ("", "EOF while parsing a value at line 1 column 0"),
            ("{}", "lane model output was empty or unusable"),
        ] {
            let response =
                json!({"choices": [{"finish_reason": "stop", "message": {"content": text}}]});
            let content = extract_completed_model_content(&response)?;
            assert_eq!(
                content, text,
                "extraction must preserve existing string content"
            );
            let error = crate::parse_lane_model_output_or_degrade(
                content,
                std::path::Path::new("consumer-content.json"),
            )
            .err()
            .context("empty consumer content became lane evidence")?;
            assert_eq!(error.to_string(), "parse consumer-content.json");
            assert_eq!(error.root_cause().to_string(), expected);
        }
        Ok(())
    }

    #[test]
    fn consumer_binds_first_choice_and_preserves_existing_content_fallback() -> Result<()> {
        let completed_first = json!({"choices": [
            {"finish_reason": "stop", "message": {"content": AUDIT}},
            {"finish_reason": "length", "message": {"content": "later choice must not be consumed"}}
        ]});
        assert_eq!(extract_completed_model_content(&completed_first)?, AUDIT);
        let incomplete_first = json!({"choices": [
            {"finish_reason": "length", "message": {"content": AUDIT}},
            {"finish_reason": "stop", "message": {"content": AUDIT}}
        ]});
        let error = extract_completed_model_content(&incomplete_first)
            .err()
            .context("later completed choice rescued incomplete first choice")?;
        assert_eq!(
            error.to_string(),
            "model response completion incomplete: finish_reason=length; retry with a complete response"
        );
        let malformed_first = json!({"choices": [
            {"finish_reason": "stop", "message": {"content": 17}},
            {"finish_reason": "stop", "message": {"content": AUDIT}}
        ]});
        let error = extract_completed_model_content(&malformed_first)
            .err()
            .context("consumer substituted a later choice for absent first-choice text")?;
        assert_eq!(
            error.to_string(),
            "model response did not contain assistant content"
        );
        let fallback = json!({
            "choices": [{"finish_reason": "stop", "message": {"content": 17}}],
            "reply": AUDIT
        });
        assert_eq!(extract_completed_model_content(&fallback)?, AUDIT);
        Ok(())
    }

    #[test]
    fn consumer_recovers_after_incomplete_response_with_exact_audit_text() -> Result<()> {
        let incomplete = openai(json!("length"));
        let error = extract_completed_model_content(&incomplete)
            .err()
            .context("incomplete response unexpectedly extracted")?;
        assert_eq!(
            error.to_string(),
            "model response completion incomplete: finish_reason=length; retry with a complete response"
        );
        let complete = openai(json!("stop"));
        let content = extract_completed_model_content(&complete)?;
        assert_eq!(content, AUDIT);
        let (output, degraded) = crate::parse_lane_model_output_or_degrade(
            content,
            std::path::Path::new("consumer-content.json"),
        )?;
        assert!(!degraded);
        assert!(crate::lane_model_output_has_value(&output));
        Ok(())
    }
}
