//! Proof planning: request construction/grouping, planner artifacts, and
//! receipt routing (cleanup train step 8, pure code motion).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use crate::*;

const MUTATION_HEAVY_WITNESS_SKIP_REASON: &str = "Targeted mutation proof is a heavy witness; skipped because this runtime profile does not lease mutation proof. Use a risk-pack/manual-heavy profile to run it.";
const SANITIZER_HEAVY_WITNESS_SKIP_REASON: &str = "Sanitizer proof is a heavy witness; skipped because this runtime profile does not lease sanitizer proof. Use a risk-pack/manual-heavy profile to run it.";
const MUTATION_HEAVY_WITNESS_PARKED_REASON: &str = "Targeted mutation proof is leased by this runtime profile, but ub-review has no mutation executor route yet; parked as manual-heavy evidence until executor routing lands.";
const SANITIZER_HEAVY_WITNESS_PARKED_REASON: &str = "Sanitizer proof is leased by this runtime profile, but ub-review has no sanitizer executor route yet; parked as manual-heavy evidence until executor routing lands.";

pub(crate) fn append_follow_up_proof_requests(
    proof_requests: &mut Vec<ProofRequest>,
    evidence: &FollowUpEvidenceArtifact,
) {
    let mut seen_ids = proof_requests
        .iter()
        .map(|request| request.id.clone())
        .collect::<BTreeSet<_>>();
    for request in &evidence.proof_requests {
        if !seen_ids.insert(request.id.clone()) {
            continue;
        }
        let mut request = request.clone();
        request.reason = post_broker_follow_up_proof_reason(&request.reason);
        proof_requests.push(request);
    }
}

pub(crate) fn post_broker_follow_up_proof_reason(reason: &str) -> String {
    const NOTE: &str = "Follow-up proof request arrived after primary proof execution; routed to the follow-up broker scheduling pass.";
    if reason.contains(NOTE) {
        return reason.to_owned();
    }
    let reason = reason.trim();
    if reason.is_empty() {
        NOTE.to_owned()
    } else if reason.ends_with('.') || reason.ends_with('!') || reason.ends_with('?') {
        format!("{reason} {NOTE}")
    } else {
        format!("{reason}. {NOTE}")
    }
}

pub(crate) fn build_proof_planner_input<'a>(
    diff: &'a DiffContext,
    profile: &Profile,
    box_state: &'a BoxState,
    pr_thread_context: &'a PrThreadContext,
    proof_requests: &'a [ProofRequest],
) -> Result<ProofPlannerInput<'a>> {
    let budget = proof_budget(profile)?;
    Ok(ProofPlannerInput {
        schema: PROOF_PLANNER_INPUT_SCHEMA,
        diff_class: diff.diff_class.key(),
        changed_files: &diff.changed_files,
        pr_thread_context_status: &pr_thread_context.status,
        proof_requests,
        runtime_budget: ProofPlannerRuntimeBudget {
            target_timeout_sec: profile.budgets.default_timeout_sec,
            hard_timeout_sec: profile.budgets.hard_timeout_sec,
            max_focused_tests: budget.max_focused_tests,
            per_command_timeout_sec: budget.per_command_timeout_sec,
            total_proof_timeout_sec: budget.max_total_seconds,
        },
        box_shape: box_state,
    })
}

pub(crate) fn build_proof_planner_output(
    diff: &DiffContext,
    profile: &Profile,
    proof_requests: &[ProofRequest],
) -> Result<ProofPlannerOutput> {
    let budget = proof_budget(profile)?;
    let lease_budget = proof_lease_budget(profile)?;
    let plans = focused_proof_plans_from_diff(diff, proof_requests, budget);
    let build_plans = focused_build_plans_from_requests(proof_requests, budget);
    let proof_tasks = plans
        .into_iter()
        .map(|plan| proof_task_artifact(plan, budget, lease_budget))
        .chain(
            build_plans
                .into_iter()
                .map(|plan| focused_build_task_artifact(plan, budget, lease_budget)),
        )
        .collect::<Vec<_>>();
    let skip = proof_planner_skips(diff, profile);
    Ok(ProofPlannerOutput {
        schema: PROOF_PLANNER_OUTPUT_SCHEMA,
        lane: "proof-planner",
        proof_tasks,
        skip,
    })
}

pub(crate) fn write_proof_planner_artifacts(
    out: &Path,
    diff: &DiffContext,
    plan: &Plan,
    profile: &Profile,
    box_state: &BoxState,
    pr_thread_context: &PrThreadContext,
    proof_requests: &[ProofRequest],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let input =
        build_proof_planner_input(diff, profile, box_state, pr_thread_context, proof_requests)?;
    let output = build_proof_planner_output(diff, profile, proof_requests)?;
    fs::write(
        review_dir.join("proof_planner_input.json"),
        serde_json::to_vec_pretty(&input)?,
    )?;
    fs::write(
        review_dir.join("proof_planner_output.json"),
        serde_json::to_vec_pretty(&output)?,
    )?;
    let mut ndjson = String::new();
    for task in &output.proof_tasks {
        ndjson.push_str(&serde_json::to_string(task)?);
        ndjson.push('\n');
    }
    fs::write(out.join("proof_tasks.ndjson"), ndjson)?;
    write_work_queue_artifacts(out, plan, &output.proof_tasks)?;
    Ok(())
}

