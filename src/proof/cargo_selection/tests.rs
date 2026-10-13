use super::*;
use anyhow::ensure;

// Source facts for legacy lifecycle mocks: only execution is mocked, not
// package/target provenance. Metadata is still queried through native Cargo.
pub(crate) fn declared_mock_package(root: &Path, name: &str, test: Option<&str>) -> Result<()> {
    fs::create_dir_all(root.join("src"))?;
    let tests = test
        .map(|name| format!("[[test]]\nname=\"{name}\"\npath=\"probe.rs\"\n"))
        .unwrap_or_default();
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname=\"{name}\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[workspace]\n{tests}"
        ),
    )?;
    fs::write(root.join("src/lib.rs"), "pub fn value()->u8{1}\n")?;
    fs::write(root.join("probe.rs"), "#[test] fn declaration() {}\n")?;
    fs::write(
        root.join("Cargo.lock"),
        format!("version=4\n[[package]]\nname=\"{name}\"\nversion=\"0.1.0\"\n"),
    )?;
    Ok(())
}

fn package(root: &Path, name: &str, version: &str, deps: &str, tests: &str) -> Result<()> {
    fs::create_dir_all(root.join("src"))?;
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname=\"{name}\"\nversion=\"{version}\"\nedition=\"2024\"\n{deps}{tests}"
        ),
    )?;
    fs::write(
        root.join("src/lib.rs"),
        "#[test]\nfn passes(){assert_eq!(2+2,4);}\n#[test]\nfn fails(){assert_eq!(2+2,5);}\n",
    )?;
    Ok(())
}

fn fixture(root: &Path) -> Result<()> {
    fs::create_dir_all(root)?;
    fs::write(
        root.join("Cargo.toml"),
        "[workspace]\nmembers=[\"alpha\",\"beta\"]\ndefault-members=[\"alpha\"]\nexclude=[\"external\"]\nresolver=\"3\"\n",
    )?;
    package(
        &root.join("alpha"),
        "alpha",
        "0.1.0",
        "[dependencies]\nexternal-dep={path=\"../external\"}\n",
        "[features]\nproof=[]\n[[test]]\nname=\"plain_probe\"\npath=\"probes/plain.rs\"\n[[test]]\nname=\"feature_gate\"\npath=\"probes/feature.rs\"\nrequired-features=[\"proof\"]\n[[test]]\nname=\"custom_probe\"\npath=\"probes/custom.rs\"\nharness=false\n",
    )?;
    package(
        &root.join("beta"),
        "beta",
        "0.1.0",
        "",
        "[[test]]\nname=\"beta_only\"\npath=\"probes/beta.rs\"\n",
    )?;
    package(
        &root.join("external"),
        "external-dep",
        "0.1.0",
        "",
        "[[test]]\nname=\"external_only\"\npath=\"probes/external.rs\"\n",
    )?;
    for (directory, files) in [
        ("alpha", vec!["plain", "feature"]),
        ("beta", vec!["beta"]),
        ("external", vec!["external"]),
    ] {
        fs::create_dir_all(root.join(directory).join("probes"))?;
        for file in files {
            fs::write(
                root.join(directory)
                    .join("probes")
                    .join(format!("{file}.rs")),
                "#[test]\nfn passes(){assert_eq!(2+2,4);}\n#[test]\nfn fails(){assert_eq!(2+2,5);}\n",
            )?;
        }
    }
    fs::write(
        root.join("alpha/probes/custom.rs"),
        "fn main(){println!(\"custom harness executed\");}\n",
    )?;
    fs::write(
        root.join("Cargo.lock"),
        "version=4\n[[package]]\nname=\"alpha\"\nversion=\"0.1.0\"\ndependencies=[\"external-dep\"]\n[[package]]\nname=\"beta\"\nversion=\"0.1.0\"\n[[package]]\nname=\"external-dep\"\nversion=\"0.1.0\"\n",
    )?;
    Ok(())
}

