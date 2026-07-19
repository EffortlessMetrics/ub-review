// Sensor, cargo, RIPR, and unsafe-review test cluster, extracted from src/main.rs mod tests (#597).
// Resolves shared fixtures through `super::*` and production symbols through `crate::*`.
use super::*;
use crate::*;

#[test]
fn unsafe_tokens_trigger_native_risk() {
    let flags = classify_diff(&["src/lib.rs".to_owned()], "+ let p = bytes.as_ptr();");
    assert!(flags.rust_changed);
    assert!(flags.unsafe_or_native_risk);
    assert_eq!(
        classify_diff_class(&["src/lib.rs".to_owned()], &flags),
        DiffClass::SourceUb
    );
}

#[test]
fn gh_runner_profiles_carry_ripr_lease_override_quick_boxes_do_not() -> Result<()> {
    let profiles = builtin_profiles();
    for name in ["gh-runner", "gh-runner-standard", "gh-runner-full"] {
        let profile = profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| anyhow::anyhow!("missing builtin profile {name}"))?;
        assert_eq!(
            profile.tool_timeouts.get("ripr"),
            Some(&720),
            "{name} must carry the ripr lease override"
        );
    }
    for name in ["cx23", "cx33", "cx43"] {
        let profile = profiles
            .iter()
            .find(|profile| profile.name == name)
            .ok_or_else(|| anyhow::anyhow!("missing builtin profile {name}"))?;
        assert!(
            profile.tool_timeouts.is_empty(),
            "{name} keeps one-size sensor leases"
        );
    }

    let mut config: Config = toml::from_str(include_str!("../../.ub-review.toml"))?;
    config.merge_defaults();
    let profile = config.selected_profile()?;
    assert_eq!(profile.tool_timeouts.get("ripr"), Some(&720));
    let ripr = config
        .tools
        .get("ripr")
        .ok_or_else(|| anyhow::anyhow!("ripr tool missing from repo config"))?;
    assert!(
        ripr.provided.timeout_sec,
        "this repo pins ripr's lease explicitly in .ub-review.toml"
    );
    let resolved = crate::resolve_sensor_timeout_sec(ripr, profile);
    assert_eq!(
        resolved,
        ripr.timeout_sec.min(profile.budgets.default_timeout_sec),
        "repo-pinned lease wins over the profile override"
    );
    assert_ne!(resolved, 720, "profile table must not shadow repo config");
    Ok(())
}

