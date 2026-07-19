// Validation and inline-comment test cluster, extracted from src/main.rs mod tests (#597).
// Resolves shared fixtures through `super::*` and production symbols through `crate::*`.
use super::*;
use crate::*;

#[test]
fn summary_only_guard_rejects_unsupported_model_findings() -> Result<()> {
    let lane = default_lanes()
        .into_iter()
        .find(|lane| lane.id == "tests")
        .ok_or_else(|| anyhow::anyhow!("tests lane missing"))?;

    let accepted = validate_summary_only_candidate(
        &lane,
        ModelCandidateFinding {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            reason: "The test reaches the helper but does not reveal the changed behavior."
                .to_owned(),
            evidence: "ripr summary excerpt".to_owned(),
        },
    );
    assert_eq!(accepted.severity, "medium");
    assert_eq!(accepted.confidence, "medium-high");
    assert_eq!(accepted.evidence, "ripr summary excerpt");

    let rejected = validate_summary_only_candidate(
        &lane,
        ModelCandidateFinding {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            reason: " ".to_owned(),
            evidence: "".to_owned(),
        },
    );
    assert_eq!(rejected.severity, "low");
    assert_eq!(rejected.confidence, "medium");
    assert!(rejected.reason.contains("reason_present=false"));
    assert!(rejected.reason.contains("evidence_present=false"));
    assert_eq!(rejected.evidence, "model summary-only candidate guardrail");
    Ok(())
}

#[test]
fn sibling_source_route_prompt_requires_scan_boundaries() -> Result<()> {
    let lane = default_lanes()
        .into_iter()
        .find(|lane| lane.id == "source-route")
        .ok_or_else(|| anyhow::anyhow!("source-route lane missing"))?;

    let guidance = crate::lane_specific_prompt_guidance(&lane);

    assert!(guidance.contains("no-match scan"));
    assert!(guidance.contains("not proof that no sibling paths exist"));
    assert!(guidance.contains("checked pattern/scope"));
    assert!(guidance.contains("unscanned variants"));
    Ok(())
}

#[test]
fn sibling_summary_completeness_claim_becomes_verification_observation() -> Result<()> {
    let lane = lane_plan("sibling-paths");
    let output = LaneModelOutput {
            summary: Some(
                "No analogous sibling panic paths were found, so the fix is correctly scoped and need not be broadened."
                    .to_owned(),
            ),
            inline_comments: Vec::new(),
            candidate_findings: Vec::new(),
            summary_only_findings: Vec::new(),
            observations: Vec::new(),
            failed_objections: Vec::new(),
            proof_requests: Vec::new(),
            issue_candidates: Vec::new(),
            degraded: false,
        };
    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    let mut observations = Vec::new();
    let mut proof_requests = Vec::new();
    let mut issue_candidates = Vec::new();

    apply_model_output(
        &lane,
        output,
        &BTreeSet::new(),
        ModelOutputSinks {
            inline_comments: &mut inline_comments,
            summary_only_findings: &mut summary_only_findings,
            model_observations: &mut observations,
            proof_requests: &mut proof_requests,
            issue_candidates: &mut issue_candidates,
        },
    );

    assert!(inline_comments.is_empty());
    assert!(summary_only_findings.is_empty());
    assert!(proof_requests.is_empty());
    assert_eq!(observations.len(), 1);
    let observation = &observations[0];
    assert_eq!(observation.question, "sibling-path-coverage");
    assert_eq!(observation.kind, "source-route-gap");
    assert_eq!(observation.status, "open");
    assert_eq!(observation.severity, "medium");
    assert_eq!(observation.confidence, "high");
    assert_eq!(
        observation.dedupe_key,
        crate::SIBLING_COMPLETENESS_OVERCLAIM_DEDUPE_KEY
    );
    assert!(
        observation
            .evidence
            .iter()
            .any(|item| item.contains("narrow no-match scans"))
    );

    let pr_body = render_review_body(
        "shared-context-test",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[],
        &[],
        &[],
        &[],
        &observations,
        &[],
        16_384,
        ReviewBodyAudience::PullRequest,
    );

    assert!(pr_body.contains("## Decision"));
    assert!(pr_body.contains("## Verification questions"));
    assert!(pr_body.contains("Check sibling-path scan coverage"));
    assert!(!pr_body.contains("## Refuted"));
    assert!(!pr_body.contains("correctly scoped"));
    assert!(!pr_body.contains("No analogous"));
    Ok(())
}