fn lease(id: &str) -> ResourceLease {
    ResourceLease {
        revision: None,
        schema: RESOURCE_LEASE_SCHEMA.to_owned(),
        id: format!("lease-{id}"),
        kind: "focused-test".to_owned(),
        consumer: id.to_owned(),
        status: "granted".to_owned(),
        reason: "controlled grant".to_owned(),
        cpu: 1,
        memory_mb: 512,
        disk_mb: 64,
        timeout_sec: 30,
        network: false,
        scratch: false,
        worktree: None,
        command: None,
    }
}
fn spec(root: &Path, tail: &[&str]) -> ProofCommandSpec {
    ProofCommandSpec {
        argv: [
            vec!["cargo".to_owned(), "test".to_owned(), "--locked".to_owned()],
            tail.iter().map(|s| (*s).to_owned()).collect(),
        ]
        .concat(),
        env: BTreeMap::from([(
            "CARGO_TARGET_DIR".to_owned(),
            root.join("target").display().to_string(),
        )]),
    }
}
fn invoke(root: &Path, id: &str, tail: &[&str]) -> Result<(SelectionCheckedCommand, usize)> {
    let spec = spec(root, tail);
    let out = root.join("receipts").join(id);
    let mut calls = 0;
    let result = run_selection_checked_command(
        ProofCommandInvocation {
            command_root: root,
            out: &out,
            receipt_id: id,
            side: "head",
            spec: &spec,
            timeout_sec: 30,
            lease: &lease(id),
            task_ledger: None,
            task: None,
            cleanup: None,
        },
        &mut |root, argv, env, timeout, stdout, stderr, observer| {
            calls += 1;
            ensure!(timeout <= 30 && timeout > 0);
            run_command_to_files_with_spawn_observer(
                root, argv, env, timeout, stdout, stderr, observer,
            )
        },
    )?;
    Ok((result, calls))
}

#[test]
fn native_guard_distinguishes_target_absence_assertion_success_and_metadata_failure() -> Result<()>
{
    let temp = tempfile::tempdir()?;
    fixture(temp.path())?;
    for (id, tail, unresolved, exit, calls) in [
        (
            "absent-target",
            vec!["-p", "alpha", "--test", "absent"],
            true,
            None,
            0,
        ),
        (
            "absent-package",
            vec!["-p", "absent", "--test", "missing"],
            true,
            None,
            0,
        ),
        (
            "real-assertion",
            vec!["-p", "alpha", "--test", "plain_probe", "fails"],
            false,
            Some(101),
            1,
        ),
        (
            "real-success",
            vec!["-p", "alpha", "--test", "plain_probe", "passes"],
            false,
            Some(0),
            1,
        ),
        (
            "custom-path-harness",
            vec!["-p", "alpha", "--test", "custom_probe"],
            false,
            Some(0),
            1,
        ),
        (
            "non-workspace-dependency",
            vec!["-p", "external-dep", "--test", "external_only", "passes"],
            false,
            Some(0),
            1,
        ),
        (
            "unit-assertion",
            vec!["-p", "alpha", "--lib", "fails"],
            false,
            Some(101),
            1,
        ),
        (
            "disabled-required-feature",
            vec!["-p", "alpha", "--test", "feature_gate"],
            true,
            None,
            0,
        ),
    ] {
        let (result, observed) = invoke(temp.path(), id, &tail)?;
        ensure!(
            result.unresolved == unresolved
                && result.command.exit_code == exit
                && observed == calls,
            "{id}: {result:?}, calls={observed}"
        );
        if unresolved {
            ensure!(result.command.status == "skipped" && result.command.duration_ms == 0);
        }
    }
    fs::remove_file(temp.path().join("Cargo.lock"))?;
    let (unavailable, calls) = invoke(
        temp.path(),
        "missing-lock",
        &["-p", "alpha", "--test", "plain_probe"],
    )?;
    ensure!(unavailable.unresolved && calls == 0 && unavailable.command.exit_code.is_none());
    ensure!(
        !temp.path().join("Cargo.lock").exists(),
        "locked query must not create a lockfile"
    );
    Ok(())
}

