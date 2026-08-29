from pathlib import Path


def replace_one(path: str, old: str, new: str) -> None:
    source = Path(path)
    text = source.read_text(encoding="utf-8")
    count = text.count(old)
    if count != 1:
        raise SystemExit(f"{path}: expected one replacement, found {count}")
    source.write_text(text.replace(old, new, 1), encoding="utf-8")


replace_one(
    "src/main.rs",
    """{
    let out = Path::new(&args.out);
    let root = Path::new(&args.root);
    let revision = standalone_worker_revision(root, &request)?;
    let recorder = TaskLedgerRecorder::new(&revision, &Instant::now())?;
    let task_ledger = ProofTaskLedger::new(recorder);
    let timeout_sec = args.timeout_sec.max(request.timeout_sec).max(1);
""",
    """{
    validate_worker_request_id(&request.id)?;
    let out = Path::new(&args.out);
    let root = Path::new(&args.root);
    let revision = standalone_worker_revision(root, &request)?;
    let recorder = TaskLedgerRecorder::new(&revision, &Instant::now())?;
    let task_ledger = ProofTaskLedger::new(recorder);
    let timeout_sec = worker_timeout_sec(args.timeout_sec, request.timeout_sec);
""",
)

replace_one(
    "src/main.rs",
    """    if let Err(error) = publish_result {
        let failure = format!("standalone worker result publication failed: {error:#}");
        fail_pending_worker_receipts(&task_ledger, [&preflight_task, &command_task], &failure)?;
        task_ledger.write_artifacts(out)?;
        return Err(error).context("publish standalone worker proof result");
    }
""",
    """    if let Err(error) = publish_result {
        let artifact_cleanup = remove_worker_publication_artifacts(out);
        let failure = format!("standalone worker result publication failed: {error:#}");
        let ledger_failure = (|| -> Result<()> {
            fail_pending_worker_receipts(
                &task_ledger,
                [&preflight_task, &command_task],
                &failure,
            )?;
            task_ledger.write_artifacts(out)
        })();
        let mut context = "publish standalone worker proof result".to_owned();
        if let Err(cleanup_error) = artifact_cleanup {
            context.push_str(&format!(
                "; canonical artifact cleanup also failed: {cleanup_error:#}"
            ));
        }
        if let Err(ledger_error) = ledger_failure {
            context.push_str(&format!(
                "; TaskLedger failure reconciliation also failed: {ledger_error:#}"
            ));
        }
        return Err(error).context(context);
    }
""",
)

replace_one(
    "src/main.rs",
    """fn prepare_worker_attempt(out: &Path, request_id: &str) -> Result<()> {
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    crate::task_ledger_artifact::remove_task_ledger_artifacts(out)?;
    remove_optional_worker_file(&out.join("proof_receipt.json"))?;
    remove_optional_worker_file(&out.join("resource_lease.json"))?;
    remove_optional_worker_file(&out.join("proof_receipt.json.tmp"))?;
    remove_optional_worker_file(&out.join("resource_lease.json.tmp"))?;
    let proof_dir = out.join("proof").join(request_id);
""",
    """fn worker_timeout_sec(operator_ceiling_sec: u64, request_timeout_sec: u64) -> u64 {
    request_timeout_sec.min(operator_ceiling_sec).max(1)
}

fn validate_worker_request_id(request_id: &str) -> Result<()> {
    let path = Path::new(request_id);
    let mut components = path.components();
    let is_single_component = matches!(
        components.next(),
        Some(std::path::Component::Normal(component))
            if component == std::ffi::OsStr::new(request_id)
    ) && components.next().is_none();
    anyhow::ensure!(
        is_single_component
            && !request_id
                .chars()
                .any(|character| matches!(character, '/' | '\\')),
        "standalone worker request id must be one portable path component"
    );
    Ok(())
}

fn prepare_worker_attempt(out: &Path, request_id: &str) -> Result<()> {
    validate_worker_request_id(request_id)?;
    fs::create_dir_all(out).with_context(|| format!("create {}", out.display()))?;
    crate::task_ledger_artifact::remove_task_ledger_artifacts(out)?;
    remove_worker_publication_artifacts(out)?;
    let proof_dir = out.join("proof").join(request_id);
""",
)

