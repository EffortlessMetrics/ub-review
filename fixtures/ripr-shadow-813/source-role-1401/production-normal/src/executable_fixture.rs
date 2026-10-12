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
