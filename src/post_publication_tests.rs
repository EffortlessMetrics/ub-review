use super::*;

fn fixture() -> Result<(tempfile::TempDir, PostArgs)> {
    let temp = tempfile::tempdir()?;
    let out = temp.path().join("review");
    fs::create_dir_all(&out)?;
    let args = PostArgs {
        review_json: out.join("github-review.json"),
        diff_patch: Some(out.join("diff.patch")),
        out,
        github_token: None,
        repo: Some("Example/review-fixture".to_owned()),
        pull_number: Some(9),
        github_api_url: "https://fixture.invalid".to_owned(),
        fail_on_post_error: false,
    };
    fs::write(
        &args.review_json,
        br#"{"event":"COMMENT","body":"Authored synthetic review.","comments":[{"path":"src/lib.rs","line":2,"side":"RIGHT","body":"[tests] The authored fixture demonstrates a missing boundary assertion."}]}"#,
    )?;
    fs::write(
        args.out.join("diff.patch"),
        "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,3 @@\n pub fn count() -> usize {\n+    let value = 1;\n     1\n",
    )?;
    fs::write(
        args.out.join("gate_outcome.json"),
        serde_json::to_vec(&serde_json::json!({
            "schema":"ub-review.gate_outcome.v1", "publication_result":"not_proven",
        "analysis_result":"findings", "conclusion":"pass", "gate_result":"not_proven",
        "code_gate_result":"pass", "not_proven_reasons":["publication: prepared"],
            "revision":{"digest":"a".repeat(64),"semantics":"candidate_head","reviewed_commit":"b".repeat(40)}
        }))?,
    )?;
    Ok((temp, args))
}

fn gate(args: &PostArgs) -> Result<serde_json::Value> {
    Ok(serde_json::from_slice(&fs::read(
        args.out.join("gate_outcome.json"),
    )?)?)
}

fn success(args: &PostArgs) -> serde_json::Value {
    serde_json::json!({"schema_version":1,"status":"ok","repo":"Example/review-fixture",
        "repo_valid":true,"pull_number":9,"review_json":args.review_json.display().to_string(),
        "review_json_exists":true,"review_json_valid":true,"token_present":true,"payload_written":true,
        "http_status":200,"response":{"id":17,"state":"COMMENTED","commit_id":"b".repeat(40)}})
}

fn snapshot(args: &PostArgs) -> Result<PostPublication> {
    begin_post_publication(args)?.ok_or_else(|| anyhow::anyhow!("missing publication fixture"))
}

#[test]
fn publication_bytes_enforces_its_limit_and_preserves_exact_bytes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let path = temp.path().join("bounded.json");
    fs::write(&path, b"exact bytes")?;
    assert_eq!(publication_bytes(&path)?, b"exact bytes");
    fs::write(&path, vec![b'x'; 1_048_576])?;
    assert_eq!(publication_bytes(&path)?.len(), 1_048_576);
    fs::write(&path, vec![b'x'; 1_048_577])?;
    let error = publication_bytes(&path)
        .err()
        .ok_or_else(|| anyhow::anyhow!("oversized input accepted"))?;
    assert_eq!(error.to_string(), "publication input exceeds 1 MiB");
    fs::remove_file(&path)?;
    assert!(publication_bytes(&path).is_err());
    Ok(())
}

#[test]
fn publication_source_removes_only_owned_projection_fields_and_reasons() -> Result<()> {
    let (_temp, args) = fixture()?;
    let mut value = gate(&args)?;
    value["not_proven_reasons"] =
        serde_json::json!(["publication: prepared", "model-coverage: unavailable"]);
    value["delivery_receipt_sha256"] = "old".into();
    let source = publication_source(value)?;
    assert_eq!(source["code_gate_result"], "pass");
    assert_eq!(source["analysis_result"], "findings");
    assert_eq!(
        source["not_proven_reasons"],
        serde_json::json!(["model-coverage: unavailable"])
    );
    assert!(source.get("publication_result").is_none());
    assert!(source.get("gate_result").is_none());
    assert!(source.get("delivery_receipt_sha256").is_none());
    assert!(publication_source(serde_json::json!([])).is_err());
    assert!(publication_source(serde_json::json!({"schema":"unsupported"})).is_err());
    Ok(())
}

