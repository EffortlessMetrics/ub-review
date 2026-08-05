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
use std::error::Error as StdError;
use std::fmt;

#[derive(Debug)]
struct HeadRevalidationFailure;

impl fmt::Display for HeadRevalidationFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("GitHub pull head changed")
    }
}

impl StdError for HeadRevalidationFailure {}

#[derive(Debug)]
pub(crate) struct PendingReviewPostOutcome {
    pub(crate) response: serde_json::Value,
    pub(crate) http_status: Option<u16>,
}

const REPLY_DELIVERY_RECEIPT_SCHEMA: &str = "ub-review.reply_delivery_receipt.v1";

#[derive(Clone, Debug, Deserialize, Serialize)]
struct ReplyDeliveryReceipt {
    schema: &'static str,
    exact_head_sha: String,
    claim_id: String,
    action: &'static str,
    path: String,
    line: u32,
    side: String,
    source_thread_id: String,
    expected_body_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    review_id: Option<String>,
    comment_id: String,
    confirmed_head_sha: String,
}

struct ReplyDeliveryContext<'a> {
    args: &'a PostArgs,
    api: &'a str,
    repo: &'a str,
    pull_number: u64,
    token: &'a str,
    exact_head_sha: &'a str,
    review: &'a GitHubReview,
    all_planned: &'a [PlannedDelivery],
}

trait DeliveryTransport {
    fn fetch_json(&mut self, url: &str, token: &str) -> Result<serde_json::Value>;

    fn send_json(
        &mut self,
        method: &str,
        url: &str,
        token: &str,
        payload_path: &Path,
        headers: &[&str],
    ) -> Result<HttpPostOutput>;
}

struct CurlDeliveryTransport;

impl DeliveryTransport for CurlDeliveryTransport {
    fn fetch_json(&mut self, url: &str, token: &str) -> Result<serde_json::Value> {
        run_github_api_get(Path::new("."), url, token)
    }

