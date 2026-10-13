use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::process::Command;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU32, Ordering},
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

struct Server {
    url: String,
    stop: Arc<AtomicBool>,
    caller: Arc<AtomicU32>,
    handle: thread::JoinHandle<Result<(Vec<String>, Vec<u32>)>>,
}

fn serve(responses: Vec<(u16, String)>) -> Result<Server> {
    let listener = TcpListener::bind("127.0.0.1:0")?;
    listener.set_nonblocking(true)?;
    let url = format!("http://{}", listener.local_addr()?);
    let stop = Arc::new(AtomicBool::new(false));
    let shutdown = Arc::clone(&stop);
    let caller = Arc::new(AtomicU32::new(0));
    let process = Arc::clone(&caller);
    let handle = thread::spawn(move || {
        let mut requests = Vec::new();
        let mut children = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(45);
        for (status, body) in responses {
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if shutdown.load(Ordering::Relaxed) || Instant::now() > deadline {
                            return Ok((requests, children));
                        }
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(err) => return Err(err.into()),
                }
            };
            socket.set_read_timeout(Some(Duration::from_secs(2)))?;
            socket.set_write_timeout(Some(Duration::from_secs(2)))?;
            let mut request = Vec::new();
            for byte in std::io::BufReader::new((&mut socket).take(8192)).bytes() {
                request.push(byte?);
                if request.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let request = String::from_utf8(request)?;
            requests.push(request.lines().next().unwrap_or_default().to_owned());
            #[cfg(target_os = "linux")]
            {
                let pid = process.load(Ordering::Relaxed);
                for entry in fs::read_dir("/proc")? {
                    let entry = entry?;
                    let Ok(child) = entry.file_name().to_string_lossy().parse::<u32>() else {
                        continue;
                    };
                    let Ok(status) = fs::read_to_string(entry.path().join("status")) else {
                        continue;
                    };
                    if status
                        .lines()
                        .find_map(|line| line.strip_prefix("PPid:"))
                        .and_then(|parent| parent.trim().parse::<u32>().ok())
                        == Some(pid)
                    {
                        children.push(child);
                    }
                }
            }
            #[cfg(not(target_os = "linux"))]
            let _ = &process;
            if status == 408 {
                thread::sleep(Duration::from_secs(31));
            }
            // Deliberately omit Content-Length: the client must bound streamed capture.
            let header = format!(
                "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n"
            );
            match socket
                .write_all(header.as_bytes())
                .and_then(|()| socket.write_all(body.as_bytes()))
            {
                Ok(()) => {}
                Err(err)
                    if matches!(
                        err.kind(),
                        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset
                    ) => {}
                Err(err) => return Err(err.into()),
            }
        }
        Ok((requests, children))
    });
    Ok(Server {
        url,
        stop,
        caller,
        handle,
    })
}

