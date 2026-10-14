//! Coverage sensor receipts: status sidecars and the lcov summary.

use std::fs;
use std::path::Path;

use anyhow::Result;

use crate::*;

pub(crate) fn write_coverage_status_receipt(
    dir: &Path,
    fields: SensorStatusWrite<'_>,
) -> Result<()> {
    let summary = coverage_summary_receipt(dir);
    let changed_lines = serde_json::json!({
        "schema": COVERAGE_CHANGED_LINES_SCHEMA,
        "status": "not_collected",
        "reason": "changed-line coverage is not computed by the local coverage sensor yet",
        "execution_surface_only": true,
        "correctness_claim": false,
        "source_artifacts": ["sensors/coverage/lcov.info"],
    });
    let upload = serde_json::json!({
        "schema": COVERAGE_UPLOAD_SCHEMA,
        "status": "workflow_owned",
        "reason": "Codecov upload is performed by the coverage workflow, not this local sensor",
        "execution_surface_only": true,
        "correctness_claim": false,
        "source_artifacts": [],
    });
    fs::write(
        dir.join("coverage-summary.json"),
        serde_json::to_vec_pretty(&summary)?,
    )?;
    fs::write(
        dir.join("changed-lines.json"),
        serde_json::to_vec_pretty(&changed_lines)?,
    )?;
    fs::write(dir.join("upload.json"), serde_json::to_vec_pretty(&upload)?)?;
    let value = serde_json::json!({
        "schema": COVERAGE_STATUS_SCHEMA,
        "status": fields.status,
        "reason": fields.reason,
        "execution_surface_only": true,
        "correctness_claim": false,
        "lcov": {
            "path": "sensors/coverage/lcov.info",
            "present": summary["lcov"]["present"],
        },
        "summary": {
            "path": "sensors/coverage/coverage-summary.json",
            "status": summary["status"],
        },
        "changed_lines": {
            "path": "sensors/coverage/changed-lines.json",
            "status": changed_lines["status"],
        },
        "upload": {
            "path": "sensors/coverage/upload.json",
            "status": upload["status"],
        },
    });
    fs::write(dir.join("status.json"), serde_json::to_vec_pretty(&value)?)?;
    Ok(())
}

/// Report projection of the emitted telemetry receipt. Process success is
/// independent of whether LCOV totals are usable; this never changes the gate.
#[derive(Clone, Debug, serde::Serialize)]
pub(crate) struct CoverageSummaryDigest {
    pub(crate) status: String,
    pub(crate) reason: String,
    pub(crate) receipt_path: String,
}

impl CoverageSummaryDigest {
    pub(crate) fn excerpt(&self) -> String {
        format!(
            "coverage summary `{}`: {} (receipt: `{}`; execution-surface telemetry only, not a correctness claim)",
            self.status, self.reason, self.receipt_path
        )
    }
}

pub(crate) fn read_coverage_summary_digest(out: &Path) -> CoverageSummaryDigest {
    let receipt_path = "sensors/coverage/coverage-summary.json";
    let summary = fs::read(out.join(receipt_path))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let usable = summary.as_ref().filter(|value| {
        value["schema"] == COVERAGE_SUMMARY_SCHEMA
            && value["execution_surface_only"] == true
            && value["correctness_claim"] == false
            && matches!(
                value["status"].as_str(),
                Some("collected" | "unknown" | "not_collected")
            )
            && value["reason"]
                .as_str()
                .is_some_and(|reason| !reason.trim().is_empty())
            && value["lcov"]["path"]
                .as_str()
                .is_some_and(|path| !path.trim().is_empty())
            && value["lcov"]["present"].as_bool().is_some()
            && ["line_totals", "function_totals"].iter().all(|totals| {
                value[totals]["found"].as_u64().is_some() && value[totals]["hit"].as_u64().is_some()
            })
    });
    let (status, reason) = match usable {
        Some(value) => (
            value["status"].as_str().unwrap_or("unknown").to_owned(),
            value["reason"].as_str().unwrap_or_default().to_owned(),
        ),
        None => (
            "unknown".to_owned(),
            "coverage summary receipt absent, unreadable, or malformed".to_owned(),
        ),
    };
    CoverageSummaryDigest {
        status,
        reason,
        receipt_path: receipt_path.to_owned(),
    }
}

