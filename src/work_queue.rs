//! Work queue artifact construction, proof task artifacts, proof
//! receipt and resource lease artifact writing (cleanup train step 43,
//! pure code motion).

use crate::*;

pub(crate) fn proof_task_artifact(
    plan: FocusedProofPlan,
    budget: ProofBudget,
    lease_budget: ProofLeaseBudget,
) -> ProofTaskArtifact {
    let base_plus_tests_command =
        (plan.mode == FocusedProofMode::RedGreen).then(|| plan.base_plus_tests_command.clone());
    let command = match &base_plus_tests_command {
        Some(base_command) => format!("{} && {}", plan.head_command, base_command),
        None => plan.head_command.clone(),
    };
    let purpose = focused_proof_task_purpose(&plan);
    let timeout_sec = plan
        .timeout_sec
        .saturating_mul(plan.mode.command_count())
        .min(budget.max_total_seconds);
    ProofTaskArtifact {
        schema: PROOF_TASK_SCHEMA,
        id: plan.id,
        kind: "focused-test".to_owned(),
        source: "proof-planner".to_owned(),
        priority: "high".to_owned(),
        packet_policy: "late-follow-up".to_owned(),
        deadline_sec: timeout_sec,
        gate_policy: "trust-affecting".to_owned(),
        status: plan.status,
        command,
        head_command: plan.head_command,
        base_plus_tests_command,
        purpose,
        consumers: vec![
            "tests-oracle".to_owned(),
            "opposition".to_owned(),
            "compiler".to_owned(),
        ],
        value: "high".to_owned(),
        cost: "low".to_owned(),
        timeout_sec,
        lease: ProofTaskLease {
            cpu: lease_budget.cpu,
            memory_mb: lease_budget.memory_mb,
            disk_mb: lease_budget.disk_mb,
            network: lease_budget.network,
            timeout_sec,
        },
        test_file: plan.test_file,
        test_name: plan.test_name,
        mode: plan.mode.key().to_owned(),
        requested_by: plan.requested_by,
        request_ids: plan.request_ids,
    }
}

pub(crate) fn focused_build_task_artifact(
    plan: FocusedBuildPlan,
    budget: ProofBudget,
    lease_budget: ProofLeaseBudget,
) -> ProofTaskArtifact {
    let timeout_sec = plan.timeout_sec.min(budget.max_total_seconds);
    ProofTaskArtifact {
        schema: PROOF_TASK_SCHEMA,
        id: plan.id,
        kind: "focused-build".to_owned(),
        source: "proof-planner".to_owned(),
        priority: "medium".to_owned(),
        packet_policy: "late-follow-up".to_owned(),
        deadline_sec: timeout_sec,
        gate_policy: "trust-affecting".to_owned(),
        status: plan.status,
        command: plan.command.clone(),
        head_command: plan.command.clone(),
        base_plus_tests_command: None,
        purpose: format!("Run focused HEAD build proof `{}`.", plan.command),
        consumers: focused_build_task_consumers(&plan.requested_by),
        value: "medium".to_owned(),
        cost: "focused-build".to_owned(),
        timeout_sec,
        lease: ProofTaskLease {
            cpu: lease_budget.cpu,
            memory_mb: lease_budget.memory_mb,
            disk_mb: lease_budget.disk_mb,
            network: lease_budget.network,
            timeout_sec,
        },
        test_file: "workspace".to_owned(),
        test_name: None,
        mode: FocusedProofMode::HeadOnly.key().to_owned(),
        requested_by: plan.requested_by,
        request_ids: plan.request_ids,
    }
}

pub(crate) fn write_work_queue_artifacts(
    out: &Path,
    plan: &Plan,
    proof_tasks: &[ProofTaskArtifact],
) -> Result<()> {
    let mut tasks = plan
        .sensors
        .iter()
        .map(|sensor| work_queue_task_from_sensor(out, sensor))
        .collect::<Vec<_>>();
    tasks.extend(proof_tasks.iter().map(work_queue_task_from_proof_task));
    let queue = WorkQueueArtifact {
        schema: WORK_QUEUE_SCHEMA,
        initial_packet_deadline_sec: DEFAULT_INITIAL_PACKET_DEADLINE_SEC,
        follow_up_deadline_sec: DEFAULT_FOLLOW_UP_PACKET_DEADLINE_SEC,
        tasks: &tasks,
    };
    fs::write(
        out.join("work_queue.json"),
        serde_json::to_vec_pretty(&queue)?,
    )?;

    let mut ndjson = String::new();
    for task in &tasks {
        let event = WorkEventArtifact {
            schema: WORK_EVENT_SCHEMA,
            kind: "task_planned",
            task_id: task.id.clone(),
            task_kind: task.kind.clone(),
            source: task.source.clone(),
            packet_policy: task.packet_policy.clone(),
            deadline_sec: task.deadline_sec,
            consumers: task.consumers.clone(),
            gate_policy: task.gate_policy.clone(),
            status: task.status.clone(),
            initial_packet_status: task.initial_packet_status.clone(),
            receipt_path: task.receipt_path.clone(),
        };
        ndjson.push_str(&serde_json::to_string(&event)?);
        ndjson.push('\n');
    }
    fs::write(out.join("work_events.ndjson"), ndjson)?;
    Ok(())
}

