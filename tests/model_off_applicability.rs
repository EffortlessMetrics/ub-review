//! The real CLI must distinguish configured, disabled lanes from lost evidence.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

#[path = "common/cli_run.rs"]
mod common;

fn read_json(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

enum Obligation {
    None,
    Sensor,
    UnsupportedProof,
    DeterministicFailure,
    PolicyViolation,
}

fn run_model_off_packet(root: &Path, obligation: Obligation) -> Result<PathBuf> {
    run_model_packet(root, obligation, "off")
}

fn run_model_packet(root: &Path, obligation: Obligation, model_mode: &str) -> Result<PathBuf> {
    let repo = root.join("repo");
    fs::create_dir_all(&repo)?;
    common::init_minimal_repo(&repo)?;
    let mut config = String::from(
        r#"profile = "gh-runner"
[repo]
kind = "rust"
ledger = ""
[review]
enable_default_lanes = false
[[lanes]]
id = "configured-first"
role = "First configured review lane"
focus = "Check source changes"
[[lanes]]
id = "configured-second"
role = "Second configured review lane"
focus = "Check test evidence"
"#,
    );
    // No installed tools or Cargo subprocesses are needed for this obligation.
    for tool in [
        "tokmd",
        "cargo-allow",
        "ast-grep",
        "ripr",
        "unsafe-review",
        "semgrep",
        "actionlint",
        "zizmor",
        "gitleaks",
        "cargo-fmt",
        "cargo-check",
        "cargo-test",
        "cargo-clippy",
        "cargo-doc",
        "artifact-verifier",
        "coverage",
        "osv-scanner",
        "cargo-audit",
        "cargo-deny",
        "shellcheck",
        "cppcheck",
        "test",
        "build",
        "miri",
        "cargo-mutants",
    ] {
        config.push_str(&format!(
            "\n[tools.{tool}]\nenabled = false\nrequired = {}\n",
            matches!(obligation, Obligation::Sensor) && tool == "tokmd"
        ));
    }
    if matches!(obligation, Obligation::UnsupportedProof) {
        config.push_str(
            r#"
[[proof.required]]
id = "required-unavailable-proof"
languages = ["rust"]
diff_classes = ["all"]
command = "unsupported-proof-command"
cost = "focused-test"
reason = "Required proof remains owed when model review is disabled"
required = true
enabled = true
"#,
        );
    }
    if matches!(obligation, Obligation::DeterministicFailure) {
        config.push_str(
            r#"
[tools.changed-source-policy]
id = "changed-source-policy"
command = "changed-source-policy"
class = "static"
default = "always"
enabled = true
required = true
timeout_sec = 10
[profiles.gh-runner.guards]
min_free_mem_mb = 0
min_free_disk_mb = 0
max_load_1m = 1000000.0
"#,
        );
    }
    if matches!(obligation, Obligation::PolicyViolation) {
        config.push_str("\n[gate]\nrequired_check = true\n");
    }
    let config_path = root.join("config.toml");
    fs::write(&config_path, config)?;
    let fake_bin = root.join("fake-bin");
    fs::create_dir_all(&fake_bin)?;
    let curl = fake_bin.join("curl");
    fs::write(
        &curl,
        "#!/bin/sh\nprintf called > \"$MODEL_OFF_CURL_MARKER\"\nexit 99\n",
    )?;
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o755))?;
    let source_policy = fake_bin.join("changed-source-policy");
    fs::write(
        &source_policy,
        "#!/bin/sh\nexec git diff --exit-code HEAD~1 HEAD -- src/lib.rs\n",
    )?;
    fs::set_permissions(&source_policy, fs::Permissions::from_mode(0o755))?;
    let marker = root.join("provider-called");
    let search_path = std::env::join_paths(std::iter::once(fake_bin).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))?;
    let out = root.join("packet");
    let output = common::isolated_command(env!("CARGO_BIN_EXE_ub-review"), root)
        .args([
            "run",
            "--config",
            common::path_str(&config_path)?,
            "--root",
            common::path_str(&repo)?,
            "--base",
            "HEAD~1",
            "--head",
            "HEAD",
            "--out",
            common::path_str(&out)?,
            "--mode",
            "intelligent-ci",
            "--model-mode",
            model_mode,
            "--lanes",
            "configured-first,configured-second",
            "--posting",
            "artifact-only",
            "--fail-on-gate",
            "false",
            "--no-github-summary",
        ])
        .env("PATH", search_path)
        .env("MODEL_OFF_CURL_MARKER", &marker)
        .env("UB_REVIEW_MINIMAX_API_KEY", "test-key-never-send")
        .env("UB_REVIEW_OPENCODE_API_KEY", "test-key-never-send")
        .output()?;
    ensure!(
        output.status.success(),
        "model CLI failed: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if model_mode == "off" {
        ensure!(
            !marker.exists(),
            "model-mode off invoked provider transport"
        );
        let metrics = read_json(&out.join("review/metrics.json"))?;
        for pointer in [
            "/models/provider_preflight_calls_attempted",
            "/models/model_lane_calls_attempted",
            "/follow_up_results/calls_attempted",
        ] {
            assert_eq!(metrics.pointer(pointer), Some(&json!(0)), "{pointer}");
        }
    }
    Ok(out)
}

