//! Plan construction: resolved plan/selector artifacts and plan
//! building from config + args (cleanup train step 32, pure code motion).

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
        gate: tool.gate.clone(),
    }
}
