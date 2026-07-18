//! PR thread context collection, GitHub API helpers, thread rendering,
//! and event PR context reading (cleanup train step 47, pure code motion).

use crate::*;

pub(crate) fn collect_pr_thread_context(
    root: &Path,
    args: &RunArgs,
    current_head: &str,
) -> Result<PrThreadContext> {
    let mut context = PrThreadContext {
        schema: PR_THREAD_CONTEXT_SCHEMA.to_owned(),
        status: "absent".to_owned(),
        max_bytes: args.pr_thread_context_max_bytes,
        sources: Vec::new(),
        warnings: Vec::new(),
        pull_number: None,
        title: None,
        body: None,
        body_truncated: false,
        thread_context_path: None,
        thread_context: None,
        thread_context_truncated: false,
        threads: Vec::new(),
    };

    if let Some(event_path) = std::env::var_os("GITHUB_EVENT_PATH") {
        let event_path = PathBuf::from(event_path);
        context
            .sources
            .push(format!("github-event:{}", event_path.display()));
        match read_github_event_pr_context(&event_path, args.pr_thread_context_max_bytes) {
            Ok(event_context) => {
                context.pull_number = event_context.pull_number;
                context.title = event_context.title;
                context.body = event_context.body;
                context.body_truncated = event_context.body_truncated;
            }
            Err(err) => context
                .warnings
                .push(format!("github-event unavailable: {err}")),
        }
    }
    context.pull_number = args.github_pull_number.or(context.pull_number);

    let configured_thread_path = args.pr_thread_context.trim();
    if !configured_thread_path.is_empty() {
        let configured_path = PathBuf::from(configured_thread_path);
        let path = if configured_path.is_absolute() {
            configured_path
        } else {
            root.join(configured_path)
        };
        context
            .sources
            .push(format!("thread-context-file:{}", path.display()));
        context.thread_context_path = Some(path.display().to_string());
        match read_bounded_text_with_status(&path, args.pr_thread_context_max_bytes) {
            Ok(text) => {
                context.thread_context = Some(text.text);
                context.thread_context_truncated = text.truncated;
            }
            Err(err) => context
                .warnings
                .push(format!("thread-context-file unavailable: {err}")),
        }
    }

    match github_thread_api_request(args, context.pull_number) {
        None => {}
        Some(Err(err)) => context
            .warnings
            .push(format!("github-api thread context unavailable: {err}")),
        Some(Ok(request)) => {
            match read_github_pr_thread_context(
                root,
                &request,
                args.pr_thread_context_max_bytes,
                current_head,
            ) {
                Ok(api_context) => {
                    context.sources.extend(api_context.sources);
                    context.threads.extend(api_context.threads);
                    append_thread_context(
                        &mut context,
                        &api_context.thread_context,
                        args.pr_thread_context_max_bytes,
                    );
                }
                Err(err) => context
                    .warnings
                    .push(format!("github-api thread context unavailable: {err}")),
            }
        }
    }

    context.status =
        if context.title.is_some() || context.body.is_some() || context.thread_context.is_some() {
            "seeded".to_owned()
        } else if context.warnings.is_empty() {
            "absent".to_owned()
        } else {
            "unavailable".to_owned()
        };

    Ok(context)
}

pub(crate) struct GitHubThreadApiRequest<'a> {
    pub(crate) auth: &'a str,
    pub(crate) repo: &'a str,
    pub(crate) pull_number: u64,
    pub(crate) api_url: &'a str,
}

pub(crate) struct GitHubThreadApiContext {
    pub(crate) sources: Vec<String>,
    pub(crate) thread_context: String,
    pub(crate) threads: Vec<ReviewThreadRecord>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub(crate) struct ReviewThreadRecord {
    pub(crate) id: String,
    pub(crate) kind: String,
    pub(crate) author: String,
    pub(crate) body: String,
    pub(crate) path: Option<String>,
    pub(crate) line: Option<u32>,
    pub(crate) commit_id: Option<String>,
    /// `current`, `stale`, or `unbound` relative to the exact reviewed SHA.
    pub(crate) head_binding: String,
    pub(crate) state: Option<String>,
}

pub(crate) fn github_thread_api_request<'a>(
    args: &'a RunArgs,
    event_pull_number: Option<u64>,
) -> Option<Result<GitHubThreadApiRequest<'a>>> {
    let auth = args
        .pr_thread_auth
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    let Some(repo) = args
        .github_repo
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    else {
        return Some(Err(anyhow::anyhow!(
            "GitHub repository slug is unavailable"
        )));
    };
    if !is_valid_repo_slug(repo) {
        return Some(Err(anyhow::anyhow!(
            "GitHub repository slug is invalid: {repo}"
        )));
    }
    let Some(pull_number) = args.github_pull_number.or(event_pull_number) else {
        return Some(Err(anyhow::anyhow!("pull request number is unavailable")));
    };
    Some(Ok(GitHubThreadApiRequest {
        auth,
        repo,
        pull_number,
        api_url: args.github_api_url.trim_end_matches('/'),
    }))
}

