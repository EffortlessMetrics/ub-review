//! Review compiler: decide whether to compile and post a review,
//! compile the review surface, enforce the PR body policy, and detect
//! forbidden boilerplate (cleanup train step 34, pure code motion).

use crate::*;

/// The markdown heading the compiler prepends for the reporter's editorial
/// distillation. Used both to emit the section and to recognize it as
/// reviewer value, so the two sites cannot silently drift apart (one of the
/// residual risks ub-review's own self-review flagged on #731). A rename
/// here updates emission and recognition together.
const REPORTER_SUMMARY_HEADING: &str = "## Reporter summary";

pub(crate) fn should_prepare_github_review_payload(
    args: &RunArgs,
    inline_comments: &[ReviewInlineComment],
    _summary_only_findings: &[SummaryOnlyFinding],
    proof_receipts: &[ProofReceipt],
    pr_body: &str,
) -> bool {
    if matches!(args.model_mode, ModelMode::Off) {
        return false;
    }
    if has_reviewer_value(inline_comments, pr_body) {
        return true;
    }
    if proof_receipts
        .iter()
        .any(proof_receipt_changes_review_value)
    {
        return true;
    }
    pr_body_has_reviewer_value(pr_body)
}

/// Recognize the reviewer-value headings that justify posting a PR review.
///
/// `## Reporter summary` is included because it is the live reporter's editorial
/// distillation (Order 9 #696 / Order 10 #678) — the reporter "decides what is
/// worth saying" (core doctrine), and the compiler only prepends it when the
/// distillation is non-empty. Without this entry, a run whose deterministic
/// compiler produced no ranked findings/inline comments but whose reporter
/// distilled a substantive editorial (e.g. flagging real copy/accuracy issues on
/// a docs-only diff) would be misclassified as `skipped_empty_smoke` and the
/// reporter's editorial would be silently withheld from the PR. Observed on
/// ripr-swarm #1487: the reporter flagged two actionable findings, both
/// independently raised by gemini-code-assist, yet `post-result.json` recorded
/// `status=skipped` because no other reviewer-value heading was present.
pub(crate) fn pr_body_has_reviewer_value(body: &str) -> bool {
    [
        REPORTER_SUMMARY_HEADING,
        "## Confirmed findings",
        "## Findings",
        "## Verification questions",
        "## Test proof",
        "## Proof results",
        "## Refuted",
        "## Parked follow-ups",
        "## Suggested follow-up",
        "## Evidence gaps",
        "## Missing evidence",
    ]
    .iter()
    .any(|heading| body.contains(heading))
}

pub(crate) struct ReviewCompilerInput<'a> {
    pub(crate) shared_context_id: &'a str,
    pub(crate) review_body_policy: &'a ReviewBodyPolicy,
    /// Resolved run pass (never `RunPass::Auto` from the run flow).
    pub(crate) run_pass: RunPass,
    /// `[gate].post_review_on` event actions from the selected profile.
    pub(crate) post_review_on: &'a [String],
    pub(crate) args: &'a RunArgs,
    pub(crate) plan: &'a Plan,
    pub(crate) diff: &'a DiffContext,
    pub(crate) model_lanes: &'a [ModelLaneReceipt],
    pub(crate) missing_or_failed_sensor_evidence: &'a [SensorEvidenceIssue],
    pub(crate) missing_or_failed_model_evidence: &'a [ModelEvidenceIssue],
    pub(crate) inline_comments: &'a [ReviewInlineComment],
    pub(crate) summary_only_findings: &'a [SummaryOnlyFinding],
    pub(crate) observations: &'a [Observation],
    pub(crate) proof_receipts: &'a [ProofReceipt],
    /// Issue candidates the action ledger marked `suggested` (release lane
    /// step 5): rendered as a follow-up section, never blocking.
    pub(crate) suggested_issues: &'a [IssueCandidate],
    pub(crate) final_follow_up_tasks: usize,
    /// The live reporter's editorial distillation (Order 9 #696 / Order 10
    /// #678). When present and non-empty, rendered as a "## Reporter summary"
    /// section at the top of the PR review body. The compiler passes it
    /// through verbatim (firewall: validates anchors/schema/limits/redaction/
    /// posting only — does not rank or suppress the reporter's editorial).
    pub(crate) reporter_distillation: Option<&'a str>,
}

