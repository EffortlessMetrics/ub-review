//! Model output validation and observation construction (cleanup
//! train step 21, pure code motion).

use crate::*;

pub(crate) fn validate_model_observation(
    lane: &LanePlan,
    candidate: ModelCandidateObservation,
    index: usize,
) -> Observation {
    let claim = non_empty_or(
        candidate.claim.trim(),
        "model observation guard rejected empty claim",
    );
    let evidence = non_empty_evidence(candidate.evidence, "model observation");
    let kind = candidate
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|kind| {
            allowed_observation_kind(kind)
                || matches!(*kind, "empty-internal-audit" | "malformed-internal-audit")
        })
        .unwrap_or_else(|| infer_observation_kind(&lane.id, &claim, &evidence.join("\n")));
    let internal_audit_kind = matches!(kind, "empty-internal-audit" | "malformed-internal-audit");
    let status = candidate
        .status
        .as_deref()
        .map(str::trim)
        .filter(|status| {
            allowed_observation_status(status)
                || (internal_audit_kind && matches!(*status, "degraded" | "failed"))
        })
        .unwrap_or("open");
    let severity = candidate
        .severity
        .as_deref()
        .map(str::trim)
        .filter(|severity| matches!(*severity, "blocker" | "high" | "medium" | "low"))
        .unwrap_or("low");
    let confidence = candidate
        .confidence
        .as_deref()
        .map(str::trim)
        .filter(|confidence| matches!(*confidence, "high" | "medium-high" | "medium" | "low"))
        .unwrap_or("medium");
    let path = candidate
        .path
        .as_deref()
        .map(normalize_repo_path)
        .filter(|path| !path.is_empty());
    if let Some(observation) = sibling_completeness_overclaim_observation_from_text(
        lane,
        &format!("{claim}\n{}", evidence.join("\n")),
        evidence.clone(),
        path.as_ref(),
        candidate.line,
        index,
        "model-sibling-completeness-guard",
    ) {
        return observation;
    }
    if let Some(observation) = box_from_allocation_false_premise_observation_from_text(
        lane,
        &format!("{claim}\n{}", evidence.join("\n")),
        evidence.clone(),
        path.as_ref(),
        candidate.line,
        index,
        "model-false-premise-guard",
    ) {
        return observation;
    }
    make_observation(ObservationInput {
        index,
        lane: &lane.id,
        question: candidate.question.as_deref().unwrap_or(lane.id.as_str()),
        claim: &claim,
        kind,
        status,
        severity,
        confidence,
        path: path.as_ref(),
        line: candidate.line,
        evidence,
        dedupe_key: candidate.dedupe_key.as_deref(),
        source: "model-observation",
    })
}

pub(crate) fn validate_failed_objection(
    lane: &LanePlan,
    objection: ModelFailedObjection,
    index: usize,
) -> Observation {
    let claim = non_empty_or(
        objection.claim.trim(),
        "model failed objection missing claim",
    );
    let reason = non_empty_or(
        objection.reason.trim(),
        "model failed objection missing reason",
    );
    let full_claim = format!("{claim}; refuted because: {reason}");
    let evidence = non_empty_evidence(objection.evidence, "failed objection audit");
    if let Some(observation) = sibling_completeness_overclaim_observation_from_text(
        lane,
        &format!("{full_claim}\n{}", evidence.join("\n")),
        evidence.clone(),
        None,
        None,
        index,
        "model-sibling-completeness-guard",
    ) {
        return observation;
    }
    if let Some(observation) = box_from_allocation_false_premise_observation_from_text(
        lane,
        &format!("{full_claim}\n{}", evidence.join("\n")),
        evidence.clone(),
        None,
        None,
        index,
        "model-failed-objection",
    ) {
        return observation;
    }
    let kind = objection
        .kind
        .as_deref()
        .map(str::trim)
        .filter(|kind| allowed_observation_kind(kind))
        .unwrap_or_else(|| {
            if reason.to_ascii_lowercase().contains("false premise") {
                "false-premise"
            } else {
                "resolved-check"
            }
        });
    let confidence = objection
        .confidence
        .as_deref()
        .map(str::trim)
        .filter(|confidence| matches!(*confidence, "high" | "medium-high" | "medium" | "low"))
        .unwrap_or("medium");
    make_observation(ObservationInput {
        index,
        lane: &lane.id,
        question: "failed-objection",
        claim: &full_claim,
        kind,
        status: "refuted",
        severity: "low",
        confidence,
        path: None,
        line: None,
        evidence,
        dedupe_key: None,
        source: "model-failed-objection",
    })
}

pub(crate) const SIBLING_COMPLETENESS_OVERCLAIM_DEDUPE_KEY: &str =
    "sibling-path-completeness-overclaim";
pub(crate) const SIBLING_COMPLETENESS_OVERCLAIM_CLAIM: &str = "Check sibling-path scan coverage before treating the fix as complete; a narrow no-match scan is not proof that no siblings exist.";

