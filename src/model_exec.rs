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

/// Order 9 (#678): the live reporter — same-model coordinator. Runs after the
/// Resolve a reporter question target to an actual lane ID. The model often
/// writes "Question for workflow-proof: ..." or "workflow-proof lane: ..." —
/// strip common prefixes and match against known lane IDs. Also tries
/// substring matching: if any known lane ID appears within the target text,
/// use it. (Fixes the production issue where "Question for workflow-proof"
/// didn't match lane ID "workflow-proof".)
pub(crate) fn resolve_lane_target(target: &str, known_lane_ids: &[&str]) -> Option<String> {
    let cleaned = target
        .trim()
        .strip_prefix("Question for ")
        .or_else(|| target.trim().strip_prefix("question for "))
        .or_else(|| target.trim().strip_prefix("Question to "))
        .unwrap_or(target)
        .trim();
    // Exact match first.
    if known_lane_ids.contains(&cleaned) {
        return Some(cleaned.to_owned());
    }
    // Strip trailing " lane" suffix.
    let no_lane_suffix = cleaned
        .strip_suffix(" lane")
        .or_else(|| cleaned.strip_suffix(" Lane"))
        .unwrap_or(cleaned);
    if known_lane_ids.contains(&no_lane_suffix) {
        return Some(no_lane_suffix.to_owned());
    }
    // Fuzzy: if any known lane ID is a substring of the target, use it.
    // (e.g., "workflow-proof" matches inside "Question for workflow-proof")
    for id in known_lane_ids {
        if cleaned.contains(id) {
            return Some((*id).to_owned());
        }
    }
    None
}

/// Build the continuation prompt for a lane that the reporter asked a
/// follow-up question. The prompt is: the reporter's question + the lane's
/// prior conclusion, asking the lane to revise or confirm its finding.
/// (Order 9b of #678.)
pub(crate) fn lane_continuation_prompt(
    lane_id: &str,
    lane_role: &str,
    prior_conclusion: &str,
    reporter_question: &str,
    reporter_distillation: &str,
    proof_receipt_excerpts: &[String],
    late_sensor_excerpts: &[String],
) -> String {
    let mut prompt = format!(
        "# Lane continuation: `{lane_id}`\n\n\
         Your role: {lane_role}\n\n\
         ## Your prior conclusion\n\n{prior_conclusion}\n\n\
         ## The reporter's summary\n\n{reporter_distillation}\n\n\
         ## Reporter follow-up question\n\n{reporter_question}\n\n"
    );
    // Route proof receipts back to the lane (Order 9c of #678): when proof
    // evidence relevant to this lane's concern exists, include a bounded
    // excerpt so the lane can revise its conclusion based on the evidence.
    // This proves 'proof changing lane conclusions' end-to-end.
    if !proof_receipt_excerpts.is_empty() {
        prompt.push_str("## Routed proof evidence\n\n");
        prompt.push_str(
            "The following proof receipts are relevant to your concern. \
             Revise your conclusion in light of this evidence.\n\n",
        );
        for excerpt in proof_receipt_excerpts {
            prompt.push_str(&format!("- {excerpt}\n"));
        }
        prompt.push('\n');
    }
    // #325 stream-as-it-lands: late-phase deterministic outputs (full test,
    // build, coverage, leased witnesses) landed after this lane's primary
    // turn, so the lane reviewed on the fast-sensor precontext without them.
    if !late_sensor_excerpts.is_empty() {
        prompt.push_str("## Routed late deterministic evidence\n\n");
        prompt.push_str(
            "These sensor receipts landed after your primary turn (you \
             reviewed without them). Weigh them when revising.\n\n",
        );
        for excerpt in late_sensor_excerpts {
            prompt.push_str(&format!("- {excerpt}\n"));
        }
        prompt.push('\n');
    }
    prompt.push_str(
        "## Task\n\n\
         Re-examine your conclusion in light of the reporter's question and any \
         routed proof evidence. Revise, confirm, or withdraw your finding. \
         Return a JSON object: \
         {\"conclusion\": \"your revised or confirmed conclusion\", \
         \"changed\": true|false}.\n",
    );
    prompt
}

/// Continue a named lane's thread with turn-001: the reporter's question +
/// the lane's compact prior history. Returns the lane's revised conclusion
/// text. Writes turn-001 for the lane thread + emits a LaneAnswer message.
#[expect(
    clippy::too_many_arguments,
    reason = "continuation turn mirrors the reporter/proof context inputs; tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
