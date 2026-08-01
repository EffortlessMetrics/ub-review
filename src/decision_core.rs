//! PR decision core: proof receipt classification, test-witness
//! question matching, diff structural analysis, and stale-finding
//! detection (cleanup train step 23a, pure code motion).

use crate::*;

pub(crate) struct PrDecisionContext {
    pub(crate) finding_count: usize,
    pub(crate) verification_count: usize,
    pub(crate) has_test_proof_verification: bool,
    pub(crate) current_proof_failure: bool,
}

pub(crate) fn pr_decision_sentence(context: PrDecisionContext) -> Option<&'static str> {
    if context.finding_count > 0 {
        return Some("Needs reviewer attention before upstream: findings remain.");
    }
    if context.current_proof_failure {
        return Some("Needs focused proof failure resolved before upstream.");
    }
    if context.verification_count == 1 {
        if context.has_test_proof_verification {
            return Some("Needs one test-proof clarification before upstream.");
        }
        return Some("Needs one verification check before upstream.");
    }
    if context.verification_count > 1 {
        return Some("Needs verification checks before upstream.");
    }
    None
}

pub(crate) fn proof_receipt_is_test_proof_result(receipt: &ProofReceipt) -> bool {
    matches!(
        receipt.result.as_str(),
        "discriminating" | "head_passed" | "head_failed"
    )
}

pub(crate) fn proof_receipt_is_residual_risk(receipt: &ProofReceipt) -> bool {
    matches!(receipt.result.as_str(), "non_discriminating")
}

pub(crate) fn proof_receipt_is_missing_evidence(receipt: &ProofReceipt) -> bool {
    matches!(
        receipt.result.as_str(),
        "non_discriminating"
            | "base_patch_failed"
            | "timed_out"
            | "skipped_budget"
            | "skipped_profile"
    )
}

pub(crate) fn proof_receipts_answer_summary_test_witness_question(
    proof_receipts: &[ProofReceipt],
    finding: &SummaryOnlyFinding,
) -> bool {
    if lane_is_source_route(&finding.lane) {
        return false;
    }
    let text = format!("{} {}", finding.reason, finding.evidence);
    proof_receipts_answer_test_witness_question(proof_receipts, &text)
}

pub(crate) fn proof_receipts_answer_observation_test_witness_question(
    proof_receipts: &[ProofReceipt],
    observation: &ObservationGroup,
) -> bool {
    if observation
        .lanes
        .iter()
        .any(|lane| lane_is_source_route(lane))
        || kind_is_source_route_gap(&observation.kind)
    {
        return false;
    }
    let text = format!("{} {}", observation.claim, observation.evidence.join(" "));
    proof_receipts_answer_test_witness_question(proof_receipts, &text)
}

pub(crate) fn diff_structurally_answers_summary_test_witness_question(
    diff: &DiffContext,
    finding: &SummaryOnlyFinding,
) -> bool {
    if lane_is_source_route(&finding.lane) {
        return false;
    }
    let text = format!("{} {}", finding.reason, finding.evidence);
    diff_structurally_answers_test_witness_question(diff, &text)
}

pub(crate) fn diff_structurally_answers_observation_test_witness_question(
    diff: &DiffContext,
    observation: &ObservationGroup,
) -> bool {
    if observation
        .lanes
        .iter()
        .any(|lane| lane_is_source_route(lane))
        || kind_is_source_route_gap(&observation.kind)
    {
        return false;
    }
    if !matches!(
        observation.kind.as_str(),
        "missing-evidence" | "verification-question" | "test-gap"
    ) {
        return false;
    }
    let text = format!("{} {}", observation.claim, observation.evidence.join(" "));
    diff_structurally_answers_test_witness_question(diff, &text)
}

pub(crate) fn diff_structurally_answers_test_witness_question(
    diff: &DiffContext,
    text: &str,
) -> bool {
    text_requests_focused_test_witness(text)
        && diff_replaces_abort_with_recoverable_error(&diff.patch)
}

pub(crate) fn diff_replaces_abort_with_recoverable_error(patch: &str) -> bool {
    let mut removed_abort_path = false;
    let mut added_recoverable_error_path = false;

    for line in patch.lines() {
        if line.starts_with("---") || line.starts_with("+++") {
            continue;
        }
        if let Some(removed) = line.strip_prefix('-') {
            removed_abort_path |= line_mentions_abort_path(removed);
        } else if let Some(added) = line.strip_prefix('+') {
            added_recoverable_error_path |= line_mentions_recoverable_error_path(added);
        }
    }

    removed_abort_path && added_recoverable_error_path
}

pub(crate) fn line_mentions_abort_path(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    line.contains(".expect(")
        || line.contains("panic!(")
        || line.contains("abort(")
        || line.contains("unreachable!(")
}

