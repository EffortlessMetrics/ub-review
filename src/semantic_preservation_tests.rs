//! Semantic lane evidence must survive operational completion receipts.

use crate::*;

fn receipt_fixture() -> Result<ModelLaneReceipt> {
    Ok(serde_json::from_value(serde_json::json!({
        "lane": "source-route",
        "provider": "deterministic-test",
        "model": "fixture",
        "endpoint_kind": "openai-chat",
        "status": "ok",
        "reason": "completed",
        "duration_ms": null,
        "http_status": 200,
        "response_shape": "chat",
        "fallback_from": null,
        "shared_prefix_hash": "review-head-a",
        "thread_id": "cohort:source-route",
        "semantic_state": {
            "shared_prefix_hash": "review-head-a",
            "conclusion": "{\"summary\":\"The route skips the bounds guard.\"}"
        }
    }))?)
}

#[test]
fn semantic_preservation_reporter_receives_review_meaning_instead_of_completed() -> Result<()> {
    let receipt = receipt_fixture()?;
    let temp = tempfile::tempdir()?;
    let digests = reporter::lane_digests_from_receipts(temp.path(), &[receipt]);
    assert_eq!(digests.len(), 1);
    assert_eq!(digests[0].status, "ok");
    assert!(
        digests[0]
            .conclusion
            .contains("The route skips the bounds guard.")
    );
    assert_ne!(digests[0].conclusion, "completed");
    let prompt = reporter::reporter_prompt(&digests, &[]);
    assert!(prompt.contains("The route skips the bounds guard."));
    Ok(())
}

#[test]
fn semantic_preservation_projects_public_material_and_withholds_private_deliberation() -> Result<()>
{
    let output: LaneModelOutput = serde_json::from_value(serde_json::json!({
        "summary": "The route skips the bounds guard.",
        "inline_comments": [{
            "severity": "high", "confidence": "high", "path": "src/route.rs",
            "line": 4, "body": "Inline guard witness", "evidence": "unchecked index",
            "suggestion": "validate the index first"
        }],
        "candidate_findings": [{
            "severity": "medium", "confidence": "high", "path": "src/route.rs",
            "line": 8, "body": "Candidate route witness", "evidence": "public call chain"
        }],
        "summary_only_findings": [{
            "severity": "medium", "confidence": "high", "reason": "Summary-only gap",
            "evidence": "missing edge-case coverage"
        }],
        "observations": [{
            "claim": "The release route needs verification", "kind": "missing-evidence",
            "status": "open", "evidence": ["route excerpt"]
        }],
        "proof_requests": [{
            "command": "cargo test --locked bounds_guard", "reason": "Witness the guard",
            "required": true
        }],
        "proof_intents": [{
            "claim_id": "bounds-guard", "question": "Does the public route reject excess indices?",
            "expected_answer_shape": "a witnessed rejection", "proof_kind": "focused-test",
            "target": "bounds_guard"
        }],
        "internal_audit": {
            "surfaces_checked": ["PRIVATE_AUDIT_SURFACE"],
            "strongest_rejected_hypothesis": "PRIVATE_REJECTED_HYPOTHESIS"
        },
        "failed_objections": [{
            "claim": "PRIVATE_FAILED_OBJECTION", "reason": "PRIVATE_REFUTATION",
            "evidence": ["PRIVATE_OBJECTION_EVIDENCE"]
        }]
    }))?;
    let mut receipt = receipt_fixture()?;
    receipt.semantic_state = Some(LaneSemanticState::from_output(
        &output,
        &receipt.shared_prefix_hash,
    )?);
    let projection: serde_json::Value = serde_json::from_str(&receipt.semantic_conclusion())?;
    assert_eq!(projection["summary"], "The route skips the bounds guard.");
    for (field, witness) in [
        ("inline_comments", "Inline guard witness"),
        ("candidate_findings", "Candidate route witness"),
        ("summary_only_findings", "Summary-only gap"),
        ("observations", "The release route needs verification"),
        ("proof_requests", "Witness the guard"),
        (
            "proof_intents",
            "Does the public route reject excess indices?",
        ),
    ] {
        assert!(projection[field].to_string().contains(witness), "{field}");
    }
    assert!(projection.get("internal_audit").is_none());
    assert!(projection.get("failed_objections").is_none());
    let temp = tempfile::tempdir()?;
    let digests = reporter::lane_digests_from_receipts(temp.path(), &[receipt]);
    let prompt = reporter::reporter_prompt(&digests, &[]);
    assert!(prompt.contains("Inline guard witness"));
    assert!(prompt.contains("Witness the guard"));
    assert!(!prompt.contains("PRIVATE_"));
    Ok(())
}

