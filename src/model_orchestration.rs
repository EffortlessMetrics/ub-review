//! Model execution orchestration: preflight execution, available lane
//! scheduling, wave capacity, backpressure, and provider preflight
//! cacheable prefixes (cleanup train step 49, pure code motion).

use crate::*;

pub(crate) fn model_issue_from_receipt(receipt: &ModelLaneReceipt) -> ModelEvidenceIssue {
    ModelEvidenceIssue {
        lane: receipt.lane.clone(),
        provider: receipt.provider.clone(),
        model: receipt.model.clone(),
        endpoint_kind: receipt.endpoint_kind.clone(),
        status: receipt.status.clone(),
        reason: receipt.reason.clone(),
    }
}

pub(crate) fn append_preflight_evidence_issues(
    provider_preflights: &[ProviderPreflightReceipt],
    missing_or_failed_model_evidence: &mut Vec<ModelEvidenceIssue>,
) {
    for receipt in provider_preflights {
        if is_model_evidence_issue(&receipt.status) {
            missing_or_failed_model_evidence.push(ModelEvidenceIssue {
                lane: "provider-preflight".to_owned(),
                provider: receipt.provider.clone(),
                model: receipt.model.clone(),
                endpoint_kind: receipt.endpoint_kind.clone(),
                status: receipt.status.clone(),
                reason: receipt.reason.clone(),
            });
        }
    }
}

pub(crate) fn run_provider_preflights(
    root: &Path,
    review_dir: &Path,
    provider_preflights: &mut [ProviderPreflightReceipt],
    shared_context: &str,
    args: &RunArgs,
) -> Result<()> {
    let preflight_dir = review_dir.join("provider-preflight");
    fs::create_dir_all(&preflight_dir)?;
    for receipt in provider_preflights {
        if receipt.status != "planned" {
            continue;
        }
        let spec = provider_spec_from_preflight(receipt)?;
        let lane_dir = preflight_dir.join(sanitize_artifact_name(&spec.label()));
        fs::create_dir_all(&lane_dir)?;
        let prompt = "Return strict JSON only: {\"summary\":\"preflight ok\",\"inline_comments\":[],\"summary_only_findings\":[]}";
        let outcome = match provider_preflight_cacheable_prefix(&spec, shared_context, args) {
            Some(cacheable_prefix) => {
                call_model_prompt_cached(root, &lane_dir, &spec, cacheable_prefix, prompt, args)
            }
            None => call_model_prompt(root, &lane_dir, &spec, prompt, args),
        };
        match outcome {
            Ok(outcome) => {
                receipt.status = "ok".to_owned();
                receipt.reason = "completed".to_owned();
                receipt.duration_ms = Some(outcome.duration_ms);
                receipt.http_status = outcome.http_status;
                receipt.response_shape = Some(outcome.response_shape);
                receipt.cache_usage = outcome.cache_usage;
            }
            Err(err) => {
                receipt.status = classify_model_error(&err);
                receipt.reason = format!("{err:#}");
                receipt.http_status = http_status_from_error(&err);
            }
        }
    }
    Ok(())
}

pub(crate) fn provider_preflight_cacheable_prefix<'a>(
    spec: &ProviderSpec,
    shared_context: &'a str,
    args: &RunArgs,
) -> Option<&'a str> {
    model_cacheable_prefix(spec, shared_context, args)
}

pub(crate) const ARTIFACT_NAME_MAX_CHARS: usize = 96;
const ARTIFACT_NAME_HASH_CHARS: usize = 16;

