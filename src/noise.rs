//! Noise predicates and finding classification (cleanup train step 23b,
//! pure code motion). Verification/refutation/calibration classification,
//! PR body noise detection (workflow trust, tool status, meta-review),
//! and summary-finding ranking/dedupe.

use crate::*;

pub(crate) fn is_verification_question(finding: &SummaryOnlyFinding) -> bool {
    let text = finding.reason.to_ascii_lowercase();
    text.contains("verify")
        || text.contains("verification")
        || text.contains("confirm")
        || text.contains("question")
        || text.contains("witness")
        || text.contains("red/green")
        || text.contains("proof")
        || text.contains("prove")
        || text.contains("proves")
        || text.contains("proven")
}

pub(crate) fn is_verification_observation(observation: &ObservationGroup) -> bool {
    observation.kind == "verification-question"
        || matches!(observation.kind.as_str(), "test-gap" | "source-route-gap")
        || text_is_verification_question(&observation.claim)
}

pub(crate) fn is_refuted_observation(observation: &ObservationGroup) -> bool {
    observation.status == "refuted"
        || matches!(
            observation.kind.as_str(),
            "false-premise" | "resolved-check"
        )
}

pub(crate) fn is_pr_body_refuted_observation(observation: &ObservationGroup) -> bool {
    if observation.kind == "resolved-check" {
        return false;
    }
    is_refuted_observation(observation) && !is_global_calibration_refutation(observation)
}

pub(crate) fn is_refutation_confirmation_observation(observation: &ObservationGroup) -> bool {
    is_refuted_observation(observation) && !is_global_calibration_refutation(observation)
}

pub(crate) fn is_pr_body_artifact_only_observation(observation: &ObservationGroup) -> bool {
    let text =
        format!("{} {}", observation.claim, observation.evidence.join(" ")).to_ascii_lowercase();
    observation.status == "covered"
        || observation.kind == "resolved-check"
        || observation.dedupe_key.starts_with("lane-output-shape")
        || observation
            .dedupe_key
            .starts_with("lane-output-malformed-content")
        || (observation.kind == "bug" && text.contains("lane model summary"))
        || text.contains("inline guard rejected ")
        || text.contains("severity_allowed=")
        || text.contains("confidence_allowed=")
        || (text.contains("no permissions")
            && (text.contains("no new auth surface") || text.contains("no new token scope")))
        || (text.contains("no permissions block") && text.contains("no pull_request_target"))
        || (text.contains("supply-chain tightening") && text.contains("no new scope"))
        || (text.contains("out-of-hunk")
            && (text.contains("cursor")
                || text.contains("push-not-synchronize")
                || text.contains("pull_request")))
        || (text.contains("full 40-hex")
            && (text.contains("prefix collision") || text.contains("short-prefix")))
        || (text.contains("actionlint") && text.contains("sensor reports ok"))
        || (text.contains("actionlint") && text.contains("status=ok"))
        || is_unchanged_workflow_trust_posture_noise(&text)
        || is_no_finding_workflow_pin_summary_noise(&text)
        || is_stale_external_bot_objection_noise(&text)
        || is_workflow_tool_status_artifact_gap_noise(&text)
        || is_workflow_paths_ignore_no_posture_noise(&text)
        || is_actionlint_semantic_skip_proof_noise(&text)
        || is_non_workflow_verifier_scope_noise(&text)
        || is_self_test_meta_review_noise(&text)
        || is_current_pin_consistency_followup_noise(&text)
        || is_workflow_pin_lockstep_no_value_summary_noise(&text)
        || is_pr_body_meta_review_noise(&text)
        || (observation.kind == "false-premise"
            && (text.contains("short-prefix")
                || (text.contains("cache key") && text.contains("full 40-hex"))
                || (text.contains("supply-chain") && text.contains("sha pin"))
                || text.contains("floating @v0.1")
                || text.contains("pinning to a sha")
                || text.contains("pinning to immutable commit sha")
                || (text.contains("scope change") && text.contains("supply-chain tightening"))))
        || (is_missing_evidence_observation(observation) && is_tool_status_only_gap(&text))
}

