//! Runtime proof execution and lease budgets.

use anyhow::{Result, bail};

use crate::*;

pub(crate) fn proof_budget(profile: &Profile) -> Result<ProofBudget> {
    let budget = ProofBudget {
        max_focused_test_files: profile.budgets.proof_max_focused_test_files,
        max_focused_tests: profile.budgets.proof_max_focused_tests,
        per_command_timeout_sec: profile.budgets.proof_command_timeout_sec,
        max_total_seconds: profile.budgets.proof_total_timeout_sec,
    };
    if budget.max_focused_tests > 0 && budget.per_command_timeout_sec == 0 {
        bail!(
            "runtime profile {} has proof_command_timeout_sec=0 with focused proof enabled",
            profile.name
        );
    }
    if budget.max_focused_tests > 0 && budget.max_total_seconds == 0 {
        bail!(
            "runtime profile {} has proof_total_timeout_sec=0 with focused proof enabled",
            profile.name
        );
    }
    Ok(budget)
}

pub(crate) fn proof_lease_budget(profile: &Profile) -> Result<ProofLeaseBudget> {
    let budget = ProofLeaseBudget {
        cpu: profile.budgets.proof_cpu,
        memory_mb: profile.budgets.proof_memory_mb,
        disk_mb: profile.budgets.proof_disk_mb,
        network: profile.budgets.proof_network,
        scratch: profile.budgets.proof_scratch,
    };
    if profile.limits.tests > 0 && profile.budgets.proof_max_focused_tests > 0 {
        if budget.cpu == 0 {
            bail!(
                "runtime profile {} has proof_cpu=0 with focused proof enabled",
                profile.name
            );
        }
        if budget.memory_mb == 0 {
            bail!(
                "runtime profile {} has proof_memory_mb=0 with focused proof enabled",
                profile.name
            );
        }
        if budget.disk_mb == 0 {
            bail!(
                "runtime profile {} has proof_disk_mb=0 with focused proof enabled",
                profile.name
            );
        }
    }
    Ok(budget)
}

pub(crate) fn remaining_focused_proof_budget(
    mut budget: ProofBudget,
    existing_leases: &[ResourceLease],
) -> ProofBudget {
    let focused_leases = existing_leases
        .iter()
        .filter(|lease| focused_proof_lease_counts_budget(&lease.kind))
        .collect::<Vec<_>>();
    if focused_leases
        .iter()
        .any(|lease| focused_proof_lease_blocks_budget(&lease.status))
    {
        budget.max_focused_test_files = 0;
        budget.max_focused_tests = 0;
        budget.max_total_seconds = 0;
        return budget;
    }

    let granted = focused_leases
        .iter()
        .filter(|lease| lease.status == "granted")
        .count();
    let granted_seconds = focused_leases
        .iter()
        .filter(|lease| lease.status == "granted")
        .map(|lease| focused_proof_lease_charged_seconds(lease))
        .fold(0_u64, u64::saturating_add);
    budget.max_focused_tests = budget.max_focused_tests.saturating_sub(granted);
    budget.max_focused_test_files = budget.max_focused_test_files.saturating_sub(granted);
    budget.max_total_seconds = budget.max_total_seconds.saturating_sub(granted_seconds);
    budget
}

/// Charge terminal elapsed, or keep the admission reservation when completion
/// is unknown. Do not refund task slots, cap overruns, or invent zero-cost work.
pub(crate) fn focused_proof_lease_charged_seconds(lease: &ResourceLease) -> u64 {
    match lease.settled_elapsed_ms {
        Some(millis) => u64::try_from(millis.div_ceil(1000))
            .unwrap_or(u64::MAX)
            .max(1),
        None => lease.timeout_sec,
    }
}