#[test]
fn selectors_outside_exact_adapter_are_unresolved_without_claiming_absence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path())?;
    for tail in [
        vec!["-p", "alpha@0.1.0", "--lib"],
        vec!["-p", "alpha", "--test", "plain_*"],
        vec![
            "-p",
            "alpha",
            "--test",
            "feature_gate",
            "--features",
            "proof",
        ],
        vec![
            "-p",
            "alpha",
            "--test",
            "plain_probe",
            "--target",
            "custom.json",
        ],
        vec!["--manifest-path", "beta/Cargo.toml", "--test", "beta_only"],
        vec!["--workspace", "--exclude", "absent", "--test", "beta_only"],
        vec!["--test", "beta_only"],
    ] {
        let (result, calls) = invoke(temp.path(), "unknown-selector", &tail)?;
        ensure!(result.unresolved && calls == 0);
        ensure!(
            result.command.reason.contains("unresolved")
                && !result
                    .command
                    .reason
                    .contains("without integration-test target")
        );
        ensure!(result.command.command.contains(tail[0]));
    }
    Ok(())
}

#[test]
fn incomplete_and_ambiguous_metadata_never_prove_absence() -> Result<()> {
    let selection = ExactSelection {
        package: "twin".to_owned(),
        test: Some("missing".to_owned()),
    };
    for bytes in [b"{}".as_slice(), b"{\"packages\":[],\"resolve\":null}".as_slice(),
        br#"{"packages":[{"id":"one","name":"twin","targets":[]},{"id":"two","name":"twin","targets":[]}],"resolve":{"nodes":[{"id":"one"},{"id":"two"}]}}"#] {
        ensure!(matches!(check_metadata(&selection, bytes), CargoSelectionProvenance::Unresolved { .. }));
    }
    Ok(())
}

fn duplicated_fixture(root: &Path, same_version: bool) -> Result<()> {
    fs::create_dir_all(root)?;
    let name = if same_version { "same" } else { "twin" };
    fs::write(
        root.join("Cargo.toml"),
        format!(
            "[package]\nname=\"duplicate-root\"\nversion=\"0.1.0\"\nedition=\"2024\"\n[dependencies]\none={{package=\"{name}\",path=\"one\"}}\ntwo={{package=\"{name}\",path=\"two\"}}\n"
        ),
    )?;
    fs::create_dir_all(root.join("src"))?;
    fs::write(root.join("src/lib.rs"), "pub fn value()->u8{1}\n")?;
    package(&root.join("one"), name, "0.1.0", "", "")?;
    package(
        &root.join("two"),
        name,
        if same_version { "0.1.0" } else { "0.2.0" },
        "",
        "",
    )?;
    if !same_version {
        fs::write(
            root.join("Cargo.lock"),
            "version=4\n[[package]]\nname=\"duplicate-root\"\nversion=\"0.1.0\"\ndependencies=[\"twin 0.1.0\",\"twin 0.2.0\"]\n[[package]]\nname=\"twin\"\nversion=\"0.1.0\"\n[[package]]\nname=\"twin\"\nversion=\"0.2.0\"\n",
        )?;
    }
    Ok(())
}

