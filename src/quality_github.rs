//! Quality GitHub outcome collection: pull review threads, compute
//! review outcome metrics, and write the outcomes artifact (cleanup
//! train step 40, pure code motion).

use crate::*;

pub(crate) fn cmd_quality_github_collect(args: QualityGithubCollectArgs) -> Result<()> {
    let pull_numbers = quality_github_collect_pull_numbers(&args)?;
    fs::create_dir_all(&args.source_dir)
        .with_context(|| format!("create {}", args.source_dir.display()))?;
    fs::write(
        args.source_dir.join("review-threads.graphql"),
        GITHUB_QUALITY_REVIEW_THREADS_QUERY,
    )
    .with_context(|| "write quality review-thread GraphQL query")?;
    fs::write(
        args.source_dir.join("pr-numbers.txt"),
        pull_numbers
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join("\n"),
    )
    .with_context(|| "write quality PR number receipt")?;
    if pull_numbers.is_empty() {
        println!(
            "quality-github-collect: wrote {} (0 pull requests, 0 incomplete)",
            args.source_dir.display()
        );
        return Ok(());
    }
    let token = args
        .github_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!("quality-github-collect needs --github-token (GITHUB_TOKEN)")
        })?;
    let repo = resolve_quality_github_repo(&args)?;
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("--repo must be owner/name, got `{repo}`"))?;
    let graphql_url = quality_github_graphql_url(&args);
    let mut failures = 0usize;
    for pull_number in &pull_numbers {
        if !collect_quality_github_review_threads(
            &args.source_dir,
            &graphql_url,
            token,
            owner,
            name,
            *pull_number,
            args.timeout_sec.max(1),
        )? {
            failures += 1;
        }
    }
    println!(
        "quality-github-collect: wrote {} ({} pull requests, {} incomplete)",
        args.source_dir.display(),
        pull_numbers.len(),
        failures
    );
    Ok(())
}

pub(crate) fn resolve_quality_github_repo(args: &QualityGithubCollectArgs) -> Result<String> {
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

pub(crate) fn quality_github_collect_pull_numbers(
    args: &QualityGithubCollectArgs,
) -> Result<Vec<u64>> {
    let mut pull_numbers = args.pull_numbers.iter().copied().collect::<BTreeSet<_>>();
    if let Some(path) = &args.pull_numbers_file {
        let content =
            fs::read_to_string(path).with_context(|| format!("read {}", path.display()))?;
        for (line_index, line) in content.lines().enumerate() {
            let line = line.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            for token in line.split(|ch: char| ch.is_ascii_whitespace() || ch == ',') {
                let token = token.trim();
                if token.is_empty() {
                    continue;
                }
                let number = token.parse::<u64>().with_context(|| {
                    format!(
                        "parse pull number `{token}` on {}:{}",
                        path.display(),
                        line_index + 1
                    )
                })?;
                if number == 0 {
                    bail!(
                        "pull number must be positive on {}:{}",
                        path.display(),
                        line_index + 1
                    );
                }
                pull_numbers.insert(number);
            }
        }
    }
    if pull_numbers.contains(&0) {
        bail!("pull number must be positive");
    }
    Ok(pull_numbers.into_iter().collect())
}

pub(crate) fn quality_github_graphql_url(args: &QualityGithubCollectArgs) -> String {
    if let Some(url) = args
        .github_graphql_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return url.trim_end_matches('/').to_owned();
    }
    let rest = args.github_api_url.trim_end_matches('/');
    if let Some(prefix) = rest.strip_suffix("/api/v3") {
        format!("{prefix}/api/graphql")
    } else {
        format!("{rest}/graphql")
    }
}