fn run_lane_continuation_turn(
    root: &Path,
    review_dir: &Path,
    shared_context: &str,
    spec: &ProviderSpec,
    lane_receipt: &ModelLaneReceipt,
    question: &str,
    reporter_distillation: &str,
    args: &RunArgs,
    event_log: &EventLog,
    message_log: &MessageLog,
    proof_receipts: &[ProofReceipt],
    late_sensor_evidence: &[LateSensorDigest],
) -> Result<String> {
    // Route proof receipts back into the lane's continuation prompt (Order 9c
    // of #678). The lane sees the deterministic evidence and can revise its
    // conclusion — proving 'proof changing lane conclusions' end-to-end.
    //
    // Receipts are routed if EITHER:
    // - The lane explicitly requested them (requested_by match), OR
    // - They are run-level policy receipts (intelligent-ci-policy /
    //   proof-policy:*), since any lane investigating a Rust diff benefits
    //   from knowing whether the baseline checks passed. This is the common
    //   case: proof receipts are requested by the policy planner, not
    //   individual lanes, so a strict requested_by match would route nothing
    //   in most production runs.
    let proof_excerpts: Vec<String> = proof_receipts
        .iter()
        .filter(|receipt| {
            receipt.requested_by.iter().any(|r| r == &lane_receipt.lane)
                || receipt
                    .requested_by
                    .iter()
                    .any(|r| r.starts_with("proof-policy:") || r == "intelligent-ci-policy")
        })
        .map(|receipt| {
            format!(
                "proof `{}` result=`{}` reason=`{}`",
                receipt.id, receipt.result, receipt.reason
            )
        })
        .collect();
    let late_excerpts: Vec<String> = late_sensor_evidence
        .iter()
        .map(LateSensorDigest::excerpt)
        .collect();
    let prompt = lane_continuation_prompt(
        &lane_receipt.lane,
        "specialist reviewer",
        &lane_receipt.reason,
        question,
        reporter_distillation,
        &proof_excerpts,
        &late_excerpts,
    );
    let lane_thread_dir = review_dir.join("threads").join(&lane_receipt.lane);
    fs::create_dir_all(&lane_thread_dir)?;
    fs::write(lane_thread_dir.join("continuation-prompt-001.md"), &prompt)?;
    let content = call_model_prompt_content(
        root,
        &lane_thread_dir,
        spec,
        Some(shared_context),
        true,
        &prompt,
        args,
    )?;
    let revised =
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&content.json_payload) {
            parsed
                .get("conclusion")
                .and_then(|v| v.as_str())
                .unwrap_or(&content.json_payload)
                .to_owned()
        } else {
            content.json_payload
        };
    // Write turn-001 for the lane (the lane continued its thread).
    let turn = LaneThreadTurn {
        schema: LANE_THREAD_SCHEMA.to_owned(),
        thread_id: lane_receipt.thread_id.clone(),
        turn: 1,
        stage: "follow-up".to_owned(),
        prompt_packet_path: format!(
            "review/threads/{}/continuation-prompt-001.md",
            lane_receipt.lane
        ),
        response_summary: revised.clone(),
        routed_evidence_refs: vec![format!("reporter-question:{question}")],
        receipt_ref: format!("review/threads/{}/turn-001.json", lane_receipt.lane),
    };
    write_lane_thread_turn(
        review_dir,
        &lane_receipt.lane,
        &turn,
        &lane_receipt.cohort_id,
        "follow-up-answered",
    )?;
    let _ = message_log.append(
        CrossLaneMessageKind::LaneAnswer,
        &lane_receipt.lane,
        "reporter",
        1,
        vec![format!(
            "review/threads/{}/turn-001.json",
            lane_receipt.lane
        )],
        serde_json::json!({"answer": revised, "question": question}),
    );
    let _ = event_log.append(
        "lane_continuation_completed",
        serde_json::json!({"lane": lane_receipt.lane, "turn": 1}),
    );
    Ok(revised)
}