#[test]
fn independently_supplied_fourteen_selector_inputs_exercise_actual_guard() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let core = temp.path().join("core");
    fixture(&core)?;
    let duplicates = temp.path().join("duplicates");
    duplicated_fixture(&duplicates, false)?;
    let same = temp.path().join("same-version");
    duplicated_fixture(&same, true)?;
    for (number, root, tail, expected_calls) in [
        (
            1,
            &core,
            vec!["-p", "external-dep", "--test", "external_only"],
            1,
        ),
        (2, &core, vec!["-p", "absent", "--test", "missing"], 0),
        (3, &core, vec!["--workspace", "--test", "beta_only"], 0),
        (4, &core, vec!["--test", "beta_only"], 0),
        (5, &core, vec!["--test", "smoke_*"], 0),
        (6, &core, vec!["--workspace", "--test", "missing"], 0),
        (7, &core, vec!["--test", "feature_gate"], 0),
        (8, &core, vec!["--test", "custom_probe"], 0),
        (9, &core, vec!["--test", "smoke_one", "absent_filter"], 0),
        (
            10,
            &core,
            vec!["--manifest-path", "beta/Cargo.toml", "--test", "beta_only"],
            0,
        ),
        (
            11,
            &core,
            vec!["--workspace", "--exclude", "absent", "--test", "beta_only"],
            0,
        ),
        (12, &duplicates, vec!["-p", "twin@0.2.0", "--lib"], 0),
        (13, &duplicates, vec!["-p", "twin", "--lib"], 0),
        (14, &same, vec!["-p", "same", "--lib"], 0),
    ] {
        let (result, calls) = invoke(root, &format!("matrix-{number}"), &tail)?;
        ensure!(
            calls == expected_calls,
            "matrix {number}: {result:?}, calls={calls}"
        );
        if number == 1 {
            // The external fixture contains a deliberate failing assertion;
            // this confirms real execution, not a false package-absence skip.
            ensure!(!result.unresolved && result.command.exit_code == Some(101));
        } else {
            ensure!(result.unresolved && result.command.exit_code.is_none());
            ensure!(
                !result
                    .command
                    .reason
                    .contains("without integration-test target")
            );
        }
    }
    // Rows4 and6 are known absent to the broader independent native fixture,
    // but this narrow adapter deliberately does not emulate default-member
    // or workspace unions. It retains unresolved rather than guessing.
    Ok(())
}

#[test]
fn missing_tool_declines_without_running_proof_or_losing_prepared_root_cleanup() -> Result<()> {
    let temp = tempfile::tempdir()?;
    fixture(temp.path())?;
    let id = "missing-tool";
    let mut spec = spec(temp.path(), &["-p", "alpha", "--test", "plain_probe"]);
    spec.env.insert(
        "PATH".to_owned(),
        temp.path().join("empty-path").display().to_string(),
    );
    let out = temp.path().join("receipts");
    let mut cleanup_calls = 0;
    let mut proof_calls = 0;
    let mut cleanup = || {
        cleanup_calls += 1;
        Ok(())
    };
    let result = run_selection_checked_command(
        ProofCommandInvocation {
            command_root: temp.path(),
            out: &out,
            receipt_id: id,
            side: "base-plus-tests",
            spec: &spec,
            timeout_sec: 30,
            lease: &lease(id),
            task_ledger: None,
            task: None,
            cleanup: Some(&mut cleanup),
        },
        &mut |root, argv, env, timeout, stdout, stderr, observer| {
            proof_calls += 1;
            run_command_to_files_with_spawn_observer(
                root, argv, env, timeout, stdout, stderr, observer,
            )
        },
    )?;
    ensure!(result.unresolved && result.command.status == "skipped");
    ensure!(result.command.exit_code.is_none() && proof_calls == 0 && cleanup_calls == 1);
    Ok(())
}

