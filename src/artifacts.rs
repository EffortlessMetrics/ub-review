//! Artifact schema constants: every `ub-review.<name>.vN` string the run
//! writes, in one file (cleanup train PR 3). The verifier pins the exact
//! literals on the Python side and the inline tests keep asserting literal
//! strings on purpose - a test asserting the constant against itself would
//! be tautological. Changing a value here is a schema bump: update the
//! verifier, the artifact maturity table (SPEC-0004), and the consumers in
//! the same PR.

pub(crate) const CACHE_EVENT_SCHEMA: &str = "ub-review.cache_event.v1";
pub(crate) const CACHE_MANIFEST_SCHEMA: &str = "ub-review.cache_manifest.v1";
pub(crate) const CANDIDATE_SCHEMA: &str = "ub-review.candidate.v1";
pub(crate) const CI_CORRELATION_SCHEMA: &str = "ub-review.ci_correlation.v1";
pub(crate) const CI_COSTS_SCHEMA: &str = "ub-review.ci_costs.v1";
pub(crate) const CI_HISTORY_SCHEMA: &str = "ub-review.ci_history.v1";
pub(crate) const CI_INVENTORY_SCHEMA: &str = "ub-review.ci_inventory.v1";
pub(crate) const CI_RECOMMENDATIONS_SCHEMA: &str = "ub-review.ci_recommendations.v1";
pub(crate) const COVERAGE_CHANGED_LINES_SCHEMA: &str = "ub-review.coverage_changed_lines.v1";
pub(crate) const COVERAGE_STATUS_SCHEMA: &str = "ub-review.coverage_status.v1";
pub(crate) const COVERAGE_SUMMARY_SCHEMA: &str = "ub-review.coverage_summary.v1";
pub(crate) const COVERAGE_UPLOAD_SCHEMA: &str = "ub-review.coverage_upload.v1";
pub(crate) const DROPPED_OBSERVATION_SCHEMA: &str = "ub-review.dropped_observation.v1";
pub(crate) const FINAL_COMPILER_INPUT_V2_SCHEMA: &str = "ub-review.final_compiler_input.v2";
pub(crate) const FOLLOW_UP_EVIDENCE_SCHEMA: &str = "ub-review.follow_up_evidence.v1";
pub(crate) const FOLLOW_UP_OUTPUT_SCHEMA: &str = "ub-review.follow_up_output.v1";
pub(crate) const FOLLOW_UP_QUESTION_PACKET_SCHEMA: &str = "ub-review.follow_up_question_packet.v1";
pub(crate) const FOLLOW_UP_QUESTION_SCHEMA: &str = "ub-review.follow_up_question.v1";
pub(crate) const FOLLOW_UP_RESULT_SCHEMA: &str = "ub-review.follow_up_result.v1";
pub(crate) const GATE_OUTCOME_SCHEMA: &str = "ub-review.gate_outcome.v1";
pub(crate) const ISSUE_ACTION_SCHEMA: &str = "ub-review.issue_action.v1";
pub(crate) const ISSUE_BROKER_PLAN_SCHEMA: &str = "ub-review.issue_broker_plan.v1";
pub(crate) const ISSUE_BROKER_RESULT_SCHEMA: &str = "ub-review.issue_broker_result.v1";
pub(crate) const ISSUE_CANDIDATE_SCHEMA: &str = "ub-review.issue_candidate.v1";
pub(crate) const MERGED_OBSERVATION_SCHEMA: &str = "ub-review.merged_observation.v1";
pub(crate) const MODEL_STAGE_SCHEMA: &str = "ub-review.model_stage.v1";
pub(crate) const OBSERVATION_GROUP_SCHEMA: &str = "ub-review.observation_group.v1";
pub(crate) const OBSERVATION_SCHEMA: &str = "ub-review.observation.v1";
pub(crate) const ORCHESTRATOR_EVIDENCE_GROUP_SCHEMA: &str =
    "ub-review.orchestrator_evidence_group.v1";
