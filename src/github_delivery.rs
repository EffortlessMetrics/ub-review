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
    let planned = build_planned_inline_deliveries(review, &expected_head, claim_graph.as_ref())?;
    let mut transaction = DeliveryTransaction::new(expected_head.clone(), planned.clone())?;

    let pending_payload = serde_json::json!({
        "commit_id": expected_head,
        "body": api_payload.body,
        "comments": api_payload.comments,
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
        let observed = observed_deliveries(&listed, &planned, &expected_head)?;
        let reconciliation =
            reconcile_deliveries(&expected_head, &created_review_id, &planned, &observed)?;
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

        let rechecked_head = fetch_pull_head(transport, api, repo, pull_number, token)?;
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

fn build_planned_inline_deliveries(
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
mod tests {
    use super::*;
    use anyhow::ensure;
    use std::collections::BTreeMap;
    use std::collections::VecDeque;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::thread;
    use std::time::Duration;

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
    struct FakeHttpResponse {
        status: u16,
        body: String,
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

    fn spawn_fake_delivery_api(
        responses: Vec<FakeHttpResponse>,
    ) -> Result<(String, FakeApiHandle)> {
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(false)?;
        let address = format!("http://{}", listener.local_addr()?);
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for response in responses {
                let (stream, _) = listener.accept()?;
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