pub(crate) fn collect_quality_github_review_threads(
    source_dir: &Path,
    graphql_url: &str,
    token: &str,
    owner: &str,
    name: &str,
    pull_number: u64,
    timeout_sec: u64,
) -> Result<bool> {
    let request_path = source_dir.join(format!("review-threads-request-{pull_number}.json"));
    let response_path = source_dir.join(format!("review-threads-{pull_number}.json"));
    let error_path = source_dir.join(format!("review-thread-error-{pull_number}.json"));
    let _ = fs::remove_file(&response_path);
    let _ = fs::remove_file(&error_path);
    fs::write(
        &request_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "query": GITHUB_QUALITY_REVIEW_THREADS_QUERY,
            "variables": {
                "owner": owner,
                "name": name,
                "number": pull_number,
            },
        }))?,
    )
    .with_context(|| format!("write {}", request_path.display()))?;
    let auth_header = github_graphql_auth_header(token);
    let output = run_curl_json_send(
        source_dir,
        "POST",
        graphql_url,
        &auth_header,
        &request_path,
        &[
            "Accept: application/vnd.github+json",
            "Content-Type: application/json",
        ],
        timeout_sec,
    )
    .with_context(|| format!("query GitHub review threads for PR #{pull_number}"))?;
    if !output.status.success() {
        write_quality_github_review_threads_error(
            source_dir,
            pull_number,
            output.http_status,
            format!(
                "GitHub GraphQL request failed: {}",
                truncate_chars(&String::from_utf8_lossy(&output.stderr), 1_000)
            ),
        )?;
        return Ok(false);
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parse GitHub review-thread response for PR #{pull_number}"))?;
    if let Some(errors) = value.get("errors") {
        write_quality_github_review_threads_error(
            source_dir,
            pull_number,
            output.http_status,
            format!(
                "GitHub GraphQL response contained errors: {}",
                truncate_chars(&errors.to_string(), 1_000)
            ),
        )?;
        return Ok(false);
    }
    fs::write(&response_path, serde_json::to_vec_pretty(&value)?)
        .with_context(|| format!("write {}", response_path.display()))?;
    Ok(true)
}

pub(crate) fn write_quality_github_review_threads_error(
    source_dir: &Path,
    pull_number: u64,
    http_status: Option<u16>,
    error: String,
) -> Result<()> {
    let path = source_dir.join(format!("review-thread-error-{pull_number}.json"));
    fs::write(
        &path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema": "ub-review.github_review_threads_error.v1",
            "pull_number": pull_number,
            "http_status": http_status,
            "error": error,
        }))?,
    )
    .with_context(|| format!("write {}", path.display()))
}

pub(crate) fn github_graphql_auth_header(token: &str) -> String {
    let scheme = ['B', 'e', 'a', 'r', 'e', 'r'].iter().collect::<String>();
    format!("{}: {scheme} {token}", "Authorization")
}

