//! Deterministic orchestration proof, not a live-model review-quality evaluation.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

#[path = "common/cli_lock.rs"]
mod cli_lock;

// Keep this integration harness independent from unrelated GitHub/tool fixtures.
mod common {
    use super::*;
    pub use crate::cli_lock::cli_subprocess_test_lock;
    use std::process::{Command, Output, Stdio};
    use std::time::Instant;
    use wait_timeout::ChildExt;
    pub const FAKE_HTTP_STREAM_TIMEOUT: Duration = Duration::from_secs(10);

    pub fn path_str(path: &Path) -> Result<&str> {
        path.to_str().context("fixture path must be UTF-8")
    }

    pub fn read_json(path: &Path) -> Result<Value> {
        Ok(serde_json::from_slice(&fs::read(path)?)?)
    }

    fn bounded_output(command: &mut Command) -> Result<Output> {
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        if child.wait_timeout(Duration::from_secs(45))?.is_none() {
            child.kill()?;
            let output = child.wait_with_output()?;
            anyhow::bail!(
                "fixture subprocess exceeded 45 seconds: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(child.wait_with_output()?)
    }

    pub fn run_capture_with_env(
        cwd: &Path,
        program: &str,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Result<String> {
        let mut command = Command::new(program);
        command.current_dir(cwd).args(args);
        for (name, _) in std::env::vars_os() {
            let text = name.to_string_lossy();
            if text.starts_with("UB_REVIEW_")
                || matches!(
                    text.as_ref(),
                    "GITHUB_ACTIONS" | "GITHUB_EVENT_PATH" | "RUNNER_ENVIRONMENT" | "RUNNER_NAME"
                )
            {
                command.env_remove(name);
            }
        }
        command.envs(envs.iter().copied());
        let output = bounded_output(&mut command)?;
        let text = format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        ensure!(output.status.success(), "{program} {args:?} failed: {text}");
        Ok(text)
    }

    pub fn run(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
        run_capture_with_env(cwd, program, args, &[]).map(|_| ())
    }

    pub fn run_with_env(
        cwd: &Path,
        program: &str,
        args: &[&str],
        envs: &[(&str, &str)],
    ) -> Result<()> {
        run_capture_with_env(cwd, program, args, envs).map(|_| ())
    }

    pub fn run_expect_failure(cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
        let output = bounded_output(Command::new(program).current_dir(cwd).args(args))?;
        ensure!(!output.status.success(), "{program} unexpectedly passed");
        Ok(format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
    }

    pub fn serve_fake_http(
        listener: TcpListener,
        expected: usize,
        label: &str,
        timeout: Duration,
        handler: impl Fn(usize, TcpStream) -> Result<String>,
    ) -> Result<Vec<String>> {
        let mut requests = Vec::new();
        let mut deadline = Instant::now() + timeout;
        while requests.len() < expected {
            match listener.accept() {
                Ok((stream, _)) => {
                    requests.push(handler(requests.len(), stream)?);
                    deadline = Instant::now() + timeout;
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    ensure!(
                        Instant::now() < deadline,
                        "{label}: received {} of {expected} requests",
                        requests.len()
                    );
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(requests)
    }

    pub fn join_fake_provider(
        handle: thread::JoinHandle<Result<Vec<String>>>,
    ) -> Result<Vec<String>> {
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("fixture provider thread panicked"))?
    }
}

const STRICT: &str = "pub fn byte_at(bytes: &[u8], index: usize) -> Option<u8> {\n    if index < bytes.len() { Some(bytes[index]) } else { None }\n}\n";
const DEFECT: &str = "pub fn byte_at(bytes: &[u8], index: usize) -> Option<u8> {\n    if index <= bytes.len() { Some(bytes[index]) } else { None }\n}\n";
const REPAIRED: &str = "pub fn byte_at(bytes: &[u8], index: usize) -> Option<u8> {\n    if index < bytes.len() { Some(bytes[index]) } else { None }\n}\n\n#[test]\nfn rejects_equal_length() {\n    assert_eq!(byte_at(&[7], 1), None);\n    assert_eq!(byte_at(&[], 0), None);\n}\n";
const CONTROL: &str = "pub fn byte_at(bytes: &[u8], index: usize) -> Option<u8> {\n    bytes.get(index).copied()\n}\n";
const LIMIT: &str = "LIMIT: source and boundary witness only; no concurrency assessment";
const DEFECT_FINDING: &str = "index equal to bytes.len() enters indexing and panics";
const PRIVATE: &str = "PRIVATE_AUDIT_MARKER_NEVER_ROUTE";

#[derive(Clone, Copy)]
enum Revision {
    Defect,
    Repair,
    Control,
}

impl Revision {
    fn summary(self) -> &'static str {
        match self {
            Self::Defect => "DEFECT: equality is accepted before indexing",
            Self::Repair => "REPAIRED: strict guard rejects equality with boundary tests",
            Self::Control => "CONTROL: checked bytes.get independently rejects equality",
        }
    }

    fn verdict(self) -> &'static str {
        match self {
            Self::Defect => "changes_requested",
            Self::Repair | Self::Control => "clear",
        }
    }

    fn primary(self) -> Value {
        let findings = match self {
            Self::Defect => {
                json!([{"severity":"high", "confidence":"high", "path":"src/lib.rs","line":2,"body":DEFECT_FINDING, "evidence":"src/lib.rs:2; byte_at(&[7], 1) panics"}])
            }
            Self::Repair | Self::Control => json!([]),
        };
        json!({
            "summary":self.summary(), "inline_comments":[],
            "summary_only_findings":[], "candidate_findings":findings,
            "observations":[{"claim":LIMIT,"kind":"coverage_gap","status":"unresolved","evidence":["src/lib.rs"]}],
            "internal_audit":{"surfaces_checked":["src/lib.rs:byte_at"],"strongest_rejected_hypothesis":PRIVATE,"remaining_local_uncertainty":null}
        })
    }
}

fn serve_request(stream: TcpStream, revision: Revision) -> Result<String> {
    serve_request_mode(stream, revision, false)
}

fn serve_request_mode(
    mut stream: TcpStream,
    revision: Revision,
    malformed: bool,
) -> Result<String> {
    stream.set_read_timeout(Some(common::FAKE_HTTP_STREAM_TIMEOUT))?;
    stream.set_write_timeout(Some(common::FAKE_HTTP_STREAM_TIMEOUT))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut length = 0;
    loop {
        let mut line = String::new();
        ensure!(
            reader.read_line(&mut line)? != 0,
            "provider request ended in headers"
        );
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            length = value.trim().parse::<usize>()?;
        }
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    let request: Value = serde_json::from_slice(&bytes)?;
    let prompt = request["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["content"].as_str())
        .context("provider user prompt")?;
    let prompt = prompt
        .rsplit("\n\nLane task:\n\n")
        .next()
        .context("role prompt")?;
    // Choose by the actual role prompt, never by call ordinal or fixture source.
    let content = if prompt.contains("Return strict JSON only:") && prompt.contains("preflight ok")
    {
        json!({"summary":"preflight ok","inline_comments":[],"summary_only_findings":[]})
    } else if prompt.starts_with("# Reporter Coordination Task") {
        let redistilling = prompt.contains("CONTINUED:");
        json!({"distillation":format!("Reporter assessed {}",revision.summary()),"verdict":revision.verdict(),"proposed_follow_ups":if redistilling { vec![] } else { vec!["bounds: confirm equality and scope limits"] }})
    } else if prompt.starts_with("You are the final refuter") {
        json!({"decisions":[{"path":"src/lib.rs","line":2,"disposition":"inline","confidence":"high","reason":"The equality witness confirms the changed guard admits an out-of-range index"}]})
    } else if prompt.starts_with("# Lane continuation:") {
        json!({"conclusion":format!("CONTINUED: {}; {LIMIT}",revision.summary())})
    } else if prompt.starts_with("Lane: bounds\n") {
        if malformed {
            json!({})
        } else {
            revision.primary()
        }
    } else {
        anyhow::bail!(
            "unexpected model role prompt: {}",
            prompt.chars().take(100).collect::<String>()
        );
    };
    let response =
        serde_json::to_vec(&json!({"choices":[{"message":{"content":content.to_string()}}]}))?;
    write!(
        stream,
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.len()
    )?;
    stream.write_all(&response)?;
    Ok(String::from_utf8(bytes)?)
}

fn commit(repo: &Path, content: &str, message: &str) -> Result<String> {
    fs::write(repo.join("src/lib.rs"), content)?;
    common::run(repo, "git", &["add", "src/lib.rs"])?;
    common::run(repo, "git", &["commit", "-m", message])?;
    Ok(
        common::run_capture_with_env(repo, "git", &["rev-parse", "HEAD"], &[])?
            .trim()
            .to_owned(),
    )
}

struct ReviewRun {
    revision: Revision,
    out: PathBuf,
    requests: Vec<Value>,
}

fn review(
    repo: &Path,
    config: &Path,
    base: &str,
    head: &str,
    out: PathBuf,
    revision: Revision,
    prior_context: Option<&Path>,
) -> Result<ReviewRun> {
    // Explicit developer experiment against an original executable; no production override.
    let baseline_binary = std::env::var_os("UB_REVIEW_SEMANTIC_BASELINE_BIN").map(PathBuf::from);
    let expected = if matches!(revision, Revision::Defect) {
        6
    } else {
        5
    };
    let mut args = vec![
        "run",
        "--dry-run",
        "--config",
        common::path_str(config)?,
        "--root",
        common::path_str(repo)?,
        "--base",
        base,
        "--head",
        head,
        "--out",
        common::path_str(&out)?,
        "--no-github-summary",
        "--model-mode",
        "auto",
        "--provider-policy",
        "minimax-only",
        "--minimax-provider-kind",
        "openai",
        "--lanes",
        "bounds",
        "--tools",
        "cargo-allow",
        "--model-concurrency",
        "1",
        "--max-model-calls",
        if matches!(revision, Revision::Defect) {
            "5"
        } else {
            "4"
        },
        "--model-timeout-sec",
        "5",
    ];
    if let Some(path) = prior_context {
        args.extend(["--pr-thread-context", common::path_str(path)?]);
    }
    let execute = |binary: &str| -> Result<Vec<Value>> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let provider = thread::spawn(move || {
            common::serve_fake_http(
                listener,
                expected,
                "semantic preservation",
                Duration::from_secs(15),
                move |_, stream| serve_request(stream, revision),
            )
        });
        let result = common::run_with_env(
            repo,
            binary,
            &args,
            &[
                ("UB_REVIEW_MINIMAX_API_KEY", "semantic-test-only"),
                ("UB_REVIEW_MINIMAX_API_URL", &url),
            ],
        );
        let captured = common::join_fake_provider(provider)?;
        result?;
        let requests = captured
            .iter()
            .map(|s| serde_json::from_str(s))
            .collect::<std::result::Result<Vec<Value>, _>>()?;
        Ok(requests)
    };
    let baseline = if let Some(binary) = baseline_binary.as_deref() {
        let requests = execute(common::path_str(binary)?)?;
        let public = common::read_json(&out.join("review/github-review.json"))?;
        let gate = common::read_json(&out.join("review/gate_outcome.json"))?;
        let terminal = common::read_json(&out.join("review/terminal_state.json"))?;
        fs::remove_dir_all(&out)?;
        Some((requests, public, gate, terminal))
    } else {
        None
    };
    let requests = execute(env!("CARGO_BIN_EXE_ub-review"))?;
    if let Some((prior_requests, public, gate, terminal)) = baseline {
        ensure!(
            requests == prior_requests,
            "shadow candidate changed original baseline provider prompts or call ordering"
        );
        for (artifact, expected) in [
            ("github-review.json", public),
            ("gate_outcome.json", gate),
            ("terminal_state.json", terminal),
        ] {
            let actual = common::read_json(&out.join("review").join(artifact))?;
            ensure!(
                actual == expected,
                "shadow candidate changed original baseline {artifact}: expected {expected}; actual {actual}"
            );
        }
    }
    ensure!(
        requests.len() == expected,
        "expected preflight, primary, optional refuter, reporter, continuation and redistillation"
    );
    let primary_request = requests
        .iter()
        .filter_map(|request| request["messages"].as_array()?.last()?["content"].as_str())
        .find(|prompt| prompt.contains("\n\nLane task:\n\nLane: bounds\n"))
        .context("captured primary request")?;
    ensure!(
        primary_request.contains(base) && primary_request.contains(head),
        "primary request must identify exact reviewed base {base} and head {head}"
    );
    let thread_context = common::read_json(&out.join("review/pr_thread_context.json"))?;
    match prior_context {
        Some(path) if path.exists() => {
            let prior = fs::read_to_string(path)?;
            ensure!(
                thread_context["status"] == "seeded" && thread_context["thread_context"] == prior,
                "repair must seed the persisted prior review without replacing it: {thread_context}"
            );
            ensure!(
                primary_request.contains("Historical review at prior head")
                    && primary_request.contains(DEFECT_FINDING)
                    && primary_request.contains("byte_at(&[7], 1) panics")
                    && primary_request.contains(Revision::Defect.summary())
                    && primary_request.contains("Seeded Thread Reuse Rules"),
                "repair prompt lost prior finding, mechanism or historical reuse instructions"
            );
        }
        Some(_) => {
            ensure!(
                thread_context["status"] == "unavailable"
                    && thread_context["thread_context"].is_null()
                    && thread_context["warnings"]
                        .as_array()
                        .is_some_and(|warnings| warnings.iter().any(|warning| warning
                            .as_str()
                            .is_some_and(|text| text.contains("thread-context-file unavailable")))),
                "missing prior context must be unavailable evidence: {thread_context}"
            );
            ensure!(
                !primary_request.contains(DEFECT_FINDING),
                "unavailable prior context synthesized the earlier defect"
            );
        }
        None => ensure!(
            thread_context["status"] == "absent",
            "unseeded run must record absent context: {thread_context}"
        ),
    }
    let diff = common::run_capture_with_env(
        repo,
        "git",
        &[
            "diff",
            "--no-ext-diff",
            "--unified=3",
            base,
            head,
            "--",
            "src/lib.rs",
        ],
        &[],
    )?;
    for line in diff.lines().filter(|s| {
        s.starts_with("@@")
            || ((s.starts_with('+') || s.starts_with('-'))
                && !s.starts_with("+++")
                && !s.starts_with("---"))
    }) {
        ensure!(
            primary_request.contains(line),
            "captured requests omitted exact diff line {line}"
        );
    }
    let raw = common::read_json(&out.join("review/review.json"))?;
    let inline = raw["inline_comments"]
        .as_array()
        .context("raw inline findings")?;
    let lane = raw["model_lanes"]
        .as_array()
        .context("model lane receipts")?
        .iter()
        .find(|lane| lane["lane"] == "bounds")
        .context("bounds receipt")?;
    ensure!(
        lane["status"] == "ok" && lane["reason"] == "completed",
        "operational receipt must remain operational: {lane}"
    );
    if matches!(revision, Revision::Defect) {
        ensure!(
            inline.len() == 1 && inline[0]["path"] == "src/lib.rs" && inline[0]["line"] == 2,
            "defect must survive refuter as a concrete inline finding: {inline:?}"
        );
        ensure!(
            inline[0]["body"]
                .as_str()
                .is_some_and(|body| body.contains(DEFECT_FINDING)),
            "defect meaning lost in raw review artifact"
        );
        ensure!(
            requests
                .iter()
                .any(|request| request.to_string().contains("You are the final refuter")),
            "defect never reached refuter"
        );
    } else {
        ensure!(
            inline.is_empty() && raw["terminal_state"]["substantive_summary_only_findings"] == 0,
            "fresh repaired/control review retained substantive raw findings: {raw}"
        );
    }
    ensure!(
        !raw["body"].as_str().unwrap_or_default().contains(PRIVATE),
        "raw review body exposed private audit"
    );
    let public = common::read_json(&out.join("review/github-review.json"))?;
    let comments = public["comments"]
        .as_array()
        .context("final GitHub review comments")?;
    let body = public["body"]
        .as_str()
        .context("final GitHub review body")?;
    ensure!(
        public["event"] == "COMMENT",
        "final GitHub review must retain the bounded COMMENT event: {public}"
    );
    ensure!(
        !public.to_string().contains(PRIVATE),
        "final GitHub review payload exposed private audit: {public}"
    );
    if matches!(revision, Revision::Defect) {
        ensure!(
            comments.len() == 1
                && comments[0]["path"] == "src/lib.rs"
                && comments[0]["line"] == 2
                && comments[0]["side"] == "RIGHT"
                && comments[0]["body"]
                    .as_str()
                    .is_some_and(|comment| comment.contains(DEFECT_FINDING)),
            "final GitHub review must preserve the concrete defect at its changed line: {public}"
        );
    } else {
        ensure!(
            comments.is_empty() && !body.contains(DEFECT_FINDING),
            "final repaired/control GitHub review retained the stale defect: {public}"
        );
    }
    Ok(ReviewRun {
        revision,
        out,
        requests,
    })
}

fn check_semantics(run: &ReviewRun) -> Result<()> {
    let prompts: Vec<&str> = run
        .requests
        .iter()
        .filter_map(|r| r["messages"].as_array()?.last()?["content"].as_str())
        .filter_map(|prompt| prompt.rsplit("\n\nLane task:\n\n").next())
        .collect();
    let reporter = prompts
        .iter()
        .find(|p| p.starts_with("# Reporter Coordination Task"))
        .context("reporter request")?;
    let continuation = prompts
        .iter()
        .find(|p| p.starts_with("# Lane continuation:"))
        .context("continuation request")?;
    let primary: Value = common::read_json(&run.out.join("review/threads/bounds/turn-000.json"))?;
    let messages = fs::read_to_string(run.out.join("review/messages.ndjson"))?;
    let message: Value = messages
        .lines()
        .map(serde_json::from_str::<Value>)
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .find(|m| m["kind"] == "lane_report" && m["from"] == "bounds" && m["turn"] == 0)
        .context("primary lane report")?;
    ensure!(
        primary["response_summary"] == "completed"
            && message["payload"]["conclusion"] == "completed",
        "primary live turn/message must preserve original operational conclusion"
    );
    let live_prior = continuation
        .split_once("## Your prior conclusion\n\n")
        .and_then(|(_, rest)| rest.split_once("\n\n## The reporter's summary"))
        .map(|(prior, _)| prior)
        .context("live continuation prior-conclusion section")?;
    for (name, text) in [
        ("live reporter", *reporter),
        ("live continuation prior", live_prior),
    ] {
        ensure!(
            text.contains("completed"),
            "{name} lost original operational input"
        );
        ensure!(
            !text.contains(run.revision.summary())
                && !text.contains(DEFECT_FINDING)
                && !text.contains(LIMIT),
            "inspection-only semantic state entered {name}: {text}"
        );
    }
    let raw = common::read_json(&run.out.join("review/review.json"))?;
    let semantic = raw["model_lanes"]
        .as_array()
        .and_then(|lanes| lanes.iter().find(|lane| lane["lane"] == "bounds"))
        .and_then(|lane| lane["semantic_state"]["conclusion"].as_str())
        .context("persisted primary semantic conclusion")?;
    let shadow_dir = run.out.join("review/semantic-shadow");
    let shadow_primary = common::read_json(&shadow_dir.join("primary.json"))?;
    let shadow_reporter = common::read_json(&shadow_dir.join("reporter-000.json"))?;
    let shadow_continuation = common::read_json(&shadow_dir.join("continuation-bounds-001.json"))?;
    for (name, sidecar) in [
        ("primary", &shadow_primary),
        ("reporter", &shadow_reporter),
        ("continuation", &shadow_continuation),
    ] {
        ensure!(
            sidecar["schema"] == "ub-review.semantic-shadow.v1" && sidecar["authority"] == false,
            "{name} sidecar must explicitly remain inspection-only: {sidecar}"
        );
    }
    for (name, sidecar) in [
        ("reporter", &shadow_reporter),
        ("continuation", &shadow_continuation),
    ] {
        ensure!(
            sidecar["extra_provider_calls"] == 0
                && sidecar["response_provenance"]
                    .as_str()
                    .is_some_and(|provenance| provenance.contains("not separately answered")),
            "{name} must distinguish recorded live response from unexecuted shadow prompt"
        );
    }
    let primary_conclusion = shadow_primary["lanes"]
        .as_array()
        .and_then(|lanes| lanes.iter().find(|lane| lane["lane"] == "bounds"))
        .and_then(|lane| lane["conclusion"].as_str())
        .context("primary shadow bounds conclusion")?;
    for (name, text) in [
        ("persisted primary state", semantic.to_owned()),
        ("primary shadow", primary_conclusion.to_owned()),
        (
            "reporter shadow",
            shadow_reporter["counterfactual_prompt"]
                .as_str()
                .context("shadow reporter prompt")?
                .to_owned(),
        ),
        (
            "continuation shadow",
            shadow_continuation["counterfactual_prompt"]
                .as_str()
                .context("shadow continuation prompt")?
                .to_owned(),
        ),
    ] {
        ensure!(
            text.contains(run.revision.summary()),
            "{name} lost semantic summary: {text}"
        );
        ensure!(text.contains(LIMIT), "{name} lost coverage limit: {text}");
        ensure!(!text.contains(PRIVATE), "{name} exposed internal audit");
        if matches!(run.revision, Revision::Defect) {
            ensure!(
                text.contains(DEFECT_FINDING),
                "{name} lost material finding: {text}"
            );
        } else {
            ensure!(
                !text.contains(DEFECT_FINDING),
                "{name} inherited stale defect: {text}"
            );
        }
    }
    let latest = common::read_json(&run.out.join("review/threads/reporter/turn-001.json"))?;
    ensure!(
        latest["verdict"] == run.revision.verdict(),
        "wrong revision verdict: {latest}"
    );
    Ok(())
}

#[test]
fn cli_defect_repair_and_independent_control_preserve_semantics() -> Result<()> {
    let _guard = common::cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    fs::create_dir_all(repo.join("src"))?;
    common::run(&repo, "git", &["init"])?;
    common::run(
        &repo,
        "git",
        &["config", "user.email", "semantic@example.invalid"],
    )?;
    common::run(
        &repo,
        "git",
        &["config", "user.name", "Semantic regression"],
    )?;
    let baseline = commit(&repo, STRICT, "strict baseline")?;
    let defect = commit(&repo, DEFECT, "introduce equality defect")?;
    let witness_source = temp.path().join("boundary-witness.rs");
    let witness_bin = temp.path().join("boundary-witness");
    let boundary_test = "#[test] fn rejects_equal_length() { assert_eq!(byte_at(&[7], 1), None); assert_eq!(byte_at(&[], 0), None); }";
    fs::write(&witness_source, format!("{DEFECT}\n{boundary_test}"))?;
    common::run(
        &repo,
        "rustc",
        &[
            "--test",
            common::path_str(&witness_source)?,
            "-o",
            common::path_str(&witness_bin)?,
        ],
    )?;
    let defect_witness = common::run_expect_failure(&repo, common::path_str(&witness_bin)?, &[])?;
    ensure!(
        defect_witness.contains("index out of bounds"),
        "defect witness must expose equality panic: {defect_witness}"
    );
    let config = temp.path().join("review.toml");
    fs::write(
        &config,
        "[review]\nenable_default_lanes = false\n[[lanes]]\nid = \"bounds\"\nrole = \"Bounds review\"\nfocus = \"Review byte_at equality behavior\"\n[tools.cargo-allow]\nenabled = false\n",
    )?;
    // The first transaction produces the historical evidence used for reconsideration.
    let defective = review(
        &repo,
        &config,
        &baseline,
        &defect,
        temp.path().join("defect"),
        Revision::Defect,
        None,
    )?;
    let prior_turn = common::read_json(&defective.out.join("review/threads/bounds/turn-000.json"))?;
    let prior_review = common::read_json(&defective.out.join("review/review.json"))?;
    let prior_receipt = prior_review["model_lanes"]
        .as_array()
        .and_then(|lanes| lanes.iter().find(|lane| lane["lane"] == "bounds"))
        .context("persisted prior bounds receipt")?;
    ensure!(
        prior_receipt["semantic_state"]["conclusion"]
            .as_str()
            .is_some_and(|conclusion| conclusion.contains(DEFECT_FINDING)),
        "historical context must come from persisted substantive semantic state"
    );
    let prior_context = temp.path().join("prior-defect-review.txt");
    fs::write(
        &prior_context,
        format!(
            "Historical review at prior head {defect}; reconsider against the current diff, not as a current-head finding.\n{}\n{}\n",
            serde_json::to_string_pretty(&prior_turn)?,
            serde_json::to_string_pretty(&prior_receipt["semantic_state"])?
        ),
    )?;
    let repair = commit(&repo, REPAIRED, "restore strict guard and cover boundary")?;
    common::run(
        &repo,
        "rustc",
        &[
            "--test",
            "src/lib.rs",
            "-o",
            common::path_str(&temp.path().join("boundary-tests"))?,
        ],
    )?;
    common::run(
        &repo,
        common::path_str(&temp.path().join("boundary-tests"))?,
        &[],
    )?;
    let repaired = review(
        &repo,
        &config,
        &defect,
        &repair,
        temp.path().join("repair"),
        Revision::Repair,
        Some(&prior_context),
    )?;
    common::run(&repo, "git", &["checkout", "--detach", &baseline])?;
    let control = commit(&repo, CONTROL, "independent checked-access control")?;
    fs::write(&witness_source, format!("{CONTROL}\n{boundary_test}"))?;
    common::run(
        &repo,
        "rustc",
        &[
            "--test",
            common::path_str(&witness_source)?,
            "-o",
            common::path_str(&witness_bin)?,
        ],
    )?;
    common::run(&repo, common::path_str(&witness_bin)?, &[])?;
    let independent = review(
        &repo,
        &config,
        &baseline,
        &control,
        temp.path().join("control"),
        Revision::Control,
        Some(&temp.path().join("unavailable-prior-review.txt")),
    )?;
    for run in [&defective, &repaired, &independent] {
        check_semantics(run)?;
    }
    Ok(())
}

#[test]
fn reused_output_rejects_semantic_shadow_from_prior_review_identity() -> Result<()> {
    use sha2::{Digest, Sha256};
    let _guard = common::cli_subprocess_test_lock()?;
    let temp = tempfile::Builder::new()
        .prefix("reuse-999-cli-")
        .tempdir()?;
    let retained = temp.path().to_path_buf();
    let _cleanup = if std::env::var_os("UB_REVIEW_FRESHNESS_RETAIN").is_some() {
        let _retained_path = temp.keep();
        None
    } else {
        Some(temp)
    };
    eprintln!("retained freshness reproduction: {}", retained.display());
    let repo = retained.join("repo");
    fs::create_dir_all(repo.join("src"))?;
    common::run(&repo, "git", &["init"])?;
    common::run(
        &repo,
        "git",
        &["config", "user.email", "freshness@example.invalid"],
    )?;
    common::run(
        &repo,
        "git",
        &["config", "user.name", "Freshness regression"],
    )?;
    let base = commit(&repo, STRICT, "baseline")?;
    let old_head = commit(&repo, DEFECT, "prior unmistakable equality defect")?;
    let config = retained.join("review.toml");
    fs::write(
        &config,
        "[review]\nenable_default_lanes = false\n[[lanes]]\nid = \"bounds\"\nrole = \"Bounds review\"\nfocus = \"Review byte_at equality behavior\"\n[tools.cargo-allow]\nenabled = false\n",
    )?;
    let out = retained.join("reused-out");
    let first = review(
        &repo,
        &config,
        &base,
        &old_head,
        out.clone(),
        Revision::Defect,
        None,
    )?;
    check_semantics(&first)?;
    let stage = out.join("review/semantic-shadow/reporter-001.json");
    let old_stage = fs::read(&stage)?;
    ensure!(
        String::from_utf8_lossy(&old_stage).contains(DEFECT_FINDING),
        "initial stage lacks prior defect"
    );
    let old_review = fs::read(out.join("review/review.json"))?;
    let old_current: Value = serde_json::from_slice(&old_review)?;
    let new_head = commit(&repo, REPAIRED, "distinct repaired review identity")?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}", listener.local_addr()?);
    let provider = thread::spawn(move || {
        common::serve_fake_http(
            listener,
            2,
            "malformed primary freshness",
            Duration::from_secs(15),
            |_, stream| serve_request_mode(stream, Revision::Repair, true),
        )
    });
    let result = common::run_capture_with_env(
        &repo,
        env!("CARGO_BIN_EXE_ub-review"),
        &[
            "run",
            "--dry-run",
            "--config",
            common::path_str(&config)?,
            "--root",
            common::path_str(&repo)?,
            "--base",
            &old_head,
            "--head",
            &new_head,
            "--out",
            common::path_str(&out)?,
            "--no-github-summary",
            "--model-mode",
            "auto",
            "--provider-policy",
            "minimax-only",
            "--minimax-provider-kind",
            "openai",
            "--lanes",
            "bounds",
            "--tools",
            "cargo-allow",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "1",
            "--model-timeout-sec",
            "5",
        ],
        &[
            ("UB_REVIEW_MINIMAX_API_KEY", "semantic-test-only"),
            ("UB_REVIEW_MINIMAX_API_URL", &url),
        ],
    );
    let requests = common::join_fake_provider(provider)?;
    let stdout = result?;
    fs::write(retained.join("second-run-output.txt"), stdout)?;
    fs::write(
        retained.join("second-run-requests.json"),
        serde_json::to_vec_pretty(&requests)?,
    )?;
    let current_review_bytes = fs::read(out.join("review/review.json"))?;
    let current: Value = serde_json::from_slice(&current_review_bytes)?;
    let lanes = current["model_lanes"]
        .as_array()
        .context("current model lanes")?;
    let lane = lanes
        .iter()
        .find(|lane| lane["lane"] == "bounds")
        .context("current bounds lane")?;
    ensure!(
        lane["semantic_state"].is_null(),
        "malformed primary unexpectedly has semantic state: {lane}"
    );
    ensure!(
        lane["status"] != "ok",
        "malformed primary incorrectly completed: {lane}"
    );
    let primary = fs::read(out.join("review/semantic-shadow/primary.json"))?;
    let latest_stage = fs::read(&stage)?;
    let envelope: Value = serde_json::from_slice(&latest_stage)?;
    let public_path = out.join("review/github-review.json");
    let public = if public_path.exists() {
        fs::read_to_string(&public_path)?
    } else {
        String::new()
    };
    let gate = fs::read_to_string(out.join("review/gate_outcome.json"))?;
    let retained_evidence = json!({
        "old_head":old_head,"current_head":new_head,
        "old_review_sha256":format!("{:x}", Sha256::digest(&old_review)),
        "current_review_sha256":format!("{:x}", Sha256::digest(&current_review_bytes)),
        "old_stage_sha256":format!("{:x}", Sha256::digest(&old_stage)),
        "current_stage_sha256":format!("{:x}", Sha256::digest(&latest_stage)),
        "current_primary_shadow_sha256":format!("{:x}", Sha256::digest(&primary)),
        "current_primary_lane":lane,
        "stage_top_level_keys":envelope.as_object().context("stage envelope")?.keys().collect::<Vec<_>>(),
        "stage_invocation":envelope["invocation"],
        "current_invocation":current["semantic_shadow_invocation"],
        "public_contains_prior_finding":public.contains(DEFECT_FINDING),
        "gate_contains_prior_finding":gate.contains(DEFECT_FINDING),
    });
    fs::write(
        retained.join("freshness-evidence.json"),
        serde_json::to_vec_pretty(&retained_evidence)?,
    )?;
    eprintln!("freshness evidence: {retained_evidence}");
    ensure!(
        !public.contains(DEFECT_FINDING) && !gate.contains(DEFECT_FINDING),
        "shadow stale data entered current public/gate output"
    );
    // A supported same-head, model-off rerun also skips reporter execution.
    fs::write(
        retained.join("failed-primary-review.json"),
        &current_review_bytes,
    )?;
    common::run(
        &repo,
        env!("CARGO_BIN_EXE_ub-review"),
        &[
            "run",
            "--dry-run",
            "--config",
            common::path_str(&config)?,
            "--root",
            common::path_str(&repo)?,
            "--base",
            &old_head,
            "--head",
            &new_head,
            "--out",
            common::path_str(&out)?,
            "--no-github-summary",
            "--model-mode",
            "off",
            "--lanes",
            "bounds",
            "--tools",
            "cargo-allow",
            "--max-model-calls",
            "0",
        ],
    )?;
    let rerun_stage = fs::read(&stage)?;
    let rerun_review = fs::read(out.join("review/review.json"))?;
    let rerun: Value = serde_json::from_slice(&rerun_review)?;
    let rerun_evidence = json!({
        "head":new_head, "mode":"off", "max_model_calls":0,
        "review_sha256":format!("{:x}", Sha256::digest(&rerun_review)),
        "stage_sha256":format!("{:x}", Sha256::digest(&rerun_stage)),
        "stage_is_prior_bytes":rerun_stage == old_stage,
        "invocation":rerun["semantic_shadow_invocation"],
    });
    fs::write(
        retained.join("same-head-skipped-evidence.json"),
        serde_json::to_vec_pretty(&rerun_evidence)?,
    )?;
    eprintln!("same-head skipped evidence: {rerun_evidence}");
    let prior_invocation = &envelope["invocation"];
    let current_invocation = &current["semantic_shadow_invocation"];
    let rerun_invocation = &rerun["semantic_shadow_invocation"];
    ensure!(
        prior_invocation["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
            && current_invocation["id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
            && rerun_invocation["id"]
                .as_str()
                .is_some_and(|id| !id.is_empty()),
        "STALE SHADOW CANNOT BE CLASSIFIED: old stage/current failed review/same-head skipped rerun lack invocation identity; evidence {}",
        retained.display()
    );
    ensure!(
        prior_invocation["head"] == old_head && current_invocation["head"] == new_head,
        "stage and current review must retain distinct reviewed heads"
    );
    ensure!(
        *prior_invocation == old_current["semantic_shadow_invocation"],
        "successful first reporter stage must match its current review invocation"
    );
    ensure!(
        prior_invocation != current_invocation && prior_invocation != rerun_invocation,
        "prior reporter stage must be classifiable stale for both current transactions"
    );
    ensure!(
        current_invocation["id"] != rerun_invocation["id"],
        "same-head skipped rerun must have an independent invocation identity"
    );
    let primary_envelope: Value = serde_json::from_slice(&primary)?;
    ensure!(
        primary_envelope["invocation"] == *current_invocation,
        "current primary shadow must match the failed review invocation"
    );
    Ok(())
}
