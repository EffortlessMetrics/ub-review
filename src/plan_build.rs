//! Plan construction: resolved plan/selector artifacts and plan
//! building from config + args (cleanup train step 32, pure code motion).

use crate::diff_posture::default_lanes_for_diff_context;
use crate::*;

pub(crate) fn resolved_plan_artifact(
    config: &Config,
    profile: &Profile,
    diff: &DiffContext,
    plan: &Plan,
    run_args: Option<&RunArgs>,
    selectors: &SelectorArgs,
    effective_model_lanes: Option<&[LanePlan]>,
) -> serde_json::Value {
    let run_pass = run_args
        .map(|args| resolved_run_pass(args.run_pass).key())
        .unwrap_or("plan-default");
    serde_json::json!({
        "schema": RESOLVED_PLAN_SCHEMA,
        "base": &plan.base,
        "head": &plan.head,
        "run_pass": run_pass,
        "diff_class": diff.diff_class.key(),
        "language_mix": &plan.language_mix,
        "proof_policy": resolved_proof_policy_artifact(config, diff, &plan.language_mix),
        "review_profile": &config.review_profile,
        "profile_name": &plan.profile_name,
        "runtime_profile": &profile.name,
        "budgets": &profile.budgets,
        "trusted_repo": &profile.trusted_repo,
        "guards": &profile.guards,
        "limits": &profile.limits,
        "posting": &config.review,
        "review_body": &config.review_body,
        "gate": &config.gate,
        "selectors": resolved_selector_artifact(run_args, selectors, effective_model_lanes),
        "sensors": &plan.sensors,
        "lanes": &plan.lanes,
        "notes": &plan.notes,
    })
}

pub(crate) fn resolved_selector_artifact(
    run_args: Option<&RunArgs>,
    selectors: &SelectorArgs,
    effective_model_lanes: Option<&[LanePlan]>,
) -> serde_json::Value {
    let lane_include = selector_values_or_empty(&selectors.lanes);
    let lane_exclude = selector_values_or_empty(&selectors.except_lanes);
    let tool_include = selector_values_or_empty(&selectors.tools);
    let tool_exclude = selector_values_or_empty(&selectors.except_tools);
    let effective_lanes = effective_model_lanes
        .map(|lanes| lanes.iter().map(|lane| lane.id.clone()).collect::<Vec<_>>())
        .unwrap_or_default();
    if let Some(args) = run_args {
        serde_json::json!({
            "run_pass": resolved_run_pass(args.run_pass).key(),
            "depth": args.depth.key(),
            "lane_width": args.lane_width,
            "model_concurrency": args.model_concurrency,
            "max_model_calls": args.max_model_calls,
            "max_inline_comments": args.max_inline_comments,
            "lanes": lane_include,
            "except_lanes": lane_exclude,
            "tools": tool_include,
            "except_tools": tool_exclude,
            "effective_model_lanes": effective_lanes,
        })
    } else {
        serde_json::json!({
            "run_pass": "plan-default",
            "depth": ReviewDepth::Standard.key(),
            "lane_width": STANDARD_LANE_WIDTH,
            "model_concurrency": STANDARD_MODEL_CONCURRENCY,
            "max_model_calls": STANDARD_MAX_MODEL_CALLS,
            "max_inline_comments": 8,
            "lanes": lane_include,
            "except_lanes": lane_exclude,
            "tools": tool_include,
            "except_tools": tool_exclude,
            "effective_model_lanes": effective_lanes,
            "source": "plan-default",
        })
    }
}

pub(crate) fn validate_selector_syntax(selectors: &SelectorArgs) -> Result<()> {
    parse_selector_set(&selectors.lanes, "--lanes")?;
    parse_selector_set(&selectors.except_lanes, "--except-lanes")?;
    parse_selector_set(&selectors.tools, "--tools")?;
    parse_selector_set(&selectors.except_tools, "--except-tools")?;
    Ok(())
}

pub(crate) fn selector_values_or_empty(value: &str) -> Vec<String> {
    let mut values = value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    values.sort();
    values.dedup();
    values
}

pub(crate) fn parse_selector_set(value: &str, flag: &str) -> Result<BTreeSet<String>> {
    let mut selected = BTreeSet::new();
    for item in value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
    {
        if !is_selector_id(item) {
            bail!("{flag} contains invalid selector id `{item}`");
        }
        selected.insert(item.to_owned());
    }
    Ok(selected)
}

pub(crate) fn is_selector_id(value: &str) -> bool {
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
}

