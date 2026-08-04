//! GitHub transport for the bounded inline delivery transaction (#827).
//!
//! This adapter deliberately owns only pending-review creation, exact comment
//! reconciliation, head revalidation, submission, and receipts. Replies,
//! retries, and body fallback are later delivery slices.

use crate::delivery_transaction::{
    CleanupOutcome, DeliveryAction, DeliveryFailureStage, DeliveryLocation, DeliveryTransaction,
    DeliveryTransactionState, ObservedDelivery, PlannedDelivery, reconcile_deliveries,
};
use crate::*;
use anyhow::ensure;

#[derive(Debug)]
pub(crate) struct PendingReviewPostOutcome {
    pub(crate) response: serde_json::Value,
    pub(crate) http_status: Option<u16>,
}

pub(crate) fn execute_pending_review_delivery(
    args: &PostArgs,
    review: &GitHubReview,
    api_payload: &GitHubReviewPostPayload,
) -> Result<PendingReviewPostOutcome> {
    let token = args
        .github_token
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("github token is required for posting"))?;
    let repo = args
        .repo
        .as_deref()
        .filter(|value| is_valid_repo_slug(value))
        .ok_or_else(|| anyhow::anyhow!("valid GitHub repository slug is required"))?;
    let pull_number = args
        .pull_number
        .or_else(detect_pull_number_from_event)
        .ok_or_else(|| anyhow::anyhow!("pull request number is required for posting"))?;
    let api = args.github_api_url.trim_end_matches('/');
    let review_url = format!("{api}/repos/{repo}/pulls/{pull_number}/reviews");

    let expected_head = read_expected_delivery_head(args, review)?;
    let current_head = fetch_pull_head(api, repo, pull_number, token)?;
    let expected_head = expected_head.unwrap_or(current_head.clone());
    ensure_heads_match(
        "before pending-review creation",
        &expected_head,
        &current_head,
    )?;
    let planned = build_planned_inline_deliveries(args, review, &expected_head)?;
    let mut transaction = DeliveryTransaction::new(expected_head.clone(), planned.clone())?;

    let pending_payload = serde_json::json!({
        "event": "PENDING",
        "body": api_payload.body,
        "comments": api_payload.comments,
    });
    let pending_payload_path = args.out.join("delivery-pending-review-payload.json");
    write_json(&pending_payload_path, &pending_payload)?;

    let mut review_id = None;
    let result = (|| -> Result<PendingReviewPostOutcome> {
        let pending = send_json(
            "POST",
            &review_url,
            token,
            &pending_payload_path,
            &[
                "Accept: application/vnd.github+json",
                "Content-Type: application/json",
                "X-GitHub-Api-Version: 2022-11-28",
            ],
        )?;
        write_response_artifacts(&args.out, "pending-review", &pending)?;
        let pending_json = parse_success_json(&pending, "pending review creation")?;
        let created_review_id = json_identifier(&pending_json, "id", "pending review")?;
        review_id = Some(created_review_id.clone());
        transaction.transition(DeliveryTransactionState::PendingReviewCreated)?;
        write_transaction(&args.out, &transaction)?;

        transaction.transition(DeliveryTransactionState::CommentsCreated)?;
        let comments_url = format!("{review_url}/{created_review_id}/comments");
        let listed = fetch_json(&comments_url, token)?;
        write_json(
            &args.out.join("delivery-pending-review-comments.json"),
            &listed,
        )?;
        let observed = observed_deliveries(&listed, &planned, &expected_head)?;
        let reconciliation =
            reconcile_deliveries(&expected_head, &created_review_id, &planned, &observed)?;
        transaction.transition(DeliveryTransactionState::CommentsReconciled)?;
        let reconciliation_value = serde_json::to_value(&reconciliation)?;
        write_json(
            &args.out.join("review/delivery-reconciliation.json"),
            &reconciliation_value,
        )?;
        write_json(
            &args.out.join("review/delivery-receipts.json"),
            &reconciliation_value["receipts"],
        )?;
        write_transaction(&args.out, &transaction)?;

        let rechecked_head = fetch_pull_head(api, repo, pull_number, token)?;
        ensure_heads_match(
            "before pending-review submission",
            &expected_head,
            &rechecked_head,
        )?;
        transaction.transition(DeliveryTransactionState::HeadRevalidated)?;
        write_transaction(&args.out, &transaction)?;

        let submit_payload = serde_json::json!({
            "event": "COMMENT",
            "body": api_payload.body,
        });
        let submit_payload_path = args.out.join("delivery-submit-review-payload.json");
        write_json(&submit_payload_path, &submit_payload)?;
        let submitted = send_json(
            "PUT",
            &format!("{review_url}/{created_review_id}"),
            token,
            &submit_payload_path,
            &[
                "Accept: application/vnd.github+json",
                "Content-Type: application/json",
                "X-GitHub-Api-Version: 2022-11-28",
            ],
        )?;
        write_response_artifacts(&args.out, "post", &submitted)?;
        let response = parse_success_json(&submitted, "pending review submission")?;
        transaction.transition(DeliveryTransactionState::Submitted)?;
        transaction.transition(DeliveryTransactionState::ReceiptsPersisted)?;
        write_transaction(&args.out, &transaction)?;
        Ok(PendingReviewPostOutcome {
            response,
            http_status: submitted.http_status,
        })
    })();

    match result {
        Ok(outcome) => Ok(outcome),
        Err(error) => {
            let cleanup = if let Some(id) = review_id.as_deref() {
                match delete_pending_review(&review_url, id, token, &args.out) {
                    Ok(()) => CleanupOutcome::Succeeded,
                    Err(cleanup_error) => {
                        CleanupOutcome::Failed(sanitize_reason(&format!("{cleanup_error:#}")))
                    }
                }
            } else {
                CleanupOutcome::NotAttempted
            };
            let stage = failure_stage(&error, &transaction);
            if review_id.is_some() {
                transaction.record_failure(stage, sanitize_reason(&format!("{error:#}")), true)?;
                transaction.finish_cleanup(cleanup)?;
            } else {
                transaction.record_failure(stage, sanitize_reason(&format!("{error:#}")), false)?;
            }
            write_transaction(&args.out, &transaction)?;
            Err(error)
        }
    }
}

