use super::*;

#[test]
fn native_http_500_reaches_current_receipt_and_finalizer() -> Result<()> {
    let _lock = lock_fake_delivery_tests()?;
    for status in [500, 200] {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph_for("src/lib.rs", 12, "inline", HEAD))?,
        )?;
        let (review, _) = delivery_review();
        let mut responses = successful_delivery_responses();
        if status == 500 {
            responses[4] = FakeHttpResponse {
                status,
                body: r#"{"message":"synthetic submission failure"}"#.to_owned(),
            };
            responses.push(FakeHttpResponse {
                status: 204,
                body: String::new(),
            });
        }
        let (api, server) = spawn_fake_delivery_api(responses)?;
        let mut args = delivery_args(temp.path(), &api);
        args.fail_on_post_error = false;
        let _publication = native_publication_fixture(&args, &review)?;
        let mut command_args = delivery_args(temp.path(), &api);
        command_args.fail_on_post_error = false;
        crate::cmd_post(command_args)?;
        let requests = server
            .join()
            .map_err(|_| anyhow::anyhow!("fake API panicked"))??;
        assert_eq!(requests.len(), if status == 500 { 6 } else { 5 });
        assert!(
            requests[4]
                .0
                .starts_with("POST /repos/owner/repo/pulls/42/reviews/987/events ")
        );
        assert!(requests[4].1.contains("COMMENT"));
        let gate: serde_json::Value = serde_json::from_slice(&fs::read(
            args.review_json.with_file_name("gate_outcome.json"),
        )?)?;
        assert_eq!(gate["code_gate_result"], "pass");
        assert_eq!(gate["conclusion"], "pass");
        assert_eq!(gate["analysis_result"], "findings");
        assert_eq!(gate["delivery_attempt"], "attempted");
        let transaction: serde_json::Value =
            serde_json::from_slice(&fs::read(args.out.join("delivery-transaction.json"))?)?;
        if status == 500 {
            assert!(
                requests[5]
                    .0
                    .starts_with("DELETE /repos/owner/repo/pulls/42/reviews/987 ")
            );
            let receipt: serde_json::Value =
                serde_json::from_slice(&fs::read(args.out.join("post-error.json"))?)?;
            assert_eq!(receipt["http_status"], 500);
            assert_eq!(receipt["error_kind"], "post_http_error");
            assert_eq!(receipt["failure_stage"], "network_post");
            assert_eq!(receipt["payload_written"], true);
            assert_eq!(receipt["would_post"], true);
            assert_eq!(receipt["failure_tolerated"], true);
            assert!(
                receipt["reason"]
                    .as_str()
                    .is_some_and(|s| s.contains("GitHub pending review submission failed")
                        && s.contains("HTTP status Some(500)"))
            );
            assert_eq!(gate["publication_result"], "failed");
            assert_eq!(gate["delivery_result"], "failed");
            assert_eq!(gate["delivery_reason"], "post_failed");
            assert_eq!(transaction["state"], "cleaned_up");
            assert_eq!(transaction["failure"]["stage"], "submission");
            assert_eq!(transaction["cleanup"]["status"], "succeeded");
            assert!(!args.out.join("post-result.json").exists());
        } else {
            let receipt: serde_json::Value =
                serde_json::from_slice(&fs::read(args.out.join("post-result.json"))?)?;
            assert_eq!(receipt["http_status"], 200);
            assert_eq!(gate["publication_result"], "posted");
            assert_eq!(gate["delivery_result"], "confirmed");
            assert_eq!(transaction["state"], "receipts_persisted");
            assert!(!args.out.join("post-error.json").exists());
        }
    }
    Ok(())
}

#[test]
fn non_http_error_receipt_remains_unknown_without_attempt_evidence() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let (review, _) = delivery_review();
    let args = delivery_args(temp.path(), "http://unused.invalid");
    let publication = native_publication_fixture(&args, &review)?;
    fs::write(args.out.join("github-review-post-payload.json"), "exists")?;
    for message in [
        "pending review identity mismatch 500",
        "native operation HTTP status None",
    ] {
        let error = anyhow::anyhow!("{message}").context("native operation");
        let receipt = build_post_error_receipt(&args, &error);
        assert_eq!(receipt.http_status, None);
        assert_eq!(receipt.error_kind, "failed");
        assert_eq!(receipt.failure_stage, "unknown");
        assert!(!receipt.payload_written);
        crate::post_command::write_post_receipt_and_finalize(
            &args,
            Some(&publication),
            "post-error.json",
            &receipt,
        )?;
        let gate: serde_json::Value = serde_json::from_slice(&fs::read(
            args.review_json.with_file_name("gate_outcome.json"),
        )?)?;
        assert_eq!(gate["publication_result"], "failed");
        assert_eq!(gate["delivery_attempt"], "unknown");
        assert_eq!(gate["code_gate_result"], "pass");
    }
    Ok(())
}