pub(crate) fn proof_planner_skips(diff: &DiffContext, profile: &Profile) -> Vec<ProofPlannerSkip> {
    [
        (!diff.flags.unsafe_or_native_risk).then(|| ProofPlannerSkip {
            kind: "miri".to_owned(),
            reason: "No new unsafe/native aliasing surface was detected; cheaper focused proof is preferred when available.".to_owned(),
        }),
        (!diff.flags.workflow_changed).then(|| ProofPlannerSkip {
            kind: "actionlint".to_owned(),
            reason: "No workflow files changed.".to_owned(),
        }),
        Some(ProofPlannerSkip {
            kind: "mutation".to_owned(),
            reason: if profile.budgets.mutation {
                MUTATION_HEAVY_WITNESS_PARKED_REASON
            } else {
                MUTATION_HEAVY_WITNESS_SKIP_REASON
            }
            .to_owned(),
        }),
        Some(ProofPlannerSkip {
            kind: "sanitizer".to_owned(),
            reason: if profile.budgets.sanitizer {
                SANITIZER_HEAVY_WITNESS_PARKED_REASON
            } else {
                SANITIZER_HEAVY_WITNESS_SKIP_REASON
            }
            .to_owned(),
        }),
    ]
    .into_iter()
    .flatten()
    .collect()
}

pub(crate) fn write_proof_request_artifacts(
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    proof_requests: &[ProofRequest],
    proof_receipts: &[ProofReceipt],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let proof_groups = proof_request_groups(proof_requests);
    let focused_plans = focused_proof_plans_from_diff(diff, proof_requests, proof_budget(profile)?);
    let focused_build_plans =
        focused_build_plans_from_requests(proof_requests, proof_budget(profile)?);
    fs::write(
        review_dir.join("proof_requests.json"),
        serde_json::to_vec_pretty(proof_requests)?,
    )?;
    fs::write(
        review_dir.join("proof_request_groups.json"),
        serde_json::to_vec_pretty(&proof_groups)?,
    )?;

    let proof_request_dir = out.join("proof_requests");
    if proof_request_dir.exists() {
        fs::remove_dir_all(&proof_request_dir)
            .with_context(|| format!("remove {}", proof_request_dir.display()))?;
    }
    fs::create_dir_all(&proof_request_dir)
        .with_context(|| format!("create {}", proof_request_dir.display()))?;

    let mut ndjson = String::new();
    for request in proof_requests {
        ndjson.push_str(&serde_json::to_string(request)?);
        ndjson.push('\n');
        fs::write(
            proof_request_dir.join(format!("{}.json", sanitize_artifact_name(&request.id))),
            serde_json::to_vec_pretty(request)?,
        )?;
    }
    fs::write(out.join("proof_requests.ndjson"), ndjson)?;

    let mut plan = String::new();
    plan.push_str("# Proof request plan\n\n");
    if proof_requests.is_empty() && focused_plans.is_empty() && focused_build_plans.is_empty() {
        plan.push_str("No proof requests were emitted by model lanes.\n");
    } else {
        if proof_requests.is_empty() {
            plan.push_str("No model-lane proof requests were emitted.\n\n");
        } else {
            plan.push_str(&format!(
                "Grouped proof broker tasks: {} unique from {} request(s).\n\n",
                proof_groups.len(),
                proof_requests.len()
            ));
            for group in &proof_groups {
                plan.push_str(&format!(
                    "- `{}` requested by `{}`: `{}` ({}, timeout {}s, required={}, status={}, merged_requests={})\n",
                    group.id,
                    group.requested_by.join(", "),
                    group.command,
                    group.cost,
                    group.timeout_sec,
                    group.required,
                    group.status,
                    group.duplicate_count
                ));
                for reason in &group.reasons {
                    plan.push_str(&format!("  - Reason: {}\n", escape_md(reason)));
                }
            }
            plan.push('\n');
        }
        if focused_plans.is_empty() && focused_build_plans.is_empty() {
            plan.push_str(
                "No focused proof targets were planned from the diff or proof requests.\n",
            );
        } else {
            plan.push_str("## Focused proof plan\n\n");
            if proof_receipts.is_empty() {
                plan.push_str(
                    "No proof broker commands were executed in this planner-only pass.\n\n",
                );
            } else {
                plan.push_str(
                    "Proof broker v0 executed focused proof under the runtime budget.\n\n",
                );
                for receipt in proof_receipts {
                    plan.push_str(&format!(
                        "- Receipt `{}`: kind=`{}`, test_patch_mode=`{}`, result=`{}`, commands=`{}`.\n",
                        receipt.id,
                        receipt.kind,
                        receipt.test_patch_mode,
                        receipt.result,
                        receipt.commands.len()
                    ));
                }
                plan.push('\n');
            }
            for plan_item in focused_plans {
                plan.push_str(&format!(
                    "- `{}` `{}`{} requested by `{}`: mode=`{}`, status=`{}`, cost=`focused-test`, head=`{}`, base+tests=`{}`. {}\n",
                    plan_item.id,
                    plan_item.test_file,
                    plan_item
                        .test_name
                        .as_ref()
                        .map(|name| format!(" - `{}`", escape_md(name)))
                        .unwrap_or_default(),
                    plan_item.requested_by.join(", "),
                    plan_item.mode.key(),
                    plan_item.status,
                    escape_md(&plan_item.head_command),
                    escape_md(&plan_item.base_plus_tests_command),
                    escape_md(&plan_item.reason)
                ));
                if !plan_item.request_ids.is_empty() {
                    plan.push_str(&format!(
                        "  - Merged requests: `{}`\n",
                        plan_item.request_ids.join("`, `")
                    ));
                }
            }
            for plan_item in focused_build_plans {
                plan.push_str(&format!(
                    "- `{}` requested by `{}`: mode=`head-only`, status=`{}`, cost=`focused-build`, head=`{}`. {}\n",
                    plan_item.id,
                    plan_item.requested_by.join(", "),
                    plan_item.status,
                    escape_md(&plan_item.command),
                    escape_md(&plan_item.reason)
                ));
                if !plan_item.request_ids.is_empty() {
                    plan.push_str(&format!(
                        "  - Merged requests: `{}`\n",
                        plan_item.request_ids.join("`, `")
                    ));
                }
            }
        }
    }
    fs::write(review_dir.join("proof_plan.md"), plan)?;
    Ok(())
}