pub(crate) fn apply_plan_selectors(plan: &mut Plan, selectors: &SelectorArgs) -> Result<()> {
    let tool_include = parse_selector_set(&selectors.tools, "--tools")?;
    let tool_exclude = parse_selector_set(&selectors.except_tools, "--except-tools")?;
    if !tool_include.is_empty() || !tool_exclude.is_empty() {
        plan.sensors = filter_sensor_plans(
            std::mem::take(&mut plan.sensors),
            &tool_include,
            &tool_exclude,
        )?;
        plan.notes.push(format!(
            "tool selectors applied: tools=[{}] except-tools=[{}]",
            tool_include
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(","),
            tool_exclude
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    let lane_include = parse_selector_set(&selectors.lanes, "--lanes")?;
    let lane_exclude = parse_selector_set(&selectors.except_lanes, "--except-lanes")?;
    if !lane_include.is_empty() || !lane_exclude.is_empty() {
        plan.notes.push(format!(
            "lane selectors will filter model assignments: lanes=[{}] except-lanes=[{}]",
            lane_include
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(","),
            lane_exclude
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    Ok(())
}

pub(crate) fn filter_sensor_plans(
    sensors: Vec<SensorPlan>,
    include: &BTreeSet<String>,
    exclude: &BTreeSet<String>,
) -> Result<Vec<SensorPlan>> {
    validate_known_selectors(
        "tool",
        sensors.iter().map(|sensor| sensor.id.as_str()),
        include,
    )?;
    validate_known_selectors(
        "tool",
        sensors.iter().map(|sensor| sensor.id.as_str()),
        exclude,
    )?;
    Ok(sensors
        .into_iter()
        .filter(|sensor| include.is_empty() || include.contains(&sensor.id))
        .filter(|sensor| !exclude.contains(&sensor.id))
        .collect())
}

pub(crate) fn filter_lane_plans(
    lanes: Vec<LanePlan>,
    include: &BTreeSet<String>,
    exclude: &BTreeSet<String>,
) -> Result<Vec<LanePlan>> {
    validate_known_selectors("lane", lanes.iter().map(|lane| lane.id.as_str()), include)?;
    validate_known_selectors("lane", lanes.iter().map(|lane| lane.id.as_str()), exclude)?;
    Ok(lanes
        .into_iter()
        .filter(|lane| include.is_empty() || include.contains(&lane.id))
        .filter(|lane| !exclude.contains(&lane.id))
        .collect())
}

pub(crate) fn validate_known_selectors<'a>(
    kind: &str,
    available: impl Iterator<Item = &'a str>,
    selected: &BTreeSet<String>,
) -> Result<()> {
    if selected.is_empty() {
        return Ok(());
    }
    let available = available.collect::<BTreeSet<_>>();
    let unknown = selected
        .iter()
        .filter(|item| !available.contains(item.as_str()))
        .cloned()
        .collect::<Vec<_>>();
    if !unknown.is_empty() {
        bail!(
            "unknown {kind} selector(s): {}; available: {}",
            unknown.join(","),
            available.into_iter().collect::<Vec<_>>().join(",")
        );
    }
    Ok(())
}

pub(crate) fn print_plan(plan: &Plan, box_state: &BoxState) {
    println!("Profile: {}", plan.profile_name);
    println!("Diff class: {}", plan.diff_class.key());
    println!("Box: {}", box_state.summary_line());
    println!("Sensors:");
    for sensor in &plan.sensors {
        let marker = if sensor.run { "run" } else { "skip" };
        println!("  {:<5} {:<16} {}", marker, sensor.id, sensor.reason);
    }
    println!("Lanes:");
    for lane in &plan.lanes {
        println!("  {:<13} {}", lane.id, lane.model_display);
    }
}

impl DiffContext {
    pub(crate) fn from_git(root: &Path, base: &str, head: &str) -> Result<Self> {
        let range = format!("{base}...{head}");
        let changed_files = git_lines(root, &["diff", "--name-only", &range])
            .or_else(|_| git_lines(root, &["diff", "--name-only", base, head]))
            .with_context(|| format!("git diff --name-only {range}"))?;
        let patch = git_text(root, &["diff", "--patch", &range])
            .or_else(|_| git_text(root, &["diff", "--patch", base, head]))
            .unwrap_or_else(|_| String::new());
        let flags = classify_diff(&changed_files, &patch);
        let diff_class = classify_diff_class(&changed_files, &flags);
        Ok(Self {
            base: base.to_owned(),
            head: head.to_owned(),
            changed_files,
            patch,
            flags,
            diff_class,
        })
    }

    /// Build a diff from a previously validated admission. This function has
    /// no filesystem or Git inputs: the head remains metadata and cannot be
    /// resolved as a ref, tree, configuration, plugin, script, or library.
    pub(crate) fn from_trusted_admission(admission: &TrustedDiffAdmission) -> Self {
        let changed_files = admission.changed_files.clone();
        let patch = admission.patch.clone();
        let flags = classify_diff(&changed_files, &patch);
        let diff_class = classify_diff_class(&changed_files, &flags);
        Self {
            base: admission.base_tree_sha.clone(),
            head: admission.head_sha.clone(),
            changed_files,
            patch,
            flags,
            diff_class,
        }
    }
}

/// Immutable result of validating trusted-base explicit diff inputs. All
/// filesystem reads happen while constructing this value, before config or
/// `DiffContext` use. Receipts reuse these exact admitted bytes and digests.
#[derive(Clone, Debug)]
pub(crate) struct TrustedDiffAdmission {
    trusted_root: PathBuf,
    trusted_out: PathBuf,
    pub(crate) base_tree_sha: String,
    pub(crate) head_sha: String,
    pub(crate) observed_checkout_tree_sha: String,
    pub(crate) changed_files: Vec<String>,
    pub(crate) patch: String,
    pub(crate) changed_files_object_sha256: String,
    pub(crate) patch_object_sha256: String,
    pub(crate) changed_files_sha256: String,
    pub(crate) patch_sha256: String,
}

impl TrustedDiffAdmission {
    pub(crate) fn load(
        root: &Path,
        out: &Path,
        base_tree: &str,
        head: &str,
        changed_files_path: &Path,
        patch_path: &Path,
    ) -> Result<Self> {
        validate_git_object_id(base_tree, "trusted base tree")?;
        validate_git_object_id(head, "trusted head")?;

        let root = fs::canonicalize(root)
            .with_context(|| format!("canonicalize trusted repository root {}", root.display()))?;
        let observed_checkout_tree_sha = validate_trusted_root(&root, base_tree)?;
        let trusted_out = validate_trusted_output_path(&root, out)?;
        let changed_files_path =
            validate_trusted_object_path(&root, changed_files_path, "changed-files")?;
        let patch_path = validate_trusted_object_path(&root, patch_path, "patch")?;
        if changed_files_path == patch_path {
            bail!("trusted changed-files and patch objects must be distinct files");
        }

        let changed_files_bytes = fs::read(&changed_files_path).with_context(|| {
            format!(
                "read trusted changed-files object {}",
                changed_files_path.display()
            )
        })?;
        let changed_files = parse_trusted_changed_files(&changed_files_bytes)?;
        let patch_bytes = fs::read(&patch_path)
            .with_context(|| format!("read trusted patch object {}", patch_path.display()))?;
        if patch_bytes.is_empty() || patch_bytes.contains(&0) {
            bail!("trusted patch object must be nonempty and contain no NUL bytes");
        }
        let patch = std::str::from_utf8(&patch_bytes)
            .context("trusted patch object must be valid UTF-8")?
            .to_owned();
        validate_trusted_patch(&root, &patch_bytes, &changed_files)?;

        let observed_after_read = validate_trusted_root(&root, base_tree)?;
        if observed_after_read != observed_checkout_tree_sha {
            bail!("trusted repository tree changed during explicit diff admission");
        }

        Ok(Self {
            trusted_root: root,
            trusted_out,
            base_tree_sha: base_tree.to_owned(),
            head_sha: head.to_owned(),
            observed_checkout_tree_sha,
            changed_files_sha256: sha256_hex(&encode_trusted_changed_files(&changed_files)),
            patch_sha256: sha256_hex(patch.as_bytes()),
            changed_files_object_sha256: sha256_hex(&changed_files_bytes),
            patch_object_sha256: sha256_hex(&patch_bytes),
            changed_files,
            patch,
        })
    }

    pub(crate) fn verify_root(&self) -> Result<()> {
        let observed = validate_trusted_root(&self.trusted_root, &self.base_tree_sha)?;
        if observed != self.observed_checkout_tree_sha {
            bail!("trusted repository tree changed after explicit diff admission");
        }
        Ok(())
    }

    pub(crate) fn verify_output(&self, out: &Path) -> Result<()> {
        let observed = validate_trusted_output_path(&self.trusted_root, out)?;
        if observed != self.trusted_out {
            bail!("trusted output path changed after explicit diff admission");
        }
        Ok(())
    }
}

fn validate_trusted_root(root: &Path, base_tree: &str) -> Result<String> {
    let observed_tree = git_tree_sha(root, "HEAD")?;
    if observed_tree != base_tree {
        bail!(
            "trusted-base admission rejected: checkout tree {observed_tree} does not match base tree {base_tree}"
        );
    }
    let status = git_text(
        root,
        &[
            "status",
            "--porcelain=v1",
            "--untracked-files=all",
            "--ignored=matching",
        ],
    )?;
    if !status.trim().is_empty() {
        bail!("trusted-base admission requires a clean trusted repository root");
    }
    Ok(observed_tree)
}

fn validate_trusted_object_path(root: &Path, path: &Path, label: &str) -> Result<PathBuf> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
        || path
            .to_string_lossy()
            .split(['/', '\\'])
            .any(|component| component == ".." || component == ".")
    {
        bail!("trusted {label} object path must be absolute without lexical traversal");
    }
    if fs::symlink_metadata(path)
        .with_context(|| format!("inspect trusted {label} object {}", path.display()))?
        .file_type()
        .is_symlink()
    {
        bail!("trusted {label} object path must not be a symbolic link");
    }
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("canonicalize trusted {label} object {}", path.display()))?;
    if canonical.starts_with(root) {
        bail!("trusted {label} object must be outside the trusted repository root");
    }
    if !fs::metadata(&canonical)
        .with_context(|| format!("inspect trusted {label} object {}", canonical.display()))?
        .is_file()
    {
        bail!("trusted {label} object must be a regular file");
    }
    Ok(canonical)
}

fn validate_trusted_output_path(root: &Path, out: &Path) -> Result<PathBuf> {
    if !out.is_absolute()
        || out
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::CurDir))
    {
        bail!("trusted output path must be absolute without lexical traversal");
    }
    let observed = if out.exists() {
        if fs::symlink_metadata(out)?.file_type().is_symlink() {
            bail!("trusted output path must not be a symbolic link");
        }
        fs::canonicalize(out)
            .with_context(|| format!("canonicalize trusted output path {}", out.display()))?
    } else {
        let parent = out
            .parent()
            .ok_or_else(|| anyhow::anyhow!("trusted output path has no parent"))?;
        let parent = fs::canonicalize(parent)
            .with_context(|| format!("canonicalize trusted output parent {}", parent.display()))?;
        parent.join(
            out.file_name()
                .ok_or_else(|| anyhow::anyhow!("trusted output path has no final component"))?,
        )
    };
    if observed.starts_with(root) {
        bail!("trusted output path must be outside the trusted repository root");
    }
    Ok(observed)
}

pub(crate) fn validate_git_object_id(value: &str, label: &str) -> Result<()> {
    if value.len() != 40 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("{label} must be exactly 40 hexadecimal characters");
    }
    Ok(())
}

