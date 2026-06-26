//! Model execution core: run model lane tasks, call model endpoints
//! (refuter, proof planner, lane), apply model output to sinks (cleanup
//! train step 20, pure code motion).

use crate::*;

pub(crate) fn run_model_lane_tasks(
    context: &ModelRunContext<'_>,
    model_dir: &Path,
    tasks: Vec<ModelLaneTask>,
) -> Result<Vec<ModelLaneTaskResult>> {
    if tasks.is_empty() {
        return Ok(Vec::new());
    }
    let worker_count = context.args.model_concurrency.max(1).min(tasks.len());
    let queue = Arc::new(Mutex::new(VecDeque::from(tasks)));
    let (tx, rx) = mpsc::channel();
    let results = thread::scope(|scope| {
        for _ in 0..worker_count {
            let queue = Arc::clone(&queue);
            let tx = tx.clone();
            scope.spawn(move || {
                loop {
                    let task = match queue.lock() {
                        Ok(mut queue) => queue.pop_front(),
                        Err(_) => None,
                    };
                    let Some(task) = task else {
                        break;
                    };
                    let lane_dir = model_dir.join(&task.lane.id);
                    let result = fs::create_dir_all(&lane_dir)
                        .with_context(|| format!("create {}", lane_dir.display()))
                        .and_then(|()| {
                            call_model_lane(
                                context.root,
                                &lane_dir,
                                &task.lane,
                                &task.spec,
                                context.shared_context,
                                context.args,
                            )
                        });
                    let _ = tx.send(ModelLaneTaskResult {
                        index: task.index,
                        result,
                    });
                }
            });
        }
        drop(tx);
        rx.into_iter().collect::<Vec<_>>()
    });
    Ok(results)
}

pub(crate) fn run_refuter_pass(
    context: RefuterRunContext<'_>,
    model_lanes: &mut Vec<ModelLaneReceipt>,
    missing_or_failed_model_evidence: &mut Vec<ModelEvidenceIssue>,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) -> Result<usize> {
    let spec = direct_minimax_spec(context.args);
    let prefix_hash = sha256_hex(context.shared_context.as_bytes());
    let (cohort_id, shared_prefix_hash, thread_id, turn, cohort_broken) = cohort_stamp(
        spec.provider.key(),
        &spec.model,
        &prefix_hash,
        "refuter",
        0,
        None,
    );
    let mut receipt = ModelLaneReceipt {
        lane: "refuter".to_owned(),
        provider: spec.provider.key().to_owned(),
        model: spec.model.clone(),
        endpoint_kind: spec.endpoint_kind.key().to_owned(),
        status: "planned".to_owned(),
        reason: "planned M3 refuter pass for validated inline candidates".to_owned(),
        duration_ms: None,
        http_status: None,
        response_shape: None,
        fallback_from: None,
        cache_usage: ModelCacheUsage::default(),
        cohort_id,
        shared_prefix_hash,
        thread_id,
        turn,
        cohort_broken,
    };

    if inline_comments.is_empty() {
        receipt.status = "skipped".to_owned();
        receipt.reason = "no inline candidates passed guardrails before refuter".to_owned();
        model_lanes.push(receipt);
        return Ok(0);
    }
    if context.model_calls_used >= context.args.max_model_calls {
        receipt.status = "skipped".to_owned();
        receipt.reason = "model call budget exhausted before refuter pass".to_owned();
        if is_model_receipt_evidence_issue(&receipt) {
            missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        }
        demote_inline_candidates_for_refuter_unavailable(
            &receipt.reason,
            inline_comments,
            summary_only_findings,
        );
        model_lanes.push(receipt);
        return Ok(0);
    }
    if !provider_preflight_ok(&spec, context.provider_preflights) {
        receipt.status = "preflight_failed".to_owned();
        receipt.reason = provider_preflight_reason(&spec, context.provider_preflights)
            .unwrap_or_else(|| "MiniMax preflight did not succeed".to_owned());
        missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        demote_inline_candidates_for_refuter_unavailable(
            &receipt.reason,
            inline_comments,
            summary_only_findings,
        );
        model_lanes.push(receipt);
        return Ok(0);
    }
    let env_name = model_api_key_env(spec.provider);
    if !env_value_present(env_name) {
        let key_label = model_api_key_label(spec.provider);
        receipt.status = "missing_key".to_owned();
        receipt.reason = format!("{key_label} not provided; refuter output unavailable");
        missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
        demote_inline_candidates_for_refuter_unavailable(
            &receipt.reason,
            inline_comments,
            summary_only_findings,
        );
        model_lanes.push(receipt);
        return Ok(0);
    }

    let refuter_dir = context.review_dir.join("model").join("refuter");
    fs::create_dir_all(&refuter_dir)?;
    receipt.status = "running".to_owned();
    match call_model_refuter(
        context.root,
        &refuter_dir,
        &spec,
        context.shared_context,
        inline_comments,
        context.args,
    ) {
        Ok(outcome) => {
            receipt.status = "ok".to_owned();
            receipt.reason = "completed".to_owned();
            receipt.duration_ms = Some(outcome.duration_ms);
            receipt.http_status = outcome.http_status;
            receipt.response_shape = Some(outcome.response_shape);
            receipt.cache_usage = outcome.cache_usage;
            apply_refuter_output(outcome.output, inline_comments, summary_only_findings);
        }
        Err(err) => {
            receipt.status = classify_model_error(&err);
            receipt.reason = format!("{err:#}");
            receipt.http_status = http_status_from_error(&err);
            missing_or_failed_model_evidence.push(model_issue_from_receipt(&receipt));
            demote_inline_candidates_for_refuter_unavailable(
                &receipt.reason,
                inline_comments,
                summary_only_findings,
            );
        }
    }
    model_lanes.push(receipt);
    Ok(1)
}

