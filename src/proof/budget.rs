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
        .map(|lease| lease.timeout_sec)
        .sum::<u64>();
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
