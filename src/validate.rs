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
        .filter(|kind| allowed_observation_kind(kind))
        .unwrap_or_else(|| infer_observation_kind(&lane.id, &claim, &evidence.join("\n")));
    let status = candidate
        .status
        .as_deref()
        .map(str::trim)
        .filter(|status| allowed_observation_status(status))
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

pub(crate) fn inline_comment_rank(comment: &ReviewInlineComment) -> (u8, u8) {
    (
        severity_rank(&comment.severity),
        confidence_rank(&comment.confidence),
    )
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
    let concise = body.chars().count() <= 1_200;
    let body_present = !body_text.is_empty();
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
    } else {
        Err(SummaryOnlyFinding {
            lane: lane.id.clone(),
            severity: candidate.severity,
            confidence: candidate.confidence,
            reason: format!(
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
            ),
            evidence,
        })
    }
}