#[test]
fn semantic_preservation_keeps_fallback_diagnostics_separate_from_meaning() -> Result<()> {
    let mut receipt = receipt_fixture()?;
    receipt.reason = "primary failed: provider quota; fallback completed".to_owned();
    receipt.fallback_from = Some("primary-provider".to_owned());
    receipt.provider = "fallback-provider".to_owned();
    receipt.cohort_broken = true;
    let conclusion = receipt.semantic_conclusion();
    assert!(conclusion.contains("The route skips the bounds guard."));
    assert!(!conclusion.contains("provider quota"));
    let serialized = serde_json::to_value(&receipt)?;
    assert_eq!(
        serialized["reason"],
        "primary failed: provider quota; fallback completed"
    );
    assert_eq!(serialized["fallback_from"], "primary-provider");
    Ok(())
}

#[test]
fn semantic_preservation_missing_and_stale_evidence_never_reuse_completion() -> Result<()> {
    let mut receipt = receipt_fixture()?;
    receipt.semantic_state = None;
    let missing = receipt.semantic_conclusion();
    assert!(!missing.trim().is_empty());
    assert_ne!(missing, receipt.reason);
    assert!(missing.to_ascii_lowercase().contains("missing"));
    receipt.semantic_state = Some(LaneSemanticState {
        shared_prefix_hash: "superseded-review-head".to_owned(),
        conclusion: "STALE_MEANING_MUST_NOT_REACH_REPORTER".to_owned(),
    });
    let stale = receipt.semantic_conclusion();
    assert!(stale.to_ascii_lowercase().contains("stale"));
    assert!(!stale.contains("STALE_MEANING_MUST_NOT_REACH_REPORTER"));
    let temp = tempfile::tempdir()?;
    let digests = reporter::lane_digests_from_receipts(temp.path(), &[receipt]);
    let prompt = reporter::reporter_prompt(&digests, &[]);
    assert!(!prompt.contains("STALE_MEANING_MUST_NOT_REACH_REPORTER"));
    assert!(!prompt.contains("completed"));
    let mut unidentified = receipt_fixture()?;
    unidentified.shared_prefix_hash.clear();
    unidentified.semantic_state = Some(LaneSemanticState {
        shared_prefix_hash: String::new(),
        conclusion: "UNBOUND_MEANING_MUST_NOT_REACH_REPORTER".to_owned(),
    });
    let unbound = unidentified.semantic_conclusion();
    assert!(unbound.to_ascii_lowercase().contains("missing"));
    assert!(!unbound.contains("UNBOUND_MEANING_MUST_NOT_REACH_REPORTER"));
    Ok(())
}

#[test]
fn semantic_preservation_explicit_zero_findings_is_distinct_from_missing() -> Result<()> {
    let (output, degraded) = parse_lane_model_output_or_degrade(
        r#"{"summary":"Reviewed the bounds route: no material issue remains.","inline_comments":[],"candidate_findings":[],"summary_only_findings":[]}"#,
        Path::new("model/source-route/content.json"),
    )?;
    assert!(!degraded);
    let mut receipt = receipt_fixture()?;
    receipt.semantic_state = None;
    let missing = receipt.semantic_conclusion();
    receipt.semantic_state = Some(LaneSemanticState::from_output(
        &output,
        &receipt.shared_prefix_hash,
    )?);
    let conclusion = receipt.semantic_conclusion();
    assert!(conclusion.contains("no material issue remains"));
    assert_ne!(conclusion, missing);
    let projection: serde_json::Value = serde_json::from_str(&conclusion)?;
    assert_eq!(projection["candidate_findings"], serde_json::json!([]));
    Ok(())
}

