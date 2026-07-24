//! Current-head gate watchdog: a pure classifier over **frozen** run/proof
//! observations that decides whether the PR head has reached an honest terminal
//! state, and emits one versioned `review/gate_watchdog.json` receipt.
//!
//! Issue #745 (child of #658). This slice defines deterministic detection and
//! receipt semantics only: it does **not** query GitHub, publish a check,
//! mutate branch protection, or replace the candidate-supplied self-gate. The
//! command is additive and inert until a future stable-coordinator workflow
//! consumes the artifact.
//!
//! Fail-closed is the governing invariant: missing or malformed evidence can
//! never serialize as `pass`. Only an exact-head completed run whose matching
//! `gate_outcome.json` records a verdict is trusted, and even then a crashed
//! coordinator or an orphaned required proof downgrades the head to
//! `inconclusive`. Every non-terminal verdict carries a receipt pointer and a
//! retry action so an operator (or the future coordinator) knows what to do.

use std::fs;

use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};

use crate::artifacts::GATE_WATCHDOG_SCHEMA;
use crate::cli::GateWatchdogArgs;

/// One observed check/workflow run at (or near) the PR head. `status` mirrors
/// the GitHub run lifecycle (`queued` / `in_progress` / `completed`);
/// `conclusion` is only meaningful once `status == "completed"`.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WatchdogRunObservation {
    #[serde(default)]
    pub(crate) sha: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) conclusion: Option<String>,
    #[serde(default)]
    pub(crate) run_id: u64,
    #[serde(default)]
    pub(crate) attempt: u64,
}

impl WatchdogRunObservation {
    fn is_completed(&self) -> bool {
        self.status.trim().eq_ignore_ascii_case("completed")
    }

    fn conclusion_is(&self, expected: &str) -> bool {
        self.conclusion
            .as_deref()
            .is_some_and(|c| c.trim().eq_ignore_ascii_case(expected))
    }
}

/// The recorded `gate_outcome.json` observation for the head, if any. `present`
/// is authoritative: a `false` value means the artifact was absent, so the head
/// cannot be trusted as terminal regardless of the other fields.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct WatchdogGateArtifact {
    #[serde(default)]
    pub(crate) present: bool,
    #[serde(default)]
    pub(crate) conclusion: Option<String>,
    #[serde(default)]
    pub(crate) run_id: Option<u64>,
    #[serde(default)]
    pub(crate) sha: Option<String>,
}

/// The coordinator terminal marker for the head. A crashed coordinator makes
/// the verdict substrate untrustworthy even if a stale artifact survived.
#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct WatchdogCoordinatorMarker {
    #[serde(default)]
    pub(crate) present: bool,
    #[serde(default)]
    pub(crate) crashed: bool,
    #[serde(default)]
    pub(crate) sha: Option<String>,
}

/// A required-proof terminal marker. A proof bound to a different head, or one
/// that never reached a terminal state before the deadline, is orphaned.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WatchdogRequiredProof {
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) terminal: bool,
    #[serde(default)]
    pub(crate) sha: Option<String>,
}

/// Frozen observation bundle consumed by the classifier. This is the on-disk
/// input contract for `ub-review gate-watchdog --observations <file>`.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct WatchdogObservations {
    #[serde(default)]
    pub(crate) expected_sha: String,
    /// Whether the observation deadline has elapsed. Before the deadline a head
    /// with no completed run stays `pending`; after it the same head is an
    /// honest `inconclusive`.
    #[serde(default)]
    pub(crate) deadline_passed: bool,
    #[serde(default)]
    pub(crate) runs: Vec<WatchdogRunObservation>,
    #[serde(default)]
    pub(crate) gate_artifact: WatchdogGateArtifact,
    #[serde(default)]
    pub(crate) coordinator: WatchdogCoordinatorMarker,
    #[serde(default)]
    pub(crate) required_proofs: Vec<WatchdogRequiredProof>,
}