fn assert_disabled_model_receipts(out: &Path) -> Result<()> {
    let review = read_json(&out.join("review/review.json"))?;
    let terminal = read_json(&out.join("review/terminal_state.json"))?;
    let lanes = review["model_lanes"].as_array().context("model lanes")?;
    assert_eq!(lanes.len(), 2);
    for id in ["configured-first", "configured-second"] {
        ensure!(
            lanes.iter().any(|lane| {
                lane["lane"] == id
                    && lane["status"] == "skipped"
                    && lane["reason"] == "model-mode off"
            }),
            "missing configured skipped receipt: {id}"
        );
    }
    assert_eq!(terminal["model_mode"], "off");
    assert_eq!(terminal["model_lanes"], 2);
    assert_eq!(terminal["usable_model_lanes"], 0);
    assert_eq!(review["missing_or_failed_model_evidence"], json!([]));
    let gate = read_json(&out.join("review/gate_outcome.json"))?;
    assert_eq!(
        gate["model_coverage"],
        json!({
            "lanes_total": 0,
            "lanes_usable": 0,
            "budget_exhausted_lanes": 0,
            "budget_exhausted": false,
        })
    );
    assert_eq!(gate["evidence_gaps_advisory"], 0);
    let reasons = gate["not_proven_reasons"]
        .as_array()
        .context("coverage reasons")?;
    ensure!(reasons.iter().all(|reason| {
        !reason
            .as_str()
            .unwrap_or_default()
            .starts_with("model-coverage:")
    }));
    assert!(!out.join("review/github-review.json").exists());
    let summary = fs::read_to_string(out.join("running-summary.md"))?;
    let missing = summary
        .split("## Missing or failed model evidence\n")
        .nth(1)
        .context("missing model-evidence summary section")?
        .split("\n## ")
        .next()
        .context("model-evidence summary content")?;
    ensure!(
        missing.contains("No planned model evidence is currently missing or failed.")
            && !missing.contains("model-mode off"),
        "disabled model diagnostics must not become missing evidence: {missing}"
    );
    for id in ["configured-first", "configured-second"] {
        ensure!(
            summary.contains(&format!("| `{id}` |")) && summary.contains("model-mode off"),
            "configured skipped diagnostics must remain visible: {id}"
        );
    }
    let verifier =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/verify-bun-review-artifacts.py");
    common::run(
        out,
        "python3",
        &[
            common::path_str(&verifier)?,
            common::path_str(out)?,
            "--expected-repo-kind",
            "rust",
            "--expected-review-profile",
            "bun-ub-v0",
        ],
    )?;
    Ok(())
}

#[test]
fn model_off_configured_lanes_are_not_missing_coverage() -> Result<()> {
    let _guard = common::cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let out = run_model_off_packet(temp.path(), Obligation::None)?;
    assert_disabled_model_receipts(&out)?;
    let gate = read_json(&out.join("review/gate_outcome.json"))?;
    assert_eq!(gate["conclusion"], "pass");
    assert_eq!(gate["analysis_result"], "clean");
    assert_eq!(gate["publication_result"], "not_needed");
    assert_eq!(gate["gate_result"], "pass");
    assert_eq!(gate["not_proven_reasons"], json!([]));
    let terminal = read_json(&out.join("review/terminal_state.json"))?;
    assert_eq!(terminal["status"], "artifact-only");
    assert_eq!(terminal["evidence_gaps"], 0);
    let summary = fs::read_to_string(out.join("running-summary.md"))?;
    ensure!(summary.contains("- Model review: `off` (not performed); `2` configured lanes retained as skipped diagnostics"), "summary must explicitly name model mode off");
    Ok(())
}