pub(crate) fn coverage_summary_receipt(dir: &Path) -> serde_json::Value {
    let lcov_path = dir.join("lcov.info");
    let mut lines_found = 0u64;
    let mut lines_hit = 0u64;
    let mut functions_found = 0u64;
    let mut functions_hit = 0u64;
    let present = lcov_path.is_file();
    let mut status = "not_collected";
    let mut reason = "lcov.info not present";
    let text = if present {
        match fs::read_to_string(&lcov_path) {
            Ok(text) => Some(text),
            Err(_) => {
                status = "unknown";
                reason = "lcov.info unreadable";
                None
            }
        }
    } else {
        None
    };
    if let Some(text) = text {
        let mut source_record_open = false;
        let mut unterminated_record = false;
        for line in text.lines() {
            if line.starts_with("SF:") {
                unterminated_record |= source_record_open;
                source_record_open = true;
            } else if line == "end_of_record" {
                source_record_open = false;
            }
        }
        if source_record_open || unterminated_record {
            status = "unknown";
            reason = "lcov.info contains an unterminated source record";
        } else {
            status = "collected";
            reason = "lcov.info parsed";
        }
        for line in text.lines() {
            if status != "collected" {
                break;
            }
            let field = if let Some(value) = line.strip_prefix("LF:") {
                Some((&mut lines_found, value))
            } else if let Some(value) = line.strip_prefix("LH:") {
                Some((&mut lines_hit, value))
            } else if let Some(value) = line.strip_prefix("FNF:") {
                Some((&mut functions_found, value))
            } else {
                line.strip_prefix("FNH:")
                    .map(|value| (&mut functions_hit, value))
            };
            if let Some((total, value)) = field {
                if let Some(sum) = total.checked_add(parse_lcov_count(value)) {
                    *total = sum;
                } else {
                    status = "unknown";
                    reason = "lcov.info coverage totals overflow";
                    lines_found = 0;
                    lines_hit = 0;
                    functions_found = 0;
                    functions_hit = 0;
                    break;
                }
            }
        }
    }
    serde_json::json!({
        "schema": COVERAGE_SUMMARY_SCHEMA,
        "status": status,
        "reason": reason,
        "execution_surface_only": true,
        "correctness_claim": false,
        "lcov": {
            "path": "sensors/coverage/lcov.info",
            "present": present,
        },
        "line_totals": {
            "found": lines_found,
            "hit": lines_hit,
        },
        "function_totals": {
            "found": functions_found,
            "hit": functions_hit,
        },
    })
}

#[cfg(test)]
mod tests {

    use anyhow::Result;

    use crate::*;

