//! Focused proof task and plan types.

use serde::Serialize;

use super::ProofCommandSpec;

#[derive(Clone, Debug)]
pub(crate) struct FocusedTestTask {
    pub(crate) id: String,
    pub(crate) file: String,
    pub(crate) test_name: Option<String>,
    pub(crate) mode: FocusedProofMode,
    pub(crate) command_specs: Option<FocusedTestCommandSpecs>,
    pub(crate) timeout_sec: Option<u64>,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
}

#[derive(Clone, Debug)]
pub(crate) struct FocusedTestCommandSpecs {
    pub(crate) head: ProofCommandSpec,
    pub(crate) base_plus_tests: ProofCommandSpec,
}

#[derive(Clone, Debug)]
pub(crate) struct FocusedBuildTask {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) argv: Vec<String>,
    pub(crate) timeout_sec: u64,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FocusedProofMode {
    HeadOnly,
    RedGreen,
}

impl FocusedProofMode {
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::HeadOnly => "head-only",
            Self::RedGreen => "red-green",
        }
    }

    pub(crate) fn command_count(self) -> u64 {
        match self {
            Self::HeadOnly => 1,
            Self::RedGreen => 2,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct FocusedProofPlan {
    pub(crate) id: String,
    pub(crate) test_file: String,
    pub(crate) test_name: Option<String>,
    pub(crate) mode: FocusedProofMode,
    pub(crate) timeout_sec: u64,
    pub(crate) head_command: String,
    pub(crate) base_plus_tests_command: String,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
    pub(crate) status: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug)]
pub(crate) struct FocusedBuildPlan {
    pub(crate) id: String,
    pub(crate) command: String,
    pub(crate) timeout_sec: u64,
    pub(crate) requested_by: Vec<String>,
    pub(crate) request_ids: Vec<String>,
    pub(crate) status: String,
    pub(crate) reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProofPlannerRuntimeBudget {
    pub(crate) target_timeout_sec: u64,
    pub(crate) hard_timeout_sec: u64,
    pub(crate) max_focused_tests: usize,
    pub(crate) per_command_timeout_sec: u64,
    pub(crate) total_proof_timeout_sec: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focused_proof_mode_keys_and_command_counts_are_stable() {
        assert_eq!(FocusedProofMode::HeadOnly.key(), "head-only");
        assert_eq!(FocusedProofMode::HeadOnly.command_count(), 1);
        assert_eq!(FocusedProofMode::RedGreen.key(), "red-green");
        assert_eq!(FocusedProofMode::RedGreen.command_count(), 2);
    }
}