fn read_expected_delivery_head(args: &PostArgs, review: &GitHubReview) -> Result<Option<String>> {
    let graph_path = args
        .review_json
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("claim_graph.json");
    if !review.comments.is_empty() {
        let graph: serde_json::Value = serde_json::from_slice(
            &fs::read(&graph_path).with_context(|| format!("read {}", graph_path.display()))?,
        )
        .with_context(|| format!("parse {}", graph_path.display()))?;
        let head = graph
            .get("head_sha")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("claim graph has no exact head SHA"))?;
        return Ok(Some(head.to_owned()));
    }
    // Empty reviews have no compiler claim graph to bind. The pre-create GET
    // is still the exact subject binding for the transaction.
    Ok(None)
}

fn build_planned_inline_deliveries(
    args: &PostArgs,
    review: &GitHubReview,
    exact_head_sha: &str,
) -> Result<Vec<PlannedDelivery>> {
    if review.comments.is_empty() {
        return Ok(Vec::new());
    }
    let graph_path = args
        .review_json
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("claim_graph.json");
    let graph: serde_json::Value = serde_json::from_slice(&fs::read(&graph_path)?)?;
    let topics = graph
        .get("topics")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("claim graph topics are missing"))?;
    review
        .comments
        .iter()
        .map(|comment| {
            let path = normalize_repo_path(&comment.path);
            let matches = topics
                .iter()
                .filter(|topic| {
                    topic
                        .get("planned_action")
                        .and_then(serde_json::Value::as_str)
                        == Some("inline")
                        && topic.get("head_sha").and_then(serde_json::Value::as_str)
                            == Some(exact_head_sha)
                        && topic
                            .get("path")
                            .and_then(serde_json::Value::as_str)
                            .map(normalize_repo_path)
                            .as_deref()
                            == Some(path.as_str())
                        && topic.get("anchor").and_then(serde_json::Value::as_u64)
                            == Some(u64::from(comment.line))
                })
                .collect::<Vec<_>>();
            let topic = matches.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "inline comment {}:{} has no current-head inline claim plan",
                    path,
                    comment.line
                )
            })?;
            ensure!(
                matches.len() == 1,
                "inline comment {}:{} has ambiguous claim plan",
                path,
                comment.line
            );
            let claim_id = topic
                .get("claim_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("inline claim plan has no claim id"))?;
            let body = github_review_post_comment_body(comment)?;
            PlannedDelivery::new(
                exact_head_sha,
                claim_id,
                DeliveryAction::Inline,
                DeliveryLocation::new(path, comment.line, comment.side.clone()),
                None,
                sha256_hex(body.as_bytes()),
            )
        })
        .collect()
}

fn fetch_pull_head(api: &str, repo: &str, pull_number: u64, token: &str) -> Result<String> {
    let value = fetch_json(&format!("{api}/repos/{repo}/pulls/{pull_number}"), token)?;
    value
        .get("head")
        .and_then(|head| head.get("sha"))
        .and_then(serde_json::Value::as_str)
        .filter(|sha| !sha.trim().is_empty())
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("GitHub pull response has no head.sha"))
}

