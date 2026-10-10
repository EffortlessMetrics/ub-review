use super::helpers::*;
use super::*;

#[test]
fn planned_sensor_projection_preserves_the_complete_row() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    let plan_task = sensor_task("alpha", "planned");
    write_sensor_receipt(out, "alpha", "ok")?;
    let mut sources = BTreeSet::new();
    let task = terminalize_planned_task(
        out,
        &plan_task,
        &[],
        &BTreeMap::new(),
        &BTreeMap::new(),
        &mut sources,
    )?;
    assert_eq!(
        serde_json::to_value(task)?,
        serde_json::json!({
            "id": "sensor-alpha",
            "kind": "sensor",
            "source": "tool-registry",
            "plan_status": "planned",
            "status": "ok",
            "reason": "sensor ended ok",
            "request_ids": [],
            "receipt_ids": [],
            "receipt_path": "sensors/alpha/ub-review-sensor-status.json",
            "task_path": "resolved-tools.json",
            "plan_task": plan_task
        })
    );
    assert_eq!(
        sources,
        BTreeSet::from(["sensors/alpha/ub-review-sensor-status.json".to_owned()])
    );
    Ok(())
}

#[test]
fn planned_proof_projection_preserves_exact_identity_and_metadata() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let plan_task = proof_plan_task("proof-a");
    let receipts = vec![proof_receipt(
        "receipt-z",
        &["req-z", "req-a"],
        &["tests-oracle"],
    )];
    let requests = BTreeMap::from([(
        "proof-a".to_owned(),
        vec!["req-z".to_owned(), "req-a".to_owned(), "req-z".to_owned()],
    )]);
    let receipt_requests = validate_proof_receipts(&receipts)?;
    let task = terminalize_planned_task(
        temp.path(),
        &plan_task,
        &receipts,
        &requests,
        &receipt_requests,
        &mut BTreeSet::new(),
    )?;
    assert_eq!(
        serde_json::to_value(task)?,
        serde_json::json!({
            "id": "proof-a",
            "kind": "focused-test",
            "source": "proof-planner",
            "plan_status": "planned",
            "status": "head_passed",
            "reason": "receipt-z=head_passed join=request_identity",
            "request_ids": ["req-a", "req-z"],
            "receipt_ids": ["receipt-z"],
            "receipt_path": "review/proof_receipts.json",
            "task_path": "proof_tasks.ndjson",
            "plan_task": plan_task
        })
    );
    Ok(())
}

#[test]
fn terminal_queue_and_events_are_exact_under_receipt_reordering() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    let plan = write_plan(out, Vec::new())?;
    let mut receipts = vec![
        proof_receipt("zeta", &["req-z"], &["impact-planner"]),
        proof_receipt("alpha", &["req-a"], &["impact-planner"]),
    ];
    write_terminal_work_queue_artifacts(out, &receipts)?;
    let queue_bytes = fs::read(out.join(TERMINAL_QUEUE_FILE))?;
    let event_bytes = fs::read(out.join(TERMINAL_EVENTS_FILE))?;
    receipts.reverse();
    write_terminal_work_queue_artifacts(out, &receipts)?;
    assert_eq!(fs::read(out.join(TERMINAL_QUEUE_FILE))?, queue_bytes);
    assert_eq!(fs::read(out.join(TERMINAL_EVENTS_FILE))?, event_bytes);
    let queue: serde_json::Value = serde_json::from_slice(&queue_bytes)?;
    assert_eq!(queue["source_plan"], "work_queue_plan.json");
    assert_eq!(queue["source_plan_sha256"], sha256_hex(&plan));
    assert_eq!(
        queue["source_receipts"],
        serde_json::json!(["review/proof_receipts.json"])
    );
    let rows = queue["tasks"]
        .as_array()
        .context("terminal task array absent")?;
    assert_eq!(rows.len(), 2);
    let event_text = std::str::from_utf8(&event_bytes)?;
    let events = event_text
        .lines()
        .map(serde_json::from_str)
        .collect::<serde_json::Result<Vec<serde_json::Value>>>()?;
    assert_eq!(events.len(), 2);
    for (index, (id, request)) in [("alpha", "req-a"), ("zeta", "req-z")].iter().enumerate() {
        assert_eq!(
            rows[index],
            serde_json::json!({
                "id": id,
                "kind": "focused-head",
                "source": "proof-receipt",
                "status": "head_passed",
                "reason": "focused HEAD proof passed",
                "request_ids": [request],
                "receipt_ids": [id],
                "receipt_path": format!("review/proof_receipts.json#{id}")
            })
        );
        assert_eq!(
            events[index],
            serde_json::json!({
                "schema": WORK_EVENT_TERMINAL_SCHEMA,
                "kind": "task_terminal_projected",
                "task_id": id,
                "task_kind": "focused-head",
                "source": "proof-receipt",
                "status": "head_passed",
                "reason": "focused HEAD proof passed",
                "request_ids": [request],
                "receipt_ids": [id]
            })
        );
    }
    Ok(())
}