pub(crate) fn sibling_completeness_overclaim_observation_from_text(
    lane: &LanePlan,
    text: &str,
    evidence: Vec<String>,
    path: Option<&String>,
    line: Option<u32>,
    index: usize,
    source: &str,
) -> Option<Observation> {
    if !is_sibling_completeness_overclaim(&lane.id, text, &evidence) {
        return None;
    }
    let mut evidence = non_empty_evidence(evidence, "sibling completeness guard");
    let invariant = "Sibling-path calibration: narrow no-match scans must report coverage and cannot assert global sibling absence.";
    if !evidence.iter().any(|item| item == invariant) {
        evidence.push(invariant.to_owned());
    }
    let unsupported = format!(
        "Unsupported sibling completeness claim: {}",
        truncate_chars(text.trim(), 240)
    );
    if !unsupported.trim().is_empty() && !evidence.iter().any(|item| item == &unsupported) {
        evidence.push(unsupported);
    }
    Some(make_observation(ObservationInput {
        index,
        lane: &lane.id,
        question: "sibling-path-coverage",
        claim: SIBLING_COMPLETENESS_OVERCLAIM_CLAIM,
        kind: "source-route-gap",
        status: "open",
        severity: "medium",
        confidence: "high",
        path,
        line,
        evidence,
        dedupe_key: Some(SIBLING_COMPLETENESS_OVERCLAIM_DEDUPE_KEY),
        source,
    }))
}

pub(crate) fn is_sibling_completeness_overclaim(
    lane_id: &str,
    text: &str,
    evidence: &[String],
) -> bool {
    let lane_id = lane_id.to_ascii_lowercase();
    let evidence_text = evidence.join("\n");
    let combined = format!("{text}\n{evidence_text}").to_ascii_lowercase();
    let lane_hint = lane_id.contains("source-route") || lane_id.contains("sibling");
    let mentions_sibling = combined.contains("sibling") || combined.contains("analogous");
    if !mentions_sibling || !lane_hint {
        return false;
    }
    if has_broad_sibling_coverage_claim(&combined) {
        return false;
    }

    let negative_scan = contains_any(
        &combined,
        &[
            "no sibling",
            "no siblings",
            "no analogous",
            "none widen",
            "none of the sibling",
            "not found",
            "no match",
            "no matches",
            "nothing else",
        ],
    );
    let completeness_claim = contains_any(
        &combined,
        &[
            "correctly scoped",
            "need not be broadened",
            "does not need to be broadened",
            "no need to broaden",
            "complete fix",
            "fix is complete",
            "scope is complete",
            "no siblings exist",
            "no sibling paths exist",
            "no sibling concern",
            "no sibling gap",
        ],
    );
    let scoped_no_match = has_honest_limited_sibling_scope(&combined) && !completeness_claim;
    if scoped_no_match {
        return false;
    }
    (negative_scan && completeness_claim)
        || contains_any(
            &combined,
            &[
                "no siblings exist",
                "no sibling paths exist",
                "no analogous sibling",
            ],
        )
}

pub(crate) fn has_broad_sibling_coverage_claim(text: &str) -> bool {
    contains_any(
        text,
        &[
            "across all",
            "all ffi entry",
            "all entry point",
            "all public route",
            "all sibling",
            "every sibling",
            "every ffi",
            "exhaustive",
            "meta-class",
        ],
    )
}

pub(crate) fn has_honest_limited_sibling_scope(text: &str) -> bool {
    contains_any(
        text,
        &[
            "checked scope",
            "scan scope",
            "scanned scope",
            "limited to",
            "did not scan",
            "not scanned",
            "unscanned",
            "only checked",
            "only scanned",
        ],
    )
}

pub(crate) fn contains_any(value: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| value.contains(needle))
}

pub(crate) const BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY: &str =
    "rust-box-from-allocation-failure";
pub(crate) const BOX_FROM_ALLOCATION_FALSE_PREMISE_CLAIM: &str = "`Box::from(slice)` allocation failure does not return `None`; recoverable fallback claims are dropped.";

pub(crate) fn box_from_allocation_false_premise_observation_from_candidate(
    lane: &LanePlan,
    candidate: &ModelCandidateComment,
    index: usize,
) -> Option<Observation> {
    let text = format!("{}\n{}", candidate.body, candidate.evidence);
    let path = normalize_repo_path(&candidate.path);
    let path = if path.is_empty() { None } else { Some(path) };
    box_from_allocation_false_premise_observation_from_text(
        lane,
        &text,
        vec![candidate.evidence.clone()],
        path.as_ref(),
        Some(candidate.line),
        index,
        "model-false-premise-guard",
    )
}

pub(crate) fn box_from_allocation_false_premise_observation_from_summary_only(
    lane: &LanePlan,
    candidate: &ModelCandidateFinding,
    index: usize,
) -> Option<Observation> {
    box_from_allocation_false_premise_observation_from_text(
        lane,
        &format!("{}\n{}", candidate.reason, candidate.evidence),
        vec![candidate.evidence.clone()],
        None,
        None,
        index,
        "model-false-premise-guard",
    )
}

pub(crate) fn box_from_allocation_false_premise_observation_from_text(
    lane: &LanePlan,
    text: &str,
    evidence: Vec<String>,
    path: Option<&String>,
    line: Option<u32>,
    index: usize,
    source: &str,
) -> Option<Observation> {
    if !is_box_from_allocation_false_premise(text) {
        return None;
    }
    let mut evidence = non_empty_evidence(evidence, "model false-premise guard");
    let invariant =
        "Rust allocation semantics: Box::from(&[u8]) does not return None on allocation failure.";
    if !evidence.iter().any(|item| item == invariant) {
        evidence.push(invariant.to_owned());
    }
    Some(make_observation(ObservationInput {
        index,
        lane: &lane.id,
        question: "false-premise",
        claim: BOX_FROM_ALLOCATION_FALSE_PREMISE_CLAIM,
        kind: "false-premise",
        status: "refuted",
        severity: "low",
        confidence: "high",
        path,
        line,
        evidence,
        dedupe_key: Some(BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY),
        source,
    }))
}

