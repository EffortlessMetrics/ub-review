//! #1341 archive advisory/delivery qualification campaign, recovered from the
//! retained `test/archive-advisory-campaign` branch onto current main.
//!
//! The candidate IS current main until this campaign lands: the executable
//! scenarios drive the source-built `ub-review` binary (`CARGO_BIN_EXE`) over
//! a scripted, credential-free transport, so the deterministic maintainer
//! cases (3–7) observe candidate content without network or provider keys.
//! Live provider cases (1–2) are qualified by the hosted Action-smoke
//! receipt; this file ships the consumer of that receipt and its negative
//! controls. The archive-bit-for-bit qualification remains with the retained
//! packaging artifacts per docs/ARCHIVE_ADVISORY_RUN.md.
//!
//! Owner: release acceptance tests. See docs/ARCHIVE_ADVISORY_RUN.md for the
//! seven-case coverage map, boundaries, and evidence.

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
#[path = "common/cli_lock.rs"]
mod cli_lock;

mod oracle {
    //! Platform-free oracles: they discriminate the campaign's checks on
    //! every host, including the crafted-fixture rejection of an incomplete
    //! or misattributed hosted model-smoke receipt.

    use anyhow::{Context, Result, ensure};
    use serde_json::{Value, json};

    /// Subset equality oracle for emitted result planes. Extra keys are
    /// tolerated; every named key must match exactly.
    pub(crate) fn checked_result(actual: &Value, expected: &Value) -> Result<()> {
        for (key, value) in expected.as_object().context("expected result object")? {
            ensure!(
                actual.get(key) == Some(value),
                "wrong result plane {key}: expected {value}, observed {}",
                actual[key]
            );
        }
        Ok(())
    }

    /// Imported deterministic outcome bytes must survive delivery untouched.
    pub(crate) fn unchanged(before: &[u8], after: &[u8]) -> Result<()> {
        ensure!(
            before == after,
            "delivery rewrote imported deterministic outcome"
        );
        Ok(())
    }

    /// The mismatched-identity specimen is exactly the submitted-state
    /// response with the foreign review id. A response sharing either the
    /// real id (987) or the pending state is not identity-negative evidence,
    /// so the campaign must refuse to count it as such.
    pub(crate) fn require_identity_negative_control(response: &Value) -> Result<()> {
        checked_result(response, &json!({"id":988,"state":"COMMENTED"}))?;
        ensure!(
            checked_result(response, &json!({"id":987})).is_err(),
            "mismatched review identity accepted by campaign oracle"
        );
        Ok(())
    }

    /// Verdict predicate for the packet-contract verifier: on the emitted
    /// structured packet the verifier must pass; on the corrupted copy it
    /// must fail with exactly the gate-schema denial and nothing else. The
    /// verifier prints its denials on stderr under its own script-name
    /// prefix, and the campaign captures stdout plus stderr.
    pub(crate) fn packet_corruption_verdict(corrupted: bool, output: &str) -> Result<()> {
        let schema_denial =
            "verify-bun-review-artifacts: gate outcome has wrong schema: 'CONTROL_INVALID_SCHEMA'";
        if corrupted {
            ensure!(
                output.trim() == schema_denial,
                "corrupted packet must fail with exactly the gate-schema denial: {output}"
            );
        } else {
            ensure!(
                output.trim().is_empty(),
                "emitted packet verification must pass without denials: {output}"
            );
        }
        Ok(())
    }

    /// Case 1/2 hosted-smoke receipt consumer. The documented Action-smoke
    /// model job runs `provider-policy: minimax-only`, so the receipt is
    /// usable advisory-review evidence only when every lane ran on the
    /// approved provider, at least `min_usable` lanes finished usable, and
    /// the grouped review reached a usable conclusion. Anything else rejects.
    pub(crate) fn smoke_receipt_verdict(
        review: &Value,
        preflights: &Value,
        min_usable: usize,
    ) -> Result<()> {
        let conclusion = review
            .get("conclusion")
            .and_then(Value::as_str)
            .context("smoke review.json has no conclusion")?;
        ensure!(
            conclusion == "pass",
            "hosted model smoke must reach a usable pass conclusion, observed {conclusion}"
        );
        let lanes = review
            .get("model_lanes")
            .and_then(Value::as_array)
            .context("smoke review.json has no model_lanes array")?;
        let approved_provider = "minimax";
        for lane in lanes {
            let provider = lane
                .get("provider")
                .and_then(Value::as_str)
                .context("smoke lane receipt has no provider")?;
            ensure!(
                provider == approved_provider,
                "hosted smoke ran lane {} on unapproved provider {provider}",
                lane.get("lane").and_then(Value::as_str).unwrap_or("?")
            );
        }
        let usable = lanes
            .iter()
            .filter(|lane| {
                // Mirror the repo verifier's usable-lane denominator
                // (scripts/verify-bun-review-artifacts.py): ok/degraded
                // lanes, with refuter lanes excluded from the count.
                matches!(
                    lane.get("status").and_then(Value::as_str),
                    Some("ok") | Some("degraded")
                ) && lane.get("lane").and_then(Value::as_str) != Some("refuter")
            })
            .count();
        ensure!(
            usable >= min_usable,
            "hosted smoke produced {usable} usable ok/degraded non-refuter lanes, expected at least {min_usable}"
        );
        for receipt in preflights.as_array().context("preflight receipts array")? {
            let provider = receipt
                .get("provider")
                .and_then(Value::as_str)
                .context("preflight receipt has no provider")?;
            ensure!(
                provider == approved_provider,
                "hosted smoke preflight targeted unapproved provider {provider}"
            );
        }
        Ok(())
    }

    #[test]
    fn campaign_oracle_rejects_wrong_result_and_delivery_rewrite() -> Result<()> {
        let expected = json!({"conclusion":"inconclusive","gate_result":"not_proven"});
        checked_result(&expected, &expected)?;
        ensure!(
            checked_result(
                &json!({"conclusion":"pass","gate_result":"pass"}),
                &expected
            )
            .is_err(),
            "wrong outcome survived"
        );
        unchanged(
            b"retained deterministic receipt",
            b"retained deterministic receipt",
        )?;
        ensure!(
            unchanged(b"retained deterministic receipt", b"rewritten by delivery").is_err(),
            "delivery override survived"
        );
        Ok(())
    }

