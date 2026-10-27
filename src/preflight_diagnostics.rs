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
    let (source, path) = match requested {
        Some(path) => ("explicit --config/UB_REVIEW_CONFIG", path),
        None => ("discovered", Path::new(".ub-review.toml")),
    };
    let mut message = format!(
        "invalid configuration from {source} `{}` ({} policy error(s); rejected values redacted):",
        bounded_label(&path.to_string_lossy()),
        config.policy_errors.len()
    );
    for error in &config.policy_errors {
        message.push_str(&format!(
            "\n  - {}: {}",
            field_label(&error.section),
            correction(&error.section)
        ));
    }
    message.push_str("\nCorrect the selected config and retry; see docs/PREFLIGHT_DIAGNOSTICS.md.");
    Some(message)
}

/// Plan's investigation packet retains all policy receipts and valid siblings,
/// but must not persist the rejected payload through raw receipt details.
/// Run keeps the loader's original strip-and-receipt/gate contract.
pub(crate) fn diagnostic_artifact_config(config: &Config) -> Config {
    let mut redacted = config.clone();
    for error in &mut redacted.policy_errors {
        error.detail = format!("Rejected value redacted. {}", correction(&error.section));
        // A proof's section can embed its configured id, which is a value.
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
        "proof.required.[redacted id]".to_owned()
    } else {
        bounded_label(section)
    }
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
            "Correct the runtime profile name (for example gh-runner), or declare its [profiles.<name>] table."
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
