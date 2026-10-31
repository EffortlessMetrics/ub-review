//! Current advisory-release consumer boundary (#1339/#1341).
//! Workspace CLI proof only; final-archive qualification remains separate.

use std::fs;
use std::process::Command;

use anyhow::Result;
use serde_json::json;

#[path = "common/cli_lock.rs"]
mod cli_lock;
use cli_lock::cli_subprocess_test_lock;

#[test]
fn advisory_result_planes_cannot_override_legacy_gate_enforcement() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let artifact = temp.path().join("gate_outcome.json");
    let bin = env!("CARGO_BIN_EXE_ub-review");

    // Enumerate the published vocabulary independently of production types.
    // Contradictory fixtures are intentional: reported evidence/review/delivery
    // must neither pardon a legacy failure nor turn an outage into a code defect.
    for (conclusion, enforced_success, diagnostic) in [
        ("pass", true, "gate conclusion is `pass`"),
        ("fail", false, "blocking reasons: acceptance-obligation"),
        (
            "inconclusive",
            false,
            "required evidence unavailable: acceptance-obligation",
        ),
    ] {
        for analysis in ["clean", "findings", "limited", "not_proven"] {
            for publication in ["posted", "not_needed", "failed", "not_proven"] {
                for gate in ["pass", "finding", "not_proven"] {
                    let bytes = serde_json::to_vec_pretty(&json!({
                        "schema": "ub-review.gate_outcome.v1",
                        "conclusion": conclusion,
                        "analysis_result": analysis,
                        "publication_result": publication,
                        "gate_result": gate,
                        "not_proven_reasons": ["model-coverage: unavailable"],
                        "reasons": [{
                            "kind": "required-proof",
                            "id": "acceptance-obligation",
                            "detail": "fixture deliberately lacks a successful receipt",
                            "receipt": "review/proof_receipts.json#acceptance-obligation"
                        }]
                    }))?;
                    fs::write(&artifact, &bytes)?;
                    for (mode, enforcement, expected_success) in [
                        ("intelligent-ci", "auto", enforced_success),
                        ("review-byok", "true", enforced_success),
                        ("review-byok", "auto", true),
                        ("intelligent-ci", "false", true),
                    ] {
                        let mut command = Command::new(bin);
                        command.current_dir(temp.path());
                        for (name, _) in std::env::vars_os() {
                            let name_text = name.to_string_lossy();
                            if name_text.starts_with("UB_REVIEW_")
                                || matches!(
                                    name_text.as_ref(),
                                    "GITHUB_ACTIONS" | "RUNNER_ENVIRONMENT" | "RUNNER_NAME"
                                )
                            {
                                command.env_remove(name);
                            }
                        }
                        let output = command
                            .args(["gate-check", "--gate-outcome"])
                            .arg(&artifact)
                            .args(["--mode", mode, "--fail-on-gate", enforcement])
                            .output()?;
                        let text = format!(
                            "{}{}",
                            String::from_utf8_lossy(&output.stdout),
                            String::from_utf8_lossy(&output.stderr)
                        );
                        let case = format!(
                            "{conclusion}/{analysis}/{publication}/{gate}; {mode}/{enforcement}"
                        );
                        assert_eq!(output.status.success(), expected_success, "{case}: {text}");
                        let expected_diagnostic = if enforcement == "false"
                            || mode == "review-byok" && enforcement == "auto"
                        {
                            "gate enforcement is off"
                        } else {
                            diagnostic
                        };
                        assert!(text.contains(expected_diagnostic), "{case}: {text}");
                        assert_eq!(fs::read(&artifact)?, bytes, "{case}: receipt mutated");
                    }
                }
            }
        }
    }
    // The consumer must not create delivery or review artifacts while checking.
    assert_eq!(fs::read_dir(temp.path())?.count(), 1);
    Ok(())
}
