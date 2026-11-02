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
    serve_with_open_prefix(responses, None)
}

fn serve_with_open_prefix(
    responses: Vec<(u16, String)>,
    open_prefix: Option<usize>,
) -> Result<Server> {
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
        for (index, (status, body)) in responses.into_iter().enumerate() {
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
            if open_prefix == Some(index) {
                // Keep EOF withheld until the client closes its oversized reader.
                // A read-to-EOF mutant instead waits for this eight-second fallback.
                socket.set_read_timeout(Some(Duration::from_secs(8)))?;
                let mut byte = [0];
                match socket.read(&mut byte) {
                    Ok(0) => {}
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::WouldBlock
                                | std::io::ErrorKind::TimedOut
                                | std::io::ErrorKind::ConnectionReset
                        ) => {}
                    Ok(_) => anyhow::bail!("unexpected data after fixture request"),
                    Err(err) => return Err(err.into()),
                }
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

fn regenerate_summary(out: &Path, receipt: &Value) -> Result<String> {
    fs::write(
        out.join("review/review.json"),
        serde_json::to_vec_pretty(receipt)?,
    )?;
    let output = Command::new(env!("CARGO_BIN_EXE_ub-review"))
        .env_clear()
        .args(["summary", "--run-dir"])
        .arg(out)
        .output()?;
    ensure!(
        output.status.success(),
        "summary: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    fs::read_to_string(out.join("running-summary.md")).context("read regenerated summary")
}

#[test]
fn summary_hydration_preserves_recorded_degraded_evidence_and_legacy_fallback() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let out = temp.path().join("summary-packet");
    hydrate(
        &root,
        &out,
        serve(vec![
            (200, "[]".to_owned()),
            (200, "[]".to_owned()),
            (200, "[]".to_owned()),
        ])?,
        8192,
    )?;
    let reason = "internal audit artifact write failed: fixture-failure";
    let lane = json!({"lane":"audit-fixture", "provider":"fixture", "model":"fixture-model", "endpoint_kind":"openai-chat", "status":"degraded", "reason":reason});
    let mut receipt = json!({"model_mode":"auto", "model_lanes":[lane.clone()], "missing_or_failed_model_evidence":[lane.clone()]});
    let summary = regenerate_summary(&out, &receipt)?;
    let missing = summary
        .split_once("## Missing or failed model evidence")
        .context("missing model evidence section")?
        .1;
    ensure!(
        missing.contains("- Lane `audit-fixture` via `fixture` model `fixture-model` endpoint `openai-chat`: `degraded` - internal audit artifact write failed: fixture-failure"),
        "hydration lost recorded failure: {missing}"
    );
    ensure!(!summary.contains("No planned model evidence is currently missing or failed."));

    // Persisted issue identity remains useful even without a lane receipt.
    receipt["model_lanes"] = json!([]);
    let standalone = regenerate_summary(&out, &receipt)?;
    ensure!(standalone.contains("fixture-failure"));
    ensure!(!standalone.contains("No planned model evidence is currently missing or failed."));

    // Explicit audit evidence and a different legacy status gap must coexist.
    let mut timed_out = lane.clone();
    timed_out["lane"] = json!("other-fixture");
    timed_out["status"] = json!("timed_out");
    timed_out["reason"] = json!("independent-timeout");
    receipt["model_lanes"] = json!([lane.clone(), timed_out]);
    let combined = regenerate_summary(&out, &receipt)?;
    let missing = combined
        .split_once("## Missing or failed model evidence")
        .context("missing model evidence section")?
        .1;
    ensure!(missing.contains("fixture-failure") && missing.contains("independent-timeout"));

    // A different recorded reason for the same lane is distinct evidence.
    let mut earlier_issue = lane.clone();
    earlier_issue["reason"] = json!("earlier-audit-failure");
    receipt["missing_or_failed_model_evidence"] = json!([lane.clone(), earlier_issue]);
    let history = regenerate_summary(&out, &receipt)?;
    let missing = history
        .split_once("## Missing or failed model evidence")
        .context("missing model evidence section")?
        .1;
    ensure!(missing.contains("fixture-failure") && missing.contains("earlier-audit-failure"));

    // Contentful degradation alone is still usable evidence, not a generic gap.
    let mut healthy = lane.clone();
    healthy["reason"] = json!("contentful lane output was preserved as degraded evidence");
    receipt["model_lanes"] = json!([healthy]);
    receipt["missing_or_failed_model_evidence"] = json!([]);
    let healthy_summary = regenerate_summary(&out, &receipt)?;
    ensure!(healthy_summary.contains("No planned model evidence is currently missing or failed."));
    ensure!(!healthy_summary.contains("fixture-failure"));

    // Old packets without the explicit array retain status-based reconstruction.
    let mut failed = lane;
    failed["status"] = json!("failed");
    failed["reason"] = json!("legacy-failure");
    receipt = json!({"model_mode":"auto", "model_lanes":[failed.clone()]});
    let legacy = regenerate_summary(&out, &receipt)?;
    ensure!(legacy.contains("legacy-failure"));
    ensure!(!legacy.contains("No planned model evidence is currently missing or failed."));

    // Explicit and reconstructed copies of the same issue render only once.
    receipt["missing_or_failed_model_evidence"] = json!([failed]);
    let deduplicated = regenerate_summary(&out, &receipt)?;
    let missing = deduplicated
        .split_once("## Missing or failed model evidence")
        .context("missing model evidence section")?
        .1;
    ensure!(
        missing.matches("legacy-failure").count() == 1,
        "duplicate issue after hydration: {missing}"
    );

    // The producer records preflight issues with this reserved lane identity.
    let preflight = json!({"provider":"fixture", "model":"fixture-model", "endpoint_kind":"openai-chat", "status":"failed", "reason":"preflight-failure"});
    let mut preflight_issue = preflight.clone();
    preflight_issue["lane"] = json!("provider-preflight");
    receipt = json!({"provider_preflights":[preflight], "missing_or_failed_model_evidence":[preflight_issue]});
    let preflight_summary = regenerate_summary(&out, &receipt)?;
    let missing = preflight_summary
        .split_once("## Missing or failed model evidence")
        .context("missing model evidence section")?
        .1;
    ensure!(
        missing.matches("preflight-failure").count() == 1,
        "duplicate preflight issue: {missing}"
    );
    ensure!(missing.contains("Provider preflight `fixture`"));
    Ok(())
}

#[test]
fn summary_hydration_excludes_only_legacy_disabled_lane_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let out = temp.path().join("summary-packet");
    hydrate(
        &root,
        &out,
        serve(vec![
            (200, "[]".to_owned()),
            (200, "[]".to_owned()),
            (200, "[]".to_owned()),
        ])?,
        8192,
    )?;

    let disabled = json!({"lane":"legacy-off", "provider":"fixture", "model":"fixture-model", "endpoint_kind":"openai-chat", "status":"skipped", "reason":"model-mode off"});
    let failed = json!({"lane":"real-failure", "provider":"fixture", "model":"fixture-model", "endpoint_kind":"openai-chat", "status":"failed", "reason":"explicit-failure"});
    let contradictory = json!({"lane":"failed-off", "provider":"fixture", "model":"fixture-model", "endpoint_kind":"openai-chat", "status":"failed", "reason":"model-mode off"});
    let budget = json!({"lane":"budget-gap", "provider":"fixture", "model":"fixture-model", "endpoint_kind":"openai-chat", "status":"skipped", "reason":"model call budget reached before lane execution"});
    let preflight = json!({"lane":"provider-preflight", "provider":"fixture", "model":"fixture-model", "endpoint_kind":"openai-chat", "status":"failed", "reason":"explicit-preflight-failure"});
    let ambiguous_preflight = json!({"lane":"provider-preflight", "provider":"other-fixture", "model":"fixture-model", "endpoint_kind":"openai-chat", "status":"skipped", "reason":"model-mode off"});
    let empty_lane = json!({"lane":"", "provider":"empty-lane-fixture", "model":"fixture-model", "endpoint_kind":"openai-chat", "status":"skipped", "reason":"model-mode off"});
    let mut receipt = json!({"model_mode":"off", "model_lanes":[disabled.clone()], "missing_or_failed_model_evidence":[disabled.clone(), failed, contradictory, budget, preflight, ambiguous_preflight, empty_lane]});
    let summary = regenerate_summary(&out, &receipt)?;
    ensure!(
        summary.contains("| `legacy-off` |"),
        "disabled lane diagnostics disappeared"
    );
    let missing = summary
        .split_once("## Missing or failed model evidence\n")
        .context("missing model evidence section")?
        .1
        .split("\n## ")
        .next()
        .context("model evidence section")?;
    ensure!(
        !missing.contains("Lane `legacy-off`"),
        "disabled lane was wrongly reported missing: {missing}"
    );
    ensure!(
        missing.contains("explicit-failure")
            && missing.contains("failed-off")
            && missing.contains("budget-gap")
            && missing.contains("explicit-preflight-failure")
            && missing.contains("other-fixture")
            && missing.contains("empty-lane-fixture"),
        "recorded failures disappeared: {missing}"
    );

    receipt["missing_or_failed_model_evidence"] = json!([disabled.clone()]);
    let off_summary = regenerate_summary(&out, &receipt)?;
    ensure!(off_summary.contains("No planned model evidence is currently missing or failed."));

    // Without explicit Off mode, preserve even anomalous recorded evidence.
    receipt["model_mode"] = json!("auto");
    let auto_summary = regenerate_summary(&out, &receipt)?;
    let auto_missing = auto_summary
        .split_once("## Missing or failed model evidence\n")
        .context("missing model evidence section")?
        .1;
    ensure!(
        auto_missing.contains("Lane `legacy-off`"),
        "Auto recorded issue disappeared"
    );
    receipt["model_mode"] = json!("");
    let unknown_summary = regenerate_summary(&out, &receipt)?;
    let unknown_missing = unknown_summary
        .split_once("## Missing or failed model evidence\n")
        .context("missing model evidence section")?
        .1;
    ensure!(
        unknown_missing.contains("Lane `legacy-off`"),
        "unknown-mode issue disappeared"
    );

    // Invalid explicit evidence must remain an incomplete receipt, never a clean result.
    receipt["model_mode"] = json!("off");
    receipt["missing_or_failed_model_evidence"] = json!([{"lane": 42}]);
    let invalid_summary = regenerate_summary(&out, &receipt)?;
    ensure!(
        invalid_summary.contains("Review evidence is incomplete")
            && !invalid_summary
                .contains("No planned model evidence is currently missing or failed."),
        "invalid evidence was hidden"
    );
    Ok(())
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
fn non_object_thread_items_fail_the_endpoint_without_fabricating_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let malformed =
        json!([{"id":202,"body":"Valid prefix must not hide malformed admitted item."}, null])
            .to_string();
    let (partial, requests) = hydrate(
        &root,
        &temp.path().join("mixed-item"),
        serve(vec![
            (200, response(101, "Original issue context.")),
            (200, malformed),
            (200, response(303, "Original inline context.")),
        ])?,
        8192,
    )?;
    ensure!(requests.len() == 3);
    ensure!(
        partial["threads"].as_array().context("threads")?.len() == 2,
        "malformed endpoint fabricated evidence: {partial}"
    );
    ensure!(partial["threads"][0]["id"] == "101" && partial["threads"][1]["id"] == "303");
    ensure!(partial["sources"].as_array().context("sources")?.len() == 2);
    let warning = partial["warnings"].to_string();
    ensure!(
        warning.contains("review-summaries")
            && warning.contains("item 2")
            && warning.contains("object")
            && warning.contains("retry"),
        "malformed item was not receipted: {warning}"
    );
    let (recovered, _) = hydrate(
        &root,
        &temp.path().join("items-recovered"),
        serve(vec![
            (200, response(101, "Original issue context.")),
            (200, response(202, "Valid review item restored.")),
            (200, response(303, "Original inline context.")),
        ])?,
        8192,
    )?;
    ensure!(recovered["threads"].as_array().context("threads")?.len() == 3);
    ensure!(
        recovered["threads"][0] == partial["threads"][0]
            && recovered["threads"][2] == partial["threads"][1]
    );
    ensure!(recovered["warnings"] == json!([]));
    Ok(())
}

#[test]
fn scalar_thread_arrays_are_unavailable_and_keep_failed_values_private() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let (context, requests) = hydrate(
        &root,
        &temp.path().join("scalar-items"),
        serve(vec![
            (200, "[null]".to_owned()),
            (200, json!(["PRIVATE_BAD_ITEM_SENTINEL"]).to_string()),
            (200, "[42]".to_owned()),
        ])?,
        8192,
    )?;
    ensure!(requests.len() == 3);
    ensure!(
        context["status"] == "unavailable",
        "malformed arrays became successful context: {context}"
    );
    ensure!(context["threads"] == json!([]) && context["sources"] == json!([]));
    let warnings = context["warnings"].as_array().context("warnings")?;
    ensure!(warnings.len() == 3);
    ensure!(
        warnings.iter().all(|warning| warning
            .as_str()
            .is_some_and(|warning| warning.contains("item 1")
                && warning.contains("object")
                && warning.contains("retry")))
    );
    ensure!(!context.to_string().contains("PRIVATE_BAD_ITEM_SENTINEL"));
    Ok(())
}