pub(crate) const ORCHESTRATOR_OBSERVATION_GROUP_SCHEMA: &str =
    "ub-review.orchestrator_observation_group.v1";
pub(crate) const ORCHESTRATOR_PLAN_SCHEMA: &str = "ub-review.orchestrator_plan.v1";
pub(crate) const ORCHESTRATOR_ROUTED_EVIDENCE_SCHEMA: &str =
    "ub-review.orchestrator_routed_evidence.v1";
pub(crate) const PROOF_PLANNER_INPUT_SCHEMA: &str = "ub-review.proof_planner_input.v1";
pub(crate) const PROOF_PLANNER_OUTPUT_SCHEMA: &str = "ub-review.proof_planner_output.v1";
pub(crate) const PROOF_POLICY_RESOLUTION_SCHEMA: &str = "ub-review.proof_policy_resolution.v1";
pub(crate) const PROOF_RECEIPT_SCHEMA: &str = "ub-review.proof_receipt.v1";
pub(crate) const PROOF_REQUEST_GROUP_SCHEMA: &str = "ub-review.proof_request_group.v1";
pub(crate) const PROOF_REQUEST_SCHEMA: &str = "ub-review.proof_request.v1";
pub(crate) const PROOF_TASK_SCHEMA: &str = "ub-review.proof_task.v1";
pub(crate) const PR_THREAD_CONTEXT_SCHEMA: &str = "ub-review.pr_thread_context.v1";
pub(crate) const QUESTION_OBSERVATIONS_SCHEMA: &str = "ub-review.question_observations.v1";
pub(crate) const RECEIPT_ROUTES_SCHEMA: &str = "ub-review.receipt_routes.v1";
pub(crate) const RECEIPT_ROUTE_SCHEMA: &str = "ub-review.receipt_route.v1";
pub(crate) const RIPR_EXPOSURE_GAPS_SCHEMA: &str = "ub-review.ripr_exposure_gaps.v1";
pub(crate) const RESOLVED_CANDIDATE_SCHEMA: &str = "ub-review.resolved_candidate.v1";
pub(crate) const RESOLVED_PLAN_SCHEMA: &str = "ub-review.resolved_plan.v1";
pub(crate) const RESOLVED_PROFILE_SCHEMA: &str = "ub-review.resolved_profile.v1";
pub(crate) const RESOLVED_TOOLS_SCHEMA: &str = "ub-review.resolved_tools.v1";
pub(crate) const RESOURCE_LEASE_SCHEMA: &str = "ub-review.resource_lease.v1";
pub(crate) const SCHEDULER_SCHEMA: &str = "ub-review.scheduler.v1";
pub(crate) const SETUP_PR_ERROR_SCHEMA: &str = "ub-review.setup_pr_error.v1";
pub(crate) const SETUP_PR_RESULT_SCHEMA: &str = "ub-review.setup_pr_result.v1";
pub(crate) const TERMINAL_STATE_SCHEMA: &str = "ub-review.terminal_state.v1";
pub(crate) const TOOL_GATE_OUTCOMES_SCHEMA: &str = "ub-review.tool_gate_outcomes.v1";
pub(crate) const TOOL_GATE_OUTCOME_SCHEMA: &str = "ub-review.tool_gate_outcome.v1";
pub(crate) const TOOL_STATUS_SCHEMA: &str = "ub-review.tool_status.v1";
pub(crate) const UB_REVIEW_COST_SCHEMA: &str = "ub-review.cost_receipt.v1";
pub(crate) const WITNESS_REGISTRY_SCHEMA: &str = "ub-review.witness_registry.v1";
pub(crate) const WITNESS_SCHEMA: &str = "ub-review.witness.v1";
pub(crate) const WORK_EVENT_SCHEMA: &str = "ub-review.work_event.v1";
pub(crate) const WORK_QUEUE_SCHEMA: &str = "ub-review.work_queue.v1";
pub(crate) const WORK_QUEUE_TASK_SCHEMA: &str = "ub-review.work_queue_task.v1";
