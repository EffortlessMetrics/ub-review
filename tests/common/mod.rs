//! Shared helpers for integration tests (extracted from tests/cli.rs, #613).

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

pub const CARGO_ALLOW_FOREIGN_REASON: &str = "policy/allow.toml is not a cargo-allow-dialect ledger; add \
     policy/cargo-allow.toml (see EffortlessMetrics/cargo-allow#1465)";

pub fn assert_fake_core_review_tool_version(dir: &Path, tool: &str, expected: &str) -> Result<()> {
    let executable = if cfg!(windows) {
        dir.join(format!("{tool}.exe"))
    } else {
        dir.join(tool)
    };
    assert!(
        executable.exists(),
        "fake {tool} executable should exist at {}",
        executable.display()
    );
    let output = run_capture_with_env(dir, path_str(&executable)?, &[], &[])?;
    assert_eq!(output.trim(), format!("{tool} {expected}"));
    Ok(())
}

pub fn prepend_to_path(dir: &Path) -> Result<String> {
    let mut paths = vec![dir.to_path_buf()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    Ok(std::env::join_paths(paths)?.to_string_lossy().into_owned())
}

pub fn spawn_fake_github_api() -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}", listener.local_addr()?);
    let handle = thread::spawn(move || -> Result<Vec<String>> {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            match listener.accept() {
                Ok((stream, _addr)) => return Ok(vec![handle_fake_github_request(stream)?]),
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!("fake GitHub API received no requests");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(err.into()),
            }
        }
    });
    Ok((url, handle))
}

pub fn spawn_fake_setup_ci_api(
    expected_requests: usize,
    config_exists: bool,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}", listener.local_addr()?);
    let handle = thread::spawn(move || -> Result<Vec<String>> {
        let deadline = Instant::now() + Duration::from_secs(20);
        let mut requests = Vec::new();
        while requests.len() < expected_requests {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    requests.push(handle_fake_setup_ci_request(stream, config_exists)?);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!(
                            "fake setup-ci API received {} of {} requests",
                            requests.len(),
                            expected_requests
                        );
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(requests)
    });
    Ok((url, handle))
}

pub fn handle_fake_setup_ci_request(mut stream: TcpStream, config_exists: bool) -> Result<String> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            bail!("fake setup-ci request ended before headers finished");
        }
        headers.push_str(&line);
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or_default();
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let request_line = headers.lines().next().unwrap_or_default().to_owned();
    let (status_line, response_body) =
        if request_line.starts_with("GET /repos/") && request_line.contains("/git/ref/heads/") {
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({
                    "object": {"sha": "basesha0000000000000000000000000000000000"}
                }))?,
            )
        } else if request_line.starts_with("GET /repos/") && request_line.contains("/git/trees/") {
            let mut entries = vec![serde_json::json!({"path": "README.md"})];
            if config_exists {
                entries.push(serde_json::json!({"path": ".ub-review.toml"}));
            }
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({"tree": entries}))?,
            )
        } else if request_line.starts_with("GET /repos/") {
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({"default_branch": "main"}))?,
            )
        } else if request_line.starts_with("POST ") && request_line.contains("/pulls") {
            (
                "HTTP/1.1 201 Created",
                serde_json::to_vec(&serde_json::json!({
                    "html_url": "https://github.com/acme/widgets/pull/77"
                }))?,
            )
        } else {
            (
                "HTTP/1.1 201 Created",
                serde_json::to_vec(&serde_json::json!({}))?,
            )
        };
    write!(
        stream,
        "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response_body.len()
    )?;
    stream.write_all(&response_body)?;
    Ok(format!(
        "{request_line}\n{}",
        String::from_utf8_lossy(&body)
    ))
}

pub fn handle_fake_github_request(mut stream: TcpStream) -> Result<String> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            bail!("fake GitHub request ended before headers finished");
        }
        headers.push_str(&line);
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or_default();
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let request_text = format!("{headers}{}", String::from_utf8_lossy(&body));
    let response_body = serde_json::to_vec(&serde_json::json!({
        "id": 987,
        "state": "COMMENTED",
        "body": "fake review posted"
    }))?;
    write!(
        stream,
        "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response_body.len()
    )?;
    stream.write_all(&response_body)?;
    Ok(request_text)
}

pub fn spawn_fake_openai_provider(
    expected_requests: usize,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    spawn_fake_openai_provider_with_contents(
        (0..expected_requests)
            .map(|_| fake_openai_lane_content())
            .collect(),
    )
}