pub(crate) fn is_global_calibration_refutation(observation: &ObservationGroup) -> bool {
    observation.dedupe_key == BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY
        && observation.path.is_none()
        && observation
            .sources
            .iter()
            .any(|source| source == "model-false-premise-guard")
}

pub(crate) fn is_missing_evidence_observation(observation: &ObservationGroup) -> bool {
    observation.kind == "missing-evidence"
}

pub(crate) fn is_tool_status_only_gap(text: &str) -> bool {
    (text.contains("sensor `") || text.contains(" sensor ") || text.contains("sensors:"))
        && (text.contains("missing")
            || text.contains("command not found")
            || text.contains("disabled"))
        && !text.contains("base+tests")
        && !text.contains("red/green")
        && !text.contains("regression test")
        && !text.contains("changed-line coverage")
}

pub(crate) fn is_pr_body_meta_review_noise(text: &str) -> bool {
    is_internal_review_machinery_text(text)
        || text.contains("cached prior observation")
        || text.contains("refuter demoted inline candidate")
        || text.contains("gate proof is pending")
        || text.contains("cannot perform from cached context")
        || text.contains("commit-existence/ancestry proof")
        || text.contains("upstream commit-existence")
        || text.contains("general bot output")
        || (text.contains("the refutation claiming")
            && text.contains("still matches current evidence"))
        || is_gap_noise_meta_review_noise(text)
        || (text.contains("pr-body contract hardening")
            && text.contains("not verifiable from the repo diff"))
        || (text.contains("cache key/uses ref")
            && text.contains("40 hex")
            && text.contains("non-zero"))
        || (text.contains("sha were 39-hex") && text.contains("all-zero"))
        || is_self_test_meta_review_noise(text)
        || is_checkout_persistence_no_change_noise(text)
        || text.contains("actionlint ran ok")
}

pub(crate) fn is_internal_review_machinery_text(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "duplicate inline",
        "merged into path",
        "lane conflict",
        "cross-lane conflict",
        "resolve cross-lane",
        "inline plan",
        "comment plan",
    ]
    .iter()
    .any(|needle| text.contains(needle))
}

pub(crate) fn is_checkout_persistence_no_change_noise(text: &str) -> bool {
    (text.contains("checkout credential persistence")
        || text.contains("checkout config")
        || text.contains("persist-credentials"))
        && (text.contains("did not change checkout")
            || text.contains("does not change checkout")
            || text.contains("no new persistence vector")
            || text.contains("read-only github_token"))
}

pub(crate) fn is_unchanged_workflow_trust_posture_noise(text: &str) -> bool {
    let mentions_workflow_trust = text.contains("upstream trust")
        || text.contains("upstream sha trust")
        || text.contains("trust in upstream")
        || text.contains("malicious or compromised")
        || text.contains("would exfiltrate")
        || text.contains("reproducibly verified")
        || text.contains("repo-level policy item")
        || text.contains("secrets.minimax")
        || text.contains("github.token")
        || text.contains("workflow-level permissions")
        || text.contains("permissions block")
        || text.contains("exposure surface");
    let says_unchanged_or_out_of_scope = text.contains("not introduced by this")
        || text.contains("pre-existing")
        || text.contains("not a diff target")
        || text.contains("identical to prior")
        || text.contains("identical in posture")
        || text.contains("no widened attack surface")
        || text.contains("zero new secret")
        || text.contains("zero new")
        || text.contains("not a diff finding")
        || text.contains("not a diff-introduced")
        || text.contains("no permission/trigger/pinning posture change")
        || text.contains("no permission")
        || text.contains("no permissions")
        || text.contains("unchanged")
        || text.contains("standing-repo concern")
        || text.contains("standing repo concern");
    mentions_workflow_trust && says_unchanged_or_out_of_scope
}