#[test]
fn sensor_projection_distinguishes_missing_defaulted_and_unreadable_receipts() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    let path = "sensors/alpha/ub-review-sensor-status.json";
    let mut sources = BTreeSet::new();
    assert_eq!(
        terminalize_sensor(out, "sensor-alpha", "planned", Some(path), &mut sources)?,
        (
            "missing_receipt".to_owned(),
            "planned sensor produced no terminal status receipt".to_owned(),
            Vec::new(),
            Vec::new()
        )
    );
    assert!(sources.is_empty());
    write_sensor_receipt_value(
        out,
        "alpha",
        &serde_json::json!({"sensor": "alpha", "status": "ok"}),
    )?;
    assert_eq!(
        terminalize_sensor(out, "sensor-alpha", "planned", Some(path), &mut sources)?,
        (
            "ok".to_owned(),
            "sensor terminal receipt supplied no reason".to_owned(),
            Vec::new(),
            Vec::new()
        )
    );
    assert_eq!(sources, BTreeSet::from([path.to_owned()]));
    let error = terminalize_sensor(out, "sensor-alpha", "planned", None, &mut sources)
        .err()
        .context("planned sensor without a receipt path was accepted")?;
    assert_eq!(error.to_string(), "planned sensor has no receipt path");
    fs::remove_file(out.join(path))?;
    fs::create_dir(out.join(path))?;
    let error = terminalize_sensor(out, "sensor-alpha", "planned", Some(path), &mut sources)
        .err()
        .context("sensor receipt directory was accepted")?;
    assert_eq!(
        error.to_string(),
        format!("read sensor receipt {}", out.join(path).display())
    );
    sources.clear();
    assert_eq!(
        terminalize_sensor(out, "sensor-alpha", "skipped", None, &mut sources)?,
        (
            "skipped".to_owned(),
            "sensor was not selected by the immutable plan".to_owned(),
            Vec::new(),
            Vec::new()
        )
    );
    assert!(sources.is_empty());
    Ok(())
}

#[test]
fn missing_planner_identity_fields_cannot_commit_terminal_output() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path();
    for field in ["id", "kind", "source", "status"] {
        let mut task = proof_plan_task("proof-a");
        task.as_object_mut()
            .context("fixture is not an object")?
            .remove(field);
        write_plan(out, vec![task])?;
        let error = write_terminal_work_queue_artifacts(out, &[])
            .err()
            .with_context(|| format!("plan without {field} was accepted"))?;
        assert_eq!(
            error.to_string(),
            format!("terminal queue object has no nonempty {field}")
        );
        assert!(!out.join(TERMINAL_QUEUE_FILE).exists());
        assert!(!out.join(TERMINAL_EVENTS_FILE).exists());
    }
    write_plan(out, vec![serde_json::json!([])])?;
    let error = write_terminal_work_queue_artifacts(out, &[])
        .err()
        .context("non-object plan task was accepted")?;
    assert_eq!(
        error.to_string(),
        "terminal queue plan task is not an object"
    );
    assert!(!out.join(TERMINAL_QUEUE_FILE).exists());
    Ok(())
}

#[test]
fn proof_decline_preserves_sorted_consumer_identity_without_a_receipt() -> Result<()> {
    let requests = BTreeMap::from([(
        "proof-a".to_owned(),
        vec!["req-z".to_owned(), "req-a".to_owned(), "req-z".to_owned()],
    )]);
    assert_eq!(
        terminalize_proof("proof-a", "skipped", &[], &requests, &BTreeMap::new())?,
        (
            "skipped".to_owned(),
            "proof task retained its terminal planner disposition".to_owned(),
            vec!["req-a".to_owned(), "req-z".to_owned()],
            Vec::new()
        )
    );
    Ok(())
}