#[test]
fn sibling_failed_objection_completeness_claim_is_not_refuted() {
    let lane = lane_plan("source-route");

    let observation = validate_failed_objection(
        &lane,
        ModelFailedObjection {
            claim: "No analogous sibling panic paths were found.".to_owned(),
            reason: "The fix is correctly scoped and need not be broadened.".to_owned(),
            confidence: Some("high".to_owned()),
            kind: Some("resolved-check".to_owned()),
            evidence: vec!["single-pattern write/dispose scan".to_owned()],
        },
        0,
    );

    assert_eq!(observation.question, "sibling-path-coverage");
    assert_eq!(observation.kind, "source-route-gap");
    assert_eq!(observation.status, "open");
    assert_ne!(observation.status, "refuted");
    assert_eq!(
        observation.dedupe_key,
        crate::SIBLING_COMPLETENESS_OVERCLAIM_DEDUPE_KEY
    );
}

#[test]
fn scoped_sibling_scan_limit_remains_coverage_limited() {
    let lane = lane_plan("sibling-paths");

    let observation = validate_model_observation(
        &lane,
        ModelCandidateObservation {
            claim: "Checked write/dispose only; did not scan ptr/toBuffer or to_int64 paths."
                .to_owned(),
            question: Some("sibling-paths".to_owned()),
            kind: Some("source-route-gap".to_owned()),
            status: Some("open".to_owned()),
            severity: Some("medium".to_owned()),
            confidence: Some("medium".to_owned()),
            path: None,
            line: None,
            evidence: vec!["coverage-limited sibling scan".to_owned()],
            dedupe_key: Some("coverage-limited-sibling-scan".to_owned()),
        },
        0,
    );

    assert_eq!(
        observation.claim,
        "Checked write/dispose only; did not scan ptr/toBuffer or to_int64 paths."
    );
    assert_eq!(observation.kind, "source-route-gap");
    assert_eq!(observation.status, "open");
    assert_eq!(observation.dedupe_key, "coverage-limited-sibling-scan");
    assert_eq!(observation.source, "model-observation");
}

#[test]
fn inline_guard_accepts_only_right_side_diff_lines() -> Result<()> {
    let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
";
    let line_map = right_side_diff_lines(patch);
    let lane = default_lanes()
        .into_iter()
        .find(|lane| lane.id == "tests")
        .ok_or_else(|| anyhow::anyhow!("tests lane missing"))?;
    let accepted = validate_inline_candidate(
        &lane,
        ModelCandidateComment {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            body: "This reaches the helper but does not assert the changed boundary.".to_owned(),
            evidence: "diff hunk".to_owned(),
            suggestion: None,
        },
        &line_map,
    )
    .map_err(|finding| anyhow::anyhow!("unexpected rejection: {}", finding.reason))?;
    assert_eq!(accepted.side, "RIGHT");
    assert!(accepted.body.starts_with("[tests]"));
    assert!(accepted.suggestion.is_none());

    let model_suggestion = validate_inline_candidate(
        &lane,
        ModelCandidateComment {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            body: "[tests] model-proposed edit must remain advisory".to_owned(),
            evidence: "diff hunk".to_owned(),
            suggestion: Some("assert!(proved);".to_owned()),
        },
        &line_map,
    )
    .map_err(|finding| anyhow::anyhow!("unexpected rejection: {}", finding.reason))?;
    assert!(
        model_suggestion.suggestion.is_none(),
        "non-unsafe-review lanes must not smuggle suggestion blocks"
    );

    let rejected = validate_inline_candidate(
        &lane,
        ModelCandidateComment {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 50,
            body: "[tests] guessed stale line".to_owned(),
            evidence: "none".to_owned(),
            suggestion: None,
        },
        &line_map,
    );
    assert!(rejected.is_err());
    let missing_evidence = validate_inline_candidate(
        &lane,
        ModelCandidateComment {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            body: "[tests] line-valid but unsupported claim".to_owned(),
            evidence: "".to_owned(),
            suggestion: None,
        },
        &line_map,
    );
    assert!(
        missing_evidence
            .is_err_and(|finding| { finding.reason.contains("evidence_present=false") })
    );

    let empty_body = validate_inline_candidate(
        &lane,
        ModelCandidateComment {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            body: "   ".to_owned(),
            evidence: "diff hunk".to_owned(),
            suggestion: None,
        },
        &line_map,
    );
    assert!(empty_body.is_err_and(|finding| { finding.reason.contains("body_present=false") }));
    Ok(())
}

