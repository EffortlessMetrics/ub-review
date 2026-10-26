//! Structural Actions predicates plus execution of the actual native live checks.
//! The gh fixture is local; this does not emulate GitHub event scheduling.
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};

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
    let value = tail
        .split_once("        run: ")
        .context("native shell body")?
        .1;
    let Some(body) = value.strip_prefix("|\n") else {
        return Ok(value
            .lines()
            .next()
            .context("inline native shell")?
            .to_owned());
    };
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
        ensure!(candidate.contains("      observation: ${{ steps.current.outputs.observation }}"));
        ensure!(candidate.contains("      admitted: ${{ steps.current.outputs.admitted }}"));
        ensure!(candidate.contains(&format!("  {job}:\n    needs: admission\n")));
        ensure!(candidate.contains("needs.admission.outputs.admitted == 'true'"));
        ensure!(!candidate.contains("EXPECTED_UPDATED"));
        ensure!(!candidate.contains(".updated_at =="));
        ensure!(candidate.contains("cancel-in-progress: false"));
        ensure!(candidate.contains("&& 'qualification' || format('observe-{0}', github.run_id)"));
        ensure!(!candidate.contains("checks: write"));
        ensure!(!candidate.contains("pull-requests: write"));
        ensure!(candidate.contains("persist-credentials: false"));
        // Preserve the whole evidence worker, including its matrix, permissions,
        // checkout subject and credential isolation before the first check.
        let worker = active
            .split_once(&format!("  {job}:\n"))
            .context("active evidence worker")?
            .1;
        let end = if job == "gate" {
            "  # Coverage publication"
        } else {
            "  finalize:"
        };
        let body = worker
            .split_once("    runs-on:")
            .context("active proof start")?
            .1
            .split_once(end)
            .context("active proof end")?
            .0;
        ensure!(candidate.contains(body), "preserve evidence worker {name}");
        let native_name = if job == "gate" {
            "ub-review/gate"
        } else {
            "ub-review/independent-baseline"
        };
        ensure!(candidate.contains(&format!("    name: ${{{{ needs.admission.outputs.observation == 'true' && 'admission observation' || '{native_name}' }}}}\n    needs: [admission, {job}]\n    if: ${{{{ always() }}}}")));
        if job == "gate" {
            ensure!(candidate.contains("  workflow_dispatch:\n"));
            ensure!(
                candidate.contains("  pull_request_review:\n    types: [submitted, dismissed]")
            );
            ensure!(candidate.contains("needs.gate.result != 'skipped' && always()"));
        }
        let admission = shell(&candidate, "Classify event and check current PR")?;
        let finalizer = shell(&candidate, final_step)?;
        ensure!(admission.contains("if [[ \"${EVENT_LABEL,,}\" != merge-ready ]]"));
        ensure!(finalizer.contains("test \"$EVIDENCE_RESULT\" = success"));
        ensure!(finalizer.contains("test \"$WORKFLOW_CANCELLED\" = false"));
        ensure!(
            candidate.contains(
                "WORKFLOW_CANCELLED: ${{ steps.cancellation.outputs.cancelled == 'true' }}"
            )
        );
        ensure!(candidate.contains("      - name: Capture cancellation before finalization\n        id: cancellation\n        if: ${{ cancelled() }}"));
        ensure!(candidate.contains("      - name: Reject workflow cancellation during finalization\n        if: ${{ cancelled() }}\n        shell: bash\n        run: exit 1"));
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
struct Execution {
    success: bool,
    admitted: bool,
    observation: bool,
    cancellation_captured: bool,
}

