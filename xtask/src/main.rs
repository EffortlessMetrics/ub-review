use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};

use anyhow::{Context, Result, bail};
use serde_json::{Value as JsonValue, json};
use toml::Value;
use toml::map::Map;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = env::args().skip(1);
    let command = args.next().unwrap_or_else(|| "help".to_owned());
    let root = env::current_dir().context("resolve current directory")?;

    match command.as_str() {
        "policy-check" => {
            reject_extra_args(args)?;
            let report = check_policy(&root)?;
            println!("{}", report.summary());
        }
        "policy-inventory" => {
            reject_extra_args(args)?;
            let report = check_policy(&root)?;
            print!("{}", report.inventory());
        }
        "audit" => {
            reject_extra_args(args)?;
            run_cargo_audit(&root)?;
        }
        "precommit" => {
            let options = PrecommitOptions::parse(args)?;
            let report = run_precommit(&root, options)?;
            print!("{}", report.summary_md);
            if report.blocking_failures > 0 {
                bail!(
                    "precommit failed with {} blocking finding(s); see {}",
                    report.blocking_failures,
                    report.out_dir.display()
                );
            }
        }
        "help" | "-h" | "--help" => {
            reject_extra_args(args)?;
            print_help();
        }
        other => {
            bail!(
                "unknown xtask command `{other}`; expected policy-check, policy-inventory, audit, precommit, or help"
            )
        }
    }

    Ok(())
}

fn reject_extra_args(mut args: impl Iterator<Item = String>) -> Result<()> {
    if let Some(extra) = args.next() {
        bail!("unexpected argument `{extra}`");
    }
    Ok(())
}

fn print_help() {
    println!(
        "\
cargo xtask commands

  cargo xtask policy-check      parse and validate repo policy receipts
  cargo xtask policy-inventory  print receipt and CI policy counts
  cargo xtask audit             run cargo-audit for RUSTSEC advisories (advisory)
  cargo xtask precommit         run diff-scoped Rust precommit checks

precommit options

  --staged                      inspect only staged changes
"
    );
}

#[derive(Clone, Copy, Debug, Default)]
struct PrecommitOptions {
    staged: bool,
}

impl PrecommitOptions {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self> {
        let mut options = Self::default();
        for arg in args {
            match arg.as_str() {
                "--staged" => options.staged = true,
                other => bail!("unexpected precommit argument `{other}`"),
            }
        }
        Ok(options)
    }
}

/// Run `cargo audit` to check the dependency tree for RUSTSEC advisories.
/// Advisory (non-blocking) by default: the function reports findings but does
/// not bail on them — the repo's cost-discipline doctrine treats supply-chain
/// advisories as a monitored signal, not a merge gate. A missing `cargo-audit`
/// install is a notice, not an error, so the xtask stays runnable without it.
/// See issue #621 / tracker UB-40.
fn run_cargo_audit(root: &Path) -> Result<()> {
    let cargo = env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let result = Command::new(&cargo)
        .arg("audit")
        .arg("--locked")
        .current_dir(root)
        .output();
    let output = match result {
        Ok(output) => output,
        Err(error) => {
            eprintln!(
                "notice: could not invoke `cargo audit` ({error}); \
                 install with `cargo install cargo-audit` to enable RUSTSEC monitoring"
            );
            return Ok(());
        }
    };
    // cargo-audit exits 0 if clean, non-zero if vulnerabilities found. We
    // surface the output either way but do not propagate the failure — the
    // caller decides whether to act. Print stdout/stderr verbatim so the
    // advisory detail is visible.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stdout.is_empty() {
        print!("{stdout}");
    }
    if !stderr.is_empty() {
        eprint!("{stderr}");
    }
    // Detect "command not found" style errors (older cargo emits to stderr).
    let combined = format!("{stdout}{stderr}");
    if combined.contains("no such command: `audit`")
        || combined.contains("no such command: `cargo-audit`")
        || combined.contains("is not installed")
    {
        eprintln!(
            "notice: `cargo audit` is not installed; \
             run `cargo install cargo-audit` to enable RUSTSEC monitoring"
        );
        return Ok(());
    }
    if !output.status.success() {
        eprintln!(
            "warning: cargo audit reported vulnerabilities (exit {:?}); \
             review the output above and update affected dependencies",
            output.status.code()
        );
    } else {
        println!("cargo audit: no advisories found");
    }
    Ok(())
}

#[derive(Debug)]
struct PrecommitReport {
    out_dir: PathBuf,
    summary_md: String,
    blocking_failures: usize,
}

#[derive(Debug)]
struct CommandReceipt {
    name: String,
    command: String,
    status: Option<i32>,
    success: bool,
    skipped: bool,
    /// A tool that should have run but is not installed (#320): distinct
    /// from a relevance skip, never `success: true`, carries an install
    /// hint in `reason`. Stays non-blocking so missing optional tooling
    /// does not fail an unrelated precommit, but it can never read as a
    /// clean pass again.
    missing: bool,
    reason: Option<String>,
    stdout: String,
    stderr: String,
    /// Captured streams are bounded (#317); true when output was clipped.
    stdout_truncated: bool,
    stderr_truncated: bool,
}

#[derive(Clone, Debug)]
struct ChangedFile {
    path: String,
    lines: BTreeSet<u64>,
}

#[derive(Clone, Debug)]
struct WorkspacePackage {
    name: String,
    manifest_dir: PathBuf,
    targets: Vec<WorkspaceTarget>,
}

#[derive(Clone, Debug)]
struct WorkspaceTarget {
    name: String,
    kind: Vec<String>,
    src_path: PathBuf,
}

#[derive(Debug)]
struct ClippyDiagnostic {
    package: String,
    path: String,
    line: u64,
    level: String,
    message: String,
}

