// Review compiler and reporter-facing test cluster, extracted from src/main.rs mod tests (#597).
// Resolves shared fixtures through `super::*` and production symbols through `crate::*`.
use super::has_standalone_approval_line;
use super::*;
use crate::diff_posture::default_lanes_for_diff_context;
use crate::*;

#[test]
fn compiler_surface_keeps_refuted_only_follow_up_artifact_only() -> Result<()> {
    let args = test_run_args(Path::new("target/ub-review").to_path_buf());
    let plan = test_plan(Vec::new());
    let diff = test_diff();
    let model_lanes = vec![model_lane_receipt("workflow-opposition", "ok")];
    let follow_up_observation = test_observation(
        "orchestrator-follow-up-route",
        "The source-route concern was refuted by the routed proof receipt.",
        "false-premise",
        "refuted",
        "medium",
        "high",
        "source-route-refuted",
    );

    let surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &ReviewBodyPolicy::default(),
        run_pass: crate::RunPass::Manual,
        post_review_on: &[],
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &[],
        summary_only_findings: &[],
        observations: &[follow_up_observation],
        proof_receipts: &[],
        final_follow_up_tasks: 2,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;

    assert!(!surface.should_prepare_github_review);
    assert_eq!(surface.review_payload_status, "skipped_empty_smoke");
    assert_eq!(surface.terminal_state.status, "sufficient");
    assert_eq!(surface.terminal_state.final_follow_up_tasks, 2);
    assert!(surface.github_review.body.is_empty());
    assert!(surface.github_review.comments.is_empty());
    Ok(())
}

#[test]
fn compiler_surface_keeps_resolved_check_artifact_only() -> Result<()> {
    let args = test_run_args(Path::new("target/ub-review").to_path_buf());
    let plan = test_plan(Vec::new());
    let diff = test_diff();
    let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
    let resolved_observation = test_observation(
        "tests-oracle",
        "Prior author reply already answered the test-proof question.",
        "resolved-check",
        "covered",
        "low",
        "high",
        "prior-test-proof-resolved",
    );

    let surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &ReviewBodyPolicy::default(),
        run_pass: crate::RunPass::Manual,
        post_review_on: &[],
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &[],
        summary_only_findings: &[],
        observations: &[resolved_observation],
        proof_receipts: &[],
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;

    assert!(!surface.should_prepare_github_review);
    assert_eq!(surface.review_payload_status, "skipped_empty_smoke");
    assert_eq!(surface.terminal_state.status, "sufficient");
    assert!(surface.github_review.body.is_empty());
    assert!(surface.github_review.comments.is_empty());
    Ok(())
}

#[test]
fn compiler_surface_suppresses_policy_rejected_artifact_only_pr_body() -> Result<()> {
    let args = test_run_args(Path::new("target/ub-review").to_path_buf());
    let plan = test_plan(Vec::new());
    let diff = test_diff();
    let model_lanes = vec![model_lane_receipt("workflow-opposition", "ok")];
    let summary_only_findings = vec![SummaryOnlyFinding {
        lane: "workflow-opposition".to_owned(),
        severity: "low".to_owned(),
        confidence: "medium".to_owned(),
        reason: "No blocking finding after bounded review; residual risk remains for human review."
            .to_owned(),
        evidence: "bounded lane summary".to_owned(),
    }];

    let surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &ReviewBodyPolicy::default(),
        run_pass: crate::RunPass::Manual,
        post_review_on: &[],
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &[],
        summary_only_findings: &summary_only_findings,
        observations: &[],
        proof_receipts: &[],
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;

    assert!(!surface.should_prepare_github_review);
    assert_eq!(surface.review_payload_status, "skipped_artifact_only_body");
    assert_eq!(surface.terminal_state.status, "sufficient");
    assert!(surface.github_review.body.is_empty());
    assert!(surface.github_review.comments.is_empty());
    Ok(())
}

#[test]
fn compiler_surface_caps_inline_comments_after_value_ranking() -> Result<()> {
    let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
    args.max_inline_comments = 1;
    let plan = test_plan(Vec::new());
    let diff = test_diff();
    let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
    let inline_comments = vec![
        ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            path: "src/main.rs".to_owned(),
            line: 100,
            side: "RIGHT".to_owned(),
            body: "[tests-oracle] Low-value candidate arrived first.".to_owned(),
            evidence: "arrival-order fixture".to_owned(),
            suggestion: None,
        },
        ReviewInlineComment {
            lane: "security".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: "src/main.rs".to_owned(),
            line: 101,
            side: "RIGHT".to_owned(),
            body: "[security] High-value candidate arrived after the cap would have filled."
                .to_owned(),
            evidence: "ranking fixture".to_owned(),
            suggestion: None,
        },
    ];

    let surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &ReviewBodyPolicy::default(),
        run_pass: crate::RunPass::Manual,
        post_review_on: &[],
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &inline_comments,
        summary_only_findings: &[],
        observations: &[],
        proof_receipts: &[],
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;

    assert!(surface.should_prepare_github_review);
    assert_eq!(surface.review_payload_status, "prepared");
    assert_eq!(surface.terminal_state.status, "needs-reviewer-attention");
    assert_eq!(surface.github_review.comments.len(), 1);
    assert_eq!(surface.github_review.comments[0].line, 101);
    assert!(surface.github_review.body.contains("High-value candidate"));
    assert!(!surface.github_review.body.contains("Low-value candidate"));
    assert!(
        surface
            .artifact_body
            .contains("High-value candidate arrived after the cap")
    );
    assert!(
        surface
            .artifact_body
            .contains("Low-value candidate arrived first")
    );
    Ok(())
}

/// Substantive summary-only finding (severity medium+) whose reason trips
/// the boilerplate suppressor when rendered into the PR body.
fn substantive_summary_finding() -> SummaryOnlyFinding {
    SummaryOnlyFinding {
        lane: "opposition".to_owned(),
        severity: "medium".to_owned(),
        confidence: "medium-high".to_owned(),
        reason: "Residual risk remains for human review in the resize realloc ordering.".to_owned(),
        evidence: "diff hunk src/lib.rs:42".to_owned(),
    }
}

/// Non-substantive summary-only finding (severity low, confidence medium)
/// whose reason trips the boilerplate suppressor when rendered.
fn non_substantive_summary_finding() -> SummaryOnlyFinding {
    SummaryOnlyFinding {
        lane: "workflow-opposition".to_owned(),
        severity: "low".to_owned(),
        confidence: "medium".to_owned(),
        reason: "No blocking finding after bounded review; residual risk remains for human review."
            .to_owned(),
        evidence: "bounded lane summary".to_owned(),
    }
}

fn compile_summary_only_surface(
    summary_only_body: SummaryOnlyBodyPolicy,
    summary_only_findings: &[SummaryOnlyFinding],
) -> Result<crate::CompiledReviewSurface> {
    let args = test_run_args(Path::new("target/ub-review").to_path_buf());
    let plan = test_plan(Vec::new());
    let diff = test_diff();
    let model_lanes = vec![model_lane_receipt("opposition", "ok")];
    let review_body_policy = ReviewBodyPolicy {
        summary_only_body,
        ..ReviewBodyPolicy::default()
    };
    compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &review_body_policy,
        run_pass: crate::RunPass::Manual,
        post_review_on: &[],
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &[],
        summary_only_findings,
        observations: &[],
        proof_receipts: &[],
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })
}

#[test]
fn summary_only_finding_substantive_classification() {
    assert!(crate::summary_only_finding_is_substantive(
        &substantive_summary_finding()
    ));
    let mut confidence_only = substantive_summary_finding();
    confidence_only.severity = "low".to_owned();
    assert!(
        crate::summary_only_finding_is_substantive(&confidence_only),
        "confidence medium-high alone should qualify"
    );
    assert!(!crate::summary_only_finding_is_substantive(
        &non_substantive_summary_finding()
    ));
    let lane_status_note = SummaryOnlyFinding {
        lane: "tests-oracle".to_owned(),
        severity: "high".to_owned(),
        confidence: "high".to_owned(),
        reason: "Lane reviewed the packet without findings.".to_owned(),
        evidence: "lane model summary".to_owned(),
    };
    assert!(
        !crate::summary_only_finding_is_substantive(&lane_status_note),
        "pure lane-status notes never count as substantive"
    );
}

