//! Plan artifact writers and resolved profile artifact construction
//! (cleanup train step 59, pure code motion).

use crate::*;

pub(crate) fn prepare_plan(
    args: &ReviewArgs,
    allow_heavy: bool,
    selectors: &SelectorArgs,
) -> Result<(
    Config,
    DiffContext,
    BoxState,
    Plan,
    Option<TrustedDiffAdmission>,
)> {
    validate_selector_syntax(selectors)?;
    let trusted_mode = trusted_admission_complete(args)?;
    let trusted_admission = if trusted_mode {
        let admission = TrustedDiffAdmission::load(
            &args.root,
            args.trusted_base_tree
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("trusted base tree missing"))?,
            args.trusted_head
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("trusted head missing"))?,
            args.trusted_changed_files
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("trusted changed-files object missing"))?,
            args.trusted_patch
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("trusted patch object missing"))?,
        )?;
        reject_repo_controlled_config(args)?;
        Some(admission)
    } else {
        None
    };
    let trusted_diff = trusted_admission
        .as_ref()
        .map(DiffContext::from_trusted_admission);
    let config = Config::load_or_default(
        &args.config,
        runtime_profile_override(args.profile.as_ref(), args.runtime_profile.as_ref()),
    )?;
    let profile = config.selected_profile()?;
    let box_state = BoxState::detect()?;
    let diff = if let Some(diff) = trusted_diff {
        diff
    } else {
        DiffContext::from_git(&args.root, &args.base, &args.head)?
    };
    let mut plan = build_plan(&config, profile, &box_state, &diff, &args.root, allow_heavy);
    apply_plan_selectors(&mut plan, selectors)?;
    Ok((config, diff, box_state, plan, trusted_admission))
}

pub(crate) fn trusted_admission_requested(args: &ReviewArgs) -> bool {
    args.trusted_base_tree.is_some()
        || args.trusted_head.is_some()
        || args.trusted_changed_files.is_some()
        || args.trusted_patch.is_some()
}

pub(crate) fn trusted_admission_complete(args: &ReviewArgs) -> Result<bool> {
    if !trusted_admission_requested(args) {
        return Ok(false);
    }
    if args
        .trusted_base_tree
        .as_deref()
        .is_none_or(|value| value.trim().is_empty())
        || args
            .trusted_head
            .as_deref()
            .is_none_or(|value| value.trim().is_empty())
        || args
            .trusted_changed_files
            .as_deref()
            .is_none_or(|path| path.as_os_str().is_empty())
        || args
            .trusted_patch
            .as_deref()
            .is_none_or(|path| path.as_os_str().is_empty())
    {
        bail!("trusted-base admission requires all four nonempty explicit inputs");
    }
    validate_git_object_id(
        args.trusted_base_tree.as_deref().unwrap_or_default(),
        "trusted base tree",
    )?;
    validate_git_object_id(
        args.trusted_head.as_deref().unwrap_or_default(),
        "trusted head",
    )?;
    Ok(true)
}

pub(crate) fn validate_trusted_execution_settings(
    trusted: bool,
    model_mode: ModelMode,
    provider_policy: ModelProviderPolicy,
) -> Result<()> {
    if !trusted {
        return Ok(());
    }
    if !matches!(model_mode, ModelMode::Off) {
        bail!("trusted-base mode requires --model-mode off; model execution is disabled");
    }
    if !matches!(provider_policy, ModelProviderPolicy::Auto) {
        bail!("trusted-base mode requires --provider-policy auto");
    }
    Ok(())
}

fn reject_repo_controlled_config(args: &ReviewArgs) -> Result<()> {
    let root = fs::canonicalize(&args.root).with_context(|| {
        format!(
            "canonicalize trusted repository root {}",
            args.root.display()
        )
    })?;
    validate_trusted_config_path(&root, &args.config)
}