fn run_precommit(root: &Path, options: PrecommitOptions) -> Result<PrecommitReport> {
    let out_dir = prepare_precommit_out_dir(root)?;

    let changed = changed_files(root, options.staged)?;
    let workspace = workspace_packages(root)?;
    let affected = affected_packages(root, &workspace, &changed)?;
    write_affected_packages(&out_dir, &affected, &changed)?;
    let diff_path = write_diff_artifact(root, &out_dir, options.staged)?;
    let diff_arg = diff_path.display().to_string();

    let mut receipts = Vec::new();
    let mut blocking_failures = 0;

    let mut fmt = run_capture(root, "cargo", &["fmt", "--all", "--", "--check"])?;
    fmt.name = "fmt".to_owned();
    write_command_artifact(&out_dir.join("fmt.md"), "fmt", &fmt)?;
    if !fmt.success {
        blocking_failures += 1;
    }
    receipts.push(fmt);

    if affected.is_empty() {
        let check = skipped_receipt("cargo check", "no affected Rust workspace packages");
        write_markdown(&out_dir.join("check.md"), &receipt_markdown(&check))?;
        receipts.push(check);

        let clippy = skipped_receipt("clippy", "no affected Rust workspace packages");
        fs::write(out_dir.join("clippy.json"), "[]\n")?;
        write_markdown(
            &out_dir.join("clippy-on-diff.md"),
            &receipt_markdown(&clippy),
        )?;
        receipts.push(clippy);
    } else {
        for package in &affected {
            let package_arg = format!("-p={}", package.name);
            let mut check = run_capture(root, "cargo", &["check", &package_arg, "--locked"])?;
            check.name = format!("cargo check {}", package.name);
            write_command_artifact(
                &out_dir.join(format!("check-{}.md", safe_name(&package.name))),
                &format!("cargo check {}", package.name),
                &check,
            )?;
            if !check.success {
                blocking_failures += 1;
            }
            receipts.push(check);
        }

        let (clippy_receipts, clippy_findings) =
            run_clippy_on_diff(root, &out_dir, &affected, &changed)?;
        if !clippy_findings.is_empty() {
            blocking_failures += clippy_findings.len();
        }
        receipts.extend(clippy_receipts);
    }

    let cargo_allow_receipt = out_dir.join("cargo-allow.receipt.json");
    let cargo_allow_receipt_arg = cargo_allow_receipt.display().to_string();
    let cargo_allow_output = out_dir.join("cargo-allow.md");
    let cargo_allow_output_arg = cargo_allow_output.display().to_string();
    let cargo_allow = run_relevant_tool(
        root,
        &out_dir.join("cargo-allow.json"),
        "cargo-allow",
        &[
            "cargo-allow",
            "check",
            // Point cargo-allow at the repo's native 0.1 ledger explicitly;
            // its default discovery would pick up `policy/allow.toml`, which
            // is the xtask-owned repo-policy ledger in a different dialect.
            // No `--mode`: the ledger's default_mode governs.
            // https://github.com/EffortlessMetrics/cargo-allow/issues/1465
            "--config",
            "policy/cargo-allow.toml",
            "--format",
            "markdown",
            "--receipt",
            cargo_allow_receipt_arg.as_str(),
            "--output",
            cargo_allow_output_arg.as_str(),
        ],
        relevant_cargo_allow(&changed),
        "no changed source exception surfaces",
    )?;
    if cargo_allow.success_is_blocking_failure() {
        blocking_failures += 1;
    }
    receipts.push(cargo_allow);

    let ripr = run_relevant_tool(
        root,
        &out_dir.join("ripr.md"),
        "ripr",
        &[
            "ripr",
            "check",
            "--diff",
            diff_arg.as_str(),
            "--mode",
            "draft",
            "--format",
            "json",
        ],
        relevant_rust_change(&changed),
        "no changed Rust behavior surface",
    )?;
    receipts.push(ripr);

    let unsafe_review = run_relevant_tool(
        root,
        &out_dir.join("unsafe-review.md"),
        "unsafe-review",
        &[
            "unsafe-review",
            "check",
            "--root",
            ".",
            "--diff",
            diff_arg.as_str(),
            "--format",
            "markdown",
            "--policy",
            "advisory",
        ],
        relevant_unsafe_or_native(&changed),
        "no changed unsafe/native surface",
    )?;
    if unsafe_review.success_is_blocking_failure() {
        blocking_failures += 1;
    }
    receipts.push(unsafe_review);

    let actionlint = run_relevant_tool(
        root,
        &out_dir.join("actionlint.md"),
        "actionlint",
        &["actionlint"],
        relevant_workflow(&changed),
        "no changed workflow files",
    )?;
    if actionlint.success_is_blocking_failure() {
        blocking_failures += 1;
    }
    receipts.push(actionlint);

    let ast_grep_config = root.join("tools/ub-rules/sgconfig.yml");
    let ast_grep_config_arg = ast_grep_config.display().to_string();
    let ast_grep_argv = if ast_grep_config.exists() {
        vec![
            "ast-grep",
            "scan",
            "--config",
            ast_grep_config_arg.as_str(),
            ".",
        ]
    } else {
        vec!["ast-grep", "scan"]
    };
    let ast_grep = run_relevant_tool(
        root,
        &out_dir.join("ast-grep.md"),
        "ast-grep",
        &ast_grep_argv,
        relevant_rust_change(&changed),
        "no changed Rust files",
    )?;
    receipts.push(ast_grep);

    let summary =
        render_precommit_summary(options, &changed, &affected, &receipts, blocking_failures);
    write_markdown(&out_dir.join("summary.md"), &summary)?;

    Ok(PrecommitReport {
        out_dir,
        summary_md: summary,
        blocking_failures,
    })
}

impl CommandReceipt {
    fn success_is_blocking_failure(&self) -> bool {
        !self.skipped && !self.success
    }
}

fn skipped_receipt(name: &str, reason: &str) -> CommandReceipt {
    CommandReceipt {
        name: name.to_owned(),
        command: String::new(),
        status: None,
        success: true,
        skipped: true,
        missing: false,
        reason: Some(reason.to_owned()),
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
    }
}

/// How to install each tool precommit knows about (#321). One table shared
/// by the missing-tool receipts; versions track scripts/install-gh-runner-tools.sh.
fn install_hint(name: &str) -> &'static str {
    match name {
        "tokmd" => "cargo install tokmd --locked --version 1.12.0 --force",
        "cargo-allow" => "cargo install cargo-allow --locked",
        "ripr" => "cargo install ripr --locked --version 0.8.0 --force",
        "unsafe-review" => "cargo install unsafe-review --locked --version 0.3.4 --force",
        "ast-grep" => "npm install -g @ast-grep/cli",
        "actionlint" => {
            "go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.12; add $(go env GOPATH)/bin to PATH"
        }
        _ => "see scripts/install-gh-runner-tools.sh",
    }
}

/// A required-but-absent tool (#320): skipped (it cannot run, and missing
/// optional tooling must not block unrelated work) but never `success` -
/// the receipt is distinguishable from a relevance skip and says how to
/// install (#321).
fn missing_receipt(name: &str) -> CommandReceipt {
    CommandReceipt {
        name: name.to_owned(),
        command: String::new(),
        status: None,
        success: false,
        skipped: true,
        missing: true,
        reason: Some(format!(
            "{name} not installed; install: {}",
            install_hint(name)
        )),
        stdout: String::new(),
        stderr: String::new(),
        stdout_truncated: false,
        stderr_truncated: false,
    }
}

fn changed_files(root: &Path, staged: bool) -> Result<Vec<ChangedFile>> {
    let mut args = if staged {
        vec!["diff", "--cached", "--name-only", "--diff-filter=ACMRTUXB"]
    } else {
        vec!["diff", "HEAD", "--name-only", "--diff-filter=ACMRTUXB"]
    };
    let output = command_output(root, "git", &args)?;
    if !output.status.success() {
        bail!(
            "git changed-file detection failed: {}",
            output.stderr.trim()
        );
    }

    let mut files = BTreeMap::new();
    for line in output.stdout.lines() {
        let path = line.trim();
        if !path.is_empty() {
            files.insert(path.to_owned(), BTreeSet::new());
        }
    }

    if !staged {
        args = vec!["ls-files", "--others", "--exclude-standard"];
        let untracked = command_output(root, "git", &args)?;
        if untracked.status.success() {
            for line in untracked.stdout.lines() {
                let path = line.trim();
                if !path.is_empty() {
                    files.insert(path.to_owned(), all_file_lines(root, path)?);
                }
            }
        }
    }

    for (path, lines) in diff_changed_lines(root, staged)? {
        files.entry(path).or_default().extend(lines);
    }

    Ok(files
        .into_iter()
        .map(|(path, lines)| ChangedFile { path, lines })
        .collect())
}

fn diff_changed_lines(root: &Path, staged: bool) -> Result<BTreeMap<String, BTreeSet<u64>>> {
    let args = if staged {
        vec!["diff", "--cached", "--unified=0"]
    } else {
        vec!["diff", "HEAD", "--unified=0"]
    };
    let output = command_output(root, "git", &args)?;
    if !output.status.success() {
        bail!(
            "git changed-line detection failed: {}",
            output.stderr.trim()
        );
    }

    let mut lines_by_file: BTreeMap<String, BTreeSet<u64>> = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in output.stdout.lines() {
        if let Some(rest) = line.strip_prefix("+++ b/") {
            current = Some(rest.to_owned());
        } else if line.starts_with("@@")
            && let Some(path) = current.as_ref()
            && let Some((start, count)) = parse_hunk_added_range(line)
        {
            let entry = lines_by_file.entry(path.clone()).or_default();
            for offset in 0..count {
                entry.insert(start + offset);
            }
        }
    }
    Ok(lines_by_file)
}

