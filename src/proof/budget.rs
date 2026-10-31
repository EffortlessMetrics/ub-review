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
    budget: ProofBudget,
    existing_leases: &[ResourceLease],
) -> ProofBudget {
    focused_proof_budget_with_charges(budget, existing_leases, |lease| lease.timeout_sec)
}

/// A ceiling remains reserved until one unambiguous, matching receipt settles
/// it. Keep the original lease intact: its timeout is still the admission cap.
pub(crate) fn settled_focused_proof_budget(
    budget: ProofBudget,
    leases: &[ResourceLease],
    receipts: &[ProofReceipt],
    head: &str,
) -> ProofBudget {
    if receipts.is_empty() {
        return remaining_focused_proof_budget(budget, leases);
    }
    focused_proof_budget_with_charges(budget, leases, |lease| {
        let mut matching = receipts
            .iter()
            .filter(|receipt| receipt.id == lease.consumer);
        let Some(receipt) = matching.next() else {
            return lease.timeout_sec;
        };
        if matching.next().is_some()
            || leases.iter().filter(|other| other.id == lease.id).count() != 1
            || leases
                .iter()
                .filter(|other| other.consumer == lease.consumer && other.status == "granted")
                .count()
                != 1
            || !receipt.head.eq_ignore_ascii_case(head)
            || receipt.revision != lease.revision
            || !matches!(
                (lease.kind.as_str(), receipt.kind.as_str()),
                ("focused-test", "focused-head" | "focused-red-green")
                    | ("focused-build", "focused-build")
            )
        {
            return lease.timeout_sec;
        }
        focused_proof_receipt_spent_seconds(receipt).unwrap_or(lease.timeout_sec)
    })
}

/// Budget units are whole seconds; round the aggregate actual duration up.
/// A real timeout spends at least its allowance, including when its receipt
/// clock rounds down. Unknown/legacy error timing cannot create free credit.
pub(crate) fn focused_proof_receipt_spent_seconds(receipt: &ProofReceipt) -> Option<u64> {
    if receipt.commands.is_empty() {
        return None;
    }
    let expected_sides = match receipt.kind.as_str() {
        "focused-red-green" if receipt.commands[0].status == "passed" => {
            &["head", "base-plus-tests"][..]
        }
        "focused-red-green" | "focused-head" | "focused-build" => &["head"][..],
        _ => return None,
    };
    if !receipt
        .commands
        .iter()
        .map(|command| command.side.as_str())
        .eq(expected_sides.iter().copied())
    {
        return None;
    }
    let mut duration_ms = 0_u128;
    for command in &receipt.commands {
        let spent_ms = match command.status.as_str() {
            "passed" | "failed" if !command.timed_out && command.timeout_sec > 0 => {
                command.duration_ms.max(1)
            }
            "timed_out" if command.timed_out && command.timeout_sec > 0 => command
                .duration_ms
                .max(u128::from(command.timeout_sec) * 1_000),
            "skipped"
                if !command.timed_out && (command.timeout_sec == 0 || command.duration_ms > 0) =>
            {
                command.duration_ms
            }
            _ => return None,
        };
        duration_ms = duration_ms.saturating_add(spent_ms);
    }
    Some(u64::try_from(duration_ms.div_ceil(1_000)).unwrap_or(u64::MAX))
}