#[derive(Debug, Serialize)]
pub(crate) struct FinalCompilerInputArtifact<'a> {
    pub(crate) schema: &'static str,
    pub(crate) phase: &'static str,
    pub(crate) source_artifacts: &'static [&'static str],
    pub(crate) model_lanes: &'a [ModelLaneReceipt],
    pub(crate) missing_or_failed_sensor_evidence: &'a [SensorEvidenceIssue],
    pub(crate) missing_or_failed_model_evidence: &'a [ModelEvidenceIssue],
    /// Candidates the follow-up pass resolved to `refuted` or `dropped`;
    /// their original review surfaces are excluded from `inline_comments`
    /// and `summary_only_findings` below (v2 contract).
    pub(crate) follow_up_resolved_candidate_ids: &'a [String],
    pub(crate) inline_comments: &'a [ReviewInlineComment],
    pub(crate) summary_only_findings: &'a [SummaryOnlyFinding],
    pub(crate) observations: &'a [Observation],
    pub(crate) proof_receipts: &'a [ProofReceipt],
}

pub(crate) struct CompiledReviewSurface {
    pub(crate) artifact_body: String,
    pub(crate) github_review: GitHubReview,
    pub(crate) should_prepare_github_review: bool,
    /// True when `[review_body].summary_only_body` posted a body the
    /// suppressor classified as no-value boilerplate; the payload writer
    /// waives the suppressible body-policy checks for exactly this case.
    pub(crate) summary_only_policy_posted: bool,
    pub(crate) review_payload_status: &'static str,
    pub(crate) terminal_state: ReviewTerminalState,
}

/// Pass-level posting policy: when the resolved posting mode is `review`, a
/// pass may carry the grouped PR review only if the profile's
/// `[gate].post_review_on` lists its `pull_request` event action. Manual runs
/// (workflow_dispatch/local) are explicit operator requests and are not gated;
/// catch-all `pull_request_other` passes never post. Artifact-only runs keep
/// preparing the payload artifact because nothing is posted from them.
pub(crate) fn pass_policy_permits_review_post(
    posting: PostingMode,
    run_pass: RunPass,
    post_review_on: &[String],
) -> bool {
    if !matches!(posting, PostingMode::Review) {
        return true;
    }
    match run_pass.event_action() {
        Some(action) => post_review_on.iter().any(|allowed| allowed == action),
        // Only an explicit operator request (`manual`) bypasses the profile
        // pass list. `Auto` must be resolved before compilation; if it leaks
        // here, denying the post is the safe failure (gate inline finding on
        // run 27053765285: an Auto leak would post on every profile).
        None => matches!(run_pass, RunPass::Manual),
    }
}