pub(crate) fn line_mentions_recoverable_error_path(line: &str) -> bool {
    let line = line.to_ascii_lowercase();
    line.contains("map_err")
        || line.contains("throw")
        || line.contains("typeerror")
        || line.contains("jserror")
        || line.contains("return err")
        || line.contains("return error")
}

pub(crate) fn lane_is_source_route(lane: &str) -> bool {
    let lane = lane.to_ascii_lowercase();
    lane == "source-route" || lane == "source_route"
}

pub(crate) fn kind_is_source_route_gap(kind: &str) -> bool {
    let kind = kind.to_ascii_lowercase();
    kind == "source-route-gap" || kind == "source_route_gap"
}

pub(crate) fn proof_receipts_answer_test_witness_question(
    proof_receipts: &[ProofReceipt],
    text: &str,
) -> bool {
    proof_receipts
        .iter()
        .any(focused_test_proof_receipt_is_answer)
        && text_requests_focused_test_witness(text)
}

pub(crate) fn focused_test_proof_receipt_is_answer(receipt: &ProofReceipt) -> bool {
    receipt.kind == "focused-red-green"
        && receipt.test_patch_mode == "base-plus-tests"
        && matches!(
            receipt.result.as_str(),
            "discriminating"
                | "non_discriminating"
                | "base_patch_failed"
                | "timed_out"
                | "skipped_budget"
                | "skipped_profile"
        )
}

pub(crate) fn text_requests_focused_test_witness(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    let asks_for_witness = text.contains("proof")
        || text.contains("prove")
        || text.contains("proves")
        || text.contains("proven")
        || text.contains("witness")
        || text.contains("demonstrate")
        || text.contains("demonstrated")
        || text.contains("run")
        || text.contains("execute")
        || text.contains("confirm")
        || text.contains("attach")
        || text.contains("pass")
        || text.contains("fail");
    let names_focused_test_witness = text.contains("red/green")
        || text.contains("base+tests")
        || text.contains("base plus tests")
        || text.contains("old-main")
        || text.contains("old main")
        || text.contains("unpatched")
        || text.contains("base witness")
        || text.contains("red witness")
        || text.contains("green proof")
        || text.contains("live green")
        || text.contains("test sensor")
        || (text.contains("head") && text.contains("base") && text.contains("test"));
    asks_for_witness && names_focused_test_witness
}

pub(crate) fn text_is_test_proof_decision_question(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    if text_requests_focused_test_witness(&text) {
        return true;
    }
    let asks_for_witness = text.contains("proof")
        || text.contains("prove")
        || text.contains("proves")
        || text.contains("proven")
        || text.contains("witness")
        || text.contains("demonstrate")
        || text.contains("demonstrated");
    asks_for_witness
        && (text.contains("asan")
            || text.contains("bad-free")
            || text.contains("oracle")
            || text.contains("post-gc")
            || text.contains("memory-validity"))
}

pub(crate) fn summary_finding_is_test_proof_decision_question(
    finding: &&SummaryOnlyFinding,
) -> bool {
    !lane_is_source_route(&finding.lane) && text_is_test_proof_decision_question(&finding.reason)
}

pub(crate) fn observation_is_test_proof_decision_question(observation: &&ObservationGroup) -> bool {
    !(observation
        .lanes
        .iter()
        .any(|lane| lane_is_source_route(lane))
        || kind_is_source_route_gap(&observation.kind))
        && text_is_test_proof_decision_question(&observation.claim)
}

pub(crate) fn is_pr_body_artifact_only_finding(finding: &SummaryOnlyFinding) -> bool {
    let reason = finding.reason.to_ascii_lowercase();
    let evidence = finding.evidence.to_ascii_lowercase();
    let text = format!("{reason} {evidence}");
    reason.starts_with("inline guard rejected ")
        || reason.contains("severity_allowed=")
        || reason.contains("confidence_allowed=")
        || reason.contains("line_valid=")
        || reason.contains("body_present=")
        || reason.contains("evidence_present=")
        || reason.contains("repo_relative=")
        || evidence == "lane model summary"
        || reason.contains("lane model summary")
        || reason.contains("lane is clean")
        || (reason.contains("no token-scope") && reason.contains("no new"))
        || (reason.contains("no permissions") && reason.contains("no new auth surface"))
        || (reason.contains("no permissions") && reason.contains("no new attack surface"))
        || reason.contains("worth a one-line note for future audits")
        || (reason.contains("supply-chain tightening") && reason.contains("no new scope"))
        || (reason.contains("supply-chain tightening") && reason.contains("not widening"))
        || (reason.contains("passes core opposition tests")
            && reason.contains("remaining concerns"))
        || is_unchanged_workflow_trust_posture_noise(&text)
        || is_no_finding_workflow_pin_summary_noise(&text)
        || is_stale_external_bot_objection_noise(&text)
        || is_workflow_tool_status_artifact_gap_noise(&text)
        || is_workflow_paths_ignore_no_posture_noise(&text)
        || is_actionlint_semantic_skip_proof_noise(&text)
        || is_non_workflow_verifier_scope_noise(&text)
        || is_self_test_meta_review_noise(&text)
        || is_current_pin_consistency_followup_noise(&text)
        || is_workflow_pin_lockstep_no_value_summary_noise(&text)
        || is_pr_body_meta_review_noise(&text)
}