/// Order 9 (#678): the live reporter — same-model coordinator. Runs after the
/// primary wave, reads lane digests, makes one same-model distillation call
/// (same cohort, same cached prefix), records the conclusion as a reporter
/// thread artifact, and emits reporter messages. Advisory (feeds the compiler;
/// does not post or gate). Errors are logged, not propagated.
#[expect(
    clippy::too_many_arguments,
    reason = "reporter coordination mirrors the refuter/proof run context inputs; tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn run_reporter_coordination(
    root: &Path,
    review_dir: &Path,
    shared_context: &str,
    model_lanes: &[ModelLaneReceipt],
    proof_receipts: &[ProofReceipt],
    late_sensor_evidence: &[LateSensorDigest],
    args: &RunArgs,
    model_calls_used: usize,
    event_log: &EventLog,
    message_log: &MessageLog,
) -> Result<usize> {
    let digests = lane_digests_from_receipts(model_lanes);
    if digests.is_empty() || model_calls_used >= args.max_model_calls {
        let reason = if digests.is_empty() {
            "no lane digests"
        } else {
            "budget exhausted"
        };
        let _ = message_log.append(
            CrossLaneMessageKind::TopicUpdate,
            "reporter",
            "all-lanes",
            0,
            vec![],
            serde_json::json!({"topic": "reporter_skipped", "reason": reason}),
        );
        return Ok(0);
    }
    let spec = direct_minimax_spec(args);
    let prefix_hash = sha256_hex(shared_context.as_bytes());
    let cohort_id = cohort_id_for(spec.provider.key(), &spec.model, &prefix_hash);
    let thread_id = format!("{cohort_id}:reporter");
    let prompt = reporter_prompt(&digests, late_sensor_evidence);
    let reporter_dir = review_dir.join("threads").join("reporter");
    fs::create_dir_all(&reporter_dir)?;
    fs::write(reporter_dir.join("prompt.md"), &prompt)?;
    let content = match call_model_prompt_content(
        root,
        &reporter_dir,
        &spec,
        Some(shared_context),
        true,
        &prompt,
        args,
    ) {
        Ok(c) => c,
        Err(e) => {
            let _ = event_log.append(
                "reporter_call_failed",
                serde_json::json!({"error": format!("{e:#}")}),
            );
            let _ = message_log.append(
                CrossLaneMessageKind::TopicUpdate,
                "reporter",
                "all-lanes",
                0,
                vec![],
                serde_json::json!({"topic": "reporter_failed", "error": format!("{e:#}")}),
            );
            return Ok(1);
        }
    };
    let conclusion = parse_reporter_conclusion(&content.json_payload, &cohort_id, &thread_id);
    write_reporter_thread(review_dir, &conclusion)?;
    let _ = message_log.append(
        CrossLaneMessageKind::LaneReport,
        "reporter",
        "all-lanes",
        0,
        vec!["review/threads/reporter/turn-000.json".to_owned()],
        serde_json::json!({"distillation": conclusion.distillation, "cohort_id": conclusion.cohort_id}),
    );
    for question in &conclusion.proposed_follow_ups {
        let (raw_target, q_text) = question
            .split_once(':')
            .map(|(t, q)| (t.trim().to_owned(), q.trim().to_owned()))
            .unwrap_or((String::new(), question.clone()));
        // Resolve the target to an actual lane ID for the message routing.
        let known_ids: Vec<&str> = model_lanes
            .iter()
            .filter(|r| !r.thread_id.is_empty())
            .map(|r| r.lane.as_str())
            .collect();
        let resolved = resolve_lane_target(&raw_target, &known_ids).unwrap_or(raw_target.clone());
        let _ = message_log.append(
            CrossLaneMessageKind::ReporterQuestion,
            "reporter",
            &resolved,
            0,
            vec![],
            serde_json::json!({"question": q_text}),
        );
    }
    let _ = event_log.append(
        "reporter_completed",
        serde_json::json!({
            "distillation_length": conclusion.distillation.len(),
            "proposed_follow_ups": conclusion.proposed_follow_ups.len(),
        }),
    );

    // Multi-turn continuation (Order 9b of #678): the reporter proposed
    // follow-up questions for named lanes. For each question that targets a
    // known lane (by lane_id prefix) and where budget allows, continue that
    // lane's thread with turn-001: shared prefix + compact lane history +
    // reporter question. The lane revises its conclusion, which feeds back to
    // the reporter for a re-distillation (turn-001).
    let mut calls_used = model_calls_used + 1; // +1 for the reporter call above
    let mut lane_answers: Vec<(String, String)> = Vec::new();
    let known_lane_ids: Vec<&str> = model_lanes
        .iter()
        .filter(|r| !r.thread_id.is_empty())
        .map(|r| r.lane.as_str())
        .collect();
    for question in &conclusion.proposed_follow_ups {
        let (target, q_text) = match question
            .split_once(':')
            .map(|(t, q)| (t.trim().to_owned(), q.trim().to_owned()))
        {
            Some((t, q)) if !t.is_empty() && !q.is_empty() => (t, q),
            _ => continue,
        };
        // Strip common prefixes the model adds ("Question for X", "X lane", etc.)
        // and match against known lane IDs. Also try fuzzy matching: if any
        // known lane ID is a substring of the target, use that lane.
        let resolved_target = resolve_lane_target(&target, &known_lane_ids);
        let lane_receipt = match resolved_target {
            Some(lane_id) => model_lanes
                .iter()
                .find(|r| r.lane == lane_id && !r.thread_id.is_empty()),
            None => continue,
        };
        let lane_receipt = match lane_receipt {
            Some(r) => r,
            None => continue,
        };
        if calls_used >= args.max_model_calls {
            break;
        }
        let answer = run_lane_continuation_turn(
            root,
            review_dir,
            shared_context,
            &spec,
            lane_receipt,
            &q_text,
            &conclusion.distillation,
            args,
            event_log,
            message_log,
            proof_receipts,
            late_sensor_evidence,
        );
        calls_used += 1;
        if let Ok(revised) = answer {
            lane_answers.push((target.clone(), revised));
        }
    }

    // If any lane answered, the reporter re-distills (turn-001) with the
    // updated conclusions.
    if !lane_answers.is_empty() && calls_used < args.max_model_calls {
        let updated_digests: Vec<LaneDigest> = model_lanes
            .iter()
            .filter(|r| !r.thread_id.is_empty())
            .map(|r| {
                // If this lane answered, use the revised conclusion.
                let revised = lane_answers
                    .iter()
                    .find(|(lane, _)| lane == &r.lane)
                    .map(|(_, c)| c.as_str());
                LaneDigest {
                    lane: r.lane.clone(),
                    status: r.status.clone(),
                    conclusion: revised.unwrap_or(&r.reason).to_owned(),
                    thread_id: r.thread_id.clone(),
                }
            })
            .collect();
        let re_prompt = reporter_prompt(&updated_digests, late_sensor_evidence);
        let re_dir = review_dir.join("threads").join("reporter");
        calls_used += 1;
        if let Ok(re_content) = call_model_prompt_content(
            root,
            &re_dir,
            &spec,
            Some(shared_context),
            true,
            &re_prompt,
            args,
        ) {
            let re_conclusion =
                parse_reporter_conclusion(&re_content.json_payload, &cohort_id, &thread_id);
            // Write the reporter's revised conclusion as turn-001.
            let re_turn = LaneThreadTurn {
                schema: REPORTER_THREAD_SCHEMA.to_owned(),
                thread_id: thread_id.clone(),
                turn: 1,
                stage: "reporter".to_owned(),
                prompt_packet_path: "review/threads/reporter/prompt.md".to_owned(),
                response_summary: re_conclusion.distillation.clone(),
                routed_evidence_refs: lane_answers
                    .iter()
                    .map(|(lane, _)| format!("lane-answer:{lane}"))
                    .collect(),
                receipt_ref: "review/threads/reporter/turn-001.json".to_owned(),
            };
            let _ = write_lane_thread_turn(
                review_dir,
                "reporter",
                &re_turn,
                &cohort_id,
                "reporter_re_distilled",
            );
            let _ = message_log.append(
                CrossLaneMessageKind::LaneReport,
                "reporter",
                "all-lanes",
                1,
                vec!["review/threads/reporter/turn-001.json".to_owned()],
                serde_json::json!({
                    "distillation": re_conclusion.distillation,
                    "cohort_id": re_conclusion.cohort_id,
                    "round": "re-distillation",
                }),
            );
            let _ = event_log.append(
                "reporter_re_distilled",
                serde_json::json!({
                    "lane_answers": lane_answers.len(),
                    "distillation_length": re_conclusion.distillation.len(),
                }),
            );
        }
    }

    Ok(calls_used.saturating_sub(model_calls_used))
}

