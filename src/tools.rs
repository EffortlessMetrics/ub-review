//! Tool registry receipts and the tool-gate evaluation path: resolved-tools,
//! tool-status, tool-gate-outcomes writers and the gate-decision threshold
//! reader (cleanup train step 6, pure code motion).

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Result;
use serde::Deserialize;

use crate::*;

pub(crate) fn write_resolved_tools_artifacts(
    out: &Path,
    config: &Config,
    profile: &Profile,
    plan: &Plan,
) -> Result<()> {
    let artifact = resolved_tools_artifact(config, profile, plan);
    let bytes = serde_json::to_vec_pretty(&artifact)?;
    fs::write(out.join("resolved-tools.json"), &bytes)?;
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    fs::write(review_dir.join("resolved-tools.json"), bytes)?;
    Ok(())
}

pub(crate) fn resolved_tools_artifact(
    config: &Config,
    profile: &Profile,
    plan: &Plan,
) -> ResolvedToolArtifact {
    let plan_by_id = plan
        .sensors
        .iter()
        .map(|sensor| (sensor.id.as_str(), sensor))
        .collect::<BTreeMap<_, _>>();
    let tools = config
        .tools
        .values()
        .map(|tool| {
            let planned = plan_by_id.get(tool.id.as_str());
            ResolvedToolEntry {
                id: tool.id.clone(),
                class: tool.class,
                command: tool.command.clone(),
                required_if: tool.default,
                required: planned.is_some_and(|sensor| sensor.required),
                required_reason: trigger_description(tool.default).to_owned(),
                runtime_profile: profile.name.clone(),
                enabled: tool.enabled,
                planned_run: planned.is_some_and(|sensor| sensor.run),
                plan_reason: planned
                    .map(|sensor| sensor.reason.clone())
                    .unwrap_or_else(|| "not present in resolved plan".to_owned()),
                timeout_sec: planned
                    .map(|sensor| sensor.timeout_sec)
                    .unwrap_or(tool.timeout_sec),
                artifact_budget_mb: tool.artifact_budget_mb,
                requires_lease: tool.requires_lease,
                gate: tool.gate.clone(),
                artifact_paths: tool_artifact_paths(&tool.id),
            }
        })
        .collect();
    ResolvedToolArtifact {
        schema: RESOLVED_TOOLS_SCHEMA,
        runtime_profile: profile.name.clone(),
        tools,
    }
}

/// Writes the tool-status and tool-gate-outcome artifacts and returns the
/// gate outcomes so `cmd_run` can thread them into the gate verdict.
pub(crate) fn write_tool_status_artifacts(
    out: &Path,
    config: &Config,
    profile: &Profile,
    plan: &Plan,
) -> Result<ToolGateOutcomeArtifact> {
    let artifact = tool_status_artifact(out, config, profile, plan);
    let bytes = serde_json::to_vec_pretty(&artifact)?;
    fs::write(out.join("tool-status.json"), &bytes)?;
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    fs::write(review_dir.join("tool-status.json"), bytes)?;
    write_tool_gate_outcome_artifacts(out, config, profile, &artifact)
}

