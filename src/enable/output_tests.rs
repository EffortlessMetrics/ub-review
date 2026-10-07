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
                let error = match cmd_enable_with_resolver(args(root, inspect, true), offline_source) {
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
            anyhow::ensure!(parsed["profile"].as_str() == Some("gh-runner"));
            anyhow::ensure!(
                parsed["repo"]["kind"].as_str() == Some(if inspect { "rust" } else { "generic" })
            );
            anyhow::ensure!(parsed["gate"]["required_check"].as_str() == Some("ub-review/gate"));
            anyhow::ensure!(
                parsed["providers"]["policy"].as_str() == Some("primary-with-fallback")
            );
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
