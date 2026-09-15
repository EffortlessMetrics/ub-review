//! Work queue artifact construction and terminal queue projection.
//!
//! The legacy planner writer remains the compatibility implementation. This
//! module preserves its bytes as immutable plan artifacts, then publishes a
//! separate terminal projection after final proof receipts exist.

use crate::*;

#[path = "work_queue_legacy.rs"]
mod legacy;
mod terminal;

pub(crate) use legacy::{
    focused_build_task_artifact, focused_build_task_consumers, focused_proof_task_purpose,
    proof_task_artifact, work_queue_dedupe_key, work_queue_initial_packet_status,
    work_queue_sensor_consumers, work_queue_sensor_deadline, work_queue_sensor_gate_policy,
    work_queue_sensor_lease, work_queue_sensor_packet_policy, work_queue_sensor_priority,
    work_queue_task_from_proof_task, work_queue_task_from_sensor, write_resource_lease_artifacts,
};

pub(crate) fn write_work_queue_artifacts(
    out: &Path,
    plan: &Plan,
    proof_tasks: &[ProofTaskArtifact],
) -> Result<()> {
    legacy::write_work_queue_artifacts(out, plan, proof_tasks)?;
    let queue = fs::read(out.join("work_queue.json"))?;
    let events = fs::read(out.join("work_events.ndjson"))?;
    fs::write(out.join("work_queue_plan.json"), queue)?;
    fs::write(out.join("work_events_plan.ndjson"), events)?;
    Ok(())
}

pub(crate) fn write_proof_receipt_artifacts(
    out: &Path,
    proof_receipts: &[ProofReceipt],
    revision: Option<&crate::RevisionRef>,
) -> Result<()> {
    legacy::write_proof_receipt_artifacts(out, proof_receipts, revision)?;
    if out.join("work_queue_plan.json").is_file() {
        terminal::write_terminal_work_queue_artifacts(out, proof_receipts)?;
    }
    Ok(())
}