/// A lane's answer after deterministic proof arrived after the reporter's
/// first pass.  The request IDs are retained so the final claim compiler can
/// update only the claims that the receipt explicitly answers.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReceiptReconsideration {
    pub(crate) lane: String,
    pub(crate) receipt_ids: Vec<String>,
    pub(crate) request_ids: Vec<String>,
    pub(crate) conclusion: String,
    pub(crate) disposition: String,
    pub(crate) changed: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct ReceiptReconsiderationResult {
    pub(crate) reconsiderations: Vec<ReceiptReconsideration>,
    pub(crate) calls_attempted: usize,
}

/// Render the bounded, answer-shaped prompt used when proof lands after the
/// ordinary reporter continuation.  This is deliberately separate from the
/// reporter question prompt: the new evidence, rather than another model
/// opinion, is the reason the lane is being reopened.
pub(crate) fn receipt_reconsideration_prompt(
    lane: &str,
    prior_conclusion: &str,
    receipts: &[&ProofReceipt],
) -> String {
    let mut prompt = format!(
        "# Evidence reconsideration: `{lane}`\n\n\
         ## Prior conclusion\n\n{prior_conclusion}\n\n\
         ## New deterministic receipts\n\n"
    );
    for receipt in receipts {
        let reason: String = receipt.reason.chars().take(800).collect();
        prompt.push_str(&format!(
            "- receipt `{}`: result=`{}`; reason=`{}`; request_ids=`{}`\n",
            receipt.id,
            receipt.result,
            reason,
            receipt.request_ids.join(", "),
        ));
    }
    prompt.push_str(
        "\n## Task\n\n\
         Reconsider only the claims linked by these request IDs. Return strict JSON:\n\
         {\"conclusion\":\"...\",\"changed\":true|false,\
         \"disposition\":\"confirm|refute|narrow|park|withdraw|unchanged\"}.\n\
         `changed` is true only when the receipt changes the prior conclusion.\n",
    );
    prompt
}

