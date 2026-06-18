//! Issue broker: candidate fingerprinting, evaluation, classification,
//! broker plan building, and resolution tracking (cleanup train step 27,
//! pure code motion).

use crate::*;

pub(crate) const ISSUE_CANDIDATE_KINDS: &[&str] = &[
    "implementation-follow-up",
    "tool-defect",
    "repo-policy",
    "test-gap",
    "proof-broker-gap",
];

pub(crate) fn issue_candidate_fingerprint(candidate: &IssueCandidate) -> String {
    let normalized_title = candidate
        .title
        .to_ascii_lowercase()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { ' ' })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    sha256_hex(
        format!(
            "{}\n{}\n{normalized_title}",
            candidate.target_repo, candidate.kind
        )
        .as_bytes(),
    )
}

/// The bar a follow-up must meet to exist at all: specific repo, specific
/// problem, why it is not in this PR, evidence, an implementation plan, and
/// acceptance criteria. Below the bar is `invalid`, never silently dropped.
pub(crate) fn issue_candidate_invalid_reason(candidate: &IssueCandidate) -> Option<String> {
    if candidate.target_repo.trim().is_empty() {
        return Some("target_repo is required".to_owned());
    }
    if candidate.title.trim().is_empty() {
        return Some("title is required".to_owned());
    }
    if candidate.problem.trim().is_empty() {
        return Some("problem is required".to_owned());
    }
    if candidate.why_not_this_pr.trim().is_empty() {
        return Some("why_not_this_pr is required".to_owned());
    }
    if !ISSUE_CANDIDATE_KINDS.contains(&candidate.kind.as_str()) {
        return Some(format!(
            "kind `{}` is not in the allowed set {ISSUE_CANDIDATE_KINDS:?}",
            candidate.kind
        ));
    }
    if !matches!(candidate.confidence.as_str(), "low" | "medium" | "high") {
        return Some(format!(
            "confidence `{}` must be low, medium, or high",
            candidate.confidence
        ));
    }
    if !matches!(
        candidate.current_pr_disposition.as_str(),
        "do-not-block" | "blocks-if-claimed"
    ) {
        return Some(format!(
            "current_pr_disposition `{}` must be do-not-block or blocks-if-claimed",
            candidate.current_pr_disposition
        ));
    }
    if !candidate.evidence.iter().any(|evidence| {
        evidence
            .path
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
            || evidence
                .url
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
    }) {
        return Some("at least one evidence entry with a path or url is required".to_owned());
    }
    if candidate
        .implementation_plan
        .iter()
        .all(|step| step.trim().is_empty())
    {
        return Some("a non-empty implementation_plan is required".to_owned());
    }
    if candidate
        .acceptance
        .iter()
        .all(|item| item.trim().is_empty())
    {
        return Some("non-empty acceptance criteria are required".to_owned());
    }
    None
}

/// True when the issue posture lets a valid candidate render for the
/// reviewer: enabled, mode suggest or open-high-confidence, high confidence,
/// and a do-not-block disposition. Suggested follow-ups never block; under
/// open-high-confidence the broker may additionally attempt opening at post
/// time, but the run-side action stays `suggested` either way.
pub(crate) fn issue_candidate_renders(issues: &IssuesConfig, candidate: &IssueCandidate) -> bool {
    issues.enabled
        && matches!(issues.mode.as_str(), "suggest" | "open-high-confidence")
        && candidate.confidence == "high"
        && candidate.current_pr_disposition == "do-not-block"
}

