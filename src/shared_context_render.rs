//! Shared context rendering, GitHub review skip receipts, and shared
//! context cache artifacts (cleanup train step 46, pure code motion).

use crate::diff_posture::{diff_class_posture_heading, review_posture_for_diff_class};
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
) -> Result<(String, Vec<PrefixSection>)> {
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
        // #325: late-phase sensors overlap the model wave, so their receipts
        // are deliberately not read here — the prefix renders them as
        // scheduled work deterministically (independent of how far the late
        // pool got), keeping the shared prefix a function of the plan rather
        // than of scheduler timing. Late is not missing: the receipts land
        // before the reporter/compile/gate and are routed to lanes then.
        if sensor.run && matches!(sensor.phase, SensorPhase::Late) {
            text.push_str(&format!(
                "- `{}`: `scheduled-late` - runs during the model wave; receipt lands before the gate (late is not missing)\n",
                sensor.id
            ));
            continue;
        }
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
    if let Some(unsafe_review) = plan.sensors.iter().find(|s| s.id == "unsafe-review") {
        text.push_str("\n## unsafe-review Coverage Evidence\n\n");
        let sensor_dir = out.join("sensors").join("unsafe-review");
        let status_path = sensor_dir.join("ub-review-sensor-status.json");
        // #325: an unsafe-review sensor deferred to the late phase has no
        // stable evidence at prefix time; render it as scheduled work
        // deterministically instead of reading a racing receipt.
        let sensor_status = if unsafe_review.run && matches!(unsafe_review.phase, SensorPhase::Late)
        {
            "scheduled-late".to_owned()
        } else {
            read_sensor_receipt(&status_path)
                .map(|r| r.status)
                .unwrap_or_else(|| "receipt-absent".to_owned())
        };
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
    // Diff and thread excerpts can contain fixture assignments that look like
    // real credentials. Redact those values before the shared context crosses
    // into model/provider storage or durable artifacts; placeholders such as
    // `${{ secrets.OPENCODE }}` remain safe and useful context.
    let text = redact_shared_context_secret_assignments(&text);
    // Derive the ordered source sections from the rendered prefix by scanning
    // for `## ` headers. The prefix is generated here from stable headers, so
    // this reads exactly what was written (no drift). Each section's byte range
    // runs from its header to the next header (or end of text). This makes the
    // cohort's shared-prefix contract inspectable. (Order 6 of #678.)
    let sections = prefix_sections_from_rendered(&text);
    Ok((text, sections))
}

const SHARED_CONTEXT_SECRET_NAMES: [&str; 9] = [
    "FACTORY_API_KEY",
    "GITHUB_TOKEN",
    "MINIMAX_API_KEY",
    "OPENCODE",
    "OPENCODE_API_KEY",
    "UB_REVIEW_GITHUB_TOKEN",
    "UB_REVIEW_MINIMAX_API_KEY",
    "UB_REVIEW_OPENCODE_API_KEY",
    "github_token",
];

const SHARED_CONTEXT_SAFE_SECRET_VALUES: [&str; 11] = [
    "", "false", "masked", "missing", "none", "null", "present", "redacted", "true", "unset",
    "unknown",
];

/// Redact credential-like assignment values in all shared-context material,
/// including diff and test-source excerpts. This is intentionally
/// conservative: a model does not need a literal credential value to reason
/// about the changed path, while a leaked value cannot be recovered once the
/// prefix is cached or persisted.
pub(crate) fn redact_shared_context_secret_assignments(text: &str) -> String {
    text.split_inclusive('\n')
        .map(redact_shared_context_secret_line)
        .collect()
}

fn redact_shared_context_secret_line(line: &str) -> String {
    let mut output = String::with_capacity(line.len());
    let mut cursor = 0usize;
    while cursor < line.len() {
        let Some((relative_start, name)) = find_shared_context_secret_name(&line[cursor..]) else {
            output.push_str(&line[cursor..]);
            break;
        };
        let name_start = cursor + relative_start;
        let name_end = name_start + name.len();
        let mut separator = name_end;
        while line
            .as_bytes()
            .get(separator)
            .is_some_and(u8::is_ascii_whitespace)
        {
            separator += 1;
        }
        if !matches!(line.as_bytes().get(separator), Some(b'=') | Some(b':')) {
            output.push_str(&line[cursor..name_end]);
            cursor = name_end;
            continue;
        }
        let mut value_start = separator + 1;
        while line
            .as_bytes()
            .get(value_start)
            .is_some_and(u8::is_ascii_whitespace)
        {
            value_start += 1;
        }
        let quote = line
            .as_bytes()
            .get(value_start)
            .copied()
            .filter(|byte| matches!(byte, b'\'' | b'"'));
        let content_start = value_start + usize::from(quote.is_some());
        let content_end = if let Some(quote) = quote {
            line[content_start..]
                .find(char::from(quote))
                .map_or(line.len(), |offset| content_start + offset)
        } else {
            line[content_start..]
                .find(|character: char| {
                    character.is_ascii_whitespace()
                        || matches!(character, ',' | ';' | '}' | ']' | ')' | '"' | '\'')
                })
                .map_or(line.len(), |offset| content_start + offset)
        };
        let value = &line[content_start..content_end];
        output.push_str(&line[cursor..content_start]);
        if shared_context_secret_value_needs_redaction(value) {
            output.push_str("[redacted]");
        } else {
            output.push_str(value);
        }
        cursor = content_end;
    }
    output
}

fn find_shared_context_secret_name(text: &str) -> Option<(usize, &'static str)> {
    let upper = text.to_ascii_uppercase();
    SHARED_CONTEXT_SECRET_NAMES
        .iter()
        .filter_map(|name| {
            let upper_name = name.to_ascii_uppercase();
            let start = upper.find(&upper_name)?;
            let before_is_boundary = start == 0
                || !upper.as_bytes()[start - 1].is_ascii_alphanumeric()
                    && upper.as_bytes()[start - 1] != b'_';
            let end = start + name.len();
            let after_is_boundary = end == upper.len()
                || !upper.as_bytes()[end].is_ascii_alphanumeric() && upper.as_bytes()[end] != b'_';
            (before_is_boundary && after_is_boundary).then_some((start, *name))
        })
        .min_by_key(|(start, _)| *start)
}

fn shared_context_secret_value_needs_redaction(value: &str) -> bool {
    let trimmed = value.trim().trim_matches(['"', '\'']);
    if SHARED_CONTEXT_SAFE_SECRET_VALUES
        .iter()
        .any(|safe| trimmed.eq_ignore_ascii_case(safe))
        || trimmed.starts_with(['$', '%', '<', '[', '`'])
    {
        return false;
    }
    let compact = trimmed
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .collect::<String>();
    compact.len() >= 16
}

/// Scan rendered shared-context Markdown for `## ` headers and produce
/// `PrefixSection` records with byte ranges [header_start, next_header_start).
fn prefix_sections_from_rendered(text: &str) -> Vec<PrefixSection> {
    let bytes = text.as_bytes();
    let mut header_starts: Vec<(usize, String)> = Vec::new();
    let mut i = 0;
    while i + 3 <= bytes.len() {
        // A `## ` header at the start of a line (line start or after `\n`).
        if bytes[i] == b'#'
            && bytes[i + 1] == b'#'
            && bytes[i + 2] == b' '
            && (i == 0 || bytes[i - 1] == b'\n')
        {
            let line_end = bytes[i..]
                .iter()
                .position(|&b| b == b'\n')
                .map(|p| i + p)
                .unwrap_or(bytes.len());
            let name = String::from_utf8_lossy(&bytes[i + 3..line_end])
                .trim()
                .to_owned();
            header_starts.push((i, name));
            i = line_end + 1;
        } else {
            i += 1;
        }
    }
    header_starts
        .iter()
        .enumerate()
        .map(|(idx, (start, name))| {
            let byte_end = header_starts
                .get(idx + 1)
                .map(|(s, _)| *s)
                .unwrap_or(bytes.len());
            PrefixSection {
                name: name.clone(),
                byte_start: *start,
                byte_end,
            }
        })
        .collect()
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

/// Write `review/shared-prefix-manifest.json` (Order 6 of #678): the byte-stable
/// shared-prefix contract for a cohort. Records the hash, byte length, base/head
/// identity, the ordered source sections composing the prefix (with byte
/// ranges), the cache policy, and truncations. Makes the cohort's
/// cache-coherence claim (one immutable prefix, byte-stable across lanes)
/// inspectable rather than assumed.
pub(crate) fn write_shared_prefix_manifest(
    review_dir: &Path,
    hash: &str,
    byte_length: usize,
    diff: &DiffContext,
    sections: &[PrefixSection],
    args: &RunArgs,
) -> Result<()> {
    let cache_mode = model_cache_mode_for_args(args, "minimax", "anthropic-messages");
    let manifest = SharedPrefixManifest {
        schema: SHARED_PREFIX_MANIFEST_SCHEMA.to_owned(),
        hash: hash.to_owned(),
        byte_length,
        base: diff.base.clone(),
        head: diff.head.clone(),
        ordered_source_sections: sections.to_vec(),
        cache_policy: SharedPrefixCachePolicy {
            provider: "minimax".to_owned(),
            endpoint_kind: "anthropic-messages".to_owned(),
            mode: cache_mode.to_owned(),
            lifetime: "provider-ephemeral".to_owned(),
        },
        truncations: Vec::new(),
    };
    fs::write(
        review_dir.join("shared-prefix-manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shared_context_redacts_credential_like_assignments_but_keeps_placeholders() {
        let source = concat!(
            "OPENCODE=opencodeSecret123456\n",
            "FACTORY_API_KEY=abcdefghijklmnop\n",
            "github_token=1234567890123456\n",
            "UB_REVIEW_OPENCODE_API_KEY=${{ secrets.OPENCODE }}\n",
            "MINIMAX_API_KEY=present\n",
        );
        let redacted = redact_shared_context_secret_assignments(source);
        assert!(!redacted.contains("opencodeSecret123456"));
        assert!(!redacted.contains("abcdefghijklmnop"));
        assert!(!redacted.contains("1234567890123456"));
        assert!(redacted.contains("OPENCODE=[redacted]"));
        assert!(redacted.contains("${{ secrets.OPENCODE }}"));
        assert!(redacted.contains("MINIMAX_API_KEY=present"));
    }

    #[test]
    fn prefix_sections_scan_finds_headers_with_byte_ranges() {
        let text = "# Shared UB Review Context\n\nintro\n\n## PR Summary\n\nbody1\n\n## Diff Summary\n\nbody2\n";
        let sections = prefix_sections_from_rendered(text);
        assert!(
            sections.len() >= 2,
            "should find >=2 sections, got {:?}",
            sections
        );
        let names: Vec<&str> = sections.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"PR Summary"), "names: {names:?}");
        assert!(names.contains(&"Diff Summary"), "names: {names:?}");
        // Byte ranges are contiguous and cover the full text.
        for s in &sections {
            assert!(s.byte_end > s.byte_start, "non-empty range for {}", s.name);
            assert!(s.byte_end <= text.len());
        }
    }

    #[test]
    fn prefix_sections_handle_no_headers() {
        let text = "no headers here at all\n";
        let sections = prefix_sections_from_rendered(text);
        assert!(sections.is_empty());
    }

    /// The manifest's hash must equal the shared_context hash, and byte_length
    /// must equal the prefix length — the cache-coherence contract.
    #[test]
    fn prefix_manifest_hash_and_length_match_prefix() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let review_dir = temp.path().join("review");
        fs::create_dir_all(&review_dir)?;
        let prefix = "## PR Summary\n\nbody\n\n## Diff Summary\n\nbody2\n";
        let hash = sha256_hex(prefix.as_bytes());
        let diff = DiffContext {
            base: "abc".to_owned(),
            head: "def".to_owned(),
            changed_files: vec![],
            patch: String::new(),
            flags: DiffFlags::default(),
            diff_class: crate::DiffClass::SourceGeneral,
        };
        let sections = prefix_sections_from_rendered(prefix);
        let args = crate::tests::test_run_args(temp.path().to_path_buf());
        write_shared_prefix_manifest(&review_dir, &hash, prefix.len(), &diff, &sections, &args)?;
        let manifest_bytes = fs::read(review_dir.join("shared-prefix-manifest.json"))?;
        let manifest: SharedPrefixManifest = serde_json::from_slice(&manifest_bytes)?;
        assert_eq!(manifest.hash, hash);
        assert_eq!(manifest.byte_length, prefix.len());
        assert_eq!(manifest.base, "abc");
        assert_eq!(manifest.head, "def");
        assert!(!manifest.ordered_source_sections.is_empty());
        assert_eq!(manifest.schema, "ub-review.shared_prefix_manifest.v1");
        Ok(())
    }
}