#[test]
fn summary_only_body_suppress_withholds_substantive_findings() -> Result<()> {
    let findings = vec![substantive_summary_finding()];
    let surface = compile_summary_only_surface(SummaryOnlyBodyPolicy::Suppress, &findings)?;
    assert!(!surface.should_prepare_github_review);
    assert!(!surface.summary_only_policy_posted);
    assert_eq!(surface.review_payload_status, "skipped_artifact_only_body");
    assert!(surface.github_review.body.is_empty());
    assert_eq!(surface.terminal_state.summary_only_findings, 1);
    assert_eq!(surface.terminal_state.substantive_summary_only_findings, 1);
    Ok(())
}

#[test]
fn summary_only_body_post_substantive_posts_substantive_findings() -> Result<()> {
    let findings = vec![
        non_substantive_summary_finding(),
        substantive_summary_finding(),
    ];
    let surface = compile_summary_only_surface(SummaryOnlyBodyPolicy::PostSubstantive, &findings)?;
    assert!(
        surface.should_prepare_github_review,
        "post_substantive should post a body with a substantive finding: {}",
        surface.terminal_state.reason
    );
    assert!(surface.summary_only_policy_posted);
    assert_eq!(surface.review_payload_status, "prepared");
    assert!(
        surface
            .github_review
            .body
            .contains("Residual risk remains for human review"),
        "posted body should keep the finding content: {}",
        surface.github_review.body
    );
    assert_eq!(surface.terminal_state.status, "needs-reviewer-attention");
    assert_eq!(surface.terminal_state.substantive_summary_only_findings, 1);
    Ok(())
}

#[test]
fn summary_only_body_post_substantive_withholds_non_substantive_findings() -> Result<()> {
    let findings = vec![non_substantive_summary_finding()];
    let surface = compile_summary_only_surface(SummaryOnlyBodyPolicy::PostSubstantive, &findings)?;
    assert!(!surface.should_prepare_github_review);
    assert!(!surface.summary_only_policy_posted);
    assert_eq!(surface.review_payload_status, "skipped_artifact_only_body");
    assert!(surface.github_review.body.is_empty());
    assert_eq!(surface.terminal_state.summary_only_findings, 1);
    assert_eq!(surface.terminal_state.substantive_summary_only_findings, 0);
    Ok(())
}

#[test]
fn summary_only_body_post_all_posts_non_substantive_findings() -> Result<()> {
    let findings = vec![non_substantive_summary_finding()];
    let surface = compile_summary_only_surface(SummaryOnlyBodyPolicy::PostAll, &findings)?;
    assert!(
        surface.should_prepare_github_review,
        "post_all should post whenever any summary-only finding exists: {}",
        surface.terminal_state.reason
    );
    assert!(surface.summary_only_policy_posted);
    assert_eq!(surface.review_payload_status, "prepared");
    assert!(!surface.github_review.body.is_empty());
    Ok(())
}

#[test]
fn summary_only_body_post_all_does_not_create_payload_without_summary_findings() -> Result<()> {
    // A zero inline cap leaves no PR-facing comments and no summary-only
    // findings; the summary-only knob must not create reviewer content.
    let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
    args.max_inline_comments = 0;
    let plan = test_plan(Vec::new());
    let diff = test_diff();
    let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
    let inline_comments = (0..3)
        .map(|index| ReviewInlineComment {
            lane: "tests-oracle".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            path: "src/main.rs".to_owned(),
            line: 100 + index,
            side: "RIGHT".to_owned(),
            body: format!("Confirm concise-review guard boundary {index}."),
            evidence: "generated regression fixture".to_owned(),
            suggestion: None,
        })
        .collect::<Vec<_>>();
    let review_body_policy = ReviewBodyPolicy {
        summary_only_body: SummaryOnlyBodyPolicy::PostAll,
        ..ReviewBodyPolicy::default()
    };

    let surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &review_body_policy,
        run_pass: crate::RunPass::Manual,
        post_review_on: &[],
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &inline_comments,
        summary_only_findings: &[],
        observations: &[],
        proof_receipts: &[],
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;

    assert!(!surface.should_prepare_github_review);
    assert!(!surface.summary_only_policy_posted);
    assert_eq!(surface.review_payload_status, "skipped_empty_smoke");
    assert!(surface.github_review.body.is_empty());
    assert!(surface.github_review.comments.is_empty());
    Ok(())
}

#[test]
fn summary_only_withheld_terminal_reason_names_policy_and_counts() {
    // A value-changing proof receipt keeps reviewer_value_present true so
    // the terminal state takes the withheld-payload branch.
    let args = test_run_args(Path::new("target/ub-review").to_path_buf());
    let plan = test_plan(Vec::new());
    let findings = vec![non_substantive_summary_finding()];
    let proof_receipts = vec![test_proof_receipt("discriminating", "ok")];
    let state = build_review_terminal_state(TerminalStateInput {
        args: &args,
        run_pass: crate::RunPass::Manual,
        plan: &plan,
        review_payload_status: "skipped_artifact_only_body",
        should_prepare_github_review: false,
        pr_body: "",
        inline_comments: &[],
        summary_only_findings: &findings,
        summary_only_body: SummaryOnlyBodyPolicy::PostSubstantive,
        model_lanes: &[],
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        proof_receipts: &proof_receipts,
        final_follow_up_tasks: 0,
    });

    assert_eq!(state.status, "needs-reviewer-attention");
    assert!(
        state
            .reason
            .contains("summary_only_body = `post_substantive`"),
        "terminal reason should name the policy value: {}",
        state.reason
    );
    assert!(
        state
            .reason
            .contains("1 summary-only findings, 0 substantive"),
        "terminal reason should name the counts: {}",
        state.reason
    );
    assert!(
        !state
            .reason
            .contains("withheld as no-value boilerplate; diagnostics"),
        "old unconditional wording should be gone: {}",
        state.reason
    );
    assert_eq!(state.substantive_summary_only_findings, 0);
}

#[test]
fn review_body_waiver_skips_suppressible_checks_only() -> Result<()> {
    let policy = ReviewBodyPolicy::default();
    let boilerplate_body = "## Confirmed findings\n\n- [opposition] Residual risk remains for human review in the resize path.";
    assert!(
        crate::validate_pr_review_body_policy(boilerplate_body, &policy).is_err(),
        "boilerplate body must still fail without the waiver"
    );
    crate::validate_pr_review_body_policy_with_waiver(boilerplate_body, &policy, true)?;

    let sensor_table_body = "## Confirmed findings\n\n- A finding.\n\n## Sensor status\n\n- ok";
    let err = crate::validate_pr_review_body_policy_with_waiver(sensor_table_body, &policy, true)
        .err()
        .ok_or_else(|| anyhow::anyhow!("sensor table unexpectedly passed under waiver"))?;
    assert!(err.to_string().contains("sensor status table"), "{err:#}");
    Ok(())
}

#[test]
fn review_body_summary_only_body_parses_known_values() -> Result<()> {
    for (value, expected) in [
        ("suppress", SummaryOnlyBodyPolicy::Suppress),
        ("post_substantive", SummaryOnlyBodyPolicy::PostSubstantive),
        ("post-substantive", SummaryOnlyBodyPolicy::PostSubstantive),
        ("post_all", SummaryOnlyBodyPolicy::PostAll),
        ("post-all", SummaryOnlyBodyPolicy::PostAll),
    ] {
        let config = Config::from_toml_with_policy_receipts(&format!(
            "[review_body]\nsummary_only_body = \"{value}\"\n"
        ))?;
        assert!(
            config.policy_errors.is_empty(),
            "`{value}` should parse without policy errors: {:?}",
            config.policy_errors
        );
        assert_eq!(config.review_body.summary_only_body, expected, "{value}");
    }
    assert_eq!(
        Config::default().review_body.summary_only_body,
        SummaryOnlyBodyPolicy::Suppress,
        "consumer default must stay suppress"
    );
    Ok(())
}

#[test]
fn review_body_unknown_policy_values_are_receipted() -> Result<()> {
    let config = Config::from_toml_with_policy_receipts(
        "[review_body]\ninclude_successful_lane_table = true\nsummary_only_body = \"post-everything\"\n",
    )?;
    assert_eq!(config.policy_errors.len(), 1, "{:?}", config.policy_errors);
    assert_eq!(
        config.policy_errors[0].section,
        "review_body.summary_only_body"
    );
    assert!(
        config.policy_errors[0].detail.contains("summary_only_body"),
        "{:?}",
        config.policy_errors[0]
    );
    // Valid siblings keep working; the bad key falls back to the default.
    assert!(config.review_body.include_successful_lane_table);
    assert_eq!(
        config.review_body.summary_only_body,
        SummaryOnlyBodyPolicy::Suppress
    );

    let misspelled = Config::from_toml_with_policy_receipts(
        "[review_body]\nsummary_only_bodyy = \"suppress\"\n",
    )?;
    assert_eq!(
        misspelled.policy_errors.len(),
        1,
        "{:?}",
        misspelled.policy_errors
    );
    assert_eq!(
        misspelled.policy_errors[0].section,
        "review_body.summary_only_bodyy"
    );
    Ok(())
}

