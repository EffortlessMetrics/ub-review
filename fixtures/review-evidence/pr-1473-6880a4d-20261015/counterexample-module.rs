

#[cfg(test)]
mod independent_counterexamples {
    use crate::*;
    use anyhow::Result;

    #[test]
    fn independent_worker_and_head_matrix() -> Result<()> {
        let root = PathBuf::from("/workspace/review-evidence/parent-matrix");
        fs::create_dir_all(&root)?;
        let kinds = [ProofKind::FocusedTest, ProofKind::FocusedBuild, ProofKind::BasePlusTests,
            ProofKind::SanitizerWitness, ProofKind::MutationWitness, ProofKind::MiriWitness,
            ProofKind::SourceRouteProbe, ProofKind::ExternalCheck];
        let mut rows = Vec::new();
        for kind in kinds {
            for status in ["passed", "failed", "timed_out", "skipped"] {
                let mut receipt = crate::tests::test_proof_receipt("", status);
                receipt.kind = kind.key().to_owned();
                receipt.commands[0].exit_code = if status == "failed" { Some(1) } else if status == "passed" { Some(0) } else { None };
                receipt.commands[0].timed_out = status == "timed_out";
                receipt.result = classify_worker_proof_result(&kind, &receipt.commands[0]);
                for heads in ["current", "stale", "mixed"] {
                    receipt.head = if heads == "current" { "HEAD" } else { "old-head" }.to_owned();
                    receipt.id = format!("{}-{status}-{heads}", kind.key());
                    let mut receipts = vec![receipt.clone()];
                    if heads == "mixed" { receipts.push(crate::tests::test_proof_receipt("head_passed", "passed")); }
                    rows.push(exercise(&root, &receipt.id, &receipts)?);
                }
            }
        }
        for result in ["head_failed", "skipped_unresolved", "discriminating"] {
            for heads in ["current", "stale", "mixed"] {
                let mut receipt = crate::tests::test_proof_receipt(result, if result == "head_failed" { "failed" } else { "passed" });
                receipt.head = if heads == "current" { "HEAD" } else { "old-head" }.to_owned();
                receipt.id = format!("{result}-{heads}");
                receipt.commands[0].exit_code = Some(if result == "head_failed" { 1 } else { 0 });
                let mut receipts = vec![receipt.clone()];
                if heads == "mixed" { receipts.push(crate::tests::test_proof_receipt("head_passed", "passed")); }
                rows.push(exercise(&root, &receipt.id, &receipts)?);
            }
        }
        fs::write(root.join("matrix.json"), serde_json::to_vec_pretty(&rows)?)?;
        Ok(())
    }
    fn exercise(root: &Path, id: &str, input: &[ProofReceipt]) -> Result<serde_json::Value> {
        let out = root.join(id);
        write_proof_receipt_artifacts(&out, input, None)?;
        let receipts: Vec<ProofReceipt> = serde_json::from_slice(&fs::read(out.join("review/proof_receipts.json"))?)?;
        let args = crate::tests::test_run_args(out.clone());
        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "independent-context", review_body_policy: &ReviewBodyPolicy::default(),
            run_pass: RunPass::Manual, post_review_on: &[], args: &args,
            plan: &crate::tests::test_plan(Vec::new()), diff: &crate::tests::test_diff(),
            model_lanes: &[], missing_or_failed_sensor_evidence: &[], missing_or_failed_model_evidence: &[],
            inline_comments: &[], summary_only_findings: &[], observations: &[], proof_receipts: &receipts,
            suggested_issues: &[], final_follow_up_tasks: 0, reporter_distillation: None,
        })?;
        fs::write(out.join("artifact.md"), &surface.artifact_body)?;
        fs::write(out.join("public.md"), &surface.github_review.body)?;
        let decision = surface.artifact_body.split("## Decision\n\n").nth(1).and_then(|s| s.lines().next()).unwrap_or("");
        Ok(serde_json::json!({"id":id,"result":receipts[0].result,"receipt_head":receipts[0].head,
            "plan_head":"HEAD","decision":decision,"no_blocking_heading":surface.artifact_body.contains("## No blocking finding after checking"),
            "none_recorded":surface.artifact_body.contains("## Missing or failed evidence\n\n- None recorded."),
            "prepare_public":surface.should_prepare_github_review,"public":surface.github_review.body}))
    }
}
