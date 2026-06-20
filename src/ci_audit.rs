//! audit-ci CI right-sizing report and setup-ci gate scaffolding:
//! workflow scanning, GitHub CI history fetch, job-tier classification,
//! report rendering, and setup-ci PR generation (cleanup train step 60,
//! pure code motion).

use crate::*;

// ---------------------------------------------------------------------------
// audit-ci: read-only CI right-sizing report (docs/CI_AUDIT_WIZARD.md).
// v0 is deterministic only: no model calls, judgment is always "deterministic".
// ---------------------------------------------------------------------------

// Broad on purpose: `push`, `apply`, and `docker` will overmatch legitimate
// job names (e.g. `push-docs-preview`). That is acceptable — ambiguity
// resolves to flag-for-human per docs/CI_AUDIT_WIZARD.md; a false-positive
// flag costs a human a glance, a false-negative escape auto-right-sizes a
// security-sensitive job.
pub(crate) const CI_AUDIT_SECURITY_PATTERNS: &[&str] = &[
    "codeql",
    "secret",
    "scan",
    "sign",
    "provenance",
    "attest",
    "deploy",
    "release",
    "publish",
    "permission",
    "push",
    "apply",
    "terraform",
    "sarif",
    "compliance",
    "docker",
];
pub(crate) const CI_AUDIT_RUN_PAGE_CAP: usize = 10;
pub(crate) const CI_AUDIT_RUNS_PER_PAGE: usize = 100;
pub(crate) const CI_AUDIT_MIN_HISTORY_RUNS: usize = 20;
pub(crate) const CI_AUDIT_CHEAP_P50_SEC: u64 = 300;
pub(crate) const CI_AUDIT_HIGH_VOLUME_RUNS: usize = 100;
pub(crate) const CI_AUDIT_MEDIUM_VOLUME_RUNS: usize = 50;
pub(crate) const CI_AUDIT_INDEPENDENT_FAILURE_RULE: &str = "a job counts an independent failure when it \
fails on a pull_request run while every cheaper job (lower duration p50 within the same \
workflow run) passed; skipped cheaper jobs count as passed";
pub(crate) const CI_AUDIT_DYNAMIC_SECRET_REF: &str = "dynamic-secret-reference";

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CiWorkflowScan {
    pub(crate) path: String,
    pub(crate) triggers: Vec<String>,
    pub(crate) path_filters: Vec<String>,
    pub(crate) path_ignore_filters: Vec<String>,
    pub(crate) cancel_in_progress: bool,
    pub(crate) permissions: Option<serde_json::Value>,
    pub(crate) uses_secrets: Vec<String>,
    pub(crate) yaml_jobs: Vec<CiWorkflowYamlJob>,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub(crate) struct CiWorkflowYamlJob {
    pub(crate) id: String,
    pub(crate) name: Option<String>,
    pub(crate) runs_on: Vec<String>,
    pub(crate) matrix_size: usize,
    pub(crate) timeout_minutes: Option<u64>,
    pub(crate) uses: Vec<String>,
    pub(crate) permissions: Option<serde_json::Value>,
    pub(crate) uses_secrets: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CiApiWorkflow {
    id: u64,
    #[serde(default)]
    name: String,
    #[serde(default)]
    path: String,
}

pub(crate) fn ci_default_run_attempt() -> u32 {
    1
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CiApiRun {
    id: u64,
    workflow_id: u64,
    #[serde(default = "ci_default_run_attempt")]
    run_attempt: u32,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    event: String,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct CiApiJob {
    #[serde(default)]
    name: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    started_at: Option<DateTime<Utc>>,
    #[serde(default)]
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone)]
pub(crate) struct CiRunWithJobs {
    pub(crate) run: CiApiRun,
    pub(crate) jobs: Vec<CiApiJob>,
}

#[derive(Debug)]
pub(crate) struct CiAuditFetch {
    pub(crate) workflows: Vec<CiApiWorkflow>,
    pub(crate) runs: Vec<CiRunWithJobs>,
    pub(crate) pages_fetched: usize,
    pub(crate) truncated: bool,
    pub(crate) required_checks: CiRequiredChecks,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct CiRequiredChecks {
    /// Required check context -> source (`branch-protection` or `ruleset`).
    pub(crate) contexts: BTreeMap<String, String>,
    /// Source used for negative determinations once the API answered.
    pub(crate) default_source: Option<String>,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug, Clone)]
pub(crate) struct CiJobStats {
    workflow_path: String,
    workflow_name: String,
    pub(crate) job: String,
    pub(crate) runs: usize,
    failures: usize,
    cancellations: usize,
    cancelled_started_without_completion: usize,
    pub(crate) duration_p50_sec: Option<u64>,
    duration_p90_sec: Option<u64>,
    duration_p99_sec: Option<u64>,
    total_duration_sec: u64,
    pub(crate) independent_failures: usize,
    pub(crate) co_failing_jobs: Vec<String>,
    pub(crate) cheaper_jobs_compared: Vec<String>,
    rerun_then_pass: usize,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct CiInventoryJob {
    workflow: String,
    pub(crate) job: String,
    pub(crate) name: String,
    triggers: Vec<String>,
    path_filters: Vec<String>,
    pub(crate) matrix_size: usize,
    timeout_minutes: Option<u64>,
    pub(crate) permissions: Option<serde_json::Value>,
    pub(crate) uses_secrets: Vec<String>,
    pub(crate) required_check: Option<bool>,
    pub(crate) required_check_source: String,
    #[serde(default)]
    pub(crate) required_check_context: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct CiInventoryArtifact {
    pub(crate) schema: String,
    generated_at: DateTime<Utc>,
    pub(crate) repo: String,
    pub(crate) window_days: u32,
    pub(crate) jobs: Vec<CiInventoryJob>,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CiHistoryJob {
    job: String,
    workflow: String,
    window_days: u32,
    runs: usize,
    failure_rate: f64,
    cancellation_rate: f64,
    flake_rate: f64,
    rerun_then_pass: usize,
    evidence_gaps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CiHistoryArtifact {
    pub(crate) schema: String,
    repo: String,
    window_days: u32,
    runs_fetched: usize,
    pages_fetched: usize,
    page_cap: usize,
    run_cap: usize,
    truncated: bool,
    pub(crate) jobs: Vec<CiHistoryJob>,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CiCostsJob {
    pub(crate) job: String,
    workflow: String,
    duration_p50_sec: Option<u64>,
    duration_p90_sec: Option<u64>,
    duration_p99_sec: Option<u64>,
    runner_minutes_per_month: u64,
    pub(crate) matrix_expansion: usize,
}

#[derive(Debug, Serialize)]
pub(crate) struct CiCostsArtifact {
    pub(crate) schema: String,
    repo: String,
    window_days: u32,
    pub(crate) jobs: Vec<CiCostsJob>,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CiCorrelationJob {
    job: String,
    workflow: String,
    independent_failures: usize,
    co_failing_jobs: Vec<String>,
    cheaper_jobs_compared: Vec<String>,
    window_days: u32,
}

#[derive(Debug, Serialize)]
pub(crate) struct CiCorrelationArtifact {
    pub(crate) schema: String,
    repo: String,
    window_days: u32,
    independent_failure_rule: String,
    pub(crate) jobs: Vec<CiCorrelationJob>,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct CiRecommendation {
    pub(crate) job: String,
    pub(crate) workflow: String,
    pub(crate) tier: String,
    pub(crate) positioned_to_catch: String,
    pub(crate) has_caught: String,
    pub(crate) receipts: Vec<String>,
    pub(crate) proposed_policy: String,
    confidence: String,
    pub(crate) judgment: String,
    pub(crate) reason: String,
    /// Short fragment for the markdown report line; never repeats the
    /// receipts (runs, p50, independent failures) already on the line.
    report_note: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct CiRecommendationsArtifact {
    pub(crate) schema: String,
    pub(crate) repo: String,
    pub(crate) window_days: u32,
    pub(crate) jobs: Vec<CiRecommendation>,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CiRunnerCancellation {
    pub(crate) classification: String,
    workflow: String,
    workflow_name: String,
    pub(crate) job: String,
    runs: usize,
    cancellations: usize,
    cancellation_rate: f64,
    pub(crate) audit_cancel_events: Option<usize>,
    pub(crate) runner_shutdown_signal: bool,
    pub(crate) github_hosted: bool,
    runner_labels: Vec<String>,
    pub(crate) suggested_action: String,
    pub(crate) receipts: Vec<String>,
    evidence: Vec<String>,
    evidence_gaps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct CiRunnerCancellationsArtifact {
    pub(crate) schema: String,
    repo: String,
    window_days: u32,
    pub(crate) classifications: Vec<CiRunnerCancellation>,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct CiAuditArtifacts {
    pub(crate) inventory: CiInventoryArtifact,
    pub(crate) history: CiHistoryArtifact,
    pub(crate) costs: CiCostsArtifact,
    pub(crate) correlation: CiCorrelationArtifact,
    pub(crate) recommendations: CiRecommendationsArtifact,
    pub(crate) runner_cancellations: CiRunnerCancellationsArtifact,
    pub(crate) report: String,
}

pub(crate) fn cmd_audit_ci(args: AuditCiArgs) -> Result<()> {
    let out_dir = args.out.join("ci-audit");
    fs::create_dir_all(&out_dir).with_context(|| format!("create {}", out_dir.display()))?;
    let repo = resolve_ci_audit_repo(&args)?;
    let scans = scan_local_workflows(&args.root)?;
    let token = args
        .github_token
        .clone()
        .filter(|value| !value.trim().is_empty());
    let fetch = match token {
        Some(token) => Some(fetch_ci_audit_history(&args, &repo, &token, &out_dir)?),
        None => None,
    };
    let artifacts = build_ci_audit_artifacts(
        &repo,
        args.window_days,
        &scans,
        fetch.as_ref(),
        args.audit_cancel_events,
        Utc::now(),
    );
    write_ci_audit_artifacts(&out_dir, &artifacts)?;
    println!(
        "audit-ci: wrote {} ({} jobs, {} mode)",
        out_dir.display(),
        artifacts.inventory.jobs.len(),
        if fetch.is_some() {
            "history"
        } else {
            "inventory-only"
        }
    );
    Ok(())
}

pub(crate) fn resolve_ci_audit_repo(args: &AuditCiArgs) -> Result<String> {
    // A set-but-empty GITHUB_REPOSITORY (clap env fallback) is treated like
    // an absent value and falls back to the git origin remote, mirroring the
    // token path's empty/whitespace filtering.
    if let Some(repo) = &args.repo {
        let trimmed = repo.trim();
        if !trimmed.is_empty() {
            if is_valid_repo_slug(trimmed) {
                return Ok(trimmed.to_owned());
            }
            bail!("--repo must be a valid owner/name slug, got `{repo}`");
        }
    }
    let url = git_text(&args.root, &["remote", "get-url", "origin"])
        .with_context(|| "derive owner/repo from git remote origin")?;
    ci_repo_slug_from_remote_url(url.trim())
        .ok_or_else(|| anyhow::anyhow!("cannot derive owner/repo from origin url `{}`", url.trim()))
}

pub(crate) fn ci_repo_slug_from_remote_url(url: &str) -> Option<String> {
    let trimmed = url.trim().trim_end_matches('/').trim_end_matches(".git");
    let tail = if let Some((_, rest)) = trimmed.split_once("://") {
        let (_, path) = rest.split_once('/')?;
        path
    } else if let Some((_, rest)) = trimmed.split_once(':') {
        rest
    } else {
        return None;
    };
    let mut parts = tail.trim_matches('/').split('/');
    let owner = parts.next()?;
    let name = parts.next()?;
    let slug = format!("{owner}/{name}");
    is_valid_repo_slug(&slug).then_some(slug)
}

pub(crate) fn scan_local_workflows(root: &Path) -> Result<Vec<CiWorkflowScan>> {
    let dir = root.join(".github").join("workflows");
    let mut scans = Vec::new();
    if !dir.is_dir() {
        return Ok(scans);
    }
    let mut paths: Vec<PathBuf> = fs::read_dir(&dir)
        .with_context(|| format!("read {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            matches!(
                path.extension().and_then(|ext| ext.to_str()),
                Some("yml") | Some("yaml")
            )
        })
        .collect();
    paths.sort();
    for path in paths {
        let text = fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("workflow.yml");
        scans.push(scan_workflow_text(
            &format!(".github/workflows/{file_name}"),
            &text,
        ));
    }
    Ok(scans)
}

pub(crate) fn ci_yaml_strip_comment(line: &str) -> &str {
    let mut in_single = false;
    let mut in_double = false;
    for (index, character) in line.char_indices() {
        match character {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '#' if !in_single && !in_double => {
                let before = line[..index].chars().next_back();
                if index == 0 || before.is_some_and(char::is_whitespace) {
                    return &line[..index];
                }
            }
            _ => {}
        }
    }
    line
}

pub(crate) fn ci_yaml_unquote(value: &str) -> String {
    let trimmed = value.trim();
    let unquoted = trimmed
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
        .or_else(|| {
            trimmed
                .strip_prefix('\'')
                .and_then(|rest| rest.strip_suffix('\''))
        })
        .unwrap_or(trimmed);
    unquoted.to_owned()
}

pub(crate) fn ci_yaml_inline_list(value: &str) -> Option<Vec<String>> {
    let trimmed = value.trim();
    let inner = trimmed.strip_prefix('[')?.strip_suffix(']')?;
    Some(
        inner
            .split(',')
            .map(ci_yaml_unquote)
            .filter(|item| !item.is_empty())
            .collect(),
    )
}

pub(crate) fn ci_yaml_scalar_json(value: &str) -> serde_json::Value {
    let unquoted = ci_yaml_unquote(value);
    match unquoted.to_ascii_lowercase().as_str() {
        "null" | "~" => serde_json::Value::Null,
        "true" => serde_json::Value::Bool(true),
        "false" => serde_json::Value::Bool(false),
        _ => serde_json::Value::String(unquoted),
    }
}

pub(crate) fn ci_yaml_inline_mapping(
    value: &str,
) -> Option<serde_json::Map<String, serde_json::Value>> {
    let trimmed = value.trim();
    let inner = trimmed.strip_prefix('{')?.strip_suffix('}')?.trim();
    let mut map = serde_json::Map::new();
    if inner.is_empty() {
        return Some(map);
    }
    for item in inner.split(',') {
        let (key, value) = item.split_once(':')?;
        let key = ci_yaml_unquote(key);
        if key.is_empty() {
            return None;
        }
        map.insert(key, ci_yaml_scalar_json(value));
    }
    Some(map)
}

pub(crate) fn ci_yaml_permissions_at(
    lines: &[(usize, String)],
    key_index: usize,
    key_indent: usize,
    inline_value: &str,
) -> (Option<serde_json::Value>, usize) {
    let inline = inline_value.trim();
    if !inline.is_empty() {
        let value = ci_yaml_inline_mapping(inline)
            .map(serde_json::Value::Object)
            .unwrap_or_else(|| ci_yaml_scalar_json(inline));
        return (Some(value), key_index + 1);
    }

    let mut map = serde_json::Map::new();
    let mut saw_child = false;
    let mut index = key_index + 1;
    while index < lines.len() {
        let (indent, content) = &lines[index];
        if *indent <= key_indent {
            break;
        }
        saw_child = true;
        let body = content.strip_prefix("- ").unwrap_or(content);
        if let Some((key, value)) = body.split_once(':') {
            let key = ci_yaml_unquote(key);
            if !key.is_empty() {
                let value = value.trim();
                map.insert(
                    key,
                    if value.is_empty() {
                        serde_json::Value::String("unparsed nested permission".to_owned())
                    } else {
                        ci_yaml_scalar_json(value)
                    },
                );
            }
        }
        index += 1;
    }

    let value = if !map.is_empty() {
        serde_json::Value::Object(map)
    } else if saw_child {
        serde_json::Value::String("unparsed permissions block".to_owned())
    } else {
        serde_json::Value::String("empty permissions block".to_owned())
    };
    (Some(value), index)
}

pub(crate) fn ci_collect_secret_refs(text: &str, out: &mut BTreeSet<String>) {
    let mut offset = 0;
    while let Some(relative) = text[offset..].find("secrets.") {
        let start = offset + relative + "secrets.".len();
        let mut end = start;
        for (index, character) in text[start..].char_indices() {
            if character.is_ascii_alphanumeric() || character == '_' {
                end = start + index + character.len_utf8();
            } else {
                break;
            }
        }
        if end > start {
            out.insert(text[start..end].to_owned());
            offset = end;
        } else {
            offset = start;
        }
    }

    let mut offset = 0;
    while let Some(relative) = text[offset..].find("secrets[") {
        let start = offset + relative + "secrets[".len();
        let rest = text[start..].trim_start();
        if let Some(quote) = rest
            .chars()
            .next()
            .filter(|quote| *quote == '\'' || *quote == '"')
        {
            let name_start = start + text[start..].len() - rest.len() + quote.len_utf8();
            let mut end = name_start;
            let mut valid = true;
            for (index, character) in text[name_start..].char_indices() {
                if character == quote {
                    break;
                }
                if character.is_ascii_alphanumeric() || character == '_' {
                    end = name_start + index + character.len_utf8();
                } else {
                    valid = false;
                    break;
                }
            }
            if valid && end > name_start {
                out.insert(text[name_start..end].to_owned());
                offset = end;
                continue;
            }
        }
        out.insert(CI_AUDIT_DYNAMIC_SECRET_REF.to_owned());
        offset = start;
    }
}

pub(crate) fn ci_workflow_permissions_value(indent: usize, content: &str) -> Option<&str> {
    if indent == 0 {
        content.strip_prefix("permissions:")
    } else {
        None
    }
}

/// Collect a YAML list value: inline `[a, b]`, inline scalar, or dash items on
/// following deeper-indented lines. Returns the items and the next line index.
pub(crate) fn ci_yaml_list_at(
    lines: &[(usize, String)],
    key_index: usize,
    key_indent: usize,
    inline_value: &str,
) -> (Vec<String>, usize) {
    let inline = inline_value.trim();
    if !inline.is_empty() {
        let items = ci_yaml_inline_list(inline)
            .unwrap_or_else(|| vec![ci_yaml_unquote(inline)])
            .into_iter()
            .filter(|item| !item.is_empty())
            .collect();
        return (items, key_index + 1);
    }
    let mut items = Vec::new();
    let mut index = key_index + 1;
    while index < lines.len() {
        let (indent, content) = &lines[index];
        if *indent <= key_indent {
            break;
        }
        let Some(rest) = content
            .strip_prefix("- ")
            .or_else(|| if content == "-" { Some("") } else { None })
        else {
            break;
        };
        let item = ci_yaml_unquote(rest);
        if !item.is_empty() {
            items.push(item);
        }
        index += 1;
    }
    (items, index)
}

pub(crate) fn ci_yaml_skip_nested_block(
    lines: &[(usize, String)],
    start: usize,
    parent_indent: usize,
) -> usize {
    let mut index = start + 1;
    while index < lines.len() && lines[index].0 > parent_indent {
        index += 1;
    }
    index
}

pub(crate) fn ci_yaml_matrix_literal_size_at(
    lines: &[(usize, String)],
    key_index: usize,
    key_indent: usize,
    inline_value: &str,
) -> (usize, Vec<String>, usize) {
    let inline = inline_value.trim();
    if !inline.is_empty() {
        return (
            1,
            vec![format!("inline matrix value `{inline}` is not parsed")],
            key_index + 1,
        );
    }

    let mut size = 1usize;
    let mut saw_dimension = false;
    let mut gaps = Vec::new();
    let mut child_indent: Option<usize> = None;
    let mut index = key_index + 1;
    while index < lines.len() {
        let (indent, content) = &lines[index];
        if *indent <= key_indent {
            break;
        }
        let expected = *child_indent.get_or_insert(*indent);
        if *indent != expected || content.starts_with('-') {
            index += 1;
            continue;
        }
        let Some((raw_key, raw_value)) = content.split_once(':') else {
            gaps.push(format!(
                "matrix entry `{content}` is not a key/value dimension"
            ));
            index += 1;
            continue;
        };
        let key = ci_yaml_unquote(raw_key);
        let value = raw_value.trim();
        if matches!(key.as_str(), "include" | "exclude") {
            gaps.push(format!(
                "matrix `{key}` adjustments are not applied to matrix_size"
            ));
            index = ci_yaml_skip_nested_block(lines, index, *indent);
            continue;
        }
        if value.contains("${{") {
            gaps.push(format!(
                "matrix dimension `{key}` uses a dynamic expression"
            ));
            index += 1;
            continue;
        }
        if value.is_empty() {
            let (items, next) = ci_yaml_list_at(lines, index, *indent, value);
            if items.is_empty() {
                gaps.push(format!("matrix dimension `{key}` is not a literal list"));
            } else {
                saw_dimension = true;
                size = size.saturating_mul(items.len().max(1));
            }
            index = next;
            continue;
        }
        if let Some(items) = ci_yaml_inline_list(value) {
            saw_dimension = true;
            size = size.saturating_mul(items.len().max(1));
        } else {
            // A scalar matrix dimension is a single literal value in GitHub
            // Actions, but recording it keeps the expansion exact at x1.
            saw_dimension = true;
        }
        index += 1;
    }

    if !saw_dimension && gaps.is_empty() {
        gaps.push("matrix block did not contain literal dimensions".to_owned());
    }
    (size.max(1), gaps, index)
}

pub(crate) fn ci_yaml_strategy_matrix_size_at(
    lines: &[(usize, String)],
    key_index: usize,
    key_indent: usize,
    inline_value: &str,
) -> (usize, Vec<String>, usize) {
    let inline = inline_value.trim();
    if !inline.is_empty() {
        return (
            1,
            vec![format!("inline strategy value `{inline}` is not parsed")],
            key_index + 1,
        );
    }

    let mut matrix_size = 1usize;
    let mut gaps = Vec::new();
    let mut child_indent: Option<usize> = None;
    let mut index = key_index + 1;
    while index < lines.len() {
        let (indent, content) = &lines[index];
        if *indent <= key_indent {
            break;
        }
        let expected = *child_indent.get_or_insert(*indent);
        if *indent != expected || content.starts_with('-') {
            index += 1;
            continue;
        }
        if let Some(rest) = content.strip_prefix("matrix:") {
            let (size, matrix_gaps, next) =
                ci_yaml_matrix_literal_size_at(lines, index, *indent, rest);
            matrix_size = size;
            gaps.extend(matrix_gaps);
            index = next;
            continue;
        }
        index += 1;
    }
    (matrix_size.max(1), gaps, index)
}

/// Targeted line-scan of a workflow file. This is intentionally not a YAML
/// parser: it extracts only `on:` triggers (with branch refinement), workflow
/// level `paths`/`paths-ignore`, workflow cancellation concurrency,
/// workflow/job `permissions`, per-job secret references, and per-job
/// `runs-on`, literal `strategy.matrix` dimensions, `timeout-minutes`, `name`,
/// and `uses` references. Everything else is an explicit evidence gap.
pub(crate) fn scan_workflow_text(path: &str, text: &str) -> CiWorkflowScan {
    let lines: Vec<(usize, String)> = text
        .lines()
        .filter_map(|raw| {
            let stripped = ci_yaml_strip_comment(raw);
            let trimmed_end = stripped.trim_end();
            if trimmed_end.trim().is_empty() {
                return None;
            }
            let indent = trimmed_end.len() - trimmed_end.trim_start().len();
            Some((indent, trimmed_end.trim_start().to_owned()))
        })
        .collect();
    let mut triggers = Vec::new();
    let mut path_filters = Vec::new();
    let mut path_ignore_filters = Vec::new();
    let mut workflow_uses_secrets = BTreeSet::new();
    let cancel_in_progress = lines.iter().any(|(_, content)| {
        content
            .strip_prefix("cancel-in-progress:")
            .is_some_and(|rest| ci_yaml_unquote(rest).eq_ignore_ascii_case("true"))
    });
    let mut workflow_permissions = None;
    let mut yaml_jobs = Vec::new();
    let mut evidence_gaps = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let (indent, content) = &lines[index];
        ci_collect_secret_refs(content, &mut workflow_uses_secrets);
        if let Some(rest) = ci_workflow_permissions_value(*indent, content) {
            let (permissions, next) = ci_yaml_permissions_at(&lines, index, *indent, rest);
            workflow_permissions = permissions.or(workflow_permissions);
            index = next;
            continue;
        }
        let is_on_key = *indent == 0
            && (content == "on:"
                || content.starts_with("on: ")
                || content == "\"on\":"
                || content.starts_with("\"on\": "));
        if is_on_key {
            let value = content
                .split_once(':')
                .map(|(_, rest)| rest.trim())
                .unwrap_or("");
            if !value.is_empty() {
                match ci_yaml_inline_list(value) {
                    Some(items) => triggers.extend(items),
                    None => triggers.push(ci_yaml_unquote(value)),
                }
                index += 1;
                continue;
            }
            index += 1;
            let mut child_indent: Option<usize> = None;
            while index < lines.len() {
                let (line_indent, line_content) = &lines[index];
                if *line_indent == 0 {
                    break;
                }
                let trigger_indent = *child_indent.get_or_insert(*line_indent);
                if *line_indent != trigger_indent || line_content.starts_with('-') {
                    index += 1;
                    continue;
                }
                let key = ci_yaml_unquote(line_content.split(':').next().unwrap_or(""));
                let mut branches = Vec::new();
                let mut cursor = index + 1;
                while cursor < lines.len() && lines[cursor].0 > trigger_indent {
                    let (cursor_indent, cursor_content) = &lines[cursor];
                    if let Some(rest) = cursor_content.strip_prefix("branches:") {
                        let (items, next) = ci_yaml_list_at(&lines, cursor, *cursor_indent, rest);
                        branches.extend(items);
                        cursor = next;
                        continue;
                    }
                    if let Some(rest) = cursor_content.strip_prefix("paths-ignore:") {
                        let (items, next) = ci_yaml_list_at(&lines, cursor, *cursor_indent, rest);
                        path_ignore_filters.extend(items);
                        cursor = next;
                        continue;
                    }
                    if let Some(rest) = cursor_content.strip_prefix("paths:") {
                        let (items, next) = ci_yaml_list_at(&lines, cursor, *cursor_indent, rest);
                        path_filters.extend(items);
                        cursor = next;
                        continue;
                    }
                    cursor += 1;
                }
                if branches.is_empty() {
                    triggers.push(key);
                } else {
                    for branch in &branches {
                        triggers.push(format!("{key}:{branch}"));
                    }
                }
                index = cursor;
            }
            continue;
        }
        if *indent == 0 && content == "jobs:" {
            index += 1;
            let mut job_indent: Option<usize> = None;
            while index < lines.len() {
                let (line_indent, line_content) = &lines[index];
                if *line_indent == 0 {
                    break;
                }
                let expected = *job_indent.get_or_insert(*line_indent);
                if *line_indent != expected
                    || line_content.starts_with('-')
                    || !line_content.ends_with(':')
                {
                    index += 1;
                    continue;
                }
                let id = ci_yaml_unquote(line_content.trim_end_matches(':'));
                let mut name = None;
                let mut runs_on = Vec::new();
                let mut matrix_size = 1usize;
                let mut timeout_minutes = None;
                let mut uses = Vec::new();
                let mut permissions = None;
                let mut uses_secrets = BTreeSet::new();
                let mut cursor = index + 1;
                while cursor < lines.len() && lines[cursor].0 > expected {
                    let body = lines[cursor].1.as_str();
                    let body = body.strip_prefix("- ").unwrap_or(body);
                    ci_collect_secret_refs(body, &mut uses_secrets);
                    if let Some(rest) = body.strip_prefix("runs-on:") {
                        let (items, next) = ci_yaml_list_at(&lines, cursor, lines[cursor].0, rest);
                        runs_on.extend(items);
                        cursor = next;
                        continue;
                    } else if let Some(rest) = body.strip_prefix("strategy:") {
                        let (size, gaps, next) =
                            ci_yaml_strategy_matrix_size_at(&lines, cursor, lines[cursor].0, rest);
                        matrix_size = size;
                        for gap in gaps {
                            evidence_gaps.push(format!(
                                "{path} job `{id}` strategy.matrix: {gap}; matrix_size is best-effort"
                            ));
                        }
                        cursor = next;
                        continue;
                    } else if let Some(rest) = body.strip_prefix("timeout-minutes:") {
                        timeout_minutes = rest.trim().parse::<u64>().ok().or(timeout_minutes);
                    } else if let Some(rest) = body.strip_prefix("uses:") {
                        let reference = ci_yaml_unquote(rest);
                        if !reference.is_empty() {
                            uses.push(reference);
                        }
                    } else if let Some(rest) = body.strip_prefix("permissions:") {
                        let (value, next) =
                            ci_yaml_permissions_at(&lines, cursor, lines[cursor].0, rest);
                        permissions = value.or(permissions);
                        cursor = next;
                        continue;
                    } else if let Some(rest) = body.strip_prefix("secrets:")
                        && ci_yaml_unquote(rest).eq_ignore_ascii_case("inherit")
                    {
                        uses_secrets.insert("inherit".to_owned());
                    } else if name.is_none()
                        && lines[cursor].1.starts_with("name:")
                        && let Some(rest) = body.strip_prefix("name:")
                    {
                        let value = ci_yaml_unquote(rest);
                        if !value.is_empty() {
                            name = Some(value);
                        }
                    }
                    cursor += 1;
                }
                yaml_jobs.push(CiWorkflowYamlJob {
                    id,
                    name,
                    runs_on,
                    matrix_size,
                    timeout_minutes,
                    uses,
                    permissions,
                    uses_secrets: uses_secrets.into_iter().collect(),
                });
                index = cursor;
            }
            continue;
        }
        index += 1;
    }
    CiWorkflowScan {
        path: path.to_owned(),
        triggers,
        path_filters,
        path_ignore_filters,
        cancel_in_progress,
        permissions: workflow_permissions,
        uses_secrets: workflow_uses_secrets.into_iter().collect(),
        yaml_jobs,
        evidence_gaps,
    }
}

pub(crate) fn run_curl_json_get(
    scratch_dir: &Path,
    url: &str,
    auth_header: &str,
    headers: &[&str],
    timeout_sec: u64,
) -> Result<HttpPostOutput> {
    let stem = format!("get-{}", &sha256_hex(url.as_bytes())[..12]);
    let stdout_path = scratch_dir.join(format!("{stem}.curl.stdout.tmp"));
    let stderr_path = scratch_dir.join(format!("{stem}.curl.stderr.tmp"));
    let stdout =
        File::create(&stdout_path).with_context(|| format!("create {}", stdout_path.display()))?;
    let stderr =
        File::create(&stderr_path).with_context(|| format!("create {}", stderr_path.display()))?;
    let mut command = ProcessCommand::new("curl");
    command
        .arg("-sS")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg(timeout_sec.to_string())
        .arg("-w")
        .arg("\nUB_REVIEW_HTTP_STATUS:%{http_code}\n")
        .arg("-X")
        .arg("GET")
        .arg("-K")
        .arg("-")
        .arg(url)
        .current_dir(scratch_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            remove_output_files(&stdout_path, &stderr_path);
            return Err(err).with_context(|| "spawn curl");
        }
    };
    let write_config_result = (|| -> Result<()> {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("curl stdin unavailable"))?;
        use std::io::Write as _;
        for header in headers {
            writeln!(stdin, "header = \"{}\"", curl_config_quote(header))?;
        }
        writeln!(stdin, "header = \"{}\"", curl_config_quote(auth_header))?;
        Ok(())
    })();
    if let Err(err) = write_config_result {
        let _ = child.kill();
        let _ = child.wait();
        remove_output_files(&stdout_path, &stderr_path);
        return Err(err);
    }
    let output = wait_for_child_output_files(child, &stdout_path, &stderr_path, timeout_sec)
        .with_context(|| "wait for curl")?;
    let (stdout, http_status) = split_curl_http_status(output.stdout);
    Ok(HttpPostOutput {
        status: output.status,
        stdout,
        stderr: output.stderr,
        http_status,
    })
}

pub(crate) fn ci_github_get_json(
    args: &AuditCiArgs,
    token: &str,
    path_and_query: &str,
    scratch_dir: &Path,
) -> Result<serde_json::Value> {
    let url = format!(
        "{}/{}",
        args.github_api_url.trim_end_matches('/'),
        path_and_query
    );
    let output = run_curl_json_get(
        scratch_dir,
        &url,
        &format!("Authorization: Bearer {token}"),
        &[
            "Accept: application/vnd.github+json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    )
    .with_context(|| format!("GET {url}"))?;
    if !output.status.success() {
        bail!(
            "GitHub GET {} failed with exit code {:?} and http status {:?}: {}",
            url,
            output.status.code(),
            output.http_status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout).with_context(|| format!("parse GET {url} response"))
}

/// Parse the array under `key`, returning the deserialized items and the
/// count of items that failed to deserialize (dropped items become an
/// evidence-gap line, never a silent omission).
pub(crate) fn parse_ci_api_array<T: DeserializeOwned>(
    value: &serde_json::Value,
    key: &str,
) -> (Vec<T>, usize) {
    let Some(items) = value.get(key).and_then(serde_json::Value::as_array) else {
        return (Vec::new(), 0);
    };
    let mut parsed = Vec::with_capacity(items.len());
    let mut dropped = 0usize;
    for item in items {
        match serde_json::from_value(item.clone()) {
            Ok(value) => parsed.push(value),
            Err(_) => dropped += 1,
        }
    }
    (parsed, dropped)
}

pub(crate) fn ci_required_check_contexts_from_branch_protection(
    value: &serde_json::Value,
) -> BTreeSet<String> {
    let mut contexts = BTreeSet::new();
    if let Some(items) = value.get("contexts").and_then(serde_json::Value::as_array) {
        contexts.extend(
            items
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|context| !context.is_empty())
                .map(ToOwned::to_owned),
        );
    }
    if let Some(items) = value.get("checks").and_then(serde_json::Value::as_array) {
        contexts.extend(
            items
                .iter()
                .filter_map(|item| item.get("context").and_then(serde_json::Value::as_str))
                .map(str::trim)
                .filter(|context| !context.is_empty())
                .map(ToOwned::to_owned),
        );
    }
    contexts
}

pub(crate) fn ci_ref_pattern_matches_default_branch(pattern: &str, default_branch: &str) -> bool {
    let pattern = pattern.trim();
    if pattern.is_empty() {
        return false;
    }
    if matches!(pattern, "~DEFAULT_BRANCH" | "~ALL" | "*") {
        return true;
    }
    let default_ref = format!("refs/heads/{default_branch}");
    if pattern == default_branch || pattern == default_ref {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix('*') {
        return default_branch.starts_with(prefix) || default_ref.starts_with(prefix);
    }
    false
}

pub(crate) fn ci_ruleset_applies_to_default_branch(
    ruleset: &serde_json::Value,
    default_branch: &str,
) -> Option<bool> {
    match ruleset.get("target").and_then(serde_json::Value::as_str) {
        Some("branch") => {}
        Some(_) => return Some(false),
        None => return None,
    }
    match ruleset
        .get("enforcement")
        .and_then(serde_json::Value::as_str)
    {
        Some("active") => {}
        Some(_) => return Some(false),
        None => return None,
    }
    let ref_name = ruleset
        .get("conditions")
        .and_then(|conditions| conditions.get("ref_name"))?;
    let includes: Vec<&str> = ref_name
        .get("include")
        .and_then(serde_json::Value::as_array)?
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    let excludes: Vec<&str> = ref_name
        .get("exclude")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .collect()
        })
        .unwrap_or_default();
    if excludes
        .iter()
        .any(|pattern| ci_ref_pattern_matches_default_branch(pattern, default_branch))
    {
        return Some(false);
    }
    Some(
        includes
            .iter()
            .any(|pattern| ci_ref_pattern_matches_default_branch(pattern, default_branch)),
    )
}

pub(crate) fn ci_required_check_contexts_from_rulesets(
    value: &serde_json::Value,
    default_branch: &str,
) -> (BTreeSet<String>, Vec<String>) {
    let mut contexts = BTreeSet::new();
    let mut gaps = Vec::new();
    let Some(rulesets) = value.as_array() else {
        gaps.push("ruleset required checks unreadable: response was not an array".to_owned());
        return (contexts, gaps);
    };
    for ruleset in rulesets {
        let Some(rules) = ruleset.get("rules").and_then(serde_json::Value::as_array) else {
            gaps.push(
                "ruleset required checks unreadable: rulesets response omitted rule details"
                    .to_owned(),
            );
            continue;
        };
        let has_required_status_checks = rules.iter().any(|rule| {
            rule.get("type").and_then(serde_json::Value::as_str) == Some("required_status_checks")
        });
        if !has_required_status_checks {
            continue;
        }
        match ci_ruleset_applies_to_default_branch(ruleset, default_branch) {
            Some(true) => {}
            Some(false) => continue,
            None => {
                gaps.push(
                    "ruleset required checks unreadable: active branch ruleset did not prove default-branch applicability"
                        .to_owned(),
                );
                continue;
            }
        }
        for rule in rules {
            if rule.get("type").and_then(serde_json::Value::as_str)
                != Some("required_status_checks")
            {
                continue;
            }
            let Some(required) = rule
                .get("parameters")
                .and_then(|parameters| parameters.get("required_status_checks"))
                .and_then(serde_json::Value::as_array)
            else {
                gaps.push(
                    "ruleset required checks unreadable: required_status_checks parameters missing"
                        .to_owned(),
                );
                continue;
            };
            contexts.extend(
                required
                    .iter()
                    .filter_map(|item| item.get("context").and_then(serde_json::Value::as_str))
                    .map(str::trim)
                    .filter(|context| !context.is_empty())
                    .map(ToOwned::to_owned),
            );
        }
    }
    (contexts, gaps)
}

pub(crate) fn fetch_ci_required_checks(
    args: &AuditCiArgs,
    repo: &str,
    token: &str,
    scratch_dir: &Path,
) -> CiRequiredChecks {
    let mut required = CiRequiredChecks::default();
    let repo_meta = match ci_github_get_json(args, token, &format!("repos/{repo}"), scratch_dir) {
        Ok(value) => value,
        Err(err) => {
            required.evidence_gaps.push(format!(
                "required-check status unreadable: repo metadata unavailable: {err:#}"
            ));
            return required;
        }
    };
    let Some(default_branch) = repo_meta
        .get("default_branch")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|branch| !branch.is_empty())
    else {
        required.evidence_gaps.push(
            "required-check status unreadable: repo metadata omitted default_branch".to_owned(),
        );
        return required;
    };
    let encoded_branch = percent_encode_query(default_branch);
    let mut branch_protection_readable = false;
    let mut rulesets_readable = false;
    match ci_github_get_json(
        args,
        token,
        &format!("repos/{repo}/branches/{encoded_branch}/protection/required_status_checks"),
        scratch_dir,
    ) {
        Ok(value) => {
            branch_protection_readable = true;
            for context in ci_required_check_contexts_from_branch_protection(&value) {
                required
                    .contexts
                    .entry(context)
                    .or_insert_with(|| "branch-protection".to_owned());
            }
        }
        Err(err) => required.evidence_gaps.push(format!(
            "branch-protection required checks unreadable for `{default_branch}`: {err:#}"
        )),
    }
    match ci_github_get_json(
        args,
        token,
        &format!("repos/{repo}/rulesets?includes_parents=true&per_page=100"),
        scratch_dir,
    ) {
        Ok(value) => {
            let (contexts, gaps) = ci_required_check_contexts_from_rulesets(&value, default_branch);
            rulesets_readable = gaps.is_empty();
            required.evidence_gaps.extend(gaps);
            for context in contexts {
                required
                    .contexts
                    .entry(context)
                    .or_insert_with(|| "ruleset".to_owned());
            }
        }
        Err(err) => required.evidence_gaps.push(format!(
            "ruleset required checks unreadable for `{default_branch}`: {err:#}"
        )),
    }
    if branch_protection_readable && rulesets_readable {
        required.default_source = Some("branch-protection".to_owned());
    } else if !branch_protection_readable && !rulesets_readable {
        required.default_source = None;
    }
    required
}

pub(crate) fn fetch_ci_audit_history(
    args: &AuditCiArgs,
    repo: &str,
    token: &str,
    scratch_dir: &Path,
) -> Result<CiAuditFetch> {
    let workflows_value = ci_github_get_json(
        args,
        token,
        &format!("repos/{repo}/actions/workflows?per_page=100"),
        scratch_dir,
    )?;
    let (workflows, mut dropped_items): (Vec<CiApiWorkflow>, usize) =
        parse_ci_api_array(&workflows_value, "workflows");
    let since = (Utc::now() - chrono::Duration::days(i64::from(args.window_days)))
        .format("%Y-%m-%d")
        .to_string();
    let mut runs: Vec<CiApiRun> = Vec::new();
    let mut pages_fetched = 0;
    let mut truncated = false;
    for page in 1..=CI_AUDIT_RUN_PAGE_CAP {
        let value = ci_github_get_json(
            args,
            token,
            &format!(
                "repos/{repo}/actions/runs?event=pull_request&per_page={CI_AUDIT_RUNS_PER_PAGE}&page={page}&created=%3E%3D{since}"
            ),
            scratch_dir,
        )?;
        let (batch, dropped): (Vec<CiApiRun>, usize) = parse_ci_api_array(&value, "workflow_runs");
        dropped_items += dropped;
        pages_fetched = page;
        let batch_len = batch.len();
        runs.extend(batch);
        if batch_len < CI_AUDIT_RUNS_PER_PAGE {
            break;
        }
        if page == CI_AUDIT_RUN_PAGE_CAP {
            truncated = true;
        }
    }
    let mut evidence_gaps = Vec::new();
    let mut jobs_fetch_failures = 0usize;
    let mut jobs_truncated_runs = 0usize;
    let mut runs_with_jobs = Vec::new();
    for run in runs {
        match ci_github_get_json(
            args,
            token,
            &format!("repos/{repo}/actions/runs/{}/jobs?per_page=100", run.id),
            scratch_dir,
        ) {
            Ok(value) => {
                let total = value
                    .get("total_count")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(0);
                if total > 100 {
                    jobs_truncated_runs += 1;
                }
                let (jobs, dropped): (Vec<CiApiJob>, usize) = parse_ci_api_array(&value, "jobs");
                dropped_items += dropped;
                runs_with_jobs.push(CiRunWithJobs { run, jobs });
            }
            Err(err) => {
                jobs_fetch_failures += 1;
                if jobs_fetch_failures <= 3 {
                    evidence_gaps.push(format!("jobs unavailable for run {}: {err:#}", run.id));
                }
            }
        }
    }
    if jobs_fetch_failures > 3 {
        evidence_gaps.push(format!(
            "jobs unavailable for {jobs_fetch_failures} runs in total"
        ));
    }
    if jobs_truncated_runs > 0 {
        evidence_gaps.push(format!(
            "{jobs_truncated_runs} runs had more than 100 jobs; only the first page of jobs was fetched"
        ));
    }
    if dropped_items > 0 {
        evidence_gaps.push(format!(
            "{dropped_items} API items failed to deserialize and were dropped from history"
        ));
    }
    let required_checks = fetch_ci_required_checks(args, repo, token, scratch_dir);
    Ok(CiAuditFetch {
        workflows,
        runs: runs_with_jobs,
        pages_fetched,
        truncated,
        required_checks,
        evidence_gaps,
    })
}

pub(crate) fn ci_percentile(sorted: &[u64], pct: f64) -> Option<u64> {
    if sorted.is_empty() {
        return None;
    }
    let rank = ((pct / 100.0) * sorted.len() as f64).ceil() as usize;
    let index = rank.clamp(1, sorted.len()) - 1;
    sorted.get(index).copied()
}

pub(crate) fn ci_round_rate(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        return 0.0;
    }
    let rate = numerator as f64 / denominator as f64;
    (rate * 10_000.0).round() / 10_000.0
}

pub(crate) fn ci_conclusion_blocks_independence(conclusion: Option<&str>) -> bool {
    !matches!(
        conclusion,
        Some("success") | Some("skipped") | Some("neutral")
    )
}

/// Compute per-job stats over PR runs, including the independent
/// merge-decision signal: a failure counts as independent when every cheaper
/// job (lower duration p50, tie-broken by name) present in the same workflow
/// run passed (skipped counts as passed).
pub(crate) fn compute_ci_job_stats(
    workflows: &[CiApiWorkflow],
    runs: &[CiRunWithJobs],
) -> Vec<CiJobStats> {
    let workflow_meta: BTreeMap<u64, (&str, &str)> = workflows
        .iter()
        .map(|workflow| {
            (
                workflow.id,
                (workflow.path.as_str(), workflow.name.as_str()),
            )
        })
        .collect();
    let mut durations: BTreeMap<(u64, String), Vec<u64>> = BTreeMap::new();
    let mut run_counts: BTreeMap<(u64, String), usize> = BTreeMap::new();
    let mut failure_counts: BTreeMap<(u64, String), usize> = BTreeMap::new();
    let mut cancel_counts: BTreeMap<(u64, String), usize> = BTreeMap::new();
    let mut cancelled_without_completion_counts: BTreeMap<(u64, String), usize> = BTreeMap::new();
    for entry in runs {
        if entry.run.event != "pull_request" {
            continue;
        }
        for job in &entry.jobs {
            let key = (entry.run.workflow_id, job.name.clone());
            *run_counts.entry(key.clone()).or_default() += 1;
            match job.conclusion.as_deref() {
                Some("failure") => *failure_counts.entry(key.clone()).or_default() += 1,
                Some("cancelled") => {
                    *cancel_counts.entry(key.clone()).or_default() += 1;
                    if job.started_at.is_some() && job.completed_at.is_none() {
                        *cancelled_without_completion_counts
                            .entry(key.clone())
                            .or_default() += 1;
                    }
                }
                _ => {}
            }
            if let (Some(started), Some(completed)) = (job.started_at, job.completed_at) {
                let seconds = (completed - started).num_seconds();
                if seconds >= 0 {
                    durations.entry(key).or_default().push(seconds as u64);
                }
            }
        }
    }
    let mut p50: BTreeMap<(u64, String), Option<u64>> = BTreeMap::new();
    for (key, values) in &mut durations {
        values.sort_unstable();
        p50.insert(key.clone(), ci_percentile(values, 50.0));
    }
    let order_key = |workflow_id: u64, name: &str| -> (u64, String) {
        let median = p50
            .get(&(workflow_id, name.to_owned()))
            .copied()
            .flatten()
            .unwrap_or(u64::MAX);
        (median, name.to_owned())
    };
    let mut independent: BTreeMap<(u64, String), usize> = BTreeMap::new();
    let mut co_failing: BTreeMap<(u64, String), BTreeSet<String>> = BTreeMap::new();
    let mut rerun_then_pass: BTreeMap<u64, usize> = BTreeMap::new();
    for entry in runs {
        if entry.run.event != "pull_request" {
            continue;
        }
        if entry.run.run_attempt > 1 && entry.run.conclusion.as_deref() == Some("success") {
            *rerun_then_pass.entry(entry.run.workflow_id).or_default() += 1;
        }
        let mut conclusion_by_name: BTreeMap<&str, Option<&str>> = BTreeMap::new();
        for job in &entry.jobs {
            conclusion_by_name.insert(job.name.as_str(), job.conclusion.as_deref());
        }
        for (name, conclusion) in &conclusion_by_name {
            if *conclusion != Some("failure") {
                continue;
            }
            let own_order = order_key(entry.run.workflow_id, name);
            let blocked = conclusion_by_name.iter().any(|(other, other_conclusion)| {
                other != name
                    && order_key(entry.run.workflow_id, other) < own_order
                    && ci_conclusion_blocks_independence(*other_conclusion)
            });
            let key = (entry.run.workflow_id, (*name).to_owned());
            if !blocked {
                *independent.entry(key.clone()).or_default() += 1;
            }
            let co_set = co_failing.entry(key).or_default();
            for (other, other_conclusion) in &conclusion_by_name {
                if other != name && *other_conclusion == Some("failure") {
                    co_set.insert((*other).to_owned());
                }
            }
        }
    }
    let mut stats = Vec::new();
    for (key, runs_count) in &run_counts {
        let (workflow_id, job_name) = key;
        let (workflow_path, workflow_name) =
            workflow_meta.get(workflow_id).copied().unwrap_or(("", ""));
        let workflow_path = if workflow_path.is_empty() {
            format!("unknown-workflow-{workflow_id}")
        } else {
            workflow_path.to_owned()
        };
        let sorted = durations.get(key).cloned().unwrap_or_default();
        let own_order = order_key(*workflow_id, job_name);
        let cheaper: Vec<String> = run_counts
            .keys()
            .filter(|(other_workflow, other_name)| {
                other_workflow == workflow_id
                    && other_name != job_name
                    && order_key(*other_workflow, other_name) < own_order
            })
            .map(|(_, other_name)| other_name.clone())
            .collect();
        stats.push(CiJobStats {
            workflow_path,
            workflow_name: workflow_name.to_owned(),
            job: job_name.clone(),
            runs: *runs_count,
            failures: failure_counts.get(key).copied().unwrap_or(0),
            cancellations: cancel_counts.get(key).copied().unwrap_or(0),
            cancelled_started_without_completion: cancelled_without_completion_counts
                .get(key)
                .copied()
                .unwrap_or(0),
            duration_p50_sec: ci_percentile(&sorted, 50.0),
            duration_p90_sec: ci_percentile(&sorted, 90.0),
            duration_p99_sec: ci_percentile(&sorted, 99.0),
            total_duration_sec: sorted.iter().sum(),
            independent_failures: independent.get(key).copied().unwrap_or(0),
            co_failing_jobs: co_failing
                .get(key)
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default(),
            cheaper_jobs_compared: cheaper,
            rerun_then_pass: rerun_then_pass.get(workflow_id).copied().unwrap_or(0),
        });
    }
    stats.sort_by(|a, b| {
        (a.workflow_path.as_str(), a.job.as_str()).cmp(&(b.workflow_path.as_str(), b.job.as_str()))
    });
    stats
}

pub(crate) fn ci_security_pattern_match(candidates: &[&str]) -> Option<String> {
    for pattern in CI_AUDIT_SECURITY_PATTERNS {
        for candidate in candidates {
            if candidate.to_ascii_lowercase().contains(pattern) {
                return Some((*pattern).to_owned());
            }
        }
    }
    None
}

pub(crate) fn ci_permissions_risk(permissions: Option<&serde_json::Value>) -> Option<&'static str> {
    let Some(value) = permissions else {
        return Some("missing permissions");
    };
    match value {
        serde_json::Value::Null => Some("ambiguous permissions"),
        serde_json::Value::String(value) => {
            let lower = value.trim().to_ascii_lowercase();
            match lower.as_str() {
                "read-all" | "none" => None,
                "write-all" => Some("write-scoped permissions"),
                "" => Some("ambiguous permissions"),
                _ if lower.contains("write") => Some("write-scoped permissions"),
                _ => Some("ambiguous permissions"),
            }
        }
        serde_json::Value::Object(map) => {
            let mut ambiguous = false;
            for value in map.values() {
                match value {
                    serde_json::Value::String(scope) => {
                        let lower = scope.trim().to_ascii_lowercase();
                        match lower.as_str() {
                            "read" | "none" => {}
                            "write" => return Some("write-scoped permissions"),
                            "" => ambiguous = true,
                            _ if lower.contains("write") => {
                                return Some("write-scoped permissions");
                            }
                            _ => ambiguous = true,
                        }
                    }
                    serde_json::Value::Null => {}
                    _ => ambiguous = true,
                }
            }
            ambiguous.then_some("ambiguous permissions")
        }
        _ => Some("ambiguous permissions"),
    }
}

#[derive(Debug)]
pub(crate) struct CiTierEvidence {
    pub(crate) job: String,
    pub(crate) workflow_path: String,
    pub(crate) workflow_name: String,
    pub(crate) uses: Vec<String>,
    pub(crate) permissions: Option<serde_json::Value>,
    pub(crate) uses_secrets: Vec<String>,
    pub(crate) triggers: Vec<String>,
    pub(crate) path_filters: Vec<String>,
    pub(crate) required_check: Option<bool>,
    pub(crate) required_check_source: String,
    pub(crate) required_check_context: Option<String>,
    pub(crate) history_available: bool,
    pub(crate) runs: usize,
    pub(crate) independent_failures: usize,
    pub(crate) duration_p50_sec: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct CiTierDecision {
    pub(crate) tier: &'static str,
    pub(crate) confidence: &'static str,
    pub(crate) reason: String,
    /// Short report-line fragment. Must not repeat run counts, p50, or
    /// independent-failure counts that the report line already prints; empty
    /// when the report header already carries the context (tokenless mode).
    report_note: String,
    pub(crate) proposed_policy: String,
}

/// Deterministic v0 tier rules (docs/CI_AUDIT_WIZARD.md). Conservative:
/// security-sensitive jobs are always flag-for-human, insufficient history is
/// never auto-right-sized, nothing becomes optional below adaptive, and
/// absence of failures alone caps confidence at low.
pub(crate) fn classify_ci_job_tier(
    evidence: &CiTierEvidence,
    sibling_has_independent_signal: bool,
) -> CiTierDecision {
    let security_candidates: Vec<&str> = std::iter::once(evidence.job.as_str())
        .chain(std::iter::once(evidence.workflow_path.as_str()))
        .chain(std::iter::once(evidence.workflow_name.as_str()))
        .chain(evidence.uses.iter().map(String::as_str))
        .collect();
    if let Some(pattern) = ci_security_pattern_match(&security_candidates) {
        return CiTierDecision {
            tier: "flag-for-human",
            confidence: "high",
            reason: format!("security-sensitive: matched `{pattern}`; never auto-right-sized"),
            report_note: format!(
                "security-sensitive (matched `{pattern}`); never auto-right-sized"
            ),
            proposed_policy: "human decision required; not auto-right-sized".to_owned(),
        };
    }
    if let Some(permission_risk) = ci_permissions_risk(evidence.permissions.as_ref()) {
        return CiTierDecision {
            tier: "flag-for-human",
            confidence: "high",
            reason: format!("{permission_risk}; never auto-right-sized"),
            report_note: format!("{permission_risk}; never auto-right-sized"),
            proposed_policy: "human decision required; not auto-right-sized".to_owned(),
        };
    }
    if !evidence.uses_secrets.is_empty() {
        let summary = evidence
            .uses_secrets
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return CiTierDecision {
            tier: "flag-for-human",
            confidence: "high",
            reason: format!("uses workflow secret reference(s): {summary}; never auto-right-sized"),
            report_note: "uses workflow secrets; never auto-right-sized".to_owned(),
            proposed_policy: "human decision required; not auto-right-sized".to_owned(),
        };
    }
    if !evidence.history_available {
        return CiTierDecision {
            tier: "flag-for-human",
            confidence: "low",
            reason: "no run history: tokenless inventory-only mode".to_owned(),
            // Empty on purpose: the report header already states
            // inventory-only mode once for every job.
            report_note: String::new(),
            proposed_policy: "human decision required; rerun audit-ci with a token".to_owned(),
        };
    }
    if evidence.runs < CI_AUDIT_MIN_HISTORY_RUNS {
        return CiTierDecision {
            tier: "flag-for-human",
            confidence: "low",
            reason: format!(
                "insufficient history: {} runs (< {CI_AUDIT_MIN_HISTORY_RUNS}) in window; never adaptive on thin data",
                evidence.runs
            ),
            report_note: format!(
                "below the {CI_AUDIT_MIN_HISTORY_RUNS}-run history floor; never adaptive on thin data"
            ),
            proposed_policy: "human decision required; not enough history to right-size".to_owned(),
        };
    }
    let Some(p50) = evidence.duration_p50_sec else {
        return CiTierDecision {
            tier: "flag-for-human",
            confidence: "low",
            reason: "duration evidence missing despite run history".to_owned(),
            report_note: "duration receipts incomplete despite run history".to_owned(),
            proposed_policy: "human decision required; duration receipts incomplete".to_owned(),
        };
    };
    let proven_required_check = ci_proven_required_check(evidence);
    if evidence.independent_failures > 0 {
        let confidence =
            if p50 < CI_AUDIT_CHEAP_P50_SEC && evidence.runs >= CI_AUDIT_MEDIUM_VOLUME_RUNS {
                "high"
            } else {
                "medium"
            };
        if proven_required_check {
            return CiTierDecision {
                tier: "move-to-ub-review-required",
                confidence,
                reason: format!(
                    "{} independent failures in {} runs; already required as `{}` so fold into ub-review/gate without weakening the gate",
                    evidence.independent_failures,
                    evidence.runs,
                    evidence.required_check_context.as_deref().unwrap_or(&evidence.job)
                ),
                report_note: "already required; fold into ub-review/gate as required proof"
                    .to_owned(),
                proposed_policy:
                    "[[proof.required]] inside ub-review/gate with required = true; remove the old required context only after red/green proof"
                        .to_owned(),
            };
        }
        return CiTierDecision {
            tier: "keep-required",
            confidence,
            reason: format!(
                "{} independent failures in {} runs; earns its required slot",
                evidence.independent_failures, evidence.runs
            ),
            report_note: "earns its required slot".to_owned(),
            proposed_policy: "keep as a standalone required check".to_owned(),
        };
    }
    if p50 >= CI_AUDIT_CHEAP_P50_SEC {
        let confidence =
            if evidence.runs >= CI_AUDIT_HIGH_VOLUME_RUNS && sibling_has_independent_signal {
                "medium"
            } else {
                "low"
            };
        let coverage = if sibling_has_independent_signal {
            "a cheaper sibling job has proven independent signal"
        } else {
            "no sibling job has proven independent signal; confidence capped at low"
        };
        return CiTierDecision {
            tier: "adaptive",
            confidence,
            reason: format!(
                "0 independent failures in {} runs at p50 {}; absence of failures alone is weak evidence ({coverage})",
                evidence.runs,
                format_ci_duration(p50)
            ),
            report_note: if sibling_has_independent_signal {
                "no independent signal; a cheaper sibling job has proven coverage".to_owned()
            } else {
                "no independent signal and no proven sibling coverage; confidence capped at low"
                    .to_owned()
            },
            proposed_policy:
                "[[proof.required]] gated on matching diff classes/paths; skip on unrelated diffs"
                    .to_owned(),
        };
    }
    if proven_required_check {
        return CiTierDecision {
            tier: "move-to-ub-review-required",
            confidence: "low",
            reason: format!(
                "cheap (p50 {} < 5m) with 0 independent failures in {} runs; already required as `{}` so fold into ub-review/gate without weakening the gate",
                format_ci_duration(p50),
                evidence.runs,
                evidence.required_check_context.as_deref().unwrap_or(&evidence.job)
            ),
            report_note: "already required; fold into ub-review/gate as required proof".to_owned(),
            proposed_policy:
                "[[proof.required]] inside ub-review/gate with required = true; remove the old required context only after red/green proof"
                    .to_owned(),
        };
    }
    CiTierDecision {
        tier: "keep-required",
        confidence: "low",
        reason: format!(
            "cheap (p50 {} < 5m) with 0 independent failures in {} runs; keeping costs little and absence of failures caps confidence",
            format_ci_duration(p50),
            evidence.runs
        ),
        report_note:
            "under the 5-minute cheap threshold with no independent signal; keeping costs little"
                .to_owned(),
        proposed_policy: "keep as a standalone required check".to_owned(),
    }
}

pub(crate) fn ci_proven_required_check(evidence: &CiTierEvidence) -> bool {
    evidence.required_check == Some(true)
        && evidence.required_check_source != "unknown"
        && evidence.required_check_context.is_some()
}

pub(crate) fn ci_positioned_to_catch(evidence: &CiTierEvidence) -> String {
    let triggers = if evidence.triggers.is_empty() {
        "unknown".to_owned()
    } else {
        evidence.triggers.join(", ")
    };
    let paths = if evidence.path_filters.is_empty() {
        "all paths".to_owned()
    } else {
        evidence.path_filters.join(", ")
    };
    format!(
        "triggers: {triggers}; paths: {paths}; scope inferred from job name `{}`",
        evidence.job
    )
}

pub(crate) fn format_ci_duration(seconds: u64) -> String {
    if seconds >= 60 {
        format!("{}m{:02}s", seconds / 60, seconds % 60)
    } else {
        format!("{seconds}s")
    }
}

pub(crate) fn ci_match_yaml_job<'a>(
    scan: Option<&'a CiWorkflowScan>,
    observed: &str,
) -> Option<&'a CiWorkflowYamlJob> {
    let scan = scan?;
    let observed_lower = observed.to_ascii_lowercase();
    scan.yaml_jobs.iter().find(|job| {
        std::iter::once(job.id.as_str())
            .chain(job.name.as_deref())
            .any(|candidate| {
                let candidate_lower = candidate.to_ascii_lowercase();
                observed_lower == candidate_lower
                    || observed_lower.starts_with(&format!("{candidate_lower} ("))
            })
    })
}

pub(crate) fn ci_required_check_status(
    required: Option<&CiRequiredChecks>,
    candidates: &[&str],
) -> (Option<bool>, String, Option<String>) {
    let Some(required) = required else {
        return (None, "unknown".to_owned(), None);
    };
    for candidate in candidates {
        let trimmed = candidate.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(source) = required.contexts.get(trimmed) {
            return (Some(true), source.clone(), Some(trimmed.to_owned()));
        }
    }
    match &required.default_source {
        Some(source) => (Some(false), source.clone(), None),
        None => (None, "unknown".to_owned(), None),
    }
}

pub(crate) fn ci_job_github_hosted(labels: &[String]) -> bool {
    let labels: Vec<String> = labels
        .iter()
        .map(|label| label.to_ascii_lowercase())
        .collect();
    if labels.iter().any(|label| label == "self-hosted") {
        return false;
    }
    labels.iter().any(|label| {
        label == "ubuntu-latest"
            || label == "windows-latest"
            || label == "macos-latest"
            || label.starts_with("ubuntu-")
            || label.starts_with("windows-")
            || label.starts_with("macos-")
    })
}

pub(crate) fn ci_runner_cancellation_for_job(
    stat: &CiJobStats,
    scan: Option<&CiWorkflowScan>,
    yaml: Option<&CiWorkflowYamlJob>,
    audit_cancel_events: Option<usize>,
) -> Option<CiRunnerCancellation> {
    if stat.cancellations == 0 {
        return None;
    }
    let github_hosted = yaml
        .map(|yaml| ci_job_github_hosted(&yaml.runs_on))
        .unwrap_or(false);
    let runner_shutdown_signal = stat.cancelled_started_without_completion > 0;
    let cancel_in_progress = scan.is_some_and(|scan| scan.cancel_in_progress);
    let classification = if cancel_in_progress {
        "cancelled_superseded"
    } else if audit_cancel_events == Some(0) && github_hosted && runner_shutdown_signal {
        "runner_eviction_suspected"
    } else if github_hosted && stat.cancellations >= 3 {
        "unavailable_repeated"
    } else {
        "unknown"
    };
    let suggested_action = match classification {
        "cancelled_superseded" => {
            "inspect the newer run for the same PR/head; do not treat this cancellation as code evidence"
        }
        "runner_eviction_suspected" => "rerun on self-hosted or cx profile",
        "unavailable_repeated" => {
            "check audit-log cancellation events and runner shutdown markers, then rerun on self-hosted or cx profile if infrastructure cancellation repeats"
        }
        _ => {
            "inspect Actions run metadata, audit-log cancellation events, and runner shutdown markers before treating this as code evidence"
        }
    };
    let mut evidence = Vec::new();
    if cancel_in_progress {
        evidence.push("workflow has cancel-in-progress: true".to_owned());
    }
    if runner_shutdown_signal {
        evidence.push(format!(
            "{} cancelled job(s) started without a completed_at timestamp",
            stat.cancelled_started_without_completion
        ));
    }
    if github_hosted {
        evidence.push("runs-on matches GitHub-hosted runner labels".to_owned());
    }
    let mut evidence_gaps = Vec::new();
    if audit_cancel_events.is_none() {
        evidence_gaps.push(
            "audit-log cancellation event count not supplied; pass --audit-cancel-events after a read-only audit-log check"
                .to_owned(),
        );
    }
    if yaml.is_none() {
        evidence_gaps
            .push("workflow YAML job not matched to API job; runner labels unknown".to_owned());
    }
    if !runner_shutdown_signal {
        evidence_gaps.push("runner shutdown signal not observed in job timestamps".to_owned());
    }
    Some(CiRunnerCancellation {
        classification: classification.to_owned(),
        workflow: stat.workflow_path.clone(),
        workflow_name: stat.workflow_name.clone(),
        job: stat.job.clone(),
        runs: stat.runs,
        cancellations: stat.cancellations,
        cancellation_rate: ci_round_rate(stat.cancellations, stat.runs),
        audit_cancel_events,
        runner_shutdown_signal,
        github_hosted,
        runner_labels: yaml.map(|yaml| yaml.runs_on.clone()).unwrap_or_default(),
        suggested_action: suggested_action.to_owned(),
        receipts: vec![format!("ci-audit/history.json#{}", stat.job)],
        evidence,
        evidence_gaps,
    })
}

pub(crate) fn build_ci_audit_artifacts(
    repo: &str,
    window_days: u32,
    scans: &[CiWorkflowScan],
    fetch: Option<&CiAuditFetch>,
    audit_cancel_events: Option<usize>,
    generated_at: DateTime<Utc>,
) -> CiAuditArtifacts {
    let scan_by_path: BTreeMap<&str, &CiWorkflowScan> = scans
        .iter()
        .map(|scan| (scan.path.as_str(), scan))
        .collect();
    let stats = fetch
        .map(|fetch| compute_ci_job_stats(&fetch.workflows, &fetch.runs))
        .unwrap_or_default();
    let mut inventory_gaps: Vec<String> = scans
        .iter()
        .flat_map(|scan| scan.evidence_gaps.iter().cloned())
        .collect();
    let required_checks = fetch.map(|fetch| &fetch.required_checks);
    match required_checks {
        Some(required) if required.default_source.is_some() => {
            inventory_gaps.extend(required.evidence_gaps.iter().cloned());
        }
        Some(required) => {
            inventory_gaps.push(
                "required-check status unknown: branch protection and rulesets unreadable"
                    .to_owned(),
            );
            inventory_gaps.extend(required.evidence_gaps.iter().cloned());
        }
        None => {
            inventory_gaps.push(
                "required-check status unknown: branch protection and rulesets need a GitHub token"
                    .to_owned(),
            );
        }
    }
    // Job universe: observed API jobs plus yaml-declared jobs that never ran on
    // pull_request events in the window (release/deploy lanes must still show
    // up so the security rule can flag them).
    struct CiJobSeed<'a> {
        workflow_path: String,
        workflow_name: String,
        job: String,
        stats: Option<&'a CiJobStats>,
        yaml: Option<&'a CiWorkflowYamlJob>,
    }
    let mut seeds: Vec<CiJobSeed> = Vec::new();
    let mut seen: BTreeSet<(String, String)> = BTreeSet::new();
    for stat in &stats {
        let scan = scan_by_path.get(stat.workflow_path.as_str()).copied();
        if scan.is_none()
            && !stat.workflow_path.is_empty()
            && !inventory_gaps
                .iter()
                .any(|gap| gap.contains(&stat.workflow_path))
        {
            inventory_gaps.push(format!(
                "workflow file not found locally: {}; triggers and paths unknown",
                stat.workflow_path
            ));
        }
        seen.insert((stat.workflow_path.clone(), stat.job.clone()));
        seeds.push(CiJobSeed {
            workflow_path: stat.workflow_path.clone(),
            workflow_name: stat.workflow_name.clone(),
            job: stat.job.clone(),
            stats: Some(stat),
            yaml: ci_match_yaml_job(scan, &stat.job),
        });
    }
    for scan in scans {
        for yaml_job in &scan.yaml_jobs {
            let already_observed = stats.iter().any(|stat| {
                stat.workflow_path == scan.path
                    && ci_match_yaml_job(Some(scan), &stat.job)
                        .is_some_and(|matched| matched.id == yaml_job.id)
            });
            if already_observed {
                continue;
            }
            let key = (scan.path.clone(), yaml_job.id.clone());
            if !seen.insert(key) {
                continue;
            }
            seeds.push(CiJobSeed {
                workflow_path: scan.path.clone(),
                workflow_name: String::new(),
                job: yaml_job.id.clone(),
                stats: None,
                yaml: Some(yaml_job),
            });
        }
    }
    seeds.sort_by(|a, b| {
        (a.workflow_path.as_str(), a.job.as_str()).cmp(&(b.workflow_path.as_str(), b.job.as_str()))
    });
    let history_available = fetch.is_some();
    const CI_NO_TOKEN_GAP: &str = "no GitHub token: run history, durations, and correlation unavailable (inventory-only mode)";
    // Degradations that make every derived artifact incomplete are mirrored
    // into history, costs, and correlation so each artifact is honest when
    // read standalone.
    let mut shared_degradation_gaps: Vec<String> = Vec::new();
    match fetch {
        Some(fetch) => {
            if fetch.truncated {
                shared_degradation_gaps.push(format!(
                    "run history truncated at {CI_AUDIT_RUN_PAGE_CAP} pages / {} runs",
                    CI_AUDIT_RUN_PAGE_CAP * CI_AUDIT_RUNS_PER_PAGE
                ));
            }
        }
        None => shared_degradation_gaps.push(CI_NO_TOKEN_GAP.to_owned()),
    }
    if !history_available {
        inventory_gaps.push(CI_NO_TOKEN_GAP.to_owned());
    }
    let mut history_gaps: Vec<String> = Vec::new();
    if let Some(fetch) = fetch {
        history_gaps.extend(fetch.evidence_gaps.iter().cloned());
        history_gaps.push(
            "flake_rate and rerun_then_pass are workflow-level approximations (jobs fetched for the latest attempt only)"
                .to_owned(),
        );
    }
    history_gaps.extend(shared_degradation_gaps.iter().cloned());
    let mut inventory_jobs = Vec::new();
    let mut history_jobs = Vec::new();
    let mut costs_jobs = Vec::new();
    let mut correlation_jobs = Vec::new();
    let mut recommendations = Vec::new();
    let mut runner_cancellations = Vec::new();
    let workflow_has_independent_signal: BTreeSet<&str> = stats
        .iter()
        .filter(|stat| stat.independent_failures > 0)
        .map(|stat| stat.workflow_path.as_str())
        .collect();
    for seed in &seeds {
        let scan = scan_by_path.get(seed.workflow_path.as_str()).copied();
        let triggers = scan.map(|scan| scan.triggers.clone()).unwrap_or_default();
        let path_filters = scan
            .map(|scan| scan.path_filters.clone())
            .unwrap_or_default();
        let matrix_size = match seed.yaml {
            Some(yaml_job) => yaml_job.matrix_size.max(
                stats
                    .iter()
                    .filter(|stat| {
                        stat.workflow_path == seed.workflow_path
                            && ci_match_yaml_job(scan, &stat.job)
                                .is_some_and(|matched| matched.id == yaml_job.id)
                    })
                    .count()
                    .max(1),
            ),
            None => 1,
        };
        // Observed API job names are already display names; the YAML `name:`
        // fills in only for jobs that never ran in the window (where `job` is
        // the raw YAML job id).
        let display_name = match (seed.stats, seed.yaml) {
            (None, Some(yaml_job)) => yaml_job.name.clone().unwrap_or_else(|| seed.job.clone()),
            _ => seed.job.clone(),
        };
        let mut required_check_candidates = vec![seed.job.as_str(), display_name.as_str()];
        if let Some(yaml_job) = seed.yaml {
            required_check_candidates.push(yaml_job.id.as_str());
            if let Some(name) = yaml_job.name.as_deref() {
                required_check_candidates.push(name);
            }
        }
        let (required_check, required_check_source, required_check_context) =
            ci_required_check_status(required_checks, &required_check_candidates);
        let permissions = seed
            .yaml
            .and_then(|yaml_job| yaml_job.permissions.clone())
            .or_else(|| scan.and_then(|scan| scan.permissions.clone()));
        let mut uses_secret_set: BTreeSet<String> = scan
            .map(|scan| scan.uses_secrets.iter().cloned().collect())
            .unwrap_or_default();
        if let Some(yaml_job) = seed.yaml {
            uses_secret_set.extend(yaml_job.uses_secrets.iter().cloned());
        }
        let uses_secrets: Vec<String> = uses_secret_set.into_iter().collect();
        inventory_jobs.push(CiInventoryJob {
            workflow: seed.workflow_path.clone(),
            job: seed.job.clone(),
            name: display_name,
            triggers: triggers.clone(),
            path_filters: path_filters.clone(),
            matrix_size,
            timeout_minutes: seed.yaml.and_then(|yaml_job| yaml_job.timeout_minutes),
            permissions: permissions.clone(),
            uses_secrets: uses_secrets.clone(),
            required_check,
            required_check_source: required_check_source.clone(),
            required_check_context: required_check_context.clone(),
        });
        if let Some(stat) = seed.stats {
            history_jobs.push(CiHistoryJob {
                job: stat.job.clone(),
                workflow: stat.workflow_path.clone(),
                window_days,
                runs: stat.runs,
                failure_rate: ci_round_rate(stat.failures, stat.runs),
                cancellation_rate: ci_round_rate(stat.cancellations, stat.runs),
                flake_rate: ci_round_rate(stat.rerun_then_pass, stat.runs),
                rerun_then_pass: stat.rerun_then_pass,
                evidence_gaps: Vec::new(),
            });
            costs_jobs.push(CiCostsJob {
                job: stat.job.clone(),
                workflow: stat.workflow_path.clone(),
                duration_p50_sec: stat.duration_p50_sec,
                duration_p90_sec: stat.duration_p90_sec,
                duration_p99_sec: stat.duration_p99_sec,
                runner_minutes_per_month: ((stat.total_duration_sec as f64 / 60.0) * 30.0
                    / f64::from(window_days.max(1)))
                .round() as u64,
                matrix_expansion: matrix_size,
            });
            correlation_jobs.push(CiCorrelationJob {
                job: stat.job.clone(),
                workflow: stat.workflow_path.clone(),
                independent_failures: stat.independent_failures,
                co_failing_jobs: stat.co_failing_jobs.clone(),
                cheaper_jobs_compared: stat.cheaper_jobs_compared.clone(),
                window_days,
            });
            if let Some(cancellation) =
                ci_runner_cancellation_for_job(stat, scan, seed.yaml, audit_cancel_events)
            {
                runner_cancellations.push(cancellation);
            }
        }
        let evidence = CiTierEvidence {
            job: seed.job.clone(),
            workflow_path: seed.workflow_path.clone(),
            workflow_name: seed.workflow_name.clone(),
            uses: seed
                .yaml
                .map(|yaml_job| yaml_job.uses.clone())
                .unwrap_or_default(),
            permissions,
            uses_secrets,
            triggers,
            path_filters,
            required_check,
            required_check_source: required_check_source.clone(),
            required_check_context: required_check_context.clone(),
            history_available,
            runs: seed.stats.map(|stat| stat.runs).unwrap_or(0),
            independent_failures: seed
                .stats
                .map(|stat| stat.independent_failures)
                .unwrap_or(0),
            duration_p50_sec: seed.stats.and_then(|stat| stat.duration_p50_sec),
        };
        let decision = classify_ci_job_tier(
            &evidence,
            workflow_has_independent_signal.contains(seed.workflow_path.as_str()),
        );
        let has_caught = if history_available {
            format!(
                "{} independent failures in {} runs / {window_days} days",
                evidence.independent_failures, evidence.runs
            )
        } else {
            "unknown: no run history".to_owned()
        };
        let mut receipts = if seed.stats.is_some() {
            vec![
                format!("ci-audit/correlation.json#{}", seed.job),
                format!("ci-audit/costs.json#{}", seed.job),
                format!("ci-audit/history.json#{}", seed.job),
            ]
        } else {
            vec![format!("ci-audit/inventory.json#{}", seed.job)]
        };
        if decision.tier == "move-to-ub-review-required" {
            let inventory_receipt = format!("ci-audit/inventory.json#{}", seed.job);
            if !receipts.contains(&inventory_receipt) {
                receipts.push(inventory_receipt);
            }
        }
        recommendations.push(CiRecommendation {
            job: seed.job.clone(),
            workflow: seed.workflow_path.clone(),
            tier: decision.tier.to_owned(),
            positioned_to_catch: ci_positioned_to_catch(&evidence),
            has_caught,
            receipts,
            proposed_policy: decision.proposed_policy,
            confidence: decision.confidence.to_owned(),
            judgment: "deterministic".to_owned(),
            reason: decision.reason,
            report_note: decision.report_note,
        });
    }
    if let Some(required) = required_checks {
        let mut audited_contexts: BTreeSet<&str> = inventory_jobs
            .iter()
            .flat_map(|job| [job.job.as_str(), job.name.as_str()])
            .collect();
        for scan in scans {
            for yaml_job in &scan.yaml_jobs {
                audited_contexts.insert(yaml_job.id.as_str());
                if let Some(name) = yaml_job.name.as_deref() {
                    audited_contexts.insert(name);
                }
            }
        }
        let unmatched: Vec<&str> = required
            .contexts
            .keys()
            .map(String::as_str)
            .filter(|context| !audited_contexts.contains(context))
            .collect();
        if !unmatched.is_empty() {
            inventory_gaps.push(format!(
                "required-check context not matched to an audited workflow job: {}",
                unmatched.join(", ")
            ));
        }
    }
    let runs_fetched = fetch.map(|fetch| fetch.runs.len()).unwrap_or(0);
    let inventory = CiInventoryArtifact {
        schema: CI_INVENTORY_SCHEMA.to_owned(),
        generated_at,
        repo: repo.to_owned(),
        window_days,
        jobs: inventory_jobs,
        evidence_gaps: inventory_gaps,
    };
    let history = CiHistoryArtifact {
        schema: CI_HISTORY_SCHEMA.to_owned(),
        repo: repo.to_owned(),
        window_days,
        runs_fetched,
        pages_fetched: fetch.map(|fetch| fetch.pages_fetched).unwrap_or(0),
        page_cap: CI_AUDIT_RUN_PAGE_CAP,
        run_cap: CI_AUDIT_RUN_PAGE_CAP * CI_AUDIT_RUNS_PER_PAGE,
        truncated: fetch.is_some_and(|fetch| fetch.truncated),
        jobs: history_jobs,
        evidence_gaps: history_gaps,
    };
    let costs = CiCostsArtifact {
        schema: CI_COSTS_SCHEMA.to_owned(),
        repo: repo.to_owned(),
        window_days,
        jobs: costs_jobs,
        evidence_gaps: shared_degradation_gaps.clone(),
    };
    let correlation = CiCorrelationArtifact {
        schema: CI_CORRELATION_SCHEMA.to_owned(),
        repo: repo.to_owned(),
        window_days,
        independent_failure_rule: CI_AUDIT_INDEPENDENT_FAILURE_RULE.to_owned(),
        jobs: correlation_jobs,
        evidence_gaps: shared_degradation_gaps,
    };
    let recommendations = CiRecommendationsArtifact {
        schema: CI_RECOMMENDATIONS_SCHEMA.to_owned(),
        repo: repo.to_owned(),
        window_days,
        jobs: recommendations,
        evidence_gaps: Vec::new(),
    };
    let mut runner_cancellation_gaps = Vec::new();
    if !history_available {
        runner_cancellation_gaps.push(CI_NO_TOKEN_GAP.to_owned());
    }
    if audit_cancel_events.is_none() {
        runner_cancellation_gaps.push(
            "audit-log cancellation event count not supplied; runner-eviction classification stays conservative"
                .to_owned(),
        );
    }
    let runner_cancellations = CiRunnerCancellationsArtifact {
        schema: CI_RUNNER_CANCELLATIONS_SCHEMA.to_owned(),
        repo: repo.to_owned(),
        window_days,
        classifications: runner_cancellations,
        evidence_gaps: runner_cancellation_gaps,
    };
    let report = render_ci_audit_report(
        repo,
        window_days,
        &inventory,
        &history,
        &costs,
        &correlation,
        &recommendations,
    );
    CiAuditArtifacts {
        inventory,
        history,
        costs,
        correlation,
        recommendations,
        runner_cancellations,
        report,
    }
}

/// Report sections in decision-relevance order (docs/CI_AUDIT_WIZARD.md
/// setup-ci PR-body structure): action items first, human-review last.
pub(crate) const CI_AUDIT_REPORT_TIER_SECTIONS: &[(&str, &str)] = &[
    ("adaptive", "Right-size to adaptive"),
    ("move-to-ub-review-required", "Move into ub-review/gate"),
    ("keep-required", "Keep required"),
    ("advisory", "Advisory"),
    ("nightly-release", "Nightly / release"),
    ("label-gated", "Label-gated"),
    ("flag-for-human", "Human review required"),
];

pub(crate) fn render_ci_audit_report(
    repo: &str,
    window_days: u32,
    inventory: &CiInventoryArtifact,
    history: &CiHistoryArtifact,
    costs: &CiCostsArtifact,
    correlation: &CiCorrelationArtifact,
    recommendations: &CiRecommendationsArtifact,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# CI audit: {repo}\n\n"));
    let workflow_count = inventory
        .jobs
        .iter()
        .map(|job| job.workflow.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    if history.runs_fetched == 0 && history.pages_fetched == 0 {
        out.push_str(&format!(
            "Window: {window_days} days. Inventory-only: no GitHub token, no run history fetched.\n"
        ));
    } else {
        out.push_str(&format!(
            "Window: {window_days} days. {} pull_request runs across {workflow_count} workflows (history capped at {} pages / {} runs{}).\n",
            history.runs_fetched,
            history.page_cap,
            history.run_cap,
            if history.truncated { "; cap reached" } else { "" }
        ));
        out.push_str(&format!(
            "Independent-failure rule: {CI_AUDIT_INDEPENDENT_FAILURE_RULE}.\n"
        ));
    }
    out.push_str("\n## Jobs\n");
    let known_tiers: BTreeSet<&str> = CI_AUDIT_REPORT_TIER_SECTIONS
        .iter()
        .map(|(tier, _)| *tier)
        .collect();
    let mut sections: Vec<(&str, Vec<&CiRecommendation>)> = CI_AUDIT_REPORT_TIER_SECTIONS
        .iter()
        .map(|(tier, heading)| {
            (
                *heading,
                recommendations
                    .jobs
                    .iter()
                    .filter(|recommendation| recommendation.tier == *tier)
                    .collect(),
            )
        })
        .collect();
    let unclassified: Vec<&CiRecommendation> = recommendations
        .jobs
        .iter()
        .filter(|recommendation| !known_tiers.contains(recommendation.tier.as_str()))
        .collect();
    if !unclassified.is_empty() {
        sections.push(("Unclassified", unclassified));
    }
    for (heading, entries) in sections {
        if entries.is_empty() {
            continue;
        }
        out.push_str(&format!("\n### {heading}\n\n"));
        for recommendation in entries {
            let workflow_file = recommendation
                .workflow
                .rsplit('/')
                .next()
                .unwrap_or(recommendation.workflow.as_str());
            let cost = costs.jobs.iter().find(|job| {
                job.job == recommendation.job && job.workflow == recommendation.workflow
            });
            let history_job = history.jobs.iter().find(|job| {
                job.job == recommendation.job && job.workflow == recommendation.workflow
            });
            let note = if recommendation.report_note.is_empty() {
                String::new()
            } else {
                format!(" — {}", recommendation.report_note)
            };
            let receipts = format_ci_audit_report_receipts(&recommendation.receipts);
            match (cost, history_job) {
                (Some(cost), Some(history_job)) => {
                    let p50 = cost
                        .duration_p50_sec
                        .map(format_ci_duration)
                        .unwrap_or_else(|| "unknown".to_owned());
                    let matrix = if cost.matrix_expansion > 1 {
                        format!(", matrix fan-out x{}", cost.matrix_expansion)
                    } else {
                        String::new()
                    };
                    let independent_failures = correlation
                        .jobs
                        .iter()
                        .find(|job| {
                            job.job == recommendation.job && job.workflow == recommendation.workflow
                        })
                        .map(|job| job.independent_failures)
                        .unwrap_or(0);
                    out.push_str(&format!(
                        "- `{}` ({workflow_file}) [{}]: p50 {p50}, {} runs, {} independent failures, ~{} runner-min/mo{matrix}, receipts: {receipts}{note}\n",
                        recommendation.job,
                        recommendation.confidence,
                        history_job.runs,
                        independent_failures,
                        cost.runner_minutes_per_month,
                    ));
                }
                _ => {
                    out.push_str(&format!(
                        "- `{}` ({workflow_file}) [{}]: {}, receipts: {receipts}{note}\n",
                        recommendation.job,
                        recommendation.confidence,
                        recommendation.positioned_to_catch,
                    ));
                }
            }
        }
    }
    let mut gaps: Vec<&str> = Vec::new();
    for gap in inventory
        .evidence_gaps
        .iter()
        .chain(history.evidence_gaps.iter())
    {
        if !gaps.contains(&gap.as_str()) {
            gaps.push(gap.as_str());
        }
    }
    if !gaps.is_empty() {
        out.push_str("\n## Evidence gaps\n\n");
        for gap in gaps {
            out.push_str(&format!("- {gap}\n"));
        }
    }
    out
}

pub(crate) fn format_ci_audit_report_receipts(receipts: &[String]) -> String {
    if receipts.is_empty() {
        return "none".to_owned();
    }
    receipts
        .iter()
        .map(|receipt| format!("`{receipt}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn write_ci_audit_json<T: Serialize>(dir: &Path, name: &str, value: &T) -> Result<()> {
    let path = dir.join(name);
    fs::write(&path, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("write {}", path.display()))
}

pub(crate) fn write_ci_audit_artifacts(dir: &Path, artifacts: &CiAuditArtifacts) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    write_ci_audit_json(dir, "inventory.json", &artifacts.inventory)?;
    write_ci_audit_json(dir, "history.json", &artifacts.history)?;
    write_ci_audit_json(dir, "costs.json", &artifacts.costs)?;
    write_ci_audit_json(dir, "correlation.json", &artifacts.correlation)?;
    write_ci_audit_json(dir, "recommendations.json", &artifacts.recommendations)?;
    write_ci_audit_json(
        dir,
        "runner-cancellations.json",
        &artifacts.runner_cancellations,
    )?;
    let report_path = dir.join("audit-report.md");
    fs::write(&report_path, &artifacts.report)
        .with_context(|| format!("write {}", report_path.display()))?;
    Ok(())
}

/// One `--accept <job>=<command>` pair. The audit receipts record triggers,
/// timings, and correlation - never the runnable command - so the
/// maintainer supplies it and the generator never invents one.
#[derive(Clone, Debug)]
pub(crate) struct SetupCiAccept {
    job: String,
    command: String,
}

pub(crate) fn parse_setup_ci_accepts(raw: &[String]) -> Result<Vec<SetupCiAccept>> {
    let mut accepts = Vec::new();
    for entry in raw {
        let Some((job, command)) = entry.split_once('=') else {
            bail!(
                "--accept needs `<job>=<command>` (the audit receipts do not record the \
                 runnable command; supply it explicitly): got `{entry}`"
            );
        };
        let job = job.trim();
        let command = command.trim();
        if job.is_empty() || command.is_empty() {
            bail!("--accept `<job>=<command>` needs both halves non-empty: got `{entry}`");
        }
        accepts.push(SetupCiAccept {
            job: job.to_owned(),
            command: command.to_owned(),
        });
    }
    Ok(accepts)
}

pub(crate) fn setup_ci_required_flag_for_tier(tier: &str) -> Option<bool> {
    match tier {
        "move-to-ub-review-required" => Some(true),
        "adaptive" => Some(false),
        _ => None,
    }
}

pub(crate) fn load_ci_audit_receipt<T: serde::de::DeserializeOwned>(
    dir: &Path,
    name: &str,
    expected_schema: &str,
) -> Result<T> {
    let path = dir.join(name);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "missing audit receipt {}; run `ub-review audit-ci` first",
            path.display()
        )
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    let schema = value.get("schema").and_then(serde_json::Value::as_str);
    if schema != Some(expected_schema) {
        bail!(
            "{} has schema {:?}; expected {expected_schema}",
            path.display(),
            schema
        );
    }
    serde_json::from_value(value).with_context(|| format!("decode {}", path.display()))
}

/// Sanitize an audited job id into a `[[proof.required]]` id: lowercase
/// alphanumerics and dashes, collapsing every other byte to a dash.
pub(crate) fn setup_ci_proof_id(job: &str) -> String {
    let mut id = String::with_capacity(job.len());
    for ch in job.chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_matches('-').to_owned();
    if id.is_empty() { "job".to_owned() } else { id }
}

pub(crate) fn toml_basic_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

/// Render the generated `.ub-review.toml` additions for the accepted jobs.
/// Emits nothing but `[gate].required_check` and one `[[proof.required]]`
/// entry per accepted generated-proof job - no `[providers]`, no
/// `synchronize_mode`, no `[tools.*.gate]` thresholds (spec 0008: never
/// ship decorative policy into a consumer repo).
pub(crate) fn render_setup_ci_gate_config(
    accepts: &[SetupCiAccept],
    recommendations: &[CiRecommendation],
    inventory: &CiInventoryArtifact,
    required_check: &str,
) -> String {
    let mut text = String::from("[gate]\n");
    text.push_str(&format!(
        "required_check = {}\n",
        toml_basic_string(required_check)
    ));
    for accept in accepts {
        let recommendation = recommendations.iter().find(|entry| entry.job == accept.job);
        let required = recommendation
            .and_then(|entry| setup_ci_required_flag_for_tier(&entry.tier))
            .unwrap_or(false);
        let receipt = recommendation
            .and_then(|entry| entry.receipts.first().cloned())
            .unwrap_or_else(|| format!("ci-audit/recommendations.json#{}", accept.job));
        let timeout_sec = inventory
            .jobs
            .iter()
            .find(|job| job.job == accept.job)
            .and_then(|job| job.timeout_minutes)
            .map(|minutes| minutes.saturating_mul(60))
            .filter(|seconds| *seconds > 0)
            .unwrap_or(600);
        let reason = if required {
            format!(
                "moved to required proof from audited job `{}`; receipt {receipt}",
                accept.job
            )
        } else {
            format!(
                "right-sized to adaptive proof from audited job `{}`; receipt {receipt}",
                accept.job
            )
        };
        text.push_str(&format!(
            "\n[[proof.required]]\nid = {id}\nlanguages = [\"all\"]\ndiff_classes = [\"all\"]\ncommand = {command}\nreason = {reason}\ntimeout_sec = {timeout_sec}\nrequired = {required}\nenabled = true\n",
            id = toml_basic_string(&setup_ci_proof_id(&accept.job)),
            command = toml_basic_string(&accept.command),
            reason = toml_basic_string(&reason),
        ));
    }
    text
}

pub(crate) fn setup_ci_section_bullets(recommendations: &[CiRecommendation], tier: &str) -> String {
    let entries: Vec<&CiRecommendation> = recommendations
        .iter()
        .filter(|entry| entry.tier == tier)
        .collect();
    if entries.is_empty() {
        return "- none recommended by this audit\n".to_owned();
    }
    let mut text = String::new();
    for entry in entries {
        text.push_str(&format!(
            "- `{}` ({}) - {}. judgment: {}; receipts: {}\n",
            entry.job,
            entry.workflow,
            entry.reason,
            entry.judgment,
            entry.receipts.join(", ")
        ));
    }
    text
}

pub(crate) fn setup_ci_move_required_bullets(
    recommendations: &[CiRecommendation],
    accepts: &[SetupCiAccept],
) -> String {
    let entries: Vec<&CiRecommendation> = recommendations
        .iter()
        .filter(|entry| entry.tier == "move-to-ub-review-required")
        .collect();
    if entries.is_empty() {
        return "- none recommended by this audit\n".to_owned();
    }
    let mut text = String::new();
    for entry in entries {
        let status = match accepts.iter().find(|accept| accept.job == entry.job) {
            Some(accept) => format!("accepted; command `{}`", accept.command),
            None => "not accepted; no policy generated".to_owned(),
        };
        text.push_str(&format!(
            "- `{}` ({}) - {}. {status}. judgment: {}; receipts: {}\n",
            entry.job,
            entry.workflow,
            entry.reason,
            entry.judgment,
            entry.receipts.join(", ")
        ));
    }
    text
}

pub(crate) fn setup_ci_branch_protection_change(
    inventory: &CiInventoryArtifact,
    required_check: &str,
) -> String {
    let status_unknown = inventory.jobs.iter().any(|job| {
        job.required_check.is_none()
            || job.required_check_source == "unknown"
            || (job.required_check == Some(true) && job.required_check_context.is_none())
    });
    let required_gap = inventory.evidence_gaps.iter().any(|gap| {
        gap.contains("required-check status unknown")
            || gap.contains("required-check context not matched")
            || gap.contains("required checks unreadable")
    });
    if status_unknown || required_gap {
        let source = inventory
            .jobs
            .first()
            .map(|job| job.required_check_source.as_str())
            .unwrap_or("unknown");
        return format!(
            "- add required check: `{required_check}`\n- old required checks unknown: \
             audit-ci did not prove the full branch-protection/ruleset remove list \
             (inventory records `required_check_source: \"{source}\"`), so this plan refuses \
             to invent it. Review the repository's required checks by hand before removing \
             anything.\n"
        );
    }

    let required_jobs: Vec<&CiInventoryJob> = inventory
        .jobs
        .iter()
        .filter(|job| job.required_check == Some(true))
        .collect();
    let mut text = format!("- add required check: `{required_check}`\n");
    if required_jobs.is_empty() {
        text.push_str("- remove required checks: none reported by audit-ci receipts\n");
    } else {
        text.push_str("- remove required checks reported by audit-ci receipts:\n");
        for job in required_jobs {
            let context = job
                .required_check_context
                .as_deref()
                .unwrap_or(job.job.as_str());
            text.push_str(&format!(
                "  - `{}` from `{}` (source: {}; receipt: ci-audit/inventory.json#{})\n",
                context, job.workflow, job.required_check_source, job.job
            ));
        }
    }
    text.push_str(
        "- apply this manually after the migration PR has passed the repo's existing CI; \
         setup-ci did not mutate branch protection.\n",
    );
    text
}

pub(crate) fn render_setup_ci_branch_protection_doc(
    inventory: &CiInventoryArtifact,
    recommendations: &CiRecommendationsArtifact,
    required_check: &str,
) -> String {
    let mut doc = format!(
        "# Branch protection change\n\nRepo: {} (window: {} days). Generated by `ub-review setup-ci` from `ci-audit/inventory.json` required-check receipts.\n\n",
        recommendations.repo, recommendations.window_days
    );
    doc.push_str("## Decision\n\n");
    doc.push_str(
        "Branch protection remains manual. `setup-ci` opened a migration PR only; it did not mutate repository protection rules.\n\n",
    );
    doc.push_str("## Change\n\n");
    let change = setup_ci_branch_protection_change(inventory, required_check);
    let exact_remove_list = !change.contains("old required checks unknown");
    doc.push_str(&change);
    doc.push_str("\n## Apply after\n\n");
    doc.push_str("- the migration PR passes the repository's existing required checks;\n");
    doc.push_str(
        "- the new `ub-review/gate` check has one observed red proof and one quiet-green proof;\n",
    );
    doc.push_str("- a maintainer has reviewed the required-check remove list against the repository settings UI.\n");
    doc.push_str("\n## Rollback\n\n");
    doc.push_str(
        "- Revert the migration PR. If `ub-review/gate` was added manually, remove that required check by hand.\n",
    );
    if exact_remove_list {
        doc.push_str(
            "- If old required checks were removed manually, restore the exact checks listed above.\n",
        );
    } else {
        doc.push_str(
            "- This file does not prove an old-check remove list; review repository settings by hand before removing or restoring old required checks.\n",
        );
    }
    doc
}

pub(crate) fn render_setup_ci_migration_plan(
    inventory: &CiInventoryArtifact,
    recommendations: &CiRecommendationsArtifact,
    accepts: &[SetupCiAccept],
    required_check: &str,
) -> String {
    let jobs = &recommendations.jobs;
    let mut plan = format!(
        "# CI migration plan\n\nRepo: {} (window: {} days). Rendered by `ub-review setup-ci \
         --print-pr` from the ci-audit receipts; nothing below was applied.\n\n",
        recommendations.repo, recommendations.window_days
    );
    plan.push_str("## Decision\n\n");
    if accepts.is_empty() {
        plan.push_str(&format!(
            "No jobs accepted into the generated gate policy, so there is no migration PR \
             to open. The audit covered {} job(s); pass `--accept <job>=<command>` for each \
             adaptive or move-to-ub-review-required job to fold into `{required_check}`.\n\n",
            jobs.len()
        ));
    } else {
        plan.push_str(&format!(
            "Fold {} accepted job(s) into one required check `{required_check}` as adaptive \
             or required proof; every other job keeps its current posture per the tiers below.\n\n",
            accepts.len()
        ));
    }
    plan.push_str("## Keep required\n\n");
    plan.push_str(&setup_ci_section_bullets(jobs, "keep-required"));
    plan.push_str("\n## Move into ub-review/gate\n\n");
    plan.push_str(&setup_ci_move_required_bullets(jobs, accepts));
    plan.push_str("\n## Right-size to adaptive\n\n");
    let adaptive: Vec<&CiRecommendation> = jobs
        .iter()
        .filter(|entry| entry.tier == "adaptive")
        .collect();
    if adaptive.is_empty() {
        plan.push_str("- none recommended by this audit\n");
    } else {
        for entry in &adaptive {
            let accepted = accepts.iter().find(|accept| accept.job == entry.job);
            let status = match accepted {
                Some(accept) => format!("accepted; command `{}`", accept.command),
                None => "not accepted; no policy generated".to_owned(),
            };
            plan.push_str(&format!(
                "- `{}` ({}) - {}. {status}. judgment: {}; receipts: {}\n",
                entry.job,
                entry.workflow,
                entry.reason,
                entry.judgment,
                entry.receipts.join(", ")
            ));
        }
    }
    plan.push_str("\n## Label-gated / nightly / release\n\n");
    plan.push_str(&setup_ci_section_bullets(jobs, "label-gated"));
    plan.push_str("\n## Human review required\n\n");
    plan.push_str(&setup_ci_section_bullets(jobs, "flag-for-human"));
    plan.push_str("\n## Proposed branch protection change\n\n");
    plan.push_str(&setup_ci_branch_protection_change(
        inventory,
        required_check,
    ));
    plan.push_str("\n## Rollback\n\n");
    plan.push_str(
        "- revert the migration PR; nothing else changed. Branch protection is never \
         mutated by setup-ci, so the only manual step is removing the required check if \
         it was added by hand.\n",
    );
    if !accepts.is_empty() {
        plan.push_str("\n## Generated .ub-review.toml additions\n\n```toml\n");
        plan.push_str(&render_setup_ci_gate_config(
            accepts,
            jobs,
            inventory,
            required_check,
        ));
        plan.push_str("```\n");
    }
    plan
}

/// Standard base64 (RFC 4648, with padding) for the GitHub contents API.
pub(crate) fn base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        encoded.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        encoded.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    encoded
}

/// Render the generated consumer gate workflow, pinned to the given
/// ub-review commit SHA. Mirrors this repository's own gate workflow shape
/// (job name = the required check name) at the zero-key tier: model-mode
/// off, no heavy witnesses, tool-bundle core. Model keys are a documented
/// edit, never a generated secret reference.
pub(crate) fn render_setup_ci_gate_workflow(action_sha: &str, required_check: &str) -> String {
    format!(
        r#"name: {required_check}

# Generated by `ub-review setup-ci`. The gate runs the proofs declared in
# .ub-review.toml and reports one required check. Model lanes are off until
# the repo opts in (model-mode + a provider key input).
on:
  pull_request:
    types: [opened, reopened, ready_for_review, synchronize]

permissions:
  contents: read
  pull-requests: write
  checks: write

concurrency:
  group: ub-review-gate-${{{{ github.event.pull_request.number || github.ref }}}}
  cancel-in-progress: true

jobs:
  gate:
    name: {required_check}
    runs-on: ubuntu-latest
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@v5
        with:
          fetch-depth: 0
          persist-credentials: false

      - name: ub-review gate
        uses: EffortlessMetrics/ub-review@{action_sha}
        with:
          mode: intelligent-ci
          fail-on-gate: auto
          root: .
          base: origin/${{{{ github.base_ref }}}}
          head: HEAD
          out: target/ub-review
          install-tools: 'true'
          tool-bundle: core
          posting: artifact-only
          model-mode: 'off'
          github-token: ${{{{ github.token }}}}

      - name: Upload ub-review artifacts
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: ub-review-${{{{ github.event.pull_request.number || github.run_id }}}}
          path: target/ub-review
          if-no-files-found: warn
          retention-days: 7
"#
    )
}

pub(crate) struct SetupCiGeneratedFile {
    pub(crate) path: &'static str,
    content: String,
    pub(crate) message: &'static str,
}

pub(crate) fn setup_ci_generated_files(
    plan: &str,
    generated_config: &str,
    branch_protection_doc: &str,
    action_sha: &str,
    required_check: &str,
) -> Vec<SetupCiGeneratedFile> {
    vec![
        SetupCiGeneratedFile {
            path: ".ub-review.toml",
            content: generated_config.to_owned(),
            message: "Add the ub-review gate policy from the CI audit",
        },
        SetupCiGeneratedFile {
            path: ".github/workflows/ub-review-gate.yml",
            content: render_setup_ci_gate_workflow(action_sha, required_check),
            message: "Add the ub-review gate workflow",
        },
        SetupCiGeneratedFile {
            path: "docs/ci/ub-review-migration.md",
            content: plan.to_owned(),
            message: "Record the CI migration plan and its audit receipts",
        },
        SetupCiGeneratedFile {
            path: "docs/ci/branch-protection-change.md",
            content: branch_protection_doc.to_owned(),
            message: "Record the manual branch protection change",
        },
    ]
}

pub(crate) fn valid_setup_ci_action_sha(args: &SetupCiArgs) -> Result<Option<&str>> {
    match args.action_sha.as_deref().map(str::trim) {
        Some(sha) if sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(Some(sha)),
        Some(_) => bail!(
            "--action-sha must be the full 40-hex ub-review commit to pin in the generated workflow"
        ),
        None => Ok(None),
    }
}

pub(crate) fn write_setup_ci_preview_files(
    dir: &Path,
    files: &[SetupCiGeneratedFile],
) -> Result<Vec<String>> {
    let preview_dir = dir.join("preview");
    let mut written = Vec::new();
    for file in files {
        let path = preview_dir.join(file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&path, &file.content).with_context(|| format!("write {}", path.display()))?;
        written.push(file.path.to_owned());
    }
    Ok(written)
}

/// The receipt `--open-pr` writes (ub-review.setup_pr_result.v1).
#[derive(Debug, Serialize)]
pub(crate) struct SetupPrResult {
    schema: String,
    repo: String,
    base: String,
    branch: String,
    pr_url: String,
    files: Vec<String>,
    action_sha: String,
}

pub(crate) struct SetupCiOpenContext<'a> {
    token: &'a str,
    out_dir: &'a Path,
}

pub(crate) fn setup_ci_api_post(
    context: &SetupCiOpenContext<'_>,
    method: &str,
    url: &str,
    payload: &serde_json::Value,
    receipt_name: &str,
) -> Result<serde_json::Value> {
    let payload_path = context.out_dir.join(receipt_name);
    fs::write(&payload_path, serde_json::to_vec_pretty(payload)?)
        .with_context(|| format!("write {}", payload_path.display()))?;
    let output = run_curl_json_send(
        Path::new("."),
        method,
        url,
        &format!("Authorization: Bearer {}", context.token),
        &payload_path,
        &[
            "Accept: application/vnd.github+json",
            "Content-Type: application/json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    )
    .with_context(|| format!("{method} {url}"))?;
    if !output.status.success() {
        bail!(
            "{method} {url} failed with http status {:?}: {}",
            output.http_status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null))
}

/// Open the migration PR: one new branch from the default branch, four new
/// files (config, gate workflow, migration plan doc, branch-protection doc), one PR whose body is
/// the plan. Refuses to edit a repo that already carries a .ub-review.toml
/// (file edits are a later slice); never touches branch protection.
pub(crate) fn execute_setup_ci_open_pr(
    args: &SetupCiArgs,
    plan: &str,
    generated_config: &str,
    branch_protection_doc: &str,
    required_check: &str,
) -> Result<SetupPrResult> {
    let token = args
        .github_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("--open-pr needs a GitHub token (GITHUB_TOKEN)"))?;
    let repo = args
        .repo
        .as_deref()
        .filter(|value| is_valid_repo_slug(value))
        .ok_or_else(|| anyhow::anyhow!("--open-pr needs a valid --repo owner/name slug"))?;
    let action_sha = valid_setup_ci_action_sha(args)?.ok_or_else(|| {
        anyhow::anyhow!(
            "--open-pr needs --action-sha, the full 40-hex ub-review commit to pin \
             in the generated workflow; the generator refuses to invent a pin"
        )
    })?;
    let api_url = args.github_api_url.trim_end_matches('/');
    let out_dir = args.out.join("ci-audit");
    let context = SetupCiOpenContext {
        token,
        out_dir: &out_dir,
    };

    let repo_value = run_github_api_get(Path::new("."), &format!("{api_url}/repos/{repo}"), token)
        .with_context(|| "read repository metadata")?;
    let base = repo_value
        .get("default_branch")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("repository metadata has no default_branch"))?
        .to_owned();
    let base_ref = run_github_api_get(
        Path::new("."),
        &format!("{api_url}/repos/{repo}/git/ref/heads/{base}"),
        token,
    )
    .with_context(|| "read default branch ref")?;
    let base_sha = base_ref
        .get("object")
        .and_then(|object| object.get("sha"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("default branch ref has no object.sha"))?
        .to_owned();
    let tree = run_github_api_get(
        Path::new("."),
        &format!("{api_url}/repos/{repo}/git/trees/{base_sha}"),
        token,
    )
    .with_context(|| "read default branch tree")?;
    let has_config = tree
        .get("tree")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry.get("path").and_then(serde_json::Value::as_str) == Some(".ub-review.toml")
            })
        });
    if has_config {
        bail!(
            "{repo} already has a .ub-review.toml; this slice only creates new files. \
             Apply the printed additions by hand or wait for the config-edit slice."
        );
    }

    setup_ci_api_post(
        &context,
        "POST",
        &format!("{api_url}/repos/{repo}/git/refs"),
        &serde_json::json!({
            "ref": format!("refs/heads/{}", args.branch),
            "sha": base_sha,
        }),
        "setup-pr-branch-payload.json",
    )
    .with_context(|| format!("create branch {} (does it already exist?)", args.branch))?;

    let files = setup_ci_generated_files(
        plan,
        generated_config,
        branch_protection_doc,
        action_sha,
        required_check,
    );
    let mut file_paths = Vec::new();
    for (index, file) in files.iter().enumerate() {
        setup_ci_api_post(
            &context,
            "PUT",
            &format!("{api_url}/repos/{repo}/contents/{}", file.path),
            &serde_json::json!({
                "message": file.message,
                "content": base64_standard(file.content.as_bytes()),
                "branch": args.branch,
            }),
            &format!("setup-pr-file-payload-{index}.json"),
        )
        .with_context(|| format!("create {}", file.path))?;
        file_paths.push(file.path.to_owned());
    }

    let pr = setup_ci_api_post(
        &context,
        "POST",
        &format!("{api_url}/repos/{repo}/pulls"),
        &serde_json::json!({
            "title": "Adopt ub-review/gate from the CI audit",
            "head": args.branch,
            "base": base,
            "body": plan,
        }),
        "setup-pr-pull-payload.json",
    )
    .with_context(|| "open the migration PR")?;
    let pr_url = pr
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(SetupPrResult {
        schema: SETUP_PR_RESULT_SCHEMA.to_owned(),
        repo: repo.to_owned(),
        base,
        branch: args.branch.clone(),
        pr_url,
        files: file_paths,
        action_sha: action_sha.to_owned(),
    })
}

pub(crate) fn remove_stale_setup_ci_terminal_receipt(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("remove stale {}", path.display())),
    }
}

