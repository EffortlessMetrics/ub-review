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

    let mut claim_graph = load_claim_graph(args, !review.comments.is_empty())?;
    let current_head = fetch_pull_head(transport, api, repo, pull_number, token)?;
    let merge_head =
        bind_merge_delivery(args, review, claim_graph.as_mut(), &current_head, transport)?;
    let expected_head = match merge_head {
        Some(head) => {
            let live_head = fetch_pull_head(transport, api, repo, pull_number, token)?;
            ensure_heads_match("after merge delivery binding", &head, &live_head)?;
            Some(head)
        }
        None => read_expected_delivery_head(review, claim_graph.as_ref())?,
    };
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
        if !remaining_replies.is_empty() {
            reserve_delivery_attempt(args, api, repo, pull_number, &expected_head, api_payload)?;
        }
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

    reserve_delivery_attempt(args, api, repo, pull_number, &expected_head, api_payload)?;
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

// Reserve before the first remote write, retaining even a partial reservation
// after failure. An absent response cannot prove the server did not publish.
// Exact receipted reuse happens before this boundary; another write requires
// explicit reconciliation of the previous attempt, never an automatic retry.
fn reserve_delivery_attempt(
    args: &PostArgs,
    api: &str,
    repo: &str,
    pull_number: u64,
    exact_head_sha: &str,
    payload: &GitHubReviewPostPayload,
) -> Result<()> {
    use std::io::Write;

    let reservation = serde_json::to_vec_pretty(&serde_json::json!({
        "schema": "ub-review.delivery_attempt.v1",
        "api": api,
        "repo": repo,
        "pull_number": pull_number,
        "exact_head_sha": exact_head_sha,
        "payload_digest": sha256_hex(&serde_json::to_vec(payload)?),
    }))?;
    fs::create_dir_all(&args.out)?;
    let path = args.out.join("delivery-attempt.json");
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| {
            format!(
                "reserve {}; reconcile the previous delivery attempt before another GitHub write",
                path.display()
            )
        })?;
    file.write_all(&reservation)?;
    file.sync_all()?;
    // Unix permits syncing the containing directory as well as the new file.
    // Other platforms retain process-retry safety without a crash guarantee.
    #[cfg(unix)]
    fs::File::open(&args.out)?.sync_all()?;
    Ok(())
}

fn load_claim_graph(args: &PostArgs, required: bool) -> Result<Option<serde_json::Value>> {
    let graph_path = args
        .review_json
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("claim_graph.json");
    if !required {
        match fs::symlink_metadata(&graph_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            metadata => {
                metadata.with_context(|| format!("inspect {}", graph_path.display()))?;
            }
        }
    }
    let bytes = fs::read(&graph_path).with_context(|| format!("read {}", graph_path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("parse {}", graph_path.display()))
        .map(Some)
}

// Bind a saved merge packet without relabeling its analysis identity. This
// projection is private to delivery; the source admission and graph stay intact.
fn bind_merge_delivery(
    args: &PostArgs,
    review: &GitHubReview,
    graph: Option<&mut serde_json::Value>,
    current_head: &str,
    transport: &mut impl DeliveryTransport,
) -> Result<Option<String>> {
    let Some(graph) = graph else { return Ok(None) };
    if graph
        .pointer("/revision/semantics")
        .and_then(serde_json::Value::as_str)
        != Some("merge_result")
    {
        return Ok(None);
    }
    let revision: RevisionRef = serde_json::from_value(graph["revision"].clone())?;
    revision.validate()?;
    let packet = args
        .review_json
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| anyhow::anyhow!("merge delivery requires a saved review packet"))?;
    let admission: crate::revision_admission::RevisionAdmission = serde_json::from_slice(
        &fs::read(packet.join("input/revision-admission.json"))
            .context("read merge delivery admission")?,
    )?;
    admission.validate()?;
    let identity =
        crate::revision_identity::RevisionIdentity::from_canonical(&admission.identity_canonical)?;
    let (base, head, reviewed) = identity.merge_delivery_sides()?;
    ensure!(
        revision.digest == identity.identity_digest()
            && revision.reviewed_commit == reviewed.commit_oid()
            && admission.semantics == "merge_result"
            && admission.reviewed_commit_oid == reviewed.commit_oid()
            && admission.pr_head_commit.as_deref() == Some(head.commit_oid()),
        "merge delivery admission does not match the reviewed revision"
    );
    ensure_heads_match(
        "before merge delivery binding",
        head.commit_oid(),
        current_head,
    )?;
    let api = args.github_api_url.trim_end_matches('/');
    let repo = args
        .repo
        .as_deref()
        .context("merge delivery repository missing")?;
    let token = args
        .github_token
        .as_deref()
        .context("merge delivery token missing")?;
    let mut objects = Vec::new();
    for side in [reviewed, head, base] {
        let object = transport.fetch_json(
            &format!("{api}/repos/{repo}/git/commits/{}", side.commit_oid()),
            token,
        )?;
        ensure!(
            object["sha"].as_str() == Some(side.commit_oid())
                && object
                    .pointer("/tree/sha")
                    .and_then(serde_json::Value::as_str)
                    == Some(side.tree_oid()),
            "merge delivery commit/tree mismatch"
        );
        objects.push(object);
    }
    let merge_object = &objects[0];
    let parents = merge_object["parents"]
        .as_array()
        .context("merge delivery parents missing")?;
    ensure!(
        parents.len() == 2
            && parents[0]["sha"].as_str() == Some(base.commit_oid())
            && parents[1]["sha"].as_str() == Some(head.commit_oid()),
        "merge delivery parent relationship mismatch"
    );
    let mut paths = std::collections::BTreeSet::new();
    for comment in &review.comments {
        ensure!(
            comment.side == "RIGHT",
            "merge delivery requires a post-image anchor"
        );
        paths.insert(comment.path.as_str());
    }
    let mut anchors = Vec::new();
    for path in paths {
        // Whole-file blob equality is conservative: if integration changed
        // this file, no line-number equivalence is inferred.
        let encoded = path
            .split('/')
            .map(percent_encode_query)
            .collect::<Vec<_>>()
            .join("/");
        let mut blobs = Vec::new();
        for side in [reviewed, head] {
            let file = transport.fetch_json(
                &format!(
                    "{api}/repos/{repo}/contents/{encoded}?ref={}",
                    side.commit_oid()
                ),
                token,
            )?;
            ensure!(
                file["type"] == "file" && file["path"].as_str() == Some(path),
                "merge delivery anchor is not the requested file"
            );
            let sha = file["sha"]
                .as_str()
                .context("merge delivery file blob missing")?;
            crate::revision_identity::CommitTree::new("anchor blob", sha, sha)?;
            blobs.push(sha.to_owned());
        }
        ensure!(
            blobs[0] == blobs[1],
            "merge delivery file differs from publication head: {path}"
        );
        anchors.push(serde_json::json!({"path": path, "blob": blobs[0]}));
    }
    let source_label = graph["head_sha"]
        .as_str()
        .context("merge claim graph subject missing")?
        .to_owned();
    ensure!(
        source_label == "HEAD"
            || source_label == reviewed.commit_oid()
            || source_label == head.commit_oid(),
        "merge claim graph subject belongs to another head"
    );
    for topic in graph["topics"]
        .as_array_mut()
        .context("merge claim graph topics missing")?
    {
        let subject = topic["head_sha"]
            .as_str()
            .context("merge topic subject missing")?;
        ensure!(
            subject == source_label
                || subject == reviewed.commit_oid()
                || subject == head.commit_oid(),
            "merge topic belongs to another subject"
        );
        topic["head_sha"] = serde_json::json!(head.commit_oid());
    }
    write_json(
        &args.out.join("delivery-revision-binding.json"),
        &serde_json::json!({
            "schema": "ub-review.delivery_revision_binding.v1",
            "publication_head": head.commit_oid(),
            "admission": admission,
            "merge_object": merge_object,
            "head_object": objects[1],
            "base_object": objects[2],
            "anchors": anchors,
        }),
    )?;
    Ok(Some(head.commit_oid().to_owned()))
}