pub(crate) fn tool_status_artifact(
    out: &Path,
    config: &Config,
    profile: &Profile,
    plan: &Plan,
) -> ToolStatusArtifact {
    let plan_by_id = plan
        .sensors
        .iter()
        .map(|sensor| (sensor.id.as_str(), sensor))
        .collect::<BTreeMap<_, _>>();
    let tools = config
        .tools
        .values()
        .map(|tool| {
            let planned = plan_by_id.get(tool.id.as_str());
            let receipt = planned.and_then(|sensor| {
                let receipt_path = out
                    .join("sensors")
                    .join(&sensor.id)
                    .join("ub-review-sensor-status.json");
                read_sensor_receipt(&receipt_path)
            });
            ToolStatusEntry {
                id: tool.id.clone(),
                class: planned.map(|sensor| sensor.class).unwrap_or(tool.class),
                command: tool.command.clone(),
                required_if: tool.default,
                required: planned.is_some_and(|sensor| sensor.required),
                required_reason: trigger_description(tool.default).to_owned(),
                runtime_profile: profile.name.clone(),
                planned_run: planned.is_some_and(|sensor| sensor.run),
                timeout_sec: planned
                    .map(|sensor| sensor.timeout_sec)
                    .unwrap_or(tool.timeout_sec),
                artifact_budget_mb: planned
                    .map(|sensor| sensor.artifact_budget_mb)
                    .unwrap_or(tool.artifact_budget_mb),
                requires_lease: planned
                    .map(|sensor| sensor.requires_lease)
                    .unwrap_or(tool.requires_lease),
                status: receipt
                    .as_ref()
                    .map(|receipt| receipt.status.clone())
                    .unwrap_or_else(|| {
                        if planned.is_some() {
                            "receipt_absent".to_owned()
                        } else {
                            "not_planned".to_owned()
                        }
                    }),
                reason: receipt
                    .as_ref()
                    .map(|receipt| receipt.reason.clone())
                    .or_else(|| planned.map(|sensor| sensor.reason.clone()))
                    .unwrap_or_else(|| "not present in resolved plan".to_owned()),
                exit_code: receipt.as_ref().and_then(|receipt| receipt.exit_code),
                timed_out: receipt.as_ref().is_some_and(|receipt| receipt.timed_out),
                gate: tool.gate.clone(),
                artifact_paths: tool_artifact_paths(&tool.id),
            }
        })
        .collect();
    ToolStatusArtifact {
        schema: TOOL_STATUS_SCHEMA,
        runtime_profile: profile.name.clone(),
        tools,
    }
}

pub(crate) fn write_tool_gate_outcome_artifacts(
    out: &Path,
    config: &Config,
    profile: &Profile,
    tool_status: &ToolStatusArtifact,
) -> Result<ToolGateOutcomeArtifact> {
    let artifact = tool_gate_outcome_artifact(out, config, profile, tool_status);
    let bytes = serde_json::to_vec_pretty(&artifact)?;
    fs::write(out.join("tool-gate-outcomes.json"), &bytes)?;
    let review_dir = out.join("review");
    fs::create_dir_all(&review_dir)?;
    fs::write(review_dir.join("tool-gate-outcomes.json"), bytes)?;
    let mut ndjson = String::new();
    for outcome in &artifact.outcomes {
        ndjson.push_str(&serde_json::to_string(outcome)?);
        ndjson.push('\n');
    }
    fs::write(out.join("tool_gate_outcomes.ndjson"), ndjson)?;
    Ok(artifact)
}

pub(crate) fn tool_gate_outcome_artifact(
    out: &Path,
    config: &Config,
    profile: &Profile,
    tool_status: &ToolStatusArtifact,
) -> ToolGateOutcomeArtifact {
    let status_by_id = tool_status
        .tools
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    let outcomes = config
        .tools
        .values()
        .filter_map(|tool| {
            let policy = tool.gate.clone()?;
            let status = status_by_id.get(tool.id.as_str());
            Some(tool_gate_outcome_entry(out, tool, policy, status.copied()))
        })
        .collect();
    ToolGateOutcomeArtifact {
        schema: TOOL_GATE_OUTCOMES_SCHEMA,
        runtime_profile: profile.name.clone(),
        outcomes,
    }
}