#[test]
fn absent_base_target_cleans_before_decline_and_head_release_waits_for_publication() -> Result<()> {
    use crate::task_ledger::{TaskEvent, TaskNonExecutionDisposition};
    let temp = tempfile::tempdir()?;
    let head = temp.path().join("head");
    let base = temp.path().join("base");
    declared_mock_package(&head, "fixture", Some("probe"))?;
    declared_mock_package(&base, "fixture", None)?;
    let spec = spec(&head, &["-p", "fixture", "--test", "probe"]);
    let task = FocusedTestTask {
        id: "base-absent".to_owned(),
        file: "probe.rs".to_owned(),
        test_name: Some("probe".to_owned()),
        mode: FocusedProofMode::RedGreen,
        command_specs: Some(FocusedTestCommandSpecs {
            head: spec.clone(),
            base_plus_tests: spec,
        }),
        timeout_sec: Some(30),
        required: false,
        requested_by: vec!["tests".to_owned()],
        request_ids: vec!["request-base-absent".to_owned()],
    };
    let revision = RevisionRef {
        digest: "a".repeat(64),
        semantics: "candidate_head".to_owned(),
        reviewed_commit: "b".repeat(40),
    };
    let recorder = TaskLedgerRecorder::new(&revision, &Instant::now())?;
    let ledger = ProofTaskLedger::new(recorder.clone());
    let diff = DiffContext {
        base: "a".repeat(40),
        head: "b".repeat(40),
        changed_files: vec!["probe.rs".to_owned()],
        patch: String::new(),
        flags: DiffFlags::default(),
        diff_class: DiffClass::SourceUb,
    };
    let out = temp.path().join("out");
    let mut cleanup_calls = 0;
    let mut proof_calls = 0;
    let receipt = super::super::red_green::run_focused_red_green_proof_task_with_cleanup(
        &head,
        &out,
        &diff,
        &task,
        30,
        &lease(&task.id),
        Some(&ledger),
        ProofExecutionPhase::ModelRequest,
        &mut |root, argv, env, timeout, stdout, stderr, observer| {
            proof_calls += 1;
            run_command_to_files_with_spawn_observer(
                root, argv, env, timeout, stdout, stderr, observer,
            )
        },
        &mut |_root, _out, _diff| Ok(base.clone()),
        &mut |_root, prepared| {
            ensure!(prepared == base && prepared.is_dir());
            let before = recorder.inputs()?;
            ensure!(!before.iter().any(|input| matches!(
                input.event,
                TaskEvent::TerminallyDeclined { .. } | TaskEvent::ResourcesReleased { .. }
            )));
            fs::remove_dir_all(prepared)?;
            cleanup_calls += 1;
            Ok(())
        },
    )?;
    ensure!(receipt.result == "skipped_unresolved" && receipt.commands.len() == 2);
    ensure!(receipt.commands[0].status == "passed" && receipt.commands[1].status == "skipped");
    ensure!(cleanup_calls == 1 && proof_calls == 1 && !base.exists());
    let before = recorder.inputs()?;
    ensure!(before.iter().any(|input| matches!(
        input.event,
        TaskEvent::TerminallyDeclined {
            disposition: TaskNonExecutionDisposition::Unsupported,
            ..
        }
    )));
    ensure!(
        before
            .iter()
            .filter(|input| matches!(input.event, TaskEvent::RunStarted { .. }))
            .count()
            == 1
    );
    ensure!(
        !before
            .iter()
            .any(|input| matches!(input.event, TaskEvent::ResourcesReleased { .. }))
    );
    write_proof_receipt_artifacts(&out, &[receipt], Some(&revision))?;
    ledger.reconcile_published_receipts(&out)?;
    let after = recorder.inputs()?;
    ensure!(
        after
            .iter()
            .filter(|input| matches!(input.event, TaskEvent::ResourcesReleased { .. }))
            .count()
            == 1
    );
    ensure!(
        after
            .iter()
            .filter(|input| matches!(input.event, TaskEvent::RunStarted { .. }))
            .count()
            == 1
    );
    Ok(())
}

#[test]
fn preparation_and_cleanup_errors_remain_errors_without_proof_execution() -> Result<()> {
    let temp = tempfile::tempdir()?;
    declared_mock_package(temp.path(), "fixture", None)?;
    let spec = spec(temp.path(), &["-p", "fixture", "--test", "absent"]);
    for broken_output in [false, true] {
        let out = temp
            .path()
            .join(if broken_output { "blocked" } else { "out" });
        if broken_output {
            fs::write(&out, b"not a directory")?;
        }
        let mut cleanup_calls = 0;
        let mut cleanup = || {
            cleanup_calls += 1;
            if broken_output {
                Ok(())
            } else {
                anyhow::bail!("controlled cleanup failure")
            }
        };
        let error = run_selection_checked_command(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: "error",
                side: "base-plus-tests",
                spec: &spec,
                timeout_sec: 30,
                lease: &lease("error"),
                task_ledger: None,
                task: None,
                cleanup: Some(&mut cleanup),
            },
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, _observer| {
                anyhow::bail!("proof runner must not execute")
            },
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("expected preparation/cleanup failure"))?;
        ensure!(cleanup_calls == 1);
        if !broken_output {
            ensure!(format!("{error:#}").contains("controlled cleanup failure"));
        }
        ensure!(!out.join("proof/error/base-plus-tests/stdout.txt").exists());
    }
    Ok(())
}

