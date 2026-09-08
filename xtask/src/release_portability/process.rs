use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use serde::Serialize;
use wait_timeout::ChildExt;

const STREAM_LIMIT: u64 = 8 * 1024 * 1024;

pub(super) struct Capture {
    pub success: bool,
    pub code: Option<i32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl Capture {
    pub fn require_success(self, label: &str) -> Result<Self> {
        ensure!(
            self.success,
            "{label} failed with {:?}; inspect command logs",
            self.code
        );
        Ok(self)
    }

    pub fn text(&self) -> Result<&str> {
        Ok(std::str::from_utf8(&self.stdout)
            .context("command stdout is not UTF-8")?
            .trim())
    }
}

#[derive(Serialize)]
struct CommandReceipt<'a> {
    program: &'a str,
    arguments: &'a [String],
    exit_code: Option<i32>,
    timed_out: bool,
    stdout_bytes: usize,
    stderr_bytes: usize,
}

pub(super) struct Runner {
    logs: PathBuf,
    sequence: u64,
}

impl Runner {
    pub fn new(root: &Path) -> Result<Self> {
        let logs = root.join("commands");
        fs::create_dir(&logs).context("create fresh command log directory")?;
        Ok(Self { logs, sequence: 0 })
    }

    pub fn run(&mut self, program: &str, args: &[String]) -> Result<Capture> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .context("command sequence overflow")?;
        let prefix = self.logs.join(format!("{:04}", self.sequence));
        // File-backed logs do not require waiting for descendant-held pipe
        // handles. The timeout owns this CLI process; Docker container teardown
        // is owned separately by runtime::Container. This is not a general
        // process-tree or on-disk stream-size enforcement mechanism.
        let stdout_path = prefix.with_extension("stdout");
        let stderr_path = prefix.with_extension("stderr");
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(fs::File::create(&stdout_path)?)
            .stderr(fs::File::create(&stderr_path)?)
            .spawn()
            .with_context(|| format!("spawn {program}"))?;
        let read = |path: &Path| -> std::io::Result<Vec<u8>> {
            let mut bytes = Vec::new();
            fs::File::open(path)?
                .take(STREAM_LIMIT + 1)
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        };
        let completed = match child.wait_timeout(Duration::from_secs(180)) {
            Ok(completed) => completed,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("wait for proof command");
            }
        };
        let timed_out = completed.is_none();
        let status = match completed {
            Some(status) => status,
            None => {
                let _ = child.kill();
                child.wait().context("reap timed out proof command")?
            }
        };
        let stdout = read(&stdout_path)?;
        let stderr = read(&stderr_path)?;
        super::receipts::write_json(
            &prefix.with_extension("json"),
            &CommandReceipt {
                program,
                arguments: args,
                exit_code: status.code(),
                timed_out,
                stdout_bytes: stdout.len(),
                stderr_bytes: stderr.len(),
            },
        )?;
        ensure!(
            !timed_out,
            "{program} exceeded the 180-second command budget"
        );
        ensure!(
            u64::try_from(stdout.len())? <= STREAM_LIMIT
                && u64::try_from(stderr.len())? <= STREAM_LIMIT,
            "{program} exceeded the captured-stream budget"
        );
        Ok(Capture {
            success: status.success(),
            code: status.code(),
            stdout,
            stderr,
        })
    }

    pub fn checked(&mut self, program: &str, args: &[String]) -> Result<Capture> {
        self.run(program, args)?.require_success(program)
    }
}

pub(super) fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|arg| (*arg).to_owned()).collect()
}