fn read_expected_delivery_head(
    review: &GitHubReview,
    claim_graph: Option<&serde_json::Value>,
) -> Result<Option<String>> {
    if !review.comments.is_empty() || claim_graph.is_some() {
        let graph = claim_graph.ok_or_else(|| anyhow::anyhow!("claim graph is required"))?;
        // A1.3 (#950): the immutable revision reference is authoritative.
        // Candidate-head runs review their publication head. Merge-result
        // packets must instead pass the separate admission/object binding.
        if let Some(revision) = graph.get("revision") {
            ensure!(
                revision["semantics"] != "merge_result",
                "merge delivery requires admission binding"
            );
            let commit = revision
                .get("reviewed_commit")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!("claim graph revision reference is missing reviewed_commit")
                })?;
            return Ok(Some(commit.to_owned()));
        }
        // Legacy fallback: head_sha may carry a symbolic label (`HEAD`),
        // which is weak authority kept only for pre-A1.3 packets. It must
        // never be mistaken for immutable identity (A1.4 enforces).
        let head = graph
            .get("head_sha")
            .and_then(serde_json::Value::as_str)
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("claim graph has no exact head SHA"))?;
        return Ok(Some(head.to_owned()));
    }
    // Graphless manual/legacy body-only reviews retain their live-head binding.
    // A present prepared graph binds summary-only and inline reviews alike.
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
    let has_surface_dispositions = topics.iter().any(|topic| topic.get("delivery").is_some());
    review
        .comments
        .iter()
        .map(|comment| {
            let path = normalize_repo_path(&comment.path);
            let matches = topics
                .iter()
                .filter(|topic| {
                    // Retained private observations can share a public
                    // comment's location and provisional action. Only the
                    // compiler-selected surface authorizes delivery. Legacy
                    // packets wholly without this field retain the exact,
                    // unique location check. A missing field in a mixed packet
                    // or an explicit unknown value is not public authority.
                    let selected_surface = match topic.get("delivery") {
                        None => !has_surface_dispositions,
                        Some(value) => value.as_str() == Some("inline-candidate"),
                    };
                    selected_surface
                        && matches!(
                            topic
                                .get("planned_action")
                                .and_then(serde_json::Value::as_str),
                            Some("inline") | Some("reply")
                        )
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
    let reserved_attempt = args.out.join("delivery-attempt.json").exists();
    let transaction_path = args.out.join("delivery-transaction.json");
    let inline_submission_confirmed = if reserved_attempt {
        if transaction_path.exists() {
            let value: serde_json::Value = serde_json::from_slice(&fs::read(&transaction_path)?)?;
            let transaction: DeliveryTransaction = serde_json::from_value(value.clone())?;
            transaction.state() == &DeliveryTransactionState::ReceiptsPersisted
                && value["exact_head_sha"] == exact_head_sha
        } else {
            false
        }
    } else {
        true
    };
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
        if current.is_some()
            && ((receipt_match
                && (item.action() == DeliveryAction::Reply || inline_submission_confirmed))
                || (item.action() == DeliveryAction::Reply && !reserved_attempt))
        {
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
pub(crate) use tests::{FakeHttpResponse, lock_fake_delivery_tests, spawn_fake_delivery_api};

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{bail, ensure};
    use std::collections::BTreeMap;
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Mutex, MutexGuard};
    use std::thread;
    use std::time::{Duration, Instant};

    static FAKE_DELIVERY_TEST_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn lock_fake_delivery_tests() -> Result<MutexGuard<'static, ()>> {
        FAKE_DELIVERY_TEST_LOCK
            .lock()
            .map_err(|_| anyhow::anyhow!("fake delivery test lock was poisoned"))
    }

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    #[path = "main_http_status_tests.rs"]
    mod main_http_status_tests;

    fn planned() -> Result<PlannedDelivery> {
        let body = "exact body";
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
            let mut last_error: Option<String> = None;
            for (response_index, response) in responses.iter().enumerate() {
                let deadline = Instant::now() + crate::tests::FAKE_HTTP_IDLE_DEADLINE;
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            let result = (|| -> Result<()> {
                                // The listener is non-blocking so the accept
                                // loop can time out, and on Windows the
                                // accepted socket inherits that mode. Reading a
                                // request whose body has not arrived yet then
                                // fails with WouldBlock and aborts the client's
                                // connection, which showed up as a flaky
                                // `curl: (56) Recv failure`. Read timeouts
                                // bound this socket instead.
                                stream.set_nonblocking(false)?;
                                stream.set_read_timeout(Some(
                                    crate::tests::FAKE_HTTP_STREAM_TIMEOUT,
                                ))?;
                                stream.set_write_timeout(Some(
                                    crate::tests::FAKE_HTTP_STREAM_TIMEOUT,
                                ))?;
                                let request = read_fake_request(stream.try_clone()?)?;
                                write_fake_response(stream, response)?;
                                requests.push(request);
                                Ok(())
                            })();
                            if let Err(error) = result {
                                last_error = Some(error.to_string());
                                continue;
                            }
                            break;
                        }
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
                                "fake delivery API timed out waiting for request {}; captured {}: {captured:?}; last error: {:?}",
                                response_index + 1,
                                requests.len(),
                                last_error
                            );
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
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
                    r#"[{{"id":123,"path":"src/lib.rs","line":12,"side":"RIGHT","commit_id":"{HEAD}","body":"exact body"}}]"#
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
    fn merge_result_inline_delivery_preserves_analysis_and_rejects_stale_subjects() -> Result<()> {
        use crate::revision_identity::{CommitTree, ReviewSemantics, RevisionIdentity};
        struct RecordingTransport {
            inner: ScriptedTransport,
            reads: Vec<String>,
            writes: Vec<serde_json::Value>,
            requests: Vec<(String, String, Vec<String>)>,
            fail_read: Option<usize>,
        }
        impl DeliveryTransport for RecordingTransport {
            fn fetch_json(&mut self, url: &str, token: &str) -> Result<serde_json::Value> {
                self.reads.push(url.to_owned());
                if self.fail_read == Some(self.reads.len()) {
                    bail!("scripted transport failure");
                }
                self.inner.fetch_json(url, token)
            }
            fn send_json(
                &mut self,
                method: &str,
                url: &str,
                token: &str,
                path: &Path,
                headers: &[&str],
            ) -> Result<HttpPostOutput> {
                self.requests.push((
                    method.to_owned(),
                    url.to_owned(),
                    headers.iter().map(|h| (*h).to_owned()).collect(),
                ));
                self.writes.push(serde_json::from_slice(&fs::read(path)?)?);
                self.inner.send_json(method, url, token, path, headers)
            }
        }
        let base = "a".repeat(40);
        let merge = "b".repeat(40);
        let base_tree = "c".repeat(40);
        let head_tree = "d".repeat(40);
        let merge_tree = "e".repeat(40);
        let blob = "f".repeat(40);
        let identity = RevisionIdentity::new(
            CommitTree::new("base", &base, &base_tree)?,
            CommitTree::new("head", HEAD, &head_tree)?,
            CommitTree::new("reviewed", &merge, &merge_tree)?,
            Some(CommitTree::new("merge", &merge, &merge_tree)?),
            ReviewSemantics::MergeResult,
            &RevisionIdentity::changed_paths_digest(["src/lib.rs"]),
            &RevisionIdentity::diff_digest(b"fixture patch"),
        )?;
        let admission = crate::revision_admission::RevisionAdmission {
            schema: crate::revision_admission::REVISION_ADMISSION_SCHEMA.to_owned(),
            identity_canonical: identity.canonical_form(),
            identity_digest: identity.identity_digest(),
            semantics: "merge_result".to_owned(),
            reviewed_commit_oid: merge.clone(),
            pr_head_commit: Some(HEAD.to_owned()),
            worktree_dirty: false,
        };
        for case in [
            "valid",
            "valid_summary",
            "summary_stale",
            "summary_late_stale",
            "valid_merge_label",
            "valid_head_label",
            "unrelated_graph_admitted_topic",
            "unrelated_graph_and_topic",
            "stale",
            "late_stale",
            "wrong_parent",
            "wrong_tree",
            "wrong_blob",
            "wrong_digest",
            "wrong_metadata",
            "wrong_topic",
            "missing_admission",
            "legacy_stale",
            "commit_transport_failure",
            "file_transport_failure",
            "binding_write_failure",
        ] {
            let summary_only = case.contains("summary");
            let temp = tempfile::tempdir()?;
            let review_dir = temp.path().join("review");
            fs::create_dir_all(&review_dir)?;
            fs::create_dir_all(temp.path().join("input"))?;
            let mut graph = graph_for("src/lib.rs", 12, "inline", "HEAD");
            graph["revision"] = serde_json::to_value(RevisionRef::from_admission(&admission))?;
            match case {
                "valid_merge_label" => {
                    graph["head_sha"] = serde_json::json!(merge);
                    graph["topics"][0]["head_sha"] = serde_json::json!(merge);
                }
                "valid_head_label" => {
                    graph["head_sha"] = serde_json::json!(HEAD);
                    graph["topics"][0]["head_sha"] = serde_json::json!(HEAD);
                }
                "unrelated_graph_and_topic" => {
                    graph["head_sha"] = serde_json::json!(base);
                    graph["topics"][0]["head_sha"] = serde_json::json!(base);
                }
                "unrelated_graph_admitted_topic" => {
                    graph["head_sha"] = serde_json::json!(base);
                    graph["topics"][0]["head_sha"] = serde_json::json!(HEAD);
                }
                _ => {}
            }

            if case == "wrong_digest" {
                graph["revision"]["digest"] = serde_json::json!("0".repeat(64));
            }
            if case == "wrong_topic" {
                graph["topics"][0]["head_sha"] = serde_json::json!(base);
            }
            if case == "legacy_stale" {
                graph
                    .as_object_mut()
                    .context("graph object")?
                    .remove("revision");
                graph["head_sha"] = serde_json::json!(base);
                graph["topics"][0]["head_sha"] = serde_json::json!(base);
            }
            let graph_bytes = serde_json::to_vec(&graph)?;
            fs::write(review_dir.join("claim_graph.json"), &graph_bytes)?;
            let mut saved_admission = admission.clone();
            if case == "wrong_metadata" {
                saved_admission.pr_head_commit = Some(merge.clone());
            }
            let admission_bytes = serde_json::to_vec(&saved_admission)?;
            fs::write(
                temp.path().join("input/revision-admission.json"),
                &admission_bytes,
            )?;
            if case == "missing_admission" {
                fs::rename(
                    temp.path().join("input/revision-admission.json"),
                    temp.path().join("input/retained.json"),
                )?;
            }
            let mut scripted = ScriptedTransport {
                gets: VecDeque::from([
                    serde_json::json!({"head": {"sha": if matches!(case, "stale" | "summary_stale") { &merge } else { HEAD }}}),
                    serde_json::json!({"sha": merge, "tree": {"sha": if case == "wrong_tree" { &base_tree } else { &merge_tree }},
                        "parents": [{"sha": base}, {"sha": if case == "wrong_parent" { &base } else { HEAD }}]}),
                    serde_json::json!({"sha": HEAD, "tree": {"sha": head_tree}}),
                    serde_json::json!({"sha": base, "tree": {"sha": base_tree}}),
                    serde_json::json!({"sha": blob, "type": "file", "path": "src/lib.rs"}),
                    serde_json::json!({"sha": if case == "wrong_blob" { &base } else { &blob }, "type": "file", "path": "src/lib.rs"}),
                    serde_json::json!({"head": {"sha": if matches!(case, "late_stale" | "summary_late_stale") { &merge } else { HEAD }}}),
                    serde_json::json!([{"id":987,"path":"src/lib.rs","line":12,"side":"RIGHT","commit_id":HEAD,"body":"exact body"}]),
                    serde_json::json!({"head": {"sha": HEAD}}),
                ]),
                sends: VecDeque::from([
                    scripted_output(r#"{"id":987}"#, true)?,
                    scripted_output(r#"{"id":987,"state":"commented"}"#, true)?,
                ]),
            };
            if summary_only {
                // Summary-only merge binding verifies commit identity without
                // querying inline anchor blobs. Response reconciliation still runs.
                scripted.gets[7] = serde_json::json!([]);
                scripted.gets.remove(5);
                scripted.gets.remove(4);
            }
            let mut transport = RecordingTransport {
                inner: scripted,
                reads: Vec::new(),
                writes: Vec::new(),
                requests: Vec::new(),
                fail_read: match case {
                    "commit_transport_failure" => Some(2),
                    "file_transport_failure" => Some(5),
                    _ => None,
                },
            };
            let (mut review, mut payload) = delivery_review();
            if summary_only {
                review.comments.clear();
                payload.comments.clear();
            }
            let args = delivery_args(&review_dir, "http://scripted");
            if case == "binding_write_failure" {
                fs::create_dir_all(args.out.join("delivery-revision-binding.json"))?;
            }
            let result = execute_pending_review_delivery_with_transport(
                &args,
                &review,
                &payload,
                &mut transport,
            );
            if case.starts_with("valid") {
                let outcome = result.with_context(|| format!("case {case}"))?;
                ensure!(outcome.response["state"] == "commented");
                ensure!(transport.writes.len() == 2);
                let headers = vec![
                    "Accept: application/vnd.github+json".to_owned(),
                    "Content-Type: application/json".to_owned(),
                    "X-GitHub-Api-Version: 2022-11-28".to_owned(),
                ];
                ensure!(
                    transport.requests
                        == vec![
                            (
                                "POST".to_owned(),
                                "http://scripted/repos/owner/repo/pulls/42/reviews".to_owned(),
                                headers.clone()
                            ),
                            (
                                "POST".to_owned(),
                                "http://scripted/repos/owner/repo/pulls/42/reviews/987/events"
                                    .to_owned(),
                                headers
                            ),
                        ]
                );
                ensure!(transport.writes[0]["commit_id"] == HEAD);
                if summary_only {
                    ensure!(
                        transport.writes[0]["comments"]
                            .as_array()
                            .is_none_or(Vec::is_empty)
                    );
                } else {
                    ensure!(transport.writes[0]["comments"][0]["line"] == 12);
                }
                ensure!(transport.writes[1]["event"] == "COMMENT");
                ensure!(
                    transport.reads
                        == vec![
                            "http://scripted/repos/owner/repo/pulls/42".to_owned(),
                            format!("http://scripted/repos/owner/repo/git/commits/{merge}"),
                            format!("http://scripted/repos/owner/repo/git/commits/{HEAD}"),
                            format!("http://scripted/repos/owner/repo/git/commits/{base}"),
                            format!(
                                "http://scripted/repos/owner/repo/contents/src/lib.rs?ref={merge}"
                            ),
                            format!(
                                "http://scripted/repos/owner/repo/contents/src/lib.rs?ref={HEAD}"
                            ),
                            "http://scripted/repos/owner/repo/pulls/42".to_owned(),
                            "http://scripted/repos/owner/repo/pulls/42/reviews/987/comments"
                                .to_owned(),
                            "http://scripted/repos/owner/repo/pulls/42".to_owned(),
                        ]
                        .into_iter()
                        .filter(|url| !summary_only || !url.contains("/contents/"))
                        .collect::<Vec<_>>(),
                    "unexpected GETs: {:?}",
                    transport.reads
                );
                ensure!(transport.inner.gets.is_empty() && transport.inner.sends.is_empty());
                let transaction: DeliveryTransaction =
                    serde_json::from_slice(&fs::read(args.out.join("delivery-transaction.json"))?)?;
                ensure!(transaction.state() == &DeliveryTransactionState::ReceiptsPersisted);
                let binding: serde_json::Value = serde_json::from_slice(&fs::read(
                    args.out.join("delivery-revision-binding.json"),
                )?)?;
                ensure!(binding["publication_head"] == HEAD);
                ensure!(binding["admission"]["reviewed_commit_oid"] == merge);
                ensure!(binding["admission"]["identity_canonical"] == identity.canonical_form());
                ensure!(binding["merge_object"]["parents"][0]["sha"] == base);
                ensure!(binding["merge_object"]["parents"][1]["sha"] == HEAD);
            } else {
                let failure = match result {
                    Err(error) => error,
                    Ok(_) => bail!("{case} accepted"),
                };
                let error = format!("{failure:#}");
                match case {
                    "stale" | "legacy_stale" | "summary_stale" => {
                        ensure!(
                            error.contains("GitHub pull head changed"),
                            "{case}: {error}"
                        );
                        ensure!(transport.reads.len() == 1, "{case} continued reads");
                    }
                    "commit_transport_failure" | "file_transport_failure" => {
                        ensure!(
                            error.contains("scripted transport failure"),
                            "{case}: {error}"
                        );
                        ensure!(Some(transport.reads.len()) == transport.fail_read);
                    }
                    "binding_write_failure" => {
                        ensure!(
                            failure.downcast_ref::<std::io::Error>().is_some(),
                            "{error}"
                        )
                    }
                    _ => {}
                }
                if case.starts_with("unrelated_graph") {
                    ensure!(
                        !args.out.join("delivery-revision-binding.json").exists(),
                        "{case} wrote binding"
                    );
                    ensure!(
                        !args
                            .out
                            .join("delivery-pending-review-payload.json")
                            .exists(),
                        "{case} prepared pending review"
                    );
                }

                ensure!(
                    transport.inner.sends.len() == 2 && transport.writes.is_empty(),
                    "{case} performed a write"
                );
                ensure!(
                    !args.out.join("delivery-transaction.json").exists(),
                    "{case} began transaction"
                );
            }
            ensure!(fs::read(review_dir.join("claim_graph.json"))? == graph_bytes);
            let saved = if case == "missing_admission" {
                "input/retained.json"
            } else {
                "input/revision-admission.json"
            };
            ensure!(fs::read(temp.path().join(saved))? == admission_bytes);
        }
        Ok(())
    }

    #[test]
    fn load_claim_graph_preserves_value_and_errors() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = delivery_args(temp.path(), "http://scripted");
        let graph_path = temp.path().join("claim_graph.json");
        ensure!(load_claim_graph(&args, false)?.is_none());
        let missing = load_claim_graph(&args, true)
            .err()
            .ok_or_else(|| anyhow::anyhow!("required absent graph was accepted"))?;
        ensure!(missing.to_string() == format!("read {}", graph_path.display()));
        ensure!(
            missing
                .downcast_ref::<std::io::Error>()
                .map(std::io::Error::kind)
                == Some(std::io::ErrorKind::NotFound)
        );

        let expected = serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": "HEAD",
            "revision": {"reviewed_commit": HEAD, "digest": "e".repeat(64), "semantics": "candidate_head"},
            "topics": [{"claim_id": "keep-complete-topic", "path": "src/lib.rs", "anchor": 12}],
            "metadata": {"preserve": [1, "two", null]}
        });
        fs::write(&graph_path, serde_json::to_vec(&expected)?)?;
        for required in [false, true] {
            ensure!(
                load_claim_graph(&args, required)? == Some(expected.clone()),
                "loader changed the full graph for required={required}"
            );
        }

        fs::write(&graph_path, b"{")?;
        for required in [false, true] {
            let malformed = load_claim_graph(&args, required).err().ok_or_else(|| {
                anyhow::anyhow!("malformed graph was accepted for required={required}")
            })?;
            ensure!(malformed.to_string() == format!("parse {}", graph_path.display()));
            ensure!(malformed.downcast_ref::<serde_json::Error>().is_some());
        }

        fs::remove_file(&graph_path)?;
        fs::create_dir(&graph_path)?;
        for required in [false, true] {
            let unreadable = load_claim_graph(&args, required).err().ok_or_else(|| {
                anyhow::anyhow!("unreadable graph was accepted for required={required}")
            })?;
            ensure!(unreadable.to_string() == format!("read {}", graph_path.display()));
            ensure!(unreadable.downcast_ref::<std::io::Error>().is_some());
        }
        #[cfg(unix)]
        {
            let blocked_parent = temp.path().join("not-a-directory");
            fs::write(&blocked_parent, b"regular file")?;
            let mut blocked_args = delivery_args(temp.path(), "http://scripted");
            blocked_args.review_json = blocked_parent.join("review.json");
            let blocked_graph = blocked_parent.join("claim_graph.json");
            for (required, operation) in [(false, "inspect"), (true, "read")] {
                let error = load_claim_graph(&blocked_args, required)
                    .err()
                    .ok_or_else(|| anyhow::anyhow!("non-directory graph parent was accepted"))?;
                ensure!(
                    error.to_string() == format!("{operation} {}", blocked_graph.display()),
                    "wrong graph lookup error for required={required}: {error:#}"
                );
                ensure!(
                    error
                        .downcast_ref::<std::io::Error>()
                        .map(std::io::Error::kind)
                        == Some(std::io::ErrorKind::NotADirectory)
                );
            }
        }
        Ok(())
    }

    #[test]
    fn summary_only_delivery_requires_present_graph_binding() -> Result<()> {
        const OLD_HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let bound_graph = |commit: &str| {
            serde_json::json!({
                "schema": "ub-review.claim_graph.v1", "head_sha": "HEAD",
                "revision": {"digest": "e".repeat(64), "semantics": "candidate_head", "reviewed_commit": commit},
                "topics": []
            })
        };
        for (case, graph, rejection) in [
            (
                "current immutable",
                Some(serde_json::to_vec(&bound_graph(HEAD))?),
                None,
            ),
            (
                "current legacy",
                Some(serde_json::to_vec(
                    &serde_json::json!({"head_sha": HEAD, "topics": []}),
                )?),
                None,
            ),
            ("manual without graph", None, None),
            (
                "stale immutable",
                Some(serde_json::to_vec(&bound_graph(OLD_HEAD))?),
                Some("head changed before pending-review creation"),
            ),
            (
                "stale legacy",
                Some(serde_json::to_vec(
                    &serde_json::json!({"head_sha": OLD_HEAD, "topics": []}),
                )?),
                Some("head changed before pending-review creation"),
            ),
            (
                "missing binding",
                Some(b"{}".to_vec()),
                Some("claim graph has no exact head SHA"),
            ),
            (
                "missing reviewed commit",
                Some(serde_json::to_vec(
                    &serde_json::json!({"head_sha": HEAD, "revision": {"digest": "e".repeat(64), "semantics": "candidate_head"}}),
                )?),
                Some("revision reference is missing reviewed_commit"),
            ),
            ("malformed graph", Some(b"{".to_vec()), Some("parse")),
            ("unreadable graph", None, Some("read")),
            #[cfg(unix)]
            ("dangling graph", None, Some("read")),
            #[cfg(unix)]
            (
                "linked current graph",
                Some(serde_json::to_vec(&bound_graph(HEAD))?),
                None,
            ),
        ] {
            let temp = tempfile::tempdir()?;
            let args = delivery_args(temp.path(), "http://scripted");
            let review = GitHubReview {
                event: "COMMENT".to_owned(),
                body: "## Findings\n\n- `src/lib.rs:12`: The allocation can be reused before its last reader finishes.".to_owned(),
                comments: Vec::new(),
            };
            validate_github_review_payload_for_post(&args, &review)?;
            let graph_path = temp.path().join("claim_graph.json");
            if let Some(graph) = graph {
                fs::write(&graph_path, graph)?;
            } else if case == "unreadable graph" {
                fs::create_dir(&graph_path)?;
            }
            #[cfg(unix)]
            if case == "dangling graph" {
                std::os::unix::fs::symlink(temp.path().join("missing-graph.json"), &graph_path)?;
            } else if case == "linked current graph" {
                let target = temp.path().join("linked-graph.json");
                fs::rename(&graph_path, &target)?;
                std::os::unix::fs::symlink(target, &graph_path)?;
            }
            let payload = github_review_post_payload(&review)?;
            let mut transport = ScriptedTransport {
                gets: VecDeque::from([
                    serde_json::json!({"head": {"sha": HEAD}}),
                    serde_json::json!([]),
                    serde_json::json!({"head": {"sha": HEAD}}),
                ]),
                sends: VecDeque::from([
                    scripted_output(r#"{"id":987}"#, true)?,
                    scripted_output(r#"{"id":987,"state":"commented"}"#, true)?,
                ]),
            };
            let outcome = execute_pending_review_delivery_with_transport(
                &args,
                &review,
                &payload,
                &mut transport,
            );
            if let Some(rejection) = rejection {
                let error = outcome.err().ok_or_else(|| {
                    anyhow::anyhow!("{case}: invalid summary-only review was submitted")
                })?;
                let expected_rejection = match rejection {
                    "head changed before pending-review creation" => format!(
                        "GitHub pull head changed before pending-review creation: expected {OLD_HEAD}, got {HEAD}"
                    ),
                    "revision reference is missing reviewed_commit" => {
                        "claim graph revision reference is missing reviewed_commit".to_owned()
                    }
                    "read" | "parse" => format!("{rejection} {}", graph_path.display()),
                    message => message.to_owned(),
                };
                ensure!(
                    error.to_string() == expected_rejection,
                    "{case}: wrong rejection: {error:#}"
                );
                ensure!(
                    transport.sends.len() == 2,
                    "{case}: attempted a remote write"
                );
                ensure!(
                    !args
                        .out
                        .join("delivery-pending-review-payload.json")
                        .exists(),
                    "{case}: prepared a remote write"
                );
            } else {
                outcome?;
                ensure!(transport.gets.is_empty() && transport.sends.is_empty());
                let pending: serde_json::Value = serde_json::from_slice(&fs::read(
                    args.out.join("delivery-pending-review-payload.json"),
                )?)?;
                ensure!(
                    pending
                        == serde_json::json!({"commit_id": HEAD, "body": review.body, "comments": []})
                );
                let submit: serde_json::Value = serde_json::from_slice(&fs::read(
                    args.out.join("delivery-submit-review-payload.json"),
                )?)?;
                ensure!(submit == serde_json::json!({"event": "COMMENT", "body": review.body}));
            }
        }
        Ok(())
    }

    #[test]
    fn inline_stale_revision_still_rejects_before_writes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = delivery_args(temp.path(), "http://scripted");
        let review = review_with_comment("src/lib.rs", 12, "[tests] exact body");
        let mut graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        graph["revision"] = serde_json::json!({
            "digest": "e".repeat(64), "semantics": "candidate_head",
            "reviewed_commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        });
        write_json(&temp.path().join("claim_graph.json"), &graph)?;
        let mut transport = ScriptedTransport {
            gets: VecDeque::from([serde_json::json!({"head": {"sha": HEAD}})]),
            sends: VecDeque::from([scripted_output(r#"{"id":987}"#, true)?]),
        };
        let error = execute_pending_review_delivery_with_transport(
            &args,
            &review,
            &github_review_post_payload(&review)?,
            &mut transport,
        )
        .err()
        .ok_or_else(|| anyhow::anyhow!("stale inline review was accepted"))?;
        ensure!(format!("{error:#}").contains("head changed"));
        ensure!(transport.gets.is_empty() && transport.sends.len() == 1);
        ensure!(
            !args
                .out
                .join("delivery-pending-review-payload.json")
                .exists()
        );
        Ok(())
    }

    #[test]
    fn lost_submission_response_retry_never_sends_another_review() -> Result<()> {
        uncertain_delivery_retry(false)
    }

    #[test]
    fn receipt_write_failure_retry_never_sends_another_review() -> Result<()> {
        uncertain_delivery_retry(true)
    }

    fn uncertain_delivery_retry(receipt_write_failure: bool) -> Result<()> {
        {
            let temp = tempfile::tempdir()?;
            fs::write(
                temp.path().join("claim_graph.json"),
                serde_json::to_vec(&graph_for("src/lib.rs", 12, "inline", HEAD))?,
            )?;
            let args = delivery_args(temp.path(), "http://scripted");
            if receipt_write_failure {
                fs::create_dir_all(args.out.join("post-stdout.json"))?;
            }
            let (review, payload) = delivery_review();
            let make_transport = || -> Result<ScriptedTransport> {
                Ok(ScriptedTransport {
                    gets: VecDeque::from([
                        serde_json::json!({"head": {"sha": HEAD}}),
                        returned("exact body"),
                        serde_json::json!({"head": {"sha": HEAD}}),
                    ]),
                    sends: VecDeque::from([
                        scripted_output(r#"{"id":987}"#, true)?,
                        scripted_output(r#"{"id":987,"state":"commented"}"#, true)?,
                    ]),
                })
            };
            let mut first = make_transport()?;
            if !receipt_write_failure {
                // The server accepted submission; its response was lost in transit.
                first.sends[1] = scripted_output("", false)?;
                first.sends.push_back(scripted_output("", false)?);
            }
            ensure!(
                execute_pending_review_delivery_with_transport(
                    &args, &review, &payload, &mut first
                )
                .is_err(),
                "uncertain delivery claimed success"
            );
            ensure!(!args.out.join("delivery-receipts.json").exists());
            if receipt_write_failure {
                fs::remove_dir(args.out.join("post-stdout.json"))?;
            }
            let mut retry = make_transport()?;
            let result = execute_pending_review_delivery_with_transport(
                &args, &review, &payload, &mut retry,
            );
            ensure!(
                retry.sends.len() == 2,
                "retry sent duplicate review after receipt_write_failure={receipt_write_failure}"
            );
            ensure!(result.is_err(), "uncertain delivery retry claimed success");
        }
        Ok(())
    }

    #[test]
    fn delivery_reservation_is_exclusive_and_contains_no_token() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = delivery_args(temp.path(), "http://scripted");
        let (_, payload) = delivery_review();
        reserve_delivery_attempt(&args, "http://scripted", "owner/repo", 42, HEAD, &payload)?;
        let bytes = fs::read(args.out.join("delivery-attempt.json"))?;
        let value: serde_json::Value = serde_json::from_slice(&bytes)?;
        ensure!(
            value["schema"] == "ub-review.delivery_attempt.v1",
            "reservation schema changed"
        );
        ensure!(value["api"] == "http://scripted", "reservation API changed");
        ensure!(value["repo"] == "owner/repo" && value["pull_number"] == 42);
        ensure!(value["exact_head_sha"] == HEAD);
        ensure!(value["payload_digest"] == sha256_hex(&serde_json::to_vec(&payload)?));
        ensure!(!String::from_utf8_lossy(&bytes).contains("test-token"));
        ensure!(
            reserve_delivery_attempt(&args, "http://scripted", "owner/repo", 42, HEAD, &payload)
                .is_err()
        );
        ensure!(fs::read(args.out.join("delivery-attempt.json"))? == bytes);
        Ok(())
    }

    #[test]
    fn body_only_delivery_retry_and_unwritable_reservation_never_post() -> Result<()> {
        for prior_success in [false, true] {
            let temp = tempfile::tempdir()?;
            let args = delivery_args(temp.path(), "http://scripted");
            let (mut review, mut payload) = delivery_review();
            review.comments.clear();
            payload.comments.clear();
            let mut first = ScriptedTransport {
                gets: VecDeque::from([
                    serde_json::json!({"head": {"sha": HEAD}}),
                    serde_json::json!([]),
                    serde_json::json!({"head": {"sha": HEAD}}),
                ]),
                sends: VecDeque::from([
                    scripted_output(r#"{"id":987}"#, true)?,
                    scripted_output(r#"{"id":987,"state":"commented"}"#, true)?,
                ]),
            };
            if prior_success {
                execute_pending_review_delivery_with_transport(
                    &args, &review, &payload, &mut first,
                )?;
                ensure!(first.sends.is_empty());
            } else {
                fs::create_dir_all(args.out.join("delivery-attempt.json"))?;
            }
            let mut retry = ScriptedTransport {
                gets: VecDeque::from([
                    serde_json::json!({"head": {"sha": HEAD}}),
                    serde_json::json!([]),
                ]),
                sends: VecDeque::from([scripted_output(r#"{"id":988}"#, true)?]),
            };
            let result = execute_pending_review_delivery_with_transport(
                &args, &review, &payload, &mut retry,
            );
            ensure!(result.is_err());
            ensure!(result.err().is_some_and(|error| {
                format!("{error:#}").contains("reconcile the previous delivery attempt")
            }));
            ensure!(
                retry.sends.len() == 1,
                "body-only retry created another review"
            );
        }
        Ok(())
    }

    #[test]
    fn unreceipted_reply_after_response_loss_cannot_claim_delivery() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let args = delivery_args(temp.path(), "http://scripted");
        let mut graph = graph_for("src/lib.rs", 12, "reply", HEAD);
        graph["topics"][0]["planned_thread_id"] = serde_json::json!("123");
        fs::write(
            temp.path().join("claim_graph.json"),
            serde_json::to_vec(&graph)?,
        )?;
        let (review, payload) = delivery_review();
        let source = serde_json::json!({
            "id": 123, "path": "src/lib.rs", "line": 12, "side": "RIGHT",
            "commit_id": HEAD, "body": "prior finding"
        });
        let mut first = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::json!({"head": {"sha": HEAD}}),
                serde_json::json!([source.clone()]),
            ]),
            sends: VecDeque::from([scripted_output("", false)?]),
        };
        ensure!(
            execute_pending_review_delivery_with_transport(&args, &review, &payload, &mut first)
                .is_err()
        );
        ensure!(!args.out.join("delivery-reply-receipts.json").exists());
        let mut retry = ScriptedTransport {
            gets: VecDeque::from([
                serde_json::json!({"head": {"sha": HEAD}}),
                serde_json::json!([source, {
                    "id": 456, "path": "src/lib.rs", "line": 12, "side": "RIGHT",
                    "commit_id": HEAD, "body": "exact body", "in_reply_to_id": 123
                }]),
                serde_json::json!({"head": {"sha": HEAD}}),
            ]),
            sends: VecDeque::from([scripted_output("", false)?]),
        };
        ensure!(
            execute_pending_review_delivery_with_transport(&args, &review, &payload, &mut retry)
                .is_err(),
            "response loss plus missing receipt was called delivered"
        );
        ensure!(retry.sends.len() == 1, "unreceipted reply was posted again");
        Ok(())
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
                    "body": "exact body"
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
        let _lock = lock_fake_delivery_tests()?;
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
        let _lock = lock_fake_delivery_tests()?;
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
    fn planned_inline_delivery_selects_the_public_surface_without_losing_ambiguity() -> Result<()> {
        let review = review_with_comment("src/lib.rs", 12, "[tests] exact body");
        let mut graph = graph_for("src/lib.rs", 12, "inline", HEAD);
        graph["topics"][0]["delivery"] = serde_json::json!("inline-candidate");
        let public = graph["topics"][0].clone();

        // Real retained packets keep private observations beside the selected
        // comment, sometimes with the same location and provisional action.
        for (id, disposition) in [
            ("private", serde_json::json!("no-human-surface")),
            ("summary", serde_json::json!("summary-only")),
            ("unknown", serde_json::json!("future-disposition")),
            ("malformed", serde_json::Value::Null),
        ] {
            let mut topic = public.clone();
            topic["claim_id"] = serde_json::json!(id);
            topic["delivery"] = disposition;
            graph["topics"]
                .as_array_mut()
                .context("fixture topics")?
                .push(topic);
        }
        let mut missing = public.clone();
        missing["claim_id"] = serde_json::json!("missing-disposition");
        missing
            .as_object_mut()
            .context("fixture topic")?
            .remove("delivery");
        graph["topics"]
            .as_array_mut()
            .context("fixture topics")?
            .push(missing);
        let before = graph.clone();
        let planned = build_planned_inline_deliveries(&review, HEAD, Some(&graph))?;
        ensure!(planned.len() == 1);
        ensure!(serde_json::to_value(&planned[0])?["claim_id"] == "claim-1");
        ensure!(
            graph == before,
            "delivery selection changed retained evidence"
        );

        let mut withheld = graph.clone();
        withheld["topics"][0]["delivery"] = serde_json::json!("no-human-surface");
        ensure!(
            build_planned_inline_deliveries(&review, HEAD, Some(&withheld)).is_err(),
            "a withheld claim authorized delivery"
        );

        let mut competing = graph;
        let mut second_public = public;
        second_public["claim_id"] = serde_json::json!("distinct-public-claim");
        competing["topics"]
            .as_array_mut()
            .context("fixture topics")?
            .push(second_public);
        ensure!(
            build_planned_inline_deliveries(&review, HEAD, Some(&competing)).is_err(),
            "distinct public claims at the same location lost their ambiguity"
        );
        Ok(())
    }

    #[test]
    fn public_surface_legacy_fallback_is_graph_wide() -> Result<()> {
        let review = review_with_comment("src/lib.rs", 12, "exact body");
        let legacy = graph_for("src/lib.rs", 12, "inline", HEAD);
        ensure!(build_planned_inline_deliveries(&review, HEAD, Some(&legacy))?.len() == 1);
        for disposition in [
            serde_json::json!("inline-candidate"),
            serde_json::Value::Null,
        ] {
            let mut mixed = legacy.clone();
            let mut marker = mixed["topics"][0].clone();
            marker["claim_id"] = serde_json::json!("elsewhere");
            marker["path"] = serde_json::json!("src/other.rs");
            marker["anchor"] = serde_json::json!(99);
            marker["delivery"] = disposition;
            mixed["topics"]
                .as_array_mut()
                .context("topics")?
                .push(marker);
            let before = mixed.clone();
            ensure!(
                build_planned_inline_deliveries(&review, HEAD, Some(&mixed)).is_err(),
                "a remote disposition marker allowed legacy delivery"
            );
            ensure!(mixed == before);
        }
        Ok(())
    }

    #[test]
    fn public_surface_malformed_dispositions_never_authorize_delivery() -> Result<()> {
        let review = review_with_comment("src/lib.rs", 12, "exact body");
        for disposition in [
            serde_json::json!(42),
            serde_json::json!(true),
            serde_json::json!({}),
            serde_json::json!([]),
            serde_json::Value::Null,
            serde_json::json!("summary-only"),
            serde_json::json!("no-human-surface"),
            serde_json::json!("future-disposition"),
        ] {
            let mut graph = graph_for("src/lib.rs", 12, "inline", HEAD);
            graph["topics"][0]["delivery"] = disposition.clone();
            ensure!(
                build_planned_inline_deliveries(&review, HEAD, Some(&graph)).is_err(),
                "malformed or withheld disposition authorized delivery: {disposition}"
            );
            let mut public = graph["topics"][0].clone();
            public["claim_id"] = serde_json::json!("selected");
            public["delivery"] = serde_json::json!("inline-candidate");
            graph["topics"]
                .as_array_mut()
                .context("topics")?
                .push(public);
            let before = graph.clone();
            let planned = build_planned_inline_deliveries(&review, HEAD, Some(&graph))?;
            ensure!(planned.len() == 1);
            ensure!(serde_json::to_value(&planned[0])?["claim_id"] == "selected");
            ensure!(graph == before);
        }
        Ok(())
    }

    #[test]
    fn public_surface_selection_preserves_action_and_reply_eligibility() -> Result<()> {
        let review = review_with_comment("src/lib.rs", 12, "exact body");
        for action in [Some("none"), Some("unknown"), None] {
            let mut graph = graph_for("src/lib.rs", 12, "inline", HEAD);
            graph["topics"][0]["delivery"] = serde_json::json!("inline-candidate");
            let mut ineligible = graph["topics"][0].clone();
            ineligible["claim_id"] = serde_json::json!("ineligible");
            if let Some(action) = action {
                ineligible["planned_action"] = serde_json::json!(action);
            } else {
                ineligible
                    .as_object_mut()
                    .context("topic")?
                    .remove("planned_action");
            }
            graph["topics"]
                .as_array_mut()
                .context("topics")?
                .push(ineligible);
            let planned = build_planned_inline_deliveries(&review, HEAD, Some(&graph))?;
            ensure!(
                planned.len() == 1,
                "ineligible action interfered with valid claim"
            );
            ensure!(serde_json::to_value(&planned[0])?["claim_id"] == "claim-1");
        }
        let mut reply = graph_for("src/lib.rs", 12, "reply", HEAD);
        reply["topics"][0]["delivery"] = serde_json::json!("inline-candidate");
        for thread in [None, Some(""), Some("   ")] {
            if let Some(thread) = thread {
                reply["topics"][0]["planned_thread_id"] = serde_json::json!(thread);
            } else {
                reply["topics"][0]
                    .as_object_mut()
                    .context("topic")?
                    .remove("planned_thread_id");
            }
            ensure!(build_planned_deliveries(&review, HEAD, Some(&reply)).is_err());
        }
        reply["topics"][0]["planned_thread_id"] = serde_json::json!("123");
        let planned = build_planned_deliveries(&review, HEAD, Some(&reply))?;
        ensure!(planned.len() == 1 && planned[0].action() == DeliveryAction::Reply);
        ensure!(serde_json::to_value(&planned[0])?["source_thread_id"] == "123");
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
                    r#"{{"id":456,"path":"src/lib.rs","line":12,"side":"RIGHT","commit_id":"{HEAD}","body":"exact body","in_reply_to_id":123,"pull_request_review_id":987}}"#
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
            r#"{{"id":456,"path":"src/lib.rs","line":12,"side":"RIGHT","commit_id":"{HEAD}","body":"exact body","in_reply_to_id":123}}"#
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
                        "body": "exact body",
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
                r#"{"id":456,"commit_id":"wrong-head","body":"exact body"}"#,
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
                    r#"{{"id":456,"commit_id":"{HEAD}","body":"exact body","in_reply_to_id":999}}"#
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
    fn delivery_head_prefers_immutable_revision_over_legacy_label() -> Result<()> {
        // A1.3: the revision reference is authoritative; the legacy
        // head_sha label may be a symbolic `HEAD` and must not win.
        let graph = serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": "HEAD",
            "revision": {
                "digest": "a".repeat(64),
                "semantics": "candidate_head",
                "reviewed_commit": "b".repeat(40),
            },
            "topics": [],
        });
        let review = crate::GitHubReview {
            event: String::new(),
            body: String::new(),
            comments: vec![crate::GitHubReviewComment {
                path: "src/lib.rs".to_owned(),
                line: 1,
                side: "RIGHT".to_owned(),
                body: "exact body".to_owned(),
                suggestion: None,
            }],
        };
        let expected = read_expected_delivery_head(&review, Some(&graph))?
            .ok_or_else(|| anyhow::anyhow!("immutable revision must bind the delivery head"))?;
        assert_eq!(expected, "b".repeat(40));

        // Pre-A1.3 packets keep working through the weak legacy label.
        let legacy = serde_json::json!({
            "schema": "ub-review.claim_graph.v1",
            "head_sha": "abc123",
            "topics": [],
        });
        let expected = read_expected_delivery_head(&review, Some(&legacy))?
            .ok_or_else(|| anyhow::anyhow!("legacy head label still binds"))?;
        assert_eq!(expected, "abc123");
        Ok(())
    }

    #[test]
    fn reply_identity_requires_exact_head_source_and_body() -> Result<()> {
        let body = "exact body";
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
        let body = "## Confirmed findings\n\n- exact body\n- exact body\n- unrelated finding\n";
        let filtered = body_after_confirmed_delivery(&review, body, &planned, &planned)?;
        ensure!(filtered == "## Confirmed findings\n\n- exact body\n- unrelated finding");
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
        let body = "- Guard evidence is missing.\n\n```suggestion\nlet header = guarded_header_read(ptr)?;\n```\n- retain this finding";
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
        let body = "- first body\n- second body";
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
            "body": "exact body"
        }]);
        ensure!(
            prior_confirmed_deliveries(&args, &planned, Some(&current), HEAD)?.len() == 1,
            "exact current receipt was not reused"
        );
        let (_, payload) = delivery_review();
        reserve_delivery_attempt(&args, "http://scripted", "owner/repo", 42, HEAD, &payload)?;
        let mut transaction = DeliveryTransaction::new(HEAD, planned.clone())?;
        for state in [
            DeliveryTransactionState::PendingReviewCreated,
            DeliveryTransactionState::CommentsCreated,
            DeliveryTransactionState::CommentsReconciled,
            DeliveryTransactionState::HeadRevalidated,
        ] {
            transaction.transition(state)?;
        }
        write_transaction(&args.out, &transaction)?;
        ensure!(
            prior_confirmed_deliveries(&args, &planned, Some(&current), HEAD)?.is_empty(),
            "pre-submission reconciliation was falsely reused as delivered"
        );
        transaction.transition(DeliveryTransactionState::Submitted)?;
        transaction.transition(DeliveryTransactionState::ReceiptsPersisted)?;
        write_transaction(&args.out, &transaction)?;
        ensure!(prior_confirmed_deliveries(&args, &planned, Some(&current), HEAD)?.len() == 1);
        let stale_current = serde_json::json!([{
            "id": 456,
            "path": "src/lib.rs",
            "line": 12,
            "side": "RIGHT",
            "commit_id": "old-head",
            "body": "exact body"
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
        let observed = observed_deliveries(&returned("exact body"), &[planned], HEAD)?;
        ensure!(observed.len() == 1, "expected one reconciled comment");
        Ok(())
    }

    #[test]
    fn returned_comment_wrong_body_or_head_is_rejected() -> Result<()> {
        let planned = planned()?;
        ensure!(
            observed_deliveries(
                &returned("different body"),
                std::slice::from_ref(&planned),
                HEAD
            )
            .is_err(),
            "wrong returned body was accepted"
        );
        let mut wrong_head = returned("exact body");
        wrong_head[0]["commit_id"] = serde_json::json!("another-head");
        ensure!(
            observed_deliveries(&wrong_head, std::slice::from_ref(&planned), HEAD).is_err(),
            "wrong returned head was accepted"
        );
        let mut missing_head = returned("exact body");
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
            "body": "exact body"
        }]);
        ensure!(
            observed_deliveries(&unexpected, std::slice::from_ref(&planned), HEAD).is_err(),
            "unexpected returned comment was accepted"
        );
        let duplicate = serde_json::json!([
            {"id": 42, "path": "src/lib.rs", "line": 12, "side": "RIGHT", "commit_id": HEAD, "body": "exact body"},
            {"id": 42, "path": "src/lib.rs", "line": 12, "side": "RIGHT", "commit_id": HEAD, "body": "exact body"}
        ]);
        let observed = observed_deliveries(&duplicate, std::slice::from_ref(&planned), HEAD)?;
        ensure!(
            reconcile_deliveries(HEAD, "review-1", &[planned], &observed).is_err(),
            "duplicate returned comment id was accepted"
        );
        Ok(())
    }
}