    #[test]
    fn gate_plane_oracle_requires_exact_plane_and_rejects_drift() -> Result<()> {
        // Every plane name the campaign pins exists in the published
        // vocabulary exercised by tests/release_result_planes.rs; the oracle
        // must reject a wrong value in any single plane, a missing plane, and
        // an expected object that names nothing.
        let gate = json!({
            "conclusion": "pass",
            "gate_result": "finding",
            "analysis_result": "findings",
            "publication_result": "not_needed",
        });
        checked_result(&gate, &gate)?;
        for drifted in [
            json!({"conclusion":"fail"}),
            json!({"gate_result":"not_proven"}),
            json!({"analysis_result":"limited"}),
            json!({"publication_result":"posted"}),
            json!({"conclusion":"inconclusive","gate_result":"not_proven"}),
        ] {
            ensure!(
                checked_result(&gate, &drifted).is_err(),
                "plane drift accepted: {drifted}"
            );
        }
        Ok(())
    }

    #[test]
    fn known_packet_denial_cannot_accept_success_or_unrelated_failures() -> Result<()> {
        packet_corruption_verdict(false, "")?;
        packet_corruption_verdict(
            true,
            "verify-bun-review-artifacts: gate outcome has wrong schema: 'CONTROL_INVALID_SCHEMA'",
        )?;
        ensure!(
            packet_corruption_verdict(true, "").is_err(),
            "missing corruption denial accepted"
        );
        ensure!(
            packet_corruption_verdict(true, "review/gate_outcome.json is not an object").is_err(),
            "unrelated rejection accepted"
        );
        ensure!(
            packet_corruption_verdict(
                true,
                "verify-bun-review-artifacts: review/candidates.json is not an array"
            )
            .is_err(),
            "unrelated rejection accepted"
        );
        ensure!(
            packet_corruption_verdict(
                true,
                "verify-bun-review-artifacts: gate outcome has wrong schema: \
                 'CONTROL_INVALID_SCHEMA'\nextra denial"
            )
            .is_err(),
            "extra rejection masked"
        );
        ensure!(
            packet_corruption_verdict(false, "packet contract verified").is_err(),
            "verifier chatter accepted as silence"
        );
        Ok(())
    }

    #[test]
    fn identity_negative_control_requires_exact_mismatch_and_submitted_state() -> Result<()> {
        require_identity_negative_control(&json!({"id":988,"state":"COMMENTED"}))?;
        for response in [
            json!({"id":987,"state":"PENDING"}),
            json!({"id":987,"state":"COMMENTED"}),
            json!({"id":988,"state":"PENDING"}),
        ] {
            ensure!(
                require_identity_negative_control(&response).is_err(),
                "wrong identity/state control accepted as identity-negative evidence: {response}"
            );
        }
        checked_result(&json!({"id":987,"state":"PENDING"}), &json!({"id":987}))?;
        checked_result(&json!({"id":987,"state":"COMMENTED"}), &json!({"id":987}))?;
        Ok(())
    }

    #[test]
    fn smoke_receipt_consumer_rejects_incomplete_or_misattributed_smoke() -> Result<()> {
        let usable_lane = json!({"lane":"spec-honesty","provider":"minimax","status":"ok"});
        let degraded_lane =
            json!({"lane":"summary-pressure","provider":"minimax","status":"degraded"});
        // Refuter lanes exist and may be ok, but the repo verifier excludes
        // them from the usable denominator; the consumer must mirror that.
        let refuter_lane = json!({"lane":"refuter","provider":"minimax","status":"ok"});
        let preflights = json!([{"provider":"minimax","status":"ok"}]);
        let good =
            json!({"conclusion":"pass","model_lanes":[usable_lane, degraded_lane, refuter_lane]});
        smoke_receipt_verdict(&good, &preflights, 2)?;

        for (review, label) in [
            (
                json!({"conclusion":"inconclusive","model_lanes":[usable_lane, degraded_lane, refuter_lane]}),
                "inconclusive conclusion",
            ),
            (
                json!({"conclusion":"fail","model_lanes":[usable_lane, degraded_lane, refuter_lane]}),
                "fail conclusion",
            ),
            (
                json!({"conclusion":"pass","model_lanes":[
                    usable_lane,
                    degraded_lane,
                    {"lane":"refuter","provider":"opencode-go","status":"ok"}
                ]}),
                "unapproved provider lane",
            ),
            (
                json!({"conclusion":"pass","model_lanes":[usable_lane]}),
                "too few usable lanes",
            ),
            (
                json!({"conclusion":"pass","model_lanes":[refuter_lane, degraded_lane]}),
                "refuter lanes do not count as usable",
            ),
        ] {
            ensure!(
                smoke_receipt_verdict(&review, &preflights, 2).is_err(),
                "incomplete or misattributed smoke accepted: {label}"
            );
        }
        ensure!(
            smoke_receipt_verdict(&good, &json!([{"provider":"opencode-go","status":"ok"}]), 2)
                .is_err(),
            "unapproved provider preflight accepted"
        );
        Ok(())
    }

    /// Live hosted facet (cases 1–2). Run only after the orchestrator
    /// dispatches `action-smoke.yml` with `run_model_smoke=true` and the
    /// uploaded `ub-review-model-smoke` artifact is extracted to
    /// `UB_MODEL_SMOKE_RECEIPTS`. Applies the same verdict the crafted
    /// negative controls above must reject: usable pass conclusion, enough
    /// usable lanes, approved provider only.
    #[test]
    #[ignore = "requires the hosted action-smoke model receipt; dispatch action-smoke with run_model_smoke=true, extract the ub-review-model-smoke artifact, and point UB_MODEL_SMOKE_RECEIPTS at it"]
    fn hosted_model_smoke_receipt_qualifies_live_advisory_conclusion() -> Result<()> {
        let receipts = std::env::var("UB_MODEL_SMOKE_RECEIPTS").context(
            "UB_MODEL_SMOKE_RECEIPTS required: dispatch action-smoke with run_model_smoke=true \
             and extract the ub-review-model-smoke artifact",
        )?;
        let root = std::path::PathBuf::from(receipts);
        let review = read_json(&root.join("review/review.json"))?;
        let preflights = read_json(&root.join("review/provider-preflight-status.json"))?;
        let min_usable = std::env::var("UB_MODEL_SMOKE_MIN_USABLE_LANES")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(5);
        smoke_receipt_verdict(&review, &preflights, min_usable)
    }

    fn read_json(path: &std::path::Path) -> Result<Value> {
        Ok(serde_json::from_slice(&std::fs::read(path)?)?)
    }
}

#[cfg(all(target_os = "linux", target_arch = "x86_64"))]
mod campaign {
    //! Executable deterministic campaign over the source-built candidate
    //! binary. Requires Linux x86_64 (`/usr/bin/git`, `/usr/bin/python3`,
    //! Unix permissions); the repository gate runs it on Linux, so the
    //! scenarios here are gate-covered.

    use anyhow::{Context, Result, ensure};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::Duration;
    use wait_timeout::ChildExt;

    use super::oracle::{
        checked_result, packet_corruption_verdict, require_identity_negative_control, unchanged,
    };
    use crate::cli_lock::cli_subprocess_test_lock;