pub fn spawn_fake_openai_provider_with_delay(
    expected_requests: usize,
    delay_ms: u64,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    spawn_fake_openai_provider_with_contents_and_delay(
        (0..expected_requests)
            .map(|_| fake_openai_lane_content())
            .collect(),
        Duration::from_millis(delay_ms),
    )
}

pub fn spawn_fake_openai_provider_with_contents(
    contents: Vec<String>,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    spawn_fake_openai_provider_with_contents_and_delay(contents, Duration::ZERO)
}

pub fn spawn_fake_openai_provider_with_contents_and_delay(
    contents: Vec<String>,
    response_delay: Duration,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let handle = thread::spawn(move || -> Result<Vec<String>> {
        let expected_requests = contents.len();
        let mut deadline = Instant::now() + Duration::from_secs(120);
        let mut requests = Vec::new();
        while requests.len() < expected_requests {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    let content = contents
                        .get(requests.len())
                        .ok_or_else(|| anyhow::anyhow!("fake provider response missing"))?;
                    requests.push(handle_fake_openai_request(stream, content, response_delay)?);
                    deadline = Instant::now() + Duration::from_secs(120);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!(
                            "fake provider received {} of {} requests",
                            requests.len(),
                            expected_requests
                        );
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(requests)
    });
    Ok((url, handle))
}

pub fn spawn_fake_openai_provider_with_statuses(
    statuses: Vec<u16>,
) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}/v1/chat/completions", listener.local_addr()?);
    let handle = thread::spawn(move || -> Result<Vec<String>> {
        let expected_requests = statuses.len();
        let mut deadline = Instant::now() + Duration::from_secs(120);
        let mut requests = Vec::new();
        while requests.len() < expected_requests {
            match listener.accept() {
                Ok((stream, _addr)) => {
                    let status = statuses
                        .get(requests.len())
                        .copied()
                        .ok_or_else(|| anyhow::anyhow!("fake provider status missing"))?;
                    requests.push(handle_fake_openai_request_with_status(
                        stream,
                        "fake provider status response",
                        Duration::ZERO,
                        status,
                    )?);
                    deadline = Instant::now() + Duration::from_secs(120);
                }
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    if Instant::now() >= deadline {
                        bail!(
                            "fake provider received {} of {} requests",
                            requests.len(),
                            expected_requests
                        );
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(requests)
    });
    Ok((url, handle))
}

pub fn cli_subprocess_test_lock() -> Result<MutexGuard<'static, ()>> {
    static CLI_SUBPROCESS_TEST_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    // Recover a poisoned lock instead of erroring: one failing test must
    // produce one failure receipt, not cascade into every later subprocess
    // test in the suite.
    Ok(CLI_SUBPROCESS_TEST_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner))
}

pub fn fake_openai_lane_content() -> String {
    serde_json::json!({
        "summary": "fake provider ok",
        "inline_comments": [],
        "summary_only_findings": []
    })
    .to_string()
}

pub fn handle_fake_openai_request(
    stream: TcpStream,
    content: &str,
    response_delay: Duration,
) -> Result<String> {
    handle_fake_openai_request_with_status(stream, content, response_delay, 200)
}

fn handle_fake_openai_request_with_status(
    mut stream: TcpStream,
    content: &str,
    response_delay: Duration,
    status: u16,
) -> Result<String> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut headers = String::new();
    loop {
        let mut line = String::new();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0 {
            bail!("fake provider request ended before headers finished");
        }
        headers.push_str(&line);
        if line == "\r\n" || line == "\n" {
            break;
        }
    }
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or_default();
    let mut body = vec![0; content_length];
    reader.read_exact(&mut body)?;
    let request_text = format!("{headers}{}", String::from_utf8_lossy(&body));
    if !response_delay.is_zero() {
        thread::sleep(response_delay);
    }
    let response_body = if status >= 400 {
        serde_json::to_vec(&serde_json::json!({
            "error": {"message": content}
        }))?
    } else {
        serde_json::to_vec(&serde_json::json!({
            "choices": [
                {
                    "message": {
                        "content": content
                    }
                }
            ]
        }))?
    };
    let reason = match status {
        200 => "OK",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        _ => "Fake Provider Status",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response_body.len(),
    )?;
    stream.write_all(&response_body)?;
    Ok(request_text)
}