fn parse_hunk_added_range(line: &str) -> Option<(u64, u64)> {
    for part in line.split_whitespace() {
        if let Some(range) = part.strip_prefix('+') {
            let mut pieces = range.split(',');
            let start = pieces.next()?.parse().ok()?;
            let count = match pieces.next() {
                Some(value) => value.parse().ok()?,
                None => 1,
            };
            return Some((start, count));
        }
    }
    None
}

fn all_file_lines(root: &Path, relative: &str) -> Result<BTreeSet<u64>> {
    let text = fs::read_to_string(root.join(relative))
        .with_context(|| format!("read changed file {relative}"))?;
    let mut lines = BTreeSet::new();
    for (index, _) in text.lines().enumerate() {
        let number = u64::try_from(index).context("line number overflow")? + 1;
        lines.insert(number);
    }
    Ok(lines)
}

fn prepare_precommit_out_dir(root: &Path) -> Result<PathBuf> {
    let out_dir = root.join("target/precommit");
    if out_dir.exists() {
        fs::remove_dir_all(&out_dir)
            .with_context(|| format!("remove stale {}", out_dir.display()))?;
    }
    fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    Ok(out_dir)
}

fn workspace_packages(root: &Path) -> Result<Vec<WorkspacePackage>> {
    let output = command_output(
        root,
        "cargo",
        &["metadata", "--format-version=1", "--no-deps"],
    )?;
    if !output.status.success() {
        bail!("cargo metadata failed: {}", output.stderr.trim());
    }
    let metadata: JsonValue =
        serde_json::from_str(&output.stdout).context("parse cargo metadata")?;
    let packages = metadata
        .get("packages")
        .and_then(JsonValue::as_array)
        .context("cargo metadata missing packages")?;

    let mut parsed = Vec::new();
    for package in packages {
        let name = package
            .get("name")
            .and_then(JsonValue::as_str)
            .context("cargo metadata package missing name")?
            .to_owned();
        let manifest_path = package
            .get("manifest_path")
            .and_then(JsonValue::as_str)
            .context("cargo metadata package missing manifest_path")?;
        let manifest_dir = PathBuf::from(manifest_path)
            .parent()
            .context("manifest path missing parent")?
            .to_path_buf();
        let targets = package
            .get("targets")
            .and_then(JsonValue::as_array)
            .context("cargo metadata package missing targets")?
            .iter()
            .map(parse_workspace_target)
            .collect::<Result<Vec<_>>>()?;
        parsed.push(WorkspacePackage {
            name,
            manifest_dir,
            targets,
        });
    }
    parsed.sort_by(|left, right| left.name.cmp(&right.name));
    let canonical_root = root
        .canonicalize()
        .context("canonicalize repository root")?;
    for package in &mut parsed {
        let manifest_dir = if package.manifest_dir.is_relative() {
            canonical_root.join(&package.manifest_dir)
        } else {
            package.manifest_dir.clone()
        };
        package.manifest_dir = manifest_dir
            .canonicalize()
            .with_context(|| format!("canonicalize package {}", package.name))?;
    }
    Ok(parsed)
}

fn parse_workspace_target(target: &JsonValue) -> Result<WorkspaceTarget> {
    let name = target
        .get("name")
        .and_then(JsonValue::as_str)
        .context("cargo metadata target missing name")?
        .to_owned();
    let src_path = PathBuf::from(
        target
            .get("src_path")
            .and_then(JsonValue::as_str)
            .context("cargo metadata target missing src_path")?,
    );
    let kind = target
        .get("kind")
        .and_then(JsonValue::as_array)
        .context("cargo metadata target missing kind")?
        .iter()
        .filter_map(JsonValue::as_str)
        .map(str::to_owned)
        .collect();
    Ok(WorkspaceTarget {
        name,
        kind,
        src_path,
    })
}

fn affected_packages(
    root: &Path,
    packages: &[WorkspacePackage],
    changed: &[ChangedFile],
) -> Result<Vec<WorkspacePackage>> {
    let canonical_root = root
        .canonicalize()
        .context("canonicalize repository root")?;
    let mut affected = BTreeSet::new();
    for file in changed {
        let normalized = normalize_path(&file.path);
        if normalized == "Cargo.lock" || normalized == "Cargo.toml" {
            affected.extend(packages.iter().map(|package| package.name.clone()));
            continue;
        }

        let absolute = repo_absolute_path(&canonical_root, &normalized);
        if normalized.ends_with("Cargo.toml") {
            if let Some(package) = packages
                .iter()
                .find(|package| absolute == package.manifest_dir.join("Cargo.toml"))
            {
                affected.insert(package.name.clone());
            }
            continue;
        }

        if normalized.ends_with(".rs")
            && let Some(package) = nearest_package_for_path(packages, &absolute)
        {
            affected.insert(package.name.clone());
        }
    }
    Ok(packages
        .iter()
        .filter(|package| affected.contains(&package.name))
        .cloned()
        .collect())
}

