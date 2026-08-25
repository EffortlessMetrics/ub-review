//! Golden review-experience contracts.
//!
//! This module keeps the first real-world review fixture executable without
//! coupling the fixture to model wording or GitHub transport. The classifier
//! is intentionally small: it proves structural identity, current-head
//! supersession, and thread reuse before the result is wired into the active
//! compiler.

use serde::Deserialize;
use std::collections::BTreeSet;

use crate::*;

#[derive(Debug, Deserialize)]
struct ReviewExperienceFixture {
    schema: String,
    case_id: String,
    repository: String,
    pull_number: u64,
    title: String,
    base_sha: String,
    buggy_head_sha: String,
    fixed_head_sha: String,
    pr_body: FixturePrBody,
    diff: Vec<FixtureDiff>,
    receipts: Vec<FixtureReceipt>,
    claims: Vec<FixtureClaim>,
    threads: Vec<FixtureThread>,
    heads: Vec<FixtureHead>,
}

#[derive(Debug, Deserialize)]
struct FixturePrBody {
    claim: String,
    focused_command: String,
    other_commands: Vec<String>,
    known_red: String,
}

#[derive(Debug, Deserialize)]
struct FixtureDiff {
    path: String,
    hunk: String,
}

#[derive(Debug, Deserialize)]
struct FixtureReceipt {
    kind: String,
    status: String,
    command: String,
}

#[derive(Debug, Deserialize)]
struct FixtureClaim {
    id: String,
    path: String,
    symbol: String,
    failure_family: String,
    mechanism: String,
    anchor: Option<u32>,
    fixed_on_head: String,
}

#[derive(Debug, Deserialize)]
struct FixtureThread {
    id: String,
    author: String,
    body: String,
    path: String,
    anchor: Option<u32>,
    symbol: String,
    failure_family: String,
    mechanism: String,
    status: String,
}

#[derive(Debug, Deserialize)]
struct FixtureHead {
    sha: String,
    expected_dispositions: Vec<ExpectedDisposition>,
    public_surface: Vec<PublicSurfaceItem>,
    forbidden_public_fragments: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ExpectedDisposition {
    claim_id: String,
    disposition: String,
}

#[derive(Debug, Deserialize)]
struct PublicSurfaceItem {
    claim_id: String,
    action: String,
    #[serde(default)]
    thread_id: Option<String>,
    path: Option<String>,
    line: Option<u32>,
    head_sha: String,
    body: String,
    #[serde(default)]
    delivery_receipt: Option<FixtureDeliveryReceipt>,
}

#[derive(Debug, Deserialize)]
struct FixtureDeliveryReceipt {
    status: String,
    action: String,
    claim_id: String,
    head_sha: String,
    #[serde(default)]
    source_thread_id: Option<String>,
    #[serde(default)]
    comment_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FixtureDisposition {
    AlreadyCovered,
    Novel,
    FixedOnCurrentHead,
}

fn structural_identity(
    path: &str,
    symbol: &str,
    failure_family: &str,
    mechanism: &str,
    anchor: Option<u32>,
) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        path.trim().replace('\\', "/"),
        symbol.trim(),
        failure_family.trim(),
        mechanism.trim(),
        anchor.map_or_else(|| "-".to_owned(), |line| line.to_string())
    )
}

fn claim_identity(claim: &FixtureClaim) -> String {
    structural_identity(
        &claim.path,
        &claim.symbol,
        &claim.failure_family,
        &claim.mechanism,
        claim.anchor,
    )
}

fn thread_identity(thread: &FixtureThread) -> String {
    structural_identity(
        &thread.path,
        &thread.symbol,
        &thread.failure_family,
        &thread.mechanism,
        thread.anchor,
    )
}

