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
pub struct DeliveryLocation {
    path: String,
    line: u32,
    side: String,
}

impl DeliveryLocation {
    pub fn new(path: impl Into<String>, line: u32, side: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            line,
            side: side.into(),
        }
    }
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

impl PlannedDelivery {
    pub fn new(
        exact_head_sha: impl Into<String>,
        claim_id: impl Into<String>,
        action: DeliveryAction,
        location: DeliveryLocation,
        source_thread_id: Option<String>,
        expected_body_digest: impl Into<String>,
    ) -> Result<Self> {
        let delivery = Self {
            exact_head_sha: exact_head_sha.into(),
            claim_id: claim_id.into(),
            action,
            path: location.path,
            line: location.line,
            side: location.side,
            source_thread_id,
            expected_body_digest: expected_body_digest.into(),
        };
        validate_planned_delivery(&delivery.exact_head_sha, &delivery)?;
        Ok(delivery)
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ObservedDelivery {
    comment_id: String,
    delivery: PlannedDelivery,
}

impl ObservedDelivery {
    pub fn new(comment_id: impl Into<String>, delivery: PlannedDelivery) -> Result<Self> {
        let observed = Self {
            comment_id: comment_id.into(),
            delivery,
        };
        validate_observed_delivery(&observed.delivery.exact_head_sha, &observed)?;
        Ok(observed)
    }
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
    use serde_json::Value;

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

    #[test]
    fn public_constructors_validate_and_preserve_delivery_identity() -> Result<()> {
        let planned = PlannedDelivery::new(
            HEAD,
            "claim-a",
            DeliveryAction::Reply,
            DeliveryLocation::new("src/lib.rs", 12, "RIGHT"),
            Some("thread-7".to_owned()),
            "digest-a",
        )?;
        assert_eq!(planned.exact_head_sha, HEAD);
        assert_eq!(planned.claim_id, "claim-a");
        assert_eq!(planned.action, DeliveryAction::Reply);
        assert_eq!(planned.path, "src/lib.rs");
        assert_eq!(planned.line, 12);
        assert_eq!(planned.side, "RIGHT");
        assert_eq!(planned.source_thread_id.as_deref(), Some("thread-7"));
        assert_eq!(planned.expected_body_digest, "digest-a");

        let observed = ObservedDelivery::new("comment-1", planned.clone())?;
        assert_eq!(observed.comment_id, "comment-1");
        assert_eq!(observed.delivery, planned);
        Ok(())
    }

    #[test]
    fn transaction_constructor_returns_the_complete_initial_contract() -> Result<()> {
        let transaction = DeliveryTransaction::new(HEAD, vec![])?;
        assert_eq!(
            transaction,
            DeliveryTransaction {
                schema: DELIVERY_TRANSACTION_SCHEMA.to_owned(),
                exact_head_sha: HEAD.to_owned(),
                planned: vec![],
                state: DeliveryTransactionState::Planned,
                failure: None,
                cleanup: CleanupOutcome::NotAttempted,
            }
        );
        let transaction = DeliveryTransaction::new(HEAD, vec![])?;
        assert_eq!(transaction.schema, DELIVERY_TRANSACTION_SCHEMA);
        assert_eq!(transaction.exact_head_sha, HEAD);
        assert!(transaction.planned.is_empty());
        assert_eq!(transaction.state, DeliveryTransactionState::Planned);
        assert_eq!(transaction.failure, None);
        assert_eq!(transaction.cleanup, CleanupOutcome::NotAttempted);
        Ok(())
    }

    #[test]
    fn delivery_identity_labels_are_stable_for_inline_and_reply() {
        let inline_identity = DeliveryIdentity::from(&inline("claim-a", "src/a.rs", 4));
        assert_eq!(
            identity_label(&inline_identity),
            "claim-a:inline:src/a.rs:4:RIGHT:-"
        );
        let reply_identity = DeliveryIdentity::from(&reply("claim-b", "thread-7"));
        assert_eq!(
            identity_label(&reply_identity),
            "claim-b:reply:src/lib.rs:12:RIGHT:thread-7"
        );
    }

    #[test]
    fn delivery_validation_rejects_each_invalid_identity_component() {
        type Mutator = fn(&mut PlannedDelivery);
        let cases: [(&str, Mutator, &str); 6] = [
            (
                "claim",
                |delivery| delivery.claim_id.clear(),
                "claim id must be non-empty",
            ),
            (
                "path",
                |delivery| delivery.path.clear(),
                "comment path must be non-empty",
            ),
            (
                "line",
                |delivery| delivery.line = 0,
                "comment line must be positive",
            ),
            (
                "side",
                |delivery| delivery.side = "BOTH".to_owned(),
                "comment side must be LEFT or RIGHT, got BOTH",
            ),
            (
                "digest",
                |delivery| delivery.expected_body_digest.clear(),
                "expected body digest must be non-empty",
            ),
            (
                "control",
                |delivery| delivery.claim_id = "claim\n-a".to_owned(),
                "claim id must not contain control characters",
            ),
        ];
        for (label, mutate, expected_error) in cases {
            let mut invalid = inline("claim-a", "src/a.rs", 4);
            mutate(&mut invalid);
            let error = require_error(
                DeliveryTransaction::new(HEAD, vec![invalid]),
                "invalid delivery identity must fail",
            );
            assert_eq!(
                error.to_string(),
                expected_error,
                "{label} error contract changed"
            );
        }

        let error = require_error(DeliveryTransaction::new("", vec![]), "empty head must fail");
        assert_eq!(error.to_string(), "exact head SHA must be non-empty");

        let error = require_error(
            DeliveryTransaction::new(
                HEAD,
                vec![PlannedDelivery {
                    exact_head_sha: "other-head".to_owned(),
                    ..inline("claim-a", "src/a.rs", 4)
                }],
            ),
            "wrong head must fail",
        );
        assert!(error.to_string().contains("bound to head"));

        let error = require_error(
            ObservedDelivery::new("", inline("claim-a", "src/a.rs", 4)),
            "empty comment id must fail",
        );
        assert_eq!(error.to_string(), "GitHub comment id must be non-empty");
    }

    fn observed(comment_id: &str, delivery: PlannedDelivery) -> ObservedDelivery {
        ObservedDelivery {
            comment_id: comment_id.to_owned(),
            delivery,
        }
    }

    fn require_error<T>(result: Result<T>, context: &str) -> anyhow::Error {
        result.err().unwrap_or_else(|| anyhow::anyhow!("{context}"))
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
        assert_eq!(result.receipts[0].schema, DELIVERY_RECEIPT_SCHEMA);
        assert_eq!(result.receipts[0].claim_id, "claim-a");
        assert_eq!(result.receipts[0].action, DeliveryAction::Reply);
        assert_eq!(result.receipts[0].path, "src/lib.rs");
        assert_eq!(result.receipts[0].line, 12);
        assert_eq!(result.receipts[0].side, "RIGHT");
        assert_eq!(result.receipts[0].expected_body_digest, "digest-claim-a");
        assert_eq!(result.receipts[0].review_id, "review-42");
        assert_eq!(result.receipts[0].comment_id, "comment-a");
        assert_eq!(
            result,
            DeliveryReconciliation {
                schema: DELIVERY_RECEIPT_SCHEMA.to_owned(),
                exact_head_sha: HEAD.to_owned(),
                planned_count: 2,
                observed_count: 2,
                receipts: vec![
                    DeliveryReceipt {
                        schema: DELIVERY_RECEIPT_SCHEMA.to_owned(),
                        exact_head_sha: HEAD.to_owned(),
                        claim_id: "claim-a".to_owned(),
                        action: DeliveryAction::Reply,
                        path: "src/lib.rs".to_owned(),
                        line: 12,
                        side: "RIGHT".to_owned(),
                        source_thread_id: Some("thread-7".to_owned()),
                        expected_body_digest: "digest-claim-a".to_owned(),
                        review_id: "review-42".to_owned(),
                        comment_id: "comment-a".to_owned(),
                        confirmed_head_sha: HEAD.to_owned(),
                    },
                    DeliveryReceipt {
                        schema: DELIVERY_RECEIPT_SCHEMA.to_owned(),
                        exact_head_sha: HEAD.to_owned(),
                        claim_id: "claim-b".to_owned(),
                        action: DeliveryAction::Inline,
                        path: "src/b.rs".to_owned(),
                        line: 8,
                        side: "RIGHT".to_owned(),
                        source_thread_id: None,
                        expected_body_digest: "digest-claim-b".to_owned(),
                        review_id: "review-42".to_owned(),
                        comment_id: "comment-b".to_owned(),
                        confirmed_head_sha: HEAD.to_owned(),
                    },
                ],
            }
        );
        assert_eq!(
            serde_json::to_value(&result)?,
            serde_json::json!({
                "schema": DELIVERY_RECEIPT_SCHEMA,
                "exact_head_sha": HEAD,
                "planned_count": 2,
                "observed_count": 2,
                "receipts": [
                    {
                        "schema": DELIVERY_RECEIPT_SCHEMA,
                        "exact_head_sha": HEAD,
                        "claim_id": "claim-a",
                        "action": "reply",
                        "path": "src/lib.rs",
                        "line": 12,
                        "side": "RIGHT",
                        "source_thread_id": "thread-7",
                        "expected_body_digest": "digest-claim-a",
                        "review_id": "review-42",
                        "comment_id": "comment-a",
                        "confirmed_head_sha": HEAD,
                    },
                    {
                        "schema": DELIVERY_RECEIPT_SCHEMA,
                        "exact_head_sha": HEAD,
                        "claim_id": "claim-b",
                        "action": "inline",
                        "path": "src/b.rs",
                        "line": 8,
                        "side": "RIGHT",
                        "source_thread_id": Value::Null,
                        "expected_body_digest": "digest-claim-b",
                        "review_id": "review-42",
                        "comment_id": "comment-b",
                        "confirmed_head_sha": HEAD,
                    },
                ],
            })
        );
        Ok(())
    }

    #[test]
    fn empty_reconciliation_is_a_deterministic_empty_receipt_set() -> Result<()> {
        let result = reconcile_deliveries(HEAD, "review-42", &[], &[])?;
        assert_eq!(result.schema, DELIVERY_RECEIPT_SCHEMA);
        assert_eq!(result.exact_head_sha, HEAD);
        assert_eq!(result.planned_count, 0);
        assert_eq!(result.observed_count, 0);
        assert!(result.receipts.is_empty());
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
        assert_eq!(
            error.to_string(),
            "GitHub delivery set mismatch; missing=[\"claim-b:inline:src/b.rs:5:RIGHT:-\"], unexpected=[]"
        );
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
        assert_eq!(
            error.to_string(),
            "GitHub delivery set mismatch; missing=[\"claim-a:inline:src/a.rs:4:RIGHT:-\"], unexpected=[\"claim-other:inline:src/a.rs:4:RIGHT:-\"]"
        );

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
        assert_eq!(
            error.to_string(),
            "planned delivery claim claim-a is bound to head fedcba9876543210fedcba9876543210fedcba98, expected 0123456789abcdef0123456789abcdef01234567"
        );
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
        assert_eq!(
            error.to_string(),
            "reply delivery requires a source thread id"
        );

        let mut inline_with_thread = inline("claim-a", "src/a.rs", 4);
        inline_with_thread.source_thread_id = Some("thread-7".to_owned());
        let error = require_error(
            DeliveryTransaction::new(HEAD, vec![inline_with_thread]),
            "inline with a source thread must fail",
        );
        assert_eq!(
            error.to_string(),
            "inline delivery must not carry a source thread id"
        );
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
    fn legal_transition_matrix_covers_success_and_cleanup_paths() {
        let legal = [
            (
                DeliveryTransactionState::Planned,
                DeliveryTransactionState::PendingReviewCreated,
            ),
            (
                DeliveryTransactionState::PendingReviewCreated,
                DeliveryTransactionState::CommentsCreated,
            ),
            (
                DeliveryTransactionState::CommentsCreated,
                DeliveryTransactionState::CommentsReconciled,
            ),
            (
                DeliveryTransactionState::CommentsReconciled,
                DeliveryTransactionState::HeadRevalidated,
            ),
            (
                DeliveryTransactionState::HeadRevalidated,
                DeliveryTransactionState::Submitted,
            ),
            (
                DeliveryTransactionState::Submitted,
                DeliveryTransactionState::ReceiptsPersisted,
            ),
            (
                DeliveryTransactionState::PendingReviewCreated,
                DeliveryTransactionState::CleanupAttempted,
            ),
            (
                DeliveryTransactionState::CommentsCreated,
                DeliveryTransactionState::CleanupAttempted,
            ),
            (
                DeliveryTransactionState::CommentsReconciled,
                DeliveryTransactionState::CleanupAttempted,
            ),
            (
                DeliveryTransactionState::HeadRevalidated,
                DeliveryTransactionState::CleanupAttempted,
            ),
            (
                DeliveryTransactionState::Submitted,
                DeliveryTransactionState::CleanupAttempted,
            ),
        ];
        for (current, next) in legal {
            assert!(legal_transition(&current, &next), "{current:?} -> {next:?}");
        }
        for (current, next) in [
            (
                DeliveryTransactionState::Planned,
                DeliveryTransactionState::CommentsCreated,
            ),
            (
                DeliveryTransactionState::ReceiptsPersisted,
                DeliveryTransactionState::CleanupAttempted,
            ),
            (
                DeliveryTransactionState::CleanedUp,
                DeliveryTransactionState::Failed,
            ),
        ] {
            assert!(
                !legal_transition(&current, &next),
                "{current:?} -> {next:?}"
            );
        }
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
        assert_eq!(
            serde_json::to_value(&failed)?,
            serde_json::json!({
                "schema": DELIVERY_TRANSACTION_SCHEMA,
                "exact_head_sha": HEAD,
                "planned": [{
                    "exact_head_sha": HEAD,
                    "claim_id": "claim-a",
                    "action": "inline",
                    "path": "src/a.rs",
                    "line": 4,
                    "side": "RIGHT",
                    "source_thread_id": Value::Null,
                    "expected_body_digest": "digest-claim-a",
                }],
                "state": "failed",
                "failure": {
                    "stage": "submission",
                    "reason": "submit rejected",
                },
                "cleanup": {
                    "status": "failed",
                    "reason": "delete rejected",
                },
            })
        );

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
    fn failure_and_cleanup_guards_reject_terminal_or_unstarted_states() -> Result<()> {
        let mut planned = DeliveryTransaction::new(HEAD, vec![])?;
        let error = require_error(
            planned.finish_cleanup(CleanupOutcome::Succeeded),
            "cleanup before an attempt must fail",
        );
        assert!(
            error
                .to_string()
                .contains("only finish after cleanup was attempted")
        );

        planned.transition(DeliveryTransactionState::PendingReviewCreated)?;
        planned.transition(DeliveryTransactionState::CleanupAttempted)?;
        let error = require_error(
            planned.finish_cleanup(CleanupOutcome::NotAttempted),
            "not-attempted cleanup must fail",
        );
        assert!(error.to_string().contains("must record success or failure"));

        let mut persisted = DeliveryTransaction::new(HEAD, vec![])?;
        for next in [
            DeliveryTransactionState::PendingReviewCreated,
            DeliveryTransactionState::CommentsCreated,
            DeliveryTransactionState::CommentsReconciled,
            DeliveryTransactionState::HeadRevalidated,
            DeliveryTransactionState::Submitted,
            DeliveryTransactionState::ReceiptsPersisted,
        ] {
            persisted.transition(next)?;
        }
        let error = require_error(
            persisted.record_failure(DeliveryFailureStage::Submission, "too late", true),
            "persisted transaction must be terminal",
        );
        assert!(
            error
                .to_string()
                .contains("cannot record failure after transaction is terminal")
        );
        Ok(())
    }

    #[test]
    fn reconciliation_validates_empty_inputs_and_duplicate_identities() {
        let error = require_error(
            reconcile_deliveries("", "review-42", &[], &[]),
            "empty head must fail",
        );
        assert!(error.to_string().contains("exact head SHA"));

        let error = require_error(
            reconcile_deliveries(HEAD, "", &[], &[]),
            "empty review id must fail",
        );
        assert!(error.to_string().contains("review id"));

        let planned = inline("claim-a", "src/a.rs", 4);
        let error = require_error(
            reconcile_deliveries(
                HEAD,
                "review-42",
                std::slice::from_ref(&planned),
                &[
                    observed("comment-a", planned.clone()),
                    observed("comment-b", planned.clone()),
                ],
            ),
            "duplicate identities must fail even with distinct comment ids",
        );
        assert!(
            error
                .to_string()
                .contains("duplicate planned delivery identity")
        );
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
        assert_eq!(
            value,
            serde_json::json!({
                "schema": DELIVERY_TRANSACTION_SCHEMA,
                "exact_head_sha": HEAD,
                "planned": [{
                    "exact_head_sha": HEAD,
                    "claim_id": "claim-a",
                    "action": "reply",
                    "path": "src/lib.rs",
                    "line": 12,
                    "side": "RIGHT",
                    "source_thread_id": "thread-7",
                    "expected_body_digest": "digest-claim-a",
                }],
                "state": "planned",
                "failure": Value::Null,
                "cleanup": {"status": "not_attempted"},
            })
        );

        let inline_value = serde_json::to_value(DeliveryTransaction::new(
            HEAD,
            vec![inline("claim-b", "src/lib.rs", 12)],
        )?)?;
        assert_eq!(inline_value["planned"][0]["source_thread_id"], Value::Null);
        Ok(())
    }
}