pub(crate) fn demote_inline_candidates_for_refuter_unavailable(
    reason: &str,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) {
    for comment in std::mem::take(inline_comments) {
        summary_only_findings.push(summary_from_refuted_inline(
            comment,
            &format!("refuter unavailable; candidate kept summary-only: {reason}"),
        ));
    }
}

pub(crate) fn call_model_refuter(
    root: &Path,
    lane_dir: &Path,
    spec: &ProviderSpec,
    shared_context: &str,
    inline_comments: &[ReviewInlineComment],
    args: &RunArgs,
) -> Result<ModelCallOutcome<RefuterOutput>> {
    let prompt = render_refuter_prompt(inline_comments)?;
    call_model_prompt_typed_cached(root, lane_dir, spec, shared_context, &prompt, args)
}

pub(crate) fn render_refuter_prompt(inline_comments: &[ReviewInlineComment]) -> Result<String> {
    let candidates = serde_json::to_string_pretty(inline_comments)?;
    Ok(format!(
        r#"You are the final refuter for a Bun UB PR review.

Use only the cached shared context and candidate inline comments below.
Do not browse. Do not infer safety from missing evidence.
Do not post, mutate files, or run shell commands. The refuter only classifies candidates.
Return strict JSON only:
{{
  "decisions": [
    {{
      "path": "repo-relative/path.rs",
      "line": 123,
      "disposition": "inline|summary|drop",
      "confidence": "high|medium-high|medium|low",
      "reason": "why this candidate should remain inline, move to summary, or be dropped"
    }}
  ]
}}

Rules:
- `inline` only when the candidate is grounded, actionable, and not contradicted.
- `summary` for plausible but uncertain, broad, off-proof, or needs-human-verification concerns.
- `drop` only for high-confidence false positives or duplicates.
- If uncertain, use `summary`.
- Do not approve the PR and do not output LGTM language.

Candidate inline comments:
{candidates}"#
    ))
}

