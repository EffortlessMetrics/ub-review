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