replace_one(
    "src/main.rs",
    """fn remove_optional_worker_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove stale {}", path.display())),
    }
}

""",
    """fn remove_optional_worker_file(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove stale {}", path.display())),
    }
}

fn remove_worker_publication_artifacts(out: &Path) -> Result<()> {
    for name in [
        "proof_receipt.json",
        "resource_lease.json",
        "proof_receipt.json.tmp",
        "resource_lease.json.tmp",
    ] {
        remove_optional_worker_file(&out.join(name))?;
    }
    Ok(())
}

""",
)

replace_one(
    "src/main.rs",
    """    #[test]
    fn worker_preflight_and_proof_are_distinct_revision_bound_tasks() -> Result<()> {
""",
    """    #[test]
    fn worker_request_timeout_cannot_raise_operator_ceiling() -> Result<()> {
        ensure!(worker_timeout_sec(30, 7) == 7);
        ensure!(worker_timeout_sec(7, 30) == 7);
        ensure!(worker_timeout_sec(0, 30) == 1);

        let temp = tempfile::tempdir()?;
        let (root, base, head) = worker_repo(&temp)?;
        let out = temp.path().join("out");
        let mut request = request(
            "proof-request-timeout",
            &base,
            &head,
            ProofKind::FocusedTest,
            "cargo test --locked worker_test",
        );
        request.timeout_sec = 300;
        let mut worker_args = args(&root, &out);
        worker_args.timeout_sec = 5;
        let mut observed_timeouts = Vec::new();

        run_worker_request_with_runner(
            &worker_args,
            request,
            &mut |_root, _argv, _env, timeout, stdout, stderr, observe_process| {
                observed_timeouts.push(timeout);
                observe_process(CommandProcessObservation::Spawned);
                fs::write(stdout, b"ok\n")?;
                fs::write(stderr, b"")?;
                Ok(successful_status())
            },
        )?;

        ensure!(observed_timeouts == [5, 5]);
        let receipt: ProofReceipt =
            serde_json::from_slice(&fs::read(out.join("proof_receipt.json"))?)?;
        ensure!(
            receipt
                .commands
                .iter()
                .all(|command| command.timeout_sec == 5)
        );
        Ok(())
    }

    #[test]
    fn worker_preflight_and_proof_are_distinct_revision_bound_tasks() -> Result<()> {
""",
)