pub fn join_fake_provider(handle: thread::JoinHandle<Result<Vec<String>>>) -> Result<Vec<String>> {
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("fake provider thread panicked"))?
}

/// Builds a child command with ambient ub-review/runtime profile state scrubbed.
///
/// When the dogfood gate runs this suite, the surrounding GitHub Actions step
/// exports `UB_REVIEW_PROFILE`, `UB_REVIEW_RUNTIME_PROFILE`,
/// `UB_REVIEW_TOOL_BUNDLE`, and friends. The spawned `ub-review` binary picks
/// those up through clap `env = "UB_REVIEW_..."` fallbacks, so nested test
/// runs silently resolve a gh-runner profile and assertions about default
/// profile output fail only inside the gate. Scrubbing the prefix first keeps
/// tests hermetic.
///
/// Hosted runner identity is also scrubbed: many CLI fixture tests exercise
/// specific planner branches, and inheriting `GITHUB_ACTIONS=true` can make an
/// unrelated box guard run before the branch under test. Explicit per-test envs
/// are applied afterwards and still win.
pub fn isolated_command(program: &str, cwd: &Path) -> Command {
    let mut command = Command::new(program);
    command.current_dir(cwd);
    for (name, _) in std::env::vars_os() {
        let name_string = name.to_string_lossy();
        if name_string.starts_with("UB_REVIEW_")
            || matches!(
                name_string.as_ref(),
                "GITHUB_ACTIONS" | "RUNNER_ENVIRONMENT" | "RUNNER_NAME"
            )
        {
            command.env_remove(&name);
        }
    }
    command
}

pub fn run(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
    let output = isolated_command(program, cwd).args(args).output()?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{} {:?} failed\nstdout:\n{}\nstderr:\n{}",
        program,
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn run_with_env(cwd: &Path, program: &str, args: &[&str], envs: &[(&str, &str)]) -> Result<()> {
    let mut command = isolated_command(program, cwd);
    command.args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output()?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{} {:?} failed\nstdout:\n{}\nstderr:\n{}",
        program,
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

pub fn run_capture_with_env(
    cwd: &Path,
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<String> {
    let mut command = isolated_command(program, cwd);
    command.args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output()?;
    let combined = format!(
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if output.status.success() {
        return Ok(combined);
    }
    bail!("{program} {args:?} failed\n{combined}");
}

pub fn run_expect_failure(cwd: &Path, program: &str, args: &[&str]) -> Result<String> {
    let output = isolated_command(program, cwd).args(args).output()?;
    if !output.status.success() {
        return Ok(format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    bail!("{program} {args:?} unexpectedly succeeded");
}

pub fn run_expect_failure_with_env(
    cwd: &Path,
    program: &str,
    args: &[&str],
    envs: &[(&str, &str)],
) -> Result<String> {
    let mut command = isolated_command(program, cwd);
    command.args(args);
    for (name, value) in envs {
        command.env(name, value);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Ok(format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    bail!("{program} {args:?} unexpectedly succeeded");
}

pub fn path_str(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {}", path.display()))
}

pub fn json_array_field<'a>(
    value: &'a serde_json::Value,
    field: &str,
) -> Result<&'a [serde_json::Value]> {
    value
        .get(field)
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| anyhow::anyhow!("JSON field `{field}` is not an array"))
}

pub fn json_str_field<'a>(value: &'a serde_json::Value, field: &str) -> Result<&'a str> {
    value
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("JSON field `{field}` is not a string"))
}

pub fn read_json(path: &Path) -> Result<serde_json::Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}

pub fn base64_standard_for_test(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = chunk.get(1).copied().map(u32::from).unwrap_or(0);
        let b2 = chunk.get(2).copied().map(u32::from).unwrap_or(0);
        let triple = (b0 << 16) | (b1 << 8) | b2;
        encoded.push(ALPHABET[((triple >> 18) & 0x3f) as usize] as char);
        encoded.push(ALPHABET[((triple >> 12) & 0x3f) as usize] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[((triple >> 6) & 0x3f) as usize] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[(triple & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    encoded
}

pub fn tool_entry<'a>(
    artifact: &'a serde_json::Value,
    tool_id: &str,
) -> Result<&'a serde_json::Value> {
    json_array_field(artifact, "tools")?
        .iter()
        .find(|tool| tool["id"] == tool_id)
        .ok_or_else(|| anyhow::anyhow!("{tool_id} tool entry missing"))
}

pub fn tool_gate_outcome<'a>(
    artifact: &'a serde_json::Value,
    tool_id: &str,
) -> Result<&'a serde_json::Value> {
    json_array_field(artifact, "outcomes")?
        .iter()
        .find(|outcome| outcome["tool"] == tool_id)
        .ok_or_else(|| anyhow::anyhow!("{tool_id} tool gate outcome missing"))
}