pub(crate) fn apply_refuter_output(
    output: RefuterOutput,
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) {
    let mut decisions = BTreeMap::new();
    for decision in output.decisions {
        decisions.insert(
            (normalize_repo_path(&decision.path), decision.line),
            decision,
        );
    }

    let mut kept = Vec::new();
    for comment in std::mem::take(inline_comments) {
        let key = (comment.path.clone(), comment.line);
        let Some(decision) = decisions.remove(&key) else {
            summary_only_findings.push(summary_from_refuted_inline(
                comment,
                "refuter returned no decision for this candidate; kept as summary-only",
            ));
            continue;
        };
        let confidence = decision
            .confidence
            .as_deref()
            .unwrap_or("medium")
            .trim()
            .to_ascii_lowercase();
        let confident = matches!(confidence.as_str(), "high" | "medium-high");
        let disposition = decision.disposition.trim().to_ascii_lowercase();
        match disposition.as_str() {
            "inline" if confident => kept.push(comment),
            "drop" if confident => {}
            "summary" | "summary-only" => {
                summary_only_findings.push(summary_from_refuted_inline(comment, &decision.reason));
            }
            "drop" | "inline" => {
                let reason = format!(
                    "refuter returned `{}` with `{}` confidence; kept as summary-only: {}",
                    disposition, confidence, decision.reason
                );
                summary_only_findings.push(summary_from_refuted_inline(comment, &reason));
            }
            _ => {
                let reason = format!(
                    "refuter returned unknown disposition `{}`; kept as summary-only: {}",
                    decision.disposition, decision.reason
                );
                summary_only_findings.push(summary_from_refuted_inline(comment, &reason));
            }
        }
    }
    inline_comments.extend(kept);
}

pub(crate) fn summary_from_refuted_inline(
    comment: ReviewInlineComment,
    reason: &str,
) -> SummaryOnlyFinding {
    SummaryOnlyFinding {
        lane: comment.lane,
        severity: comment.severity,
        confidence: comment.confidence,
        reason: format!(
            "refuter demoted inline candidate at {}:{}: {}",
            comment.path, comment.line, reason
        ),
        evidence: comment.evidence,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "proof-planner prompt mirrors deterministic planner inputs"
)]
pub(crate) fn call_model_proof_planner(
    root: &Path,
    lane_dir: &Path,
    lane: &LanePlan,
    spec: &ProviderSpec,
    shared_context: &str,
    diff: &DiffContext,
    profile: &Profile,
    box_state: &BoxState,
    pr_thread_context: &PrThreadContext,
    proof_requests: &[ProofRequest],
    args: &RunArgs,
) -> Result<ModelCallOutcome<LaneModelOutput>> {
    let input =
        build_proof_planner_input(diff, profile, box_state, pr_thread_context, proof_requests)?;
    let output = build_proof_planner_output(diff, profile, proof_requests)?;
    let prompt = render_proof_planner_model_task_prompt(lane, spec, &input, &output)?;
    call_model_prompt_cached(root, lane_dir, spec, shared_context, &prompt, args)
}

pub(crate) fn render_proof_planner_model_task_prompt(
    lane: &LanePlan,
    spec: &ProviderSpec,
    input: &ProofPlannerInput<'_>,
    output: &ProofPlannerOutput,
) -> Result<String> {
    let input_json = serde_json::to_string_pretty(input)?;
    let output_json = serde_json::to_string_pretty(output)?;
    Ok(format!(
        r#"Lane: {lane}
Provider: {provider}
Model: {model}
Endpoint kind: {endpoint_kind}
Role: {role}
Focus: {focus}

Use the cached shared context. You are an advisory proof-planner lane for the intelligent-ci gate.
The deterministic planner remains the source of proof_tasks.ndjson. Add only proof requests or observations that would improve the central proof broker's plan.

Planner input:
```json
{input_json}
```

Current deterministic planner output:
```json
{output_json}
```

Return only one strict JSON object:
{{
  "summary": null,
  "observations": [
    {{
      "claim": "terse proof-planning observation, 300 chars max",
      "question": "proof-planner",
      "kind": "verification-question|missing-evidence|test-gap|source-route-gap|resolved-check|parked-follow-up",
      "status": "open|covered|confirmed|refuted|parked",
      "severity": "high|medium|low",
      "confidence": "high|medium-high|medium|low",
      "path": "optional repo-relative/path.rs",
      "line": 123,
      "evidence": ["artifact, diff, receipt, or invariant"],
      "dedupe_key": "stable proof-planning key when known"
    }}
  ],
  "candidate_findings": [],
  "summary_only_findings": [],
  "failed_objections": [
    {{
      "claim": "proof idea considered",
      "reason": "why it is already covered, too costly, or not relevant",
      "confidence": "high|medium-high|medium|low",
      "kind": "resolved-check|false-premise",
      "evidence": ["artifact, diff, receipt, or invariant"]
    }}
  ],
  "proof_requests": [
    {{
      "command": "focused command requested from central proof broker",
      "reason": "why this proof would change the review decision",
      "cost": "focused-test|focused-build|manual",
      "timeout_sec": 300,
      "required": false
    }}
  ]
}}

Hard caps: at most 2 observations, 2 failed_objections, and 2 proof_requests.
Do not emit inline_comments, candidate_findings, summary_only_findings, or PR-facing findings.
Do not duplicate proof already represented in Current deterministic planner output.
Do not post, mutate files, or run shell commands. Lanes request proof; only the central broker executes.
"#,
        lane = lane.id,
        provider = spec.provider.key(),
        model = spec.model,
        endpoint_kind = spec.endpoint_kind.key(),
        role = lane.role,
        focus = lane.focus,
        input_json = input_json,
        output_json = output_json,
    ))
}