pub(crate) fn parse_trusted_changed_files(bytes: &[u8]) -> Result<Vec<String>> {
    if bytes.is_empty() || !bytes.ends_with(&[0]) {
        bail!("trusted changed-files object must use trailing-NUL path encoding");
    }
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        let file =
            std::str::from_utf8(raw).context("trusted changed-files paths must be valid UTF-8")?;
        if file.is_empty() {
            bail!("trusted changed-files object contains an empty path");
        }
        validate_trusted_changed_path(file)?;
        if !seen.insert(file) {
            bail!("trusted changed-files object contains a duplicate path");
        }
        files.push(file.to_owned());
    }
    if files.is_empty() {
        bail!("trusted changed-files object must contain at least one path");
    }
    Ok(files)
}

fn validate_trusted_changed_path(file: &str) -> Result<()> {
    let path = Path::new(file);
    if path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::CurDir
                    | Component::ParentDir
                    | Component::RootDir
                    | Component::Prefix(_)
            )
        })
        || file
            .split(['/', '\\'])
            .any(|component| component == ".." || component == ".")
    {
        bail!("trusted changed-files object contains an unsafe path");
    }
    Ok(())
}

fn encode_trusted_changed_files(files: &[String]) -> Vec<u8> {
    let mut encoded = Vec::new();
    for file in files {
        encoded.extend_from_slice(file.as_bytes());
        encoded.push(0);
    }
    encoded
}