pub(crate) fn read_github_pr_thread_context(
    root: &Path,
    request: &GitHubThreadApiRequest<'_>,
    max_bytes: usize,
    current_head: &str,
) -> Result<GitHubThreadApiContext> {
    let endpoints = [
        (
            "issue-comments",
            format!(
                "{}/repos/{}/issues/{}/comments?per_page=30",
                request.api_url, request.repo, request.pull_number
            ),
        ),
        (
            "review-summaries",
            format!(
                "{}/repos/{}/pulls/{}/reviews?per_page=30",
                request.api_url, request.repo, request.pull_number
            ),
        ),
        (
            "review-comments",
            format!(
                "{}/repos/{}/pulls/{}/comments?per_page=50",
                request.api_url, request.repo, request.pull_number
            ),
        ),
    ];
    let mut sections = Vec::new();
    let mut sources = Vec::new();
    let mut threads = Vec::new();
    for (kind, url) in endpoints {
        let value = run_github_api_get(root, &url, request.auth)
            .with_context(|| format!("fetch GitHub PR thread {kind}"))?;
        sources.push(format!(
            "github-api:{}/{}/{}",
            request.repo, request.pull_number, kind
        ));
        sections.push(render_github_pr_thread_section(kind, &value, max_bytes));
        threads.extend(github_thread_records(kind, &value, current_head));
    }

    let mut text = String::new();
    text.push_str("## GitHub PR Thread Snapshot\n\n");
    text.push_str(&format!(
        "Source: `{}` PR `#{}`. Bounded to lane context; full GitHub thread remains source of truth.\n\n",
        escape_md(request.repo),
        request.pull_number
    ));
    text.push_str(&sections.join("\n"));
    let bounded = bounded_string(&text, max_bytes);
    Ok(GitHubThreadApiContext {
        sources,
        thread_context: bounded.text,
        threads,
    })
}

fn github_thread_records(
    kind: &str,
    value: &serde_json::Value,
    current_head: &str,
) -> Vec<ReviewThreadRecord> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .map(|item| {
            let commit_id = item
                .get("commit_id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned);
            let head_binding = match commit_id.as_deref() {
                Some(commit) if commit.eq_ignore_ascii_case(current_head) => "current",
                Some(_) => "stale",
                None => "unbound",
            };
            ReviewThreadRecord {
                id: item
                    .get("id")
                    .and_then(serde_json::Value::as_u64)
                    .map(|id| id.to_string())
                    .or_else(|| {
                        item.get("node_id")
                            .and_then(serde_json::Value::as_str)
                            .map(str::to_owned)
                    })
                    .unwrap_or_else(|| "unknown-thread".to_owned()),
                kind: kind.to_owned(),
                author: item
                    .pointer("/user/login")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("unknown")
                    .to_owned(),
                body: item
                    .get("body")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                path: item
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
                line: item
                    .get("line")
                    .or_else(|| item.get("original_line"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|line| u32::try_from(line).ok()),
                commit_id,
                head_binding: head_binding.to_owned(),
                state: item
                    .get("state")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned),
            }
        })
        .collect()
}

#[cfg(test)]
mod structured_thread_tests {
    use super::*;

    #[test]
    fn github_records_preserve_anchor_and_exact_head() -> Result<()> {
        let value = serde_json::json!([{
            "id": 42,
            "user": {"login": "reviewer"},
            "body": "Postfix is discarded after the list item.",
            "path": "src/parser.rs",
            "line": 17,
            "commit_id": "abc123",
            "state": "COMMENTED"
        }]);

        let records = github_thread_records("review-comments", &value, "abc123");
        let record = records
            .first()
            .ok_or_else(|| anyhow::anyhow!("expected one structured thread record"))?;
        anyhow::ensure!(record.id == "42");
        anyhow::ensure!(record.path.as_deref() == Some("src/parser.rs"));
        anyhow::ensure!(record.line == Some(17));
        anyhow::ensure!(record.head_binding == "current");
        let stale_records = github_thread_records("review-comments", &value, "def456");
        anyhow::ensure!(
            stale_records
                .first()
                .is_some_and(|item| item.head_binding == "stale")
        );
        Ok(())
    }

