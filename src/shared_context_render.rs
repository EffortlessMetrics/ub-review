//! Shared context rendering, GitHub review skip receipts, and shared
//! context cache artifacts (cleanup train step 46, pure code motion).

use crate::*;

pub(crate) fn write_github_review_skip_receipt(
    review_dir: &Path,
    receipt: GitHubReviewSkipReceipt,
) -> Result<()> {
    let review_json = review_dir.join("github-review.json");
    if review_json.exists() {
        fs::remove_file(&review_json)?;
    }
    fs::write(
        github_review_skip_path(&review_json),
        serde_json::to_vec_pretty(&receipt)?,
    )?;
    Ok(())
}

pub(crate) fn build_github_review_skip_receipt(
    args: &RunArgs,
    review: &ReviewArtifacts,
    summary_only_body: SummaryOnlyBodyPolicy,
) -> GitHubReviewSkipReceipt {
    // The receipt reason must name the skip cause, not restate the terminal
    // state: a pass excluded by the profile's posting policy says so directly
    // instead of borrowing a sentence that can read like a contradiction, and
    // a body withheld by the boilerplate suppressor names the configured
    // [review_body].summary_only_body value and the finding counts it ruled
    // on.
    let reason = if review.terminal_state.review_payload_status == "skipped_pass_policy" {
        format!(
            "pass `{}` is not in [gate].post_review_on; the profile keeps this pass artifact-only.",
            review.run_pass
        )
    } else if review.terminal_state.review_payload_status == "skipped_artifact_only_body" {
        format!(
            "summary_only_body = `{}` withheld the PR-facing body as no-value boilerplate: {} summary-only findings, {} substantive; diagnostics remain in artifacts.",
            summary_only_body.key(),
            review.terminal_state.summary_only_findings,
            review.terminal_state.substantive_summary_only_findings
        )
    } else if review.terminal_state.review_payload_status == "skipped_gate_failure_artifact_only" {
        "the gate concluded `fail` and no reviewer-postable content was prepared; blocking reasons are receipted in review/gate_outcome.json."
            .to_owned()
    } else {
        review.terminal_state.reason.clone()
    };
    GitHubReviewSkipReceipt {
        schema_version: 1,
        status: "skipped".to_owned(),
        reason,
        review_payload_status: review.terminal_state.review_payload_status.clone(),
        terminal_state: review.terminal_state.status.clone(),
        github_review_json: None,
        run_pass: review.run_pass.clone(),
        model_mode: args.model_mode.key().to_owned(),
        inline_comments: review.inline_comments.len(),
        summary_only_findings: review.summary_only_findings.len(),
        missing_or_failed_sensor_evidence: review.missing_or_failed_sensor_evidence.len(),
        missing_or_failed_model_evidence: review.missing_or_failed_model_evidence.len(),
    }
}

