//! Provider completion metadata must survive the public intelligent-CI verdict path.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Value, json};
use wait_timeout::ChildExt;

#[path = "common/cli_lock.rs"]
mod cli_lock;

#[derive(Clone, Copy)]
enum ProviderShape {
    OpenAi,
    Anthropic,
}

impl ProviderShape {
    fn cli_kind(self) -> &'static str {
        match self {
            Self::OpenAi => "openai",
            Self::Anthropic => "anthropic",
        }
    }

    fn complete_reason(self) -> &'static str {
        match self {
            Self::OpenAi => "stop",
            Self::Anthropic => "end_turn",
        }
    }

    fn envelope(self, reason: Option<Value>, content: &str) -> Value {
        let mut response = match self {
            Self::OpenAi => json!({"choices": [{"message": {"content": content}}]}),
            Self::Anthropic => json!({"content": [{"type": "text", "text": content}]}),
        };
        if let Some(reason) = reason {
            match self {
                Self::OpenAi => response["choices"][0]["finish_reason"] = reason,
                Self::Anthropic => response["stop_reason"] = reason,
            }
        }
        response
    }
}

struct Fixture {
    path: PathBuf,
    _temporary: Option<tempfile::TempDir>,
    lane_response_delay: Duration,
    model_timeout_sec: u64,
}

impl Fixture {
    fn new(name: &str) -> Result<Self> {
        // A caller may retain the original RED/GREEN packets outside the checkout.
        let retained = std::env::var_os("UB_REVIEW_COMPLETION_RECEIPTS");
        let temp = if let Some(root) = &retained {
            fs::create_dir_all(root)?;
            tempfile::Builder::new().prefix(name).tempdir_in(root)?
        } else {
            tempfile::Builder::new().prefix(name).tempdir()?
        };
        if retained.is_some() {
            Ok(Self {
                path: temp.keep(),
                _temporary: None,
                lane_response_delay: Duration::ZERO,
                model_timeout_sec: 10,
            })
        } else {
            Ok(Self {
                path: temp.path().to_path_buf(),
                _temporary: Some(temp),
                lane_response_delay: Duration::ZERO,
                model_timeout_sec: 10,
            })
        }
    }

    fn repo(&self) -> PathBuf {
        self.path.join("repo")
    }
}

struct RunWitness {
    out: PathBuf,
    cli_success: bool,
    gate: Value,
    terminal: Value,
    lane: Value,
}

fn isolated_command(program: &std::ffi::OsStr, cwd: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(cwd);
    for (name, _) in std::env::vars_os() {
        let text = name.to_string_lossy();
        if text.starts_with("UB_REVIEW_")
            || matches!(
                text.as_ref(),
                "GITHUB_ACTIONS" | "RUNNER_ENVIRONMENT" | "RUNNER_NAME"
            )
        {
            command.env_remove(name);
        }
    }
    command
}

fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let output = isolated_command(std::ffi::OsStr::new("git"), repo)
        .args(args)
        .output()?;
    ensure!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn init_repo(fixture: &Fixture) -> Result<String> {
    let repo = fixture.repo();
    fs::create_dir_all(repo.join("src"))?;
    fs::write(repo.join("src/lib.rs"), "pub fn answer() -> usize { 41 }\n")?;
    git(&repo, &["init"])?;
    git(
        &repo,
        &["config", "user.email", "provider-fixture@example.invalid"],
    )?;
    git(&repo, &["config", "user.name", "Provider Fixture"])?;
    git(&repo, &["add", "."])?;
    git(&repo, &["commit", "-m", "baseline"])?;
    commit_answer(&repo, 42)
}

fn commit_answer(repo: &Path, value: usize) -> Result<String> {
    fs::write(
        repo.join("src/lib.rs"),
        format!("pub fn answer() -> usize {{ {value} }}\n"),
    )?;
    git(repo, &["add", "src/lib.rs"])?;
    git(repo, &["commit", "-m", "change answer"])?;
    git(repo, &["rev-parse", "HEAD"])
}

fn audit_content() -> String {
    json!({
        "findings": [],
        "observations": [],
        "proof_intents": [],
        "internal_audit": {
            "surfaces_checked": ["src/lib.rs:answer"],
            "strongest_rejected_hypothesis": "the changed return value bypasses validation",
            "remaining_local_uncertainty": null
        }
    })
    .to_string()
}