    fn send_json(
        &mut self,
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
}

pub(crate) fn execute_pending_review_delivery(
    args: &PostArgs,
    review: &GitHubReview,
    api_payload: &GitHubReviewPostPayload,
) -> Result<PendingReviewPostOutcome> {
    let mut transport = CurlDeliveryTransport;
    execute_pending_review_delivery_with_transport(args, review, api_payload, &mut transport)
}

fn execute_pending_review_delivery_with_transport(
    args: &PostArgs,
    review: &GitHubReview,
    api_payload: &GitHubReviewPostPayload,
    transport: &mut impl DeliveryTransport,
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

    let claim_graph = (!review.comments.is_empty())
        .then(|| load_claim_graph(args))
        .transpose()?;
    let expected_head = read_expected_delivery_head(review, claim_graph.as_ref())?;
    let current_head = fetch_pull_head(transport, api, repo, pull_number, token)?;
    let expected_head = expected_head.unwrap_or(current_head.clone());
    ensure_heads_match(
        "before pending-review creation",
        &expected_head,
        &current_head,
    )?;
    let all_planned = build_planned_deliveries(review, &expected_head, claim_graph.as_ref())?;
    let needs_existing_state = all_planned
        .iter()
        .any(|item| item.action() == DeliveryAction::Reply)
        || args.out.join("delivery-reconciliation.json").exists()
        || args.out.join("delivery-reply-receipts.json").exists();
    let existing_comments = if needs_existing_state {
        Some(fetch_pull_review_comments(
            transport,
            api,
            repo,
            pull_number,
            token,
        )?)
    } else {
        None
    };
    let prior_confirmed = prior_confirmed_deliveries(
        args,
        &all_planned,
        existing_comments.as_ref(),
        &expected_head,
    )?;
    let remaining = all_planned
        .iter()
        .filter(|item| !prior_confirmed.iter().any(|confirmed| confirmed == *item))
        .cloned()
        .collect::<Vec<_>>();
    let remaining_inline = remaining
        .iter()
        .filter(|item| item.action() == DeliveryAction::Inline)
        .cloned()
        .collect::<Vec<_>>();
    let remaining_replies = remaining
        .iter()
        .filter(|item| item.action() == DeliveryAction::Reply)
        .cloned()
        .collect::<Vec<_>>();
    let mut confirmed_for_body = prior_confirmed;

    // A reply has no pending-review container in the REST API. When a review
    // has no new inline comments, deliver the replies directly and retain the
    // exact current-head receipt packet without manufacturing a review id.
    if !review.comments.is_empty() && remaining_inline.is_empty() {
        let replies = execute_reply_deliveries(
            ReplyDeliveryContext {
                args,
                api,
                repo,
                pull_number,
                token,
                exact_head_sha: &expected_head,
                review,
                all_planned: &all_planned,
            },
            &remaining_replies,
            existing_comments.as_ref(),
            transport,
        )?;
        let rechecked_head = fetch_pull_head(transport, api, repo, pull_number, token)?;
        ensure_heads_match(
            "after direct reply delivery",
            &expected_head,
            &rechecked_head,
        )?;
        confirmed_for_body.extend(
            replies
                .iter()
                .filter_map(|receipt| {
                    all_planned.iter().find(|planned| {
                        planned.claim_id() == receipt.claim_id
                            && planned.source_thread_id() == Some(receipt.source_thread_id.as_str())
                    })
                })
                .cloned(),
        );
        write_retry_decisions(args, &all_planned, &confirmed_for_body)?;
        let response = replies
            .last()
            .map(|receipt| serde_json::json!({"id": receipt.comment_id, "state": "commented"}))
            .unwrap_or_else(|| serde_json::json!({"state": "already_delivered"}));
        return Ok(PendingReviewPostOutcome {
            response,
            http_status: Some(200),
        });
    }

    let mut transaction =
        DeliveryTransaction::new(expected_head.clone(), remaining_inline.clone())?;

    let pending_comments = api_payload
        .comments
        .iter()
        .zip(all_planned.iter())
        .filter(|(_, planned)| {
            planned.action() == DeliveryAction::Inline
                && remaining_inline.iter().any(|item| item == *planned)
        })
        .map(|(comment, _)| comment)
        .collect::<Vec<_>>();

    let pending_payload = serde_json::json!({
        "commit_id": expected_head,
        "body": api_payload.body,
        "comments": pending_comments,
    });
    let pending_payload_path = args.out.join("delivery-pending-review-payload.json");
    write_json(&pending_payload_path, &pending_payload)?;

    let mut review_id = None;
    let result = (|| -> Result<PendingReviewPostOutcome> {
        let pending = transport.send_json(
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
        let pending_json = parse_success_json(&pending, "pending review creation")?;
        let created_review_id = json_identifier(&pending_json, "id", "pending review")?;
        review_id = Some(created_review_id.clone());
        transaction.transition(DeliveryTransactionState::PendingReviewCreated)?;
        write_transaction(&args.out, &transaction)?;
        write_response_artifacts(&args.out, "pending-review", &pending)?;

        transaction.transition(DeliveryTransactionState::CommentsCreated)?;
        let comments_url = format!("{review_url}/{created_review_id}/comments");
        let listed = transport.fetch_json(&comments_url, token)?;
        write_json(
            &args.out.join("delivery-pending-review-comments.json"),
            &listed,
        )?;
        let observed = observed_deliveries(&listed, &remaining_inline, &expected_head)?;
        let reconciliation = reconcile_deliveries(
            &expected_head,
            &created_review_id,
            &remaining_inline,
            &observed,
        )?;
        transaction.transition(DeliveryTransactionState::CommentsReconciled)?;
        let reconciliation_value = serde_json::to_value(&reconciliation)?;
        write_json(
            &args.out.join("delivery-reconciliation.json"),
            &reconciliation_value,
        )?;
        write_json(
            &args.out.join("delivery-receipts.json"),
            &reconciliation_value["receipts"],
        )?;
        write_transaction(&args.out, &transaction)?;

        let replies = execute_reply_deliveries(
            ReplyDeliveryContext {
                args,
                api,
                repo,
                pull_number,
                token,
                exact_head_sha: &expected_head,
                review,
                all_planned: &all_planned,
            },
            &remaining_replies,
            existing_comments.as_ref(),
            transport,
        )?;
        confirmed_for_body.extend(
            remaining_inline.iter().cloned().chain(
                replies
                    .iter()
                    .filter_map(|receipt| {
                        all_planned.iter().find(|planned| {
                            planned.claim_id() == receipt.claim_id
                                && planned.source_thread_id()
                                    == Some(receipt.source_thread_id.as_str())
                        })
                    })
                    .cloned(),
            ),
        );
        write_retry_decisions(args, &all_planned, &confirmed_for_body)?;

        let rechecked_head = fetch_pull_head(transport, api, repo, pull_number, token)?;
        ensure_heads_match(
            "before pending-review submission",
            &expected_head,
            &rechecked_head,
        )?;
        transaction.transition(DeliveryTransactionState::HeadRevalidated)?;
        write_transaction(&args.out, &transaction)?;

        let submitted_body = body_after_confirmed_delivery(
            review,
            api_payload.body.as_str(),
            &confirmed_for_body,
            &all_planned,
        )?;
        let submit_payload = serde_json::json!({
            "event": "COMMENT",
            "body": submitted_body,
        });
        let submit_payload_path = args.out.join("delivery-submit-review-payload.json");
        write_json(&submit_payload_path, &submit_payload)?;
        let submitted = transport.send_json(
            "POST",
            &format!("{review_url}/{created_review_id}/events"),
            token,
            &submit_payload_path,
            &[
                "Accept: application/vnd.github+json",
                "Content-Type: application/json",
                "X-GitHub-Api-Version: 2022-11-28",
            ],
        )?;
        if submitted.status.success() {
            transaction.transition(DeliveryTransactionState::Submitted)?;
        }
        write_response_artifacts(&args.out, "post", &submitted)?;
        let response = parse_success_json(&submitted, "pending review submission")?;
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
            let review_is_pending = matches!(
                transaction.state(),
                DeliveryTransactionState::PendingReviewCreated
                    | DeliveryTransactionState::CommentsCreated
                    | DeliveryTransactionState::CommentsReconciled
                    | DeliveryTransactionState::HeadRevalidated
            );
            let cleanup = if review_is_pending {
                if let Some(id) = review_id.as_deref() {
                    match delete_pending_review(transport, &review_url, id, token, &args.out) {
                        Ok(()) => CleanupOutcome::Succeeded,
                        Err(cleanup_error) => {
                            CleanupOutcome::Failed(sanitize_reason(&format!("{cleanup_error:#}")))
                        }
                    }
                } else {
                    CleanupOutcome::NotAttempted
                }
            } else {
                CleanupOutcome::NotAttempted
            };
            remove_confirmation_artifacts(&args.out);
            let bookkeeping = if matches!(
                transaction.state(),
                DeliveryTransactionState::Submitted | DeliveryTransactionState::ReceiptsPersisted
            ) {
                transaction.record_post_submission_failure(
                    DeliveryFailureStage::ReceiptPersistence,
                    sanitize_reason(&format!("{error:#}")),
                )
            } else if review_is_pending && review_id.is_some() {
                let stage = failure_stage(&error, &transaction);
                transaction
                    .record_failure(stage, sanitize_reason(&format!("{error:#}")), true)
                    .and_then(|()| transaction.finish_cleanup(cleanup))
            } else {
                let stage = failure_stage(&error, &transaction);
                transaction.record_failure(stage, sanitize_reason(&format!("{error:#}")), false)
            };
            if bookkeeping.is_ok() {
                let _ = write_transaction(&args.out, &transaction);
            }
            Err(error)
        }
    }
}

fn load_claim_graph(args: &PostArgs) -> Result<serde_json::Value> {
    let graph_path = args
        .review_json
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("claim_graph.json");
    serde_json::from_slice(
        &fs::read(&graph_path).with_context(|| format!("read {}", graph_path.display()))?,
    )
    .with_context(|| format!("parse {}", graph_path.display()))
}

fn read_expected_delivery_head(
    review: &GitHubReview,
    claim_graph: Option<&serde_json::Value>,
) -> Result<Option<String>> {
    if !review.comments.is_empty() {
        let graph = claim_graph.ok_or_else(|| anyhow::anyhow!("claim graph is required"))?;
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

fn build_planned_deliveries(
    review: &GitHubReview,
    exact_head_sha: &str,
    claim_graph: Option<&serde_json::Value>,
) -> Result<Vec<PlannedDelivery>> {
    if review.comments.is_empty() {
        return Ok(Vec::new());
    }
    let graph = claim_graph.ok_or_else(|| anyhow::anyhow!("claim graph is required"))?;
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
                    matches!(
                        topic
                            .get("planned_action")
                            .and_then(serde_json::Value::as_str),
                        Some("inline") | Some("reply")
                    ) && topic.get("head_sha").and_then(serde_json::Value::as_str)
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
            ensure!(
                matches.len() == 1,
                "inline comment {}:{} has ambiguous claim plan",
                path,
                comment.line
            );
            let topic = matches.first().ok_or_else(|| {
                anyhow::anyhow!(
                    "inline comment {}:{} has no current-head inline claim plan",
                    path,
                    comment.line
                )
            })?;
            let action = topic
                .get("planned_action")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("delivery plan has no action"))?;
            let (action, source_thread_id) = match action {
                "inline" => (DeliveryAction::Inline, None),
                "reply" => {
                    let thread_id = topic
                        .get("planned_thread_id")
                        .and_then(serde_json::Value::as_str)
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| {
                            anyhow::anyhow!("reply delivery plan has no current source thread")
                        })?;
                    (DeliveryAction::Reply, Some(thread_id.to_owned()))
                }
                other => bail!("unsupported delivery action {other:?}"),
            };
            let claim_id = topic
                .get("claim_id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| anyhow::anyhow!("inline claim plan has no claim id"))?;
            let body = github_review_post_comment_body(comment)?;
            PlannedDelivery::new(
                exact_head_sha,
                claim_id,
                action,
                DeliveryLocation::new(path, comment.line, comment.side.clone()),
                source_thread_id,
                sha256_hex(body.as_bytes()),
            )
        })
        .collect()
}

#[cfg(test)]
fn build_planned_inline_deliveries(
    review: &GitHubReview,
    exact_head_sha: &str,
    claim_graph: Option<&serde_json::Value>,
) -> Result<Vec<PlannedDelivery>> {
    build_planned_deliveries(review, exact_head_sha, claim_graph).map(|planned| {
        planned
            .into_iter()
            .filter(|item| item.action() == DeliveryAction::Inline)
            .collect()
    })
}

fn fetch_pull_review_comments(
    transport: &mut impl DeliveryTransport,
    api: &str,
    repo: &str,
    pull_number: u64,
    token: &str,
) -> Result<serde_json::Value> {
    let mut page: usize = 1;
    let mut comments = Vec::new();
    loop {
        let value = transport.fetch_json(
            &format!("{api}/repos/{repo}/pulls/{pull_number}/comments?per_page=100&page={page}"),
            token,
        )?;
        let page_comments = value.as_array().ok_or_else(|| {
            anyhow::anyhow!("GitHub pull review comments response must be an array")
        })?;
        let page_len = page_comments.len();
        comments.extend(page_comments.iter().cloned());
        if page_len < 100 {
            break;
        }
        page = page
            .checked_add(1)
            .ok_or_else(|| anyhow::anyhow!("GitHub pull review comments page overflowed"))?;
    }
    Ok(serde_json::Value::Array(comments))
}

fn prior_confirmed_deliveries(
    args: &PostArgs,
    planned: &[PlannedDelivery],
    current_comments: Option<&serde_json::Value>,
    exact_head_sha: &str,
) -> Result<Vec<PlannedDelivery>> {
    let Some(current_comments) = current_comments else {
        return Ok(Vec::new());
    };
    let items = current_comments
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("current GitHub review comments must be an array"))?;
    let mut receipts = Vec::new();
    for path in [
        args.out.join("delivery-reconciliation.json"),
        args.out.join("delivery-reply-receipts.json"),
    ] {
        if !path.exists() {
            continue;
        }
        let value: serde_json::Value = serde_json::from_slice(
            &fs::read(&path).with_context(|| format!("read {}", path.display()))?,
        )?;
        if let Some(array) = value.as_array() {
            receipts.extend(array.iter().cloned());
        } else if let Some(array) = value.get("receipts").and_then(serde_json::Value::as_array) {
            receipts.extend(array.iter().cloned());
        }
    }
    let mut confirmed = Vec::new();
    for item in planned {
        let identity = serde_json::to_value(item)?;
        let current = items.iter().find(|comment| {
            comment_matches_delivery(comment, item, exact_head_sha).unwrap_or(false)
        });
        let receipt_match = receipts.iter().any(|receipt| {
            receipt.get("exact_head_sha") == identity.get("exact_head_sha")
                && receipt.get("claim_id") == identity.get("claim_id")
                && receipt.get("action") == identity.get("action")
                && receipt.get("path") == identity.get("path")
                && receipt.get("line") == identity.get("line")
                && receipt.get("side") == identity.get("side")
                && receipt.get("source_thread_id") == identity.get("source_thread_id")
                && receipt.get("expected_body_digest") == identity.get("expected_body_digest")
                && receipt.get("confirmed_head_sha")
                    == Some(&serde_json::Value::String(exact_head_sha.to_owned()))
                && receipt.get("comment_id").is_some()
        });
        if current.is_some() && (receipt_match || item.action() == DeliveryAction::Reply) {
            confirmed.push(item.clone());
        }
    }
    Ok(confirmed)
}