pub(crate) fn receipt_route_artifacts(
    proof_receipts: &[ProofReceipt],
    resource_leases: &[ResourceLease],
) -> Vec<ReceiptRouteArtifact> {
    proof_receipts
        .iter()
        .map(|receipt| {
            let lease_ids = resource_leases
                .iter()
                .filter(|lease| lease.consumer == receipt.id)
                .map(|lease| lease.id.clone())
                .collect::<Vec<_>>();
            let source_artifacts = receipt_route_source_artifacts(&receipt.id, &lease_ids);
            ReceiptRouteArtifact {
                schema: RECEIPT_ROUTE_SCHEMA,
                id: format!("receipt-route-{}", receipt.id),
                receipt_id: receipt.id.clone(),
                phase: receipt_route_phase(receipt).to_owned(),
                receipt_kind: receipt.kind.clone(),
                result: receipt.result.clone(),
                status: routed_status_for_proof_receipt(receipt).to_owned(),
                requested_by: receipt.requested_by.clone(),
                request_ids: receipt.request_ids.clone(),
                consumers: receipt_route_consumers(receipt),
                lease_ids,
                source_artifacts,
                reason: receipt.reason.clone(),
            }
        })
        .collect()
}

fn receipt_route_source_artifacts(receipt_id: &str, lease_ids: &[String]) -> Vec<String> {
    let mut source_artifacts = vec![
        "review/proof_receipts.json".to_owned(),
        format!("review/proof_receipts.json#{receipt_id}"),
    ];
    if !lease_ids.is_empty() {
        source_artifacts.push("review/resource_leases.json".to_owned());
        for lease_id in lease_ids {
            push_unique(
                &mut source_artifacts,
                &format!("review/resource_leases.json#{lease_id}"),
            );
        }
    }
    source_artifacts
}

pub(crate) fn receipt_route_phase(receipt: &ProofReceipt) -> &'static str {
    if receipt
        .requested_by
        .iter()
        .any(|lane| lane.starts_with("orchestrator-follow-up"))
        || receipt
            .request_ids
            .iter()
            .any(|request_id| request_id.contains("follow-up"))
    {
        "follow-up-receipt"
    } else if receipt
        .requested_by
        .iter()
        .any(|lane| lane == "proof-broker")
        && receipt.request_ids.is_empty()
    {
        "initial-diff-receipt"
    } else {
        "model-request-receipt"
    }
}

pub(crate) fn receipt_route_consumers(receipt: &ProofReceipt) -> Vec<String> {
    let mut consumers = Vec::new();
    if receipt
        .requested_by
        .iter()
        .any(|lane| lane == "proof-broker")
    {
        match receipt.kind.as_str() {
            "focused-head" | "focused-red-green" => {
                push_unique(&mut consumers, "tests-oracle");
                push_unique(&mut consumers, "opposition");
            }
            "focused-build" => push_unique(&mut consumers, "architecture"),
            _ => {}
        }
    }
    for lane in &receipt.requested_by {
        if lane != "proof-broker" {
            push_unique(&mut consumers, lane);
        }
    }
    push_unique(&mut consumers, "compiler");
    consumers
}

pub(crate) fn write_receipt_route_artifacts(
    out: &Path,
    routes: &[ReceiptRouteArtifact],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let artifact = ReceiptRoutesArtifact {
        schema: RECEIPT_ROUTES_SCHEMA,
        source_artifacts: vec!["review/proof_receipts.json", "review/resource_leases.json"],
        routes,
    };
    fs::write(
        review_dir.join("receipt_routes.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    let mut ndjson = String::new();
    for route in routes {
        ndjson.push_str(&serde_json::to_string(route)?);
        ndjson.push('\n');
    }
    fs::write(out.join("receipt_routes.ndjson"), ndjson)?;
    Ok(())
}

pub(crate) fn proof_request_groups(proof_requests: &[ProofRequest]) -> Vec<ProofRequestGroup> {
    let mut groups = BTreeMap::<(String, String, u64), ProofRequestGroup>::new();
    for request in proof_requests {
        let group_command = canonical_proof_request_group_command(&request.command, &request.cost);
        let key = (
            group_command.clone(),
            request.cost.clone(),
            request.timeout_sec,
        );
        let fingerprint = sha256_hex(
            format!(
                "{}\n{}\n{}",
                group_command, request.cost, request.timeout_sec
            )
            .as_bytes(),
        );
        let group = groups.entry(key).or_insert_with(|| ProofRequestGroup {
            schema: PROOF_REQUEST_GROUP_SCHEMA.to_owned(),
            id: format!("proof-group-{}", &fingerprint[..12]),
            command: request.command.clone(),
            cost: request.cost.clone(),
            timeout_sec: request.timeout_sec,
            required: false,
            status: "invalid".to_owned(),
            requested_by: Vec::new(),
            request_ids: Vec::new(),
            reasons: Vec::new(),
            duplicate_count: 0,
        });
        group.required |= request.required;
        match request.status.as_str() {
            "requested" => group.status = "requested".to_owned(),
            "unsupported" if group.status != "requested" => {
                group.status = "unsupported".to_owned();
            }
            _ => {}
        }
        push_unique(&mut group.requested_by, &request.lane);
        for lane in &request.requested_by {
            push_unique(&mut group.requested_by, lane);
        }
        push_unique(&mut group.request_ids, &request.id);
        push_unique(&mut group.reasons, &request.reason);
        group.duplicate_count += 1;
    }
    groups.into_values().collect()
}

#[expect(
    clippy::too_many_arguments,
    reason = "proof requests normalize the same fields regardless of source"
)]
pub(crate) fn build_proof_request(
    lane: &str,
    requested_by: Vec<String>,
    command: &str,
    reason: &str,
    missing_reason_fallback: &str,
    cost: Option<&str>,
    timeout_sec: Option<u64>,
    required: bool,
    index: usize,
) -> ProofRequest {
    let command = command.trim().replace(['\r', '\n'], " ");
    let reason = non_empty_or(reason.trim(), missing_reason_fallback);
    let command = non_empty_or(&command, "<missing command>");
    let cost = classify_proof_cost(cost, &command);
    let status = proof_request_status(&command, &cost);
    let timeout_sec = timeout_sec.unwrap_or(300).clamp(1, 900);
    let fingerprint = sha256_hex(
        format!(
            "{}\n{}\n{}\n{}\n{}",
            lane, command, reason, cost, timeout_sec
        )
        .as_bytes(),
    );
    let short = &fingerprint[..12];
    ProofRequest {
        schema: PROOF_REQUEST_SCHEMA.to_owned(),
        id: format!("proof-{index:04}-{short}"),
        lane: lane.to_owned(),
        requested_by,
        command,
        reason,
        cost,
        timeout_sec,
        required,
        status: status.to_owned(),
    }
}