pub(crate) fn apply_proof_planner_model_output(
    lane: &LanePlan,
    output: LaneModelOutput,
    line_map: &BTreeSet<(String, u32)>,
    model_observations: &mut Vec<Observation>,
    proof_requests: &mut Vec<ProofRequest>,
) {
    let advisory_output = LaneModelOutput {
        summary: None,
        inline_comments: Vec::new(),
        candidate_findings: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: output.observations,
        failed_objections: output.failed_objections,
        proof_requests: output.proof_requests,
        issue_candidates: output.issue_candidates,
        degraded: output.degraded,
    };
    let mut ignored_inline_comments = Vec::new();
    let mut ignored_summary_only_findings = Vec::new();
    let mut ignored_issue_candidates = Vec::new();
    apply_model_output(
        lane,
        advisory_output,
        line_map,
        ModelOutputSinks {
            inline_comments: &mut ignored_inline_comments,
            summary_only_findings: &mut ignored_summary_only_findings,
            model_observations,
            proof_requests,
            issue_candidates: &mut ignored_issue_candidates,
        },
    );
}

pub(crate) fn call_model_lane(
    root: &Path,
    lane_dir: &Path,
    lane: &LanePlan,
    spec: &ProviderSpec,
    shared_context: &str,
    args: &RunArgs,
) -> Result<ModelCallOutcome<LaneModelOutput>> {
    let prompt = render_lane_model_task_prompt(lane, spec);
    call_model_prompt_cached(root, lane_dir, spec, shared_context, &prompt, args)
}

pub(crate) fn call_model_prompt(
    root: &Path,
    lane_dir: &Path,
    spec: &ProviderSpec,
    prompt: &str,
    args: &RunArgs,
) -> Result<ModelCallOutcome<LaneModelOutput>> {
    let content = call_model_prompt_content(root, lane_dir, spec, None, false, prompt, args)?;
    let (output, degraded) =
        parse_lane_model_output_or_degrade(&content.json_payload, &content.parse_path)?;
    Ok(ModelCallOutcome {
        output,
        duration_ms: content.duration_ms,
        http_status: content.http_status,
        response_shape: content.response_shape,
        cache_usage: content.cache_usage,
        degraded,
    })
}

pub(crate) fn call_model_prompt_cached(
    root: &Path,
    lane_dir: &Path,
    spec: &ProviderSpec,
    cacheable_prefix: &str,
    prompt: &str,
    args: &RunArgs,
) -> Result<ModelCallOutcome<LaneModelOutput>> {
    let use_cache_control = model_cacheable_prefix(spec, cacheable_prefix, args).is_some();
    let content = call_model_prompt_content(
        root,
        lane_dir,
        spec,
        Some(cacheable_prefix),
        use_cache_control,
        prompt,
        args,
    )?;
    let (output, degraded) =
        parse_lane_model_output_or_degrade(&content.json_payload, &content.parse_path)?;
    Ok(ModelCallOutcome {
        output,
        duration_ms: content.duration_ms,
        http_status: content.http_status,
        response_shape: content.response_shape,
        cache_usage: content.cache_usage,
        degraded,
    })
}