fn comment_matches_delivery(
    comment: &serde_json::Value,
    planned: &PlannedDelivery,
    exact_head_sha: &str,
) -> Result<bool> {
    let id = comment.get("id").and_then(|value| {
        value
            .as_u64()
            .map(|id| id.to_string())
            .or_else(|| value.as_str().map(str::to_owned))
    });
    let path = comment
        .get("path")
        .and_then(serde_json::Value::as_str)
        .map(normalize_repo_path);
    let line = comment
        .get("line")
        .and_then(serde_json::Value::as_u64)
        .and_then(|line| u32::try_from(line).ok());
    let side = comment.get("side").and_then(serde_json::Value::as_str);
    let head = comment.get("commit_id").and_then(serde_json::Value::as_str);
    let body = comment.get("body").and_then(serde_json::Value::as_str);
    let (planned_path, planned_line, planned_side) = planned.location();
    let shape_matches = id.is_some()
        && path.as_deref() == Some(planned_path)
        && line == Some(planned_line)
        && side == Some(planned_side)
        && head == Some(exact_head_sha)
        && body.is_some_and(|body| sha256_hex(body.as_bytes()) == planned.expected_body_digest());
    if !shape_matches {
        return Ok(false);
    }
    if planned.action() == DeliveryAction::Reply {
        Ok(comment
            .get("in_reply_to_id")
            .and_then(|value| {
                value
                    .as_u64()
                    .map(|id| id.to_string())
                    .or_else(|| value.as_str().map(str::to_owned))
            })
            .as_deref()
            == planned.source_thread_id())
    } else {
        Ok(comment
            .get("in_reply_to_id")
            .is_none_or(serde_json::Value::is_null))
    }
}

fn comment_for_planned<'a>(
    review: &'a GitHubReview,
    all_planned: &[PlannedDelivery],
    planned: &PlannedDelivery,
) -> Result<&'a GitHubReviewComment> {
    review
        .comments
        .iter()
        .zip(all_planned.iter())
        .find(|(_, candidate)| *candidate == planned)
        .map(|(comment, _)| comment)
        .ok_or_else(|| anyhow::anyhow!("delivery plan has no matching review comment"))
}

fn execute_reply_deliveries(
    context: ReplyDeliveryContext<'_>,
    planned_replies: &[PlannedDelivery],
    current_comments: Option<&serde_json::Value>,
    transport: &mut impl DeliveryTransport,
) -> Result<Vec<ReplyDeliveryReceipt>> {
    if planned_replies.is_empty() {
        return Ok(Vec::new());
    }
    let current_comments = current_comments
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("current review comments are required for replies"))?;
    let mut receipts = Vec::new();
    for planned in planned_replies {
        let source_thread_id = planned
            .source_thread_id()
            .ok_or_else(|| anyhow::anyhow!("reply delivery has no source thread"))?;
        let source = current_comments
            .iter()
            .filter(|comment| {
                let id = comment.get("id").and_then(|value| {
                    value
                        .as_u64()
                        .map(|id| id.to_string())
                        .or_else(|| value.as_str().map(str::to_owned))
                });
                id.as_deref() == Some(source_thread_id)
                    && comment.get("commit_id").and_then(serde_json::Value::as_str)
                        == Some(context.exact_head_sha)
                    && comment.get("path").and_then(serde_json::Value::as_str)
                        == Some(planned.location().0)
                    && comment.get("line").and_then(serde_json::Value::as_u64)
                        == Some(u64::from(planned.location().1))
                    && comment.get("side").and_then(serde_json::Value::as_str)
                        == Some(planned.location().2)
            })
            .collect::<Vec<_>>();
        ensure!(
            source.len() == 1,
            "reply source thread {} is missing, stale, or ambiguous",
            source_thread_id
        );
        let comment = comment_for_planned(context.review, context.all_planned, planned)?;
        let body = github_review_post_comment_body(comment)?;
        let source_id = source_thread_id.parse::<u64>().with_context(|| {
            format!("reply source thread {source_thread_id} is not a numeric GitHub comment id")
        })?;
        let payload = serde_json::json!({
            "body": body,
            "in_reply_to": source_id,
        });
        let payload_path = context.args.out.join("delivery-reply-payload.json");
        write_json(&payload_path, &payload)?;
        let output = transport.send_json(
            "POST",
            &format!(
                "{}/repos/{}/pulls/{}/comments",
                context.api, context.repo, context.pull_number
            ),
            context.token,
            &payload_path,
            &[
                "Accept: application/vnd.github+json",
                "Content-Type: application/json",
                "X-GitHub-Api-Version: 2022-11-28",
            ],
        )?;
        let response = parse_success_json(&output, "review comment reply")?;
        let comment_id = json_identifier(&response, "id", "review comment reply")?;
        ensure!(
            response
                .get("commit_id")
                .and_then(serde_json::Value::as_str)
                == Some(context.exact_head_sha),
            "review comment reply {} was returned for another head",
            comment_id
        );
        let response_source_thread = response.get("in_reply_to_id").and_then(|value| {
            value
                .as_u64()
                .map(|id| id.to_string())
                .or_else(|| value.as_str().map(str::to_owned))
        });
        ensure!(
            response_source_thread.as_deref() == Some(source_thread_id),
            "review comment reply {} was returned for another source thread",
            comment_id
        );
        ensure!(
            response.get("body").and_then(serde_json::Value::as_str)
                .is_some_and(|value| sha256_hex(value.as_bytes()) == planned.expected_body_digest()),
            "review comment reply {} body does not match the planned digest",
            comment_id
        );
        let receipt = ReplyDeliveryReceipt {
            schema: REPLY_DELIVERY_RECEIPT_SCHEMA,
            exact_head_sha: context.exact_head_sha.to_owned(),
            claim_id: planned.claim_id().to_owned(),
            action: "reply",
            path: planned.location().0.to_owned(),
            line: planned.location().1,
            side: planned.location().2.to_owned(),
            source_thread_id: source_thread_id.to_owned(),
            expected_body_digest: planned.expected_body_digest().to_owned(),
            review_id: response.get("pull_request_review_id").and_then(|value| {
                value
                    .as_u64()
                    .map(|id| id.to_string())
                    .or_else(|| value.as_str().map(str::to_owned))
            }),
            comment_id,
            confirmed_head_sha: context.exact_head_sha.to_owned(),
        };
        receipts.push(receipt);
        write_json(
            &context.args.out.join("delivery-reply-receipts.json"),
            &serde_json::to_value(&receipts)?,
        )?;
    }
    Ok(receipts)
}

