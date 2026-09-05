//! Focused proof execution: HEAD-only and base+tests red/green command
//! receipts under the broker runtime budget.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::task_ledger::TaskNonExecutionDisposition;
use crate::*;

#[expect(
    clippy::too_many_arguments,
    reason = "tracked in policy/allow.toml#clippy-too-many-arguments-artifact-writers"
)]
pub(crate) fn run_focused_red_green_proof_tasks_with_runner<F, G>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    profile: &Profile,
    args: &RunArgs,
    budget: ProofBudget,
    tasks: Vec<FocusedTestTask>,
    task_ledger: Option<&ProofTaskLedger>,
    execution_phase: ProofExecutionPhase,
    mut runner: F,
    mut prepare_base_plus_tests: G,
) -> Result<ProofBrokerResult>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
        &mut dyn FnMut(CommandProcessObservation),
    ) -> Result<CommandStatus>,
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
{
    let mut receipts = Vec::new();
    let mut leases = Vec::new();
    let mut executed_tasks = 0_usize;
    let mut executed_files = BTreeSet::new();
    let mut estimated_seconds = 0_u64;
    let lease_budget = proof_lease_budget(profile)?;
    for task in tasks {
        let task_timeout_sec = focused_test_task_command_timeout(&task, budget);
        if args.dry_run {
            leases.push(focused_test_resource_lease(
                &task,
                budget,
                lease_budget,
                "skipped_profile",
                "dry-run; resource broker did not grant a proof lease",
            ));
            if let Some(ledger) = task_ledger {
                ledger.decline_command(
                    &ProofCommandTask::focused_test(
                        &task,
                        "head",
                        task_timeout_sec,
                        execution_phase,
                    ),
                    TaskNonExecutionDisposition::Refused,
                    "dry-run; proof broker did not execute focused tests",
                )?;
            }
            receipts.push(skipped_focused_proof_receipt(
                out,
                diff,
                &task,
                "skipped_profile",
                "dry-run; proof broker did not execute focused tests",
            )?);
            continue;
        }
        if profile.limits.tests == 0 {
            leases.push(focused_test_resource_lease(
                &task,
                budget,
                lease_budget,
                "absent",
                "profile allows zero focused test leases",
            ));
            if let Some(ledger) = task_ledger {
                ledger.decline_command(
                    &ProofCommandTask::focused_test(
                        &task,
                        "head",
                        task_timeout_sec,
                        execution_phase,
                    ),
                    TaskNonExecutionDisposition::Refused,
                    "profile allows zero focused test leases",
                )?;
            }
            receipts.push(skipped_focused_proof_receipt(
                out,
                diff,
                &task,
                "skipped_profile",
                "profile allows zero focused test leases",
            )?);
            continue;
        }
        if !focused_proof_budget_allows_next(
            executed_tasks,
            &executed_files,
            &task.file,
            estimated_seconds,
            task_timeout_sec,
            task.mode.command_count(),
            budget,
        ) {
            leases.push(focused_test_resource_lease(
                &task,
                budget,
                lease_budget,
                "exhausted",
                "focused red/green proof lease budget exhausted by runtime profile",
            ));
            if let Some(ledger) = task_ledger {
                ledger.decline_command(
                    &ProofCommandTask::focused_test(
                        &task,
                        "head",
                        task_timeout_sec,
                        execution_phase,
                    ),
                    TaskNonExecutionDisposition::BudgetDeferred,
                    "focused red/green proof lease budget exhausted by runtime profile",
                )?;
            }
            receipts.push(skipped_focused_proof_receipt(
                out,
                diff,
                &task,
                "skipped_budget",
                "focused red/green proof lease budget exhausted by runtime profile",
            )?);
            continue;
        }
        executed_files.insert(task.file.clone());
        let lease = focused_test_resource_lease(
            &task,
            budget,
            lease_budget,
            "granted",
            "focused red/green proof lease granted by runtime profile",
        );
        let receipt = match task.mode {
            FocusedProofMode::HeadOnly => {
                let head_spec = proof_task_command_spec(&task, "head");
                let head = run_proof_command_receipt_for_task(
                    root,
                    out,
                    &task,
                    "head",
                    &head_spec,
                    task_timeout_sec,
                    &lease,
                    task_ledger,
                    execution_phase,
                    &mut runner,
                )?;
                let result = match head.status.as_str() {
                    "passed" => "head_passed",
                    "failed" => "head_failed",
                    "timed_out" => "timed_out",
                    _ => "skipped_profile",
                };
                let reason = format!("HEAD proof {}: {}", head.status, head.reason);
                focused_head_receipt(diff, &task, vec![head], result.to_owned(), reason)
            }
            FocusedProofMode::RedGreen => run_focused_red_green_proof_task(
                root,
                out,
                diff,
                &task,
                task_timeout_sec,
                &lease,
                task_ledger,
                execution_phase,
                &mut runner,
                &mut prepare_base_plus_tests,
            )?,
        };
        leases.push(lease);
        receipts.push(receipt);
        executed_tasks += 1;
        estimated_seconds = estimated_seconds
            .saturating_add(task_timeout_sec.saturating_mul(task.mode.command_count()));
    }
    Ok(ProofBrokerResult {
        proof_receipts: receipts,
        resource_leases: leases,
    })
}