pub(crate) fn configured_required_proof_requests(
    config: &Config,
    diff: &DiffContext,
    args: &RunArgs,
    start_index: usize,
) -> Vec<ProofRequest> {
    if args.mode != RunMode::IntelligentCi {
        return Vec::new();
    }
    let language_mix = classify_language_mix(&diff.changed_files);
    config
        .proof
        .required
        .iter()
        .filter(|policy| required_proof_policy_matches_diff(policy, diff, &language_mix))
        .enumerate()
        .map(|(offset, policy)| {
            let index = start_index + offset;
            let policy_label = proof_policy_requester(policy, offset);
            build_proof_request(
                REQUIRED_PROOF_POLICY_LANE,
                vec![REQUIRED_PROOF_POLICY_LANE.to_owned(), policy_label],
                &policy.command,
                &policy.reason,
                "configured proof policy missing reason",
                policy.cost.as_deref(),
                Some(policy.timeout_sec),
                policy.required,
                index,
            )
        })
        .collect()
}

pub(crate) fn append_configured_required_proof_requests(
    config: &Config,
    diff: &DiffContext,
    args: &RunArgs,
    proof_requests: &mut Vec<ProofRequest>,
) {
    proof_requests.extend(configured_required_proof_requests(
        config,
        diff,
        args,
        proof_requests.len(),
    ));
}

pub(crate) fn proof_policy_requester(policy: &RequiredProofPolicy, index: usize) -> String {
    let id = policy.id.trim();
    if id.is_empty() {
        format!("proof-policy:required-{index}")
    } else {
        format!("proof-policy:{id}")
    }
}

pub(crate) fn proof_request_status(command: &str, cost: &str) -> &'static str {
    if command == "<missing command>" {
        return "invalid";
    }
    if proof_request_allowed_v0(command, cost) {
        "requested"
    } else {
        "unsupported"
    }
}