pub(crate) fn is_pr_body_stale_for_current_diff(
    finding: &SummaryOnlyFinding,
    diff: &DiffContext,
) -> bool {
    if diff.diff_class != DiffClass::WorkflowTooling {
        return false;
    }
    let text = format!("{} {}", finding.reason, finding.evidence).to_ascii_lowercase();
    (mentions_workflow_tool_cache_path(&text) && !diff_adds_workflow_tool_cache_path(&diff.patch))
        || (mentions_standing_workflow_secret_trust(&text)
            && diff_is_workflow_tool_pin_bump_only(&diff.patch))
}

pub(crate) fn is_pr_body_stale_for_current_diff_observation(
    observation: &ObservationGroup,
    diff: &DiffContext,
) -> bool {
    if diff.diff_class != DiffClass::WorkflowTooling {
        return false;
    }
    let text =
        format!("{} {}", observation.claim, observation.evidence.join(" ")).to_ascii_lowercase();
    (mentions_workflow_tool_cache_path(&text) && !diff_adds_workflow_tool_cache_path(&diff.patch))
        || (mentions_pr_trigger_synchronize_scope(&text)
            && !diff_adds_pr_trigger_scope(&diff.patch))
        || (mentions_standing_workflow_secret_trust(&text)
            && diff_is_workflow_tool_pin_bump_only(&diff.patch))
}

pub(crate) fn mentions_workflow_tool_cache_path(text: &str) -> bool {
    text.contains("~/go/bin/actionlint")
        || text.contains("~/.cargo/bin/cargo-allow")
        || ((text.contains("actionlint") || text.contains("cargo-allow"))
            && (text.contains("cache path")
                || text.contains("cache paths")
                || text.contains("cached binary")
                || text.contains("cached-binary")))
}

pub(crate) fn diff_adds_workflow_tool_cache_path(patch: &str) -> bool {
    patch.lines().any(|line| {
        line.starts_with('+')
            && !line.starts_with("+++")
            && (line.contains("~/go/bin/actionlint") || line.contains("~/.cargo/bin/cargo-allow"))
    })
}

pub(crate) fn mentions_pr_trigger_synchronize_scope(text: &str) -> bool {
    text.contains("push-not-synchronize")
        || (text.contains("cursor")
            && text.contains("pull_request")
            && (text.contains("synchronize") || text.contains("ready_for_review")))
        || (text.contains("ready_for_review")
            && (text.contains("synchronize")
                || text.contains("pushes to skip")
                || text.contains("skip re-running")
                || text.contains("do not re-run")
                || text.contains("does not re-run")
                || text.contains("evidence can stale")))
}

pub(crate) fn diff_adds_pr_trigger_scope(patch: &str) -> bool {
    patch.lines().any(|line| {
        line.starts_with('+')
            && !line.starts_with("+++")
            && (line.contains("pull_request")
                || line.contains("types:")
                || line.contains("ready_for_review")
                || line.contains("synchronize"))
    })
}

pub(crate) fn mentions_standing_workflow_secret_trust(text: &str) -> bool {
    let mentions_secret_receiver = text.contains("secrets.minimax")
        || text.contains("github.token")
        || text.contains("secret/permission surface")
        || text.contains("secret")
        || text.contains("exposure surface");
    let mentions_standing_action_trust = text.contains("upstream trust")
        || text.contains("trust in upstream")
        || text.contains("malicious or compromised")
        || text.contains("would exfiltrate")
        || text.contains("third-party action token scope")
        || text.contains("residual trust")
        || text.contains("standing-repo concern")
        || text.contains("standing repo concern")
        || text.contains("does not eliminate upstream trust");
    mentions_secret_receiver && mentions_standing_action_trust
}

pub(crate) fn diff_is_workflow_tool_pin_bump_only(patch: &str) -> bool {
    let mut saw_change = false;
    for line in patch.lines() {
        if !(line.starts_with('+') || line.starts_with('-'))
            || line.starts_with("+++")
            || line.starts_with("---")
        {
            continue;
        }
        saw_change = true;
        let changed = line[1..].trim().to_ascii_lowercase();
        if changed.is_empty() {
            continue;
        }
        if !(changed.contains("ub-review-gh-runner-v")
            || changed.starts_with("uses: effortlessmetrics/ub-review@"))
        {
            return false;
        }
    }
    saw_change
}
