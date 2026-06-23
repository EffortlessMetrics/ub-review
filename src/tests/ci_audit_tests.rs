// CI audit test cluster, extracted from src/main.rs mod tests (#597).
// Resolves symbols via `super::*` (the mod tests scope) and `crate::*` for
// production functions.
use super::*;
use crate::*;

    // ----------------------------------------------------------------------

    const CI_FIXTURE_WORKFLOW: &str = r#"name: CI

on:
  push:
    branches: [main]
    paths-ignore:
      - "docs/**"
  pull_request:
    paths:
      - "src/**"
      - "Cargo.toml"
  workflow_dispatch:

permissions: read-all

jobs:
  fmt:
    runs-on: ubuntu-latest
    timeout-minutes: 10
    steps:
      - uses: actions/checkout@v4
      - run: cargo fmt --check
  test:
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - uses: actions/checkout@v4
      - run: cargo test
  e2e:
    name: End to end
    timeout-minutes: 45
    steps:
      - uses: actions/checkout@v4
      - run: ./scripts/e2e.sh
"#;

    const CI_SENSITIVE_WORKFLOW: &str = r#"name: Sensitive CI

on: [pull_request]

permissions:
  contents: read

env:
  GLOBAL_TOKEN: ${{ secrets['GLOBAL_TOKEN'] }}

jobs:
  wide:
    runs-on: ubuntu-latest
    permissions:
      contents: write
      id-token: write
    steps:
      - run: echo "${{ secrets.DEPLOY_TOKEN }}"
  notify:
    runs-on: ubuntu-latest
    steps:
      - run: echo "${{ secrets[env.NOTIFY_SECRET] }}"
  docs:
    runs-on: ubuntu-latest
    permissions: { contents: read }
    steps:
      - run: cargo test --doc
  nullperms:
    runs-on: ubuntu-latest
    permissions: null
    steps:
      - run: cargo check
"#;

    const CI_MISSING_PERMISSIONS_WORKFLOW: &str = r#"name: Missing permissions CI

on: [pull_request]

jobs:
  integration:
    runs-on: ubuntu-latest
    steps:
      - run: cargo test --workspace
"#;

    const CI_MATRIX_WORKFLOW: &str = r#"name: Matrix CI

on: [pull_request]

jobs:
  test:
    name: Unit tests
    runs-on: ${{ matrix.os }}
    strategy:
      fail-fast: false
      matrix:
        os: [ubuntu-latest, windows-latest]
        rust:
          - stable
          - beta
    steps:
      - run: cargo test --workspace
  generated:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        package: ${{ fromJSON(needs.plan.outputs.packages) }}
    steps:
      - run: cargo test -p ${{ matrix.package }}
  adjusted:
    runs-on: ubuntu-latest
    strategy:
      matrix:
        os: [ubuntu-latest, windows-latest]
        include:
          - os: ubuntu-latest
            extra: true
    steps:
      - run: cargo check
