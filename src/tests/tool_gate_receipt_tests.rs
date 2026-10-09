//! Format-admission regressions for #1333. These invoke the production reader
//! and tool-gate projection with real files, without launching any analyzer.

use std::fs;
use std::path::Path;

use anyhow::{Result, anyhow, bail};

use super::{ToolGateDecisionState, read_tool_gate_decision};
use crate::tests::test_diff;
use crate::*;

fn read_receipt(text: &str) -> Result<ToolGateDecisionState> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("gate-decision.json");
    fs::write(&path, text)?;
    Ok(read_tool_gate_decision(&path))
}

fn malformed_reason(text: &str) -> Result<String> {
    match read_receipt(text)? {
        ToolGateDecisionState::Malformed(reason) => Ok(reason),
        ToolGateDecisionState::Missing => bail!("existing receipt was missing: {text}"),
        ToolGateDecisionState::Present(decision) => {
            bail!(
                "unadmitted receipt yielded count {}: {text}",
                decision.new_unsuppressed
            )
        }
    }
}

#[test]
fn supported_native_and_badge_versions_preserve_exact_counts() -> Result<()> {
    // v0.5 is the retained 0.8.0 fixture contract. Released RIPR v0.10.0
    // declares BADGE_SCHEMA_VERSION = "0.6" and emits preview_skipped.
    for count in [0_u64, 1, u64::MAX] {
        let receipts = [
            format!(r#"{{"new_unsuppressed":{count}}}"#),
            format!(
                r#"{{"schema_version":"0.5","counts":{{"unsuppressed_exposure_gaps":{count}}}}}"#
            ),
            format!(
                r#"{{"schema_version":"0.6","counts":{{"unsuppressed_exposure_gaps":{count}}},"preview_skipped":[]}}"#
            ),
        ];
        for text in receipts {
            let ToolGateDecisionState::Present(decision) = read_receipt(&text)? else {
                bail!("supported receipt rejected: {text}");
            };
            assert_eq!(decision.new_unsuppressed, count, "{text}");
        }
    }
    Ok(())
}

#[test]
fn badge_unknown_versions_cannot_evaluate_familiar_zero_counts() -> Result<()> {
    for version in ["9.9", "0.7", "", "0.5.0", "0.6 "] {
        let text = format!(
            r#"{{"schema_version":"{version}","counts":{{"unsuppressed_exposure_gaps":0}},"preview_skipped":[]}}"#
        );
        let reason = malformed_reason(&text)?;
        assert!(reason.contains("schema_version"), "{reason}");
        assert!(reason.contains(version), "{reason}");
    }
    Ok(())
}

#[test]
fn badge_requires_an_explicit_string_schema_version() -> Result<()> {
    let cases = [
        r#"{"counts":{"unsuppressed_exposure_gaps":0}}"#,
        r#"{"schema_version":null,"counts":{"unsuppressed_exposure_gaps":0}}"#,
        r#"{"schema_version":0.5,"counts":{"unsuppressed_exposure_gaps":0}}"#,
        r#"{"schema_version":[],"counts":{"unsuppressed_exposure_gaps":0}}"#,
    ];
    for text in cases {
        let reason = malformed_reason(text)?;
        assert!(!reason.is_empty(), "{text}");
    }
    Ok(())
}

#[test]
fn native_count_cannot_bypass_badge_admission() -> Result<()> {
    let cases = [
        r#"{"new_unsuppressed":0,"schema_version":"9.9","counts":{"unsuppressed_exposure_gaps":1}}"#,
        r#"{"new_unsuppressed":0,"schema_version":"0.5","counts":{"unsuppressed_exposure_gaps":1}}"#,
        r#"{"new_unsuppressed":0,"schema_version":"0.6","counts":{"unsuppressed_exposure_gaps":0},"preview_skipped":[]}"#,
        r#"{"new_unsuppressed":0,"counts":{"unsuppressed_exposure_gaps":1}}"#,
        r#"{"new_unsuppressed":0,"schema_version":"0.5"}"#,
        r#"{"new_unsuppressed":0,"schema_version":null}"#,
        r#"{"new_unsuppressed":0,"counts":null}"#,
        r#"{"new_unsuppressed":0,"preview_skipped":["typescript"]}"#,
    ];
    for text in cases {
        let reason = malformed_reason(text)?;
        assert!(!reason.is_empty(), "{text}");
    }
    Ok(())
}

#[test]
fn duplicate_authority_fields_are_not_last_value_wins() -> Result<()> {
    let cases = [
        r#"{"new_unsuppressed":1,"new_unsuppressed":0}"#,
        r#"{"schema_version":"9.9","schema_version":"0.5","counts":{"unsuppressed_exposure_gaps":0}}"#,
        r#"{"schema_version":"0.5","counts":{"unsuppressed_exposure_gaps":1},"counts":{"unsuppressed_exposure_gaps":0}}"#,
        r#"{"schema_version":"0.5","counts":{"unsuppressed_exposure_gaps":1,"unsuppressed_exposure_gaps":0}}"#,
        r#"{"schema_version":"0.6","counts":{"unsuppressed_exposure_gaps":0},"preview_skipped":["perl"],"preview_skipped":[]}"#,
    ];
    for text in cases {
        let reason = malformed_reason(text)?;
        assert!(reason.contains("duplicate field"), "{reason}");
    }
    Ok(())
}

#[test]
fn malformed_counts_cannot_be_coerced_or_fall_through() -> Result<()> {
    for count in ["null", "-1", "0.5", "\"0\"", "true", "18446744073709551616"] {
        let receipts = [
            format!(r#"{{"new_unsuppressed":{count}}}"#),
            format!(
                r#"{{"schema_version":"0.5","counts":{{"unsuppressed_exposure_gaps":{count}}}}}"#
            ),
            format!(
                r#"{{"new_unsuppressed":{count},"schema_version":"0.5","counts":{{"unsuppressed_exposure_gaps":0}}}}"#
            ),
        ];
        for text in receipts {
            let reason = malformed_reason(&text)?;
            assert!(!reason.is_empty(), "{text}");
        }
    }
    Ok(())
}

#[test]
fn only_json_objects_with_complete_receipt_fields_are_admitted() -> Result<()> {
    let cases = [
        "not json",
        "null",
        "[]",
        "[0]",
        "{}",
        r#"{"schema_version":"0.5","counts":[0]}"#,
        r#"{"schema_version":"0.5","counts":{}}"#,
        r#"{"new_unsuppressed":0} trailing"#,
    ];
    for text in cases {
        let reason = malformed_reason(text)?;
        assert!(!reason.is_empty(), "{text}");
    }
    Ok(())
}

#[test]
fn badge_v06_requires_complete_preview_coverage() -> Result<()> {
    let cases = [
        r#"{"schema_version":"0.6","counts":{"unsuppressed_exposure_gaps":0}}"#,
        r#"{"schema_version":"0.6","counts":{"unsuppressed_exposure_gaps":0},"preview_skipped":["typescript"]}"#,
        r#"{"schema_version":"0.6","counts":{"unsuppressed_exposure_gaps":0},"preview_skipped":null}"#,
        r#"{"schema_version":"0.6","counts":{"unsuppressed_exposure_gaps":0},"preview_skipped":"perl"}"#,
        r#"{"schema_version":"0.6","counts":{"unsuppressed_exposure_gaps":0},"preview_skipped":[1]}"#,
        r#"{"schema_version":"0.5","counts":{"unsuppressed_exposure_gaps":0},"preview_skipped":["perl"]}"#,
    ];
    for text in cases {
        let reason = malformed_reason(text)?;
        assert!(!reason.is_empty(), "{text}");
    }
    Ok(())
}

#[test]
fn non_authoritative_metadata_does_not_change_admitted_counts() -> Result<()> {
    let cases = [
        r#"{"new_unsuppressed":7,"note":{"future_metadata":true}}"#,
        r#"{"schema_version":"0.5","kind":"ripr","status":"pass","counts":{"unsuppressed_exposure_gaps":7,"analyzed_findings":99},"note":[null,true]}"#,
        r#"{"schema_version":"0.6","kind":"ripr","scope":"diff","basis":"finding_exposure","status":"pass","counts":{"unsuppressed_exposure_gaps":7,"analyzed_findings":99},"preview_skipped":[],"warnings":[]}"#,
    ];
    for text in cases {
        let ToolGateDecisionState::Present(decision) = read_receipt(text)? else {
            bail!("compatible metadata rejected: {text}");
        };
        assert_eq!(decision.new_unsuppressed, 7);
    }
    Ok(())
}

#[test]
fn unavailable_files_stay_distinct_from_malformed_receipts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    assert!(matches!(
        read_tool_gate_decision(&temp.path().join("absent.json")),
        ToolGateDecisionState::Missing
    ));
    let directory = temp.path().join("directory.json");
    fs::create_dir(&directory)?;
    assert!(matches!(
        read_tool_gate_decision(&directory),
        ToolGateDecisionState::Malformed(_)
    ));
    Ok(())
}

#[test]
fn unknown_badge_is_missing_evidence_without_changing_requiredness() -> Result<()> {
    let mut config: Config = toml::from_str(include_str!("../../.ub-review.toml"))?;
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
    fs::write(
        temp.path().join("sensors/ripr/gate-decision.json"),
        r#"{"schema_version":"9.9","counts":{"unsuppressed_exposure_gaps":0}}"#,
    )?;
    let tool = config
        .tools
        .get("ripr")
        .ok_or_else(|| anyhow!("ripr tool missing"))?;
    let policy = tool
        .gate
        .clone()
        .ok_or_else(|| anyhow!("ripr gate policy missing"))?;
    assert_eq!(policy.max_new_unsuppressed, Some(0));
    let status =
        super::tool_status_artifact(temp.path(), &config, config.selected_profile()?, &plan);
    let mut entry = status
        .tools
        .into_iter()
        .find(|entry| entry.id == "ripr")
        .ok_or_else(|| anyhow!("ripr tool status missing"))?;
    for required in [false, true] {
        entry.required = required;
        let outcome =
            super::tool_gate_outcome_entry(temp.path(), tool, policy.clone(), Some(&entry));
        assert_eq!(outcome.required, required);
        assert_eq!(outcome.outcome, "missing_evidence");
        assert!(!outcome.evaluated);
        assert_eq!(outcome.metrics.new_unsuppressed, None);
        assert!(
            outcome.reason.contains("schema_version"),
            "{}",
            outcome.reason
        );
        assert!(
            outcome
                .source_artifacts
                .contains(&"sensors/ripr/gate-decision.json".to_owned())
        );
        let value = serde_json::to_value(&outcome)?;
        assert_eq!(value["outcome"], "missing_evidence");
        assert_eq!(value["required"], required);
        assert!(value["metrics"]["new_unsuppressed"].is_null());
    }
    Ok(())
}
