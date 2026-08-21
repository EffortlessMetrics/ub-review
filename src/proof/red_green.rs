//! Focused proof execution: HEAD-only and base+tests red/green command
//! receipts under the broker runtime budget.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;

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
            FocusedProofMode::HeadOnly => run_focused_head_proof_task(
                root,
                out,
                diff,
                &task,
                task_timeout_sec,
                &lease,
                &mut runner,
            )?,
            FocusedProofMode::RedGreen => run_focused_red_green_proof_task(
                root,
                out,
                diff,
                &task,
                task_timeout_sec,
                &lease,
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

fn run_focused_head_proof_task<F>(
    root: &Path,
    out: &Path,
    diff: &DiffContext,
    task: &FocusedTestTask,
    timeout_sec: u64,
    lease: &ResourceLease,
    runner: &mut F,
) -> Result<ProofReceipt>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
    ) -> Result<CommandStatus>,
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
        runner,
    )?;
    let result = match head.status.as_str() {
        "passed" => "head_passed",
        "failed" => "head_failed",
        "timed_out" => "timed_out",
        _ => "skipped_profile",
    };
    let reason = format!("HEAD proof {}: {}", head.status, head.reason);
    Ok(focused_head_receipt(
        diff,
        task,
        vec![head],
        result.to_owned(),
        reason,
    ))
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
    ) -> Result<CommandStatus>,
    G: FnMut(&Path, &Path, &DiffContext) -> Result<PathBuf>,
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
    let base = {
        let result = run_proof_command_receipt_for_task(
            &base_root,
            out,
            task,
            "base-plus-tests",
            &base_spec,
            timeout_sec,
            lease,
            runner,
        );
        let _ = cleanup_base_plus_tests_worktree(root, &base_root);
        result?
    };
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
    use crate::tests::{run_test_command, test_run_args};
    use std::fs;

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
            |root, _argv, _env, _timeout, stdout, stderr| {
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
            |_root, _argv, _env, _timeout, stdout, stderr| {
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
}
