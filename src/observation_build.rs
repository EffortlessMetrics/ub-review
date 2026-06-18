//! Observation construction and cross-lane conflict detection (cleanup
//! train step 25, pure code motion). Builds observations from review
//! artifacts, detects cross-lane refutation conflicts, and dedupes.

use crate::*;

pub(crate) fn combined_observations(review: &ReviewArtifacts) -> Vec<Observation> {
    let mut observations = review.observations.clone();
    observations.extend(build_observations(review));
    for (index, observation) in observations.iter_mut().enumerate() {
        let short = observation
            .fingerprint
            .get(..12)
            .unwrap_or(&observation.fingerprint);
        observation.id = format!("obs-{index:04}-{short}");
    }
    observations
}

pub(crate) fn build_observations(review: &ReviewArtifacts) -> Vec<Observation> {
    let mut observations = Vec::new();
    for comment in &review.inline_comments {
        observations.push(make_observation(ObservationInput {
            index: observations.len(),
            lane: &comment.lane,
            question: &comment.lane,
            claim: &comment.body,
            kind: infer_observation_kind(&comment.lane, &comment.body, &comment.evidence),
            status: "confirmed",
            severity: &comment.severity,
            confidence: &comment.confidence,
            path: Some(&comment.path),
            line: Some(comment.line),
            evidence: vec![comment.evidence.clone()],
            dedupe_key: None,
            source: "inline-comment",
        }));
    }
    for finding in &review.summary_only_findings {
        let parked = is_parked_follow_up(finding);
        let kind = if parked {
            "parked-follow-up"
        } else {
            infer_observation_kind(&finding.lane, &finding.reason, &finding.evidence)
        };
        let status = if parked { "parked" } else { "open" };
        observations.push(make_observation(ObservationInput {
            index: observations.len(),
            lane: &finding.lane,
            question: &finding.lane,
            claim: &finding.reason,
            kind,
            status,
            severity: &finding.severity,
            confidence: &finding.confidence,
            path: None,
            line: None,
            evidence: vec![finding.evidence.clone()],
            dedupe_key: None,
            source: "summary-only-finding",
        }));
    }
    for issue in &review.missing_or_failed_sensor_evidence {
        let claim = format!(
            "Sensor `{}` evidence is `{}`: {}",
            issue.sensor, issue.status, issue.reason
        );
        observations.push(make_observation(ObservationInput {
            index: observations.len(),
            lane: &format!("sensor-{}", issue.sensor),
            question: "missing-sensor-evidence",
            claim: &claim,
            kind: "missing-evidence",
            status: "open",
            severity: "medium",
            confidence: "high",
            path: None,
            line: None,
            evidence: vec![issue.reason.clone()],
            dedupe_key: None,
            source: "missing-sensor-evidence",
        }));
    }
    for issue in &review.missing_or_failed_model_evidence {
        let claim = format!(
            "Lane `{}` via `{}` model `{}` endpoint `{}` is `{}`: {}",
            issue.lane,
            issue.provider,
            issue.model,
            issue.endpoint_kind,
            issue.status,
            issue.reason
        );
        observations.push(make_observation(ObservationInput {
            index: observations.len(),
            lane: &issue.lane,
            question: "missing-model-evidence",
            claim: &claim,
            kind: "missing-evidence",
            status: "open",
            severity: "medium",
            confidence: "high",
            path: None,
            line: None,
            evidence: vec![issue.reason.clone()],
            dedupe_key: None,
            source: "missing-model-evidence",
        }));
    }
    observations
}

pub(crate) const CROSS_LANE_CONFLICT_SOURCE: &str = "cross-lane-conflict-detector";
const MAX_CROSS_LANE_CONFLICT_OBSERVATIONS: usize = 3;

#[derive(Clone, Debug)]
pub(crate) struct ConflictSurface {
    pub(crate) lane: String,
    pub(crate) claim: String,
    pub(crate) evidence: String,
    pub(crate) severity: String,
    pub(crate) confidence: String,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<u32>,
    pub(crate) surface_key: String,
}