pub(crate) fn work_queue_task_from_sensor(
    out: &Path,
    sensor: &SensorPlan,
) -> WorkQueueTaskArtifact {
    let packet_policy = work_queue_sensor_packet_policy(sensor);
    let receipt_path = format!("sensors/{}/ub-review-sensor-status.json", sensor.id);
    let status = if sensor.run { "planned" } else { "skipped" }.to_owned();
    let receipt_ready = out.join(&receipt_path).is_file();
    WorkQueueTaskArtifact {
        schema: WORK_QUEUE_TASK_SCHEMA,
        id: format!("sensor-{}", sensor.id),
        kind: "sensor".to_owned(),
        source: "tool-registry".to_owned(),
        priority: work_queue_sensor_priority(sensor),
        packet_policy: packet_policy.clone(),
        deadline_sec: work_queue_sensor_deadline(sensor),
        consumers: work_queue_sensor_consumers(&sensor.id),
        gate_policy: work_queue_sensor_gate_policy(sensor),
        dedupe_key: format!("tool-registry:sensor:{}", sensor.id),
        lease: work_queue_sensor_lease(sensor),
        receipt_path,
        status: status.clone(),
        initial_packet_status: work_queue_initial_packet_status(
            &packet_policy,
            &status,
            receipt_ready,
        ),
        task_path: "resolved-tools.json".to_owned(),
    }
}

pub(crate) fn work_queue_task_from_proof_task(task: &ProofTaskArtifact) -> WorkQueueTaskArtifact {
    let receipt_ready = false;
    WorkQueueTaskArtifact {
        schema: WORK_QUEUE_TASK_SCHEMA,
        id: task.id.clone(),
        kind: task.kind.clone(),
        source: task.source.clone(),
        priority: task.priority.clone(),
        packet_policy: task.packet_policy.clone(),
        deadline_sec: task.deadline_sec,
        consumers: task.consumers.clone(),
        gate_policy: task.gate_policy.clone(),
        dedupe_key: work_queue_dedupe_key(task),
        lease: task.lease.clone(),
        receipt_path: "review/proof_receipts.json".to_owned(),
        status: task.status.clone(),
        initial_packet_status: work_queue_initial_packet_status(
            &task.packet_policy,
            &task.status,
            receipt_ready,
        ),
        task_path: "proof_tasks.ndjson".to_owned(),
    }
}

pub(crate) fn work_queue_initial_packet_status(
    packet_policy: &str,
    status: &str,
    receipt_ready: bool,
) -> String {
    match packet_policy {
        "must-run" | "include-if-ready" if status == "planned" && receipt_ready => {
            "ready_for_initial_packet".to_owned()
        }
        "must-run" | "include-if-ready" if status == "planned" => {
            "pending_initial_packet".to_owned()
        }
        "late-follow-up" | "adaptive" if status == "planned" => "pending_initial_packet".to_owned(),
        _ => "not_initial_packet".to_owned(),
    }
}

pub(crate) fn work_queue_sensor_priority(sensor: &SensorPlan) -> String {
    if sensor.required {
        "high".to_owned()
    } else if sensor.run {
        "medium".to_owned()
    } else {
        "low".to_owned()
    }
}

pub(crate) fn work_queue_sensor_packet_policy(sensor: &SensorPlan) -> String {
    if sensor.required {
        "must-run".to_owned()
    } else if sensor.run {
        "include-if-ready".to_owned()
    } else {
        "artifact-only".to_owned()
    }
}

pub(crate) fn work_queue_sensor_deadline(sensor: &SensorPlan) -> u64 {
    if sensor.run { sensor.timeout_sec } else { 0 }
}