/// The watchdog verdict artifact (`review/gate_watchdog.json`). `state` is one
/// of `terminal` / `pending` / `inconclusive`; `conclusion` is populated only
/// on a `terminal` state and only from a trusted gate artifact, so a
/// `pass` conclusion can never appear on missing or malformed evidence.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub(crate) struct GateWatchdog {
    pub(crate) schema: String,
    pub(crate) expected_sha: String,
    pub(crate) state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) conclusion: Option<String>,
    pub(crate) reason: String,
    pub(crate) detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) receipt_pointer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) retry_action: Option<String>,
}

impl GateWatchdog {
    fn terminal(expected_sha: &str, conclusion: &str, detail: String, receipt: String) -> Self {
        GateWatchdog {
            schema: GATE_WATCHDOG_SCHEMA.to_owned(),
            expected_sha: expected_sha.to_owned(),
            state: "terminal".to_owned(),
            conclusion: Some(conclusion.to_owned()),
            reason: "exact-head-terminal".to_owned(),
            detail,
            receipt_pointer: Some(receipt),
            retry_action: None,
        }
    }

    fn pending(
        expected_sha: &str,
        reason: &str,
        detail: String,
        receipt: String,
        retry: &str,
    ) -> Self {
        GateWatchdog {
            schema: GATE_WATCHDOG_SCHEMA.to_owned(),
            expected_sha: expected_sha.to_owned(),
            state: "pending".to_owned(),
            conclusion: None,
            reason: reason.to_owned(),
            detail,
            receipt_pointer: Some(receipt),
            retry_action: Some(retry.to_owned()),
        }
    }

    fn inconclusive(
        expected_sha: &str,
        reason: &str,
        detail: String,
        receipt: String,
        retry: &str,
    ) -> Self {
        GateWatchdog {
            schema: GATE_WATCHDOG_SCHEMA.to_owned(),
            expected_sha: expected_sha.to_owned(),
            state: "inconclusive".to_owned(),
            conclusion: None,
            reason: reason.to_owned(),
            detail,
            receipt_pointer: Some(receipt),
            retry_action: Some(retry.to_owned()),
        }
    }
}

/// Normalize a recorded gate conclusion to the closed set the terminal state is
/// allowed to preserve. Anything outside `pass` / `fail` / `inconclusive` is
/// malformed evidence and must not be trusted (returns `None`).
fn normalized_gate_conclusion(raw: Option<&str>) -> Option<&'static str> {
    match raw.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("pass") => Some("pass"),
        Some("fail") => Some("fail"),
        Some("inconclusive") => Some("inconclusive"),
        _ => None,
    }
}

/// Whether the recorded gate artifact genuinely belongs to `selected` at the
/// expected head. Fail-closed: the artifact must be present, its SHA must match
/// the expected head, and when it names a run that run must be the selected one.
fn gate_artifact_matches(
    artifact: &WatchdogGateArtifact,
    selected: &WatchdogRunObservation,
    expected_sha: &str,
) -> bool {
    if !artifact.present {
        return false;
    }
    if artifact.sha.as_deref() != Some(expected_sha) {
        return false;
    }
    match artifact.run_id {
        Some(run_id) => run_id == selected.run_id,
        None => true,
    }
}

/// A required proof is orphaned when it is bound to a different head SHA, or
/// when it never reached a terminal state and the deadline has already passed.
fn required_proof_orphaned(
    proof: &WatchdogRequiredProof,
    expected_sha: &str,
    deadline_passed: bool,
) -> bool {
    if proof.sha.as_deref().is_some_and(|sha| sha != expected_sha) {
        return true;
    }
    !proof.terminal && deadline_passed
}