fn write_affected_packages(
    out_dir: &Path,
    affected: &[WorkspacePackage],
    changed: &[ChangedFile],
) -> Result<()> {
    let changed_json = changed
        .iter()
        .map(|file| {
            json!({
                "path": file.path,
                "lines": file.lines.iter().copied().collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    let packages_json = affected
        .iter()
        .map(|package| {
            json!({
                "name": package.name,
                "manifest_dir": package.manifest_dir,
                "targets": package.targets.iter().map(|target| json!({
                    "name": target.name,
                    "kind": target.kind,
                    "src_path": target.src_path,
                })).collect::<Vec<_>>()
            })
        })
        .collect::<Vec<_>>();
    let value = json!({
        "changed_files": changed_json,
        "affected_packages": packages_json,
    });
    fs::write(
        out_dir.join("affected-packages.json"),
        serde_json::to_string_pretty(&value).context("serialize affected packages")? + "\n",
    )?;
    Ok(())
}

fn write_diff_artifact(root: &Path, out_dir: &Path, staged: bool) -> Result<PathBuf> {
    let args = if staged {
        vec!["diff", "--cached", "--unified=3"]
    } else {
        vec!["diff", "HEAD", "--unified=3"]
    };
    let output = command_output(root, "git", &args)?;
    if !output.status.success() {
        bail!("git diff artifact failed: {}", output.stderr.trim());
    }
    let path = out_dir.join(if staged {
        "staged.diff"
    } else {
        "working-tree.diff"
    });
    fs::write(&path, output.stdout).with_context(|| format!("write {}", path.display()))?;
    Ok(path)
}

fn run_clippy_on_diff(
    root: &Path,
    out_dir: &Path,
    affected: &[WorkspacePackage],
    changed: &[ChangedFile],
) -> Result<(Vec<CommandReceipt>, Vec<ClippyDiagnostic>)> {
    let changed_map = changed
        .iter()
        .map(|file| (normalize_repo_path(root, &file.path), file.lines.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut receipts = Vec::new();
    let mut all_messages = Vec::new();
    let mut findings = Vec::new();

    for package in affected {
        let package_arg = format!("-p={}", package.name);
        let mut receipt = run_capture(
            root,
            "cargo",
            &[
                "clippy",
                &package_arg,
                "--all-targets",
                "--locked",
                "--message-format=json",
            ],
        )?;
        receipt.name = format!("cargo clippy {}", package.name);
        for line in receipt
            .stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
        {
            match serde_json::from_str::<JsonValue>(line) {
                Ok(value) => {
                    collect_clippy_finding(
                        root,
                        &package.name,
                        &value,
                        &changed_map,
                        &mut findings,
                    );
                    all_messages.push(value);
                }
                Err(_) => all_messages.push(json!({ "text": line })),
            }
        }
        write_command_artifact(
            &out_dir.join(format!("clippy-{}.md", safe_name(&package.name))),
            &format!("cargo clippy {}", package.name),
            &receipt,
        )?;
        receipts.push(receipt);
    }

    fs::write(
        out_dir.join("clippy.json"),
        serde_json::to_string_pretty(&all_messages).context("serialize clippy json")? + "\n",
    )?;
    write_markdown(
        &out_dir.join("clippy-on-diff.md"),
        &render_clippy_on_diff(&findings),
    )?;
    Ok((receipts, findings))
}

fn collect_clippy_finding(
    root: &Path,
    package: &str,
    value: &JsonValue,
    changed: &BTreeMap<String, BTreeSet<u64>>,
    findings: &mut Vec<ClippyDiagnostic>,
) {
    if value.get("reason").and_then(JsonValue::as_str) != Some("compiler-message") {
        return;
    }
    let Some(message) = value.get("message") else {
        return;
    };
    let level = message
        .get("level")
        .and_then(JsonValue::as_str)
        .unwrap_or("unknown");
    if !matches!(level, "warning" | "error") {
        return;
    }
    let text = message
        .get("message")
        .and_then(JsonValue::as_str)
        .unwrap_or("");
    let Some(spans) = message.get("spans").and_then(JsonValue::as_array) else {
        return;
    };
    for span in spans {
        if span.get("is_primary").and_then(JsonValue::as_bool) != Some(true) {
            continue;
        }
        let Some(path) = span.get("file_name").and_then(JsonValue::as_str) else {
            continue;
        };
        let normalized = normalize_repo_path(root, path);
        let line = span
            .get("line_start")
            .and_then(JsonValue::as_u64)
            .unwrap_or(0);
        if line == 0 {
            continue;
        }
        if changed
            .get(&normalized)
            .is_some_and(|lines| lines.contains(&line))
        {
            findings.push(ClippyDiagnostic {
                package: package.to_owned(),
                path: normalized,
                line,
                level: level.to_owned(),
                message: text.to_owned(),
            });
        }
    }
}

fn render_clippy_on_diff(findings: &[ClippyDiagnostic]) -> String {
    let mut text = String::new();
    text.push_str("# Clippy on diff\n\n");
    text.push_str(
        "Clippy ran at affected package/target granularity. This receipt gates only diagnostics whose primary span touches changed files and changed lines.\n\n",
    );
    if findings.is_empty() {
        text.push_str("No Clippy diagnostics touched changed lines.\n");
    } else {
        text.push_str("## Blocking diagnostics\n\n");
        for finding in findings {
            text.push_str(&format!(
                "- {}:{} [{}] {} ({})\n",
                finding.path, finding.line, finding.level, finding.message, finding.package
            ));
        }
    }
    text
}

fn run_relevant_tool(
    root: &Path,
    artifact: &Path,
    name: &str,
    argv: &[&str],
    relevant: bool,
    skip_reason: &str,
) -> Result<CommandReceipt> {
    if !relevant {
        let receipt = skipped_receipt(name, skip_reason);
        write_tool_artifact(artifact, &receipt, "")?;
        return Ok(receipt);
    }
    if !command_available(root, argv[0])? {
        let receipt = missing_receipt(name);
        write_tool_artifact(artifact, &receipt, "")?;
        return Ok(receipt);
    }
    let (program, args) = argv.split_first().context("tool argv must not be empty")?;
    let receipt = run_capture(root, program, args)?;
    write_tool_artifact(artifact, &receipt, &format_command(program, args))?;
    Ok(receipt)
}

fn relevant_cargo_allow(changed: &[ChangedFile]) -> bool {
    changed.iter().any(|file| {
        file.path == "policy/allow.toml"
            || file.path == "policy/cargo-allow.toml"
            || file.path.ends_with(".rs")
            || file.path.ends_with("Cargo.toml")
    })
}

fn relevant_rust_change(changed: &[ChangedFile]) -> bool {
    changed.iter().any(|file| {
        file.path.ends_with(".rs") || file.path.ends_with("Cargo.toml") || file.path == "Cargo.lock"
    })
}

fn relevant_unsafe_or_native(changed: &[ChangedFile]) -> bool {
    changed.iter().any(|file| {
        file.path.ends_with(".rs")
            || file.path.ends_with("build.rs")
            || file.path.ends_with(".c")
            || file.path.ends_with(".cc")
            || file.path.ends_with(".cpp")
            || file.path.ends_with(".h")
            || file.path.ends_with(".hpp")
    })
}

fn relevant_workflow(changed: &[ChangedFile]) -> bool {
    changed.iter().any(|file| {
        file.path.starts_with(".github/workflows/")
            && (file.path.ends_with(".yml") || file.path.ends_with(".yaml"))
    })
}

fn command_available(root: &Path, program: &str) -> Result<bool> {
    let output = Command::new(program)
        .arg("--version")
        .current_dir(root)
        .output();
    match output {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).with_context(|| format!("probe {program}")),
    }
}

#[derive(Debug)]
struct CapturedOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

fn command_output(root: &Path, program: &str, args: &[&str]) -> Result<CapturedOutput> {
    let output = Command::new(program)
        .args(args)
        .current_dir(root)
        .output()
        .with_context(|| format!("run {}", format_command(program, args)))?;
    Ok(CapturedOutput {
        status: output.status,
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// Per-stream capture budget (#317): head plus tail with an elision marker,
/// so one loud tool cannot turn a receipt into a 450 MB markdown file.
const CAPTURE_HEAD_BYTES: usize = 64 * 1024;
const CAPTURE_TAIL_BYTES: usize = 16 * 1024;

fn clip_capture(text: String) -> (String, bool) {
    let budget = CAPTURE_HEAD_BYTES + CAPTURE_TAIL_BYTES;
    if text.len() <= budget {
        return (text, false);
    }
    let mut head_end = CAPTURE_HEAD_BYTES;
    while !text.is_char_boundary(head_end) {
        head_end -= 1;
    }
    let mut tail_start = text.len() - CAPTURE_TAIL_BYTES;
    while !text.is_char_boundary(tail_start) {
        tail_start += 1;
    }
    let elided = tail_start - head_end;
    let marker = format!(
        "
[... {elided} bytes truncated by the precommit capture budget ...]
"
    );
    (
        format!("{}{marker}{}", &text[..head_end], &text[tail_start..]),
        true,
    )
}

fn run_capture(root: &Path, program: &str, args: &[&str]) -> Result<CommandReceipt> {
    let output = command_output(root, program, args)?;
    let (stdout, stdout_truncated) = clip_capture(output.stdout);
    let (stderr, stderr_truncated) = clip_capture(output.stderr);
    Ok(CommandReceipt {
        name: program.to_owned(),
        command: format_command(program, args),
        status: output.status.code(),
        success: output.status.success(),
        skipped: false,
        missing: false,
        reason: None,
        stdout,
        stderr,
        stdout_truncated,
        stderr_truncated,
    })
}

fn write_command_artifact(path: &Path, title: &str, receipt: &CommandReceipt) -> Result<()> {
    let mut text = String::new();
    text.push_str(&format!("# {title}\n\n"));
    text.push_str(&receipt_markdown(receipt));
    write_markdown(path, &text)
}

fn write_tool_artifact(path: &Path, receipt: &CommandReceipt, command: &str) -> Result<()> {
    if path.extension().and_then(|value| value.to_str()) == Some("json") {
        let value = json!({
            "tool": receipt.name,
            "command": command,
            "status": receipt.status,
            "success": receipt.success,
            "skipped": receipt.skipped,
            "detail": receipt.reason,
            "stdout": receipt.stdout,
            "stderr": receipt.stderr,
        });
        fs::write(
            path,
            serde_json::to_string_pretty(&value).context("serialize tool artifact")? + "\n",
        )?;
    } else {
        write_markdown(path, &receipt_markdown(receipt))?;
    }
    Ok(())
}

fn receipt_markdown(receipt: &CommandReceipt) -> String {
    let mut text = String::new();
    text.push_str(&format!("- tool: {}\n", receipt.name));
    if !receipt.command.is_empty() {
        text.push_str(&format!("- command: `{}`\n", receipt.command));
    }
    if let Some(status) = receipt.status {
        text.push_str(&format!("- status: {status}\n"));
    }
    text.push_str(&format!("- success: {}\n", receipt.success));
    if receipt.missing {
        text.push_str(
            "- missing: true
",
        );
    } else if receipt.skipped {
        text.push_str(
            "- skipped: true
",
        );
    }
    if receipt.stdout_truncated || receipt.stderr_truncated {
        text.push_str(
            "- output truncated by capture budget
",
        );
    }
    if let Some(reason) = &receipt.reason {
        text.push_str("\n```text\n");
        text.push_str(reason);
        if !reason.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```\n");
    }
    if !receipt.stdout.is_empty() || !receipt.stderr.is_empty() {
        text.push_str("\n## stdout\n\n```text\n");
        text.push_str(&receipt.stdout);
        if !receipt.stdout.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```\n\n## stderr\n\n```text\n");
        text.push_str(&receipt.stderr);
        if !receipt.stderr.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```\n");
    }
    text
}

fn write_markdown(path: &Path, text: &str) -> Result<()> {
    fs::write(path, text).with_context(|| format!("write {}", path.display()))
}

fn render_precommit_summary(
    options: PrecommitOptions,
    changed: &[ChangedFile],
    affected: &[WorkspacePackage],
    receipts: &[CommandReceipt],
    blocking_failures: usize,
) -> String {
    let mode = if options.staged {
        "staged"
    } else {
        "working tree"
    };
    let mut text = String::new();
    text.push_str("# Precommit summary\n\n");
    text.push_str(&format!("- mode: {mode}\n"));
    text.push_str(&format!("- changed files: {}\n", changed.len()));
    text.push_str(&format!("- affected Rust packages: {}\n", affected.len()));
    for package in affected {
        text.push_str(&format!("  - {}\n", package.name));
    }
    text.push_str(&format!("- blocking findings: {blocking_failures}\n\n"));
    text.push_str("## Checks\n\n");
    for receipt in receipts {
        let status = if receipt.missing {
            "missing"
        } else if receipt.skipped {
            "skipped"
        } else if receipt.success {
            "pass"
        } else {
            "fail"
        };
        let detail = receipt
            .reason
            .as_ref()
            .filter(|_| receipt.skipped)
            .map(|reason| format!(" ({reason})"))
            .unwrap_or_default();
        text.push_str(&format!("- {status}: {}{detail}\n", receipt.name));
    }
    text.push_str("\nArtifacts are under `target/precommit/`.\n");
    text
}

fn format_command(program: &str, args: &[&str]) -> String {
    let mut command = program.to_owned();
    for arg in args {
        command.push(' ');
        command.push_str(arg);
    }
    command
}

fn safe_name(name: &str) -> String {
    name.chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => character,
            _ => '-',
        })
        .collect()
}

fn normalize_path(path: &str) -> String {
    path.trim_start_matches("./").replace('\\', "/")
}

fn repo_absolute_path(canonical_root: &Path, path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        canonical_root.join(candidate)
    }
}

fn nearest_package_for_path<'a>(
    packages: &'a [WorkspacePackage],
    absolute: &Path,
) -> Option<&'a WorkspacePackage> {
    packages
        .iter()
        .filter(|package| absolute.starts_with(&package.manifest_dir))
        .max_by_key(|package| package.manifest_dir.as_os_str().len())
}

fn normalize_repo_path(root: &Path, path: &str) -> String {
    let normalized = normalize_path(path);
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return normalized;
    }

    let Some(relative) = repo_relative_path(root, candidate) else {
        return normalized;
    };
    relative
}

fn repo_relative_path(root: &Path, candidate: &Path) -> Option<String> {
    let canonical_root = root.canonicalize().ok()?;
    let absolute = candidate
        .canonicalize()
        .unwrap_or_else(|_| candidate.to_path_buf());
    absolute
        .strip_prefix(canonical_root)
        .ok()
        .map(path_to_slash_string)
}

fn path_to_slash_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn missing_tool_receipt_is_never_success_and_carries_install_hint() {
        // #320: a missing tool can never read as a clean pass again, and
        // #321: the receipt says exactly how to install it. It stays
        // non-blocking (skipped) so absent optional tooling does not fail
        // unrelated precommits.
        let receipt = missing_receipt("ripr");
        assert!(!receipt.success);
        assert!(receipt.skipped);
        assert!(receipt.missing);
        assert!(!receipt.success_is_blocking_failure());
        let reason = receipt.reason.as_deref().unwrap_or_default();
        assert!(reason.contains("not installed"), "{reason}");
        assert!(
            reason.contains("cargo install ripr --locked --version 0.8.0 --force"),
            "{reason}"
        );
        // A relevance skip stays success: true and missing: false - the two
        // receipt shapes can never be confused.
        let skip = skipped_receipt("ripr", "no Rust changes");
        assert!(skip.success && skip.skipped && !skip.missing);
        // Rendering keeps the distinction.
        let markdown = receipt_markdown(&receipt);
        assert!(markdown.contains("- missing: true"));
        assert!(!markdown.contains("- skipped: true"));
        let summary = render_precommit_summary(
            PrecommitOptions { staged: false },
            &[],
            &[],
            &[receipt, skip],
            0,
        );
        assert!(summary.contains("missing: ripr"), "{summary}");
        assert!(summary.contains("skipped: ripr"), "{summary}");
    }

    #[test]
    fn missing_tool_install_hints_match_standard_runner_fixes() {
        let expected = [
            (
                "tokmd",
                "cargo install tokmd --locked --version 1.12.0 --force",
            ),
            ("cargo-allow", "cargo install cargo-allow --locked"),
            (
                "ripr",
                "cargo install ripr --locked --version 0.8.0 --force",
            ),
            (
                "unsafe-review",
                "cargo install unsafe-review --locked --version 0.3.4 --force",
            ),
            ("ast-grep", "npm install -g @ast-grep/cli"),
            (
                "actionlint",
                "go install github.com/rhysd/actionlint/cmd/actionlint@v1.7.12; add $(go env GOPATH)/bin to PATH",
            ),
        ];

        for (tool, hint) in expected {
            assert_eq!(install_hint(tool), hint, "{tool} install hint drifted");
        }
        assert!(
            !install_hint("unsafe-review").contains("0.3.3"),
            "unsafe-review missing-tool hint must not point operators at the stale pre-0.3.4 sensor"
        );
    }

    #[test]
    fn clip_capture_bounds_streams_with_head_tail_and_marker() {
        // #317: under budget passes through untouched; over budget keeps
        // the head and tail with a marker naming the elided byte count, so
        // a 450 MB tool dump can never become a 450 MB receipt.
        let small = "small output".to_owned();
        let (kept, truncated) = clip_capture(small.clone());
        assert_eq!(kept, small);
        assert!(!truncated);

        let big = "a".repeat(CAPTURE_HEAD_BYTES + CAPTURE_TAIL_BYTES + 10_000);
        let (clipped, truncated) = clip_capture(big);
        assert!(truncated);
        assert!(clipped.len() < CAPTURE_HEAD_BYTES + CAPTURE_TAIL_BYTES + 200);
        assert!(
            clipped.contains("10000 bytes truncated by the precommit capture budget"),
            "marker names the elided bytes"
        );
        let receipt = CommandReceipt {
            name: "loud".to_owned(),
            command: "loud".to_owned(),
            status: Some(0),
            success: true,
            skipped: false,
            missing: false,
            reason: None,
            stdout: clipped,
            stderr: String::new(),
            stdout_truncated: true,
            stderr_truncated: false,
        };
        assert!(receipt_markdown(&receipt).contains("output truncated by capture budget"));
    }

    fn temp_repo_root(name: &str) -> Result<PathBuf> {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .context("system time before unix epoch")?
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "ub-review-xtask-{name}-{}-{suffix}",
            std::process::id()
        ));
        fs::create_dir_all(&root).with_context(|| format!("create {}", root.display()))?;
        Ok(root)
    }

    fn package(name: &str, manifest_dir: PathBuf) -> WorkspacePackage {
        WorkspacePackage {
            name: name.to_owned(),
            manifest_dir,
            targets: Vec::new(),
        }
    }

    fn changed(path: &str, lines: &[u64]) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            lines: lines.iter().copied().collect(),
        }
    }

    fn changed_names(packages: Vec<WorkspacePackage>) -> Vec<String> {
        packages
            .into_iter()
            .map(|package| package.name)
            .collect::<Vec<_>>()
    }

    fn write_bun_pin_docs(root: &Path, pin: &str, pr: &str) -> Result<()> {
        fs::create_dir_all(root.join("docs/calibration"))?;
        fs::create_dir_all(root.join("examples/bun/.github/workflows"))?;

        let action_ref = format!("EffortlessMetrics/ub-review@{pin}");
        let proof_ref = format!("EffortlessSteven/bun#{pr}");
        let files = [
            (
                "README.md",
                format!("{action_ref}\nvalidated by `{proof_ref}`\n"),
            ),
            (
                "REPO_READY.md",
                format!("current known-good pin `{pin}` validated by `{proof_ref}`\n"),
            ),
            ("RELEASE_NOTES.md", format!("uses `{action_ref}`\n")),
            (
                "RELEASE_NOTES_GH_RUNNER.md",
                format!("uses: {action_ref}\n"),
            ),
            ("docs/ACTION_CONSUMER_BUN.md", format!("pin is `{pin}`\n")),
            (
                "docs/GH_RUNNER_BUN.md",
                format!("{action_ref}\nvalidated by `{proof_ref}`\n"),
            ),
            (
                "docs/GH_RUNNER_SETUP.md",
                format!("{action_ref}\nvalidated by `{proof_ref}`\n"),
            ),
            (
                "docs/REPO_BOOTSTRAP.md",
                format!("known-good pin is `{pin}`\n"),
            ),
            (
                "docs/REPO_OPERATING_HANDOFF.md",
                format!("- Bun PR #{pr}: the Bun gate is pinned to `{action_ref}`\n"),
            ),
            (
                "docs/ROADMAP.md",
                format!(
                    "The v0 gate is `{action_ref}`.\nknown-good Bun workflow pin was advanced in `{proof_ref}` after validation.\n"
                ),
            ),
            (
                "docs/calibration/bun-ub-review-ledger.md",
                format!(
                    "# Bun UB Review Calibration Ledger\n\n## Current Bun gate pin\n\nPR: `#{pr}`\nPin: `{action_ref}`\nRun: `26954325725`\nArtifact: `ub-review-packet-{pr}`\n\n## Earlier item\n"
                ),
            ),
            (
                "examples/bun/.github/workflows/ub-review-packet.yml",
                format!(
                    "key: ub-review-gh-runner-v2-{pin}-${{{{ runner.os }}}}-rust-1.95-core\nrestore-keys: |\n  ub-review-gh-runner-v2-{pin}-${{{{ runner.os }}}}-rust-1.95-\nuses: {action_ref}\n"
                ),
            ),
        ];

        for (relative, text) in files {
            let path = root.join(relative);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&path, text).with_context(|| format!("write {}", path.display()))?;
        }
        Ok(())
    }

    #[test]
    fn bun_gate_pin_policy_accepts_consistent_docs() -> Result<()> {
        let root = temp_repo_root("bun-pin-consistent")?;
        write_bun_pin_docs(&root, "217f123e688e42ddfce98eec5795b88bf457dd34", "45")?;

        validate_bun_gate_pin(&root)?;

        fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
        Ok(())
    }

    #[test]
    fn bun_gate_pin_policy_rejects_example_pin_drift() -> Result<()> {
        let root = temp_repo_root("bun-pin-drift")?;
        let current = "217f123e688e42ddfce98eec5795b88bf457dd34";
        let stale = "1111111111111111111111111111111111111111";
        write_bun_pin_docs(&root, current, "45")?;
        let workflow = root.join("examples/bun/.github/workflows/ub-review-packet.yml");
        let text = fs::read_to_string(&workflow)?.replacen(current, stale, 1);
        fs::write(&workflow, text)?;

        let error = match validate_bun_gate_pin(&root) {
            Ok(()) => bail!("policy accepted a split Bun gate pin"),
            Err(error) => error,
        };
        assert!(
            error.to_string().contains("known-good Bun gate pin drift"),
            "{error:#}"
        );

        fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
        Ok(())
    }

    #[test]
    fn precommit_out_dir_starts_fresh() -> Result<()> {
        let root = temp_repo_root("precommit-out")?;
        let out_dir = root.join("target/precommit");
        fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;
        let stale = out_dir.join("stale-receipt.md");
        fs::write(&stale, "stale receipt\n")
            .with_context(|| format!("write {}", stale.display()))?;

        let prepared = prepare_precommit_out_dir(&root)?;

        assert_eq!(prepared, out_dir);
        assert!(prepared.is_dir());
        assert!(!stale.exists());
        fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
        Ok(())
    }

    #[test]
    fn affected_packages_include_package_manifest_changes() -> Result<()> {
        let root = temp_repo_root("manifest")?;
        let xtask_dir = root.join("xtask");
        fs::create_dir_all(&xtask_dir)
            .with_context(|| format!("create {}", xtask_dir.display()))?;
        let packages = vec![
            package("ub-review", root.canonicalize()?),
            package("xtask", xtask_dir.canonicalize()?),
        ];

        let affected = affected_packages(&root, &packages, &[changed("xtask/Cargo.toml", &[])])?;

        assert_eq!(changed_names(affected), vec!["xtask"]);
        fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
        Ok(())
    }

    #[test]
    fn affected_packages_include_all_packages_for_root_manifest_and_lockfile() -> Result<()> {
        let root = temp_repo_root("workspace")?;
        let xtask_dir = root.join("xtask");
        fs::create_dir_all(&xtask_dir)
            .with_context(|| format!("create {}", xtask_dir.display()))?;
        let packages = vec![
            package("ub-review", root.canonicalize()?),
            package("xtask", xtask_dir.canonicalize()?),
        ];

        let manifest_affected = affected_packages(&root, &packages, &[changed("Cargo.toml", &[])])?;
        let lock_affected = affected_packages(&root, &packages, &[changed("Cargo.lock", &[])])?;

        assert_eq!(changed_names(manifest_affected), vec!["ub-review", "xtask"]);
        assert_eq!(changed_names(lock_affected), vec!["ub-review", "xtask"]);
        fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
        Ok(())
    }

    #[test]
    fn clippy_findings_match_absolute_diagnostic_paths() -> Result<()> {
        let root = temp_repo_root("absolute-diagnostic")?;
        let source_dir = root.join("xtask/src");
        fs::create_dir_all(&source_dir)
            .with_context(|| format!("create {}", source_dir.display()))?;
        let source = source_dir.join("main.rs");
        fs::write(&source, "fn main() {}\n")
            .with_context(|| format!("write {}", source.display()))?;

        let mut changed = BTreeMap::new();
        changed.insert("xtask/src/main.rs".to_owned(), [1].into_iter().collect());
        let diagnostic = json!({
            "reason": "compiler-message",
            "message": {
                "level": "warning",
                "message": "lint on changed line",
                "spans": [{
                    "is_primary": true,
                    "file_name": source.display().to_string(),
                    "line_start": 1
                }]
            }
        });
        let mut findings = Vec::new();

        collect_clippy_finding(&root, "xtask", &diagnostic, &changed, &mut findings);

        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].path, "xtask/src/main.rs");
        fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
        Ok(())
    }

    #[test]
    fn clippy_findings_do_not_expand_empty_line_sets_to_whole_file() -> Result<()> {
        let root = temp_repo_root("empty-lines")?;
        let mut changed = BTreeMap::new();
        changed.insert("src/main.rs".to_owned(), BTreeSet::new());
        let diagnostic = json!({
            "reason": "compiler-message",
            "message": {
                "level": "warning",
                "message": "existing lint",
                "spans": [{
                    "is_primary": true,
                    "file_name": "src/main.rs",
                    "line_start": 10
                }]
            }
        });
        let mut findings = Vec::new();

        collect_clippy_finding(&root, "ub-review", &diagnostic, &changed, &mut findings);

        assert!(findings.is_empty());
        fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
        Ok(())
    }
}

#[derive(Debug, Default)]
struct PolicyReport {
    policy_files: usize,
    exceptions: usize,
    exception_kinds: BTreeMap<String, usize>,
    ci_lanes: usize,
    implemented_lanes: usize,
    risk_packs: usize,
}

impl PolicyReport {
    fn summary(&self) -> String {
        format!(
            "policy check passed: {} policy files, {} allow receipts, {} CI lanes, {} risk packs",
            self.policy_files, self.exceptions, self.ci_lanes, self.risk_packs
        )
    }

    fn inventory(&self) -> String {
        let mut text = String::new();
        text.push_str("# Policy inventory\n\n");
        text.push_str(&format!("- policy files: {}\n", self.policy_files));
        text.push_str(&format!("- allow receipts: {}\n", self.exceptions));
        for (kind, count) in &self.exception_kinds {
            text.push_str(&format!("  - {kind}: {count}\n"));
        }
        text.push_str(&format!("- CI lanes: {}\n", self.ci_lanes));
        text.push_str(&format!(
            "- implemented CI lanes: {}\n",
            self.implemented_lanes
        ));
        text.push_str(&format!("- CI risk packs: {}\n", self.risk_packs));
        text
    }
}

fn check_policy(root: &Path) -> Result<PolicyReport> {
    let policy_dir = root.join("policy");
    let mut report = PolicyReport::default();

    for file in policy_files(&policy_dir)? {
        parse_toml(&file)?;
        report.policy_files += 1;
    }

    validate_allow(&policy_dir.join("allow.toml"), &mut report)?;
    validate_ci_budget(&policy_dir.join("ci-budget.toml"))?;
    validate_ci_lanes(&policy_dir.join("ci-lanes.toml"), &mut report)?;
    validate_ci_risk_packs(&policy_dir.join("ci-risk-packs.toml"), &mut report)?;
    validate_bun_gate_pin(root)?;

    Ok(report)
}

const BUN_GATE_PIN_FILES: &[&str] = &[
    "README.md",
    "REPO_READY.md",
    "RELEASE_NOTES.md",
    "RELEASE_NOTES_GH_RUNNER.md",
    "docs/ACTION_CONSUMER_BUN.md",
    "docs/GH_RUNNER_BUN.md",
    "docs/GH_RUNNER_SETUP.md",
    "docs/REPO_BOOTSTRAP.md",
    "docs/REPO_OPERATING_HANDOFF.md",
    "docs/ROADMAP.md",
    "docs/calibration/bun-ub-review-ledger.md",
    "examples/bun/.github/workflows/ub-review-packet.yml",
];

fn validate_bun_gate_pin(root: &Path) -> Result<()> {
    let mut pins_by_value = BTreeMap::<String, Vec<String>>::new();

    for relative in BUN_GATE_PIN_FILES {
        let path = root.join(relative);
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let scanned = if *relative == "docs/calibration/bun-ub-review-ledger.md" {
            current_bun_gate_section(&text)?.to_owned()
        } else {
            text
        };
        let pins = sha40_strings(&scanned);
        if pins.is_empty() {
            bail!("{relative} must include the current Bun gate SHA pin");
        }
        for pin in pins {
            pins_by_value
                .entry(pin)
                .or_default()
                .push((*relative).to_owned());
        }
    }

    if pins_by_value.len() != 1 {
        let mut details = Vec::new();
        for (pin, files) in &pins_by_value {
            details.push(format!("{pin}: {}", files.join(", ")));
        }
        bail!(
            "known-good Bun gate pin drift: expected one SHA across docs/example, found {}",
            details.join("; ")
        );
    }

    let pin = pins_by_value
        .keys()
        .next()
        .context("known-good Bun gate pin missing")?;
    validate_example_bun_workflow(root, pin)?;
    validate_current_bun_gate_ledger(root, pin)?;
    Ok(())
}

fn validate_example_bun_workflow(root: &Path, pin: &str) -> Result<()> {
    let relative = "examples/bun/.github/workflows/ub-review-packet.yml";
    let path = root.join(relative);
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let action_ref = format!("uses: EffortlessMetrics/ub-review@{pin}");
    if !text.contains(&action_ref) {
        bail!("{relative} must use the current Bun gate pin in the action ref");
    }
    let count = count_occurrences(&text, pin);
    if count != 3 {
        bail!("{relative} expected current Bun gate pin exactly 3 times, found {count}");
    }
    Ok(())
}

fn validate_current_bun_gate_ledger(root: &Path, pin: &str) -> Result<()> {
    let ledger_path = root.join("docs/calibration/bun-ub-review-ledger.md");
    let text = fs::read_to_string(&ledger_path)
        .with_context(|| format!("read {}", ledger_path.display()))?;
    let section = current_bun_gate_section(&text)?;
    let expected_pin = format!("Pin: `EffortlessMetrics/ub-review@{pin}`");
    if !section.contains(&expected_pin) {
        bail!("current Bun gate ledger pin must match adoption docs");
    }

    let pr = extract_backtick_field(section, "PR: `#")?;
    let run = extract_backtick_field(section, "Run: `")?;
    let artifact = extract_backtick_field(section, "Artifact: `")?;
    if run.chars().any(|character| !character.is_ascii_digit()) {
        bail!("current Bun gate ledger run must be a numeric GitHub run id");
    }
    if !artifact.starts_with("ub-review-packet-") {
        bail!("current Bun gate ledger artifact must name the Bun packet artifact");
    }

    require_current_bun_pr_reference(root, "README.md", pr, "EffortlessSteven/bun#")?;
    require_current_bun_pr_reference(root, "REPO_READY.md", pr, "EffortlessSteven/bun#")?;
    require_current_bun_pr_reference(root, "docs/GH_RUNNER_BUN.md", pr, "EffortlessSteven/bun#")?;
    require_current_bun_pr_reference(root, "docs/GH_RUNNER_SETUP.md", pr, "EffortlessSteven/bun#")?;
    require_current_bun_pr_reference(root, "docs/REPO_OPERATING_HANDOFF.md", pr, "Bun PR #")?;
    require_current_bun_pr_reference(root, "docs/ROADMAP.md", pr, "EffortlessSteven/bun#")?;
    Ok(())
}

fn require_current_bun_pr_reference(
    root: &Path,
    relative: &str,
    pr: &str,
    prefix: &str,
) -> Result<()> {
    let path = root.join(relative);
    let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
    let expected = format!("{prefix}{pr}");
    if !text.contains(&expected) {
        bail!("{relative} must reference current Bun gate proof {expected}");
    }
    Ok(())
}

fn current_bun_gate_section(text: &str) -> Result<&str> {
    let marker = "## Current Bun gate pin";
    let start = text
        .find(marker)
        .context("docs/calibration/bun-ub-review-ledger.md missing current Bun gate section")?;
    let rest = &text[start..];
    let after_marker = &rest[marker.len()..];
    if let Some(next_heading) = after_marker.find("\n## ") {
        Ok(&rest[..marker.len() + next_heading])
    } else {
        Ok(rest)
    }
}

fn extract_backtick_field<'a>(section: &'a str, prefix: &str) -> Result<&'a str> {
    let start = section
        .find(prefix)
        .with_context(|| format!("current Bun gate ledger missing `{prefix}` field"))?
        + prefix.len();
    let tail = &section[start..];
    let end = tail
        .find('`')
        .with_context(|| format!("current Bun gate ledger `{prefix}` field must close with `"))?;
    let value = &tail[..end];
    if value.trim().is_empty() {
        bail!("current Bun gate ledger `{prefix}` field must not be empty");
    }
    Ok(value)
}