pub(crate) fn compile_review_surface(
    input: ReviewCompilerInput<'_>,
) -> Result<CompiledReviewSurface> {
    let ranked_inline_comments = ranked_inline_comments(input.inline_comments);
    let pr_inline_candidates = ranked_inline_comments
        .iter()
        .take(input.args.max_inline_comments)
        .cloned()
        .collect::<Vec<_>>();
    let artifact_body = render_review_body(
        input.shared_context_id,
        input.plan,
        input.diff,
        input.model_lanes,
        input.missing_or_failed_sensor_evidence,
        input.missing_or_failed_model_evidence,
        &ranked_inline_comments,
        input.summary_only_findings,
        input.observations,
        input.proof_receipts,
        input.args.review_body_max_bytes,
        ReviewBodyAudience::Artifact,
    );
    let mut pr_body = render_review_body(
        input.shared_context_id,
        input.plan,
        input.diff,
        input.model_lanes,
        input.missing_or_failed_sensor_evidence,
        input.missing_or_failed_model_evidence,
        &pr_inline_candidates,
        input.summary_only_findings,
        input.observations,
        input.proof_receipts,
        input.args.review_body_max_bytes,
        ReviewBodyAudience::PullRequest,
    );
    // Order 10 (#678): the live reporter's editorial distillation renders at
    // the TOP of the PR review body, before any finding sections. The compiler
    // passes it through verbatim — it is the reporter's editorial judgment,
    // not a deterministic finding the compiler ranks or suppresses (firewall,
    // not truth reducer). Subject only to the existing body-size limit.
    if let Some(distillation) = input.reporter_distillation {
        let trimmed = distillation.trim();
        if !trimmed.is_empty() {
            let reporter_section = format!("{REPORTER_SUMMARY_HEADING}\n\n{trimmed}\n\n");
            pr_body = if pr_body.is_empty() {
                reporter_section.trim_end().to_owned()
            } else {
                format!("{reporter_section}{pr_body}")
            };
        }
    }
    // Release lane step 5: suggested follow-ups render last - they explain
    // why the PR's scope was not broadened, never block, and only appear
    // when the action ledger promoted a candidate to `suggested`. The full
    // draft (plan + acceptance) lives in review/suggested_issues.md.
    if !input.suggested_issues.is_empty() {
        let mut section = String::from(
            "
## Suggested follow-up

",
        );
        for candidate in input.suggested_issues {
            section.push_str(&format!(
                "- {} (`{}`). {} Plan and acceptance criteria: review/suggested_issues.md.
",
                escape_md(&candidate.title),
                candidate.target_repo,
                escape_md(&candidate.why_not_this_pr)
            ));
        }
        if pr_body.is_empty() {
            // A suggested follow-up is reviewer value on its own: it tells
            // the reviewer why this PR stays small.
            pr_body = section.trim_start().to_owned();
        } else {
            pr_body.push_str(&section);
        }
        pr_body = cap_review_body(pr_body.clone(), input.args.review_body_max_bytes);
    }
    let substantive_summary_only_findings =
        count_substantive_summary_only_findings(input.summary_only_findings);
    let mut suppressed_artifact_only_pr_body = false;
    let mut summary_only_policy_posted = false;
    if let Err(err) = validate_pr_review_body_policy(&pr_body, input.review_body_policy) {
        if !is_suppressible_pr_body_policy_error(&err) {
            return Err(err).with_context(|| "validate pull request review body policy");
        }
        if !matches!(input.args.model_mode, ModelMode::Off)
            && summary_only_body_policy_permits_post(
                input.review_body_policy.summary_only_body,
                input.summary_only_findings.len(),
                substantive_summary_only_findings,
            )
        {
            // The configured [review_body].summary_only_body posture posts
            // this body despite the suppressible classification. Everything
            // outside the suppressible classes must still hold; rendered PR
            // bodies never carry those sections, so a failure here means a
            // body-construction bug, not reviewer content worth withholding.
            validate_pr_review_body_policy_with_waiver(&pr_body, input.review_body_policy, true)
                .with_context(
                    || "validate pull request review body policy under summary_only_body waiver",
                )?;
            summary_only_policy_posted = true;
        } else {
            pr_body.clear();
            suppressed_artifact_only_pr_body = true;
        }
    }
    let pr_inline_comments: &[ReviewInlineComment] = if suppressed_artifact_only_pr_body {
        &[]
    } else {
        &pr_inline_candidates
    };
    let github_review = GitHubReview {
        event: "COMMENT".to_owned(),
        body: pr_body.clone(),
        comments: pr_inline_comments
            .iter()
            .map(|comment| GitHubReviewComment {
                path: comment.path.clone(),
                line: comment.line,
                side: comment.side.clone(),
                body: comment.body.clone(),
                suggestion: comment.suggestion.clone(),
            })
            .collect(),
    };
    let pass_policy_permits_post =
        pass_policy_permits_review_post(input.args.posting, input.run_pass, input.post_review_on);
    let should_prepare_github_review = pass_policy_permits_post
        && !suppressed_artifact_only_pr_body
        && (summary_only_policy_posted
            || should_prepare_github_review_payload(
                input.args,
                pr_inline_comments,
                input.summary_only_findings,
                input.proof_receipts,
                &pr_body,
            ));
    let review_payload_status = if should_prepare_github_review {
        "prepared"
    } else if !pass_policy_permits_post {
        "skipped_pass_policy"
    } else if suppressed_artifact_only_pr_body {
        "skipped_artifact_only_body"
    } else {
        "skipped_empty_smoke"
    };
    let terminal_state = build_review_terminal_state(TerminalStateInput {
        args: input.args,
        plan: input.plan,
        run_pass: input.run_pass,
        review_payload_status,
        should_prepare_github_review,
        pr_body: &pr_body,
        inline_comments: pr_inline_comments,
        summary_only_findings: input.summary_only_findings,
        summary_only_body: input.review_body_policy.summary_only_body,
        model_lanes: input.model_lanes,
        missing_or_failed_sensor_evidence: input.missing_or_failed_sensor_evidence,
        missing_or_failed_model_evidence: input.missing_or_failed_model_evidence,
        proof_receipts: input.proof_receipts,
        final_follow_up_tasks: input.final_follow_up_tasks,
    });
    Ok(CompiledReviewSurface {
        artifact_body,
        github_review,
        should_prepare_github_review,
        summary_only_policy_posted,
        review_payload_status,
        terminal_state,
    })
}

