//! Pure contract types for transactional review delivery.
//!
//! This module deliberately has no GitHub, filesystem, process, or network
//! access. It gives the later posting adapter one exact contract for planned
//! public items, comments returned by GitHub, confirmed delivery receipts, and
//! lifecycle/cleanup states around them.

use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const DELIVERY_TRANSACTION_SCHEMA: &str = "ub-review.delivery_transaction.v1";
pub const DELIVERY_RECEIPT_SCHEMA: &str = "ub-review.delivery_receipt.v1";

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryAction {
    Inline,
    Reply,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct PlannedDelivery {
    exact_head_sha: String,
    claim_id: String,
    action: DeliveryAction,
    path: String,
    line: u32,
    side: String,
    source_thread_id: Option<String>,
    expected_body_digest: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ObservedDelivery {
    comment_id: String,
    delivery: PlannedDelivery,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeliveryReceipt {
    schema: String,
    exact_head_sha: String,
    claim_id: String,
    action: DeliveryAction,
    path: String,
    line: u32,
    side: String,
    source_thread_id: Option<String>,
    expected_body_digest: String,
    review_id: String,
    comment_id: String,
    confirmed_head_sha: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeliveryReconciliation {
    schema: String,
    exact_head_sha: String,
    planned_count: usize,
    observed_count: usize,
    receipts: Vec<DeliveryReceipt>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryTransactionState {
    Planned,
    PendingReviewCreated,
    CommentsCreated,
    CommentsReconciled,
    HeadRevalidated,
    Submitted,
    ReceiptsPersisted,
    CleanupAttempted,
    CleanedUp,
    Failed,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryFailureStage {
    PendingReviewCreation,
    CommentCreation,
    CommentReconciliation,
    HeadRevalidation,
    Submission,
    ReceiptSerialization,
    ReceiptPersistence,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case", tag = "status", content = "reason")]
pub enum CleanupOutcome {
    NotAttempted,
    Succeeded,
    Failed(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeliveryFailure {
    stage: DeliveryFailureStage,
    reason: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct DeliveryTransaction {
    schema: String,
    exact_head_sha: String,
    planned: Vec<PlannedDelivery>,
    state: DeliveryTransactionState,
    failure: Option<DeliveryFailure>,
    cleanup: CleanupOutcome,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct DeliveryIdentity {
    exact_head_sha: String,
    claim_id: String,
    action: DeliveryAction,
    path: String,
    line: u32,
    side: String,
    source_thread_id: Option<String>,
    expected_body_digest: String,
}

impl From<&PlannedDelivery> for DeliveryIdentity {
    fn from(delivery: &PlannedDelivery) -> Self {
        Self {
            exact_head_sha: delivery.exact_head_sha.clone(),
            claim_id: delivery.claim_id.clone(),
            action: delivery.action.clone(),
            path: delivery.path.clone(),
            line: delivery.line,
            side: delivery.side.clone(),
            source_thread_id: delivery.source_thread_id.clone(),
            expected_body_digest: delivery.expected_body_digest.clone(),
        }
    }
}

impl From<&ObservedDelivery> for DeliveryIdentity {
    fn from(delivery: &ObservedDelivery) -> Self {
        DeliveryIdentity::from(&delivery.delivery)
    }
}

impl DeliveryTransaction {
    pub fn new(exact_head_sha: impl Into<String>, planned: Vec<PlannedDelivery>) -> Result<Self> {
        let exact_head_sha = exact_head_sha.into();
        validate_head_sha(&exact_head_sha)?;
        let planned = canonical_planned_deliveries(&exact_head_sha, planned)?;
        Ok(Self {
            schema: DELIVERY_TRANSACTION_SCHEMA.to_owned(),
            exact_head_sha,
            planned,
            state: DeliveryTransactionState::Planned,
            failure: None,
            cleanup: CleanupOutcome::NotAttempted,
        })
    }

    pub fn transition(&mut self, next: DeliveryTransactionState) -> Result<()> {
        ensure!(
            legal_transition(&self.state, &next),
            "illegal delivery transaction transition: {:?} -> {:?}",
            self.state,
            next
        );
        self.state = next;
        Ok(())
    }

    pub fn record_failure(
        &mut self,
        stage: DeliveryFailureStage,
        reason: impl Into<String>,
        cleanup_attempted: bool,
    ) -> Result<()> {
        let cleanup_required = !matches!(self.state, DeliveryTransactionState::Planned);
        ensure!(
            !cleanup_required || cleanup_attempted,
            "failure after pending-review creation requires cleanup attempt"
        );
        ensure!(
            !matches!(
                self.state,
                DeliveryTransactionState::ReceiptsPersisted
                    | DeliveryTransactionState::CleanedUp
                    | DeliveryTransactionState::Failed
            ),
            "cannot record failure after transaction is terminal: {:?}",
            self.state
        );
        self.failure = Some(DeliveryFailure {
            stage,
            reason: reason.into(),
        });
        if cleanup_attempted {
            self.transition(DeliveryTransactionState::CleanupAttempted)?;
        } else {
            self.state = DeliveryTransactionState::Failed;
        }
        Ok(())
    }

    pub fn finish_cleanup(&mut self, outcome: CleanupOutcome) -> Result<()> {
        ensure!(
            self.state == DeliveryTransactionState::CleanupAttempted,
            "cleanup can only finish after cleanup was attempted, current state: {:?}",
            self.state
        );
        ensure!(
            !matches!(outcome, CleanupOutcome::NotAttempted),
            "cleanup attempt must record success or failure"
        );
        self.cleanup = outcome;
        self.state = if matches!(self.cleanup, CleanupOutcome::Succeeded) {
            DeliveryTransactionState::CleanedUp
        } else {
            DeliveryTransactionState::Failed
        };
        Ok(())
    }
}

pub fn reconcile_deliveries(
    exact_head_sha: &str,
    review_id: &str,
    planned: &[PlannedDelivery],
    observed: &[ObservedDelivery],
) -> Result<DeliveryReconciliation> {
    validate_head_sha(exact_head_sha)?;
    validate_identifier(review_id, "review id")?;
    let canonical_planned = canonical_planned_deliveries(exact_head_sha, planned.to_vec())?;
    let mut observed_by_identity = BTreeMap::new();
    let mut comment_ids = BTreeSet::new();
    for item in observed {
        validate_observed_delivery(exact_head_sha, item)?;
        ensure!(
            comment_ids.insert(item.comment_id.clone()),
            "duplicate GitHub comment id in returned delivery set: {}",
            item.comment_id
        );
        ensure!(
            observed_by_identity
                .insert(DeliveryIdentity::from(item), item)
                .is_none(),
            "duplicate planned delivery identity in returned delivery set"
        );
    }

    let planned_by_identity = canonical_planned
        .iter()
        .map(|item| (DeliveryIdentity::from(item), item))
        .collect::<BTreeMap<_, _>>();
    let missing = planned_by_identity
        .keys()
        .filter(|identity| !observed_by_identity.contains_key(*identity))
        .map(identity_label)
        .collect::<Vec<_>>();
    let unexpected = observed_by_identity
        .keys()
        .filter(|identity| !planned_by_identity.contains_key(*identity))
        .map(identity_label)
        .collect::<Vec<_>>();
    ensure!(
        missing.is_empty() && unexpected.is_empty(),
        "GitHub delivery set mismatch; missing={missing:?}, unexpected={unexpected:?}"
    );

    let receipts = planned_by_identity
        .keys()
        .map(|identity| {
            let observed = observed_by_identity.get(identity).ok_or_else(|| {
                anyhow::anyhow!("reconciled delivery disappeared while building receipt")
            })?;
            Ok(delivery_receipt(review_id, exact_head_sha, observed))
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(DeliveryReconciliation {
        schema: DELIVERY_RECEIPT_SCHEMA.to_owned(),
        exact_head_sha: exact_head_sha.to_owned(),
        planned_count: planned.len(),
        observed_count: observed.len(),
        receipts,
    })
}

fn delivery_receipt(
    review_id: &str,
    exact_head_sha: &str,
    observed: &ObservedDelivery,
) -> DeliveryReceipt {
    let delivery = &observed.delivery;
    DeliveryReceipt {
        schema: DELIVERY_RECEIPT_SCHEMA.to_owned(),
        exact_head_sha: exact_head_sha.to_owned(),
        claim_id: delivery.claim_id.clone(),
        action: delivery.action.clone(),
        path: delivery.path.clone(),
        line: delivery.line,
        side: delivery.side.clone(),
        source_thread_id: delivery.source_thread_id.clone(),
        expected_body_digest: delivery.expected_body_digest.clone(),
        review_id: review_id.to_owned(),
        comment_id: observed.comment_id.clone(),
        confirmed_head_sha: delivery.exact_head_sha.clone(),
    }
}

fn canonical_planned_deliveries(
    exact_head_sha: &str,
    mut planned: Vec<PlannedDelivery>,
) -> Result<Vec<PlannedDelivery>> {
    let mut identities = BTreeSet::new();
    for item in &planned {
        validate_planned_delivery(exact_head_sha, item)?;
        ensure!(
            identities.insert(DeliveryIdentity::from(item)),
            "duplicate planned delivery identity for claim {}",
            item.claim_id
        );
    }
    planned.sort_by_key(|item| DeliveryIdentity::from(item));
    Ok(planned)
}

fn validate_planned_delivery(expected_head: &str, item: &PlannedDelivery) -> Result<()> {
    validate_head_sha(expected_head)?;
    ensure!(
        item.exact_head_sha == expected_head,
        "planned delivery claim {} is bound to head {}, expected {}",
        item.claim_id,
        item.exact_head_sha,
        expected_head
    );
    validate_identifier(&item.claim_id, "claim id")?;
    validate_identifier(&item.path, "comment path")?;
    ensure!(item.line > 0, "comment line must be positive");
    ensure!(
        matches!(item.side.as_str(), "LEFT" | "RIGHT"),
        "comment side must be LEFT or RIGHT, got {}",
        item.side
    );
    validate_identifier(&item.expected_body_digest, "expected body digest")?;
    match item.action {
        DeliveryAction::Inline => ensure!(
            item.source_thread_id.is_none(),
            "inline delivery must not carry a source thread id"
        ),
        DeliveryAction::Reply => {
            let source_thread_id = item
                .source_thread_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("reply delivery requires a source thread id"))?;
            validate_identifier(source_thread_id, "source thread id")?;
        }
    }
    Ok(())
}

fn validate_observed_delivery(expected_head: &str, item: &ObservedDelivery) -> Result<()> {
    validate_identifier(&item.comment_id, "GitHub comment id")?;
    validate_planned_delivery(expected_head, &item.delivery)
}

fn validate_head_sha(value: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "exact head SHA must be non-empty");
    Ok(())
}

fn validate_identifier(value: &str, label: &str) -> Result<()> {
    ensure!(!value.trim().is_empty(), "{label} must be non-empty");
    ensure!(
        !value.chars().any(char::is_control),
        "{label} must not contain control characters"
    );
    Ok(())
}

fn identity_label(identity: &DeliveryIdentity) -> String {
    format!(
        "{}:{}:{}:{}:{}:{}",
        identity.claim_id,
        identity.action.as_str(),
        identity.path,
        identity.line,
        identity.side,
        identity.source_thread_id.as_deref().unwrap_or("-")
    )
}

impl DeliveryAction {
    fn as_str(&self) -> &'static str {
        match self {
            Self::Inline => "inline",
            Self::Reply => "reply",
        }
    }
}

fn legal_transition(current: &DeliveryTransactionState, next: &DeliveryTransactionState) -> bool {
    matches!(
        (current, next),
        (
            DeliveryTransactionState::Planned,
            DeliveryTransactionState::PendingReviewCreated
        ) | (
            DeliveryTransactionState::PendingReviewCreated,
            DeliveryTransactionState::CommentsCreated
        ) | (
            DeliveryTransactionState::CommentsCreated,
            DeliveryTransactionState::CommentsReconciled
        ) | (
            DeliveryTransactionState::CommentsReconciled,
            DeliveryTransactionState::HeadRevalidated
        ) | (
            DeliveryTransactionState::HeadRevalidated,
            DeliveryTransactionState::Submitted
        ) | (
            DeliveryTransactionState::Submitted,
            DeliveryTransactionState::ReceiptsPersisted
        ) | (
            DeliveryTransactionState::PendingReviewCreated,
            DeliveryTransactionState::CleanupAttempted
        ) | (
            DeliveryTransactionState::CommentsCreated,
            DeliveryTransactionState::CleanupAttempted
        ) | (
            DeliveryTransactionState::CommentsReconciled,
            DeliveryTransactionState::CleanupAttempted
        ) | (
            DeliveryTransactionState::HeadRevalidated,
            DeliveryTransactionState::CleanupAttempted
        ) | (
            DeliveryTransactionState::Submitted,
            DeliveryTransactionState::CleanupAttempted
        )
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Result;

    const HEAD: &str = "0123456789abcdef0123456789abcdef01234567";

    fn inline(claim_id: &str, path: &str, line: u32) -> PlannedDelivery {
        PlannedDelivery {
            exact_head_sha: HEAD.to_owned(),
            claim_id: claim_id.to_owned(),
            action: DeliveryAction::Inline,
            path: path.to_owned(),
            line,
            side: "RIGHT".to_owned(),
            source_thread_id: None,
            expected_body_digest: format!("digest-{claim_id}"),
        }
    }

    fn reply(claim_id: &str, thread_id: &str) -> PlannedDelivery {
        PlannedDelivery {
            exact_head_sha: HEAD.to_owned(),
            claim_id: claim_id.to_owned(),
            action: DeliveryAction::Reply,
            path: "src/lib.rs".to_owned(),
            line: 12,
            side: "RIGHT".to_owned(),
            source_thread_id: Some(thread_id.to_owned()),
            expected_body_digest: format!("digest-{claim_id}"),
        }
    }

    fn observed(comment_id: &str, delivery: PlannedDelivery) -> ObservedDelivery {
        ObservedDelivery {
            comment_id: comment_id.to_owned(),
            delivery,
        }
    }

    fn require_error<T>(result: Result<T>, context: &str) -> anyhow::Error {
        match result {
            Ok(_) => anyhow::anyhow!("{context}"),
            Err(error) => error,
        }
    }

    #[test]
    fn exact_reconciliation_returns_sorted_confirmed_receipts() -> Result<()> {
        let first = inline("claim-b", "src/b.rs", 8);
        let second = reply("claim-a", "thread-7");
        let result = reconcile_deliveries(
            HEAD,
            "review-42",
            &[first.clone(), second.clone()],
            &[observed("comment-b", first), observed("comment-a", second)],
        )?;

        assert_eq!(result.schema, DELIVERY_RECEIPT_SCHEMA);
        assert_eq!(result.planned_count, 2);
        assert_eq!(result.observed_count, 2);
        assert_eq!(
            result
                .receipts
                .iter()
                .map(|receipt| receipt.claim_id.as_str())
                .collect::<Vec<_>>(),
            vec!["claim-a", "claim-b"]
        );
        assert_eq!(
            result.receipts[0].source_thread_id.as_deref(),
            Some("thread-7")
        );
        assert_eq!(result.receipts[0].confirmed_head_sha, HEAD);
        Ok(())
    }

    #[test]
    fn reconciliation_rejects_partial_returned_set() {
        let first = inline("claim-a", "src/a.rs", 4);
        let second = inline("claim-b", "src/b.rs", 5);
        let error = require_error(
            reconcile_deliveries(
                HEAD,
                "review-42",
                &[first.clone(), second],
                &[observed("comment-a", first)],
            ),
            "partial returned set must not confirm delivery",
        );
        assert!(error.to_string().contains("missing"));
    }

    #[test]
    fn reconciliation_rejects_duplicate_or_malformed_comment_ids() {
        let first = inline("claim-a", "src/a.rs", 4);
        let duplicate = observed("comment-a", first.clone());
        let error = require_error(
            reconcile_deliveries(
                HEAD,
                "review-42",
                &[first.clone(), first.clone()],
                &[duplicate.clone(), duplicate],
            ),
            "duplicate planned and returned identities must fail",
        );
        assert!(error.to_string().contains("duplicate"));

        let error = require_error(
            reconcile_deliveries(
                HEAD,
                "review-42",
                std::slice::from_ref(&first),
                &[observed("", first.clone())],
            ),
            "empty returned IDs must fail",
        );
        assert!(error.to_string().contains("GitHub comment id"));
    }

    #[test]
    fn reconciliation_rejects_wrong_identity_or_head() {
        let planned = inline("claim-a", "src/a.rs", 4);
        let mut wrong_claim = planned.clone();
        wrong_claim.claim_id = "claim-other".to_owned();
        let error = require_error(
            reconcile_deliveries(
                HEAD,
                "review-42",
                std::slice::from_ref(&planned),
                &[observed("comment-a", wrong_claim)],
            ),
            "wrong claim cannot confirm delivery",
        );
        assert!(error.to_string().contains("unexpected"));

        let mut wrong_head = planned.clone();
        wrong_head.exact_head_sha = "fedcba9876543210fedcba9876543210fedcba98".to_owned();
        let error = require_error(
            reconcile_deliveries(
                HEAD,
                "review-42",
                std::slice::from_ref(&planned),
                &[observed("comment-a", wrong_head)],
            ),
            "wrong head cannot confirm delivery",
        );
        assert!(error.to_string().contains("bound to head"));
    }

    #[test]
    fn reconciliation_binds_path_line_side_and_body_digest() {
        let planned = inline("claim-a", "src/a.rs", 4);
        type Mutator = fn(&mut PlannedDelivery);
        let cases: [(&str, Mutator); 4] = [
            ("path", |delivery: &mut PlannedDelivery| {
                delivery.path = "src/other.rs".to_owned();
            }),
            ("line", |delivery: &mut PlannedDelivery| {
                delivery.line = 5;
            }),
            ("side", |delivery: &mut PlannedDelivery| {
                delivery.side = "LEFT".to_owned();
            }),
            ("body digest", |delivery: &mut PlannedDelivery| {
                delivery.expected_body_digest = "digest-other".to_owned();
            }),
        ];
        for (label, mutate) in cases {
            let mut mismatched = planned.clone();
            mutate(&mut mismatched);
            let error = require_error(
                reconcile_deliveries(
                    HEAD,
                    "review-42",
                    std::slice::from_ref(&planned),
                    &[observed("comment-a", mismatched)],
                ),
                "mismatched delivery identity must fail",
            );
            assert!(
                error.to_string().contains("unexpected"),
                "{label} mismatch should be reported as unexpected: {error:#}"
            );
        }
    }

    #[test]
    fn reply_and_inline_thread_identity_rules_are_fail_closed() {
        let mut missing_thread = reply("claim-a", "thread-7");
        missing_thread.source_thread_id = None;
        let error = require_error(
            DeliveryTransaction::new(HEAD, vec![missing_thread]),
            "reply without a source thread must fail",
        );
        assert!(error.to_string().contains("source thread"));

        let mut inline_with_thread = inline("claim-a", "src/a.rs", 4);
        inline_with_thread.source_thread_id = Some("thread-7".to_owned());
        let error = require_error(
            DeliveryTransaction::new(HEAD, vec![inline_with_thread]),
            "inline with a source thread must fail",
        );
        assert!(error.to_string().contains("must not carry"));
    }

    #[test]
    fn transaction_state_machine_requires_exact_order_and_receipts() -> Result<()> {
        let mut transaction =
            DeliveryTransaction::new(HEAD, vec![inline("claim-a", "src/a.rs", 4)])?;
        transaction.transition(DeliveryTransactionState::PendingReviewCreated)?;
        transaction.transition(DeliveryTransactionState::CommentsCreated)?;
        transaction.transition(DeliveryTransactionState::CommentsReconciled)?;
        transaction.transition(DeliveryTransactionState::HeadRevalidated)?;
        transaction.transition(DeliveryTransactionState::Submitted)?;
        transaction.transition(DeliveryTransactionState::ReceiptsPersisted)?;
        assert_eq!(transaction.cleanup, CleanupOutcome::NotAttempted);

        let error = require_error(
            transaction.transition(DeliveryTransactionState::CleanedUp),
            "persisted transaction cannot jump to cleanup",
        );
        assert!(error.to_string().contains("illegal"));
        Ok(())
    }

    #[test]
    fn failure_after_creation_records_cleanup_success_or_failure() -> Result<()> {
        let mut cleaned = DeliveryTransaction::new(HEAD, vec![inline("claim-a", "src/a.rs", 4)])?;
        cleaned.transition(DeliveryTransactionState::PendingReviewCreated)?;
        cleaned.record_failure(
            DeliveryFailureStage::CommentReconciliation,
            "returned comment set was partial",
            true,
        )?;
        cleaned.finish_cleanup(CleanupOutcome::Succeeded)?;
        assert_eq!(cleaned.state, DeliveryTransactionState::CleanedUp);
        assert_eq!(cleaned.cleanup, CleanupOutcome::Succeeded);
        assert_eq!(
            cleaned.failure.as_ref().map(|failure| &failure.stage),
            Some(&DeliveryFailureStage::CommentReconciliation)
        );

        let mut failed = DeliveryTransaction::new(HEAD, vec![inline("claim-a", "src/a.rs", 4)])?;
        failed.transition(DeliveryTransactionState::PendingReviewCreated)?;
        failed.record_failure(DeliveryFailureStage::Submission, "submit rejected", true)?;
        failed.finish_cleanup(CleanupOutcome::Failed("delete rejected".to_owned()))?;
        assert_eq!(failed.state, DeliveryTransactionState::Failed);
        assert!(matches!(failed.cleanup, CleanupOutcome::Failed(_)));

        let mut missing_cleanup =
            DeliveryTransaction::new(HEAD, vec![inline("claim-a", "src/a.rs", 4)])?;
        missing_cleanup.transition(DeliveryTransactionState::PendingReviewCreated)?;
        let error = require_error(
            missing_cleanup.record_failure(
                DeliveryFailureStage::Submission,
                "submit rejected",
                false,
            ),
            "post-creation failure without cleanup must fail closed",
        );
        assert!(error.to_string().contains("requires cleanup"));
        Ok(())
    }

    #[test]
    fn transaction_serialization_preserves_contract_fields() -> Result<()> {
        let transaction = DeliveryTransaction::new(HEAD, vec![reply("claim-a", "thread-7")])?;
        let value = serde_json::to_value(transaction)?;
        assert_eq!(value["schema"], DELIVERY_TRANSACTION_SCHEMA);
        assert_eq!(value["planned"][0]["action"], "reply");
        assert_eq!(value["planned"][0]["source_thread_id"], "thread-7");
        assert_eq!(value["state"], "planned");
        assert_eq!(value["cleanup"]["status"], "not_attempted");
        Ok(())
    }
}