#[test]
fn unsafe_review_swarm_recommended_config_loads_advisory_floor() -> Result<()> {
    let mut config = Config::from_toml_with_policy_receipts(include_str!(
        "../../configs/unsafe-review-swarm.ub-review.toml"
    ))?;
    config.merge_defaults();

    assert!(
        config.policy_errors.is_empty(),
        "recommended config should not rely on stripped or deprecated keys: {:?}",
        config.policy_errors
    );
    assert_eq!(config.review_profile, "bun-ub-v0");
    assert_eq!(config.profile, "gh-runner-full");
    assert_eq!(config.repo.kind, "rust");
    assert_eq!(config.repo.ledger, "docs/dogfood");
    assert_eq!(
        config.review_body.summary_only_body,
        SummaryOnlyBodyPolicy::PostSubstantive
    );
    assert_eq!(config.gate.required_check, "ub-review/gate");
    assert_eq!(
        config.gate.post_review_on,
        vec!["opened".to_owned(), "ready_for_review".to_owned()]
    );
    assert!(config.gate.blocking.required_proof_unproven);
    assert!(!config.gate.blocking.tool_gate_missing_evidence);
    assert_eq!(config.providers.policy, "primary-with-fallback");

    let expected_proofs = [(
        "check-pr",
        "cargo run --locked -p xtask -- check-pr",
        "focused-build",
        300_u64,
    )];
    assert_eq!(config.proof.required.len(), expected_proofs.len());
    for (id, command, cost, timeout_sec) in expected_proofs {
        let policy = config
            .proof
            .required
            .iter()
            .find(|policy| policy.id == id)
            .ok_or_else(|| anyhow::anyhow!("missing required proof {id}"))?;
        assert!(policy.enabled);
        assert!(policy.required);
        assert_eq!(policy.command, command);
        assert_eq!(policy.cost.as_deref(), Some(cost));
        assert_eq!(policy.timeout_sec, timeout_sec);
        assert_eq!(
            crate::proof_request_status(&policy.command, cost),
            "requested",
            "unsafe-review-swarm required proof {id} must be brokerable"
        );
    }

    for id in ["cargo-fmt", "cargo-check", "cargo-test", "cargo-clippy"] {
        let tool = config
            .tools
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("missing required cargo tool {id}"))?;
        assert!(tool.enabled, "{id} should be enabled");
        assert!(tool.required, "{id} should be required");
        assert_eq!(tool.default, crate::Trigger::Always);
    }
    let unsafe_review = config
        .tools
        .get("unsafe-review")
        .ok_or_else(|| anyhow::anyhow!("missing unsafe-review tool"))?;
    assert!(unsafe_review.enabled);
    assert!(unsafe_review.required);
    let ripr = config
        .tools
        .get("ripr")
        .ok_or_else(|| anyhow::anyhow!("missing ripr tool"))?;
    assert!(ripr.enabled);
    assert!(!ripr.required);
    let cargo_allow = config
        .tools
        .get("cargo-allow")
        .ok_or_else(|| anyhow::anyhow!("missing cargo-allow tool"))?;
    assert!(cargo_allow.enabled);
    assert!(!cargo_allow.required);

    let plan = crate::build_plan(
        &config,
        config.selected_profile()?,
        &BoxState {
            cpus: 4,
            free_mem_mb: Some(8_000),
            free_disk_mb: Some(20_000),
            load_1m: Some(0.5),
            github_actions: true,
        },
        &test_diff(),
        Path::new("."),
        false,
    );
    let unsafe_review_sensor = plan
        .sensors
        .iter()
        .find(|sensor| sensor.id == "unsafe-review")
        .ok_or_else(|| anyhow::anyhow!("missing planned unsafe-review sensor"))?;
    assert!(
        unsafe_review_sensor.required,
        "unsafe-review remains required when its trigger matches"
    );
    for id in ["cargo-fmt", "cargo-check", "cargo-test", "cargo-clippy"] {
        let sensor = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == id)
            .ok_or_else(|| anyhow::anyhow!("missing planned cargo sensor {id}"))?;
        assert!(sensor.run, "{id} should run in every advisory swarm pass");
        assert!(sensor.required, "{id} should stay in the required floor");
    }
    let resolved_profile = crate::resolved_profile_artifact(&config, config.selected_profile()?);
    assert_eq!(
        resolved_profile["proof"]["required"]
            .as_array()
            .map(Vec::len),
        Some(expected_proofs.len())
    );
    Ok(())
}

#[test]
fn cargo_allow_foreign_dialect_reason_wins_before_box_guard() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::create_dir_all(root.join("policy"))?;
    fs::write(
        root.join("policy/allow.toml"),
        "schema_version = \"1\"\ntool = \"xtask-policy\"\n",
    )?;
    let config = Config::default();
    let profile = config.selected_profile()?;
    let cargo_allow = config
        .tools
        .get("cargo-allow")
        .ok_or_else(|| anyhow::anyhow!("cargo-allow tool policy missing"))?;
    let flags = DiffFlags {
        source_changed: true,
        ..DiffFlags::default()
    };
    let diff = DiffContext {
        base: "HEAD~1".to_owned(),
        head: "HEAD".to_owned(),
        changed_files: vec!["src/lib.rs".to_owned()],
        patch: "diff --git a/src/lib.rs b/src/lib.rs\n".to_owned(),
        flags,
        diff_class: DiffClass::SourceGeneral,
    };

    let plan = crate::plan_tool(cargo_allow, profile, &diff, root, false, false);

    assert!(!plan.run);
    assert_eq!(
        plan.reason,
        "policy/allow.toml is not a cargo-allow-dialect ledger; add \
             policy/cargo-allow.toml (see EffortlessMetrics/cargo-allow#1465)"
    );
    Ok(())
}

