//! #1554: exercise human projections against real dry-run packets and gate-check.
//! No provider, sensor command, proof command, or posting is executed.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde_json::Value;
use sha2::{Digest, Sha256};

use super::{cli_subprocess_test_lock, path_str, run, run_expect_failure, write_file};

const BIN: &str = env!("CARGO_BIN_EXE_ub-review");
const CONFIG: &str = "profile = \"gh-runner\"\n[repo]\nkind = \"generic\"\nledger = \"\"\n";

struct Fixture {
    temp: tempfile::TempDir,
    repo: PathBuf,
}

impl Fixture {
    fn new() -> Result<Self> {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("repo");
        fs::create_dir_all(&repo)?;
        write_file(&repo.join("README.md"), "# Before\n")?;
        run(&repo, "git", &["init"])?;
        run(
            &repo,
            "git",
            &["config", "user.email", "ub-review@example.invalid"],
        )?;
        run(&repo, "git", &["config", "user.name", "UB Review Test"])?;
        run(&repo, "git", &["add", "."])?;
        run(&repo, "git", &["commit", "-m", "baseline"])?;
        write_file(&repo.join("README.md"), "# After\n")?;
        run(&repo, "git", &["add", "."])?;
        run(&repo, "git", &["commit", "-m", "documentation change"])?;
        Ok(Self { temp, repo })
    }

    fn packet(&self, name: &str, policy: &str, enforced: bool) -> Result<PathBuf> {
        let config = self.temp.path().join(format!("{name}.toml"));
        write_file(&config, &format!("{CONFIG}{policy}"))?;
        let out = self.temp.path().join(name);
        let args = [
            "run",
            "--root",
            path_str(&self.repo)?,
            "--config",
            path_str(&config)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            path_str(&out)?,
            "--dry-run",
            "--model-mode",
            "off",
            "--posting",
            "artifact-only",
            "--mode",
            "intelligent-ci",
            "--fail-on-gate",
            if enforced { "true" } else { "false" },
            "--no-github-summary",
        ];
        if enforced {
            let failure = run_expect_failure(self.temp.path(), BIN, &args)?;
            assert!(failure.contains("gate conclusion is `fail`"), "{failure}");
        } else {
            run(self.temp.path(), BIN, &args)?;
        }
        let metrics = read_json(&out.join("review/metrics.json"))?;
        assert_eq!(metrics["models"]["model_lane_calls_attempted"], 0);
        assert!(!out.join("review/post-result.json").exists());
        Ok(out)
    }

    fn summary(&self, out: &Path) -> Result<String> {
        run(
            self.temp.path(),
            BIN,
            &["summary", "--run-dir", path_str(out)?],
        )?;
        Ok(fs::read_to_string(out.join("running-summary.md"))?)
    }

    fn gate_check(&self, out: &Path, passes: bool) -> Result<String> {
        let gate = out.join("review/gate_outcome.json");
        let args = [
            "gate-check",
            "--gate-outcome",
            path_str(&gate)?,
            "--fail-on-gate",
            "true",
        ];
        if passes {
            run(self.temp.path(), BIN, &args)?;
            Ok(String::new())
        } else {
            run_expect_failure(self.temp.path(), BIN, &args)
        }
    }
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path)?).with_context(|| format!("parse {}", path.display()))
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn assert_gate(summary: &str, conclusion: &str, reasons: usize) {
    assert!(
        summary.contains(&format!(
            "Gate: `{conclusion}` with `{reasons}` blocking reasons"
        )),
        "human result must retain the admitted gate conclusion and actual reason count:\n{summary}"
    );
}