replace_one(
    "src/main.rs",
    """    #[test]
    fn symbolic_worker_revision_is_rejected_before_any_spawn() -> Result<()> {
""",
    """    #[test]
    fn worker_request_id_cannot_escape_the_output_directory() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (root, base, head) = worker_repo(&temp)?;
        let out = temp.path().join("out");
        let protected = temp.path().join("protected");
        fs::create_dir_all(&protected)?;
        let sentinel = protected.join("sentinel.txt");
        fs::write(&sentinel, b"retain")?;
        let request_id = protected.display().to_string();
        let request = request(
            &request_id,
            &base,
            &head,
            ProofKind::FocusedTest,
            "cargo test --locked worker_test",
        );
        let mut calls = 0_usize;

        let error = run_worker_request_with_runner(
            &args(&root, &out),
            request,
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                calls += 1;
                Ok(successful_status())
            },
        )
        .err()
        .context("absolute worker request id must fail closed")?;

        ensure!(calls == 0);
        ensure!(
            format!("{error:#}")
                .contains("request id must be one portable path component")
        );
        ensure!(fs::read(&sentinel)? == b"retain");
        ensure!(!out.exists());
        Ok(())
    }

    #[test]
    fn worker_checkout_head_must_match_the_admitted_request() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (root, base, _head) = worker_repo(&temp)?;
        let out = temp.path().join("out");
        let request = request(
            "proof-request-wrong-head",
            &base,
            &base,
            ProofKind::FocusedTest,
            "cargo test --locked worker_test",
        );
        let mut calls = 0_usize;

        let error = run_worker_request_with_runner(
            &args(&root, &out),
            request,
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                calls += 1;
                Ok(successful_status())
            },
        )
        .err()
        .context("wrong worker checkout head must fail closed")?;

        ensure!(calls == 0);
        ensure!(format!("{error:#}").contains("checkout HEAD"));
        ensure!(format!("{error:#}").contains("does not match admitted head"));
        Ok(())
    }

    #[test]
    fn worker_checkout_must_be_clean_before_proof_execution() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (root, base, head) = worker_repo(&temp)?;
        let out = temp.path().join("out");
        fs::write(root.join("fixture.txt"), "dirty tracked bytes\n")?;
        let request = request(
            "proof-request-dirty",
            &base,
            &head,
            ProofKind::FocusedTest,
            "cargo test --locked worker_test",
        );
        let mut calls = 0_usize;

        let error = run_worker_request_with_runner(
            &args(&root, &out),
            request,
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, _observe_process| {
                calls += 1;
                Ok(successful_status())
            },
        )
        .err()
        .context("dirty worker checkout must fail closed")?;

        ensure!(calls == 0);
        ensure!(format!("{error:#}").contains("checkout must be clean"));
        ensure!(format!("{error:#}").contains("fixture.txt"));
        Ok(())
    }

    #[test]
    fn symbolic_worker_revision_is_rejected_before_any_spawn() -> Result<()> {
""",
)

replace_one(
    "src/main.rs",
    """        ensure!(calls == 2);
        ensure!(format!("{error:#}").contains("publish standalone worker proof result"));
        ensure!(!out.join("proof_receipt.json").exists());
        for side in ["nightly-preflight", "head"] {
""",
    """        ensure!(calls == 2);
        ensure!(format!("{error:#}").contains("publish standalone worker proof result"));
        ensure!(!out.join("proof_receipt.json").exists());
        ensure!(!out.join("resource_lease.json").exists());
        for side in ["nightly-preflight", "head"] {
""",
)

replace_one(
    "src/main.rs",
    """        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct PostErrorReceipt {
""",
    """        Ok(())
    }

    #[test]
    fn worker_reconciliation_failure_removes_canonical_artifacts() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (root, base, head) = worker_repo(&temp)?;
        let out = temp.path().join("out");
        let request = request(
            "proof-request-reconcile-failure",
            &base,
            &head,
            ProofKind::FocusedTest,
            "cargo test --locked worker_test",
        );
        let mut prepare = prepare_worker_attempt;
        let mut publish = |out: &Path, lease: &ResourceLease, receipt: &ProofReceipt| {
            let mut mismatched = receipt.clone();
            mismatched.id = "different-worker-request".to_owned();
            publish_worker_artifacts(out, lease, &mismatched)
        };
        let mut calls = 0_usize;

        let error = run_worker_request_with_runner_and_boundaries(
            &args(&root, &out),
            request,
            &mut |_root, _argv, _env, _timeout, stdout, stderr, observe_process| {
                calls += 1;
                observe_process(CommandProcessObservation::Spawned);
                fs::write(stdout, b"ok\n")?;
                fs::write(stderr, b"")?;
                Ok(successful_status())
            },
            WorkerAttemptBoundaries {
                prepare: &mut prepare,
                publish: &mut publish,
            },
        )
        .err()
        .context("worker receipt reconciliation failure must propagate")?;

        let diagnostic = format!("{error:#}");
        ensure!(calls == 2);
        ensure!(diagnostic.contains("publish standalone worker proof result"));
        ensure!(diagnostic.contains("does not match request"));
        for name in [
            "proof_receipt.json",
            "resource_lease.json",
            "proof_receipt.json.tmp",
            "resource_lease.json.tmp",
        ] {
            ensure!(!out.join(name).exists(), "stale canonical worker artifact {name}");
        }
        for side in ["nightly-preflight", "head"] {
            let task = snapshot_task(
                &out,
                &proof_command_task_id("proof-request-reconcile-failure", side)?,
            )?;
            ensure!(task["state"] == serde_json::json!({"ResourcesReleased": "Succeeded"}));
            ensure!(
                task["receipt"]["CreationFailed"]["reason"]
                    .as_str()
                    .is_some_and(|reason| reason.contains("does not match request"))
            );
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
struct PostErrorReceipt {
""",
)