#[test]
fn semantic_preservation_malformed_output_preserves_degradation_instead_of_clean_meaning()
-> Result<()> {
    assert!(serde_json::from_str::<LaneModelOutput>(r#"{"summary":42}"#).is_err());
    let parse_path = Path::new("model/source-route/content.json");
    for raw in ["{", "{}"] {
        assert!(parse_lane_model_output_or_degrade(raw, parse_path).is_err());
    }
    let (output, degraded) = parse_lane_model_output_or_degrade(
        r#"{"observations":"The guard route remains unverified"}"#,
        parse_path,
    )?;
    assert!(degraded);
    let semantic = LaneSemanticState::from_output(&output, "review-head-a")?;
    let projection: serde_json::Value = serde_json::from_str(&semantic.conclusion)?;
    assert_eq!(projection["degraded"], true);
    assert!(
        projection["observations"]
            .to_string()
            .contains("The guard route remains unverified")
    );
    Ok(())
}

#[test]
fn semantic_preservation_private_parser_diagnostics_stay_out_of_reporter_context() -> Result<()> {
    let parse_path = Path::new("model/source-route/content.json");
    for raw in [
        r#"{"summary":"Checked bounds route","failed_objections":"PRIVATE_REJECTED_HYPOTHESIS"}"#,
        r#"{"summary":"Checked bounds route","internal_audit":{}}"#,
        r#"{"summary":"Checked bounds route","internal_audit":{"surfaces_checked":42}}"#,
        r#"{"internal_audit":{"surfaces_checked":["PRIVATE_REJECTED_HYPOTHESIS"]"#,
    ] {
        let (output, degraded) = parse_lane_model_output_or_degrade(raw, parse_path)?;
        assert!(degraded);
        let mut receipt = receipt_fixture()?;
        receipt.semantic_state = Some(LaneSemanticState::from_output(
            &output,
            &receipt.shared_prefix_hash,
        )?);
        let conclusion = receipt.semantic_conclusion();
        let projection: serde_json::Value = serde_json::from_str(&conclusion)?;
        assert_eq!(projection["degraded"], true);
        assert!(!conclusion.contains("PRIVATE_REJECTED_HYPOTHESIS"));
        assert!(!conclusion.contains("empty-internal-audit"));
        assert!(!conclusion.contains("malformed-internal-audit"));
        let temp = tempfile::tempdir()?;
        let digests = reporter::lane_digests_from_receipts(temp.path(), &[receipt]);
        let prompt = reporter::reporter_prompt(&digests, &[]);
        assert!(!prompt.contains("PRIVATE_REJECTED_HYPOTHESIS"));
        assert!(!prompt.contains("empty-internal-audit"));
        assert!(!prompt.contains("malformed-internal-audit"));
    }
    Ok(())
}

#[test]
fn semantic_preservation_empty_semantic_state_is_missing_evidence() -> Result<()> {
    let mut receipt = receipt_fixture()?;
    receipt.semantic_state = Some(LaneSemanticState {
        shared_prefix_hash: receipt.shared_prefix_hash.clone(),
        conclusion: "  \n\t".to_owned(),
    });
    let conclusion = receipt.semantic_conclusion();
    assert!(conclusion.to_ascii_lowercase().contains("missing"));
    assert_ne!(conclusion, receipt.reason);
    Ok(())
}

#[test]
fn semantic_preservation_runtime_fallback_keeps_successful_provider_review_meaning() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let cli = Cli::try_parse_from([
        "ub-review",
        "run",
        "--max-model-calls",
        "4",
        "--model-concurrency",
        "1",
    ])?;
    let Command::Run(args) = cli.command else {
        bail!("run arguments did not select run command")
    };
    let primary = direct_minimax_spec(&args);
    let fallback = opencode_canary_spec(&args);
    let assignments = vec![ModelAssignment {
        lane: LanePlan {
            id: "source-route".to_owned(),
            role: "Review public route".to_owned(),
            model: primary.model.clone(),
            model_display: primary.model.clone(),
            receives: Vec::new(),
            focus: "Check bounds guarding".to_owned(),
        },
        spec: primary.clone(),
        fallback: Some(fallback.clone()),
    }];
    let preflights = vec![ProviderPreflightReceipt {
        provider: primary.provider.key().to_owned(),
        model: primary.model.clone(),
        endpoint_kind: primary.endpoint_kind.key().to_owned(),
        status: "ok".to_owned(),
        reason: "preflight ok".to_owned(),
        duration_ms: Some(1),
        http_status: Some(200),
        response_shape: None,
        cache_usage: ModelCacheUsage::default(),
    }];
    let mut receipt = receipt_fixture()?;
    receipt.status = "planned".to_owned();
    receipt.reason = "planned model lane".to_owned();
    receipt.semantic_state = None;
    receipt.shared_prefix_hash.clear();
    receipt.thread_id.clear();
    let mut receipts = vec![receipt];
    let mut issues = Vec::new();
    let line_map = BTreeSet::new();
    let attempted_providers = std::cell::RefCell::new(Vec::new());
    let calls = run_available_model_lanes_with_runner(
        ModelRunContext {
            root: temp.path(),
            review_dir: temp.path(),
            assignments: &assignments,
            provider_preflights: &preflights,
            shared_context: "immutable reviewed context",
            args: &args,
            line_map: &line_map,
            key_present: |_| true,
            provider_concurrency: ProviderConcurrencyLimits::default(),
        },
        &mut receipts,
        &mut issues,
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        &mut Vec::new(),
        |_context, _model_dir, tasks| {
            tasks
                .into_iter()
                .map(|task| {
                    attempted_providers
                        .borrow_mut()
                        .push(task.spec.provider.key().to_owned());
                    let result = if task.spec.provider == primary.provider {
                        Err(anyhow::anyhow!(
                            "model curl: http status Some(429) too many requests"
                        ))
                    } else {
                        Ok(ModelCallOutcome {
                            output: serde_json::from_value(serde_json::json!({
                                "summary": "Fallback review found a missing bounds guard",
                                "proof_requests": [{
                                    "command": "cargo test --locked bounds_guard",
                                    "reason": "Reproduce out-of-bounds route"
                                }]
                            }))?,
                            duration_ms: 1,
                            http_status: Some(200),
                            response_shape: "deterministic-test".to_owned(),
                            cache_usage: ModelCacheUsage::default(),
                            degraded: false,
                        })
                    };
                    Ok(ModelLaneTaskResult {
                        index: task.index,
                        result,
                    })
                })
                .collect::<Result<Vec<_>>>()
        },
    )?;
    assert_eq!(calls, 2);
    assert_eq!(
        *attempted_providers.borrow(),
        vec![primary.provider.key(), fallback.provider.key()]
    );
    assert_eq!(receipts[0].status, "ok");
    assert_eq!(receipts[0].reason, "completed after runtime fallback retry");
    assert_eq!(receipts[0].provider, fallback.provider.key());
    assert_eq!(receipts[0].fallback_from, Some(primary.label()));
    assert!(issues.is_empty());
    let digests = reporter::lane_digests_from_receipts(temp.path(), &receipts);
    let prompt = reporter::reporter_prompt(&digests, &[]);
    assert!(prompt.contains("Fallback review found a missing bounds guard"));
    assert!(prompt.contains("Reproduce out-of-bounds route"));
    assert!(!prompt.contains("completed after runtime fallback retry"));
    Ok(())
}