/// A summary-only finding is substantive when it carries reviewer-relevant
/// weight on its own: severity medium or higher, or confidence medium-high or
/// higher. Pure lane-status and guardrail notes
/// (`is_pr_body_artifact_only_finding`) are never substantive regardless of
/// the severity/confidence they record.
pub(crate) fn summary_only_finding_is_substantive(finding: &SummaryOnlyFinding) -> bool {
    if is_pr_body_artifact_only_finding(finding) {
        return false;
    }
    matches!(finding.severity.as_str(), "blocker" | "high" | "medium")
        || matches!(finding.confidence.as_str(), "high" | "medium-high")
}

pub(crate) fn count_substantive_summary_only_findings(findings: &[SummaryOnlyFinding]) -> usize {
    findings
        .iter()
        .filter(|finding| summary_only_finding_is_substantive(finding))
        .count()
}

/// `[review_body].summary_only_body` decision for a PR body the suppressor
/// classified as no-value boilerplate: `suppress` always withholds,
/// `post_substantive` posts when at least one summary-only finding is
/// substantive, `post_all` posts whenever any summary-only finding exists.
pub(crate) fn summary_only_body_policy_permits_post(
    policy: SummaryOnlyBodyPolicy,
    summary_only_findings: usize,
    substantive_summary_only_findings: usize,
) -> bool {
    match policy {
        SummaryOnlyBodyPolicy::Suppress => false,
        SummaryOnlyBodyPolicy::PostSubstantive => substantive_summary_only_findings > 0,
        SummaryOnlyBodyPolicy::PostAll => summary_only_findings > 0,
    }
}

const MAX_PR_REVIEW_BODY_BYTES: usize = 6_000;
const MAX_PR_REVIEW_BODY_BULLETS: usize = 12;

pub(crate) fn is_suppressible_pr_body_policy_error(error: &anyhow::Error) -> bool {
    let text = error.to_string();
    text.contains("artifact-only boilerplate")
        || text.contains("refuted-only artifact note")
        || text.contains("not concise enough")
}

pub(crate) fn validate_pr_review_body_policy(body: &str, policy: &ReviewBodyPolicy) -> Result<()> {
    validate_pr_review_body_policy_with_waiver(body, policy, false)
}

/// Body-policy validation with an optional waiver for the suppressible
/// classes (`is_suppressible_pr_body_policy_error`: conciseness, artifact-only
/// boilerplate, refuted-only note). The waiver exists for exactly one caller
/// posture: `[review_body].summary_only_body` decided to post a body the
/// suppressor would have withheld. The non-suppressible checks (lane,
/// provider, and sensor tables plus the execution summary) always run.
pub(crate) fn validate_pr_review_body_policy_with_waiver(
    body: &str,
    policy: &ReviewBodyPolicy,
    waive_suppressible: bool,
) -> Result<()> {
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Ok(());
    }
    if !waive_suppressible {
        if trimmed.len() > MAX_PR_REVIEW_BODY_BYTES {
            bail!(
                "github review body is not concise enough: {} bytes over max {}",
                trimmed.len(),
                MAX_PR_REVIEW_BODY_BYTES
            );
        }
        let bullet_count = pr_body_bullet_count(trimmed);
        if bullet_count > MAX_PR_REVIEW_BODY_BULLETS {
            bail!(
                "github review body is not concise enough: {bullet_count} bullets over max {MAX_PR_REVIEW_BODY_BULLETS}"
            );
        }
        if has_forbidden_pr_review_boilerplate(trimmed) {
            bail!("github review body contains artifact-only boilerplate");
        }
        if is_refuted_only_pr_body(trimmed) {
            bail!("github review body contains refuted-only artifact note");
        }
    }
    if !policy.include_successful_lane_table && contains_successful_lane_table(trimmed) {
        bail!("github review body contains successful lane table");
    }
    match policy.include_provider_table {
        ReviewBodyTablePolicy::Always => {}
        ReviewBodyTablePolicy::Never | ReviewBodyTablePolicy::OnFailure => {
            if contains_provider_status_table(trimmed) {
                bail!("github review body contains provider status table");
            }
        }
    }
    match policy.include_sensor_table {
        ReviewBodyTablePolicy::Always => {}
        ReviewBodyTablePolicy::Never | ReviewBodyTablePolicy::OnFailure => {
            if contains_sensor_status_table(trimmed) {
                bail!("github review body contains sensor status table");
            }
        }
    }
    match policy.include_execution_summary {
        ReviewBodyExecutionSummaryPolicy::Always => {}
        ReviewBodyExecutionSummaryPolicy::None => {
            if contains_execution_summary(trimmed) {
                bail!("github review body contains execution summary");
            }
        }
        ReviewBodyExecutionSummaryPolicy::OnFailure => {
            if contains_execution_summary(trimmed) && !pr_body_has_failure_context(trimmed) {
                bail!("github review body contains success execution summary");
            }
        }
    }
    Ok(())
}