pub fn assert_cargo_allow_foreign_skip_artifacts(out: &Path) -> Result<()> {
    let resolved_tools = read_json(&out.join("resolved-tools.json"))?;
    let review_resolved_tools = read_json(&out.join("review/resolved-tools.json"))?;
    assert_eq!(resolved_tools, review_resolved_tools);
    let cargo_allow = tool_entry(&resolved_tools, "cargo-allow")?;
    assert_eq!(cargo_allow["planned_run"], serde_json::json!(false));
    assert_eq!(cargo_allow["plan_reason"], CARGO_ALLOW_FOREIGN_REASON);

    let sensor_status = read_json(&out.join("sensors/cargo-allow/ub-review-sensor-status.json"))?;
    assert_eq!(sensor_status["sensor"], "cargo-allow");
    assert_eq!(sensor_status["status"], "skipped");
    assert_eq!(sensor_status["reason"], CARGO_ALLOW_FOREIGN_REASON);

    let tool_status = read_json(&out.join("tool-status.json"))?;
    let review_tool_status = read_json(&out.join("review/tool-status.json"))?;
    assert_eq!(tool_status, review_tool_status);
    let cargo_allow_status = tool_entry(&tool_status, "cargo-allow")?;
    assert_eq!(cargo_allow_status["planned_run"], serde_json::json!(false));
    assert_eq!(cargo_allow_status["status"], "skipped");
    assert_eq!(cargo_allow_status["reason"], CARGO_ALLOW_FOREIGN_REASON);

    let tool_gate_outcomes = read_json(&out.join("tool-gate-outcomes.json"))?;
    let review_tool_gate_outcomes = read_json(&out.join("review/tool-gate-outcomes.json"))?;
    assert_eq!(tool_gate_outcomes, review_tool_gate_outcomes);
    let cargo_allow_outcome = tool_gate_outcome(&tool_gate_outcomes, "cargo-allow")?;
    assert_eq!(cargo_allow_outcome["planned_run"], serde_json::json!(false));
    assert_eq!(cargo_allow_outcome["sensor_status"], "skipped");
    assert_eq!(
        cargo_allow_outcome["sensor_reason"],
        CARGO_ALLOW_FOREIGN_REASON
    );
    assert_eq!(cargo_allow_outcome["outcome"], "not_evaluated");
    assert!(
        cargo_allow_outcome["reason"]
            .as_str()
            .is_some_and(|reason| reason.contains(CARGO_ALLOW_FOREIGN_REASON)),
        "tool gate reason should preserve linked cargo-allow skip reason: {cargo_allow_outcome}"
    );
    Ok(())
}

pub fn event_kinds(path: &Path) -> Result<Vec<String>> {
    let events = event_records(path)?;
    Ok(events
        .iter()
        .filter_map(|event| event["kind"].as_str().map(str::to_owned))
        .collect())
}

pub fn event_records(path: &Path) -> Result<Vec<serde_json::Value>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut kinds = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let event: serde_json::Value = serde_json::from_str(&line)?;
        assert!(event["ts"].as_str().is_some(), "event missing ts: {event}");
        assert!(
            event["payload"].is_object(),
            "event missing payload object: {event}"
        );
        let kind = event["kind"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("event missing kind: {event}"))?;
        assert!(!kind.is_empty(), "event kind is empty: {event}");
        kinds.push(event);
    }
    Ok(kinds)
}

pub fn sum_json_object_values(value: &serde_json::Value) -> u64 {
    value
        .as_object()
        .map(|values| values.values().filter_map(serde_json::Value::as_u64).sum())
        .unwrap_or_default()
}

pub fn has_standalone_approval_line(text: &str) -> bool {
    text.lines().any(|line| {
        let trimmed = line
            .trim()
            .trim_start_matches("- ")
            .trim_start_matches("* ")
            .trim()
            .to_ascii_lowercase();
        matches!(
            trimmed.as_str(),
            "lgtm"
                | "looks good"
                | "clean"
                | "solid"
                | "no issues found"
                | "no actionable findings"
                | "no actionable"
        )
    })
}