fn ensure_heads_match(stage: &str, expected: &str, actual: &str) -> Result<()> {
    if expected != actual {
        bail!("GitHub pull head changed {stage}: expected {expected}, got {actual}");
    }
    Ok(())
}

fn observed_deliveries(
    value: &serde_json::Value,
    planned: &[PlannedDelivery],
    exact_head_sha: &str,
) -> Result<Vec<ObservedDelivery>> {
    let items = value
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("GitHub review comments response must be an array"))?;
    let mut observed = Vec::with_capacity(items.len());
    for item in items {
        let id = json_identifier(item, "id", "GitHub review comment")?;
        let path = item
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(normalize_repo_path)
            .ok_or_else(|| anyhow::anyhow!("GitHub review comment {id} has no path"))?;
        let line = item
            .get("line")
            .and_then(serde_json::Value::as_u64)
            .and_then(|line| u32::try_from(line).ok())
            .ok_or_else(|| anyhow::anyhow!("GitHub review comment {id} has no valid line"))?;
        let side = item
            .get("side")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("GitHub review comment {id} has no side"))?;
        let body = item
            .get("body")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("GitHub review comment {id} has no body"))?;
        if let Some(commit_id) = item.get("commit_id").and_then(serde_json::Value::as_str) {
            ensure!(
                commit_id == exact_head_sha,
                "GitHub review comment {id} is bound to another head"
            );
        }
        let candidates = planned
            .iter()
            .filter(|delivery| delivery.location() == (path.as_str(), line, side))
            .collect::<Vec<_>>();
        let delivery = candidates.first().ok_or_else(|| {
            anyhow::anyhow!("GitHub returned unexpected comment {id} at {path}:{line}")
        })?;
        ensure!(
            candidates.len() == 1,
            "GitHub returned comment {id} with an ambiguous planned location"
        );
        ensure!(
            sha256_hex(body.as_bytes()) == delivery.expected_body_digest(),
            "GitHub returned comment {id} with an unexpected body"
        );
        observed.push(ObservedDelivery::new(id, (*delivery).clone())?);
    }
    Ok(observed)
}

fn fetch_json(url: &str, token: &str) -> Result<serde_json::Value> {
    run_github_api_get(Path::new("."), url, token)
}

fn send_json(
    method: &str,
    url: &str,
    token: &str,
    payload_path: &Path,
    headers: &[&str],
) -> Result<HttpPostOutput> {
    run_curl_json_send(
        Path::new("."),
        method,
        url,
        &format!("Authorization: Bearer {token}"),
        payload_path,
        headers,
        60,
    )
}