pub(crate) fn proof_request_allowed_v0(command: &str, cost: &str) -> bool {
    if has_shell_control_token(command) {
        return false;
    }
    match cost {
        "focused-test" => {
            let parts = command.split_whitespace().collect::<Vec<_>>();
            if let Some((file, _args)) = focused_bun_request_parts(&parts) {
                is_bun_focused_test_file(file)
            } else {
                focused_cargo_test_command_spec(command).is_some()
            }
        }
        "focused-build" => focused_build_command_spec(command).is_some(),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use crate::tests::test_diff;
    use crate::*;

    #[test]
    fn proof_request_artifacts_group_duplicate_commands_once() -> Result<()> {
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts".to_owned(),
                reason: "Need old-main red witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-opposition-001".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts".to_owned(),
                reason: "Confirm the same focused test before posting.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: true,
                status: "requested".to_owned(),
            },
        ];

        let temp = tempfile::tempdir()?;
        write_proof_request_artifacts(
            temp.path(),
            &test_diff(),
            &Profile::default(),
            &proof_requests,
            &[] as &[ProofReceipt],
        )?;

        let proof_json: Vec<super::ProofRequest> =
            serde_json::from_slice(&fs::read(temp.path().join("review/proof_requests.json"))?)?;
        let proof_groups: Vec<ProofRequestGroup> = serde_json::from_slice(&fs::read(
            temp.path().join("review/proof_request_groups.json"),
        )?)?;
        let proof_plan = fs::read_to_string(temp.path().join("review/proof_plan.md"))?;
        let proof_ndjson = fs::read_to_string(temp.path().join("proof_requests.ndjson"))?;

        assert_eq!(proof_json.len(), 2);
        assert_eq!(proof_ndjson.lines().count(), 2);
        assert_eq!(proof_groups.len(), 1);
        let group = &proof_groups[0];
        assert_eq!(group.schema, "ub-review.proof_request_group.v1");
        assert_eq!(
            group.command,
            "bun test test/js/bun/md/md-edge-cases.test.ts"
        );
        assert_eq!(
            group.requested_by,
            vec!["tests-oracle".to_owned(), "opposition".to_owned()]
        );
        assert_eq!(
            group.request_ids,
            vec![
                "proof-tests-001".to_owned(),
                "proof-opposition-001".to_owned()
            ]
        );
        assert_eq!(group.reasons.len(), 2);
        assert_eq!(group.duplicate_count, 2);
        assert!(group.required);
        assert_eq!(group.status, "requested");
        assert!(proof_plan.contains("Grouped proof broker tasks: 1 unique from 2 request(s)."));
        assert!(proof_plan.contains("merged_requests=2"));
        Ok(())
    }

    #[test]
    fn focused_proof_tasks_detect_changed_test_names_and_merge_lane_requests() -> Result<()> {
        let patch = "\
diff --git a/test/js/bun/md/md-edge-cases.test.ts b/test/js/bun/md/md-edge-cases.test.ts
index 1111111..2222222 100644
--- a/test/js/bun/md/md-edge-cases.test.ts
+++ b/test/js/bun/md/md-edge-cases.test.ts
@@ -1,2 +1,4 @@
 import { test } from 'bun:test';
+test(\"snapshots resizable ArrayBuffer input\", () => {});
+it('keeps stable bytes after getter reentry', () => {});
";
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["test/js/bun/md/md-edge-cases.test.ts".to_owned()],
            patch: patch.to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        };
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Need green witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 120,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-opposition-001".to_owned(),
                lane: "opposition".to_owned(),
                requested_by: vec!["opposition".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Confirm the same focused test.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 180,
                required: true,
                status: "requested".to_owned(),
            },
        ];

        let tasks = focused_test_tasks_from_diff(
            &diff,
            &proof_requests,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 6,
                per_command_timeout_sec: 300,
                max_total_seconds: 1_200,
            },
        );

        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].file, "test/js/bun/md/md-edge-cases.test.ts");
        assert_eq!(tasks[0].mode, super::FocusedProofMode::RedGreen);
        assert_eq!(tasks[0].timeout_sec, Some(180));
        assert_eq!(
            tasks[0].test_name.as_deref(),
            Some("snapshots resizable ArrayBuffer input")
        );
        assert_eq!(tasks[0].requested_by.len(), 2);
        assert!(
            tasks[0]
                .requested_by
                .iter()
                .any(|lane| lane == "tests-oracle")
        );
        assert!(
            tasks[0]
                .requested_by
                .iter()
                .any(|lane| lane == "opposition")
        );
        assert_eq!(tasks[0].request_ids.len(), 2);
        assert!(
            tasks[0]
                .request_ids
                .iter()
                .any(|request_id| request_id == "proof-tests-001")
        );
        assert!(
            tasks[0]
                .request_ids
                .iter()
                .any(|request_id| request_id == "proof-opposition-001")
        );
        assert_eq!(
            tasks[1].test_name.as_deref(),
            Some("keeps stable bytes after getter reentry")
        );
        assert_eq!(tasks[1].mode, super::FocusedProofMode::RedGreen);
        assert_eq!(tasks[1].timeout_sec, None);
        let plans = super::focused_proof_plans_from_diff(
            &diff,
            &proof_requests,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 6,
                per_command_timeout_sec: 300,
                max_total_seconds: 1_200,
            },
        );
        assert_eq!(plans[0].timeout_sec, 180);
        assert_eq!(plans[1].timeout_sec, 300);
        let artifact = super::proof_task_artifact(
            plans[0].clone(),
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 6,
                per_command_timeout_sec: 300,
                max_total_seconds: 1_200,
            },
            super::proof_lease_budget(&Profile::default())?,
        );
        assert_eq!(artifact.timeout_sec, 360);
        let time_capped_tasks = focused_test_tasks_from_diff(
            &diff,
            &proof_requests,
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 6,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
        );
        assert_eq!(time_capped_tasks.len(), 1);
        assert_eq!(proof_budget(&Profile::default())?.max_focused_tests, 1);
        Ok(())
    }

    #[test]
    fn focused_proof_tasks_detect_modified_bun_test_callees() -> Result<()> {
        assert_eq!(
            super::extract_focused_test_name("test.only(\"bad free witness\", () => {})")
                .as_deref(),
            Some("bad free witness")
        );
        assert_eq!(
            super::extract_focused_test_name(
                "it.skip('parked until runtime supports it', () => {})"
            )
            .as_deref(),
            Some("parked until runtime supports it")
        );
        assert_eq!(
            super::extract_focused_test_name(
                "test.concurrent.failing('racy callback path', () => {})"
            )
            .as_deref(),
            Some("racy callback path")
        );
        assert_eq!(
            super::extract_focused_test_name(
                "test.each([[\"arraybuffer\"]])('table case %s', () => {})"
            )
            .as_deref(),
            Some("table case %s")
        );
        assert_eq!(
            super::extract_focused_test_name(
                "describe.each([{ name: \"ffi\" }])('ownership %s', () => {})"
            )
            .as_deref(),
            Some("ownership %s")
        );
        assert!(super::extract_focused_test_name("testHelper('not a test', () => {})").is_none());
        assert!(
            super::extract_focused_test_name("test.unknown('not brokered', () => {})").is_none()
        );

        let patch = "\
diff --git a/test/js/bun/ffi/ffi.test.js b/test/js/bun/ffi/ffi.test.js
index 1111111..2222222 100644
--- a/test/js/bun/ffi/ffi.test.js
+++ b/test/js/bun/ffi/ffi.test.js
@@ -1,2 +1,7 @@
 import { test, describe } from 'bun:test';
+test.only(\"bad free witness\", () => {});
+it.skip('parked until runtime supports it', () => {});
+test.concurrent.failing('racy callback path', () => {});
+test.each([[\"arraybuffer\"]])('table case %s', () => {});
+describe.each([{ name: \"ffi\" }])('ownership %s', () => {});
";
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["test/js/bun/ffi/ffi.test.js".to_owned()],
            patch: patch.to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        };

        let tasks = focused_test_tasks_from_diff(
            &diff,
            &[],
            ProofBudget {
                max_focused_test_files: 2,
                max_focused_tests: 8,
                per_command_timeout_sec: 300,
                max_total_seconds: 4_800,
            },
        );
        let names = tasks
            .iter()
            .map(|task| task.test_name.as_deref().unwrap_or_default())
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "bad free witness",
                "parked until runtime supports it",
                "racy callback path",
                "table case %s",
                "ownership %s",
            ]
        );
        assert!(
            tasks
                .iter()
                .all(|task| task.mode == super::FocusedProofMode::RedGreen)
        );
        Ok(())
    }

    #[test]
    fn proof_planner_skips_actionlint_when_workflows_are_unchanged() {
        let mut diff = test_diff();
        diff.flags.workflow_changed = false;

        let skips = super::proof_planner_skips(&diff, &Profile::default());

        assert!(skips.iter().any(|skip| {
            skip.kind == "actionlint" && skip.reason == "No workflow files changed."
        }));
    }

    #[test]
    fn proof_planner_keeps_actionlint_relevant_for_workflow_changes() {
        let mut diff = test_diff();
        diff.flags.workflow_changed = true;
        diff.changed_files = vec![".github/workflows/ci.yml".to_owned()];

        let skips = super::proof_planner_skips(&diff, &Profile::default());

        assert!(!skips.iter().any(|skip| skip.kind == "actionlint"));
    }

    #[test]
    fn proof_planner_records_heavy_witness_skips_without_leases() -> Result<()> {
        let diff = test_diff();

        let output = super::build_proof_planner_output(&diff, &Profile::default(), &[])?;
        let skips = output.skip;
        let mutation_skips = skips
            .iter()
            .filter(|skip| skip.kind == "mutation")
            .collect::<Vec<_>>();
        let sanitizer_skips = skips
            .iter()
            .filter(|skip| skip.kind == "sanitizer")
            .collect::<Vec<_>>();

        assert_eq!(mutation_skips.len(), 1);
        assert_eq!(sanitizer_skips.len(), 1);
        assert_eq!(
            mutation_skips[0].reason,
            super::MUTATION_HEAVY_WITNESS_SKIP_REASON
        );
        assert_eq!(
            sanitizer_skips[0].reason,
            super::SANITIZER_HEAVY_WITNESS_SKIP_REASON
        );
        Ok(())
    }

    #[test]
    fn proof_planner_parks_leased_heavy_witnesses_until_executors_route_them() -> Result<()> {
        let diff = test_diff();
        let mut profile = Profile::default();
        profile.budgets.mutation = true;
        profile.budgets.sanitizer = true;

        let output = super::build_proof_planner_output(&diff, &profile, &[])?;
        let skips = output.skip;
        let mutation = skips
            .iter()
            .find(|skip| skip.kind == "mutation")
            .ok_or_else(|| anyhow::anyhow!("leased mutation skip missing"))?;
        let sanitizer = skips
            .iter()
            .find(|skip| skip.kind == "sanitizer")
            .ok_or_else(|| anyhow::anyhow!("leased sanitizer skip missing"))?;

        assert_eq!(mutation.reason, super::MUTATION_HEAVY_WITNESS_PARKED_REASON);
        assert_eq!(
            sanitizer.reason,
            super::SANITIZER_HEAVY_WITNESS_PARKED_REASON
        );
        assert!(mutation.reason.contains("no mutation executor route yet"));
        assert!(sanitizer.reason.contains("no sanitizer executor route yet"));
        Ok(())
    }

    #[test]
    fn proof_request_status_enforces_v0_focused_allowlist() -> Result<()> {
        let lane = model_lane(
            "tests-oracle",
            "Tests oracle",
            &["tokmd"],
            "Check focused proof requests.",
        );
        let requests = vec![
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots input'"
                        .to_owned(),
                    reason: "Run the focused Bun test.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(true),
                },
                0,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command:
                        "bun bd test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots input'"
                            .to_owned(),
                    reason: "Confirm the patched Bun development binary.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                1,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command:
                        "USE_SYSTEM_BUN=1 bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots input'"
                            .to_owned(),
                    reason: "Confirm the old-main red side shape exactly.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                2,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command:
                        "cargo test --locked -p ub-review proof_request_status_enforces_v0_focused_allowlist -- --exact"
                            .to_owned(),
                    reason: "Run the focused Cargo regression test.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                3,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "cargo test --workspace --all-targets --locked".to_owned(),
                    reason: "Broad cargo test should not be brokered as focused proof."
                        .to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                4,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "cargo test --workspace".to_owned(),
                    reason: "Unlocked cargo test should not be brokered.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                5,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "cargo check --workspace --all-targets --locked".to_owned(),
                    reason: "Compile the workspace.".to_owned(),
                    cost: Some("focused-build".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                6,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "cargo build --workspace".to_owned(),
                    reason: "Unlocked build should not be brokered.".to_owned(),
                    cost: Some("focused-build".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                7,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "FOO=1 bun test test/js/bun/md/md-edge-cases.test.ts".to_owned(),
                    reason: "Arbitrary env assignment should not be brokered.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                8,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "USE_SYSTEM_BUN=2 bun test test/js/bun/md/md-edge-cases.test.ts"
                        .to_owned(),
                    reason: "Only exact USE_SYSTEM_BUN=1 is allowed.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                9,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: "bun test test/js/bun/md/md-edge-cases.test.ts && rm -rf target"
                        .to_owned(),
                    reason: "Shell-shaped command should not be brokered.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                10,
            ),
            super::validate_proof_request(
                &lane,
                super::ModelProofRequest {
                    command: String::new(),
                    reason: "Missing command.".to_owned(),
                    cost: Some("focused-test".to_owned()),
                    timeout_sec: Some(300),
                    required: Some(false),
                },
                11,
            ),
        ];

        assert_eq!(requests[0].status, "requested");
        assert_eq!(requests[0].cost, "focused-test");
        assert_eq!(requests[1].status, "requested");
        assert_eq!(requests[1].cost, "focused-test");
        assert_eq!(requests[2].status, "requested");
        assert_eq!(requests[2].cost, "focused-test");
        assert_eq!(requests[3].status, "requested");
        assert_eq!(requests[3].cost, "focused-test");
        assert_eq!(requests[4].status, "unsupported");
        assert_eq!(requests[5].status, "unsupported");
        assert_eq!(requests[6].status, "requested");
        assert_eq!(requests[6].cost, "focused-build");
        assert_eq!(requests[7].status, "unsupported");
        assert_eq!(requests[8].status, "unsupported");
        assert_eq!(requests[9].status, "unsupported");
        assert_eq!(requests[10].status, "unsupported");
        assert_eq!(requests[11].status, "invalid");
        assert_eq!(requests[11].command, "<missing command>");

        let groups = super::proof_request_groups(&requests);
        assert_eq!(groups.len(), 10);
        assert!(groups.iter().any(|group| group.status == "requested"));
        assert!(groups.iter().any(|group| group.status == "unsupported"));
        assert!(groups.iter().any(|group| group.status == "invalid"));
        let bun_group = groups
            .iter()
            .find(|group| {
                group
                    .request_ids
                    .iter()
                    .any(|request_id| request_id == &requests[2].id)
            })
            .ok_or_else(|| anyhow::anyhow!("missing grouped Bun proof request"))?;
        assert_eq!(bun_group.duplicate_count, 3);
        assert_eq!(
            bun_group.request_ids,
            vec![
                requests[0].id.clone(),
                requests[1].id.clone(),
                requests[2].id.clone()
            ]
        );

        let task = super::focused_test_task(
            "test/js/bun/md/md-edge-cases.test.ts",
            Some("snapshots input".to_owned()),
            &groups,
        );
        assert_eq!(task.requested_by, vec!["tests-oracle".to_owned()]);
        assert_eq!(task.request_ids.len(), 3);
        assert!(task.request_ids.contains(&requests[0].id));
        assert!(task.request_ids.contains(&requests[1].id));
        assert!(task.request_ids.contains(&requests[2].id));
        let focused_test_tasks = super::focused_test_candidates_from_requests(&requests);
        assert_eq!(focused_test_tasks.len(), 2);
        let cargo_test_task = focused_test_tasks
            .iter()
            .find(|task| task.command_specs.is_some())
            .ok_or_else(|| {
                anyhow::anyhow!("safe cargo test proof request should become a focused test task")
            })?;
        assert_eq!(cargo_test_task.file, "cargo-package:ub-review");
        assert_eq!(
            cargo_test_task.test_name.as_deref(),
            Some("proof_request_status_enforces_v0_focused_allowlist")
        );
        assert_eq!(cargo_test_task.request_ids, vec![requests[3].id.clone()]);
        let build_tasks = super::focused_build_candidates_from_requests(&requests);
        assert_eq!(build_tasks.len(), 1);
        assert_eq!(
            build_tasks[0].command,
            "cargo check --workspace --all-targets --locked"
        );
        assert_eq!(build_tasks[0].requested_by, vec!["tests-oracle".to_owned()]);
        assert_eq!(build_tasks[0].request_ids, vec![requests[6].id.clone()]);
        Ok(())
    }

    #[test]
    fn proof_request_status_rejects_manual_cost_and_shell_tokens() {
        let lane = model_lane(
            "tests-oracle",
            "Tests oracle",
            &["tokmd"],
            "Check focused proof requests.",
        );
        let requests = [
            super::ModelProofRequest {
                command:
                    "cargo test --locked -p ub-review proof_request_status_enforces_v0_focused_allowlist -- --exact"
                        .to_owned(),
                reason: "Manual-cost command must not be brokered automatically.".to_owned(),
                cost: Some("manual".to_owned()),
                timeout_sec: Some(300),
                required: Some(false),
            },
            super::ModelProofRequest {
                command: "cargo check --workspace --all-targets --locked && cargo test --locked"
                    .to_owned(),
                reason: "Shell control tokens must not reach execution.".to_owned(),
                cost: Some("focused-build".to_owned()),
                timeout_sec: Some(300),
                required: Some(false),
            },
            super::ModelProofRequest {
                command: "bun test test/js/bun/md/md-edge-cases.test.ts; rm -rf target"
                    .to_owned(),
                reason: "Shell control tokens must not be brokered even with manual cost."
                    .to_owned(),
                cost: Some("manual".to_owned()),
                timeout_sec: Some(300),
                required: Some(false),
            },
        ]
        .into_iter()
        .enumerate()
        .map(|(index, request)| super::validate_proof_request(&lane, request, index))
        .collect::<Vec<_>>();

        assert_eq!(requests[0].cost, "manual");
        assert_eq!(requests[0].status, "unsupported");
        assert_eq!(requests[1].cost, "focused-build");
        assert_eq!(requests[1].status, "unsupported");
        assert_eq!(requests[2].cost, "manual");
        assert_eq!(requests[2].status, "unsupported");
        assert!(
            super::proof_request_groups(&requests)
                .iter()
                .all(|group| group.status != "requested")
        );
    }

    #[test]
    fn proof_request_artifacts_write_focused_planner_without_execution() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let patch = "\
diff --git a/test/js/bun/md/md-edge-cases.test.ts b/test/js/bun/md/md-edge-cases.test.ts
index 1111111..2222222 100644
--- a/test/js/bun/md/md-edge-cases.test.ts
+++ b/test/js/bun/md/md-edge-cases.test.ts
@@ -1,2 +1,3 @@
 import { test } from 'bun:test';
+test(\"snapshots resizable ArrayBuffer input\", () => {});
";
        let diff = DiffContext {
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            changed_files: vec!["test/js/bun/md/md-edge-cases.test.ts".to_owned()],
            patch: patch.to_owned(),
            flags: DiffFlags::default(),
            diff_class: DiffClass::TestsOnly,
        };
        let proof_requests = vec![
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-tests-001".to_owned(),
                lane: "tests-oracle".to_owned(),
                requested_by: vec!["tests-oracle".to_owned()],
                command: "bun test test/js/bun/md/md-edge-cases.test.ts -t 'snapshots resizable ArrayBuffer input'".to_owned(),
                reason: "Need focused red/green witness.".to_owned(),
                cost: "focused-test".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
            ProofRequest {
                schema: "ub-review.proof_request.v1".to_owned(),
                id: "proof-build-001".to_owned(),
                lane: "architecture".to_owned(),
                requested_by: vec!["architecture".to_owned()],
                command: "cargo check --workspace --all-targets --locked".to_owned(),
                reason: "Compile proof is brokered as a focused build.".to_owned(),
                cost: "focused-build".to_owned(),
                timeout_sec: 300,
                required: false,
                status: "requested".to_owned(),
            },
        ];

        write_proof_request_artifacts(
            temp.path(),
            &diff,
            &Profile::default(),
            &proof_requests,
            &[] as &[ProofReceipt],
        )?;

        let proof_plan = fs::read_to_string(temp.path().join("review/proof_plan.md"))?;
        let proof_groups: Vec<ProofRequestGroup> = serde_json::from_slice(&fs::read(
            temp.path().join("review/proof_request_groups.json"),
        )?)?;

        assert!(proof_plan.contains("## Focused proof plan"));
        assert!(proof_plan.contains("mode=`red-green`"));
        assert!(proof_plan.contains("cost=`focused-build`"));
        assert!(proof_plan.contains("cargo check --workspace --all-targets --locked"));
        assert!(proof_plan.contains("No proof broker commands were executed"));
        assert!(
            proof_groups
                .iter()
                .any(|group| { group.status == "requested" && group.cost == "focused-build" })
        );
        assert!(proof_plan.contains(
            "head=`cwd=target/ub-review/proof-worktrees/head bun bd test test/js/bun/md/md-edge-cases.test.ts -t"
        ));
        assert!(
            proof_plan.contains(
                "base+tests=`cwd=target/ub-review/proof-worktrees/base-plus-tests USE_SYSTEM_BUN=1 bun test test/js/bun/md/md-edge-cases.test.ts -t"
            )
        );
        assert!(!temp.path().join("review/proof_receipts.json").exists());
        assert!(!temp.path().join("proof_receipts.ndjson").exists());
        Ok(())
    }

    #[test]
    fn receipt_route_source_artifacts_include_exact_anchors() {
        assert_eq!(
            super::receipt_route_source_artifacts(
                "proof-initial",
                &[
                    "lease-proof-initial".to_owned(),
                    "lease-proof-initial".to_owned()
                ]
            ),
            vec![
                "review/proof_receipts.json",
                "review/proof_receipts.json#proof-initial",
                "review/resource_leases.json",
                "review/resource_leases.json#lease-proof-initial"
            ]
        );
        assert_eq!(
            super::receipt_route_source_artifacts("proof-model", &[]),
            vec![
                "review/proof_receipts.json",
                "review/proof_receipts.json#proof-model"
            ]
        );
    }

    #[test]
    fn receipt_routes_capture_initial_model_and_follow_up_consumers() {
        let initial = ProofReceipt {
            schema: "ub-review.proof_receipt.v1".to_owned(),
            id: "proof-initial".to_owned(),
            kind: "focused-red-green".to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            test_patch_mode: "base-plus-tests".to_owned(),
            requested_by: vec!["proof-broker".to_owned()],
            request_ids: Vec::new(),
            commands: Vec::new(),
            result: "discriminating".to_owned(),
            reason: "HEAD passed; base+tests failed".to_owned(),
        };
        let model_request = ProofReceipt {
            schema: "ub-review.proof_receipt.v1".to_owned(),
            id: "proof-model".to_owned(),
            kind: "focused-build".to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["architecture".to_owned()],
            request_ids: vec!["proof-request-1".to_owned()],
            commands: Vec::new(),
            result: "head_passed".to_owned(),
            reason: "focused build passed".to_owned(),
        };
        let follow_up = ProofReceipt {
            schema: "ub-review.proof_receipt.v1".to_owned(),
            id: "proof-follow-up".to_owned(),
            kind: "focused-head".to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["orchestrator-follow-up-tests-oracle".to_owned()],
            request_ids: vec!["proof-follow-up-1".to_owned()],
            commands: Vec::new(),
            result: "skipped_budget".to_owned(),
            reason: "budget exhausted".to_owned(),
        };
        let lease = ResourceLease {
            schema: "ub-review.resource_lease.v1".to_owned(),
            id: "lease-proof-initial".to_owned(),
            kind: "focused-test".to_owned(),
            consumer: "proof-initial".to_owned(),
            status: "granted".to_owned(),
            reason: "lease granted".to_owned(),
            cpu: 2,
            memory_mb: 2048,
            disk_mb: 1024,
            timeout_sec: 300,
            network: false,
            worktree: None,
            command: None,
            scratch: true,
        };

        let routes = super::receipt_route_artifacts(&[initial, model_request, follow_up], &[lease]);
        assert_eq!(routes.len(), 3);
        assert_eq!(routes[0].phase, "initial-diff-receipt");
        assert_eq!(
            routes[0].consumers,
            vec!["tests-oracle", "opposition", "compiler"]
        );
        assert_eq!(routes[0].lease_ids, vec!["lease-proof-initial"]);
        assert_eq!(
            routes[0].source_artifacts,
            vec![
                "review/proof_receipts.json",
                "review/proof_receipts.json#proof-initial",
                "review/resource_leases.json",
                "review/resource_leases.json#lease-proof-initial"
            ]
        );
        assert_eq!(routes[0].status, "tool-confirmed");
        assert_eq!(routes[1].phase, "model-request-receipt");
        assert_eq!(routes[1].consumers, vec!["architecture", "compiler"]);
        assert_eq!(
            routes[1].source_artifacts,
            vec![
                "review/proof_receipts.json",
                "review/proof_receipts.json#proof-model"
            ]
        );
        assert_eq!(routes[2].phase, "follow-up-receipt");
        assert_eq!(
            routes[2].consumers,
            vec!["orchestrator-follow-up-tests-oracle", "compiler"]
        );
        assert_eq!(routes[2].status, "missing-evidence");
    }
}