replace_one(
    "src/proof_task_ledger.rs",
    """    validate_worker_oid("base", &request.base)?;
    validate_worker_oid("head", &request.head)?;
    let diff = DiffContext::from_git(root, &request.base, &request.head)
""",
    """    validate_worker_oid("base", &request.base)?;
    validate_worker_oid("head", &request.head)?;
    validate_worker_checkout(root, &request.head)?;
    let diff = DiffContext::from_git(root, &request.base, &request.head)
""",
)

replace_one(
    "src/proof_task_ledger.rs",
    """fn validate_worker_oid(label: &str, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 40 || bytes.len() == 64,
        "standalone worker request {label} must be a 40- or 64-character object id"
    );
    ensure!(
        bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)),
        "standalone worker request {label} must be lowercase hexadecimal"
    );
    ensure!(
        bytes.iter().any(|byte| *byte != b'0'),
        "standalone worker request {label} cannot be the null object id"
    );
    Ok(())
}

""",
    """fn validate_worker_oid(label: &str, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    ensure!(
        bytes.len() == 40 || bytes.len() == 64,
        "standalone worker request {label} must be a 40- or 64-character object id"
    );
    ensure!(
        bytes
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte)),
        "standalone worker request {label} must be lowercase hexadecimal"
    );
    ensure!(
        bytes.iter().any(|byte| *byte != b'0'),
        "standalone worker request {label} cannot be the null object id"
    );
    Ok(())
}

fn validate_worker_checkout(root: &std::path::Path, expected_head: &str) -> Result<()> {
    let checkout_head = git_text(root, &["rev-parse", "--verify", "HEAD^{commit}"])
        .context("resolve standalone worker checkout HEAD")?;
    ensure!(
        checkout_head.trim() == expected_head,
        "standalone worker checkout HEAD {} does not match admitted head {expected_head}",
        checkout_head.trim()
    );
    let dirty = git_lines(
        root,
        &["status", "--porcelain=v1", "--untracked-files=all"],
    )
    .context("inspect standalone worker checkout cleanliness")?;
    ensure!(
        dirty.is_empty(),
        "standalone worker checkout must be clean before proof execution; observed {}",
        dirty.join("; ")
    );
    Ok(())
}

""",
)