fn classify_claim(
    claim: &FixtureClaim,
    current_head: &str,
    threads: &[FixtureThread],
) -> FixtureDisposition {
    if claim.fixed_on_head == current_head {
        return FixtureDisposition::FixedOnCurrentHead;
    }

    let identity = claim_identity(claim);
    if threads
        .iter()
        .any(|thread| thread.status == "open" && thread_identity(thread) == identity)
    {
        FixtureDisposition::AlreadyCovered
    } else {
        FixtureDisposition::Novel
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result as AnyResult;
    use clap::Parser;
    use std::fs;
    use std::path::Path;

    fn fixture() -> Result<ReviewExperienceFixture, String> {
        serde_json::from_str(include_str!(
            "../fixtures/review-experience/perl-lsp-3627.json"
        ))
        .map_err(|error| format!("golden review fixture must be valid JSON: {error}"))
    }

    fn require(condition: bool, message: impl Into<String>) -> Result<(), String> {
        if condition {
            Ok(())
        } else {
            Err(message.into())
        }
    }

    fn production_thread_context(
        fixture: &ReviewExperienceFixture,
        thread_commit: &str,
        review_head: &str,
    ) -> PrThreadContext {
        production_thread_context_with_shape(fixture, thread_commit, review_head, None, &[])
    }

    fn production_thread_context_with_shape(
        fixture: &ReviewExperienceFixture,
        thread_commit: &str,
        review_head: &str,
        stale_thread_id: Option<&str>,
        omitted_thread_ids: &[&str],
    ) -> PrThreadContext {
        // Mirrors the production binding rule in `github_thread_records`: an
        // inline thread is `current` only when its commit is the reviewed head.
        PrThreadContext {
            schema: PR_THREAD_CONTEXT_SCHEMA.to_owned(),
            status: "seeded".to_owned(),
            max_bytes: 65_536,
            sources: vec!["fixture:perl-lsp-3627".to_owned()],
            warnings: Vec::new(),
            pull_number: Some(fixture.pull_number),
            title: Some(fixture.title.clone()),
            body: Some(fixture.pr_body.claim.clone()),
            body_truncated: false,
            thread_context_path: None,
            thread_context: None,
            thread_context_truncated: false,
            threads: fixture
                .threads
                .iter()
                .filter(|thread| !omitted_thread_ids.contains(&thread.id.as_str()))
                .map(|thread| ReviewThreadRecord {
                    id: thread.id.clone(),
                    kind: "review-comment".to_owned(),
                    author: thread.author.clone(),
                    body: thread.body.clone(),
                    path: (!thread.path.is_empty()).then(|| thread.path.clone()),
                    line: thread.anchor,
                    commit_id: Some(if Some(thread.id.as_str()) == stale_thread_id {
                        fixture.base_sha.clone()
                    } else {
                        thread_commit.to_owned()
                    }),
                    head_binding: if Some(thread.id.as_str()) == stale_thread_id
                        || thread_commit != review_head
                    {
                        "stale"
                    } else {
                        "current"
                    }
                    .to_owned(),
                    state: Some(thread.status.clone()),
                })
                .collect(),
        }
    }

    fn production_observations(fixture: &ReviewExperienceFixture) -> Vec<Observation> {
        fixture
            .claims
            .iter()
            .map(|claim| {
                let claim_body = fixture
                    .threads
                    .iter()
                    .find(|thread| thread.path == claim.path && thread.anchor == claim.anchor)
                    .map(|thread| thread.body.clone())
                    .unwrap_or_else(|| claim.mechanism.clone());
                Observation {
                    schema: "ub-review.observation.v1".to_owned(),
                    id: format!("fixture-{}", claim.id),
                    lane: "fixture".to_owned(),
                    question: fixture.pr_body.claim.clone(),
                    claim: claim_body,
                    kind: claim.failure_family.clone(),
                    status: "confirmed".to_owned(),
                    severity: "medium".to_owned(),
                    confidence: "high".to_owned(),
                    path: Some(claim.path.clone()),
                    line: claim.anchor,
                    fingerprint: claim.id.clone(),
                    evidence: vec!["fixture focused-proof receipt".to_owned()],
                    dedupe_key: claim.mechanism.clone(),
                    source: "fixture".to_owned(),
                }
            })
            .collect()
    }

    fn fixture_run_args(root: &Path, out: &Path, head: &str) -> Result<RunArgs, String> {
        let root = root.to_string_lossy().into_owned();
        let out = out.to_string_lossy().into_owned();
        let cli = Cli::try_parse_from([
            "ub-review",
            "run",
            "--root",
            root.as_str(),
            "--out",
            out.as_str(),
            "--base",
            "fixture-base",
            "--head",
            head,
            "--posting",
            "review",
            "--run-pass",
            "manual",
            "--max-inline-comments",
            "2",
        ])
        .map_err(|error| error.to_string())?;
        match cli.command {
            Command::Run(args) => Ok(*args),
            _ => Err("fixture argument parser returned a non-run command".to_owned()),
        }
    }

    fn fixture_plan(fixture: &ReviewExperienceFixture) -> Plan {
        Plan {
            base: fixture.base_sha.clone(),
            head: fixture.buggy_head_sha.clone(),
            profile_name: "fixture".to_owned(),
            diff_class: DiffClass::SourceGeneral,
            changed_files: fixture.diff.iter().map(|diff| diff.path.clone()).collect(),
            language_mix: LanguageMix::default(),
            sensors: Vec::new(),
            lanes: Vec::new(),
            repo_lanes: Vec::new(),
            docs_only: false,
            notes: Vec::new(),
        }
    }

    fn fixture_diff(fixture: &ReviewExperienceFixture, head: &str) -> DiffContext {
        DiffContext {
            base: fixture.base_sha.clone(),
            head: head.to_owned(),
            changed_files: fixture.diff.iter().map(|diff| diff.path.clone()).collect(),
            patch: fixture
                .diff
                .iter()
                .map(|diff| diff.hunk.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            flags: DiffFlags {
                source_changed: true,
                ..DiffFlags::default()
            },
            diff_class: DiffClass::SourceGeneral,
        }
    }

    fn fixture_comment(thread: &FixtureThread, body: &str) -> ReviewInlineComment {
        ReviewInlineComment {
            lane: "fixture".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: thread.path.clone(),
            line: thread.anchor.unwrap_or_default(),
            side: "RIGHT".to_owned(),
            body: format!("[fixture] {body}"),
            evidence: "focused parser proof receipt".to_owned(),
            suggestion: None,
        }
    }

    type FixtureApiHandle = std::thread::JoinHandle<anyhow::Result<Vec<(String, String)>>>;

    fn spawn_fixture_delivery_api(
        exact_head: &str,
        source_thread_id: &str,
        source_path: &str,
        source_line: u32,
        inline_comment: &GitHubReviewComment,
        reply_body: &str,
    ) -> AnyResult<(String, FixtureApiHandle)> {
        // GitHub echoes back what was posted, which is the reviewer-facing
        // rendering of the comment (no lane identity, suggestion fenced), not
        // the artifact body the compiler stored.
        let inline_posted_body = github_review_post_comment_body(inline_comment)?;
        crate::github_delivery::spawn_fake_delivery_api(vec![
            crate::github_delivery::FakeHttpResponse::new(
                200,
                serde_json::json!({"head": {"sha": exact_head}}).to_string(),
            ),
            crate::github_delivery::FakeHttpResponse::new(
                200,
                serde_json::json!([{
                    "id": source_thread_id,
                    "path": source_path,
                    "line": source_line,
                    "side": "RIGHT",
                    "commit_id": exact_head,
                    "body": "The existing parser thread is current on this head."
                }])
                .to_string(),
            ),
            crate::github_delivery::FakeHttpResponse::new(
                200,
                serde_json::json!({"id": 987, "state": "PENDING"}).to_string(),
            ),
            crate::github_delivery::FakeHttpResponse::new(
                200,
                serde_json::json!([{
                    "id": 2001,
                    "path": inline_comment.path,
                    "line": inline_comment.line,
                    "side": inline_comment.side,
                    "commit_id": exact_head,
                    "body": inline_posted_body
                }])
                .to_string(),
            ),
            crate::github_delivery::FakeHttpResponse::new(
                200,
                serde_json::json!({
                    "id": 2002,
                    "commit_id": exact_head,
                    "in_reply_to_id": source_thread_id,
                    "body": reply_body
                })
                .to_string(),
            ),
            crate::github_delivery::FakeHttpResponse::new(
                200,
                serde_json::json!({"head": {"sha": exact_head}}).to_string(),
            ),
            crate::github_delivery::FakeHttpResponse::new(
                200,
                serde_json::json!({"id": 987, "state": "COMMENTED"}).to_string(),
            ),
        ])
    }

    #[test]
    fn perl_lsp_3627_keeps_material_claims_structurally_distinct() -> Result<(), String> {
        let fixture = fixture()?;
        require(
            fixture.schema == "ub-review.review-experience-fixture.v1",
            "unexpected fixture schema",
        )?;
        require(
            fixture.case_id == "perl-lsp-swarm#3627",
            "unexpected fixture case",
        )?;
        require(
            fixture.repository == "EffortlessMetrics/perl-lsp-swarm" && fixture.pull_number == 3627,
            "unexpected fixture repository or pull number",
        )?;
        require(!fixture.title.trim().is_empty(), "fixture title is empty")?;
        require(
            !fixture.pr_body.claim.trim().is_empty(),
            "fixture PR claim is empty",
        )?;
        require(
            fixture
                .pr_body
                .focused_command
                .contains("ripr_seam_proof_unparenthesized_declarations"),
            "fixture focused proof command is missing",
        )?;
        require(
            !fixture.pr_body.other_commands.is_empty() && !fixture.pr_body.known_red.is_empty(),
            "fixture PR verification context is incomplete",
        )?;
        require(
            fixture.diff.len() >= 2
                && fixture
                    .diff
                    .iter()
                    .all(|diff| !diff.path.trim().is_empty() && !diff.hunk.trim().is_empty()),
            "fixture diff context is incomplete",
        )?;
        require(
            fixture.receipts.len() >= 3
                && fixture.receipts.iter().all(|receipt| {
                    !receipt.kind.trim().is_empty()
                        && !receipt.status.trim().is_empty()
                        && !receipt.command.trim().is_empty()
                }),
            "fixture receipt context is incomplete",
        )?;
        for required_kind in [
            "focused-test",
            "full-suite",
            "artifact-verifier",
            "cargo-allow",
            "heavy-witness",
        ] {
            require(
                fixture
                    .receipts
                    .iter()
                    .any(|receipt| receipt.kind == required_kind),
                format!("fixture is missing {required_kind} receipt context"),
            )?;
        }
        require(
            fixture.base_sha != fixture.buggy_head_sha,
            "base and buggy heads must differ",
        )?;
        require(
            fixture.buggy_head_sha != fixture.fixed_head_sha,
            "buggy and fixed heads must differ",
        )?;

        let identities = fixture
            .claims
            .iter()
            .map(claim_identity)
            .collect::<BTreeSet<_>>();
        require(
            identities.len() == fixture.claims.len(),
            "structurally distinct claims must not collapse",
        )?;
        let thread_ids = fixture
            .threads
            .iter()
            .map(|thread| thread.id.as_str())
            .collect::<BTreeSet<_>>();
        require(
            thread_ids.len() == fixture.threads.len()
                && thread_ids.iter().all(|id| !id.trim().is_empty()),
            "thread receipts must have unique non-empty IDs",
        )
    }

    #[test]
    fn perl_lsp_3627_reuses_threads_and_silences_the_fixed_head() -> Result<(), String> {
        let fixture = fixture()?;
        for head in &fixture.heads {
            let actual = fixture
                .claims
                .iter()
                .map(|claim| {
                    (
                        claim.id.as_str(),
                        classify_claim(claim, &head.sha, &fixture.threads),
                    )
                })
                .collect::<Vec<_>>();

            for expected in &head.expected_dispositions {
                let (_, disposition) = actual
                    .iter()
                    .find(|(claim_id, _)| *claim_id == expected.claim_id)
                    .ok_or_else(|| {
                        format!(
                            "expected disposition must name a fixture claim: {}",
                            expected.claim_id
                        )
                    })?;
                let expected_disposition = match expected.disposition.as_str() {
                    "already_covered" => FixtureDisposition::AlreadyCovered,
                    "novel" => FixtureDisposition::Novel,
                    "fixed_on_current_head" => FixtureDisposition::FixedOnCurrentHead,
                    other => return Err(format!("unsupported fixture disposition {other}")),
                };
                require(
                    *disposition == expected_disposition,
                    format!("unexpected disposition for claim {}", expected.claim_id),
                )?;
            }

            let surfaced_claims = head
                .public_surface
                .iter()
                .map(|item| item.claim_id.as_str())
                .collect::<Vec<_>>();
            require(
                surfaced_claims.len() == surfaced_claims.iter().collect::<BTreeSet<_>>().len(),
                "each claim may have at most one human-facing location",
            )?;
            if head.sha == fixture.buggy_head_sha {
                require(
                    head.public_surface.len() <= 3,
                    "buggy head exceeded the few-public-surfaces budget",
                )?;
            }

            for item in &head.public_surface {
                require(
                    item.head_sha == head.sha,
                    "delivery receipt has a stale head",
                )?;
                require(!item.body.trim().is_empty(), "public finding body is empty")?;
                if item.action == "inline" {
                    require(item.path.is_some(), "inline delivery is missing a path")?;
                    require(item.line.is_some(), "inline delivery is missing a line")?;
                }
                let delivery = item
                    .delivery_receipt
                    .as_ref()
                    .ok_or_else(|| "public surface item is missing delivery receipt".to_owned())?;
                require(
                    delivery.status == "posted"
                        && delivery.action == item.action
                        && delivery.claim_id == item.claim_id
                        && delivery.head_sha == head.sha,
                    format!("delivery receipt is not bound to {}", item.claim_id),
                )?;
                require(
                    delivery.comment_id.is_some(),
                    format!(
                        "delivery receipt is missing comment id for {}",
                        item.claim_id
                    ),
                )?;
                if item.action == "reply" {
                    let thread_id = item
                        .thread_id
                        .as_deref()
                        .ok_or_else(|| "reply delivery is missing source thread".to_owned())?;
                    require(
                        fixture.threads.iter().any(|thread| thread.id == thread_id),
                        format!("reply delivery names unknown thread {thread_id}"),
                    )?;
                    require(
                        delivery.source_thread_id.as_deref() == Some(thread_id),
                        format!("reply receipt is not bound to thread {thread_id}"),
                    )?;
                }
            }

            for forbidden in &head.forbidden_public_fragments {
                for item in &head.public_surface {
                    require(
                        !item
                            .body
                            .to_ascii_lowercase()
                            .contains(&forbidden.to_ascii_lowercase()),
                        format!("internal planning phrase leaked into public output: {forbidden}"),
                    )?;
                }
            }

            if head.sha == fixture.fixed_head_sha {
                require(
                    head.public_surface.is_empty(),
                    "fixed head must produce a silent public surface",
                )?;
            }
        }

        let candidate_thread = fixture
            .threads
            .first()
            .ok_or_else(|| "fixture has no inline thread candidate".to_owned())?;
        let candidate = ReviewInlineComment {
            lane: "fixture".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            path: candidate_thread.path.clone(),
            line: candidate_thread
                .anchor
                .ok_or_else(|| "fixture candidate thread has no anchor".to_owned())?,
            side: "RIGHT".to_owned(),
            body: candidate_thread.body.clone(),
            evidence: "focused proof receipt".to_owned(),
            suggestion: None,
        };
        let observations = production_observations(&fixture);
        let current_graph = build_active_claim_graph(
            &fixture.buggy_head_sha,
            &observations,
            std::slice::from_ref(&candidate),
            &[],
            &[],
            &[],
            &production_thread_context(&fixture, &fixture.buggy_head_sha, &fixture.buggy_head_sha),
        );
        require(
            current_graph
                .topics
                .iter()
                .any(|topic| !topic.existing_threads.is_empty()),
            "production graph did not retain the current fixture thread",
        )?;
        require(
            current_graph.topics.len() == fixture.claims.len(),
            "production graph collapsed structurally distinct fixture claims",
        )?;
        require(
            current_graph
                .topics
                .iter()
                .all(|topic| topic.thread_disposition == "already_covered"),
            "current-head fixture threads were not classified as already covered",
        )?;
        require(
            reconcile_inline_comments(&current_graph, std::slice::from_ref(&candidate)).is_empty(),
            "production reconciliation did not suppress a current-thread duplicate",
        )?;

        let stale_graph = build_active_claim_graph(
            &fixture.fixed_head_sha,
            &observations,
            std::slice::from_ref(&candidate),
            &[],
            &[],
            &[],
            &production_thread_context(&fixture, &fixture.buggy_head_sha, &fixture.fixed_head_sha),
        );
        let stale_reconciled =
            reconcile_inline_comments(&stale_graph, std::slice::from_ref(&candidate));
        require(
            stale_reconciled.len() == 1,
            "production reconciliation incorrectly suppressed a stale-thread candidate",
        )?;
        require(
            stale_graph.topics.iter().all(|topic| {
                topic.head_sha == fixture.fixed_head_sha
                    && topic.thread_disposition == "superseded_by_head_change"
            }),
            "fixed-head graph did not invalidate the prior review certification",
        )?;
        Ok(())
    }

    #[test]
    fn perl_lsp_3627_thread_fixture_includes_external_reviewers_and_receipts() -> Result<(), String>
    {
        let fixture = fixture()?;
        let authors = fixture
            .threads
            .iter()
            .map(|thread| thread.author.as_str())
            .collect::<BTreeSet<_>>();
        for author in [
            "factory-droid[bot]",
            "cubic-dev-ai[bot]",
            "sourcery-ai[bot]",
            "github-actions[bot]",
        ] {
            require(
                authors.contains(author),
                format!("missing fixture author {author}"),
            )?;
        }

        let buggy = fixture
            .heads
            .iter()
            .find(|head| head.sha == fixture.buggy_head_sha)
            .ok_or_else(|| "buggy head case is present".to_owned())?;
        require(
            buggy
                .public_surface
                .iter()
                .all(|item| item.head_sha == fixture.buggy_head_sha),
            "buggy head delivery receipt is stale",
        )?;
        require(
            buggy
                .public_surface
                .iter()
                .any(|item| item.action == "reply"),
            "buggy head must reuse an existing thread",
        )
    }

    #[test]
    fn perl_lsp_3627_runs_production_compiler_and_delivery_replay() -> Result<(), String> {
        let _lock = crate::github_delivery::lock_fake_delivery_tests()
            .map_err(|error| error.to_string())?;
        let fixture = fixture()?;
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let out = temp.path().join("review");
        fs::create_dir_all(&out).map_err(|error| error.to_string())?;

        let reply_thread = fixture
            .threads
            .iter()
            .find(|thread| thread.id == "3558771748")
            .ok_or_else(|| "fixture reply thread is missing".to_owned())?;
        let inline_thread = fixture
            .threads
            .iter()
            .find(|thread| thread.id == "3558766480")
            .ok_or_else(|| "fixture inline thread is missing".to_owned())?;
        let mut observations = production_observations(&fixture);
        let reply_observation = observations
            .iter_mut()
            .find(|observation| observation.id == "fixture-parser:later-variable-subscript")
            .ok_or_else(|| "fixture reply observation is missing".to_owned())?;
        reply_observation.status = "refuted".to_owned();
        reply_observation.evidence = vec!["focused parser proof receipt".to_owned()];

        let reply_comment = fixture_comment(reply_thread, &reply_thread.body);
        let inline_comment = fixture_comment(inline_thread, &inline_thread.body);
        for observation in &mut observations {
            if observation.line == reply_thread.anchor || observation.line == inline_thread.anchor {
                observation.claim = format!("[fixture] {}", observation.claim);
            }
        }
        let comments = vec![reply_comment, inline_comment];
        let thread_context = production_thread_context_with_shape(
            &fixture,
            &fixture.buggy_head_sha,
            &fixture.buggy_head_sha,
            None,
            &[inline_thread.id.as_str(), "3558707519"],
        );
        let initial_graph = build_active_claim_graph(
            &fixture.buggy_head_sha,
            &observations,
            &comments,
            &[],
            &[],
            &[],
            &thread_context,
        );
        let reply_claim_id = initial_graph
            .topics
            .iter()
            .find(|topic| topic.path.as_deref() == Some(reply_thread.path.as_str()))
            .map(|topic| topic.claim_id.clone())
            .ok_or_else(|| "reply claim was not produced by the production graph".to_owned())?;
        let proof_receipt = ProofReceipt {
            revision: None,
            schema: "ub-review.proof_receipt.v1".to_owned(),
            id: "fixture-focused-parser-proof".to_owned(),
            kind: "focused-test".to_owned(),
            base: fixture.base_sha.clone(),
            head: fixture.buggy_head_sha.clone(),
            test_patch_mode: "none".to_owned(),
            requested_by: vec![reply_claim_id.clone()],
            request_ids: vec![reply_claim_id],
            commands: vec![ProofCommandReceipt {
                side: "head".to_owned(),
                command: fixture.pr_body.focused_command.clone(),
                env: std::collections::BTreeMap::new(),
                status: "passed".to_owned(),
                exit_code: Some(0),
                timed_out: false,
                timeout_sec: 300,
                duration_ms: 1,
                stdout: "focused proof passed".to_owned(),
                stderr: String::new(),
                reason: "fixture proof receipt".to_owned(),
            }],
            result: "passed".to_owned(),
            reason: "focused parser proof changed the reply disposition".to_owned(),
        };
        let proof_receipts = vec![proof_receipt];
        let graph = build_active_claim_graph(
            &fixture.buggy_head_sha,
            &observations,
            &comments,
            &[],
            &[],
            &proof_receipts,
            &thread_context,
        );
        let reply_topic = graph
            .topics
            .iter()
            .find(|topic| topic.path.as_deref() == Some(reply_thread.path.as_str()))
            .ok_or_else(|| "reply topic was not retained".to_owned())?;
        require(
            reply_topic.planned_action == "reply"
                && reply_topic.planned_thread_id.as_deref() == Some(reply_thread.id.as_str())
                && reply_topic.proof_receipts == vec!["fixture-focused-parser-proof"],
            "production graph did not create an exact proof-backed reply plan",
        )?;
        let inline_topic = graph
            .topics
            .iter()
            .find(|topic| topic.path.as_deref() == Some(inline_thread.path.as_str()))
            .ok_or_else(|| "inline topic was not retained".to_owned())?;
        require(
            inline_topic.planned_action == "inline" && inline_topic.planned_thread_id.is_none(),
            "production graph did not create a novel inline plan",
        )?;
        require(
            graph.topics.len() == fixture.claims.len(),
            format!(
                "production graph collapsed distinct Perl claims: topics={} claims={} ids={:?}",
                graph.topics.len(),
                fixture.claims.len(),
                graph
                    .topics
                    .iter()
                    .map(|topic| {
                        format!(
                            "{}:{}:{}:{}",
                            topic.path.as_deref().unwrap_or("<summary>"),
                            topic.anchor.unwrap_or_default(),
                            topic.mechanism,
                            topic.subject
                        )
                    })
                    .collect::<Vec<_>>()
            ),
        )?;
        write_claim_graph(temp.path(), &graph).map_err(|error| error.to_string())?;

        let plan = fixture_plan(&fixture);
        let diff = fixture_diff(&fixture, &fixture.buggy_head_sha);
        let args = fixture_run_args(temp.path(), &out, &fixture.buggy_head_sha)?;
        let body_policy = ReviewBodyPolicy::default();
        let post_review_on = vec!["pull_request".to_owned()];
        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "fixture-context",
            review_body_policy: &body_policy,
            run_pass: RunPass::Manual,
            post_review_on: &post_review_on,
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &comments,
            summary_only_findings: &[],
            observations: &observations,
            proof_receipts: &proof_receipts,
            suggested_issues: &[],
            final_follow_up_tasks: 0,
            reporter_distillation: Some(
                "The focused receipt changes the disposition; internal lane and queue state stays hidden.",
            ),
        })
        .map_err(|error| error.to_string())?;
        require(
            surface.should_prepare_github_review && surface.github_review.comments.len() == 2,
            "production compiler did not retain the reply and inline surfaces",
        )?;
        for forbidden in &fixture
            .heads
            .iter()
            .find(|head| head.sha == fixture.buggy_head_sha)
            .ok_or_else(|| "fixture buggy head is missing".to_owned())?
            .forbidden_public_fragments
        {
            require(
                !surface
                    .github_review
                    .body
                    .to_ascii_lowercase()
                    .contains(&forbidden.to_ascii_lowercase()),
                format!("forbidden internal text reached compiled body: {forbidden}"),
            )?;
            for comment in &surface.github_review.comments {
                require(
                    !comment
                        .body
                        .to_ascii_lowercase()
                        .contains(&forbidden.to_ascii_lowercase()),
                    format!("forbidden internal text reached compiled comment: {forbidden}"),
                )?;
            }
        }
        let reply_index = surface
            .github_review
            .comments
            .iter()
            .position(|comment| comment.line == reply_thread.anchor.unwrap_or_default())
            .ok_or_else(|| "compiled reply comment is missing".to_owned())?;
        let inline_index = surface
            .github_review
            .comments
            .iter()
            .position(|comment| comment.line == inline_thread.anchor.unwrap_or_default())
            .ok_or_else(|| "compiled inline comment is missing".to_owned())?;
        let reply_body =
            github_review_post_comment_body(&surface.github_review.comments[reply_index])
                .map_err(|error| error.to_string())?;
        let (api, server) = spawn_fixture_delivery_api(
            &fixture.buggy_head_sha,
            &reply_thread.id,
            &reply_thread.path,
            reply_thread.anchor.unwrap_or_default(),
            &surface.github_review.comments[inline_index],
            &reply_body,
        )
        .map_err(|error| error.to_string())?;
        let post_args = PostArgs {
            review_json: out.join("github-review.json"),
            diff_patch: None,
            out: out.clone(),
            github_token: Some("fixture-token".to_owned()),
            repo: Some("owner/repo".to_owned()),
            pull_number: Some(42),
            github_api_url: api,
            fail_on_post_error: true,
        };
        let payload = github_review_post_payload(&surface.github_review)
            .map_err(|error| error.to_string())?;
        let outcome = execute_pending_review_delivery(&post_args, &surface.github_review, &payload)
            .map_err(|error| error.to_string())?;
        require(
            outcome.response["state"] == "COMMENTED",
            "production delivery did not submit the compiled review",
        )?;
        let requests = server
            .join()
            .map_err(|_| "fixture GitHub server panicked".to_owned())?
            .map_err(|error| error.to_string())?;
        require(
            requests.len() == 7
                && requests[0].0.starts_with("GET /repos/owner/repo/pulls/42 ")
                && requests[1].0.contains("/pulls/42/comments?")
                && requests[2]
                    .0
                    .starts_with("POST /repos/owner/repo/pulls/42/reviews ")
                && requests[3].0.contains("/reviews/987/comments")
                && requests[4]
                    .0
                    .starts_with("POST /repos/owner/repo/pulls/42/comments ")
                && requests[5].0.starts_with("GET /repos/owner/repo/pulls/42 ")
                && requests[6].0.contains("/reviews/987/events")
                && !requests[6]
                    .1
                    .contains(&surface.github_review.comments[inline_index].body),
            format!("unexpected production delivery request sequence: {requests:?}"),
        )?;
        let transaction: serde_json::Value = serde_json::from_slice(
            &fs::read(out.join("delivery-transaction.json")).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        require(
            transaction["state"] == "receipts_persisted"
                && out.join("delivery-reconciliation.json").is_file()
                && out.join("delivery-reply-receipts.json").is_file(),
            "production delivery did not leave terminal receipts",
        )?;

        let fixed_args = fixture_run_args(temp.path(), &out, &fixture.fixed_head_sha)?;
        let fixed_diff = fixture_diff(&fixture, &fixture.fixed_head_sha);
        let fixed_plan = Plan {
            head: fixture.fixed_head_sha.clone(),
            ..plan.clone()
        };
        let fixed_surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "fixture-context-fixed",
            review_body_policy: &body_policy,
            run_pass: RunPass::Manual,
            post_review_on: &post_review_on,
            args: &fixed_args,
            plan: &fixed_plan,
            diff: &fixed_diff,
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[],
            summary_only_findings: &[],
            observations: &[],
            proof_receipts: &[],
            suggested_issues: &[],
            final_follow_up_tasks: 0,
            reporter_distillation: None,
        })
        .map_err(|error| error.to_string())?;
        require(
            !fixed_surface.should_prepare_github_review
                && fixed_surface.github_review.comments.is_empty()
                && fixed_surface.github_review.body.is_empty(),
            "fixed production head did not remain silent",
        )?;
        Ok(())
    }

    #[test]
    fn perl_lsp_3627_replay_deduplicates_public_claims_and_keeps_diagnostics_artifact_only()
    -> Result<(), String> {
        let fixture = fixture()?;
        let temp = tempfile::tempdir().map_err(|error| error.to_string())?;
        let out = temp.path().join("review");
        fs::create_dir_all(&out).map_err(|error| error.to_string())?;
        let reply_thread = fixture
            .threads
            .iter()
            .find(|thread| thread.id == "3558771748")
            .ok_or_else(|| "fixture reply thread is missing".to_owned())?;

        let material_claim = reply_thread.body.clone();
        let inline = fixture_comment(reply_thread, &material_claim);
        let summary = SummaryOnlyFinding {
            lane: "fixture".to_owned(),
            severity: "high".to_owned(),
            confidence: "high".to_owned(),
            reason: material_claim.clone(),
            evidence: "duplicate summary rendering of the inline claim".to_owned(),
        };
        let homework_summary = SummaryOnlyFinding {
            lane: "proof-planner".to_owned(),
            severity: "low".to_owned(),
            confidence: "medium".to_owned(),
            reason: "Proof homework: rerun the unrelated workspace test before posting.".to_owned(),
            evidence: "skipped proof request retained for diagnostics".to_owned(),
        };
        let mut observations = production_observations(&fixture);
        observations
            .retain(|observation| observation.id == "fixture-parser:later-variable-subscript");
        let duplicate = observations
            .first()
            .cloned()
            .ok_or_else(|| "fixture material observation is missing".to_owned())?;
        observations.push(duplicate);
        let mut skipped_homework = observations
            .first()
            .cloned()
            .ok_or_else(|| "fixture material observation was removed".to_owned())?;
        skipped_homework.id = "fixture-skipped-proof-homework".to_owned();
        skipped_homework.kind = "test-gap".to_owned();
        skipped_homework.status = "open".to_owned();
        skipped_homework.claim =
            "Proof homework: rerun the unrelated workspace test before posting.".to_owned();
        skipped_homework.dedupe_key = "unrelated-proof-gap".to_owned();
        skipped_homework.fingerprint = "unrelated-proof-gap".to_owned();
        observations.push(skipped_homework);
        let skipped_receipt = ProofReceipt {
            revision: None,
            schema: "ub-review.proof_receipt.v1".to_owned(),
            id: "fixture-unrelated-skipped-proof".to_owned(),
            kind: "focused-test".to_owned(),
            base: fixture.base_sha.clone(),
            head: fixture.buggy_head_sha.clone(),
            test_patch_mode: "none".to_owned(),
            requested_by: vec!["unrelated-proof-gap".to_owned()],
            request_ids: vec!["unrelated-proof-gap".to_owned()],
            commands: vec![],
            result: "skipped".to_owned(),
            reason: "unrelated proof gap retained for diagnostics only".to_owned(),
        };
        let plan = fixture_plan(&fixture);
        let diff = fixture_diff(&fixture, &fixture.buggy_head_sha);
        let args = fixture_run_args(temp.path(), &out, &fixture.buggy_head_sha)?;
        let body_policy = ReviewBodyPolicy::default();
        let post_review_on = vec!["pull_request".to_owned()];
        let surface = compile_review_surface(ReviewCompilerInput {
            shared_context_id: "fixture-dedup-context",
            review_body_policy: &body_policy,
            run_pass: RunPass::Manual,
            post_review_on: &post_review_on,
            args: &args,
            plan: &plan,
            diff: &diff,
            model_lanes: &[],
            missing_or_failed_sensor_evidence: &[],
            missing_or_failed_model_evidence: &[],
            inline_comments: &[inline],
            summary_only_findings: &[summary, homework_summary],
            observations: &observations,
            proof_receipts: &[skipped_receipt],
            suggested_issues: &[],
            final_follow_up_tasks: 0,
            reporter_distillation: None,
        })
        .map_err(|error| error.to_string())?;

        let public_body = &surface.github_review.body;
        require(
            surface.github_review.comments.len() == 1,
            format!(
                "expected one retained inline claim, got {}",
                surface.github_review.comments.len()
            ),
        )?;
        let retained = surface
            .github_review
            .comments
            .first()
            .ok_or_else(|| "retained inline claim is missing".to_owned())?;
        require(
            retained.path == reply_thread.path
                && retained.line == reply_thread.anchor.unwrap_or_default()
                && retained.body.contains(&material_claim),
            "retained public claim identity did not preserve the validated inline anchor",
        )?;
        require(
            public_body.matches(&material_claim).count() == 1,
            format!("material claim was not retained exactly once: {public_body}"),
        )?;
        require(
            !public_body.contains("duplicate summary rendering")
                && !public_body.contains("Proof homework")
                && !public_body.contains("unrelated proof gap"),
            format!("internal duplicate/homework text reached public body: {public_body}"),
        )?;
        require(
            surface
                .artifact_body
                .contains("duplicate summary rendering")
                && surface.artifact_body.contains("Proof homework"),
            format!(
                "artifact body did not retain diagnostic-only replay inputs: {}",
                surface.artifact_body
            ),
        )?;
        Ok(())
    }
}