/// Classify the current head from frozen observations. Pure and deterministic:
/// identical input always yields an identical artifact, and no branch can
/// serialize `pass` without a trusted exact-head gate artifact.
///
/// Precedence (documented decision): a still-running head within the deadline
/// stays `pending` first, so a transient state never prematurely reds a head
/// that may still complete; a crashed coordinator and an orphaned proof then
/// win over any surviving artifact, because they mean the verdict substrate is
/// untrustworthy; only after those do the exact-head run and staleness checks
/// run.
pub(crate) fn build_gate_watchdog(obs: &WatchdogObservations) -> GateWatchdog {
    let sha = obs.expected_sha.trim();

    let exact: Vec<&WatchdogRunObservation> =
        obs.runs.iter().filter(|r| r.sha.trim() == sha).collect();
    let has_in_progress = exact.iter().any(|r| !r.is_completed());

    // 1. A newer in-progress replacement before the deadline: stay pending and
    //    never serialize pass or fail while the head may still complete.
    if has_in_progress && !obs.deadline_passed {
        let latest = exact
            .iter()
            .filter(|r| !r.is_completed())
            .max_by_key(|r| (r.run_id, r.attempt));
        let run_id = latest.map(|r| r.run_id).unwrap_or_default();
        return GateWatchdog::pending(
            sha,
            "in-progress-replacement",
            format!("run {run_id} is still in progress at head {sha} before the deadline"),
            format!("run:{run_id}"),
            "await run completion before the observation deadline",
        );
    }

    // 2. A crashed coordinator — or one whose marker is bound to a different
    //    head — makes the verdict substrate untrustworthy. A stale-SHA marker
    //    cannot vouch for the current head any more than a crashed one can.
    if obs.coordinator.present {
        let stale_marker = obs
            .coordinator
            .sha
            .as_deref()
            .is_some_and(|marker_sha| marker_sha.trim() != sha);
        if obs.coordinator.crashed || stale_marker {
            let detail = if stale_marker {
                let marker_sha = obs.coordinator.sha.as_deref().unwrap_or_default().trim();
                format!("coordinator marker is bound to head {marker_sha}, not {sha}")
            } else {
                format!("coordinator terminated without a terminal marker at head {sha}")
            };
            return GateWatchdog::inconclusive(
                sha,
                "coordinator-crash",
                detail,
                "coordinator-marker".to_owned(),
                "restart the coordinator for the head and re-observe",
            );
        }
    }

    // 3. An orphaned required proof (wrong head, or never terminal by deadline).
    if let Some(proof) = obs
        .required_proofs
        .iter()
        .find(|p| required_proof_orphaned(p, sha, obs.deadline_passed))
    {
        let name = if proof.name.trim().is_empty() {
            "<unnamed>"
        } else {
            proof.name.trim()
        };
        return GateWatchdog::inconclusive(
            sha,
            "orphaned-proof",
            format!("required proof `{name}` is orphaned relative to head {sha}"),
            format!("proof:{name}"),
            "re-request the required proof for the head and re-observe",
        );
    }

    // 4. Exact-head completed runs.
    if let Some(selected) = exact
        .iter()
        .filter(|r| r.is_completed())
        .max_by_key(|r| (r.run_id, r.attempt))
    {
        if selected.conclusion_is("cancelled") {
            // Step 1 already ruled out an in-progress replacement, so the newest
            // exact-head run is a cancellation with nothing replacing it.
            return GateWatchdog::inconclusive(
                sha,
                "cancelled-without-replacement",
                format!(
                    "exact-head run {} was cancelled with no newer replacement",
                    selected.run_id
                ),
                format!("run:{}", selected.run_id),
                "re-dispatch the gate workflow for the head",
            );
        }

        if !gate_artifact_matches(&obs.gate_artifact, selected, sha) {
            return GateWatchdog::inconclusive(
                sha,
                "missing-artifact",
                format!(
                    "exact-head run {} completed but no matching gate_outcome.json was recorded",
                    selected.run_id
                ),
                format!("run:{}", selected.run_id),
                "re-run the gate to regenerate review/gate_outcome.json for the head",
            );
        }

        let Some(conclusion) = normalized_gate_conclusion(obs.gate_artifact.conclusion.as_deref())
        else {
            return GateWatchdog::inconclusive(
                sha,
                "missing-artifact",
                format!(
                    "exact-head run {} recorded a malformed gate conclusion",
                    selected.run_id
                ),
                format!("run:{}", selected.run_id),
                "re-run the gate to regenerate review/gate_outcome.json for the head",
            );
        };

        return GateWatchdog::terminal(
            sha,
            conclusion,
            format!(
                "exact-head run {} completed with gate conclusion {conclusion}",
                selected.run_id
            ),
            format!("review/gate_outcome.json@run={}", selected.run_id),
        );
    }

    // 5. No exact-head completed run.
    if obs.deadline_passed {
        if let Some(stale) = obs
            .runs
            .iter()
            .find(|r| r.sha.trim() != sha && r.is_completed() && r.conclusion_is("success"))
        {
            return GateWatchdog::inconclusive(
                sha,
                "stale-success",
                format!(
                    "run {} succeeded on stale head {}, which cannot satisfy head {sha}",
                    stale.run_id,
                    stale.sha.trim()
                ),
                format!("run:{}", stale.run_id),
                "dispatch the gate workflow for the current head",
            );
        }
        return GateWatchdog::inconclusive(
            sha,
            "missing-run",
            format!("no completed run was observed for head {sha} before the deadline"),
            format!("sha:{sha}"),
            "dispatch the gate workflow for the head",
        );
    }

    GateWatchdog::pending(
        sha,
        "awaiting-run",
        format!("no run has been observed for head {sha} yet, within the deadline"),
        format!("sha:{sha}"),
        "await the gate workflow dispatch for the head",
    )
}

