//! Structural Actions predicates plus execution of the actual native live checks.
//! The gh fixture is local; this does not emulate GitHub event scheduling.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

const EDGE: &str = "if: ${{ github.event.action == 'labeled' && github.event.label.name == 'merge-ready' && github.event.pull_request.state == 'open' && !github.event.pull_request.draft && contains(github.event.pull_request.labels.*.name, 'merge-ready') }}";
const EVENTS: &str = "types: [opened, reopened, ready_for_review, synchronize, labeled, unlabeled, converted_to_draft, edited, closed]";
const TEMPLATES: [(&str, &str, &str); 2] = [
    (
        "ub-review-gate",
        "gate",
        "Require current admitted successful candidate evidence",
    ),
    (
        "independent-baseline",
        "evidence",
        "Enforce independent baseline",
    ),
];

fn edge(action: &str, event_label: &str, open: bool, draft: bool, labels: &[&str]) -> bool {
    action == "labeled"
        && event_label.eq_ignore_ascii_case("merge-ready")
        && open
        && !draft
        && labels
            .iter()
            .any(|label| label.eq_ignore_ascii_case("merge-ready"))
}

#[test]
fn heavy_allocation_requires_a_fresh_label_edge() {
    assert!(edge(
        "labeled",
        "merge-ready",
        true,
        false,
        &["merge-ready"]
    ));
    assert!(edge(
        "labeled",
        "MERGE-READY",
        true,
        false,
        &["MERGE-READY"]
    ));
    for action in [
        "opened",
        "reopened",
        "ready_for_review",
        "synchronize",
        "unlabeled",
        "converted_to_draft",
        "edited",
        "closed",
    ] {
        assert!(
            !edge(action, "merge-ready", true, false, &["merge-ready"]),
            "sticky label must not admit {action}"
        );
    }
    for label in ["ci/default", "merge-ready-extra", ""] {
        assert!(!edge("labeled", label, true, false, &["merge-ready"]));
    }
    assert!(!edge(
        "labeled",
        "merge-ready",
        false,
        false,
        &["merge-ready"]
    ));
    assert!(!edge(
        "labeled",
        "merge-ready",
        true,
        true,
        &["merge-ready"]
    ));
    assert!(!edge("labeled", "merge-ready", true, false, &[]));
}

fn workflow(name: &str) -> Result<String> {
    std::fs::read_to_string(format!("templates/ci/{name}.yml")).context("read dormant workflow")
}