pub(crate) fn is_workflow_trust_posture_review_noise(text: &str) -> bool {
    is_unchanged_workflow_trust_posture_noise(text)
        || ((text.contains("does not eliminate upstream trust")
            || text.contains("trust in upstream tag"))
            && (text.contains("secrets.minimax")
                || text.contains("github.token")
                || text.contains("malicious or compromised")))
        || is_no_finding_workflow_pin_summary_noise(text)
        || is_stale_external_bot_objection_noise(text)
        || is_workflow_tool_status_artifact_gap_noise(text)
        || is_workflow_paths_ignore_no_posture_noise(text)
        || is_actionlint_semantic_skip_proof_noise(text)
        || is_current_pin_consistency_followup_noise(text)
        || is_workflow_pin_lockstep_no_value_summary_noise(text)
}

pub(crate) fn is_no_finding_workflow_pin_summary_noise(text: &str) -> bool {
    let mentions_pin = text.contains("pinning")
        || text.contains("sha-pinning")
        || text.contains("sha bump")
        || text.contains("sha swap")
        || text.contains("mechanical sha")
        || text.contains("action uses")
        || text.contains("uses: ref")
        || text.contains("cache key")
        || text.contains("per-action full-sha")
        || text.contains("40-hex")
        || text.contains("all-zero");
    let says_no_defect = text.contains("no pinning defect introduced")
        || text.contains("no actionable finding")
        || text.contains("nothing to pin-review")
        || text.contains("pinning posture preserved")
        || text.contains("sha-pinning control remains effective")
        || text.contains("sha-pinning control is effective")
        || text.contains("old pin fully absent")
        || text.contains("pin is 40-hex non-zero")
        || text.contains("matches expected sha-1 shape")
        || text.contains("pin shape valid 40-hex");
    let says_not_current_diff = text.contains("not a diff finding")
        || text.contains("not a diff-introduced")
        || text.contains("not introduced by this")
        || text.contains("identical in posture")
        || text.contains("byte-identical")
        || text.contains("repo-level policy item")
        || text.contains("no action versions")
        || text.contains("no action references")
        || text.contains("no workflow yaml")
        || text.contains("no github actions yaml")
        || text.contains("unchanged from prior pin")
        || text.contains("net new secret/permission surface")
        || text.contains("net new secret surface")
        || text.contains("no new permission")
        || text.contains("no permission, token-scope")
        || text.contains("no blocker introduced");
    mentions_pin && (says_no_defect || says_not_current_diff)
}

pub(crate) fn is_non_workflow_verifier_scope_noise(text: &str) -> bool {
    let verifier_script = text.contains("scripts/verify-bun-review-artifacts.py")
        || text.contains("python verifier")
        || text.contains("python-only")
        || text.contains("python script");
    let workflow_not_changed = text.contains("no .github/workflows")
        || text.contains("no github actions yaml")
        || text.contains("no workflow yaml")
        || text.contains("no workflow file")
        || text.contains("no workflow files changed")
        || text.contains("no action versions")
        || text.contains("no action references")
        || text.contains("no actions, reusable workflows")
        || text.contains("no workflow trigger")
        || text.contains("diff does not modify any github actions yaml");
    let says_scope_only = text.contains("actionlint")
        || text.contains("zizmor")
        || text.contains("pinning")
        || text.contains("permissions")
        || text.contains("token-scope")
        || text.contains("out of scope")
        || text.contains("not applicable")
        || text.contains("not a trust gap")
        || text.contains("no actionable finding")
        || text.contains("nothing to pin-review")
        || text.contains("surfaces are limited")
        || text.contains("validator script itself");
    verifier_script && workflow_not_changed && says_scope_only
}

pub(crate) fn is_self_test_meta_review_noise(text: &str) -> bool {
    let mentions_self_test = text.contains("self-test")
        || text.contains("run_self_tests")
        || text.contains("--self-test")
        || text.contains("tempfile.temporarydirectory");
    let says_meta = text.contains("receipt not in seeded thread")
        || text.contains("pr body asserts")
        || text.contains("focused smoke proof pattern")
        || text.contains("suitable for python change verification")
        || text.contains("confirm new self-tests")
        || text.contains("if --self-test is not executed in ci");
    mentions_self_test && says_meta
}