#[test]
fn post_validation_honors_effective_summary_only_body_policy() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let run_dir = temp.path();
    let review_dir = run_dir.join("review");
    fs::create_dir_all(&review_dir)?;
    let args = PostArgs {
        review_json: review_dir.join("github-review.json"),
        diff_patch: None,
        out: review_dir.clone(),
        github_token: Some("token".to_owned()),
        repo: Some("EffortlessMetrics/ub-review".to_owned()),
        pull_number: Some(1),
        github_api_url: "https://api.github.com".to_owned(),
        fail_on_post_error: false,
    };
    let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "## Confirmed findings\n\n- [opposition] Residual risk remains for human review in the resize path.".to_owned(),
            comments: Vec::new(),
        };

    // Without an effective config the conservative default rejects the body.
    let err = crate::validate_github_review_payload_for_post(&args, &review)
        .err()
        .ok_or_else(|| anyhow::anyhow!("boilerplate body unexpectedly passed post checks"))?;
    assert!(
        err.to_string().contains("artifact-only boilerplate"),
        "{err:#}"
    );

    // With a posting posture in effective-config.json the post step honors
    // the run's compile decision and waives the suppressible classes.
    let mut config = Config::default();
    config.review_body.summary_only_body = SummaryOnlyBodyPolicy::PostSubstantive;
    fs::write(
        run_dir.join("effective-config.json"),
        serde_json::to_vec_pretty(&config)?,
    )?;
    crate::validate_github_review_payload_for_post(&args, &review)?;
    Ok(())
}

fn test_pass_policy_inline_comment() -> ReviewInlineComment {
    ReviewInlineComment {
        lane: "tests-oracle".to_owned(),
        severity: "medium".to_owned(),
        confidence: "high".to_owned(),
        path: "src/main.rs".to_owned(),
        line: 100,
        side: "RIGHT".to_owned(),
        body: "Confirm the resize path cannot alias the detached buffer.".to_owned(),
        evidence: "generated regression fixture".to_owned(),
        suggestion: None,
    }
}

#[test]
fn compiler_surface_skips_pass_excluded_by_post_review_on() -> Result<()> {
    let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
    args.posting = PostingMode::Review;
    let plan = test_plan(Vec::new());
    let diff = test_diff();
    let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
    let inline_comments = vec![test_pass_policy_inline_comment()];
    let two_pass: Vec<String> = vec!["opened".to_owned(), "ready_for_review".to_owned()];

    let surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &ReviewBodyPolicy::default(),
        run_pass: crate::RunPass::Synchronize,
        post_review_on: &two_pass,
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &inline_comments,
        summary_only_findings: &[],
        observations: &[],
        proof_receipts: &[],
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;

    assert!(!surface.should_prepare_github_review);
    assert_eq!(surface.review_payload_status, "skipped_pass_policy");
    assert_eq!(surface.terminal_state.status, "needs-reviewer-attention");
    assert!(surface.terminal_state.reviewer_value_present);
    assert!(
        surface
            .terminal_state
            .reason
            .contains("pass `synchronize` is not in [gate].post_review_on"),
        "terminal reason should name the pass policy: {}",
        surface.terminal_state.reason
    );
    Ok(())
}

#[test]
fn compiler_surface_preserves_unsafe_review_suggestions() -> Result<()> {
    let args = test_run_args(Path::new("target/ub-review").to_path_buf());
    let plan = test_plan(Vec::new());
    let diff = test_diff();
    let model_lanes = vec![model_lane_receipt("unsafe-review", "ok")];
    let inline_comments = vec![ReviewInlineComment {
        lane: "unsafe-review".to_owned(),
        severity: "medium".to_owned(),
        confidence: "medium-high".to_owned(),
        path: "src/main.rs".to_owned(),
        line: 100,
        side: "RIGHT".to_owned(),
        body: "[unsafe-review] Guard evidence is missing.".to_owned(),
        evidence: "unsafe-review comment-plan card-001".to_owned(),
        suggestion: Some("let header = guarded_header_read(ptr)?;".to_owned()),
    }];

    let surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &ReviewBodyPolicy::default(),
        run_pass: crate::RunPass::Manual,
        post_review_on: &[],
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &inline_comments,
        summary_only_findings: &[],
        observations: &[],
        proof_receipts: &[],
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;

    assert!(surface.should_prepare_github_review);
    assert_eq!(
        surface.github_review.comments[0].suggestion.as_deref(),
        Some("let header = guarded_header_read(ptr)?;")
    );
    Ok(())
}

#[test]
fn compiler_surface_prepares_review_for_synchronize_pass_in_profile_list() -> Result<()> {
    let mut args = test_run_args(Path::new("target/ub-review").to_path_buf());
    args.posting = PostingMode::Review;
    let plan = test_plan(Vec::new());
    let diff = test_diff();
    let model_lanes = vec![model_lane_receipt("tests-oracle", "ok")];
    let inline_comments = vec![test_pass_policy_inline_comment()];
    let every_pass: Vec<String> = vec![
        "opened".to_owned(),
        "reopened".to_owned(),
        "ready_for_review".to_owned(),
        "synchronize".to_owned(),
    ];

    let surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &ReviewBodyPolicy::default(),
        run_pass: crate::RunPass::Synchronize,
        post_review_on: &every_pass,
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &inline_comments,
        summary_only_findings: &[],
        observations: &[],
        proof_receipts: &[],
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;

    assert!(
        surface.should_prepare_github_review,
        "synchronize pass in post_review_on should keep the payload prepared: {}",
        surface.terminal_state.reason
    );
    assert_eq!(surface.review_payload_status, "prepared");
    assert_eq!(surface.terminal_state.status, "needs-reviewer-attention");
    assert_eq!(surface.github_review.comments.len(), 1);
    assert_eq!(
        surface.terminal_state.reason,
        "Reviewer-value content survived compilation; a grouped PR review was prepared."
    );
    Ok(())
}