fn serve_request(mut stream: TcpStream, response: &Value, delay: Duration) -> Result<Value> {
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.set_write_timeout(Some(Duration::from_secs(10)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut content_length = None;
    loop {
        let mut line = String::new();
        ensure!(
            reader.read_line(&mut line)? > 0,
            "request ended before headers"
        );
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((key, value)) = line.split_once(':')
            && key.eq_ignore_ascii_case("content-length")
        {
            content_length = Some(value.trim().parse::<usize>()?);
        }
    }
    let mut body = vec![0; content_length.context("request content-length absent")?];
    reader.read_exact(&mut body)?;
    let request = serde_json::from_slice(&body)?;
    let body = serde_json::to_vec(response)?;
    if !delay.is_zero() {
        thread::sleep(delay);
    }
    let sent = write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    ).and_then(|()| stream.write_all(&body));
    if let Err(error) = sent {
        ensure!(
            !delay.is_zero()
                && matches!(
                    error.kind(),
                    std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                ),
            "send provider response: {error}"
        );
    }
    Ok(request)
}

fn provider(
    responses: Vec<Value>,
    lane_delay: Duration,
) -> Result<(String, thread::JoinHandle<Result<Vec<Value>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/v1/messages", listener.local_addr()?);
    let worker = thread::spawn(move || {
        let mut requests = Vec::new();
        let mut deadline = Instant::now() + Duration::from_secs(30);
        while requests.len() < responses.len() {
            match listener.accept() {
                Ok((stream, _)) => {
                    let delay = if requests.is_empty() {
                        Duration::ZERO
                    } else {
                        lane_delay
                    };
                    requests.push(serve_request(stream, &responses[requests.len()], delay)?);
                    deadline = Instant::now() + Duration::from_secs(30);
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!(
                            "provider received {} of {} requests",
                            requests.len(),
                            responses.len()
                        );
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(requests)
    });
    Ok((url, worker))
}

fn read_json(path: &Path) -> Result<Value> {
    serde_json::from_slice(&fs::read(path)?).with_context(|| path.display().to_string())
}

fn record_process(out: &Path, output: &Output) -> Result<()> {
    fs::write(out.join("cli.stdout.txt"), &output.stdout)?;
    fs::write(out.join("cli.stderr.txt"), &output.stderr)?;
    fs::write(
        out.join("cli.status.json"),
        serde_json::to_vec_pretty(
            &json!({"success": output.status.success(), "code": output.status.code()}),
        )?,
    )?;
    Ok(())
}

fn run_case(
    fixture: &Fixture,
    name: &str,
    shape: ProviderShape,
    reason: Option<Value>,
    content: &str,
) -> Result<RunWitness> {
    let repo = fixture.repo();
    let out = fixture.path.join(name);
    let fixture_config = fixture.path.join("config.toml");
    let config = if fixture_config.exists() {
        fixture_config
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("profiles/bun-ub-v0.toml")
    };
    let lane_response = shape.envelope(reason, content);
    let responses = vec![
        shape.envelope(Some(json!(shape.complete_reason())), &audit_content()),
        lane_response.clone(),
    ];
    let (url, worker) = provider(responses.clone(), fixture.lane_response_delay)?;
    let binary = std::env::var_os("UB_REVIEW_COMPLETION_CLI_BIN")
        .unwrap_or_else(|| env!("CARGO_BIN_EXE_ub-review").into());
    let model_timeout = fixture.model_timeout_sec.to_string();
    let mut child = isolated_command(&binary, &fixture.path)
        .args(["run", "--config"])
        .arg(config)
        .arg("--root")
        .arg(&repo)
        .args(["--base", "HEAD~1", "--head", "HEAD", "--out"])
        .arg(&out)
        .args([
            "--mode",
            "intelligent-ci",
            "--no-github-summary",
            "--model-mode",
            "auto",
            "--provider-policy",
            "minimax-only",
            "--minimax-provider-kind",
            shape.cli_kind(),
            "--lanes",
            "correctness",
            "--tools",
            "cargo-allow",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "1",
            "--model-timeout-sec",
            &model_timeout,
        ])
        .env(
            "UB_REVIEW_MINIMAX_API_KEY",
            "dummy-local-completion-fixture-key",
        )
        .env("UB_REVIEW_MINIMAX_API_URL", url)
        .env("CARGO_TARGET_DIR", fixture.path.join("cargo-target"))
        .env("CARGO_NET_OFFLINE", "true")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    match child.wait_timeout(Duration::from_secs(60)) {
        Ok(Some(_)) => {}
        result => {
            let _ = child.kill();
            let _ = child.wait();
            let _ = worker.join();
            bail!("CLI completion unconfirmed within fixture budget: {result:?}");
        }
    }
    let output = child.wait_with_output()?;
    let requests = worker
        .join()
        .map_err(|_| anyhow::anyhow!("provider thread panicked"))??;
    fs::create_dir_all(&out)?;
    record_process(&out, &output)?;
    fs::write(
        out.join("provider.requests.json"),
        serde_json::to_vec_pretty(&requests)?,
    )?;
    fs::write(
        out.join("provider.response-plan.json"),
        serde_json::to_vec_pretty(&responses)?,
    )?;
    ensure!(
        out.join("review/review.json").exists(),
        "CLI failed before producing review: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        requests.len(),
        2,
        "exactly one preflight and one model attempt"
    );
    let preflights = read_json(&out.join("review/provider-preflight-status.json"))?;
    assert_eq!(preflights[0]["status"], "ok", "{preflights}");
    let review = read_json(&out.join("review/review.json"))?;
    let lane = review["model_lanes"]
        .as_array()
        .context("model lanes missing")?
        .iter()
        .find(|lane| lane["lane"] == "correctness")
        .context("correctness model receipt missing")?
        .clone();
    let gate = read_json(&out.join("review/gate_outcome.json"))?;
    let terminal = read_json(&out.join("review/terminal_state.json"))?;
    let head = git(&repo, &["rev-parse", "HEAD"])?;
    let admission = read_json(&out.join("input/revision-admission.json"))?;
    assert_eq!(admission["reviewed_commit_oid"], head);
    assert_eq!(gate["revision"]["reviewed_commit"], head);
    assert_eq!(gate["revision"]["semantics"], "candidate_head");
    assert_eq!(gate["revision"]["digest"], admission["identity_digest"]);
    let response_path = out.join("review/model/correctness/response.json");
    if fixture.lane_response_delay.is_zero() {
        assert_eq!(read_json(&response_path)?, lane_response);
    } else if response_path.exists() {
        assert!(
            fs::read(response_path)?.is_empty(),
            "timed-out transfer cannot leave complete assistant content"
        );
    }
    Ok(RunWitness {
        out,
        cli_success: output.status.success(),
        gate,
        terminal,
        lane,
    })
}

fn assert_incomplete(witness: &RunWitness, reason: &str) -> Result<()> {
    assert!(witness.cli_success, "provider failure remains advisory");
    assert!(
        !matches!(witness.lane["status"].as_str(), Some("ok" | "degraded")),
        "{reason}: {}",
        witness.lane
    );
    assert_eq!(
        witness.terminal["usable_model_lanes"], 0,
        "{reason}: {}",
        witness.terminal
    );
    assert_eq!(
        witness.gate["model_coverage"]["lanes_usable"], 0,
        "{reason}: {}",
        witness.gate
    );
    assert_eq!(
        witness.gate["analysis_result"], "not_proven",
        "{reason}: {}",
        witness.gate
    );
    assert_eq!(
        witness.gate["gate_result"], "not_proven",
        "{reason}: {}",
        witness.gate
    );
    assert!(
        witness.gate["not_proven_reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.iter().any(|reason| reason
                .as_str()
                .is_some_and(|reason| reason.starts_with("model-coverage:"))))
    );
    assert!(
        !witness
            .out
            .join("review/model/correctness/content.json")
            .exists(),
        "incomplete content must not enter normalization"
    );
    let diagnostic = if reason == "provider_new_reason" {
        "unsupported"
    } else {
        reason
    };
    assert!(
        witness.lane["reason"]
            .as_str()
            .is_some_and(|message| message.contains(diagnostic)),
        "completion diagnosis missing: {}",
        witness.lane
    );
    Ok(())
}

fn assert_completed_lane(witness: &RunWitness) {
    assert!(witness.cli_success);
    assert_eq!(witness.lane["status"], "ok", "{}", witness.lane);
    assert_eq!(
        witness.terminal["usable_model_lanes"], 1,
        "{}",
        witness.terminal
    );
    assert_eq!(witness.gate["model_coverage"]["lanes_usable"], 1);
    assert!(
        witness
            .out
            .join("review/model/correctness/content.json")
            .exists()
    );
}

fn assert_completed(witness: &RunWitness) -> Result<()> {
    assert_completed_lane(witness);
    assert_eq!(witness.gate["gate_result"], "pass", "{}", witness.gate);
    assert!(
        !witness.gate["not_proven_reasons"]
            .as_array()
            .context("reasons absent")?
            .iter()
            .any(|reason| reason
                .as_str()
                .is_some_and(|reason| reason.starts_with("model-coverage:")))
    );
    Ok(())
}

fn incomplete_then_recover(shape: ProviderShape, incomplete_reason: &str) -> Result<()> {
    let fixture = Fixture::new("completion-recovery-")?;
    let head = init_repo(&fixture)?;
    let incomplete = run_case(
        &fixture,
        "incomplete",
        shape,
        Some(json!(incomplete_reason)),
        &audit_content(),
    )?;
    let original_path = incomplete
        .out
        .join("review/model/correctness/response.json");
    let original = fs::read(&original_path)?;
    assert_incomplete(&incomplete, incomplete_reason)?;
    assert_eq!(incomplete.gate["revision"]["reviewed_commit"], head);
    let recovered = run_case(
        &fixture,
        "recovered-same-sha",
        shape,
        Some(json!(shape.complete_reason())),
        &audit_content(),
    )?;
    assert_completed(&recovered)?;
    assert_eq!(recovered.gate["revision"], incomplete.gate["revision"]);
    let next_head = commit_answer(&fixture.repo(), 43)?;
    assert_ne!(next_head, head);
    let next = run_case(
        &fixture,
        "completed-next-sha",
        shape,
        Some(json!(shape.complete_reason())),
        &audit_content(),
    )?;
    assert_completed(&next)?;
    assert_eq!(next.gate["revision"]["reviewed_commit"], next_head);
    assert_ne!(
        next.gate["revision"]["digest"],
        incomplete.gate["revision"]["digest"]
    );
    assert_eq!(
        fs::read(original_path)?,
        original,
        "original incomplete witness was modified"
    );
    Ok(())
}

#[test]
fn openai_length_cannot_pass_and_completed_retry_recovers_exact_subject() -> Result<()> {
    let _guard = cli_lock::cli_subprocess_test_lock()?;
    incomplete_then_recover(ProviderShape::OpenAi, "length")
}

#[test]
fn anthropic_max_tokens_cannot_pass_and_completed_retry_recovers_exact_subject() -> Result<()> {
    let _guard = cli_lock::cli_subprocess_test_lock()?;
    incomplete_then_recover(ProviderShape::Anthropic, "max_tokens")
}

#[test]
fn explicit_nonterminal_unknown_and_null_reasons_never_credit_a_lane() -> Result<()> {
    let _guard = cli_lock::cli_subprocess_test_lock()?;
    let fixture = Fixture::new("completion-matrix-")?;
    init_repo(&fixture)?;
    for (shape, reasons) in [
        (
            ProviderShape::OpenAi,
            vec![
                "content_filter",
                "tool_calls",
                "function_call",
                "provider_new_reason",
            ],
        ),
        (
            ProviderShape::Anthropic,
            vec![
                "pause_turn",
                "tool_use",
                "refusal",
                "model_context_window_exceeded",
                "provider_new_reason",
            ],
        ),
    ] {
        for reason in reasons {
            let name = format!("{}-{reason}", shape.cli_kind());
            let witness = run_case(
                &fixture,
                &name,
                shape,
                Some(json!(reason)),
                &audit_content(),
            )?;
            assert_incomplete(&witness, reason)?;
        }
        for (name, reason, diagnostic) in [
            ("null", Value::Null, "null"),
            ("number", json!(7), "non-string"),
            ("boolean", json!(true), "non-string"),
            ("object", json!({"reason": "stop"}), "non-string"),
        ] {
            let name = format!("{}-{name}", shape.cli_kind());
            let witness = run_case(&fixture, &name, shape, Some(reason), &audit_content())?;
            assert_incomplete(&witness, diagnostic)?;
        }
    }
    Ok(())
}

#[test]
fn completed_and_legacy_metadata_controls_stay_usable() -> Result<()> {
    let _guard = cli_lock::cli_subprocess_test_lock()?;
    let fixture = Fixture::new("completion-controls-")?;
    init_repo(&fixture)?;
    for (shape, reason) in [
        (ProviderShape::OpenAi, Some(json!("stop"))),
        (ProviderShape::Anthropic, Some(json!("end_turn"))),
        (ProviderShape::Anthropic, Some(json!("stop_sequence"))),
        (ProviderShape::OpenAi, None),
        (ProviderShape::Anthropic, None),
    ] {
        let name = format!(
            "{}-{}",
            shape.cli_kind(),
            reason
                .as_ref()
                .and_then(Value::as_str)
                .unwrap_or("legacy-absent")
        );
        let witness = run_case(&fixture, &name, shape, reason, &audit_content())?;
        assert_completed(&witness)?;
    }
    Ok(())
}

#[test]
fn completed_envelope_does_not_credit_empty_unusable_content() -> Result<()> {
    let _guard = cli_lock::cli_subprocess_test_lock()?;
    let fixture = Fixture::new("completion-empty-content-")?;
    init_repo(&fixture)?;
    for shape in [ProviderShape::OpenAi, ProviderShape::Anthropic] {
        let witness = run_case(
            &fixture,
            shape.cli_kind(),
            shape,
            Some(json!(shape.complete_reason())),
            "{}",
        )?;
        assert!(
            witness.cli_success,
            "unusable model evidence remains advisory"
        );
        assert_eq!(witness.lane["status"], "invalid_json", "{}", witness.lane);
        assert_eq!(witness.terminal["usable_model_lanes"], 0);
        assert_eq!(witness.gate["model_coverage"]["lanes_usable"], 0);
        assert_eq!(witness.gate["analysis_result"], "not_proven");
        assert_eq!(witness.gate["gate_result"], "not_proven");
        assert_eq!(
            fs::read_to_string(witness.out.join("review/model/correctness/content.json"))?,
            "{}"
        );
    }
    Ok(())
}

#[test]
fn provider_timeout_stays_unknown_and_completed_retry_recovers() -> Result<()> {
    let _guard = cli_lock::cli_subprocess_test_lock()?;
    let mut fixture = Fixture::new("completion-timeout-")?;
    init_repo(&fixture)?;
    fixture.model_timeout_sec = 1;
    fixture.lane_response_delay = Duration::from_secs(2);
    let timed_out = run_case(
        &fixture,
        "timed-out",
        ProviderShape::OpenAi,
        Some(json!("stop")),
        &audit_content(),
    )?;
    assert!(timed_out.cli_success, "provider timeout remains advisory");
    assert_eq!(timed_out.lane["status"], "timed_out", "{}", timed_out.lane);
    assert_eq!(timed_out.terminal["usable_model_lanes"], 0);
    assert_eq!(timed_out.gate["model_coverage"]["lanes_usable"], 0);
    assert_eq!(timed_out.gate["analysis_result"], "not_proven");
    assert_eq!(timed_out.gate["gate_result"], "not_proven");
    assert!(
        !timed_out
            .out
            .join("review/model/correctness/content.json")
            .exists()
    );
    let original_path = timed_out.out.join("review/model/correctness/response.json");
    let original = if original_path.exists() {
        Some(fs::read(&original_path)?)
    } else {
        None
    };
    // run_case joins the delayed server before returning; no provider worker survives recovery.
    fixture.lane_response_delay = Duration::ZERO;
    fixture.model_timeout_sec = 10;
    let recovered = run_case(
        &fixture,
        "timeout-recovered",
        ProviderShape::OpenAi,
        Some(json!("stop")),
        &audit_content(),
    )?;
    assert_completed(&recovered)?;
    assert_eq!(recovered.gate["revision"], timed_out.gate["revision"]);
    match original {
        Some(original) => assert_eq!(fs::read(original_path)?, original),
        None => assert!(
            !original_path.exists(),
            "recovery must preserve original absent transfer witness"
        ),
    }
    Ok(())
}

fn init_required_build_fixture(fixture: &Fixture) -> Result<()> {
    let repo = fixture.repo();
    fs::create_dir_all(&repo)?;
    fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname = \"completion-compile-control\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    fs::write(
        repo.join("Cargo.lock"),
        "version = 4\n\n[[package]]\nname = \"completion-compile-control\"\nversion = \"0.1.0\"\n",
    )?;
    init_repo(fixture)?;
    fs::write(
        fixture.path.join("config.toml"),
        r#"review_profile = "bun-ub-v0"
profile = "gh-runner-full"

[repo]
kind = "rust"
ledger = ""
base = "HEAD~1"
head = "HEAD"

[[proof.required]]
id = "completion-compile-control"
languages = ["rust"]
diff_classes = ["source-general"]
command = "cargo check --workspace --locked"
reason = "Independent deterministic build control for provider completion integrity."
cost = "focused-build"
timeout_sec = 30
required = true
"#,
    )?;
    Ok(())
}

#[test]
fn completed_provider_cannot_mask_real_compile_failure_and_compile_recovery() -> Result<()> {
    let _guard = cli_lock::cli_subprocess_test_lock()?;
    let fixture = Fixture::new("completion-build-control-")?;
    init_required_build_fixture(&fixture)?;
    let repo = fixture.repo();
    fs::write(
        repo.join("src/lib.rs"),
        "pub fn answer() -> usize { \"wrong type\" }\n",
    )?;
    git(&repo, &["add", "src/lib.rs"])?;
    git(&repo, &["commit", "-m", "deliberate compile failure"])?;
    let failed = run_case(
        &fixture,
        "compile-failed",
        ProviderShape::OpenAi,
        Some(json!("stop")),
        &audit_content(),
    )?;
    assert!(
        !failed.cli_success,
        "required deterministic build failure must fail intelligent CI"
    );
    assert_eq!(failed.lane["status"], "ok");
    assert_eq!(failed.gate["model_coverage"]["lanes_usable"], 1);
    assert_eq!(failed.gate["conclusion"], "fail");
    assert_eq!(failed.gate["analysis_result"], "findings");
    assert_eq!(failed.gate["gate_result"], "finding");
    assert_eq!(failed.gate["required_proof"]["failed"], 1);
    let receipts = read_json(&failed.out.join("review/proof_receipts.json"))?;
    let build = receipts
        .as_array()
        .context("proof receipts missing")?
        .iter()
        .find(|receipt| receipt["kind"] == "focused-build")
        .context("build receipt missing")?;
    assert_eq!(build["revision"], failed.gate["revision"]);
    assert_eq!(build["result"], "head_failed");
    assert_eq!(build["commands"][0]["status"], "failed");
    assert_eq!(build["commands"][0]["exit_code"], 101);
    let stderr = build["commands"][0]["stderr"]
        .as_str()
        .context("build stderr missing")?;
    let compiler_witness = fs::read(failed.out.join(stderr))?;
    assert!(String::from_utf8_lossy(&compiler_witness).contains("mismatched types"));
    let recovered_sha = commit_answer(&repo, 43)?;
    let recovered = run_case(
        &fixture,
        "compile-recovered",
        ProviderShape::OpenAi,
        Some(json!("stop")),
        &audit_content(),
    )?;
    assert_completed_lane(&recovered);
    assert_eq!(recovered.gate["conclusion"], "pass");
    let planner_unproven = recovered.gate["planner_required_proofs"]["unproven"]
        .as_u64()
        .context("planner coverage missing")?;
    if planner_unproven > 0 {
        // A passing compile must not erase an independently unproven planner obligation.
        assert_eq!(recovered.gate["analysis_result"], "not_proven");
        assert_eq!(recovered.gate["gate_result"], "not_proven");
        assert!(
            recovered.gate["not_proven_reasons"]
                .as_array()
                .context("reasons missing")?
                .iter()
                .any(|reason| reason
                    .as_str()
                    .is_some_and(|reason| reason.starts_with("required-planner-proof:")))
        );
    } else {
        assert_eq!(recovered.gate["gate_result"], "pass");
    }
    assert_eq!(recovered.gate["revision"]["reviewed_commit"], recovered_sha);
    assert_ne!(recovered.gate["revision"], failed.gate["revision"]);
    assert_eq!(recovered.gate["required_proof"]["passed"], 1);
    assert_eq!(recovered.gate["required_proof"]["failed"], 0);
    assert_eq!(fs::read(failed.out.join(stderr))?, compiler_witness);
    Ok(())
}