pub(crate) fn is_box_from_allocation_false_premise(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    let compact = lower
        .chars()
        .filter(|ch| !ch.is_whitespace() && *ch != '`')
        .collect::<String>();
    let mentions_box_from =
        compact.contains("box::from(") || compact.contains("box::<[u8]>::from(");
    let mentions_allocation = lower.contains("allocation failure")
        || lower.contains("allocation fails")
        || lower.contains("alloc failure")
        || lower.contains("out of memory")
        || lower.contains("oom");
    let mentions_recoverable_result = lower.contains("none")
        || lower.contains("empty box")
        || lower.contains("fallback")
        || lower.contains("fall through")
        || lower.contains("fallthrough");
    mentions_box_from && mentions_allocation && mentions_recoverable_result
}

pub(crate) fn validate_proof_request(
    lane: &LanePlan,
    request: ModelProofRequest,
    index: usize,
) -> ProofRequest {
    build_proof_request(
        &lane.id,
        vec![lane.id.clone()],
        &request.command,
        &request.reason,
        "model proof request missing reason",
        request.cost.as_deref(),
        request.timeout_sec,
        request.required.unwrap_or(false),
        index,
    )
}

/// Normalize a model-suggested proof command to match the broker's allowlist:
/// - Replace `-p <name>` with `--package <name>` (short to long form).
/// - Strip shell pipes (`| ...`) and redirects (`2>&1`, `> file`, etc.).
/// - Add `--locked` after `cargo test/build/check/doc` if missing.
/// - Strip `--nocapture` from after `--` if present (not in passthrough allowlist).
///
/// If the command doesn't start with `cargo`, return it unchanged (the broker
/// will reject non-cargo commands via its own allowlist).
pub(crate) fn normalize_proof_command(command: &str) -> String {
    let mut cmd = command.trim().to_owned();
    // Strip shell pipes and redirects: take only the part before the first pipe/redirect.
    for sep in [" | ", " 2>&1", " >/dev/null", " > /dev/null", " && "] {
        if let Some(idx) = cmd.find(sep) {
            cmd.truncate(idx);
        }
    }
    // Replace `-p <name>` with `--package <name>`.
    cmd = cmd.replace(" -p ", " --package ");
    // Add --locked after cargo subcommand if missing.
    if cmd.starts_with("cargo ") && !cmd.contains("--locked") {
        // Insert --locked right after the subcommand word.
        let parts: Vec<&str> = cmd.splitn(3, ' ').collect();
        if parts.len() >= 2 {
            cmd = format!(
                "{} {} --locked {}",
                parts[0],
                parts[1],
                parts.get(2).unwrap_or(&"")
            );
            cmd = cmd.trim_end().to_owned();
        }
    }
    // Strip --nocapture from after -- (not in passthrough allowlist).
    cmd = cmd.replace(" --nocapture", "");
    cmd = cmd.replace(" -- --", " --");
    cmd.trim().to_owned()
}

pub(crate) fn has_shell_control_token(command: &str) -> bool {
    command
        .chars()
        .any(|ch| matches!(ch, '&' | '|' | ';' | '`' | '>' | '<' | '$'))
}

pub(crate) fn classify_proof_cost(cost: Option<&str>, command: &str) -> String {
    let supplied = cost.unwrap_or("").trim().to_ascii_lowercase();
    if matches!(
        supplied.as_str(),
        "focused-test" | "focused-build" | "manual"
    ) {
        return supplied;
    }
    let command = command.to_ascii_lowercase();
    if supplied.contains("test")
        || command.contains(" test ")
        || command.starts_with("bun test")
        || command.starts_with("cargo test")
        || command.starts_with("npm test")
    {
        return "focused-test".to_owned();
    }
    if supplied.contains("build")
        || command.contains(" build")
        || command.starts_with("cargo build")
        || command.starts_with("bun build")
        || command.starts_with("ninja")
        || command.starts_with("cmake")
    {
        return "focused-build".to_owned();
    }
    "manual".to_owned()
}

pub(crate) fn non_empty_or(value: &str, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_owned()
    } else {
        value.to_owned()
    }
}

pub(crate) fn non_empty_evidence(values: Vec<String>, fallback: &str) -> Vec<String> {
    let cleaned = values
        .into_iter()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    if cleaned.is_empty() {
        vec![fallback.to_owned()]
    } else {
        cleaned
    }
}

pub(crate) fn allowed_observation_kind(value: &str) -> bool {
    matches!(
        value,
        "bug"
            | "verification-question"
            | "missing-evidence"
            | "test-gap"
            | "source-route-gap"
            | "security-risk"
            | "false-premise"
            | "parked-follow-up"
            | "residual-risk"
            | "resolved-check"
    )
}

pub(crate) fn allowed_observation_status(value: &str) -> bool {
    matches!(
        value,
        "open" | "covered" | "confirmed" | "refuted" | "demoted" | "parked" | "duplicate"
    )
}

pub(crate) fn is_candidate_only_lane(lane_id: &str) -> bool {
    is_opencode_fast_lane(lane_id)
}