pub(crate) fn is_stale_external_bot_objection_noise(text: &str) -> bool {
    let mentions_bots =
        text.contains("cursor[bot]") || text.contains("coderabbit") || text.contains("stale-bot");
    let says_stale_false_positive = (text.contains("stale") || text.contains("false positive"))
        && (text.contains("false positive")
            || text.contains("reopens nothing")
            || text.contains("not real findings")
            || text.contains("current diff")
            || text.contains("live diff"));
    let contradicted_target_advice = (text.contains("different sha")
        || text.contains("targeting a different sha")
        || text.contains("0 references to")
        || text.contains("scripted check showing 0 references")
        || text.contains("not match to gate target"))
        && (text.contains("used in the diff") || text.contains("current diff"));
    let mentions_pin_ref_mismatch = text.contains("claim target")
        || text.contains("pin mismatch")
        || text.contains("target sha")
        || text.contains("current head sha")
        || text.contains("pr title")
        || text.contains("pr body");
    mentions_bots
        && mentions_pin_ref_mismatch
        && (says_stale_false_positive || contradicted_target_advice)
}

pub(crate) fn is_workflow_tool_status_artifact_gap_noise(text: &str) -> bool {
    let actionlint_ok = text.contains("actionlint")
        && (text.contains("receipt is 'ok'")
            || text.contains("actionlint=ok")
            || text.contains("actionlint receipt ok")
            || text.contains("sensor table"));
    let not_inlined = text.contains("no per-line output")
        || text.contains("not inlined")
        || text.contains("central proof broker artifact")
        || text.contains("sensors/actionlint");
    let yaml_pin = text.contains("4-line workflow pin")
        || text.contains("4-line sha-swap")
        || text.contains("yaml-only")
        || text.contains("pin/uses ref consistent");
    let skipped_heavy = text.contains("build/test skipped")
        || text.contains("--allow-heavy")
        || text.contains("no fresh pr-build smoke")
        || text.contains("heavy smoke adds limited value");
    let disabled_workflow_tools = (text.contains("zizmor")
        || text.contains("gitleaks")
        || text.contains("osv-scanner")
        || text.contains("cargo-audit")
        || text.contains("cargo-deny")
        || text.contains("shellcheck")
        || text.contains("semgrep")
        || text.contains("coverage"))
        && (text.contains("disabled by config") || text.contains("trigger-mismatched"))
        && (text.contains("workflow file") || text.contains("security/pinning tool"));
    let local_actionlint_gap = text.contains("actionlint")
        && text.contains("not installed locally")
        && (text.contains("local pre-push run") || text.contains("ub-review gate"));
    let non_workflow_lint_skip = text.contains("actionlint")
        && (text.contains("zizmor") || text.contains("shellcheck"))
        && (text.contains("skipped") || text.contains("disabled"))
        && (text.contains("no .github diff")
            || text.contains("no github actions yaml")
            || text.contains("no workflow")
            || text.contains("consumer workflow")
            || text.contains("invokes this script")
            || text.contains("no yaml in diff"));
    (actionlint_ok && (not_inlined || yaml_pin))
        || (skipped_heavy && yaml_pin)
        || disabled_workflow_tools
        || local_actionlint_gap
        || non_workflow_lint_skip
        || ((text.contains("parked follow-up") || text.contains("not a blocker"))
            && actionlint_ok
            && yaml_pin)
}

pub(crate) fn is_gap_noise_meta_review_noise(text: &str) -> bool {
    let mentions_gap_noise =
        text.contains("gap-noise") || text.contains("is_workflow_tool_status_artifact_gap_noise");
    let mentions_meta_surface = text.contains("observation text")
        || text.contains("observation string")
        || text.contains("string literal")
        || text.contains("trust_language_softening")
        || text.contains("trust-language softening")
        || text.contains("substring-based matching");
    let mentions_softening = text.contains("softened")
        || text.contains("softening")
        || text.contains("not trust-affecting")
        || text.contains("absence of proof");
    mentions_gap_noise && mentions_meta_surface && mentions_softening
}