#[test]
fn model_off_keeps_required_sensor_gap_unproven() -> Result<()> {
    let _guard = common::cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let out = run_model_off_packet(temp.path(), Obligation::Sensor)?;
    assert_disabled_model_receipts(&out)?;
    let gate = read_json(&out.join("review/gate_outcome.json"))?;
    assert_eq!(gate["conclusion"], "inconclusive");
    assert_eq!(gate["analysis_result"], "not_proven");
    assert_eq!(gate["gate_result"], "not_proven");
    assert_eq!(gate["evidence_gaps_blocking"], 1);
    assert_eq!(gate["sensor_coverage"]["required_total"], 1);
    assert_eq!(gate["sensor_coverage"]["required_completed"], 0);
    let reasons = gate["reasons"].as_array().context("gate reasons")?;
    ensure!(
        reasons
            .iter()
            .any(|reason| reason["kind"] == "required-sensor" && reason["id"] == "tokmd")
    );
    let coverage = gate["not_proven_reasons"]
        .as_array()
        .context("coverage reasons")?;
    ensure!(coverage.iter().any(|reason| {
        reason
            .as_str()
            .unwrap_or_default()
            .starts_with("required-sensor-coverage:")
    }));
    let terminal = read_json(&out.join("review/terminal_state.json"))?;
    assert_eq!(terminal["status"], "failed-to-review");
    assert_eq!(terminal["evidence_gaps"], 1);
    let enforced = common::run_expect_failure(
        temp.path(),
        env!("CARGO_BIN_EXE_ub-review"),
        &[
            "gate-check",
            "--gate-outcome",
            common::path_str(&out.join("review/gate_outcome.json"))?,
            "--fail-on-gate",
            "true",
            "--mode",
            "intelligent-ci",
        ],
    )?;
    ensure!(
        enforced.contains("tokmd"),
        "Required sensor enforcement lost: {enforced}"
    );
    Ok(())
}

#[test]
fn model_off_keeps_required_proof_unproven() -> Result<()> {
    let _guard = common::cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let out = run_model_off_packet(temp.path(), Obligation::UnsupportedProof)?;
    assert_disabled_model_receipts(&out)?;
    let gate = read_json(&out.join("review/gate_outcome.json"))?;
    assert_eq!(
        gate["required_proof"],
        json!({
            "matched": 1, "passed": 0, "failed": 0, "skipped": 1,
        })
    );
    // Legacy enforcement is advisory unless required_proof_unproven opts in.
    assert_eq!(gate["conclusion"], "pass");
    assert_eq!(gate["analysis_result"], "not_proven");
    assert_eq!(gate["gate_result"], "not_proven");
    let reasons = gate["not_proven_reasons"]
        .as_array()
        .context("coverage reasons")?;
    ensure!(reasons.iter().any(|reason| reason.as_str()
        == Some("required-proof: 1 of 1 required proof requests produced no passing receipt")));
    let requests = read_json(&out.join("review/proof_requests.json"))?;
    let requests = requests.as_array().context("proof requests")?;
    ensure!(requests.iter().any(|request| request["required"] == true
        && request["command"] == "unsupported-proof-command"
        && request["status"] == "unsupported"));
    Ok(())
}

#[test]
fn model_off_keeps_demonstrated_required_sensor_failure_blocking() -> Result<()> {
    let _guard = common::cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let out = run_model_off_packet(temp.path(), Obligation::DeterministicFailure)?;
    assert_disabled_model_receipts(&out)?;
    let gate = read_json(&out.join("review/gate_outcome.json"))?;
    assert_eq!(gate["conclusion"], "fail");
    assert_eq!(gate["analysis_result"], "findings");
    // The existing terminal producer marks required sensor failures
    // failed-to-review, so publication and its additive gate stay unproven.
    assert_eq!(gate["gate_result"], "not_proven");
    assert_eq!(gate["publication_result"], "not_proven");
    assert_eq!(gate["terminal_status"], "failed-to-review");
    let coverage = gate["not_proven_reasons"]
        .as_array()
        .context("coverage reasons")?;
    ensure!(coverage.iter().any(|reason| reason.as_str() == Some(
        "publication: the run never reached a reviewable state, so whether a PR review was needed is unproven"
    )));
    assert_eq!(gate["evidence_gaps_blocking"], 0);
    let reasons = gate["reasons"].as_array().context("gate reasons")?;
    let finding = reasons
        .iter()
        .find(|reason| reason["kind"] == "sensor-finding")
        .context("demonstrated sensor finding")?;
    assert_eq!(finding["id"], "changed-source-policy");
    assert_eq!(
        finding["receipt"],
        "sensors/changed-source-policy/ub-review-sensor-status.json"
    );
    let receipt =
        read_json(&out.join("sensors/changed-source-policy/ub-review-sensor-status.json"))?;
    assert_eq!(receipt["status"], "failed");
    assert_eq!(receipt["exit_code"], 1);
    let stdout = fs::read_to_string(out.join("sensors/changed-source-policy/stdout.txt"))?;
    ensure!(
        stdout.contains("-    41") && stdout.contains("+    42"),
        "failure must demonstrate the changed source diff"
    );
    let enforced = common::run_expect_failure(
        temp.path(),
        env!("CARGO_BIN_EXE_ub-review"),
        &[
            "gate-check",
            "--gate-outcome",
            common::path_str(&out.join("review/gate_outcome.json"))?,
            "--fail-on-gate",
            "true",
            "--mode",
            "intelligent-ci",
        ],
    )?;
    ensure!(
        enforced.contains("changed-source-policy"),
        "demonstrated finding enforcement lost: {enforced}"
    );
    Ok(())
}