pub(crate) fn call_model_prompt_typed_cached<T>(
    root: &Path,
    lane_dir: &Path,
    spec: &ProviderSpec,
    cacheable_prefix: &str,
    prompt: &str,
    args: &RunArgs,
) -> Result<ModelCallOutcome<T>>
where
    T: DeserializeOwned,
{
    let use_cache_control = model_cacheable_prefix(spec, cacheable_prefix, args).is_some();
    let content = call_model_prompt_content(
        root,
        lane_dir,
        spec,
        Some(cacheable_prefix),
        use_cache_control,
        prompt,
        args,
    )?;
    let parsed_output = serde_json::from_str(&content.json_payload)
        .with_context(|| format!("parse {}", content.parse_path.display()))?;
    Ok(ModelCallOutcome {
        output: parsed_output,
        duration_ms: content.duration_ms,
        http_status: content.http_status,
        response_shape: content.response_shape,
        cache_usage: content.cache_usage,
        degraded: false,
    })
}

pub(crate) fn model_cacheable_prefix<'a>(
    spec: &ProviderSpec,
    cacheable_prefix: &'a str,
    args: &RunArgs,
) -> Option<&'a str> {
    (model_cache_mode_for_args(args, spec.provider.key(), spec.endpoint_kind.key())
        == "explicit-anthropic-cache-control")
        .then_some(cacheable_prefix)
}

pub(crate) struct ModelPromptContent {
    json_payload: String,
    parse_path: PathBuf,
    duration_ms: u128,
    http_status: Option<u16>,
    response_shape: String,
    cache_usage: ModelCacheUsage,
}

pub(crate) fn call_model_prompt_content(
    root: &Path,
    lane_dir: &Path,
    spec: &ProviderSpec,
    cacheable_prefix: Option<&str>,
    use_cache_control: bool,
    prompt: &str,
    args: &RunArgs,
) -> Result<ModelPromptContent> {
    let env_name = model_api_key_env(spec.provider);
    let token = env_value(env_name).with_context(|| format!("{env_name} missing"))?;
    let url = model_api_url(spec);
    let auth_header = model_auth_header(spec, &token);
    let payload = model_request_payload_parts_with_cache_control(
        spec,
        cacheable_prefix,
        prompt,
        use_cache_control,
    );
    let request_path = lane_dir.join("request.json");
    let response_path = lane_dir.join("response.json");
    let stderr_path = lane_dir.join("stderr.txt");
    fs::write(&request_path, serde_json::to_vec_pretty(&payload)?)?;
    let started = Instant::now();
    let process_output = run_curl_json_post(
        root,
        &url,
        &auth_header,
        &request_path,
        &["Accept: application/json", "Content-Type: application/json"],
        args.model_timeout_sec,
    )
    .with_context(|| "run model curl")?;
    let duration_ms = started.elapsed().as_millis();
    fs::write(&response_path, &process_output.stdout)?;
    fs::write(&stderr_path, &process_output.stderr)?;
    if !process_output.status.success() {
        let response_text = String::from_utf8_lossy(&process_output.stdout);
        bail!(
            "model curl exited {:?} with http status {:?}: stderr: {}; stdout: {}",
            process_output.status.code(),
            process_output.http_status,
            String::from_utf8_lossy(&process_output.stderr),
            response_text
        );
    }
    let response: serde_json::Value = serde_json::from_slice(&process_output.stdout)
        .with_context(|| format!("parse {}", response_path.display()))?;
    let response_shape = model_response_shape(&response).to_owned();
    let cache_usage = model_cache_usage(&response);
    let content = extract_model_content(&response)
        .ok_or_else(|| anyhow::anyhow!("model response did not contain assistant content"))?;
    let content_path = lane_dir.join("content.json");
    fs::write(&content_path, content.as_bytes())?;
    let json_payload = model_json_payload(content);
    let parse_path = if json_payload == content {
        content_path
    } else {
        let normalized_path = lane_dir.join("content-normalized.json");
        fs::write(&normalized_path, json_payload.as_bytes())?;
        normalized_path
    };
    Ok(ModelPromptContent {
        json_payload,
        parse_path,
        duration_ms,
        http_status: process_output.http_status,
        response_shape,
        cache_usage,
    })
}

#[cfg(test)]
pub(crate) fn render_lane_model_prompt(
    lane: &LanePlan,
    spec: &ProviderSpec,
    shared_context: &str,
) -> String {
    combined_model_prompt(
        Some(shared_context),
        &render_lane_model_task_prompt(lane, spec),
    )
}