#[test]
fn sensor_timeout_resolves_per_profile_with_repo_override_winning() -> Result<()> {
    let tool = crate::ToolPolicy {
        id: "ripr".to_owned(),
        command: "ripr".to_owned(),
        default: crate::Trigger::Always,
        timeout_sec: 240,
        ..crate::ToolPolicy::default()
    };
    let diff = test_diff();
    let temp = tempfile::tempdir()?;
    let quick = Profile::default();
    let mut hosted = Profile::default();
    hosted.tool_timeouts.insert("ripr".to_owned(), 720);

    let quick_plan = crate::plan_tool(&tool, &quick, &diff, temp.path(), true, false);
    let hosted_plan = crate::plan_tool(&tool, &hosted, &diff, temp.path(), true, false);
    assert!(quick_plan.run, "{}", quick_plan.reason);
    assert!(hosted_plan.run, "{}", hosted_plan.reason);
    assert_eq!(quick_plan.timeout_sec, 240, "no override: built-in lease");
    assert_eq!(
        hosted_plan.timeout_sec, 720,
        "profile [tool_timeouts] applies"
    );

    let mut repo_tool = tool.clone();
    repo_tool.timeout_sec = 600;
    repo_tool.provided.timeout_sec = true;
    let repo_plan = crate::plan_tool(&repo_tool, &hosted, &diff, temp.path(), true, false);
    assert_eq!(repo_plan.timeout_sec, 600, "explicit repo config wins");

    let mut capped_profile = hosted.clone();
    capped_profile.budgets.default_timeout_sec = 300;
    let capped_plan = crate::plan_tool(&tool, &capped_profile, &diff, temp.path(), true, false);
    assert_eq!(capped_plan.timeout_sec, 300, "budget cap still bounds");
    Ok(())
}

#[test]
fn running_summary_reports_planned_skipped_sensor_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path().join("out");
    let planned_dry_run = sensor_plan("tokmd", "tokmd", true);
    let trigger_skipped = sensor_plan("ripr", "ripr", false);
    write_sensor_status(
        &out,
        &planned_dry_run,
        SensorStatusWrite {
            status: "skipped",
            argv: &["tokmd".to_owned()],
            duration_ms: 0,
            reason: "dry-run; sensor not executed",
            exit_code: None,
            timed_out: false,
        },
    )?;
    write_sensor_status(
        &out,
        &trigger_skipped,
        SensorStatusWrite {
            status: "skipped",
            argv: &["ripr".to_owned()],
            duration_ms: 0,
            reason: "trigger did not match this diff",
            exit_code: None,
            timed_out: false,
        },
    )?;
    let plan = test_plan(vec![planned_dry_run, trigger_skipped]);

    let summary = render_summary(&out, &plan, &test_diff())?;
    let missing = summary_section(&summary, "## Missing evidence", "## Lane packets")
        .ok_or_else(|| anyhow::anyhow!("missing evidence section not found"))?;

    assert!(missing.contains("tokmd skipped; deterministic repository/diff packet unavailable; reason: dry-run; sensor not executed."));
    assert!(!missing.contains("ripr"));
    assert!(!missing.contains("No planned sensor evidence is currently missing."));
    Ok(())
}