fn next_lane_turn_number(review_dir: &Path, lane: &str) -> u32 {
    let Some(entries) = review_dir.join("threads").join(lane).read_dir().ok() else {
        return 1;
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter_map(|name| {
            name.strip_prefix("turn-")?
                .strip_suffix(".json")?
                .parse::<u32>()
                .ok()
        })
        .max()
        .map_or(1, |turn| turn.saturating_add(1))
}

/// Reopen receipt-linked lanes after the late proof broker.  The function is
/// intentionally bounded by the remaining model budget and skips the
/// compiler-only route.  It returns auditable answers; the caller applies
/// those answers to observations only when request identity is exact.
#[expect(
    clippy::too_many_arguments,
    reason = "receipt reconsideration carries the immutable run context and the bounded model budget"
)]
pub(crate) fn run_receipt_reconsiderations(
    root: &Path,
    review_dir: &Path,
    shared_context: &str,
    model_lanes: &mut [ModelLaneReceipt],
    receipts: &[ProofReceipt],
    args: &RunArgs,
    model_calls_used: usize,
    event_log: &EventLog,
    message_log: &MessageLog,
) -> Result<ReceiptReconsiderationResult> {
    let mut receipts_by_lane = BTreeMap::<String, Vec<&ProofReceipt>>::new();
    for receipt in receipts {
        for consumer in receipt_route_consumers(receipt) {
            if consumer != "compiler"
                && model_lanes
                    .iter()
                    .any(|lane| lane.lane == consumer && !lane.thread_id.is_empty())
            {
                receipts_by_lane.entry(consumer).or_default().push(receipt);
            }
        }
    }

    let mut result = ReceiptReconsiderationResult::default();
    let spec = direct_minimax_spec(args);
    for (lane_id, lane_receipts) in receipts_by_lane {
        if model_calls_used.saturating_add(result.calls_attempted) >= args.max_model_calls {
            let _ = event_log.append(
                "receipt_reconsideration_deferred",
                serde_json::json!({"lane": lane_id, "reason": "model-budget-exhausted"}),
            );
            break;
        }
        let Some(lane) = model_lanes.iter().find(|lane| lane.lane == lane_id) else {
            continue;
        };
        let prior_conclusion = lane.reason.clone();
        let prompt = receipt_reconsideration_prompt(&lane_id, &prior_conclusion, &lane_receipts);
        let turn = next_lane_turn_number(review_dir, &lane_id);
        let lane_dir = review_dir.join("threads").join(&lane_id);
        fs::create_dir_all(&lane_dir)?;
        let prompt_path = lane_dir.join(format!("evidence-reconsideration-{turn:03}.md"));
        fs::write(&prompt_path, &prompt)?;
        result.calls_attempted = result.calls_attempted.saturating_add(1);

        let content = match call_model_prompt_content(
            root,
            &lane_dir,
            &spec,
            Some(shared_context),
            true,
            &prompt,
            args,
        ) {
            Ok(content) => content,
            Err(error) => {
                let _ = event_log.append(
                    "receipt_reconsideration_failed",
                    serde_json::json!({"lane": lane_id, "error": format!("{error:#}")}),
                );
                continue;
            }
        };
        let parsed = serde_json::from_str::<serde_json::Value>(&content.json_payload).ok();
        let conclusion = parsed
            .as_ref()
            .and_then(|value| value.get("conclusion"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim()
            .to_owned();
        let changed = parsed
            .as_ref()
            .and_then(|value| value.get("changed"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let disposition = parsed
            .as_ref()
            .and_then(|value| value.get("disposition"))
            .and_then(serde_json::Value::as_str)
            .filter(|value| {
                matches!(
                    *value,
                    "confirm" | "refute" | "narrow" | "park" | "withdraw" | "unchanged"
                )
            })
            .unwrap_or("unchanged")
            .to_owned();
        let receipt_ids = lane_receipts
            .iter()
            .map(|receipt| receipt.id.clone())
            .collect();
        let request_ids = lane_receipts
            .iter()
            .flat_map(|receipt| receipt.request_ids.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let turn_ref = format!("review/threads/{lane_id}/turn-{turn:03}.json");
        let reconsideration = ReceiptReconsideration {
            lane: lane_id.clone(),
            receipt_ids,
            request_ids: request_ids.clone(),
            conclusion: conclusion.clone(),
            disposition: disposition.clone(),
            changed,
        };
        let turn_record = LaneThreadTurn {
            schema: LANE_THREAD_SCHEMA.to_owned(),
            thread_id: lane.thread_id.clone(),
            turn,
            stage: "follow-up-evidence".to_owned(),
            prompt_packet_path: format!(
                "review/threads/{lane_id}/evidence-reconsideration-{turn:03}.md"
            ),
            response_summary: conclusion.clone(),
            routed_evidence_refs: lane_receipts
                .iter()
                .map(|receipt| format!("review/proof_receipts.json#{}", receipt.id))
                .collect(),
            receipt_ref: turn_ref.clone(),
        };
        write_lane_thread_turn(
            review_dir,
            &lane_id,
            &turn_record,
            &lane.cohort_id,
            "follow-up-evidence-answered",
        )?;
        let _ = message_log.append(
            CrossLaneMessageKind::LaneAnswer,
            &lane_id,
            "reporter",
            turn,
            lane_receipts
                .iter()
                .map(|receipt| format!("review/proof_receipts.json#{}", receipt.id))
                .collect(),
            serde_json::json!({
                "source": "proof-receipt",
                "conclusion": conclusion.clone(),
                "changed": changed,
                "disposition": disposition.clone(),
                "request_ids": request_ids.clone(),
            }),
        );
        let _ = message_log.append(
            CrossLaneMessageKind::TopicUpdate,
            &lane_id,
            "compiler",
            turn,
            lane_receipts
                .iter()
                .map(|receipt| format!("review/proof_receipts.json#{}", receipt.id))
                .collect(),
            serde_json::json!({
                "source": "proof-receipt",
                "changed": changed,
                "disposition": disposition,
                "request_ids": request_ids,
            }),
        );
        apply_lane_reconsideration_update(model_lanes, &lane_id, &reconsideration, turn);
        result.reconsiderations.push(reconsideration);
    }
    Ok(result)
}

fn apply_lane_reconsideration_update(
    model_lanes: &mut [ModelLaneReceipt],
    lane_id: &str,
    reconsideration: &ReceiptReconsideration,
    turn: u32,
) {
    if !reconsideration.changed || reconsideration.conclusion.is_empty() {
        return;
    }
    if let Some(lane) = model_lanes.iter_mut().find(|lane| lane.lane == lane_id) {
        lane.reason = reconsideration.conclusion.clone();
        lane.turn = turn;
    }
}

/// Apply only exact request-linked reconsiderations to observations.  A lane
/// may own several unrelated claims; lane ownership alone must never rewrite
/// all of them from one receipt.
pub(crate) fn apply_receipt_reconsiderations(
    observations: &mut [Observation],
    reconsiderations: &[ReceiptReconsideration],
) -> usize {
    let mut changed = 0;
    for reconsideration in reconsiderations {
        if !reconsideration.changed || reconsideration.request_ids.is_empty() {
            continue;
        }
        for observation in observations.iter_mut().filter(|observation| {
            observation.lane == reconsideration.lane
                && reconsideration.request_ids.iter().any(|request_id| {
                    request_id == &observation.id || request_id == &observation.dedupe_key
                })
        }) {
            let next_status = match reconsideration.disposition.as_str() {
                "confirm" => Some("confirmed"),
                "refute" => Some("refuted"),
                "park" => Some("parked"),
                "withdraw" => Some("dropped"),
                "narrow" | "unchanged" => None,
                _ => None,
            };
            if let Some(status) = next_status {
                observation.status = status.to_owned();
            }
            observation.evidence.push(format!(
                "receipt reconsideration: {} ({})",
                reconsideration.conclusion, reconsideration.disposition
            ));
            observation.source = "proof-reconsideration".to_owned();
            changed += 1;
        }
    }
    changed
}

pub(crate) fn write_receipt_reconsideration_artifact(
    out: &Path,
    result: &ReceiptReconsiderationResult,
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    let artifact = serde_json::json!({
        "schema": "ub-review.receipt_reconsiderations.v1",
        "source_artifacts": ["review/proof_receipts.json", "review/messages.ndjson"],
        "calls_attempted": result.calls_attempted,
        "reconsiderations": result.reconsiderations,
    });
    fs::write(
        review_dir.join("receipt_reconsiderations.json"),
        serde_json::to_vec_pretty(&artifact)?,
    )?;
    Ok(())
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
    impact_candidates: Vec<crate::ImpactCandidateSummary>,
    args: &RunArgs,
) -> Result<ModelCallOutcome<LaneModelOutput>> {
    let input = build_proof_planner_input(
        diff,
        profile,
        box_state,
        pr_thread_context,
        proof_requests,
        impact_candidates,
    )?;
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
If impact_candidates are present in the planner input, prioritize proof that targets the highest-ranked candidates the deterministic planner skipped.
Prefer typed `proof_intents` for new proof requests: describe the question and expected answer shape, and let the deterministic broker resolve an approved command.

IMPORTANT: proof commands must use the EXACT syntax the proof broker's allowlist accepts.
- Use `--package <name>` not `-p <name>`.
- Always include `--locked`.
- For focused tests: `cargo test --locked --package <name> --test <target> [filter]`
- For focused builds: `cargo check --locked --package <name>` or `cargo build --locked --package <name>`
- Passthrough test args after `--`: only `--exact`, `--nocapture`, `--show-output`, `--ignored`, `--include-ignored`, `--test-threads N`.
- Commands not matching this syntax will be rejected by the broker and produce no receipt.

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
  ],
  "proof_intents": [
    {{
      "claim_id": "stable-claim-id",
      "question": "the material question this proof should answer",
      "expected_answer_shape": "the observable result that confirms or refutes it",
      "proof_kind": "focused-test|focused-build|base-plus-tests|sanitizer-witness|mutation-witness|miri-witness|source-route-probe|external-check",
      "target": "safe repository test, package, symbol, or route target",
      "estimated_value": "high|medium-high|medium|low"
    }}
  ]
}}

Hard caps: at most 2 observations, 2 failed_objections, 2 proof_requests, and 2 proof_intents.
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
    proof_intents: &mut Vec<ProofIntent>,
) {
    let advisory_output = LaneModelOutput {
        summary: None,
        inline_comments: Vec::new(),
        candidate_findings: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: output.observations,
        failed_objections: output.failed_objections,
        proof_requests: output.proof_requests,
        proof_intents: output.proof_intents,
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
            proof_intents,
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
    pub(crate) json_payload: String,
    pub(crate) parse_path: PathBuf,
    pub(crate) duration_ms: u128,
    pub(crate) http_status: Option<u16>,
    pub(crate) response_shape: String,
    pub(crate) cache_usage: ModelCacheUsage,
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
  ],
  "proof_intents": [
    {{
      "claim_id": "stable-claim-id",
      "question": "the material question this proof should answer",
      "expected_answer_shape": "the observable result that confirms or refutes it",
      "proof_kind": "focused-test|focused-build|base-plus-tests|sanitizer-witness|mutation-witness|miri-witness|source-route-probe|external-check",
      "target": "safe repository test, package, symbol, or route target",
      "estimated_value": "high|medium-high|medium|low"
    }}
  ]
}}

Hard caps: at most 3 observations, 2 candidate_findings, 1 summary_only_findings item, 2 failed_objections, 1 proof_request, and 2 proof_intents.
If there is no blocker/high/medium actionable issue, use empty arrays and put the failed-objection audit in summary.
Only propose candidate_findings for valid RIGHT-side changed or context lines in the PR diff.
Legacy `inline_comments` is accepted as an alias for `candidate_findings`, but prefer `candidate_findings`.
Do not post, mutate files, or run shell commands. Request executable proof through `proof_requests` or `proof_intents`.
Prefer `proof_intents` for new requests: describe the claim question and expected
answer, and let the deterministic broker choose an approved command. `target`
is a repository symbol or test/package label, never a shell command.
IMPORTANT: proof commands must use exact syntax the broker accepts. Use `--package <name>` (not `-p`), always include `--locked`. Examples: `cargo test --locked --package <name> --test <target>`, `cargo check --locked --package <name>`, `cargo doc --locked --package <name> --no-deps`. Commands with `-p`, missing `--locked`, or shell pipes will be rejected.
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
    pub(crate) proof_intents: &'a mut Vec<ProofIntent>,
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
        proof_intents,
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
    for intent in output.proof_intents {
        match validate_proof_intent(lane, intent, proof_intents.len(), model_observations.len()) {
            Ok(validated_intent) => proof_intents.push(validated_intent),
            Err(observation) => model_observations.push(*observation),
        }
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

fn validate_proof_intent(
    lane: &LanePlan,
    intent: ModelProofIntent,
    intent_ordinal: usize,
    observation_ordinal: usize,
) -> std::result::Result<ProofIntent, Box<Observation>> {
    let claim_id = intent.claim_id.trim();
    let question = intent.question.trim();
    let expected_answer_shape = intent.expected_answer_shape.trim();
    let target = intent.target.trim();
    let target_valid = !target.is_empty()
        && !target.starts_with('/')
        && !target.contains("..")
        && !target.contains(":/")
        && !has_shell_control_token(target)
        && !target.chars().any(char::is_whitespace)
        && target.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':' | '/')
        });

    let rejection_reason = if claim_id.is_empty() {
        Some("proof intent has empty claim_id")
    } else if question.is_empty() {
        Some("proof intent has empty question")
    } else if expected_answer_shape.is_empty() {
        Some("proof intent has empty expected_answer_shape")
    } else if !target_valid {
        Some("proof intent target is not a safe repository symbol, test, package, or route label")
    } else {
        None
    };

    if let Some(reason) = rejection_reason {
        return Err(Box::new(Observation {
            schema: "observation".to_owned(),
            id: format!("proof-intent-validation-{}-{observation_ordinal}", lane.id),
            lane: lane.id.clone(),
            question: "proof-intent-validation".to_owned(),
            claim: "Model proof intent was rejected by deterministic validation.".to_owned(),
            kind: "missing-evidence".to_owned(),
            status: "open".to_owned(),
            severity: "low".to_owned(),
            confidence: "high".to_owned(),
            path: None,
            line: None,
            fingerprint: sha256_hex(
                format!("{}\\n{}\\n{}\\n{reason}", lane.id, claim_id, target).as_bytes(),
            ),
            evidence: vec![format!(
                "lane={} claim_id={} target={} reason={reason}",
                lane.id,
                truncate_chars(claim_id, 80),
                truncate_chars(target, 120),
            )],
            dedupe_key: format!("proof-intent-validation-{}", lane.id),
            source: "proof-intent-validation".to_owned(),
        }));
    }

    let estimated_value = intent
        .estimated_value
        .as_deref()
        .map(str::trim)
        .filter(|value| matches!(*value, "high" | "medium-high" | "medium" | "low"))
        .unwrap_or("medium")
        .to_owned();
    let digest = sha256_hex(
        format!(
            "{}\n{}\n{}\n{}\n{}\n{}",
            lane.id,
            claim_id,
            question,
            expected_answer_shape,
            intent.proof_kind.key(),
            target
        )
        .as_bytes(),
    );

    Ok(ProofIntent {
        id: format!("intent-model-{}-{}", &digest[..16], intent_ordinal),
        claim_id: claim_id.to_owned(),
        question: truncate_chars(question, 500),
        expected_answer_shape: truncate_chars(expected_answer_shape, 300),
        proof_kind: intent.proof_kind,
        target: target.to_owned(),
        estimated_value,
        requested_by: vec![lane.id.clone()],
        status: "requested".to_owned(),
    })
}

