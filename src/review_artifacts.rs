//! Review artifact emission: writes review/sensor/model packets, the
//! GitHub review payload, terminal-state and review-metrics receipts
//! (cleanup train step 61, pure code motion).

use crate::*;

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn write_review_artifacts(
    root: &Path,
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    box_state: &BoxState,
    plan: &Plan,
    tool_gate_outcomes: &[ToolGateOutcomeEntry],
    running_summary: &str,
    pr_thread_context: PrThreadContext,
    args: &RunArgs,
    event_log: &EventLog,
    run_started: &Instant,
    run_loop_tracker: &mut RunLoopTracker,
    elapsed: Duration,
) -> Result<GateOutcome> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    let profile = config.selected_profile()?;
    let provider_concurrency = provider_concurrency_limits(config);
    let mut proof_requests = Vec::new();
    append_configured_required_proof_requests(config, diff, args, &mut proof_requests);
    let shared_context = render_shared_context(
        root,
        out,
        config,
        diff,
        plan,
        running_summary,
        args,
        &pr_thread_context,
        profile,
        &proof_requests,
    )?;
    fs::write(review_dir.join("shared_context.md"), &shared_context)?;
    fs::write(
        review_dir.join("pr_thread_context.json"),
        serde_json::to_vec_pretty(&pr_thread_context)?,
    )?;
    let prior_resolved_candidates = load_prior_resolved_candidates(root, out, args)?;
    let shared_context_id = sha256_hex(shared_context.as_bytes());
    let line_map = right_side_diff_lines(&diff.patch);
    let assignments = model_assignments(plan, args)?;
    let mut provider_preflights = build_provider_preflight_receipts(&assignments, args);
    if should_run_proof_planner_model_lane(args, diff) {
        ensure_provider_preflight_receipts_for_assignment(
            &mut provider_preflights,
            &proof_planner_assignment(args),
            args,
        );
    }
    write_shared_context_cache_artifacts(
        out,
        &shared_context,
        &assignments,
        &provider_preflights,
        &[],
        &[],
        args,
    )?;
    let mut model_lanes = build_model_lane_receipts(&assignments, args);
    let missing_or_failed_sensor_evidence = collect_sensor_evidence_issues(out, plan);
    let mut missing_or_failed_model_evidence = model_lanes
        .iter()
        .filter(|receipt| is_model_receipt_evidence_issue(receipt))
        .map(model_issue_from_receipt)
        .collect::<Vec<_>>();
    let mut summary_only_findings = Vec::new();
    let mut inline_comments = Vec::new();
    let mut model_observations = Vec::new();
    let mut issue_candidates: Vec<IssueCandidate> = Vec::new();
    let mut model_calls_used = 0usize;
    let seeded_proof_requests = proof_requests.clone();

    let mut proof_result = ProofBrokerResult::default();
    if matches!(args.model_mode, ModelMode::Auto) {
        let initial_proof_loop = start_run_loop(
            event_log,
            run_started,
            "proof",
            "proof",
            "initial-diff-broker",
        )?;
        thread::scope(|scope| -> Result<()> {
            let proof_handle = scope.spawn(move || {
                run_seeded_proof_stream_v0(
                    root,
                    out,
                    diff,
                    profile,
                    args,
                    &seeded_proof_requests,
                    initial_proof_loop,
                    event_log,
                    run_started,
                )
            });

            let model_loop =
                start_run_loop(event_log, run_started, "model", "investigation", "primary")?;
            run_provider_preflights(
                root,
                &review_dir,
                &mut provider_preflights,
                &shared_context,
                args,
            )?;
            append_preflight_evidence_issues(
                &provider_preflights,
                &mut missing_or_failed_model_evidence,
            );
            model_calls_used = run_available_model_lanes(
                ModelRunContext {
                    root,
                    review_dir: &review_dir,
                    assignments: &assignments,
                    provider_preflights: &provider_preflights,
                    shared_context: &shared_context,
                    args,
                    line_map: &line_map,
                    key_present: env_value_present,
                    provider_concurrency,
                },
                &mut model_lanes,
                &mut missing_or_failed_model_evidence,
                &mut inline_comments,
                &mut summary_only_findings,
                &mut model_observations,
                &mut proof_requests,
                &mut issue_candidates,
            )?;
            dedupe_inline_comments(&mut inline_comments, &mut summary_only_findings);
            apply_unsafe_review_comment_plan_candidates(
                &out.join("sensors").join("unsafe-review"),
                &line_map,
                ModelOutputSinks {
                    inline_comments: &mut inline_comments,
                    summary_only_findings: &mut summary_only_findings,
                    model_observations: &mut model_observations,
                    proof_requests: &mut proof_requests,
                    issue_candidates: &mut issue_candidates,
                },
            );
            dedupe_inline_comments(&mut inline_comments, &mut summary_only_findings);
            model_calls_used += run_proof_planner_model_lane(
                ProofPlannerRunContext {
                    root,
                    review_dir: &review_dir,
                    provider_preflights: &provider_preflights,
                    shared_context: &shared_context,
                    args,
                    diff,
                    profile,
                    box_state,
                    pr_thread_context: &pr_thread_context,
                    model_calls_used,
                    key_present: env_value_present,
                    line_map: &line_map,
                },
                &mut model_lanes,
                &mut missing_or_failed_model_evidence,
                &mut model_observations,
                &mut proof_requests,
            )?;
            model_calls_used += run_refuter_pass(
                RefuterRunContext {
                    root,
                    review_dir: &review_dir,
                    provider_preflights: &provider_preflights,
                    shared_context: &shared_context,
                    args,
                    model_calls_used,
                },
                &mut model_lanes,
                &mut missing_or_failed_model_evidence,
                &mut inline_comments,
                &mut summary_only_findings,
            )?;
            append_cross_lane_conflict_observations(
                &inline_comments,
                &summary_only_findings,
                &mut model_observations,
            );
            finish_run_loop(
                event_log,
                run_started,
                run_loop_tracker,
                model_loop,
                "completed",
            )?;

            let (seeded_result, proof_phases) = proof_handle
                .join()
                .map_err(|_| anyhow::anyhow!("seeded proof stream thread panicked"))??;
            for phase in proof_phases {
                run_loop_tracker.record(phase);
            }
            proof_result = seeded_result;
            Ok(())
        })?;
    } else {
        let model_loop =
            start_run_loop(event_log, run_started, "model", "investigation", "primary")?;
        finish_run_loop(
            event_log,
            run_started,
            run_loop_tracker,
            model_loop,
            "skipped_model_mode_off",
        )?;
        let initial_proof_loop = start_run_loop(
            event_log,
            run_started,
            "proof",
            "proof",
            "initial-diff-broker",
        )?;
        proof_result = run_initial_diff_proof_broker_v0(root, out, diff, profile, args)?;
        finish_run_loop(
            event_log,
            run_started,
            run_loop_tracker,
            initial_proof_loop,
            "completed",
        )?;
    }
    attach_request_metadata_to_focused_receipts(
        diff,
        &proof_requests,
        &mut proof_result.proof_receipts,
    );
    write_proof_planner_artifacts(
        out,
        diff,
        plan,
        profile,
        box_state,
        &pr_thread_context,
        &proof_requests,
    )?;
    if has_unreceipted_proof_request_tasks(&proof_requests, &proof_result.proof_receipts) {
        let request_proof_loop = start_run_loop(
            event_log,
            run_started,
            "proof",
            "proof",
            "model-request-broker",
        )?;
        let request_proof_result = run_request_proof_broker_v0(
            root,
            out,
            diff,
            profile,
            &proof_requests,
            &proof_result.proof_receipts,
            &proof_result.resource_leases,
            args,
        )?;
        proof_result
            .proof_receipts
            .extend(request_proof_result.proof_receipts);
        proof_result
            .resource_leases
            .extend(request_proof_result.resource_leases);
        finish_run_loop(
            event_log,
            run_started,
            run_loop_tracker,
            request_proof_loop,
            "completed",
        )?;
    }
    let proof_receipts = proof_result.proof_receipts;
    let resource_leases = proof_result.resource_leases;
    let compiler_loop = start_run_loop(
        event_log,
        run_started,
        "compiler",
        "coordination",
        "preliminary",
    )?;
    // Invariant (#314): candidates are built from the same inline-comment
    // and summary-finding values the final compile later filters through
    // `candidate_matches_inline_comment` / `candidate_matches_summary_finding`.
    // No dedupe, trim, or other normalization may run between this point and
    // that filter - the matchers compare exact fields, and a normalized
    // surface would fail the match OPEN, letting a refuted candidate post.
    // The verifier's negative self-test pins the closed loop from the other
    // side: a leaked refuted surface in final_compiler_input.json reds the
    // gate's verifier step.
    let candidates = build_candidate_records(&inline_comments, &summary_only_findings);
    write_candidate_artifacts(out, &candidates)?;
    let candidates = read_candidate_records(out)?;
    let (inline_comments, summary_only_findings) = read_candidate_review_surfaces(out)?;

    let run_pass = resolved_run_pass(args.run_pass);
    let preliminary_surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: &shared_context_id,
        review_body_policy: &config.review_body,
        run_pass,
        post_review_on: &config.gate.post_review_on,
        args,
        plan,
        diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence: &missing_or_failed_model_evidence,
        inline_comments: &inline_comments,
        summary_only_findings: &summary_only_findings,
        observations: &model_observations,
        proof_receipts: &proof_receipts,
        final_follow_up_tasks: 0,
        suggested_issues: &[],
    })?;
    let mut review = ReviewArtifacts {
        shared_context_id,
        review_profile: config.review_profile.clone(),
        mode: args.mode.key().to_owned(),
        posting: args.posting.key().to_owned(),
        runtime_profile: profile.name.clone(),
        run_pass: run_pass.key().to_owned(),
        model_mode: args.model_mode.key().to_owned(),
        depth: args.depth.key().to_owned(),
        provider_policy: args.provider_policy.key().to_owned(),
        model_provider_policy: args.provider_policy.key().to_owned(),
        lane_width: args.lane_width,
        model_concurrency: args.model_concurrency,
        max_model_calls: args.max_model_calls,
        max_inline_comments: args.max_inline_comments,
        model_timeout_sec: args.model_timeout_sec,
        ledger_path: effective_ledger_path(config, args),
        ledger_max_bytes: args.ledger_max_bytes,
        pr_thread_context,
        terminal_state: preliminary_surface.terminal_state,
        provider_preflights,
        model_lanes,
        missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence,
        inline_comments,
        summary_only_findings,
        observations: model_observations,
        proof_requests,
        proof_receipts,
        resource_leases,
        body: preliminary_surface.artifact_body,
    };
    let observations = combined_observations(&review);
    let observation_summary = observation_summary_artifacts(&observations);
    let orchestrator_plan = build_orchestrator_plan(
        &candidates,
        &observation_summary.unique,
        &review.proof_receipts,
        &review.resource_leases,
        &[],
    );
    write_observation_artifacts(out, &observations)?;
    write_orchestrator_artifacts(out, &orchestrator_plan, &review.proof_receipts)?;
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        compiler_loop,
        "completed",
    )?;
    let mut follow_up_results = Vec::new();
    let mut follow_up_outputs = Vec::new();
    let follow_up_model_loop = start_run_loop(
        event_log,
        run_started,
        "model",
        "investigation",
        "follow-up",
    )?;
    run_follow_up_model_pass(
        FollowUpRunContext {
            root,
            out,
            review_dir: &review_dir,
            provider_preflights: &review.provider_preflights,
            shared_context: &shared_context,
            args,
            model_calls_used,
            key_present: env_value_present,
            tasks: &orchestrator_plan.follow_up_tasks,
            line_map: &line_map,
        },
        &mut follow_up_results,
        &mut follow_up_outputs,
    )?;
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        follow_up_model_loop,
        "completed",
    )?;
    write_follow_up_result_artifacts(out, &follow_up_results)?;
    write_follow_up_output_artifacts(out, &follow_up_outputs)?;
    let resolved_candidates = resolved_candidate_records(
        &candidates,
        &follow_up_results,
        &follow_up_outputs,
        &prior_resolved_candidates,
    );
    write_resolved_candidate_artifacts(out, &resolved_candidates)?;
    // The late receipt turn exists to change candidate dispositions, so the
    // final compiler must honor those changes: candidates the follow-up pass
    // resolved to `refuted` or `dropped` lose their review surface here.
    // The full audit trail stays in candidates.json, resolved_candidates.json,
    // and the follow-up artifacts. Candidates resolved to `parked-follow-up`
    // keep their surface — parked items render in the dedicated parked
    // section instead of disappearing.
    let resolved_away_candidate_ids = follow_up_resolved_away_candidate_ids(&resolved_candidates);
    let resolved_away_candidates = candidates
        .iter()
        .filter(|candidate| resolved_away_candidate_ids.contains(&candidate.id))
        .collect::<Vec<_>>();
    write_model_stage_artifacts(out, &review.model_lanes, &follow_up_results, args)?;
    write_shared_context_cache_artifacts(
        out,
        &shared_context,
        &assignments,
        &review.provider_preflights,
        &review.model_lanes,
        &follow_up_results,
        args,
    )?;
    let follow_up_evidence = follow_up_evidence_from_outputs(&follow_up_outputs);
    write_follow_up_evidence_artifact(out, &follow_up_evidence)?;
    append_follow_up_proof_requests(&mut review.proof_requests, &follow_up_evidence);
    let follow_up_proof_loop =
        start_run_loop(event_log, run_started, "proof", "proof", "follow-up-broker")?;
    let follow_up_proof_result = run_follow_up_proof_broker_v0(
        root,
        out,
        diff,
        profile,
        &follow_up_evidence.proof_requests,
        &review.proof_receipts,
        &review.resource_leases,
        args,
    )?;
    review
        .proof_receipts
        .extend(follow_up_proof_result.proof_receipts);
    review
        .resource_leases
        .extend(follow_up_proof_result.resource_leases);
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        follow_up_proof_loop,
        "completed",
    )?;
    let receipt_routes = receipt_route_artifacts(&review.proof_receipts, &review.resource_leases);
    write_receipt_route_artifacts(out, &receipt_routes)?;
    // Release lane step 4: lane-emitted follow-up candidates become the
    // issue-capture artifacts. v0 is artifact-only - no PR-body rendering,
    // no GitHub side effects; classification is the whole pipeline.
    let (issue_capture_candidates, issue_capture_actions) =
        classify_issue_candidates(&config.issues, std::mem::take(&mut issue_candidates));
    write_issue_capture_artifacts(out, &issue_capture_candidates, &issue_capture_actions)?;
    // Step 6: under open-high-confidence, `run` decides what the post-time
    // broker may attempt and renders the full issue text into a pure plan
    // artifact. `run` itself never opens issues.
    if config.issues.enabled && config.issues.mode == "open-high-confidence" {
        let broker_plan = build_issue_broker_plan(
            &config.issues,
            &issue_capture_candidates,
            &issue_capture_actions,
            args.github_repo.as_deref(),
            args.github_pull_number
                .or_else(detect_pull_number_from_event),
        );
        write_issue_broker_plan(out, &broker_plan)?;
    }
    let suggested_issue_ids = issue_capture_actions
        .iter()
        .filter(|action| action.action == "suggested")
        .map(|action| action.candidate_id.as_str())
        .collect::<BTreeSet<_>>();
    let suggested_issues = issue_capture_candidates
        .iter()
        .filter(|candidate| suggested_issue_ids.contains(candidate.id.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    let final_orchestrator_plan = build_final_orchestrator_plan(
        &candidates,
        &observation_summary.unique,
        &review.proof_receipts,
        &review.resource_leases,
        tool_gate_outcomes,
    );
    write_final_orchestrator_artifact(out, &final_orchestrator_plan)?;
    let final_compiler_loop =
        start_run_loop(event_log, run_started, "compiler", "coordination", "final")?;
    let compiler_inline_comments = review
        .inline_comments
        .iter()
        .filter(|comment| {
            !resolved_away_candidates
                .iter()
                .any(|candidate| candidate_matches_inline_comment(candidate, comment))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut compiler_summary_only_findings = review.summary_only_findings.clone();
    compiler_summary_only_findings.retain(|finding| {
        !resolved_away_candidates
            .iter()
            .any(|candidate| candidate_matches_summary_finding(candidate, finding))
    });
    compiler_summary_only_findings.extend(follow_up_evidence.summary_only_findings.clone());
    let mut compiler_observations = review.observations.clone();
    compiler_observations.extend(follow_up_evidence.observations.clone());
    write_final_compiler_input_artifact(
        out,
        FinalCompilerInputArtifact {
            schema: FINAL_COMPILER_INPUT_V2_SCHEMA,
            phase: "final",
            source_artifacts: &[
                "review/review.json",
                "review/follow_up_evidence.json",
                "review/resolved_candidates.json",
                PRIOR_RESOLVED_CANDIDATES_ARTIFACT,
                "review/proof_receipts.json",
                "review/tool-gate-outcomes.json",
                "review/receipt_routes.json",
                "review/final_orchestrator_plan.json",
            ],
            model_lanes: &review.model_lanes,
            missing_or_failed_sensor_evidence: &review.missing_or_failed_sensor_evidence,
            missing_or_failed_model_evidence: &review.missing_or_failed_model_evidence,
            follow_up_resolved_candidate_ids: &resolved_away_candidate_ids,
            inline_comments: &compiler_inline_comments,
            summary_only_findings: &compiler_summary_only_findings,
            observations: &compiler_observations,
            proof_receipts: &review.proof_receipts,
        },
    )?;
    let final_surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: &review.shared_context_id,
        review_body_policy: &config.review_body,
        run_pass,
        post_review_on: &config.gate.post_review_on,
        args,
        plan,
        diff,
        model_lanes: &review.model_lanes,
        missing_or_failed_sensor_evidence: &review.missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence: &review.missing_or_failed_model_evidence,
        inline_comments: &compiler_inline_comments,
        summary_only_findings: &compiler_summary_only_findings,
        observations: &compiler_observations,
        proof_receipts: &review.proof_receipts,
        suggested_issues: &suggested_issues,
        final_follow_up_tasks: final_orchestrator_plan.follow_up_tasks.len(),
    })?;
    let mut review_payload_status = final_surface.review_payload_status;
    let should_prepare_github_review = final_surface.should_prepare_github_review;
    let summary_only_policy_posted = final_surface.summary_only_policy_posted;
    let github_review = final_surface.github_review;
    let artifact_body = final_surface.artifact_body;
    // unsafe-review comment-plan candidates entered the compiler intake before
    // candidate records were built. No post-compile comment injection happens
    // here; appending here would bypass the ledger, cap, dedupe, and refuter.
    let terminal_state = final_surface.terminal_state;
    review.terminal_state = terminal_state.clone();
    review.body = artifact_body.clone();
    let mut witnesses = build_witness_records(
        &review.inline_comments,
        &review.summary_only_findings,
        &observations,
        &review.proof_receipts,
    );
    append_follow_up_evidence_witnesses(
        &mut witnesses,
        &follow_up_evidence,
        &review.proof_receipts,
    );
    write_witness_artifacts(out, &witnesses)?;
    write_proof_receipt_artifacts(out, &review.proof_receipts)?;
    write_resource_lease_artifacts(out, &review.resource_leases)?;
    write_proof_request_artifacts(
        out,
        diff,
        profile,
        &review.proof_requests,
        &review.proof_receipts,
    )?;
    finish_run_loop(
        event_log,
        run_started,
        run_loop_tracker,
        final_compiler_loop,
        "completed",
    )?;
    let gate_outcome = build_gate_outcome(GateOutcomeInput {
        args,
        config,
        plan,
        terminal_state: &review.terminal_state,
        proof_requests: &review.proof_requests,
        proof_receipts: &review.proof_receipts,
        tool_gate_outcomes,
        missing_or_failed_sensor_evidence: &review.missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence: &review.missing_or_failed_model_evidence,
    });
    if gate_outcome.conclusion == "fail" && review_payload_status == "skipped_empty_smoke" {
        review_payload_status = "skipped_gate_failure_artifact_only";
        review.terminal_state.review_payload_status = review_payload_status.to_owned();
    }
    event_log.append(
        "terminal_state",
        serde_json::json!({
            "status": review.terminal_state.status,
            "review_payload_status": review.terminal_state.review_payload_status,
        }),
    )?;
    let run_loop_metrics = run_loop_tracker.metrics();
    let metrics = build_review_metrics(ReviewMetricsInput {
        out,
        diff,
        plan,
        review: &review,
        github_review: if should_prepare_github_review {
            Some(&github_review)
        } else {
            None
        },
        review_payload_status,
        observations_count: observations.len(),
        follow_up_results: &follow_up_results,
        final_follow_up_tasks: final_orchestrator_plan.follow_up_tasks.len(),
        run: run_loop_metrics,
        elapsed,
        args,
    });

    fs::write(
        review_dir.join("review.json"),
        serde_json::to_vec_pretty(&review)?,
    )?;
    fs::write(
        review_dir.join("metrics.json"),
        serde_json::to_vec_pretty(&metrics)?,
    )?;
    let cost_receipt =
        write_cost_receipt_artifact(root, out, config, &metrics, &review, &follow_up_results)?;
    write_floor_trend_artifact(out, &cost_receipt)?;
    let fill_ledger = write_fill_ledger_artifact(FillLedgerInput {
        out,
        diff,
        profile,
        plan,
        tool_gate_outcomes,
        gate_outcome: &gate_outcome,
        review: &review,
        metrics: &metrics,
    })?;
    let quality_receipt = write_quality_receipt_artifact(out, &metrics, &review, &fill_ledger)?;
    write_quality_trend_artifact(out, &quality_receipt)?;
    write_scheduler_artifact(&review_dir, &metrics.run)?;
    fs::write(
        review_dir.join("terminal_state.json"),
        serde_json::to_vec_pretty(&review.terminal_state)?,
    )?;
    fs::write(
        review_dir.join("gate_outcome.json"),
        serde_json::to_vec_pretty(&gate_outcome)?,
    )?;
    event_log.append(
        "gate_outcome",
        serde_json::json!({
            "conclusion": gate_outcome.conclusion,
            "terminal_status": gate_outcome.terminal_status,
            "reasons": gate_outcome.reasons.len(),
            "fail_on_gate": args.fail_on_gate.key(),
            "fail_on_gate_resolved": args.fail_on_gate.resolved(args.mode),
        }),
    )?;
    fs::write(
        review_dir.join("provider-preflight-status.json"),
        serde_json::to_vec_pretty(&review.provider_preflights)?,
    )?;
    fs::write(review_dir.join("review.md"), artifact_body)?;
    if should_prepare_github_review {
        write_github_review_payload(
            &review_dir,
            &github_review,
            &line_map,
            &config.review_body,
            summary_only_policy_posted,
        )?;
    } else {
        write_github_review_skip_receipt(
            &review_dir,
            build_github_review_skip_receipt(args, &review, config.review_body.summary_only_body),
        )?;
    }
    Ok(gate_outcome)
}

pub(crate) fn build_review_terminal_state(input: TerminalStateInput<'_>) -> ReviewTerminalState {
    let substantive_summary_only_findings =
        count_substantive_summary_only_findings(input.summary_only_findings);
    let usable_model_lanes = input
        .model_lanes
        .iter()
        .filter(|receipt| model_lane_is_usable_for_terminal_state(receipt))
        .count();
    let evidence_gaps = input.missing_or_failed_sensor_evidence.len()
        + input.missing_or_failed_model_evidence.len();
    let reviewer_value_present = input.should_prepare_github_review
        || has_reviewer_value(input.inline_comments, input.pr_body)
        || input
            .proof_receipts
            .iter()
            .any(proof_receipt_changes_review_value);

    let (status, reason) = if reviewer_value_present {
        let reason = if input.should_prepare_github_review {
            "Reviewer-value content survived compilation; a grouped PR review was prepared."
                .to_owned()
        } else if input.review_payload_status == "skipped_pass_policy" {
            format!(
                "Reviewer-value content survived compilation, but pass `{}` is not in [gate].post_review_on; diagnostics remain in artifacts.",
                input.run_pass.key()
            )
        } else {
            format!(
                "Reviewer-value content survived compilation, but summary_only_body = `{}` withheld the PR-facing payload as no-value boilerplate: {} summary-only findings, {} substantive; diagnostics remain in artifacts.",
                input.summary_only_body.key(),
                input.summary_only_findings.len(),
                substantive_summary_only_findings
            )
        };
        ("needs-reviewer-attention", reason)
    } else if input.args.dry_run {
        (
            "artifact-only",
            "Dry run requested; this run produced artifacts but no reviewer-facing review."
                .to_owned(),
        )
    } else if matches!(input.args.mode, RunMode::IntelligentCi)
        && has_required_sensor_evidence_gap(input.plan, input.missing_or_failed_sensor_evidence)
    {
        (
            "failed-to-review",
            "A required intelligent-ci sensor was missing, skipped, failed, or timed out, so the gate did not reach a sufficient review state.".to_owned(),
        )
    } else if matches!(input.args.model_mode, ModelMode::Off) {
        (
            "artifact-only",
            "Model mode was off; this run produced artifacts but no reviewer-facing review."
                .to_owned(),
        )
    } else if input.plan.diff_class == DiffClass::ArtifactOnlySmoke {
        (
            "artifact-only",
            "Artifact-only smoke diff; diagnostics remain in artifacts and no PR review was prepared.".to_owned(),
        )
    } else if usable_model_lanes == 0 && input.proof_receipts.is_empty() {
        (
            "failed-to-review",
            "No usable model lane or proof receipt was available, so the run did not reach a sufficient review state.".to_owned(),
        )
    } else {
        (
            "sufficient",
            "No reviewer-value content survived compilation; the run reached a sufficient terminal state and stayed artifact-only.".to_owned(),
        )
    };

    ReviewTerminalState {
        schema: TERMINAL_STATE_SCHEMA.to_owned(),
        status: status.to_owned(),
        reason,
        review_payload_status: input.review_payload_status.to_owned(),
        reviewer_value_present,
        diff_class: input.plan.diff_class.key().to_owned(),
        model_mode: input.args.model_mode.key().to_owned(),
        usable_model_lanes,
        model_lanes: input.model_lanes.len(),
        evidence_gaps,
        proof_receipts: input.proof_receipts.len(),
        final_follow_up_tasks: input.final_follow_up_tasks,
        inline_comments: input.inline_comments.len(),
        summary_only_findings: input.summary_only_findings.len(),
        substantive_summary_only_findings,
    }
}

/// Shared predicate for gate blocking and terminal-state routing: a sensor
/// evidence issue is blocking material only when the plan marks that sensor as
/// required. Keeping one helper prevents the two call sites from drifting.
pub(crate) fn sensor_issue_is_required(plan: &Plan, issue: &SensorEvidenceIssue) -> bool {
    plan.sensors
        .iter()
        .any(|sensor| sensor.id == issue.sensor && sensor.required)
}

pub(crate) fn has_required_sensor_evidence_gap(
    plan: &Plan,
    issues: &[SensorEvidenceIssue],
) -> bool {
    issues
        .iter()
        .any(|issue| sensor_issue_is_required(plan, issue))
}

pub(crate) fn model_lane_is_usable_for_terminal_state(receipt: &ModelLaneReceipt) -> bool {
    matches!(receipt.status.as_str(), "ok" | "degraded")
}

pub(crate) fn write_github_review_payload(
    review_dir: &Path,
    github_review: &GitHubReview,
    right_lines: &BTreeSet<(String, u32)>,
    review_body_policy: &ReviewBodyPolicy,
    waive_suppressible_body_policy: bool,
) -> Result<()> {
    validate_github_review_payload_for_right_lines(
        github_review,
        right_lines,
        "generated diff context",
        review_body_policy,
        waive_suppressible_body_policy,
    )?;
    fs::write(
        review_dir.join("github-review.json"),
        serde_json::to_vec_pretty(github_review)?,
    )?;
    Ok(())
}

pub(crate) struct ReviewMetricsInput<'a> {
    pub(crate) out: &'a Path,
    pub(crate) diff: &'a DiffContext,
    pub(crate) plan: &'a Plan,
    pub(crate) review: &'a ReviewArtifacts,
    pub(crate) github_review: Option<&'a GitHubReview>,
    pub(crate) review_payload_status: &'a str,
    pub(crate) observations_count: usize,
    pub(crate) follow_up_results: &'a [FollowUpResult],
    pub(crate) final_follow_up_tasks: usize,
    pub(crate) run: RunLoopMetrics,
    pub(crate) elapsed: Duration,
    pub(crate) args: &'a RunArgs,
}