pub(crate) fn tool_gate_outcome_entry(
    out: &Path,
    tool: &ToolPolicy,
    policy: ToolGatePolicy,
    status: Option<&ToolStatusEntry>,
) -> ToolGateOutcomeEntry {
    let sensor_receipt_path = format!("sensors/{}/ub-review-sensor-status.json", tool.id);
    let gate_decision_path = format!("sensors/{}/gate-decision.json", tool.id);
    let gate_decision_state = read_tool_gate_decision(&out.join(&gate_decision_path));
    let gate_decision = match &gate_decision_state {
        ToolGateDecisionState::Present(decision) => Some(decision),
        ToolGateDecisionState::Missing | ToolGateDecisionState::Malformed(_) => None,
    };
    let sensor_status = status
        .map(|entry| entry.status.clone())
        .unwrap_or_else(|| "missing_status".to_owned());
    let sensor_reason = status
        .map(|entry| entry.reason.clone())
        .unwrap_or_else(|| "tool-status entry missing".to_owned());
    let required = status.is_some_and(|entry| entry.required);
    let planned_run = status.is_some_and(|entry| entry.planned_run);
    let (outcome, evaluated, reason, new_unsuppressed) = match status {
        None => (
            "missing_evidence".to_owned(),
            false,
            "tool-status entry missing for configured gate policy".to_owned(),
            None,
        ),
        Some(entry) if !entry.planned_run => (
            "not_evaluated".to_owned(),
            false,
            format!("tool gate was not evaluated because {}", entry.reason),
            None,
        ),
        // A sensor that crashed or timed out never produced a verdict, so the
        // threshold could not be evaluated. That is missing evidence, not a
        // threshold failure: only an actually-evaluated, actually-exceeded
        // threshold gets the unconditionally-blocking `failed` outcome.
        // Missing evidence stays advisory unless repo policy opts required
        // tools into blocking via [gate.blocking] tool_gate_missing_evidence.
        Some(entry) if matches!(entry.status.as_str(), "failed" | "timed_out") => (
            "missing_evidence".to_owned(),
            false,
            format!(
                "tool gate threshold could not be evaluated because the sensor did not \
                 produce a verdict (sensor status `{}`)",
                entry.status
            ),
            None,
        ),
        Some(entry) if entry.status == "ok" => {
            match (&gate_decision_state, policy.max_new_unsuppressed) {
                (ToolGateDecisionState::Malformed(reason), Some(_)) => (
                    "missing_evidence".to_owned(),
                    false,
                    format!("`{}` gate-decision receipt is malformed: {reason}", tool.id),
                    None,
                ),
                _ => evaluate_tool_gate_threshold(tool, &policy, gate_decision),
            }
        }
        Some(entry) => (
            "missing_evidence".to_owned(),
            false,
            format!(
                "tool gate could not be evaluated because sensor status was `{}`",
                entry.status
            ),
            None,
        ),
    };
    let mut source_artifacts = vec![sensor_receipt_path.clone(), "tool-status.json".to_owned()];
    if !matches!(gate_decision_state, ToolGateDecisionState::Missing) {
        source_artifacts.push(gate_decision_path);
    }
    let exposure_gap_details_path = format!("sensors/{}/exposure-gaps.json", tool.id);
    if tool.id == "ripr" && out.join(&exposure_gap_details_path).is_file() {
        source_artifacts.push(exposure_gap_details_path);
    }
    ToolGateOutcomeEntry {
        schema: TOOL_GATE_OUTCOME_SCHEMA,
        tool: tool.id.clone(),
        policy,
        required,
        planned_run,
        sensor_status,
        sensor_reason,
        sensor_receipt_path,
        status_source: "tool-status.json",
        outcome,
        evaluated,
        reason,
        metrics: ToolGateOutcomeMetrics { new_unsuppressed },
        source_artifacts,
        packet_policy: "gate-only",
        gate_policy: "trust-affecting",
    }
}

