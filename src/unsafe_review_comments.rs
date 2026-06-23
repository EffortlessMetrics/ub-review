//! Unsafe-review inline comment planning: repair-queue read, comment-plan
//! candidate selection, and GitHub inline comment construction
//! (cleanup train step 61, pure code motion).

use crate::*;

/// Execute the run-written broker plan, never fatally: read
/// review/issue_broker_plan.json next to the review payload, perform the
/// remote duplicate search and opens for `attempt` entries, and write
/// issue_broker_results artifacts. Absent plan means the broker was not
/// opted in; any whole-step error is reported to stderr and swallowed
/// (broker outcomes never affect the gate or the post exit code).
pub(crate) fn read_repair_queue(
    sensor_dir: &Path,
    artifacts: &UnsafeReviewGate,
) -> std::collections::BTreeMap<String, RepairQueueEntry> {
    let out_dir = sensor_dir.join(UNSAFE_REVIEW_OUTPUT_SUBDIR);
    let rq_path = artifacts
        .artifacts
        .get("repair_queue")
        .map(|rel| out_dir.join(rel))
        .unwrap_or_else(|| out_dir.join("repair-queue.json"));
    let text = match fs::read_to_string(&rq_path) {
        Ok(t) => t,
        Err(_) => return std::collections::BTreeMap::new(),
    };
    let rq: RepairQueueFile = match serde_json::from_str(&text) {
        Ok(r) => r,
        Err(_) => return std::collections::BTreeMap::new(),
    };
    let mut by_card: std::collections::BTreeMap<String, RepairQueueEntry> =
        std::collections::BTreeMap::new();
    for entries in rq.buckets.into_values() {
        for entry in entries {
            by_card.entry(entry.card_id.clone()).or_insert(entry);
        }
    }
    by_card
}