#[test]
fn generated_pass_policy_failure_and_inconclusive_keep_enforcement_distinct() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let fixture = Fixture::new()?;
    let required = "\n[gate.blocking]\nrequired_proof_unproven = true\n\n[[proof.required]]\nid = \"required-check\"\ncommand = \"cargo check --workspace --locked\"\nreason = \"Required fixture evidence\"\ncost = \"focused-build\"\ntimeout_sec = 30\nrequired = true\n";
    for (name, policy, expected, enforced) in [
        ("pass", "", "pass", false),
        (
            "policy-fail",
            "\n[gate]\ntarget_minutes = \"oops\"\n",
            "fail",
            true,
        ),
        ("required-unproven", required, "fail", false),
        (
            "inconclusive",
            "\n[tools.tokmd]\nrequired = true\n",
            "inconclusive",
            false,
        ),
    ] {
        let out = fixture.packet(name, policy, enforced)?;
        let gate = read_json(&out.join("review/gate_outcome.json"))?;
        assert_eq!(gate["conclusion"], expected, "{gate}");
        let reasons = gate["reasons"]
            .as_array()
            .context("generated gate reasons")?;
        assert_gate(&fixture.summary(&out)?, expected, reasons.len());
        let checked = fixture.gate_check(&out, expected == "pass")?;
        if name == "policy-fail" {
            assert!(checked.contains("gate.target_minutes"), "{checked}");
        }
        if expected != "pass" {
            let reason_id = reasons
                .first()
                .and_then(|reason| reason["id"].as_str())
                .context("generated blocking reason id")?;
            assert!(checked.contains(reason_id), "{checked}");
        }
        if name == "inconclusive" {
            assert!(
                reasons
                    .iter()
                    .any(|reason| reason["kind"] == "required-sensor" && reason["id"] == "tokmd"),
                "{gate}"
            );
            assert_eq!(gate["sensor_coverage"]["required_total"], 1, "{gate}");
        }
        if name == "required-unproven" {
            assert_eq!(gate["required_proof"]["matched"], 1, "{gate}");
            assert_eq!(gate["planner_required_proofs"]["total"], 1, "{gate}");
            let original_summary = fixture.summary(&out)?;
            assert!(
                original_summary.contains("Required planner proof unproven:"),
                "{original_summary}"
            );
            let metrics_path = out.join("review/metrics.json");
            let metrics = fs::read(&metrics_path)?;
            fs::remove_file(&metrics_path)?;
            let without_metrics = fixture.summary(&out)?;
            assert_gate(&without_metrics, expected, reasons.len());
            let required_lines = original_summary
                .lines()
                .filter(|line| line.contains("Required planner proof"))
                .collect::<Vec<_>>();
            assert!(!required_lines.is_empty());
            for line in required_lines {
                assert!(
                    without_metrics.contains(line),
                    "Required task details disappeared with optional telemetry: {without_metrics}"
                );
            }
            fs::write(&metrics_path, metrics)?;
            assert_eq!(fixture.summary(&out)?, original_summary);
        }
    }
    Ok(())
}

#[test]
fn failed_gate_survives_missing_or_truncated_metrics_and_restoration() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let fixture = Fixture::new()?;
    let out = fixture.packet("failed", "\n[gate]\ntarget_minutes = \"oops\"\n", true)?;
    let gate_path = out.join("review/gate_outcome.json");
    let gate_bytes = fs::read(&gate_path)?;
    let metrics_path = out.join("review/metrics.json");
    let metrics_bytes = fs::read(&metrics_path)?;
    let original_summary = fixture.summary(&out)?;
    assert_gate(&original_summary, "fail", 1);
    for mutation in ["missing", "directory", "truncated", "null", "scalar"] {
        match mutation {
            "missing" => fs::remove_file(&metrics_path)?,
            "directory" => {
                fs::remove_file(&metrics_path)?;
                fs::create_dir(&metrics_path)?;
            }
            "truncated" => fs::write(&metrics_path, b"{\"wall_clock_seconds\":")?,
            "null" => fs::write(&metrics_path, b"null")?,
            _ => fs::write(&metrics_path, b"42")?,
        }
        let summary = fixture.summary(&out)?;
        assert_gate(&summary, "fail", 1);
        assert!(
            summary.contains("review/metrics.json")
                && summary.to_lowercase().contains("unavailable"),
            "{summary}"
        );
        if mutation == "missing" {
            assert!(
                summary.contains("`review/metrics.json` is missing"),
                "missing telemetry needs a distinct recovery diagnostic: {summary}"
            );
        } else if mutation == "directory" {
            assert!(
                summary.contains("`review/metrics.json` cannot be read"),
                "an unreadable telemetry path must not be reported as absent: {summary}"
            );
        }
        assert!(
            fixture
                .gate_check(&out, false)?
                .contains("gate.target_minutes")
        );
        assert_eq!(digest(&fs::read(&gate_path)?), digest(&gate_bytes));
        if mutation == "directory" {
            fs::remove_dir(&metrics_path)?;
        }
        fs::write(&metrics_path, &metrics_bytes)?;
        assert_eq!(fixture.summary(&out)?, original_summary);
    }
    Ok(())
}

