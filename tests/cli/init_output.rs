use super::*;

#[test]
fn init_rejects_directory_guide_before_replacing_config() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let config = temp.path().join("config.toml");
    let guide = temp.path().join("guide-directory");
    write_file(&repo.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n")?;
    fs::write(&config, b"existing config sentinel\n")?;
    write_file(&guide.join("keep.txt"), "guide directory sentinel\n")?;

    let mut failures = Vec::new();
    let mut previous_output = None;
    for attempt in 0..2 {
        let output = Command::new(env!("CARGO_BIN_EXE_ub-review"))
            .env_clear()
            .current_dir(temp.path())
            .args([
                "init",
                "--root",
                path_str(&repo)?,
                "--path",
                path_str(&config)?,
                "--guide-out",
                path_str(&guide)?,
                "--force",
            ])
            .output()?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let config_preserved = fs::read(&config)? == b"existing config sentinel\n";
        let directory_preserved =
            guide.is_dir() && fs::read(guide.join("keep.txt"))? == b"guide directory sentinel\n";
        let paths_preserved = collect_relative_file_paths(temp.path())?
            == vec!["config.toml", "guide-directory/keep.txt", "repo/src/lib.rs"];
        if output.status.success()
            || !output.stdout.is_empty()
            || !stderr.contains(path_str(&guide)?)
            || !stderr.contains("is a directory")
            || !config_preserved
            || !directory_preserved
            || !paths_preserved
        {
            failures.push(format!(
                "attempt {attempt}: status={}, stdout={:?}, stderr={stderr:?}, \
                 config_preserved={config_preserved}, directory_preserved={directory_preserved}, \
                 paths_preserved={paths_preserved}",
                output.status,
                String::from_utf8_lossy(&output.stdout)
            ));
        }
        let current_output = (output.stdout, output.stderr);
        if let Some(previous) = &previous_output {
            assert_eq!(&current_output, previous, "unchanged rejection must replay");
        }
        previous_output = Some(current_output);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(
        fs::read(repo.join("src/lib.rs"))?,
        b"pub fn answer() -> u8 { 42 }\n"
    );
    Ok(())
}

#[test]
fn init_fresh_and_forced_file_outputs_preserve_rendered_content() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let config = temp.path().join("config.toml");
    let guide = temp.path().join("guide.md");
    write_file(&repo.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n")?;
    let mut expected = None;
    for forced in [false, true, true] {
        if forced {
            fs::write(&config, b"old forced config\n")?;
            fs::write(&guide, b"old forced guide\n")?;
        }
        let mut command = Command::new(env!("CARGO_BIN_EXE_ub-review"));
        command.env_clear().current_dir(temp.path()).args([
            "init",
            "--root",
            path_str(&repo)?,
            "--path",
            path_str(&config)?,
            "--guide-out",
            path_str(&guide)?,
        ]);
        if forced {
            command.arg("--force");
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "init file output failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8(output.stdout.clone())?,
            format!("wrote {}\nwrote {}\n", config.display(), guide.display())
        );
        let config_bytes = fs::read(&config)?;
        let parsed: toml::Value = toml::from_str(std::str::from_utf8(&config_bytes)?)?;
        assert_eq!(parsed["profile"].as_str(), Some("gh-runner"));
        let guide_bytes = fs::read(&guide)?;
        assert!(
            std::str::from_utf8(&guide_bytes)?.starts_with("# ub-review init guide\n"),
            "init must render the actual guide"
        );
        assert_eq!(
            collect_relative_file_paths(temp.path())?,
            vec!["config.toml", "guide.md", "repo/src/lib.rs"]
        );
        let current = (config_bytes, guide_bytes, output.stdout, output.stderr);
        if let Some(first) = &expected {
            assert_eq!(&current, first, "forced output must match fresh rendering");
        } else {
            expected = Some(current);
        }
    }
    assert_eq!(
        fs::read(repo.join("src/lib.rs"))?,
        b"pub fn answer() -> u8 { 42 }\n"
    );
    Ok(())
}

#[test]
fn init_no_guide_ignores_directory_destination_and_missing_root() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let config = temp.path().join("config.toml");
    let guide = temp.path().join("unused-directory");
    let root = temp.path().join("unused-missing-root");
    fs::write(&config, b"old config without guide\n")?;
    write_file(&guide.join("keep.txt"), "unused guide sentinel\n")?;
    let mut expected = None;
    for _ in 0..2 {
        let output = Command::new(env!("CARGO_BIN_EXE_ub-review"))
            .env_clear()
            .current_dir(temp.path())
            .args([
                "init",
                "--root",
                path_str(&root)?,
                "--path",
                path_str(&config)?,
                "--guide-out",
                path_str(&guide)?,
                "--no-guide",
                "--force",
            ])
            .output()?;
        assert!(
            output.status.success(),
            "no-guide init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert_eq!(
            String::from_utf8(output.stdout.clone())?,
            format!("wrote {}\n", config.display())
        );
        let config_bytes = fs::read(&config)?;
        let parsed: toml::Value = toml::from_str(std::str::from_utf8(&config_bytes)?)?;
        assert_eq!(parsed["profile"].as_str(), Some("gh-runner"));
        assert!(guide.is_dir());
        assert_eq!(
            fs::read(guide.join("keep.txt"))?,
            b"unused guide sentinel\n"
        );
        assert!(!root.exists());
        assert_eq!(
            collect_relative_file_paths(temp.path())?,
            vec!["config.toml", "unused-directory/keep.txt"]
        );
        let current = (config_bytes, output.stdout, output.stderr);
        if let Some(first) = &expected {
            assert_eq!(&current, first, "no-guide rendering must replay");
        } else {
            expected = Some(current);
        }
    }
    Ok(())
}