pub(crate) fn evaluate_tool_gate_threshold(
    tool: &ToolPolicy,
    policy: &ToolGatePolicy,
    gate_decision: Option<&ToolGateDecision>,
) -> (String, bool, String, Option<u64>) {
    let Some(max_new_unsuppressed) = policy.max_new_unsuppressed else {
        return (
            "not_evaluated".to_owned(),
            false,
            "configured gate policy has no supported threshold to evaluate".to_owned(),
            None,
        );
    };
    let Some(decision) = gate_decision else {
        return (
            "missing_evidence".to_owned(),
            false,
            format!(
                "`{}` ran ok, but no machine-readable gate-decision receipt was available",
                tool.id
            ),
            None,
        );
    };
    if decision.new_unsuppressed <= max_new_unsuppressed {
        (
            "passed".to_owned(),
            true,
            format!(
                "new_unsuppressed={} is within configured maximum {}",
                decision.new_unsuppressed, max_new_unsuppressed
            ),
            Some(decision.new_unsuppressed),
        )
    } else {
        (
            "failed".to_owned(),
            true,
            format!(
                "new_unsuppressed={} exceeds configured maximum {}",
                decision.new_unsuppressed, max_new_unsuppressed
            ),
            Some(decision.new_unsuppressed),
        )
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct ToolGateDecision {
    pub(crate) new_unsuppressed: u64,
}

/// ripr's `check --format badge-json` receipt, the shape the tool actually
/// ships (#316). Only the structure the threshold needs is bound: the
/// schema_version string floats across ripr releases (0.3 in 0.5.0, 0.5 in
/// 0.8.0), so the badge contract keys on the counts block, not the version.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RiprBadgeReceipt {
    pub(crate) counts: RiprBadgeCounts,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct RiprBadgeCounts {
    pub(crate) unsuppressed_exposure_gaps: u64,
}

pub(crate) enum ToolGateDecisionState {
    Missing,
    Malformed(String),
    Present(ToolGateDecision),
}

pub(crate) fn read_tool_gate_decision(path: &Path) -> ToolGateDecisionState {
    if !path.exists() {
        return ToolGateDecisionState::Missing;
    }
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) => return ToolGateDecisionState::Malformed(err.to_string()),
    };
    // Native shape first ({"new_unsuppressed": N}), then ripr's badge-json
    // (the receipt the tool actually ships, copied verbatim from sensor
    // stdout). Anything else stays malformed -> missing_evidence; a receipt
    // that parses as neither must never read as clean.
    match serde_json::from_str::<ToolGateDecision>(&text) {
        Ok(decision) => ToolGateDecisionState::Present(decision),
        Err(native_err) => match serde_json::from_str::<RiprBadgeReceipt>(&text) {
            Ok(badge) => ToolGateDecisionState::Present(ToolGateDecision {
                new_unsuppressed: badge.counts.unsuppressed_exposure_gaps,
            }),
            Err(_) => ToolGateDecisionState::Malformed(native_err.to_string()),
        },
    }
}

pub(crate) fn tool_artifact_paths(id: &str) -> Vec<String> {
    let sensor = SensorPlan {
        id: id.to_owned(),
        command: id.to_owned(),
        run: false,
        reason: String::new(),
        required: false,
        timeout_sec: 0,
        artifact_budget_mb: 0,
        class: ToolClass::Static,
        weight: 0,
        requires_lease: false,
        gate: None,
    };
    let mut paths = vec![format!("sensors/{id}/ub-review-sensor-status.json")];
    paths.extend(
        sensor_outputs(&sensor)
            .into_iter()
            .map(|output| format!("sensors/{id}/{output}")),
    );
    paths
}

