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
    ) -> PrThreadContext {
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
                .map(|thread| ReviewThreadRecord {
                    id: thread.id.clone(),
                    kind: "review-comment".to_owned(),
                    author: thread.author.clone(),
                    body: thread.body.clone(),
                    path: (!thread.path.is_empty()).then(|| thread.path.clone()),
                    line: thread.anchor,
                    commit_id: Some(thread_commit.to_owned()),
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
            &production_thread_context(&fixture, &fixture.buggy_head_sha),
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
            &production_thread_context(&fixture, &fixture.buggy_head_sha),
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
}