pub(crate) fn is_workflow_paths_ignore_no_posture_noise(text: &str) -> bool {
    let mentions_paths_ignore = text.contains("paths-ignore") || text.contains("path-ignore");
    let mentions_workflow_posture = text.contains("token scopes")
        || text.contains("permissions block")
        || text.contains("permission expansion")
        || text.contains("job-level security context")
        || text.contains("trigger activation")
        || text.contains("pull_request_target")
        || text.contains("checkout")
        || text.contains("semantic skip behavior")
        || text.contains("focused smoke proof")
        || text.contains("workflow_run")
        || text.contains("droid noise");
    let says_no_posture_change = text.contains("only filters trigger activation")
        || text.contains("does not alter")
        || text.contains("no new trigger")
        || text.contains("no new persistence vector")
        || text.contains("not modified in this pr")
        || text.contains("diff only mutates a paths-ignore")
        || text.contains("not proven by sensors")
        || text.contains("trust rests on actionlint parse")
        || (text.contains("future pr") && text.contains("re-trigger droid"))
        || text.contains("ub gate is the authoritative review")
        || (text.contains("future rename") && text.contains("re-enable"));
    mentions_paths_ignore && mentions_workflow_posture && says_no_posture_change
}

pub(crate) fn is_actionlint_semantic_skip_proof_noise(text: &str) -> bool {
    let mentions_actionlint_skip = text.contains("actionlint")
        && (text.contains("semantic skip behavior")
            || (text.contains("skip behavior") && text.contains("droid")));
    let says_proof_is_not_decisive = text.contains("no semantic proof")
        || text.contains("trust rests on actionlint parse")
        || text.contains("unproven beyond actionlint parse")
        || text.contains("not proven by sensors")
        || text.contains("no focused smoke proof");
    let scoped_to_auxiliary_lane = text.contains("droid lane")
        || text.contains("droid")
        || text.contains("auxiliary/non-blocking")
        || text.contains("ub gate is authoritative")
        || text.contains("ub gate is the authoritative");
    mentions_actionlint_skip && says_proof_is_not_decisive && scoped_to_auxiliary_lane
}

pub(crate) fn is_current_pin_consistency_followup_noise(text: &str) -> bool {
    let mentions_cache_pin = (text.contains("cache key")
        || text.contains("restore-keys")
        || text.contains("cache restore"))
        && (text.contains("action sha") || text.contains("repin") || text.contains("uses:"));
    let says_future_or_parked = text.contains("future repin")
        || text.contains("future pin")
        || text.contains("partial repin")
        || text.contains("parked for follow-up")
        || text.contains("parked for lint-rule")
        || text.contains("lint-rule follow-up")
        || text.contains("follow-up lint rule")
        || text.contains("follow-up lint")
        || text.contains("lint rule or script");
    let says_currently_consistent = text.contains("current state is consistent")
        || text.contains("current state consistent")
        || text.contains("current pr state is consistent")
        || text.contains("not actionable in this pr");
    mentions_cache_pin && says_future_or_parked && says_currently_consistent
}

pub(crate) fn is_workflow_pin_lockstep_no_value_summary_noise(text: &str) -> bool {
    let workflow_scope = text.contains("workflow")
        || text.contains("ub-review")
        || text.contains("actionlint")
        || text.contains("paths-ignore")
        || text.contains("droid");
    let mentions_lockstep_pin = text.contains("pin lockstep")
        || text.contains("lockstep sha pin")
        || text.contains("pin bump is lockstep")
        || text.contains("pin/uses ref consistent")
        || (text.contains("cache key/restore-keys")
            && (text.contains("prefix match")
                || text.contains("prefix is coupled")
                || text.contains("updated in lockstep")
                || text.contains("must be updated in lockstep")))
        || (text.contains("cache key")
            && text.contains("restore-keys")
            && text.contains("uses:")
            && text.contains("lockstep"));
    let says_no_current_issue = text.contains("old pin absent")
        || text.contains("current state is consistent")
        || text.contains("current state consistent")
        || text.contains("current pr state is consistent")
        || text.contains("no blocker")
        || text.contains("not a blocker")
        || text.contains("no other third-party actions changed")
        || text.contains("no syntactic regression")
        || text.contains("no source, no permissions, no token, no checkout changes")
        || (text.contains("no new")
            && (text.contains("permission")
                || text.contains("token")
                || text.contains("third-party action")
                || text.contains("checkout")));
    workflow_scope && mentions_lockstep_pin && says_no_current_issue
}