pub(crate) fn classify_issue_candidates(
    issues: &IssuesConfig,
    raw: Vec<IssueCandidate>,
) -> (Vec<IssueCandidate>, Vec<IssueAction>) {
    let mut candidates = Vec::new();
    let mut actions = Vec::new();
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for (index, mut candidate) in raw.into_iter().enumerate() {
        candidate.schema = ISSUE_CANDIDATE_SCHEMA.to_owned();
        let fingerprint = issue_candidate_fingerprint(&candidate);
        candidate.id = format!("issue-candidate-{index:03}-{}", &fingerprint[..12]);
        let action = if let Some(reason) = issue_candidate_invalid_reason(&candidate) {
            IssueAction {
                schema: ISSUE_ACTION_SCHEMA.to_owned(),
                candidate_id: candidate.id.clone(),
                action: "invalid".to_owned(),
                reason,
                existing: None,
            }
        } else if let Some(existing) = seen.get(&fingerprint) {
            IssueAction {
                schema: ISSUE_ACTION_SCHEMA.to_owned(),
                candidate_id: candidate.id.clone(),
                action: "duplicate".to_owned(),
                reason: "same target repo, kind, and normalized title as an earlier candidate"
                    .to_owned(),
                existing: Some(existing.clone()),
            }
        } else {
            seen.insert(fingerprint, candidate.id.clone());
            if issue_candidate_renders(issues, &candidate) {
                IssueAction {
                    schema: ISSUE_ACTION_SCHEMA.to_owned(),
                    candidate_id: candidate.id.clone(),
                    action: "suggested".to_owned(),
                    reason: if issues.mode == "open-high-confidence" {
                        "high-confidence follow-up rendered for the reviewer; the \
                         issue broker attempts opening at post time for allowlisted \
                         target repos"
                            .to_owned()
                    } else {
                        "high-confidence follow-up rendered for the reviewer; \
                         suggested follow-ups never block"
                            .to_owned()
                    },
                    existing: None,
                }
            } else {
                IssueAction {
                    schema: ISSUE_ACTION_SCHEMA.to_owned(),
                    candidate_id: candidate.id.clone(),
                    action: "artifact-only".to_owned(),
                    reason: "below the rendering bar or rendering disabled; preserved \
                             in artifacts with no GitHub side effects"
                        .to_owned(),
                    existing: None,
                }
            }
        };
        actions.push(action);
        candidates.push(candidate);
    }
    (candidates, actions)
}

pub(crate) fn write_issue_capture_artifacts(
    out: &Path,
    candidates: &[IssueCandidate],
    actions: &[IssueAction],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("issue_candidates.json"),
        serde_json::to_vec_pretty(candidates)?,
    )?;
    fs::write(
        review_dir.join("issue_actions.json"),
        serde_json::to_vec_pretty(actions)?,
    )?;
    let mut candidate_lines = String::new();
    for candidate in candidates {
        candidate_lines.push_str(&serde_json::to_string(candidate)?);
        candidate_lines.push('\n');
    }
    fs::write(out.join("issue_candidates.ndjson"), candidate_lines)?;
    let mut action_lines = String::new();
    for action in actions {
        action_lines.push_str(&serde_json::to_string(action)?);
        action_lines.push('\n');
    }
    fs::write(out.join("issue_actions.ndjson"), action_lines)?;

    let mut drafts = String::from(
        "# Suggested issues\n\nHuman-readable drafts for valid issue candidates. v0 is \
         artifact-only: nothing here was posted, suggested in the PR body, or opened on \
         GitHub - those are later release-lane steps.\n",
    );
    let valid_ids = actions
        .iter()
        .filter(|action| matches!(action.action.as_str(), "artifact-only" | "suggested"))
        .map(|action| action.candidate_id.as_str())
        .collect::<BTreeSet<_>>();
    for candidate in candidates {
        if !valid_ids.contains(candidate.id.as_str()) {
            continue;
        }
        drafts.push_str(&format!(
            "\n## {} ({})\n\nProblem: {}\n\nSource lane: {}\n",
            candidate.title, candidate.target_repo, candidate.problem, candidate.source
        ));
        for evidence in &candidate.evidence {
            if let Some(path) = &evidence.path {
                drafts.push_str(&format!("- {}: {path}\n", evidence.kind));
            }
            if let Some(url) = &evidence.url {
                drafts.push_str(&format!("- {}: {url}\n", evidence.kind));
            }
        }
        drafts.push_str(&format!(
            "\nWhy not this PR: {}\n\nImplementation plan:\n",
            candidate.why_not_this_pr
        ));
        for (index, step) in candidate.implementation_plan.iter().enumerate() {
            drafts.push_str(&format!("{}. {step}\n", index + 1));
        }
        drafts.push_str("\nAcceptance:\n");
        for item in &candidate.acceptance {
            drafts.push_str(&format!("- [ ] {item}\n"));
        }
    }
    fs::write(review_dir.join("suggested_issues.md"), drafts)?;
    Ok(())
}

/// The body marker the broker's remote duplicate search keys on. Embedded in
/// every broker-opened issue; searched verbatim before any open attempt.
pub(crate) fn issue_broker_fingerprint_marker(fingerprint: &str) -> String {
    format!("ub-review-fingerprint: {fingerprint}")
}