fn sha40_strings(text: &str) -> BTreeSet<String> {
    let mut values = BTreeSet::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if !bytes[index].is_ascii_hexdigit() {
            index += 1;
            continue;
        }
        let start = index;
        while index < bytes.len() && bytes[index].is_ascii_hexdigit() {
            index += 1;
        }
        if index - start == 40 {
            values.insert(text[start..index].to_owned());
        }
    }
    values
}

fn count_occurrences(text: &str, needle: &str) -> usize {
    if needle.is_empty() {
        return 0;
    }
    let mut count = 0;
    let mut rest = text;
    while let Some(index) = rest.find(needle) {
        count += 1;
        rest = &rest[index + needle.len()..];
    }
    count
}

fn policy_files(policy_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in fs::read_dir(policy_dir)
        .with_context(|| format!("read policy directory {}", policy_dir.display()))?
    {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) == Some("toml") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn parse_toml(path: &Path) -> Result<Value> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    toml::from_str(&text).with_context(|| format!("parse {}", path.display()))
}

fn validate_allow(path: &Path, report: &mut PolicyReport) -> Result<()> {
    let value = parse_toml(path)?;
    let root = table(&value, path, "root")?;
    require_schema_version(root, path)?;
    require_str(root, path, "tool")?;
    let exceptions = array(root, path, "exception")?;
    let mut ids = BTreeSet::new();

    for (index, exception) in exceptions.iter().enumerate() {
        let context = format!("exception[{index}]");
        let item = table(exception, path, &context)?;
        let id = require_str(item, path, "id")?;
        if !ids.insert(id.to_owned()) {
            bail!("{} duplicate exception id `{id}`", path.display());
        }
        let kind = require_str(item, path, "kind")?;
        require_str(item, path, "owner")?;
        require_str(item, path, "reason")?;
        require_str(item, path, "created")?;
        require_str(item, path, "review_after")?;
        if item.get("path").is_none() && item.get("glob").is_none() {
            bail!(
                "{} exception `{id}` must include either `path` or `glob`",
                path.display()
            );
        }
        if let Some(expires) = item.get("expires") {
            non_empty_str(expires, path, "expires")?;
        }
        *report.exception_kinds.entry(kind.to_owned()).or_insert(0) += 1;
        report.exceptions += 1;
    }

    Ok(())
}