pub(crate) fn append_cross_lane_conflict_observations(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
    observations: &mut Vec<Observation>,
) {
    let surfaces = conflict_surfaces(inline_comments, summary_only_findings);
    if surfaces.is_empty() {
        return;
    }

    let mut seen = observations
        .iter()
        .filter(|observation| observation.source == CROSS_LANE_CONFLICT_SOURCE)
        .map(|observation| observation.dedupe_key.clone())
        .collect::<BTreeSet<_>>();
    let refutations = observations
        .iter()
        .filter(|observation| observation_is_cross_lane_refutation(observation))
        .cloned()
        .collect::<Vec<_>>();
    let mut added = 0usize;

    for surface in surfaces {
        if severity_rank(&surface.severity) < severity_rank("medium") {
            continue;
        }
        for refutation in &refutations {
            if surface.lane == refutation.lane {
                continue;
            }
            if !surface_conflicts_with_refutation(&surface, refutation) {
                continue;
            }
            let dedupe_key = cross_lane_conflict_dedupe_key(&surface, refutation);
            if !seen.insert(dedupe_key.clone()) {
                continue;
            }
            observations.push(cross_lane_conflict_observation(
                observations.len(),
                &surface,
                refutation,
                &dedupe_key,
            ));
            added += 1;
            if added >= MAX_CROSS_LANE_CONFLICT_OBSERVATIONS {
                return;
            }
            break;
        }
    }
}

pub(crate) fn conflict_surfaces(
    inline_comments: &[ReviewInlineComment],
    summary_only_findings: &[SummaryOnlyFinding],
) -> Vec<ConflictSurface> {
    let mut surfaces = inline_comments
        .iter()
        .map(|comment| ConflictSurface {
            lane: comment.lane.clone(),
            claim: comment.body.clone(),
            evidence: comment.evidence.clone(),
            severity: comment.severity.clone(),
            confidence: comment.confidence.clone(),
            path: Some(comment.path.clone()),
            line: Some(comment.line),
            surface_key: inline_comment_conflict_key(comment),
        })
        .collect::<Vec<_>>();
    surfaces.extend(summary_only_findings.iter().map(|finding| ConflictSurface {
        lane: finding.lane.clone(),
        claim: finding.reason.clone(),
        evidence: finding.evidence.clone(),
        severity: finding.severity.clone(),
        confidence: finding.confidence.clone(),
        path: None,
        line: None,
        surface_key: summary_finding_conflict_key(finding),
    }));
    surfaces
}

pub(crate) fn inline_comment_conflict_key(comment: &ReviewInlineComment) -> String {
    sha256_hex(
        format!(
            "inline\n{}\n{}\n{}\n{}\n{}",
            comment.lane, comment.path, comment.line, comment.body, comment.evidence
        )
        .as_bytes(),
    )
}

pub(crate) fn summary_finding_conflict_key(finding: &SummaryOnlyFinding) -> String {
    sha256_hex(
        format!(
            "summary\n{}\n{}\n{}",
            finding.lane, finding.reason, finding.evidence
        )
        .as_bytes(),
    )
}

pub(crate) fn observation_is_cross_lane_refutation(observation: &Observation) -> bool {
    observation.status == "refuted"
        || matches!(
            observation.kind.as_str(),
            "false-premise" | "resolved-check"
        )
}

pub(crate) fn surface_conflicts_with_refutation(
    surface: &ConflictSurface,
    refutation: &Observation,
) -> bool {
    let surface_text = normalized_review_text(&format!("{} {}", surface.claim, surface.evidence));
    let refutation_text = normalized_review_text(&format!(
        "{} {}",
        refutation.claim,
        refutation.evidence.join(" ")
    ));
    if surface_text.chars().count() < 24 || refutation_text.chars().count() < 24 {
        return false;
    }
    if refutation_text.contains(&surface_text) || surface_text.contains(&refutation_text) {
        return true;
    }

    let surface_tokens = conflict_tokens(&surface_text);
    let refutation_tokens = conflict_tokens(&refutation_text);
    if surface_tokens.len() < 4 || refutation_tokens.len() < 4 {
        return false;
    }
    let shared = surface_tokens.intersection(&refutation_tokens).count();
    shared >= 4 && shared * 4 >= surface_tokens.len().min(refutation_tokens.len())
}

pub(crate) fn conflict_tokens(text: &str) -> BTreeSet<String> {
    const STOP_WORDS: &[&str] = &[
        "about",
        "after",
        "against",
        "because",
        "before",
        "candidate",
        "claim",
        "could",
        "diff",
        "evidence",
        "finding",
        "lane",
        "model",
        "refuted",
        "review",
        "should",
        "that",
        "this",
        "with",
        "would",
    ];
    text.split_whitespace()
        .filter(|token| token.len() >= 4 && !STOP_WORDS.contains(token))
        .map(ToOwned::to_owned)
        .collect()
}

pub(crate) fn cross_lane_conflict_dedupe_key(
    surface: &ConflictSurface,
    refutation: &Observation,
) -> String {
    let hash = sha256_hex(format!("{}\n{}", surface.surface_key, refutation.dedupe_key).as_bytes());
    format!("cross-lane-conflict:{}", &hash[..12])
}

