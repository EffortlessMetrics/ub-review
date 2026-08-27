//! Plan artifact writers and resolved profile artifact construction
//! (cleanup train step 59, pure code motion).

use crate::*;

pub(crate) fn prepare_plan(
    args: &ReviewArgs,
    allow_heavy: bool,
    selectors: &SelectorArgs,
) -> Result<(Config, DiffContext, BoxState, Plan, RevisionAdmission)> {
    validate_selector_syntax(selectors)?;
    // Trusted admission must validate the clean base root and explicit diff
    // objects before any repository config is read. A dirty or mismatched
    // root must not get an opportunity to influence configuration parsing.
    let trusted_admission = trusted_diff_inputs(args)?
        .map(|inputs| admit_trusted_diff(&args.root, &inputs))
        .transpose()?;
    let config = Config::load_or_default(
        &args.config,
        runtime_profile_override(args.profile.as_ref(), args.runtime_profile.as_ref()),
    )?;
    let profile = config.selected_profile()?;
    let box_state = BoxState::detect()?;
    let (diff, revision) = if let Some(admission) = trusted_admission {
        admission
    } else {
        let diff = DiffContext::from_git(&args.root, &args.base, &args.head)?;
        // A1.2 (#949): admit the exact reviewed revision next to the diff so the
        // digests bind to the same changed-file set and patch. Ambiguity here is
        // an explicit evidence failure, never a relabeled identity. Hosted
        // non-PR events render the workflow expression as an empty string, which
        // means "no metadata" exactly like an unset variable.
        let pr_head_sha = args
            .pr_head_sha
            .as_deref()
            .map(str::trim)
            .filter(|sha| !sha.is_empty());
        let revision = admit_revision(
            &args.root,
            &args.base,
            &args.head,
            pr_head_sha,
            &diff.changed_files,
            &diff.patch,
        )?;
        (diff, revision)
    };
    revision.validate()?;
    let mut plan = build_plan(&config, profile, &box_state, &diff, &args.root, allow_heavy);
    apply_plan_selectors(&mut plan, selectors)?;
    Ok((config, diff, box_state, plan, revision))
}

pub(crate) fn trusted_diff_inputs(args: &ReviewArgs) -> Result<Option<TrustedDiffInputs>> {
    match (
        &args.trusted_base_tree,
        &args.trusted_head_sha,
        &args.trusted_changed_files,
        &args.trusted_diff_patch,
    ) {
        (None, None, None, None) => Ok(None),
        (Some(base_tree), Some(head_sha), Some(changed_files), Some(diff_patch)) => {
            Ok(Some(TrustedDiffInputs {
                base_tree: base_tree.clone(),
                head_sha: head_sha.clone(),
                changed_files: changed_files.clone(),
                diff_patch: diff_patch.clone(),
                pr_head_sha: args.pr_head_sha.clone(),
            }))
        }
        _ => bail!(
            "trusted-base diff admission requires --trusted-base-tree, --trusted-head-sha, --trusted-changed-files, and --trusted-diff-patch together"
        ),
    }
}

pub(crate) fn write_plan_artifacts(
    out: &Path,
    config: &Config,
    diff: &DiffContext,
    box_state: &BoxState,
    plan: &Plan,
    revision: Option<&RevisionAdmission>,
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
    if let Some(admission) = revision {
        fs::write(
            out.join("input/revision-admission.json"),
            serde_json::to_vec_pretty(admission)?,
        )?;
    }
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
    /// Opaque unsafe-review operation family.  Repair joins are keyed by both
    /// card ID and family so a reused card ID cannot borrow another family's
    /// guidance.
    pub(crate) operation_family: String,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trusted_diff_inputs_are_absent_or_complete() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let mut review = crate::tests::test_run_args(temp.path().join("out")).review;
        assert!(trusted_diff_inputs(&review)?.is_none());

        review.trusted_base_tree = Some("a".repeat(40));
        let Err(error) = trusted_diff_inputs(&review) else {
            bail!("partial trusted-diff inputs must fail closed");
        };
        assert!(error.to_string().contains("requires --trusted-base-tree"));

        review.trusted_head_sha = Some("b".repeat(40));
        review.trusted_changed_files = Some(temp.path().join("changed-files.txt"));
        review.trusted_diff_patch = Some(temp.path().join("diff.patch"));
        let inputs = trusted_diff_inputs(&review)?
            .ok_or_else(|| anyhow::anyhow!("complete trusted-diff inputs were not admitted"))?;
        assert_eq!(inputs.base_tree, "a".repeat(40));
        assert_eq!(inputs.head_sha, "b".repeat(40));
        Ok(())
    }
}