fn write_retry_decisions(
    args: &PostArgs,
    planned: &[PlannedDelivery],
    confirmed: &[PlannedDelivery],
) -> Result<()> {
    let decisions = planned
        .iter()
        .map(|item| {
            serde_json::json!({
                "identity": item,
                "status": if confirmed.iter().any(|candidate| candidate == item) { "confirmed_current_head" } else { "unconfirmed" },
            })
        })
        .collect::<Vec<_>>();
    write_json(
        &args.out.join("delivery-retry-decisions.json"),
        &serde_json::Value::Array(decisions),
    )
}

fn remove_one_confirmed_body_surface(body: &str, comment: &str) -> String {
    let comment_lines = comment.lines().collect::<Vec<_>>();
    if comment_lines.is_empty() {
        return body.to_owned();
    }
    let body_lines = body.lines().collect::<Vec<_>>();
    let Some(start) = body_lines.windows(comment_lines.len()).position(|window| {
        window
            .first()
            .map(|line| {
                let first = line.trim().strip_prefix("- ").unwrap_or(line.trim());
                first == comment_lines[0]
            })
            .unwrap_or(false)
            && window
                .iter()
                .skip(1)
                .zip(comment_lines.iter().skip(1))
                .all(|(actual, expected)| actual == expected)
    }) else {
        return body.to_owned();
    };
    body_lines
        .into_iter()
        .enumerate()
        .filter_map(|(index, line)| {
            ((index < start) || (index >= start + comment_lines.len())).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn body_after_confirmed_delivery(
    review: &GitHubReview,
    body: &str,
    confirmed: &[PlannedDelivery],
    all_planned: &[PlannedDelivery],
) -> Result<String> {
    let confirmed_comments = review
        .comments
        .iter()
        .zip(all_planned.iter())
        .filter(|(_, planned)| confirmed.iter().any(|candidate| candidate == *planned))
        .map(|(comment, _)| github_review_post_comment_body(comment))
        .collect::<Result<Vec<_>>>()?;
    if confirmed_comments.is_empty() {
        return Ok(body.to_owned());
    }
    let mut filtered = body.to_owned();
    for comment in confirmed_comments {
        filtered = remove_one_confirmed_body_surface(&filtered, &comment);
    }
    Ok(filtered)
}

fn fetch_pull_head(
    transport: &mut impl DeliveryTransport,
    api: &str,
    repo: &str,
    pull_number: u64,
    token: &str,
) -> Result<String> {
    let value = transport.fetch_json(&format!("{api}/repos/{repo}/pulls/{pull_number}"), token)?;
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
        return Err(anyhow::Error::new(HeadRevalidationFailure).context(format!(
            "GitHub pull head changed {stage}: expected {expected}, got {actual}"
        )));
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
        let commit_id = item
            .get("commit_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| anyhow::anyhow!("GitHub review comment {id} has no commit_id"))?;
        ensure!(
            commit_id == exact_head_sha,
            "GitHub review comment {id} is bound to another head"
        );
        let candidates = planned
            .iter()
            .filter(|delivery| delivery.location() == (path.as_str(), line, side))
            .collect::<Vec<_>>();
        ensure!(
            candidates.len() == 1,
            "GitHub returned comment {id} with an ambiguous planned location"
        );
        let delivery = candidates.first().ok_or_else(|| {
            anyhow::anyhow!("GitHub returned unexpected comment {id} at {path}:{line}")
        })?;
        ensure!(
            sha256_hex(body.as_bytes()) == delivery.expected_body_digest(),
            "GitHub returned comment {id} with an unexpected body"
        );
        observed.push(ObservedDelivery::new(id, (*delivery).clone())?);
    }
    Ok(observed)
}

fn delete_pending_review(
    transport: &mut impl DeliveryTransport,
    review_url: &str,
    review_id: &str,
    token: &str,
    out: &Path,
) -> Result<()> {
    let path = out.join("delivery-cleanup-payload.json");
    write_json(&path, &serde_json::json!({}))?;
    let output = transport.send_json(
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
    write_json(&out.join("delivery-transaction.json"), &value)
}

fn remove_confirmation_artifacts(out: &Path) {
    let _ = fs::remove_file(out.join("delivery-reconciliation.json"));
    let _ = fs::remove_file(out.join("delivery-receipts.json"));
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
    if error
        .chain()
        .any(|cause| cause.downcast_ref::<HeadRevalidationFailure>().is_some())
    {
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
pub(crate) use tests::{FakeHttpResponse, spawn_fake_delivery_api};

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{bail, ensure};
    use std::collections::BTreeMap;
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::{Duration, Instant};

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

    fn review_with_comment(path: &str, line: u32, body: &str) -> GitHubReview {
        GitHubReview {
            event: "COMMENT".to_owned(),
            body: "review body".to_owned(),
            comments: vec![GitHubReviewComment {
                path: path.to_owned(),
                line,
                side: "RIGHT".to_owned(),
                body: body.to_owned(),
                suggestion: None,
            }],
        }
    }

    fn graph_for(path: &str, line: u32, action: &str, head: &str) -> serde_json::Value {
        serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": head,
            "topics": [{
                "claim_id": "claim-1",
                "planned_action": action,
                "head_sha": head,
                "path": path,
                "anchor": line
            }]
        })
    }

    #[derive(Clone)]
    pub(crate) struct FakeHttpResponse {
        status: u16,
        body: String,
    }

    impl FakeHttpResponse {
        pub(crate) fn new(status: u16, body: impl Into<String>) -> Self {
            Self {
                status,
                body: body.into(),
            }
        }
    }

    type FakeRequest = (String, String);
    type FakeApiHandle = thread::JoinHandle<Result<Vec<FakeRequest>>>;

    fn read_fake_request(stream: TcpStream) -> Result<(String, String)> {
        let mut reader = BufReader::new(stream);
        let mut request_line = String::new();
        reader.read_line(&mut request_line)?;
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line == "\r\n" || line.is_empty() {
                break;
            }
            if let Some(value) = line.strip_prefix("Content-Length:") {
                content_length = value.trim().parse()?;
            }
        }
        let mut body = vec![0u8; content_length];
        reader.read_exact(&mut body)?;
        Ok((request_line.trim().to_owned(), String::from_utf8(body)?))
    }

    fn write_fake_response(mut stream: TcpStream, response: &FakeHttpResponse) -> Result<()> {
        let reason = if response.status < 300 { "OK" } else { "ERROR" };
        let bytes = response.body.as_bytes();
        write!(
            stream,
            "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.status,
            reason,
            bytes.len(),
            response.body
        )?;
        stream.flush()?;
        Ok(())
    }

    pub(crate) fn spawn_fake_delivery_api(
        responses: Vec<FakeHttpResponse>,
    ) -> Result<(String, FakeApiHandle)> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let address = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || {
            let mut requests: Vec<FakeRequest> = Vec::new();
            for response in responses {
                let deadline = Instant::now() + Duration::from_secs(5);
                let (stream, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error)
                            if error.kind() == ErrorKind::WouldBlock
                                && Instant::now() < deadline =>
                        {
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(error) if error.kind() == ErrorKind::WouldBlock => {
                            let captured = requests
                                .iter()
                                .map(|(line, _)| line.as_str())
                                .collect::<Vec<_>>();
                            bail!(
                                "fake delivery API timed out waiting for request {}; captured {}: {captured:?}",
                                requests.len() + 1,
                                requests.len()
                            );
                        }
                        Err(error) => return Err(error.into()),
                    }
                };
                stream.set_read_timeout(Some(Duration::from_secs(5)))?;
                let request = read_fake_request(stream.try_clone()?)?;
                write_fake_response(stream, &response)?;
                requests.push(request);
            }
            Ok(requests)
        });
        Ok((address, handle))
    }

    fn delivery_args(root: &Path, api: &str) -> PostArgs {
        PostArgs {
            review_json: root.join("github-review.json"),
            diff_patch: None,
            out: root.join("review"),
            github_token: Some("test-token".to_owned()),
            repo: Some("owner/repo".to_owned()),
            pull_number: Some(42),
            github_api_url: api.to_owned(),
            fail_on_post_error: true,
        }
    }

    fn delivery_review() -> (GitHubReview, GitHubReviewPostPayload) {
        let comment = GitHubReviewComment {
            path: "src/lib.rs".to_owned(),
            line: 12,
            side: "RIGHT".to_owned(),
            body: "[tests] exact body".to_owned(),
            suggestion: None,
        };
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "review body".to_owned(),
            comments: vec![comment.clone()],
        };
        let payload = GitHubReviewPostPayload {
            event: review.event.clone(),
            body: review.body.clone(),
            comments: vec![GitHubReviewPostComment {
                path: comment.path,
                line: comment.line,
                side: comment.side,
                body: comment.body,
            }],
        };
        (review, payload)
    }

    fn successful_delivery_responses() -> Vec<FakeHttpResponse> {
        vec![
            FakeHttpResponse {
                status: 200,
                body: format!(r#"{{"head":{{"sha":"{HEAD}"}}}}"#),
            },
            FakeHttpResponse {
                status: 200,
                body: r#"{"id":987}"#.to_owned(),
            },
            FakeHttpResponse {
                status: 200,
                body: format!(
                    r#"[{{"id":123,"path":"src/lib.rs","line":12,"side":"RIGHT","commit_id":"{HEAD}","body":"[tests] exact body"}}]"#
                ),
            },
            FakeHttpResponse {
                status: 200,
                body: format!(r#"{{"head":{{"sha":"{HEAD}"}}}}"#),
            },
            FakeHttpResponse {
                status: 200,
                body: r#"{"id":987,"state":"commented"}"#.to_owned(),
            },
        ]
    }

    struct ScriptedTransport {
        gets: VecDeque<serde_json::Value>,
        sends: VecDeque<HttpPostOutput>,
    }

    impl DeliveryTransport for ScriptedTransport {
        fn fetch_json(&mut self, _url: &str, _token: &str) -> Result<serde_json::Value> {
            self.gets
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("scripted GET exhausted"))
        }

        fn send_json(
            &mut self,
            _method: &str,
            _url: &str,
            _token: &str,
            _payload_path: &Path,
            _headers: &[&str],
        ) -> Result<HttpPostOutput> {
            self.sends
                .pop_front()
                .ok_or_else(|| anyhow::anyhow!("scripted send exhausted"))
        }
    }

    fn command_status(success: bool) -> Result<std::process::ExitStatus> {
        #[cfg(windows)]
        {
            let code = if success { "exit 0" } else { "exit 1" };
            Ok(std::process::Command::new("cmd")
                .arg("/c")
                .arg(code)
                .status()?)
        }
        #[cfg(not(windows))]
        {
            Ok(std::process::Command::new(if success { "true" } else { "false" }).status()?)
        }
    }

    fn scripted_output(body: &str, success: bool) -> Result<HttpPostOutput> {
        Ok(HttpPostOutput {
            status: command_status(success)?,
            stdout: body.as_bytes().to_vec(),
            stderr: if success {
                Vec::new()
            } else {
                b"scripted failure".to_vec()
            },
            http_status: Some(if success { 200 } else { 500 }),
        })
    }

    #[test]
    fn generic_transport_path_preserves_receipts_and_payload_boundaries() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph)?,
        )?;
        let (review, payload) = delivery_review();
        let mut transport = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::json!({"head": {"sha": HEAD}}),
                serde_json::json!([{
                    "id": 987,
                    "path": "src/lib.rs",
                    "line": 12,
                    "side": "RIGHT",
                    "commit_id": HEAD,
                    "body": "[tests] exact body"
                }]),
                serde_json::json!({"head": {"sha": HEAD}}),
            ]),
            sends: VecDeque::from([
                scripted_output(r#"{"id":987}"#, true)?,
                scripted_output(r#"{"id":987,"state":"commented"}"#, true)?,
            ]),
        };
        let outcome = execute_pending_review_delivery_with_transport(
            &delivery_args(temp.path(), "http://scripted"),
            &review,
            &payload,
            &mut transport,
        )?;
        ensure!(outcome.response["state"] == "commented");
        let transaction: DeliveryTransaction = serde_json::from_slice(&fs::read(
            temp.path().join("review/delivery-transaction.json"),
        )?)?;
        ensure!(transaction.state() == &DeliveryTransactionState::ReceiptsPersisted);
        ensure!(transport.gets.is_empty() && transport.sends.is_empty());
        Ok(())
    }

    #[test]
    fn production_delivery_reconciles_head_comments_and_submission() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph)?,
        )?;
        let (review, payload) = delivery_review();
        let (api, server) = spawn_fake_delivery_api(successful_delivery_responses())?;
        let outcome =
            execute_pending_review_delivery(&delivery_args(temp.path(), &api), &review, &payload)?;
        ensure!(outcome.response["state"] == "commented");
        let transaction: DeliveryTransaction = serde_json::from_slice(&fs::read(
            temp.path().join("review/delivery-transaction.json"),
        )?)?;
        ensure!(
            transaction.state() == &DeliveryTransactionState::ReceiptsPersisted,
            "successful delivery did not persist terminal state"
        );
        let reconciliation: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/delivery-reconciliation.json"),
        )?)?;
        ensure!(reconciliation["planned_count"] == 1);
        ensure!(reconciliation["receipts"][0]["comment_id"] == "123");
        let requests = server
            .join()
            .map_err(|_| anyhow::anyhow!("fake API panicked"))??;
        ensure!(requests.len() == 5, "expected five API calls");
        ensure!(requests[0].0.starts_with("GET /repos/owner/repo/pulls/42 "));
        ensure!(
            requests[1]
                .0
                .starts_with("POST /repos/owner/repo/pulls/42/reviews ")
        );
        ensure!(requests[1].1.contains("commit_id") && requests[1].1.contains(HEAD));
        ensure!(
            !requests[1].1.contains("PENDING"),
            "pending payload submitted an event"
        );
        ensure!(requests[2].0.contains("/comments HTTP/1.1"));
        ensure!(requests[3].0.starts_with("GET /repos/owner/repo/pulls/42 "));
        ensure!(requests[4].0.contains("/events HTTP/1.1"));
        ensure!(requests[4].1.contains("event") && requests[4].1.contains("COMMENT"));
        Ok(())
    }

    #[test]
    fn production_delivery_cleans_up_when_head_changes_before_submit() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph)?,
        )?;
        let (review, payload) = delivery_review();
        let responses = vec![
            successful_delivery_responses()[0].clone(),
            successful_delivery_responses()[1].clone(),
            successful_delivery_responses()[2].clone(),
            FakeHttpResponse {
                status: 200,
                body: r#"{"head":{"sha":"another-head"}}"#.to_owned(),
            },
            FakeHttpResponse {
                status: 204,
                body: String::new(),
            },
        ];
        let (api, server) = spawn_fake_delivery_api(responses)?;
        ensure!(
            execute_pending_review_delivery(&delivery_args(temp.path(), &api), &review, &payload)
                .is_err(),
            "changed head was accepted for submission"
        );
        let transaction: DeliveryTransaction = serde_json::from_slice(&fs::read(
            temp.path().join("review/delivery-transaction.json"),
        )?)?;
        ensure!(transaction.state() == &DeliveryTransactionState::CleanedUp);
        ensure!(
            !temp
                .path()
                .join("review/delivery-reconciliation.json")
                .exists()
        );
        let requests = server
            .join()
            .map_err(|_| anyhow::anyhow!("fake API panicked"))??;
        ensure!(
            requests[4]
                .0
                .starts_with("DELETE /repos/owner/repo/pulls/42/reviews/987 ")
        );
        Ok(())
    }

    #[test]
    fn planned_inline_delivery_binds_exact_claim_graph_identity() -> Result<()> {
        let review = review_with_comment("src\\lib.rs", 12, "[tests] exact body");
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        let planned = build_planned_inline_deliveries(&review, HEAD, Some(&graph))?;
        ensure!(planned.len() == 1, "expected one planned delivery");
        let planned_value = serde_json::to_value(&planned[0])?;
        ensure!(planned_value["claim_id"] == "claim-1", "wrong claim id");
        ensure!(planned_value["exact_head_sha"] == HEAD, "wrong exact head");
        ensure!(planned[0].location() == ("src/lib.rs", 12, "RIGHT"));
        ensure!(
            planned_value["source_thread_id"].is_null(),
            "inline has thread id"
        );
        Ok(())
    }

    #[test]
    fn planned_inline_delivery_rejects_missing_stale_and_ambiguous_plans() -> Result<()> {
        let review = review_with_comment("src/lib.rs", 12, "[tests] exact body");
        ensure!(
            build_planned_inline_deliveries(&review, HEAD, None).is_err(),
            "missing claim graph was accepted"
        );
        let stale = graph_for(
            "src/lib.rs",
            12,
            "inline",
            "fedcba9876543210fedcba9876543210fedcba98",
        );
        ensure!(
            build_planned_inline_deliveries(&review, HEAD, Some(&stale)).is_err(),
            "stale claim plan was accepted"
        );
        let mut ambiguous = graph_for("src/lib.rs", 12, "inline", HEAD);
        ambiguous["topics"] = serde_json::json!([
            {"claim_id": "claim-1", "planned_action": "inline", "head_sha": HEAD, "path": "src/lib.rs", "anchor": 12},
            {"claim_id": "claim-2", "planned_action": "inline", "head_sha": HEAD, "path": "src/lib.rs", "anchor": 12}
        ]);
        ensure!(
            build_planned_inline_deliveries(&review, HEAD, Some(&ambiguous)).is_err(),
            "ambiguous claim plan was accepted"
        );
        Ok(())
    }

    #[test]
    fn reply_delivery_posts_to_exact_current_source_and_receipts_it() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let graph = serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": HEAD,
            "topics": [{
                "claim_id": "claim-1",
                "planned_action": "reply",
                "planned_thread_id": "123",
                "head_sha": HEAD,
                "path": "src/lib.rs",
                "anchor": 12
            }]
        });
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph)?,
        )?;
        let (review, payload) = delivery_review();
        let mut transport = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::json!({"head": {"sha": HEAD}}),
                serde_json::json!([{
                    "id": 123,
                    "path": "src/lib.rs",
                    "line": 12,
                    "side": "RIGHT",
                    "commit_id": HEAD,
                    "body": "prior finding"
                }]),
                serde_json::json!({"head": {"sha": HEAD}}),
            ]),
            sends: VecDeque::from([scripted_output(
                &format!(
                    r#"{{"id":456,"path":"src/lib.rs","line":12,"side":"RIGHT","commit_id":"{HEAD}","body":"[tests] exact body","in_reply_to_id":123,"pull_request_review_id":987}}"#
                ),
                true,
            )?]),
        };
        let outcome = execute_pending_review_delivery_with_transport(
            &delivery_args(temp.path(), "http://scripted"),
            &review,
            &payload,
            &mut transport,
        )?;
        ensure!(outcome.response["id"] == "456");
        let receipts: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/delivery-reply-receipts.json"),
        )?)?;
        ensure!(receipts[0]["claim_id"] == "claim-1");
        ensure!(receipts[0]["source_thread_id"] == "123");
        ensure!(receipts[0]["comment_id"] == "456");
        ensure!(receipts[0]["review_id"] == "987");
        ensure!(
            transport.gets.is_empty() && transport.sends.is_empty(),
            "reply path left scripted transport work"
        );
        Ok(())
    }

    #[test]
    fn reply_delivery_reuses_exact_current_comment_without_duplicate_post() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let graph = serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": HEAD,
            "topics": [{
                "claim_id": "claim-1",
                "planned_action": "reply",
                "planned_thread_id": "123",
                "head_sha": HEAD,
                "path": "src/lib.rs",
                "anchor": 12
            }]
        });
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph)?,
        )?;
        let (review, payload) = delivery_review();
        let first_reply = format!(
            r#"{{"id":456,"path":"src/lib.rs","line":12,"side":"RIGHT","commit_id":"{HEAD}","body":"[tests] exact body","in_reply_to_id":123}}"#
        );
        let mut first_transport = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::json!({"head": {"sha": HEAD}}),
                serde_json::json!([{
                    "id": 123,
                    "path": "src/lib.rs",
                    "line": 12,
                    "side": "RIGHT",
                    "commit_id": HEAD,
                    "body": "prior finding"
                }]),
                serde_json::json!({"head": {"sha": HEAD}}),
            ]),
            sends: VecDeque::from([scripted_output(&first_reply, true)?]),
        };
        execute_pending_review_delivery_with_transport(
            &delivery_args(temp.path(), "http://scripted"),
            &review,
            &payload,
            &mut first_transport,
        )?;
        ensure!(first_transport.gets.is_empty() && first_transport.sends.is_empty());

        let mut retry_transport = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::json!({"head": {"sha": HEAD}}),
                serde_json::json!([
                    {
                        "id": 123,
                        "path": "src/lib.rs",
                        "line": 12,
                        "side": "RIGHT",
                        "commit_id": HEAD,
                        "body": "prior finding"
                    },
                    {
                        "id": 456,
                        "path": "src/lib.rs",
                        "line": 12,
                        "side": "RIGHT",
                        "commit_id": HEAD,
                        "body": "[tests] exact body",
                        "in_reply_to_id": 123
                    }
                ]),
                serde_json::json!({"head": {"sha": HEAD}}),
            ]),
            sends: VecDeque::new(),
        };
        let outcome = execute_pending_review_delivery_with_transport(
            &delivery_args(temp.path(), "http://scripted"),
            &review,
            &payload,
            &mut retry_transport,
        )?;
        ensure!(outcome.response["state"] == "already_delivered");
        ensure!(retry_transport.gets.is_empty() && retry_transport.sends.is_empty());
        let decisions: serde_json::Value = serde_json::from_slice(&fs::read(
            temp.path().join("review/delivery-retry-decisions.json"),
        )?)?;
        ensure!(decisions[0]["status"] == "confirmed_current_head");
        Ok(())
    }

    #[test]
    fn reply_delivery_rejects_stale_source_without_posting() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let graph = serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": HEAD,
            "topics": [{
                "claim_id": "claim-1",
                "planned_action": "reply",
                "planned_thread_id": "123",
                "head_sha": HEAD,
                "path": "src/lib.rs",
                "anchor": 12
            }]
        });
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph)?,
        )?;
        let (review, payload) = delivery_review();
        let mut transport = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::json!({"head": {"sha": HEAD}}),
                serde_json::json!([{
                    "id": 123,
                    "path": "src/lib.rs",
                    "line": 12,
                    "side": "RIGHT",
                    "commit_id": "fedcba9876543210fedcba9876543210fedcba98",
                    "body": "prior finding"
                }]),
            ]),
            sends: VecDeque::new(),
        };
        let error = match execute_pending_review_delivery_with_transport(
            &delivery_args(temp.path(), "http://scripted"),
            &review,
            &payload,
            &mut transport,
        ) {
            Ok(_) => return Err(anyhow::anyhow!("stale source thread was accepted")),
            Err(error) => error,
        };
        ensure!(
            format!("{error:#}").contains("missing, stale, or ambiguous"),
            "stale source diagnostic lost its actionable classification: {error:#}"
        );
        ensure!(transport.sends.is_empty(), "stale source triggered a POST");
        Ok(())
    }

    #[test]
    fn reply_delivery_rejects_wrong_response_identity_without_receipt() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let graph = serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": HEAD,
            "topics": [{
                "claim_id": "claim-1",
                "planned_action": "reply",
                "planned_thread_id": "123",
                "head_sha": HEAD,
                "path": "src/lib.rs",
                "anchor": 12
            }]
        });
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph)?,
        )?;
        let (review, payload) = delivery_review();
        let mut transport = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::json!({"head": {"sha": HEAD}}),
                serde_json::json!([{
                    "id": 123,
                    "path": "src/lib.rs",
                    "line": 12,
                    "side": "RIGHT",
                    "commit_id": HEAD,
                    "body": "prior finding"
                }]),
            ]),
            sends: VecDeque::from([scripted_output(
                r#"{"id":456,"commit_id":"wrong-head","body":"[tests] exact body"}"#,
                true,
            )?]),
        };
        let error = match execute_pending_review_delivery_with_transport(
            &delivery_args(temp.path(), "http://scripted"),
            &review,
            &payload,
            &mut transport,
        ) {
            Ok(_) => return Err(anyhow::anyhow!("wrong-head reply response was accepted")),
            Err(error) => error,
        };
        ensure!(
            format!("{error:#}").contains("returned for another head"),
            "wrong-head response lost its exact identity diagnostic: {error:#}"
        );
        ensure!(
            !temp
                .path()
                .join("review/delivery-reply-receipts.json")
                .exists(),
            "invalid reply response produced a receipt"
        );
        Ok(())
    }

    #[test]
    fn reply_delivery_rejects_wrong_response_source_thread_without_receipt() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let graph = serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": HEAD,
            "topics": [{
                "claim_id": "claim-1",
                "planned_action": "reply",
                "planned_thread_id": "123",
                "head_sha": HEAD,
                "path": "src/lib.rs",
                "anchor": 12
            }]
        });
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph)?,
        )?;
        let (review, payload) = delivery_review();
        let mut transport = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::json!({"head": {"sha": HEAD}}),
                serde_json::json!([{
                    "id": 123,
                    "path": "src/lib.rs",
                    "line": 12,
                    "side": "RIGHT",
                    "commit_id": HEAD,
                    "body": "prior finding"
                }]),
            ]),
            sends: VecDeque::from([scripted_output(
                &format!(
                    r#"{{"id":456,"commit_id":"{HEAD}","body":"[tests] exact body","in_reply_to_id":999}}"#
                ),
                true,
            )?]),
        };
        let error = match execute_pending_review_delivery_with_transport(
            &delivery_args(temp.path(), "http://scripted"),
            &review,
            &payload,
            &mut transport,
        ) {
            Ok(_) => return Err(anyhow::anyhow!("wrong-source reply response was accepted")),
            Err(error) => error,
        };
        ensure!(format!("{error:#}").contains("another source thread"));
        ensure!(
            !temp
                .path()
                .join("review/delivery-reply-receipts.json")
                .exists()
        );
        Ok(())
    }

    #[test]
    fn reply_identity_requires_exact_head_source_and_body() -> Result<()> {
        let body = "[tests] exact body";
        let planned = PlannedDelivery::new(
            HEAD,
            "claim-1",
            DeliveryAction::Reply,
            DeliveryLocation::new("src/lib.rs", 12, "RIGHT"),
            Some("123".to_owned()),
            sha256_hex(body.as_bytes()),
        )?;
        let valid = serde_json::json!({
            "id": 456,
            "path": "src/lib.rs",
            "line": 12,
            "side": "RIGHT",
            "commit_id": HEAD,
            "body": body,
            "in_reply_to_id": 123
        });
        ensure!(comment_matches_delivery(&valid, &planned, HEAD)?);
        for (label, mut invalid) in [
            ("wrong head", valid.clone()),
            ("wrong source", valid.clone()),
            ("wrong body", valid.clone()),
        ] {
            match label {
                "wrong head" => invalid["commit_id"] = serde_json::json!("other-head"),
                "wrong source" => invalid["in_reply_to_id"] = serde_json::json!(999),
                "wrong body" => invalid["body"] = serde_json::json!("different body"),
                _ => {}
            }
            ensure!(
                !comment_matches_delivery(&invalid, &planned, HEAD)?,
                "{label} was accepted"
            );
        }
        Ok(())
    }

    #[test]
    fn confirmed_delivery_removes_only_the_matching_body_surface() -> Result<()> {
        let (review, _) = delivery_review();
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        let planned = build_planned_deliveries(&review, HEAD, Some(&graph))?;
        let body = "## Confirmed findings\n\n- [tests] exact body\n- [tests] exact body\n- [tests] unrelated finding\n";
        let filtered = body_after_confirmed_delivery(&review, body, &planned, &planned)?;
        ensure!(
            filtered
                == "## Confirmed findings\n\n- [tests] exact body\n- [tests] unrelated finding"
        );
        Ok(())
    }

    #[test]
    fn confirmed_delivery_removes_a_multiline_suggestion_surface_once() -> Result<()> {
        let comment = GitHubReviewComment {
            path: "src/lib.rs".to_owned(),
            line: 12,
            side: "RIGHT".to_owned(),
            body: "[unsafe-review] Guard evidence is missing.".to_owned(),
            suggestion: Some("let header = guarded_header_read(ptr)?;".to_owned()),
        };
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: String::new(),
            comments: vec![comment],
        };
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        let planned = build_planned_deliveries(&review, HEAD, Some(&graph))?;
        let body = "- [unsafe-review] Guard evidence is missing.\n\n```suggestion\nlet header = guarded_header_read(ptr)?;\n```\n- retain this finding";
        let filtered = body_after_confirmed_delivery(&review, body, &planned, &planned)?;
        ensure!(!filtered.contains("Guard evidence is missing"));
        ensure!(!filtered.contains("guarded_header_read"));
        ensure!(filtered.contains("retain this finding"));
        Ok(())
    }

    #[test]
    fn pull_review_comments_fetches_all_pages() -> Result<()> {
        let first_page = (0..100)
            .map(|id| serde_json::json!({"id": id}))
            .collect::<Vec<_>>();
        let mut transport = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::Value::Array(first_page),
                serde_json::json!([{"id": 100}]),
            ]),
            sends: VecDeque::new(),
        };
        let comments = fetch_pull_review_comments(
            &mut transport,
            "http://scripted",
            "owner/repo",
            42,
            "token",
        )?;
        ensure!(comments.as_array().is_some_and(|items| items.len() == 101));
        ensure!(transport.gets.is_empty());
        let mut malformed = ScriptedTransport {
            gets: VecDeque::from([serde_json::json!({"not": "an array"})]),
            sends: VecDeque::new(),
        };
        let error = fetch_pull_review_comments(
            &mut malformed,
            "http://scripted",
            "owner/repo",
            42,
            "token",
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("malformed comments response was accepted"))?;
        ensure!(format!("{error:#}").contains("must be an array"));
        Ok(())
    }

    #[test]
    fn unconfirmed_delivery_keeps_a_concise_body_fallback() -> Result<()> {
        let first = GitHubReviewComment {
            path: "src/lib.rs".to_owned(),
            line: 12,
            side: "RIGHT".to_owned(),
            body: "[tests] first body".to_owned(),
            suggestion: None,
        };
        let second = GitHubReviewComment {
            path: "src/lib.rs".to_owned(),
            line: 24,
            side: "RIGHT".to_owned(),
            body: "[tests] second body".to_owned(),
            suggestion: None,
        };
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: "review body".to_owned(),
            comments: vec![first, second],
        };
        let graph = serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": HEAD,
            "topics": [
                {"claim_id": "claim-1", "planned_action": "inline", "head_sha": HEAD, "path": "src/lib.rs", "anchor": 12},
                {"claim_id": "claim-2", "planned_action": "inline", "head_sha": HEAD, "path": "src/lib.rs", "anchor": 24}
            ]
        });
        let planned = build_planned_deliveries(&review, HEAD, Some(&graph))?;
        ensure!(planned.len() == 2);
        let body = "- [tests] first body\n- [tests] second body";
        let filtered = body_after_confirmed_delivery(&review, body, &planned[..1], &planned)?;
        ensure!(!filtered.contains("first body"));
        ensure!(filtered.contains("second body"));
        Ok(())
    }

    #[test]
    fn prior_inline_receipt_requires_exact_current_comment_and_head() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = delivery_args(temp.path(), "http://scripted");
        let review = review_with_comment("src/lib.rs", 12, "[tests] exact body");
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        let planned = build_planned_deliveries(&review, HEAD, Some(&graph))?;
        let identity = serde_json::to_value(&planned[0])?;
        let receipt = serde_json::json!({
            "schema": "ub-review.delivery_receipt.v1",
            "exact_head_sha": HEAD,
            "claim_id": identity["claim_id"],
            "action": "inline",
            "path": "src/lib.rs",
            "line": 12,
            "side": "RIGHT",
            "source_thread_id": null,
            "expected_body_digest": identity["expected_body_digest"],
            "review_id": "987",
            "comment_id": "456",
            "confirmed_head_sha": HEAD
        });
        fs::create_dir_all(&args.out)?;
        fs::write(
            args.out.join("delivery-reconciliation.json"),
            serde_json::to_vec(&serde_json::json!({"receipts": [receipt]}))?,
        )?;
        let current = serde_json::json!([{
            "id": 456,
            "path": "src/lib.rs",
            "line": 12,
            "side": "RIGHT",
            "commit_id": HEAD,
            "body": "[tests] exact body"
        }]);
        ensure!(
            prior_confirmed_deliveries(&args, &planned, Some(&current), HEAD)?.len() == 1,
            "exact current receipt was not reused"
        );
        let stale_current = serde_json::json!([{
            "id": 456,
            "path": "src/lib.rs",
            "line": 12,
            "side": "RIGHT",
            "commit_id": "old-head",
            "body": "[tests] exact body"
        }]);
        ensure!(
            prior_confirmed_deliveries(&args, &planned, Some(&stale_current), HEAD)?.is_empty(),
            "stale current comment was reused"
        );
        Ok(())
    }

    #[test]
    fn planned_reply_requires_a_current_source_thread() -> Result<()> {
        let review = review_with_comment("src/lib.rs", 12, "[tests] exact body");
        let mut graph = graph_for("src/lib.rs", 12, "reply", HEAD);
        if let Some(topic) = graph["topics"][0].as_object_mut() {
            topic.remove("planned_thread_id");
        }
        let error = build_planned_deliveries(&review, HEAD, Some(&graph))
            .err()
            .ok_or_else(|| anyhow::anyhow!("reply without source thread was accepted"))?;
        ensure!(
            format!("{error:#}").contains("no current source thread"),
            "missing source-thread diagnostic lost: {error:#}"
        );
        Ok(())
    }

    #[test]
    fn retry_decisions_record_exact_identity_and_unconfirmed_status() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = delivery_args(temp.path(), "http://scripted");
        let review = review_with_comment("src/lib.rs", 12, "[tests] exact body");
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        let planned = build_planned_deliveries(&review, HEAD, Some(&graph))?;
        write_retry_decisions(&args, &planned, &[])?;
        let decisions: serde_json::Value =
            serde_json::from_slice(&fs::read(args.out.join("delivery-retry-decisions.json"))?)?;
        ensure!(decisions.as_array().is_some_and(|items| items.len() == 1));
        ensure!(decisions[0]["status"] == "unconfirmed");
        ensure!(decisions[0]["identity"]["claim_id"] == "claim-1");
        ensure!(decisions[0]["identity"]["exact_head_sha"] == HEAD);
        ensure!(decisions[0]["identity"]["action"] == "inline");
        Ok(())
    }

    #[test]
    fn planned_inline_delivery_keeps_empty_review_empty() -> Result<()> {
        let review = GitHubReview {
            event: "COMMENT".to_owned(),
            body: String::new(),
            comments: Vec::new(),
        };
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        let planned = build_planned_inline_deliveries(&review, HEAD, Some(&graph))?;
        ensure!(planned.is_empty(), "empty review planned a delivery");
        ensure!(read_expected_delivery_head(&review, None)?.is_none());
        Ok(())
    }

    #[test]
    fn expected_delivery_head_requires_current_graph_for_comments() -> Result<()> {
        let review = review_with_comment("src/lib.rs", 12, "[tests] exact body");
        ensure!(
            read_expected_delivery_head(&review, None).is_err(),
            "commented review accepted without graph"
        );
        let graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        ensure!(
            read_expected_delivery_head(&review, Some(&graph))?.as_deref() == Some(HEAD),
            "graph head was not selected"
        );
        let mut missing_head = graph.clone();
        if let Some(object) = missing_head.as_object_mut() {
            object.remove("head_sha");
        }
        ensure!(
            read_expected_delivery_head(&review, Some(&missing_head)).is_err(),
            "graph without head was accepted"
        );
        Ok(())
    }

    #[test]
    fn head_revalidation_is_exact_and_classified() -> Result<()> {
        ensure_heads_match("before create", HEAD, HEAD)?;
        let error = match ensure_heads_match("before submit", HEAD, "another-head") {
            Ok(()) => return Err(anyhow::anyhow!("changed head was accepted")),
            Err(error) => error,
        };
        ensure!(
            error
                .chain()
                .any(|cause| cause.downcast_ref::<HeadRevalidationFailure>().is_some()),
            "head error lost its typed discriminator"
        );
        let transaction = DeliveryTransaction::new(HEAD, Vec::new())?;
        ensure!(
            failure_stage(&error, &transaction) == DeliveryFailureStage::HeadRevalidation,
            "head error was classified as another failure stage"
        );
        Ok(())
    }

    #[test]
    fn response_parsing_and_identifiers_fail_closed() -> Result<()> {
        ensure!(json_identifier(&serde_json::json!({"id": 7}), "id", "review")? == "7");
        ensure!(
            json_identifier(&serde_json::json!({"id": "review-7"}), "id", "review")? == "review-7"
        );
        for value in [serde_json::json!({}), serde_json::json!({"id": ""})] {
            ensure!(
                json_identifier(&value, "id", "review").is_err(),
                "invalid identifier was accepted"
            );
        }
        let success = HttpPostOutput {
            status: command_status(true)?,
            stdout: br#"{"id":7}"#.to_vec(),
            stderr: Vec::new(),
            http_status: Some(200),
        };
        ensure!(parse_success_json(&success, "operation")?["id"] == 7);
        let malformed = HttpPostOutput {
            stdout: b"not-json".to_vec(),
            ..success
        };
        ensure!(parse_success_json(&malformed, "operation").is_err());
        let failed = HttpPostOutput {
            status: command_status(false)?,
            stdout: Vec::new(),
            stderr: b"rejected".to_vec(),
            http_status: Some(422),
        };
        ensure!(parse_success_json(&failed, "operation").is_err());
        Ok(())
    }

    #[test]
    fn failure_stage_tracks_each_delivery_lifecycle_boundary() -> Result<()> {
        let error = anyhow::anyhow!("transport failed");
        let expected = BTreeMap::from([
            (
                DeliveryTransactionState::Planned,
                DeliveryFailureStage::PendingReviewCreation,
            ),
            (
                DeliveryTransactionState::PendingReviewCreated,
                DeliveryFailureStage::CommentCreation,
            ),
            (
                DeliveryTransactionState::CommentsCreated,
                DeliveryFailureStage::CommentReconciliation,
            ),
            (
                DeliveryTransactionState::CommentsReconciled,
                DeliveryFailureStage::ReceiptPersistence,
            ),
            (
                DeliveryTransactionState::HeadRevalidated,
                DeliveryFailureStage::Submission,
            ),
            (
                DeliveryTransactionState::Submitted,
                DeliveryFailureStage::ReceiptPersistence,
            ),
            (
                DeliveryTransactionState::ReceiptsPersisted,
                DeliveryFailureStage::ReceiptPersistence,
            ),
        ]);
        for (state, stage) in expected {
            let mut transaction = DeliveryTransaction::new(HEAD, Vec::new())?;
            if state != DeliveryTransactionState::Planned {
                for next in [
                    DeliveryTransactionState::PendingReviewCreated,
                    DeliveryTransactionState::CommentsCreated,
                    DeliveryTransactionState::CommentsReconciled,
                    DeliveryTransactionState::HeadRevalidated,
                    DeliveryTransactionState::Submitted,
                    DeliveryTransactionState::ReceiptsPersisted,
                ] {
                    let reached = next == state;
                    transaction.transition(next)?;
                    if reached {
                        break;
                    }
                }
            }
            let actual = failure_stage(&error, &transaction);
            ensure!(
                actual == stage,
                "wrong failure stage for {state:?} (actual {:?}): {actual:?} != {stage:?}",
                transaction.state()
            );
        }
        Ok(())
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
        let mut missing_head = returned("[tests] exact body");
        if let Some(comment) = missing_head
            .as_array_mut()
            .and_then(|items| items.first_mut())
        {
            comment
                .as_object_mut()
                .ok_or_else(|| anyhow::anyhow!("test comment is not an object"))?
                .remove("commit_id");
        }
        ensure!(
            observed_deliveries(&missing_head, std::slice::from_ref(&planned), HEAD).is_err(),
            "missing returned head was accepted"
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