#[test]
fn model_off_keeps_malformed_required_check_policy_blocking() -> Result<()> {
    let _guard = common::cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let out = run_model_off_packet(temp.path(), Obligation::PolicyViolation)?;
    assert_disabled_model_receipts(&out)?;
    let gate = read_json(&out.join("review/gate_outcome.json"))?;
    assert_eq!(gate["conclusion"], "fail");
    assert_eq!(gate["analysis_result"], "findings");
    assert_eq!(gate["gate_result"], "finding");
    assert_eq!(gate["evidence_gaps_blocking"], 0);
    let reasons = gate["reasons"].as_array().context("gate reasons")?;
    assert_eq!(reasons.len(), 1);
    assert_eq!(reasons[0]["kind"], "policy");
    assert_eq!(reasons[0]["id"], "gate.required_check");
    assert_eq!(reasons[0]["receipt"], "effective-config.json");
    let config = read_json(&out.join("effective-config.json"))?;
    let errors = config["policy_errors"]
        .as_array()
        .context("policy errors")?;
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["section"], "gate.required_check");
    ensure!(
        errors[0]["detail"]
            .as_str()
            .unwrap_or_default()
            .contains("invalid [gate] key `required_check`")
    );
    let enforced = common::run_expect_failure(
        temp.path(),
        env!("CARGO_BIN_EXE_ub-review"),
        &[
            "gate-check",
            "--gate-outcome",
            common::path_str(&out.join("review/gate_outcome.json"))?,
            "--fail-on-gate",
            "true",
            "--mode",
            "intelligent-ci",
        ],
    )?;
    ensure!(
        enforced.contains("gate.required_check"),
        "policy enforcement lost: {enforced}"
    );
    Ok(())
}

#[test]
fn model_auto_keeps_transport_failure_missing_and_unproven() -> Result<()> {
    let _guard = common::cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let out = run_model_packet(temp.path(), Obligation::None, "auto")?;
    ensure!(temp.path().join("provider-called").exists());
    let metrics = read_json(&out.join("review/metrics.json"))?;
    assert_eq!(metrics["models"]["provider_preflight_calls_attempted"], 1);
    assert_eq!(metrics["models"]["model_lane_calls_attempted"], 0);
    let gate = read_json(&out.join("review/gate_outcome.json"))?;
    assert_eq!(gate["conclusion"], "pass");
    assert_eq!(gate["analysis_result"], "not_proven");
    assert_eq!(gate["publication_result"], "not_proven");
    assert_eq!(gate["gate_result"], "not_proven");
    ensure!(
        gate["model_coverage"]["lanes_total"]
            .as_u64()
            .unwrap_or_default()
            > 0
    );
    assert_eq!(gate["model_coverage"]["lanes_usable"], 0);
    let review = read_json(&out.join("review/review.json"))?;
    let issues = review["missing_or_failed_model_evidence"]
        .as_array()
        .context("model evidence issues")?;
    ensure!(
        issues
            .iter()
            .any(|issue| issue["status"] == "preflight_failed")
    );
    let summary = fs::read_to_string(out.join("running-summary.md"))?;
    ensure!(
        summary.contains("- Model lanes: `0/4` usable (`0` ok, `0` degraded)"),
        "enabled transport failure must retain model-lane efficiency disclosure: {summary}"
    );
    let missing = summary
        .split("## Missing or failed model evidence\n")
        .nth(1)
        .context("missing model-evidence summary section")?
        .split("\n## ")
        .next()
        .context("model-evidence summary content")?;
    ensure!(
        missing.contains("Provider preflight")
            && missing.contains("Lane `configured-first`")
            && missing.contains("preflight_failed"),
        "enabled transport failure must remain missing evidence: {missing}"
    );
    Ok(())
}