replace_one(
    "src/proof/command.rs",
    """#[derive(Default)]
pub(crate) struct ProofBrokerResult {
    pub(crate) proof_receipts: Vec<ProofReceipt>,
    pub(crate) resource_leases: Vec<ResourceLease>,
}

""",
    """#[derive(Default)]
pub(crate) struct ProofBrokerResult {
    pub(crate) proof_receipts: Vec<ProofReceipt>,
    pub(crate) resource_leases: Vec<ResourceLease>,
}

fn run_physical_cleanup(cleanup: Option<&mut dyn FnMut() -> Result<()>>) -> Result<()> {
    match cleanup {
        Some(cleanup) => cleanup(),
        None => Ok(()),
    }
}

fn fail_after_physical_cleanup<T>(
    cleanup: Option<&mut dyn FnMut() -> Result<()>>,
    primary_error: anyhow::Error,
    cleanup_context: &str,
) -> Result<T> {
    match run_physical_cleanup(cleanup) {
        Ok(()) => Err(primary_error),
        Err(cleanup_error) => Err(cleanup_error).context(format!(
            "{cleanup_context}; original failure: {primary_error:#}"
        )),
    }
}

fn combine_proof_post_run_results(
    terminal_result: Result<()>,
    stream_result: Result<()>,
    cleanup_result: Result<()>,
) -> Result<()> {
    let mut failures = Vec::new();
    for (stage, result) in [
        ("TaskLedger terminal observation", terminal_result),
        ("proof command stream bounding", stream_result),
        ("proof command physical cleanup", cleanup_result),
    ] {
        if let Err(error) = result {
            failures.push((stage, error));
        }
    }
    if failures.is_empty() {
        return Ok(());
    }
    let (primary_stage, primary_error) = failures.remove(0);
    let additional = failures
        .into_iter()
        .map(|(stage, error)| format!("{stage}: {error:#}"))
        .collect::<Vec<_>>()
        .join("; ");
    if additional.is_empty() {
        Err(primary_error).context(primary_stage)
    } else {
        Err(primary_error).context(format!(
            "{primary_stage}; additional post-run failures: {additional}"
        ))
    }
}

""",
)

replace_one(
    "src/proof/command.rs",
    """            let cleanup_result = match cleanup {
                Some(cleanup) => cleanup(),
                None => Ok(()),
            };
""",
    """            let cleanup_result = run_physical_cleanup(cleanup);
""",
)

replace_one(
    "src/proof/command.rs",
    """    if let Some(error) = observation_error {
        return Err(error).context("record proof process observation");
    }
    if completion_unconfirmed {
        return match status {
            Err(error) => Err(error).context("proof child completion remains unconfirmed"),
            Ok(_) => bail!("proof child completion remains unconfirmed"),
        };
    }
    let disposition = match &status {
        Ok(status) if status.timed_out => TaskTerminalDisposition::TimedOut,
        Ok(status) if status.success => TaskTerminalDisposition::Succeeded,
        Ok(_) => TaskTerminalDisposition::DeterministicFailure,
        Err(_) => TaskTerminalDisposition::Cancelled,
    };
    if let Some((ledger, task)) = ledger_task {
        if process_spawned {
            ledger.process_finished(task, disposition)?;
        } else {
            ledger.setup_failed(task)?;
        }
    }
    let stream_result = bound_proof_command_streams(&paths);
    let cleanup_result = match cleanup {
        Some(cleanup) => cleanup(),
        None => Ok(()),
    };
    if process_spawned
        && stream_result.is_ok()
        && cleanup_result.is_ok()
        && let Some((ledger, task)) = ledger_task
    {
        ledger.cleanup_finished(task)?;
    }
    match (stream_result, cleanup_result) {
        (Ok(()), Ok(())) => {}
        (Err(stream_error), Ok(())) => return Err(stream_error),
        (Ok(()), Err(cleanup_error)) => return Err(cleanup_error),
        (Err(stream_error), Err(cleanup_error)) => {
            return Err(cleanup_error).context(format!(
                "proof command stream cleanup also failed: {stream_error:#}"
            ));
        }
    }
""",
    """    if let Some(error) = observation_error {
        return fail_after_physical_cleanup(
            cleanup,
            error.context("record proof process observation"),
            "physical cleanup after proof process observation failure",
        );
    }
    if completion_unconfirmed {
        let error = match status {
            Err(error) => error.context("proof child completion remains unconfirmed"),
            Ok(_) => anyhow::anyhow!("proof child completion remains unconfirmed"),
        };
        return fail_after_physical_cleanup(
            cleanup,
            error,
            "physical cleanup after unconfirmed proof child completion",
        );
    }
    let disposition = match &status {
        Ok(status) if status.timed_out => TaskTerminalDisposition::TimedOut,
        Ok(status) if status.success => TaskTerminalDisposition::Succeeded,
        Ok(_) => TaskTerminalDisposition::DeterministicFailure,
        Err(_) => TaskTerminalDisposition::Cancelled,
    };
    let terminal_result = match ledger_task {
        Some((ledger, task)) if process_spawned => ledger.process_finished(task, disposition),
        Some((ledger, task)) => ledger.setup_failed(task),
        None => Ok(()),
    };
    let stream_result = bound_proof_command_streams(&paths);
    let cleanup_result = run_physical_cleanup(cleanup);
    combine_proof_post_run_results(terminal_result, stream_result, cleanup_result)?;
    if process_spawned && let Some((ledger, task)) = ledger_task {
        ledger.cleanup_finished(task)?;
    }
""",
)

