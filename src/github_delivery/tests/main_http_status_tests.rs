use super::*;

#[test]
fn main_native_http_status_reaches_post_receipts_without_finalizer() -> Result<()> {
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
        let args = delivery_args(temp.path(), &api);
        fs::write(&args.review_json, serde_json::to_vec(&review)?)?;
        let diff = crate::post_diff_patch_path(&args);
        fs::create_dir_all(
            diff.parent()
                .ok_or_else(|| anyhow::anyhow!("missing diff parent"))?,
        )?;
        fs::write(
            diff,
            "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -11,1 +11,3 @@\n existing line\n+    let fixture = 1;\n+    let second = 2;\n",
        )?;
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
        assert!(args.out.join("github-review-post-payload.json").is_file());
        assert!(
            !args
                .review_json
                .with_file_name("gate_outcome.json")
                .exists()
        );
        let transaction: serde_json::Value =
            serde_json::from_slice(&fs::read(args.out.join("delivery-transaction.json"))?)?;
        let receipt_name = if status == 500 {
            "post-error.json"
        } else {
            "post-result.json"
        };
        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(args.out.join(receipt_name))?)?;
        assert_eq!(receipt["http_status"], status);
        assert_eq!(receipt["payload_written"], true);
        if status == 500 {
            assert_eq!(receipt["status"], "failed");
            assert_eq!(receipt["error_kind"], "post_http_error");
            assert_eq!(receipt["failure_stage"], "network_post");
            assert_eq!(receipt["failure_tolerated"], true);
            assert_eq!(receipt["would_post"], true);
            assert!(
                receipt["reason"]
                    .as_str()
                    .is_some_and(|s| s.contains("GitHub pending review submission failed")
                        && s.contains("HTTP status Some(500)"))
            );
            assert!(
                requests[5]
                    .0
                    .starts_with("DELETE /repos/owner/repo/pulls/42/reviews/987 ")
            );
            assert_eq!(transaction["state"], "cleaned_up");
            assert_eq!(transaction["failure"]["stage"], "submission");
            assert_eq!(
                transaction["cleanup"],
                serde_json::json!({"status":"succeeded"})
            );
            assert!(!args.out.join("post-result.json").exists());
        } else {
            assert_eq!(receipt["status"], "ok");
            assert_eq!(receipt["response"]["id"], 987);
            assert_eq!(transaction["state"], "receipts_persisted");
            assert!(!args.out.join("post-error.json").exists());
        }
    }
    Ok(())
}
