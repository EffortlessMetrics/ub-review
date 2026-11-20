use super::*;

const SOURCE: &[u8] = b"pub fn retained_source() -> u32 { 37 }\n";
const CONFIG_CHILD: &[u8] = b"retained config-directory child\n";
const WORKFLOW: &[u8] = b"name: retained\non: pull_request\njobs: {}\n";

fn offline_source() -> ReleaseLookup {
    ReleaseLookup::Unavailable {
        reason: ReleaseFallbackReason::RequestUnavailable,
    }
}

fn offline_release() -> ReleaseLookup {
    ReleaseLookup::Installable {
        tag: "v9.9.9".to_owned(),
    }
}

fn args(root: &Path, inspect: bool, force: bool) -> EnableArgs {
    EnableArgs {
        mode: ReviewModePreset::Advisory,
        model: "minimax".to_owned(),
        action_sha: Some("a".repeat(40)),
        root: root.to_path_buf(),
        inspect,
        force,
    }
}

fn prepare_source(root: &Path) -> Result<()> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("src/lib.rs"), SOURCE)?;
    fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"retained-source\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )?;
    Ok(())
}

fn inventory(root: &Path) -> Result<Vec<PathBuf>> {
    fn visit(root: &Path, directory: &Path, paths: &mut Vec<PathBuf>) -> Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            paths.push(path.strip_prefix(root)?.to_path_buf());
            if entry.file_type()?.is_dir() {
                visit(root, &path, paths)?;
            }
        }
        Ok(())
    }
    let mut paths = Vec::new();
    visit(root, root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

#[test]
fn enable_rejects_directory_config_before_any_workflow_output() -> Result<()> {
    let mut failures = Vec::new();
    for inspect in [false, true] {
        for existing_workflow in [false, true] {
            for attempt in 0..2 {
                let temp = tempfile::tempdir()?;
                let root = temp.path();
                prepare_source(root)?;
                let config = root.join(".ub-review.toml");
                let workflow = root.join(".github/workflows/ub-review.yml");
                fs::create_dir(&config)?;
                fs::write(config.join("retained.txt"), CONFIG_CHILD)?;
                if existing_workflow {
                    fs::create_dir_all(root.join(".github/workflows"))?;
                    fs::write(&workflow, WORKFLOW)?;
                }
                let before = inventory(root)?;
                anyhow::ensure!(config.is_dir());
                anyhow::ensure!(fs::read(root.join("src/lib.rs"))? == SOURCE);
                let error =
                    match cmd_enable_with_resolver(args(root, inspect, true), offline_source) {
                        Ok(()) => None,
                        Err(error) => Some(format!("{error:#}")),
                    };
                let rejected = error.is_some();
                let diagnostic = error.as_deref().is_some_and(|message| {
                    message.contains(&config.display().to_string())
                        && message.contains("is a directory")
                        && message.contains("remove or rename")
                });
                let workflow_preserved = if existing_workflow {
                    fs::read(&workflow).ok().as_deref() == Some(WORKFLOW)
                } else {
                    !workflow.exists() && !root.join(".github").exists()
                };
                let child_preserved = fs::read(config.join("retained.txt"))? == CONFIG_CHILD;
                let source_preserved = fs::read(root.join("src/lib.rs"))? == SOURCE;
                let paths_preserved = inventory(root)? == before;
                if !(rejected
                    && diagnostic
                    && workflow_preserved
                    && child_preserved
                    && source_preserved
                    && paths_preserved)
                {
                    failures.push(format!(
                        "inspect={inspect} existing_workflow={existing_workflow} attempt={attempt}: rejected={rejected} diagnostic={diagnostic} workflow_preserved={workflow_preserved} child_preserved={child_preserved} source_preserved={source_preserved} paths_preserved={paths_preserved} error={error:?}"
                    ));
                }
            }
        }
    }
    anyhow::ensure!(
        failures.is_empty(),
        "directory config output violations:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[test]
fn enable_regular_file_outputs_preserve_rendering_and_replay() -> Result<()> {
    for resolve in [
        offline_source as fn() -> ReleaseLookup,
        offline_release as fn() -> ReleaseLookup,
    ] {
        for inspect in [false, true] {
            let temp = tempfile::tempdir()?;
            let root = temp.path();
            prepare_source(root)?;
            let config = root.join(".ub-review.toml");
            let workflow = root.join(".github/workflows/ub-review.yml");
            cmd_enable_with_resolver(args(root, inspect, false), resolve)?;
            let expected_config = fs::read(&config)?;
            let expected_workflow = fs::read(&workflow)?;
            let parsed: toml::Value = toml::from_str(std::str::from_utf8(&expected_config)?)?;
            anyhow::ensure!(
                parsed.get("profile").and_then(toml::Value::as_str) == Some("gh-runner")
            );
            let kind = parsed
                .get("repo")
                .and_then(|value| value.get("kind"))
                .and_then(toml::Value::as_str);
            anyhow::ensure!(kind == Some(if inspect { "rust" } else { "generic" }));
            let required_check = parsed
                .get("gate")
                .and_then(|value| value.get("required_check"))
                .and_then(toml::Value::as_str);
            anyhow::ensure!(required_check == Some("ub-review/gate"));
            if inspect {
                let provider_policy = parsed
                    .get("providers")
                    .and_then(|value| value.get("policy"))
                    .and_then(toml::Value::as_str);
                anyhow::ensure!(provider_policy == Some("primary-with-fallback"));
            } else {
                anyhow::ensure!(parsed.get("providers").is_none());
            }
            let yaml = std::str::from_utf8(&expected_workflow)?;
            anyhow::ensure!(yaml.contains("          review-mode: advisory\n"));
            anyhow::ensure!(yaml.contains("          config: .ub-review.toml\n"));
            let expected_paths = inventory(root)?;
            for _ in 0..2 {
                fs::write(&config, "profile = \"retained-regular-file\"\n")?;
                fs::write(&workflow, WORKFLOW)?;
                cmd_enable_with_resolver(args(root, inspect, true), resolve)?;
                anyhow::ensure!(fs::read(&config)? == expected_config);
                anyhow::ensure!(fs::read(&workflow)? == expected_workflow);
                let replay: toml::Value = toml::from_str(&fs::read_to_string(&config)?)?;
                anyhow::ensure!(replay == parsed);
                anyhow::ensure!(fs::read(root.join("src/lib.rs"))? == SOURCE);
                anyhow::ensure!(inventory(root)? == expected_paths);
            }
        }
    }
    Ok(())
}

#[test]
fn enable_directory_config_preserves_validation_precedence() -> Result<()> {
    for case in ["action-sha", "model", "non-force", "source-fallback"] {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        prepare_source(root)?;
        let config = root.join(".ub-review.toml");
        let workflow = root.join(".github/workflows/ub-review.yml");
        fs::create_dir(&config)?;
        fs::write(config.join("retained.txt"), CONFIG_CHILD)?;
        fs::create_dir_all(root.join(".github/workflows"))?;
        fs::write(&workflow, WORKFLOW)?;
        let before = inventory(root)?;
        for _ in 0..2 {
            let mut input = args(root, true, true);
            let expected = match case {
                "action-sha" => {
                    input.action_sha = Some("not-a-sha".to_owned());
                    input.model = "openai".to_owned();
                    "--action-sha must be the full 40-hex"
                }
                "model" => {
                    input.model = "openai".to_owned();
                    "not supported in v0"
                }
                "non-force" => {
                    input.force = false;
                    "already exists; re-run with --force"
                }
                _ => {
                    input.action_sha = None;
                    "no installable ub-review release was resolvable"
                }
            };
            let error = cmd_enable_with_resolver(input, offline_source)
                .err()
                .ok_or_else(|| anyhow::anyhow!("{case} should reject before output"))?;
            anyhow::ensure!(
                error.to_string().contains(expected),
                "{case}: expected {expected}, got {error:#}"
            );
            anyhow::ensure!(fs::read(&workflow)? == WORKFLOW);
            anyhow::ensure!(fs::read(config.join("retained.txt"))? == CONFIG_CHILD);
            anyhow::ensure!(fs::read(root.join("src/lib.rs"))? == SOURCE);
            anyhow::ensure!(inventory(root)? == before);
        }
    }
    Ok(())
}

const NEUTRAL_WORKFLOW_PATH: &str = ".github/workflows/review.yml";
const PINNED_ACTION: &str = "EffortlessMetrics/ub-review@0123456789abcdef0123456789abcdef01234567";

fn prepare_workflow(root: &Path, path: &str, contents: &str) -> Result<()> {
    prepare_source(root)?;
    fs::create_dir_all(root.join(".github/workflows"))?;
    fs::write(root.join(path), contents)?;
    Ok(())
}

fn review_workflow(steps: &str) -> String {
    format!(
        "name: Review\non: pull_request\njobs:\n  review:\n    runs-on: ubuntu-latest\n    steps:\n{steps}"
    )
}

#[test]
fn enable_refuses_existing_action_under_neutral_workflow_name_before_resolving() -> Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering};

    static RESOLVER_CALLS: AtomicUsize = AtomicUsize::new(0);
    let resolve = || {
        RESOLVER_CALLS.fetch_add(1, Ordering::Relaxed);
        offline_release()
    };
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let contents = review_workflow(&format!("      - uses: '{PINNED_ACTION}' # pinned\n"));
    prepare_workflow(root, NEUTRAL_WORKFLOW_PATH, &contents)?;
    let scans = scan_local_workflows(root)?;
    anyhow::ensure!(matches!(
        scans.as_slice(),
        [scan] if scan.path == NEUTRAL_WORKFLOW_PATH
            && matches!(scan.yaml_jobs.as_slice(), [job] if job.uses == [PINNED_ACTION])
    ));
    let before = inventory(root)?;
    for _ in 0..2 {
        let result = cmd_enable_with_resolver(args(root, false, false), resolve);
        let paths = inventory(root)?;
        let message = result.as_ref().err().map(ToString::to_string);
        anyhow::ensure!(
            message
                .as_deref()
                .is_some_and(|message| message.contains(NEUTRAL_WORKFLOW_PATH)
                    && message.contains("--force"))
                && RESOLVER_CALLS.load(Ordering::Relaxed) == 0
                && paths == before
                && !root.join(WORKFLOW_RELATIVE_PATH).exists()
                && !root.join(CONFIG_RELATIVE_PATH).exists(),
            "expected pre-resolver refusal for {NEUTRAL_WORKFLOW_PATH}, got {result:?}, {} resolver calls, paths {paths:?}",
            RESOLVER_CALLS.load(Ordering::Relaxed),
        );
        anyhow::ensure!(fs::read(root.join(NEUTRAL_WORKFLOW_PATH))? == contents.as_bytes());
        anyhow::ensure!(fs::read(root.join("src/lib.rs"))? == SOURCE);
    }
    Ok(())
}

#[test]
fn enable_permits_unrelated_workflow_with_action_mentions() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let contents = review_workflow(&format!(
        "      # uses: {PINNED_ACTION}\n      - uses: other/ub-review@v1\n        with:\n          uses: {PINNED_ACTION}\n      - uses: EffortlessMetrics/ub-review-extra@v1\n      - run: |\n          cat <<'EOF'\n          uses: {PINNED_ACTION}\n          EOF\n"
    ));
    prepare_workflow(root, NEUTRAL_WORKFLOW_PATH, &contents)?;
    let workflow = root.join(WORKFLOW_RELATIVE_PATH);
    let config = root.join(CONFIG_RELATIVE_PATH);

    cmd_enable_with_resolver(args(root, false, false), offline_release)?;
    let rendered_workflow = fs::read(&workflow)?;
    let rendered_config = fs::read(&config)?;
    let parsed: toml::Value = toml::from_str(std::str::from_utf8(&rendered_config)?)?;
    anyhow::ensure!(parsed.get("profile").and_then(toml::Value::as_str) == Some("gh-runner"));
    anyhow::ensure!(
        parsed
            .get("gate")
            .and_then(|gate| gate.get("required_check"))
            .and_then(toml::Value::as_str)
            == Some(REQUIRED_CHECK)
    );

    fs::remove_file(&workflow)?;
    fs::remove_file(&config)?;
    cmd_enable_with_resolver(args(root, false, false), offline_release)?;
    anyhow::ensure!(fs::read(&workflow)? == rendered_workflow);
    anyhow::ensure!(fs::read(&config)? == rendered_config);
    anyhow::ensure!(fs::read(root.join(NEUTRAL_WORKFLOW_PATH))? == contents.as_bytes());
    anyhow::ensure!(fs::read(root.join("src/lib.rs"))? == SOURCE);
    Ok(())
}