pub(crate) fn is_residual_risk_observation(observation: &ObservationGroup) -> bool {
    observation.kind == "residual-risk"
}

pub(crate) fn pr_review_heading_for_diff_class(diff_class: DiffClass) -> &'static str {
    match diff_class {
        DiffClass::SourceUb => "UB Review",
        DiffClass::SourceGeneral => "Source Review",
        DiffClass::TestsOnly => "Test Review",
        DiffClass::WorkflowTooling => "Workflow Review",
        DiffClass::DocsOnly => "Docs Review",
        DiffClass::ArtifactOnlySmoke => "Review Packet",
    }
}

pub(crate) fn residual_risk_for_diff_class(diff_class: DiffClass) -> &'static str {
    match diff_class {
        DiffClass::SourceUb => {
            "Artifact audit note: unsafe/native route truth, oracle strength, and unavailable evidence remain tracked in artifacts."
        }
        DiffClass::SourceGeneral => {
            "Artifact audit note: changed behavior, route truth, test strength, and unavailable evidence remain tracked in artifacts."
        }
        DiffClass::TestsOnly => {
            "Artifact audit note: red/green discrimination, oracle strength, flake risk, and unavailable proof evidence remain tracked in artifacts."
        }
        DiffClass::WorkflowTooling => {
            "Artifact audit note: workflow permissions, trigger safety, action pinning, checkout credentials, fork behavior, and actionlint/zizmor evidence remain tracked in artifacts."
        }
        DiffClass::DocsOnly => {
            "Artifact audit note: claim accuracy, links, examples, and unavailable evidence remain tracked in artifacts."
        }
        DiffClass::ArtifactOnlySmoke => {
            "Artifact audit note: packet completeness and unavailable evidence remain tracked in artifacts."
        }
    }
}

pub(crate) fn is_parked_observation(observation: &ObservationGroup) -> bool {
    observation.status == "parked" || observation.kind == "parked-follow-up"
}

pub(crate) fn text_is_verification_question(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("verify")
        || text.contains("verification")
        || text.contains("confirm")
        || text.contains("question")
        || text.contains("witness")
        || text.contains("red/green")
        || text.contains("proof")
        || text.contains("prove")
        || text.contains("proves")
        || text.contains("proven")
}

pub(crate) fn unique_summary_review_findings<'a>(
    findings: impl IntoIterator<Item = &'a SummaryOnlyFinding>,
) -> Vec<&'a SummaryOnlyFinding> {
    let mut unique = Vec::<&SummaryOnlyFinding>::new();
    let mut indexes = BTreeMap::<String, usize>::new();

    for finding in findings {
        let key = summary_finding_review_dedupe_key(finding);
        let index = indexes.get(&key).copied().or_else(|| {
            unique
                .iter()
                .position(|existing| review_claims_match(&existing.reason, &finding.reason))
        });
        if let Some(index) = index {
            if summary_finding_rank(finding) > summary_finding_rank(unique[index]) {
                unique[index] = finding;
            }
        } else {
            indexes.insert(key, unique.len());
            unique.push(finding);
        }
    }

    unique
}

pub(crate) fn summary_finding_review_dedupe_key(finding: &SummaryOnlyFinding) -> String {
    let normalized = normalized_review_text(&reviewer_facing_pr_text(&finding.reason));
    if normalized.chars().count() >= 24 {
        normalized
    } else {
        format!("{}:{normalized}", finding.lane)
    }
}

pub(crate) fn summary_finding_rank(finding: &SummaryOnlyFinding) -> (u8, u8) {
    (
        severity_rank(&finding.severity),
        confidence_rank(&finding.confidence),
    )
}