/// Render the full GitHub issue body for a broker attempt at `run` time so
/// `post` performs zero formatting. Carries the candidate's problem,
/// why-not-this-PR, evidence, plan, acceptance, a provenance line, and the
/// fingerprint marker.
pub(crate) fn issue_broker_body(
    candidate: &IssueCandidate,
    fingerprint: &str,
    source_repo: Option<&str>,
    pull_number: Option<u64>,
) -> String {
    let mut body = format!(
        "{}\n\nWhy not the source PR: {}\n",
        candidate.problem, candidate.why_not_this_pr
    );
    if !candidate.evidence.is_empty() {
        body.push_str("\n## Evidence\n\n");
        for evidence in &candidate.evidence {
            if let Some(path) = &evidence.path {
                body.push_str(&format!("- {}: {path}\n", evidence.kind));
            }
            if let Some(url) = &evidence.url {
                body.push_str(&format!("- {}: {url}\n", evidence.kind));
            }
        }
    }
    body.push_str("\n## Implementation plan\n\n");
    for (index, step) in candidate.implementation_plan.iter().enumerate() {
        body.push_str(&format!("{}. {step}\n", index + 1));
    }
    body.push_str("\n## Acceptance\n\n");
    for item in &candidate.acceptance {
        body.push_str(&format!("- [ ] {item}\n"));
    }
    let provenance = match (source_repo, pull_number) {
        (Some(repo), Some(number)) => {
            format!(
                "opened by ub-review from {repo}#{number}, lane {}",
                candidate.source
            )
        }
        _ => format!("opened by ub-review, lane {}", candidate.source),
    };
    body.push_str(&format!(
        "\n---\n{provenance}\n{}\n",
        issue_broker_fingerprint_marker(fingerprint)
    ));
    body
}

/// Decide at `run` time what the post-time broker may do. Pure: only
/// `suggested` candidates are considered, in ledger order; each becomes an
/// `attempt` (valid allowlisted slug, under `open_cap`) or a `skip` with the
/// reason recorded. Returns an empty plan unless mode is
/// open-high-confidence. No silent caps: cap-excluded candidates appear as
/// skips.
pub(crate) fn build_issue_broker_plan(
    issues: &IssuesConfig,
    candidates: &[IssueCandidate],
    actions: &[IssueAction],
    source_repo: Option<&str>,
    pull_number: Option<u64>,
) -> Vec<IssueBrokerPlanEntry> {
    if !issues.enabled || issues.mode != "open-high-confidence" {
        return Vec::new();
    }
    let by_id: BTreeMap<&str, &IssueCandidate> = candidates
        .iter()
        .map(|candidate| (candidate.id.as_str(), candidate))
        .collect();
    let mut plan = Vec::new();
    let mut attempts = 0u32;
    for action in actions {
        if action.action != "suggested" {
            continue;
        }
        let Some(candidate) = by_id.get(action.candidate_id.as_str()) else {
            continue;
        };
        let fingerprint = issue_candidate_fingerprint(candidate);
        let (decision, reason) = if !is_valid_repo_slug(&candidate.target_repo) {
            (
                "skip",
                "target repo is not a valid owner/repo slug".to_owned(),
            )
        } else if !issues
            .open_in
            .iter()
            .any(|allowed| allowed == &candidate.target_repo)
        {
            (
                "skip",
                "target repo is not in the issues.open_in allowlist; the candidate \
                 stays suggested"
                    .to_owned(),
            )
        } else if attempts >= issues.open_cap {
            (
                "skip",
                format!(
                    "issues.open_cap={} reached for this post; the candidate stays \
                     suggested",
                    issues.open_cap
                ),
            )
        } else {
            attempts += 1;
            (
                "attempt",
                "high-confidence do-not-block candidate targeting an allowlisted \
                 repo; post runs a fingerprint duplicate search before opening"
                    .to_owned(),
            )
        };
        plan.push(IssueBrokerPlanEntry {
            schema: ISSUE_BROKER_PLAN_SCHEMA.to_owned(),
            candidate_id: candidate.id.clone(),
            fingerprint: fingerprint.clone(),
            target_repo: candidate.target_repo.clone(),
            decision: decision.to_owned(),
            reason,
            title: candidate.title.clone(),
            body: issue_broker_body(candidate, &fingerprint, source_repo, pull_number),
            labels: candidate.labels.clone(),
        });
    }
    plan
}