#[test]
fn artifact_review_body_keeps_model_lane_roster() {
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[model_lane_receipt("ub-memory-lifetime", "ok")],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[] as &[SummaryOnlyFinding],
        &[] as &[Observation],
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::Artifact,
    );

    assert!(body.contains("## Model lanes"));
    assert!(body.contains("Lane: `ub-memory-lifetime`"));
    assert!(body.contains("Provider: `minimax`"));
    assert!(body.contains("Model: `MiniMax-M3`"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn workflow_pr_body_uses_workflow_route_language() {
    let mut plan = test_plan(Vec::new());
    plan.diff_class = DiffClass::WorkflowTooling;
    plan.lanes =
        default_lanes_for_diff_context(DiffClass::WorkflowTooling, &LanguageMix::default());
    let diff = DiffContext {
        base: "origin/main".to_owned(),
        head: "HEAD".to_owned(),
        changed_files: vec![".github/workflows/ub-review.yml".to_owned()],
        patch: "+permissions:\n+  contents: read\n".to_owned(),
        flags: classify_diff(
            &[".github/workflows/ub-review.yml".to_owned()],
            "+permissions:\n+  contents: read\n",
        ),
        diff_class: DiffClass::WorkflowTooling,
    };

    let body = render_review_body(
        "abc123",
        &plan,
        &diff,
        &[model_lane_receipt("workflow-permissions", "ok")],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[] as &[SummaryOnlyFinding],
        &[] as &[Observation],
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.is_empty());
    assert!(!body.contains("ArrayBuffer"));
    assert!(!body.contains("worker handoff"));
    assert!(!body.contains("unsafe/native seams"));
    assert!(!body.contains("test-oracle strength"));
    assert!(!body.contains("actionlint/zizmor"));
    assert!(!body.contains("## Residual risk"));
    assert!(!body.contains("A human should still inspect"));
}

#[test]
fn verifier_script_scope_noise_stays_artifact_only() -> Result<()> {
    let args = test_run_args(Path::new("target/ub-review").to_path_buf());
    let mut plan = test_plan(Vec::new());
    plan.diff_class = DiffClass::WorkflowTooling;
    plan.lanes =
        default_lanes_for_diff_context(DiffClass::WorkflowTooling, &LanguageMix::default());
    let diff = DiffContext {
        base: "origin/main".to_owned(),
        head: "HEAD".to_owned(),
        changed_files: vec!["scripts/verify-bun-review-artifacts.py".to_owned()],
        patch: "+import tempfile\n".to_owned(),
        flags: classify_diff(
            &["scripts/verify-bun-review-artifacts.py".to_owned()],
            "+import tempfile\n",
        ),
        diff_class: DiffClass::WorkflowTooling,
    };
    let model_lanes = vec![model_lane_receipt("workflow-proof", "ok")];
    let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "workflow-pinning".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "workflow-pinning lane has no actionable finding: no action versions, runner images, or setup steps were modified by this Python-only diff.".to_owned(),
                evidence: "Changed files: scripts/verify-bun-review-artifacts.py; no workflow YAML changes.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "No workflow lint proof is applicable because the PR diff does not modify any GitHub Actions YAML. Actionlint availability is not a trust gap for this PR.".to_owned(),
                evidence: "Changed files=1, scripts/verify-bun-review-artifacts.py; actionlint skipped because no workflow files changed.".to_owned(),
            },
        ];
    let self_test_coverage = test_observation(
        "workflow-proof",
        "New self-tests use tempfile.TemporaryDirectory and assert both happy-path and failure-path via expect_self_test_failure; this is a focused smoke proof pattern suitable for Python change verification.",
        "test-gap",
        "open",
        "low",
        "medium",
        "self-test-coverage",
    );
    let mut actionlint_skip = test_observation(
        "workflow-pinning",
        "actionlint/zizmor unavailable (skipped) for this diff; no GitHub Actions YAML changed, so absence of proof is not trust-affecting for pinning review.",
        "missing-evidence",
        "open",
        "low",
        "medium",
        "wp.missing.actionlint-zizmor",
    );
    actionlint_skip.path = Some("scripts/verify-bun-review-artifacts.py".to_owned());
    actionlint_skip.evidence = vec![
        "actionlint skipped - trigger did not match; zizmor disabled by config; no YAML in diff."
            .to_owned(),
    ];
    let mut python_only_scope = test_observation(
        "workflow-opposition",
        "Diff is Python-only in scripts/; no GitHub Actions YAML, permissions, triggers, or action pins touched, so workflow/tooling opposition surfaces are limited to the validator script itself.",
        "verification-question",
        "open",
        "medium",
        "medium-high",
        "empty-candidates-dir-acceptance",
    );
    python_only_scope.path = Some("scripts/verify-bun-review-artifacts.py".to_owned());
    let mut trust_language_softening = test_observation(
        "opposition",
        "Observation text for actionlint/zizmor pinning changed from 'unverified by sensors for this run' to 'unavailability is not trust-affecting' - this is a softer trust claim that should be defended by a concrete rule, not just narrative softening.",
        "source-route-gap",
        "open",
        "medium",
        "medium",
        "source-route/trust-language-softening",
    );
    trust_language_softening.path = Some("src/main.rs".to_owned());
    trust_language_softening.evidence = vec![
            "String literal changed to '...absence of proof is not trust-affecting for pinning review' while the new gap-noise clause now matches 'no yaml in diff' and 'no github actions yaml'."
                .to_owned(),
        ];
    let self_test_receipt = test_observation(
        "proof-planner",
        "Self-test wiring is in run_self_tests; if --self-test is not executed in CI, the new branches are unverified at gate time. PR body asserts it ran, but receipt not in seeded thread.",
        "verification-question",
        "open",
        "medium",
        "medium",
        "proof-planner.self-test-evidence",
    );
    let observations = vec![
        self_test_coverage,
        actionlint_skip,
        python_only_scope,
        trust_language_softening,
        self_test_receipt,
    ];

    let surface = compile_review_surface(ReviewCompilerInput {
        shared_context_id: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
        review_body_policy: &ReviewBodyPolicy::default(),
        run_pass: crate::RunPass::Manual,
        post_review_on: &[],
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &model_lanes,
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &[],
        summary_only_findings: &summary_only_findings,
        observations: &observations,
        proof_receipts: &[],
        final_follow_up_tasks: 0,
        suggested_issues: &[],
        reporter_distillation: None,
    })?;

    assert!(
        !surface.should_prepare_github_review,
        "{}",
        surface.github_review.body
    );
    assert_eq!(surface.review_payload_status, "skipped_empty_smoke");
    assert_eq!(surface.terminal_state.status, "sufficient");
    assert!(surface.github_review.body.is_empty());
    assert!(surface.github_review.comments.is_empty());
    assert!(surface.artifact_body.contains("workflow-pinning lane"));
    Ok(())
}

#[test]
fn pr_review_body_hides_machine_metadata_for_findings() {
    let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[ReviewInlineComment {
                lane: "opposition".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                path: "src/postgres.rs".to_owned(),
                line: 196,
                side: "RIGHT".to_owned(),
                body: "Confirm the C++ copy cannot race detach or resize between the Rust guard and native read.".to_owned(),
                evidence: "line 196 calls Bun__createArrayBufferForCopy".to_owned(),
                suggestion: None,
            }],
            &[] as &[SummaryOnlyFinding],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

    assert!(body.contains("## Confirmed findings"));
    assert!(body.contains("Confirm the C++ copy cannot race detach or resize"));
    assert!(!body.contains("Shared context"));
    assert!(!body.contains("Profile:"));
    assert!(!body.contains("Changed files:"));
    assert!(!body.contains("Inline comments:"));
    assert!(!body.contains("`[opposition]`"));
    assert!(!body.contains("medium-high"));
    assert!(!body.contains("src/postgres.rs"));
    assert!(!body.contains("Evidence:"));
    assert!(!body.contains("line 196 calls"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_keeps_compiler_residue_artifact_only() {
    let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "source-route".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "inline guard rejected src/lib.rs:12; severity_allowed=true confidence_allowed=true line_valid=false concise=true body_present=true evidence_present=true repo_relative=true".to_owned(),
                evidence: "compiler guard metadata".to_owned(),
            }],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

    assert!(body.is_empty());
    assert!(!body.contains("inline guard rejected"));
    assert!(!body.contains("severity_allowed"));
    assert!(!body.contains("compiler guard metadata"));
    assert!(!body.contains("## Confirmed findings"));
    assert!(!body.contains("## Verification questions"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_keeps_clean_lane_summary_artifact_only() {
    let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Workflow-permissions lane: no token-scope, permissions, pull_request_target, or fork-vector change. Lane is clean.".to_owned(),
                evidence: "lane model summary".to_owned(),
            }],
            &[] as &[Observation],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

    assert!(body.is_empty());
    assert!(!body.contains("Lane is clean"));
}

#[test]
fn pr_review_body_drops_stale_workflow_cache_path_summary() {
    let mut diff = test_diff();
    diff.diff_class = DiffClass::WorkflowTooling;
    diff.changed_files = vec![".github/workflows/ub-review-packet.yml".to_owned()];
    diff.patch =
        "+ key: ub-review-gh-runner-v2-c52b0e4403384cc7836e162a54df005cbcab968d\n".to_owned();
    let finding = SummaryOnlyFinding {
            lane: "workflow-proof".to_owned(),
            severity: "high".to_owned(),
            confidence: "medium-high".to_owned(),
            reason: "actionlint sensor ran ok, but the diff adds '~/go/bin/actionlint' to the cache path block and prior seeded reviews failed; attach a fresh actionlint proof.".to_owned(),
            evidence: "cache path '~/go/bin/actionlint' added in hunk with no install step visible.".to_owned(),
        };
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &diff,
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        std::slice::from_ref(&finding),
        &[] as &[Observation],
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.is_empty());
    assert!(!body.contains("actionlint"));

    diff.patch.push_str("+            ~/go/bin/actionlint\n");
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &diff,
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[finding],
        &[] as &[Observation],
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.contains("actionlint"));
}