    /// Private-audit positive control: must exist in the lane's private audit
    /// artifact and must never reach the public payload or the transport log.
    const PRIVATE_AUDIT_SENTINEL: &str = "PRIVATE_ARCHIVE_AUDIT_SENTINEL";
    const FINDING_SUMMARY: &str =
        "Archive fixture needs a boundary regression for the changed return value.";
    const RUN_SCENARIOS: &[&str] = &[
        "structured-finding",
        "provider-fallback",
        "provider-outage",
        "provider-text",
        "missing-key",
        "model-off",
        "required-gap",
    ];
    const VERIFIER_ARGS: [&str; 2] = ["--expected-repo-kind", "rust"];

    fn sha(bytes: &[u8]) -> String {
        format!("{:x}", Sha256::digest(bytes))
    }
    fn read(path: &Path) -> Result<Value> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }
    fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
        fs::create_dir_all(path.parent().context("parent")?)?;
        fs::write(path, bytes)?;
        Ok(())
    }
    fn script(path: &Path, body: &str) -> Result<()> {
        write(path, body)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
        Ok(())
    }
    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|value| (*value).to_owned()).collect()
    }
    fn copy_packet(from: &Path, to: &Path) -> Result<()> {
        fs::create_dir(to)?;
        for entry in fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_packet(&entry.path(), &target)?;
            } else {
                ensure!(
                    entry.file_type()?.is_file(),
                    "regular packet files required"
                );
                fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }

    // No network client is present: even an unknown URL cannot escape this
    // adapter. Each route first demands the synthetic fixture credential of
    // its endpoint family; nothing here can reach a real provider or GitHub.
    const TRANSPORT: &str = r#"#!/usr/bin/python3
import json, os, sys
argv = sys.argv[1:]
method = 'GET'; url = ''; body_path = None
headers = sys.stdin.read()
for index, value in enumerate(argv):
    if value == '-X': method = argv[index + 1]
    if value == '--data-binary': body_path = argv[index + 1][1:]
    if value.startswith('https://'): url = value
scenario = json.load(open(os.environ['CAMPAIGN_TRANSPORT']))
status = 200
result = {}
minimax_url = scenario.get('minimax_url')
opencode_url = scenario.get('opencode_url')
if minimax_url and url == minimax_url and method == 'POST':
    if 'X-Api-Key: test-token-redacted' not in headers: sys.exit(94)
    if scenario['provider'] in ('outage', 'primary-down'):
        status = 503; result = {'error': {'message': 'synthetic primary unavailable'}}
    else:
        result = {'content': [{'type': 'text', 'text': scenario['content']}]}
elif opencode_url and url == opencode_url and method == 'POST':
    if 'Authorization: Bearer test-token-redacted' not in headers: sys.exit(94)
    result = {'content': [{'type': 'text', 'text': scenario['content']}]}
elif url == 'https://github.invalid/repos/example/repo/pulls/123' and method == 'GET':
    result = {'head': {'sha': scenario['head']}}
elif url == 'https://github.invalid/repos/example/repo/pulls/123/reviews' and method == 'POST':
    status = 201; result = {'id': 987, 'state': 'PENDING'}
elif url == 'https://github.invalid/repos/example/repo/pulls/123/reviews/987/comments' and method == 'GET':
    result = []
elif url == 'https://github.invalid/repos/example/repo/pulls/123/reviews/987/events' and method == 'POST':
    if scenario['post'] == 'error':
        status = 500; result = {'message': 'synthetic submit error'}
    elif scenario['post'] == 'mismatched':
        result = {'id': 988, 'state': 'COMMENTED', 'body': 'synthetic submission'}
    else:
        result = {'id': 987, 'state': 'COMMENTED', 'body': 'synthetic submission'}
elif url == 'https://github.invalid/repos/example/repo/pulls/123/reviews/987' and method == 'DELETE':
    status = 204; result = {}
else:
    print('unallowlisted transport: ' + method + ' ' + url, file=sys.stderr); sys.exit(95)
with open(os.environ['CAMPAIGN_TRANSPORT_LOG'], 'a') as log:
    body = json.load(open(body_path)) if body_path else None
    log.write(json.dumps({'method': method, 'url': url, 'body': body, 'status': status}) + '\n')