fn validate_ci_budget(path: &Path) -> Result<()> {
    let value = parse_toml(path)?;
    let root = table(&value, path, "root")?;
    require_schema_version(root, path)?;
    let budget = table_key(root, path, "budget")?;
    require_integer(budget, path, "preferred_default_lem")?;
    require_integer(budget, path, "default_limit_lem")?;
    require_integer(budget, path, "elevated_limit_lem")?;
    require_integer(budget, path, "hard_limit_lem")?;
    table_key(root, path, "bands")?;
    Ok(())
}

fn validate_ci_lanes(path: &Path, report: &mut PolicyReport) -> Result<()> {
    let value = parse_toml(path)?;
    let root = table(&value, path, "root")?;
    require_schema_version(root, path)?;
    require_str(root, path, "summary_check")?;
    let lanes = array(root, path, "lane")?;
    let mut ids = BTreeSet::new();

    for (index, lane) in lanes.iter().enumerate() {
        let context = format!("lane[{index}]");
        let item = table(lane, path, &context)?;
        let id = require_str(item, path, "id")?;
        if !ids.insert(id.to_owned()) {
            bail!("{} duplicate lane id `{id}`", path.display());
        }
        require_str(item, path, "when")?;
        require_bool(item, path, "target_required")?;
        if require_bool(item, path, "implemented")? {
            report.implemented_lanes += 1;
        }
        require_str(item, path, "reason")?;
        report.ci_lanes += 1;
    }

    Ok(())
}