/// The native sequential runners call this only after their receipt constructor
/// returns. Failed setup/unavailable commands are not confirmed completion and
/// cannot refund their reservation. Command/cleanup errors abort before this call.
pub(crate) fn settle_focused_proof_lease(
    lease: &mut ResourceLease,
    receipt: &ProofReceipt,
    task_elapsed_ms: u128,
) {
    if lease.status != "granted"
        || lease.consumer != receipt.id
        || lease.revision != receipt.revision
        || !matches!(
            receipt.result.as_str(),
            "head_passed" | "head_failed" | "timed_out" | "discriminating" | "non_discriminating"
        )
        || !matches!(
            (lease.kind.as_str(), receipt.kind.as_str()),
            ("focused-build", "focused-build")
                | ("focused-test", "focused-head" | "focused-red-green")
        )
        || !focused_proof_lease_counts_budget(&lease.kind)
        || receipt.commands.is_empty()
        || receipt
            .commands
            .iter()
            .any(|command| !match command.status.as_str() {
                "passed" => !command.timed_out && command.exit_code == Some(0),
                "failed" => !command.timed_out && command.exit_code.is_some_and(|code| code != 0),
                "timed_out" => command.timed_out,
                _ => false,
            })
    {
        return;
    }
    // The task clock includes setup/cleanup. The command sum is a conservative
    // floor, including when tests inject a runner's measured duration.
    let command_ms = receipt
        .commands
        .iter()
        .map(|command| command.duration_ms)
        .fold(0_u128, u128::saturating_add);
    lease.settled_elapsed_ms = Some(task_elapsed_ms.max(command_ms));
}

fn focused_proof_lease_counts_budget(kind: &str) -> bool {
    matches!(kind, "focused-test" | "focused-build")
}