print(json.dumps(result))
print('UB_REVIEW_HTTP_STATUS:' + str(status))
sys.exit(22 if status >= 400 else 0)
"#;

    struct Campaign {
        root: PathBuf,
        bin: PathBuf,
        binary_sha256: String,
        fixture_head: String,
        commands: Vec<Value>,
        scenarios: Value,
    }

    impl Campaign {
        fn receipt(&self, status: &str, error: Option<String>) -> Result<()> {
            write(
                &self.root.join("receipt.json"),
                serde_json::to_vec_pretty(&json!({
                    "schema": "ub-review.archive_advisory_campaign.v2",
                    "phase": "source_candidate_deterministic",
                    "status": status,
                    "error": error,
                    "candidate_binary_sha256": self.binary_sha256,
                    "candidate_version": env!("CARGO_PKG_VERSION"),
                    "fixture_head": self.fixture_head,
                    "harness_sha256": sha(include_bytes!("archive_advisory_run.rs")),
                    "transport_adapter_sha256": sha(TRANSPORT.as_bytes()),
                    "commands": self.commands,
                    "scenarios": self.scenarios,
                    "boundaries": {
                        "provider": "inert synthetic curl adapter",
                        "github": "inert synthetic curl adapter",
                        "ambient_environment": "cleared",
                        "live_calls": false
                    },
                    "pending": [
                        "archive bit-for-bit qualification over the retained packaging artifacts (runbook)",
                        "hosted action-smoke live facets for cases 1-2 (orchestrator dispatch)",
                        "platform matrix",
                        "general process-tree containment"
                    ]
                }))?,
            )
        }

        fn invoke(
            &mut self,
            program: &Path,
            cwd: &Path,
            args: &[String],
            env: &[(&str, String)],
        ) -> Result<(bool, String)> {
            if program == self.bin {
                ensure!(
                    fs::symlink_metadata(program)?.is_file(),
                    "selected binary must remain regular"
                );
                ensure!(
                    sha(&fs::read(program)?) == self.binary_sha256,
                    "selected candidate binary moved"
                );
            }
            let index = self.commands.len();
            let logs = self.root.join(format!("logs/{index}"));
            fs::create_dir_all(&logs)?;
            let mut cmd = Command::new(program);
            cmd.args(args)
                .current_dir(cwd)
                .env_clear()
                .env("PATH", self.root.join("bin"))
                .env("HOME", &self.root)
                .env("LC_ALL", "C")
                .stdin(Stdio::null())
                .stdout(fs::File::create(logs.join("stdout"))?)
                .stderr(fs::File::create(logs.join("stderr"))?);
            for (key, value) in env {
                cmd.env(key, value);
            }
            self.commands.push(
                json!({"program": program, "cwd": cwd, "argv": args, "state": "attempted",
                       "environment": env.iter()
                           .filter(|(key, _)| !key.ends_with("KEY") && *key != "GITHUB_TOKEN")
                           .map(|(key, value)| json!([key, value]))
                           .collect::<Vec<_>>(),
                       "timeout_seconds": 30}),
            );
            self.receipt("running", None)?;
            let mut child = match cmd.spawn() {
                Ok(child) => child,
                Err(error) => {
                    self.commands[index]["state"] = json!("spawn_failed");
                    return Err(error.into());
                }
            };
            let status = match child.wait_timeout(Duration::from_secs(30))? {
                Some(status) => status,
                None => {
                    child.kill()?;
                    child.wait()?;
                    self.commands[index]["state"] = json!("timed_out_direct_child_reaped");
                    anyhow::bail!("command timed out: {}", logs.display());
                }
            };
            let stdout = fs::read(logs.join("stdout"))?;
            let stderr = fs::read(logs.join("stderr"))?;
            self.commands[index]["state"] = json!("exited");
            self.commands[index]["exit"] = json!(status.code());
            self.commands[index]["stdout_sha256"] = json!(sha(&stdout));
            self.commands[index]["stderr_sha256"] = json!(sha(&stderr));
            self.receipt("running", None)?;
            Ok((
                status.success(),
                format!(
                    "{}{}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                ),
            ))
        }

        fn fixture_git(&mut self, cwd: &Path, args: &[String]) -> Result<String> {
            let (ok, text) = self.invoke(Path::new("/usr/bin/git"), cwd, args, &[])?;
            ensure!(ok, "fixture git {args:?} failed: {text}");
            Ok(text)
        }

        fn cli(&mut self, args: &[String], env: &[(&str, String)]) -> Result<(bool, String)> {
            let bin = self.bin.clone();
            let root = self.root.clone();
            self.invoke(&bin, &root, args, env)
        }

        fn verify_packet(
            &mut self,
            verifier: &Path,
            packet: &Path,
            cwd: &Path,
        ) -> Result<(bool, String)> {
            let mut args = vec![verifier.display().to_string(), packet.display().to_string()];
            args.extend(strings(&VERIFIER_ARGS));
            self.invoke(Path::new("/usr/bin/python3"), cwd, &args, &[])
        }
    }

    /// The fixture config disables every builtin sensor so the campaign
    /// observes model/delivery behavior alone. If a builtin tool is added
    /// without extending this list, its missing command surfaces as sensor
    /// evidence and the pinned result planes below fail loudly.
    fn config(required_gap: bool) -> String {
        let mut text = String::from(
            "profile = \"gh-runner\"\n\
             [repo]\nkind = \"rust\"\nledger = \"\"\n\
             [review]\nenable_default_lanes = false\n\
             [[lanes]]\nid = \"archive-lane\"\n\
             role = \"Controlled archive fixture\"\n\
             focus = \"Inspect changed return value\"\n",
        );
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
            text.push_str(&format!(
                "\n[tools.{tool}]\nenabled = false\nrequired = {}\n",
                required_gap && tool == "tokmd"
            ));
        }
        text
    }

    fn structured_content() -> String {
        json!({
            "summary": FINDING_SUMMARY,
            "inline_comments": [],
            "summary_only_findings": [{
                "severity": "medium",
                "confidence": "high",
                "reason": FINDING_SUMMARY,
                "evidence": "src/lib.rs return changes from 41 to 42 without a regression test."
            }],
            "internal_audit": {
                "surfaces_checked": [PRIVATE_AUDIT_SENTINEL],
                "strongest_rejected_hypothesis": PRIVATE_AUDIT_SENTINEL,
                "remaining_local_uncertainty": null
            }
        })
        .to_string()
    }

    struct ScenarioSpec {
        id: &'static str,
        provider: &'static str,
        content: String,
        mode: &'static str,
        model_mode: &'static str,
        provider_policy: &'static str,
        posting: &'static str,
        with_opencode: bool,
        provider_key: bool,
        required_gap: bool,
    }

    fn scenario_specs() -> Vec<ScenarioSpec> {
        vec![
            ScenarioSpec {
                id: "structured-finding",
                provider: "success",
                content: structured_content(),
                mode: "review-byok",
                model_mode: "auto",
                provider_policy: "minimax-only",
                posting: "review",
                with_opencode: false,
                provider_key: true,
                required_gap: false,
            },
            ScenarioSpec {
                id: "provider-fallback",
                provider: "primary-down",
                content: structured_content(),
                mode: "review-byok",
                model_mode: "auto",
                provider_policy: "primary-with-fallback",
                posting: "artifact-only",
                with_opencode: true,
                provider_key: true,
                required_gap: false,
            },
            ScenarioSpec {
                id: "provider-outage",
                provider: "outage",
                content: String::new(),
                mode: "review-byok",
                model_mode: "auto",
                provider_policy: "minimax-only",
                posting: "artifact-only",
                with_opencode: false,
                provider_key: true,
                required_gap: false,
            },
            ScenarioSpec {
                id: "provider-text",
                provider: "success",
                content: "Readable synthetic lane evidence without structured JSON".to_owned(),
                mode: "review-byok",
                model_mode: "auto",
                provider_policy: "minimax-only",
                posting: "artifact-only",
                with_opencode: false,
                provider_key: true,
                required_gap: false,
            },
            ScenarioSpec {
                id: "missing-key",
                provider: "success",
                content: String::new(),
                mode: "review-byok",
                model_mode: "auto",
                provider_policy: "minimax-only",
                posting: "artifact-only",
                with_opencode: false,
                provider_key: false,
                required_gap: false,
            },
            ScenarioSpec {
                id: "model-off",
                provider: "success",
                content: String::new(),
                mode: "review-byok",
                model_mode: "off",
                provider_policy: "minimax-only",
                posting: "artifact-only",
                with_opencode: false,
                provider_key: true,
                required_gap: false,
            },
            ScenarioSpec {
                id: "required-gap",
                provider: "success",
                content: String::new(),
                mode: "intelligent-ci",
                model_mode: "off",
                provider_policy: "minimax-only",
                posting: "artifact-only",
                with_opencode: false,
                provider_key: true,
                required_gap: true,
            },
        ]
    }

    /// Pinned current-contract planes for each deterministic scenario. These
    /// were observed against the source candidate (see the PR evidence); they
    /// are the contract the release campaign qualifies, and the wrong-outcome
    /// control below rejects any single-plane drift.
    fn expected_planes(id: &str) -> Result<Value> {
        Ok(match id {
            // Observed current contract: a run that prepares a review payload
            // labels `publication_result: posted` from preparation alone
            // (#959 producer label); the in-packet post then actually
            // confirms delivery, so the label is backed by a receipt here.
            "structured-finding" | "provider-fallback" => {
                json!({"conclusion":"pass","gate_result":"pass","analysis_result":"findings","publication_result":"posted"})
            }
            "provider-outage" | "missing-key" => {
                json!({"conclusion":"pass","gate_result":"not_proven","analysis_result":"not_proven","publication_result":"not_proven"})
            }
            "provider-text" => {
                json!({"conclusion":"pass","gate_result":"pass","analysis_result":"limited","publication_result":"not_needed"})
            }
            "model-off" => {
                json!({"conclusion":"pass","gate_result":"not_proven","analysis_result":"not_proven","publication_result":"not_needed"})
            }
            "required-gap" => {
                json!({"conclusion":"inconclusive","gate_result":"not_proven","analysis_result":"not_proven","publication_result":"not_proven"})
            }
            other => anyhow::bail!("unknown scenario {other}"),
        })
    }

    /// Pinned terminal states, observed against the source candidate (see the
    /// PR evidence) and asserted against both gate_outcome.terminal_status
    /// and review/terminal_state.json so the recorded value cannot drift from
    /// the enforced one.
    fn expected_terminal(id: &str) -> Result<&'static str> {
        Ok(match id {
            "structured-finding" | "provider-fallback" => "needs-reviewer-attention",
            "provider-outage" | "missing-key" | "required-gap" => "failed-to-review",
            "provider-text" => "sufficient",
            "model-off" => "artifact-only",
            other => anyhow::bail!("unknown scenario {other}"),
        })
    }

    fn run_scenario(
        c: &mut Campaign,
        spec: &ScenarioSpec,
        repo: &Path,
        verifier: &Path,
    ) -> Result<Value> {
        let root = c.root.join(spec.id);
        fs::create_dir(&root)?;
        let cfg = root.join("config.toml");
        write(&cfg, config(spec.required_gap))?;
        let transport = root.join("transport.json");
        let log = root.join("transport.ndjson");
        let mut scenario = json!({
            "provider": spec.provider,
            "post": "success",
            "head": c.fixture_head,
            "content": spec.content,
            "minimax_url": "https://provider.invalid/v1/messages",
            "opencode_url": if spec.with_opencode {
                json!("https://fallback.invalid/zen/go/v1/messages")
            } else {
                json!(null)
            },
        });
        if !spec.with_opencode {
            scenario
                .as_object_mut()
                .context("scenario object")?
                .remove("opencode_url");
        }
        write(&transport, serde_json::to_vec(&scenario)?)?;
        let out = root.join("packet");
        let mut args = strings(&["run", "--config"]);
        args.push(cfg.display().to_string());
        args.extend(strings(&["--root"]));
        args.push(repo.display().to_string());
        args.extend(strings(&["--base", "HEAD~1", "--head", "HEAD", "--out"]));
        args.push(out.display().to_string());
        args.extend(strings(&[
            "--mode",
            spec.mode,
            "--fail-on-gate",
            "false",
            "--model-mode",
            spec.model_mode,
            "--provider-policy",
            spec.provider_policy,
            "--minimax-provider-kind",
            "anthropic",
            "--lanes",
            "archive-lane",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "1",
            "--max-inline-comments",
            "0",
            "--model-timeout-sec",
            "5",
            "--run-pass",
            "opened",
            "--posting",
            spec.posting,
            "--github-repo",
            "example/repo",
            "--github-pull-number",
            "123",
            "--no-github-summary",
        ]));
        let mut env = vec![
            (
                "CAMPAIGN_TRANSPORT".to_owned(),
                transport.display().to_string(),
            ),
            (
                "CAMPAIGN_TRANSPORT_LOG".to_owned(),
                log.display().to_string(),
            ),
            (
                "UB_REVIEW_MINIMAX_API_URL".to_owned(),
                "https://provider.invalid/v1/messages".to_owned(),
            ),
        ];
        if spec.provider_key {
            env.push((
                "UB_REVIEW_MINIMAX_API_KEY".to_owned(),
                "test-token-redacted".to_owned(),
            ));
        }
        if spec.with_opencode {
            env.push((
                "UB_REVIEW_OPENCODE_API_URL".to_owned(),
                "https://fallback.invalid/zen/go/v1/messages".to_owned(),
            ));
            env.push((
                "UB_REVIEW_OPENCODE_API_KEY".to_owned(),
                "test-token-redacted".to_owned(),
            ));
        }
        let env_refs = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>();
        let (ok, text) = c.cli(&args, &env_refs)?;
        ensure!(ok, "{} run failed: {text}", spec.id);

        // Case 6 prepared phase: a prepared payload must not claim delivery
        // before any post step ran.
        if spec.id == "structured-finding" {
            ensure!(
                !out.join("review/post-result.json").exists(),
                "prepared payload claimed transport confirmation"
            );
        }
        // The workflow contract runs the post step after every run; packets
        // without a prepared payload carry the skip receipt through the
        // tokenless post path, and prepared payloads are delivered over the
        // scripted transport inside the packet.
        let post_receipt = in_packet_post(c, &out, &root)?;

        // Packet contract: the emitted packet must satisfy the ordinary
        // verifier; a deliberately corrupted copy must fail with exactly the
        // gate-schema denial (corruption control, run once below).
        let (verified, verification) = c.verify_packet(verifier, &out, &root)?;
        ensure!(
            verified,
            "{} packet contract failed: {verification}",
            spec.id
        );

        let gate = read(&out.join("review/gate_outcome.json"))?;
        let terminal = read(&out.join("review/terminal_state.json"))?;
        let review = read(&out.join("review/review.json"))?;
        let expected = expected_planes(spec.id)?;
        checked_result(&gate, &expected)?;
        let mut wrong = expected.clone();
        wrong["conclusion"] = json!("fail");
        ensure!(
            checked_result(&gate, &wrong).is_err(),
            "{} wrong-outcome control accepted",
            spec.id
        );
        // The pinned terminal state must hold in both planes that record it,
        // so the recorded value cannot drift from the enforced one.
        let expected_terminal = expected_terminal(spec.id)?;
        checked_result(&gate, &json!({"terminal_status": expected_terminal}))?;
        checked_result(&terminal, &json!({"status": expected_terminal}))?;

        let calls = if log.exists() {
            fs::read_to_string(&log)?
        } else {
            String::new()
        };
        let expected_calls = match spec.id {
            "model-off" | "required-gap" | "missing-key" => 0,
            // Observed against the source candidate: the outage denominator
            // is the preflight plus one provider attempt, both refused.
            "provider-outage" => 2,
            "provider-fallback" => 3,
            _ => 2,
        };
        ensure!(
            calls.lines().count() == expected_calls,
            "{} unexpected provider denominator: {} calls, expected {expected_calls}",
            spec.id,
            calls.lines().count()
        );
        if spec.id == "provider-outage" {
            let rows = calls_rows(&log)?;
            ensure!(
                rows.iter().all(|row| row["method"] == "POST"
                    && row["url"]
                        .as_str()
                        .is_some_and(|url| url.starts_with("https://provider.invalid/"))
                    && row["status"] == 503),
                "outage scenario observed non-refused transport rows: {rows:?}"
            );
        }

        // Revision identity (case 7): the emitted packet must bind the
        // reviewed revision to the fixture's exact head object.
        let admission = read(&out.join("input/revision-admission.json"))?;
        checked_result(
            &admission,
            &json!({
                "schema": "ub-review.revision_admission.v1",
                "semantics": "candidate_head",
                "reviewed_commit_oid": c.fixture_head,
            }),
        )?;
        let admission_digest = admission
            .get("identity_digest")
            .and_then(Value::as_str)
            .context("admission identity_digest")?;
        ensure!(
            admission_digest.len() == 64
                && admission_digest.chars().all(|ch| ch.is_ascii_hexdigit()),
            "revision identity digest must be sha256-width hex: {admission_digest}"
        );

        if spec.id == "structured-finding" {
            // Case 7 payload checks: the public summary carries the finding,
            // the private audit positive control stays private, and the
            // bounded inline budget is respected.
            let payload = fs::read_to_string(out.join("review/github-review.json"))?;
            ensure!(
                payload.contains(FINDING_SUMMARY),
                "public finding lost from prepared payload"
            );
            ensure!(
                !payload.contains(PRIVATE_AUDIT_SENTINEL),
                "private audit leaked into public payload"
            );
            let public_review = read(&out.join("review/github-review.json"))?;
            checked_result(&public_review, &json!({"comments": []}))?;
            let audit_path = out.join("review/model/archive-lane/internal_audit.json");
            let audit = fs::read_to_string(&audit_path)?;
            ensure!(
                audit.contains(PRIVATE_AUDIT_SENTINEL),
                "private positive control lost: {}",
                audit_path.display()
            );
            // Case 6 confirmed side: the in-packet post must have submitted
            // the pending review 987 to the COMMENTED state, and the submitted
            // identity must match the actual pending response.
            checked_result(
                &post_receipt,
                &json!({"schema_version":1,"status":"ok","http_status":200,"comments":0}),
            )?;
            checked_result(
                &post_receipt["response"],
                &json!({"id":987,"state":"COMMENTED"}),
            )?;
            let pending = read(&out.join("review/pending-review-stdout.json"))?;
            checked_result(&pending, &json!({"id":987,"state":"PENDING"}))?;
            ensure!(
                post_receipt["response"]["id"] == pending["id"],
                "submitted review identity does not match actual pending response"
            );
            let transaction = read(&out.join("review/delivery-transaction.json"))?;
            checked_result(&transaction, &json!({"state":"receipts_persisted"}))?;
            // Corruption control: an ordinary copy with a broken gate schema
            // must be rejected with exactly the schema denial.
            let corrupt = root.join("invalid-gate-control");
            copy_packet(&out, &corrupt)?;
            let path = corrupt.join("review/gate_outcome.json");
            let mut gate_copy = read(&path)?;
            gate_copy["schema"] = json!("CONTROL_INVALID_SCHEMA");
            write(&path, serde_json::to_vec_pretty(&gate_copy)?)?;
            let (_, corruption) = c.verify_packet(verifier, &corrupt, &root)?;
            packet_corruption_verdict(true, &corruption)?;
        }
        if spec.id == "provider-fallback" {
            // Case 2 discriminator: review completed on the approved
            // fallback provider, with the primary named as the fallback
            // source.
            let lanes = review
                .get("model_lanes")
                .and_then(Value::as_array)
                .context("model_lanes")?;
            ensure!(
                lanes.iter().any(|lane| lane["provider"] == "opencode-go"
                    && lane["status"] == "ok"
                    && lane["fallback_from"]
                        .as_str()
                        .is_some_and(|from| from.starts_with("minimax"))),
                "no lane completed review via the approved fallback: {review}"
            );
        }
        if spec.id == "provider-text" {
            ensure!(
                review["model_lanes"]
                    .as_array()
                    .context("lanes")?
                    .iter()
                    .any(|lane| lane["status"] == "degraded"),
                "text lane was not retained as degraded"
            );
        }
        if spec.id == "required-gap" {
            // Case 5 cause discriminator: the inconclusive conclusion must
            // name the tokmd required-sensor gap. Without this, a run whose
            // inconclusive came from some other cause would still pass the
            // plane pins.
            let reasons = gate
                .get("reasons")
                .and_then(Value::as_array)
                .context("gate reasons")?;
            ensure!(
                reasons
                    .iter()
                    .any(|reason| reason["kind"] == "required-sensor" && reason["id"] == "tokmd"),
                "required-gap conclusion lost the tokmd required-sensor reason: {reasons:?}"
            );
        }
        Ok(json!({
            "status": "observed_current_contract",
            "expected": expected,
            "gate": gate,
            "terminal_status": terminal.get("status"),
            "config_sha256": sha(&fs::read(&cfg)?),
            "transport_sha256": sha(&fs::read(&transport)?),
            "transport_log_sha256": sha(calls.as_bytes()),
            "provider_calls": calls.lines().count(),
            "packet_path": out,
            "packet_contract_verified": verified,
            "packet_verifier_output": verification,
            "gate_sha256": sha(&fs::read(out.join("review/gate_outcome.json"))?),
            "review_sha256": sha(&fs::read(out.join("review/review.json"))?),
            "revision_admission_sha256": sha(&fs::read(out.join("input/revision-admission.json"))?),
            "post_receipt_sha256": sha(&serde_json::to_vec(&post_receipt)?),
            "wrong_outcome_rejected": true
        }))
    }

    /// The workflow contract: run, then post into the same packet directory.
    /// A prepared payload is delivered over the scripted transport with the
    /// synthetic token; a skip receipt rides the tokenless skip path. The
    /// post phase uses its own transport log so the run-phase provider
    /// denominator stays exact.
    fn in_packet_post(c: &mut Campaign, out: &Path, root: &Path) -> Result<Value> {
        let before = fs::read(out.join("review/gate_outcome.json"))?;
        let payload = out.join("review/github-review.json");
        let transport = root.join("post-transport.json");
        let log = root.join("post-transport.ndjson");
        write(
            &transport,
            serde_json::to_vec(&json!({"head": c.fixture_head, "post": "success"}))?,
        )?;
        let review_dir = out.join("review");
        let mut args = strings(&["post", "--review-json"]);
        args.push(payload.display().to_string());
        args.extend(strings(&["--out"]));
        args.push(review_dir.display().to_string());
        args.extend(strings(&["--repo", "example/repo", "--pull-number", "123"]));
        let env = [
            (
                "CAMPAIGN_TRANSPORT".to_owned(),
                transport.display().to_string(),
            ),
            (
                "CAMPAIGN_TRANSPORT_LOG".to_owned(),
                log.display().to_string(),
            ),
        ];
        if payload.exists() {
            args.extend(strings(&[
                "--github-api-url",
                "https://github.invalid",
                "--github-token",
                "test-token-redacted",
            ]));
        }
        let env_refs = env
            .iter()
            .map(|(key, value)| (key.as_str(), value.clone()))
            .collect::<Vec<_>>();
        let (ok, text) = c.cli(&args, &env_refs)?;
        ensure!(ok, "post failed: {text}");
        unchanged(&before, &fs::read(out.join("review/gate_outcome.json"))?)?;
        let receipt_path = review_dir.join("post-result.json");
        if !receipt_path.exists() {
            let error_receipt = review_dir.join("post-error.json");
            let detail = if error_receipt.exists() {
                let receipt = read(&error_receipt)?;
                format!(
                    "error_kind={} failure_stage={} reason={}",
                    receipt
                        .get("error_kind")
                        .and_then(Value::as_str)
                        .unwrap_or("?"),
                    receipt
                        .get("failure_stage")
                        .and_then(Value::as_str)
                        .unwrap_or("?"),
                    receipt.get("reason").and_then(Value::as_str).unwrap_or("?")
                )
            } else {
                "no error receipt".to_owned()
            };
            anyhow::bail!(
                "post did not confirm; payload_exists={}; post said {text}; {detail}",
                payload.exists()
            );
        }
        read(&receipt_path)
    }

    /// Case 6/7 delivery failure and identity controls. Confirmed delivery
    /// itself is observed in-packet (`in_packet_post`); these controls pin
    /// the failed-submit cleanup and the mismatched submitted identity.
    /// Nothing here touches real GitHub.
    fn posts(c: &mut Campaign, packet: &Path) -> Result<()> {
        let before = fs::read(packet.join("review/gate_outcome.json"))?;
        {
            let id = "post-error";
            let root = c.root.join(id);
            fs::create_dir(&root)?;
            let transport = root.join("transport.json");
            let log = root.join("transport.ndjson");
            write(
                &transport,
                serde_json::to_vec(&json!({"head": c.fixture_head, "post": "error"}))?,
            )?;
            let mut args = strings(&["post", "--review-json"]);
            args.push(
                packet
                    .join("review/github-review.json")
                    .display()
                    .to_string(),
            );
            args.extend(strings(&["--out"]));
            args.push(root.display().to_string());
            args.extend(strings(&[
                "--repo",
                "example/repo",
                "--pull-number",
                "123",
                "--github-api-url",
                "https://github.invalid",
                "--github-token",
                "test-token-redacted",
                "--fail-on-post-error",
            ]));
            let env = [
                ("CAMPAIGN_TRANSPORT", transport.display().to_string()),
                ("CAMPAIGN_TRANSPORT_LOG", log.display().to_string()),
            ];
            let (ok, text) = c.cli(&args, &env)?;
            ensure!(!ok, "{id} unexpected post exit: {text}");
            unchanged(&before, &fs::read(packet.join("review/gate_outcome.json"))?)?;
            let receipt = read(&root.join("post-error.json"))?;
            checked_result(
                &receipt,
                &json!({
                    "schema_version":1,
                    "status":"failed",
                    "failure_tolerated":false,
                    "fail_on_post_error":true,
                    "http_status":500,
                    "error_kind":"post_http_error",
                    "failure_stage":"network_post"
                }),
            )?;
            let transaction = read(&root.join("delivery-transaction.json"))?;
            checked_result(
                &transaction,
                &json!({"state":"cleaned_up","cleanup":{"status":"succeeded"}}),
            )?;
            let rows = calls_rows(&log)?;
            ensure!(
                rows.iter().any(|row| row["method"] == "POST"
                    && row["url"]
                        .as_str()
                        .is_some_and(|url| url.ends_with("/reviews/987/events"))
                    && row["status"] == 500),
                "missing failed submit control"
            );
            ensure!(
                rows.iter().any(|row| row["method"] == "DELETE"
                    && row["url"]
                        .as_str()
                        .is_some_and(|url| url.ends_with("/reviews/987"))
                    && row["status"] == 204),
                "missing cleanup control"
            );
            let calls = fs::read_to_string(&log)?;
            ensure!(
                calls.contains("reviews/987/events"),
                "{id} never reached submit boundary"
            );
            ensure!(
                !calls.contains(PRIVATE_AUDIT_SENTINEL),
                "audit reached outbound payload"
            );
            c.scenarios[id] = json!({
                "status": "observed_current_contract",
                "expected_exit_success": false,
                "post_receipt": receipt,
                "delivery_transaction": transaction,
                "transport_log_sha256": sha(calls.as_bytes()),
                "public_payload_sha256": sha(&fs::read(packet.join("review/github-review.json"))?),
                "deterministic_bytes_unchanged": true
            });
            println!("campaign scenario {id}: {}", c.scenarios[id]);
            c.receipt("running", None)?;
        }

        // Negative identity control (case 7): the CLI still accepts a
        // submitted response carrying a foreign review id; the campaign
        // oracle refuses to count that acceptance as identity-negative
        // evidence unless the specimen is exactly the mismatched submitted
        // state, and the production identity authority remains #1317.
        let mismatch = c.root.join("mismatched-identity");
        fs::create_dir(&mismatch)?;
        let transport = mismatch.join("transport.json");
        let log = mismatch.join("transport.ndjson");
        write(
            &transport,
            serde_json::to_vec(&json!({"head": c.fixture_head, "post": "mismatched"}))?,
        )?;
        let mut args = strings(&["post", "--review-json"]);
        args.push(
            packet
                .join("review/github-review.json")
                .display()
                .to_string(),
        );
        args.extend(strings(&["--out"]));
        args.push(mismatch.display().to_string());
        args.extend(strings(&[
            "--repo",
            "example/repo",
            "--pull-number",
            "123",
            "--github-api-url",
            "https://github.invalid",
            "--github-token",
            "test-token-redacted",
            "--fail-on-post-error",
        ]));
        let env = [
            ("CAMPAIGN_TRANSPORT", transport.display().to_string()),
            ("CAMPAIGN_TRANSPORT_LOG", log.display().to_string()),
        ];
        let (accepted, text) = c.cli(&args, &env)?;
        ensure!(
            accepted,
            "mismatched identity observation changed; refresh the pinned control: {text}"
        );
        let receipt = read(&mismatch.join("post-result.json"))?;
        require_identity_negative_control(&receipt["response"])?;
        unchanged(&before, &fs::read(packet.join("review/gate_outcome.json"))?)?;
        let pending = read(&mismatch.join("pending-review-stdout.json"))?;
        checked_result(&pending, &json!({"id":987}))?;
        ensure!(
            receipt["response"]["id"] != pending["id"],
            "mismatch control did not actually mismatch the pending identity"
        );
        c.scenarios["mismatched-identity"] = json!({
            "status": "observed_current_contract",
            "legacy_cli_accepted": true,
            "campaign_identity_oracle_rejected": true,
            "post_receipt_sha256": sha(&fs::read(mismatch.join("post-result.json"))?),
            "transport_log_sha256": sha(fs::read_to_string(&log)?.as_bytes())
        });
        println!(
            "campaign scenario mismatched-identity: {}",
            c.scenarios["mismatched-identity"]
        );
        c.receipt("running", None)?;
        Ok(())
    }

    fn calls_rows(log: &Path) -> Result<Vec<Value>> {
        fs::read_to_string(log)?
            .lines()
            .map(serde_json::from_str::<Value>)
            .collect::<std::result::Result<Vec<_>, _>>()
            .context("transport log rows")
    }

    fn campaign_run(c: &mut Campaign) -> Result<()> {
        fs::create_dir(c.root.join("bin"))?;
        for tool in ["git", "uname", "sh"] {
            symlink(format!("/usr/bin/{tool}"), c.root.join("bin").join(tool))?;
        }
        script(&c.root.join("bin/curl"), TRANSPORT)?;
        let verifier =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/verify-bun-review-artifacts.py");
        let (ok, version) = c.cli(&strings(&["--version"]), &[])?;
        ensure!(
            ok && version.trim() == format!("ub-review {}", env!("CARGO_PKG_VERSION")),
            "candidate version mismatch: {version}"
        );

        // Fixture repository: one deterministic source change, admitted as a
        // candidate head. Git revision/diff commands must exit successfully
        // before their output becomes input evidence.
        let repo = c.root.join("fixture repo");
        fs::create_dir(&repo)?;
        write(
            &repo.join("src/lib.rs"),
            "pub fn answer() -> usize { 41 }\n",
        )?;
        for args in [
            strings(&["init"]),
            strings(&["config", "user.email", "archive@example.invalid"]),
            strings(&["config", "user.name", "Archive Fixture"]),
            strings(&["add", "."]),
            strings(&["commit", "-m", "base"]),
        ] {
            c.fixture_git(&repo, &args)?;
        }
        write(
            &repo.join("src/lib.rs"),
            "pub fn answer() -> usize { 42 }\n",
        )?;
        for args in [strings(&["add", "."]), strings(&["commit", "-m", "head"])] {
            c.fixture_git(&repo, &args)?;
        }
        let head = c
            .fixture_git(&repo, &strings(&["rev-parse", "HEAD"]))?
            .trim()
            .to_owned();
        c.fixture_head = head;

        // Every scenario runs and records even when an earlier one fails, so
        // one campaign execution observes the whole contract; the test still
        // fails at the end if any observation did not hold.
        let mut failures = Vec::new();
        for spec in scenario_specs() {
            match run_scenario(c, &spec, &repo, &verifier) {
                Ok(observed) => {
                    println!(
                        "campaign scenario {}: {}",
                        spec.id,
                        serde_json::to_string(&observed)?
                    );
                    c.scenarios[spec.id] = observed;
                }
                Err(error) => {
                    println!("campaign scenario {} FAILED: {error:#}", spec.id);
                    failures.push(format!("{}: {error:#}", spec.id));
                    c.scenarios[spec.id] =
                        json!({"status": "failed", "error": format!("{error:#}")});
                }
            }
            c.receipt("running", None)?;
        }
        if let Err(error) = posts(c, &c.root.join("structured-finding/packet")) {
            failures.push(format!("posts: {error:#}"));
        }
        ensure!(
            c.scenarios
                .as_object()
                .context("scenarios")?
                .keys()
                .all(|id| c.scenarios[id]["status"] != "not_run"),
            "scenario omitted"
        );
        ensure!(
            failures.is_empty(),
            "campaign observations failed:\n{}",
            failures.join("\n")
        );
        Ok(())
    }

    #[test]
    fn source_candidate_archive_advisory_campaign() -> Result<()> {
        let _cli_subprocess_guard = cli_subprocess_test_lock()?;
        let root = tempfile::tempdir()?;
        let bin = PathBuf::from(env!("CARGO_BIN_EXE_ub-review"));
        let binary_sha256 = sha(&fs::read(&bin)?);
        let scenarios = RUN_SCENARIOS
            .iter()
            .chain(["post-error", "mismatched-identity"].iter())
            .map(|id| ((*id).to_owned(), json!({"status": "not_run"})))
            .collect::<serde_json::Map<String, Value>>();
        let mut campaign = Campaign {
            root: root.path().to_owned(),
            bin,
            binary_sha256,
            fixture_head: String::new(),
            commands: vec![],
            scenarios: Value::Object(scenarios),
        };
        let result = campaign_run(&mut campaign);
        campaign.receipt(
            if result.is_ok() {
                "bounded_observations_pass"
            } else {
                "failed"
            },
            result.as_ref().err().map(|error| format!("{error:#}")),
        )?;
        result
    }

    #[test]
    fn fixture_identity_and_diff_fail_closed_on_real_git_errors() -> Result<()> {
        let _cli_subprocess_guard = cli_subprocess_test_lock()?;
        let root = tempfile::tempdir()?;
        let cwd = root.path().join("not-a-repository");
        fs::create_dir(&cwd)?;
        let mut campaign = Campaign {
            root: root.path().to_owned(),
            bin: root.path().join("unused-binary"),
            binary_sha256: String::new(),
            fixture_head: String::new(),
            commands: vec![],
            scenarios: json!({}),
        };
        for args in [
            strings(&["rev-parse", "HEAD"]),
            strings(&["rev-parse", "HEAD~1"]),
            strings(&["diff", "--binary", "HEAD~1", "HEAD"]),
        ] {
            let result = campaign.fixture_git(&cwd, &args);
            let error = match result {
                Ok(text) => anyhow::bail!("failed Git output accepted as identity/patch: {text}"),
                Err(error) => error,
            };
            ensure!(
                error.to_string().contains("fixture git") && error.to_string().contains("failed:"),
                "wrong failure route: {error}"
            );
            ensure!(
                campaign.fixture_head.is_empty(),
                "failed git accepted input evidence"
            );
        }
        ensure!(
            campaign.commands.len() == 3
                && campaign
                    .commands
                    .iter()
                    .all(|row| row["state"] == "exited" && row["exit"] != 0),
            "failure command receipts omitted"
        );
        Ok(())
    }
}
