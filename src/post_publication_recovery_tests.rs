use super::*;

#[test]
fn current_blocked_attempt_overrides_retained_success_receipt() -> Result<()> {
    for code_result in ["pass", "finding", "not_proven"] {
        let (_temp, args) = fixture()?;
        let mut initial = gate(&args)?;
        initial["code_gate_result"] = code_result.into();
        fs::write(
            args.out.join("gate_outcome.json"),
            serde_json::to_vec(&initial)?,
        )?;
        let source = publication_source(initial)?;
        let publication = snapshot(&args)?;
        let old_success = success(&args);
        write_post_receipt_and_finalize(
            &args,
            Some(&publication),
            "post-result.json",
            &old_success,
        )?;
        assert_eq!(gate(&args)?["publication_result"], "posted");
        let retained = fs::read(args.out.join("post-result.json"))?;
        let out = args.out.clone();
        // Execute the actual second command: missing token is tolerated, but
        // neither its exit status nor the retained success can prove delivery.
        cmd_post(args)?;
        let current: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("gate_outcome.json"))?)?;
        let error: serde_json::Value =
            serde_json::from_slice(&fs::read(out.join("post-error.json"))?)?;
        assert_eq!(fs::read(out.join("post-result.json"))?, retained);
        assert_eq!(error["error_kind"], "missing_token");
        assert_eq!(error["failure_tolerated"], true);
        assert_eq!(error["would_post"], false);
        assert_eq!(current["publication_result"], "failed");
        assert_eq!(current["delivery_result"], "failed");
        assert_eq!(current["delivery_attempt"], "blocked");
        assert_eq!(current["delivery_reason"], "missing_token");
        assert_eq!(current["gate_result"], "not_proven");
        assert_eq!(
            current["delivery_receipt_sha256"],
            sha256_hex(&serde_json::to_vec(&error)?)
        );
        assert_ne!(
            current["delivery_receipt_sha256"],
            sha256_hex(&serde_json::to_vec(&old_success)?)
        );
        assert_eq!(publication_source(current)?, source);
    }
    Ok(())
}

#[test]
fn current_success_overrides_retained_error_only_after_new_confirmation() -> Result<()> {
    for code_result in ["pass", "finding", "not_proven"] {
        let (_temp, args) = fixture()?;
        let mut initial = gate(&args)?;
        initial["code_gate_result"] = code_result.into();
        fs::write(
            args.out.join("gate_outcome.json"),
            serde_json::to_vec(&initial)?,
        )?;
        let source = publication_source(initial)?;
        let first = snapshot(&args)?;
        let error = build_post_error_receipt(&args, &anyhow::anyhow!("authored missing token"));
        write_post_receipt_and_finalize(&args, Some(&first), "post-error.json", &error)?;
        assert_eq!(gate(&args)?["publication_result"], "failed");
        let retained = fs::read(args.out.join("post-error.json"))?;
        let second = snapshot(&args)?;
        let prepared = gate(&args)?;
        assert_eq!(prepared["publication_result"], "not_proven");
        assert_eq!(prepared["delivery_result"], "prepared");
        assert!(prepared.get("delivery_receipt_sha256").is_none());
        let receipt = success(&args);
        write_post_receipt_and_finalize(&args, Some(&second), "post-result.json", &receipt)?;
        let current = gate(&args)?;
        assert_eq!(fs::read(args.out.join("post-error.json"))?, retained);
        assert_eq!(current["publication_result"], "posted");
        assert_eq!(current["delivery_result"], "confirmed");
        assert_eq!(current["delivery_attempt"], "attempted");
        assert_eq!(current["delivery_reason"], "current_revision_confirmed");
        assert_eq!(current["gate_result"], code_result);
        assert_eq!(
            current["delivery_receipt_sha256"],
            sha256_hex(&serde_json::to_vec(&receipt)?)
        );
        assert_ne!(
            current["delivery_receipt_sha256"],
            sha256_hex(&serde_json::to_vec(&error)?)
        );
        assert_eq!(publication_source(current)?, source);
    }
    Ok(())
}