"#;

    fn ci_api_job_value(name: &str, conclusion: &str, seconds: u64) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "conclusion": conclusion,
            "started_at": "2026-01-01T00:00:00Z",
            "completed_at": format!("2026-01-01T00:{:02}:{:02}Z", seconds / 60, seconds % 60),
        })
    }

    fn ci_api_job_value_without_completion(name: &str, conclusion: &str) -> serde_json::Value {
        serde_json::json!({
            "name": name,
            "conclusion": conclusion,
            "started_at": "2026-01-01T00:00:00Z",
            "completed_at": null,
        })
    }

    fn ci_fixture_workflows() -> Vec<crate::CiApiWorkflow> {
        let value = serde_json::json!({
            "workflows": [
                {"id": 1, "name": "CI", "path": ".github/workflows/ci.yml", "state": "active"}
            ]
        });
        crate::parse_ci_api_array(&value, "workflows").0
    }

    fn ci_no_required_checks() -> crate::CiRequiredChecks {
        crate::CiRequiredChecks::default()
    }

    fn ci_required_checks(contexts: &[(&str, &str)]) -> crate::CiRequiredChecks {
        crate::CiRequiredChecks {
            contexts: contexts
                .iter()
                .map(|(context, source)| ((*context).to_owned(), (*source).to_owned()))
                .collect(),
            default_source: Some("branch-protection".to_owned()),
            evidence_gaps: Vec::new(),
        }
    }

    fn spawn_fake_ci_required_checks_api()
    -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || -> Result<Vec<String>> {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut requests = Vec::new();
            while requests.len() < 3 {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        requests.push(handle_fake_ci_required_checks_request(stream)?);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            bail!(
                                "fake audit-ci API received {} of 3 requests",
                                requests.len()
                            );
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            Ok(requests)
        });
        Ok((url, handle))
    }

    fn spawn_fake_ci_required_checks_404_api()
    -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || -> Result<Vec<String>> {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut requests = Vec::new();
            while requests.len() < 3 {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        requests.push(handle_fake_ci_required_checks_404_request(stream)?);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            bail!(
                                "fake audit-ci 404 API received {} of 3 requests",
                                requests.len()
                            );
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            Ok(requests)
        });
        Ok((url, handle))
    }

    fn handle_fake_ci_required_checks_404_request(mut stream: TcpStream) -> Result<String> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                bail!("fake audit-ci 404 request ended before headers finished");
            }
            headers.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let request_line = headers.lines().next().unwrap_or_default();
        let (status, response_body) = if request_line == "GET /repos/acme/widgets HTTP/1.1" {
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({"default_branch": "main"}))?,
            )
        } else if request_line.contains("/protection/required_status_checks")
            || request_line.contains("/rulesets?includes_parents=true&per_page=100")
        {
            (
                "HTTP/1.1 404 Not Found",
                serde_json::to_vec(&serde_json::json!({"message": "Not Found"}))?,
            )
        } else {
            bail!("unexpected fake audit-ci 404 request: {request_line}");
        };
        write!(
            stream,
            "{status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        )?;
        stream.write_all(&response_body)?;
        Ok(headers)
    }

    fn handle_fake_ci_required_checks_request(mut stream: TcpStream) -> Result<String> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                bail!("fake audit-ci request ended before headers finished");
            }
            headers.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let request_line = headers.lines().next().unwrap_or_default();
        let response_body = if request_line == "GET /repos/acme/widgets HTTP/1.1" {
            serde_json::to_vec(&serde_json::json!({"default_branch": "release/2026"}))?
        } else if request_line.contains(
            "/repos/acme/widgets/branches/release%2F2026/protection/required_status_checks",
        ) {
            serde_json::to_vec(&serde_json::json!({
                "contexts": ["fmt"],
                "checks": [{"context": "test"}]
            }))?
        } else if request_line
            .contains("/repos/acme/widgets/rulesets?includes_parents=true&per_page=100")
        {
            serde_json::to_vec(&serde_json::json!([
                {
                    "name": "main rules",
                    "target": "branch",
                    "enforcement": "active",
                    "conditions": {
                        "ref_name": {
                            "include": ["~DEFAULT_BRANCH"],
                            "exclude": []
                        }
                    },
                    "rules": [
                        {
                            "type": "required_status_checks",
                            "parameters": {
                                "required_status_checks": [{"context": "End to end"}]
                            }
                        }
                    ]
                }
            ]))?
        } else {
            bail!("unexpected fake audit-ci request: {request_line}");
        };
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        )?;
        stream.write_all(&response_body)?;
        Ok(headers)
    }

    /// Canned run spec: (run id, [(job name, conclusion, duration seconds)]).
    type CiRunSpec<'a> = (u64, Vec<(&'a str, &'a str, u64)>);

    fn ci_fixture_run_with_jobs(
        id: u64,
        jobs: Vec<serde_json::Value>,
    ) -> Result<crate::CiRunWithJobs> {
        let run_value = serde_json::json!({
            "id": id,
            "workflow_id": 1,
            "run_attempt": 1,
            "conclusion": "cancelled",
            "event": "pull_request",
        });
        let jobs_value = serde_json::json!({
            "total_count": jobs.len(),
            "jobs": jobs,
        });
        Ok(crate::CiRunWithJobs {
            run: serde_json::from_value(run_value).context("parse canned run json")?,
            jobs: crate::parse_ci_api_array(&jobs_value, "jobs").0,
        })
    }

    fn ci_fixture_runs(specs: &[CiRunSpec<'_>]) -> Result<Vec<crate::CiRunWithJobs>> {
        let mut runs = Vec::new();
        for (id, jobs) in specs {
            let run_value = serde_json::json!({
                "id": id,
                "workflow_id": 1,
                "run_attempt": 1,
                "conclusion": "success",
                "event": "pull_request",
            });
            let jobs_value = serde_json::json!({
                "total_count": jobs.len(),
                "jobs": jobs
                    .iter()
                    .map(|(name, conclusion, seconds)| ci_api_job_value(name, conclusion, *seconds))
                    .collect::<Vec<_>>(),
            });
            runs.push(crate::CiRunWithJobs {
                run: serde_json::from_value(run_value).context("parse canned run json")?,
                jobs: crate::parse_ci_api_array(&jobs_value, "jobs").0,
            });
        }
        Ok(runs)
    }

    fn ci_fixture_correlation_runs() -> Result<Vec<crate::CiRunWithJobs>> {
        ci_fixture_runs(&[
            (
                101,
                vec![
                    ("fmt", "success", 60),
                    ("test", "success", 120),
                    ("e2e", "success", 600),
                ],
            ),
            (
                102,
                vec![
                    ("fmt", "success", 60),
                    ("test", "success", 120),
                    ("e2e", "failure", 600),
                ],
            ),
            (
                103,
                vec![
                    ("fmt", "success", 60),
                    ("test", "failure", 120),
                    ("e2e", "failure", 600),
                ],
            ),
            (
                104,
                vec![
                    ("fmt", "failure", 60),
                    ("test", "success", 120),
                    ("e2e", "success", 600),
                ],
            ),
        ])
    }

    fn ci_tier_evidence(
        job: &str,
        runs: usize,
        independent: usize,
        p50: Option<u64>,
    ) -> crate::CiTierEvidence {
        crate::CiTierEvidence {
            job: job.to_owned(),
            workflow_path: ".github/workflows/ci.yml".to_owned(),
            workflow_name: "CI".to_owned(),
            uses: Vec::new(),
            permissions: Some(serde_json::json!({"contents": "read"})),
            uses_secrets: Vec::new(),
            triggers: vec!["pull_request".to_owned()],
            path_filters: Vec::new(),
            required_check: Some(false),
            required_check_source: "branch-protection".to_owned(),
            required_check_context: None,
            history_available: true,
            runs,
            independent_failures: independent,
            duration_p50_sec: p50,
        }
    }

    #[test]
    fn ci_workflow_scan_extracts_triggers_paths_timeouts_and_uses() -> Result<()> {
        let scan = crate::scan_workflow_text(".github/workflows/ci.yml", CI_FIXTURE_WORKFLOW);
        assert!(scan.triggers.contains(&"push:main".to_owned()));
        assert!(scan.triggers.contains(&"pull_request".to_owned()));
        assert!(scan.triggers.contains(&"workflow_dispatch".to_owned()));
        assert_eq!(
            scan.path_filters,
            vec!["src/**".to_owned(), "Cargo.toml".to_owned()]
        );
        assert_eq!(scan.path_ignore_filters, vec!["docs/**".to_owned()]);
        assert!(!scan.cancel_in_progress);
        let ids: Vec<&str> = scan.yaml_jobs.iter().map(|job| job.id.as_str()).collect();
        assert_eq!(ids, vec!["fmt", "test", "e2e"]);
        let fmt = scan.yaml_jobs.first().context("fmt yaml job present")?;
        assert_eq!(fmt.runs_on, vec!["ubuntu-latest".to_owned()]);
        assert_eq!(fmt.timeout_minutes, Some(10));
        assert!(fmt.uses.contains(&"actions/checkout@v4".to_owned()));
        let e2e = scan.yaml_jobs.last().context("e2e yaml job present")?;
        assert_eq!(e2e.name.as_deref(), Some("End to end"));
        assert_eq!(e2e.timeout_minutes, Some(45));
        Ok(())
    }

    #[test]
    fn ci_workflow_scan_extracts_literal_matrix_size_with_gaps() -> Result<()> {
        let scan = crate::scan_workflow_text(".github/workflows/matrix.yml", CI_MATRIX_WORKFLOW);
        let test = scan
            .yaml_jobs
            .iter()
            .find(|job| job.id == "test")
            .context("test matrix job")?;
        assert_eq!(test.matrix_size, 4);

        let generated = scan
            .yaml_jobs
            .iter()
            .find(|job| job.id == "generated")
            .context("generated matrix job")?;
        assert_eq!(generated.matrix_size, 1);
        assert!(
            scan.evidence_gaps
                .iter()
                .any(|gap| gap.contains("generated") && gap.contains("dynamic expression")),
            "dynamic matrix gap missing: {:?}",
            scan.evidence_gaps
        );

        let adjusted = scan
            .yaml_jobs
            .iter()
            .find(|job| job.id == "adjusted")
            .context("adjusted matrix job")?;
        assert_eq!(adjusted.matrix_size, 2);
        assert!(
            scan.evidence_gaps
                .iter()
                .any(|gap| gap.contains("adjusted") && gap.contains("include")),
            "include adjustment gap missing: {:?}",
            scan.evidence_gaps
        );
        Ok(())
    }

    #[test]
    fn ci_workflow_scan_extracts_permissions_and_secret_refs() -> Result<()> {
        let scan = crate::scan_workflow_text(".github/workflows/ci.yml", CI_SENSITIVE_WORKFLOW);
        assert_eq!(
            scan.permissions,
            Some(serde_json::json!({"contents": "read"}))
        );
        assert_eq!(scan.uses_secrets, vec!["GLOBAL_TOKEN".to_owned()]);
        let wide = scan
            .yaml_jobs
            .iter()
            .find(|job| job.id == "wide")
            .context("wide job")?;
        assert_eq!(
            wide.permissions,
            Some(serde_json::json!({"contents": "write", "id-token": "write"}))
        );
        assert_eq!(wide.uses_secrets, vec!["DEPLOY_TOKEN".to_owned()]);

        let notify = scan
            .yaml_jobs
            .iter()
            .find(|job| job.id == "notify")
            .context("notify job")?;
        assert_eq!(notify.permissions, None);
        assert_eq!(
            notify.uses_secrets,
            vec![crate::CI_AUDIT_DYNAMIC_SECRET_REF.to_owned()]
        );

        let docs = scan
            .yaml_jobs
            .iter()
            .find(|job| job.id == "docs")
            .context("docs job")?;
        assert_eq!(
            docs.permissions,
            Some(serde_json::json!({"contents": "read"}))
        );
        assert!(docs.uses_secrets.is_empty());

        let nullperms = scan
            .yaml_jobs
            .iter()
            .find(|job| job.id == "nullperms")
            .context("nullperms job")?;
        assert_eq!(nullperms.permissions, Some(serde_json::Value::Null));
        Ok(())
    }

    #[test]
    fn ci_workflow_permissions_key_requires_top_level_indent() {
        assert_eq!(
            crate::ci_workflow_permissions_value(0, "permissions:"),
            Some("")
        );
        assert_eq!(
            crate::ci_workflow_permissions_value(0, "permissions: write-all"),
            Some(" write-all")
        );
        assert_eq!(
            crate::ci_workflow_permissions_value(2, "permissions: write-all"),
            None
        );
        assert_eq!(crate::ci_workflow_permissions_value(0, "jobs:"), None);
    }

    #[test]
    fn ci_parse_api_array_counts_dropped_items() {
        let value = serde_json::json!({
            "workflows": [
                {"id": 1, "name": "CI", "path": ".github/workflows/ci.yml"},
                {"id": "not-a-number"},
                {"name": "missing id"},
            ]
        });
        let (parsed, dropped): (Vec<crate::CiApiWorkflow>, usize) =
            crate::parse_ci_api_array(&value, "workflows");
        assert_eq!(parsed.len(), 1);
        assert_eq!(dropped, 2);
        let (parsed, dropped): (Vec<crate::CiApiWorkflow>, usize) =
            crate::parse_ci_api_array(&serde_json::json!({}), "workflows");
        assert!(parsed.is_empty());
        assert_eq!(dropped, 0);
    }

    #[test]
    fn ci_required_check_parsers_extract_branch_and_ruleset_contexts() {
        let branch = serde_json::json!({
            "contexts": ["fmt"],
            "checks": [
                {"context": "test", "app_id": 15368},
                {"context": "  "}
            ]
        });
        assert_eq!(
            crate::ci_required_check_contexts_from_branch_protection(&branch),
            BTreeSet::from(["fmt".to_owned(), "test".to_owned()])
        );

        let rulesets = serde_json::json!([
            {
                "name": "main",
                "target": "branch",
                "enforcement": "active",
                "conditions": {
                    "ref_name": {
                        "include": ["refs/heads/main"],
                        "exclude": []
                    }
                },
                "rules": [
                    {
                        "type": "required_status_checks",
                        "parameters": {
                            "required_status_checks": [
                                {"context": "End to end"},
                                {"context": ""}
                            ]
                        }
                    }
                ]
            },
            {
                "name": "inactive",
                "target": "branch",
                "enforcement": "evaluate",
                "conditions": {
                    "ref_name": {
                        "include": ["refs/heads/main"],
                        "exclude": []
                    }
                },
                "rules": [
                    {
                        "type": "required_status_checks",
                        "parameters": {
                            "required_status_checks": [{"context": "Advisory only"}]
                        }
                    }
                ]
            },
            {
                "name": "linear history",
                "target": "branch",
                "enforcement": "active",
                "conditions": {
                    "ref_name": {
                        "include": ["refs/heads/main"],
                        "exclude": []
                    }
                },
                "rules": [{"type": "required_linear_history"}]
            }
        ]);
        let (contexts, gaps) = crate::ci_required_check_contexts_from_rulesets(&rulesets, "main");
        assert_eq!(contexts, BTreeSet::from(["End to end".to_owned()]));
        assert!(gaps.is_empty(), "{gaps:?}");
        let (_contexts, gaps) = crate::ci_required_check_contexts_from_rulesets(
            &serde_json::json!([
                {"id": 1, "name": "summary-only ruleset"}
            ]),
            "main",
        );
        assert!(
            gaps.iter().any(|gap| gap.contains("omitted rule details")),
            "{gaps:?}"
        );
        let (_contexts, gaps) = crate::ci_required_check_contexts_from_rulesets(
            &serde_json::json!([
                {
                    "name": "missing applicability",
                    "rules": [
                        {
                            "type": "required_status_checks",
                            "parameters": {
                                "required_status_checks": [{"context": "unknown"}]
                            }
                        }
                    ]
                }
            ]),
            "main",
        );
        assert!(
            gaps.iter()
                .any(|gap| gap.contains("default-branch applicability")),
            "{gaps:?}"
        );
    }

    #[test]
    fn ci_fetch_required_checks_reads_branch_protection_and_rulesets() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (api_url, handle) = spawn_fake_ci_required_checks_api()?;
        let args = crate::AuditCiArgs {
            root: temp.path().to_path_buf(),
            out: temp.path().join("out"),
            repo: Some("acme/widgets".to_owned()),
            github_token: Some("test-token".to_owned()),
            github_api_url: api_url,
            window_days: 90,
            audit_cancel_events: None,
        };
        let required =
            crate::fetch_ci_required_checks(&args, "acme/widgets", "test-token", temp.path());
        let requests = handle
            .join()
            .map_err(|_| anyhow::anyhow!("fake audit-ci API thread panicked"))??;
        assert!(requests[0].starts_with("GET /repos/acme/widgets "));
        assert!(
            requests[1].contains(
                "/repos/acme/widgets/branches/release%2F2026/protection/required_status_checks"
            ),
            "{}",
            requests[1]
        );
        assert!(
            requests[2].contains("/repos/acme/widgets/rulesets?includes_parents=true&per_page=100"),
            "{}",
            requests[2]
        );
        assert_eq!(
            required.default_source.as_deref(),
            Some("branch-protection")
        );
        assert_eq!(
            required.contexts.get("fmt").map(String::as_str),
            Some("branch-protection")
        );
        assert_eq!(
            required.contexts.get("test").map(String::as_str),
            Some("branch-protection")
        );
        assert_eq!(
            required.contexts.get("End to end").map(String::as_str),
            Some("ruleset")
        );
        assert!(required.evidence_gaps.is_empty());
        Ok(())
    }

    #[test]
    fn ci_fetch_required_checks_treats_not_found_as_evidence_gap() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (api_url, handle) = spawn_fake_ci_required_checks_404_api()?;
        let args = crate::AuditCiArgs {
            root: temp.path().to_path_buf(),
            out: temp.path().join("out"),
            repo: Some("acme/widgets".to_owned()),
            github_token: Some("test-token".to_owned()),
            github_api_url: api_url,
            window_days: 90,
            audit_cancel_events: None,
        };
        let required =
            crate::fetch_ci_required_checks(&args, "acme/widgets", "test-token", temp.path());
        let requests = handle
            .join()
            .map_err(|_| anyhow::anyhow!("fake audit-ci 404 API thread panicked"))??;
        assert_eq!(requests.len(), 3);
        assert_eq!(required.default_source, None);
        assert!(required.contexts.is_empty());
        assert!(
            required
                .evidence_gaps
                .iter()
                .any(|gap| gap.contains("branch-protection required checks unreadable")),
            "{:?}",
            required.evidence_gaps
        );
        assert!(
            required
                .evidence_gaps
                .iter()
                .any(|gap| gap.contains("ruleset required checks unreadable")),
            "{:?}",
            required.evidence_gaps
        );
        Ok(())
    }

    #[test]
    fn ci_percentile_uses_nearest_rank() {
        let values: Vec<u64> = (1..=100).collect();
        assert_eq!(crate::ci_percentile(&values, 50.0), Some(50));
        assert_eq!(crate::ci_percentile(&values, 90.0), Some(90));
        assert_eq!(crate::ci_percentile(&values, 99.0), Some(99));
        assert_eq!(crate::ci_percentile(&[], 50.0), None);
    }

    #[test]
    fn ci_independent_failures_require_all_cheaper_jobs_passing() -> Result<()> {
        let stats =
            crate::compute_ci_job_stats(&ci_fixture_workflows(), &ci_fixture_correlation_runs()?);
        let e2e = stats
            .iter()
            .find(|stat| stat.job == "e2e")
            .context("e2e stats")?;
        assert_eq!(e2e.runs, 4);
        assert_eq!(e2e.duration_p50_sec, Some(600));
        // Run 102 counts (fmt and test passed); run 103 does not (cheaper test failed).
        assert_eq!(e2e.independent_failures, 1);
        assert_eq!(
            e2e.cheaper_jobs_compared,
            vec!["fmt".to_owned(), "test".to_owned()]
        );
        assert!(e2e.co_failing_jobs.contains(&"test".to_owned()));
        let test_job = stats
            .iter()
            .find(|stat| stat.job == "test")
            .context("test stats")?;
        // Run 103: test failed while cheaper fmt passed.
        assert_eq!(test_job.independent_failures, 1);
        assert_eq!(test_job.cheaper_jobs_compared, vec!["fmt".to_owned()]);
        let fmt = stats
            .iter()
            .find(|stat| stat.job == "fmt")
            .context("fmt stats")?;
        // Run 104: fmt is the cheapest job, so its failure is independent by definition.
        assert_eq!(fmt.independent_failures, 1);
        assert!(fmt.cheaper_jobs_compared.is_empty());
        Ok(())
    }

    #[test]
    fn ci_security_jobs_always_flag_for_human() {
        let decision = crate::classify_ci_job_tier(
            &ci_tier_evidence("CodeQL analyze", 500, 40, Some(60)),
            true,
        );
        assert_eq!(decision.tier, "flag-for-human");
        assert!(decision.reason.contains("codeql"));
        let mut evidence = ci_tier_evidence("build", 500, 40, Some(60));
        evidence.uses = vec!["acme/artifact-sign@v1".to_owned()];
        let decision = crate::classify_ci_job_tier(&evidence, true);
        assert_eq!(decision.tier, "flag-for-human");
        assert!(decision.reason.contains("sign"));
        // Concrete escapes from the adversarial review: each must flag even
        // with rich, healthy-looking history.
        for job in [
            "docker-push",
            "push-image",
            "tf-apply",
            "upload-sarif",
            "compliance-check",
            // Overmatch is acceptable by design: flag-for-human is the safe side.
            "push-docs-preview",
        ] {
            let decision =
                crate::classify_ci_job_tier(&ci_tier_evidence(job, 500, 40, Some(60)), true);
            assert_eq!(decision.tier, "flag-for-human", "job `{job}` must flag");
            assert!(
                decision.reason.contains("security-sensitive"),
                "job `{job}` reason: {}",
                decision.reason
            );
        }
    }

    #[test]
    fn ci_permissions_and_secrets_flag_for_human() {
        let mut evidence = ci_tier_evidence("integration", 500, 40, Some(60));
        evidence.permissions = None;
        let decision = crate::classify_ci_job_tier(&evidence, true);
        assert_eq!(decision.tier, "flag-for-human");
        assert!(decision.reason.contains("missing permissions"));

        let mut evidence = ci_tier_evidence("integration", 500, 40, Some(60));
        evidence.permissions = Some(serde_json::json!({"contents": "write"}));
        let decision = crate::classify_ci_job_tier(&evidence, true);
        assert_eq!(decision.tier, "flag-for-human");
        assert!(decision.reason.contains("write-scoped permissions"));

        let mut evidence = ci_tier_evidence("integration", 500, 40, Some(60));
        evidence.permissions = Some(serde_json::json!({"contents": "${{ inputs.scope }}"}));
        let decision = crate::classify_ci_job_tier(&evidence, true);
        assert_eq!(decision.tier, "flag-for-human");
        assert!(decision.reason.contains("ambiguous permissions"));

        let mut evidence = ci_tier_evidence("integration", 500, 40, Some(60));
        evidence.permissions = Some(serde_json::Value::Null);
        let decision = crate::classify_ci_job_tier(&evidence, true);
        assert_eq!(decision.tier, "flag-for-human");
        assert!(decision.reason.contains("ambiguous permissions"));

        let mut evidence = ci_tier_evidence("integration", 500, 40, Some(60));
        evidence.uses_secrets = vec!["DEPLOY_TOKEN".to_owned()];
        let decision = crate::classify_ci_job_tier(&evidence, true);
        assert_eq!(decision.tier, "flag-for-human");
        assert!(decision.reason.contains("DEPLOY_TOKEN"));

        let mut evidence = ci_tier_evidence("fmt", 200, 4, Some(45));
        evidence.permissions = Some(serde_json::json!({"contents": "read"}));
        let decision = crate::classify_ci_job_tier(&evidence, false);
        assert_eq!(decision.tier, "keep-required");
    }

    #[test]
    fn ci_cheap_job_with_independent_failures_keeps_required() {
        let decision =
            crate::classify_ci_job_tier(&ci_tier_evidence("fmt", 200, 4, Some(45)), false);
        assert_eq!(decision.tier, "keep-required");
        assert_eq!(decision.confidence, "high");
    }

    #[test]
    fn ci_proven_required_jobs_move_inside_ub_review_gate() {
        let mut evidence = ci_tier_evidence("fmt", 200, 4, Some(45));
        evidence.required_check = Some(true);
        evidence.required_check_source = "branch-protection".to_owned();
        evidence.required_check_context = Some("fmt".to_owned());
        let decision = crate::classify_ci_job_tier(&evidence, false);
        assert_eq!(decision.tier, "move-to-ub-review-required");
        assert_eq!(decision.confidence, "high");
        assert!(decision.reason.contains("already required as `fmt`"));
        assert!(decision.proposed_policy.contains("required = true"));

        let mut quiet = ci_tier_evidence("unit", 200, 0, Some(60));
        quiet.required_check = Some(true);
        quiet.required_check_source = "ruleset".to_owned();
        quiet.required_check_context = Some("Unit tests".to_owned());
        let decision = crate::classify_ci_job_tier(&quiet, false);
        assert_eq!(decision.tier, "move-to-ub-review-required");
        assert_eq!(decision.confidence, "low");
        assert!(decision.reason.contains("Unit tests"));
    }

    #[test]
    fn ci_required_move_tier_needs_exact_required_check_context() {
        let mut evidence = ci_tier_evidence("fmt", 200, 4, Some(45));
        evidence.required_check = Some(true);
        evidence.required_check_source = "unknown".to_owned();
        evidence.required_check_context = Some("fmt".to_owned());
        let decision = crate::classify_ci_job_tier(&evidence, false);
        assert_eq!(decision.tier, "keep-required");

        evidence.required_check_source = "branch-protection".to_owned();
        evidence.required_check_context = None;
        let decision = crate::classify_ci_job_tier(&evidence, false);
        assert_eq!(decision.tier, "keep-required");
    }

    #[test]
    fn ci_expensive_quiet_job_right_sizes_to_adaptive_by_data_volume() {
        let decision =
            crate::classify_ci_job_tier(&ci_tier_evidence("integration", 150, 0, Some(900)), true);
        assert_eq!(decision.tier, "adaptive");
        assert_eq!(decision.confidence, "medium");
        let decision =
            crate::classify_ci_job_tier(&ci_tier_evidence("integration", 60, 0, Some(900)), true);
        assert_eq!(decision.tier, "adaptive");
        assert_eq!(decision.confidence, "low");
    }

    #[test]
    fn ci_survivorship_caps_confidence_without_proven_sibling_signal() {
        let decision =
            crate::classify_ci_job_tier(&ci_tier_evidence("integration", 150, 0, Some(900)), false);
        assert_eq!(decision.tier, "adaptive");
        assert_eq!(decision.confidence, "low");
        assert!(decision.reason.contains("capped at low"));
    }

    #[test]
    fn ci_insufficient_history_is_never_adaptive() {
        let decision =
            crate::classify_ci_job_tier(&ci_tier_evidence("integration", 5, 0, Some(900)), true);
        assert_eq!(decision.tier, "flag-for-human");
        assert_eq!(decision.confidence, "low");
        assert!(decision.reason.contains("insufficient history"));
    }

    #[test]
    fn ci_tokenless_evidence_flags_for_human() {
        let mut evidence = ci_tier_evidence("integration", 0, 0, None);
        evidence.history_available = false;
        let decision = crate::classify_ci_job_tier(&evidence, false);
        assert_eq!(decision.tier, "flag-for-human");
        assert!(decision.reason.contains("tokenless"));
    }

    fn write_setup_ci_fixture(dir: &Path) -> Result<()> {
        fs::create_dir_all(dir)?;
        let job = |name: &str| {
            serde_json::json!({
                "workflow": ".github/workflows/ci.yml",
                "job": name,
                "name": name,
                "triggers": ["pull_request"],
                "path_filters": [],
                "matrix_size": 1,
                "timeout_minutes": 30,
                "permissions": null,
                "uses_secrets": [],
                "required_check": null,
                "required_check_source": "unknown",
                "required_check_context": null,
            })
        };
        fs::write(
            dir.join("inventory.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "ub-review.ci_inventory.v1",
                "generated_at": "2026-06-07T00:00:00Z",
                "repo": "acme/widgets",
                "window_days": 90,
                "jobs": [job("integration"), job("unit"), job("fmt"), job("deploy")],
                "evidence_gaps": [],
            }))?,
        )?;
        let recommendation = |name: &str, tier: &str| {
            serde_json::json!({
                "job": name,
                "workflow": ".github/workflows/ci.yml",
                "tier": tier,
                "positioned_to_catch": "regressions in its scope",
                "has_caught": "2 independent failures in the window",
                "receipts": [format!("ci-audit/correlation.json#{name}")],
                "proposed_policy": "per tier",
                "confidence": "medium",
                "judgment": "deterministic",
                "reason": "expensive and quiet on unrelated diffs",
                "report_note": "",
            })
        };
        fs::write(
            dir.join("recommendations.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "schema": "ub-review.ci_recommendations.v1",
                "repo": "acme/widgets",
                "window_days": 90,
                "jobs": [
                    recommendation("integration", "adaptive"),
                    recommendation("unit", "move-to-ub-review-required"),
                    recommendation("fmt", "keep-required"),
                    recommendation("deploy", "flag-for-human"),
                ],
                "evidence_gaps": [],
            }))?,
        )?;
        for (name, schema) in [
            ("history.json", "ub-review.ci_history.v1"),
            ("costs.json", "ub-review.ci_costs.v1"),
            ("correlation.json", "ub-review.ci_correlation.v1"),
        ] {
            fs::write(
                dir.join(name),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "schema": schema,
                    "repo": "acme/widgets",
                    "window_days": 90,
                    "jobs": [],
                    "evidence_gaps": [],
                }))?,
            )?;
        }
        Ok(())
    }

    fn setup_ci_args(out: &Path, accept: Vec<String>) -> crate::SetupCiArgs {
        crate::SetupCiArgs {
            out: out.to_path_buf(),
            print_pr: true,
            accept,
            config: out.join("no-such-config.toml"),
            open_pr: false,
            repo: None,
            github_token: None,
            github_api_url: "https://api.github.com".to_owned(),
            action_sha: None,
            branch: "ub-review/setup-ci-migration".to_owned(),
        }
    }

    #[test]
    fn setup_ci_print_pr_renders_sections_policy_and_round_trips() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("run");
        write_setup_ci_fixture(&out.join("ci-audit"))?;
        crate::cmd_setup_ci(setup_ci_args(
            &out,
            vec!["integration=cargo test --workspace --locked".to_owned()],
        ))?;
        let plan = fs::read_to_string(out.join("ci-audit/migration-plan.md"))?;
        // The eight PR-body sections in spec 0008 order.
        let headings = [
            "## Decision",
            "## Keep required",
            "## Move into ub-review/gate",
            "## Right-size to adaptive",
            "## Label-gated / nightly / release",
            "## Human review required",
            "## Proposed branch protection change",
            "## Rollback",
        ];
        let mut last = 0;
        for heading in headings {
            let position = plan
                .find(heading)
                .with_context(|| format!("missing {heading}"))?;
            assert!(position > last, "{heading} out of spec order");
            last = position;
        }
        // Receipts on rendered bullets; refusal instead of an invented
        // branch-protection remove list; accepted command rendered.
        assert!(plan.contains("ci-audit/correlation.json#integration"));
        assert!(plan.contains("old required checks unknown"));
        assert!(plan.contains("refuses to invent it"));
        assert!(plan.contains("accepted; command `cargo test --workspace --locked`"));
        assert!(
            plan.contains(
                "judgment: deterministic; receipts: ci-audit/correlation.json#integration"
            )
        );
        assert!(plan.contains("judgment: deterministic; receipts: ci-audit/correlation.json#fmt"));
        assert!(plan.contains("- none recommended by this audit"));
        // Generated config block: round-trips with zero policy receipts and
        // never carries reserved or inert keys.
        let toml_block = plan
            .split("```toml\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .context("generated toml block")?;
        assert!(toml_block.contains("[[proof.required]]"));
        assert!(toml_block.contains("required = false"));
        assert!(!toml_block.contains("[providers]"));
        assert!(!toml_block.contains("synchronize_mode"));
        assert!(!toml_block.contains("[tools."));
        let reloaded = Config::from_toml_with_policy_receipts(toml_block)?;
        assert!(
            reloaded.policy_errors.is_empty(),
            "generated config must reload clean: {:?}",
            reloaded.policy_errors
        );
        assert_eq!(reloaded.gate.required_check, "ub-review/gate");
        assert_eq!(reloaded.proof.required.len(), 1);
        assert_eq!(reloaded.proof.required[0].id, "integration");
        assert_eq!(
            reloaded.proof.required[0].command,
            "cargo test --workspace --locked"
        );
        assert_eq!(reloaded.proof.required[0].timeout_sec, 30 * 60);
        assert!(!reloaded.proof.required[0].required);
        assert!(!out.join("ci-audit/preview/.ub-review.toml").exists());
        // Determinism: same receipts, byte-identical plan; no timestamps.
        crate::cmd_setup_ci(setup_ci_args(
            &out,
            vec!["integration=cargo test --workspace --locked".to_owned()],
        ))?;
        assert_eq!(
            plan,
            fs::read_to_string(out.join("ci-audit/migration-plan.md"))?
        );
        Ok(())
    }

    #[test]
    fn setup_ci_print_pr_writes_open_pr_preview_files_with_action_sha() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("run");
        write_setup_ci_fixture(&out.join("ci-audit"))?;
        let mut args = setup_ci_args(
            &out,
            vec!["integration=cargo test --workspace --locked".to_owned()],
        );
        args.action_sha = Some("c".repeat(40));
        crate::cmd_setup_ci(args)?;

        let preview = out.join("ci-audit/preview");
        let generated_config = fs::read_to_string(preview.join(".ub-review.toml"))?;
        let workflow = fs::read_to_string(preview.join(".github/workflows/ub-review-gate.yml"))?;
        let migration = fs::read_to_string(preview.join("docs/ci/ub-review-migration.md"))?;
        let branch_doc = fs::read_to_string(preview.join("docs/ci/branch-protection-change.md"))?;

        assert_eq!(
            migration,
            fs::read_to_string(out.join("ci-audit/migration-plan.md"))?
        );
        assert!(generated_config.contains("[[proof.required]]"));
        assert!(generated_config.contains("command = \"cargo test --workspace --locked\""));
        assert!(workflow.contains(&format!("EffortlessMetrics/ub-review@{}", "c".repeat(40))));
        assert!(workflow.contains("model-mode: 'off'"));
        assert!(branch_doc.contains("Branch protection remains manual"));
        assert!(branch_doc.contains("old required checks unknown"));
        assert!(branch_doc.contains("one observed red proof"));
        assert!(branch_doc.contains("does not prove an old-check remove list"));

        crate::cmd_setup_ci(setup_ci_args(
            &out,
            vec!["integration=cargo test --workspace --locked".to_owned()],
        ))?;
        assert!(
            !preview.exists(),
            "plan-only rerun must not leave stale preview files"
        );
        Ok(())
    }

    #[test]
    fn setup_ci_accepts_move_required_jobs_as_required_proof() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("run");
        write_setup_ci_fixture(&out.join("ci-audit"))?;
        crate::cmd_setup_ci(setup_ci_args(
            &out,
            vec![
                "integration=cargo test --workspace --locked".to_owned(),
                "unit=cargo test --lib --locked".to_owned(),
            ],
        ))?;
        let plan = fs::read_to_string(out.join("ci-audit/migration-plan.md"))?;
        assert!(plan.contains("accepted; command `cargo test --workspace --locked`"));
        assert!(plan.contains("accepted; command `cargo test --lib --locked`"));
        assert!(plan.contains("judgment: deterministic; receipts: ci-audit/correlation.json#unit"));
        assert!(plan.contains("moved to required proof from audited job `unit`"));
        assert!(plan.contains("right-sized to adaptive proof from audited job `integration`"));

        let toml_block = plan
            .split("```toml\n")
            .nth(1)
            .and_then(|rest| rest.split("```").next())
            .context("generated toml block")?;
        let reloaded = Config::from_toml_with_policy_receipts(toml_block)?;
        assert!(
            reloaded.policy_errors.is_empty(),
            "generated config must reload clean: {:?}",
            reloaded.policy_errors
        );
        let proof = |id: &str| {
            reloaded
                .proof
                .required
                .iter()
                .find(|entry| entry.id == id)
                .with_context(|| format!("{id} proof entry"))
        };
        let integration = proof("integration")?;
        assert_eq!(integration.command, "cargo test --workspace --locked");
        assert!(!integration.required);
        let unit = proof("unit")?;
        assert_eq!(unit.command, "cargo test --lib --locked");
        assert!(unit.required);
        assert_eq!(unit.timeout_sec, 30 * 60);
        Ok(())
    }

    fn setup_ci_err(args: crate::SetupCiArgs) -> Result<anyhow::Error> {
        match crate::cmd_setup_ci(args) {
            Err(err) => Ok(err),
            Ok(()) => bail!("expected setup-ci to fail"),
        }
    }

    fn spawn_fake_setup_ci_api(
        expected_requests: usize,
        config_exists: bool,
    ) -> Result<(String, thread::JoinHandle<Result<Vec<String>>>)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || -> Result<Vec<String>> {
            let deadline = Instant::now() + Duration::from_secs(20);
            let mut requests = Vec::new();
            while requests.len() < expected_requests {
                match listener.accept() {
                    Ok((stream, _addr)) => {
                        requests.push(handle_fake_setup_ci_request(stream, config_exists)?);
                    }
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        if Instant::now() >= deadline {
                            bail!(
                                "fake setup-ci API received {} of {} requests",
                                requests.len(),
                                expected_requests
                            );
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(err) => return Err(err.into()),
                }
            }
            Ok(requests)
        });
        Ok((url, handle))
    }

    fn handle_fake_setup_ci_request(mut stream: TcpStream, config_exists: bool) -> Result<String> {
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;
        stream.set_write_timeout(Some(Duration::from_secs(5)))?;
        let mut reader = BufReader::new(stream.try_clone()?);
        let mut headers = String::new();
        loop {
            let mut line = String::new();
            let bytes = reader.read_line(&mut line)?;
            if bytes == 0 {
                bail!("fake setup-ci request ended before headers finished");
            }
            headers.push_str(&line);
            if line == "\r\n" || line == "\n" {
                break;
            }
        }
        let content_length = headers
            .lines()
            .find_map(|line| {
                line.to_ascii_lowercase()
                    .strip_prefix("content-length:")
                    .map(|value| value.trim().parse::<usize>().unwrap_or(0))
            })
            .unwrap_or(0);
        let mut body = vec![0u8; content_length];
        if content_length > 0 {
            use std::io::Read as _;
            reader.read_exact(&mut body)?;
        }
        let request_line = headers.lines().next().unwrap_or_default().to_owned();
        let (status_line, response_body) = if request_line.starts_with("GET /repos/")
            && request_line.contains("/git/ref/heads/")
        {
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({
                    "object": {"sha": "basesha0000000000000000000000000000000000"}
                }))?,
            )
        } else if request_line.starts_with("GET /repos/") && request_line.contains("/git/trees/") {
            let mut entries = vec![serde_json::json!({"path": "README.md"})];
            if config_exists {
                entries.push(serde_json::json!({"path": ".ub-review.toml"}));
            }
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({"tree": entries}))?,
            )
        } else if request_line.starts_with("GET /repos/") {
            (
                "HTTP/1.1 200 OK",
                serde_json::to_vec(&serde_json::json!({"default_branch": "main"}))?,
            )
        } else if request_line.starts_with("POST ") && request_line.contains("/pulls") {
            (
                "HTTP/1.1 201 Created",
                serde_json::to_vec(&serde_json::json!({
                    "html_url": "https://github.com/acme/widgets/pull/77"
                }))?,
            )
        } else {
            (
                "HTTP/1.1 201 Created",
                serde_json::to_vec(&serde_json::json!({}))?,
            )
        };
        write!(
            stream,
            "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response_body.len()
        )?;
        stream.write_all(&response_body)?;
        Ok(format!(
            "{request_line}\n{}",
            String::from_utf8_lossy(&body)
        ))
    }

    #[test]
    fn setup_ci_open_pr_creates_branch_files_and_pr_with_receipts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("run");
        write_setup_ci_fixture(&out.join("ci-audit"))?;
        fs::write(out.join("ci-audit/setup-pr-error.json"), "{}")?;
        let mut preview_args =
            setup_ci_args(&out, vec!["integration=cargo test --locked".to_owned()]);
        preview_args.action_sha = Some("a".repeat(40));
        crate::cmd_setup_ci(preview_args)?;
        let preview = out.join("ci-audit/preview");
        // Sequence: repo meta, base ref, base tree, create ref, 4 file PUTs,
        // open PR = 9 requests.
        let (api_url, handle) = spawn_fake_setup_ci_api(9, false)?;
        let mut args = setup_ci_args(&out, vec!["integration=cargo test --locked".to_owned()]);
        args.print_pr = false;
        args.open_pr = true;
        args.repo = Some("acme/widgets".to_owned());
        args.github_token = Some("test-token".to_owned());
        args.github_api_url = api_url;
        args.action_sha = Some("a".repeat(40));
        crate::cmd_setup_ci(args)?;
        let requests = match handle.join() {
            Ok(result) => result?,
            Err(_) => bail!("fake setup-ci API thread panicked"),
        };
        assert!(requests[0].starts_with("GET /repos/acme/widgets"));
        assert!(requests[1].contains("/git/ref/heads/main"));
        assert!(requests[2].contains("/git/trees/basesha"));
        assert!(requests[3].starts_with("POST /repos/acme/widgets/git/refs"));
        assert!(requests[3].contains("ub-review/setup-ci-migration"));
        assert!(requests[4].contains("PUT /repos/acme/widgets/contents/.ub-review.toml"));
        assert!(
            requests[5]
                .contains("PUT /repos/acme/widgets/contents/.github/workflows/ub-review-gate.yml")
        );
        assert!(
            requests[6].contains("PUT /repos/acme/widgets/contents/docs/ci/ub-review-migration.md")
        );
        assert!(
            requests[7]
                .contains("PUT /repos/acme/widgets/contents/docs/ci/branch-protection-change.md")
        );
        assert!(requests[8].starts_with("POST /repos/acme/widgets/pulls"));
        assert!(requests[8].contains("Adopt ub-review/gate"));
        let result: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("ci-audit/setup-pr-result.json"))?)?;
        assert!(
            !out.join("ci-audit/setup-pr-error.json").exists(),
            "successful setup-ci --open-pr must remove stale error receipts"
        );
        assert_eq!(result["schema"], "ub-review.setup_pr_result.v1");
        assert_eq!(result["pr_url"], "https://github.com/acme/widgets/pull/77");
        assert_eq!(result["base"], "main");
        assert_eq!(
            result["files"],
            serde_json::json!([
                ".ub-review.toml",
                ".github/workflows/ub-review-gate.yml",
                "docs/ci/ub-review-migration.md",
                "docs/ci/branch-protection-change.md",
            ])
        );
        let inventory: crate::CiInventoryArtifact =
            serde_json::from_slice(&fs::read(out.join("ci-audit/inventory.json"))?)?;
        let recommendations: crate::CiRecommendationsArtifact =
            serde_json::from_slice(&fs::read(out.join("ci-audit/recommendations.json"))?)?;
        let accepts =
            crate::parse_setup_ci_accepts(&["integration=cargo test --locked".to_owned()])?;
        let plan = fs::read_to_string(out.join("ci-audit/migration-plan.md"))?;
        let generated_config = crate::render_setup_ci_gate_config(
            &accepts,
            &recommendations.jobs,
            &inventory,
            "ub-review/gate",
        );
        let branch_doc = crate::render_setup_ci_branch_protection_doc(
            &inventory,
            &recommendations,
            "ub-review/gate",
        );
        let expected_files = crate::setup_ci_generated_files(
            &plan,
            &generated_config,
            &branch_doc,
            &"a".repeat(40),
            "ub-review/gate",
        );
        for (index, file) in expected_files.iter().enumerate() {
            let payload: serde_json::Value = serde_json::from_slice(&fs::read(
                out.join(format!("ci-audit/setup-pr-file-payload-{index}.json")),
            )?)?;
            assert_eq!(
                payload["message"]
                    .as_str()
                    .with_context(|| format!("{} payload message", file.path))?,
                file.message
            );
            assert_eq!(
                payload["branch"]
                    .as_str()
                    .with_context(|| format!("{} payload branch", file.path))?,
                "ub-review/setup-ci-migration"
            );
            let preview_bytes = fs::read(preview.join(file.path))
                .with_context(|| format!("read preview {}", file.path))?;
            assert_eq!(
                payload["content"]
                    .as_str()
                    .with_context(|| format!("{} payload content", file.path))?,
                crate::base64_standard(&preview_bytes),
                "{} payload must match the print-pr preview bytes",
                file.path
            );
        }
        // The generated workflow carries the pin and the zero-key posture.
        let payload: serde_json::Value = serde_json::from_slice(&fs::read(
            out.join("ci-audit/setup-pr-file-payload-1.json"),
        )?)?;
        let content = payload["content"].as_str().context("workflow content")?;
        let workflow = crate::render_setup_ci_gate_workflow(&"a".repeat(40), "ub-review/gate");
        assert_eq!(crate::base64_standard(workflow.as_bytes()), content);
        assert!(workflow.contains(&format!("EffortlessMetrics/ub-review@{}", "a".repeat(40))));
        assert!(workflow.contains("model-mode: 'off'"));
        assert!(workflow.contains("name: ub-review/gate"));
        let payload: serde_json::Value = serde_json::from_slice(&fs::read(
            out.join("ci-audit/setup-pr-file-payload-3.json"),
        )?)?;
        let branch_doc = crate::render_setup_ci_branch_protection_doc(
            &serde_json::from_slice(&fs::read(out.join("ci-audit/inventory.json"))?)?,
            &serde_json::from_slice(&fs::read(out.join("ci-audit/recommendations.json"))?)?,
            "ub-review/gate",
        );
        assert_eq!(
            payload["content"]
                .as_str()
                .context("branch-protection content")?,
            crate::base64_standard(branch_doc.as_bytes())
        );
        assert!(branch_doc.contains("Branch protection remains manual"));
        assert!(branch_doc.contains("old required checks unknown"));
        assert!(branch_doc.contains("one observed red proof"));
        Ok(())
    }

    #[test]
    fn setup_ci_open_pr_fails_closed_on_existing_config_and_missing_pin() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("run");
        write_setup_ci_fixture(&out.join("ci-audit"))?;
        let accept = vec!["integration=cargo test --locked".to_owned()];

        // No accepted jobs: nothing to open.
        let mut args = setup_ci_args(&out, Vec::new());
        args.open_pr = true;
        let err = setup_ci_err(args)?;
        assert!(
            format!("{err:#}").contains("no migration PR to open"),
            "{err:#}"
        );

        // Missing --action-sha: the generator refuses to invent the pin.
        let mut args = setup_ci_args(&out, accept.clone());
        args.open_pr = true;
        args.repo = Some("acme/widgets".to_owned());
        args.github_token = Some("test-token".to_owned());
        let err = setup_ci_err(args)?;
        assert!(
            format!("{err:#}").contains("refuses to invent a pin"),
            "{err:#}"
        );

        // Existing .ub-review.toml in the target repo: refuse to edit.
        let (api_url, handle) = spawn_fake_setup_ci_api(3, true)?;
        let mut args = setup_ci_args(&out, accept);
        args.print_pr = false;
        args.open_pr = true;
        args.repo = Some("acme/widgets".to_owned());
        args.github_token = Some("test-token".to_owned());
        args.github_api_url = api_url;
        args.action_sha = Some("b".repeat(40));
        fs::write(out.join("ci-audit/setup-pr-result.json"), "{}")?;
        let err = setup_ci_err(args)?;
        assert!(
            format!("{err:#}").contains("already has a .ub-review.toml"),
            "{err:#}"
        );
        assert!(
            !out.join("ci-audit/setup-pr-result.json").exists(),
            "failed setup-ci --open-pr must remove stale success receipts"
        );
        let error_receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("ci-audit/setup-pr-error.json"))?)?;
        assert_eq!(error_receipt["schema"], "ub-review.setup_pr_error.v1");
        match handle.join() {
            Ok(result) => {
                result?;
            }
            Err(_) => bail!("fake setup-ci API thread panicked"),
        }
        Ok(())
    }

    #[test]
    fn setup_ci_fails_closed_on_missing_receipts_and_bad_accepts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("run");

        // Bare setup-ci is inert: operators must choose local render or PR open.
        let mut bare = setup_ci_args(&out, Vec::new());
        bare.print_pr = false;
        let err = setup_ci_err(bare)?;
        assert!(format!("{err:#}").contains("--print-pr"), "{err:#}");

        // Missing receipts name the artifact and the prerequisite command.
        let err = setup_ci_err(setup_ci_args(&out, Vec::new()))?;
        let text = format!("{err:#}");
        assert!(
            text.contains("inventory.json") && text.contains("audit-ci"),
            "{text}"
        );

        write_setup_ci_fixture(&out.join("ci-audit"))?;

        // Schema mismatch is named, not tolerated.
        let inventory_path = out.join("ci-audit/inventory.json");
        let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&inventory_path)?)?;
        value["schema"] = serde_json::Value::String("ub-review.ci_inventory.v2".to_owned());
        fs::write(&inventory_path, serde_json::to_vec_pretty(&value)?)?;
        let err = setup_ci_err(setup_ci_args(&out, Vec::new()))?;
        assert!(
            format!("{err:#}").contains("expected ub-review.ci_inventory.v1"),
            "{err:#}"
        );
        write_setup_ci_fixture(&out.join("ci-audit"))?;

        // Accept validation: malformed pair, unknown job, refused tiers.
        for (accept, expected) in [
            ("integration", "--accept needs"),
            ("ghost=cargo test", "does not match any job"),
            ("deploy=./deploy.sh", "flag-for-human"),
            ("fmt=cargo fmt", "tier `keep-required`"),
        ] {
            let err = setup_ci_err(setup_ci_args(&out, vec![accept.to_owned()]))?;
            assert!(
                format!("{err:#}").contains(expected),
                "accept `{accept}`: {err:#}"
            );
        }

        // Empty accept list renders the no-PR explanation and exits ok.
        crate::cmd_setup_ci(setup_ci_args(&out, Vec::new()))?;
        let plan = fs::read_to_string(out.join("ci-audit/migration-plan.md"))?;
        assert!(plan.contains("no migration PR"));
        assert!(!plan.contains("```toml"));
        Ok(())
    }

    #[test]
    fn ci_audit_artifacts_carry_schema_fields_and_receipts() -> Result<()> {
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/ci.yml",
            CI_FIXTURE_WORKFLOW,
        )];
        let fetch = crate::CiAuditFetch {
            workflows: ci_fixture_workflows(),
            runs: ci_fixture_correlation_runs()?,
            pages_fetched: 1,
            truncated: false,
            required_checks: ci_no_required_checks(),
            evidence_gaps: Vec::new(),
        };
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            Some(&fetch),
            None,
            crate::Utc::now(),
        );
        assert_eq!(artifacts.inventory.schema, "ub-review.ci_inventory.v1");
        assert_eq!(artifacts.history.schema, "ub-review.ci_history.v1");
        assert_eq!(artifacts.costs.schema, "ub-review.ci_costs.v1");
        assert_eq!(artifacts.correlation.schema, "ub-review.ci_correlation.v1");
        assert_eq!(
            artifacts.recommendations.schema,
            "ub-review.ci_recommendations.v1"
        );
        assert_eq!(
            artifacts.runner_cancellations.schema,
            "ub-review.ci_runner_cancellations.v1"
        );
        let temp = tempfile::tempdir()?;
        crate::write_ci_audit_artifacts(temp.path(), &artifacts)?;
        for name in [
            "inventory.json",
            "history.json",
            "costs.json",
            "correlation.json",
            "recommendations.json",
        ] {
            let value: serde_json::Value =
                serde_json::from_slice(&fs::read(temp.path().join(name))?)?;
            assert!(value.get("schema").is_some(), "{name} missing schema");
            assert!(
                value
                    .get("jobs")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|jobs| !jobs.is_empty()),
                "{name} missing jobs"
            );
            assert!(
                value.get("evidence_gaps").is_some(),
                "{name} missing evidence_gaps"
            );
        }
        let runner_cancellations: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("runner-cancellations.json"))?)?;
        assert_eq!(
            runner_cancellations
                .get("schema")
                .and_then(serde_json::Value::as_str),
            Some("ub-review.ci_runner_cancellations.v1")
        );
        assert!(
            runner_cancellations
                .get("classifications")
                .and_then(serde_json::Value::as_array)
                .is_some(),
            "runner-cancellations missing classifications"
        );
        assert!(
            runner_cancellations.get("evidence_gaps").is_some(),
            "runner-cancellations missing evidence_gaps"
        );
        let inventory: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("inventory.json"))?)?;
        let first_job = inventory
            .get("jobs")
            .and_then(serde_json::Value::as_array)
            .and_then(|jobs| jobs.first())
            .context("inventory first job")?;
        for field in [
            "workflow",
            "job",
            "name",
            "triggers",
            "path_filters",
            "matrix_size",
            "timeout_minutes",
            "permissions",
            "uses_secrets",
            "required_check",
            "required_check_source",
            "required_check_context",
        ] {
            assert!(
                first_job.get(field).is_some(),
                "inventory job missing {field}"
            );
        }
        let history: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("history.json"))?)?;
        assert_eq!(
            history.get("page_cap").and_then(serde_json::Value::as_u64),
            Some(10)
        );
        assert_eq!(
            history.get("run_cap").and_then(serde_json::Value::as_u64),
            Some(1000)
        );
        let history_job = history
            .get("jobs")
            .and_then(serde_json::Value::as_array)
            .and_then(|jobs| jobs.first())
            .context("history first job")?;
        for field in [
            "job",
            "window_days",
            "runs",
            "failure_rate",
            "cancellation_rate",
            "flake_rate",
            "rerun_then_pass",
            "evidence_gaps",
        ] {
            assert!(
                history_job.get(field).is_some(),
                "history job missing {field}"
            );
        }
        let costs: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("costs.json"))?)?;
        let costs_job = costs
            .get("jobs")
            .and_then(serde_json::Value::as_array)
            .and_then(|jobs| jobs.first())
            .context("costs first job")?;
        for field in [
            "job",
            "duration_p50_sec",
            "duration_p90_sec",
            "duration_p99_sec",
            "runner_minutes_per_month",
            "matrix_expansion",
        ] {
            assert!(costs_job.get(field).is_some(), "costs job missing {field}");
        }
        let correlation: serde_json::Value =
            serde_json::from_slice(&fs::read(temp.path().join("correlation.json"))?)?;
        assert!(correlation.get("independent_failure_rule").is_some());
        let correlation_job = correlation
            .get("jobs")
            .and_then(serde_json::Value::as_array)
            .and_then(|jobs| jobs.first())
            .context("correlation first job")?;
        for field in [
            "job",
            "independent_failures",
            "co_failing_jobs",
            "cheaper_jobs_compared",
            "window_days",
        ] {
            assert!(
                correlation_job.get(field).is_some(),
                "correlation job missing {field}"
            );
        }
        for recommendation in &artifacts.recommendations.jobs {
            assert_eq!(recommendation.judgment, "deterministic");
            assert!(
                !recommendation.receipts.is_empty(),
                "recommendation without receipt"
            );
            assert!(!recommendation.positioned_to_catch.is_empty());
            assert!(!recommendation.has_caught.is_empty());
        }
        let report = fs::read_to_string(temp.path().join("audit-report.md"))?;
        assert!(report.contains("# CI audit: acme/widgets"));
        Ok(())
    }

    #[test]
    fn ci_audit_artifacts_use_literal_matrix_size_for_inventory_and_costs() -> Result<()> {
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/matrix.yml",
            CI_MATRIX_WORKFLOW,
        )];
        let workflows_value = serde_json::json!({
            "workflows": [
                {"id": 1, "name": "Matrix CI", "path": ".github/workflows/matrix.yml", "state": "active"}
            ]
        });
        let fetch = crate::CiAuditFetch {
            workflows: crate::parse_ci_api_array(&workflows_value, "workflows").0,
            runs: ci_fixture_runs(&[(
                201,
                vec![("Unit tests (ubuntu-latest, stable)", "success", 120)],
            )])?,
            pages_fetched: 1,
            truncated: false,
            required_checks: ci_no_required_checks(),
            evidence_gaps: Vec::new(),
        };
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            Some(&fetch),
            None,
            crate::Utc::now(),
        );
        let inventory_job = artifacts
            .inventory
            .jobs
            .iter()
            .find(|job| job.job == "Unit tests (ubuntu-latest, stable)")
            .context("observed matrix inventory job")?;
        assert_eq!(inventory_job.matrix_size, 4);
        let cost_job = artifacts
            .costs
            .jobs
            .iter()
            .find(|job| job.job == "Unit tests (ubuntu-latest, stable)")
            .context("observed matrix cost job")?;
        assert_eq!(cost_job.matrix_expansion, 4);
        let matrix_line = artifacts
            .report
            .lines()
            .find(|line| line.starts_with("- `Unit tests (ubuntu-latest, stable)`"))
            .context("matrix report line present")?;
        assert!(
            matrix_line.contains("~1 runner-min/mo, matrix fan-out x4"),
            "matrix fan-out should sit beside the cost receipt: {matrix_line}"
        );
        assert!(
            artifacts
                .inventory
                .evidence_gaps
                .iter()
                .any(|gap| gap.contains("generated") && gap.contains("dynamic expression")),
            "dynamic matrix gap missing: {:?}",
            artifacts.inventory.evidence_gaps
        );
        assert!(
            artifacts
                .inventory
                .evidence_gaps
                .iter()
                .any(|gap| gap.contains("adjusted") && gap.contains("include")),
            "include matrix gap missing: {:?}",
            artifacts.inventory.evidence_gaps
        );
        assert!(
            artifacts
                .inventory
                .evidence_gaps
                .iter()
                .all(|gap| !gap.contains("matrix structure not extracted")),
            "stale blanket matrix gap remained: {:?}",
            artifacts.inventory.evidence_gaps
        );
        Ok(())
    }

    #[test]
    fn ci_audit_artifacts_record_required_check_receipts_for_setup_ci() -> Result<()> {
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/ci.yml",
            CI_FIXTURE_WORKFLOW,
        )];
        let mut specs: Vec<CiRunSpec<'_>> = Vec::new();
        for index in 0..25u64 {
            specs.push((
                900 + index,
                vec![
                    ("fmt", "success", 60),
                    ("test", "success", 120),
                    ("e2e", "success", 600),
                ],
            ));
        }
        let fetch = crate::CiAuditFetch {
            workflows: ci_fixture_workflows(),
            runs: ci_fixture_runs(&specs)?,
            pages_fetched: 1,
            truncated: false,
            required_checks: ci_required_checks(&[
                ("fmt", "branch-protection"),
                ("End to end", "ruleset"),
            ]),
            evidence_gaps: Vec::new(),
        };
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            Some(&fetch),
            None,
            crate::Utc::now(),
        );

        let inventory_job = |name: &str| -> Result<&crate::CiInventoryJob> {
            artifacts
                .inventory
                .jobs
                .iter()
                .find(|job| job.job == name)
                .with_context(|| format!("{name} inventory job"))
        };
        let fmt = inventory_job("fmt")?;
        assert_eq!(fmt.required_check, Some(true));
        assert_eq!(fmt.required_check_source, "branch-protection");
        assert_eq!(fmt.required_check_context.as_deref(), Some("fmt"));
        let e2e = inventory_job("e2e")?;
        assert_eq!(e2e.required_check, Some(true));
        assert_eq!(e2e.required_check_source, "ruleset");
        assert_eq!(e2e.required_check_context.as_deref(), Some("End to end"));
        let test = inventory_job("test")?;
        assert_eq!(test.required_check, Some(false));
        assert_eq!(test.required_check_source, "branch-protection");
        assert_eq!(test.required_check_context, None);
        let recommendation = |name: &str| -> Result<&crate::CiRecommendation> {
            artifacts
                .recommendations
                .jobs
                .iter()
                .find(|job| job.job == name)
                .with_context(|| format!("{name} recommendation"))
        };
        let fmt_recommendation = recommendation("fmt")?;
        assert_eq!(fmt_recommendation.tier, "move-to-ub-review-required");
        assert!(
            fmt_recommendation
                .receipts
                .iter()
                .any(|receipt| receipt == "ci-audit/inventory.json#fmt"),
            "{:?}",
            fmt_recommendation.receipts
        );
        let e2e_recommendation = recommendation("e2e")?;
        assert_eq!(e2e_recommendation.tier, "adaptive");
        let test_recommendation = recommendation("test")?;
        assert_eq!(test_recommendation.tier, "keep-required");
        assert!(
            !artifacts
                .inventory
                .evidence_gaps
                .iter()
                .any(|gap| gap.contains("required-check status unknown")),
            "{:?}",
            artifacts.inventory.evidence_gaps
        );

        let plan = crate::render_setup_ci_migration_plan(
            &artifacts.inventory,
            &artifacts.recommendations,
            &[],
            "ub-review/gate",
        );
        assert!(plan.contains("remove required checks reported by audit-ci receipts"));
        assert!(plan.contains("`fmt` from `.github/workflows/ci.yml`"));
        assert!(plan.contains("`End to end` from `.github/workflows/ci.yml`"));
        assert!(!plan.contains("`e2e` from `.github/workflows/ci.yml`"));
        assert!(!plan.contains("old required checks unknown"));
        assert!(plan.contains("setup-ci did not mutate branch protection"));
        let branch_doc = crate::render_setup_ci_branch_protection_doc(
            &artifacts.inventory,
            &artifacts.recommendations,
            "ub-review/gate",
        );
        assert!(branch_doc.contains("# Branch protection change"));
        assert!(branch_doc.contains("remove required checks reported by audit-ci receipts"));
        assert!(branch_doc.contains("`fmt` from `.github/workflows/ci.yml`"));
        assert!(branch_doc.contains("`End to end` from `.github/workflows/ci.yml`"));
        assert!(!branch_doc.contains("old required checks unknown"));
        assert!(branch_doc.contains("Branch protection remains manual"));
        assert!(branch_doc.contains("restore the exact checks listed above"));
        Ok(())
    }

    #[test]
    fn setup_ci_refuses_exact_branch_protection_plan_with_unmatched_required_context() -> Result<()>
    {
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/ci.yml",
            CI_FIXTURE_WORKFLOW,
        )];
        let mut specs: Vec<CiRunSpec<'_>> = Vec::new();
        for index in 0..25u64 {
            specs.push((
                950 + index,
                vec![
                    ("fmt", "success", 60),
                    ("test", "success", 120),
                    ("e2e", "success", 600),
                ],
            ));
        }
        let fetch = crate::CiAuditFetch {
            workflows: ci_fixture_workflows(),
            runs: ci_fixture_runs(&specs)?,
            pages_fetched: 1,
            truncated: false,
            required_checks: ci_required_checks(&[
                ("fmt", "branch-protection"),
                ("external/codecov", "branch-protection"),
            ]),
            evidence_gaps: Vec::new(),
        };
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            Some(&fetch),
            None,
            crate::Utc::now(),
        );
        assert!(
            artifacts
                .inventory
                .evidence_gaps
                .iter()
                .any(|gap| gap.contains("required-check context not matched")),
            "{:?}",
            artifacts.inventory.evidence_gaps
        );
        let plan = crate::render_setup_ci_migration_plan(
            &artifacts.inventory,
            &artifacts.recommendations,
            &[],
            "ub-review/gate",
        );
        assert!(plan.contains("old required checks unknown"));
        assert!(plan.contains("refuses to invent"));
        assert!(!plan.contains("remove required checks reported by audit-ci receipts"));
        let branch_doc = crate::render_setup_ci_branch_protection_doc(
            &artifacts.inventory,
            &artifacts.recommendations,
            "ub-review/gate",
        );
        assert!(branch_doc.contains("old required checks unknown"));
        assert!(branch_doc.contains("refuses to invent"));
        assert!(!branch_doc.contains("remove required checks reported by audit-ci receipts"));
        assert!(branch_doc.contains("does not prove an old-check remove list"));
        Ok(())
    }

    #[test]
    fn ci_audit_artifacts_record_permissions_secrets_and_flag_sensitive_jobs() -> Result<()> {
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/ci.yml",
            CI_SENSITIVE_WORKFLOW,
        )];
        let mut specs: Vec<CiRunSpec<'_>> = Vec::new();
        for index in 0..25u64 {
            specs.push((
                700 + index,
                vec![
                    ("wide", "success", 60),
                    ("notify", "success", 60),
                    ("docs", "success", 60),
                    ("nullperms", "success", 60),
                ],
            ));
        }
        let fetch = crate::CiAuditFetch {
            workflows: ci_fixture_workflows(),
            runs: ci_fixture_runs(&specs)?,
            pages_fetched: 1,
            truncated: false,
            required_checks: ci_no_required_checks(),
            evidence_gaps: Vec::new(),
        };
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            Some(&fetch),
            None,
            crate::Utc::now(),
        );

        let wide = artifacts
            .inventory
            .jobs
            .iter()
            .find(|job| job.job == "wide")
            .context("wide inventory job")?;
        assert_eq!(
            wide.permissions.as_ref(),
            Some(&serde_json::json!({"contents": "write", "id-token": "write"}))
        );
        assert_eq!(
            wide.uses_secrets,
            vec!["DEPLOY_TOKEN".to_owned(), "GLOBAL_TOKEN".to_owned()]
        );

        let notify = artifacts
            .inventory
            .jobs
            .iter()
            .find(|job| job.job == "notify")
            .context("notify inventory job")?;
        assert_eq!(
            notify.permissions.as_ref(),
            Some(&serde_json::json!({"contents": "read"}))
        );
        assert_eq!(
            notify.uses_secrets,
            vec![
                "GLOBAL_TOKEN".to_owned(),
                crate::CI_AUDIT_DYNAMIC_SECRET_REF.to_owned()
            ]
        );

        let docs = artifacts
            .inventory
            .jobs
            .iter()
            .find(|job| job.job == "docs")
            .context("docs inventory job")?;
        assert_eq!(
            docs.permissions.as_ref(),
            Some(&serde_json::json!({"contents": "read"}))
        );
        assert_eq!(docs.uses_secrets, vec!["GLOBAL_TOKEN".to_owned()]);

        let nullperms = artifacts
            .inventory
            .jobs
            .iter()
            .find(|job| job.job == "nullperms")
            .context("nullperms inventory job")?;
        assert_eq!(
            nullperms.permissions.as_ref(),
            Some(&serde_json::Value::Null)
        );

        let recommendation = |job: &str| -> Result<&crate::CiRecommendation> {
            artifacts
                .recommendations
                .jobs
                .iter()
                .find(|recommendation| recommendation.job == job)
                .with_context(|| format!("{job} recommendation"))
        };
        let wide = recommendation("wide")?;
        assert_eq!(wide.tier, "flag-for-human");
        assert!(wide.reason.contains("write-scoped permissions"));
        let notify = recommendation("notify")?;
        assert_eq!(notify.tier, "flag-for-human");
        assert!(notify.reason.contains(crate::CI_AUDIT_DYNAMIC_SECRET_REF));
        let docs = recommendation("docs")?;
        assert_eq!(docs.tier, "flag-for-human");
        assert!(docs.reason.contains("GLOBAL_TOKEN"));
        let nullperms = recommendation("nullperms")?;
        assert_eq!(nullperms.tier, "flag-for-human");
        assert!(nullperms.reason.contains("ambiguous permissions"));
        Ok(())
    }

    #[test]
    fn ci_audit_artifacts_flag_missing_permissions_as_human_review() -> Result<()> {
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/ci.yml",
            CI_MISSING_PERMISSIONS_WORKFLOW,
        )];
        let mut specs: Vec<CiRunSpec<'_>> = Vec::new();
        for index in 0..25u64 {
            specs.push((900 + index, vec![("integration", "success", 60)]));
        }
        let fetch = crate::CiAuditFetch {
            workflows: ci_fixture_workflows(),
            runs: ci_fixture_runs(&specs)?,
            pages_fetched: 1,
            truncated: false,
            required_checks: ci_no_required_checks(),
            evidence_gaps: Vec::new(),
        };
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            Some(&fetch),
            None,
            crate::Utc::now(),
        );
        let inventory = artifacts
            .inventory
            .jobs
            .iter()
            .find(|job| job.job == "integration")
            .context("integration inventory job")?;
        assert_eq!(inventory.permissions, None);
        let recommendation = artifacts
            .recommendations
            .jobs
            .iter()
            .find(|recommendation| recommendation.job == "integration")
            .context("integration recommendation")?;
        assert_eq!(recommendation.tier, "flag-for-human");
        assert!(recommendation.reason.contains("missing permissions"));
        assert!(
            recommendation
                .proposed_policy
                .contains("human decision required")
        );
        Ok(())
    }

    #[test]
    fn ci_audit_runner_cancellation_receipts_separate_superseded_from_eviction() -> Result<()> {
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/ci.yml",
            CI_FIXTURE_WORKFLOW,
        )];
        let fetch = crate::CiAuditFetch {
            workflows: ci_fixture_workflows(),
            runs: vec![ci_fixture_run_with_jobs(
                501,
                vec![ci_api_job_value_without_completion("fmt", "cancelled")],
            )?],
            pages_fetched: 1,
            truncated: false,
            required_checks: ci_no_required_checks(),
            evidence_gaps: Vec::new(),
        };
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            Some(&fetch),
            Some(0),
            crate::Utc::now(),
        );
        let eviction = artifacts
            .runner_cancellations
            .classifications
            .iter()
            .find(|entry| entry.job == "fmt")
            .context("fmt cancellation receipt")?;
        assert_eq!(eviction.classification, "runner_eviction_suspected");
        assert_eq!(eviction.audit_cancel_events, Some(0));
        assert!(eviction.runner_shutdown_signal);
        assert!(eviction.github_hosted);
        assert!(
            eviction
                .suggested_action
                .contains("self-hosted or cx profile")
        );
        assert!(
            eviction
                .receipts
                .contains(&"ci-audit/history.json#fmt".to_owned())
        );

        let superseded_workflow = format!(
            "concurrency:\n  group: ci-main\n  cancel-in-progress: true\n\n{CI_FIXTURE_WORKFLOW}"
        );
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/ci.yml",
            &superseded_workflow,
        )];
        let fetch = crate::CiAuditFetch {
            workflows: ci_fixture_workflows(),
            runs: vec![ci_fixture_run_with_jobs(
                502,
                vec![ci_api_job_value_without_completion("fmt", "cancelled")],
            )?],
            pages_fetched: 1,
            truncated: false,
            required_checks: ci_no_required_checks(),
            evidence_gaps: Vec::new(),
        };
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            Some(&fetch),
            Some(0),
            crate::Utc::now(),
        );
        let superseded = artifacts
            .runner_cancellations
            .classifications
            .iter()
            .find(|entry| entry.job == "fmt")
            .context("fmt superseded cancellation receipt")?;
        assert_eq!(superseded.classification, "cancelled_superseded");
        assert_ne!(superseded.classification, "runner_eviction_suspected");
        assert!(
            superseded
                .suggested_action
                .contains("do not treat this cancellation as code evidence")
        );
        Ok(())
    }

    #[test]
    fn ci_audit_tokenless_degrades_to_inventory_only() -> Result<()> {
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/ci.yml",
            CI_FIXTURE_WORKFLOW,
        )];
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            None,
            None,
            crate::Utc::now(),
        );
        assert_eq!(artifacts.inventory.jobs.len(), 3);
        assert!(artifacts.history.jobs.is_empty());
        assert!(artifacts.costs.jobs.is_empty());
        assert!(artifacts.correlation.jobs.is_empty());
        assert!(artifacts.runner_cancellations.classifications.is_empty());
        // Every degraded artifact carries the no-token gap standalone.
        for gaps in [
            &artifacts.inventory.evidence_gaps,
            &artifacts.history.evidence_gaps,
            &artifacts.costs.evidence_gaps,
            &artifacts.correlation.evidence_gaps,
            &artifacts.runner_cancellations.evidence_gaps,
        ] {
            assert!(
                gaps.iter().any(|gap| gap.contains("no GitHub token")),
                "missing no-token gap in {gaps:?}"
            );
        }
        // YAML display name is used when the job never ran in the window.
        let e2e = artifacts
            .inventory
            .jobs
            .iter()
            .find(|job| job.job == "e2e")
            .context("e2e inventory job")?;
        assert_eq!(e2e.name, "End to end");
        for recommendation in &artifacts.recommendations.jobs {
            assert_eq!(recommendation.tier, "flag-for-human");
            assert_eq!(recommendation.judgment, "deterministic");
            assert!(!recommendation.receipts.is_empty());
            assert!(recommendation.has_caught.contains("no run history"));
        }
        assert!(artifacts.report.contains("Inventory-only"));
        assert!(artifacts.report.contains("### Human review required"));
        // The header states inventory-only mode once; per-job lines must not
        // repeat the tokenless boilerplate, and the gap appears once in the
        // Evidence gaps section.
        assert!(!artifacts.report.contains("tokenless"));
        assert_eq!(
            artifacts
                .report
                .matches("no GitHub token: run history, durations, and correlation unavailable")
                .count(),
            1
        );
        Ok(())
    }

    #[test]
    fn ci_audit_report_lines_carry_receipts_without_boilerplate() -> Result<()> {
        let scans = vec![crate::scan_workflow_text(
            ".github/workflows/ci.yml",
            CI_FIXTURE_WORKFLOW,
        )];
        let mut specs: Vec<CiRunSpec<'_>> = Vec::new();
        for index in 0..25u64 {
            let fmt_conclusion = if index < 3 { "failure" } else { "success" };
            specs.push((
                100 + index,
                vec![
                    ("fmt", fmt_conclusion, 60),
                    ("test", "success", 120),
                    ("e2e", "success", 600),
                ],
            ));
        }
        let fetch = crate::CiAuditFetch {
            workflows: ci_fixture_workflows(),
            runs: ci_fixture_runs(&specs)?,
            pages_fetched: 1,
            truncated: false,
            required_checks: ci_no_required_checks(),
            evidence_gaps: Vec::new(),
        };
        let artifacts = crate::build_ci_audit_artifacts(
            "acme/widgets",
            90,
            &scans,
            Some(&fetch),
            None,
            crate::Utc::now(),
        );
        let report = &artifacts.report;
        assert!(report.contains("Independent-failure rule:"));
        // Tier sections in decision-relevance order: adaptive (action items)
        // before keep-required; empty tiers omitted.
        let adaptive_at = report
            .find("### Right-size to adaptive")
            .context("adaptive section present")?;
        let keep_at = report
            .find("### Keep required")
            .context("keep-required section present")?;
        assert!(adaptive_at < keep_at, "adaptive section must come first");
        assert!(!report.contains("### Human review required"));
        assert!(!report.contains("### Label-gated"));
        assert!(report.contains("p50"));
        assert!(report.contains("runner-min/mo"));
        // Receipts appear once per line; the reason fragment must not repeat
        // run counts or independent-failure counts.
        assert!(report.contains("25 runs, 3 independent failures"));
        assert!(!report.contains("independent failures in"));
        assert!(report.contains("earns its required slot"));
        let fmt_line = report
            .lines()
            .find(|line| line.starts_with("- `fmt`"))
            .context("fmt report line present")?;
        assert_eq!(fmt_line.matches("25").count(), 1, "line: {fmt_line}");
        assert_eq!(fmt_line.matches('3').count(), 1, "line: {fmt_line}");
        assert!(
            fmt_line.contains("receipts: `ci-audit/correlation.json#fmt`"),
            "line: {fmt_line}"
        );
        assert!(
            fmt_line.contains("`ci-audit/costs.json#fmt`"),
            "line: {fmt_line}"
        );
        assert!(
            fmt_line.contains("`ci-audit/history.json#fmt`"),
            "line: {fmt_line}"
        );
        assert!(report.contains("## Evidence gaps"));
        let lowercase = report.to_ascii_lowercase();
        for banned in [
            "lgtm",
            "looks good",
            "human should still review",
            "generated by",
            "no issues found",
            "all checks passed",
            "tool roster",
        ] {
            assert!(
                !lowercase.contains(banned),
                "report contains banned phrase: {banned}"
            );
        }
        Ok(())
    }

    #[test]
    fn ci_audit_repo_empty_override_falls_back_to_git_origin() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let repo_dir = temp.path().join("repo");
        fs::create_dir_all(&repo_dir)?;
        run_test_command(&repo_dir, "git", &["init"])?;
        run_test_command(
            &repo_dir,
            "git",
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/acme/widgets.git",
            ],
        )?;
        let args = |repo: Option<&str>| crate::AuditCiArgs {
            root: repo_dir.clone(),
            out: temp.path().join("out"),
            repo: repo.map(str::to_owned),
            github_token: None,
            github_api_url: "https://api.github.com".to_owned(),
            window_days: 90,
            audit_cancel_events: None,
        };
        // Set-but-empty GITHUB_REPOSITORY (clap env fallback) must fall back
        // to the git origin remote, like the empty-token path.
        assert_eq!(
            crate::resolve_ci_audit_repo(&args(Some("")))?,
            "acme/widgets"
        );
        assert_eq!(
            crate::resolve_ci_audit_repo(&args(Some("   ")))?,
            "acme/widgets"
        );
        assert_eq!(crate::resolve_ci_audit_repo(&args(None))?, "acme/widgets");
        assert_eq!(
            crate::resolve_ci_audit_repo(&args(Some("acme/explicit")))?,
            "acme/explicit"
        );
        assert!(crate::resolve_ci_audit_repo(&args(Some("not a slug"))).is_err());
        Ok(())
    }