#[test]
fn calibration_cannot_turn_invalid_current_gate_into_pass() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let fixture = Fixture::new()?;
    let out = fixture.packet("passing", "", false)?;
    let gate_path = out.join("review/gate_outcome.json");
    let original = fs::read(&gate_path)?;
    let gate: Value = serde_json::from_slice(&original)?;
    let calibration_path = out.join("review/calibration.json");
    let calibration = fs::read(&calibration_path)?;
    assert_eq!(read_json(&calibration_path)?["gate_policy"], "pass");
    let original_summary = fixture.summary(&out)?;
    assert_gate(&original_summary, "pass", 0);
    for mutation in [
        "missing",
        "directory",
        "truncated",
        "unsupported-schema",
        "mistyped-reasons",
        "unsupported-conclusion",
        "null",
        "scalar",
        "duplicate-conclusion",
        "duplicate-schema",
        "duplicate-reasons",
    ] {
        match mutation {
            "missing" => fs::remove_file(&gate_path)?,
            "directory" => {
                fs::remove_file(&gate_path)?;
                fs::create_dir(&gate_path)?;
            }
            "truncated" => fs::write(&gate_path, b"{\"schema\":")?,
            "null" => fs::write(&gate_path, b"null")?,
            "scalar" => fs::write(&gate_path, b"42")?,
            "duplicate-conclusion" | "duplicate-schema" | "duplicate-reasons" => {
                let duplicate = match mutation {
                    "duplicate-conclusion" => r#""conclusion":"fail","#,
                    "duplicate-schema" => r#""schema":"ub-review.gate_outcome.v999","#,
                    _ => {
                        r#""reasons":[{"kind":"policy","id":"duplicate-reason","detail":"blocking evidence","receipt":"effective-config.json"}],"#
                    }
                };
                let remaining = std::str::from_utf8(&original)?
                    .strip_prefix('{')
                    .context("generated gate must start with an object")?;
                // Preserve duplicate fields as raw bytes: a Value round-trip
                // would erase the ambiguity this control must expose.
                fs::write(&gate_path, format!("{{{duplicate}{remaining}"))?;
            }
            _ => {
                let mut invalid = gate.clone();
                if mutation == "unsupported-schema" {
                    invalid["schema"] = Value::String("ub-review.gate_outcome.v999".to_owned());
                } else if mutation == "mistyped-reasons" {
                    invalid["reasons"] = Value::String("not an array".to_owned());
                } else {
                    invalid["conclusion"] = Value::String("approved".to_owned());
                }
                fs::write(&gate_path, serde_json::to_vec(&invalid)?)?;
            }
        }
        let summary = fixture.summary(&out)?;
        assert!(
            !summary.contains("- Gate: `pass`") && !summary.contains("| Gate: `pass`"),
            "{mutation} promoted calibration into current pass:\n{summary}"
        );
        assert!(
            summary.contains("review/gate_outcome.json")
                && summary.to_lowercase().contains("unavailable"),
            "{mutation}: {summary}"
        );
        if mutation == "missing" {
            assert!(
                summary.contains("`review/gate_outcome.json` is missing"),
                "missing gate evidence needs a distinct recovery diagnostic: {summary}"
            );
        } else if mutation == "directory" {
            assert!(
                summary.contains("`review/gate_outcome.json` cannot be read"),
                "an unreadable gate path must not be reported as absent: {summary}"
            );
        }
        let failure = fixture.gate_check(&out, false)?;
        assert!(!failure.is_empty());
        assert_eq!(digest(&fs::read(&calibration_path)?), digest(&calibration));
        if mutation == "directory" {
            fs::remove_dir(&gate_path)?;
        }
        fs::write(&gate_path, &original)?;
        assert_eq!(digest(&fs::read(&gate_path)?), digest(&original));
        fixture.gate_check(&out, true)?;
        assert_eq!(fixture.summary(&out)?, original_summary);
    }
    Ok(())
}