#[cfg(unix)]
fn execute(
    script: &str,
    live: &Value,
    action: &str,
    label: &str,
    result: &str,
    api_exit: &str,
) -> Result<Execution> {
    let tmp = tempfile::tempdir()?;
    let output_file = tmp.path().join("outputs");
    let command = format!(
        r#"gh() {{
      if [[ "$*" == 'api repos/EffortlessMetrics/ub-review/pulls/7' ]]; then printf '%s' "$LIVE_PR";
      elif [[ "$*" == 'pr view 7 --repo EffortlessMetrics/ub-review --json reviewDecision' ]]; then printf '%s' "$LIVE_REVIEW";
      else return 9; fi
      return "$API_EXIT"
    }}
    {script}"#
    );
    let decision = live
        .get("fixture_review_decision")
        .cloned()
        .unwrap_or(json!("APPROVED"));
    let output = std::process::Command::new("bash")
        .args(["-c", &command])
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("RUNNER_TEMP", tmp.path())
        .env("GITHUB_OUTPUT", &output_file)
        .env("GITHUB_REPOSITORY", "EffortlessMetrics/ub-review")
        .env("EXPECTED_PR", "7")
        .env(
            "EXPECTED_HEAD",
            live["fixture_expected_head"].as_str().unwrap_or("head-a"),
        )
        .env("EXPECTED_HEAD_REPO", "EffortlessMetrics/ub-review")
        .env(
            "EXPECTED_BASE",
            live["fixture_expected_base"].as_str().unwrap_or("base-a"),
        )
        .env(
            "EXPECTED_BASE_REF",
            live["fixture_expected_ref"].as_str().unwrap_or("main"),
        )
        .env("EXPECTED_BASE_REPO", "EffortlessMetrics/ub-review")
        .env("CANDIDATE_SHA", "head-a")
        .env(
            "EVENT_REVIEW_STATE",
            live["fixture_review_state"].as_str().unwrap_or(""),
        )
        .env(
            "EVENT_NAME",
            live["fixture_event_name"]
                .as_str()
                .unwrap_or("pull_request"),
        )
        .env(
            "EVENT_RETARGET",
            if live["fixture_retarget"] == true {
                "true"
            } else {
                "false"
            },
        )
        .env(
            "WORKFLOW_CANCELLED",
            if live["fixture_cancelled"] == true {
                "true"
            } else {
                "false"
            },
        )
        .env(
            "OBSERVATION_ONLY",
            if live["fixture_observation"] == true {
                "true"
            } else {
                "false"
            },
        )
        .env("EVENT_ACTION", action)
        .env("EVENT_LABEL", label)
        .env("EVIDENCE_RESULT", result)
        .env("LIVE_PR", live.to_string())
        .env(
            "LIVE_REVIEW",
            json!({"reviewDecision":decision}).to_string(),
        )
        .env("API_EXIT", api_exit)
        .output()
        .context("execute native Bash/jq with local gh fixture")?;
    let outputs = if output_file.exists() {
        std::fs::read_to_string(output_file)?
    } else {
        String::new()
    };
    Ok(Execution {
        success: output.status.success(),
        admitted: outputs.lines().any(|line| line == "admitted=true"),
        observation: outputs.lines().any(|line| line == "observation=true"),
        cancellation_captured: outputs.lines().any(|line| line == "cancelled=true"),
    })
}