pub(crate) fn work_queue_sensor_consumers(id: &str) -> Vec<String> {
    let consumers: &[&str] = match id {
        "ripr" => &["tests-oracle", "proof-planner", "compiler"],
        "coverage" => &["tests-oracle", "source-route", "compiler"],
        "unsafe-review" => &["ub-memory-lifetime", "security", "compiler"],
        "actionlint" => &[
            "workflow-permissions",
            "workflow-pinning",
            "workflow-proof",
            "workflow-opposition",
            "compiler",
        ],
        "ast-grep" => &["source-route", "compiler"],
        "tokmd" => &["all-lanes", "compiler"],
        "cargo-allow" => &["security", "compiler"],
        _ => &["compiler"],
    };
    consumers
        .iter()
        .map(|consumer| (*consumer).to_owned())
        .collect()
}

pub(crate) fn work_queue_sensor_gate_policy(sensor: &SensorPlan) -> String {
    if sensor.required {
        "gate-required".to_owned()
    } else if sensor.gate.is_some() {
        "trust-affecting".to_owned()
    } else if sensor.run {
        "review-context".to_owned()
    } else {
        "artifact-only".to_owned()
    }
}

pub(crate) fn work_queue_sensor_lease(sensor: &SensorPlan) -> ProofTaskLease {
    ProofTaskLease {
        cpu: u32::from(sensor.run),
        memory_mb: 0,
        disk_mb: sensor.artifact_budget_mb,
        network: false,
        timeout_sec: work_queue_sensor_deadline(sensor),
    }
}

pub(crate) fn work_queue_dedupe_key(task: &ProofTaskArtifact) -> String {
    if task.request_ids.is_empty() {
        return format!("{}:{}:{}", task.source, task.kind, task.id);
    }
    format!(
        "{}:{}:{}",
        task.source,
        task.kind,
        task.request_ids.join("+")
    )
}

pub(crate) fn focused_build_task_consumers(requested_by: &[String]) -> Vec<String> {
    let mut consumers = requested_by.to_vec();
    push_unique(&mut consumers, "compiler");
    consumers
}

pub(crate) fn focused_proof_task_purpose(plan: &FocusedProofPlan) -> String {
    match plan.mode {
        FocusedProofMode::HeadOnly => {
            format!(
                "Prove the focused test target `{}` passes on HEAD.",
                plan.test_file
            )
        }
        FocusedProofMode::RedGreen => format!(
            "Prove the focused test target `{}` fails on base+tests and passes on HEAD.",
            plan.test_file
        ),
    }
}

pub(crate) fn write_proof_receipt_artifacts(
    out: &Path,
    proof_receipts: &[ProofReceipt],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("proof_receipts.json"),
        serde_json::to_vec_pretty(proof_receipts)?,
    )?;
    let mut ndjson = String::new();
    for receipt in proof_receipts {
        ndjson.push_str(&serde_json::to_string(receipt)?);
        ndjson.push('\n');
    }
    fs::write(out.join("proof_receipts.ndjson"), ndjson)?;
    Ok(())
}

pub(crate) fn write_resource_lease_artifacts(
    out: &Path,
    resource_leases: &[ResourceLease],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("resource_leases.json"),
        serde_json::to_vec_pretty(resource_leases)?,
    )?;

    let mut ndjson = String::new();
    for lease in resource_leases {
        ndjson.push_str(&serde_json::to_string(lease)?);
        ndjson.push('\n');
    }
    fs::write(out.join("resource_leases.ndjson"), ndjson)?;

    let mut plan = String::new();
    plan.push_str("# Resource lease plan\n\n");
    if resource_leases.is_empty() {
        plan.push_str("No local proof leases were requested in this packet.\n");
    } else {
        plan.push_str("## Focused proof leases\n\n");
        for lease in resource_leases {
            plan.push_str(&format!(
                "- `{}` kind=`{}` consumer=`{}` status=`{}` cpu=`{}` memory_mb=`{}` disk_mb=`{}` timeout_sec=`{}` network=`{}` scratch=`{}`",
                lease.id,
                lease.kind,
                lease.consumer,
                lease.status,
                lease.cpu,
                lease.memory_mb,
                lease.disk_mb,
                lease.timeout_sec,
                lease.network,
                lease.scratch
            ));
            if let Some(worktree) = &lease.worktree {
                plan.push_str(&format!(" worktree=`{}`", escape_md(worktree)));
            }
            if let Some(command) = &lease.command {
                plan.push_str(&format!(" command=`{}`", escape_md(command)));
            }
            plan.push_str(&format!(". {}\n", escape_md(&lease.reason)));
        }
    }
    fs::write(review_dir.join("resource_plan.md"), plan)?;
    Ok(())
}