#[test]
fn candidate_only_lanes_cannot_emit_inline_comments() {
    let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
";
    let line_map = right_side_diff_lines(patch);
    let lane = model_lane(
        "source-route-fast",
        "Fast source-route candidate generation",
        &["tokmd", "ast-grep"],
        "Generate candidate-only public API route and helper caller gaps.",
    );
    let output = LaneModelOutput {
        summary: None,
        inline_comments: vec![ModelCandidateComment {
            severity: "medium".to_owned(),
            confidence: "medium-high".to_owned(),
            path: "src/lib.rs".to_owned(),
            line: 2,
            body: "[source-route-fast] This is line-valid but must stay candidate-only.".to_owned(),
            evidence: "diff hunk".to_owned(),
            suggestion: None,
        }],
        candidate_findings: Vec::new(),
        summary_only_findings: Vec::new(),
        observations: Vec::new(),
        failed_objections: Vec::new(),
        proof_requests: Vec::new(),
        issue_candidates: Vec::new(),
        degraded: false,
    };
    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    let mut model_observations = Vec::new();
    let mut proof_requests = Vec::new();
    let mut issue_candidates = Vec::new();

    apply_model_output(
        &lane,
        output,
        &line_map,
        ModelOutputSinks {
            inline_comments: &mut inline_comments,
            summary_only_findings: &mut summary_only_findings,
            model_observations: &mut model_observations,
            proof_requests: &mut proof_requests,
            issue_candidates: &mut issue_candidates,
        },
    );

    assert!(inline_comments.is_empty());
    assert_eq!(summary_only_findings.len(), 1);
    assert_eq!(summary_only_findings[0].lane, "source-route-fast");
    assert!(
        summary_only_findings[0]
            .reason
            .contains("candidate-only lane emitted inline candidate")
    );
    assert_eq!(summary_only_findings[0].evidence, "diff hunk");
}

#[test]
fn lane_output_split_accepts_observations_candidates_and_proof_requests() -> Result<()> {
    let patch = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111..2222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 pub fn active_len(len: usize) -> usize {
+    let ptr = &len as *const usize;
     len
 }
";
    let line_map = right_side_diff_lines(patch);
    let lane = model_lane(
        "tests-oracle",
        "Test oracle review",
        &["tokmd", "ripr"],
        "Check test proof.",
    );
    let json = r#"{
  "summary": "Checked red/green and route proof.",
  "observations": [
    {
      "claim": "The new test needs a witnessed old-main red run.",
      "question": "red-green",
      "kind": "missing-evidence",
      "status": "open",
      "severity": "medium",
      "confidence": "high",
      "evidence": ["PR body claims old code fails"],
      "dedupe_key": "markdown-red-green-witness"
    }
  ],
  "candidate_findings": [
    {
      "severity": "medium",
      "confidence": "medium-high",
      "path": "src/lib.rs",
      "line": 2,
      "body": "[tests-oracle] The changed pointer path needs a test oracle.",
      "evidence": "diff hunk"
    }
  ],
  "failed_objections": [
    {
      "claim": "Box::from(slice) can return None on allocation failure",
      "reason": "false premise: allocation failure does not return None",
      "confidence": "high",
      "kind": "false-premise",
      "evidence": ["Rust allocation semantics"]
    }
  ],
  "proof_requests": [
    {
      "command": "bun test test/js/bun/md/md-edge-cases.test.ts",
      "reason": "Need a focused green witness on HEAD",
      "cost": "focused-test",
      "timeout_sec": 300,
      "required": false
    }
  ]
}"#;
    let output: LaneModelOutput = serde_json::from_str(json)?;
    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    let mut observations = Vec::new();
    let mut proof_requests = Vec::new();
    let mut issue_candidates = Vec::new();

    apply_model_output(
        &lane,
        output,
        &line_map,
        ModelOutputSinks {
            inline_comments: &mut inline_comments,
            summary_only_findings: &mut summary_only_findings,
            model_observations: &mut observations,
            proof_requests: &mut proof_requests,
            issue_candidates: &mut issue_candidates,
        },
    );

    assert_eq!(inline_comments.len(), 1);
    assert_eq!(inline_comments[0].lane, "tests-oracle");
    assert_eq!(summary_only_findings.len(), 1);
    assert_eq!(observations.len(), 2);
    assert!(observations.iter().any(|observation| {
        observation.kind == "missing-evidence"
            && observation.dedupe_key == "markdown-red-green-witness"
            && observation.source == "model-observation"
    }));
    assert!(observations.iter().any(|observation| {
        observation.kind == "false-premise"
            && observation.status == "refuted"
            && observation.source == "model-failed-objection"
    }));
    assert_eq!(proof_requests.len(), 1);
    assert_eq!(proof_requests[0].schema, "ub-review.proof_request.v1");
    assert_eq!(proof_requests[0].status, "requested");
    assert_eq!(
        proof_requests[0].requested_by,
        vec!["tests-oracle".to_owned()]
    );

    let temp = tempfile::tempdir()?;
    write_proof_request_artifacts(
        temp.path(),
        &test_diff(),
        &Profile::default(),
        &proof_requests,
        &[] as &[ProofReceipt],
    )?;
    let proof_json: Vec<super::ProofRequest> =
        serde_json::from_slice(&fs::read(temp.path().join("review/proof_requests.json"))?)?;
    let proof_groups: Vec<ProofRequestGroup> = serde_json::from_slice(&fs::read(
        temp.path().join("review/proof_request_groups.json"),
    )?)?;
    let proof_request_file: serde_json::Value = serde_json::from_slice(&fs::read(
        temp.path()
            .join("proof_requests")
            .join(format!("{}.json", proof_requests[0].id)),
    )?)?;
    let proof_plan = fs::read_to_string(temp.path().join("review/proof_plan.md"))?;
    let proof_ndjson = fs::read_to_string(temp.path().join("proof_requests.ndjson"))?;
    assert_eq!(proof_json.len(), 1);
    assert_eq!(proof_request_file, serde_json::to_value(&proof_json[0])?);
    assert_eq!(proof_groups.len(), 1);
    assert_eq!(proof_groups[0].duplicate_count, 1);
    assert!(proof_plan.contains("## Focused proof plan"));
    assert!(proof_plan.contains("mode=`red-green`"));
    assert!(proof_plan.contains("base+tests=`cwd=target/ub-review/proof-worktrees/base-plus-tests USE_SYSTEM_BUN=1 bun test test/js/bun/md/md-edge-cases.test.ts`"));
    assert!(proof_ndjson.contains("bun test test/js/bun/md/md-edge-cases.test.ts"));
    Ok(())
}