#[cfg(test)]
mod receipt_reconsideration_tests {
    use anyhow::Result;

    use super::*;

    fn observation(id: &str) -> Observation {
        Observation {
            schema: "observation".to_owned(),
            id: id.to_owned(),
            lane: "tests-oracle".to_owned(),
            question: "does the focused proof answer this claim?".to_owned(),
            claim: id.to_owned(),
            kind: "bug".to_owned(),
            status: "confirmed".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(12),
            fingerprint: format!("fingerprint-{id}"),
            evidence: vec!["model observation".to_owned()],
            dedupe_key: format!("dedupe-{id}"),
            source: "model".to_owned(),
        }
    }

    #[test]
    fn reconsideration_prompt_is_answer_shaped_and_claim_linked() {
        let receipt = ProofReceipt {
            schema: "proof".to_owned(),
            id: "receipt-1".to_owned(),
            kind: "focused-test".to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            request_ids: vec!["claim-1".to_owned()],
            commands: Vec::new(),
            result: "head_failed".to_owned(),
            reason: "the focused test still fails".to_owned(),
        };
        let prompt =
            receipt_reconsideration_prompt("tests-oracle", "the prior conclusion", &[&receipt]);
        assert!(prompt.contains("receipt-1"));
        assert!(prompt.contains("claim-1"));
        assert!(prompt.contains("confirm|refute|narrow|park|withdraw|unchanged"));
    }

