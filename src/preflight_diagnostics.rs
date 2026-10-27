//! Caller-only diagnosis of the loader's retained semantic policy errors.

use std::path::Path;

use crate::Config;

/// Do not render PolicyError.detail: older receipts can contain rejected
/// scalar/array payloads and serde errors can quote secret-bearing values.
/// Keep every receipt, using its field and trusted correction text instead.
pub(crate) fn config_diagnostic(config: &Config, requested: Option<&Path>) -> Option<String> {
    if config.policy_errors.is_empty() {
        return None;
    }
    // The loader also records errors introduced by runtime-profile overrides.
    // Do not infer that optional discovery found a file, or re-read a path to
    // guess which configuration supplied a retained error.
    let source = match requested {
        Some(path) => format!(
            "explicit --config/UB_REVIEW_CONFIG `{}` and runtime-profile selection",
            bounded_label(&path.to_string_lossy())
        ),
        None => {
            let discovery = ".ub-review.toml is used only when discovered";
            format!("optional discovery/defaults ({discovery}) and runtime-profile selection")
        }
    };
    let mut message = format!(
        "invalid configuration from {source} ({} policy error(s); rejected values redacted):",
        config.policy_errors.len()
    );
    for error in &config.policy_errors {
        message.push_str(&format!(
            "\n  - {}: {}",
            field_label(&error.section),
            correction(&error.section)
        ));
    }
    message.push_str(
        "\nCorrect the configuration or runtime-profile override and retry; see docs/PREFLIGHT_DIAGNOSTICS.md.",
    );
    Some(message)
}

/// Plan's investigation packet retains all policy receipts and valid siblings,
/// but must not persist the rejected payload through raw receipt details.
/// Run keeps the loader's original strip-and-receipt/gate contract.
pub(crate) fn diagnostic_artifact_config(config: &Config) -> Config {
    let mut redacted = config.clone();
    for error in &mut redacted.policy_errors {
        error.detail = format!("Rejected value redacted. {}", correction(&error.section));
        // Sections can embed caller-controlled proof/tool IDs or tool fields.
        error.section = field_label(&error.section);
    }
    if config
        .policy_errors
        .iter()
        .any(|error| error.section == "profile")
    {
        // The loader records this error only when it selects the gh-runner
        // fallback. Record the actual selected key, never the rejected name.
        redacted.profile = "gh-runner".to_owned();
    }
    redacted
}

fn field_label(section: &str) -> String {
    if section.starts_with("proof.required.") {
        return "proof.required.[redacted id]".to_owned();
    }
    if let Some(tool_field) = section.strip_prefix("tools.") {
        let (id, field) = tool_field.split_once('.').unwrap_or((tool_field, ""));
        // Only names from the compiled-in tool registry are public labels.
        // This is presentation, not a restriction on valid custom tool IDs.
        let builtin = crate::builtin_tools().iter().any(|tool| tool.id == id);
        let id = if builtin { id } else { "[redacted id]" };
        if field.is_empty() {
            return format!("tools.{id}");
        }
        let known_field =
            crate::config::KNOWN_TOOL_POLICY_KEYS.contains(&field) || field == "gate.scope";
        let field = if known_field {
            field
        } else {
            "[redacted field]"
        };
        return format!("tools.{id}.{field}");
    }
    bounded_label(section)
}

/// Bound each path/field independently so all errors remain visible. Escaping
/// happens before the limit, keeping control characters out of terminal rows.
fn bounded_label(value: &str) -> String {
    let mut chars = value.escape_debug();
    let mut label: String = chars.by_ref().take(256).collect();
    if chars.next().is_some() {
        label.push_str("...[truncated]");
    }
    label
}

