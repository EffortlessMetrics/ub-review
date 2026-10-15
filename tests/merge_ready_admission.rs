//! Structural/trigger fixtures for dormant native Actions candidates.
//! These model the documented Actions predicate; they do not emulate GitHub.
use anyhow::{Context, Result, ensure};

const ADMISSION: &str = "if: ${{ github.event_name == 'pull_request_target' && github.event.pull_request.state == 'open' && !github.event.pull_request.draft && contains(github.event.pull_request.labels.*.name, 'merge-ready') }}";
const EVENTS: &str = "types: [opened, reopened, ready_for_review, synchronize, labeled, unlabeled, converted_to_draft]";
const ACTIONS: [&str; 7] = [
    "opened",
    "reopened",
    "ready_for_review",
    "synchronize",
    "labeled",
    "unlabeled",
    "converted_to_draft",
];

fn admitted(
    native_event: &str,
    event: &str,
    action: &str,
    open: bool,
    draft: bool,
    labels: &[&str],
) -> bool {
    event == native_event
        && ACTIONS.contains(&action)
        && open
        && !draft
        && labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case("merge-ready"))
}

#[test]
fn native_trigger_fixtures() {
    for native_event in ["pull_request", "pull_request_target"] {
        assert!(admitted(
            native_event,
            native_event,
            "labeled",
            true,
            false,
            &["MERGE-READY"]
        ));
        for action in ACTIONS {
            assert!(admitted(
                native_event,
                native_event,
                action,
                true,
                false,
                &["merge-ready"]
            ));
            assert!(!admitted(
                native_event,
                native_event,
                action,
                true,
                true,
                &["merge-ready"]
            ));
            assert!(!admitted(
                native_event,
                native_event,
                action,
                true,
                false,
                &[]
            ));
            assert!(!admitted(
                native_event,
                native_event,
                action,
                false,
                false,
                &["merge-ready"]
            ));
        }
        for event in ["push", "workflow_dispatch", "schedule"] {
            assert!(!admitted(
                native_event,
                event,
                "opened",
                true,
                false,
                &["merge-ready"]
            ));
        }
        assert!(!admitted(
            native_event,
            native_event,
            "closed",
            true,
            false,
            &["merge-ready"]
        ));
        assert!(!admitted(
            native_event,
            native_event,
            "labeled",
            true,
            false,
            &["merge-ready-extra"]
        ));
        assert!(admitted(
            native_event,
            native_event,
            "synchronize",
            true,
            false,
            &["merge-ready", "ci/default"]
        ));
        assert!(!admitted(
            native_event,
            native_event,
            "unlabeled",
            true,
            false,
            &["ci/default"]
        ));
    }
}

#[test]
fn dormant_candidates_preserve_native_checks_and_execution() -> Result<()> {
    for (name, job) in [
        ("independent-baseline", "evidence"),
        ("ub-review-gate", "gate"),
    ] {
        let active = std::fs::read_to_string(format!(".github/workflows/{name}.yml"))?;
        let candidate = std::fs::read_to_string(format!("templates/ci/{name}.yml"))?;
        ensure!(!active.contains("merge-ready"), "legacy remains active");
        ensure!(candidate.contains(EVENTS));
        let predicate = if job == "gate" {
            ADMISSION.replace("pull_request_target", "pull_request")
        } else {
            ADMISSION.to_owned()
        };
        ensure!(candidate.contains(&format!("  {job}:\n    {predicate}\n")));
        ensure!(candidate.contains(if job == "gate" {
            "  pull_request:\n"
        } else {
            "  pull_request_target:\n"
        }));
        ensure!(!candidate.contains("  workflow_dispatch:\n"));
        ensure!(!candidate.contains("checks: write"));
        ensure!(!candidate.contains("pull-requests: write"));
        ensure!(candidate.contains("persist-credentials: false"));
        if job == "evidence" {
            ensure!(candidate.contains("ref: ${{ github.event.pull_request.head.sha }}"));
        }
        // Exact existing execution steps remain; admission must not replace proof.
        let start = if job == "gate" {
            "      - name: Verify task projection regressions"
        } else {
            "      - name: Verify exact candidate identity"
        };
        let end = if job == "gate" {
            "  # Coverage publication"
        } else {
            "  finalize:"
        };
        let body = active
            .split_once(start)
            .context("active execution start")?
            .1
            .split_once(end)
            .context("active execution end")?
            .0;
        ensure!(candidate.contains(body), "preserve execution for {name}");
        if job == "gate" {
            ensure!(candidate.contains("  admission-result:\n    name: ub-review/gate\n    needs: gate\n    if: ${{ always() }}"));
            ensure!(candidate.contains("run: test \"$EVIDENCE_RESULT\" = success"));
            ensure!(candidate.contains("needs.gate.result != 'skipped' && always()"));
        } else {
            ensure!(candidate.contains(
                "name: ub-review/independent-baseline\n    needs: evidence\n    if: ${{ always() }}"
            ));
            ensure!(candidate.contains("if [[ \"$EVIDENCE_RESULT\" != \"success\" ]]; then"));
        }
    }
    Ok(())
}