pub(crate) fn validate_lane_model_summary(lane: &LanePlan, summary: &str) -> SummaryOnlyFinding {
    let reason = summary.trim().to_owned();
    let reason_present = !reason.is_empty();
    let concise = reason.chars().count() <= 1_200;
    let no_standalone_approval = !has_standalone_approval_line(&reason);

    if reason_present && concise && no_standalone_approval {
        SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason,
            evidence: "lane model summary".to_owned(),
        }
    } else {
        SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason: format!(
                "lane model summary guard rejected summary; reason_present={} concise={} no_standalone_approval={}",
                reason_present, concise, no_standalone_approval
            ),
            evidence: "lane model summary guardrail".to_owned(),
        }
    }
}

pub(crate) fn validate_summary_only_candidate(
    lane: &LanePlan,
    candidate: ModelCandidateFinding,
) -> SummaryOnlyFinding {
    let severity = candidate.severity.trim().to_owned();
    let confidence = candidate.confidence.trim().to_owned();
    let reason = candidate.reason.trim().to_owned();
    let evidence = candidate.evidence.trim().to_owned();
    let severity_allowed = matches!(severity.as_str(), "blocker" | "high" | "medium" | "low");
    let confidence_allowed = matches!(confidence.as_str(), "high" | "medium-high" | "medium");
    let reason_present = !reason.is_empty();
    let evidence_present = !evidence.is_empty();
    let concise = reason.chars().count() <= 1_200 && evidence.chars().count() <= 1_200;

    if severity_allowed && confidence_allowed && reason_present && evidence_present && concise {
        SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity,
            confidence,
            reason,
            evidence,
        }
    } else {
        SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason: format!(
                "summary-only guard rejected candidate; severity_allowed={} confidence_allowed={} reason_present={} evidence_present={} concise={}",
                severity_allowed, confidence_allowed, reason_present, evidence_present, concise
            ),
            evidence: "model summary-only candidate guardrail".to_owned(),
        }
    }
}

pub(crate) fn dedupe_inline_comments(
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) {
    let mut deduped = BTreeMap::new();
    for comment in std::mem::take(inline_comments) {
        let key = (comment.path.clone(), comment.line);
        if let Some(existing) = deduped.get_mut(&key) {
            let dropped = if inline_comment_rank(&comment) > inline_comment_rank(existing) {
                std::mem::replace(existing, comment)
            } else {
                comment
            };
            merge_duplicate_inline_evidence(existing, &dropped);
            summary_only_findings.push(SummaryOnlyFinding {
                lane: dropped.lane,
                severity: dropped.severity,
                confidence: dropped.confidence,
                reason: format!(
                    "duplicate inline candidate merged into {}:{}",
                    dropped.path, dropped.line
                ),
                evidence: dropped.evidence,
            });
        } else {
            deduped.insert(key, comment);
        }
    }
    inline_comments.extend(deduped.into_values());
    dedupe_same_claim_inline_comments(inline_comments, summary_only_findings);
    // #178 value ranking: the body leads with the best finding. Survivors
    // order by severity then confidence (descending), with path:line as the
    // stable tiebreak so equal-rank findings keep a deterministic order.
    inline_comments.sort_by(|a, b| {
        inline_comment_rank(b)
            .cmp(&inline_comment_rank(a))
            .then_with(|| (a.path.as_str(), a.line).cmp(&(b.path.as_str(), b.line)))
    });
}

pub(crate) fn dedupe_same_claim_inline_comments(
    inline_comments: &mut Vec<ReviewInlineComment>,
    summary_only_findings: &mut Vec<SummaryOnlyFinding>,
) {
    let mut deduped = Vec::<ReviewInlineComment>::new();
    for comment in std::mem::take(inline_comments) {
        let duplicate_index = deduped
            .iter()
            .position(|existing| same_inline_claim(existing, &comment));
        if let Some(index) = duplicate_index {
            let dropped = if inline_comment_rank(&comment) > inline_comment_rank(&deduped[index]) {
                std::mem::replace(&mut deduped[index], comment)
            } else {
                comment
            };
            let kept = &mut deduped[index];
            let kept_location = format!("{}:{}", kept.path, kept.line);
            let dropped_location = format!("{}:{}", dropped.path, dropped.line);
            merge_duplicate_inline_evidence(kept, &dropped);
            summary_only_findings.push(SummaryOnlyFinding {
                lane: dropped.lane,
                severity: dropped.severity,
                confidence: dropped.confidence,
                reason: format!(
                    "same-claim inline candidate at {dropped_location} merged into {kept_location}"
                ),
                evidence: dropped.evidence,
            });
        } else {
            deduped.push(comment);
        }
    }
    inline_comments.extend(deduped);
}

pub(crate) fn same_inline_claim(left: &ReviewInlineComment, right: &ReviewInlineComment) -> bool {
    if left.path != right.path || left.suggestion.is_some() || right.suggestion.is_some() {
        return false;
    }
    let left_text = normalized_inline_claim_text(left);
    let right_text = normalized_inline_claim_text(right);
    if left_text.chars().count() < 32 || right_text.chars().count() < 32 {
        return false;
    }
    if left_text == right_text {
        return true;
    }
    // Cross-line merging is a destructive operation.  Similar prose is only
    // a clustering hint; require a shared, recognized failure family and a
    // shared code token so distinct claims about the same vocabulary (for
    // example, two declaration forms) remain independently reviewable.
    let Some(left_family) = inline_claim_family(&left_text) else {
        return false;
    };
    if Some(left_family) != inline_claim_family(&right_text) {
        return false;
    }
    let left_code_tokens = inline_claim_code_tokens(&reviewer_facing_pr_text(&left.body));
    let right_code_tokens = inline_claim_code_tokens(&reviewer_facing_pr_text(&right.body));
    if left_code_tokens.is_empty()
        || right_code_tokens.is_empty()
        || left_code_tokens.is_disjoint(&right_code_tokens)
    {
        return false;
    }
    let left_tokens = inline_claim_tokens(&left_text);
    let right_tokens = inline_claim_tokens(&right_text);
    if left_tokens.len() < 5 || right_tokens.len() < 5 {
        return false;
    }
    let common = left_tokens.intersection(&right_tokens).count();
    if common < 5 {
        return false;
    }
    let min_len = left_tokens.len().min(right_tokens.len());
    let union = left_tokens.union(&right_tokens).count();
    let min_overlap_percent = common * 100 / min_len;
    let union_overlap_percent = common * 100 / union;
    min_overlap_percent >= 60 && (union_overlap_percent >= 35 || common >= 6)
}