fn focused_proof_lease_blocks_budget(status: &str) -> bool {
    matches!(status, "exhausted" | "absent")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_budget() -> ProofBudget {
        ProofBudget {
            max_focused_test_files: 3,
            max_focused_tests: 4,
            per_command_timeout_sec: 300,
            max_total_seconds: 600,
        }
    }

    fn test_lease(kind: &str, status: &str, timeout_sec: u64) -> ResourceLease {
        ResourceLease {
            settled_elapsed_ms: None,
            revision: None,
            schema: "ub-review.resource_lease.v1".to_owned(),
            id: format!("lease-{kind}-{status}-{timeout_sec}"),
            kind: kind.to_owned(),
            consumer: "proof-test".to_owned(),
            status: status.to_owned(),
            reason: "test lease".to_owned(),
            cpu: 1,
            memory_mb: 1,
            disk_mb: 1,
            timeout_sec,
            network: false,
            scratch: true,
            worktree: None,
            command: None,
        }
    }

    #[test]
    fn proof_budget_comes_from_runtime_profile_budgets() -> Result<()> {
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;
        let cx23 = profiles
            .iter()
            .find(|profile| profile.name == "cx23")
            .ok_or_else(|| anyhow::anyhow!("missing cx23 profile"))?;
        let cx43 = profiles
            .iter()
            .find(|profile| profile.name == "cx43")
            .ok_or_else(|| anyhow::anyhow!("missing cx43 profile"))?;

        assert_eq!(proof_budget(gh_runner)?.max_focused_tests, 1);
        assert_eq!(proof_budget(cx23)?.max_focused_tests, 2);
        assert_eq!(proof_budget(cx43)?.max_focused_tests, 6);
        assert_eq!(proof_budget(cx43)?.per_command_timeout_sec, 600);
        assert_eq!(proof_budget(cx43)?.max_total_seconds, 1_800);
        Ok(())
    }

    #[test]
    fn invalid_enabled_proof_budget_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_command_timeout_sec: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let err = proof_budget(&profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid proof budget unexpectedly passed"))?;

        assert_eq!(
            err.to_string(),
            "runtime profile broken has proof_command_timeout_sec=0 with focused proof enabled"
        );
        Ok(())
    }

    #[test]
    fn disabled_focused_proof_allows_zero_command_budget() -> Result<()> {
        let profile = Profile {
            name: "disabled".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 0,
                proof_command_timeout_sec: 0,
                proof_total_timeout_sec: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let budget = proof_budget(&profile)?;

        assert_eq!(budget.max_focused_tests, 0);
        assert_eq!(budget.per_command_timeout_sec, 0);
        assert_eq!(budget.max_total_seconds, 0);
        Ok(())
    }

    #[test]
    fn invalid_enabled_proof_total_budget_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_command_timeout_sec: 300,
                proof_total_timeout_sec: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let err = proof_budget(&profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid proof budget unexpectedly passed"))?;

        assert_eq!(
            err.to_string(),
            "runtime profile broken has proof_total_timeout_sec=0 with focused proof enabled"
        );
        Ok(())
    }

    #[test]
    fn proof_lease_budget_comes_from_runtime_profile_budgets() -> Result<()> {
        let profiles = builtin_profiles();
        let gh_runner = profiles
            .iter()
            .find(|profile| profile.name == "gh-runner")
            .ok_or_else(|| anyhow::anyhow!("missing gh-runner profile"))?;
        let cx23 = profiles
            .iter()
            .find(|profile| profile.name == "cx23")
            .ok_or_else(|| anyhow::anyhow!("missing cx23 profile"))?;
        let cx43 = profiles
            .iter()
            .find(|profile| profile.name == "cx43")
            .ok_or_else(|| anyhow::anyhow!("missing cx43 profile"))?;

        assert_eq!(proof_lease_budget(gh_runner)?.cpu, 2);
        assert_eq!(proof_lease_budget(gh_runner)?.memory_mb, 2_048);
        assert_eq!(proof_lease_budget(gh_runner)?.disk_mb, 1_024);
        assert_eq!(proof_lease_budget(cx23)?.cpu, 1);
        assert_eq!(proof_lease_budget(cx23)?.memory_mb, 1_024);
        assert_eq!(proof_lease_budget(cx43)?.cpu, 4);
        assert_eq!(proof_lease_budget(cx43)?.disk_mb, 2_048);
        Ok(())
    }

    #[test]
    fn invalid_enabled_proof_lease_budget_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_cpu: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let err = proof_lease_budget(&profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid proof lease budget unexpectedly passed"))?;

        assert_eq!(
            err.to_string(),
            "runtime profile broken has proof_cpu=0 with focused proof enabled"
        );
        Ok(())
    }

    #[test]
    fn disabled_test_leases_allow_zero_proof_resources() -> Result<()> {
        let profile = Profile {
            name: "disabled".to_owned(),
            limits: Limits {
                tests: 0,
                ..Limits::default()
            },
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_cpu: 0,
                proof_memory_mb: 0,
                proof_disk_mb: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let budget = proof_lease_budget(&profile)?;

        assert_eq!(budget.cpu, 0);
        assert_eq!(budget.memory_mb, 0);
        assert_eq!(budget.disk_mb, 0);
        Ok(())
    }

    #[test]
    fn invalid_enabled_proof_lease_memory_budget_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_cpu: 1,
                proof_memory_mb: 0,
                proof_disk_mb: 1,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let err = proof_lease_budget(&profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid proof lease budget unexpectedly passed"))?;

        assert_eq!(
            err.to_string(),
            "runtime profile broken has proof_memory_mb=0 with focused proof enabled"
        );
        Ok(())
    }

    #[test]
    fn invalid_enabled_proof_lease_disk_budget_is_rejected() -> Result<()> {
        let profile = Profile {
            name: "broken".to_owned(),
            budgets: Budgets {
                proof_max_focused_tests: 1,
                proof_cpu: 1,
                proof_memory_mb: 1,
                proof_disk_mb: 0,
                ..Budgets::default()
            },
            ..Profile::default()
        };

        let err = proof_lease_budget(&profile)
            .err()
            .ok_or_else(|| anyhow::anyhow!("invalid proof lease budget unexpectedly passed"))?;

        assert_eq!(
            err.to_string(),
            "runtime profile broken has proof_disk_mb=0 with focused proof enabled"
        );
        Ok(())
    }

    #[test]
    fn focused_proof_lease_kinds_count_against_remaining_budget() {
        assert!(focused_proof_lease_counts_budget("focused-test"));
        assert!(focused_proof_lease_counts_budget("focused-build"));
        assert!(!focused_proof_lease_counts_budget("cargo-test"));
        assert!(!focused_proof_lease_counts_budget("sensor"));
    }

    #[test]
    fn focused_proof_lease_statuses_block_budget_when_no_command_may_run() {
        assert!(focused_proof_lease_blocks_budget("exhausted"));
        assert!(focused_proof_lease_blocks_budget("absent"));
        assert!(!focused_proof_lease_blocks_budget("granted"));
        assert!(!focused_proof_lease_blocks_budget("skipped_profile"));
    }

    // JSON keeps the legacy/missing-field case executable on the old type too.
    fn elapsed_lease(timeout: u64, elapsed_ms: u64) -> Result<ResourceLease> {
        let mut value = serde_json::to_value(test_lease("focused-build", "granted", timeout))?;
        value["settled_elapsed_ms"] = serde_json::json!(elapsed_ms);
        Ok(serde_json::from_value(value)?)
    }

    #[test]
    fn terminal_elapsed_reclaims_time_but_not_task_slots() -> Result<()> {
        let budget = ProofBudget {
            max_total_seconds: 1800,
            ..test_budget()
        };
        let leases = [
            elapsed_lease(600, 21_000)?,
            elapsed_lease(600, 21_000)?,
            elapsed_lease(300, 21_000)?,
        ];
        let remaining = remaining_focused_proof_budget(budget, &leases);
        assert_eq!(remaining.max_total_seconds, 1737);
        assert_eq!(remaining.max_focused_tests, 1);
        assert_eq!(remaining.max_focused_test_files, 0);
        assert_eq!(
            leases.iter().map(|lease| lease.timeout_sec).sum::<u64>(),
            1500
        );
        let legacy =
            remaining_focused_proof_budget(budget, &[test_lease("focused-build", "granted", 600)]);
        assert_eq!(legacy.max_total_seconds, 1200);
        Ok(())
    }

    #[test]
    fn recorded_optional_durations_refund_time_without_refunding_slots() -> Result<()> {
        let mut leases = [
            elapsed_lease(600, 17_056)?,
            elapsed_lease(600, 925)?,
            elapsed_lease(600, 11_455)?,
        ];
        for lease in &mut leases {
            lease.kind = "focused-test".to_owned();
        }
        let remaining = remaining_focused_proof_budget(
            ProofBudget {
                max_total_seconds: 1800,
                ..test_budget()
            },
            &leases,
        );
        // Recorded PR1502 command durations total29.436s; rounding per task
        // charges31s. Required-first is still needed: slots are not returned.
        assert_eq!(remaining.max_total_seconds, 1769);
        assert_eq!(remaining.max_focused_tests, 1);
        assert_eq!(
            leases.iter().map(|lease| lease.timeout_sec).sum::<u64>(),
            1800
        );
        Ok(())
    }

    #[test]
    fn elapsed_charge_rounds_up_and_never_caps_overruns() -> Result<()> {
        for (millis, expected_seconds) in [(0, 1), (999, 1), (1000, 1), (1001, 2), (600_001, 601)] {
            let budget = ProofBudget {
                max_total_seconds: 1800,
                ..test_budget()
            };
            let remaining = remaining_focused_proof_budget(budget, &[elapsed_lease(600, millis)?]);
            assert_eq!(
                remaining.max_total_seconds,
                1800 - expected_seconds,
                "{millis}ms"
            );
        }
        let remaining =
            remaining_focused_proof_budget(test_budget(), &[elapsed_lease(1, u64::MAX)?]);
        assert_eq!(remaining.max_total_seconds, 0);
        Ok(())
    }

    fn settlement_receipt(
        status: &str,
        exit_code: Option<i32>,
        timed_out: bool,
    ) -> Result<ProofReceipt> {
        Ok(serde_json::from_value(serde_json::json!({
            "schema": PROOF_RECEIPT_SCHEMA, "id": "proof-test", "kind": "focused-build",
            "base": "base", "head": "head", "test_patch_mode": "head-only",
            "requested_by": [], "request_ids": [], "result": "head_passed", "reason": "fixture",
            "commands": [{"side":"head", "command":"cargo check --locked", "env":{},
                "status":status, "exit_code":exit_code, "timed_out":timed_out,
                "timeout_sec":600, "duration_ms":21_000, "stdout":"out", "stderr":"err", "reason":"fixture"}]
        }))?)
    }

    #[test]
    fn settlement_requires_confirmed_terminal_receipts() -> Result<()> {
        for (status, exit_code, timed_out, settles) in [
            ("passed", Some(0), false, true),
            ("passed", Some(1), false, false),
            ("failed", Some(1), false, true),
            ("failed", Some(101), false, true),
            ("timed_out", None, true, true),
            ("skipped", None, false, false),
            ("passed", None, false, false),
            ("failed", Some(0), false, false),
            ("passed", Some(0), true, false),
            ("timed_out", None, false, false),
            ("failed", None, false, false),
            ("failed", Some(1), true, false),
        ] {
            let mut lease = test_lease("focused-build", "granted", 600);
            let receipt = settlement_receipt(status, exit_code, timed_out)?;
            settle_focused_proof_lease(&mut lease, &receipt, 25_001);
            assert_eq!(
                lease.settled_elapsed_ms,
                settles.then_some(25_001),
                "{status}/{exit_code:?}/{timed_out}"
            );
            assert_eq!(lease.timeout_sec, 600);
            assert_eq!(lease.status, "granted");
        }
        for bad in ["empty", "wrong-consumer", "setup-failed", "wrong-kind"] {
            let mut lease = test_lease("focused-build", "granted", 600);
            let mut receipt = settlement_receipt("passed", Some(0), false)?;
            match bad {
                "empty" => receipt.commands.clear(),
                "wrong-consumer" => receipt.id = "other".to_owned(),
                "setup-failed" => receipt.result = "base_patch_failed".to_owned(),
                _ => receipt.kind = "other".to_owned(),
            }
            settle_focused_proof_lease(&mut lease, &receipt, 1);
            assert_eq!(lease.settled_elapsed_ms, None, "{bad}");
            assert_eq!(focused_proof_lease_charged_seconds(&lease), 600);
        }
        let mut lease = test_lease("focused-build", "granted", 600);
        let mut receipt = settlement_receipt("passed", Some(0), false)?;
        receipt.commands[0].duration_ms = 600_001;
        settle_focused_proof_lease(&mut lease, &receipt, 1);
        assert_eq!(lease.settled_elapsed_ms, Some(600_001));
        assert_eq!(focused_proof_lease_charged_seconds(&lease), 601);
        Ok(())
    }

    #[test]
    fn red_green_settlement_sums_commands_before_applying_task_clock() -> Result<()> {
        let mut lease = test_lease("focused-test", "granted", 600);
        let mut receipt = settlement_receipt("passed", Some(0), false)?;
        receipt.kind = "focused-red-green".to_owned();
        receipt.result = "discriminating".to_owned();
        let mut base = receipt.commands[0].clone();
        base.side = "base".to_owned();
        base.status = "failed".to_owned();
        base.exit_code = Some(101);
        base.duration_ms = 22_000;
        receipt.commands.insert(0, base);

        settle_focused_proof_lease(&mut lease, &receipt, 1);

        assert_eq!(lease.settled_elapsed_ms, Some(43_000));
        Ok(())
    }

    #[test]
    fn elapsed_totals_saturate_instead_of_overflowing() {
        let mut lease = test_lease("focused-test", "granted", 1);
        lease.settled_elapsed_ms = Some(u128::MAX);
        assert_eq!(focused_proof_lease_charged_seconds(&lease), u64::MAX);
        let remaining = remaining_focused_proof_budget(test_budget(), &[lease.clone(), lease]);
        assert_eq!(remaining.max_total_seconds, 0);
    }

    #[test]
    fn remaining_focused_proof_budget_subtracts_granted_focused_leases() {
        let remaining = remaining_focused_proof_budget(
            test_budget(),
            &[
                test_lease("focused-test", "granted", 120),
                test_lease("focused-build", "granted", 60),
                test_lease("cargo-test", "granted", 999),
            ],
        );

        assert_eq!(remaining.max_focused_tests, 2);
        assert_eq!(remaining.max_focused_test_files, 1);
        assert_eq!(remaining.max_total_seconds, 420);
        assert_eq!(remaining.per_command_timeout_sec, 300);
    }

    #[test]
    fn remaining_focused_proof_budget_zeroes_after_exhausted_focused_lease() {
        let remaining = remaining_focused_proof_budget(
            test_budget(),
            &[test_lease("focused-test", "exhausted", 120)],
        );

        assert_eq!(remaining.max_focused_tests, 0);
        assert_eq!(remaining.max_focused_test_files, 0);
        assert_eq!(remaining.max_total_seconds, 0);
        assert_eq!(remaining.per_command_timeout_sec, 300);
    }

    #[test]
    fn remaining_focused_proof_budget_zeroes_after_absent_focused_lease() {
        let remaining = remaining_focused_proof_budget(
            test_budget(),
            &[test_lease("focused-build", "absent", 120)],
        );

        assert_eq!(remaining.max_focused_tests, 0);
        assert_eq!(remaining.max_focused_test_files, 0);
        assert_eq!(remaining.max_total_seconds, 0);
        assert_eq!(remaining.per_command_timeout_sec, 300);
    }

    #[test]
    fn remaining_focused_proof_budget_ignores_unrelated_leases() {
        let remaining = remaining_focused_proof_budget(
            test_budget(),
            &[
                test_lease("cargo-test", "granted", 120),
                test_lease("sensor", "exhausted", 60),
            ],
        );

        assert_eq!(remaining.max_focused_tests, 4);
        assert_eq!(remaining.max_focused_test_files, 3);
        assert_eq!(remaining.max_total_seconds, 600);
        assert_eq!(remaining.per_command_timeout_sec, 300);
    }
}