#[test]
fn begin_post_publication_freezes_payload_and_independent_source() -> Result<()> {
    let (_temp, args) = fixture()?;
    let publication =
        begin_post_publication(&args)?.ok_or_else(|| anyhow::anyhow!("publication absent"))?;
    assert_eq!(publication.path, args.out.join("gate_outcome.json"));
    assert_eq!(
        publication.review_sha256,
        Some(sha256_hex(&fs::read(&args.review_json)?))
    );
    assert_eq!(publication.source["code_gate_result"], "pass");
    assert_eq!(publication.source["analysis_result"], "findings");
    assert!(!publication.not_needed);
    assert_eq!(gate(&args)?["gate_result"], "not_proven");
    Ok(())
}

#[test]
fn positive_review_id_requires_a_positive_integer_without_type_coercion() {
    let results = [
        17.into(),
        "17".into(),
        0.into(),
        (-1).into(),
        true.into(),
        "invalid".into(),
    ]
    .map(|value: serde_json::Value| positive_review_id(&value));
    assert_eq!(results, [true, true, false, false, false, false]);
}

#[test]
fn project_publication_result_refreshes_publication_without_erasing_code_uncertainty() -> Result<()>
{
    let (_temp, args) = fixture()?;
    let mut value = gate(&args)?;
    value["not_proven_reasons"] =
        serde_json::json!(["publication: old", "model-coverage: unavailable"]);
    project_publication_result(&mut value, "new")?;
    assert_eq!(value["gate_result"], "not_proven");
    assert_eq!(
        value["not_proven_reasons"],
        serde_json::json!(["model-coverage: unavailable", "publication: new"])
    );
    value["publication_result"] = "posted".into();
    project_publication_result(&mut value, "confirmed")?;
    assert_eq!(value["gate_result"], "pass");
    assert_eq!(
        value["not_proven_reasons"],
        serde_json::json!(["model-coverage: unavailable"])
    );
    Ok(())
}

#[test]
fn post_publication_state_distinguishes_current_success_and_blocked_payload() -> Result<()> {
    let (_temp, args) = fixture()?;
    let publication = snapshot(&args)?;
    assert_eq!(
        post_publication_state(&args, &publication, &success(&args)),
        (
            "posted",
            "confirmed",
            "attempted",
            "current_revision_confirmed"
        )
    );
    assert_eq!(
        post_publication_state(
            &args,
            &publication,
            &serde_json::json!({"schema_version":1,"status":"failed","failure_stage":"payload_validation"})
        ),
        ("failed", "failed", "blocked", "post_failed")
    );
    assert_eq!(
        post_publication_state(
            &args,
            &publication,
            &serde_json::json!({"schema_version":2})
        ),
        ("not_proven", "unknown", "unknown", "invalid_post_receipt")
    );
    Ok(())
}

#[test]
fn prepared_only_is_unconfirmed_and_old_result_is_not_reused() -> Result<()> {
    let (_temp, args) = fixture()?;
    fs::write(
        args.out.join("post-result.json"),
        serde_json::to_vec(&success(&args))?,
    )?;
    let _snapshot = snapshot(&args)?;
    let value = gate(&args)?;
    assert_eq!(value["publication_result"], "not_proven");
    assert_eq!(value["delivery_result"], "prepared");
    assert_eq!(value["delivery_attempt"], "not_attempted");
    assert!(value.get("delivery_receipt_sha256").is_none());
    assert!(value["conclusion"] == "pass" && value["analysis_result"] == "findings");
    Ok(())
}

#[test]
fn interrupted_repeat_post_invalidates_the_previous_reported_pass() -> Result<()> {
    let (_temp, args) = fixture()?;
    let mut value = gate(&args)?;
    value["publication_result"] = "posted".into();
    value["gate_result"] = "pass".into();
    value["not_proven_reasons"] = serde_json::json!([]);
    value["delivery_receipt_sha256"] = "previous-confirmation".into();
    fs::write(
        args.out.join("gate_outcome.json"),
        serde_json::to_vec(&value)?,
    )?;
    let _publication = snapshot(&args)?; // An interruption occurs before any receipt.
    let value = gate(&args)?;
    assert_eq!(value["publication_result"], "not_proven");
    assert_eq!(value["gate_result"], "not_proven");
    assert!(value["code_gate_result"] == "pass" && value["conclusion"] == "pass");
    assert_eq!(value["analysis_result"], "findings");
    assert!(value.get("delivery_receipt_sha256").is_none());
    assert!(
        value["not_proven_reasons"]
            .as_array()
            .is_some_and(|reasons| reasons.iter().any(|reason| reason
                .as_str()
                .is_some_and(|text| text.starts_with("publication:"))))
    );
    Ok(())
}