#[test]
fn skipped_out_of_scope_sensors_are_not_missing_review_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path().join("out");
    let planned_dry_run = sensor_plan("tokmd", "tokmd", true);
    let trigger_skipped = sensor_plan("ripr", "ripr", false);
    let disabled = sensor_plan("semgrep", "semgrep", false);
    let heavy = sensor_plan("miri", "cargo", false);

    write_sensor_status(
        &out,
        &planned_dry_run,
        SensorStatusWrite {
            status: "skipped",
            argv: &["tokmd".to_owned()],
            duration_ms: 0,
            reason: "dry-run; sensor not executed",
            exit_code: None,
            timed_out: false,
        },
    )?;
    write_sensor_status(
        &out,
        &trigger_skipped,
        SensorStatusWrite {
            status: "skipped",
            argv: &["ripr".to_owned()],
            duration_ms: 0,
            reason: "trigger did not match this diff",
            exit_code: None,
            timed_out: false,
        },
    )?;
    write_sensor_status(
        &out,
        &disabled,
        SensorStatusWrite {
            status: "skipped",
            argv: &["semgrep".to_owned()],
            duration_ms: 0,
            reason: "disabled by config",
            exit_code: None,
            timed_out: false,
        },
    )?;
    write_sensor_status(
        &out,
        &heavy,
        SensorStatusWrite {
            status: "skipped",
            argv: &["cargo".to_owned(), "miri".to_owned()],
            duration_ms: 0,
            reason: "heavy/manual witness requires --allow-heavy",
            exit_code: None,
            timed_out: false,
        },
    )?;
    let plan = test_plan(vec![planned_dry_run, trigger_skipped, disabled, heavy]);

    let issues = collect_sensor_evidence_issues(&out, &plan);

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].sensor, "tokmd");
    assert_eq!(issues[0].status, "skipped");
    assert_eq!(issues[0].reason, "dry-run; sensor not executed");
    Ok(())
}

#[test]
fn skipped_required_sensor_is_missing_review_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path().join("out");
    let mut required_actionlint = sensor_plan("actionlint", "actionlint", false);
    required_actionlint.required = true;
    required_actionlint.reason = "disabled by config".to_owned();
    let mut trigger_skipped = sensor_plan("ripr", "ripr", false);
    trigger_skipped.reason = "trigger did not match this diff".to_owned();

    write_sensor_status(
        &out,
        &required_actionlint,
        SensorStatusWrite {
            status: "skipped",
            argv: &["actionlint".to_owned()],
            duration_ms: 0,
            reason: "disabled by config",
            exit_code: None,
            timed_out: false,
        },
    )?;
    let required_status: serde_json::Value = serde_json::from_slice(&fs::read(
        out.join("sensors/actionlint/ub-review-sensor-status.json"),
    )?)?;
    assert_eq!(required_status["required"], true);
    write_sensor_status(
        &out,
        &trigger_skipped,
        SensorStatusWrite {
            status: "skipped",
            argv: &["ripr".to_owned()],
            duration_ms: 0,
            reason: "trigger did not match this diff",
            exit_code: None,
            timed_out: false,
        },
    )?;
    let plan = test_plan(vec![required_actionlint, trigger_skipped]);

    let issues = collect_sensor_evidence_issues(&out, &plan);

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].sensor, "actionlint");
    assert_eq!(issues[0].status, "skipped");
    assert_eq!(issues[0].reason, "disabled by config");
    Ok(())
}

#[test]
fn unsafe_review_ok_without_gate_artifact_is_sensor_artifact_gap() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let out = temp.path().join("out");
    let mut sensor = sensor_plan("unsafe-review", "unsafe-review", true);
    sensor.required = true;
    write_sensor_status(
        &out,
        &sensor,
        SensorStatusWrite {
            status: "ok",
            argv: &["unsafe-review".to_owned(), "first-pr".to_owned()],
            duration_ms: 10,
            reason: "completed",
            exit_code: Some(0),
            timed_out: false,
        },
    )?;
    let plan = test_plan(vec![sensor.clone()]);

    let issues = collect_sensor_evidence_issues(&out, &plan);

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].sensor, "unsafe-review");
    assert_eq!(issues[0].status, "artifact-gap");
    assert_eq!(
        issues[0].reason,
        "unsafe-review-gate.json absent; structured evidence unavailable"
    );

    let gate_dir = out
        .join("sensors")
        .join("unsafe-review")
        .join(crate::UNSAFE_REVIEW_OUTPUT_SUBDIR);
    fs::create_dir_all(&gate_dir)?;
    fs::write(
        gate_dir.join("unsafe-review-gate.json"),
        r#"{"schema_version":"unsafe-review-gate/v1","status":"advisory"}"#,
    )?;

    let issues = collect_sensor_evidence_issues(&out, &plan);
    assert!(
        issues.is_empty(),
        "valid structured unsafe-review evidence should clear the gap: {issues:?}"
    );
    Ok(())
}

