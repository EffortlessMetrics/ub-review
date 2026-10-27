//! Diagnostic-only process adapter for #1557, shared by doctor and cache warm.
//!
//! Two seconds total, including a 500 ms cleanup reserve; 4 KiB retained per
//! stream, no spool files and no reader threads/tasks. Cancelling collection
//! drops both async pipe readers, including when an exited leader left a pipe
//! in a descendant. Only the direct child is owned; this is not tree containment.

use std::{io, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Child, time::Instant};

const TOTAL_BUDGET: Duration = Duration::from_secs(2);
const CLEANUP_RESERVE: Duration = Duration::from_millis(500);
const STREAM_LIMIT: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProbeState {
    Missing,
    SetupFailed,
    SpawnFailed,
    ReadFailed,
    WaitFailed,
    Nonzero,
    Timeout,
    OutputLimit,
    Unknown,
    Valid,
}

impl ProbeState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::SetupFailed => "setup-failed",
            Self::SpawnFailed => "spawn-failed",
            Self::ReadFailed => "read-failed",
            Self::WaitFailed => "wait-failed",
            Self::Nonzero => "nonzero",
            Self::Timeout => "timeout",
            Self::OutputLimit => "output-limit",
            Self::Unknown => "unknown",
            Self::Valid => "valid",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Cleanup {
    NotStarted,
    Reaped,
    CheckFailedReaped,
    KillFailed,
    ReapFailed,
    ReapTimeout,
}

impl Cleanup {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::NotStarted => "not-started",
            Self::Reaped => "reaped",
            Self::CheckFailedReaped => "check-failed-reaped",
            Self::KillFailed => "kill-failed",
            Self::ReapFailed => "reap-failed",
            Self::ReapTimeout => "reap-timeout",
        }
    }
}

pub(crate) struct VersionProbe {
    pub(crate) state: ProbeState,
    pub(crate) cleanup: Cleanup,
    pub(crate) version: Option<String>,
}

impl VersionProbe {
    fn unavailable(state: ProbeState, cleanup: Cleanup) -> Self {
        Self {
            state,
            cleanup,
            version: None,
        }
    }
}

/// Preserve the existing pin matcher and its ownership in #1067. For display
/// and cache identity, retain only numeric dotted tokens from the first line,
/// prefixed by the configured tool ID. Arbitrary prose, secrets and terminal /
/// workflow directives from a child never become diagnostics. This projection
/// is not a tool-specific version grammar or executable-identity attestation.
fn version_text(tool_id: &str, stdout: &[u8], stderr: &[u8]) -> Option<String> {
    let bytes = if stdout.is_empty() { stderr } else { stdout };
    let text = std::str::from_utf8(bytes).ok()?;
    let line = text.lines().map(str::trim).find(|line| !line.is_empty())?;
    let tokens: Vec<_> = line
        .split(|ch: char| ch.is_whitespace() || matches!(ch, ',' | ';' | '(' | ')'))
        .filter(|token| {
            let numeric = token.trim_start_matches('v');
            numeric.starts_with(|ch: char| ch.is_ascii_digit())
                && numeric.contains('.')
                && numeric.chars().all(|ch| ch.is_ascii_digit() || ch == '.')
        })
        .collect();
    if tokens.is_empty() {
        return None;
    }
    let version = format!("{tool_id} {}", tokens.join(" "));
    // Never publish a clipped identity as complete.
    (version.len() <= 160).then_some(version)
}

pub(crate) fn command_version(tool_id: &str, command: &str) -> VersionProbe {
    let deadline = Instant::now() + TOTAL_BUDGET;
    if crate::system_detect::command_path(command).is_none() {
        return VersionProbe::unavailable(ProbeState::Missing, Cleanup::NotStarted);
    }
    // This runtime belongs only to this synchronous diagnostic call. No general
    // scheduler or sensor/proof runner is changed and no background readers run.
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(_) => return VersionProbe::unavailable(ProbeState::SetupFailed, Cleanup::NotStarted),
    };
    runtime.block_on(probe(tool_id, command, deadline))
}