pub(crate) fn trigger_description(trigger: Trigger) -> &'static str {
    match trigger {
        Trigger::Always => "every review run",
        Trigger::SourceChanged => "source file changed",
        Trigger::SourceExceptionChanged => "source-tree exception surface changed",
        Trigger::RustBehaviorOrTestsChanged => "Rust behavior or tests changed",
        Trigger::UnsafeOrNativeRiskChanged => "unsafe/native-risk surface changed",
        Trigger::WorkflowChanged => "workflow or action file changed",
        Trigger::DependencyChanged => "dependency manifest or lockfile changed",
        Trigger::ShellChanged => "shell or script file changed",
        Trigger::CppChanged => "C/C++ file changed",
        Trigger::Diff => "diff-scoped advisory scan",
        Trigger::Manual => "manual proof request",
        Trigger::Never => "disabled unless explicitly selected",
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use anyhow::Result;

    use crate::tests::test_diff;
    use crate::*;

    #[test]
    fn tool_gate_outcome_records_missing_threshold_receipt() -> Result<()> {
        let mut config: Config = toml::from_str(include_str!("../.ub-review.toml"))?;
        config.merge_defaults();
        let plan = super::build_plan(
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
            true,
        );
        let temp = tempfile::tempdir()?;
        let tool_status =
            super::tool_status_artifact(temp.path(), &config, config.selected_profile()?, &plan);
        let outcomes = super::tool_gate_outcome_artifact(
            temp.path(),
            &config,
            config.selected_profile()?,
            &tool_status,
        );
        let ripr = outcomes
            .outcomes
            .iter()
            .find(|outcome| outcome.tool == "ripr")
            .ok_or_else(|| anyhow::anyhow!("ripr gate outcome missing"))?;
        assert_eq!(ripr.schema, "ub-review.tool_gate_outcome.v1");
        assert_eq!(ripr.policy.max_new_unsuppressed, Some(0));
        assert_eq!(ripr.outcome, "missing_evidence");
        assert!(!ripr.evaluated);
        assert_eq!(
            ripr.sensor_receipt_path,
            "sensors/ripr/ub-review-sensor-status.json"
        );
        assert_eq!(ripr.packet_policy, "gate-only");
        assert_eq!(ripr.gate_policy, "trust-affecting");
        Ok(())
    }

    #[test]
    fn tool_gate_outcome_passes_with_gate_decision_receipt() -> Result<()> {
        let mut config: Config = toml::from_str(include_str!("../.ub-review.toml"))?;
        config.merge_defaults();
        let plan = super::build_plan(
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
            true,
        );
        let temp = tempfile::tempdir()?;
        let ripr = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "ripr")
            .ok_or_else(|| anyhow::anyhow!("ripr sensor missing"))?;
        super::write_sensor_status(
            temp.path(),
            ripr,
            SensorStatusWrite {
                status: "ok",
                argv: &["ripr".to_owned(), "gate".to_owned()],
                duration_ms: 12,
                reason: "ripr gate receipt ok",
                exit_code: Some(0),
                timed_out: false,
            },
        )?;
        let gate_decision = temp.path().join("sensors/ripr/gate-decision.json");
        fs::write(gate_decision, br#"{"new_unsuppressed":0}"#)?;
        let tool_status =
            super::tool_status_artifact(temp.path(), &config, config.selected_profile()?, &plan);
        let outcomes = super::tool_gate_outcome_artifact(
            temp.path(),
            &config,
            config.selected_profile()?,
            &tool_status,
        );
        let ripr = outcomes
            .outcomes
            .iter()
            .find(|outcome| outcome.tool == "ripr")
            .ok_or_else(|| anyhow::anyhow!("ripr gate outcome missing"))?;
        assert_eq!(ripr.sensor_status, "ok");
        assert_eq!(ripr.outcome, "passed");
        assert!(ripr.evaluated);
        assert_eq!(ripr.metrics.new_unsuppressed, Some(0));
        assert!(
            ripr.source_artifacts
                .iter()
                .any(|artifact| artifact == "sensors/ripr/gate-decision.json")
        );
        Ok(())
    }

    #[test]
    fn tool_gate_outcome_evaluates_ripr_badge_json_receipt() -> Result<()> {
        // The receipt ripr actually ships (#316): `check --format badge-json`
        // copied verbatim from sensor stdout. Shape captured from ripr 0.8.0;
        // the parser keys on the counts block, not the floating
        // schema_version.
        let mut config: Config = toml::from_str(include_str!("../.ub-review.toml"))?;
        config.merge_defaults();
        let plan = super::build_plan(
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
            true,
        );
        let cases = [(0u64, "passed", true), (3u64, "failed", true)];
        for (gaps, expected_outcome, expected_evaluated) in cases {
            let temp = tempfile::tempdir()?;
            let ripr = plan
                .sensors
                .iter()
                .find(|sensor| sensor.id == "ripr")
                .ok_or_else(|| anyhow::anyhow!("ripr sensor missing"))?;
            super::write_sensor_status(
                temp.path(),
                ripr,
                SensorStatusWrite {
                    status: "ok",
                    argv: &["ripr".to_owned(), "check".to_owned()],
                    duration_ms: 12,
                    reason: "completed",
                    exit_code: Some(0),
                    timed_out: false,
                },
            )?;
            let badge = format!(
                r#"{{"schema_version":"0.5","kind":"ripr","scope":"diff","basis":"finding_exposure","label":"ripr","message":"{gaps}","status":"pass","color":"brightgreen","counts":{{"unsuppressed_exposure_gaps":{gaps},"unsuppressed_test_efficiency_findings":0,"analyzed_findings":246}},"policy":{{"include_unknowns":false,"fail_on_nonzero":false}},"warnings":[]}}"#
            );
            fs::write(temp.path().join("sensors/ripr/gate-decision.json"), badge)?;
            fs::write(
                temp.path().join("sensors/ripr/exposure-gaps.json"),
                r#"{"schema":"ub-review.ripr_exposure_gaps.v1","status":"ok","total_gap_findings":0,"truncated":false,"entries":[]}"#,
            )?;
            let tool_status = super::tool_status_artifact(
                temp.path(),
                &config,
                config.selected_profile()?,
                &plan,
            );
            let outcomes = super::tool_gate_outcome_artifact(
                temp.path(),
                &config,
                config.selected_profile()?,
                &tool_status,
            );
            let ripr_outcome = outcomes
                .outcomes
                .iter()
                .find(|outcome| outcome.tool == "ripr")
                .ok_or_else(|| anyhow::anyhow!("ripr gate outcome missing"))?;
            assert_eq!(ripr_outcome.outcome, expected_outcome, "gaps={gaps}");
            assert_eq!(ripr_outcome.evaluated, expected_evaluated, "gaps={gaps}");
            assert_eq!(ripr_outcome.metrics.new_unsuppressed, Some(gaps));
            assert!(
                ripr_outcome
                    .source_artifacts
                    .iter()
                    .any(|artifact| artifact == "sensors/ripr/exposure-gaps.json"),
                "ripr tool-gate outcome should point to exposure-gap detail when present"
            );
        }
        Ok(())
    }

    #[test]
    fn tool_gate_outcome_records_malformed_gate_decision_receipt() -> Result<()> {
        let mut config: Config = toml::from_str(include_str!("../.ub-review.toml"))?;
        config.merge_defaults();
        let plan = super::build_plan(
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
            true,
        );
        let temp = tempfile::tempdir()?;
        let ripr = plan
            .sensors
            .iter()
            .find(|sensor| sensor.id == "ripr")
            .ok_or_else(|| anyhow::anyhow!("ripr sensor missing"))?;
        super::write_sensor_status(
            temp.path(),
            ripr,
            SensorStatusWrite {
                status: "ok",
                argv: &["ripr".to_owned(), "gate".to_owned()],
                duration_ms: 12,
                reason: "ripr gate receipt ok",
                exit_code: Some(0),
                timed_out: false,
            },
        )?;
        fs::write(
            temp.path().join("sensors/ripr/gate-decision.json"),
            br#"{"new_unsuppressed":"zero"}"#,
        )?;
        let tool_status =
            super::tool_status_artifact(temp.path(), &config, config.selected_profile()?, &plan);
        let outcomes = super::tool_gate_outcome_artifact(
            temp.path(),
            &config,
            config.selected_profile()?,
            &tool_status,
        );
        let ripr = outcomes
            .outcomes
            .iter()
            .find(|outcome| outcome.tool == "ripr")
            .ok_or_else(|| anyhow::anyhow!("ripr gate outcome missing"))?;
        assert_eq!(ripr.sensor_status, "ok");
        assert_eq!(ripr.outcome, "missing_evidence");
        assert!(!ripr.evaluated);
        assert!(ripr.reason.contains("gate-decision receipt is malformed"));
        assert_eq!(ripr.metrics.new_unsuppressed, None);
        assert!(
            ripr.source_artifacts
                .iter()
                .any(|artifact| artifact == "sensors/ripr/gate-decision.json")
        );
        Ok(())
    }
}
