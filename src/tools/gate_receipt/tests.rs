//! Exercise partial coverage through the real threshold projection, not a
//! synthetic interpretation of the parser result.

use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow};

use crate::tests::test_diff;
use crate::*;

#[test]
fn known_partial_counts_preserve_violations_but_never_prove_pass() -> Result<()> {
    let mut config: Config = toml::from_str(include_str!("../../../.ub-review.toml"))?;
    config.merge_defaults();
    let plan = build_plan(
        &config,
        config.selected_profile()?,
        &BoxState {
            cpus: 4,
            free_mem_mb: Some(8_000),
            free_disk_mb: Some(20_000),
            load_1m: Some(0.5),
            github_actions: true,
        },
        &test_diff(),
        Path::new("."),
        true,
    );
    let temp = tempfile::tempdir()?;
    let sensor = plan
        .sensors
        .iter()
        .find(|sensor| sensor.id == "ripr")
        .ok_or_else(|| anyhow!("ripr sensor missing"))?;
    write_sensor_status(
        temp.path(),
        sensor,
        SensorStatusWrite {
            status: "ok",
            argv: &["ripr".to_owned(), "check".to_owned()],
            duration_ms: 12,
            reason: "completed",
            exit_code: Some(0),
            timed_out: false,
        },
    )?;
    let tool = config
        .tools
        .get("ripr")
        .ok_or_else(|| anyhow!("ripr tool missing"))?;
    let mut policy = tool
        .gate
        .clone()
        .ok_or_else(|| anyhow!("ripr gate policy missing"))?;
    assert_eq!(policy.max_new_unsuppressed, Some(0));
    let status =
        crate::tools::tool_status_artifact(temp.path(), &config, config.selected_profile()?, &plan);
    let mut entry = status
        .tools
        .into_iter()
        .find(|entry| entry.id == "ripr")
        .ok_or_else(|| anyhow!("ripr tool status missing"))?;
    for schema in ["0.5", "0.6", "9.9"] {
        for skipped in [false, true] {
            for count in [0_u64, 1, 2, u64::MAX] {
                let preview = if skipped { r#"["typescript"]"# } else { "[]" };
                let text = format!(
                    r#"{{"schema_version":"{schema}","counts":{{"unsuppressed_exposure_gaps":{count}}},"preview_skipped":{preview}}}"#
                );
                fs::write(temp.path().join("sensors/ripr/gate-decision.json"), &text)?;
                for maximum in [0_u64, 1, u64::MAX] {
                    policy.max_new_unsuppressed = Some(maximum);
                    for required in [false, true] {
                        entry.required = required;
                        let outcome = crate::tools::tool_gate_outcome_entry(
                            temp.path(),
                            tool,
                            policy.clone(),
                            Some(&entry),
                        );
                        let (expected, evaluated, metric) = if schema == "9.9" {
                            ("missing_evidence", false, None)
                        } else if count > maximum {
                            ("failed", true, Some(count))
                        } else if skipped {
                            ("missing_evidence", false, None)
                        } else {
                            ("passed", true, Some(count))
                        };
                        assert_eq!(outcome.outcome, expected, "{text}, max={maximum}");
                        assert_eq!(outcome.evaluated, evaluated);
                        assert_eq!(outcome.metrics.new_unsuppressed, metric);
                        assert_eq!(outcome.required, required);
                        assert_eq!(outcome.policy.max_new_unsuppressed, Some(maximum));
                        if schema != "9.9" && skipped {
                            assert!(
                                outcome.reason.contains("preview_skipped"),
                                "{}",
                                outcome.reason
                            );
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

/// The targeted mutation campaign found that erasing this recovery hint was
/// not observed by the prior rejection tests. Pin the useful type guidance,
/// not Serde's incidental line/column formatting.
#[test]
fn malformed_counts_explain_expected_object_shape() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("gate-decision.json");
    for schema in ["0.5", "0.6"] {
        for counts in ["null", "[]", "0", "true", "\"zero\""] {
            let text = format!(
                r#"{{"schema_version":"{schema}","counts":{counts},"preview_skipped":[]}}"#
            );
            fs::write(&path, &text)?;
            let crate::tools::ToolGateDecisionState::Malformed(reason) =
                crate::tools::read_tool_gate_decision(&path)
            else {
                return Err(anyhow!("invalid counts were not rejected: {text}"));
            };
            assert!(reason.contains("expected a counts object"), "{reason}");
        }
    }
    Ok(())
}