async fn capture(mut stream: impl tokio::io::AsyncRead + Unpin) -> Result<Vec<u8>, ProbeState> {
    let mut retained = Vec::with_capacity(STREAM_LIMIT);
    let mut chunk = [0; 1024];
    loop {
        let count = stream
            .read(&mut chunk)
            .await
            .map_err(|_| ProbeState::ReadFailed)?;
        if count == 0 {
            return Ok(retained);
        }
        // Check before extending: even a single hostile write never raises the
        // retained buffer above its ceiling. Fail closed on any excess bytes.
        if count > STREAM_LIMIT - retained.len() {
            return Err(ProbeState::OutputLimit);
        }
        retained.extend_from_slice(&chunk[..count]);
        tokio::task::yield_now().await;
    }
}

async fn probe(tool_id: &str, command: &str, deadline: Instant) -> VersionProbe {
    let collection_deadline = deadline - CLEANUP_RESERVE;
    if Instant::now() >= collection_deadline {
        return VersionProbe::unavailable(ProbeState::Timeout, Cleanup::NotStarted);
    }
    let mut child = match tokio::process::Command::new(command)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return VersionProbe::unavailable(ProbeState::SpawnFailed, Cleanup::NotStarted),
    };
    let collected = tokio::time::timeout_at(collection_deadline, async {
        let stdout = child.stdout.take().ok_or(ProbeState::SetupFailed)?;
        let stderr = child.stderr.take().ok_or(ProbeState::SetupFailed)?;
        tokio::try_join!(
            async { child.wait().await.map_err(|_| ProbeState::WaitFailed) },
            capture(stdout),
            capture(stderr),
        )
    })
    .await;
    let state = match collected {
        Ok(Ok((status, stdout, stderr))) => {
            if !status.success() {
                return VersionProbe::unavailable(ProbeState::Nonzero, Cleanup::Reaped);
            }
            let version = version_text(tool_id, &stdout, &stderr);
            return VersionProbe {
                state: if version.is_some() {
                    ProbeState::Valid
                } else {
                    ProbeState::Unknown
                },
                cleanup: Cleanup::Reaped,
                version,
            };
        }
        Ok(Err(state)) => state,
        Err(_) => ProbeState::Timeout,
    };
    // The collection future and its pipe readers have been dropped before
    // cleanup starts. No join or unbounded wait follows a timeout.
    let cleanup = cleanup_child(&mut child, deadline).await;
    VersionProbe::unavailable(state, cleanup)
}

// A narrow seam for deterministic kill/reap failure controls; no runner API.
trait CleanupProcess {
    fn try_reap(&mut self) -> io::Result<bool>;
    fn request_kill(&mut self) -> io::Result<()>;
    async fn reap(&mut self) -> io::Result<()>;
}

impl CleanupProcess for Child {
    fn try_reap(&mut self) -> io::Result<bool> {
        self.try_wait().map(|status| status.is_some())
    }
    fn request_kill(&mut self) -> io::Result<()> {
        self.start_kill()
    }
    async fn reap(&mut self) -> io::Result<()> {
        self.wait().await.map(|_| ())
    }
}

async fn cleanup_child(child: &mut impl CleanupProcess, deadline: Instant) -> Cleanup {
    let check_failed = match child.try_reap() {
        Ok(true) => return Cleanup::Reaped,
        Ok(false) => false,
        Err(_) => true,
    };
    if child.request_kill().is_err() {
        // Account for an exit racing kill, while retaining genuine kill failure.
        return match child.try_reap() {
            Ok(true) if check_failed => Cleanup::CheckFailedReaped,
            Ok(true) => Cleanup::Reaped,
            _ => Cleanup::KillFailed,
        };
    }
    // A failed nonblocking status check still gets a bounded reap attempt.
    match tokio::time::timeout_at(deadline, child.reap()).await {
        Ok(Ok(())) if check_failed => Cleanup::CheckFailedReaped,
        Ok(Ok(())) => Cleanup::Reaped,
        Ok(Err(_)) => Cleanup::ReapFailed,
        Err(_) => Cleanup::ReapTimeout,
    }
}

#[cfg(test)]
#[path = "version_probe_tests.rs"]
mod tests;