replace_one(
    "src/proof/command.rs",
    """        let lease = granted_lease("lease-proof-command-unconfirmed");

        let error = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: &focused.id,
                side: "head",
                spec: &spec,
                timeout_sec: 7,
                lease: &lease,
                task_ledger: Some(&ledger),
                task: Some(&command_task),
                cleanup: None,
            },
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, observe_process| {
                observe_process(CommandProcessObservation::Spawned);
                observe_process(CommandProcessObservation::CompletionUnconfirmed);
                Err(anyhow::anyhow!("injected unconfirmed cleanup"))
            },
        )
        .err()
        .context("unconfirmed child must fail closed")?;
        ensure!(format!("{error:#}").contains("completion remains unconfirmed"));
""",
    """        let lease = granted_lease("lease-proof-command-unconfirmed");
        let mut physical_cleanup_called = false;
        let mut cleanup = || -> Result<()> {
            physical_cleanup_called = true;
            anyhow::bail!("injected physical cleanup failure")
        };

        let error = run_proof_command_receipt(
            ProofCommandInvocation {
                command_root: temp.path(),
                out: &out,
                receipt_id: &focused.id,
                side: "head",
                spec: &spec,
                timeout_sec: 7,
                lease: &lease,
                task_ledger: Some(&ledger),
                task: Some(&command_task),
                cleanup: Some(&mut cleanup),
            },
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, observe_process| {
                observe_process(CommandProcessObservation::Spawned);
                observe_process(CommandProcessObservation::CompletionUnconfirmed);
                Err(anyhow::anyhow!("injected unconfirmed child failure"))
            },
        )
        .err()
        .context("unconfirmed child must fail closed")?;
        let diagnostic = format!("{error:#}");
        ensure!(physical_cleanup_called);
        ensure!(diagnostic.contains("completion remains unconfirmed"));
        ensure!(diagnostic.contains("injected unconfirmed child failure"));
        ensure!(diagnostic.contains("injected physical cleanup failure"));
""",
)

replace_one(
    "src/proof/command.rs",
    """    #[test]
    fn spawned_runner_error_is_cancelled_and_serialized_as_skipped() -> Result<()> {
""",
    """    #[test]
    fn early_proof_observation_failure_runs_cleanup_and_retains_both_errors() -> Result<()> {
        let mut cleanup_called = false;
        let mut cleanup = || -> Result<()> {
            cleanup_called = true;
            anyhow::bail!("injected cleanup failure")
        };

        let result: Result<()> = fail_after_physical_cleanup(
            Some(&mut cleanup),
            anyhow::anyhow!("injected process observation failure"),
            "physical cleanup after proof process observation failure",
        );

        let error = result
            .err()
            .context("combined observation and cleanup failure must propagate")?;
        let diagnostic = format!("{error:#}");
        ensure!(cleanup_called);
        ensure!(diagnostic.contains("injected process observation failure"));
        ensure!(diagnostic.contains("injected cleanup failure"));
        Ok(())
    }

    #[test]
    fn spawned_runner_error_is_cancelled_and_serialized_as_skipped() -> Result<()> {
""",
)