/// Persist the broker plan (review/issue_broker_plan.json + NDJSON twin).
/// Written only when the broker is opted in; an absent file means mode never
/// reached open-high-confidence, an empty list means it did and there was
/// nothing suggested.
pub(crate) fn write_issue_broker_plan(out: &Path, plan: &[IssueBrokerPlanEntry]) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("issue_broker_plan.json"),
        serde_json::to_vec_pretty(plan)?,
    )?;
    let mut lines = String::new();
    for entry in plan {
        lines.push_str(&serde_json::to_string(entry)?);
        lines.push('\n');
    }
    fs::write(out.join("issue_broker_plan.ndjson"), lines)?;
    Ok(())
}

pub(crate) fn load_prior_resolved_candidates(
    root: &Path,
    out: &Path,
    args: &RunArgs,
) -> Result<Vec<ResolvedCandidateRecord>> {
    let configured = args.prior_resolved_candidates.trim();
    let records = if configured.is_empty() {
        Vec::new()
    } else {
        let configured_path = PathBuf::from(configured);
        let path = if configured_path.is_absolute() {
            configured_path
        } else {
            root.join(configured_path)
        };
        serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )
        .with_context(|| format!("parse {}", path.display()))?
    };
    write_prior_resolved_candidate_artifact(out, &records)?;
    Ok(records)
}

pub(crate) fn write_prior_resolved_candidate_artifact(
    out: &Path,
    records: &[ResolvedCandidateRecord],
) -> Result<()> {
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    fs::write(
        review_dir.join("prior_resolved_candidates.json"),
        serde_json::to_vec_pretty(records)?,
    )?;
    Ok(())
}

pub(crate) fn resolved_candidate_records(
    candidates: &[CandidateRecord],
    follow_up_results: &[FollowUpResult],
    follow_up_outputs: &[FollowUpOutputRecord],
    prior_resolved_candidates: &[ResolvedCandidateRecord],
) -> Vec<ResolvedCandidateRecord> {
    let follow_up_result_task_ids = follow_up_results
        .iter()
        .map(|result| result.task_id.clone())
        .collect::<BTreeSet<_>>();
    candidates
        .iter()
        .map(|candidate| {
            let linked_outputs = follow_up_outputs
                .iter()
                .filter(|output| {
                    follow_up_result_task_ids.contains(&output.task_id)
                        && output.candidate_ids.iter().any(|id| id == &candidate.id)
                })
                .collect::<Vec<_>>();
            resolved_candidate_record(candidate, &linked_outputs, prior_resolved_candidates)
        })
        .collect()
}

/// Candidate ids whose follow-up outputs resolved them to `refuted` or
/// `dropped`. The final compiler excludes these candidates' original review
/// surfaces: the late receipt turn exists to change dispositions (confirm,
/// refute, or demote), so a refutation that only reached an artifact would
/// leave the posted review contradicting the run's own evidence.
/// `parked-follow-up` resolutions are deliberately not in this set — parked
/// items keep their surface and render in the parked section.
pub(crate) fn follow_up_resolved_away_candidate_ids(
    resolved_candidates: &[ResolvedCandidateRecord],
) -> Vec<String> {
    resolved_candidates
        .iter()
        .filter(|record| {
            record.resolved_status == "resolved"
                && matches!(record.resolved_disposition.as_str(), "refuted" | "dropped")
        })
        .map(|record| record.candidate_id.clone())
        .collect()
}

/// Inline-comment candidates are built 1:1 from `ReviewInlineComment` in
/// `build_candidate_records`, so matching back uses the same fields the
/// fingerprint hashed: lane, path, line, and body-as-claim.
pub(crate) fn candidate_matches_inline_comment(
    candidate: &CandidateRecord,
    comment: &ReviewInlineComment,
) -> bool {
    candidate.source == "inline-comment"
        && candidate.lane == comment.lane
        && candidate.path.as_deref() == Some(comment.path.as_str())
        && candidate.line == Some(comment.line)
        && candidate.claim == comment.body
}

/// Summary-only candidates are built 1:1 from `SummaryOnlyFinding` in
/// `build_candidate_records`; matching back uses lane, reason-as-claim, and
/// evidence.
pub(crate) fn candidate_matches_summary_finding(
    candidate: &CandidateRecord,
    finding: &SummaryOnlyFinding,
) -> bool {
    candidate.source == "summary-only-finding"
        && candidate.lane == finding.lane
        && candidate.claim == finding.reason
        && candidate.evidence == finding.evidence
}