fn validate_ci_risk_packs(path: &Path, report: &mut PolicyReport) -> Result<()> {
    let value = parse_toml(path)?;
    let root = table(&value, path, "root")?;
    require_schema_version(root, path)?;
    let packs = array(root, path, "risk_pack")?;
    let mut ids = BTreeSet::new();

    for (index, pack) in packs.iter().enumerate() {
        let context = format!("risk_pack[{index}]");
        let item = table(pack, path, &context)?;
        let id = require_str(item, path, "id")?;
        if !ids.insert(id.to_owned()) {
            bail!("{} duplicate risk_pack id `{id}`", path.display());
        }
        require_string_array(item, path, "labels")?;
        require_string_array(item, path, "lanes")?;
        require_str(item, path, "reason")?;
        report.risk_packs += 1;
    }

    Ok(())
}

fn require_schema_version(table: &Map<String, Value>, path: &Path) -> Result<()> {
    // The installed cargo-allow release deserializes schema_version as a
    // string, so the ledger records `"1"`; accept the legacy integer form too
    // so older ledgers keep validating.
    let version = table
        .get("schema_version")
        .with_context(|| format!("{} missing `schema_version`", path.display()))?;
    let matches_v1 = match version {
        Value::String(text) => text == "1",
        Value::Integer(number) => *number == 1,
        _ => false,
    };
    if !matches_v1 {
        bail!(
            "{} expected schema_version = \"1\", found {version}",
            path.display()
        );
    }
    Ok(())
}

