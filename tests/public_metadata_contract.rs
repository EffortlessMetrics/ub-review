use std::collections::BTreeMap;
use std::process::Command;

use anyhow::{Result, ensure};

// Read only the Action's flat input metadata, before outputs/runs. This bounded
// YAML scalar reader accepts the current one-line scalar schema, including
// quoted empty defaults and doubled single quotes, and rejects other forms.
fn action_inputs(text: &str) -> Result<BTreeMap<String, BTreeMap<String, String>>> {
    let inputs = text
        .split_once("\ninputs:\n")
        .and_then(|(_, rest)| rest.split_once("\noutputs:\n"))
        .ok_or_else(|| anyhow::anyhow!("missing Action input metadata boundaries"))?
        .0;
    let mut parsed = BTreeMap::<String, BTreeMap<String, String>>::new();
    let mut current = String::new();
    for line in inputs.lines().filter(|line| !line.trim().is_empty()) {
        if line.starts_with("  ") && !line.starts_with("   ") {
            current = line
                .trim()
                .strip_suffix(':')
                .ok_or_else(|| anyhow::anyhow!("unsupported Action input heading: {line}"))?
                .to_owned();
            ensure!(
                parsed.insert(current.clone(), BTreeMap::new()).is_none(),
                "duplicate input"
            );
        } else {
            ensure!(
                line.starts_with("    ") && !line.starts_with("     "),
                "unsupported metadata indentation"
            );
            let (key, value) = line
                .trim()
                .split_once(": ")
                .ok_or_else(|| anyhow::anyhow!("unsupported Action metadata scalar: {line}"))?;
            let value = if let Some(quoted) = value.strip_prefix('\'') {
                quoted
                    .strip_suffix('\'')
                    .ok_or_else(|| anyhow::anyhow!("unclosed scalar"))?
                    .replace("''", "'")
            } else {
                ensure!(
                    !value.starts_with(['"', '|', '>', '[', '{']),
                    "unsupported YAML scalar"
                );
                value.to_owned()
            };
            let fields = parsed
                .get_mut(&current)
                .ok_or_else(|| anyhow::anyhow!("field without input"))?;
            ensure!(
                fields.insert(key.to_owned(), value).is_none(),
                "duplicate metadata field"
            );
        }
    }
    Ok(parsed)
}

fn help(args: &[&str]) -> Result<String> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ub-review"));
    for (key, _) in std::env::vars_os() {
        if key.to_string_lossy().starts_with("UB_REVIEW_") {
            command.env_remove(key);
        }
    }
    let output = command.args(args).output()?;
    ensure!(
        output.status.success(),
        "help failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?)
}

fn option(help: &str, name: &str) -> Result<String> {
    let start = help
        .find(&format!("\n      --{name} "))
        .or_else(|| help.find(&format!("\n      --{name}\n")))
        .ok_or_else(|| anyhow::anyhow!("missing option --{name}"))?;
    let tail = &help[start + 1..];
    Ok(tail
        .split_once("\n      --")
        .map_or(tail, |(option, _)| option)
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" "))
}

#[test]
fn current_package_and_built_help_describe_bounded_review() -> Result<()> {
    let manifest: toml::Value = toml::from_str(include_str!("../Cargo.toml"))?;
    let description = manifest["package"]["description"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("missing description"))?;
    let top = help(&["--help"])?;
    for (surface, text) in [("Cargo", description), ("CLI", top.as_str())] {
        for concept in [
            "bounded",
            "evidence",
            "configured",
            "review",
            "optional",
            "GitHub",
        ] {
            assert!(text.contains(concept), "{surface} omits {concept}");
        }
        assert!(
            !text.contains("No-token"),
            "{surface} has obsolete no-token description"
        );
        assert!(
            !text.contains("UB-focused"),
            "{surface} has obsolete UB-only boundary"
        );
    }
    Ok(())
}