#[cfg(unix)]
#[test]
fn metadata_output_cap_and_failed_completion_decline_before_proof() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let bin = root.join("bin");
    fs::create_dir(&bin)?;
    // Valid declaration plus JSON whitespace isolates the byte boundary from parsing.
    let json = br#"{"packages":[{"id":"fixture-id","name":"fixture","targets":[{"name":"probe","kind":["test"],"src_path":"probe.rs"}]}],"resolve":{"nodes":[{"id":"fixture-id"}]}}"#;
    let cap = 4 * 1024 * 1024;
    assert_eq!(METADATA_READ_CAP, cap as u64);
    let mut payload = json.to_vec();
    payload.resize(cap, b' ');
    let shim = bin.join("cargo");
    fs::write(
        &shim,
        "#!/bin/sh\nif [ \"$1\" = metadata ]; then\n /bin/cat metadata.json\n /bin/cat diagnostics.txt >&2\n exit \"$METADATA_EXIT\"\nfi\nexit 0\n",
    )?;
    fs::set_permissions(&shim, fs::Permissions::from_mode(0o755))?;
    for (id, bytes, exit, unresolved, expected_calls) in [
        ("exact-cap", cap, "0", false, 1),
        ("over-cap", cap + 1, "0", true, 0),
        ("failed-completion", cap, "1", true, 0),
    ] {
        payload.resize(bytes, b' ');
        fs::write(root.join("metadata.json"), &payload)?;
        fs::write(root.join("diagnostics.txt"), vec![b'd'; bytes])?;
        let mut spec = spec(root, &["-p", "fixture", "--test", "probe"]);
        spec.env
            .insert("PATH".to_owned(), bin.display().to_string());
        spec.env.insert("METADATA_EXIT".to_owned(), exit.to_owned());
        let out = root.join(id);
        let mut calls = 0;
        let result = run_selection_checked_command(
            ProofCommandInvocation {
                command_root: root,
                out: &out,
                receipt_id: id,
                side: "head",
                spec: &spec,
                timeout_sec: 30,
                lease: &lease(id),
                task_ledger: None,
                task: None,
                cleanup: None,
            },
            &mut |root, argv, env, timeout, stdout, stderr, observer| {
                calls += 1;
                run_command_to_files_with_spawn_observer(
                    root, argv, env, timeout, stdout, stderr, observer,
                )
            },
        )?;
        assert_eq!(result.unresolved, unresolved, "{id}");
        assert_eq!(calls, expected_calls, "{id}");
        let paths = proof_command_paths(&out, id, "head-cargo-metadata")?;
        if id == "over-cap" {
            assert_eq!(
                fs::read(paths.stdout_path)?,
                b"metadata exceeded the read cap; selection unresolved\n"
            );
            assert_eq!(
                fs::read(paths.stderr_path)?,
                b"metadata diagnostics exceeded the retention cap\n"
            );
            assert!(
                result
                    .command
                    .reason
                    .contains("metadata exceeded the 4 MiB read budget")
            );
        } else {
            assert_eq!(fs::read(paths.stdout_path)?, payload);
            assert_eq!(fs::metadata(paths.stderr_path)?.len(), cap as u64);
        }
        if unresolved {
            assert_eq!(result.command.status, "skipped");
            assert_eq!(result.command.exit_code, None);
            assert_eq!(result.command.duration_ms, 0);
            assert!(!result.command.timed_out);
            assert!(
                !result
                    .command
                    .reason
                    .contains("without integration-test target")
            );
            assert!(!result.command.reason.contains("matched 0 packages"));
        } else {
            assert_eq!(result.command.status, "passed");
            assert_eq!(result.command.exit_code, Some(0));
        }
    }
    Ok(())
}