#[test]
fn successful_publication_preserves_independent_inconclusive_evidence() -> Result<()> {
    let (_temp, args) = fixture()?;
    let mut value = gate(&args)?;
    value["conclusion"] = "inconclusive".into();
    value["code_gate_result"] = "not_proven".into();
    value["not_proven_reasons"] = serde_json::json!([
        "publication: prepared",
        "gate-conclusion: required reporter evidence unavailable"
    ]);
    fs::write(
        args.out.join("gate_outcome.json"),
        serde_json::to_vec(&value)?,
    )?;
    let publication = snapshot(&args)?;
    finalize_post_publication(&args, &publication, &success(&args))?;
    let value = gate(&args)?;
    assert!(value["publication_result"] == "posted" && value["gate_result"] == "not_proven");
    assert!(
        value["not_proven_reasons"]
            == serde_json::json!(["gate-conclusion: required reporter evidence unavailable"])
    );
    Ok(())
}

#[test]
fn skipped_legacy_or_invalid_code_projection_never_emits_an_invalid_gate_result() -> Result<()> {
    for projection in [serde_json::Value::Null, serde_json::json!("invalid")] {
        let (_temp, args) = fixture()?;
        let mut value = gate(&args)?;
        value["publication_result"] = "not_needed".into();
        value["code_gate_result"] = projection;
        value["not_proven_reasons"] = serde_json::json!([]);
        fs::write(
            args.out.join("gate_outcome.json"),
            serde_json::to_vec(&value)?,
        )?;
        fs::remove_file(&args.review_json)?;
        assert!(begin_post_publication(&args).is_err());
        assert!(gate(&args)? == value); // Unsupported historical sources are rejected before posting.
    }
    Ok(())
}

#[test]
fn matching_success_confirms_only_after_receipt_write_and_replays_idempotently() -> Result<()> {
    let (_temp, args) = fixture()?;
    let publication = snapshot(&args)?;
    let receipt = success(&args);
    write_post_receipt_and_finalize(&args, Some(&publication), "post-result.json", &receipt)?;
    let first = fs::read(args.out.join("gate_outcome.json"))?;
    let value = gate(&args)?;
    assert!(value["publication_result"] == "posted" && value["delivery_result"] == "confirmed");
    assert_eq!(value["delivery_attempt"], "attempted");
    assert!(value["delivery_receipt_sha256"] == sha256_hex(&serde_json::to_vec(&receipt)?));
    assert!(
        value["conclusion"] == "pass"
            && value["analysis_result"] == "findings"
            && value["gate_result"] == "pass"
    );
    finalize_post_publication(&args, &publication, &receipt)?;
    assert!(fs::read(args.out.join("gate_outcome.json"))? == first);
    Ok(())
}

#[test]
fn actual_missing_token_post_is_blocked_and_failed_even_when_tolerated() -> Result<()> {
    let (_temp, args) = fixture()?;
    let out = args.out.clone();
    cmd_post(args)?; // No token: the actual producer stops before HTTP.
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("post-error.json"))?)?;
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(out.join("gate_outcome.json"))?)?;
    assert!(receipt["error_kind"] == "missing_token" && receipt["would_post"] == false);
    assert!(receipt["payload_written"] == false && receipt["failure_tolerated"] == true);
    assert!(value["publication_result"] == "failed" && value["delivery_result"] == "failed");
    assert!(value["delivery_attempt"] == "blocked" && value["delivery_reason"] == "missing_token");
    assert_eq!(value["conclusion"], "pass");
    Ok(())
}

#[test]
fn network_failure_is_attempted_but_never_posted() -> Result<()> {
    let (_temp, mut args) = fixture()?;
    args.github_token = Some("authored-unused-fixture-token".to_owned());
    let publication = snapshot(&args)?;
    let error = anyhow::anyhow!("github review post failed HTTP status 403");
    let receipt = build_post_error_receipt(&args, &error);
    assert_eq!(receipt.failure_stage, "network_post");
    assert!(receipt.review_json_valid);
    write_post_receipt_and_finalize(&args, Some(&publication), "post-error.json", &receipt)?;
    let value = gate(&args)?;
    assert_eq!(value["publication_result"], "failed");
    assert_eq!(value["delivery_attempt"], "attempted");
    assert!(value["delivery_result"] == "failed" && value["conclusion"] == "pass");
    Ok(())
}

