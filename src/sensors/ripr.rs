//! ripr gate-receipt detail: the second bounded pass and the pure
//! projection behind sensors/ripr/exposure-gaps.json (#347).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::*;

/// Path to a run input artifact (`<out>/input/<name>`) derived from a sensor
/// dir (`<out>/sensors/<id>`), for sensors that consume the run's own inputs.
/// Gap classes the detail artifact records: the classifications the
/// `[tools.ripr.gate]` threshold counts as exposure gaps.
pub(crate) const RIPR_GAP_CLASSIFICATIONS: &[&str] =
    &["weakly_exposed", "reachable_unrevealed", "no_static_path"];

pub(crate) const RIPR_GAP_DETAIL_CAP: usize = 200;

/// Project ripr's full `--format json` output into the bounded per-finding
/// detail artifact (#347): gap-class findings only, capped, each entry
/// carrying the id, classification, probe location/expression, suppression
/// state, threshold contribution, an artifact pointer, and the
/// reach/discriminate summaries a block diagnosis needs. Pure, so the
/// projection is testable without the ripr binary.
pub(crate) fn ripr_exposure_gap_details_from_value(value: &serde_json::Value) -> serde_json::Value {
    let clip = |text: &str, max: usize| -> String {
        if text.len() <= max {
            text.to_owned()
        } else {
            let mut end = max;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}...", &text[..end])
        }
    };
    let findings = value
        .get("findings")
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let gaps: Vec<&serde_json::Value> = findings
        .iter()
        .filter(|finding| {
            finding
                .get("classification")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|class| RIPR_GAP_CLASSIFICATIONS.contains(&class))
        })
        .collect();
    let total = gaps.len();
    let entries: Vec<serde_json::Value> = gaps
        .iter()
        .enumerate()
        .take(RIPR_GAP_DETAIL_CAP)
        .map(|(index, finding)| {
            let probe = finding.get("probe");
            let field = |outer: Option<&serde_json::Value>, key: &str| -> String {
                outer
                    .and_then(|value| value.get(key))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let stage = |key: &str| -> String {
                clip(
                    finding
                        .get("ripr")
                        .and_then(|value| value.get(key))
                        .and_then(|value| value.get("summary"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default(),
                    300,
                )
            };
            let line = probe
                .and_then(|value| value.get("line"))
                .and_then(serde_json::Value::as_u64);
            let suppression_state = ripr_suppression_state(finding);
            let threshold_contribution = ripr_threshold_contribution(suppression_state);
            let file = field(probe, "file");
            serde_json::json!({
                "id": field(Some(finding), "id"),
                "classification": field(Some(finding), "classification"),
                "exposure_gap_class": field(Some(finding), "classification"),
                "family": field(probe, "family"),
                "file": file,
                "path": file,
                "line": line,
                "range": {
                    "start_line": line,
                    "end_line": line,
                },
                "expression": clip(&field(probe, "expression"), 200),
                "suppression_state": suppression_state,
                "threshold_contribution": threshold_contribution,
                "artifact_pointer": format!("sensors/ripr/exposure-gaps.json#/entries/{index}"),
                "reach": stage("reach"),
                "discriminate": stage("discriminate"),
            })
        })
        .collect();
    serde_json::json!({
        "schema": RIPR_EXPOSURE_GAPS_SCHEMA,
        "status": "ok",
        "total_gap_findings": total,
        "truncated": total > RIPR_GAP_DETAIL_CAP,
        "entries": entries,
    })
}

fn ripr_suppression_state(finding: &serde_json::Value) -> &'static str {
    if finding
        .get("suppressed")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return "suppressed";
    }
    for key in ["suppression_state", "suppressionState"] {
        if finding
            .get(key)
            .and_then(serde_json::Value::as_str)
            .is_some_and(|value| value == "suppressed")
        {
            return "suppressed";
        }
    }
    if finding
        .get("suppression")
        .and_then(|value| value.get("state").or_else(|| value.get("status")))
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| value == "suppressed")
    {
        return "suppressed";
    }
    "unsuppressed"
}

fn ripr_threshold_contribution(suppression_state: &str) -> u64 {
    if suppression_state == "suppressed" {
        0
    } else {
        1
    }
}