fn git(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(root).output()?;
    ensure!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn repository(root: &Path) -> Result<String> {
    fs::create_dir_all(root.join("src"))?;
    // No Cargo manifest or executable project configuration is needed to hydrate source.
    fs::write(root.join("src/lib.rs"), "pub fn answer() -> u8 { 41 }\n")?;
    git(root, &["init", "-q"])?;
    git(root, &["config", "user.email", "fixture@example.invalid"])?;
    git(root, &["config", "user.name", "Context Fixture"])?;
    git(root, &["add", "."])?;
    git(root, &["commit", "-qm", "base"])?;
    fs::write(root.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n")?;
    git(root, &["commit", "-qam", "head"])?;
    git(root, &["rev-parse", "HEAD"])
}

fn hydrate(root: &Path, out: &Path, server: Server, budget: usize) -> Result<(Value, Vec<String>)> {
    let head = git(root, &["rev-parse", "HEAD"])?;
    let config = out.with_extension("toml");
    fs::write(
        &config,
        "profile = \"gh-runner\"\n[repo]\nkind = \"rust\"\n",
    )?;
    let child = Command::new(env!("CARGO_BIN_EXE_ub-review"))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .args([
            "run",
            "--dry-run",
            "--model-mode",
            "off",
            "--posting",
            "artifact-only",
            "--no-github-summary",
            "--root",
        ])
        .arg(root)
        .arg("--config")
        .arg(&config)
        .arg("--out")
        .arg(out)
        .args([
            "--base",
            "HEAD~1",
            "--head",
            &head,
            "--github-repo",
            "fixture/source",
            "--github-pull-number",
            "7",
            "--github-token",
            "fixture-only-no-authority",
            "--github-api-url",
        ])
        .arg(&server.url)
        .arg("--pr-thread-context-max-bytes")
        .arg(budget.to_string())
        .current_dir(root)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    server.caller.store(child.id(), Ordering::Relaxed);
    let output = child.wait_with_output()?;
    server.stop.store(true, Ordering::Relaxed);
    let (requests, children) = server
        .handle
        .join()
        .map_err(|_| anyhow::anyhow!("fixture server panicked"))??;
    #[cfg(target_os = "linux")]
    {
        ensure!(
            !children.is_empty(),
            "no live acquisition child was observed"
        );
        for child in children {
            ensure!(
                !Path::new(&format!("/proc/{child}")).exists(),
                "acquisition child {child} survived or was not reaped"
            );
        }
    }
    #[cfg(not(target_os = "linux"))]
    let _ = children;
    ensure!(
        output.status.success(),
        "CLI: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let context: Value =
        serde_json::from_slice(&fs::read(out.join("review/pr_thread_context.json"))?)?;
    ensure!(!context.to_string().contains("fixture-only-no-authority"));
    let terminal: Value =
        serde_json::from_slice(&fs::read(out.join("review/terminal_state.json"))?)?;
    ensure!(
        terminal["model_mode"] == "off" && terminal["usable_model_lanes"] == 0,
        "model-off must not create model success: {terminal}"
    );
    ensure!(
        fs::read_to_string(out.join("input/diff.patch"))?.contains("+pub fn answer() -> u8 { 42 }")
    );
    Ok((context, requests))
}

fn response(id: u64, body: &str) -> String {
    json!([{"id": id, "body": body, "user": {"login": "maintainer"}}]).to_string()
}

#[test]
fn partial_failure_preserves_source_records_and_recovers_on_next_run() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    let head = repository(&root)?;
    let inline = json!([{"id": 303, "body": "Check the 42 return value.", "path": "src/lib.rs", "line": 1, "commit_id": head, "user": {"login": "reviewer"}}]).to_string();
    let (partial, requests) = hydrate(
        &root,
        &temp.path().join("partial"),
        serve(vec![
            (200, response(101, "Issue receipt remains useful.")),
            (503, "private-error-body-must-not-be-receipted".to_owned()),
            (200, inline.clone()),
        ])?,
        8192,
    )?;
    ensure!(
        partial["threads"][0]["id"] == "101",
        "successful issue context was discarded: {partial}"
    );
    ensure!(
        requests.len() == 3,
        "independent endpoint was not attempted: {requests:?}"
    );
    ensure!(partial["threads"][1]["id"] == "303");
    ensure!(partial["threads"][1]["head_binding"] == "current");
    ensure!(partial["sources"].as_array().context("sources")?.len() == 2);
    ensure!(partial["status"] == "seeded");
    let warnings = partial["warnings"].to_string();
    ensure!(
        warnings.contains("review-summaries")
            && warnings.contains("503")
            && warnings.contains("retry"),
        "missing failure/recovery truth: {warnings}"
    );
    ensure!(!partial.to_string().contains("private-error-body"));
    let (recovered, requests) = hydrate(
        &root,
        &temp.path().join("recovered"),
        serve(vec![
            (200, response(101, "Issue receipt remains useful.")),
            (200, response(202, "Review endpoint recovered.")),
            (200, inline),
        ])?,
        8192,
    )?;
    ensure!(requests.len() == 3);
    ensure!(recovered["threads"].as_array().context("threads")?.len() == 3);
    ensure!(recovered["threads"][0] == partial["threads"][0]);
    ensure!(recovered["threads"][2] == partial["threads"][1]);
    ensure!(!recovered["warnings"].to_string().contains("unavailable"));
    ensure!(
        recovered["thread_context"]
            .as_str()
            .context("thread context")?
            .contains("first page")
    );
    Ok(())
}

#[test]
fn api_text_truncation_is_receipted_without_rewriting_original_records() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let body = "é".repeat(1000);
    let (context, _) = hydrate(
        &root,
        &temp.path().join("short"),
        serve(vec![
            (200, response(101, &body)),
            (200, "[]".to_owned()),
            (200, "[]".to_owned()),
        ])?,
        8192,
    )?;
    ensure!(
        context["thread_context_truncated"] == true,
        "API truncation flag was lost: {context}"
    );
    ensure!(context["threads"][0]["body"] == body);
    ensure!(context["threads"][0]["head_binding"] == "unbound");
    ensure!(
        context["thread_context"]
            .as_str()
            .context("thread context")?
            .len()
            < body.len()
    );
    Ok(())
}

#[test]
fn oversized_and_malformed_endpoints_are_unavailable_with_bounded_diagnostics() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let oversized = response(101, &"x".repeat(2 * 1024 * 1024));
    let (context, requests) = hydrate(
        &root,
        &temp.path().join("failed"),
        serve(vec![
            (200, oversized),
            (200, "{\"message\":\"wrong shape\"}".to_owned()),
            (200, "not JSON".to_owned()),
        ])?,
        8192,
    )?;
    ensure!(requests.len() == 3);
    ensure!(
        context["status"] == "unavailable",
        "failed hydration was called seeded: {context}"
    );
    ensure!(context["threads"] == json!([]));
    ensure!(context["sources"] == json!([]));
    let warnings = context["warnings"].to_string();
    ensure!(
        warnings.contains("1048576") && warnings.contains("array") && warnings.contains("parse"),
        "missing bounded failure causes: {warnings}"
    );
    ensure!(
        context.to_string().len() < 4096,
        "failure receipt retained response bytes"
    );
    Ok(())
}