pub(crate) fn cmd_setup_ci(args: SetupCiArgs) -> Result<()> {
    if !args.print_pr && !args.open_pr {
        bail!(
            "setup-ci does nothing implicitly: pass --print-pr to render the migration \
             PR contents from a prior audit-ci run, or --open-pr to open it."
        );
    }
    let dir = args.out.join("ci-audit");
    let inventory: CiInventoryArtifact =
        load_ci_audit_receipt(&dir, "inventory.json", CI_INVENTORY_SCHEMA)?;
    let recommendations: CiRecommendationsArtifact =
        load_ci_audit_receipt(&dir, "recommendations.json", CI_RECOMMENDATIONS_SCHEMA)?;
    for (name, expected_schema) in [
        ("history.json", CI_HISTORY_SCHEMA),
        ("costs.json", CI_COSTS_SCHEMA),
        ("correlation.json", CI_CORRELATION_SCHEMA),
    ] {
        let _: serde_json::Value = load_ci_audit_receipt(&dir, name, expected_schema)?;
    }
    if dir.join("runner-cancellations.json").exists() {
        let _: serde_json::Value = load_ci_audit_receipt(
            &dir,
            "runner-cancellations.json",
            CI_RUNNER_CANCELLATIONS_SCHEMA,
        )?;
    }
    let accepts = parse_setup_ci_accepts(&args.accept)?;
    for accept in &accepts {
        let Some(recommendation) = recommendations
            .jobs
            .iter()
            .find(|entry| entry.job == accept.job)
        else {
            bail!(
                "--accept `{}` does not match any job in ci-audit/recommendations.json",
                accept.job
            );
        };
        match recommendation.tier.as_str() {
            tier if setup_ci_required_flag_for_tier(tier).is_some() => {}
            "flag-for-human" => bail!(
                "--accept `{}` refused: flag-for-human recommendations never become \
                 generated edits; a human reviews that job directly",
                accept.job
            ),
            tier => bail!(
                "--accept `{}` refused: tier `{tier}` proposes no generated edit; only \
                 adaptive or move-to-ub-review-required jobs are acceptable",
                accept.job
            ),
        }
    }
    let required_check = Config::load_or_default(&args.config, None)
        .map(|config| config.gate.required_check)
        .unwrap_or_else(|_| "ub-review/gate".to_owned());
    let plan =
        render_setup_ci_migration_plan(&inventory, &recommendations, &accepts, &required_check);
    let branch_protection_doc =
        render_setup_ci_branch_protection_doc(&inventory, &recommendations, &required_check);
    let generated =
        render_setup_ci_gate_config(&accepts, &recommendations.jobs, &inventory, &required_check);
    if !accepts.is_empty() {
        // The round-trip oracle, enforced at runtime too: a generated config
        // the loader strips keys from is a generator failure, abort.
        let reloaded = Config::from_toml_with_policy_receipts(&generated)
            .with_context(|| "generated config failed to parse; generator failure")?;
        if !reloaded.policy_errors.is_empty() {
            bail!(
                "generator failure: generated config reloads with policy receipts: {:?}",
                reloaded.policy_errors
            );
        }
    }
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let plan_path = dir.join("migration-plan.md");
    fs::write(&plan_path, &plan).with_context(|| format!("write {}", plan_path.display()))?;
    print!("{plan}");
    eprintln!("wrote {}", plan_path.display());
    if args.print_pr && !accepts.is_empty() {
        if let Some(action_sha) = valid_setup_ci_action_sha(&args)? {
            let files = setup_ci_generated_files(
                &plan,
                &generated,
                &branch_protection_doc,
                action_sha,
                &required_check,
            );
            let written = write_setup_ci_preview_files(&dir, &files)?;
            eprintln!(
                "wrote {} setup-ci preview file(s) under {}",
                written.len(),
                dir.join("preview").display()
            );
        } else {
            let preview_dir = dir.join("preview");
            if preview_dir.exists() {
                fs::remove_dir_all(&preview_dir)
                    .with_context(|| format!("remove stale {}", preview_dir.display()))?;
            }
            eprintln!(
                "skipped setup-ci preview files; pass --action-sha <40-hex-sha> to render the pinned workflow preview"
            );
        }
    }
    if args.open_pr {
        if accepts.is_empty() {
            bail!(
                "--open-pr with no accepted jobs has no migration PR to open; the plan \
                 above explains the tiers. Pass --accept <job>=<command> for the \
                 adaptive jobs to fold in."
            );
        }
        let result_path = dir.join("setup-pr-result.json");
        let error_path = dir.join("setup-pr-error.json");
        remove_stale_setup_ci_terminal_receipt(&result_path)?;
        remove_stale_setup_ci_terminal_receipt(&error_path)?;
        match execute_setup_ci_open_pr(
            &args,
            &plan,
            &generated,
            &branch_protection_doc,
            &required_check,
        ) {
            Ok(result) => {
                fs::write(&result_path, serde_json::to_vec_pretty(&result)?)
                    .with_context(|| format!("write {}", result_path.display()))?;
                println!("opened {}", result.pr_url);
                eprintln!("wrote {}", result_path.display());
            }
            Err(err) => {
                fs::write(
                    &error_path,
                    serde_json::to_vec_pretty(&serde_json::json!({
                        "schema": SETUP_PR_ERROR_SCHEMA,
                        "status": "failed",
                        "reason": format!("{err:#}"),
                    }))?,
                )
                .with_context(|| format!("write {}", error_path.display()))?;
                return Err(err);
            }
        }
    }
    Ok(())
}