/// Read frozen observations, classify the head, and write the versioned
/// `review/gate_watchdog.json` artifact under `--out`. Artifact-only; never
/// touches GitHub, branch protection, or any live provider state.
pub(crate) fn cmd_gate_watchdog(args: GateWatchdogArgs) -> Result<()> {
    let bytes = fs::read(&args.observations)
        .with_context(|| format!("read frozen observations {}", args.observations.display()))?;
    let observations: WatchdogObservations = serde_json::from_slice(&bytes)
        .with_context(|| format!("parse frozen observations {}", args.observations.display()))?;

    let watchdog = build_gate_watchdog(&observations);

    let review_dir = args.out.join("review");
    fs::create_dir_all(&review_dir).with_context(|| format!("create {}", review_dir.display()))?;
    let out_path = review_dir.join("gate_watchdog.json");
    fs::write(&out_path, serde_json::to_vec_pretty(&watchdog)?)
        .with_context(|| format!("write {}", out_path.display()))?;

    println!(
        "gate-watchdog: wrote {} (state={} reason={})",
        out_path.display(),
        watchdog.state,
        watchdog.reason
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(
        sha: &str,
        status: &str,
        conclusion: Option<&str>,
        run_id: u64,
        attempt: u64,
    ) -> WatchdogRunObservation {
        WatchdogRunObservation {
            sha: sha.to_owned(),
            status: status.to_owned(),
            conclusion: conclusion.map(str::to_owned),
            run_id,
            attempt,
        }
    }

    fn base(expected_sha: &str) -> WatchdogObservations {
        WatchdogObservations {
            expected_sha: expected_sha.to_owned(),
            deadline_passed: true,
            runs: Vec::new(),
            gate_artifact: WatchdogGateArtifact::default(),
            coordinator: WatchdogCoordinatorMarker::default(),
            required_proofs: Vec::new(),
        }
    }

    #[test]
    fn exact_head_completed_with_artifact_is_terminal_and_preserves_conclusion() {
        for conclusion in ["pass", "fail", "inconclusive"] {
            let mut obs = base("head1");
            obs.runs = vec![run("head1", "completed", Some("success"), 10, 1)];
            obs.gate_artifact = WatchdogGateArtifact {
                present: true,
                conclusion: Some(conclusion.to_owned()),
                run_id: Some(10),
                sha: Some("head1".to_owned()),
            };
            let w = build_gate_watchdog(&obs);
            assert_eq!(w.state, "terminal", "{conclusion}");
            assert_eq!(w.conclusion.as_deref(), Some(conclusion));
            assert_eq!(w.reason, "exact-head-terminal");
            assert!(w.retry_action.is_none());
            assert_eq!(w.schema, GATE_WATCHDOG_SCHEMA);
        }
    }

    #[test]
    fn no_exact_head_run_after_deadline_is_missing_run() {
        let obs = base("head1");
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "inconclusive");
        assert_eq!(w.reason, "missing-run");
        assert!(w.conclusion.is_none());
        assert!(w.receipt_pointer.is_some());
        assert!(w.retry_action.is_some());
    }

    #[test]
    fn cancelled_exact_head_without_replacement_is_inconclusive() {
        let mut obs = base("head1");
        obs.runs = vec![run("head1", "completed", Some("cancelled"), 10, 1)];
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "inconclusive");
        assert_eq!(w.reason, "cancelled-without-replacement");
        assert!(w.conclusion.is_none());
        assert!(w.retry_action.is_some());
    }

    #[test]
    fn cancelled_with_in_progress_replacement_before_deadline_stays_pending() {
        let mut obs = base("head1");
        obs.deadline_passed = false;
        obs.runs = vec![
            run("head1", "completed", Some("cancelled"), 10, 1),
            run("head1", "in_progress", None, 11, 1),
        ];
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "pending");
        assert_eq!(w.reason, "in-progress-replacement");
        assert!(w.conclusion.is_none());
    }

    #[test]
    fn older_sha_success_cannot_satisfy_current_head() {
        let mut obs = base("head2");
        obs.runs = vec![run("head1", "completed", Some("success"), 9, 1)];
        obs.gate_artifact = WatchdogGateArtifact {
            present: true,
            conclusion: Some("pass".to_owned()),
            run_id: Some(9),
            sha: Some("head1".to_owned()),
        };
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "inconclusive");
        assert_eq!(w.reason, "stale-success");
        assert!(
            w.conclusion.is_none(),
            "stale success must never serialize pass"
        );
    }

    #[test]
    fn completed_run_missing_artifact_is_inconclusive() {
        let mut obs = base("head1");
        obs.runs = vec![run("head1", "completed", Some("success"), 10, 1)];
        // gate_artifact absent (default present=false)
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "inconclusive");
        assert_eq!(w.reason, "missing-artifact");
        assert!(w.conclusion.is_none());
    }

    #[test]
    fn artifact_bound_to_other_run_is_missing_artifact() {
        let mut obs = base("head1");
        obs.runs = vec![run("head1", "completed", Some("success"), 10, 1)];
        obs.gate_artifact = WatchdogGateArtifact {
            present: true,
            conclusion: Some("pass".to_owned()),
            run_id: Some(7), // wrong run
            sha: Some("head1".to_owned()),
        };
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "inconclusive");
        assert_eq!(w.reason, "missing-artifact");
    }

    #[test]
    fn malformed_gate_conclusion_never_passes() {
        let mut obs = base("head1");
        obs.runs = vec![run("head1", "completed", Some("success"), 10, 1)];
        obs.gate_artifact = WatchdogGateArtifact {
            present: true,
            conclusion: Some("green".to_owned()), // malformed
            run_id: Some(10),
            sha: Some("head1".to_owned()),
        };
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "inconclusive");
        assert_eq!(w.reason, "missing-artifact");
        assert!(w.conclusion.is_none());
    }

    #[test]
    fn orphaned_required_proof_is_inconclusive() {
        let mut obs = base("head1");
        obs.runs = vec![run("head1", "completed", Some("success"), 10, 1)];
        obs.gate_artifact = WatchdogGateArtifact {
            present: true,
            conclusion: Some("pass".to_owned()),
            run_id: Some(10),
            sha: Some("head1".to_owned()),
        };
        obs.required_proofs = vec![WatchdogRequiredProof {
            name: "sanitizer".to_owned(),
            terminal: true,
            sha: Some("otherhead".to_owned()), // bound to the wrong head
        }];
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "inconclusive");
        assert_eq!(w.reason, "orphaned-proof");
        assert!(
            w.conclusion.is_none(),
            "orphaned proof must not serialize pass"
        );
    }

    #[test]
    fn coordinator_crash_is_inconclusive_even_with_artifact() {
        let mut obs = base("head1");
        obs.runs = vec![run("head1", "completed", Some("success"), 10, 1)];
        obs.gate_artifact = WatchdogGateArtifact {
            present: true,
            conclusion: Some("pass".to_owned()),
            run_id: Some(10),
            sha: Some("head1".to_owned()),
        };
        obs.coordinator = WatchdogCoordinatorMarker {
            present: true,
            crashed: true,
            sha: Some("head1".to_owned()),
        };
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "inconclusive");
        assert_eq!(w.reason, "coordinator-crash");
        assert!(w.conclusion.is_none());
    }

    #[test]
    fn coordinator_marker_bound_to_other_head_is_inconclusive() {
        let mut obs = base("head1");
        obs.runs = vec![run("head1", "completed", Some("success"), 10, 1)];
        obs.gate_artifact = WatchdogGateArtifact {
            present: true,
            conclusion: Some("pass".to_owned()),
            run_id: Some(10),
            sha: Some("head1".to_owned()),
        };
        obs.coordinator = WatchdogCoordinatorMarker {
            present: true,
            crashed: false,
            sha: Some("otherhead".to_owned()),
        };
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "inconclusive");
        assert_eq!(w.reason, "coordinator-crash");
        assert!(w.conclusion.is_none());
    }

    #[test]
    fn newer_in_progress_replacement_before_deadline_never_serializes_verdict() {
        let mut obs = base("head1");
        obs.deadline_passed = false;
        obs.runs = vec![run("head1", "in_progress", None, 12, 1)];
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "pending");
        assert!(w.conclusion.is_none());
        assert_ne!(w.state, "terminal");
    }

    #[test]
    fn no_run_before_deadline_is_pending_awaiting_run() {
        let mut obs = base("head1");
        obs.deadline_passed = false;
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "pending");
        assert_eq!(w.reason, "awaiting-run");
    }

    #[test]
    fn classification_is_deterministic() -> Result<()> {
        let mut obs = base("head1");
        obs.runs = vec![run("head1", "completed", Some("success"), 10, 1)];
        obs.gate_artifact = WatchdogGateArtifact {
            present: true,
            conclusion: Some("pass".to_owned()),
            run_id: Some(10),
            sha: Some("head1".to_owned()),
        };
        let first = build_gate_watchdog(&obs);
        let second = build_gate_watchdog(&obs);
        assert_eq!(first, second);
        assert_eq!(
            serde_json::to_vec_pretty(&first)?,
            serde_json::to_vec_pretty(&second)?,
        );
        Ok(())
    }

    #[test]
    fn latest_attempt_is_selected_for_terminal() {
        let mut obs = base("head1");
        obs.runs = vec![
            run("head1", "completed", Some("cancelled"), 10, 1),
            run("head1", "completed", Some("success"), 10, 2),
        ];
        obs.gate_artifact = WatchdogGateArtifact {
            present: true,
            conclusion: Some("pass".to_owned()),
            run_id: Some(10),
            sha: Some("head1".to_owned()),
        };
        let w = build_gate_watchdog(&obs);
        assert_eq!(w.state, "terminal");
        assert_eq!(w.conclusion.as_deref(), Some("pass"));
    }
}
