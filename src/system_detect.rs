//! System detection: profile config hash, git tree SHA, command
//! discovery, doctor binary install status, and system resource
//! detection (cleanup train step 55, pure code motion).

use crate::*;

pub(crate) fn profile_config_hash(config: &Config) -> Result<String> {
    Ok(sha256_hex(&serde_json::to_vec(config)?))
}

pub(crate) fn git_tree_sha(root: &Path, rev: &str) -> Result<String> {
    let output = ProcessCommand::new("git")
        .arg("rev-parse")
        .arg(format!("{rev}^{{tree}}"))
        .current_dir(root)
        .output()
        .with_context(|| format!("run git rev-parse for {rev}"))?;
    if !output.status.success() {
        bail!(
            "git rev-parse failed for {} in {}: {}",
            rev,
            root.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let tree = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if tree.is_empty() {
        bail!("git rev-parse returned an empty tree sha for {rev}");
    }
    Ok(tree)
}

pub(crate) fn command_version(command: &str) -> Option<String> {
    if !command_on_path(command) {
        return None;
    }
    let output = ProcessCommand::new(command)
        .arg("--version")
        .output()
        .ok()?;
    let text = if output.stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).to_string()
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(160).collect())
}

pub(crate) fn command_on_path(command: &str) -> bool {
    command_path(command).is_some()
}

pub(crate) fn command_path(command: &str) -> Option<PathBuf> {
    if command.contains('/') || command.contains('\\') {
        let path = PathBuf::from(command);
        return path.exists().then_some(path);
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).find_map(|dir| {
        let candidate = dir.join(command);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let pathext = std::env::var_os("PATHEXT")
                .map(|value| {
                    value
                        .to_string_lossy()
                        .split(';')
                        .map(ToOwned::to_owned)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_else(|| vec![".EXE".to_owned(), ".CMD".to_owned(), ".BAT".to_owned()]);
            pathext.iter().find_map(|ext| {
                let candidate = dir.join(format!("{command}{ext}"));
                candidate.is_file().then_some(candidate)
            })
        }
        #[cfg(not(windows))]
        {
            None
        }
    })
}

pub(crate) fn doctor_binary_install_status(current_exe: Option<&Path>) -> String {
    let path_binary = command_path("ub-review");
    doctor_binary_install_status_from_paths(current_exe, path_binary.as_deref())
}

pub(crate) fn doctor_binary_install_status_from_paths(
    current_exe: Option<&Path>,
    path_binary: Option<&Path>,
) -> String {
    match (current_exe, path_binary) {
        (Some(current), Some(path_binary)) if same_path(current, path_binary) => {
            format!("on PATH as {}", path_binary.display())
        }
        (Some(current), Some(path_binary)) => format!(
            "running {}; PATH resolves ub-review to {}",
            current.display(),
            path_binary.display()
        ),
        (Some(current), None) => {
            let dir = current
                .parent()
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| current.display().to_string());
            format!("not on PATH; add {dir} to PATH or use install-mode=path with binary-path={}", current.display())
        }
        (None, Some(path_binary)) => format!(
            "binary path unknown; PATH resolves ub-review to {}",
            path_binary.display()
        ),
        (None, None) => {
            "binary path unknown and ub-review is not on PATH; install ub-review or use install-mode=path with binary-path".to_owned()
        }
    }
}

pub(crate) fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

pub(crate) fn detect_mem_available_mb() -> Option<u64> {
    let text = fs::read_to_string("/proc/meminfo").ok()?;
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("MemAvailable:") {
            let kb = rest.split_whitespace().next()?.parse::<u64>().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

pub(crate) fn detect_load_1m() -> Option<f32> {
    let text = fs::read_to_string("/proc/loadavg").ok()?;
    text.split_whitespace().next()?.parse::<f32>().ok()
}

pub(crate) fn detect_disk_free_mb() -> Option<u64> {
    let output = ProcessCommand::new("df")
        .arg("-Pk")
        .arg(".")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8(output.stdout).ok()?;
    let line = stdout.lines().nth(1)?;
    let available_kb = line.split_whitespace().nth(3)?.parse::<u64>().ok()?;
    Some(available_kb / 1024)
}