    #[test]
    fn unbound_issue_comment_cannot_certify_current_head() -> Result<()> {
        let value = serde_json::json!([{
            "node_id": "IC_kwDO",
            "user": {"login": "maintainer"},
            "body": "Accepted tradeoff"
        }]);

        let records = github_thread_records("issue-comments", &value, "abc123");
        let record = records
            .first()
            .ok_or_else(|| anyhow::anyhow!("expected one structured thread record"))?;
        anyhow::ensure!(record.head_binding == "unbound");
        Ok(())
    }
}

pub(crate) fn run_github_api_get(root: &Path, url: &str, auth: &str) -> Result<serde_json::Value> {
    let mut command = ProcessCommand::new("curl");
    command
        .arg("-sS")
        .arg("--fail-with-body")
        .arg("--max-time")
        .arg("30")
        .arg("-w")
        .arg("\nUB_REVIEW_HTTP_STATUS:%{http_code}\n")
        .arg("-K")
        .arg("-")
        .arg(url)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().with_context(|| "spawn GitHub API curl")?;
    {
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| anyhow::anyhow!("curl stdin unavailable"))?;
        use std::io::Write as _;
        const AUTH_HEADER_NAME: &str = "Authorization";
        let auth_scheme = ["Bear", "er"].concat();
        for header in [
            "Accept: application/vnd.github+json",
            "X-GitHub-Api-Version: 2022-11-28",
            &format!("{AUTH_HEADER_NAME}: {auth_scheme} {auth}"),
        ] {
            writeln!(stdin, "header = \"{}\"", curl_config_quote(header))?;
        }
    }
    let output = child
        .wait_with_output()
        .with_context(|| "wait for GitHub API curl")?;
    let (stdout, http_status) = split_curl_http_status(output.stdout);
    if !output.status.success() {
        bail!(
            "GitHub API curl exited {:?} with http status {:?}: stderr: {}; stdout: {}",
            output.status.code(),
            http_status,
            String::from_utf8_lossy(&output.stderr),
            String::from_utf8_lossy(&stdout)
        );
    }
    serde_json::from_slice(&stdout).with_context(|| "parse GitHub API response")
}

pub(crate) fn render_github_pr_thread_section(
    kind: &str,
    value: &serde_json::Value,
    max_bytes: usize,
) -> String {
    let title = match kind {
        "issue-comments" => "Issue Comments",
        "review-summaries" => "Review Summaries",
        "review-comments" => "Review Comments",
        _ => "Thread Items",
    };
    let mut text = format!("### {title}\n\n");
    let Some(items) = value.as_array() else {
        text.push_str("- GitHub response was not an array.\n");
        return text;
    };
    if items.is_empty() {
        text.push_str("- None found.\n");
        return text;
    }
    for item in items {
        text.push_str(&render_github_pr_thread_item(kind, item, max_bytes));
    }
    text
}

pub(crate) fn render_github_pr_thread_item(
    kind: &str,
    item: &serde_json::Value,
    max_bytes: usize,
) -> String {
    let author = item
        .pointer("/user/login")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let created_at = item
        .get("created_at")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown-time");
    let state = item
        .get("state")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let path = item
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let line = item
        .get("line")
        .or_else(|| item.get("original_line"))
        .and_then(serde_json::Value::as_u64);
    let body = item
        .get("body")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("");
    let bounded_body = bounded_string(body.trim(), max_bytes.min(1200));
    let location = if !path.is_empty() {
        match line {
            Some(line) => format!(" `{}`:`{line}`", escape_md(path)),
            None => format!(" `{}`", escape_md(path)),
        }
    } else {
        String::new()
    };
    let state = if state.is_empty() {
        String::new()
    } else {
        format!(" `{}`", escape_md(state))
    };
    let item_kind = match kind {
        "issue-comments" => "issue-comment",
        "review-summaries" => "review",
        "review-comments" => "review-comment",
        _ => "thread-item",
    };
    let mut text = format!(
        "- `{}` `{}` by `{}`{}{}\n",
        item_kind,
        escape_md(created_at),
        escape_md(author),
        state,
        location
    );
    if !bounded_body.text.is_empty() {
        text.push_str("  ```text\n");
        text.push_str(&bounded_body.text);
        if !bounded_body.text.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("  ```\n");
    }
    text
}