pub(crate) fn resolved_candidate_record(
    candidate: &CandidateRecord,
    follow_up_outputs: &[&FollowUpOutputRecord],
    prior_resolved_candidates: &[ResolvedCandidateRecord],
) -> ResolvedCandidateRecord {
    let follow_up_task_ids = unique_follow_up_values(follow_up_outputs, |output| &output.task_id);
    let follow_up_stages = unique_follow_up_values(follow_up_outputs, |output| &output.stage);
    let follow_up_statuses = unique_follow_up_values(follow_up_outputs, |output| &output.status);
    let (resolved_status, resolved_disposition, resolution_source, reason, evidence) =
        resolve_candidate_from_follow_ups(candidate, follow_up_outputs, prior_resolved_candidates);
    let mut source_artifacts = vec![
        "review/candidates.json".to_owned(),
        "review/follow_up_results.json".to_owned(),
        "review/follow_up_outputs.json".to_owned(),
    ];
    if resolution_source == "prior-resolved-candidates" {
        source_artifacts.push(PRIOR_RESOLVED_CANDIDATES_ARTIFACT.to_owned());
    }
    ResolvedCandidateRecord {
        schema: RESOLVED_CANDIDATE_SCHEMA.to_owned(),
        candidate_id: candidate.id.clone(),
        lane: candidate.lane.clone(),
        source: candidate.source.clone(),
        original_status: candidate.status.clone(),
        original_disposition: candidate.disposition.clone(),
        resolved_status,
        resolved_disposition,
        resolution_source,
        source_artifacts,
        reason,
        follow_up_task_ids,
        follow_up_stages,
        follow_up_statuses,
        evidence,
    }
}

pub(crate) fn unique_follow_up_values<F>(
    follow_up_outputs: &[&FollowUpOutputRecord],
    value: F,
) -> Vec<String>
where
    F: Fn(&FollowUpOutputRecord) -> &String,
{
    let mut values = Vec::new();
    for output in follow_up_outputs {
        let value = value(output).clone();
        if !values.contains(&value) {
            values.push(value);
        }
    }
    values
}

pub(crate) fn resolve_candidate_from_follow_ups(
    candidate: &CandidateRecord,
    follow_up_outputs: &[&FollowUpOutputRecord],
    prior_resolved_candidates: &[ResolvedCandidateRecord],
) -> (String, String, String, String, Vec<String>) {
    let mut signals = resolved_candidate_signals(follow_up_outputs);
    if !signals.is_empty() {
        let dispositions = signals
            .iter()
            .map(|signal| signal.disposition)
            .collect::<BTreeSet<_>>();
        if dispositions.len() > 1 {
            return (
                "conflicting".to_owned(),
                candidate.disposition.clone(),
                "orchestrator-follow-up".to_owned(),
                "candidate-targeted follow-ups produced conflicting disposition signals".to_owned(),
                signals
                    .into_iter()
                    .flat_map(|signal| signal.evidence)
                    .collect(),
            );
        }
        let signal = signals.remove(0);
        return (
            "resolved".to_owned(),
            signal.disposition.to_owned(),
            "orchestrator-follow-up".to_owned(),
            signal.reason,
            signal.evidence,
        );
    }

    if let Some(prior) =
        prior_resolved_candidate_for_candidate(candidate, prior_resolved_candidates)
    {
        let mut evidence = vec![format!(
            "Prior resolved candidate `{}` had disposition `{}`",
            prior.candidate_id, prior.resolved_disposition
        )];
        evidence.extend(prior.evidence.clone());
        return (
            "resolved".to_owned(),
            prior.resolved_disposition.clone(),
            "prior-resolved-candidates".to_owned(),
            "prior pass resolved the same candidate surface".to_owned(),
            evidence,
        );
    }

    if follow_up_outputs.is_empty() {
        return (
            "unchanged".to_owned(),
            candidate.disposition.clone(),
            "candidate".to_owned(),
            "no candidate-targeted follow-up output".to_owned(),
            vec![format!(
                "Original candidate disposition `{}`",
                candidate.disposition
            )],
        );
    }

    let evidence = follow_up_outputs
        .iter()
        .map(|output| {
            format!(
                "Follow-up task `{}` stage `{}` status `{}`",
                output.task_id, output.stage, output.status
            )
        })
        .collect::<Vec<_>>();
    if follow_up_outputs
        .iter()
        .any(|output| matches!(output.status.as_str(), "ok" | "degraded"))
    {
        (
            "unresolved".to_owned(),
            candidate.disposition.clone(),
            "orchestrator-follow-up".to_owned(),
            "candidate-targeted follow-up ran without a refuted, parked, or dropped disposition"
                .to_owned(),
            evidence,
        )
    } else {
        (
            "follow-up-unavailable".to_owned(),
            candidate.disposition.clone(),
            "orchestrator-follow-up".to_owned(),
            "candidate-targeted follow-up did not produce usable model output".to_owned(),
            evidence,
        )
    }
}

