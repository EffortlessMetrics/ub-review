use super::*;
use anyhow::{Result, ensure};

#[test]
fn display_projection_never_retains_child_prose_or_controls() {
    for text in [
        b"".as_slice(),
        b"\n \n",
        b"SECRET_SENTINEL\x1b[31m::error::injected",
        &[0xff],
    ] {
        assert_eq!(version_text("tokmd", text, &[]), None);
    }
    assert_eq!(
        version_text(
            "tokmd",
            b"tokmd 1.12.0 SECRET_SENTINEL\x1b[31m::error::injected",
            &[]
        ),
        Some("tokmd 1.12.0".to_owned())
    );
    assert_eq!(
        version_text("actionlint", &[], b"v1.7.12\nbuilt with go1.24"),
        Some("actionlint v1.7.12".to_owned())
    );
    // Keep #1067's existing comparator behavior, including its known grammar
    // limits: presentation filtering must not silently fix numeric pin choice.
    let projected = version_text("ripr", b"ripr 0.9.9 (compatible with 0.10.0)", &[]);
    assert!(
        projected
            .as_deref()
            .is_some_and(|text| crate::command_version_matches(text, "0.10.0"))
    );
}

#[test]
fn display_projection_rejects_non_numeric_dotted_tokens_on_both_streams() {
    // These each violate a different part of the existing projection contract.
    // None changes the separately owned tool-specific pin grammar in #1067.
    for token in ["123", ".1", "1.secret"] {
        assert_eq!(version_text("tokmd", token.as_bytes(), &[]), None);
        assert_eq!(version_text("tokmd", &[], token.as_bytes()), None);
    }
    for token in ["1.2", "v1.2"] {
        let expected = format!("tokmd {token}");
        assert_eq!(
            version_text("tokmd", token.as_bytes(), &[]).as_deref(),
            Some(expected.as_str())
        );
        assert_eq!(
            version_text("tokmd", &[], token.as_bytes()).as_deref(),
            Some(expected.as_str())
        );
    }
}

#[test]
fn stream_ceiling_is_enforced_during_capture() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    // Use the public byte ceiling, independently of the production constant.
    for count in [0, 4096, 4097, 4 * 1024 * 1024] {
        // A generated reader retains no full oversized input, independently of
        // the production capture buffer under test.
        let result = runtime.block_on(capture(tokio::io::repeat(b'x').take(count as u64)));
        if count <= 4096 {
            ensure!(result.as_ref().is_ok_and(|bytes| bytes.len() == count));
        } else {
            ensure!(result == Err(ProbeState::OutputLimit));
        }
    }
    Ok(())
}

struct FaultChild {
    checks: std::collections::VecDeque<io::Result<bool>>,
    kill_failed: bool,
    reap_failed: bool,
    reap_pending: bool,
    killed: bool,
    waited: bool,
}

impl CleanupProcess for FaultChild {
    fn try_reap(&mut self) -> io::Result<bool> {
        self.checks.pop_front().unwrap_or(Ok(false))
    }
    fn request_kill(&mut self) -> io::Result<()> {
        self.killed = true;
        if self.kill_failed {
            Err(io::Error::other("synthetic kill failure"))
        } else {
            Ok(())
        }
    }
    async fn reap(&mut self) -> io::Result<()> {
        self.waited = true;
        if self.reap_pending {
            std::future::pending::<()>().await;
        }
        if self.reap_failed {
            Err(io::Error::other("synthetic reap failure"))
        } else {
            Ok(())
        }
    }
}