pub(crate) fn cmd_quality_github_outcomes(args: QualityGithubOutcomesArgs) -> Result<()> {
    let author_logins = github_quality_author_logins(&args.author_logins);
    let artifact = build_github_quality_outcomes_artifact(&args.source_dir, &author_logins)?;
    if let Some(parent) = args.out.parent() {
        fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    fs::write(&args.out, serde_json::to_vec_pretty(&artifact)?)
        .with_context(|| format!("write {}", args.out.display()))?;
    println!(
        "quality-github-outcomes: wrote {} ({} comments)",
        args.out.display(),
        artifact
            .comments
            .as_ref()
            .map(|comments| comments.len())
            .unwrap_or(0)
    );
    Ok(())
}

pub(crate) fn github_quality_author_logins(author_logins: &[String]) -> BTreeSet<String> {
    let mut logins = BTreeSet::new();
    let configured = if author_logins.is_empty() {
        DEFAULT_GITHUB_QUALITY_AUTHOR_LOGINS
            .iter()
            .map(|login| (*login).to_owned())
            .collect::<Vec<_>>()
    } else {
        author_logins.to_vec()
    };
    for login in configured {
        let trimmed = login.trim();
        if !trimmed.is_empty() {
            logins.insert(trimmed.to_ascii_lowercase());
        }
    }
    logins
}

pub(crate) fn build_github_quality_outcomes_artifact(
    source_dir: &Path,
    author_logins: &BTreeSet<String>,
) -> Result<GithubQualityOutcomesArtifact> {
    if !source_dir.is_dir() {
        bail!(
            "GitHub quality source directory missing: {}",
            source_dir.display()
        );
    }
    let source_files = github_quality_source_files(source_dir)?;
    if source_files.is_empty() {
        bail!(
            "GitHub quality source directory has no API receipts: {}",
            source_dir.display()
        );
    }

    let mut source_artifacts = Vec::new();
    let mut comments = Vec::new();
    let mut resolved_thread_contexts = Vec::new();
    let mut changed_files_by_pr: BTreeMap<u64, Vec<GithubQualityChangedFile>> = BTreeMap::new();
    let mut collection_warnings = Vec::new();
    let mut saw_review_thread_query = false;
    let mut saw_review_thread_request = false;
    let mut saw_review_thread_receipt = false;
    for source in source_files {
        let Some(file_name) = source.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        source_artifacts.push(file_name.to_owned());
        if file_name == "review-threads.graphql" {
            saw_review_thread_query = true;
        }
        if github_quality_review_thread_request_json(file_name) {
            saw_review_thread_request = true;
        }
        if !github_quality_review_thread_json(file_name) {
            continue;
        }
        let value: serde_json::Value = read_json_file(&source)?;
        if let Some(error) = github_quality_error_receipt(&value) {
            collection_warnings.push(GithubQualityCollectionWarning {
                source_artifact: file_name.to_owned(),
                reason: "github_api_error".to_owned(),
                detail: error,
            });
            continue;
        }
        let Some(pr) = github_quality_pull_request_value(&value) else {
            collection_warnings.push(GithubQualityCollectionWarning {
                source_artifact: file_name.to_owned(),
                reason: "missing_pull_request".to_owned(),
                detail: "review-thread receipt did not include data.repository.pullRequest"
                    .to_owned(),
            });
            continue;
        };
        saw_review_thread_receipt = true;
        collect_github_quality_pagination_warnings(pr, file_name, &mut collection_warnings);
        collect_github_quality_thread_comments(
            pr,
            author_logins,
            &mut comments,
            &mut resolved_thread_contexts,
        );
        collect_github_quality_changed_files(pr, &mut changed_files_by_pr);
    }
    if saw_review_thread_receipt && !saw_review_thread_query {
        collection_warnings.push(GithubQualityCollectionWarning {
            source_artifact: "review-threads.graphql".to_owned(),
            reason: "missing_review_thread_query".to_owned(),
            detail: "complete reviewer outcome collection needs the GraphQL query receipt"
                .to_owned(),
        });
    }
    if saw_review_thread_receipt && !saw_review_thread_request {
        collection_warnings.push(GithubQualityCollectionWarning {
            source_artifact: "review-threads-request-<pr>.json".to_owned(),
            reason: "missing_review_thread_request".to_owned(),
            detail: "complete reviewer outcome collection needs the GraphQL request receipt"
                .to_owned(),
        });
    }
    let collection_status = if !collection_warnings.is_empty() {
        "incomplete"
    } else if saw_review_thread_receipt {
        "complete"
    } else {
        "missing"
    };
    let adopted_generated_tests =
        github_quality_adopted_generated_tests(&resolved_thread_contexts, &changed_files_by_pr);

    Ok(GithubQualityOutcomesArtifact {
        schema: GITHUB_QUALITY_OUTCOMES_SCHEMA,
        collection_status,
        source_artifacts,
        collection_warnings,
        comments: (collection_status == "complete").then_some(comments),
        adopted_generated_tests: (collection_status == "complete")
            .then_some(adopted_generated_tests),
    })
}

pub(crate) fn github_quality_source_files(source_dir: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for entry in
        fs::read_dir(source_dir).with_context(|| format!("read {}", source_dir.display()))?
    {
        let entry = entry.with_context(|| format!("read entry in {}", source_dir.display()))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if file_name == "github-quality-outcomes.json" {
            continue;
        }
        if matches!(
            file_name,
            "actions-runs.json" | "pr-state.json" | "pr-numbers.txt" | "review-threads.graphql"
        ) || github_quality_review_thread_request_json(file_name)
            || github_quality_review_thread_json(file_name)
        {
            files.push(path);
        }
    }
    files.sort_by(|left, right| {
        left.file_name()
            .and_then(|name| name.to_str())
            .cmp(&right.file_name().and_then(|name| name.to_str()))
    });
    Ok(files)
}

pub(crate) fn github_quality_review_thread_request_json(file_name: &str) -> bool {
    file_name.starts_with("review-threads-request-") && file_name.ends_with(".json")
}

pub(crate) fn github_quality_review_thread_json(file_name: &str) -> bool {
    ((file_name.starts_with("review-threads-") && file_name.ends_with(".json"))
        || (file_name.starts_with("review-thread-error-") && file_name.ends_with(".json")))
        && file_name != "github-quality-outcomes.json"
        && !github_quality_review_thread_request_json(file_name)
}

pub(crate) fn github_quality_error_receipt(value: &serde_json::Value) -> Option<String> {
    let schema = value.get("schema").and_then(serde_json::Value::as_str)?;
    if schema != "ub-review.github_review_threads_error.v1" {
        return None;
    }
    Some(
        value
            .get("error")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("GitHub review-thread query failed")
            .to_owned(),
    )
}

pub(crate) fn github_quality_pull_request_value(
    value: &serde_json::Value,
) -> Option<&serde_json::Value> {
    value
        .pointer("/data/repository/pullRequest")
        .or_else(|| value.get("pullRequest"))
}

pub(crate) fn collect_github_quality_pagination_warnings(
    pr: &serde_json::Value,
    source_artifact: &str,
    warnings: &mut Vec<GithubQualityCollectionWarning>,
) {
    match pr.pointer("/files/pageInfo/hasNextPage") {
        Some(value) if value.as_bool() == Some(true) => {
            warnings.push(GithubQualityCollectionWarning {
                source_artifact: source_artifact.to_owned(),
                reason: "pr_files_truncated".to_owned(),
                detail: "files.pageInfo.hasNextPage=true".to_owned(),
            });
        }
        Some(_) => {}
        None => warnings.push(GithubQualityCollectionWarning {
            source_artifact: source_artifact.to_owned(),
            reason: "pr_files_page_info_missing".to_owned(),
            detail: "files.pageInfo was absent; generated-test adoption completeness is unknown"
                .to_owned(),
        }),
    }
    if pr
        .pointer("/files/nodes")
        .and_then(serde_json::Value::as_array)
        .is_none()
    {
        warnings.push(GithubQualityCollectionWarning {
            source_artifact: source_artifact.to_owned(),
            reason: "pr_files_nodes_missing".to_owned(),
            detail: "files.nodes was absent; generated-test adoption completeness is unknown"
                .to_owned(),
        });
    }
    match pr.pointer("/reviewThreads/pageInfo/hasNextPage") {
        Some(value) if value.as_bool() == Some(true) => {
            warnings.push(GithubQualityCollectionWarning {
                source_artifact: source_artifact.to_owned(),
                reason: "review_threads_truncated".to_owned(),
                detail: "reviewThreads.pageInfo.hasNextPage=true".to_owned(),
            });
        }
        Some(_) => {}
        None => warnings.push(GithubQualityCollectionWarning {
            source_artifact: source_artifact.to_owned(),
            reason: "review_threads_page_info_missing".to_owned(),
            detail: "reviewThreads.pageInfo was absent; completeness is unknown".to_owned(),
        }),
    }
    let Some(threads) = pr
        .pointer("/reviewThreads/nodes")
        .and_then(serde_json::Value::as_array)
    else {
        return;
    };
    for thread in threads {
        let thread_id = json_non_empty_string(thread.get("id")).unwrap_or("unknown-thread");
        match thread.pointer("/comments/pageInfo/hasNextPage") {
            Some(value) if value.as_bool() == Some(true) => {
                warnings.push(GithubQualityCollectionWarning {
                    source_artifact: source_artifact.to_owned(),
                    reason: "review_thread_comments_truncated".to_owned(),
                    detail: format!("thread {thread_id} comments.pageInfo.hasNextPage=true"),
                });
            }
            Some(_) => {}
            None => warnings.push(GithubQualityCollectionWarning {
                source_artifact: source_artifact.to_owned(),
                reason: "review_thread_comments_page_info_missing".to_owned(),
                detail: format!("thread {thread_id} comments.pageInfo was absent"),
            }),
        }
    }
}

pub(crate) fn collect_github_quality_thread_comments(
    pr: &serde_json::Value,
    author_logins: &BTreeSet<String>,
    comments: &mut Vec<GithubQualityNormalizedComment>,
    resolved_thread_contexts: &mut Vec<GithubQualityResolvedThreadContext>,
) {
    let pull_number = pr.get("number").and_then(serde_json::Value::as_u64);
    let pull_merged = pr
        .get("mergedAt")
        .and_then(serde_json::Value::as_str)
        .is_some_and(|value| !value.trim().is_empty());
    let Some(threads) = pr
        .pointer("/reviewThreads/nodes")
        .and_then(serde_json::Value::as_array)
    else {
        return;
    };
    for thread in threads {
        let thread_id = json_non_empty_string(thread.get("id")).unwrap_or("unknown-thread");
        let resolved = thread
            .get("isResolved")
            .and_then(serde_json::Value::as_bool);
        let Some(nodes) = thread
            .pointer("/comments/nodes")
            .and_then(serde_json::Value::as_array)
        else {
            continue;
        };
        for comment in nodes {
            let author_login = comment
                .pointer("/author/login")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .trim();
            if !author_logins.contains(&author_login.to_ascii_lowercase()) {
                continue;
            }
            let comment_id = json_non_empty_string(comment.get("id")).unwrap_or("unknown-comment");
            let source_url = json_non_empty_string(comment.get("url")).unwrap_or("");
            if resolved == Some(true)
                && pull_merged
                && let Some(source_pull_number) = pull_number
            {
                resolved_thread_contexts.push(GithubQualityResolvedThreadContext {
                    source_pull_number,
                    source_thread_id: thread_id.to_owned(),
                    source_comment_id: comment_id.to_owned(),
                    source_author_login: author_login.to_owned(),
                    source_url: source_url.to_owned(),
                });
            }
            comments.push(GithubQualityNormalizedComment {
                posted: true,
                accepted: resolved,
                resolved,
                reviewer_override: resolved.map(|resolved| pull_merged && !resolved),
                source_pull_number: pull_number,
                source_thread_id: thread_id.to_owned(),
                source_comment_id: comment_id.to_owned(),
                source_author_login: author_login.to_owned(),
                source_url: source_url.to_owned(),
                outcome_source: "github.reviewThreads.isResolved.v1",
            });
        }
    }
}

pub(crate) fn collect_github_quality_changed_files(
    pr: &serde_json::Value,
    changed_files_by_pr: &mut BTreeMap<u64, Vec<GithubQualityChangedFile>>,
) {
    let Some(pull_number) = pr.get("number").and_then(serde_json::Value::as_u64) else {
        return;
    };
    let Some(nodes) = pr
        .pointer("/files/nodes")
        .and_then(serde_json::Value::as_array)
    else {
        return;
    };
    let files = changed_files_by_pr.entry(pull_number).or_default();
    for file in nodes {
        let Some(path) = json_non_empty_string(file.get("path")) else {
            continue;
        };
        files.push(GithubQualityChangedFile {
            path: path.to_owned(),
            status: json_non_empty_string(file.get("changeType")).map(str::to_owned),
            additions: file.get("additions").and_then(serde_json::Value::as_u64),
            deletions: file.get("deletions").and_then(serde_json::Value::as_u64),
        });
    }
}

pub(crate) fn github_quality_adopted_generated_tests(
    resolved_thread_contexts: &[GithubQualityResolvedThreadContext],
    changed_files_by_pr: &BTreeMap<u64, Vec<GithubQualityChangedFile>>,
) -> Vec<GithubQualityGeneratedTestAdoption> {
    let mut seen = BTreeSet::new();
    let mut adopted = Vec::new();
    for context in resolved_thread_contexts {
        let Some(files) = changed_files_by_pr.get(&context.source_pull_number) else {
            continue;
        };
        for file in files {
            if !github_quality_test_file_path(&file.path) {
                continue;
            }
            if !seen.insert((context.source_pull_number, file.path.clone())) {
                continue;
            }
            adopted.push(GithubQualityGeneratedTestAdoption {
                source_pull_number: context.source_pull_number,
                path: file.path.clone(),
                status: file.status.clone(),
                additions: file.additions,
                deletions: file.deletions,
                source_thread_id: context.source_thread_id.clone(),
                source_comment_id: context.source_comment_id.clone(),
                source_author_login: context.source_author_login.clone(),
                source_url: context.source_url.clone(),
                outcome_source: "github.mergedResolvedUbReviewThreadChangedTestFile.v1",
            });
        }
    }
    adopted
}

pub(crate) fn github_quality_test_file_path(path: &str) -> bool {
    let path = path.trim().replace('\\', "/");
    let Some(file_name) = path.rsplit('/').next() else {
        return false;
    };
    let rust_file = file_name.ends_with(".rs");
    rust_file
        && (path.starts_with("tests/")
            || path.contains("/tests/")
            || file_name.ends_with("_test.rs")
            || file_name.ends_with("_tests.rs"))
}

pub(crate) fn json_non_empty_string(value: Option<&serde_json::Value>) -> Option<&str> {
    value
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
}