#[test]
fn pr_review_body_keeps_workflow_status_residue_artifact_only() {
    let mut diff = test_diff();
    diff.diff_class = DiffClass::WorkflowTooling;
    diff.changed_files = vec![".github/workflows/ub-review-packet.yml".to_owned()];
    diff.patch = concat!(
            "+          key: ub-review-gh-runner-v2-4dfbd9d7caeff4f506984c63cc36f248233206e6-${{ runner.os }}-rust-1.95-core\n",
            "+            ub-review-gh-runner-v2-4dfbd9d7caeff4f506984c63cc36f248233206e6-${{ runner.os }}-rust-1.95-\n",
            "+        uses: EffortlessMetrics/ub-review@4dfbd9d7caeff4f506984c63cc36f248233206e6\n",
        )
        .to_owned();
    let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "workflow-permissions lane: no permissions, pull_request_target, token-scope, or fork-vector change in this diff. No new auth surface. actionlint ok in this packet.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "Pin of EffortlessMetrics/ub-review to a specific commit SHA is supply-chain tightening and inherits that repo's default GITHUB_TOKEN scope; no new scope is requested in the workflow itself. Worth a one-line note for future audits that third-party action token scope is not visible here.".to_owned(),
                evidence: "uses: EffortlessMetrics/ub-review@4dfbd9d7caeff4f506984c63cc36f248233206e6 with preset/profile inputs only; no permissions: override.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "refuter demoted inline candidate at .github/workflows/ub-review-packet.yml:56: the PR packet contains no upstream commit-existence/ancestry proof, and the PR body says 'Gate proof is pending this PR's UB evidence packet.' Confirming reachability requires an external API call that the lane cannot perform from cached context.".to_owned(),
                evidence: "uses: EffortlessMetrics/ub-review@4dfbd9d7caeff4f506984c63cc36f248233206e6 - only local diff evidence, no upstream commit proof".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-pinning".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "Ub-review action receives secrets.MINIMAX_API_KEY and github.token at runtime; a malicious or compromised dad0f23 would exfiltrate these. Pinning to SHA is correct posture but does not eliminate upstream trust.".to_owned(),
                evidence: "The diff is an atomic ub-review SHA/cache-key bump; the with: block is unchanged.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-pinning".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "No pinning defect introduced. The only standing concern is upstream SHA trust for EffortlessMetrics/ub-review@e76ccbc, which is identical in posture to the prior pin and is a repo-level policy item, not a diff finding.".to_owned(),
                evidence: "Per-action full-SHA pinning preserved; old pin fully removed; 3 replacement sites consistent; actionlint ok; workflow file diff is 4 lines, no perm/trigger change.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "The diff is a 4-line mechanical SHA bump (da14100 -> 88f3dcc7c344f8b54871caea2122de0b68701925) at the three expected sites: cache `key` (line 56), `restore-keys` prefix (line 58), and action `uses:` (line 62). No permission, trigger, or `with:` block change; net new secret/permission surface relative to the prior pin is zero.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Workflow-proof lane: actionlint receipt ok; no Rust/source surface touched (yaml-only). Pin/uses ref consistent at 3x ec8f890; old da14100 absent. Strongest failed objection: actionlint proof receipt not inlined for me to re-verify, and no fresh PR-build smoke. cursor/coderabbit stale SHA mismatch is a false positive (cites e76ccbc while PR targets ec8f890).".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "actionlint 'ok' status is reported by the sensor table but the underlying lint output is not inlined into the workflow-proof lane packet; trust in 'no workflow lint findings' therefore depends on the central proof broker artifact at sensors/actionlint/. For a 4-line SHA-swap with consistent 40-hex pin and unchanged permissions/trigger, this is a parked follow-up, not a blocker.".to_owned(),
                evidence: "sensor table: actionlint=ok; receipt path sensors/actionlint/; yaml-only diff; pin 40-hex non-zero".to_owned(),
            },
        ];
    let mut tokmd_gap = test_observation(
        "sensor-tokmd",
        "Sensor `tokmd` evidence is `missing`: command not found",
        "missing-evidence",
        "open",
        "low",
        "high",
        "sensor-tokmd-missing",
    );
    tokmd_gap.evidence = vec!["command not found".to_owned()];
    let observations = vec![
        test_observation(
            "workflow-permissions",
            "workflow-permissions lane: no permissions, pull_request_target, token-scope, or fork-vector change in this diff. No new auth surface. actionlint ok in this packet.",
            "bug",
            "open",
            "low",
            "medium",
            "bug:workflow-permissions",
        ),
        test_observation(
            "workflow-permissions",
            "Workflow-level permissions block on the caller workflow was not modified; perms posture remains whatever the base file declares (pre-existing, not a diff target).",
            "verification-question",
            "open",
            "low",
            "high",
            "wf-perms-unchanged",
        ),
        test_observation(
            "workflow-pinning",
            "Ub-review action receives secrets.MINIMAX_API_KEY and github.token at runtime; a malicious or compromised dad0f23 would exfiltrate these. Pinning to SHA is correct posture but does not eliminate upstream trust.",
            "security-risk",
            "open",
            "medium",
            "medium-high",
            "wf-pin-secrets-surface",
        ),
        test_observation(
            "workflow-pinning",
            "Pinning format could be 39-hex or all-zero making the gate unsafe.; refuted because: e76ccbcbe94258fd03cf6ddb4e1536833cad610d is 40 hex characters, non-zero, and matches expected SHA-1 shape; the gate's SHA-pinning control remains effective.",
            "false-premise",
            "refuted",
            "low",
            "high",
            "false-premise:workflow-pinning",
        ),
        test_observation(
            "workflow-permissions",
            "cursor[bot] and coderabbitai[bot] comments claim target is e76ccbcb... and demand swap back; PR body, diff, and head tree all show ec8f890 as the actual target. Their objection is a false positive against the current diff and reopens nothing.",
            "false-premise",
            "refuted",
            "low",
            "high",
            "wf-perms-stale-cursor-coderabbit",
        ),
        test_observation(
            "workflow-proof",
            "actionlint receipt is 'ok' per sensor table; no per-line output inlined into this lane packet, so re-verification of lint findings depends on the central proof broker artifact",
            "missing-evidence",
            "open",
            "low",
            "medium",
            "wf-proof:actionlint-receipt-inlined?",
        ),
        test_observation(
            "workflow-proof",
            "No fresh PR-build smoke run is available (build/test skipped, --allow-heavy required); only tokmd/actionlint receipts are present for this 4-line workflow pin",
            "missing-evidence",
            "open",
            "low",
            "high",
            "missing-evidence:.github/workflows/ub-review-packet.yml:62",
        ),
        test_observation(
            "orchestrator-follow-up",
            "`pull_request` types limited to `opened` and `ready_for_review` cause pushes to skip re-running the UB packet.",
            "bug",
            "open",
            "medium",
            "medium",
            "follow-up-trigger-scope",
        ),
        test_observation(
            "workflow-proof",
            "actionlint install step absent from the changed hunk: a future runner without preinstalled ~/go/bin/actionlint would cache-restore an empty path.",
            "bug",
            "open",
            "medium",
            "medium",
            "workflow-tool-cache-path",
        ),
        test_observation(
            "workflow-pinning",
            "v2-4dfbd9d7 cache key will collide on a future re-pin sharing the same prefix; refuted because the current diff uses the full 40-hex SHA in both key and restore-keys.",
            "false-premise",
            "refuted",
            "low",
            "high",
            "sha-prefix-refuted",
        ),
        test_observation(
            "workflow-permissions",
            "third-party action EffortlessMetrics/ub-review@4fdab02 may inherit broader token scope and widen permissions; refuted because no permissions key changed, and pinning to a SHA is supply-chain tightening not a scope change.",
            "false-premise",
            "refuted",
            "low",
            "high",
            "third-party-token-scope-refuted",
        ),
        test_observation(
            "workflow-pinning",
            "Floating @v0.1 tag is a supply-chain widening risk that this PR fails to close; refuted because pinning to immutable commit SHA is strict supply-chain tightening.",
            "false-premise",
            "refuted",
            "low",
            "high",
            "floating-tag-refuted",
        ),
        test_observation(
            "workflow-pinning",
            "Cache key/restore-keys embedded with full 40-hex SHA prefix; prior 7-char prefix collision objection is now resolved by using the entire SHA.",
            "residual-risk",
            "parked",
            "low",
            "high",
            "full-sha-prefix-resolved",
        ),
        test_observation(
            "workflow-pinning",
            "pull_request trigger scoped to [opened, ready_for_review] means new pushes on an open PR do not re-run the UB packet; reviewer evidence can stale as HEAD advances (cursor bug 40e40af3).",
            "residual-risk",
            "open",
            "low",
            "high",
            "cursor-trigger-stale",
        ),
        test_observation(
            "workflow-proof",
            "PR-body contract hardening claimed for ccae442 is not verifiable from the repo diff itself; trust depends on upstream tag of EffortlessMetrics/ub-review@ccae442.",
            "verification-question",
            "open",
            "medium",
            "medium",
            "pr-body-contract-meta",
        ),
        test_observation(
            "workflow-opposition",
            "Cache key/uses ref must be a single coherent bump; if SHA were 39-hex or all-zero the gate would be unsafe. Ccae442de05bb9330e5d45bbbebfd64c6c39ee93 is 40 hex and non-zero.",
            "verification-question",
            "open",
            "medium",
            "medium",
            "sha-format-meta",
        ),
        test_observation(
            "orchestrator-follow-up",
            "The cached prior observation still matches the current PR evidence and is not reopened by the diff.",
            "bug",
            "open",
            "low",
            "medium",
            "cached-prior-meta",
        ),
        test_observation(
            "orchestrator-follow-up",
            "The refutation claiming this PR is a benign SHA pin bump with no workflow posture impact still matches current evidence.",
            "bug",
            "open",
            "low",
            "medium",
            "refutation-meta",
        ),
        test_observation(
            "workflow-opposition",
            "Workflow posture is unsafe because the new pin dad0f23 receives secrets and is not reproducibly verified in this repo.; refuted because: False-premise at the opposition-lane level: the diff itself adds zero new secret/permission surface relative to the prior pin ccae442; pinning-by-SHA is the established control and is preserved. Trust in upstream tag is a standing-repo concern, not introduced by this 4-line bump.",
            "false-premise",
            "refuted",
            "low",
            "high",
            "false-premise:workflow-opposition",
        ),
        test_observation(
            "workflow-permissions",
            "Actionlint ran ok; combined with disabled zizmor/gitleaks, security-relevant linting for this pin-only diff is partial but the diff class does not require them.",
            "missing-evidence",
            "open",
            "low",
            "medium",
            "tool-status-only-gap",
        ),
        tokmd_gap,
    ];
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &diff,
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &summary_only_findings,
        &observations,
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.is_empty());
    assert!(!body.contains("Needs reviewer attention"));
    assert!(!body.contains("Confirmed findings"));
    assert!(!body.contains("workflow-permissions lane"));
    assert!(!body.contains("ready_for_review"));
    assert!(!body.contains("actionlint install step"));
    assert!(!body.contains("Floating @v0.1"));
    assert!(!body.contains("third-party action"));
    assert!(!body.contains("prefix collision"));
    assert!(!body.contains("tokmd"));
    assert!(!body.contains("cached prior observation"));
    assert!(!body.contains("refuter demoted inline candidate"));
    assert!(!body.contains("Gate proof is pending"));
    assert!(!body.contains("40 hex and non-zero"));
    assert!(!body.contains("Actionlint ran ok"));
    assert!(!body.contains("malicious or compromised"));
    assert!(!body.contains("does not eliminate upstream trust"));
    assert!(!body.contains("pre-existing, not a diff target"));
    assert!(!body.contains("standing-repo concern"));
    assert!(!body.contains("No pinning defect introduced"));
    assert!(!body.contains("repo-level policy item"));
    assert!(!body.contains("4-line mechanical SHA bump"));
    assert!(!body.contains("net new secret/permission surface"));
    assert!(!body.contains("Pinning format could be 39-hex"));
    assert!(!body.contains("SHA-pinning control remains effective"));
    assert!(!body.contains("cursor[bot]"));
    assert!(!body.contains("coderabbit"));
    assert!(!body.contains("stale-bot false positives"));
    assert!(!body.contains("central proof broker artifact"));
    assert!(!body.contains("No fresh PR-build smoke"));
    assert!(!body.contains("--allow-heavy"));
}

