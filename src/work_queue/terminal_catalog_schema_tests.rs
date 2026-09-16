//! A request-identity catalog must be admitted before it can join receipts.

use super::helpers::*;
use super::*;

#[test]
fn unsupported_catalog_schema_cannot_authorize_a_terminal_receipt_join() -> Result<()> {
    for join in ["absent", "request", "task"] {
        for schema in [
            None,
            Some(serde_json::Value::Null),
            Some(serde_json::json!(1)),
            Some(serde_json::json!(true)),
            Some(serde_json::json!("ub-review.proof_task.v999")),
            Some(serde_json::json!("ub-review.proof_receipt.v1")),
            Some(serde_json::json!(" ub-review.proof_task.v1 ")),
        ] {
            let temp = tempfile::tempdir()?;
            let out = temp.path();
            let plan_bytes = write_plan(out, vec![proof_plan_task("proof-task-a")])?;
            let valid = serde_json::json!({
                "schema": "ub-review.proof_task.v1",
                "id": "proof-task-a",
                "request_ids": ["req-a"]
            });
            fs::write(
                out.join("proof_tasks.ndjson"),
                format!("\n{}\n", serde_json::to_string(&valid)?),
            )?;
            let receipts = match join {
                "request" => vec![proof_receipt("proof-receipt-a", &["req-a"], &["tests"])],
                "task" => vec![proof_receipt("proof-task-a", &["other-request"], &["tests"])],
                _ => Vec::new(),
            };
            write_terminal_work_queue_artifacts(out, &receipts)?;
            let prior: serde_json::Value =
                serde_json::from_slice(&fs::read(out.join(TERMINAL_QUEUE_FILE))?)?;
            let expected_status = if join == "absent" {
                "not_executed"
            } else {
                "head_passed"
            };
            assert_eq!(prior["tasks"][0]["status"], expected_status);
            assert!(out.join(TERMINAL_EVENTS_FILE).is_file());
            fs::write(out.join(TERMINAL_QUEUE_TMP_FILE), b"stale queue staging")?;
            fs::write(out.join(TERMINAL_EVENTS_TMP_FILE), b"stale event staging")?;

            let mut invalid = valid;
            let object = invalid
                .as_object_mut()
                .context("catalog fixture is not an object")?;
            match schema {
                Some(value) => {
                    object.insert("schema".to_owned(), value);
                }
                None => {
                    object.remove("schema");
                }
            }
            fs::write(
                out.join("proof_tasks.ndjson"),
                format!("\n{}\n", serde_json::to_string(&invalid)?),
            )?;
            let error = write_terminal_work_queue_artifacts(out, &receipts)
                .err()
                .with_context(|| format!("unsupported catalog schema authorized {join} join"))?;
            assert_eq!(
                format!("{error:#}"),
                "proof task catalog line 2 has unsupported schema"
            );
            assert_eq!(fs::read(out.join("work_queue_plan.json"))?, plan_bytes);
            assert_eq!(fs::read(out.join("work_queue.json"))?, plan_bytes);
            for name in [
                TERMINAL_QUEUE_FILE,
                TERMINAL_EVENTS_FILE,
                TERMINAL_QUEUE_TMP_FILE,
                TERMINAL_EVENTS_TMP_FILE,
            ] {
                assert!(!out.join(name).exists(), "invalid catalog retained {name}");
            }
        }
    }
    Ok(())
}
