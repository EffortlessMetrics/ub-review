use super::*;

/// Model a server that accepted the submit before the client lost its response.
/// Cleanup cannot delete that now-submitted review. No live network is used.
struct AcceptedSubmitTransport {
    scripted: ScriptedTransport,
    lose_response: bool,
    accepted_submits: usize,
    cleanup_attempts: usize,
}

impl DeliveryTransport for AcceptedSubmitTransport {
    fn fetch_json(&mut self, url: &str, token: &str) -> Result<serde_json::Value> {
        self.scripted.fetch_json(url, token)
    }

    fn send_json(
        &mut self,
        method: &str,
        url: &str,
        token: &str,
        payload_path: &Path,
        headers: &[&str],
    ) -> Result<HttpPostOutput> {
        if url.ends_with("/987/events") {
            assert_eq!(method, "POST");
            let payload: serde_json::Value = serde_json::from_slice(&fs::read(payload_path)?)?;
            assert_eq!(payload["event"], "COMMENT");
            let response = self
                .scripted
                .send_json(method, url, token, payload_path, headers)?;
            assert!(response.status.success());
            self.accepted_submits += 1;
            if self.lose_response {
                return Err(anyhow::anyhow!(
                    "curl: authored lost accepted submit response"
                ));
            }
            return Ok(response);
        }
        if method == "DELETE" {
            assert_eq!(self.accepted_submits, 1);
            assert!(url.ends_with("/reviews/987"));
            self.cleanup_attempts += 1;
            return scripted_output(r#"{"message":"review already submitted"}"#, false);
        }
        assert_eq!(method, "POST");
        assert!(url.ends_with("/reviews"));
        self.scripted
            .send_json(method, url, token, payload_path, headers)
    }
}

#[test]
fn accepted_submit_without_response_never_confirms_publication() -> Result<()> {
    // A matching delivered response is the positive control: the negative
    // cannot pass merely because an earlier fixture/head/receipt error occurs.
    for lose_response in [false, true] {
        let temp = tempfile::tempdir()?;
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph_for("src/lib.rs", 12, "inline", HEAD))?,
        )?;
        let (review, payload) = delivery_review();
        let args = delivery_args(temp.path(), "http://scripted");
        let publication = native_publication_fixture(&args, &review)?;
        let mut transport = AcceptedSubmitTransport {
            scripted: ScriptedTransport {
                gets: VecDeque::from([
                    serde_json::json!({"head":{"sha":HEAD}}),
                    serde_json::json!([{
                        "id":987,"path":"src/lib.rs","line":12,"side":"RIGHT",
                        "commit_id":HEAD,"body":"exact body"
                    }]),
                    serde_json::json!({"head":{"sha":HEAD}}),
                ]),
                sends: VecDeque::from([
                    scripted_output(r#"{"id":987}"#, true)?,
                    scripted_output(r#"{"id":987,"state":"commented"}"#, true)?,
                ]),
            },
            lose_response,
            accepted_submits: 0,
            cleanup_attempts: 0,
        };
        let result = execute_pending_review_delivery_with_transport(
            &args,
            &review,
            &payload,
            &mut transport,
        );
        assert_eq!(transport.accepted_submits, 1);
        assert!(transport.scripted.gets.is_empty());
        assert!(transport.scripted.sends.is_empty());
        if lose_response {
            let error = result
                .err()
                .ok_or_else(|| anyhow::anyhow!("lost response confirmed"))?;
            assert_eq!(
                error.to_string(),
                "curl: authored lost accepted submit response"
            );
            assert_eq!(transport.cleanup_attempts, 1);
            assert!(!args.out.join("delivery-reconciliation.json").exists());
            assert!(!args.out.join("delivery-receipts.json").exists());
            let receipt = build_post_error_receipt(&args, &error);
            assert_eq!(receipt.failure_stage, "network_post");
            crate::post_command::write_post_receipt_and_finalize(
                &args,
                Some(&publication),
                "post-error.json",
                &receipt,
            )?;
            let gate_path = args.review_json.with_file_name("gate_outcome.json");
            let gate: serde_json::Value = serde_json::from_slice(&fs::read(&gate_path)?)?;
            assert_eq!(gate["publication_result"], "failed");
            assert_eq!(gate["delivery_result"], "failed");
            assert_eq!(gate["delivery_attempt"], "attempted");
            assert_eq!(gate["delivery_reason"], "post_failed");
            assert_eq!(gate["gate_result"], "not_proven");
            assert_eq!(gate["code_gate_result"], "pass");
            assert_eq!(gate["conclusion"], "pass");
            assert_eq!(
                gate["delivery_receipt_sha256"],
                sha256_hex(&serde_json::to_vec(&serde_json::to_value(&receipt)?)?)
            );
            // A fresh attempt cannot promote the failed transaction just
            // because the server accepted a prior submission.
            let _next = crate::post_command::begin_post_publication(&args)?
                .ok_or_else(|| anyhow::anyhow!("restart publication absent"))?;
            let restarted: serde_json::Value = serde_json::from_slice(&fs::read(gate_path)?)?;
            assert_eq!(restarted["publication_result"], "not_proven");
            assert_eq!(restarted["delivery_result"], "prepared");
            assert!(restarted.get("delivery_receipt_sha256").is_none());
            assert_eq!(restarted["code_gate_result"], "pass");
            assert_eq!(restarted["conclusion"], "pass");
        } else {
            assert_eq!(transport.cleanup_attempts, 0);
            confirm_native_publication(&args, &publication, &review, &result?)?;
        }
    }
    Ok(())
}