#[test]
fn pr_review_body_keeps_paths_ignore_no_posture_review_artifact_only() {
    let mut diff = test_diff();
    diff.diff_class = DiffClass::WorkflowTooling;
    diff.changed_files = vec![".github/workflows/droid-focused-review.yml".to_owned()];
    diff.patch = concat!(
        " pull_request:\n",
        "   paths-ignore:\n",
        "+    - \".github/workflows/ub-review-packet.yml\"\n",
    )
    .to_owned();
    let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Confirm checkout credential persistence: workflows using pull_request from forks receive a read-only GITHUB_TOKEN; this lane did not change checkout config, so no new persistence vector is introduced. Actionlint receipt 'ok' supports no syntactic regression.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Adding a workflow file to paths-ignore could grant implicit permission expansion; refuted because: paths-ignore only filters trigger activation; it does not alter token scopes, permissions blocks, or any job-level security context.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "zizmor, gitleaks, osv-scanner, cargo-audit, cargo-deny, shellcheck, semgrep, coverage all disabled by config or trigger-mismatched. No security/pinning tool independently re-validated this workflow file.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Confirm no focused smoke proof (workflow_run on a fork-PR dry-run, or a temporary pull_request_target guard test) was executed for the paths-ignore change. Trust rests on actionlint parse only; semantic skip behavior on the droid lane is not proven by sensors.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "PR body states actionlint is not installed locally, so the 'ok' receipt must come from the ub-review gate's own tooling rather than a local pre-push run; trust depends on that gate having actually executed actionlint v1 against this ref.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "CodeRabbit's review-comment at ub-review-packet.yml:58 asserts the PR gate target SHA is 892e1bb44b7cb24753b7701b405d078f4ef11ee1, not be524219e33ff37edeab61ddc28c01250a08b492 used in the diff. If that claim is correct the workflow pin does not match the upstream gate and the packet will be filtered/no-posture.".to_owned(),
                evidence: "CodeRabbit review-comment on .github/workflows/ub-review-packet.yml:58, scripted check showing 0 references to 892e1bb44b... in the file; PR body and droid-ub/droid-tests receipts only confirm internal lockstep, not match to gate target.".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Confirm actionlint receipt 'ok' confirms syntactic validity, but no semantic proof of skip behavior on the droid lane is available; trust rests on actionlint parse plus per-PR trigger semantics - the droid lane is auxiliary/non-blocking and the UB gate is authoritative, so residual workflow risk is bounded.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Residual workflow risk: cache key/restore-keys prefix is coupled to action SHA. Any future repin must update all three sites; a partial update silently mismatches cache restore. Not actionable in this PR (current state is consistent) - parked for follow-up lint rule or script.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "trust gap: no focused smoke proof (workflow_run on fork-PR dry-run or pull_request_target guard) executed for the paths-ignore change; semantic skip behavior on Droid lane unproven beyond actionlint parse.".to_owned(),
                evidence: "workflow lane summary".to_owned(),
            },
        ];
    let observations = vec![
        test_observation(
            "workflow-permissions",
            "Confirm checkout credential persistence: workflows using pull_request from forks receive a read-only GITHUB_TOKEN; this lane did not change checkout config, so no new persistence vector is introduced. Actionlint receipt 'ok' supports no syntactic regression.",
            "verification-question",
            "open",
            "low",
            "medium",
            "checkout-persistence-no-change",
        ),
        test_observation(
            "workflow-opposition",
            "Adding a workflow file to paths-ignore could grant implicit permission expansion; refuted because: paths-ignore only filters trigger activation; it does not alter token scopes, permissions blocks, or any job-level security context.",
            "false-premise",
            "refuted",
            "low",
            "medium",
            "paths-ignore-permission-refuted",
        ),
        test_observation(
            "workflow-proof",
            "paths-ignore is a literal substring/glob match; a future rename of ub-review-packet.yml silently re-enables Droid noise.",
            "parked-follow-up",
            "parked",
            "low",
            "medium",
            "paths-ignore-future-rename",
        ),
        test_observation(
            "workflow-proof",
            "Confirm no focused smoke proof (workflow_run on a fork-PR dry-run, or a temporary pull_request_target guard test) was executed for the paths-ignore change. Trust rests on actionlint parse only; semantic skip behavior on the droid lane is not proven by sensors.",
            "verification-question",
            "open",
            "low",
            "medium",
            "paths-ignore-smoke-proof-gap",
        ),
        test_observation(
            "workflow-proof",
            "Confirm actionlint receipt 'ok' confirms syntactic validity, but no semantic proof of skip behavior on the droid lane is available; trust rests on actionlint parse plus per-PR trigger semantics - the droid lane is auxiliary/non-blocking and the UB gate is authoritative, so residual workflow risk is bounded.",
            "verification-question",
            "open",
            "low",
            "medium",
            "actionlint-semantic-skip-proof",
        ),
        test_observation(
            "workflow-proof",
            "Residual workflow risk: cache key/restore-keys prefix is coupled to action SHA. Any future repin must update all three sites; a partial update silently mismatches cache restore. Not actionable in this PR (current state is consistent) - parked for follow-up lint rule or script.",
            "parked-follow-up",
            "parked",
            "low",
            "medium",
            "cache-key-current-pin-followup",
        ),
        test_observation(
            "workflow-proof",
            "trust gap: no focused smoke proof (workflow_run on fork-PR dry-run or pull_request_target guard) executed for the paths-ignore change; semantic skip behavior on Droid lane unproven beyond actionlint parse.",
            "missing-evidence",
            "open",
            "low",
            "medium",
            "actionlint-skip-proof-gap",
        ),
    ];
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &diff,
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &summary_only_findings,
        &observations,
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.is_empty());
    assert!(!body.contains("checkout credential persistence"));
    assert!(!body.contains("paths-ignore"));
    assert!(!body.contains("focused smoke proof"));
    assert!(!body.contains("semantic skip behavior"));
    assert!(!body.contains("cache key/restore-keys"));
    assert!(!body.contains("actionlint is not installed locally"));
    assert!(!body.contains("CodeRabbit"));
    assert!(!body.contains("892e1bb"));
    assert!(!body.contains("zizmor"));
    assert!(!body.contains("Droid noise"));
}