fn delete_pending_review(review_url: &str, review_id: &str, token: &str, out: &Path) -> Result<()> {
    let path = out.join("delivery-cleanup-payload.json");
    write_json(&path, &serde_json::json!({}))?;
    let output = send_json(
        "DELETE",
        &format!("{review_url}/{review_id}"),
        token,
        &path,
        &[
            "Accept: application/vnd.github+json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
    )?;
    if !output.status.success() {
        bail!(
            "pending review cleanup failed with HTTP status {:?}: {}",
            output.http_status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}

fn parse_success_json(output: &HttpPostOutput, operation: &str) -> Result<serde_json::Value> {
    if !output.status.success() {
        bail!(
            "GitHub {operation} failed with exit code {:?} and HTTP status {:?}: {}",
            output.status.code(),
            output.http_status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parse GitHub {operation} response"))
}

fn json_identifier(value: &serde_json::Value, field: &str, label: &str) -> Result<String> {
    let id = value
        .get(field)
        .and_then(|value| {
            value
                .as_u64()
                .map(|number| number.to_string())
                .or_else(|| value.as_str().map(str::to_owned))
        })
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| anyhow::anyhow!("{label} response has no valid {field}"))?;
    Ok(id)
}

fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, serde_json::to_vec_pretty(value)?)?;
    Ok(())
}

fn write_transaction(out: &Path, transaction: &DeliveryTransaction) -> Result<()> {
    let value = serde_json::to_value(transaction)?;
    write_json(&out.join("review/delivery-transaction.json"), &value)
}

fn write_response_artifacts(out: &Path, prefix: &str, output: &HttpPostOutput) -> Result<()> {
    fs::create_dir_all(out)?;
    fs::write(out.join(format!("{prefix}-stdout.json")), &output.stdout)?;
    fs::write(out.join(format!("{prefix}-stderr.txt")), &output.stderr)?;
    Ok(())
}

fn sanitize_reason(reason: &str) -> String {
    reason
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

fn failure_stage(
    error: &anyhow::Error,
    transaction: &DeliveryTransaction,
) -> crate::delivery_transaction::DeliveryFailureStage {
    let text = format!("{error:#}").to_ascii_lowercase();
    if text.contains("receipt") {
        DeliveryFailureStage::ReceiptPersistence
    } else if text.contains("head changed") {
        DeliveryFailureStage::HeadRevalidation
    } else {
        match transaction.state() {
            DeliveryTransactionState::Planned => DeliveryFailureStage::PendingReviewCreation,
            DeliveryTransactionState::PendingReviewCreated => DeliveryFailureStage::CommentCreation,
            DeliveryTransactionState::CommentsCreated => {
                DeliveryFailureStage::CommentReconciliation
            }
            DeliveryTransactionState::CommentsReconciled => {
                DeliveryFailureStage::ReceiptPersistence
            }
            DeliveryTransactionState::HeadRevalidated => DeliveryFailureStage::Submission,
            DeliveryTransactionState::Submitted | DeliveryTransactionState::ReceiptsPersisted => {
                DeliveryFailureStage::ReceiptPersistence
            }
            DeliveryTransactionState::CleanupAttempted
            | DeliveryTransactionState::CleanedUp
            | DeliveryTransactionState::Failed => DeliveryFailureStage::ReceiptPersistence,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::ensure;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    fn planned() -> Result<PlannedDelivery> {
        let body = "[tests] exact body";
        PlannedDelivery::new(
            HEAD,
            "claim-1",
            DeliveryAction::Inline,
            DeliveryLocation::new("src/lib.rs", 12, "RIGHT"),
            None,
            sha256_hex(body.as_bytes()),
        )
    }

    fn returned(body: &str) -> serde_json::Value {
        serde_json::json!([{
            "id": 42,
            "path": "src/lib.rs",
            "line": 12,
            "side": "RIGHT",
            "commit_id": HEAD,
            "body": body
        }])
    }

    #[test]
    fn returned_comment_reconciles_to_exact_planned_identity() -> Result<()> {
        let planned = planned()?;
        let observed = observed_deliveries(&returned("[tests] exact body"), &[planned], HEAD)?;
        ensure!(observed.len() == 1, "expected one reconciled comment");
        Ok(())
    }

    #[test]
    fn returned_comment_wrong_body_or_head_is_rejected() -> Result<()> {
        let planned = planned()?;
        ensure!(
            observed_deliveries(
                &returned("[tests] different body"),
                std::slice::from_ref(&planned),
                HEAD
            )
            .is_err(),
            "wrong returned body was accepted"
        );
        let mut wrong_head = returned("[tests] exact body");
        wrong_head[0]["commit_id"] = serde_json::json!("another-head");
        ensure!(
            observed_deliveries(&wrong_head, std::slice::from_ref(&planned), HEAD).is_err(),
            "wrong returned head was accepted"
        );
        Ok(())
    }

    #[test]
    fn missing_unexpected_and_duplicate_comments_are_rejected() -> Result<()> {
        let planned = planned()?;
        let observed =
            observed_deliveries(&serde_json::json!([]), std::slice::from_ref(&planned), HEAD)?;
        ensure!(
            reconcile_deliveries(HEAD, "review-1", std::slice::from_ref(&planned), &observed)
                .is_err(),
            "missing returned comment was accepted"
        );
        let unexpected = serde_json::json!([{
            "id": 43,
            "path": "src/other.rs",
            "line": 12,
            "side": "RIGHT",
            "commit_id": HEAD,
            "body": "[tests] exact body"
        }]);
        ensure!(
            observed_deliveries(&unexpected, std::slice::from_ref(&planned), HEAD).is_err(),
            "unexpected returned comment was accepted"
        );
        let duplicate = serde_json::json!([
            {"id": 42, "path": "src/lib.rs", "line": 12, "side": "RIGHT", "commit_id": HEAD, "body": "[tests] exact body"},
            {"id": 42, "path": "src/lib.rs", "line": 12, "side": "RIGHT", "commit_id": HEAD, "body": "[tests] exact body"}
        ]);
        let observed = observed_deliveries(&duplicate, std::slice::from_ref(&planned), HEAD)?;
        ensure!(
            reconcile_deliveries(HEAD, "review-1", &[planned], &observed).is_err(),
            "duplicate returned comment id was accepted"
        );
        Ok(())
    }
}