pub(crate) fn prior_resolved_candidate_for_candidate<'a>(
    candidate: &CandidateRecord,
    prior_resolved_candidates: &'a [ResolvedCandidateRecord],
) -> Option<&'a ResolvedCandidateRecord> {
    let candidate_fingerprint = candidate_id_fingerprint(&candidate.id)?;
    prior_resolved_candidates.iter().find(|prior| {
        prior.resolved_status == "resolved"
            && matches!(prior.resolved_disposition.as_str(), "refuted" | "dropped")
            && prior.lane == candidate.lane
            && prior.source == candidate.source
            && prior.original_status == candidate.status
            && candidate_id_fingerprint(&prior.candidate_id) == Some(candidate_fingerprint)
    })
}

pub(crate) fn candidate_id_fingerprint(id: &str) -> Option<&str> {
    let (_, suffix) = id.rsplit_once('-')?;
    if suffix.len() == 12 && suffix.chars().all(|ch| ch.is_ascii_hexdigit()) {
        Some(suffix)
    } else {
        None
    }
}

pub(crate) struct ResolvedCandidateSignal {
    disposition: &'static str,
    reason: String,
    evidence: Vec<String>,
}

pub(crate) fn resolved_candidate_signals(
    follow_up_outputs: &[&FollowUpOutputRecord],
) -> Vec<ResolvedCandidateSignal> {
    let mut signals = Vec::new();
    for output in follow_up_outputs {
        if let Some(evidence) = follow_up_refuted_evidence(output) {
            signals.push(ResolvedCandidateSignal {
                disposition: "refuted",
                reason: format!("follow-up task `{}` refuted the candidate", output.task_id),
                evidence,
            });
        }
    }
    for output in follow_up_outputs {
        if let Some(evidence) = follow_up_covered_evidence(output) {
            signals.push(ResolvedCandidateSignal {
                disposition: "dropped",
                reason: format!("follow-up task `{}` covered the candidate", output.task_id),
                evidence,
            });
        }
    }
    for output in follow_up_outputs {
        if let Some(evidence) = follow_up_parked_evidence(output) {
            signals.push(ResolvedCandidateSignal {
                disposition: "parked-follow-up",
                reason: format!("follow-up task `{}` parked the candidate", output.task_id),
                evidence,
            });
        }
    }
    for output in follow_up_outputs {
        if let Some(evidence) = follow_up_dropped_evidence(output) {
            signals.push(ResolvedCandidateSignal {
                disposition: "dropped",
                reason: format!("follow-up task `{}` dropped the candidate", output.task_id),
                evidence,
            });
        }
    }
    signals
}

pub(crate) fn follow_up_refuted_evidence(output: &FollowUpOutputRecord) -> Option<Vec<String>> {
    if output.observations.iter().any(observation_is_refuted) {
        return Some(vec![format!(
            "Follow-up `{}` emitted a refuted/resolved observation",
            output.task_id
        )]);
    }
    output
        .summary_only_findings
        .iter()
        .find(|finding| candidate_disposition_for_summary_finding(finding) == "refuted")
        .map(|finding| vec![format!("Follow-up summary: {}", finding.reason)])
}

pub(crate) fn follow_up_covered_evidence(output: &FollowUpOutputRecord) -> Option<Vec<String>> {
    if output.observations.iter().any(observation_is_covered) {
        return Some(vec![format!(
            "Follow-up `{}` emitted a covered/resolved observation",
            output.task_id
        )]);
    }
    None
}

pub(crate) fn follow_up_parked_evidence(output: &FollowUpOutputRecord) -> Option<Vec<String>> {
    if output.observations.iter().any(observation_is_parked) {
        return Some(vec![format!(
            "Follow-up `{}` emitted a parked observation",
            output.task_id
        )]);
    }
    output
        .summary_only_findings
        .iter()
        .find(|finding| candidate_disposition_for_summary_finding(finding) == "parked-follow-up")
        .map(|finding| vec![format!("Follow-up summary: {}", finding.reason)])
}

pub(crate) fn follow_up_dropped_evidence(output: &FollowUpOutputRecord) -> Option<Vec<String>> {
    output
        .summary_only_findings
        .iter()
        .find(|finding| candidate_disposition_for_summary_finding(finding) == "dropped")
        .map(|finding| vec![format!("Follow-up summary: {}", finding.reason)])
}