#[test]
fn cleanup_failures_remain_explicit_and_all_waits_are_bounded() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    for (checks, kill_failed, reap_failed, reap_pending, expected) in [
        (vec![Ok(true)], false, false, false, Cleanup::Reaped),
        (vec![Ok(false)], false, false, false, Cleanup::Reaped),
        (
            vec![Err(io::Error::other("synthetic check failure"))],
            false,
            false,
            false,
            Cleanup::CheckFailedReaped,
        ),
        (
            vec![Ok(false), Ok(false)],
            true,
            false,
            false,
            Cleanup::KillFailed,
        ),
        (
            vec![Ok(false), Ok(true)],
            true,
            false,
            false,
            Cleanup::Reaped,
        ),
        (
            vec![Err(io::Error::other("synthetic check failure")), Ok(true)],
            true,
            false,
            false,
            Cleanup::CheckFailedReaped,
        ),
        (vec![Ok(false)], false, true, false, Cleanup::ReapFailed),
        (vec![Ok(false)], false, false, true, Cleanup::ReapTimeout),
    ] {
        let already_reaped = checks
            .first()
            .is_some_and(|check| matches!(check, Ok(true)));
        let mut child = FaultChild {
            checks: checks.into(),
            kill_failed,
            reap_failed,
            reap_pending,
            killed: false,
            waited: false,
        };
        let result = runtime.block_on(cleanup_child(
            &mut child,
            Instant::now() + Duration::from_millis(10),
        ));
        ensure!(result == expected, "cleanup misclassified: {result:?}");
        ensure!(child.killed != already_reaped);
        ensure!(child.waited == (!already_reaped && !kill_failed));
    }
    Ok(())
}

struct ReadFailure;
impl tokio::io::AsyncRead for ReadFailure {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        _: &mut std::task::Context<'_>,
        _: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<io::Result<()>> {
        std::task::Poll::Ready(Err(io::Error::other("synthetic reader failure")))
    }
}

#[test]
fn read_failure_is_not_unknown_or_valid_output() -> Result<()> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    ensure!(runtime.block_on(capture(ReadFailure)) == Err(ProbeState::ReadFailed));
    Ok(())
}

#[test]
fn probe_state_labels_preserve_the_diagnostic_wire_contract() {
    let labels = [
        ProbeState::Missing,
        ProbeState::SetupFailed,
        ProbeState::SpawnFailed,
        ProbeState::ReadFailed,
        ProbeState::WaitFailed,
        ProbeState::Nonzero,
        ProbeState::Timeout,
        ProbeState::OutputLimit,
        ProbeState::Unknown,
        ProbeState::Valid,
    ]
    .map(ProbeState::label);
    assert_eq!(
        labels,
        [
            "missing",
            "setup-failed",
            "spawn-failed",
            "read-failed",
            "wait-failed",
            "nonzero",
            "timeout",
            "output-limit",
            "unknown",
            "valid",
        ]
    );
}

#[test]
fn cleanup_labels_preserve_the_diagnostic_wire_contract() {
    let labels = [
        Cleanup::NotStarted,
        Cleanup::Reaped,
        Cleanup::CheckFailedReaped,
        Cleanup::KillFailed,
        Cleanup::ReapFailed,
        Cleanup::ReapTimeout,
    ]
    .map(Cleanup::label);
    assert_eq!(
        labels,
        [
            "not-started",
            "reaped",
            "check-failed-reaped",
            "kill-failed",
            "reap-failed",
            "reap-timeout",
        ]
    );
}

#[test]
fn complete_version_identity_respects_its_byte_limit_on_both_streams() {
    for expected_bytes in [159, 160, 161] {
        // A numeric token stays below the stream ceiling but straddles the
        // complete identity limit. Prose or stream-overflow rejection cannot
        // accidentally satisfy the over-limit control.
        let token = format!("1.{}", "2".repeat(expected_bytes - "tokmd 1.".len()));
        let expected = format!("tokmd {token}");
        assert_eq!(expected.len(), expected_bytes);
        assert!(token.len() < STREAM_LIMIT);
        for stderr_only in [false, true] {
            let observed = if stderr_only {
                version_text("tokmd", &[], token.as_bytes())
            } else {
                version_text("tokmd", token.as_bytes(), &[])
            };
            if expected_bytes <= 160 {
                assert_eq!(observed.as_deref(), Some(expected.as_str()));
            } else {
                assert_eq!(observed, None, "over-limit identity must not be clipped");
            }
        }
    }
}