/// Run the second, detail-producing ripr pass and persist
/// `sensors/ripr/exposure-gaps.json`. Infallible by design: any failure
/// writes a `detail_unavailable` artifact naming the error, so absence of
/// detail is itself receipted and the sensor status never changes.
pub(crate) fn write_ripr_exposure_gap_details(
    root: &Path,
    dir: &Path,
    command: &str,
    timeout_sec: u64,
) {
    let artifact_path = dir.join("exposure-gaps.json");
    let stdout_path = dir.join("exposure-gaps.stdout.tmp");
    let stderr_path = dir.join("exposure-gaps.stderr.tmp");
    let argv = vec![
        command.to_owned(),
        "check".to_owned(),
        "--root".to_owned(),
        root.display().to_string(),
        "--diff".to_owned(),
        sensor_run_input_path(dir, "diff.patch"),
        "--mode".to_owned(),
        "ready".to_owned(),
        "--format".to_owned(),
        "json".to_owned(),
    ];
    let detail = (|| -> Result<serde_json::Value> {
        let result = run_command_to_files(
            root,
            &argv,
            &BTreeMap::new(),
            timeout_sec,
            &stdout_path,
            &stderr_path,
        )?;
        if result.timed_out || !result.success {
            bail!(
                "detail pass {}: {}",
                if result.timed_out {
                    "timed out"
                } else {
                    "failed"
                },
                result.reason
            );
        }
        let value: serde_json::Value =
            serde_json::from_slice(&fs::read(&stdout_path).with_context(|| "read detail stdout")?)
                .with_context(|| "parse ripr --format json output")?;
        Ok(ripr_exposure_gap_details_from_value(&value))
    })()
    .unwrap_or_else(|err| {
        serde_json::json!({
            "schema": RIPR_EXPOSURE_GAPS_SCHEMA,
            "status": "detail_unavailable",
            "error": format!("{err:#}"),
            "total_gap_findings": 0,
            "truncated": false,
            "entries": [],
        })
    });
    let _ = fs::remove_file(&stdout_path);
    let _ = fs::remove_file(&stderr_path);
    match serde_json::to_vec_pretty(&detail) {
        Ok(bytes) => {
            if let Err(err) = fs::write(&artifact_path, bytes) {
                eprintln!("ripr exposure-gap detail write failed (tolerated): {err:#}");
            }
        }
        Err(err) => {
            eprintln!("ripr exposure-gap detail serialize failed (tolerated): {err:#}");
        }
    }
}

#[cfg(test)]
mod tests {

    use anyhow::{Context as _, Result};