#[test]
fn generated_preliminary_plan_does_not_warn_about_missing_terminal_artifacts() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let fixture = Fixture::new()?;
    let config = fixture.temp.path().join("plan.toml");
    write_file(&config, CONFIG)?;
    let out = fixture.temp.path().join("preliminary");
    run(
        fixture.temp.path(),
        BIN,
        &[
            "plan",
            "--write",
            "--root",
            path_str(&fixture.repo)?,
            "--config",
            path_str(&config)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            path_str(&out)?,
        ],
    )?;
    assert!(!out.join("review/terminal_state.json").exists());
    assert!(!out.join("review/gate_outcome.json").exists());
    assert!(!out.join("review/metrics.json").exists());
    let summary = fixture.summary(&out)?;
    assert!(!summary.contains("Gate: `pass`"), "{summary}");
    for artifact in [
        "review/gate_outcome.json",
        "review/metrics.json",
        "review/calibration.json",
    ] {
        assert!(
            !summary.contains(artifact),
            "a preliminary plan has no terminal evidence requirement: {summary}"
        );
    }
    Ok(())
}

#[test]
fn plan_for_new_revision_does_not_relabel_an_old_run_gate_as_current() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let fixture = Fixture::new()?;
    let out = fixture.packet("replanned", "", false)?;
    let gate_path = out.join("review/gate_outcome.json");
    let old_gate_bytes = fs::read(&gate_path)?;
    let old_gate: Value = serde_json::from_slice(&old_gate_bytes)?;
    let old_revision = old_gate["revision"]["digest"]
        .as_str()
        .context("generated run A revision digest")?;
    assert_gate(&fixture.summary(&out)?, "pass", 0);
    fs::remove_file(out.join("review/metrics.json"))?;

    // A real third commit selects a different file and admitted revision.
    // Preserve all other run A artifacts to exercise normal directory reuse.
    write_file(&fixture.repo.join("docs/new-plan.md"), "# New revision B\n")?;
    run(&fixture.repo, "git", &["add", "."])?;
    run(&fixture.repo, "git", &["commit", "-m", "new plan subject"])?;
    let current_head =
        super::run_capture_with_env(&fixture.repo, "git", &["rev-parse", "HEAD"], &[])?;
    let config = fixture.temp.path().join("replanned.toml");
    run(
        fixture.temp.path(),
        BIN,
        &[
            "plan",
            "--write",
            "--root",
            path_str(&fixture.repo)?,
            "--config",
            path_str(&config)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            path_str(&out)?,
        ],
    )?;
    assert_eq!(digest(&fs::read(&gate_path)?), digest(&old_gate_bytes));
    // Legacy gate-check validates the supplied old receipt, not the plan's
    // revision. Its unchanged pass cannot establish evidence for revision B.
    fixture.gate_check(&out, true)?;
    let planned_summary = fixture.summary(&out)?;
    assert!(
        planned_summary.contains("- `docs/new-plan.md`"),
        "{planned_summary}"
    );
    assert!(
        !planned_summary.contains("- `README.md`"),
        "{planned_summary}"
    );
    for stale_terminal_claim in [
        "Gate: `pass`",
        "## Calibration",
        "## Provider preflights",
        "## Model lane status",
    ] {
        assert!(
            !planned_summary.contains(stale_terminal_claim),
            "a written plan for B must not present A's terminal evidence as current: {planned_summary}"
        );
    }
    assert_eq!(digest(&fs::read(&gate_path)?), digest(&old_gate_bytes));

    let regenerated = fixture.packet("replanned", "", false)?;
    let current_gate = read_json(&regenerated.join("review/gate_outcome.json"))?;
    let current_revision = current_gate["revision"]["digest"]
        .as_str()
        .context("generated run B revision digest")?;
    assert_ne!(current_revision, old_revision);
    assert_eq!(
        current_gate["revision"]["reviewed_commit"],
        current_head.trim()
    );
    assert_gate(&fixture.summary(&regenerated)?, "pass", 0);
    fixture.gate_check(&regenerated, true)?;
    Ok(())
}

