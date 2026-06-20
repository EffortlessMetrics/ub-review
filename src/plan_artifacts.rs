//! Plan artifact writers and resolved profile artifact construction
//! (cleanup train step 59, pure code motion).

use crate::*;

pub(crate) fn prepare_plan(
    args: &ReviewArgs,
    allow_heavy: bool,
    selectors: &SelectorArgs,
) -> Result<(Config, DiffContext, BoxState, Plan)> {
    validate_selector_syntax(selectors)?;
    let config = Config::load_or_default(
        &args.config,
        runtime_profile_override(args.profile.as_ref(), args.runtime_profile.as_ref()),
    )?;
    let profile = config.selected_profile()?;
    let box_state = BoxState::detect()?;
    let diff = DiffContext::from_git(&args.root, &args.base, &args.head)?;
    let mut plan = build_plan(&config, profile, &box_state, &diff, &args.root, allow_heavy);
    apply_plan_selectors(&mut plan, selectors)?;
    Ok((config, diff, box_state, plan))
}

pub(crate) fn write_plan_artifacts(
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    box_state: &BoxState,
    plan: &Plan,
    selectors: PlanArtifactSelectors<'_>,
) -> Result<()> {
    fs::create_dir_all(out.join("input"))?;
    let profile = config.selected_profile()?;
    fs::write(out.join("plan.json"), serde_json::to_vec_pretty(plan)?)?;
    fs::write(
        out.join("effective-config.json"),
        serde_json::to_vec_pretty(config)?,
    )?;
    fs::write(
        out.join("resolved-profile.json"),
        serde_json::to_vec_pretty(&resolved_profile_artifact(config, profile))?,
    )?;
    fs::write(
        out.join("resolved-plan.json"),
        serde_json::to_vec_pretty(&resolved_plan_artifact(
            config,
            profile,
            diff,
            plan,
            selectors.run_args,
            selectors.selectors,
            selectors.effective_model_lanes,
        ))?,
    )?;
    write_resolved_tools_artifacts(out, config, profile, plan)?;
    fs::write(
        out.join("box-state.json"),
        serde_json::to_vec_pretty(box_state)?,
    )?;
    fs::write(
        out.join("input/diff-context.json"),
        serde_json::to_vec_pretty(diff)?,
    )?;
    fs::write(
        out.join("input/changed-files.txt"),
        diff.changed_files.join("\n"),
    )?;
    fs::write(out.join("input/diff.patch"), &diff.patch)?;
    fs::write(out.join("input/pr.md"), render_pr_packet(diff))?;
    fs::write(out.join("input/claims.md"), render_claim_prompt(diff))?;
    Ok(())
}

pub(crate) struct PlanArtifactSelectors<'a> {
    pub(crate) run_args: Option<&'a RunArgs>,
    pub(crate) selectors: &'a SelectorArgs,
    pub(crate) effective_model_lanes: Option<&'a [LanePlan]>,
}

pub(crate) fn resolved_profile_artifact(config: &Config, profile: &Profile) -> serde_json::Value {
    serde_json::json!({
        "schema": RESOLVED_PROFILE_SCHEMA,
        "selected_profile": &profile.name,
        "selected_review_profile": &config.review_profile,
        "selected_runtime_profile": &profile.name,
        "repo": &config.repo,
        "review": &config.review,
        "review_body": &config.review_body,
        "gate": &config.gate,
        "proof": &config.proof,
        "review_profile": {
            "name": &config.review_profile,
            "repo_kind": &config.repo.kind,
            "default_lanes_enabled": config.review.enable_default_lanes,
            "posting_engine": &config.review.posting_engine,
        },
        "profile": profile,
        "tools": &config.tools,
    })
}

/// One entry from a bucket in `repair-queue.json` (schema_version `"0.1"`).
///
/// Validated against real `unsafe-review 0.3.4` output. The repair queue
/// classifies each card into one of several buckets (`repairable_by_guard`,
/// `requires_witness_receipt`, `requires_human_review`, …) with per-entry
/// fields for routing and missing-evidence description.
///
/// **Honest capability assessment**: the repair queue provides guidance — it
/// names the missing evidence and classifies the repair class — but does NOT
/// supply a concrete replacement text that could power a one-click GitHub
/// suggestion block. Fields like `operation` (the unsafe expression as-is),
/// `missing_evidence` (why it lacks a guard), and `do_not_do` (negative
/// constraints) are present; a `replacement` / `new_text` / `suggestion_text`
/// field is absent. Suggestion blocks therefore cannot be emitted from this
/// source without fabricating edits. See the narrow follow-up issue:
/// "repair-queue should emit applicable edits for suggestion blocks".
///
/// That assessment is for observed `repair-queue/0.1` output. The optional
/// fields below are forward-compatible producer fields and are used only when
/// the tool supplies concrete replacement text.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RepairQueueEntry {
    pub(crate) card_id: String,
    /// Bucket reason explains why this entry landed in its bucket.
    #[serde(default)]
    pub(crate) bucket_reason: Option<String>,
    /// The unsafe operation text (read-only context; not a replacement).
    #[serde(default)]
    pub(crate) operation: Option<String>,
    /// Missing evidence items (prose guidance; not a diff suggestion).
    #[serde(default)]
    pub(crate) missing_evidence: Vec<String>,
    /// Future producer fields for a concrete replacement. Current
    /// unsafe-review 0.3.4 output does not emit these.
    #[serde(default)]
    pub(crate) replacement: Option<String>,
    #[serde(default)]
    pub(crate) replacement_text: Option<String>,
    #[serde(default)]
    pub(crate) new_text: Option<String>,
    #[serde(default)]
    pub(crate) suggestion_text: Option<String>,
    #[serde(default)]
    pub(crate) applicable_edit: Option<RepairQueueApplicableEdit>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RepairQueueApplicableEdit {
    #[serde(default)]
    replacement: Option<String>,
    #[serde(default)]
    replacement_text: Option<String>,
    #[serde(default)]
    new_text: Option<String>,
    #[serde(default)]
    suggestion_text: Option<String>,
}

impl RepairQueueApplicableEdit {
    pub(crate) fn suggestion(&self) -> Option<String> {
        [
            self.suggestion_text.as_deref(),
            self.replacement_text.as_deref(),
            self.new_text.as_deref(),
            self.replacement.as_deref(),
        ]
        .into_iter()
        .find_map(normalize_github_suggestion_text)
    }
}

impl RepairQueueEntry {
    pub(crate) fn suggestion(&self) -> Option<String> {
        [
            self.suggestion_text.as_deref(),
            self.replacement_text.as_deref(),
            self.new_text.as_deref(),
            self.replacement.as_deref(),
        ]
        .into_iter()
        .find_map(normalize_github_suggestion_text)
        .or_else(|| {
            self.applicable_edit
                .as_ref()
                .and_then(|edit| edit.suggestion())
        })
    }
}

/// Top-level shape of `repair-queue.json` (schema_version `"0.1"`).
///
/// Only the `buckets` map is consumed here; all other top-level fields are
/// silently tolerated so forward-compatible additions do not break ingestion.
#[derive(Debug, Deserialize)]
pub(crate) struct RepairQueueFile {
    /// All bucket names map to lists of `RepairQueueEntry`. Known keys:
    /// `repairable_by_guard`, `repairable_by_safety_docs`, `repairable_by_test`,
    /// `requires_witness_receipt`, `requires_human_review`, `do_not_auto_repair`.
    #[serde(default)]
    pub(crate) buckets: std::collections::BTreeMap<String, Vec<RepairQueueEntry>>,
}