fn inline_claim_family(text: &str) -> Option<&'static str> {
    let text = text.to_ascii_lowercase();
    let has = |terms: &[&str]| terms.iter().any(|term| text.contains(term));
    if has(&["assert", "oracle", "tothrow", "discriminat"]) {
        Some("test-oracle")
    } else if has(&["subscript", "postfix", "index", "slice"]) {
        Some("indexing")
    } else if has(&["error", "propagat", "fallback", "result"]) {
        Some("error-path")
    } else if has(&["unsafe", "safety", "alias", "memory", "undefined behavior"]) {
        Some("safety")
    } else if has(&["workflow", "action", "permission", "pin"]) {
        Some("workflow")
    } else {
        None
    }
}

fn inline_claim_code_tokens(text: &str) -> BTreeSet<String> {
    let mut tokens = BTreeSet::new();
    for (index, segment) in text.split('`').enumerate() {
        if index % 2 != 1 {
            continue;
        }
        for token in segment.split(|character: char| {
            !(character.is_ascii_alphanumeric() || matches!(character, '_' | '$' | '%' | '@' | '#'))
        }) {
            let token = token.to_ascii_lowercase();
            if token.len() >= 2 {
                tokens.insert(token);
            }
        }
    }
    tokens
}

pub(crate) fn normalized_inline_claim_text(comment: &ReviewInlineComment) -> String {
    normalized_review_text(&reviewer_facing_pr_text(&comment.body))
}

pub(crate) fn inline_claim_tokens(text: &str) -> BTreeSet<String> {
    text.split_whitespace()
        .filter_map(normalize_inline_claim_token)
        .collect()
}

pub(crate) fn normalize_inline_claim_token(token: &str) -> Option<String> {
    const STOP_WORDS: &[&str] = &[
        "the", "a", "an", "this", "that", "it", "is", "are", "to", "for", "and", "or", "of", "in",
        "on", "with", "from", "at", "by", "as", "be", "because", "but", "if", "then", "when",
        "only", "still", "also", "line",
    ];
    if token.len() < 3 || STOP_WORDS.contains(&token) {
        return None;
    }
    let normalized = if token.starts_with("assert") {
        "assert".to_owned()
    } else if token.contains("discriminat") {
        "discriminat".to_owned()
    } else if token.starts_with("throw") {
        "throw".to_owned()
    } else if token.ends_with("ions") && token.len() > 7 {
        token.trim_end_matches("ions").to_owned()
    } else if token.ends_with("ion") && token.len() > 6 {
        token.trim_end_matches("ion").to_owned()
    } else if token.ends_with("ing") && token.len() > 6 {
        token.trim_end_matches("ing").to_owned()
    } else if token.ends_with("ed") && token.len() > 5 {
        token.trim_end_matches("ed").to_owned()
    } else if token.ends_with('s') && token.len() > 5 {
        token.trim_end_matches('s').to_owned()
    } else {
        token.to_owned()
    };
    (normalized.len() >= 3).then_some(normalized)
}

pub(crate) fn inline_comment_rank(comment: &ReviewInlineComment) -> (u8, u8, u8) {
    (
        inline_evidence_rank(comment),
        severity_rank(&comment.severity),
        confidence_rank(&comment.confidence),
    )
}

pub(crate) fn inline_evidence_rank(comment: &ReviewInlineComment) -> u8 {
    let evidence = comment.evidence.to_ascii_lowercase();
    if evidence.contains("executed")
        || evidence.contains("receipt")
        || evidence.contains("focused test")
        || evidence.contains("red/green")
        || evidence.contains("sensor")
    {
        5
    } else if evidence.contains("source") || evidence.contains("diff") {
        3
    } else if evidence.contains("thread") || evidence.contains("existing") {
        2
    } else {
        1
    }
}

pub(crate) fn ranked_inline_comments(
    inline_comments: &[ReviewInlineComment],
) -> Vec<ReviewInlineComment> {
    let mut ranked = inline_comments.to_vec();
    ranked.sort_by(|left, right| {
        inline_comment_rank(right)
            .cmp(&inline_comment_rank(left))
            .then_with(|| left.path.cmp(&right.path))
            .then_with(|| left.line.cmp(&right.line))
            .then_with(|| left.lane.cmp(&right.lane))
            .then_with(|| left.body.cmp(&right.body))
    });
    ranked
}

pub(crate) fn severity_rank(value: &str) -> u8 {
    match value {
        "blocker" => 4,
        "high" => 3,
        "medium" => 2,
        "low" => 1,
        _ => 0,
    }
}

pub(crate) fn confidence_rank(value: &str) -> u8 {
    match value {
        "high" => 3,
        "medium-high" => 2,
        "medium" => 1,
        "low" => 0,
        _ => 0,
    }
}