#[test]
fn overflow_closes_an_open_stream_promptly_and_recovers_later_endpoints() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let started = Instant::now();
    let (context, requests) = hydrate(
        &root,
        &temp.path().join("open-overflow"),
        serve_with_open_prefix(
            vec![
                (200, "x".repeat(1_048_577)),
                (200, response(202, "Review survived early stream cutoff.")),
                (200, response(303, "Inline survived early stream cutoff.")),
            ],
            Some(0),
        )?,
        8192,
    )?;
    ensure!(
        started.elapsed() < Duration::from_secs(5),
        "capture waited for producer EOF"
    );
    ensure!(requests.len() == 3);
    ensure!(context["threads"].as_array().context("threads")?.len() == 2);
    ensure!(context["threads"][0]["id"] == "202" && context["threads"][1]["id"] == "303");
    ensure!(context["warnings"].to_string().contains("1048576"));
    Ok(())
}

#[test]
fn exact_capture_limit_includes_status_trailer_and_rejects_one_extra_byte() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().join("repo");
    repository(&root)?;
    let mut body = response(101, "Exact bounded source record.");
    // Curl adds this trailer after the HTTP body; the capture budget includes it.
    let trailer = "\nUB_REVIEW_HTTP_STATUS:200\n";
    body.push_str(&" ".repeat(1_048_576 - trailer.len() - body.len()));
    let (accepted, _) = hydrate(
        &root,
        &temp.path().join("exact-cap"),
        serve(vec![
            (200, body.clone()),
            (200, "[]".to_owned()),
            (200, "[]".to_owned()),
        ])?,
        8192,
    )?;
    ensure!(accepted["threads"][0]["id"] == "101");
    ensure!(accepted["warnings"] == json!([]));
    body.push(' ');
    let (rejected, _) = hydrate(
        &root,
        &temp.path().join("one-over-cap"),
        serve(vec![
            (200, body),
            (200, response(202, "Healthy independent endpoint.")),
            (200, "[]".to_owned()),
        ])?,
        8192,
    )?;
    ensure!(rejected["threads"].as_array().context("threads")?.len() == 1);
    ensure!(rejected["threads"][0]["id"] == "202");
    ensure!(rejected["warnings"].to_string().contains("1048576"));
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
    let mut items: Vec<Value> = (1..=30)
        .map(|id| json!({"id": id, "body": "bounded item"}))
        .collect();
    items.push(Value::Null); // Omitted tail is outside the admitted first-page slice.
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