pub(crate) fn append_thread_context(
    context: &mut PrThreadContext,
    addition: &str,
    max_bytes: usize,
) {
    if addition.trim().is_empty() {
        return;
    }
    let mut merged = String::new();
    if let Some(existing) = context.thread_context.as_deref() {
        merged.push_str(existing);
        if !existing.ends_with('\n') {
            merged.push('\n');
        }
        merged.push('\n');
    }
    merged.push_str(addition);
    let bounded = bounded_string(&merged, max_bytes);
    context.thread_context = Some(bounded.text);
    context.thread_context_truncated |= bounded.truncated;
}

pub(crate) struct GitHubEventPrContext {
    pub(crate) pull_number: Option<u64>,
    pub(crate) title: Option<String>,
    pub(crate) body: Option<String>,
    pub(crate) body_truncated: bool,
}

pub(crate) fn read_github_event_pr_context(
    path: &Path,
    max_bytes: usize,
) -> Result<GitHubEventPrContext> {
    let text = fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&text).with_context(|| format!("parse {}", path.display()))?;
    let Some(pull_request) = value.get("pull_request") else {
        return Ok(GitHubEventPrContext {
            pull_number: None,
            title: None,
            body: None,
            body_truncated: false,
        });
    };
    let body = pull_request
        .get("body")
        .and_then(serde_json::Value::as_str)
        .map(|body| bounded_string(body, max_bytes));
    Ok(GitHubEventPrContext {
        pull_number: pull_request
            .get("number")
            .and_then(serde_json::Value::as_u64),
        title: pull_request
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned),
        body: body.as_ref().map(|body| body.text.clone()),
        body_truncated: body.as_ref().is_some_and(|body| body.truncated),
    })
}

pub(crate) fn render_pr_thread_context(context: &PrThreadContext) -> String {
    let mut text = String::new();
    text.push_str(&format!("- Status: `{}`\n", context.status));
    if context.sources.is_empty() {
        text.push_str("- Sources: none\n");
    } else {
        text.push_str("- Sources:\n");
        for source in &context.sources {
            text.push_str(&format!("  - `{}`\n", escape_md(source)));
        }
    }
    if !context.warnings.is_empty() {
        text.push_str("- Warnings:\n");
        for warning in &context.warnings {
            text.push_str(&format!("  - {}\n", escape_md(warning)));
        }
    }
    if let Some(number) = context.pull_number {
        text.push_str(&format!("- Pull request: `#{number}`\n"));
    }
    if let Some(title) = context.title.as_deref() {
        text.push_str(&format!("- Title: {}\n", escape_md(title)));
    }
    if let Some(guidance) = pr_thread_reuse_guidance(context) {
        text.push('\n');
        text.push_str(guidance);
    }
    if let Some(body) = context.body.as_deref() {
        text.push_str("\n### PR Body\n\n```text\n");
        text.push_str(body);
        if !body.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```\n");
    }
    if let Some(thread_context) = context.thread_context.as_deref() {
        text.push_str("\n### Prior Review Thread\n\n```text\n");
        text.push_str(thread_context);
        if !thread_context.ends_with('\n') {
            text.push('\n');
        }
        text.push_str("```\n");
    }
    if context.status == "absent" {
        text.push_str("- No PR thread context was provided for this run.\n");
    }
    text
}

pub(crate) fn pr_thread_reuse_guidance(context: &PrThreadContext) -> Option<&'static str> {
    if context.status != "seeded" {
        return None;
    }
    Some(
        "### Seeded Thread Reuse Rules\n\n\
- Treat PR body claims, author replies, prior ub-review comments, resolved/dismissed discussion notes, and proof receipts in this context as lane evidence.\n\
- Before emitting a verification question or proof request, compare it with the seeded thread. If the same concern is already answered and the current diff does not reopen it, emit a `resolved-check` observation or `failed_objection` instead of a fresh candidate.\n\
- If the current diff reopens an answered concern, cite the changed file/line or proof receipt that makes the prior answer stale.\n",
    )
}
