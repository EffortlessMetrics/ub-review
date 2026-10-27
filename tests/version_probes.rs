//! Real CLI controls owned by #1557; version pins/grammar remain with #1067.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    path::PathBuf,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
use std::path::Path;

struct Harness {
    temp: tempfile::TempDir,
    fixture: PathBuf,
}

impl Harness {
    fn new() -> Result<Self> {
        // Reap this harness's adopted held-pipe fixtures, rather than relying on
        // PID 1 in a container. This does not alter production process ownership.
        #[cfg(target_os = "linux")]
        rustix::process::set_child_subreaper(Some(rustix::process::getpid()))?;
        let temp = tempfile::tempdir()?;
        let fixture = temp
            .path()
            .join(format!("probe{}", std::env::consts::EXE_SUFFIX));
        let compiled = Command::new("rustc")
            .args(["--edition=2024", "tests/fixtures/version_probe.rs", "-o"])
            .arg(&fixture)
            .output()?;
        ensure!(
            compiled.status.success(),
            "fixture compilation failed: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        let h = Self { temp, fixture };
        let mut config = String::from("profile = \"gh-runner\"\n");
        for (tool, version) in [
            ("tokmd", "1.12.0"),
            ("cargo-allow", "0.1.8"),
            ("ripr", "0.10.0"),
            ("unsafe-review", "0.3.4"),
            ("ast-grep", "0.0.0"),
            ("actionlint", "1.7.12"),
        ] {
            let path = if tool == "tokmd" {
                h.fixture.clone()
            } else {
                let path = h
                    .temp
                    .path()
                    .join(format!("{tool}{}", std::env::consts::EXE_SUFFIX));
                fs::copy(&h.fixture, &path)?;
                fs::write(
                    format!("{}.state", path.display()),
                    format!("version:{tool} {version}"),
                )?;
                path
            };
            config.push_str(&format!(
                "\n[tools.{tool}]\ncommand = {:?}\n",
                path.to_string_lossy()
            ));
        }
        fs::write(h.temp.path().join("config.toml"), config)?;
        Ok(h)
    }

    fn state_path(&self, extension: &str) -> PathBuf {
        PathBuf::from(format!("{}.{}", self.fixture.display(), extension))
    }

    fn mode(&self, mode: &str) -> Result<()> {
        fs::write(self.state_path("state"), mode)?;
        Ok(())
    }

    fn invoke(&self, args: &[&str]) -> Result<(Output, Duration)> {
        let started = Instant::now();
        let out = self.temp.path().join("cli.out");
        let err = self.temp.path().join("cli.err");
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ub-review"));
        cmd.current_dir(self.temp.path()).env_clear();
        // Needed only for cache-warm's local git tree lookup and Windows process startup.
        for key in ["PATH", "SystemRoot", "SYSTEMROOT", "WINDIR", "TMP", "TEMP"] {
            if let Some(value) = std::env::var_os(key) {
                cmd.env(key, value);
            }
        }
        let mut child = cmd
            .args(args)
            .args(["--config", "config.toml"])
            .stdin(Stdio::null())
            .stdout(fs::File::create(&out)?)
            .stderr(fs::File::create(&err)?)
            .spawn()?;
        let deadline = started + Duration::from_secs(6);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                // Clean fixtures before killing doctor so baseline RED does not orphan a child.
                self.cleanup();
                child.kill()?;
                child.wait()?;
                anyhow::bail!("CLI exceeded harness deadline");
            }
            thread::sleep(Duration::from_millis(5));
        };
        Ok((
            Output {
                status,
                stdout: fs::read(out)?,
                stderr: fs::read(err)?,
            },
            started.elapsed(),
        ))
    }

    fn cleanup(&self) {
        for extension in ["descendant", "pid"] {
            if let Ok(text) = fs::read_to_string(self.state_path(extension))
                && let Ok(pid) = text.trim().parse::<u32>()
            {
                #[cfg(target_os = "linux")]
                if let Some(pid) = rustix::process::Pid::from_raw(pid as i32) {
                    // Avoid signalling a recycled PID after doctor reaped it.
                    if fs::read_link(format!("/proc/{}/exe", pid.as_raw_nonzero()))
                        .ok()
                        .as_deref()
                        == Some(self.fixture.as_path())
                    {
                        let _ = rustix::process::kill_process(pid, rustix::process::Signal::KILL);
                    }
                    let deadline = Instant::now() + Duration::from_secs(1);
                    loop {
                        match rustix::process::waitpid(
                            Some(pid),
                            rustix::process::WaitOptions::NOHANG,
                        ) {
                            Ok(None) if Instant::now() < deadline => {
                                thread::sleep(Duration::from_millis(5))
                            }
                            _ => break,
                        }
                    }
                }
                #[cfg(all(unix, not(target_os = "linux")))]
                let _ = Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
                #[cfg(windows)]
                let _ = Command::new("taskkill")
                    .args(["/F", "/PID", &pid.to_string()])
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.cleanup();
    }
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn doctor_probe_states_and_recovery() -> Result<()> {
    let h = Harness::new()?;
    for (mode, expected) in [
        ("stdout", "valid"),
        ("stderr", "valid"),
        ("nonzero", "nonzero"),
        ("blank", "unknown"),
        ("malformed", "unknown"),
        ("oversized", "output-limit"),
        ("oversized-stdout", "output-limit"),
        ("oversized-stderr", "output-limit"),
        ("sleep", "timeout"),
        ("stdout", "valid"),
    ] {
        h.mode(mode)?;
        let (output, elapsed) = h.invoke(&["doctor"])?;
        let diagnostic = text(&output);
        ensure!(
            output.status.success(),
            "advisory doctor failed: {diagnostic}"
        );
        ensure!(
            elapsed < Duration::from_secs(4),
            "probe exceeded total bound: {elapsed:?}"
        );
        ensure!(
            diagnostic.contains("Providers:") && diagnostic.contains("Fixes:"),
            "remaining diagnostics absent"
        );
        let row = diagnostic
            .lines()
            .find(|line| line.trim_start().starts_with("tokmd "))
            .context("missing tool row")?;
        ensure!(
            row.contains(&format!("probe={expected}")),
            "{mode}: wrong probe state: {row}"
        );
        ensure!(
            !diagnostic.contains("SECRET_SENTINEL")
                && !diagnostic.contains("\x1b")
                && !diagnostic.contains("::error::"),
            "raw diagnostic leaked"
        );
        let (required, _) = h.invoke(&["doctor", "--require-core-tools"])?;
        ensure!(
            required.status.success() == (expected == "valid"),
            "{mode}: required result incorrect: {}",
            text(&required)
        );
        if mode == "sleep" {
            ensure!(
                row.contains("cleanup=reaped"),
                "direct child not reaped: {row}"
            );
            #[cfg(target_os = "linux")]
            {
                let pid = fs::read_to_string(h.state_path("pid"))?;
                ensure!(
                    !Path::new(&format!("/proc/{}", pid.trim())).exists(),
                    "direct child remains"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn doctor_distinguishes_missing_and_spawn_failure() -> Result<()> {
    let h = Harness::new()?;
    h.mode("stdout")?;
    fs::remove_file(&h.fixture)?;
    let (out, _) = h.invoke(&["doctor"])?;
    ensure!(text(&out).contains("probe=missing"));
    fs::write(&h.fixture, "not executable")?;
    let (out, _) = h.invoke(&["doctor"])?;
    ensure!(text(&out).contains("probe=spawn-failed"));
    Ok(())
}

#[test]
fn required_doctor_rejects_failed_probe() -> Result<()> {
    let h = Harness::new()?;
    h.mode("nonzero")?;
    let (output, _) = h.invoke(&["doctor", "--require-core-tools"])?;
    let diagnostic = text(&output);
    ensure!(!output.status.success());
    ensure!(diagnostic.contains("probe=nonzero") && diagnostic.contains("version unknown:"));
    ensure!(!diagnostic.contains("version=tokmd 1.12.0"));
    Ok(())
}

#[test]
fn cache_warm_keeps_failed_versions_unavailable() -> Result<()> {
    let h = Harness::new()?;
    let git = Command::new("git")
        .args(["-C"])
        .arg(h.temp.path())
        .args(["init", "--quiet"])
        .status()?;
    ensure!(git.success());
    // Empty tree object, requiring no commit or identity.
    let git = Command::new("git")
        .arg("-C")
        .arg(h.temp.path())
        .args(["hash-object", "-t", "tree", "--stdin", "-w"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let tree = git.wait_with_output()?;
    ensure!(tree.status.success());
    let tree = String::from_utf8(tree.stdout)?;
    for (mode, expected) in [
        ("nonzero", "nonzero"),
        ("blank", "unknown"),
        ("malformed", "unknown"),
        ("stderr", "valid"),
        ("oversized", "output-limit"),
        ("oversized-stdout", "output-limit"),
        ("oversized-stderr", "output-limit"),
        ("sleep", "timeout"),
        ("stdout", "valid"),
    ] {
        h.mode(mode)?;
        let (output, elapsed) = h.invoke(&[
            "cache",
            "warm",
            "--root",
            ".",
            "--base",
            tree.trim(),
            "--out",
            "cache",
        ])?;
        ensure!(
            output.status.success(),
            "cache warm failed: {}",
            text(&output)
        );
        ensure!(elapsed < Duration::from_secs(4));
        let manifest: serde_json::Value = serde_json::from_slice(&fs::read(
            h.temp.path().join("cache/rules/tokmd/manifest.json"),
        )?)?;
        ensure!(manifest["probe_status"] == expected, "{mode}: {manifest}");
        if expected == "valid" {
            ensure!(manifest["version"] == "tokmd 1.12.0");
        } else {
            ensure!(
                manifest["version"].is_null(),
                "failure cached as a version: {manifest}"
            );
        }
    }
    Ok(())
}

#[test]
fn doctor_sleep_deadline_reaps_direct_child() -> Result<()> {
    let h = Harness::new()?;
    h.mode("sleep")?;
    let (output, elapsed) = h.invoke(&["doctor"])?;
    ensure!(elapsed < Duration::from_secs(4));
    ensure!(text(&output).contains("probe=timeout cleanup=reaped"));
    #[cfg(target_os = "linux")]
    {
        let pid = fs::read_to_string(h.state_path("pid"))?;
        ensure!(
            !Path::new(&format!("/proc/{}", pid.trim())).exists(),
            "direct child still exists"
        );
    }
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn exited_leader_with_held_pipe_is_bounded_for_both_callers() -> Result<()> {
    let h = Harness::new()?;
    ensure!(
        Command::new("git")
            .arg("-C")
            .arg(h.temp.path())
            .args(["init", "--quiet"])
            .status()?
            .success()
    );
    let git = Command::new("git")
        .arg("-C")
        .arg(h.temp.path())
        .args(["hash-object", "-t", "tree", "--stdin", "-w"])
        .stdin(Stdio::null())
        .output()?;
    ensure!(git.status.success());
    let tree = String::from_utf8(git.stdout)?;
    for cache in [false, true] {
        h.mode("held")?;
        let args = if cache {
            vec![
                "cache",
                "warm",
                "--root",
                ".",
                "--base",
                tree.trim(),
                "--out",
                "cache",
            ]
        } else {
            vec!["doctor"]
        };
        let (output, elapsed) = h.invoke(&args)?;
        ensure!(output.status.success());
        ensure!(elapsed < Duration::from_secs(4));
        if cache {
            let manifest: serde_json::Value = serde_json::from_slice(&fs::read(
                h.temp.path().join("cache/rules/tokmd/manifest.json"),
            )?)?;
            ensure!(
                manifest["version"].is_null()
                    && manifest["probe_status"] == "timeout"
                    && manifest["probe_cleanup"] == "reaped",
                "{manifest}"
            );
        } else {
            ensure!(
                text(&output).contains("probe=timeout cleanup=reaped"),
                "{}",
                text(&output)
            );
        }
        let leader = fs::read_to_string(h.state_path("pid"))?;
        let descendant = fs::read_to_string(h.state_path("descendant"))?;
        ensure!(
            !Path::new(&format!("/proc/{}", leader.trim())).exists(),
            "leader not reaped"
        );
        ensure!(
            Path::new(&format!("/proc/{}", descendant.trim())).exists(),
            "test did not hold an inherited pipe"
        );
        // The product owns only the direct child. The subreaper harness owns
        // and kills/reaps the descendant on success and through Drop on errors.
        h.cleanup();
        ensure!(
            !Path::new(&format!("/proc/{}", descendant.trim())).exists(),
            "fixture descendant not reaped"
        );
        h.mode("stdout")?;
        let (recovered, _) = h.invoke(&["doctor", "--require-core-tools"])?;
        ensure!(recovered.status.success() && text(&recovered).contains("probe=valid"));
    }
    Ok(())
}