#[test]
fn pr_review_body_keeps_refuted_only_observations_artifact_only() {
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[] as &[SummaryOnlyFinding],
        &[test_observation(
            "workflow-opposition",
            "This diff widens the workflow permission/secret surface; refuted because the changed hunk only updates an already pinned action ref.",
            "false-premise",
            "refuted",
            "low",
            "high",
            "refuted-only-workflow-posture",
        )],
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.is_empty());
    assert!(!body.contains("## Refuted"));
    assert!(!body.contains("widens the workflow permission"));
}

#[test]
fn pr_review_body_keeps_workflow_lockstep_summaries_artifact_only() {
    let mut diff = test_diff();
    diff.diff_class = DiffClass::WorkflowTooling;
    diff.changed_files = vec![
        ".github/workflows/droid-focused-review.yml".to_owned(),
        ".github/workflows/ub-review-packet.yml".to_owned(),
    ];
    diff.patch = concat!(
            " pull_request:\n",
            "   paths-ignore:\n",
            "+    - \".github/workflows/ub-review-packet.yml\"\n",
            "+          key: ub-review-gh-runner-v2-cec5b07457f99e652a500dffc603f98e6082a7f7-${{ runner.os }}-rust-1.95-core\n",
            "+            ub-review-gh-runner-v2-cec5b07457f99e652a500dffc603f98e6082a7f7-${{ runner.os }}-rust-1.95-\n",
            "+        uses: EffortlessMetrics/ub-review@cec5b07457f99e652a500dffc603f98e6082a7f7\n",
        )
        .to_owned();
    let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "workflow-permissions".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "workflow-permissions lane: paths-ignore addition cannot alter token scopes/permissions; pin bump is lockstep across cache key, restore-keys prefix, and uses: reference. No new third-party action, no pull_request_target, no checkout credential persistence vector. Residual risk: cache key/pin coupling is a parked follow-up; no blocker.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-pinning".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Workflow-pinning lane for PR #49. Two workflow YAML files touched. Pin lockstep verified for cec5b07457f99e652a500dffc603f98e6082a7f7 across 3 sites, old pin absent, cache key/restore-keys prefix match, no other third-party actions changed.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Workflow lint proof lane: actionlint receipt ok, no focused smoke proof available, no syntactic regression. Pin lockstep and paths-ignore semantics covered by seeded thread; CodeRabbit stale SHA comment is stale (current head re-pinned to cec5b074).".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-proof".to_owned(),
                severity: "low".to_owned(),
                confidence: "high".to_owned(),
                reason: "Cache key/restore-keys prefix is coupled to action SHA; any future partial repin silently mismatches restore. Current state consistent, parked for lint-rule follow-up.".to_owned(),
                evidence: "diff: cache key, restore-keys prefix, uses: all reference cec5b074 in lockstep".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Opposition lane for workflow/tooling: paths-ignore addition + lockstep SHA pin. Actionlint ok. No source, no permissions, no token, no checkout changes. No blocker.".to_owned(),
                evidence: "lane model summary".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "workflow-opposition".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium".to_owned(),
                reason: "Residual workflow risk: cache key/restore-keys prefix is manually coupled to action SHA. Partial repin silently breaks cache restore. Parked for follow-up lint rule.".to_owned(),
                evidence: "cache key/restore-keys must be updated in lockstep with uses: pin; no automated guard exists; current PR state is consistent".to_owned(),
            },
        ];
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &diff,
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &summary_only_findings,
        &[] as &[Observation],
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.is_empty());
    assert!(!body.contains("Workflow-pinning lane"));
    assert!(!body.contains("Pin lockstep verified"));
    assert!(!body.contains("Current state consistent"));
    assert!(!body.contains("No blocker"));
    assert!(!body.contains("cache key/restore-keys"));
}