pub(crate) fn cross_lane_conflict_observation(
    index: usize,
    surface: &ConflictSurface,
    refutation: &Observation,
    dedupe_key: &str,
) -> Observation {
    let claim = format!(
        "Resolve cross-lane conflict before treating `{}` as confirmed: `{}` reports `{}`, while `{}` refutes the same concern.",
        surface.lane,
        surface.lane,
        truncate_chars(&surface.claim, 220),
        refutation.lane
    );
    let evidence = vec![
        format!(
            "finding_key={}; finding_evidence={}",
            surface.surface_key, surface.evidence
        ),
        format!(
            "refutation_observation={}; refutation_kind={}; refutation_status={}; refutation_evidence={}",
            refutation.id,
            refutation.kind,
            refutation.status,
            refutation.evidence.join(" | ")
        ),
    ];
    make_observation(ObservationInput {
        index,
        lane: "orchestrator-conflict",
        question: "cross-lane-conflict",
        claim: &claim,
        kind: "verification-question",
        status: "open",
        severity: "medium",
        confidence: conflict_confidence(&surface.confidence, &refutation.confidence),
        path: surface.path.as_ref(),
        line: surface.line,
        evidence,
        dedupe_key: Some(dedupe_key),
        source: CROSS_LANE_CONFLICT_SOURCE,
    })
}

pub(crate) fn conflict_confidence(
    surface_confidence: &str,
    refutation_confidence: &str,
) -> &'static str {
    if confidence_rank(surface_confidence).max(confidence_rank(refutation_confidence))
        >= confidence_rank("medium-high")
    {
        "medium-high"
    } else {
        "medium"
    }
}

pub(crate) struct ObservationInput<'a> {
    pub(crate) index: usize,
    pub(crate) lane: &'a str,
    pub(crate) question: &'a str,
    pub(crate) claim: &'a str,
    pub(crate) kind: &'a str,
    pub(crate) status: &'a str,
    pub(crate) severity: &'a str,
    pub(crate) confidence: &'a str,
    pub(crate) path: Option<&'a String>,
    pub(crate) line: Option<u32>,
    pub(crate) evidence: Vec<String>,
    pub(crate) dedupe_key: Option<&'a str>,
    pub(crate) source: &'a str,
}

pub(crate) fn make_observation(input: ObservationInput<'_>) -> Observation {
    let path = input.path.cloned();
    let line = input.line.filter(|line| *line > 0);
    let fingerprint_input = format!(
        "{}\n{}\n{}\n{}\n{:?}\n{:?}\n{}",
        input.lane,
        input.kind,
        input.status,
        input.claim,
        path,
        line,
        input.evidence.join("\n")
    );
    let fingerprint = sha256_hex(fingerprint_input.as_bytes());
    let short = &fingerprint[..12];
    Observation {
        schema: OBSERVATION_SCHEMA.to_owned(),
        id: format!("obs-{index:04}-{short}", index = input.index),
        lane: input.lane.to_owned(),
        question: input.question.to_owned(),
        claim: input.claim.to_owned(),
        kind: input.kind.to_owned(),
        status: input.status.to_owned(),
        severity: input.severity.to_owned(),
        confidence: input.confidence.to_owned(),
        path: path.clone(),
        line,
        fingerprint,
        evidence: input.evidence,
        dedupe_key: input
            .dedupe_key
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| {
                observation_dedupe_key(input.lane, input.kind, path.as_deref(), line)
            }),
        source: input.source.to_owned(),
    }
}

pub(crate) fn observation_dedupe_key(
    lane: &str,
    kind: &str,
    path: Option<&str>,
    line: Option<u32>,
) -> String {
    match (path, line) {
        (Some(path), Some(line)) => format!("{kind}:{path}:{line}"),
        _ => format!("{kind}:{}", sanitize_artifact_name(lane)),
    }
}

pub(crate) fn infer_observation_kind(lane: &str, claim: &str, evidence: &str) -> &'static str {
    let lane = lane.to_ascii_lowercase();
    let text = format!("{claim}\n{evidence}").to_ascii_lowercase();
    if text.contains("missing") || text.contains("unavailable") || text.contains("skipped") {
        "missing-evidence"
    } else if text.contains("parked") || text.contains("follow-up") {
        "parked-follow-up"
    } else if lane.contains("test") || text.contains("test") || text.contains("oracle") {
        "test-gap"
    } else if lane.contains("source-route") || lane.contains("sibling") || text.contains("route") {
        "source-route-gap"
    } else if lane.contains("security") || text.contains("exploit") || text.contains("secret") {
        "security-risk"
    } else if text.contains("verify") || text.contains("confirm") || text.contains("question") {
        "verification-question"
    } else {
        "bug"
    }
}