/// Build `GitHubReviewComment` entries from unsafe-review `comment-plan.json`
/// candidates for inline posting on the PR diff.
///
/// # Selection rules
/// - Only candidates with `changed_line: true` are eligible; anchoring to an
///   unchanged line would be rejected by the GitHub review API.
/// - Capped at `min(comment_plan.len(), max_inline_budget)` (the comment plan
///   is already bounded to ≤3 by unsafe-review itself).
/// - Deduplication against `existing_paths_lines` prevents double-posting if a
///   model lane already proposed a comment on the same `(path, line)` pair.
///
/// # Suggestion blocks
/// Each comment body names the `coverage_gap`, the next reviewer action
/// (`confirmation_state`), and the per-entry `trust_boundary`. A GitHub
/// `suggestion` block is emitted ONLY when unsafe-review's repair-queue
/// provides a concrete applicable code edit for the site. As of
/// `repair-queue/0.1`, the queue provides bucket classification and guidance
/// (missing evidence, do-not-do constraints) but NO replacement text — so
/// suggestion blocks are NOT emitted from this source. The field remains ready
/// on `GitHubReviewComment` for a future repair-queue version that adds
/// `replacement` / `applicable_edit` output. This is an honest capability gap
/// reported upstream as a narrow follow-up issue.
#[cfg(test)]
pub(crate) fn build_unsafe_review_inline_comments(
    sensor_dir: &Path,
    existing_paths_lines: &std::collections::BTreeSet<(String, u32)>,
    right_side_lines: &std::collections::BTreeSet<(String, u32)>,
    max_inline_budget: usize,
) -> Vec<GitHubReviewComment> {
    let artifacts = match read_unsafe_review_artifacts(sensor_dir) {
        Ok(a) => a,
        Err(_) => return Vec::new(),
    };
    let repair_queue = read_repair_queue(sensor_dir, &artifacts.gate);
    let trust = artifacts
        .gate
        .trust_boundary
        .as_deref()
        .unwrap_or("advisory");
    let mut comments: Vec<GitHubReviewComment> = Vec::new();
    for entry in &artifacts.comment_plan {
        if comments.len() >= max_inline_budget {
            break;
        }
        // Only anchor to changed lines — GitHub review API requires it.
        if entry.changed_line != Some(true) {
            continue;
        }
        let (Some(path), Some(line)) = (entry.path.as_deref(), entry.line) else {
            continue;
        };
        // Dedup: skip if a model lane already claimed this (path, line).
        let norm_path = normalize_repo_path(path);
        if !right_side_lines.contains(&(norm_path.clone(), line)) {
            continue;
        }
        if existing_paths_lines.contains(&(norm_path.clone(), line)) {
            continue;
        }
        let gap = entry
            .coverage_gap
            .as_deref()
            .unwrap_or("unsafe coverage gap");
        let action = entry
            .confirmation_state
            .as_deref()
            .unwrap_or("reviewer confirmation required");
        let card_label = entry
            .card_id
            .as_deref()
            .map(|id| format!(" (`{id}`)"))
            .unwrap_or_default();
        // Optional: if the repair queue has an entry for this card, surface the
        // bucket reason, operation, and missing evidence as additional context
        // (guidance only, not a suggestion block).
        let rq_entry = entry.card_id.as_deref().and_then(|id| repair_queue.get(id));
        let rq_context = rq_entry
            .map(|rq_entry| {
                let bucket = rq_entry
                    .bucket_reason
                    .as_deref()
                    .unwrap_or("see repair-queue.json");
                let operation_line = rq_entry
                    .operation
                    .as_deref()
                    .map(|op| format!("\n\n**Operation**: `{op}`"))
                    .unwrap_or_default();
                let evidence_lines = rq_entry
                    .missing_evidence
                    .iter()
                    .map(|e| format!("  - {e}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                if evidence_lines.is_empty() {
                    format!("\n\n**Repair class**: {bucket}{operation_line}")
                } else {
                    format!(
                        "\n\n**Repair class**: {bucket}{operation_line}\n\n**Missing evidence**:\n{evidence_lines}"
                    )
                }
            })
            .unwrap_or_default();
        let suggestion = rq_entry.and_then(RepairQueueEntry::suggestion);
        let body = format!(
            "[unsafe-review]{card_label} **{gap}**\n\n\
             **Next action**: {action}\n\n\
             **Trust boundary** (advisory): {trust}{rq_context}\n\n\
             _Suggestion sourced from unsafe-review advisory output. \
             Apply only after reviewer verification. \
             Inline comments are advisory — they do not change the merge decision._"
        );
        // No suggestion block: repair-queue/0.1 provides guidance (missing
        // evidence, bucket classification, do-not-do constraints) but no
        // concrete replacement text. suggestion = None until a future
        // repair-queue version adds an applicable edit field.
        comments.push(GitHubReviewComment {
            path: norm_path,
            line,
            side: "RIGHT".to_owned(),
            body,
            suggestion,
        });
    }
    comments
}

pub(crate) fn unsafe_review_comment_plan_candidates(
    sensor_dir: &Path,
) -> (Vec<ModelCandidateComment>, Vec<SummaryOnlyFinding>) {
    let mut candidates = Vec::new();
    let mut skips = Vec::new();
    let artifacts = match read_unsafe_review_artifacts(sensor_dir) {
        Ok(artifacts) => artifacts,
        Err(_) => return (candidates, skips),
    };
    let repair_queue = read_repair_queue(sensor_dir, &artifacts.gate);
    let gate_trust = artifacts
        .gate
        .trust_boundary
        .as_deref()
        .unwrap_or("advisory");
    for entry in &artifacts.comment_plan {
        if entry.changed_line != Some(true) {
            let label = entry
                .card_id
                .as_deref()
                .map(|id| format!(" `{id}`"))
                .unwrap_or_default();
            skips.push(SummaryOnlyFinding {
                lane: "unsafe-review".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: format!(
                    "unsafe-review comment-plan{label} did not target a changed RIGHT-side line; kept artifact-only"
                ),
                evidence: "unsafe-review comment-plan changed_line guard".to_owned(),
            });
            continue;
        }
        let (Some(path), Some(line)) = (entry.path.as_deref(), entry.line) else {
            let label = entry
                .card_id
                .as_deref()
                .map(|id| format!(" `{id}`"))
                .unwrap_or_default();
            skips.push(SummaryOnlyFinding {
                lane: "unsafe-review".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: format!(
                    "unsafe-review comment-plan{label} lacked path or line; kept artifact-only"
                ),
                evidence: "unsafe-review comment-plan anchor guard".to_owned(),
            });
            continue;
        };
        let gap = entry
            .coverage_gap
            .as_deref()
            .unwrap_or("unsafe coverage gap");
        let action = entry
            .confirmation_state
            .as_deref()
            .unwrap_or("reviewer confirmation required");
        let card_label = entry
            .card_id
            .as_deref()
            .map(|id| format!(" (`{id}`)"))
            .unwrap_or_default();
        let trust = entry.trust_boundary.as_deref().unwrap_or(gate_trust);
        let rq_entry = entry.card_id.as_deref().and_then(|id| repair_queue.get(id));
        let rq_context = rq_entry
            .map(|rq_entry| {
                let bucket = rq_entry
                    .bucket_reason
                    .as_deref()
                    .unwrap_or("see repair-queue.json");
                let operation_line = rq_entry
                    .operation
                    .as_deref()
                    .map(|op| format!("\n\n**Operation**: `{op}`"))
                    .unwrap_or_default();
                let evidence_lines = rq_entry
                    .missing_evidence
                    .iter()
                    .map(|e| format!("  - {e}"))
                    .collect::<Vec<_>>()
                    .join("\n");
                if evidence_lines.is_empty() {
                    format!("\n\n**Repair class**: {bucket}{operation_line}")
                } else {
                    format!(
                        "\n\n**Repair class**: {bucket}{operation_line}\n\n**Missing evidence**:\n{evidence_lines}"
                    )
                }
            })
            .unwrap_or_default();
        let suggestion = rq_entry.and_then(RepairQueueEntry::suggestion);
        let body = truncate_chars(
            &format!(
                "**{gap}**{card_label}\n\n\
                 **Next action**: {action}\n\n\
                 **Trust boundary** (advisory): {trust}{rq_context}\n\n\
                 _Sourced from unsafe-review advisory output. \
                 Apply only after reviewer verification. \
                 Inline comments are advisory - they do not change the merge decision._"
            ),
            1_100,
        );
        let selection = entry
            .selection_reason
            .as_deref()
            .unwrap_or("deterministic comment-plan candidate");
        candidates.push(ModelCandidateComment {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: path.to_owned(),
            line,
            body,
            evidence: format!(
                "unsafe-review comment-plan{card_label}: {selection}; confirmation_state: {action}"
            ),
            suggestion,
        });
    }
    (candidates, skips)
}

pub(crate) fn apply_unsafe_review_comment_plan_candidates(
    sensor_dir: &Path,
    line_map: &BTreeSet<(String, u32)>,
    sinks: ModelOutputSinks<'_>,
) {
    let (candidates, skips) = unsafe_review_comment_plan_candidates(sensor_dir);
    sinks.summary_only_findings.extend(skips);
    if candidates.is_empty() {
        return;
    }
    let lane = unsafe_review_sensor_lane();
    let output = LaneModelOutput {
        summary: None,
        inline_comments: Vec::new(),
        candidate_findings: candidates,
        summary_only_findings: Vec::new(),
        observations: Vec::new(),
        failed_objections: Vec::new(),
        proof_requests: Vec::new(),
        issue_candidates: Vec::new(),
        degraded: false,
    };
    apply_model_output(&lane, output, line_map, sinks);
}