pub(crate) fn validate_trusted_config_path(root: &Path, config: &Path) -> Result<()> {
    if !config.is_absolute() {
        bail!("trusted-base admission rejects repository-relative config paths");
    }
    if config
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
        || config
            .to_string_lossy()
            .split(['/', '\\'])
            .any(|component| component == "..")
    {
        bail!("trusted-base admission rejects lexical parent traversal in config path");
    }
    if config.starts_with(root) {
        bail!("trusted-base admission rejects configuration controlled by the repository");
    }
    let canonical_config = fs::canonicalize(config).with_context(|| {
        format!(
            "canonicalize trusted configuration path {}",
            config.display()
        )
    })?;
    if canonical_config.starts_with(root) {
        bail!("trusted-base admission rejects configuration reached through a repository symlink");
    }
    Ok(())
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
    if let Some(admission) = selectors.trusted_admission {
        admission.verify_root()?;
        if admission.changed_files != diff.changed_files || admission.patch != diff.patch {
            bail!("trusted-base receipt rejected a diff outside the admitted objects");
        }
        fs::write(
            out.join("input/trusted-base-receipt.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema_version": 1,
                "admission": "trusted-base-explicit-diff",
                "base_tree_sha": &admission.base_tree_sha,
                "observed_checkout_tree_sha": &admission.observed_checkout_tree_sha,
                "head_sha": &admission.head_sha,
                "changed_files_object_sha256": &admission.changed_files_object_sha256,
                "patch_object_sha256": &admission.patch_object_sha256,
                "changed_files_sha256": &admission.changed_files_sha256,
                "patch_sha256": &admission.patch_sha256,
                "head_tree_loaded": false,
                "head_config_loaded": false,
                "repository_config_loaded": false,
                "model_mode": selectors.run_args.map(|args| args.model_mode.key()),
                "provider_policy": selectors.run_args.map(|args| args.provider_policy.key()),
            }))?,
        )?;
    }
    fs::write(out.join("input/pr.md"), render_pr_packet(diff))?;
    fs::write(out.join("input/claims.md"), render_claim_prompt(diff))?;
    Ok(())
}

pub(crate) struct PlanArtifactSelectors<'a> {
    pub(crate) run_args: Option<&'a RunArgs>,
    pub(crate) selectors: &'a SelectorArgs,
    pub(crate) effective_model_lanes: Option<&'a [LanePlan]>,
    pub(crate) trusted_admission: Option<&'a TrustedDiffAdmission>,
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

#[cfg(test)]
mod trusted_config_tests {
    use super::*;

    fn review_args() -> ReviewArgs {
        ReviewArgs {
            root: PathBuf::from("."),
            base: "origin/main".to_owned(),
            head: "HEAD".to_owned(),
            trusted_base_tree: None,
            trusted_head: None,
            trusted_changed_files: None,
            trusted_patch: None,
            config: PathBuf::from(".ub-review.toml"),
            out: PathBuf::from("target/ub-review"),
            profile: None,
            runtime_profile: None,
        }
    }

    #[test]
    fn trusted_admission_requires_all_four_nonempty_inputs() -> Result<()> {
        let mut args = review_args();
        if trusted_admission_complete(&args)? {
            bail!("ordinary review args unexpectedly requested trusted admission");
        }
        args.trusted_base_tree = Some("a".repeat(40));
        if trusted_admission_complete(&args).is_ok() {
            bail!("accepted partial trusted admission inputs");
        }
        args.trusted_head = Some("b".repeat(40));
        args.trusted_changed_files = Some(PathBuf::from("changed-files.txt"));
        args.trusted_patch = Some(PathBuf::from("patch.diff"));
        if !trusted_admission_complete(&args)? {
            bail!("complete trusted admission inputs were not recognized");
        }
        args.trusted_head = Some(String::new());
        if trusted_admission_complete(&args).is_ok() {
            bail!("accepted an empty trusted head");
        }
        Ok(())
    }

    #[test]
    fn trusted_config_rejects_relative_and_repository_paths() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root_dir = temp.path().join("root");
        fs::create_dir_all(&root_dir)?;
        let root = root_dir.canonicalize()?;
        let outside = temp.path().join("outside.toml");
        fs::write(&outside, "[providers]\npolicy='auto'\n")?;
        validate_trusted_config_path(&root, &outside)?;
        if validate_trusted_config_path(&root, Path::new(".ub-review.toml")).is_ok() {
            bail!("accepted relative repository config path");
        }
        let repository_config = root.join("malicious.toml");
        fs::write(&repository_config, "[providers]\npolicy='minimax-only'\n")?;
        if validate_trusted_config_path(&root, &repository_config).is_ok() {
            bail!("accepted repository-controlled config path");
        }
        let parent_path = PathBuf::from(format!("{}\\..\\outside.toml", root.display()));
        if validate_trusted_config_path(&root, &parent_path).is_ok() {
            bail!("accepted lexical parent traversal config path");
        }
        let link = root.join("linked-config.toml");
        #[cfg(unix)]
        std::os::unix::fs::symlink(&outside, &link)?;
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(&outside, &link).is_err() {
            return Ok(());
        }
        if validate_trusted_config_path(&root, &link).is_ok() {
            bail!("accepted repository symlink to configuration");
        }
        Ok(())
    }

    #[test]
    fn trusted_execution_rejects_model_or_provider_overrides() -> Result<()> {
        validate_trusted_execution_settings(true, ModelMode::Off, ModelProviderPolicy::Auto)?;
        if validate_trusted_execution_settings(true, ModelMode::Auto, ModelProviderPolicy::Auto)
            .is_ok()
        {
            bail!("accepted trusted model execution");
        }
        if validate_trusted_execution_settings(
            true,
            ModelMode::Off,
            ModelProviderPolicy::MinimaxOnly,
        )
        .is_ok()
        {
            bail!("accepted trusted provider override");
        }
        Ok(())
    }
}