pub(crate) fn github_review_skip_path(review_json: &Path) -> PathBuf {
    review_json
        .parent()
        .map(|dir| dir.join("github-review-skip.json"))
        .unwrap_or_else(|| PathBuf::from("github-review-skip.json"))
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn render_shared_context(
    root: &Path,
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    plan: &Plan,
    running_summary: &str,
    args: &RunArgs,
    pr_thread_context: &PrThreadContext,
    profile: &Profile,
    proof_requests: &[ProofRequest],
) -> Result<String> {
    let mut text = String::new();
    text.push_str("# Shared UB Review Context\n\n");
    text.push_str("This stable prefix is intended for lane model calls and future provider-side context caching.\n\n");
    text.push_str("## PR Summary\n\n");
    text.push_str(running_summary);
    text.push_str("\n\n## Diff Summary\n\n");
    text.push_str(&format!("- Base: `{}`\n", diff.base));
    text.push_str(&format!("- Head: `{}`\n", diff.head));
    text.push_str(&format!(
        "- Changed files: `{}`\n",
        diff.changed_files.len()
    ));
    text.push_str(&format!("- Diff class: `{}`\n", diff.diff_class.key()));
    let language_mix = classify_language_mix(&diff.changed_files);
    let languages = if language_mix.languages.is_empty() {
        "none".to_owned()
    } else {
        language_mix.languages.join(", ")
    };
    let surfaces = if language_mix.surfaces.is_empty() {
        "none".to_owned()
    } else {
        language_mix.surfaces.join(", ")
    };
    text.push_str(&format!("- Changed languages: `{languages}`\n"));
    text.push_str(&format!("- Changed surfaces: `{surfaces}`\n"));
    if let Some(primary_language) = &language_mix.primary_language {
        text.push_str(&format!("- Primary language: `{primary_language}`\n"));
    }
    text.push_str(&format!(
        "- Mixed-language diff: `{}`\n",
        language_mix.mixed_language
    ));
    text.push_str(&format!(
        "- Unsafe/native risk touched: `{}`\n",
        diff.flags.unsafe_or_native_risk
    ));
    text.push_str("\n## Changed Files\n\n");
    for file in &diff.changed_files {
        text.push_str(&format!("- `{file}`\n"));
    }
    text.push_str("\n## Sensor Statuses\n\n");
    for sensor in &plan.sensors {
        let status_path = out
            .join("sensors")
            .join(&sensor.id)
            .join("ub-review-sensor-status.json");
        let receipt = read_sensor_receipt(&status_path);
        let status = receipt
            .as_ref()
            .map(|receipt| receipt.status.as_str())
            .unwrap_or("receipt-absent");
        let reason = receipt
            .as_ref()
            .map(|receipt| receipt.reason.as_str())
            .unwrap_or(&sensor.reason);
        text.push_str(&format!(
            "- `{}`: `{}` - {}\n",
            sensor.id,
            status,
            escape_md(reason)
        ));
    }
    // unsafe-review structured evidence block (#359). Included when the sensor
    // was planned and its `unsafe-review-gate.json` is present with the
    // recognised schema. Falls back to a note when absent or schema is unknown.
    // Trust boundary is always advisory; this section supplements the
    // deterministic floor, never overrides it.
    if plan.sensors.iter().any(|s| s.id == "unsafe-review") {
        text.push_str("\n## unsafe-review Coverage Evidence\n\n");
        let sensor_dir = out.join("sensors").join("unsafe-review");
        let status_path = sensor_dir.join("ub-review-sensor-status.json");
        let sensor_status = read_sensor_receipt(&status_path)
            .map(|r| r.status)
            .unwrap_or_else(|| "receipt-absent".to_owned());
        text.push_str(&format!("- Sensor status: `{sensor_status}`\n"));
        if sensor_status == "ok" {
            match read_unsafe_review_artifacts(&sensor_dir) {
                Err(gap) => {
                    text.push_str(&format!(
                        "- Structured evidence: {} (falling back to status-only)\n",
                        gap.reason()
                    ));
                }
                Ok(artifacts) => {
                    let gate = &artifacts.gate;
                    let trust = gate.trust_boundary.as_deref().unwrap_or("advisory");
                    let summary = &gate.summary;
                    // Provenance from the real manifest: which tool/version/dialect
                    // produced this evidence. Context only, never a gate input.
                    let tool = gate.tool.as_deref().unwrap_or("unsafe-review");
                    let tool_version = gate.tool_version.as_deref().unwrap_or("unknown");
                    let dialect = gate.dialect.as_deref().unwrap_or("unsafe-review");
                    text.push_str(&format!(
                        "- Source: `{tool}` `{tool_version}` (dialect: `{dialect}`)\n"
                    ));
                    text.push_str(&format!(
                        "- Advisory status (trust_boundary: `{trust}`): `{}`\n",
                        gate.status
                    ));
                    text.push_str(&format!(
                        "- Movement: new_gaps={}, worsened={}, resolved={}, inherited={}\n",
                        summary.new_gaps,
                        summary.worsened_gaps,
                        summary.resolved_gaps,
                        summary.inherited_gaps
                    ));
                    text.push_str(&format!(
                        "- Comment-plan candidates: {}\n",
                        artifacts.comment_plan.len()
                    ));
                    if !artifacts.comment_plan.is_empty() {
                        text.push_str(
                            "\n### Comment-plan entries (advisory, for #360 inline posting)\n\n",
                        );
                        text.push_str("```json\n");
                        let cp_json = serde_json::to_string_pretty(&artifacts.comment_plan)
                            .unwrap_or_else(|_| "[]".to_owned());
                        text.push_str(&cp_json);
                        text.push_str("\n```\n");
                    }
                }
            }
        }
    }
    text.push_str("\n## Initial Work Queue\n\n");
    text.push_str(&render_initial_work_queue_context(
        out,
        plan,
        diff,
        profile,
        proof_requests,
    )?);
    text.push_str(&format!(
        "\n## {} Review Posture\n\n",
        diff_class_posture_heading(diff.diff_class)
    ));
    text.push_str(review_posture_for_diff_class(diff.diff_class));
    text.push_str("\n\n## PR Thread Context\n\n");
    text.push_str(&render_pr_thread_context(pr_thread_context));
    text.push_str("\n\n## UB Ledger Context\n\n");
    text.push_str(&render_ledger_context(root, config, args)?);
    text.push_str("\n\n## Diff Patch\n\n```diff\n");
    text.push_str(&diff.patch);
    if !diff.patch.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("```\n");
    Ok(text)
}

pub(crate) fn render_initial_work_queue_context(
    out: &Path,
    plan: &Plan,
    diff: &DiffContext,
    profile: &Profile,
    proof_requests: &[ProofRequest],
) -> Result<String> {
    let sensor_tasks = plan
        .sensors
        .iter()
        .map(|sensor| {
            (
                work_queue_task_from_sensor(out, sensor),
                sensor.reason.clone(),
            )
        })
        .collect::<Vec<_>>();
    let proof_output = build_proof_planner_output(diff, profile, proof_requests)?;
    let proof_tasks = proof_output
        .proof_tasks
        .iter()
        .map(|task| {
            (
                work_queue_task_from_proof_task(task),
                format!("{} {}", task.kind, task.purpose),
            )
        })
        .collect::<Vec<_>>();
    let mut tasks = Vec::with_capacity(sensor_tasks.len() + proof_tasks.len());
    tasks.extend(sensor_tasks);
    tasks.extend(proof_tasks);

    let mut counts = BTreeMap::new();
    for (task, _) in &tasks {
        *counts
            .entry(task.initial_packet_status.as_str())
            .or_insert(0usize) += 1;
    }

    let mut text = String::new();
    text.push_str(&format!(
        "- Ready for initial packet: `{}`\n",
        counts
            .get("ready_for_initial_packet")
            .copied()
            .unwrap_or_default()
    ));
    text.push_str(&format!(
        "- Pending initial packet: `{}`\n",
        counts
            .get("pending_initial_packet")
            .copied()
            .unwrap_or_default()
    ));
    text.push_str(&format!(
        "- Not initial packet: `{}`\n",
        counts
            .get("not_initial_packet")
            .copied()
            .unwrap_or_default()
    ));
    text.push_str("- Rule: pending work is unfinished, not missing evidence.\n");
    render_initial_work_queue_task_group(
        &mut text,
        "Ready Initial Packet Receipts",
        &tasks,
        "ready_for_initial_packet",
    );
    render_initial_work_queue_task_group(
        &mut text,
        "Pending Initial Packet Tasks",
        &tasks,
        "pending_initial_packet",
    );
    Ok(text)
}

pub(crate) fn render_initial_work_queue_task_group(
    text: &mut String,
    heading: &str,
    tasks: &[(WorkQueueTaskArtifact, String)],
    status: &str,
) {
    const MAX_QUEUE_ITEMS: usize = 8;
    text.push_str(&format!("\n### {heading}\n\n"));
    let matching = tasks
        .iter()
        .filter(|(task, _)| task.initial_packet_status == status)
        .collect::<Vec<_>>();
    if matching.is_empty() {
        text.push_str("- None.\n");
        return;
    }
    for (task, detail) in matching.iter().take(MAX_QUEUE_ITEMS) {
        text.push_str(&format!(
            "- `{}` (`{}`, `{}`) -> `{}`; consumers: `{}`; {}\n",
            task.id,
            task.packet_policy,
            task.gate_policy,
            task.receipt_path,
            task.consumers.join(", "),
            escape_md(detail)
        ));
    }
    let remaining = matching.len().saturating_sub(MAX_QUEUE_ITEMS);
    if remaining > 0 {
        text.push_str(&format!(
            "- `{remaining}` more task(s) are listed in `work_queue.json`.\n"
        ));
    }
}

pub(crate) fn write_shared_context_cache_artifacts(
    out: &Path,
    shared_context: &str,
    assignments: &[ModelAssignment],
    provider_preflights: &[ProviderPreflightReceipt],
    model_lanes: &[ModelLaneReceipt],
    follow_up_results: &[FollowUpResult],
    args: &RunArgs,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let shared_context_hash = sha256_hex(shared_context.as_bytes());
    fs::write(
        review_dir.join("shared_context_cache_block.md"),
        shared_context,
    )?;
    fs::write(
        review_dir.join("shared_context_hash.txt"),
        format!("{shared_context_hash}\n"),
    )?;

    let lanes = shared_context_cache_lanes(
        &shared_context_hash,
        assignments,
        model_lanes,
        follow_up_results,
        args,
    );
    let manifest = SharedContextCacheManifest {
        schema: CACHE_MANIFEST_SCHEMA,
        shared_context_hash: shared_context_hash.clone(),
        shared_context_bytes: shared_context.len(),
        cache_block_path: "review/shared_context_cache_block.md",
        hash_path: "review/shared_context_hash.txt",
        events_path: "review/cache_events.ndjson",
        explicit_cache_provider: "minimax",
        explicit_cache_endpoint: "anthropic-messages",
        cache_lifetime: "provider-ephemeral",
        lanes,
    };
    fs::write(
        review_dir.join("cache_manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;

    let mut events = Vec::new();
    events.push(SharedContextCacheEvent {
        schema: CACHE_EVENT_SCHEMA,
        kind: "shared_context_prepared".to_owned(),
        shared_context_hash: shared_context_hash.clone(),
        lane: None,
        provider: None,
        endpoint_kind: None,
        cache_mode: "artifact-prepared".to_owned(),
        cache_creation_input_tokens: None,
        cache_read_input_tokens: None,
    });
    for receipt in provider_preflights
        .iter()
        .filter(|receipt| model_call_attempted_status(&receipt.status))
    {
        events.push(SharedContextCacheEvent {
            schema: CACHE_EVENT_SCHEMA,
            kind: "provider_preflight_cache_usage".to_owned(),
            shared_context_hash: shared_context_hash.clone(),
            lane: Some("provider-preflight".to_owned()),
            provider: Some(receipt.provider.clone()),
            endpoint_kind: Some(receipt.endpoint_kind.clone()),
            cache_mode: model_cache_mode_for_args(args, &receipt.provider, &receipt.endpoint_kind)
                .to_owned(),
            cache_creation_input_tokens: receipt.cache_usage.cache_creation_input_tokens,
            cache_read_input_tokens: receipt.cache_usage.cache_read_input_tokens,
        });
    }
    for receipt in model_lanes
        .iter()
        .filter(|receipt| model_call_attempted_status(&receipt.status))
    {
        events.push(SharedContextCacheEvent {
            schema: CACHE_EVENT_SCHEMA,
            kind: "model_lane_cache_usage".to_owned(),
            shared_context_hash: shared_context_hash.clone(),
            lane: Some(receipt.lane.clone()),
            provider: Some(receipt.provider.clone()),
            endpoint_kind: Some(receipt.endpoint_kind.clone()),
            cache_mode: model_cache_mode_for_args(args, &receipt.provider, &receipt.endpoint_kind)
                .to_owned(),
            cache_creation_input_tokens: receipt.cache_usage.cache_creation_input_tokens,
            cache_read_input_tokens: receipt.cache_usage.cache_read_input_tokens,
        });
    }
    for result in follow_up_results
        .iter()
        .filter(|result| model_call_attempted_status(&result.status))
    {
        events.push(SharedContextCacheEvent {
            schema: CACHE_EVENT_SCHEMA,
            kind: "follow_up_cache_usage".to_owned(),
            shared_context_hash: shared_context_hash.clone(),
            lane: Some(result.model_lane.clone()),
            provider: Some(result.provider.clone()),
            endpoint_kind: Some(result.endpoint_kind.clone()),
            cache_mode: model_cache_mode_for_args(args, &result.provider, &result.endpoint_kind)
                .to_owned(),
            cache_creation_input_tokens: result.cache_usage.cache_creation_input_tokens,
            cache_read_input_tokens: result.cache_usage.cache_read_input_tokens,
        });
    }
    let mut ndjson = String::new();
    for event in events {
        ndjson.push_str(&serde_json::to_string(&event)?);
        ndjson.push('\n');
    }
    fs::write(review_dir.join("cache_events.ndjson"), ndjson)?;
    Ok(())
}

pub(crate) fn shared_context_cache_lanes(
    shared_context_hash: &str,
    assignments: &[ModelAssignment],
    model_lanes: &[ModelLaneReceipt],
    follow_up_results: &[FollowUpResult],
    args: &RunArgs,
) -> Vec<SharedContextCacheLane> {
    if model_lanes.is_empty() {
        return assignments
            .iter()
            .map(|assignment| {
                shared_context_cache_lane(
                    shared_context_hash,
                    &assignment.lane.id,
                    assignment.spec.provider.key(),
                    &assignment.spec.model,
                    assignment.spec.endpoint_kind.key(),
                    args,
                )
            })
            .collect();
    }
    let mut lanes = model_lanes
        .iter()
        .map(|receipt| {
            shared_context_cache_lane(
                shared_context_hash,
                &receipt.lane,
                &receipt.provider,
                &receipt.model,
                &receipt.endpoint_kind,
                args,
            )
        })
        .collect::<Vec<_>>();
    for result in follow_up_results
        .iter()
        .filter(|result| model_call_attempted_status(&result.status))
    {
        lanes.push(shared_context_cache_lane(
            shared_context_hash,
            &result.model_lane,
            &result.provider,
            &result.model,
            &result.endpoint_kind,
            args,
        ));
    }
    lanes
}

pub(crate) fn shared_context_cache_lane(
    shared_context_hash: &str,
    lane: &str,
    provider: &str,
    model: &str,
    endpoint_kind: &str,
    args: &RunArgs,
) -> SharedContextCacheLane {
    SharedContextCacheLane {
        lane: lane.to_owned(),
        provider: provider.to_owned(),
        model: model.to_owned(),
        endpoint_kind: endpoint_kind.to_owned(),
        cache_mode: model_cache_mode_for_args(args, provider, endpoint_kind).to_owned(),
        shared_context_hash: shared_context_hash.to_owned(),
    }
}