#[cfg(unix)]
#[test]
fn material_live_checks_and_harmless_observations() -> Result<()> {
    for (name, _, final_step) in TEMPLATES {
        let source = workflow(name)?;
        let admission = shell(&source, "Classify event and check current PR")?;
        let finalizer = shell(&source, final_step)?;
        let original = live_pr();
        let fresh = execute(
            &admission,
            &original,
            "labeled",
            "merge-ready",
            "success",
            "0",
        )?;
        ensure!(
            fresh.success && fresh.admitted && !fresh.observation,
            "fresh edge {name}"
        );
        ensure!(
            execute(
                &finalizer,
                &original,
                "labeled",
                "merge-ready",
                "success",
                "0"
            )?
            .success
        );
        // Current material/readiness changes reject proof, even when evidence succeeded.
        for (field, changed) in [
            ("/head/sha", json!("head-b")),
            ("/head/repo/full_name", json!("other/fork")),
            ("/draft", json!(true)),
            ("/state", json!("closed")),
            ("/base/sha", json!("base-b")),
            ("/base/ref", json!("release")),
            ("/base/repo/full_name", json!("other/target")),
            ("/labels", json!([])),
        ] {
            let mut live = original.clone();
            *live.pointer_mut(field).context("fixture field")? = changed;
            ensure!(
                !execute(&finalizer, &live, "labeled", "merge-ready", "success", "0")?.success,
                "reject {field} {name}"
            );
            ensure!(
                !execute(&admission, &live, "labeled", "merge-ready", "success", "0")?.admitted
            );
        }
        // Harmless comments/labels can move updated_at without invalidating live proof.
        let mut harmless = original.clone();
        harmless["updated_at"] = json!("2026-10-15T03:00:02Z");
        harmless["body"] = json!("edited description");
        harmless["labels"] = json!([{"name":"merge-ready"},{"name":"ci/default"}]);
        ensure!(
            execute(
                &finalizer,
                &harmless,
                "labeled",
                "merge-ready",
                "success",
                "0"
            )?
            .success
        );
        for (action, label) in [
            ("labeled", "ci/default"),
            ("unlabeled", "ci/default"),
            ("edited", ""),
            ("commented", ""),
        ] {
            let observation = execute(&admission, &harmless, action, label, "skipped", "0")?;
            ensure!(
                observation.success && observation.observation && !observation.admitted,
                "harmless {action}/{label} {name}"
            );
            if action != "commented" {
                let offline = execute(&admission, &harmless, action, label, "skipped", "1")?;
                ensure!(
                    offline.success && offline.observation && !offline.admitted,
                    "metadata avoids failed API {name}"
                );
            }
            let mut routed = harmless.clone();
            routed["fixture_observation"] = json!(true);
            ensure!(execute(&finalizer, &routed, action, label, "skipped", "0")?.success);
        }
        // Relevant transitions fail the protected result without allocating heavy proof.
        for action in ["synchronize", "opened", "reopened", "ready_for_review"] {
            let outcome = execute(&admission, &original, action, "", "skipped", "0")?;
            ensure!(outcome.success && !outcome.observation && !outcome.admitted);
            ensure!(!execute(&finalizer, &original, action, "", "skipped", "0")?.success);
        }
        for (action, field, changed) in [
            ("unlabeled", "/labels", json!([])),
            ("converted_to_draft", "/draft", json!(true)),
            ("closed", "/state", json!("closed")),
        ] {
            let mut live = original.clone();
            *live.pointer_mut(field).context("revocation field")? = changed;
            let revoked = execute(&admission, &live, action, "merge-ready", "skipped", "0")?;
            ensure!(
                !revoked.observation && !revoked.admitted,
                "revocation {action} {name}"
            );
            ensure!(!execute(&finalizer, &live, action, "merge-ready", "skipped", "0")?.success);
        }
        let mut retarget = original.clone();
        retarget["base"]["ref"] = json!("release");
        retarget["fixture_expected_ref"] = json!("release");
        retarget["fixture_retarget"] = json!(true);
        let changed = execute(&admission, &retarget, "edited", "", "skipped", "0")?;
        ensure!(
            changed.success && !changed.observation && !changed.admitted,
            "retarget same H {name}"
        );
        // Late withdrawals after readiness is restored do not replace valid proof.
        let late = execute(
            &admission,
            &original,
            "unlabeled",
            "merge-ready",
            "skipped",
            "0",
        )?;
        ensure!(late.observation && !late.admitted);
        ensure!(
            execute(
                &finalizer,
                &original,
                "labeled",
                "merge-ready",
                "success",
                "0"
            )?
            .success
        );
        let mut review = original.clone();
        review["fixture_event_name"] = json!("pull_request_review");
        for decision in [json!("APPROVED"), json!("REVIEW_REQUIRED"), Value::Null] {
            review["fixture_review_decision"] = decision;
            let quiet = execute(&admission, &review, "submitted", "", "skipped", "0")?;
            ensure!(quiet.observation && !quiet.admitted);
        }
        review["fixture_review_state"] = json!("commented");
        let comment = execute(&admission, &review, "submitted", "", "skipped", "1")?;
        ensure!(
            comment.success && comment.observation && !comment.admitted,
            "review comment avoids API invalidation"
        );
        review["fixture_review_state"] = json!("changes_requested");
        review["fixture_review_decision"] = json!("CHANGES_REQUESTED");
        let blocked = execute(&admission, &review, "submitted", "", "skipped", "0")?;
        ensure!(!blocked.observation && !blocked.admitted);
        ensure!(
            !execute(
                &finalizer,
                &review,
                "labeled",
                "merge-ready",
                "success",
                "0"
            )?
            .success
        );
        ensure!(
            !execute(
                &admission,
                &review,
                "labeled",
                "merge-ready",
                "success",
                "0"
            )?
            .admitted
        );
        for result in ["skipped", "cancelled", "failure", "", "unknown"] {
            ensure!(
                !execute(&finalizer, &original, "labeled", "merge-ready", result, "0")?.success
            );
        }
        // Cancellation of this workflow, independently of predecessor success.
        let capture = execute(
            &shell(&source, "Capture cancellation before finalization")?,
            &original,
            "labeled",
            "merge-ready",
            "success",
            "0",
        )?;
        ensure!(capture.success && capture.cancellation_captured);
        let mut cancelled = original.clone();
        cancelled["fixture_cancelled"] = json!(true);
        ensure!(
            !execute(
                &finalizer,
                &cancelled,
                "labeled",
                "merge-ready",
                "success",
                "0"
            )?
            .success
        );
        ensure!(
            !execute(
                &shell(&source, "Reject workflow cancellation during finalization")?,
                &original,
                "labeled",
                "merge-ready",
                "success",
                "0"
            )?
            .success
        );
        ensure!(
            !execute(
                &finalizer,
                &original,
                "labeled",
                "merge-ready",
                "success",
                "1"
            )?
            .success
        );
        let malformed = execute(
            &admission,
            &Value::Null,
            "labeled",
            "merge-ready",
            "success",
            "0",
        )?;
        ensure!(!malformed.success && !malformed.observation && !malformed.admitted);
        let unavailable = execute(
            &admission,
            &original,
            "labeled",
            "merge-ready",
            "success",
            "1",
        )?;
        ensure!(!unavailable.success && !unavailable.observation && !unavailable.admitted);
        review["fixture_review_decision"] = json!("unknown");
        ensure!(
            !execute(
                &finalizer,
                &review,
                "labeled",
                "merge-ready",
                "success",
                "0"
            )?
            .success
        );
    }
    Ok(())
}