pub(crate) fn merge_duplicate_inline_evidence(
    kept: &mut ReviewInlineComment,
    dropped: &ReviewInlineComment,
) {
    if dropped.evidence.is_empty() || kept.evidence.contains(&dropped.evidence) {
        return;
    }
    let merged = format!(
        "{} Additional duplicate evidence from lane `{}`: {}",
        kept.evidence, dropped.lane, dropped.evidence
    );
    kept.evidence = truncate_chars(&merged, 2_000);
}

pub(crate) fn truncate_chars(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 3 {
        return value.chars().take(max_chars).collect();
    }
    let mut truncated = value.chars().take(max_chars - 3).collect::<String>();
    truncated.push_str("...");
    truncated
}

const GITHUB_SUGGESTION_MAX_CHARS: usize = 800;

pub(crate) fn normalize_github_suggestion_text(value: Option<&str>) -> Option<String> {
    let text = value?.trim();
    validate_github_suggestion_text(text).ok()?;
    Some(text.to_owned())
}

pub(crate) fn validate_github_suggestion_text(value: &str) -> Result<()> {
    let text = value.trim();
    if text.is_empty() {
        bail!("github review suggestion must not be empty");
    }
    if text.chars().count() > GITHUB_SUGGESTION_MAX_CHARS {
        bail!("github review suggestion must be {GITHUB_SUGGESTION_MAX_CHARS} chars or fewer");
    }
    if text.contains("```") {
        bail!("github review suggestion must not contain fenced code markers");
    }
    Ok(())
}

/// Maximum reviewer-facing characters a finding may occupy on a single source
/// line. A line comment is a senior reviewer's margin note, not an essay: the
/// accepted inline comments in `fixtures/review-experience/perl-lsp-3627.json`
/// run 75-125 reviewer-facing characters, so 400 leaves better than three
/// times the observed headroom for a two-sentence claim plus a named next
/// action, while still refusing a wall of model prose anchored to one line.
/// Longer content is not dropped: `validate_inline_candidate` demotes it to a
/// summary-only finding that keeps its text.
pub(crate) const INLINE_COMMENT_MAX_REVIEWER_CHARS: usize = 400;

/// Upper bound on the demoted summary text, matching the summary-only
/// guard's own concise limit in `validate_summary_only_candidate`.
pub(crate) const DEMOTED_SUMMARY_MAX_CHARS: usize = 1_200;

pub(crate) fn validate_inline_candidate(
    lane: &LanePlan,
    candidate: ModelCandidateComment,
    line_map: &BTreeSet<(String, u32)>,
) -> std::result::Result<ReviewInlineComment, SummaryOnlyFinding> {
    let path = normalize_repo_path(&candidate.path);
    let allowed_severity = matches!(candidate.severity.as_str(), "blocker" | "high" | "medium");
    let allowed_confidence = matches!(candidate.confidence.as_str(), "high" | "medium-high");
    let line_valid = line_map.contains(&(path.clone(), candidate.line));
    let body_text = candidate.body.trim();
    let evidence = candidate.evidence.trim().to_owned();
    let body = ensure_lane_prefix(&lane.id, body_text);
    // The wall applies to what GitHub would actually render on the line, not
    // to the artifact body: lane identity is stripped at the payload boundary
    // and must not consume the reviewer's budget.
    let reviewer_facing = reviewer_facing_pr_text(&body);
    let concise = reviewer_facing.chars().count() <= INLINE_COMMENT_MAX_REVIEWER_CHARS;
    let body_present = !reviewer_facing.is_empty();
    let evidence_present = !evidence.is_empty();
    let repo_relative = is_repo_relative_path(&path);
    let suggestion = if lane.id == "unsafe-review" {
        normalize_github_suggestion_text(candidate.suggestion.as_deref())
    } else {
        None
    };

    if allowed_severity
        && allowed_confidence
        && line_valid
        && concise
        && body_present
        && evidence_present
        && repo_relative
    {
        Ok(ReviewInlineComment {
            lane: lane.id.clone(),
            severity: candidate.severity,
            confidence: candidate.confidence,
            path,
            line: candidate.line,
            side: "RIGHT".to_owned(),
            body,
            evidence,
            suggestion,
        })
    } else if allowed_severity
        && allowed_confidence
        && line_valid
        && body_present
        && evidence_present
        && repo_relative
    {
        // Length is the only failure: the finding itself is admissible, it is
        // just too long to live on a source line. Demote it with its own
        // reviewer-facing text plus the anchor it lost, so the content reaches
        // the reviewer in the summary instead of becoming an artifact-only
        // guard receipt.
        Err(SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: candidate.severity,
            confidence: candidate.confidence,
            reason: format!(
                "{} ({}:{})",
                truncate_chars(&reviewer_facing, DEMOTED_SUMMARY_MAX_CHARS),
                path,
                candidate.line
            ),
            evidence,
        })
    } else {
        let diagnostic = format!(
            "inline guard rejected {}:{}; severity_allowed={} confidence_allowed={} line_valid={} concise={} body_present={} evidence_present={} repo_relative={}",
            path,
            candidate.line,
            allowed_severity,
            allowed_confidence,
            line_valid,
            concise,
            body_present,
            evidence_present,
            repo_relative
        );
        Err(SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: candidate.severity,
            confidence: candidate.confidence,
            reason: demoted_inline_finding_reason(
                &path,
                candidate.line,
                body_text,
                &evidence,
                &diagnostic,
            ),
            evidence: demoted_inline_finding_evidence(&evidence, &diagnostic),
        })
    }
}

