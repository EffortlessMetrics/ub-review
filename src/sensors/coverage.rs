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
    if present {
        let text = fs::read_to_string(&lcov_path).unwrap_or_default();
        for line in text.lines() {
            if let Some(value) = line.strip_prefix("LF:") {
                lines_found += parse_lcov_count(value);
            } else if let Some(value) = line.strip_prefix("LH:") {
                lines_hit += parse_lcov_count(value);
            } else if let Some(value) = line.strip_prefix("FNF:") {
                functions_found += parse_lcov_count(value);
            } else if let Some(value) = line.strip_prefix("FNH:") {
                functions_hit += parse_lcov_count(value);
            }
        }
    }
    serde_json::json!({
        "schema": COVERAGE_SUMMARY_SCHEMA,
        "status": if present { "collected" } else { "not_collected" },
        "reason": if present { "lcov.info parsed" } else { "lcov.info not present" },
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