pub(crate) fn pr_body_bullet_count(body: &str) -> usize {
    body.lines()
        .filter(|line| {
            let line = line.trim_start();
            line.starts_with("- ") || line.starts_with("* ")
        })
        .count()
}

pub(crate) fn has_forbidden_pr_review_boilerplate(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    is_workflow_trust_posture_review_noise(&lower)
        || [
            "no blocking finding after",
            "no blocking ub finding",
            "no actionable findings",
            "a human should still inspect",
            "human should still review",
            "residual risk remains for human review",
            "bounded review",
            "## residual risk",
            "cached prior observation",
            "refuter demoted inline candidate",
            "gate proof is pending",
            "cannot perform from cached context",
            "commit-existence/ancestry proof",
            "upstream commit-existence",
            "general bot output",
            "pr-body contract hardening",
            "actionlint ran ok",
            "pre-existing, not a diff target",
            "identical to prior pin",
            "no widened attack surface",
            "standing-repo concern",
            "lane transcript",
            "lane roster",
            "model lane roster",
            "raw observations",
            "provider preflight",
            "provider status",
            "sensor status",
            "shared context hash",
            "cache manifest",
            "runtime profile",
            "review payload status",
            "terminal state",
            "github-review-skip",
            "command log",
            "all checks passed",
            "no issues found",
            "looks good",
            "lgtm",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
        || has_first_person_success_announcement(&lower)
}

pub(crate) fn has_first_person_success_announcement(lower_body: &str) -> bool {
    lower_body.lines().any(|line| {
        let trimmed = trim_review_list_marker(line);
        trimmed.starts_with("we ran ") || trimmed.starts_with("i ran ")
    })
}

pub(crate) fn trim_review_list_marker(line: &str) -> &str {
    let mut trimmed = line.trim_start();
    for prefix in ["- ", "* "] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            trimmed = rest.trim_start();
            break;
        }
    }
    trimmed
}

pub(crate) fn is_refuted_only_pr_body(body: &str) -> bool {
    let lower = body.to_ascii_lowercase();
    lower.contains("## refuted")
        && ![
            "## decision",
            "## confirmed findings",
            "## verification questions",
            "## test proof",
            "## proof results",
            "## parked follow-ups",
            "## evidence gaps",
            "## missing evidence",
        ]
        .iter()
        .any(|heading| lower.contains(heading))
}

pub(crate) fn contains_successful_lane_table(body: &str) -> bool {
    contains_case_insensitive_line_prefix(
        body,
        &[
            "## model lanes",
            "## model lane status",
            "## lane status",
            "## lane roster",
        ],
    )
}

pub(crate) fn contains_provider_status_table(body: &str) -> bool {
    contains_case_insensitive_line_prefix(
        body,
        &[
            "## provider preflights",
            "## provider status",
            "## model provider status",
        ],
    )
}

pub(crate) fn contains_sensor_status_table(body: &str) -> bool {
    contains_case_insensitive_line_prefix(
        body,
        &["## sensors", "## sensor status", "## sensor receipts"],
    )
}

pub(crate) fn contains_execution_summary(body: &str) -> bool {
    contains_case_insensitive_line_prefix(
        body,
        &[
            "- shared context:",
            "- profile:",
            "- base:",
            "- head:",
            "- changed files:",
            "- inline comments:",
            "## review efficiency",
            "runtime:",
            "terminal state:",
            "review payload:",
            "follow-up results:",
        ],
    )
}