#[test]
fn sensor_jobs_use_runtime_profile_limit() -> Result<()> {
    let profiles = builtin_profiles();
    let gh_runner = profiles
        .iter()
        .find(|profile| profile.name == "gh-runner")
        .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;
    let cx23 = profiles
        .iter()
        .find(|profile| profile.name == "cx23")
        .ok_or_else(|| anyhow::anyhow!("missing cx23 profile"))?;
    let cx43 = profiles
        .iter()
        .find(|profile| profile.name == "cx43")
        .ok_or_else(|| anyhow::anyhow!("missing cx43 profile"))?;

    assert_eq!(sensor_job_count(gh_runner, 10)?, 4);
    assert_eq!(sensor_job_count(cx23, 10)?, 2);
    assert_eq!(sensor_job_count(cx43, 10)?, 6);
    Ok(())
}

#[test]
fn sensor_jobs_cap_to_runnable_sensors() -> Result<()> {
    let profiles = builtin_profiles();
    let gh_runner = profiles
        .iter()
        .find(|profile| profile.name == "gh-runner")
        .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;

    assert_eq!(sensor_job_count(gh_runner, 2)?, 2);
    Ok(())
}

#[test]
fn zero_sensor_runtime_limit_is_rejected() -> Result<()> {
    let profile = Profile {
        name: "broken".to_owned(),
        limits: Limits {
            sensor_jobs: 0,
            ..Limits::default()
        },
        ..Profile::default()
    };

    let err = sensor_job_count(&profile, 2)
        .err()
        .ok_or_else(|| anyhow::anyhow!("zero sensor limit unexpectedly passed"))?;

    assert!(
        err.to_string()
            .contains("runtime profile broken has sensor_jobs=0")
    );
    Ok(())
}

#[test]
fn tool_selectors_filter_planned_sensors() -> Result<()> {
    let mut plan = test_plan(vec![
        sensor_plan("tokmd", "tokmd", true),
        sensor_plan("ripr", "ripr", true),
        sensor_plan("ast-grep", "ast-grep", true),
    ]);
    let selectors = SelectorArgs {
        tools: "tokmd,ripr".to_owned(),
        except_tools: "ripr".to_owned(),
        ..SelectorArgs::default()
    };

    apply_plan_selectors(&mut plan, &selectors)?;

    assert_eq!(
        plan.sensors
            .iter()
            .map(|sensor| sensor.id.as_str())
            .collect::<Vec<_>>(),
        vec!["tokmd"]
    );
    assert!(
        plan.notes
            .iter()
            .any(|note| note.contains("tool selectors"))
    );
    Ok(())
}

#[test]
fn tool_gate_scope_outside_allowlist_is_receipted_and_stripped() -> Result<()> {
    let config = Config::from_toml_with_policy_receipts(
        r#"
[tools.ripr.gate]
scope = "repo-wide"
max_new_unsuppressed = 0
"#,
    )?;
    assert_eq!(config.policy_errors.len(), 1);
    assert_eq!(config.policy_errors[0].section, "tools.ripr.gate.scope");
    assert!(
        config.policy_errors[0].detail.contains("repo-wide"),
        "detail must name the rejected scope: {}",
        config.policy_errors[0].detail
    );
    // The threshold sibling survives with the unknown scope stripped, so
    // the only semantics that exist (on-diff) still apply.
    let gate = config
        .tools
        .get("ripr")
        .and_then(|tool| tool.gate.as_ref())
        .ok_or_else(|| anyhow::anyhow!("ripr gate policy missing"))?;
    assert_eq!(gate.scope, None);
    // The threshold survives from the test fixture (which uses 0); this
    // test validates scope stripping, not the repo's epic ceiling.
    assert_eq!(gate.max_new_unsuppressed, Some(0));

    let valid = Config::from_toml_with_policy_receipts(
        r#"
[tools.ripr.gate]
scope = "on-diff"
max_new_unsuppressed = 0
"#,
    )?;
    assert!(valid.policy_errors.is_empty());
    Ok(())
}