pub(crate) fn summary_finding_matches_observations(
    finding: &SummaryOnlyFinding,
    observations: &[ObservationGroup],
) -> bool {
    let summary = normalized_review_text(&format!("{} {}", finding.reason, finding.evidence));
    observations.iter().any(|observation| {
        let claim = normalized_review_text(&observation.claim);
        (claim.len() >= 24 && (summary.contains(&claim) || claim.contains(&summary)))
            || review_claims_match(&summary, &observation.claim)
    })
}

pub(crate) fn review_claims_match(left: &str, right: &str) -> bool {
    let left = normalized_review_text(&reviewer_facing_pr_text(left));
    let right = normalized_review_text(&reviewer_facing_pr_text(right));
    if left.chars().count() < 16 || right.chars().count() < 16 {
        return left == right;
    }
    if left.contains(&right) || right.contains(&left) {
        return true;
    }
    if opposing_claim_polarity(&left, &right) {
        return false;
    }
    let left_tokens = conflict_tokens(&left);
    let right_tokens = conflict_tokens(&right);
    let smaller = left_tokens.len().min(right_tokens.len());
    let shared = left_tokens.intersection(&right_tokens).count();
    smaller >= 5 && shared >= 4 && shared * 2 >= smaller
}

fn opposing_claim_polarity(left: &str, right: &str) -> bool {
    const NEGATIVE: [&str; 9] = [
        "drop", "drops", "dropped", "lose", "loses", "missing", "omitted", "reject", "refuse",
    ];
    const POSITIVE: [&str; 9] = [
        "accept",
        "accepts",
        "retain",
        "retains",
        "preserve",
        "preserves",
        "preserves",
        "allow",
        "include",
    ];
    let has = |text: &str, words: &[&str]| {
        words
            .iter()
            .any(|word| text.split_whitespace().any(|token| token == *word))
    };
    (has(left, &NEGATIVE) && has(right, &POSITIVE))
        || (has(left, &POSITIVE) && has(right, &NEGATIVE))
}

pub(crate) fn unique_review_observations_by_claim(
    observations: Vec<ObservationGroup>,
) -> Vec<ObservationGroup> {
    let mut unique = Vec::<ObservationGroup>::new();
    for observation in observations {
        let index = unique.iter().position(|existing| {
            observation_paths_compatible(existing, &observation)
                && (existing.kind == observation.kind
                    || existing.kind.is_empty()
                    || observation.kind.is_empty())
                && review_claims_match(&existing.claim, &observation.claim)
        });
        if let Some(index) = index {
            if observation_group_rank(&observation) > observation_group_rank(&unique[index]) {
                unique[index] = observation;
            }
        } else {
            unique.push(observation);
        }
    }
    unique
}

fn observation_paths_compatible(left: &ObservationGroup, right: &ObservationGroup) -> bool {
    match (&left.path, &right.path) {
        (Some(left), Some(right)) => left == right,
        _ => true,
    }
}

fn observation_group_rank(observation: &ObservationGroup) -> (u8, u8, u8, u8) {
    (
        observation_evidence_rank(observation),
        observation_status_rank(&observation.status),
        severity_rank(&observation.severity),
        confidence_rank(&observation.confidence),
    )
}

fn observation_evidence_rank(observation: &ObservationGroup) -> u8 {
    let text = format!(
        "{} {}",
        observation.sources.join(" "),
        observation.evidence.join(" ")
    )
    .to_ascii_lowercase();
    if text.contains("executed")
        || text.contains("receipt")
        || text.contains("focused test")
        || text.contains("sensor")
    {
        5
    } else if text.contains("source") || text.contains("diff") {
        3
    } else if text.contains("thread") || text.contains("existing") {
        2
    } else {
        1
    }
}

pub(crate) fn summary_finding_has_cross_lane_conflict(
    finding: &SummaryOnlyFinding,
    observations: &[ObservationGroup],
) -> bool {
    let key = summary_finding_conflict_key(finding);
    let needle = format!("finding_key={key}");
    observations.iter().any(|observation| {
        observation
            .sources
            .iter()
            .any(|source| source == CROSS_LANE_CONFLICT_SOURCE)
            && observation
                .evidence
                .iter()
                .any(|evidence| evidence.contains(&needle))
    })
}