    fn lane_receipt(reason: &str, turn: u32) -> ModelLaneReceipt {
        ModelLaneReceipt {
            lane: "tests-oracle".to_owned(),
            provider: "minimax".to_owned(),
            model: "MiniMax-M3".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            status: "ok".to_owned(),
            reason: reason.to_owned(),
            duration_ms: None,
            http_status: None,
            response_shape: None,
            fallback_from: None,
            cache_usage: ModelCacheUsage::default(),
            cohort_id: String::new(),
            shared_prefix_hash: String::new(),
            thread_id: String::new(),
            turn,
            cohort_broken: false,
        }
    }

    #[test]
    fn unchanged_reconsideration_preserves_lane_audit_state() -> Result<()> {
        let mut lanes = vec![lane_receipt("prior conclusion", 2)];
        let unchanged = ReceiptReconsideration {
            lane: "tests-oracle".to_owned(),
            receipt_ids: vec!["receipt-1".to_owned()],
            request_ids: vec!["claim-1".to_owned()],
            conclusion: "same conclusion, restated".to_owned(),
            disposition: "unchanged".to_owned(),
            changed: false,
        };
        apply_lane_reconsideration_update(&mut lanes, "tests-oracle", &unchanged, 3);
        anyhow::ensure!(lanes[0].reason == "prior conclusion");
        anyhow::ensure!(lanes[0].turn == 2);

        let mut changed = unchanged;
        changed.changed = true;
        changed.disposition = "confirm".to_owned();
        changed.conclusion = "proof confirms the claim".to_owned();
        apply_lane_reconsideration_update(&mut lanes, "tests-oracle", &changed, 3);
        anyhow::ensure!(lanes[0].reason == "proof confirms the claim");
        anyhow::ensure!(lanes[0].turn == 3);
        Ok(())
    }

