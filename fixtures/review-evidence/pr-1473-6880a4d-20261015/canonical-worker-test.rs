
    #[test]
    fn independent_canonical_worker_failure_into_compiler() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let (root, base, head) = worker_repo(&temp)?;
        let evidence = PathBuf::from("/workspace/review-evidence/canonical-worker");
        for kind in [ProofKind::FocusedTest, ProofKind::MiriWitness, ProofKind::SanitizerWitness] {
            let out = evidence.join(kind.key());
            let target = if matches!(kind, ProofKind::FocusedTest) { "cargo test --locked worker_test" } else { "worker_test" };
            let request = request(kind.key(), &base, &head, kind, target);
            fs::create_dir_all(&out)?;
            fs::write(out.join("input-request.json"), serde_json::to_vec_pretty(&request)?)?;
            let mut call = 0;
            run_worker_request_with_runner(&args(&root, &out), request,
                &mut |_root, _argv, _env, _timeout, stdout, stderr, observe_process| {
                    call += 1;
                    observe_process(CommandProcessObservation::Spawned);
                    fs::write(stdout, b"controlled runner boundary\n")?;
                    fs::write(stderr, b"")?;
                    Ok(if call == 1 { successful_status() } else { CommandStatus {
                        exit_code: Some(1), timed_out: false, success: false,
                        reason: "controlled exit 1".to_owned(), duration_ms: 1,
                    } })
                })?;
            let receipt: ProofReceipt = serde_json::from_slice(&fs::read(out.join("proof_receipt.json"))?)?;
            ensure!(receipt.revision.is_some());
            ensure!(receipt.commands[1].status == "failed");
            let mut plan = crate::tests::test_plan(Vec::new());
            plan.base = base.clone(); plan.head = head.clone();
            let mut diff = crate::tests::test_diff();
            diff.base = base.clone(); diff.head = head.clone();
            let args = crate::tests::test_run_args(out.clone());
            let surface = compile_review_surface(ReviewCompilerInput {
                shared_context_id: "independent-worker", review_body_policy: &ReviewBodyPolicy::default(),
                run_pass: RunPass::Manual, post_review_on: &[], args: &args, plan: &plan, diff: &diff,
                model_lanes: &[], missing_or_failed_sensor_evidence: &[], missing_or_failed_model_evidence: &[],
                inline_comments: &[], summary_only_findings: &[], observations: &[],
                proof_receipts: std::slice::from_ref(&receipt), suggested_issues: &[],
                final_follow_up_tasks: 0, reporter_distillation: None,
            })?;
            fs::write(out.join("artifact.md"), &surface.artifact_body)?;
            fs::write(out.join("public.md"), &surface.github_review.body)?;
            ensure!(surface.artifact_body.contains("No blocking finding after bounded review; residual risk remains"));
            ensure!(surface.artifact_body.contains("## Missing or failed evidence\n\n- None recorded."));
        }
        Ok(())
    }