    #[test]
    fn coverage_summary_survives_unreadable_process_receipts() -> Result<()> {
        use crate::tests::{sensor_plan, test_diff, test_plan};

        for phase in [SensorPhase::Fast, SensorPhase::Late] {
            for malformed in [false, true] {
                let temp = tempfile::tempdir()?;
                let out = temp.path();
                let mut sensor = sensor_plan("coverage", "cargo", true);
                sensor.phase = phase;
                let plan = test_plan(vec![sensor.clone()]);
                write_sensor_status(
                    out,
                    &sensor,
                    SensorStatusWrite {
                        status: "ok",
                        argv: &[],
                        duration_ms: 0,
                        reason: "completed",
                        exit_code: Some(0),
                        timed_out: false,
                    },
                )?;
                let path = out.join("sensors/coverage/ub-review-sensor-status.json");
                if malformed {
                    fs::write(&path, "{")?;
                } else {
                    fs::remove_file(&path)?;
                }
                let report = render_summary(out, &plan, &test_diff())?;
                assert!(
                    report.contains("lcov.info not present"),
                    "{phase:?}/{malformed}: {report}"
                );
                assert_eq!(
                    report.matches("coverage summary `not_collected`").count(),
                    1
                );
                assert!(!report.contains("No planned sensor evidence is currently missing."));
                assert!(!report.contains("coverage ran;"));
                if matches!(phase, SensorPhase::Late) {
                    assert!(report.contains("coverage scheduled in the late evidence phase"));
                } else {
                    assert!(report.contains("coverage receipt absent"));
                }
                let late = late_sensor_receipt_digests(out, &["coverage".to_owned()]);
                assert_eq!(late[0].status, "receipt-absent");
                assert!(reporter_prompt(&[], &late).contains("lcov.info not present"));
                let continuation = lane_continuation_prompt(
                    "tests",
                    "reviewer",
                    "prior",
                    "question",
                    "summary",
                    &[],
                    &[late[0].excerpt()],
                );
                assert!(continuation.contains("lcov.info not present"));

                fs::remove_file(out.join("sensors/coverage/coverage-summary.json"))?;
                let unstarted = render_summary(out, &plan, &test_diff())?;
                assert!(!unstarted.contains("coverage summary"));
                assert_eq!(
                    unstarted.contains("No planned sensor evidence is currently missing."),
                    matches!(phase, SensorPhase::Late)
                );
                sensor.run = false;
                sensor.required = false;
                fs::write(
                    out.join("sensors/coverage/coverage-summary.json"),
                    serde_json::to_vec(&coverage_summary_receipt(&out.join("sensors/coverage")))?,
                )?;
                let disabled = render_summary(out, &test_plan(vec![sensor]), &test_diff())?;
                assert!(!disabled.contains("coverage summary"));
                assert!(disabled.contains("No planned sensor evidence is currently missing."));
            }
        }
        Ok(())
    }