pub(crate) fn normalized_review_text(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod human_output_admission_tests {
    use super::*;
    use anyhow::ensure;

    #[test]
    fn paraphrased_summary_findings_compile_to_one_claim() -> Result<()> {
        let findings = [
            SummaryOnlyFinding {
                lane: "parser".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Later declaration-list variables can lose postfix subscripts".to_owned(),
                evidence: "source inspection".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "tests".to_owned(),
                severity: "high".to_owned(),
                confidence: "high".to_owned(),
                reason: "Postfix subscripts are dropped from later variables in declaration lists"
                    .to_owned(),
                evidence: "focused regression".to_owned(),
            },
        ];

        let unique = unique_summary_review_findings(&findings);
        ensure!(unique.len() == 1, "paraphrased claim was not deduplicated");
        ensure!(unique[0].lane == "tests", "strongest claim did not survive");
        Ok(())
    }

    #[test]
    fn internal_inline_planning_language_is_artifact_only() -> Result<()> {
        for phrase in [
            "duplicate inline candidate merged into path:122",
            "resolve cross-lane conflict before posting",
        ] {
            ensure!(
                is_internal_review_machinery_text(phrase),
                "internal phrase was admitted: {phrase}"
            );
        }
        ensure!(is_internal_review_machinery_text("LANE CONFLICT: review"));
        ensure!(!is_internal_review_machinery_text(
            "An inline candidate is worth review."
        ));
        Ok(())
    }

    #[test]
    fn opposite_polarity_claims_are_not_collapsed() {
        assert!(!review_claims_match(
            "The parser drops default values from later declaration variables.",
            "The parser preserves default values for later declaration variables."
        ));
    }

    #[test]
    fn observation_identity_requires_matching_path_and_kind() {
        let base = ObservationGroup {
            schema: "observation-group".to_owned(),
            id: "base".to_owned(),
            dedupe_key: "claim".to_owned(),
            claim: "The changed parser loses postfix subscripts in later declaration variables."
                .to_owned(),
            kind: "bug".to_owned(),
            status: "open".to_owned(),
            severity: "medium".to_owned(),
            confidence: "medium".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(10),
            evidence: vec!["model observation".to_owned()],
            lanes: vec!["parser".to_owned()],
            sources: vec!["model".to_owned()],
            observation_ids: vec!["base".to_owned()],
            duplicate_count: 0,
        };
        let mut different_path = base.clone();
        different_path.id = "different-path".to_owned();
        different_path.path = Some("src/other.rs".to_owned());
        let mut different_kind = base.clone();
        different_kind.id = "different-kind".to_owned();
        different_kind.kind = "verification-question".to_owned();
        assert_eq!(
            unique_review_observations_by_claim(vec![base.clone(), different_path]).len(),
            2
        );
        assert_eq!(
            unique_review_observations_by_claim(vec![base, different_kind]).len(),
            2
        );
    }

    #[test]
    fn executed_evidence_beats_model_confidence() {
        let model = ObservationGroup {
            schema: "observation-group".to_owned(),
            id: "model".to_owned(),
            dedupe_key: "claim".to_owned(),
            claim: "The parser loses postfix subscripts in later declaration variables.".to_owned(),
            kind: "bug".to_owned(),
            status: "open".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: Some("src/parser.rs".to_owned()),
            line: Some(10),
            evidence: vec!["model assertion".to_owned()],
            lanes: vec!["parser".to_owned()],
            sources: vec!["model".to_owned()],
            observation_ids: vec!["model".to_owned()],
            duplicate_count: 0,
        };
        let mut executed = model.clone();
        executed.id = "executed".to_owned();
        executed.severity = "medium".to_owned();
        executed.confidence = "medium".to_owned();
        executed.evidence = vec!["executed focused test receipt".to_owned()];
        executed.sources = vec!["proof-receipt".to_owned()];
        let unique = unique_review_observations_by_claim(vec![model.clone(), executed]);
        assert_eq!(unique.len(), 1);
        assert_eq!(unique[0].id, "executed");
    }
}
