//! Keep writable executable fixtures out of concurrently created children.
//!
//! CLOEXEC closes a descriptor at exec, not at fork/clone. Another test's
//! child can inherit a script's writer and keep its inode writable after
//! fs::write returns, making the fixture's next exec fail with ETXTBSY.
//! Readers allow concurrent process creation; only executable materialization
//! is exclusive. Release the reader before waiting for the child.

#[cfg(unix)]
use std::fs;
use std::io;
#[cfg(unix)]
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::RwLock;

static EXECUTABLE_FIXTURE_LAUNCH: RwLock<()> = RwLock::new(());

pub trait FixtureCommandExt {
    /// Every launch in a fixture test process must participate, including
    /// launches unrelated to the executable being written.
    fn fixture_spawn(&mut self) -> io::Result<Child>;
    /// Capture with null stdin and piped output. Fixture callers use these
    /// defaults; this deliberately does not support custom stdio configuration.
    fn fixture_output(&mut self) -> io::Result<Output>;
    fn fixture_status(&mut self) -> io::Result<ExitStatus>;
}

impl FixtureCommandExt for Command {
    fn fixture_spawn(&mut self) -> io::Result<Child> {
        let _guard = EXECUTABLE_FIXTURE_LAUNCH
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.spawn()
    }

    fn fixture_output(&mut self) -> io::Result<Output> {
        self.stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .fixture_spawn()?
            .wait_with_output()
    }

    fn fixture_status(&mut self) -> io::Result<ExitStatus> {
        self.fixture_spawn()?.wait()
    }
}

#[cfg(unix)]
pub fn write_executable_fixture(path: &Path, text: &str) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let _guard = EXECUTABLE_FIXTURE_LAUNCH
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    fs::write(path, text)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use anyhow::{Context, Result};
    use std::sync::{Arc, Barrier};
    use std::thread;

    #[test]
    fn concurrent_children_cannot_inherit_executable_fixture_writers() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let start = Arc::new(Barrier::new(9));
        // Larger than the doctor fixture to expose the descriptor window in
        // a bounded control, without retaining handles or delaying children.
        let script = format!(
            "#!/bin/sh\necho 'actionlint 1.7.0'\n#{}\n",
            "x".repeat(65536)
        );
        thread::scope(|scope| -> Result<()> {
            let mut readers = Vec::new();
            for _ in 0..8 {
                let start = Arc::clone(&start);
                let root = temp.path();
                readers.push(scope.spawn(move || -> Result<()> {
                    start.wait();
                    for _ in 0..250 {
                        let output = Command::new("/bin/true")
                            .current_dir(root)
                            .fixture_output()?;
                        assert!(output.status.success());
                    }
                    Ok(())
                }));
            }
            start.wait();
            for index in 0..250 {
                let path = temp.path().join(format!("actionlint-{index}"));
                write_executable_fixture(&path, &script)?;
                let output = Command::new(&path)
                    .current_dir(temp.path())
                    .fixture_output()?;
                assert!(output.status.success());
                assert_eq!(output.stdout, b"actionlint 1.7.0\n");
            }
            for reader in readers {
                reader
                    .join()
                    .map_err(|_| anyhow::anyhow!("launch worker panicked"))??;
            }
            Ok(())
        })
    }

    #[test]
    fn launch_errors_keep_io_semantics_and_child_lifetime_is_unlocked() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let path = temp.path().join("actionlint");
        write_executable_fixture(&path, "#!/bin/sh\nread value\necho \"$value\"\n")?;
        let mut child = Command::new(&path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .fixture_spawn()?;
        // A live child must not hold the spawn reader and block writers.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let unlocked = loop {
            if let Ok(writer) = EXECUTABLE_FIXTURE_LAUNCH.try_write() {
                drop(writer);
                break true;
            }
            if std::time::Instant::now() >= deadline {
                break false;
            }
            thread::yield_now();
        };
        use std::io::Write;
        child
            .stdin
            .take()
            .context("child stdin")?
            .write_all(b"done\n")?;
        let output = child.wait_with_output()?;
        assert!(
            unlocked,
            "fixture lock must end at spawn, before child wait"
        );
        assert!(output.status.success());
        assert_eq!(output.stdout, b"done\n");

        // A genuinely open writer must still fail immediately, never retry or
        // become a successful expected-nonzero result.
        let writer = fs::OpenOptions::new().write(true).open(&path)?;
        let direct = {
            // Even the native-output control must not inherit another test's
            // writer. This call fails at spawn because our own writer is open.
            let _guard = EXECUTABLE_FIXTURE_LAUNCH
                .read()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Command::new(&path)
                .output()
                .err()
                .context("direct ETXTBSY")?
        };
        let guarded = Command::new(&path)
            .fixture_output()
            .err()
            .context("guarded ETXTBSY")?;
        assert_eq!(direct.raw_os_error(), Some(26));
        assert_eq!(guarded.kind(), direct.kind());
        assert_eq!(guarded.raw_os_error(), direct.raw_os_error());
        for result in [
            super::super::run_capture_with_env(
                temp.path(),
                super::super::path_str(&path)?,
                &[],
                &[],
            ),
            super::super::run_expect_failure_with_env(
                temp.path(),
                super::super::path_str(&path)?,
                &[],
                &[],
            ),
        ] {
            let error = result
                .err()
                .context("real launch error must remain an error")?;
            let source = error
                .root_cause()
                .downcast_ref::<io::Error>()
                .context("original I/O source")?;
            assert_eq!(source.kind(), direct.kind());
            assert_eq!(source.raw_os_error(), direct.raw_os_error());
        }
        drop(writer);
        Ok(())
    }
}