#[test]
fn lane_output_split_accepts_scalar_evidence_strings() -> Result<()> {
    let lane = model_lane(
        "source-route",
        "Source route review",
        &["tokmd", "ast-grep"],
        "Check public API route proof.",
    );
    let json = r#"{
  "observations": [
    {
      "claim": "FileHandle.write route still needs proof.",
      "kind": "source-route-gap",
      "status": "open",
      "evidence": "route excerpt was scalar text"
    }
  ],
  "failed_objections": [
    {
      "claim": "writev uses the patched scalar branch",
      "reason": "sibling route still calls a separate helper",
      "evidence": "sibling-path scan was scalar text"
    }
  ]
}"#;
    let output: LaneModelOutput = serde_json::from_str(json)?;
    assert!(output.degraded);
    assert_eq!(
        output.observations[0].evidence,
        vec!["route excerpt was scalar text".to_owned()]
    );
    assert_eq!(
        output.failed_objections[0].evidence,
        vec!["sibling-path scan was scalar text".to_owned()]
    );

    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    let mut observations = Vec::new();
    let mut proof_requests = Vec::new();
    let mut issue_candidates = Vec::new();
    apply_model_output(
        &lane,
        output,
        &BTreeSet::new(),
        ModelOutputSinks {
            inline_comments: &mut inline_comments,
            summary_only_findings: &mut summary_only_findings,
            model_observations: &mut observations,
            proof_requests: &mut proof_requests,
            issue_candidates: &mut issue_candidates,
        },
    );

    assert_eq!(observations.len(), 2);
    assert!(observations.iter().any(|observation| {
        observation.source == "model-observation"
            && observation.evidence == vec!["route excerpt was scalar text".to_owned()]
    }));
    assert!(observations.iter().any(|observation| {
        observation.source == "model-failed-objection"
            && observation.evidence == vec!["sibling-path scan was scalar text".to_owned()]
    }));
    Ok(())
}