fn validate_trusted_patch(root: &Path, patch: &[u8], changed_files: &[String]) -> Result<()> {
    git_apply_output(root, &["--check", "--whitespace=nowarn"], patch)
        .context("trusted patch object failed git apply syntax/base validation")?;
    let numstat = git_apply_output(root, &["--numstat", "-z"], patch)
        .context("derive trusted patch changed paths")?;
    let patch_files = parse_git_apply_numstat(&numstat)?;
    let admitted = changed_files.iter().collect::<BTreeSet<_>>();
    let patched = patch_files.iter().collect::<BTreeSet<_>>();
    if admitted.len() != changed_files.len()
        || patched.len() != patch_files.len()
        || admitted != patched
    {
        bail!("trusted changed-files object does not exactly match trusted patch paths");
    }
    Ok(())
}

fn git_apply_output(root: &Path, args: &[&str], patch: &[u8]) -> Result<Vec<u8>> {
    let mut child = ProcessCommand::new("git")
        .arg("-C")
        .arg(root)
        .arg("apply")
        .args(args)
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("start git apply for trusted patch validation")?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| anyhow::anyhow!("open git apply stdin"))?;
    std::io::Write::write_all(&mut stdin, patch).context("write trusted patch to git apply")?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .context("wait for trusted git apply validation")?;
    if !output.status.success() {
        bail!(
            "git apply trusted patch validation failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn parse_git_apply_numstat(bytes: &[u8]) -> Result<Vec<String>> {
    if bytes.is_empty() || !bytes.ends_with(&[0]) {
        bail!("trusted patch did not produce NUL-delimited numstat paths");
    }
    let mut paths = Vec::new();
    for record in bytes[..bytes.len() - 1].split(|byte| *byte == 0) {
        let mut fields = record.splitn(3, |byte| *byte == b'\t');
        let added = fields.next();
        let deleted = fields.next();
        let path = fields.next();
        if added.is_none() || deleted.is_none() || path.is_none_or(|path| path.is_empty()) {
            bail!("trusted patch produced malformed numstat output");
        }
        let path = std::str::from_utf8(path.unwrap_or_default())
            .context("trusted patch path must be valid UTF-8")?;
        validate_trusted_changed_path(path)?;
        paths.push(path.to_owned());
    }
    if paths.is_empty() {
        bail!("trusted patch must change at least one path");
    }
    Ok(paths)
}

pub(crate) fn git_lines(root: &Path, args: &[&str]) -> Result<Vec<String>> {
    Ok(git_text(root, args)?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect())
}

pub(crate) fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| "run git")?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) fn git_text_owned(root: &Path, args: &[String]) -> Result<String> {
    let output = ProcessCommand::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .with_context(|| "run git")?;
    if !output.status.success() {
        bail!(
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub(crate) fn build_plan(
    config: &Config,
    profile: &Profile,
    box_state: &BoxState,
    diff: &DiffContext,
    root: &Path,
    allow_heavy: bool,
) -> Plan {
    let mut notes = Vec::new();
    let guard_ok = guard_ok(profile, box_state, &mut notes);
    let language_mix = classify_language_mix(&diff.changed_files);
    let mut sensors = config
        .tools
        .values()
        .map(|tool| plan_tool(tool, profile, diff, root, guard_ok, allow_heavy))
        .collect::<Vec<_>>();
    sensors.sort_by_key(|sensor| sensor_order(&sensor.id));
    if diff.flags.docs_only {
        notes.push(
            "docs-only diff detected; workflow paths-ignore should normally skip this run"
                .to_owned(),
        );
    }
    if !allow_heavy {
        notes.push("heavy witnesses are disabled unless --allow-heavy is passed".to_owned());
    }
    if matches!(
        profile.name.as_str(),
        "gh-runner" | "gh-runner-standard" | "gh-runner-full"
    ) {
        notes.push(format!(
            "{} profile: trusted repos get opened and ready_for_review evidence passes, 30m target, 60m hard timeout",
            profile.name
        ));
    }
    let repo_lanes = repo_lane_plans(&config.lanes, diff, &mut notes);
    Plan {
        base: diff.base.clone(),
        head: diff.head.clone(),
        profile_name: profile.name.clone(),
        diff_class: diff.diff_class,
        changed_files: diff.changed_files.clone(),
        language_mix: language_mix.clone(),
        sensors,
        lanes: if config.review.enable_default_lanes {
            default_lanes_for_diff_context(diff.diff_class, &language_mix)
        } else {
            Vec::new()
        },
        repo_lanes,
        docs_only: diff.flags.docs_only,
        notes,
    }
}

/// Default sensor packet for repo lanes that do not declare `receives`.
const REPO_LANE_DEFAULT_RECEIVES: &[&str] = &["tokmd", "ripr", "ast-grep"];

/// Convert `[[lanes]]` from repo config into planned lanes for this run.
/// Entries missing `id` or `focus` are skipped with a plan note - they shape
/// review output, not the gate verdict, so the loud-but-non-fatal surface is
/// the plan notes (visible in resolved-plan.json). A lane whose
/// `diff_classes` do not match this diff is silently inapplicable. Lane
/// doctrine lives in docs/specs/UB-REVIEW-SPEC-0011-lane-doctrine.md.
pub(crate) fn repo_lane_plans(
    repo_lanes: &[RepoLane],
    diff: &DiffContext,
    notes: &mut Vec<String>,
) -> Vec<LanePlan> {
    let mut lanes: Vec<LanePlan> = Vec::new();
    for repo_lane in repo_lanes {
        if repo_lane.id.trim().is_empty() || repo_lane.focus.trim().is_empty() {
            notes.push(format!(
                "repo lane skipped: id and focus are required (id=`{}`)",
                repo_lane.id
            ));
            continue;
        }
        let diff_classes = if repo_lane.diff_classes.is_empty() {
            &["all".to_owned()][..]
        } else {
            &repo_lane.diff_classes[..]
        };
        if !proof_policy_diff_class_matches(diff_classes, diff.diff_class.key()) {
            continue;
        }
        let receives = if repo_lane.receives.is_empty() {
            REPO_LANE_DEFAULT_RECEIVES
                .iter()
                .map(|value| (*value).to_owned())
                .collect()
        } else {
            repo_lane.receives.clone()
        };
        let plan_lane = if repo_lane.model.trim().is_empty() {
            let receives_refs = receives.iter().map(String::as_str).collect::<Vec<_>>();
            model_lane(
                &repo_lane.id,
                &repo_lane.role,
                &receives_refs,
                &repo_lane.focus,
            )
        } else {
            LanePlan {
                id: repo_lane.id.clone(),
                role: repo_lane.role.clone(),
                model: repo_lane.model.clone(),
                model_display: repo_lane.model.clone(),
                receives,
                focus: repo_lane.focus.clone(),
            }
        };
        notes.push(format!(
            "repo lane `{}` registered for execution",
            plan_lane.id
        ));
        if let Some(existing) = lanes.iter_mut().find(|lane| lane.id == plan_lane.id) {
            *existing = plan_lane;
        } else {
            lanes.push(plan_lane);
        }
    }
    lanes
}

pub(crate) fn plan_tool(
    tool: &ToolPolicy,
    profile: &Profile,
    diff: &DiffContext,
    root: &Path,
    guard_ok: bool,
    allow_heavy: bool,
) -> SensorPlan {
    let required = tool_required_for_diff(tool, diff);
    if !tool.enabled {
        return skipped(tool, "disabled by config", required);
    }
    if tool.requires_lease && !allow_heavy {
        return skipped(
            tool,
            "heavy/manual witness requires --allow-heavy",
            required,
        );
    }
    if matches!(tool.class, ToolClass::Test) && profile.limits.tests == 0 {
        return skipped(tool, "profile disables test leases", required);
    }
    if matches!(tool.class, ToolClass::Build) && profile.limits.builds == 0 {
        return skipped(tool, "profile disables build leases", required);
    }
    match trigger_match(tool.default, &diff.flags) {
        Some(reason) => {
            if tool.id == "cargo-allow" {
                match cargo_allow_policy_config_state(root) {
                    CargoAllowConfigState::Native => {}
                    CargoAllowConfigState::Absent => {
                        return skipped(tool, "cargo-allow policy config not found", required);
                    }
                    CargoAllowConfigState::ForeignDialect(path) => {
                        return skipped(
                            tool,
                            &format!(
                                "{path} is not a cargo-allow-dialect ledger; add \
                                 policy/cargo-allow.toml (see \
                                 EffortlessMetrics/cargo-allow#1465)"
                            ),
                            required,
                        );
                    }
                }
            }
            if !guard_ok && !matches!(tool.class, ToolClass::Packet) {
                return skipped(
                    tool,
                    "box guard failed; only packet generation is allowed",
                    required,
                );
            }
            SensorPlan {
                id: tool.id.clone(),
                command: tool.command.clone(),
                run: true,
                reason,
                required,
                timeout_sec: resolve_sensor_timeout_sec(tool, profile),
                artifact_budget_mb: tool.artifact_budget_mb,
                class: tool.class,
                weight: tool.weight,
                requires_lease: tool.requires_lease,
                phase: tool.effective_phase(),
                gate: tool.gate.clone(),
            }
        }
        None => skipped(tool, "trigger did not match this diff", false),
    }
}

pub(crate) fn resolve_sensor_timeout_sec(tool: &ToolPolicy, profile: &Profile) -> u64 {
    let base = if tool.provided.timeout_sec {
        tool.timeout_sec
    } else {
        profile
            .tool_timeouts
            .get(&tool.id)
            .copied()
            .unwrap_or(tool.timeout_sec)
    };
    base.min(profile.budgets.default_timeout_sec)
}

pub(crate) fn tool_required_for_diff(tool: &ToolPolicy, diff: &DiffContext) -> bool {
    tool.required && trigger_match(tool.default, &diff.flags).is_some()
}

/// Repo-native cargo-allow ledger that wins over cargo-allow's default
/// config discovery (`policy/allow.toml`, `.cargo/allow.toml`, `allow.toml`).
pub(crate) const CARGO_ALLOW_NATIVE_LEDGER: &str = "policy/cargo-allow.toml";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CargoAllowConfigState {
    Native,
    ForeignDialect(String),
    Absent,
}

pub(crate) fn cargo_allow_policy_config_state(root: &Path) -> CargoAllowConfigState {
    let mut foreign = None;
    for path in [
        CARGO_ALLOW_NATIVE_LEDGER,
        "policy/allow.toml",
        ".cargo/allow.toml",
        "allow.toml",
    ] {
        let candidate = root.join(path);
        if !candidate.is_file() {
            continue;
        }
        if cargo_allow_dialect_matches(&candidate) {
            return CargoAllowConfigState::Native;
        }
        foreign.get_or_insert_with(|| path.to_owned());
    }
    match foreign {
        Some(path) => CargoAllowConfigState::ForeignDialect(path),
        None => CargoAllowConfigState::Absent,
    }
}

pub(crate) fn cargo_allow_dialect_matches(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = toml::from_str::<toml::Value>(&text) else {
        return false;
    };
    value
        .get("policy")
        .and_then(toml::Value::as_str)
        .is_some_and(|policy| policy == "cargo-allow")
        || value
            .get("schema_version")
            .and_then(toml::Value::as_str)
            .is_some_and(|schema_version| schema_version == "0.1")
}

pub(crate) fn skipped(tool: &ToolPolicy, reason: &str, required: bool) -> SensorPlan {
    SensorPlan {
        id: tool.id.clone(),
        command: tool.command.clone(),
        run: false,
        reason: reason.to_owned(),
        required,
        timeout_sec: tool.timeout_sec,
        artifact_budget_mb: tool.artifact_budget_mb,
        class: tool.class,
        weight: tool.weight,
        requires_lease: tool.requires_lease,
        phase: tool.effective_phase(),
        gate: tool.gate.clone(),
    }
}

#[cfg(test)]
mod trusted_input_tests {
    use super::*;

    struct TrustedFixture {
        _temp: tempfile::TempDir,
        root: PathBuf,
        out: PathBuf,
        changed: PathBuf,
        patch: PathBuf,
        base_tree: String,
    }

    fn trusted_fixture() -> Result<TrustedFixture> {
        let temp = tempfile::tempdir()?;
        let root = temp.path().join("root");
        let objects = temp.path().join("objects");
        fs::create_dir_all(&root)?;
        fs::create_dir_all(&objects)?;
        git_text(&root, &["init", "--quiet"])?;
        git_text(
            &root,
            &["config", "user.email", "trusted-fixture@example.invalid"],
        )?;
        git_text(&root, &["config", "user.name", "Trusted Fixture"])?;
        fs::write(root.join("README.md"), "trusted base\n")?;
        fs::write(root.join(".gitignore"), "ignored/\n*.dll\n")?;
        git_text(&root, &["add", "--", "README.md", ".gitignore"])?;
        git_text(&root, &["commit", "--quiet", "-m", "trusted base"])?;
        let out = temp.path().join("out");
        let changed = objects.join("changed-files.txt");
        let patch = objects.join("patch.diff");
        fs::write(&changed, b"src/main.rs\0")?;
        fs::write(
            &patch,
            new_file_patch(&[("src/main.rs", "pub fn trusted_fixture() {}")]),
        )?;
        let base_tree = git_tree_sha(&root, "HEAD")?;
        Ok(TrustedFixture {
            _temp: temp,
            root,
            out,
            changed,
            patch,
            base_tree,
        })
    }

    fn new_file_patch(files: &[(&str, &str)]) -> String {
        let mut patch = String::new();
        for (path, content) in files {
            patch.push_str(&format!(
                "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1 @@\n+{content}\n"
            ));
        }
        patch
    }

    #[test]
    fn trusted_object_ids_require_full_sha_values() -> Result<()> {
        validate_git_object_id(&"a".repeat(40), "base")?;
        for value in ["", "a", &"g".repeat(40), &format!("{}0", "a".repeat(40))] {
            if validate_git_object_id(value, "base").is_ok() {
                bail!("accepted invalid git object id `{value}`");
            }
        }
        Ok(())
    }

    #[test]
    fn trusted_inputs_use_objects_and_reject_nul_or_unsafe_paths() -> Result<()> {
        let fixture = trusted_fixture()?;
        let admission = TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &fixture.base_tree,
            &"b".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )?;
        let diff = DiffContext::from_trusted_admission(&admission);
        if diff.changed_files != vec!["src/main.rs"] || diff.head != "b".repeat(40) {
            bail!("trusted objects were not used verbatim");
        }

        for unsafe_path in [
            "../secret.txt",
            "..\\secret.txt",
            "src/../secret.txt",
            "src\\..\\secret.txt",
            "./secret.txt",
            "src/./secret.txt",
        ] {
            fs::write(&fixture.changed, format!("{unsafe_path}\0"))?;
            if TrustedDiffAdmission::load(
                &fixture.root,
                &fixture.out,
                &fixture.base_tree,
                &"b".repeat(40),
                &fixture.changed,
                &fixture.patch,
            )
            .is_ok()
            {
                bail!("accepted parent-traversal changed path {unsafe_path}");
            }
        }
        fs::write(&fixture.changed, b"src/main.rs\0src/main.rs\0")?;
        if TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &fixture.base_tree,
            &"b".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )
        .is_ok()
        {
            bail!("accepted duplicate changed path");
        }
        fs::write(&fixture.changed, b"src/main.rs\0\0")?;
        if TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &fixture.base_tree,
            &"b".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )
        .is_ok()
        {
            bail!("accepted an empty NUL-delimited changed path");
        }
        fs::write(&fixture.changed, b"src/main.rs\0")?;
        for invalid_patch in [b"".as_slice(), b"diff\0".as_slice(), &[0xff, 0xfe]] {
            fs::write(&fixture.patch, invalid_patch)?;
            if TrustedDiffAdmission::load(
                &fixture.root,
                &fixture.out,
                &fixture.base_tree,
                &"b".repeat(40),
                &fixture.changed,
                &fixture.patch,
            )
            .is_ok()
            {
                bail!("accepted invalid trusted patch object");
            }
        }
        if TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &fixture.base_tree,
            &"b".repeat(40),
            Path::new("changed-files.txt"),
            &fixture.patch,
        )
        .is_ok()
        {
            bail!("accepted relative trusted object path");
        }
        fs::write(&fixture.patch, b"not a git patch\n")?;
        if TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &fixture.base_tree,
            &"b".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )
        .is_ok()
        {
            bail!("accepted syntactically invalid trusted patch");
        }
        fs::write(
            &fixture.patch,
            new_file_patch(&[("src/main.rs", "pub fn trusted_fixture() {}")]),
        )?;
        fs::write(&fixture.changed, b"src/other.rs\0")?;
        if TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &fixture.base_tree,
            &"b".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )
        .is_ok()
        {
            bail!("accepted changed paths that do not match the trusted patch");
        }
        Ok(())
    }

    #[test]
    fn trusted_changed_paths_use_unambiguous_nul_encoding() -> Result<()> {
        let encoded = b"line\nname.rs\0carriage\rname.rs\0";
        let paths = parse_trusted_changed_files(encoded)?;
        if paths != vec!["line\nname.rs", "carriage\rname.rs"] {
            bail!("NUL-delimited changed paths did not preserve CR/LF names exactly");
        }
        if parse_trusted_changed_files(b"line\nname.rs\ncarriage\rname.rs\n").is_ok() {
            bail!("accepted ambiguous newline-delimited changed paths");
        }
        Ok(())
    }

    #[test]
    fn trusted_inputs_reject_checkout_tree_mismatch() -> Result<()> {
        let fixture = trusted_fixture()?;
        if TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &"c".repeat(40),
            &"b".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )
        .is_ok()
        {
            bail!("accepted checkout tree mismatch");
        }
        if TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.root.join("trusted-output"),
            &fixture.base_tree,
            &"b".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )
        .is_ok()
        {
            bail!("accepted an output directory inside the trusted repository root");
        }
        Ok(())
    }

    #[test]
    fn trusted_inputs_reject_dirty_or_repository_owned_objects() -> Result<()> {
        let fixture = trusted_fixture()?;
        let admission = TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &fixture.base_tree,
            &"b".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )?;
        fs::write(fixture.root.join("hostile-plugin.toml"), "load = true\n")?;
        if admission.verify_root().is_ok() {
            bail!("receipt verification accepted a dirty trusted repository root");
        }
        if TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &fixture.base_tree,
            &"b".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )
        .is_ok()
        {
            bail!("accepted an untracked file in the trusted repository root");
        }
        fs::remove_file(fixture.root.join("hostile-plugin.toml"))?;
        let ignored = fixture.root.join("ignored");
        fs::create_dir_all(&ignored)?;
        fs::write(ignored.join(".ub-review.toml"), "[providers]\n")?;
        fs::write(ignored.join("hostile-plugin.toml"), "load = true\n")?;
        fs::write(ignored.join("hostile.dll"), b"HOSTILE_DLL_SENTINEL")?;
        if admission.verify_root().is_ok()
            || TrustedDiffAdmission::load(
                &fixture.root,
                &fixture.out,
                &fixture.base_tree,
                &"b".repeat(40),
                &fixture.changed,
                &fixture.patch,
            )
            .is_ok()
        {
            bail!("accepted ignored hostile config, plugin, or library files");
        }
        fs::remove_dir_all(&ignored)?;
        let repository_object = fixture.root.join("changed-files.txt");
        fs::write(&repository_object, b"src/main.rs\0")?;
        git_text(&fixture.root, &["add", "--", "changed-files.txt"])?;
        git_text(
            &fixture.root,
            &["commit", "--quiet", "-m", "object in root"],
        )?;
        let new_tree = git_tree_sha(&fixture.root, "HEAD")?;
        if TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &new_tree,
            &"b".repeat(40),
            &repository_object,
            &fixture.patch,
        )
        .is_ok()
        {
            bail!("accepted a repository-controlled changed-files object");
        }
        Ok(())
    }

    #[test]
    fn trusted_inputs_treat_hostile_head_files_as_data_only() -> Result<()> {
        let fixture = trusted_fixture()?;
        fs::write(
            &fixture.changed,
            b".ub-review.toml\0plugins/hostile.toml\0scripts/evil.sh\0lib/hostile.dll\0",
        )?;
        fs::write(
            &fixture.patch,
            new_file_patch(&[
                (".ub-review.toml", "EXECUTE_CONFIG_SENTINEL"),
                ("plugins/hostile.toml", "LOAD_PLUGIN_SENTINEL"),
                ("scripts/evil.sh", "RUN_SCRIPT_SENTINEL"),
                ("lib/hostile.dll", "LOAD_LIBRARY_SENTINEL"),
            ]),
        )?;
        let admission = TrustedDiffAdmission::load(
            &fixture.root,
            &fixture.out,
            &fixture.base_tree,
            // This identity deliberately does not exist in the fixture repo.
            // Success proves it is recorded as metadata, never resolved.
            &"c".repeat(40),
            &fixture.changed,
            &fixture.patch,
        )?;
        fs::write(&fixture.changed, b"different/head-controlled/path\0")?;
        fs::write(&fixture.patch, b"MUTATED_AFTER_ADMISSION\n")?;
        let diff = DiffContext::from_trusted_admission(&admission);
        if diff.changed_files.len() != 4 || diff.head != "c".repeat(40) {
            bail!("trusted hostile-head fixture was not retained as data");
        }
        for hostile_path in &diff.changed_files {
            if fixture.root.join(hostile_path).exists() {
                bail!("hostile-head path was loaded into the trusted base checkout");
            }
        }
        if diff.patch.contains("MUTATED_AFTER_ADMISSION") {
            bail!("DiffContext re-read the patch object after admission");
        }
        if admission.patch_sha256 != sha256_hex(diff.patch.as_bytes())
            || admission.changed_files_sha256
                != sha256_hex(&encode_trusted_changed_files(&diff.changed_files))
        {
            bail!("trusted diff digests do not describe the admitted DiffContext");
        }
        admission.verify_root()?;
        Ok(())
    }
}