#[test]
fn same_revision_plan_reuse_keeps_existing_terminal_receipts_out_of_plan_summary() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let fixture = Fixture::new()?;
    let out = fixture.packet("same-revision", "", false)?;
    let gate_path = out.join("review/gate_outcome.json");
    let gate_bytes = fs::read(&gate_path)?;
    let config = fixture.temp.path().join("same-revision.toml");
    assert_gate(&fixture.summary(&out)?, "pass", 0);
    run(
        fixture.temp.path(),
        BIN,
        &[
            "plan",
            "--write",
            "--root",
            path_str(&fixture.repo)?,
            "--config",
            path_str(&config)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            path_str(&out)?,
        ],
    )?;
    let summary = fixture.summary(&out)?;
    for terminal_claim in [
        "Gate: `pass`",
        "## Calibration",
        "## Provider preflights",
        "## Model lane status",
    ] {
        assert!(
            !summary.contains(terminal_claim),
            "planning does not perform a review, even when the old receipt names the same revision: {summary}"
        );
    }
    assert!(summary.contains("- `README.md`"), "{summary}");
    assert_eq!(digest(&fs::read(&gate_path)?), digest(&gate_bytes));
    fixture.gate_check(&out, true)?;
    let regenerated = fixture.packet("same-revision", "", false)?;
    assert_gate(&fixture.summary(&regenerated)?, "pass", 0);
    fixture.gate_check(&regenerated, true)?;
    Ok(())
}

#[test]
fn unavailable_or_ambiguous_phase_cannot_overwrite_a_retained_summary() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let fixture = Fixture::new()?;
    let out = fixture.packet("phase-admission", "", false)?;
    let phase_path = out.join("resolved-plan.json");
    let original_phase = fs::read(&phase_path)?;
    let phase: Value = serde_json::from_slice(&original_phase)?;
    let summary_path = out.join("running-summary.md");
    let original_summary = fixture.summary(&out)?;
    let original_summary_bytes = fs::read(&summary_path)?;
    let gate_path = out.join("review/gate_outcome.json");
    let gate_bytes = fs::read(&gate_path)?;
    for mutation in [
        "missing",
        "directory",
        "malformed",
        "wrong-schema",
        "missing-schema",
        "mistyped-schema",
        "missing-run-pass",
        "mistyped-run-pass",
        "unknown-run-pass",
        "auto-run-pass",
        "duplicate-schema",
        "duplicate-run-pass",
    ] {
        match mutation {
            "missing" => fs::remove_file(&phase_path)?,
            "directory" => {
                fs::remove_file(&phase_path)?;
                fs::create_dir(&phase_path)?;
            }
            "malformed" => fs::write(&phase_path, b"{\"run_pass\":")?,
            "duplicate-schema" | "duplicate-run-pass" => {
                let prefix = if mutation == "duplicate-schema" {
                    r#""schema":"untrusted.phase.schema","#
                } else {
                    r#""run_pass":"plan-default","#
                };
                let remaining = std::str::from_utf8(&original_phase)?
                    .strip_prefix('{')
                    .context("generated phase metadata must be an object")?;
                fs::write(&phase_path, format!("{{{prefix}{remaining}"))?;
            }
            _ => {
                let mut invalid = phase.clone();
                match mutation {
                    "missing-schema" => {
                        invalid
                            .as_object_mut()
                            .context("phase object")?
                            .remove("schema");
                    }
                    "missing-run-pass" => {
                        invalid
                            .as_object_mut()
                            .context("phase object")?
                            .remove("run_pass");
                    }
                    "mistyped-schema" => invalid["schema"] = Value::Bool(true),
                    "mistyped-run-pass" => invalid["run_pass"] = Value::Array(Vec::new()),
                    "wrong-schema" => {
                        invalid["schema"] = Value::String("untrusted.phase.schema".to_owned())
                    }
                    "auto-run-pass" => invalid["run_pass"] = Value::String("auto".to_owned()),
                    _ => invalid["run_pass"] = Value::String("unknown".to_owned()),
                }
                fs::write(&phase_path, serde_json::to_vec(&invalid)?)?;
            }
        }
        let failure = run_expect_failure(
            fixture.temp.path(),
            BIN,
            &["summary", "--run-dir", path_str(&out)?],
        )?;
        assert!(
            failure.contains("resolved-plan.json"),
            "{mutation}: {failure}"
        );
        let recovery = failure.to_lowercase();
        assert!(
            recovery.contains("recover") && recovery.contains("new identified run"),
            "phase failures need bounded recovery guidance: {mutation}: {failure}"
        );
        assert_eq!(
            fs::read(&summary_path)?,
            original_summary_bytes,
            "{mutation} overwrote the retained summary"
        );
        assert_eq!(
            digest(&fs::read(&gate_path)?),
            digest(&gate_bytes),
            "{mutation} changed the old gate receipt"
        );
        fixture.gate_check(&out, true)?;
        if mutation == "directory" {
            fs::remove_dir(&phase_path)?;
        }
        fs::write(&phase_path, &original_phase)?;
        assert_eq!(fixture.summary(&out)?, original_summary);
    }
    Ok(())
}