pub(crate) fn render_lane_model_task_prompt(lane: &LanePlan, spec: &ProviderSpec) -> String {
    let lane_guidance = lane_specific_prompt_guidance(lane);
    format!(
        r#"Lane: {lane}
Provider: {provider}
Model: {model}
Endpoint kind: {endpoint_kind}
Role: {role}
Focus: {focus}
{lane_guidance}

Use the cached shared context. Return only one strict JSON object:
{{
  "summary": "short lane summary, 300 chars max",
  "observations": [
    {{
      "claim": "terse unique observation, 300 chars max",
      "question": "{lane}",
      "kind": "bug|verification-question|missing-evidence|test-gap|source-route-gap|security-risk|false-premise|parked-follow-up|residual-risk|resolved-check",
      "status": "open|covered|confirmed|refuted|demoted|parked|duplicate",
      "severity": "blocker|high|medium|low",
      "confidence": "high|medium-high|medium|low",
      "path": "optional repo-relative/path.rs",
      "line": 123,
      "evidence": ["artifact, diff, or invariant, 240 chars max"],
      "dedupe_key": "stable coordination key when known"
    }}
  ],
  "candidate_findings": [
    {{
      "severity": "blocker|high|medium",
      "confidence": "high|medium-high",
      "path": "repo-relative/path.rs",
      "line": 123,
      "body": "[{lane}] concise actionable finding, 400 chars max",
      "evidence": "artifact, diff, or invariant, 240 chars max"
    }}
  ],
  "summary_only_findings": [
    {{
      "severity": "blocker|high|medium|low",
      "confidence": "high|medium-high|medium",
      "reason": "summary-only issue, 400 chars max",
      "evidence": "artifact, diff, or invariant, 240 chars max"
    }}
  ],
  "failed_objections": [
    {{
      "claim": "objection tested by this lane",
      "reason": "why it did not hold",
      "confidence": "high|medium-high|medium|low",
      "kind": "resolved-check|false-premise",
      "evidence": ["artifact, diff, or invariant"]
    }}
  ],
  "proof_requests": [
    {{
      "command": "focused command requested from central proof broker",
      "reason": "why this proof would matter",
      "cost": "focused-test|focused-build|manual",
      "timeout_sec": 300,
      "required": false
    }}
  ]
}}

Hard caps: at most 3 observations, 2 candidate_findings, 1 summary_only_findings item, 2 failed_objections, and 1 proof_request.
If there is no blocker/high/medium actionable issue, use empty arrays and put the failed-objection audit in summary.
Only propose candidate_findings for valid RIGHT-side changed or context lines in the PR diff.
Legacy `inline_comments` is accepted as an alias for `candidate_findings`, but prefer `candidate_findings`.
Do not post, mutate files, or run shell commands. Request executable proof only through `proof_requests`.
Do not guess line numbers. Do not use deletion-side comments. Do not output a standalone approval.
Calibration: do not introduce `Box::from(slice)` / `Box::<[u8]>::from(slice)` allocation-failure analysis unless the current PR diff, seeded thread, or a candidate explicitly raises that objection. When raised, allocation failure does not return `None`, an empty box, or a recoverable fallback; return it as a refuted false-premise failed_objection, not as a candidate finding."#,
        lane = lane.id,
        provider = spec.provider.key(),
        model = spec.model,
        endpoint_kind = spec.endpoint_kind.key(),
        role = lane.role,
        focus = lane.focus,
        lane_guidance = lane_guidance,
    )
}

pub(crate) fn lane_specific_prompt_guidance(lane: &LanePlan) -> &'static str {
    if lane.id == "tests" || lane.id.starts_with("tests-") {
        "Convergence calibration: batch every material test-oracle weakness you can substantiate in this pass; classify correctness/oracle gaps as blocker/high/medium and submaterial polish as low advisory or parked-follow-up. If the test is red/green-correct or proof receipts answer the concern, emit a resolved-check or failed_objection instead of a fresh candidate finding. Do not drip-feed one nit per pass."
    } else if lane.id.contains("source-route") || lane.id.contains("sibling") {
        "Sibling-path calibration: a no-match scan for one pattern or helper group is not proof that no sibling paths exist or that a fix is complete. Only claim no relevant siblings when you can cite broad meta-class coverage across entry points. Otherwise report the checked pattern/scope and emit a source-route-gap or verification question for unscanned variants."
    } else {
        ""
    }
}