#[test]
fn permission_denial_stops_further_reads_and_keeps_prior_context() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let (context, requests) = hydrate(
        &root,
        &temp.path().join("denied"),
        serve(vec![
            (200, response(101, "Retained prior context.")),
            (403, "denied fixture".to_owned()),
            (200, response(303, "Must not be fetched.")),
        ])?,
        8192,
    )?;
    ensure!(
        requests.len() == 2,
        "access denial did not stop: {requests:?}"
    );
    ensure!(context["threads"].as_array().context("threads")?.len() == 1);
    ensure!(context["threads"][0]["id"] == "101");
    ensure!(
        context["warnings"].to_string().contains("403")
            && context["warnings"].to_string().contains("stopped")
    );
    Ok(())
}

#[test]
fn oversized_denial_body_cannot_hide_access_status_or_resume_reads() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    for status in [401, 403] {
        let (context, requests) = hydrate(
            &root,
            &temp.path().join(format!("denied-{status}")),
            serve(vec![
                (200, response(101, "Retained prior context.")),
                (status, "private-denial-body".repeat(120_000)),
                (200, response(303, "Must not be fetched.")),
            ])?,
            8192,
        )?;
        ensure!(
            requests.len() == 2,
            "oversized denial resumed API reads: {requests:?}"
        );
        ensure!(context["threads"].as_array().context("threads")?.len() == 1);
        ensure!(context["threads"][0]["id"] == "101");
        ensure!(
            context["warnings"]
                .to_string()
                .contains(&status.to_string())
                && context["warnings"].to_string().contains("stopped")
        );
        ensure!(!context.to_string().contains("private-denial-body"));
    }
    Ok(())
}

#[test]
fn first_page_and_zero_text_budget_keep_explicit_omissions() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let items: Vec<Value> = (1..=31)
        .map(|id| json!({"id": id, "body": "bounded item"}))
        .collect();
    let (context, requests) = hydrate(
        &root,
        &temp.path().join("page"),
        serve(vec![
            (200, serde_json::to_string(&items)?),
            (200, "[]".to_owned()),
            (200, "[]".to_owned()),
        ])?,
        0,
    )?;
    ensure!(requests.len() == 3);
    ensure!(context["threads"].as_array().context("threads")?.len() == 30);
    ensure!(context["threads"][29]["id"] == "30");
    ensure!(context["thread_context_truncated"] == true);
    ensure!(context["thread_context"] == "\n[truncated]\n");
    ensure!(
        context["warnings"]
            .to_string()
            .contains("first-page limit 30")
    );
    Ok(())
}

#[test]
fn stalled_endpoint_hits_the_real_deadline_then_next_endpoints_recover() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let started = Instant::now();
    let (context, requests) = hydrate(
        &root,
        &temp.path().join("timeout"),
        serve(vec![
            (408, "[]".to_owned()),
            (200, response(202, "Review remains available.")),
            (200, response(303, "Inline remains available.")),
        ])?,
        8192,
    )?;
    ensure!(
        started.elapsed() < Duration::from_secs(40),
        "read did not finish in its production bound"
    );
    ensure!(requests.len() == 3);
    ensure!(context["threads"][0]["id"] == "202" && context["threads"][1]["id"] == "303");
    let warnings = context["warnings"].to_string();
    ensure!(
        warnings.contains("Some(28)") && warnings.contains("retry"),
        "timeout cause/recovery missing: {warnings}"
    );
    Ok(())
}