fn focused_proof_budget_with_charges(
    mut budget: ProofBudget,
    existing_leases: &[ResourceLease],
    charge: impl Fn(&ResourceLease) -> u64,
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
        .fold(0_u64, |total, lease| total.saturating_add(charge(lease)));
    budget.max_focused_tests = budget.max_focused_tests.saturating_sub(granted);
    budget.max_focused_test_files = budget.max_focused_test_files.saturating_sub(granted);
    budget.max_total_seconds = budget.max_total_seconds.saturating_sub(granted_seconds);
    budget
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

    fn accounting_receipt(status: &str, duration_ms: u128) -> ProofReceipt {
        ProofReceipt {
            revision: None,
            schema: PROOF_RECEIPT_SCHEMA.to_owned(),
            id: "proof-test".to_owned(),
            kind: "focused-head".to_owned(),
            base: "base".to_owned(),
            head: "head".to_owned(),
            test_patch_mode: "head-only".to_owned(),
            requested_by: Vec::new(),
            request_ids: Vec::new(),
            commands: vec![ProofCommandReceipt {
                side: "head".to_owned(),
                command: "fixture".to_owned(),
                env: Default::default(),
                status: status.to_owned(),
                exit_code: Some(if status == "passed" { 0 } else { 7 }),
                timed_out: status == "timed_out",
                timeout_sec: 6,
                duration_ms,
                stdout: String::new(),
                stderr: String::new(),
                reason: "fixture".to_owned(),
            }],
            result: "fixture".to_owned(),
            reason: "fixture".to_owned(),
        }
    }

    #[test]
    fn elapsed_budget_reserves_duplicate_lease_ids_for_distinct_consumers() {
        let first = test_lease("focused-test", "granted", 6);
        let mut second = first.clone();
        second.consumer = "second-proof".to_owned();
        let first_receipt = accounting_receipt("passed", 200);
        let mut second_receipt = first_receipt.clone();
        second_receipt.id = "second-proof".to_owned();
        let mut budget = test_budget();
        budget.max_total_seconds = 20;
        let remaining = settled_focused_proof_budget(
            budget,
            &[first, second],
            &[first_receipt, second_receipt],
            "head",
        );
        assert_eq!(remaining.max_total_seconds, 8);
        assert_eq!(remaining.max_focused_tests, 2);
    }

    #[test]
    fn elapsed_budget_rejects_zero_allowance_and_unknown_receipt_kind() {
        for status in ["passed", "failed", "timed_out"] {
            let mut receipt = accounting_receipt(status, 200);
            receipt.commands[0].timeout_sec = 0;
            assert_eq!(focused_proof_receipt_spent_seconds(&receipt), None);
        }
        let mut receipt = accounting_receipt("passed", 200);
        receipt.kind = "unknown".to_owned();
        assert_eq!(focused_proof_receipt_spent_seconds(&receipt), None);

        let mut receipt = accounting_receipt("skipped", 0);
        assert_eq!(focused_proof_receipt_spent_seconds(&receipt), None);
        receipt.commands[0].timeout_sec = 0;
        assert_eq!(focused_proof_receipt_spent_seconds(&receipt), Some(0));
        receipt.commands[0].timed_out = true;
        assert_eq!(focused_proof_receipt_spent_seconds(&receipt), None);
    }

    #[test]
    fn accounting_settles_actual_duration_with_conservative_rounding() {
        for status in ["passed", "failed", "skipped"] {
            for (ms, spent) in [(1, 1), (999, 1), (1_000, 1), (1_001, 2), (6_001, 7)] {
                let lease = test_lease("focused-test", "granted", 6);
                let receipt = accounting_receipt(status, ms);
                let budget =
                    settled_focused_proof_budget(test_budget(), &[lease], &[receipt], "head");
                assert_eq!(budget.max_total_seconds, 600 - spent);
                assert_eq!(budget.max_focused_tests, 3);
                assert_eq!(budget.max_focused_test_files, 2);
                assert_eq!(budget.per_command_timeout_sec, 300);
            }
        }
    }

    #[test]
    fn accounting_timeout_spends_allowance_and_aggregates_red_green_once() {
        let mut receipt = accounting_receipt("timed_out", 1);
        assert_eq!(focused_proof_receipt_spent_seconds(&receipt), Some(6));
        receipt.commands[0].duration_ms = 6_001;
        assert_eq!(focused_proof_receipt_spent_seconds(&receipt), Some(7));
        receipt = accounting_receipt("passed", 1);
        receipt.kind = "focused-red-green".to_owned();
        let mut base = receipt.commands[0].clone();
        base.side = "base-plus-tests".to_owned();
        receipt.commands.push(base);
        assert_eq!(focused_proof_receipt_spent_seconds(&receipt), Some(1));
        receipt.commands[1].status = "skipped".to_owned();
        receipt.commands[1].timeout_sec = 0;
        receipt.commands[1].duration_ms = 0;
        assert_eq!(focused_proof_receipt_spent_seconds(&receipt), Some(1));
    }

    #[test]
    fn accounting_ambiguous_or_unbound_receipts_keep_reservation() {
        let lease = test_lease("focused-test", "granted", 6);
        let valid = accounting_receipt("passed", 1);
        let mut bad = Vec::new();
        let mut receipt = valid.clone();
        receipt.head = "stale".to_owned();
        bad.push(vec![receipt]);
        let mut receipt = valid.clone();
        receipt.kind = "focused-build".to_owned();
        bad.push(vec![receipt]);
        let mut receipt = valid.clone();
        receipt.kind = "focused-red-green".to_owned();
        bad.push(vec![receipt]);
        let mut receipt = valid.clone();
        receipt.id = "unrelated".to_owned();
        bad.push(vec![receipt]);
        let mut receipt = valid.clone();
        receipt.commands.push(receipt.commands[0].clone());
        bad.push(vec![receipt]);
        let mut receipt = valid.clone();
        receipt.commands[0].side = "unknown".to_owned();
        bad.push(vec![receipt]);
        let mut receipt = valid.clone();
        receipt.commands[0].status = "running".to_owned();
        bad.push(vec![receipt]);
        let mut receipt = valid.clone();
        receipt.revision = Some(RevisionRef {
            digest: "a".repeat(64),
            semantics: "candidate_head".to_owned(),
            reviewed_commit: "b".repeat(40),
        });
        bad.push(vec![receipt]);
        bad.push(vec![accounting_receipt("skipped", 0)]);
        bad.push(vec![valid.clone(), valid]);
        bad.push(Vec::new());
        for receipts in bad {
            let budget = settled_focused_proof_budget(
                test_budget(),
                std::slice::from_ref(&lease),
                &receipts,
                "head",
            );
            assert_eq!(budget.max_total_seconds, 594);
        }
    }

    #[test]
    fn accounting_duplicate_reservations_and_overflow_never_create_credit() {
        let lease = test_lease("focused-test", "granted", 6);
        let receipt = accounting_receipt("passed", 1);
        for different_id in [false, true] {
            let mut duplicate = lease.clone();
            if different_id {
                duplicate.id.push_str("-second");
            }
            let budget = settled_focused_proof_budget(
                test_budget(),
                &[lease.clone(), duplicate],
                std::slice::from_ref(&receipt),
                "head",
            );
            assert_eq!(budget.max_total_seconds, 588);
        }
        let budget = remaining_focused_proof_budget(
            test_budget(),
            &[
                test_lease("focused-test", "granted", u64::MAX),
                test_lease("focused-build", "granted", u64::MAX),
            ],
        );
        assert_eq!(budget.max_total_seconds, 0);
        assert_eq!(
            focused_proof_receipt_spent_seconds(&accounting_receipt("passed", u128::MAX)),
            Some(u64::MAX)
        );
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