/// Reviewer-facing text that survives when an inline candidate is demoted to a
/// summary-only finding.
///
/// Demotion must not destroy what the model actually found. The internal
/// diagnostic explaining *why* the candidate lost its inline slot is machine
/// text and stays artifact-side; the public text is the model's own comment
/// body, still anchored to `path:line` so a demoted finding remains line-level
/// and actionable. A candidate with no body carries no finding to preserve, so
/// the diagnostic is the only text left.
///
/// A candidate with no evidence of its own likewise keeps the diagnostic as
/// its reason, which leaves it artifact-only. Publishing an unsupported model
/// claim under "## Confirmed findings" would break the architecture rule that
/// missing evidence is recorded as missing evidence and never as clean
/// evidence — and `evidence_present=false` is one of the rejections that
/// reaches this path. Preserving the model's text is for findings that had
/// something behind them.
pub(crate) fn demoted_inline_finding_reason(
    path: &str,
    line: u32,
    body: &str,
    evidence: &str,
    diagnostic: &str,
) -> String {
    let body = strip_bracketed_lane_prefix(body).unwrap_or(body).trim();
    if body.is_empty() || evidence.trim().is_empty() {
        return diagnostic.to_owned();
    }
    // Every sibling constructor caps its reviewer-facing text. Uncapped, a
    // single oversized demoted finding pops every other topic out of the body
    // during degradation and the rest of the review is lost — and the
    // rejection that most often reaches this path is `concise=false`, i.e. a
    // body that was already too long.
    let body = truncate_chars(body, DEMOTED_INLINE_FINDING_MAX_CHARS);
    let path = path.trim();
    if path.is_empty() || body_already_anchored(&body, path, line) {
        return body;
    }
    format!("{path}:{line} — {body}")
}

/// Reviewer-facing cap for a demoted finding, matching the summary-candidate
/// cap in this module.
const DEMOTED_INLINE_FINDING_MAX_CHARS: usize = 1_200;

/// True when the body already states this exact anchor.
///
/// A plain `contains("{path}:{line}")` also matches a longer line number, so a
/// finding at line 41 whose body mentions `src/lib.rs:412` would silently lose
/// its own anchor. Require that the match is not followed by another digit.
fn body_already_anchored(body: &str, path: &str, line: u32) -> bool {
    let needle = format!("{path}:{line}");
    body.match_indices(&needle).any(|(index, _)| {
        !body[index + needle.len()..]
            .chars()
            .next()
            .is_some_and(|next| next.is_ascii_digit())
    })
}

/// Keep the demotion diagnostic next to the candidate's own evidence. Only
/// `reason` reaches the PR body, so the diagnostic stays artifact-side here.
pub(crate) fn demoted_inline_finding_evidence(evidence: &str, diagnostic: &str) -> String {
    let evidence = evidence.trim();
    if evidence.is_empty() {
        return diagnostic.to_owned();
    }
    format!("{evidence} [demotion diagnostic: {diagnostic}]")
}

#[cfg(test)]
mod claim_identity_tests {
    use super::*;

    fn comment(evidence: &str) -> ReviewInlineComment {
        ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium".to_owned(),
            path: "src/parser.rs".to_owned(),
            line: 10,
            side: "RIGHT".to_owned(),
            body: "the declaration list claim".to_owned(),
            evidence: evidence.to_owned(),
            suggestion: None,
        }
    }

    #[test]
    fn structural_identity_requires_family_and_code_token() {
        assert_eq!(
            inline_claim_family("assert the `.toThrow()` oracle"),
            Some("test-oracle")
        );
        assert_eq!(
            inline_claim_family("postfix subscript drops `$x[0]`"),
            Some("indexing")
        );
        assert_eq!(
            inline_claim_family("fallback error result is lost"),
            Some("error-path")
        );
        assert_eq!(inline_claim_family("unsafe safety alias"), Some("safety"));
        assert_eq!(inline_claim_family("workflow action pin"), Some("workflow"));
        assert_eq!(inline_claim_family("unclassified prose"), None);

        let tokens = inline_claim_code_tokens("use `$x[0]` and `%h`");
        assert!(tokens.contains("$x"));
        assert!(tokens.contains("%h"));
    }

    #[test]
    fn evidence_rank_is_receipt_first_then_source_thread_and_model() {
        assert_eq!(
            inline_evidence_rank(&comment("executed focused test receipt")),
            5
        );
        assert_eq!(inline_evidence_rank(&comment("source diff")), 3);
        assert_eq!(inline_evidence_rank(&comment("existing thread")), 2);
        assert_eq!(inline_evidence_rank(&comment("model observation")), 1);
    }

    #[test]
    fn same_inline_claim_discriminates_exact_family_and_code_identity() {
        let make = |body: &str| ReviewInlineComment {
            body: body.to_owned(),
            ..comment("source diff")
        };
        let exact_left = make(
            "The `.toThrow()` assertion does not discriminate the thrown error; assert type or message.",
        );
        let exact_right =
            make("The bare `.toThrow()` check is non-discriminating; assert type or message.");
        assert!(same_inline_claim(&exact_left, &exact_right));

        let disjoint_code = make(
            "The `.toBe()` assertion does not discriminate the returned value; assert type or message.",
        );
        assert!(!same_inline_claim(&exact_left, &disjoint_code));

        let unclassified = make("The declaration list needs a clearer explanation here.");
        assert!(!same_inline_claim(&exact_left, &unclassified));
        assert!(inline_claim_code_tokens("plain prose").is_empty());
    }
}