#[test]
fn enable_existing_action_preserves_filename_config_and_force_controls() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let legacy_path = ".github/workflows/old-ub-review.yaml";
    let neutral =
        review_workflow("      - name: Run review\n        uses: effortlessmetrics/UB-review@v1\n");
    prepare_workflow(
        root,
        legacy_path,
        "name: Legacy\non: pull_request\njobs: {}\n",
    )?;

    let error = cmd_enable_with_resolver(args(root, false, false), offline_release)
        .err()
        .context("legacy workflow name should refuse enable")?;
    anyhow::ensure!(error.to_string().contains(legacy_path));
    fs::remove_file(root.join(legacy_path))?;
    fs::write(root.join(NEUTRAL_WORKFLOW_PATH), &neutral)?;

    let config = root.join(CONFIG_RELATIVE_PATH);
    fs::write(&config, "profile = \"retained\"\n")?;
    let before = inventory(root)?;
    let error = cmd_enable_with_resolver(args(root, false, false), offline_release)
        .err()
        .context("existing config should refuse enable first")?;
    anyhow::ensure!(error.to_string().contains(&config.display().to_string()));
    anyhow::ensure!(inventory(root)? == before);
    anyhow::ensure!(fs::read(&config)? == b"profile = \"retained\"\n");

    fs::remove_file(&config)?;
    let error = cmd_enable_with_resolver(args(root, false, false), offline_release)
        .err()
        .context("existing action with a release tag should refuse enable")?;
    anyhow::ensure!(error.to_string().contains(NEUTRAL_WORKFLOW_PATH));

    cmd_enable_with_resolver(args(root, false, true), offline_release)?;
    anyhow::ensure!(root.join(WORKFLOW_RELATIVE_PATH).is_file());
    anyhow::ensure!(fs::read(root.join(NEUTRAL_WORKFLOW_PATH))? == neutral.as_bytes());
    Ok(())
}