fn table<'a>(value: &'a Value, path: &Path, context: &str) -> Result<&'a Map<String, Value>> {
    value
        .as_table()
        .with_context(|| format!("{} {context} must be a TOML table", path.display()))
}

fn table_key<'a>(
    table: &'a Map<String, Value>,
    path: &Path,
    key: &str,
) -> Result<&'a Map<String, Value>> {
    let value = table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?;
    value
        .as_table()
        .with_context(|| format!("{} `{key}` must be a table", path.display()))
}

fn array<'a>(table: &'a Map<String, Value>, path: &Path, key: &str) -> Result<&'a [Value]> {
    let values = table
        .get(key)
        .with_context(|| format!("{} missing `[[{key}]]` entries", path.display()))?
        .as_array()
        .with_context(|| format!("{} `{key}` must be an array", path.display()))?;
    if values.is_empty() {
        bail!("{} `{key}` must not be empty", path.display());
    }
    Ok(values)
}

fn require_str<'a>(table: &'a Map<String, Value>, path: &Path, key: &str) -> Result<&'a str> {
    let value = table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?;
    non_empty_str(value, path, key)
}

fn non_empty_str<'a>(value: &'a Value, path: &Path, key: &str) -> Result<&'a str> {
    let text = value
        .as_str()
        .with_context(|| format!("{} `{key}` must be a string", path.display()))?
        .trim();
    if text.is_empty() {
        bail!("{} `{key}` must not be empty", path.display());
    }
    Ok(text)
}

fn require_integer(table: &Map<String, Value>, path: &Path, key: &str) -> Result<i64> {
    table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?
        .as_integer()
        .with_context(|| format!("{} `{key}` must be an integer", path.display()))
}

fn require_bool(table: &Map<String, Value>, path: &Path, key: &str) -> Result<bool> {
    table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?
        .as_bool()
        .with_context(|| format!("{} `{key}` must be a boolean", path.display()))
}

fn require_string_array(table: &Map<String, Value>, path: &Path, key: &str) -> Result<()> {
    let values = table
        .get(key)
        .with_context(|| format!("{} missing `{key}`", path.display()))?
        .as_array()
        .with_context(|| format!("{} `{key}` must be an array", path.display()))?;
    if values.is_empty() {
        bail!("{} `{key}` must not be empty", path.display());
    }
    for value in values {
        non_empty_str(value, path, key)?;
    }
    Ok(())
}