#[test]
fn pr_review_body_compiles_ffi_test_gap_as_decision_memo() {
    let mut global_box_refutation = test_observation(
        "ub-active-view",
        "Box::from(slice) can return None on allocation failure; refuted because allocation failure does not return None.",
        "false-premise",
        "refuted",
        "low",
        "high",
        BOX_FROM_ALLOCATION_FALSE_PREMISE_DEDUPE_KEY,
    );
    global_box_refutation.source = "model-false-premise-guard".to_owned();
    let body = render_review_body(
            "abc123",
            &test_plan(Vec::new()),
            &test_diff(),
            &[],
            &[] as &[SensorEvidenceIssue],
            &[] as &[ModelEvidenceIssue],
            &[] as &[ReviewInlineComment],
            &[SummaryOnlyFinding {
                lane: "tests-red-green".to_owned(),
                severity: "medium".to_owned(),
                confidence: "high".to_owned(),
                reason: "[tests-red-green] high high at test/js/bun/ffi/ffi.test.js:985: The no-finalizer `toBuffer(ptr(buffer))` subprocess tests assert process survival after GC, but they do not prove memory remains valid after collection/reuse. Attach the ASAN bad-free witness, or strengthen the subprocess test so it observes a real post-GC memory-validity condition rather than only `exitCode === 0`.".to_owned(),
                evidence: "lane transcript".to_owned(),
            }],
            &[
                test_observation(
                    "tests-oracle",
                    "The explicit-finalizer regression is useful guard coverage, but it is not the red/green proof for this bug. It checks that explicit ownership still works; the no-finalizer path is the actual fix surface.",
                    "parked-follow-up",
                    "parked",
                    "low",
                    "medium-high",
                    "explicit-finalizer-guard",
                ),
                global_box_refutation,
            ],
            &[] as &[ProofReceipt],
            60_000,
            ReviewBodyAudience::PullRequest,
        );

    assert!(body.contains("## Decision"));
    assert!(body.contains("Needs one test-proof clarification before upstream."));
    assert!(body.contains("## Verification questions"));
    assert!(body.contains("Confirm the no-finalizer `toBuffer(ptr(buffer))` subprocess tests"));
    assert!(body.contains("## Parked follow-ups"));
    assert!(body.contains("explicit-finalizer regression is useful guard coverage"));
    assert!(!body.contains("Shared context"));
    assert!(!body.contains("Profile:"));
    assert!(!body.contains("Changed files:"));
    assert!(!body.contains("Inline comments:"));
    assert!(!body.contains("[tests-red-green]"));
    assert!(!body.contains("high high at"));
    assert!(!body.contains("test/js/bun/ffi/ffi.test.js:985"));
    assert!(!body.contains("lane transcript"));
    assert!(!body.contains("A human should still inspect"));
    assert!(!body.contains("## Residual risk"));
    assert!(!body.contains("## Refuted"));
    assert!(!body.contains("Box::from(slice)"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_renders_discriminating_proof_receipt_once() {
    let mut receipt = test_red_green_proof_receipt("discriminating", "failed");
    receipt.commands[1].exit_code = Some(132);
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[] as &[SummaryOnlyFinding],
        &[] as &[Observation],
        &[receipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.contains("## Test proof"));
    assert!(!body.contains("## Decision"));
    assert!(!body.contains("No blocking UB finding from this pass."));
    assert!(body.contains("Focused red/green proof discriminates the patch"));
    assert!(body.contains("HEAD passed (exit 0) and base+tests failed (exit 132)"));
    assert!(!body.contains("Needs reviewer attention"));
    assert!(!body.contains("## Residual risk"));
    assert!(!body.contains("stdout.txt"));
    assert!(!body.contains("stderr.txt"));
    assert!(!body.contains("## Model lanes"));
    assert!(!body.contains("A human should still inspect"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_uses_proof_receipt_instead_of_duplicate_test_witness_question() {
    let receipt = test_red_green_proof_receipt("discriminating", "failed");
    let observations = vec![
        test_observation(
            "tests-oracle",
            "The new test needs a witnessed old-main red run.",
            "missing-evidence",
            "open",
            "medium",
            "high",
            "markdown-red-green-witness",
        ),
        test_observation(
            "source-route",
            "The red/green proof does not prove FileHandle.write reaches the patched scalar-write path; confirm the route before relying on the test.",
            "verification-question",
            "open",
            "medium",
            "medium-high",
            "filehandle-write-route",
        ),
    ];
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[SummaryOnlyFinding {
            lane: "tests-red-green".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            reason: "The new test needs a witnessed old-main red run.".to_owned(),
            evidence: "duplicate lane summary".to_owned(),
        }],
        &observations,
        &[receipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.contains("## Test proof"));
    assert!(body.contains("Focused red/green proof discriminates the patch"));
    assert!(body.contains("## Verification questions"));
    assert!(body.contains(
            "Confirm the red/green proof does not prove FileHandle.write reaches the patched scalar-write path"
        ));
    assert!(body.contains("Needs one verification check before upstream."));
    assert!(!body.contains("Needs one test-proof clarification before upstream."));
    assert!(!body.contains("witnessed old-main red run"));
    assert!(!body.contains("duplicate lane summary"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_uses_missing_proof_receipt_instead_of_duplicate_test_witness_question() {
    let mut receipt = test_red_green_proof_receipt("timed_out", "timed_out");
    receipt.commands[1].timed_out = true;
    let observations = vec![test_observation(
        "tests-oracle",
        "The new test needs a witnessed old-main red run.",
        "missing-evidence",
        "open",
        "medium",
        "high",
        "markdown-red-green-witness",
    )];
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[SummaryOnlyFinding {
            lane: "tests-red-green".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            reason: "The new test needs a witnessed old-main red run.".to_owned(),
            evidence: "duplicate lane summary".to_owned(),
        }],
        &observations,
        &[receipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.contains("## Evidence gaps"));
    assert!(body.contains("Focused proof timed out"));
    assert!(!body.contains("## Verification questions"));
    assert!(!body.contains("Needs one test-proof clarification before upstream."));
    assert!(!body.contains("witnessed old-main red run"));
    assert!(!body.contains("duplicate lane summary"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_dedupes_duplicate_summary_findings() {
    let duplicate_reason = "The bare toThrow assertion is non-discriminating for the changed FFI offset path; strengthen it with the expected TypeError message.";
    let summary_only_findings = vec![
            SummaryOnlyFinding {
                lane: "tests-oracle".to_owned(),
                severity: "medium".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: duplicate_reason.to_owned(),
                evidence: "oracle lane transcript".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "tests-red-green".to_owned(),
                severity: "medium".to_owned(),
                confidence: "high".to_owned(),
                reason: duplicate_reason.to_owned(),
                evidence: "red/green lane transcript".to_owned(),
            },
            SummaryOnlyFinding {
                lane: "sibling-paths".to_owned(),
                severity: "low".to_owned(),
                confidence: "medium-high".to_owned(),
                reason: "Parked follow-up: sweep sibling FFI offset entry points for the same expect-to-error conversion.".to_owned(),
                evidence: "sibling lane transcript".to_owned(),
            },
        ];
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &summary_only_findings,
        &[] as &[Observation],
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert_eq!(body.matches("bare toThrow assertion").count(), 1);
    assert!(body.contains("## Confirmed findings"));
    assert!(body.contains("## Parked follow-ups"));
    assert!(body.contains("sweep sibling FFI offset entry points"));
    assert!(!body.contains("oracle lane transcript"));
    assert!(!body.contains("red/green lane transcript"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_keeps_red_green_question_after_head_only_proof_receipt() {
    let receipt = test_proof_receipt("head_passed", "passed");
    let observations = vec![test_observation(
        "tests-oracle",
        "The new test still needs a base+tests red/green witness.",
        "verification-question",
        "open",
        "medium",
        "high",
        "markdown-red-green-witness",
    )];
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[SummaryOnlyFinding {
            lane: "tests-red-green".to_owned(),
            severity: "medium".to_owned(),
            confidence: "high".to_owned(),
            reason: "The new test still needs a base+tests red/green witness.".to_owned(),
            evidence: "head-only proof is not a base+tests witness".to_owned(),
        }],
        &observations,
        &[receipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.contains("## Test proof"));
    assert!(body.contains("## Verification questions"));
    assert!(body.contains("Confirm the new test still needs a base+tests red/green witness."));
    assert!(body.contains("Needs one test-proof clarification before upstream."));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_drops_structurally_answered_red_green_question() {
    let mut diff = test_diff();
    diff.patch = "\
diff --git a/src/ffi.rs b/src/ffi.rs
index 1111111..2222222 100644
--- a/src/ffi.rs
+++ b/src/ffi.rs
@@ -1,3 +1,3 @@
-let offset = usize::try_from(raw_offset).expect(\"offset must fit\");
+let offset = usize::try_from(raw_offset).map_err(|_| TypeError::new(\"offset must fit\"))?;
"
    .to_owned();
    let summary_only_findings = vec![SummaryOnlyFinding {
        lane: "tests-red-green".to_owned(),
        severity: "medium".to_owned(),
        confidence: "high".to_owned(),
        reason:
            "The added regression still needs base+tests red/green proof; old code was not run."
                .to_owned(),
        evidence: "lane transcript".to_owned(),
    }];

    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &diff,
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &summary_only_findings,
        &[] as &[Observation],
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.is_empty(), "{body}");
}

#[test]
fn structural_red_green_does_not_answer_source_route_question() {
    let mut diff = test_diff();
    diff.patch = "\
diff --git a/src/ffi.rs b/src/ffi.rs
index 1111111..2222222 100644
--- a/src/ffi.rs
+++ b/src/ffi.rs
@@ -1,3 +1,3 @@
-let offset = usize::try_from(raw_offset).expect(\"offset must fit\");
+let offset = usize::try_from(raw_offset).map_err(|_| TypeError::new(\"offset must fit\"))?;
"
    .to_owned();
    let summary_only_findings = vec![SummaryOnlyFinding {
        lane: "source-route".to_owned(),
        severity: "medium".to_owned(),
        confidence: "high".to_owned(),
        reason: "Confirm the base+tests red/green proof reaches the patched FFI offset route."
            .to_owned(),
        evidence: "route lane transcript".to_owned(),
    }];

    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &diff,
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &summary_only_findings,
        &[] as &[Observation],
        &[] as &[ProofReceipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.contains("## Verification questions"));
    assert!(body.contains("Confirm the base+tests red/green proof reaches"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_renders_non_discriminating_proof_as_evidence_gap() {
    let receipt = test_red_green_proof_receipt("non_discriminating", "passed");
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[] as &[SummaryOnlyFinding],
        &[] as &[Observation],
        &[receipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(!body.contains("## Decision"));
    assert!(!body.contains("No blocking UB finding from this pass."));
    assert!(body.contains("## Evidence gaps"));
    assert!(!body.contains("## Residual risk"));
    assert!(body.contains("HEAD passed (exit 0) and base+tests passed (exit 0)"));
    assert!(!body.contains("## Test proof"));
    assert!(!body.contains("Needs one residual-risk check"));
    assert!(!body.contains("A human should still inspect"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_renders_head_failed_proof_exit_code() {
    let mut receipt = test_proof_receipt("head_failed", "failed");
    receipt.commands[0].exit_code = Some(132);
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[] as &[SummaryOnlyFinding],
        &[] as &[Observation],
        &[receipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(body.contains("## Decision"));
    assert!(body.contains("Needs focused proof failure resolved"));
    assert!(body.contains("## Test proof"));
    assert!(body.contains("Focused HEAD proof failed (exit 132)"));
    assert!(!body.contains("stdout.txt"));
    assert!(!body.contains("stderr.txt"));
    assert!(!has_standalone_approval_line(&body));
}

#[test]
fn pr_review_body_collapses_timed_out_proof_to_missing_evidence() {
    let receipt = test_proof_receipt("timed_out", "timed_out");
    let body = render_review_body(
        "abc123",
        &test_plan(Vec::new()),
        &test_diff(),
        &[],
        &[] as &[SensorEvidenceIssue],
        &[] as &[ModelEvidenceIssue],
        &[] as &[ReviewInlineComment],
        &[] as &[SummaryOnlyFinding],
        &[] as &[Observation],
        &[receipt],
        60_000,
        ReviewBodyAudience::PullRequest,
    );

    assert!(!body.contains("## Decision"));
    assert!(!body.contains("No blocking UB finding from this pass."));
    assert!(body.contains("## Evidence gaps"));
    assert!(body.contains("Focused proof timed out"));
    assert!(!body.contains("## Test proof"));
    assert!(!body.contains("## Residual risk"));
    assert!(!body.contains("Needs one missing evidence item resolved before upstream."));
    assert!(!body.contains("A human should still inspect"));
    assert!(!body.contains("stdout.txt"));
    assert!(!body.contains("stderr.txt"));
    assert!(!has_standalone_approval_line(&body));
}
