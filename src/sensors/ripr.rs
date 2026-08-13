//! ripr gate-receipt detail: the second bounded pass and the pure
//! projection behind sensors/ripr/exposure-gaps.json (#347).

use std::collections::{BTreeMap, BTreeSet};
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

/// Project pinned ripr 0.10.0's full `--format json` output into bounded raw,
/// pre-policy gap detail. The badge receipt remains the sole source of the
/// suppression partition and strict-zero decision (#873).
pub(crate) fn ripr_exposure_gap_details_from_value(
    value: &serde_json::Value,
) -> Result<serde_json::Value> {
    if value
        .get("schema_version")
        .and_then(serde_json::Value::as_str)
        != Some("0.2")
        || value.get("tool").and_then(serde_json::Value::as_str) != Some("ripr")
        || value.get("mode").and_then(serde_json::Value::as_str) != Some("ready")
    {
        bail!("ripr detail envelope is incompatible with pinned ripr 0.10.0");
    }
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
        .context("ripr detail omitted findings array")?;
    let summary_count = value
        .get("summary")
        .and_then(|summary| summary.get("findings"))
        .and_then(serde_json::Value::as_u64)
        .context("ripr detail omitted summary.findings")?;
    if summary_count != findings.len() as u64 {
        bail!(
            "ripr detail summary.findings={summary_count} does not match findings length {}",
            findings.len()
        );
    }
    let mut finding_ids = BTreeSet::new();
    for (index, finding) in findings.iter().enumerate() {
        let object = finding
            .as_object()
            .with_context(|| format!("ripr finding[{index}] must be an object"))?;
        let id = object
            .get("id")
            .and_then(serde_json::Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .with_context(|| format!("ripr finding[{index}] omitted id"))?;
        if !finding_ids.insert(id) {
            bail!("ripr detail contains duplicate finding id `{id}`");
        }
        object
            .get("classification")
            .and_then(serde_json::Value::as_str)
            .with_context(|| format!("ripr finding `{id}` omitted classification"))?;
        let probe = object
            .get("probe")
            .and_then(serde_json::Value::as_object)
            .with_context(|| format!("ripr finding `{id}` omitted probe object"))?;
        probe
            .get("file")
            .and_then(serde_json::Value::as_str)
            .filter(|path| !path.trim().is_empty())
            .with_context(|| format!("ripr finding `{id}` omitted probe.file"))?;
        probe
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .filter(|line| *line > 0)
            .with_context(|| format!("ripr finding `{id}` has invalid probe.line"))?;
    }
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
                "artifact_pointer": format!("sensors/ripr/exposure-gaps.json#/entries/{index}"),
                "reach": stage("reach"),
                "discriminate": stage("discriminate"),
            })
        })
        .collect();
    Ok(serde_json::json!({
        "schema": RIPR_EXPOSURE_GAPS_V2_SCHEMA,
        "status": "ok",
        "semantics": "raw_pre_policy",
        "policy_authority": "sensors/ripr/gate-decision.json",
        "source": {
            "tool": "ripr",
            "schema_version": "0.2",
            "mode": "ready",
        },
        "total_raw_findings": findings.len(),
        "total_raw_gap_findings": total,
        "entry_cap": RIPR_GAP_DETAIL_CAP,
        "truncated": total > RIPR_GAP_DETAIL_CAP,
        "entries": entries,
    }))
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
        let result = run_sensor_command_to_files(
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
        ripr_exposure_gap_details_from_value(&value)
    })()
    .unwrap_or_else(|err| {
        serde_json::json!({
            "schema": RIPR_EXPOSURE_GAPS_V2_SCHEMA,
            "status": "detail_unavailable",
            "error": format!("{err:#}"),
            "semantics": "raw_pre_policy",
            "policy_authority": "sensors/ripr/gate-decision.json",
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
        assert_eq!(detail["schema"], "ub-review.ripr_exposure_gaps.v2");
        assert_eq!(detail["status"], "detail_unavailable");
        assert!(
            detail["error"]
                .as_str()
                .is_some_and(|error| !error.is_empty()),
            "error names the failure: {detail}"
        );
        assert!(detail.get("total_raw_gap_findings").is_none());
        assert!(detail.get("entries").is_none());
        assert!(!dir.join("exposure-gaps.stdout.tmp").exists());
        assert!(!dir.join("exposure-gaps.stderr.tmp").exists());
        Ok(())
    }

    #[test]
    fn ripr_exposure_gap_details_project_filter_cap_and_unavailable_shape() -> Result<()> {
        let finding = |id: &str, class: &str| {
            serde_json::json!({
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
            })
        };
        let value = serde_json::json!({
            "schema_version": "0.2",
            "tool": "ripr",
            "mode": "ready",
            "summary": {"findings": 4},
            "findings": [
                finding("probe:a:1:call_deletion", "weakly_exposed"),
                finding("probe:b:2:side_effect", "exposed"),
                finding("probe:c:3:field_construction", "no_static_path"),
                finding("probe:suppressed:4:error_path", "reachable_unrevealed"),
            ],
        });
        let detail = super::ripr_exposure_gap_details_from_value(&value)?;
        assert_eq!(
            detail,
            super::ripr_exposure_gap_details_from_value(&value)?,
            "stable raw IDs and entry order must reproduce byte-equivalent values"
        );
        assert_eq!(detail["schema"], "ub-review.ripr_exposure_gaps.v2");
        assert_eq!(detail["status"], "ok");
        assert_eq!(detail["semantics"], "raw_pre_policy");
        assert_eq!(
            detail["policy_authority"],
            "sensors/ripr/gate-decision.json"
        );
        assert_eq!(detail["source"]["schema_version"], "0.2");
        // `exposed` is not a gap class and is filtered out.
        assert_eq!(detail["total_raw_gap_findings"], 3);
        assert_eq!(detail["entry_cap"], super::RIPR_GAP_DETAIL_CAP);
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
        assert!(entries[0].get("suppression_state").is_none());
        assert!(entries[0].get("threshold_contribution").is_none());
        assert_eq!(
            entries[0]["artifact_pointer"],
            "sensors/ripr/exposure-gaps.json#/entries/0"
        );
        assert!(entries[2].get("suppression_state").is_none());
        assert!(entries[2].get("threshold_contribution").is_none());
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

        // Exact boundary fixtures pin 200 as complete, 201 as truncated, and
        // a larger input as still bounded to the presentation cap.
        for total in [200, 201, 250] {
            let many: Vec<serde_json::Value> = (0..total)
                .map(|i| finding(&format!("probe:x:{i}:call_deletion"), "no_static_path"))
                .collect();
            let capped = super::ripr_exposure_gap_details_from_value(&serde_json::json!({
                "schema_version": "0.2",
                "tool": "ripr",
                "mode": "ready",
                "summary": {"findings": total},
                "findings": many,
            }))?;
            assert_eq!(capped["total_raw_gap_findings"], total);
            assert_eq!(capped["truncated"], total > super::RIPR_GAP_DETAIL_CAP);
            assert_eq!(
                capped["entries"]
                    .as_array()
                    .context("capped entries")?
                    .len(),
                total.min(super::RIPR_GAP_DETAIL_CAP)
            );
        }

        let empty = super::ripr_exposure_gap_details_from_value(&serde_json::json!({
            "schema_version": "0.2",
            "tool": "ripr",
            "mode": "ready",
            "summary": {"findings": 0},
            "findings": [],
        }))?;
        assert_eq!(empty["status"], "ok");
        assert_eq!(empty["total_raw_findings"], 0);
        assert_eq!(empty["total_raw_gap_findings"], 0);

        // Long fields clip with an ellipsis: expression at 200 bytes, stage
        // summaries at 300; at exactly the limit nothing is clipped.
        let mut long = finding("probe:long:1:call_deletion", "weakly_exposed");
        long["probe"]["expression"] = serde_json::Value::String("x".repeat(201));
        long["ripr"]["reach"]["summary"] = serde_json::Value::String("r".repeat(301));
        let clipped = super::ripr_exposure_gap_details_from_value(&serde_json::json!({
            "schema_version": "0.2",
            "tool": "ripr",
            "mode": "ready",
            "summary": {"findings": 1},
            "findings": [long],
        }))?;
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
        let kept = super::ripr_exposure_gap_details_from_value(&serde_json::json!({
            "schema_version": "0.2",
            "tool": "ripr",
            "mode": "ready",
            "summary": {"findings": 1},
            "findings": [exact],
        }))?;
        assert_eq!(
            kept["entries"][0]["expression"].as_str().context("kept")?,
            "y".repeat(200)
        );
        Ok(())
    }

    #[test]
    fn ripr_exposure_gap_details_reject_malformed_pinned_input() -> Result<()> {
        let valid = serde_json::json!({
            "schema_version": "0.2",
            "tool": "ripr",
            "mode": "ready",
            "summary": {"findings": 1},
            "findings": [{
                "id": "probe:src_x.rs:return_value:12345678",
                "classification": "no_static_path",
                "probe": {"file": "src/x.rs", "line": 1},
            }],
        });
        for (name, malformed) in [
            ("version", serde_json::json!({"schema_version": "0.1"})),
            (
                "findings",
                serde_json::json!({
                    "schema_version": "0.2", "tool": "ripr", "mode": "ready",
                    "summary": {"findings": 0}
                }),
            ),
            ("summary", {
                let mut value = valid.clone();
                value["summary"]["findings"] = serde_json::json!(2);
                value
            }),
            ("duplicate-id", {
                let mut value = valid.clone();
                value["summary"]["findings"] = serde_json::json!(2);
                value["findings"] =
                    serde_json::json!([value["findings"][0].clone(), value["findings"][0].clone()]);
                value
            }),
            ("whitespace-id", {
                let mut value = valid.clone();
                value["findings"][0]["id"] = serde_json::json!("   ");
                value
            }),
            ("classification", {
                let mut value = valid.clone();
                value["findings"][0]
                    .as_object_mut()
                    .context("fixture finding object")?
                    .remove("classification");
                value
            }),
            ("path", {
                let mut value = valid.clone();
                value["findings"][0]["probe"]["file"] = serde_json::json!("");
                value
            }),
            ("line", {
                let mut value = valid.clone();
                value["findings"][0]["probe"]["line"] = serde_json::json!(0);
                value
            }),
        ] {
            let error = super::ripr_exposure_gap_details_from_value(&malformed)
                .err()
                .with_context(|| format!("{name} unexpectedly accepted"))?;
            assert!(!format!("{error:#}").is_empty());
        }

        let mut after_cap = valid.clone();
        let first = valid["findings"][0].clone();
        after_cap["findings"] = serde_json::Value::Array(
            (0..=super::RIPR_GAP_DETAIL_CAP)
                .map(|index| {
                    let mut finding = valid["findings"][0].clone();
                    finding["id"] = serde_json::json!(format!("finding-{index}"));
                    finding
                })
                .chain(std::iter::once({
                    let mut malformed = first;
                    malformed["id"] = serde_json::json!("finding-after-cap");
                    malformed["probe"]["line"] = serde_json::json!(0);
                    malformed
                }))
                .collect(),
        );
        after_cap["summary"]["findings"] = serde_json::json!(202);
        let error = super::ripr_exposure_gap_details_from_value(&after_cap)
            .err()
            .context("malformed finding after the presentation cap was accepted")?;
        assert!(format!("{error:#}").contains("finding-after-cap"));
        Ok(())
    }
}