    #[test]
    fn reconsideration_updates_only_exactly_linked_observations() {
        let mut observations = vec![observation("claim-1"), observation("claim-2")];
        let updates = vec![ReceiptReconsideration {
            lane: "tests-oracle".to_owned(),
            receipt_ids: vec!["receipt-1".to_owned()],
            request_ids: vec!["claim-1".to_owned()],
            conclusion: "the focused proof refutes claim-1".to_owned(),
            disposition: "refute".to_owned(),
            changed: true,
        }];

        assert_eq!(
            apply_receipt_reconsiderations(&mut observations, &updates),
            1
        );
        assert_eq!(observations[0].status, "refuted");
        assert_eq!(observations[0].source, "proof-reconsideration");
        assert_eq!(observations[1].status, "confirmed");
        assert_eq!(observations[1].source, "model");
    }

    #[test]
    fn one_receipt_changes_two_exact_lane_topic_dispositions() -> Result<()> {
        let first = observation("claim-1");
        let mut second = observation("claim-2");
        second.lane = "opposition".to_owned();
        let mut unrelated = observation("claim-3");
        unrelated.lane = "opposition".to_owned();
        let mut observations = vec![first.clone(), second.clone(), unrelated];
        let updates = vec![
            ReceiptReconsideration {
                lane: first.lane.clone(),
                receipt_ids: vec!["receipt-shared".to_owned()],
                request_ids: vec![first.id.clone()],
                conclusion: "the receipt confirms claim-1".to_owned(),
                disposition: "confirm".to_owned(),
                changed: true,
            },
            ReceiptReconsideration {
                lane: second.lane.clone(),
                receipt_ids: vec!["receipt-shared".to_owned()],
                request_ids: vec![second.id.clone()],
                conclusion: "the receipt refutes claim-2".to_owned(),
                disposition: "refute".to_owned(),
                changed: true,
            },
        ];

        anyhow::ensure!(apply_receipt_reconsiderations(&mut observations, &updates) == 2);
        anyhow::ensure!(observations[0].status == "confirmed");
        anyhow::ensure!(observations[1].status == "refuted");
        anyhow::ensure!(observations[2].status == "confirmed");
        anyhow::ensure!(observations[0].source == "proof-reconsideration");
        anyhow::ensure!(observations[1].source == "proof-reconsideration");
        anyhow::ensure!(observations[2].source == "model");
        Ok(())
    }
}