#[test]
fn fresh_and_reused_runs_keep_terminal_evidence_out_of_preliminary_model_context() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let fixture = Fixture::new()?;
    let out = fixture.packet("reused", "", false)?;
    let fresh_context = fs::read_to_string(out.join("review/shared_context.md"))?;
    for terminal_section in [
        "## Review efficiency",
        "## Calibration",
        "- Gate:",
        "## Provider preflights",
        "## Model lane status",
    ] {
        assert!(
            !fresh_context.contains(terminal_section),
            "fresh preliminary context should not claim terminal results: {fresh_context}"
        );
    }
    for artifact in [
        "review/gate_outcome.json",
        "review/metrics.json",
        "review/calibration.json",
    ] {
        assert!(
            !fresh_context.contains(artifact),
            "fresh preliminary context should not warn about future terminal inputs: {fresh_context}"
        );
    }

    let markers = [
        "STALE_GATE_1554",
        "STALE_METRICS_1554",
        "STALE_CALIBRATION_1554",
        "STALE_REVIEW_1554",
    ];
    let gate_path = out.join("review/gate_outcome.json");
    let mut gate = read_json(&gate_path)?;
    gate["conclusion"] = Value::String("fail".to_owned());
    gate["reasons"] = serde_json::json!([{"kind":"policy", "id":markers[0], "detail":markers[0], "receipt":"effective-config.json"}]);
    fs::write(&gate_path, serde_json::to_vec(&gate)?)?;
    let metrics_path = out.join("review/metrics.json");
    let mut metrics = read_json(&metrics_path)?;
    metrics["terminal_state"] = Value::String(markers[1].to_owned());
    fs::write(&metrics_path, serde_json::to_vec(&metrics)?)?;
    let calibration_path = out.join("review/calibration.json");
    let mut calibration = read_json(&calibration_path)?;
    calibration["cohort"]["model"] = Value::String(markers[2].to_owned());
    fs::write(&calibration_path, serde_json::to_vec(&calibration)?)?;
    let review_path = out.join("review/review.json");
    let mut review = read_json(&review_path)?;
    review["provider_policy"] = Value::String(markers[3].to_owned());
    fs::write(&review_path, serde_json::to_vec(&review)?)?;

    let regenerated = fixture.packet("reused", "", false)?;
    let context = fs::read_to_string(regenerated.join("review/shared_context.md"))?;
    let summary = fixture.summary(&regenerated)?;
    for marker in markers {
        assert!(
            !context.contains(marker),
            "old terminal evidence entered the next run's model prefix: {context}"
        );
        assert!(
            !summary.contains(marker),
            "final summary retained a stale marker: {summary}"
        );
    }
    for terminal_section in [
        "## Review efficiency",
        "## Calibration",
        "- Gate:",
        "## Provider preflights",
        "## Model lane status",
    ] {
        assert!(
            !context.contains(terminal_section),
            "reused preliminary context must omit terminal sections: {context}"
        );
    }
    assert_gate(&summary, "pass", 0);
    fixture.gate_check(&regenerated, true)?;
    Ok(())
}