pub(crate) struct ModelOutputSinks<'a> {
    pub(crate) inline_comments: &'a mut Vec<ReviewInlineComment>,
    pub(crate) summary_only_findings: &'a mut Vec<SummaryOnlyFinding>,
    pub(crate) model_observations: &'a mut Vec<Observation>,
    pub(crate) proof_requests: &'a mut Vec<ProofRequest>,
    pub(crate) issue_candidates: &'a mut Vec<IssueCandidate>,
}

pub(crate) fn apply_model_output(
    lane: &LanePlan,
    output: LaneModelOutput,
    line_map: &BTreeSet<(String, u32)>,
    sinks: ModelOutputSinks<'_>,
) {
    let ModelOutputSinks {
        inline_comments,
        summary_only_findings,
        model_observations,
        proof_requests,
        issue_candidates,
    } = sinks;
    for mut candidate in output.issue_candidates {
        // Raw collection only: the lane is recorded as the source; ids,
        // validation, dedupe, and the action ledger happen centrally in
        // classify_issue_candidates. Lanes never open issues.
        candidate.source = lane.id.clone();
        issue_candidates.push(candidate);
    }
    if let Some(summary) = output.summary {
        if let Some(observation) = sibling_completeness_overclaim_observation_from_text(
            lane,
            &summary,
            vec!["lane model summary".to_owned()],
            None,
            None,
            model_observations.len(),
            "model-sibling-completeness-guard",
        ) {
            model_observations.push(observation);
        } else if let Some(observation) = box_from_allocation_false_premise_observation_from_text(
            lane,
            &summary,
            vec!["lane model summary".to_owned()],
            None,
            None,
            model_observations.len(),
            "model-false-premise-guard",
        ) {
            model_observations.push(observation);
        } else {
            summary_only_findings.push(validate_lane_model_summary(lane, &summary));
        }
    }
    for candidate in output.summary_only_findings {
        if let Some(observation) = sibling_completeness_overclaim_observation_from_text(
            lane,
            &format!("{}\n{}", candidate.reason, candidate.evidence),
            vec![candidate.evidence.clone()],
            None,
            None,
            model_observations.len(),
            "model-sibling-completeness-guard",
        ) {
            model_observations.push(observation);
        } else if let Some(observation) =
            box_from_allocation_false_premise_observation_from_summary_only(
                lane,
                &candidate,
                model_observations.len(),
            )
        {
            model_observations.push(observation);
        } else {
            summary_only_findings.push(validate_summary_only_candidate(lane, candidate));
        }
    }
    for observation in output.observations {
        model_observations.push(validate_model_observation(
            lane,
            observation,
            model_observations.len(),
        ));
    }
    for objection in output.failed_objections {
        model_observations.push(validate_failed_objection(
            lane,
            objection,
            model_observations.len(),
        ));
    }
    for request in output.proof_requests {
        proof_requests.push(validate_proof_request(lane, request, proof_requests.len()));
    }
    for candidate in output
        .candidate_findings
        .into_iter()
        .chain(output.inline_comments)
    {
        let path = normalize_repo_path(&candidate.path);
        let path = if path.is_empty() { None } else { Some(path) };
        if let Some(observation) = sibling_completeness_overclaim_observation_from_text(
            lane,
            &format!("{}\n{}", candidate.body, candidate.evidence),
            vec![candidate.evidence.clone()],
            path.as_ref(),
            Some(candidate.line),
            model_observations.len(),
            "model-sibling-completeness-guard",
        ) {
            model_observations.push(observation);
            continue;
        }
        if let Some(observation) = box_from_allocation_false_premise_observation_from_candidate(
            lane,
            &candidate,
            model_observations.len(),
        ) {
            model_observations.push(observation);
            continue;
        }
        if is_candidate_only_lane(&lane.id) {
            summary_only_findings.push(SummaryOnlyFinding {
                lane: lane.id.clone(),
                severity: candidate.severity,
                confidence: candidate.confidence,
                reason: format!(
                    "candidate-only lane emitted inline candidate for {}:{}; kept summary-only",
                    candidate.path, candidate.line
                ),
                evidence: candidate.evidence,
            });
            continue;
        }
        match validate_inline_candidate(lane, candidate, line_map) {
            Ok(comment) => inline_comments.push(comment),
            Err(finding) => summary_only_findings.push(finding),
        }
    }
}