    #[test]
    fn coverage_summary_requires_essential_structure_and_types() -> Result<()> {
        use crate::tests::{sensor_plan, test_diff, test_plan};

        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let sensor = sensor_plan("coverage", "cargo", true);
        write_sensor_status(
            out,
            &sensor,
            SensorStatusWrite {
                status: "ok",
                argv: &[],
                duration_ms: 0,
                reason: "completed",
                exit_code: Some(0),
                timed_out: false,
            },
        )?;
        let path = out.join("sensors/coverage/coverage-summary.json");
        let mut valid = coverage_summary_receipt(&out.join("sensors/coverage"));
        valid["status"] = serde_json::json!("collected");
        valid["reason"] = serde_json::json!("lcov.info parsed");
        valid["lcov"]["present"] = serde_json::json!(true);
        let metadata = serde_json::json!({"schema": COVERAGE_SUMMARY_SCHEMA, "execution_surface_only": true, "correctness_claim": false, "status": "collected", "reason": "lcov.info parsed"});
        let mut variants = vec![metadata];
        for field in ["lcov", "line_totals", "function_totals"] {
            let mut missing = valid.clone();
            missing
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("expected object"))?
                .remove(field);
            variants.push(missing);
            for wrong in [
                serde_json::Value::Null,
                serde_json::json!("object"),
                serde_json::json!([]),
            ] {
                let mut value = valid.clone();
                value[field] = wrong;
                variants.push(value);
            }
        }
        for (object, field, wrong) in [
            ("lcov", "path", serde_json::json!(7)),
            ("lcov", "path", serde_json::json!(" ")),
            ("lcov", "present", serde_json::json!("true")),
            ("line_totals", "found", serde_json::json!("0")),
            ("line_totals", "hit", serde_json::json!(-1)),
            ("function_totals", "found", serde_json::json!(1.5)),
            ("function_totals", "hit", serde_json::Value::Null),
        ] {
            let mut value = valid.clone();
            value[object][field] = wrong;
            variants.push(value);
        }
        for (index, value) in variants.into_iter().enumerate() {
            fs::write(&path, serde_json::to_vec(&value)?)?;
            let digest = read_coverage_summary_digest(out);
            assert_eq!(digest.status, "unknown", "variant {index}: {value}");
            let report = render_summary(out, &test_plan(vec![sensor.clone()]), &test_diff())?;
            assert!(report.contains(&digest.reason));
            assert!(!report.contains("coverage summary `collected`"));
            let late = late_sensor_receipt_digests(out, &["coverage".to_owned()]);
            assert_eq!(late[0].status, "ok");
            assert_eq!(late[0].reason, "completed");
            assert!(reporter_prompt(&[], &late).contains(&digest.reason));
            assert!(
                lane_continuation_prompt(
                    "tests",
                    "reviewer",
                    "prior",
                    "question",
                    "summary",
                    &[],
                    &[late[0].excerpt()]
                )
                .contains(&digest.reason)
            );
        }
        valid["line_totals"]["found"] = serde_json::json!(u64::MAX);
        fs::write(path, serde_json::to_vec(&valid)?)?;
        assert_eq!(read_coverage_summary_digest(out).status, "collected");
        Ok(())
    }

    #[test]
    fn coverage_receipts_reach_report_consumers_without_losing_diagnostics() -> Result<()> {
        use crate::tests::{sensor_plan, test_diff, test_plan, test_run_args, test_terminal_state};

        let cases: [(&str, Option<&[u8]>, &str, &str); 5] = [
            ("missing", None, "not_collected", "lcov.info not present"),
            ("truncated", Some(b"SF:src/a.rs\nLF:3\n"), "unknown", "lcov.info contains an unterminated source record"),
            ("utf8", Some(b"\xff"), "unknown", "lcov.info unreadable"),
            ("maximum", Some(b"SF:src/a.rs\nLF:18446744073709551615\nLH:18446744073709551615\nFNF:18446744073709551615\nFNH:18446744073709551615\nend_of_record\n"), "collected", "lcov.info parsed"),
            ("overflow", Some(b"SF:src/a.rs\nLF:18446744073709551615\nend_of_record\nSF:src/b.rs\nLF:1\nend_of_record\n"), "unknown", "lcov.info coverage totals overflow"),
        ];
        for (case, bytes, summary_status, summary_reason) in cases {
            for process_status in ["ok", "failed", "timed_out", "missing"] {
                let temp = tempfile::tempdir()?;
                let out = temp.path();
                let dir = out.join("sensors/coverage");
                fs::create_dir_all(&dir)?;
                if let Some(bytes) = bytes {
                    fs::write(dir.join("lcov.info"), bytes)?;
                }
                let mut sensor = sensor_plan("coverage", "cargo", true);
                sensor.required = false;
                sensor.phase = SensorPhase::Late;
                let plan = test_plan(vec![sensor.clone()]);
                write_sensor_status(
                    out,
                    &sensor,
                    SensorStatusWrite {
                        status: process_status,
                        argv: &["cargo".to_owned(), "llvm-cov".to_owned()],
                        duration_ms: 17,
                        reason: "process diagnostic",
                        exit_code: if process_status == "ok" {
                            Some(0)
                        } else {
                            Some(1)
                        },
                        timed_out: process_status == "timed_out",
                    },
                )?;
                let summary: serde_json::Value =
                    serde_json::from_slice(&fs::read(dir.join("coverage-summary.json"))?)?;
                assert_eq!(summary["status"], summary_status, "{case}/{process_status}");
                assert_eq!(summary["reason"], summary_reason);
                let process = read_sensor_receipt(&dir.join("ub-review-sensor-status.json"))
                    .ok_or_else(|| anyhow::anyhow!("missing process receipt"))?;
                assert_eq!(process.status, process_status);
                assert_eq!(process.reason, "process diagnostic");
                assert_eq!(process.timed_out, process_status == "timed_out");
                let issues = collect_sensor_evidence_issues(out, &plan);
                assert_eq!(issues.is_empty(), process_status == "ok");
                let mut args = test_run_args(out.to_path_buf());
                args.mode = RunMode::IntelligentCi;
                let gate = build_gate_outcome(GateOutcomeInput {
                    args: &args,
                    config: &Config::default(),
                    tool_gate_outcomes: &[],
                    plan: &plan,
                    terminal_state: &test_terminal_state("reviewed-no-findings"),
                    proof_requests: &[],
                    proof_receipts: &[],
                    missing_or_failed_sensor_evidence: &issues,
                    missing_or_failed_model_evidence: &[],
                    reporter_gate: ReporterGateInput::Absent,
                    planner_portfolio: None,
                    revision_digest: None,
                });
                assert_eq!(
                    gate.conclusion, "pass",
                    "advisory coverage cannot change gate policy"
                );
                assert_eq!(
                    gate.evidence_gaps_advisory,
                    usize::from(process_status != "ok")
                );
                let digests = late_sensor_receipt_digests(out, &["coverage".to_owned()]);
                assert_eq!(digests[0].status, process_status);
                assert_eq!(digests[0].reason, "process diagnostic");
                let projected = serde_json::to_value(&digests[0])?;
                assert_eq!(projected["coverage_summary"]["status"], summary_status);
                assert_eq!(projected["coverage_summary"]["reason"], summary_reason);
                let prompt = reporter_prompt(&[], &digests);
                assert!(
                    prompt.contains(summary_reason),
                    "reporter drops {case}/{process_status}: {prompt}"
                );
                assert!(digests[0].excerpt().contains(summary_reason));
                let report = render_summary(out, &plan, &test_diff())?;
                assert!(
                    report.contains(summary_reason),
                    "summary drops {case}/{process_status}: {report}"
                );
                assert!(report.contains(&format!("coverage summary `{summary_status}`")));
                assert!(!report.contains("coverage ran; sensor packet available."));
            }
        }
        Ok(())
    }

    #[test]
    fn coverage_report_consumers_fail_closed_on_unusable_summary_sidecars() -> Result<()> {
        use crate::tests::{sensor_plan, test_diff, test_plan};
        let temp = tempfile::tempdir()?;
        let out = temp.path();
        let sensor = sensor_plan("coverage", "cargo", true);
        write_sensor_status(
            out,
            &sensor,
            SensorStatusWrite {
                status: "ok",
                argv: &[],
                duration_ms: 0,
                reason: "completed",
                exit_code: Some(0),
                timed_out: false,
            },
        )?;
        let path = out.join("sensors/coverage/coverage-summary.json");
        let original: serde_json::Value = serde_json::from_slice(&fs::read(&path)?)?;
        let mut wrong_schema = original.clone();
        wrong_schema["schema"] = serde_json::json!("other");
        let mut wrong_status = original.clone();
        wrong_status["status"] = serde_json::json!("success");
        let mut correctness_claim = original.clone();
        correctness_claim["correctness_claim"] = serde_json::json!(true);
        let mut missing_reason = original.clone();
        missing_reason["reason"] = serde_json::Value::Null;
        let variants = [
            None,
            Some(b"{".to_vec()),
            Some(vec![0xff]),
            Some(serde_json::to_vec(&wrong_schema)?),
            Some(serde_json::to_vec(&wrong_status)?),
            Some(serde_json::to_vec(&correctness_claim)?),
            Some(serde_json::to_vec(&missing_reason)?),
        ];
        for bytes in variants {
            if let Some(bytes) = bytes {
                fs::write(&path, bytes)?;
            } else {
                fs::remove_file(&path)?;
            }
            let digest = read_coverage_summary_digest(out);
            assert_eq!(digest.status, "unknown");
            let report = render_summary(out, &test_plan(vec![sensor.clone()]), &test_diff())?;
            assert!(report.contains(&digest.reason));
            assert!(!report.contains("coverage ran; sensor packet available."));
            let late = late_sensor_receipt_digests(out, &["coverage".to_owned()]);
            assert_eq!(late[0].status, "ok");
            assert_eq!(late[0].reason, "completed");
            assert!(reporter_prompt(&[], &late).contains(&digest.reason));
            assert!(late[0].excerpt().contains(&digest.reason));
        }
        Ok(())
    }

    #[test]
    fn coverage_report_does_not_turn_out_of_scope_skip_into_a_gap() -> Result<()> {
        use crate::tests::{sensor_plan, test_diff, test_plan};
        let temp = tempfile::tempdir()?;
        let sensor = sensor_plan("coverage", "cargo", false);
        write_sensor_status(
            temp.path(),
            &sensor,
            SensorStatusWrite {
                status: "skipped",
                argv: &[],
                duration_ms: 0,
                reason: "disabled by config",
                exit_code: None,
                timed_out: false,
            },
        )?;
        let report = render_summary(temp.path(), &test_plan(vec![sensor]), &test_diff())?;
        assert!(report.contains("No planned sensor evidence is currently missing."));
        assert!(!report.contains("coverage summary"));
        Ok(())
    }

    #[test]
    fn coverage_summary_rejects_aggregate_count_overflow() -> Result<()> {
        let temp = tempfile::tempdir()?;
        for count in ["LF", "LH", "FNF", "FNH"] {
            fs::write(
                temp.path().join("lcov.info"),
                format!(
                    "SF:src/a.rs\nLF:1\nLH:1\nFNF:1\nFNH:1\nend_of_record\nSF:src/b.rs\n{count}:18446744073709551615\nend_of_record\n"
                ),
            )?;
            let receipt = super::coverage_summary_receipt(temp.path());
            assert_eq!(receipt["status"], "unknown");
            assert_eq!(receipt["reason"], "lcov.info coverage totals overflow");
            for totals in ["line_totals", "function_totals"] {
                assert_eq!(receipt[totals]["found"], 0);
                assert_eq!(receipt[totals]["hit"], 0);
            }
            assert_eq!(receipt["lcov"]["present"], true);
            assert_eq!(receipt["correctness_claim"], false);
            for status in ["ok", "failed", "timed_out", "missing"] {
                super::write_coverage_status_receipt(
                    temp.path(),
                    SensorStatusWrite {
                        status,
                        argv: &[],
                        duration_ms: 0,
                        reason: "process outcome",
                        exit_code: None,
                        timed_out: status == "timed_out",
                    },
                )?;
                let sidecar: serde_json::Value =
                    serde_json::from_slice(&fs::read(temp.path().join("status.json"))?)?;
                assert_eq!(sidecar["status"], status);
                assert_eq!(sidecar["reason"], "process outcome");
                assert_eq!(sidecar["summary"]["status"], "unknown");
                assert_eq!(sidecar["lcov"]["present"], true);
            }
        }
        Ok(())
    }

    #[test]
    fn coverage_summary_accepts_representable_count_limits() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let record = "SF:src/a.rs\nLF:18446744073709551615\nLH:18446744073709551615\nFNF:18446744073709551615\nFNH:18446744073709551615\nend_of_record";
        for separator in ["\n", "\r\n"] {
            let text = format!("{record}\nSF:src/a.rs\nLF:0\nLH:0\nFNF:0\nFNH:0\nend_of_record")
                .replace('\n', separator);
            fs::write(temp.path().join("lcov.info"), text)?;
            let receipt = super::coverage_summary_receipt(temp.path());
            assert_eq!(receipt["status"], "collected");
            for totals in ["line_totals", "function_totals"] {
                assert_eq!(receipt[totals]["found"], u64::MAX);
                assert_eq!(receipt[totals]["hit"], u64::MAX);
            }
        }
        Ok(())
    }

    #[test]
    fn coverage_summary_rejects_truncated_record() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path();
        fs::write(
            dir.join("lcov.info"),
            "TN:\nSF:src/complete.rs\nLF:4\nLH:3\nend_of_record\nSF:src/truncated.rs\nLF:9\nLH:8\n",
        )?;
        let summary = super::coverage_summary_receipt(dir);
        assert_eq!(summary["status"], "unknown");
        assert_eq!(
            summary["reason"],
            "lcov.info contains an unterminated source record"
        );
        assert_eq!(summary["lcov"]["present"], true);
        assert_eq!(summary["line_totals"]["found"], 0);
        assert_eq!(summary["line_totals"]["hit"], 0);
        assert_eq!(summary["correctness_claim"], false);
        Ok(())
    }

    #[test]
    fn coverage_summary_checks_each_source_record_boundary() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("lcov.info");
        for text in [
            "SF:src/a.rs\nLF:4\nLH:3\n",
            "SF:src/a.rs\nLF:4\nLH:3\nSF:src/b.rs\nLF:2\nLH:1\nend_of_record\n",
        ] {
            fs::write(&path, text)?;
            let summary = super::coverage_summary_receipt(temp.path());
            assert_eq!(summary["status"], "unknown");
            assert_eq!(summary["line_totals"]["found"], 0);
        }
        for (text, found, hit) in [
            (
                "SF:src/a.rs\nLF:4\nLH:3\nend_of_record\nSF:src/b.rs\nLF:2\nLH:1\nend_of_record",
                6,
                4,
            ),
            ("SF:src/a.rs\nLF:0\nLH:0\nend_of_record\n", 0, 0),
        ] {
            fs::write(&path, text)?;
            let summary = super::coverage_summary_receipt(temp.path());
            assert_eq!(summary["status"], "collected");
            assert_eq!(summary["reason"], "lcov.info parsed");
            assert_eq!(summary["line_totals"]["found"], found);
            assert_eq!(summary["line_totals"]["hit"], hit);
        }
        fs::write(path, [0xff])?;
        let summary = super::coverage_summary_receipt(temp.path());
        assert_eq!(summary["status"], "unknown");
        assert_eq!(summary["reason"], "lcov.info unreadable");
        assert_eq!(summary["lcov"]["present"], true);
        Ok(())
    }

    #[test]
    fn coverage_summary_gap_preserves_sensor_process_status() -> Result<()> {
        let temp = tempfile::tempdir()?;
        fs::write(temp.path().join("lcov.info"), "SF:src/lib.rs\nLF:4\n")?;
        for status in ["ok", "failed", "timed_out", "missing"] {
            super::write_coverage_status_receipt(
                temp.path(),
                SensorStatusWrite {
                    status,
                    argv: &[],
                    duration_ms: 0,
                    reason: "process outcome",
                    exit_code: None,
                    timed_out: status == "timed_out",
                },
            )?;
            let receipt: serde_json::Value =
                serde_json::from_slice(&fs::read(temp.path().join("status.json"))?)?;
            assert_eq!(receipt["status"], status);
            assert_eq!(receipt["reason"], "process outcome");
            assert_eq!(receipt["summary"]["status"], "unknown");
            assert_eq!(receipt["correctness_claim"], false);
        }
        Ok(())
    }

    #[test]
    fn coverage_summary_receipt_records_absent_and_malformed_lcov() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let dir = temp.path().join("sensors/coverage");
        fs::create_dir_all(&dir)?;

        let missing = super::coverage_summary_receipt(&dir);
        assert_eq!(missing["status"], "not_collected");
        assert_eq!(missing["reason"], "lcov.info not present");
        assert_eq!(missing["lcov"]["present"], serde_json::json!(false));
        assert_eq!(missing["line_totals"]["found"], 0);
        assert_eq!(missing["line_totals"]["hit"], 0);

        fs::write(
            dir.join("lcov.info"),
            "TN:\nSF:src/lib.rs\nFNF:not-a-number\nFNH:1\nLF:4\nLH:not-a-number\nend_of_record\n",
        )?;
        let malformed = super::coverage_summary_receipt(&dir);
        assert_eq!(malformed["status"], "collected");
        assert_eq!(malformed["line_totals"]["found"], 4);
        assert_eq!(malformed["line_totals"]["hit"], 0);
        assert_eq!(malformed["function_totals"]["found"], 0);
        assert_eq!(malformed["function_totals"]["hit"], 1);
        Ok(())
    }
}