#[test]
fn init_rejects_unusable_guide_parents_before_replacing_config() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let mut failures = Vec::new();
    for parent_is_file in [false, true] {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("repo");
        let config = temp.path().join("config.toml");
        let parent = temp.path().join("guide-parent");
        let guide = parent.join("guide.md");
        write_file(&repo.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n")?;
        if parent_is_file {
            fs::write(&parent, b"guide parent file sentinel\n")?;
        }
        let mut expected_paths = vec!["config.toml", "repo/src/lib.rs"];
        if parent_is_file {
            expected_paths.insert(1, "guide-parent");
        }
        let mut previous_output = None;
        for attempt in 0..2 {
            fs::write(&config, b"existing config sentinel\n")?;
            let output = Command::new(env!("CARGO_BIN_EXE_ub-review"))
                .env_clear()
                .current_dir(temp.path())
                .args([
                    "init",
                    "--root",
                    path_str(&repo)?,
                    "--path",
                    path_str(&config)?,
                    "--guide-out",
                    path_str(&guide)?,
                    "--force",
                ])
                .output()?;
            let stderr = String::from_utf8_lossy(&output.stderr);
            let config_preserved = fs::read(&config)? == b"existing config sentinel\n";
            let parent_preserved = if parent_is_file {
                parent.is_file() && fs::read(&parent)? == b"guide parent file sentinel\n"
            } else {
                !parent.exists()
            };
            let paths_preserved = collect_relative_file_paths(temp.path())? == expected_paths;
            if output.status.success()
                || !output.stdout.is_empty()
                || !stderr.contains(path_str(&guide)?)
                || !stderr.contains("guide parent")
                || !stderr.contains("--guide-out")
                || !config_preserved
                || !parent_preserved
                || !paths_preserved
            {
                failures.push(format!(
                    "parent_is_file={parent_is_file}, attempt {attempt}: status={}, \
                     stdout={:?}, stderr={stderr:?}, config_preserved={config_preserved}, \
                     parent_preserved={parent_preserved}, paths_preserved={paths_preserved}",
                    output.status,
                    String::from_utf8_lossy(&output.stdout)
                ));
            }
            let current_output = (output.stdout, output.stderr);
            if let Some(previous) = &previous_output {
                assert_eq!(&current_output, previous, "unchanged rejection must replay");
            }
            previous_output = Some(current_output);
        }
        assert_eq!(
            fs::read(repo.join("src/lib.rs"))?,
            b"pub fn answer() -> u8 { 42 }\n"
        );
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    Ok(())
}

#[cfg(target_os = "linux")]
#[test]
fn init_rejects_unwritable_existing_guide_before_replacing_config() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let config = temp.path().join("config.toml");
    let guide = temp.path().join("guide.md");
    let running_test = std::env::current_exe()?;
    write_file(&repo.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n")?;
    std::os::unix::fs::symlink(&running_test, &guide)?;
    assert!(fs::symlink_metadata(&guide)?.file_type().is_symlink());
    assert_eq!(fs::read_link(&guide)?, running_test);
    assert!(fs::metadata(&guide)?.is_file());

    // Linux denies write-open while this integration-test executable is running.
    // Admit the fixture without creation, truncation, or executable data access.
    let admission = fs::OpenOptions::new()
        .write(true)
        .create(false)
        .truncate(false)
        .open(&guide);
    if admission.is_ok() {
        bail!("unwritable guide fixture was not admitted; init was not invoked");
    }

    let mut failures = Vec::new();
    let mut previous_output = None;
    for attempt in 0..2 {
        fs::write(&config, b"existing config sentinel\n")?;
        let output = Command::new(env!("CARGO_BIN_EXE_ub-review"))
            .env_clear()
            .current_dir(temp.path())
            .args([
                "init",
                "--root",
                path_str(&repo)?,
                "--path",
                path_str(&config)?,
                "--guide-out",
                path_str(&guide)?,
                "--force",
            ])
            .output()?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        let config_preserved = fs::read(&config)? == b"existing config sentinel\n";
        let link_preserved = fs::symlink_metadata(&guide)?.file_type().is_symlink()
            && fs::read_link(&guide)? == running_test;
        let paths_preserved = collect_relative_file_paths(temp.path())?
            == vec!["config.toml", "repo/src/lib.rs"];
        if output.status.success()
            || !output.stdout.is_empty()
            || !stderr.contains(path_str(&guide)?)
            || !stderr.contains("cannot write guide")
            || !stderr.contains("--guide-out")
            || !config_preserved
            || !link_preserved
            || !paths_preserved
        {
            failures.push(format!(
                "attempt {attempt}: status={}, stdout={:?}, stderr={stderr:?}, \
                 config_preserved={config_preserved}, link_preserved={link_preserved}, \
                 paths_preserved={paths_preserved}",
                output.status,
                String::from_utf8_lossy(&output.stdout)
            ));
        }
        let current_output = (output.stdout, output.stderr);
        if let Some(previous) = &previous_output {
            assert_eq!(&current_output, previous, "unchanged rejection must replay");
        }
        previous_output = Some(current_output);
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    assert_eq!(
        fs::read(repo.join("src/lib.rs"))?,
        b"pub fn answer() -> u8 { 42 }\n"
    );
    assert_eq!(fs::read_link(&guide)?, running_test);
    Ok(())
}

#[test]
fn init_relative_guide_filename_preserves_successful_output_and_replay() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    write_file(&repo.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n")?;
    let mut expected = None;
    for forced in [false, true, true] {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ub-review"));
        command.env_clear().current_dir(temp.path()).args([
            "init",
            "--root",
            path_str(&repo)?,
            "--path",
            "config.toml",
            "--guide-out",
            "guide.md",
        ]);
        if forced {
            command.arg("--force");
        }
        let output = command.output()?;
        assert!(
            output.status.success(),
            "relative guide failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
        assert_eq!(output.stdout, b"wrote config.toml\nwrote guide.md\n");
        let config_bytes = fs::read(temp.path().join("config.toml"))?;
        let parsed: toml::Value = toml::from_str(std::str::from_utf8(&config_bytes)?)?;
        assert_eq!(parsed["profile"].as_str(), Some("gh-runner"));
        let guide_bytes = fs::read(temp.path().join("guide.md"))?;
        assert!(std::str::from_utf8(&guide_bytes)?.starts_with("# ub-review init guide\n"));
        assert_eq!(
            collect_relative_file_paths(temp.path())?,
            vec!["config.toml", "guide.md", "repo/src/lib.rs"]
        );
        let current = (config_bytes, guide_bytes, output.stdout, output.stderr);
        if let Some(first) = &expected {
            assert_eq!(&current, first, "relative output must replay");
        } else {
            expected = Some(current);
        }
    }
    assert_eq!(
        fs::read(repo.join("src/lib.rs"))?,
        b"pub fn answer() -> u8 { 42 }\n"
    );
    Ok(())
}

#[test]
fn init_guide_preflight_preserves_writable_guide_when_config_write_fails() -> Result<()> {
    let _cli_subprocess_guard = cli_subprocess_test_lock()?;
    let temp = tempfile::tempdir()?;
    let repo = temp.path().join("repo");
    let config = temp.path().join("config-directory");
    let guide = temp.path().join("guide.md");
    write_file(&repo.join("src/lib.rs"), "pub fn answer() -> u8 { 42 }\n")?;
    write_file(&config.join("keep.txt"), "config directory sentinel\n")?;
    fs::write(&guide, b"existing writable guide sentinel\n")?;
    let mut previous_output = None;
    for _ in 0..2 {
        let output = Command::new(env!("CARGO_BIN_EXE_ub-review"))
            .env_clear()
            .current_dir(temp.path())
            .args([
                "init",
                "--root",
                path_str(&repo)?,
                "--path",
                path_str(&config)?,
                "--guide-out",
                path_str(&guide)?,
                "--force",
            ])
            .output()?;
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(config.is_dir());
        assert_eq!(
            fs::read(config.join("keep.txt"))?,
            b"config directory sentinel\n"
        );
        assert_eq!(fs::read(&guide)?, b"existing writable guide sentinel\n");
        assert_eq!(
            collect_relative_file_paths(temp.path())?,
            vec!["config-directory/keep.txt", "guide.md", "repo/src/lib.rs"]
        );
        let current_output = (output.stdout, output.stderr);
        if let Some(previous) = &previous_output {
            assert_eq!(&current_output, previous, "unchanged failure must replay");
        }
        previous_output = Some(current_output);
    }
    assert_eq!(
        fs::read(repo.join("src/lib.rs"))?,
        b"pub fn answer() -> u8 { 42 }\n"
    );
    Ok(())
}