    #[test]
    fn ripr_exposure_gap_detail_pass_failure_writes_detail_unavailable() -> Result<()> {
        // The detail pass is infallible by design: a failing second pass
        // (here: a diff path that does not exist) must write a
        // detail_unavailable artifact naming the error, never alter the
        // sensor outcome, and never leave tmp capture files behind.
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join("sensors/ripr");
        std::fs::create_dir_all(&dir)?;
        super::write_ripr_exposure_gap_details(
            temp.path(),
            &dir,
            "ub-review-test-missing-ripr",
            30,
        );
        let detail: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("exposure-gaps.json"))?)?;
        assert_eq!(detail["schema"], "ub-review.ripr_exposure_gaps.v1");
        assert_eq!(detail["status"], "detail_unavailable");
        assert!(
            detail["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "error names the failure: {detail}"
        );
        assert_eq!(detail["total_gap_findings"], 0);
        assert!(!dir.join("exposure-gaps.stdout.tmp").exists());
        assert!(!dir.join("exposure-gaps.stderr.tmp").exists());
        Ok(())
    }

    #[test]
    fn ripr_exposure_gap_details_project_filter_cap_and_unavailable_shape() -> Result<()> {
        assert_eq!(super::ripr_threshold_contribution("unsuppressed"), 1);
        assert_eq!(super::ripr_threshold_contribution("suppressed"), 0);

        let finding = |id: &str, class: &str| {
            let mut value = serde_json::json!({
                "id": id,
                "classification": class,
                "probe": {
                    "family": "call_deletion",
                    "file": "./src/config.rs",
                    "line": 40,
                    "expression": "pub(crate) struct IssuesConfig {",
                },
                "ripr": {
                    "reach": {"summary": "Related tests appear to reach changed owner"},
                    "discriminate": {"summary": "Only relational oracle found"},
                },
            });
            if id.contains("suppressed") {
                value["suppressed"] = serde_json::Value::Bool(true);
            }
            value
        };
        let value = serde_json::json!({
            "findings": [
                finding("probe:a:1:call_deletion", "weakly_exposed"),
                finding("probe:b:2:side_effect", "exposed"),
                finding("probe:c:3:field_construction", "no_static_path"),
                finding("probe:suppressed:4:error_path", "reachable_unrevealed"),
            ],
        });
        let detail = super::ripr_exposure_gap_details_from_value(&value);
        assert_eq!(detail["schema"], "ub-review.ripr_exposure_gaps.v1");
        assert_eq!(detail["status"], "ok");
        // `exposed` is not a gap class and is filtered out.
        assert_eq!(detail["total_gap_findings"], 3);
        assert_eq!(detail["truncated"], false);
        let entries = detail["entries"].as_array().context("entries")?;
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0]["id"], "probe:a:1:call_deletion");
        assert_eq!(entries[0]["classification"], "weakly_exposed");
        assert_eq!(entries[0]["exposure_gap_class"], "weakly_exposed");
        assert_eq!(entries[0]["family"], "call_deletion");
        assert_eq!(entries[0]["file"], "./src/config.rs");
        assert_eq!(entries[0]["path"], "./src/config.rs");
        assert_eq!(entries[0]["line"], 40);
        assert_eq!(entries[0]["range"]["start_line"], 40);
        assert_eq!(entries[0]["range"]["end_line"], 40);
        assert_eq!(entries[0]["expression"], "pub(crate) struct IssuesConfig {");
        assert_eq!(entries[0]["suppression_state"], "unsuppressed");
        assert_eq!(entries[0]["threshold_contribution"], 1);
        assert_eq!(
            entries[0]["artifact_pointer"],
            "sensors/ripr/exposure-gaps.json#/entries/0"
        );
        assert_eq!(entries[2]["suppression_state"], "suppressed");
        assert_eq!(entries[2]["threshold_contribution"], 0);
        assert_eq!(
            entries[2]["artifact_pointer"],
            "sensors/ripr/exposure-gaps.json#/entries/2"
        );
        assert!(
            entries[0]["reach"]
                .as_str()
                .is_some_and(|s| s.contains("reach changed owner"))
        );
        assert!(
            entries[0]["discriminate"]
                .as_str()
                .is_some_and(|s| s.contains("relational oracle"))
        );

        // The cap bounds the artifact and records the truncation.
        let many: Vec<serde_json::Value> = (0..250)
            .map(|i| finding(&format!("probe:x:{i}:call_deletion"), "no_static_path"))
            .collect();
        let capped =
            super::ripr_exposure_gap_details_from_value(&serde_json::json!({"findings": many}));
        assert_eq!(capped["total_gap_findings"], 250);
        assert_eq!(capped["truncated"], true);
        assert_eq!(
            capped["entries"]
                .as_array()
                .context("capped entries")?
                .len(),
            super::RIPR_GAP_DETAIL_CAP
        );

        // Missing findings array projects to an empty ok artifact, not an
        // error: an empty diff legitimately has zero findings.
        let empty = super::ripr_exposure_gap_details_from_value(&serde_json::json!({}));
        assert_eq!(empty["status"], "ok");
        assert_eq!(empty["total_gap_findings"], 0);

        // Long fields clip with an ellipsis: expression at 200 bytes, stage
        // summaries at 300; at exactly the limit nothing is clipped.
        let mut long = finding("probe:long:1:call_deletion", "weakly_exposed");
        long["probe"]["expression"] = serde_json::Value::String("x".repeat(201));
        long["ripr"]["reach"]["summary"] = serde_json::Value::String("r".repeat(301));
        let clipped =
            super::ripr_exposure_gap_details_from_value(&serde_json::json!({"findings": [long]}));
        let entry = &clipped["entries"][0];
        assert_eq!(
            entry["expression"].as_str().context("expression")?,
            format!("{}...", "x".repeat(200))
        );
        assert_eq!(
            entry["reach"].as_str().context("reach")?,
            format!("{}...", "r".repeat(300))
        );
        let mut exact = finding("probe:exact:1:call_deletion", "weakly_exposed");
        exact["probe"]["expression"] = serde_json::Value::String("y".repeat(200));
        let kept =
            super::ripr_exposure_gap_details_from_value(&serde_json::json!({"findings": [exact]}));
        assert_eq!(
            kept["entries"][0]["expression"].as_str().context("kept")?,
            "y".repeat(200)
        );
        Ok(())
    }
}