fn correction(section: &str) -> &'static str {
    match section {
        "profile" => {
            "Correct the runtime profile name (for example gh-runner), remove a --runtime-profile/--profile or environment override, or supply a config declaring its [profiles.<name>] table."
        }
        "gate.target_minutes" | "gate.hard_timeout_minutes" => {
            "Correct this [gate] field to a nonnegative integer number of minutes, or remove it to use the default."
        }
        "gate.synchronize_mode" => {
            "Remove synchronize_mode; use [gate].post_review_on to select review-posting passes."
        }
        "gate" => "Correct [gate] to a table of supported gate fields.",
        "providers" => {
            "Correct [providers] to a table; set policy to auto, minimax-primary, primary-with-fallback, minimax-only, opencode-go-canary or opencode-go-wide."
        }
        "providers.minimax.max_concurrency" | "providers.opencode.max_concurrency" => {
            "Correct max_concurrency to a positive integer, or remove it to use the default."
        }
        "providers.minimax.prompt_cache" => {
            "Correct prompt_cache to explicit-anthropic or off, or remove it."
        }
        "providers.opencode.prompt_cache" => {
            "Remove prompt_cache; OpenCode prompt caching is not implemented."
        }
        "impact" | "impact.mode" => {
            "Correct [impact] to a table with mode = \"shadow\" or mode = \"active\"."
        }
        section if section.starts_with("proof.required") => {
            "Correct the [[proof.required]] entry: use supported fields, a nonempty command and supported language/diff-class selectors, or remove the entry."
        }
        section if section.starts_with("tools.") => {
            "Correct this [tools.<id>] table/field using the config examples; tool gate scope must be on-diff. Remove unsupported fields."
        }
        section if section.starts_with("gate.") => {
            "Correct this [gate] field's spelling, type or supported value using the config examples, or remove it."
        }
        section if section.starts_with("review_body") => {
            "Correct the [review_body] table/field using the config examples, or remove unsupported fields or values."
        }
        _ => {
            "Correct the section/field spelling and shape using the config examples, or remove the unsupported section/field."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_labels_preserve_public_names_but_never_dynamic_ids_or_fields() {
        for (section, expected) in [
            ("tools.tokmd", "tools.tokmd"),
            ("tools.ripr.weight", "tools.ripr.weight"),
            ("tools.tokmd.gate.scope", "tools.tokmd.gate.scope"),
            ("tools.PRIVATE_ID", "tools.[redacted id]"),
            ("tools.PRIVATE_ID.weight", "tools.[redacted id].weight"),
            ("tools.tokmd.PRIVATE_FIELD", "tools.tokmd.[redacted field]"),
            ("proof.required.PRIVATE_ID", "proof.required.[redacted id]"),
        ] {
            assert_eq!(field_label(section), expected, "{section}");
        }
    }

    #[test]
    fn corrections_bind_each_supported_family_to_its_exact_action() {
        for (section, expected) in [
            (
                "profile",
                "Correct the runtime profile name (for example gh-runner), remove a --runtime-profile/--profile or environment override, or supply a config declaring its [profiles.<name>] table.",
            ),
            (
                "gate.target_minutes",
                "Correct this [gate] field to a nonnegative integer number of minutes, or remove it to use the default.",
            ),
            (
                "gate.hard_timeout_minutes",
                "Correct this [gate] field to a nonnegative integer number of minutes, or remove it to use the default.",
            ),
            (
                "gate.synchronize_mode",
                "Remove synchronize_mode; use [gate].post_review_on to select review-posting passes.",
            ),
            (
                "gate",
                "Correct [gate] to a table of supported gate fields.",
            ),
            (
                "providers",
                "Correct [providers] to a table; set policy to auto, minimax-primary, primary-with-fallback, minimax-only, opencode-go-canary or opencode-go-wide.",
            ),
            (
                "providers.minimax.max_concurrency",
                "Correct max_concurrency to a positive integer, or remove it to use the default.",
            ),
            (
                "providers.opencode.max_concurrency",
                "Correct max_concurrency to a positive integer, or remove it to use the default.",
            ),
            (
                "providers.minimax.prompt_cache",
                "Correct prompt_cache to explicit-anthropic or off, or remove it.",
            ),
            (
                "providers.opencode.prompt_cache",
                "Remove prompt_cache; OpenCode prompt caching is not implemented.",
            ),
            (
                "impact.mode",
                "Correct [impact] to a table with mode = \"shadow\" or mode = \"active\".",
            ),
            (
                "proof.required.example",
                "Correct the [[proof.required]] entry: use supported fields, a nonempty command and supported language/diff-class selectors, or remove the entry.",
            ),
            (
                "tools.ripr.weight",
                "Correct this [tools.<id>] table/field using the config examples; tool gate scope must be on-diff. Remove unsupported fields.",
            ),
            (
                "gate.unknown",
                "Correct this [gate] field's spelling, type or supported value using the config examples, or remove it.",
            ),
            (
                "review_body.summary_only_body",
                "Correct the [review_body] table/field using the config examples, or remove unsupported fields or values.",
            ),
            (
                "gatee",
                "Correct the section/field spelling and shape using the config examples, or remove the unsupported section/field.",
            ),
        ] {
            assert_eq!(correction(section), expected, "{section}");
        }
    }
}