#[cfg(test)]
mod inline_demotion_tests {
    use std::collections::BTreeSet;

    use anyhow::{Result, anyhow};

    use crate::tests::{test_diff, test_plan};
    use crate::*;

    const MODEL_FINDING: &str = "The retry loop reuses the scratch buffer after the async write completes, so a second write can observe freed memory.";

    /// A body mentioning a longer line number at the same path must not be
    /// mistaken for the finding's own anchor. `contains("src/lib.rs:41")`
    /// matches inside `src/lib.rs:412`, which silently dropped the anchor.
    #[test]
    fn demoted_anchor_is_not_satisfied_by_a_longer_line_number() {
        let body = "The caller at src/lib.rs:412 already revalidated the pointer.";
        let reason = demoted_inline_finding_reason("src/lib.rs", 41, body, "ev", "diag");
        assert!(
            reason.starts_with("src/lib.rs:41 — "),
            "line 41 must keep its own anchor: {reason}"
        );

        // The exact anchor is still recognized and not duplicated.
        let exact = "src/lib.rs:41 is where the borrow escapes.";
        let reason = demoted_inline_finding_reason("src/lib.rs", 41, exact, "ev", "diag");
        assert_eq!(reason, exact);
    }

    /// A candidate with no evidence stays artifact-only, and an over-long body
    /// cannot pop every other topic out of the degraded body.
    #[test]
    fn demoted_reason_withholds_unsupported_claims_and_caps_length() {
        let reason = demoted_inline_finding_reason("src/lib.rs", 41, MODEL_FINDING, "  ", "diag");
        assert_eq!(reason, "diag");

        let long = "x".repeat(5_000);
        let reason = demoted_inline_finding_reason("src/lib.rs", 41, &long, "ev", "diag");
        assert!(
            reason.chars().count() <= 1_200 + "src/lib.rs:41 — ".chars().count(),
            "demoted reason must be capped, got {} chars",
            reason.chars().count()
        );
    }

    fn demotion_lane() -> LanePlan {
        LanePlan {
            id: "ub".to_owned(),
            role: "UB review".to_owned(),
            model: "custom:MiniMax-M3-3".to_owned(),
            model_display: "MiniMax-M3".to_owned(),
            receives: vec!["ripr".to_owned()],
            focus: "Check memory validity.".to_owned(),
        }
    }

    fn unanchored_candidate() -> ModelCandidateComment {
        ModelCandidateComment {
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 412,
            body: MODEL_FINDING.to_owned(),
            evidence: "src/lib.rs hunk around the retry loop".to_owned(),
            suggestion: None,
        }
    }

    #[test]
    fn anchoring_failure_keeps_the_model_finding_out_of_the_diagnostic() -> Result<()> {
        // Empty line map: the claimed anchor is not a changed RIGHT-side line.
        let line_map = BTreeSet::new();
        let finding =
            validate_inline_candidate(&demotion_lane(), unanchored_candidate(), &line_map)
                .err()
                .ok_or_else(|| anyhow!("an unanchored candidate must be demoted"))?;

        assert!(
            finding.reason.contains(MODEL_FINDING),
            "demotion dropped the model finding: {}",
            finding.reason
        );
        assert!(
            finding.reason.contains("src/lib.rs:412"),
            "demotion dropped the line anchor: {}",
            finding.reason
        );
        assert!(!finding.reason.contains("inline guard rejected"));
        assert!(!finding.reason.contains("line_valid="));
        assert!(
            finding.evidence.contains("line_valid=false"),
            "the guard diagnostic must stay artifact-side: {}",
            finding.evidence
        );
        Ok(())
    }

    #[test]
    fn demoted_anchoring_failure_survives_into_the_pull_request_body() -> Result<()> {
        let line_map = BTreeSet::new();
        let finding =
            validate_inline_candidate(&demotion_lane(), unanchored_candidate(), &line_map)
                .err()
                .ok_or_else(|| anyhow!("an unanchored candidate must be demoted"))?;

        let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[finding],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

        assert!(
            body.contains("reuses the scratch buffer after the async write"),
            "the model finding never reached the PR body: {body}"
        );
        assert!(body.contains("src/lib.rs:412"), "{body}");
        assert!(!body.contains("inline guard rejected"), "{body}");
        assert!(!body.contains("line_valid="), "{body}");
        assert!(!body.contains("severity_allowed"), "{body}");
        assert!(!body.contains("hunk around the retry loop"), "{body}");
        Ok(())
    }

    #[test]
    fn refuter_demotion_keeps_the_model_finding_and_parks_the_diagnostic() {
        let comment = ReviewInlineComment {
            lane: "ub".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 412,
            side: "RIGHT".to_owned(),
            body: format!("[ub] {MODEL_FINDING}"),
            evidence: "src/lib.rs hunk around the retry loop".to_owned(),
            suggestion: None,
        };

        let finding = summary_from_refuted_inline(comment, "confidence is not high enough");

        assert!(finding.reason.contains(MODEL_FINDING), "{}", finding.reason);
        assert!(
            finding.reason.contains("src/lib.rs:412"),
            "{}",
            finding.reason
        );
        assert!(!finding.reason.contains("refuter"), "{}", finding.reason);
        assert!(
            finding.evidence.contains("confidence is not high enough"),
            "{}",
            finding.evidence
        );
    }
}