#[test]
fn metadata_reader_and_option_lookup_reject_ambiguous_surfaces() -> Result<()> {
    let text = "\ninputs:\n  posting:\n    description: 'It''s local.'\n    default: ''\n    required: false\n\noutputs:\n";
    let parsed = action_inputs(text)?;
    assert_eq!(parsed["posting"]["description"], "It's local.");
    assert_eq!(parsed["posting"]["default"], "");
    assert!(
        action_inputs(&text.replace("    default: ''", "    default: ''\n    default: review"))
            .is_err()
    );
    assert!(action_inputs(&text.replace("    default: ''", "    default: >-")).is_err());
    assert!(action_inputs(&text.replace("    default: ''", "    default: 'unclosed")).is_err());
    assert!(option("\n      --model-mode <MODEL_MODE>\n          auto", "mode").is_err());
    Ok(())
}

#[test]
fn action_recommendation_is_explicit_and_compatibility_defaults_are_preserved() -> Result<()> {
    let action = include_str!("../action.yml");
    let inputs = action_inputs(action)?;
    for (name, expected) in [
        ("posting", "review"),
        ("mode", "review-byok"),
        ("review-mode", ""),
        ("model-mode", "auto"),
        ("fail-on-gate", "auto"),
    ] {
        assert_eq!(
            inputs[name]["default"], expected,
            "Action {name} default changed"
        );
        assert_eq!(inputs[name]["required"], "false");
    }
    let posture = &inputs["review-mode"]["description"];
    for concept in [
        "advisory",
        "artifact-only",
        "model-mode=off",
        "existing required CI",
        "gate",
        "strict",
        "legacy",
        "shadow",
        "Unset",
    ] {
        assert!(posture.contains(concept), "Action posture omits {concept}");
    }
    assert!(!posture.contains("check (recommended)"));
    assert!(inputs["posting"]["description"].contains("Default: review"));
    assert!(inputs["model-mode"]["description"].contains("Default: auto"));
    assert!(inputs["model-mode"]["description"].contains("semantic review was not performed"));
    assert!(inputs["posting"]["description"].contains("not successful remote delivery"));
    assert!(inputs["fail-on-gate"]["description"].contains("legacy conclusion"));
    // Top-level folded description is separate from the scalar input map.
    let description = action
        .split_once("description: >-\n")
        .and_then(|(_, rest)| rest.split_once("\n\n"))
        .ok_or_else(|| anyhow::anyhow!("missing Action description"))?
        .0;
    for concept in [
        "bounded",
        "evidence",
        "configured",
        "review",
        "optional",
        "GitHub",
    ] {
        assert!(
            description.contains(concept),
            "Action description omits {concept}"
        );
    }
    Ok(())
}

#[test]
fn built_run_and_enable_help_preserve_options_and_distinguish_recommendation() -> Result<()> {
    let run = help(&["run", "--help"])?;
    for (name, expected) in [
        ("posting", "artifact-only"),
        ("mode", "review-byok"),
        ("model-mode", "auto"),
        ("fail-on-gate", "auto"),
    ] {
        assert!(
            option(&run, name)?.contains(&format!("[default: {expected}]")),
            "run {name} default changed"
        );
    }
    let posture = option(&run, "review-mode")?;
    assert!(
        !posture.contains("[default:"),
        "review-mode must remain unset"
    );
    for concept in [
        "advisory",
        "--posting artifact-only",
        "--model-mode off",
        "existing required CI",
        "gate",
        "strict",
        "legacy",
        "shadow",
        "Unset",
    ] {
        assert!(posture.contains(concept), "run posture omits {concept}");
    }
    assert!(!posture.contains("check (recommended)"));
    assert!(option(&run, "fail-on-gate")?.contains("exactly `pass`"));
    assert!(option(&run, "model-mode")?.contains("semantic review was not performed"));
    let enable = help(&["enable", "--help"])?;
    let mode = option(&enable, "mode")?;
    assert!(mode.contains("advisory (recommended initial posture)"));
    assert!(mode.contains("gate (opt-in"));
    assert!(!mode.contains("check (recommended)"));
    assert!(option(&enable, "model")?.contains("[default: minimax]"));
    assert!(option(&enable, "model")?.contains("not automatically model-off or artifact-only"));
    assert!(option(&enable, "inspect")?.contains("before writing"));
    Ok(())
}