pub(crate) fn sanitize_artifact_name(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.len() <= ARTIFACT_NAME_MAX_CHARS {
        return sanitized;
    }
    let digest = sha256_hex(value.as_bytes());
    let prefix_len = ARTIFACT_NAME_MAX_CHARS - ARTIFACT_NAME_HASH_CHARS - 1;
    format!(
        "{}-{}",
        sanitized.chars().take(prefix_len).collect::<String>(),
        &digest[..ARTIFACT_NAME_HASH_CHARS]
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn run_available_model_lanes(
    context: ModelRunContext<'_>,
    model_lanes: &mut [ModelLaneReceipt],
    missing_or_failed_model_evidence: &mut Vec<ModelEvidenceIssue>,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
    model_observations: &mut Vec<Observation>,
    proof_requests: &mut Vec<ProofRequest>,
    proof_intents: &mut Vec<ProofIntent>,
    issue_candidates: &mut Vec<IssueCandidate>,
) -> Result<usize> {
    run_available_model_lanes_with_runner(
        context,
        model_lanes,
        missing_or_failed_model_evidence,
        inline_comments,
        summary_only_findings,
        model_observations,
        proof_requests,
        proof_intents,
        issue_candidates,
        run_model_lane_tasks,
    )
}

/// Wave-loop core with an injectable task runner, mirroring the proof
/// broker's `_with_runner` seam: production passes `run_model_lane_tasks`;
/// tests inject deterministic results to exercise scheduling and the runtime
/// fallback retry path without network or env mutation.
pub(crate) fn model_wave_has_capacity(
    wave_len: usize,
    calls: usize,
    max_model_calls: usize,
    global_limit: usize,
) -> bool {
    wave_len < global_limit && calls + wave_len < max_model_calls
}

pub(crate) fn provider_wave_has_capacity(
    counts: &BTreeMap<ModelProvider, usize>,
    provider: ModelProvider,
    limits: ProviderConcurrencyLimits,
    global_limit: usize,
) -> bool {
    counts.get(&provider).copied().unwrap_or(0) < limits.limit_for(provider, global_limit)
}

pub(crate) fn record_provider_wave_slot(
    counts: &mut BTreeMap<ModelProvider, usize>,
    provider: ModelProvider,
) {
    *counts.entry(provider).or_insert(0) += 1;
}

#[derive(Clone, Debug)]
pub(crate) struct ProviderBackpressure {
    pub(crate) status: String,
    pub(crate) http_status: Option<u16>,
}

pub(crate) fn model_error_triggers_provider_backpressure(
    status: &str,
    http_status: Option<u16>,
) -> bool {
    matches!(status, "rate_limited" | "timed_out")
        || (status == "failed" && http_status.is_some_and(|code| code >= 500))
}

pub(crate) fn provider_backpressure_label(backpressure: &ProviderBackpressure) -> String {
    if let Some(http_status) = backpressure.http_status {
        format!("{} http {}", backpressure.status, http_status)
    } else {
        backpressure.status.clone()
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn run_available_model_lanes_with_runner(
    context: ModelRunContext<'_>,
    model_lanes: &mut [ModelLaneReceipt],
    missing_or_failed_model_evidence: &mut Vec<ModelEvidenceIssue>,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
    model_observations: &mut Vec<Observation>,
    proof_requests: &mut Vec<ProofRequest>,
    proof_intents: &mut Vec<ProofIntent>,
    issue_candidates: &mut Vec<IssueCandidate>,
    runner: impl Fn(&ModelRunContext<'_>, &Path, Vec<ModelLaneTask>) -> Result<Vec<ModelLaneTaskResult>>,
) -> Result<usize> {
    let model_dir = context.review_dir.join("model");
    fs::create_dir_all(&model_dir)?;
    let mut calls = 0usize;
    let mut pending_assignments = (0..context.assignments.len()).collect::<VecDeque<_>>();
    let global_limit = context.args.model_concurrency.max(1);
    // Runtime fallback retry state: a lane whose primary call failed with a
    // retryable class (rate limit, timeout, server error) is queued here with
    // its fallback spec forced, so the next wave re-runs it on the fallback
    // instead of letting `selected_provider_spec` re-pick the degraded
    // primary. One retry per lane; retries spend the same `max_model_calls`
    // budget as first attempts.
    let mut forced_specs: Vec<Option<ProviderSpec>> = vec![None; context.assignments.len()];
    let mut retry_queue: VecDeque<usize> = VecDeque::new();
    let mut provider_backpressure = BTreeMap::<ModelProvider, ProviderBackpressure>::new();
    loop {
        if calls >= context.args.max_model_calls
            || (pending_assignments.is_empty() && retry_queue.is_empty())
        {
            break;
        }

        let active_provider_backpressure = std::mem::take(&mut provider_backpressure);
        let mut wave = Vec::new();
        let mut provider_counts = BTreeMap::new();
        // Drain fallback retries first: their primary failure is already
        // known, so they are the wave's most valuable slots.
        let retry_scan_count = retry_queue.len();
        for _ in 0..retry_scan_count {
            if !model_wave_has_capacity(
                wave.len(),
                calls,
                context.args.max_model_calls,
                global_limit,
            ) {
                break;
            }
            let Some(index) = retry_queue.pop_front() else {
                break;
            };
            let assignment = &context.assignments[index];
            let receipt = &mut model_lanes[index];
            let Some(spec) = forced_specs[index].clone() else {
                continue;
            };
            if !provider_wave_has_capacity(
                &provider_counts,
                spec.provider,
                context.provider_concurrency,
                global_limit,
            ) {
                retry_queue.push_back(index);
                continue;
            }
            receipt.fallback_from = Some(assignment.spec.label());
            receipt.provider = spec.provider.key().to_owned();
            receipt.model = spec.model.clone();
            receipt.endpoint_kind = spec.endpoint_kind.key().to_owned();
            receipt.status = "running".to_owned();
            record_provider_wave_slot(&mut provider_counts, spec.provider);
            wave.push(ModelLaneTask {
                index,
                lane: assignment.lane.clone(),
                spec,
            });
        }
        let assignment_scan_count = pending_assignments.len();
        for _ in 0..assignment_scan_count {
            if !model_wave_has_capacity(
                wave.len(),
                calls,
                context.args.max_model_calls,
                global_limit,
            ) {
                break;
            }
            let Some(index) = pending_assignments.pop_front() else {
                break;
            };
            let assignment = &context.assignments[index];
            let receipt = &mut model_lanes[index];
            if receipt.status != "planned" {
                continue;
            }
            let lane = &assignment.lane;
            let Some((spec, fallback_from, preflight_reason)) =
                selected_provider_spec_with_backpressure(
                    assignment,
                    context.provider_preflights,
                    &active_provider_backpressure,
                )
            else {
                receipt.status = "preflight_failed".to_owned();
                receipt.reason =
                    "provider preflight did not succeed and no fallback was available".to_owned();
                missing_or_failed_model_evidence.push(model_issue_from_receipt(receipt));
                continue;
            };
            if let Some(reason) = preflight_reason {
                receipt.reason = reason;
            }
            if let Some(original) = fallback_from {
                receipt.fallback_from = Some(original);
            }
            let env_name = model_api_key_env(spec.provider);
            if !(context.key_present)(env_name) {
                let key_label = model_api_key_label(spec.provider);
                receipt.provider = spec.provider.key().to_owned();
                receipt.model = spec.model.clone();
                receipt.endpoint_kind = spec.endpoint_kind.key().to_owned();
                receipt.status = "missing_key".to_owned();
                receipt.reason = format!(
                    "{key_label} not provided; {} lane output unavailable",
                    spec.provider.key()
                );
                missing_or_failed_model_evidence.push(model_issue_from_receipt(receipt));
                continue;
            }
            if !provider_wave_has_capacity(
                &provider_counts,
                spec.provider,
                context.provider_concurrency,
                global_limit,
            ) {
                pending_assignments.push_back(index);
                continue;
            }
            receipt.provider = spec.provider.key().to_owned();
            receipt.model = spec.model.clone();
            receipt.endpoint_kind = spec.endpoint_kind.key().to_owned();
            receipt.status = "running".to_owned();
            record_provider_wave_slot(&mut provider_counts, spec.provider);
            wave.push(ModelLaneTask {
                index,
                lane: lane.clone(),
                spec,
            });
        }

        if wave.is_empty() {
            continue;
        }

        let attempted_specs = wave
            .iter()
            .map(|task| (task.index, task.spec.clone()))
            .collect::<BTreeMap<_, _>>();
        calls += wave.len();
        let mut results = runner(&context, &model_dir, wave)?;
        results.sort_by_key(|result| result.index);
        for task_result in results {
            let receipt = &mut model_lanes[task_result.index];
            let lane = &context.assignments[task_result.index].lane;
            match task_result.result {
                Ok(outcome) => {
                    if outcome.degraded {
                        receipt.status = "degraded".to_owned();
                        receipt.reason =
                            "contentful lane output was preserved as degraded evidence".to_owned();
                    } else {
                        receipt.status = "ok".to_owned();
                        receipt.reason = if forced_specs[task_result.index].is_some() {
                            "completed after runtime fallback retry".to_owned()
                        } else if receipt.fallback_from.is_some()
                            && receipt.reason.contains("provider backed off")
                        {
                            "completed after provider backpressure fallback".to_owned()
                        } else {
                            "completed".to_owned()
                        };
                    }
                    receipt.duration_ms = Some(outcome.duration_ms);
                    receipt.http_status = outcome.http_status;
                    receipt.response_shape = Some(outcome.response_shape.clone());
                    receipt.cache_usage = outcome.cache_usage;
                    apply_model_output(
                        lane,
                        outcome.output,
                        context.line_map,
                        ModelOutputSinks {
                            inline_comments,
                            summary_only_findings,
                            model_observations,
                            proof_requests,
                            proof_intents,
                            issue_candidates,
                        },
                    );
                }
                Err(err) => {
                    let status = classify_model_error(&err);
                    let http_status = http_status_from_error(&err);
                    if model_error_triggers_provider_backpressure(&status, http_status)
                        && let Some(spec) = attempted_specs.get(&task_result.index)
                    {
                        provider_backpressure.insert(
                            spec.provider,
                            ProviderBackpressure {
                                status: status.clone(),
                                http_status,
                            },
                        );
                    }
                    let assignment = &context.assignments[task_result.index];
                    if let Some(fallback) = runtime_fallback_retry_spec(
                        assignment,
                        receipt,
                        forced_specs[task_result.index].is_some(),
                        &status,
                        http_status,
                        context.key_present,
                    ) {
                        // Transient primary failure with an available
                        // fallback: queue one retry instead of failing the
                        // lane. The model-evidence issue is recorded only if
                        // the retry itself fails or never gets budget.
                        receipt.status = "planned".to_owned();
                        receipt.reason = format!(
                            "retrying on fallback {} after primary {status}: {err:#}",
                            fallback.label()
                        );
                        receipt.http_status = http_status;
                        forced_specs[task_result.index] = Some(fallback);
                        retry_queue.push_back(task_result.index);
                    } else {
                        receipt.status = status;
                        receipt.reason = format!("{err:#}");
                        receipt.http_status = http_status;
                        missing_or_failed_model_evidence.push(model_issue_from_receipt(receipt));
                    }
                }
            }
        }
    }
    for (index, receipt) in model_lanes.iter_mut().enumerate() {
        // Stamp cohort provenance on every lane that has a provider assigned
        // (Order 5 of #678). Lanes that never reached execution (still
        // "planned") get stamped too if they have a provider, so the receipt
        // always carries cohort identity for downstream consumers.
        if !receipt.provider.is_empty() {
            let assignment = &context.assignments[index];
            let prefix_hash = sha256_hex(context.shared_context.as_bytes());
            let primary = &assignment.spec;
            let fb_spec = if receipt.fallback_from.is_some() {
                Some((receipt.provider.as_str(), receipt.model.as_str()))
            } else {
                None
            };
            let (cohort_id, shared_prefix_hash, thread_id, turn, cohort_broken) = cohort_stamp(
                primary.provider.key(),
                &primary.model,
                &prefix_hash,
                &assignment.lane.id,
                0,
                fb_spec,
            );
            receipt.cohort_id = cohort_id;
            receipt.shared_prefix_hash = shared_prefix_hash;
            receipt.thread_id = thread_id;
            receipt.turn = turn;
            receipt.cohort_broken = cohort_broken;
        }
        if receipt.status == "planned" {
            receipt.status = "skipped".to_owned();
            receipt.reason = "model call budget reached before lane execution".to_owned();
            if is_model_receipt_evidence_issue(receipt) {
                missing_or_failed_model_evidence.push(model_issue_from_receipt(receipt));
            }
        }
    }
    Ok(calls)
}