#[expect(
    clippy::too_many_arguments,
    reason = "lease is an execution precondition for the proof command; keeping it explicit pins the no-lease-no-command broker contract"
)]
pub(crate) fn run_focused_red_green_proof_task<F, G>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    task: &FocusedTestTask,
    timeout_sec: u64,
    lease: &ResourceLease,
    task_ledger: Option<&ProofTaskLedger>,
    execution_phase: ProofExecutionPhase,
    runner: &mut F,
    prepare_base_plus_tests: &mut G,
) -> Result<ProofReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
        &mut dyn FnMut(CommandProcessObservation),
    ) -> Result<CommandStatus>,
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
{
    run_focused_red_green_proof_task_with_cleanup(
        root,
        out,
        diff,
        task,
        timeout_sec,
        lease,
        task_ledger,
        execution_phase,
        runner,
        prepare_base_plus_tests,
        &mut cleanup_base_plus_tests_worktree,
    )
}

#[expect(
    clippy::too_many_arguments,
    reason = "lease is an execution precondition for the proof command; keeping it explicit pins the no-lease-no-command broker contract"
)]
fn run_focused_red_green_proof_task_with_cleanup<F, G, H>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    task: &FocusedTestTask,
    timeout_sec: u64,
    lease: &ResourceLease,
    task_ledger: Option<&ProofTaskLedger>,
    execution_phase: ProofExecutionPhase,
    runner: &mut F,
    prepare_base_plus_tests: &mut G,
    cleanup_base_plus_tests: &mut H,
) -> Result<ProofReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
        &mut dyn FnMut(CommandProcessObservation),
    ) -> Result<CommandStatus>,
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
    H: FnMut(&Path, &Path) -> Result<()>,
{
    let head_spec = proof_task_command_spec(task, "head");
    let head = run_proof_command_receipt_for_task(
        root,
        out,
        task,
        "head",
        &head_spec,
        timeout_sec,
        lease,
        task_ledger,
        execution_phase,
        runner,
    )?;
    let head_status = head.status.clone();
    if head_status != "passed" {
        let result = match head_status.as_str() {
            "timed_out" => "timed_out",
            "failed" => "head_failed",
            _ => "skipped_profile",
        };
        let reason = format!("HEAD proof {}: {}", head.status, head.reason);
        return Ok(focused_red_green_receipt(
            diff,
            task,
            vec![head],
            result.to_owned(),
            reason,
        ));
    }

    let base_root = match prepare_base_plus_tests(root, out, diff) {
        Ok(path) => path,
        Err(error) => {
            let mut commands = vec![head];
            let base_spec = proof_task_command_spec(task, "base-plus-tests");
            let patch_reason = format!("base+tests patch failed: {error:#}");
            if let Some(ledger) = task_ledger {
                ledger.decline_command(
                    &ProofCommandTask::focused_test(
                        task,
                        "base-plus-tests",
                        timeout_sec,
                        execution_phase,
                    ),
                    TaskNonExecutionDisposition::Refused,
                    &patch_reason,
                )?;
            }
            commands.push(skipped_proof_command_receipt(
                out,
                task,
                "base-plus-tests",
                &base_spec,
                "skipped",
                patch_reason.clone(),
            )?);
            return Ok(focused_red_green_receipt(
                diff,
                task,
                commands,
                "base_patch_failed".to_owned(),
                patch_reason,
            ));
        }
    };
    let base_spec = proof_task_command_spec(task, "base-plus-tests");
    let mut cleanup = || cleanup_base_plus_tests(root, &base_root);
    let base = run_proof_command_receipt_for_task_with_cleanup(
        &base_root,
        out,
        task,
        "base-plus-tests",
        &base_spec,
        timeout_sec,
        lease,
        task_ledger,
        execution_phase,
        Some(&mut cleanup),
        runner,
    )?;
    let (result, reason) = match base.status.as_str() {
        "failed" => (
            "discriminating".to_owned(),
            format!("HEAD passed; base+tests failed: {}", base.reason),
        ),
        "passed" => (
            "non_discriminating".to_owned(),
            "HEAD and base+tests both passed".to_owned(),
        ),
        "timed_out" => (
            "timed_out".to_owned(),
            format!("base+tests timed out: {}", base.reason),
        ),
        _ => (
            "skipped_profile".to_owned(),
            format!("base+tests proof unavailable: {}", base.reason),
        ),
    };
    Ok(focused_red_green_receipt(
        diff,
        task,
        vec![head, base],
        result,
        reason,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task_ledger::TaskEvent;
    use crate::tests::{run_test_command, test_diff, test_run_args};
    use anyhow::ensure;
    use std::fs;
    use std::time::Instant;

    /// A change in the shape this repository itself has: the production fix and
    /// the new unit test live in the same `src/*.rs` file, the test inside a
    /// `#[cfg(test)] mod tests` block.
    const BASE_SOURCE: &str = "pub fn classify(value: u8) -> &'static str {\n    if value > 10 {\n        \"high\"\n    } else {\n        \"low\"\n    }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn classifies_high() {\n        assert_eq!(classify(11), \"high\");\n    }\n}\n";
    const HEAD_SOURCE: &str = "pub fn classify(value: u8) -> &'static str {\n    if value >= 10 {\n        \"high\"\n    } else {\n        \"low\"\n    }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn classifies_high() {\n        assert_eq!(classify(11), \"high\");\n    }\n\n    #[test]\n    fn classifies_boundary() {\n        assert_eq!(classify(10), \"high\");\n    }\n}\n";

    fn same_file_fix_and_test_repo(root: &Path) -> Result<DiffContext> {
        run_test_command(root, "git", &["init", "--initial-branch=main"])?;
        run_test_command(
            root,
            "git",
            &["config", "user.email", "ub-review@example.invalid"],
        )?;
        run_test_command(root, "git", &["config", "user.name", "UB Review Test"])?;
        fs::create_dir_all(root.join("src"))?;
        fs::write(root.join("src/lib.rs"), BASE_SOURCE)?;
        run_test_command(root, "git", &["add", "-A"])?;
        run_test_command(
            root,
            "git",
            &["-c", "commit.gpgsign=false", "commit", "-m", "base"],
        )?;
        let base = git_text_owned(root, &["rev-parse".to_owned(), "HEAD".to_owned()])?
            .trim()
            .to_owned();
        fs::write(root.join("src/lib.rs"), HEAD_SOURCE)?;
        run_test_command(root, "git", &["add", "-A"])?;
        run_test_command(
            root,
            "git",
            &["-c", "commit.gpgsign=false", "commit", "-m", "head"],
        )?;
        let head = git_text_owned(root, &["rev-parse".to_owned(), "HEAD".to_owned()])?
            .trim()
            .to_owned();
        Ok(DiffContext {
            base,
            head,
            changed_files: vec!["src/lib.rs".to_owned()],
            patch: String::new(),
            flags: DiffFlags {
                source_changed: true,
                rust_changed: true,
                rust_tests_changed: true,
                ..DiffFlags::default()
            },
            diff_class: DiffClass::SourceUb,
        })
    }

    fn cargo_test_task() -> FocusedTestTask {
        let spec = ProofCommandSpec {
            argv: vec![
                "cargo".to_owned(),
                "test".to_owned(),
                "--locked".to_owned(),
                "classifies_boundary".to_owned(),
            ],
            env: BTreeMap::new(),
        };
        FocusedTestTask {
            id: "proof-rust-001".to_owned(),
            file: "src/lib.rs".to_owned(),
            test_name: Some("classifies_boundary".to_owned()),
            mode: FocusedProofMode::RedGreen,
            command_specs: Some(FocusedTestCommandSpecs {
                head: spec.clone(),
                base_plus_tests: spec,
            }),
            timeout_sec: Some(60),
            required: false,
            requested_by: vec!["tests".to_owned()],
            request_ids: vec!["proof-rust-001".to_owned()],
        }
    }

    /// A Rust unit test that lives beside the fix it pins must come out
    /// `discriminating`. Selecting whole files by path prefix could only either
    /// carry the fix into the base run or leave the new test out of it, and both
    /// spellings report a false `non_discriminating`.
    #[test]
    fn same_file_rust_unit_test_discriminates_against_its_own_fix() -> Result<()> {
        let repo = tempfile::tempdir()?;
        let diff = same_file_fix_and_test_repo(repo.path())?;
        let out = repo.path().join("out");
        let args = test_run_args(out.clone());
        let mut observed = Vec::<(bool, bool)>::new();

        let result = run_focused_red_green_proof_tasks_with_runner(
            repo.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 2,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            vec![cargo_test_task()],
            None,
            ProofExecutionPhase::ModelRequest,
            |root, _argv, _env, _timeout, stdout, stderr, _observe_process| {
                // Stand in for `cargo test classifies_boundary`: the new test
                // only passes where the production fix is present.
                let source = fs::read_to_string(root.join("src/lib.rs"))?;
                let has_test = source.contains("fn classifies_boundary");
                let has_fix = source.contains("value >= 10");
                observed.push((has_test, has_fix));
                let success = !has_test || has_fix;
                fs::write(
                    stdout,
                    if success {
                        b"ok\n".as_slice()
                    } else {
                        b"FAILED\n".as_slice()
                    },
                )?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(i32::from(!success)),
                    timed_out: false,
                    success,
                    reason: "completed".to_owned(),
                    duration_ms: 7,
                })
            },
            prepare_base_plus_tests_worktree,
        )?;

        // HEAD carries fix and test; base+tests carries the test alone.
        assert_eq!(observed, vec![(true, true), (true, false)]);
        let receipt = result
            .proof_receipts
            .first()
            .ok_or_else(|| anyhow::anyhow!("expected one proof receipt"))?;
        assert_eq!(receipt.result, "discriminating", "{receipt:?}");
        assert_eq!(receipt.test_patch_mode, "base-plus-tests");
        let patch = fs::read_to_string(out.join("proof/base-plus-tests.patch"))?;
        assert!(patch.contains("fn classifies_boundary"), "{patch}");
        assert!(!patch.contains("value >= 10"), "{patch}");
        Ok(())
    }

    /// The refusal path: an unbuildable base+tests patch must be reported as
    /// `base_patch_failed`, never as a verdict about the tests.
    #[test]
    fn unbuildable_base_plus_tests_patch_reports_base_patch_failed() -> Result<()> {
        let repo = tempfile::tempdir()?;
        let diff = same_file_fix_and_test_repo(repo.path())?;
        let out = repo.path().join("out");
        let args = test_run_args(out.clone());

        let result = run_focused_red_green_proof_tasks_with_runner(
            repo.path(),
            &out,
            &diff,
            &Profile::default(),
            &args,
            ProofBudget {
                max_focused_test_files: 2,
                max_focused_tests: 2,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
            vec![cargo_test_task()],
            None,
            ProofExecutionPhase::ModelRequest,
            |_root, _argv, _env, _timeout, stdout, stderr, _observe_process| {
                fs::write(stdout, b"ok\n")?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(0),
                    timed_out: false,
                    success: true,
                    reason: "completed".to_owned(),
                    duration_ms: 7,
                })
            },
            |_root, _out, _diff| anyhow::bail!("classification refused"),
        )?;

        let receipt = result
            .proof_receipts
            .first()
            .ok_or_else(|| anyhow::anyhow!("expected one proof receipt"))?;
        assert_eq!(receipt.result, "base_patch_failed", "{receipt:?}");
        assert!(
            receipt.reason.contains("classification refused"),
            "{receipt:?}"
        );
        Ok(())
    }

    #[test]
    fn base_worktree_cleanup_precedes_ledger_cleanup_completion() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let task = cargo_test_task();
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let recorder = TaskLedgerRecorder::new(&revision, &Instant::now())?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let lease = ResourceLease {
            revision: Some(revision),
            schema: RESOURCE_LEASE_SCHEMA.to_owned(),
            id: format!("lease-{}", task.id),
            kind: "focused-test".to_owned(),
            consumer: task.id.clone(),
            status: "granted".to_owned(),
            reason: "fixture lease".to_owned(),
            cpu: 1,
            memory_mb: 512,
            disk_mb: 64,
            timeout_sec: 60,
            network: false,
            scratch: false,
            worktree: Some("fixture-worktree".to_owned()),
            command: None,
        };
        let base_root = temp.path().join("base-plus-tests");
        let prepared_base_root = base_root.clone();
        let mut cleanup_observed = false;

        let receipt = run_focused_red_green_proof_task_with_cleanup(
            temp.path(),
            &out,
            &diff,
            &task,
            60,
            &lease,
            Some(&ledger),
            ProofExecutionPhase::ModelRequest,
            &mut |_root, _argv, _env, _timeout, stdout, stderr, observe_process| {
                observe_process(CommandProcessObservation::Spawned);
                fs::write(stdout, b"ok\n")?;
                fs::write(stderr, b"")?;
                Ok(CommandStatus {
                    exit_code: Some(0),
                    timed_out: false,
                    success: true,
                    reason: "completed".to_owned(),
                    duration_ms: 1,
                })
            },
            &mut move |_root, _out, _diff| {
                fs::create_dir_all(&prepared_base_root)?;
                Ok(prepared_base_root.clone())
            },
            &mut |_root, worktree| {
                ensure!(worktree.is_dir());
                let base_task_id = proof_command_task_id(&task.id, "base-plus-tests")?;
                let events = recorder
                    .inputs()?
                    .into_iter()
                    .filter(|input| input.task_id == base_task_id)
                    .map(|input| input.event)
                    .collect::<Vec<_>>();
                ensure!(
                    events
                        .iter()
                        .any(|event| matches!(event, TaskEvent::ProcessFinished { .. }))
                );
                ensure!(!events.iter().any(|event| matches!(
                    event,
                    TaskEvent::CleanupFinished { .. } | TaskEvent::ResourcesReleased { .. }
                )));
                fs::remove_dir_all(worktree)?;
                cleanup_observed = true;
                Ok(())
            },
        )?;

        ensure!(receipt.commands.len() == 2);
        ensure!(cleanup_observed);
        ensure!(!base_root.exists());
        let base_task_id = proof_command_task_id(&task.id, "base-plus-tests")?;
        let events = recorder
            .inputs()?
            .into_iter()
            .filter(|input| input.task_id == base_task_id)
            .map(|input| input.event)
            .collect::<Vec<_>>();
        ensure!(
            events
                .iter()
                .any(|event| matches!(event, TaskEvent::CleanupFinished { .. }))
        );
        ensure!(
            !events
                .iter()
                .any(|event| matches!(event, TaskEvent::ResourcesReleased { .. }))
        );
        Ok(())
    }

    #[test]
    fn unconfirmed_head_completion_prevents_base_plus_tests_launch() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let out = temp.path().join("out");
        let diff = test_diff();
        let task = cargo_test_task();
        let revision = RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        };
        let recorder = TaskLedgerRecorder::new(&revision, &Instant::now())?;
        let ledger = ProofTaskLedger::new(recorder.clone());
        let lease = ResourceLease {
            revision: None,
            schema: RESOURCE_LEASE_SCHEMA.to_owned(),
            id: format!("lease-{}", task.id),
            kind: "focused-test".to_owned(),
            consumer: task.id.clone(),
            status: "granted".to_owned(),
            reason: "fixture lease".to_owned(),
            cpu: 1,
            memory_mb: 512,
            disk_mb: 64,
            timeout_sec: 60,
            network: false,
            scratch: false,
            worktree: Some("fixture-worktree".to_owned()),
            command: None,
        };
        let mut command_calls = 0_usize;
        let mut prepare_called = false;

        let error = run_focused_red_green_proof_task(
            temp.path(),
            &out,
            &diff,
            &task,
            60,
            &lease,
            Some(&ledger),
            ProofExecutionPhase::ModelRequest,
            &mut |_root, _argv, _env, _timeout, _stdout, _stderr, observe_process| {
                command_calls += 1;
                observe_process(CommandProcessObservation::Spawned);
                observe_process(CommandProcessObservation::CompletionUnconfirmed);
                Err(anyhow::anyhow!("injected unconfirmed head cleanup"))
            },
            &mut |_root, _out, _diff| {
                prepare_called = true;
                Ok(temp.path().join("base-plus-tests"))
            },
        )
        .err()
        .context("unconfirmed head must fail the proof task")?;

        ensure!(format!("{error:#}").contains("completion remains unconfirmed"));
        ensure!(command_calls == 1);
        ensure!(!prepare_called);
        let inputs = recorder.inputs()?;
        ensure!(!inputs.iter().any(|input| {
            input.task_id.as_str().contains("base-plus-tests")
                || matches!(input.event, TaskEvent::ProcessFinished { .. })
                || matches!(input.event, TaskEvent::ResourcesReleased { .. })
        }));
        Ok(())
    }
}