#[test]
fn stale_malformed_or_incomplete_success_is_unknown_not_confirmed() -> Result<()> {
    let (_temp, args) = fixture()?;
    let publication = snapshot(&args)?;
    for (field, replacement) in [
        ("id", serde_json::json!(0)),
        ("id", serde_json::json!(true)),
        ("id", serde_json::json!("invalid")),
        ("state", serde_json::json!("PENDING")),
        ("commit_id", serde_json::json!("c".repeat(40))),
        ("commit_id", serde_json::Value::Null),
    ] {
        let mut receipt = success(&args);
        receipt["response"][field] = replacement;
        finalize_post_publication(&args, &publication, &receipt)?;
        let value = gate(&args)?;
        assert!(
            value["publication_result"] == "not_proven" && value["delivery_result"] == "unknown"
        );
    }
    for (field, replacement) in [
        ("repo", serde_json::json!("Example/other")),
        ("pull_number", serde_json::json!(10)),
        ("schema_version", serde_json::json!(2)),
        ("token_present", serde_json::json!(false)),
        ("payload_written", serde_json::json!(false)),
        ("repo_valid", serde_json::json!(false)),
        ("http_status", serde_json::json!(500)),
        ("review_json_valid", serde_json::json!(false)),
    ] {
        let mut receipt = success(&args);
        receipt[field] = replacement;
        finalize_post_publication(&args, &publication, &receipt)?;
        assert_eq!(gate(&args)?["publication_result"], "not_proven");
    }
    Ok(())
}

#[test]
fn changed_payload_unknown_and_changed_gate_source_rejected() -> Result<()> {
    let (_temp, args) = fixture()?;
    let publication = snapshot(&args)?;
    fs::write(&args.review_json, "changed")?;
    finalize_post_publication(&args, &publication, &success(&args))?;
    assert_eq!(gate(&args)?["delivery_reason"], "prepared_review_changed");
    let mut changed = gate(&args)?;
    changed["analysis_result"] = "clean".into();
    fs::write(
        args.out.join("gate_outcome.json"),
        serde_json::to_vec(&changed)?,
    )?;
    assert!(finalize_post_publication(&args, &publication, &success(&args)).is_err());
    assert_eq!(gate(&args)?["analysis_result"], "clean");
    Ok(())
}

#[test]
fn receipt_persistence_failure_invalidates_confirmation() -> Result<()> {
    let (_temp, args) = fixture()?;
    let publication = snapshot(&args)?;
    fs::create_dir(args.out.join("post-result.json"))?;
    assert!(
        write_post_receipt_and_finalize(
            &args,
            Some(&publication),
            "post-result.json",
            &success(&args)
        )
        .is_err()
    );
    let value = gate(&args)?;
    assert!(value["publication_result"] == "failed" && value["delivery_result"] == "failed");
    assert!(
        value["delivery_attempt"] == "unknown" && value["delivery_reason"] == "receipt_persistence"
    );
    Ok(())
}

#[test]
fn no_value_skip_is_not_needed_and_legacy_standalone_has_no_gate_write() -> Result<()> {
    let (_temp, args) = fixture()?;
    let mut value = gate(&args)?;
    value["publication_result"] = "not_needed".into();
    fs::write(
        args.out.join("gate_outcome.json"),
        serde_json::to_vec(&value)?,
    )?;
    fs::remove_file(&args.review_json)?;
    let publication = snapshot(&args)?;
    let skipped = serde_json::json!({"schema_version":1,"status":"skipped"});
    finalize_post_publication(&args, &publication, &skipped)?;
    assert_eq!(gate(&args)?["publication_result"], "not_needed");
    assert_eq!(gate(&args)?["delivery_result"], "not_needed");
    fs::remove_file(args.out.join("gate_outcome.json"))?;
    assert!(begin_post_publication(&args)?.is_none());
    write_post_receipt_and_finalize(&args, None, "post-result.json", &skipped)?;
    assert!(!args.out.join("gate_outcome.json").exists());
    Ok(())
}