pub(crate) fn build_review_metrics(input: ReviewMetricsInput<'_>) -> ReviewMetrics {
    let ReviewMetricsInput {
        out,
        diff,
        plan,
        review,
        github_review,
        review_payload_status,
        observations_count,
        follow_up_results,
        final_follow_up_tasks,
        mut run,
        elapsed,
        args,
    } = input;
    let sensor_statuses = plan
        .sensors
        .iter()
        .map(|sensor| sensor_status_for_metrics(out, sensor))
        .collect::<Vec<_>>();
    let preflight_statuses = review
        .provider_preflights
        .iter()
        .map(|receipt| receipt.status.as_str())
        .collect::<Vec<_>>();
    let model_lane_statuses = review
        .model_lanes
        .iter()
        .map(|receipt| receipt.status.as_str())
        .collect::<Vec<_>>();
    let follow_up_result_statuses = follow_up_results
        .iter()
        .map(|result| result.status.as_str())
        .collect::<Vec<_>>();
    run.model_call_duration_ms_sum = model_call_duration_ms_sum(review, follow_up_results);
    run.proof_command_duration_ms_sum = proof_command_duration_ms_sum(&review.proof_receipts);
    let prompt_cache = model_prompt_cache_metrics(review, follow_up_results, args);

    ReviewMetrics {
        schema_version: 1,
        wall_clock_ms: elapsed.as_millis(),
        wall_clock_seconds: elapsed.as_secs(),
        run,
        shared_context_id: review.shared_context_id.clone(),
        base: diff.base.clone(),
        head: diff.head.clone(),
        review_profile: review.review_profile.clone(),
        profile_name: plan.profile_name.clone(),
        runtime_profile: review.runtime_profile.clone(),
        mode: review.mode.clone(),
        posting: review.posting.clone(),
        run_pass: review.run_pass.clone(),
        model_mode: review.model_mode.clone(),
        depth: review.depth.clone(),
        provider_policy: review.provider_policy.clone(),
        lane_width: review.lane_width,
        model_concurrency: review.model_concurrency,
        max_model_calls: review.max_model_calls,
        max_inline_comments: review.max_inline_comments,
        changed_files: diff.changed_files.len(),
        diff_flags: diff.flags.clone(),
        lane_packets: lane_packet_count(out),
        sensors: SensorMetrics {
            total: plan.sensors.len(),
            planned: plan.sensors.iter().filter(|sensor| sensor.run).count(),
            skipped_by_plan: plan.sensors.iter().filter(|sensor| !sensor.run).count(),
            status_counts: status_counts(sensor_statuses.iter().map(String::as_str)),
        },
        models: ModelMetrics {
            provider_preflights: review.provider_preflights.len(),
            provider_preflight_status_counts: status_counts(preflight_statuses.iter().copied()),
            provider_preflight_calls_attempted: review
                .provider_preflights
                .iter()
                .filter(|receipt| model_call_attempted_status(&receipt.status))
                .count(),
            model_lanes: review.model_lanes.len(),
            model_lane_status_counts: status_counts(model_lane_statuses.iter().copied()),
            model_lane_calls_attempted: review
                .model_lanes
                .iter()
                .filter(|receipt| model_call_attempted_status(&receipt.status))
                .count(),
            model_fallbacks_used: review
                .model_lanes
                .iter()
                .filter(|receipt| receipt.fallback_from.is_some())
                .count(),
            prompt_cache_creation_input_tokens: prompt_cache.creation_input_tokens,
            prompt_cache_read_input_tokens: prompt_cache.read_input_tokens,
            prompt_cache_lane_hits: prompt_cache.lane_hits,
            prompt_cache_lane_misses: prompt_cache.lane_misses,
            prompt_cache_lane_unknown: prompt_cache.lane_unknown,
        },
        inline_comments: review.inline_comments.len(),
        github_review_comments: github_review.map_or(0, |review| review.comments.len()),
        summary_only_findings: review.summary_only_findings.len(),
        observations: observations_count,
        follow_up_results: FollowUpResultMetrics {
            total: follow_up_results.len(),
            status_counts: status_counts(follow_up_result_statuses.iter().copied()),
            calls_attempted: follow_up_results
                .iter()
                .filter(|result| model_call_attempted_status(&result.status))
                .count(),
        },
        final_follow_up_tasks,
        proof_requests: review.proof_requests.len(),
        proof_receipts: review.proof_receipts.len(),
        resource_leases: review.resource_leases.len(),
        off_diff_candidates_rejected: review
            .summary_only_findings
            .iter()
            .filter(|finding| finding.reason.contains("line_valid=false"))
            .count(),
        missing_or_failed_sensor_evidence: review.missing_or_failed_sensor_evidence.len(),
        missing_or_failed_model_evidence: review.missing_or_failed_model_evidence.len(),
        provider_evidence_failures: review
            .provider_preflights
            .iter()
            .filter(|receipt| is_model_evidence_issue(&receipt.status))
            .count(),
        terminal_state: review.terminal_state.status.clone(),
        review_payload_status: review_payload_status.to_owned(),
        post_status: "not_attempted_by_run".to_owned(),
        review_body_bytes: review.body.len(),
        artifact_review_body_bytes: review.body.len(),
        github_review_body_bytes: github_review.map_or(0, |review| review.body.len()),
        review_body_truncated: review.body.contains(REVIEW_BODY_TRUNCATED_SUFFIX.trim()),
        github_review_body_truncated: github_review
            .is_some_and(|review| review.body.contains(REVIEW_BODY_TRUNCATED_SUFFIX.trim())),
    }
}