pub(crate) fn contains_case_insensitive_line_prefix(body: &str, needles: &[&str]) -> bool {
    body.lines().any(|line| {
        let lower = line.trim_start().to_ascii_lowercase();
        needles.iter().any(|needle| lower.starts_with(needle))
    })
}

pub(crate) fn pr_body_has_failure_context(body: &str) -> bool {
    [
        "## Decision",
        "## Evidence gaps",
        "## Missing evidence",
        "## Missing or failed evidence",
        "Needs ",
        "failed",
        "timed out",
        "unavailable",
    ]
    .iter()
    .any(|needle| body.contains(needle))
}

pub(crate) struct TerminalStateInput<'a> {
    pub(crate) args: &'a RunArgs,
    pub(crate) plan: &'a Plan,
    pub(crate) run_pass: RunPass,
    pub(crate) review_payload_status: &'a str,
    pub(crate) should_prepare_github_review: bool,
    pub(crate) pr_body: &'a str,
    pub(crate) inline_comments: &'a [ReviewInlineComment],
    pub(crate) summary_only_findings: &'a [SummaryOnlyFinding],
    pub(crate) summary_only_body: SummaryOnlyBodyPolicy,
    pub(crate) model_lanes: &'a [ModelLaneReceipt],
    pub(crate) missing_or_failed_sensor_evidence: &'a [SensorEvidenceIssue],
    pub(crate) missing_or_failed_model_evidence: &'a [ModelEvidenceIssue],
    pub(crate) proof_receipts: &'a [ProofReceipt],
    pub(crate) final_follow_up_tasks: usize,
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression for ripr-swarm #1487 / ub-review gap: a run whose deterministic
    // compiler produced no ranked findings or inline comments, but whose reporter
    // lane distilled a substantive editorial, must still count as reviewer value
    // so the reporter's editorial is not silently withheld as skipped_empty_smoke.
    // `pr_body_has_reviewer_value` is the terminal predicate that
    // `should_prepare_github_review_payload` delegates to when there are no
    // inline comments and no proof receipts, so covering it here covers the gate.
    #[test]
    fn reporter_summary_heading_is_reviewer_value() {
        // Minimal body the compiler builds when the distillation is the only
        // reviewer-value content: the reporter section, nothing else. Built
        // from the same constant the emitter uses, so this also pins that the
        // emission heading stays in the recognized set.
        let body = format!(
            "{REPORTER_SUMMARY_HEADING}\n\nDocs-only README reorder that demotes vocabulary; two copy issues flagged.\n\n"
        );
        assert!(
            pr_body_has_reviewer_value(&body),
            "a non-empty Reporter summary section is reviewer value (reporter decides what is worth saying)"
        );
    }

    #[test]
    fn empty_body_without_headings_is_not_reviewer_value() {
        assert!(!pr_body_has_reviewer_value(""));
        assert!(!pr_body_has_reviewer_value(
            "lane status: ok\nno findings\n"
        ));
        // A body whose text does not contain any reviewer-value heading must
        // not match. (Like the other headings, the check is intentionally a
        // substring match, so this asserts the negative case, not a prefix rule.)
        assert!(!pr_body_has_reviewer_value(
            "## Status\nall lanes degraded\n"
        ));
    }

    #[test]
    fn other_reviewer_value_headings_still_recognized() {
        // Existing behavior preserved: deterministic findings still count.
        assert!(pr_body_has_reviewer_value("## Confirmed findings\n- x"));
        assert!(pr_body_has_reviewer_value("## Verification questions\n- y"));
        assert!(pr_body_has_reviewer_value("## Evidence gaps\n- z"));
    }

    // Drift guard for the residual risk ub-review's self-review flagged on
    // #731: if the emission heading ever diverges from the recognized heading,
    // reporter editorials would silently regress to skipped_empty_smoke. Both
    // sites reference REPORTER_SUMMARY_HEADING, and this test asserts the
    // emitted section shape is recognized as reviewer value.
    #[test]
    fn reporter_emission_heading_is_recognized_as_reviewer_value() {
        let emitted = format!(
            "{}\n\nsome editorial distillation\n\n",
            REPORTER_SUMMARY_HEADING
        );
        assert!(pr_body_has_reviewer_value(&emitted));
    }
}