#[test]
fn summary_rejects_gate_receipt_from_another_revision() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let fixture = Fixture::new()?;
    let first = fixture.packet("revision-a", "", false)?;
    assert_gate(&fixture.summary(&first)?, "pass", 0);
    let first_admission = read_json(&first.join("input/revision-admission.json"))?;
    write_file(&fixture.repo.join("docs/second.md"), "# Second revision\n")?;
    run(&fixture.repo, "git", &["add", "."])?;
    run(&fixture.repo, "git", &["commit", "-m", "second revision"])?;
    let second = fixture.packet("revision-b", "", false)?;
    let second_admission = read_json(&second.join("input/revision-admission.json"))?;
    assert_ne!(
        first_admission["identity_digest"],
        second_admission["identity_digest"]
    );
    assert_gate(&fixture.summary(&second)?, "pass", 0);
    fs::copy(
        first.join("review/gate_outcome.json"),
        second.join("review/gate_outcome.json"),
    )?;
    let summary = fixture.summary(&second)?;
    assert!(
        summary.contains("Gate: unavailable"),
        "a valid gate receipt from another revision must not be shown as current: {summary}"
    );
    assert!(!summary.contains("Gate: `pass`"), "{summary}");

    // A missing or tampered admission cannot authenticate even the original
    // same-revision gate. Restore it and verify the positive control again.
    let gate_path = second.join("review/gate_outcome.json");
    let admission_path = second.join("input/revision-admission.json");
    let original_admission = fs::read(&admission_path)?;
    let original_gate = fs::read(first.join("review/gate_outcome.json"))?;
    // Restore B's gate from a fresh B packet, not A's stale receipt.
    let fresh = fixture.packet("revision-b-fresh", "", false)?;
    fs::copy(fresh.join("review/gate_outcome.json"), &gate_path)?;
    fs::remove_file(&admission_path)?;
    assert!(fixture.summary(&second)?.contains("Gate: unavailable"));
    let mut tampered: Value = serde_json::from_slice(&original_admission)?;
    tampered["identity_digest"] = Value::String("0".repeat(64));
    fs::write(&admission_path, serde_json::to_vec(&tampered)?)?;
    assert!(fixture.summary(&second)?.contains("Gate: unavailable"));
    fs::write(&admission_path, original_admission)?;
    assert_gate(&fixture.summary(&second)?, "pass", 0);
    assert_ne!(fs::read(&gate_path)?, original_gate);

    let valid_admission = fs::read(&admission_path)?;
    let valid_gate = fs::read(&gate_path)?;
    for (admission_field, gate_field, bad_value) in [
        ("semantics", "semantics", "merge_result".to_owned()),
        ("reviewed_commit_oid", "reviewed_commit", "f".repeat(40)),
    ] {
        for (mutate_admission, mutate_gate) in [(true, false), (false, true), (true, true)] {
            let mut admission: Value = serde_json::from_slice(&valid_admission)?;
            let mut gate: Value = serde_json::from_slice(&valid_gate)?;
            if mutate_admission {
                admission[admission_field] = Value::String(bad_value.clone());
            }
            if mutate_gate {
                gate["revision"][gate_field] = Value::String(bad_value.clone());
            }
            fs::write(&admission_path, serde_json::to_vec(&admission)?)?;
            fs::write(&gate_path, serde_json::to_vec(&gate)?)?;
            let summary = fixture.summary(&second)?;
            assert!(
                summary.contains("Gate: unavailable"),
                "{admission_field}: admission={mutate_admission} gate={mutate_gate}: {summary}"
            );
            assert!(!summary.contains("Gate: `pass`"), "{summary}");
        }
    }
    fs::write(&admission_path, valid_admission)?;
    fs::write(&gate_path, valid_gate)?;
    assert_gate(&fixture.summary(&second)?, "pass", 0);
    Ok(())
}