#[test]
fn lane_output_split_degrades_scalar_sequence_fields() -> Result<()> {
    let lane = model_lane(
        "tests-oracle",
        "Test oracle review",
        &["tokmd", "ripr"],
        "Check test proof.",
    );
    let json = r#"{
  "observations": "The added regression test still needs base+tests red/green proof.",
  "candidate_findings": "Malformed inline finding text should not erase the whole lane."
}"#;
    let (output, degraded) = crate::parse_lane_model_output_or_degrade(
        json,
        Path::new("target/ub-review/review/model/tests-oracle/content.json"),
    )?;
    assert!(degraded);
    assert!(output.degraded);
    assert!(output.candidate_findings.is_empty());
    assert_eq!(output.observations.len(), 2);

    let mut inline_comments = Vec::new();
    let mut summary_only_findings = Vec::new();
    let mut observations = Vec::new();
    let mut proof_requests = Vec::new();
    let mut issue_candidates = Vec::new();
    apply_model_output(
        &lane,
        output,
        &BTreeSet::new(),
        ModelOutputSinks {
            inline_comments: &mut inline_comments,
            summary_only_findings: &mut summary_only_findings,
            model_observations: &mut observations,
            proof_requests: &mut proof_requests,
            issue_candidates: &mut issue_candidates,
        },
    );

    assert!(inline_comments.is_empty());
    assert!(summary_only_findings.is_empty());
    assert_eq!(observations.len(), 2);
    assert!(observations.iter().any(|observation| {
        observation.source == "model-observation"
            && observation.kind == "missing-evidence"
            && observation.question == "lane-output-shape"
            && observation.dedupe_key == "lane-output-shape-observations"
            && observation.claim.contains("base+tests red/green proof")
    }));
    assert!(observations.iter().any(|observation| {
        observation.source == "model-observation"
            && observation.kind == "missing-evidence"
            && observation.dedupe_key == "lane-output-shape-candidate_findings"
            && observation
                .evidence
                .iter()
                .any(|item| item.contains("Malformed inline finding text"))
    }));
    Ok(())
}

#[test]
fn lane_output_split_degrades_contentful_malformed_output() -> Result<()> {
    let raw = "args.buffer = StringOrBuffer::EncodedSlice(ZigStringSlice::init_owned(owned)); runs synchronously pre-schedule";
    let parse_path = Path::new("target/ub-review/review/model/ub-worker-handoff/content.json");

    let (output, degraded) = crate::parse_lane_model_output_or_degrade(raw, parse_path)?;

    assert!(degraded);
    assert!(output.degraded);
    assert!(output.inline_comments.is_empty());
    assert!(output.candidate_findings.is_empty());
    assert!(output.summary_only_findings.is_empty());
    assert_eq!(output.observations.len(), 1);
    assert_eq!(
        output.observations[0].question.as_deref(),
        Some("lane-output-shape")
    );
    assert_eq!(
        output.observations[0].kind.as_deref(),
        Some("missing-evidence")
    );
    assert!(output.observations[0].claim.contains("EncodedSlice"));
    assert!(
        output.observations[0]
            .evidence
            .iter()
            .any(|item| item.contains("content.json"))
    );
    Ok(())
}

#[test]
fn lane_output_split_degrades_contentful_schema_wrong_json() -> Result<()> {
    let raw = r#"{"findings":"EncodedSlice route excerpt survived as text"}"#;
    let parse_path = Path::new("target/ub-review/review/model/ub-worker-handoff/content.json");

    let (output, degraded) = crate::parse_lane_model_output_or_degrade(raw, parse_path)?;

    assert!(degraded);
    assert!(output.degraded);
    assert!(output.inline_comments.is_empty());
    assert!(output.candidate_findings.is_empty());
    assert!(output.summary_only_findings.is_empty());
    assert_eq!(output.observations.len(), 1);
    assert!(
        output.observations[0]
            .claim
            .contains("EncodedSlice route excerpt")
    );
    assert!(
        output.observations[0]
            .evidence
            .iter()
            .any(|item| item.contains("recognized lane evidence"))
    );
    Ok(())
}

#[test]
fn lane_output_split_rejects_empty_unusable_output() -> Result<()> {
    let parse_path = Path::new("target/ub-review/review/model/ub-active-view/content.json");

    for raw in ["{}", r#"{"observations": ""}"#] {
        let err = crate::parse_lane_model_output_or_degrade(raw, parse_path)
            .err()
            .ok_or_else(|| anyhow::anyhow!("empty lane output unexpectedly parsed"))?;
        assert_eq!(crate::classify_model_error(&err), "invalid_json");
        assert!(format!("{err:#}").contains("empty or unusable"));
    }
    Ok(())
}
#[test]
fn degraded_model_lane_is_attempted_but_not_missing_evidence() {
    let mut degraded = model_lane_receipt("ub-worker-handoff", "degraded");
    degraded.reason = "contentful lane output was preserved as degraded evidence".to_owned();

    assert!(crate::model_call_attempted_status("degraded"));
    assert!(!crate::is_model_receipt_evidence_issue(&degraded));
}