fn shell(source: &str, step: &str) -> Result<String> {
    let tail = source
        .split_once(&format!("      - name: {step}\n"))
        .context("named native step")?
        .1;
    let body = tail
        .split_once("        run: |\n")
        .context("native shell body")?
        .1;
    Ok(body
        .lines()
        .take_while(|line| line.starts_with("          ") || line.is_empty())
        .map(|line| line.strip_prefix("          ").unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n"))
}

#[test]
fn dormant_candidates_preserve_execution_and_native_finalizers() -> Result<()> {
    for (name, job, final_step) in TEMPLATES {
        let active = std::fs::read_to_string(format!(".github/workflows/{name}.yml"))?;
        let candidate = workflow(name)?;
        ensure!(!active.contains("merge-ready"), "legacy remains active");
        ensure!(candidate.contains(EVENTS));
        ensure!(candidate.contains(&format!(
            "  admission:\n    name: current admission snapshot\n    {EDGE}"
        )));
        ensure!(candidate.contains(&format!("  {job}:\n    needs: admission\n")));
        ensure!(candidate.contains("needs.admission.result == 'success'"));
        ensure!(candidate.contains("cancel-in-progress: false"));
        ensure!(candidate.contains("&& 'qualification' || format('observe-{0}', github.run_id)"));
        ensure!(!candidate.contains("checks: write"));
        ensure!(!candidate.contains("pull-requests: write"));
        ensure!(candidate.contains("persist-credentials: false"));
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
            .context("active proof start")?
            .1
            .split_once(end)
            .context("active proof end")?
            .0;
        ensure!(candidate.contains(body), "preserve proof population {name}");
        if job == "gate" {
            ensure!(candidate.contains("  workflow_dispatch:\n"));
            ensure!(candidate.contains(
                "name: ub-review/gate\n    needs: [admission, gate]\n    if: ${{ always() }}"
            ));
            ensure!(candidate.contains("needs.gate.result != 'skipped' && always()"));
        } else {
            ensure!(candidate.contains("name: ub-review/independent-baseline\n    needs: [admission, evidence]\n    if: ${{ always() }}"));
        }
        let admission = shell(&candidate, "Check fresh label edge against live PR")?;
        let finalizer = shell(&candidate, final_step)?;
        ensure!(
            finalizer.ends_with(
                admission
                    .strip_prefix("set -euo pipefail\n")
                    .context("admission errexit")?
            ),
            "same live check before/after proof"
        );
        ensure!(finalizer.contains("\"$EVIDENCE_RESULT\""));
    }
    Ok(())
}

fn live_pr() -> Value {
    json!({"state":"open", "draft":false, "updated_at":"2026-10-15T03:00:00Z",
        "head":{"sha":"head-a", "repo":{"full_name":"EffortlessMetrics/ub-review"}},
        "base":{"sha":"base-a", "ref":"main", "repo":{"full_name":"EffortlessMetrics/ub-review"}},
        "labels":[{"name":"merge-ready"}]})
}

#[cfg(unix)]
fn execute(
    script: &str,
    live: &str,
    action: &str,
    label: &str,
    updated: &str,
    result: &str,
    api_exit: &str,
) -> Result<bool> {
    let tmp = tempfile::tempdir()?;
    let command = format!(
        "gh() {{ test \"$*\" = 'api repos/EffortlessMetrics/ub-review/pulls/7' || return 9; printf '%s' \"$LIVE_PR\"; return \"$API_EXIT\"; }}\n{script}"
    );
    let output = std::process::Command::new("bash")
        .args(["-c", &command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("RUNNER_TEMP", tmp.path())
        .env("GITHUB_REPOSITORY", "EffortlessMetrics/ub-review")
        .env("EXPECTED_PR", "7")
        .env("EXPECTED_HEAD", "head-a")
        .env("EXPECTED_HEAD_REPO", "EffortlessMetrics/ub-review")
        .env("EXPECTED_BASE", "base-a")
        .env("EXPECTED_BASE_REF", "main")
        .env("EXPECTED_BASE_REPO", "EffortlessMetrics/ub-review")
        .env("CANDIDATE_SHA", "head-a")
        .env("EXPECTED_UPDATED", updated)
        .env("EVENT_ACTION", action)
        .env("EVENT_LABEL", label)
        .env("EVIDENCE_RESULT", result)
        .env("LIVE_PR", live)
        .env("API_EXIT", api_exit)
        .output()
        .context("execute native Bash/jq with local gh fixture")?;
    Ok(output.status.success())
}

#[cfg(unix)]
#[test]
fn native_live_checks_reject_stale_and_revoked_edges() -> Result<()> {
    let snapshot = "2026-10-15T03:00:00Z";
    for (name, _, final_step) in TEMPLATES {
        let source = workflow(name)?;
        for step in ["Check fresh label edge against live PR", final_step] {
            let script = shell(&source, step)?;
            let original = live_pr();
            ensure!(
                execute(
                    &script,
                    &original.to_string(),
                    "labeled",
                    "merge-ready",
                    snapshot,
                    "success",
                    "0"
                )?,
                "positive {name}/{step}"
            );
            for (field, changed) in [
                ("/head/sha", json!("head-b")),
                ("/head/repo/full_name", json!("other/fork")),
                ("/draft", json!(true)),
                ("/state", json!("closed")),
                ("/base/sha", json!("base-b")),
                ("/base/ref", json!("release")),
                ("/base/repo/full_name", json!("other/target")),
                ("/labels", json!([])),
                ("/updated_at", json!("2026-10-15T03:00:02Z")),
            ] {
                let mut live = original.clone();
                *live.pointer_mut(field).context("fixture field")? = changed;
                ensure!(
                    !execute(
                        &script,
                        &live.to_string(),
                        "labeled",
                        "merge-ready",
                        snapshot,
                        "success",
                        "0"
                    )?,
                    "reject {field} {name}/{step}"
                );
            }
            // Removal/reapplication at the same H: old edge rejects; new edge can qualify.
            let mut reapplied = original.clone();
            reapplied["updated_at"] = json!("2026-10-15T03:00:02Z");
            ensure!(!execute(
                &script,
                &reapplied.to_string(),
                "labeled",
                "merge-ready",
                snapshot,
                "success",
                "0"
            )?);
            ensure!(execute(
                &script,
                &reapplied.to_string(),
                "labeled",
                "merge-ready",
                "2026-10-15T03:00:02Z",
                "success",
                "0"
            )?);
            // Late non-edge deliveries and unrelated edits never acquire qualification.
            for (action, label) in [
                ("synchronize", ""),
                ("reopened", ""),
                ("ready_for_review", ""),
                ("edited", ""),
                ("closed", ""),
                ("unlabeled", "merge-ready"),
                ("labeled", "ci/default"),
                ("", ""),
            ] {
                ensure!(!execute(
                    &script,
                    &original.to_string(),
                    action,
                    label,
                    snapshot,
                    "success",
                    "0"
                )?);
            }
            ensure!(!execute(
                &script,
                &original.to_string(),
                "labeled",
                "merge-ready",
                snapshot,
                "success",
                "1"
            )?);
            ensure!(!execute(
                &script,
                "{",
                "labeled",
                "merge-ready",
                snapshot,
                "success",
                "0"
            )?);
            if step == final_step {
                for result in ["skipped", "cancelled", "failure", "", "unknown"] {
                    ensure!(
                        !execute(
                            &script,
                            &original.to_string(),
                            "labeled",
                            "merge-ready",
                            snapshot,
                            result,
                            "0"
                        )?,
                        "reject {result} {name}"
                    );
                }
            }
        }
    }
    Ok(())
}
