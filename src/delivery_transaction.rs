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
pub const DELIVERY_RECONCILIATION_SCHEMA: &str = "ub-review.delivery_reconciliation.v1";

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
#[serde(try_from = "RawPlannedDelivery")]
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

    pub(crate) fn location(&self) -> (&str, u32, &str) {
        (&self.path, self.line, &self.side)
    }

    pub(crate) fn expected_body_digest(&self) -> &str {
        &self.expected_body_digest
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(try_from = "RawObservedDelivery")]
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
#[serde(try_from = "RawDeliveryReceipt")]
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
#[serde(try_from = "RawDeliveryReconciliation")]
pub struct DeliveryReconciliation {
    schema: String,
    exact_head_sha: String,
    review_id: String,
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
#[serde(try_from = "RawDeliveryTransaction")]
pub struct DeliveryTransaction {
    schema: String,
    exact_head_sha: String,
    planned: Vec<PlannedDelivery>,
    state: DeliveryTransactionState,
    failure: Option<DeliveryFailure>,
    cleanup: CleanupOutcome,
}

#[derive(Clone, Debug, Deserialize)]
struct RawPlannedDelivery {
    exact_head_sha: String,
    claim_id: String,
    action: DeliveryAction,
    path: String,
    line: u32,
    side: String,
    source_thread_id: Option<String>,
    expected_body_digest: String,
}

#[derive(Clone, Debug, Deserialize)]
struct RawObservedDelivery {
    comment_id: String,
    delivery: RawPlannedDelivery,
}

#[derive(Clone, Debug, Deserialize)]
struct RawDeliveryReceipt {
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

#[derive(Clone, Debug, Deserialize)]
struct RawDeliveryReconciliation {
    schema: String,
    exact_head_sha: String,
    review_id: String,
    planned_count: usize,
    observed_count: usize,
    receipts: Vec<RawDeliveryReceipt>,
}

#[derive(Clone, Debug, Deserialize)]
struct RawDeliveryTransaction {
    schema: String,
    exact_head_sha: String,
    planned: Vec<RawPlannedDelivery>,
    state: DeliveryTransactionState,
    failure: Option<DeliveryFailure>,
    cleanup: CleanupOutcome,
}

impl TryFrom<RawPlannedDelivery> for PlannedDelivery {
    type Error = anyhow::Error;

    fn try_from(raw: RawPlannedDelivery) -> Result<Self> {
        Self::new(
            raw.exact_head_sha,
            raw.claim_id,
            raw.action,
            DeliveryLocation::new(raw.path, raw.line, raw.side),
            raw.source_thread_id,
            raw.expected_body_digest,
        )
    }
}

impl TryFrom<RawObservedDelivery> for ObservedDelivery {
    type Error = anyhow::Error;

    fn try_from(raw: RawObservedDelivery) -> Result<Self> {
        Self::new(raw.comment_id, PlannedDelivery::try_from(raw.delivery)?)
    }
}

impl TryFrom<RawDeliveryReceipt> for DeliveryReceipt {
    type Error = anyhow::Error;

    fn try_from(raw: RawDeliveryReceipt) -> Result<Self> {
        let receipt = Self {
            schema: raw.schema,
            exact_head_sha: raw.exact_head_sha,
            claim_id: raw.claim_id,
            action: raw.action,
            path: raw.path,
            line: raw.line,
            side: raw.side,
            source_thread_id: raw.source_thread_id,
            expected_body_digest: raw.expected_body_digest,
            review_id: raw.review_id,
            comment_id: raw.comment_id,
            confirmed_head_sha: raw.confirmed_head_sha,
        };
        validate_delivery_receipt(&receipt)?;
        Ok(receipt)
    }
}

impl TryFrom<RawDeliveryReconciliation> for DeliveryReconciliation {
    type Error = anyhow::Error;

    fn try_from(raw: RawDeliveryReconciliation) -> Result<Self> {
        let reconciliation = Self {
            schema: raw.schema,
            exact_head_sha: raw.exact_head_sha,
            review_id: raw.review_id,
            planned_count: raw.planned_count,
            observed_count: raw.observed_count,
            receipts: raw
                .receipts
                .into_iter()
                .map(DeliveryReceipt::try_from)
                .collect::<Result<Vec<_>>>()?,
        };
        validate_delivery_reconciliation(&reconciliation)?;
        Ok(reconciliation)
    }
}

impl TryFrom<RawDeliveryTransaction> for DeliveryTransaction {
    type Error = anyhow::Error;

    fn try_from(raw: RawDeliveryTransaction) -> Result<Self> {
        ensure!(
            raw.schema == DELIVERY_TRANSACTION_SCHEMA,
            "delivery transaction schema must be {DELIVERY_TRANSACTION_SCHEMA}"
        );
        let transaction = Self {
            schema: raw.schema,
            exact_head_sha: raw.exact_head_sha,
            planned: raw
                .planned
                .into_iter()
                .map(PlannedDelivery::try_from)
                .collect::<Result<Vec<_>>>()?,
            state: raw.state,
            failure: raw.failure,
            cleanup: raw.cleanup,
        };
        validate_delivery_transaction(&transaction)?;
        Ok(transaction)
    }
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

impl From<&DeliveryReceipt> for DeliveryIdentity {
    fn from(receipt: &DeliveryReceipt) -> Self {
        Self {
            exact_head_sha: receipt.exact_head_sha.clone(),
            claim_id: receipt.claim_id.clone(),
            action: receipt.action.clone(),
            path: receipt.path.clone(),
            line: receipt.line,
            side: receipt.side.clone(),
            source_thread_id: receipt.source_thread_id.clone(),
            expected_body_digest: receipt.expected_body_digest.clone(),
        }
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
            !matches!(
                next,
                DeliveryTransactionState::Failed | DeliveryTransactionState::CleanedUp
            ),
            "metadata-dependent delivery states require record_failure or finish_cleanup"
        );
        ensure!(
            legal_transition(&self.state, &next),
            "illegal delivery transaction transition: {:?} -> {:?}",
            self.state,
            next
        );
        self.state = next;
        Ok(())
    }

    pub(crate) fn state(&self) -> &DeliveryTransactionState {
        &self.state
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
        let reason = reason.into();
        validate_identifier(&reason, "failure reason")?;
        let next = if cleanup_attempted {
            DeliveryTransactionState::CleanupAttempted
        } else {
            DeliveryTransactionState::Failed
        };
        ensure!(
            legal_transition(&self.state, &next),
            "illegal delivery transaction failure transition: {:?} -> {:?}",
            self.state,
            next
        );
        self.failure = Some(DeliveryFailure { stage, reason });
        self.state = next;
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
        validate_cleanup_outcome(&outcome)?;
        let next = if matches!(outcome, CleanupOutcome::Succeeded) {
            DeliveryTransactionState::CleanedUp
        } else {
            DeliveryTransactionState::Failed
        };
        ensure!(
            legal_transition(&self.state, &next),
            "illegal delivery transaction cleanup transition: {:?} -> {:?}",
            self.state,
            next
        );
        self.cleanup = outcome;
        self.state = next;
        Ok(())
    }

    pub fn record_post_submission_failure(
        &mut self,
        stage: DeliveryFailureStage,
        reason: impl Into<String>,
    ) -> Result<()> {
        ensure!(
            matches!(
                self.state,
                DeliveryTransactionState::Submitted | DeliveryTransactionState::ReceiptsPersisted
            ),
            "post-submission failure requires a submitted transaction, current state: {:?}",
            self.state
        );
        let reason = reason.into();
        validate_identifier(&reason, "failure reason")?;
        self.failure = Some(DeliveryFailure { stage, reason });
        self.cleanup = CleanupOutcome::NotAttempted;
        self.state = DeliveryTransactionState::Failed;
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
        schema: DELIVERY_RECONCILIATION_SCHEMA.to_owned(),
        exact_head_sha: exact_head_sha.to_owned(),
        review_id: review_id.to_owned(),
        planned_count: planned.len(),
        observed_count: observed.len(),
        receipts,
    })
}

fn validate_delivery_receipt(receipt: &DeliveryReceipt) -> Result<()> {
    ensure!(
        receipt.schema == DELIVERY_RECEIPT_SCHEMA,
        "delivery receipt schema must be {DELIVERY_RECEIPT_SCHEMA}"
    );
    validate_head_sha(&receipt.exact_head_sha)?;
    validate_identifier(&receipt.claim_id, "claim id")?;
    validate_identifier(&receipt.path, "comment path")?;
    ensure!(receipt.line > 0, "comment line must be positive");
    ensure!(
        matches!(receipt.side.as_str(), "LEFT" | "RIGHT"),
        "comment side must be LEFT or RIGHT, got {}",
        receipt.side
    );
    validate_identifier(&receipt.expected_body_digest, "expected body digest")?;
    validate_identifier(&receipt.review_id, "review id")?;
    validate_identifier(&receipt.comment_id, "GitHub comment id")?;
    ensure!(
        receipt.confirmed_head_sha == receipt.exact_head_sha,
        "confirmed delivery head must match exact delivery head"
    );
    match receipt.action {
        DeliveryAction::Inline => ensure!(
            receipt.source_thread_id.is_none(),
            "inline receipt must not carry a source thread id"
        ),
        DeliveryAction::Reply => {
            let thread = receipt
                .source_thread_id
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("reply receipt requires a source thread id"))?;
            validate_identifier(thread, "source thread id")?;
        }
    }
    Ok(())
}

fn validate_delivery_reconciliation(reconciliation: &DeliveryReconciliation) -> Result<()> {
    ensure!(
        reconciliation.schema == DELIVERY_RECONCILIATION_SCHEMA,
        "delivery reconciliation schema must be {DELIVERY_RECONCILIATION_SCHEMA}"
    );
    validate_head_sha(&reconciliation.exact_head_sha)?;
    validate_identifier(&reconciliation.review_id, "review id")?;
    ensure!(
        reconciliation.planned_count == reconciliation.observed_count,
        "planned and observed delivery counts must match"
    );
    ensure!(
        reconciliation.receipts.len() == reconciliation.observed_count,
        "receipt count must match observed delivery count"
    );
    let mut identities = BTreeSet::new();
    let mut comment_ids = BTreeSet::new();
    let mut previous_identity = None;
    for receipt in &reconciliation.receipts {
        validate_delivery_receipt(receipt)?;
        ensure!(
            receipt.exact_head_sha == reconciliation.exact_head_sha,
            "reconciliation receipt is bound to another head"
        );
        ensure!(
            receipt.review_id == reconciliation.review_id,
            "reconciliation receipt is bound to another review"
        );
        ensure!(
            identities.insert(DeliveryIdentity::from(receipt)),
            "reconciliation contains duplicate delivery identities"
        );
        ensure!(
            comment_ids.insert(receipt.comment_id.clone()),
            "reconciliation contains duplicate GitHub comment ids"
        );
        let identity = DeliveryIdentity::from(receipt);
        if let Some(previous) = previous_identity {
            ensure!(
                previous < identity,
                "reconciliation receipts must be in canonical order"
            );
        }
        previous_identity = Some(identity);
    }
    Ok(())
}

fn validate_delivery_transaction(transaction: &DeliveryTransaction) -> Result<()> {
    ensure!(
        transaction.schema == DELIVERY_TRANSACTION_SCHEMA,
        "delivery transaction schema must be {DELIVERY_TRANSACTION_SCHEMA}"
    );
    validate_head_sha(&transaction.exact_head_sha)?;
    ensure!(
        canonical_planned_deliveries(&transaction.exact_head_sha, transaction.planned.clone())?
            == transaction.planned,
        "deserialized delivery transaction planned items are not in canonical order"
    );
    if let Some(failure) = &transaction.failure {
        validate_identifier(&failure.reason, "failure reason")?;
    }
    validate_cleanup_outcome(&transaction.cleanup)?;
    match transaction.state {
        DeliveryTransactionState::Planned
        | DeliveryTransactionState::PendingReviewCreated
        | DeliveryTransactionState::CommentsCreated
        | DeliveryTransactionState::CommentsReconciled
        | DeliveryTransactionState::HeadRevalidated
        | DeliveryTransactionState::Submitted
        | DeliveryTransactionState::ReceiptsPersisted => {
            ensure!(
                transaction.failure.is_none(),
                "active delivery transaction state cannot carry a failure"
            );
            ensure!(
                transaction.cleanup == CleanupOutcome::NotAttempted,
                "active delivery transaction state cannot carry cleanup outcome"
            );
        }
        DeliveryTransactionState::CleanupAttempted => {
            ensure!(
                transaction.failure.is_some(),
                "cleanup-attempted delivery transaction must carry a failure"
            );
            ensure!(
                transaction.cleanup == CleanupOutcome::NotAttempted,
                "cleanup-attempted delivery transaction cannot carry a finished cleanup outcome"
            );
        }
        DeliveryTransactionState::CleanedUp => {
            ensure!(
                transaction.failure.is_some(),
                "cleaned-up delivery transaction must carry a failure"
            );
            ensure!(
                transaction.cleanup == CleanupOutcome::Succeeded,
                "cleaned-up delivery transaction must carry successful cleanup"
            );
        }
        DeliveryTransactionState::Failed => {
            ensure!(
                transaction.failure.is_some(),
                "failed delivery transaction must carry a failure"
            );
            ensure!(
                !matches!(transaction.cleanup, CleanupOutcome::Succeeded),
                "failed delivery transaction cannot carry successful cleanup"
            );
        }
    }
    Ok(())
}

fn validate_cleanup_outcome(outcome: &CleanupOutcome) -> Result<()> {
    if let CleanupOutcome::Failed(reason) = outcome {
        validate_identifier(reason, "cleanup failure reason")?;
    }
    Ok(())
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
        "{}:{}:{}:{}:{}:{}:{}",
        identity.claim_id,
        identity.action.as_str(),
        identity.path,
        identity.line,
        identity.side,
        identity.source_thread_id.as_deref().unwrap_or("-"),
        identity.expected_body_digest
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
            DeliveryTransactionState::Planned,
            DeliveryTransactionState::Failed
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
        ) | (
            DeliveryTransactionState::Submitted,
            DeliveryTransactionState::Failed
        ) | (
            DeliveryTransactionState::ReceiptsPersisted,
            DeliveryTransactionState::Failed
        ) | (
            DeliveryTransactionState::CleanupAttempted,
            DeliveryTransactionState::CleanedUp
        ) | (
            DeliveryTransactionState::CleanupAttempted,
            DeliveryTransactionState::Failed
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
    fn contract_round_trips_each_serialized_shape_and_lifecycle_state() -> Result<()> {
        let planned_inline = PlannedDelivery::new(
            HEAD,
            "claim-inline",
            DeliveryAction::Inline,
            DeliveryLocation::new("src/lib.rs", 12, "RIGHT"),
            None,
            "digest-inline",
        )?;
        let planned_reply = PlannedDelivery::new(
            HEAD,
            "claim-reply",
            DeliveryAction::Reply,
            DeliveryLocation::new("src/lib.rs", 13, "RIGHT"),
            Some("thread-7".to_owned()),
            "digest-reply",
        )?;
        let observed_inline = ObservedDelivery::new("comment-inline", planned_inline.clone())?;
        let observed_reply = ObservedDelivery::new("comment-reply", planned_reply.clone())?;
        let reconciliation = reconcile_deliveries(
            HEAD,
            "review-42",
            &[planned_inline.clone(), planned_reply.clone()],
            &[observed_inline.clone(), observed_reply.clone()],
        )?;

        assert_eq!(
            serde_json::from_value::<PlannedDelivery>(serde_json::to_value(&planned_inline)?)?,
            planned_inline
        );
        assert_eq!(
            serde_json::from_value::<ObservedDelivery>(serde_json::to_value(&observed_reply)?)?,
            observed_reply
        );
        let receipt = reconciliation
            .receipts
            .first()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("expected a receipt"))?;
        assert_eq!(
            serde_json::from_value::<DeliveryReceipt>(serde_json::to_value(&receipt)?)?,
            receipt
        );
        assert_eq!(
            serde_json::from_value::<DeliveryReconciliation>(serde_json::to_value(
                &reconciliation
            )?,)?,
            reconciliation
        );

        assert_eq!(DeliveryAction::Inline.as_str(), "inline");
        assert_eq!(DeliveryAction::Reply.as_str(), "reply");
        assert!(validate_head_sha(HEAD).is_ok());
        assert!(validate_identifier("claim-inline", "claim id").is_ok());

        let mut transaction = DeliveryTransaction::new(HEAD, vec![planned_reply])?;
        for next in [
            DeliveryTransactionState::PendingReviewCreated,
            DeliveryTransactionState::CommentsCreated,
            DeliveryTransactionState::CommentsReconciled,
            DeliveryTransactionState::HeadRevalidated,
            DeliveryTransactionState::Submitted,
            DeliveryTransactionState::ReceiptsPersisted,
        ] {
            assert!(transaction.transition(next.clone()).is_ok());
            assert_eq!(transaction.state, next);
            assert_eq!(
                serde_json::from_value::<DeliveryTransaction>(serde_json::to_value(&transaction)?)?
                    .state,
                next
            );
        }

        let mut failed = DeliveryTransaction::new(HEAD, vec![])?;
        assert!(
            failed
                .record_failure(DeliveryFailureStage::Submission, "submit rejected", false)
                .is_ok()
        );
        assert_eq!(failed.state, DeliveryTransactionState::Failed);
        assert!(failed.failure.is_some());
        assert_eq!(
            serde_json::from_value::<DeliveryTransaction>(serde_json::to_value(&failed)?)?.state,
            DeliveryTransactionState::Failed
        );

        let mut cleaned = DeliveryTransaction::new(HEAD, vec![])?;
        assert!(
            cleaned
                .transition(DeliveryTransactionState::PendingReviewCreated)
                .is_ok()
        );
        assert!(
            cleaned
                .record_failure(
                    DeliveryFailureStage::CommentCreation,
                    "comment rejected",
                    true
                )
                .is_ok()
        );
        assert_eq!(cleaned.state, DeliveryTransactionState::CleanupAttempted);
        assert!(cleaned.finish_cleanup(CleanupOutcome::Succeeded).is_ok());
        assert_eq!(cleaned.state, DeliveryTransactionState::CleanedUp);
        assert_eq!(cleaned.cleanup, CleanupOutcome::Succeeded);
        assert_eq!(
            serde_json::from_value::<DeliveryTransaction>(serde_json::to_value(&cleaned)?)?.state,
            DeliveryTransactionState::CleanedUp
        );
        Ok(())
    }

    #[test]
    fn deserialization_reapplies_invariants_for_each_receipt_shape() -> Result<()> {
        let planned_value = serde_json::to_value(inline("claim-a", "src/a.rs", 4))?;
        let mut invalid_planned = planned_value.clone();
        invalid_planned["line"] = 0.into();
        let error = require_error(
            serde_json::from_value::<PlannedDelivery>(invalid_planned),
            "invalid planned delivery must fail on deserialization",
        );
        assert_eq!(error.to_string(), "comment line must be positive");

        let observed_value =
            serde_json::to_value(observed("comment-a", inline("claim-a", "src/a.rs", 4)))?;
        let mut invalid_observed = observed_value;
        invalid_observed["comment_id"] = "".into();
        let error = require_error(
            serde_json::from_value::<ObservedDelivery>(invalid_observed),
            "invalid observed delivery must fail on deserialization",
        );
        assert_eq!(error.to_string(), "GitHub comment id must be non-empty");

        let transaction_value = serde_json::to_value(DeliveryTransaction::new(
            HEAD,
            vec![inline("claim-a", "src/a.rs", 4)],
        )?)?;
        let mut invalid_transaction = transaction_value;
        invalid_transaction["schema"] = "wrong.schema".into();
        let error = require_error(
            serde_json::from_value::<DeliveryTransaction>(invalid_transaction),
            "invalid transaction schema must fail on deserialization",
        );
        assert!(error.to_string().contains("delivery transaction schema"));

        let receipt = reconcile_deliveries(
            HEAD,
            "review-42",
            &[inline("claim-a", "src/a.rs", 4)],
            &[observed("comment-a", inline("claim-a", "src/a.rs", 4))],
        )?
        .receipts
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("expected one receipt"))?;
        let mut invalid_receipt = serde_json::to_value(receipt)?;
        invalid_receipt["confirmed_head_sha"] = "other-head".into();
        let error = require_error(
            serde_json::from_value::<DeliveryReceipt>(invalid_receipt),
            "wrong confirmed head must fail on deserialization",
        );
        assert_eq!(
            error.to_string(),
            "confirmed delivery head must match exact delivery head"
        );

        let reconciliation = reconcile_deliveries(
            HEAD,
            "review-42",
            &[inline("claim-a", "src/a.rs", 4)],
            &[observed("comment-a", inline("claim-a", "src/a.rs", 4))],
        )?;
        let mut invalid_reconciliation = serde_json::to_value(reconciliation)?;
        invalid_reconciliation["schema"] = DELIVERY_RECEIPT_SCHEMA.into();
        let error = require_error(
            serde_json::from_value::<DeliveryReconciliation>(invalid_reconciliation),
            "receipt schema must not deserialize as reconciliation schema",
        );
        assert!(error.to_string().contains("delivery reconciliation schema"));

        let reconciliation = reconcile_deliveries(
            HEAD,
            "review-42",
            &[inline("claim-a", "src/a.rs", 4)],
            &[observed("comment-a", inline("claim-a", "src/a.rs", 4))],
        )?;
        let mut wrong_review_receipt = serde_json::to_value(reconciliation)?;
        wrong_review_receipt["receipts"][0]["review_id"] = "review-other".into();
        let error = require_error(
            serde_json::from_value::<DeliveryReconciliation>(wrong_review_receipt),
            "mixed review ids must fail on deserialization",
        );
        assert_eq!(
            error.to_string(),
            "reconciliation receipt is bound to another review"
        );

        let valid_reconciliation = reconcile_deliveries(
            HEAD,
            "review-42",
            &[inline("claim-a", "src/a.rs", 4)],
            &[observed("comment-a", inline("claim-a", "src/a.rs", 4))],
        )?;
        let mut duplicate_reconciliation = serde_json::to_value(valid_reconciliation)?;
        duplicate_reconciliation["planned_count"] = 2.into();
        duplicate_reconciliation["observed_count"] = 2.into();
        duplicate_reconciliation["receipts"] = serde_json::json!([
            duplicate_reconciliation["receipts"][0].clone(),
            duplicate_reconciliation["receipts"][0].clone()
        ]);
        let error = require_error(
            serde_json::from_value::<DeliveryReconciliation>(duplicate_reconciliation),
            "duplicate reconciliation receipts must fail on deserialization",
        );
        assert!(error.to_string().contains("duplicate delivery identities"));

        let mut invalid_state = serde_json::to_value(DeliveryTransaction::new(HEAD, vec![])?)?;
        invalid_state["state"] = "failed".into();
        let error = require_error(
            serde_json::from_value::<DeliveryTransaction>(invalid_state),
            "inconsistent transaction state must fail on deserialization",
        );
        assert!(error.to_string().contains("must carry a failure"));
        Ok(())
    }

    #[test]
    fn delivery_identity_labels_are_stable_for_inline_and_reply() {
        let inline_identity = DeliveryIdentity::from(&inline("claim-a", "src/a.rs", 4));
        assert_eq!(
            identity_label(&inline_identity),
            "claim-a:inline:src/a.rs:4:RIGHT:-:digest-claim-a"
        );
        let reply_identity = DeliveryIdentity::from(&reply("claim-b", "thread-7"));
        assert_eq!(
            identity_label(&reply_identity),
            "claim-b:reply:src/lib.rs:12:RIGHT:thread-7:digest-claim-b"
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

    fn require_error<T, E>(result: std::result::Result<T, E>, context: &str) -> anyhow::Error
    where
        E: Into<anyhow::Error>,
    {
        result
            .err()
            .map(Into::into)
            .unwrap_or_else(|| anyhow::anyhow!("{context}"))
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

        assert_eq!(result.schema, DELIVERY_RECONCILIATION_SCHEMA);
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
                schema: DELIVERY_RECONCILIATION_SCHEMA.to_owned(),
                exact_head_sha: HEAD.to_owned(),
                review_id: "review-42".to_owned(),
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
                "schema": DELIVERY_RECONCILIATION_SCHEMA,
                "exact_head_sha": HEAD,
                "review_id": "review-42",
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
        assert_eq!(result.schema, DELIVERY_RECONCILIATION_SCHEMA);
        assert_eq!(result.exact_head_sha, HEAD);
        assert_eq!(result.review_id, "review-42");
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
            "GitHub delivery set mismatch; missing=[\"claim-b:inline:src/b.rs:5:RIGHT:-:digest-claim-b\"], unexpected=[]"
        );
    }

    #[test]
    fn reconciliation_rejects_duplicate_or_malformed_comment_ids() {
        let first = inline("claim-a", "src/a.rs", 4);
        let second = inline("claim-b", "src/b.rs", 5);
        let error = require_error(
            reconcile_deliveries(
                HEAD,
                "review-42",
                &[first.clone(), second.clone()],
                &[
                    observed("comment-a", first.clone()),
                    observed("comment-a", second),
                ],
            ),
            "duplicate returned comment IDs must fail",
        );
        assert_eq!(
            error.to_string(),
            "duplicate GitHub comment id in returned delivery set: comment-a"
        );

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
            "GitHub delivery set mismatch; missing=[\"claim-a:inline:src/a.rs:4:RIGHT:-:digest-claim-a\"], unexpected=[\"claim-other:inline:src/a.rs:4:RIGHT:-:digest-claim-a\"]"
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
        assert!(error.to_string().contains("metadata-dependent"));
        Ok(())
    }

    #[test]
    fn metadata_dependent_states_require_their_validating_operations() -> Result<()> {
        let mut planned = DeliveryTransaction::new(HEAD, vec![])?;
        let error = require_error(
            planned.transition(DeliveryTransactionState::Failed),
            "direct failure transition must be rejected",
        );
        assert!(error.to_string().contains("metadata-dependent"));
        assert_eq!(planned.state, DeliveryTransactionState::Planned);

        planned.transition(DeliveryTransactionState::PendingReviewCreated)?;
        planned.transition(DeliveryTransactionState::CleanupAttempted)?;
        let error = require_error(
            planned.transition(DeliveryTransactionState::CleanedUp),
            "direct cleanup transition must be rejected",
        );
        assert!(error.to_string().contains("metadata-dependent"));
        assert_eq!(planned.state, DeliveryTransactionState::CleanupAttempted);
        Ok(())
    }

    #[test]
    fn legal_transition_matrix_covers_success_and_cleanup_paths() {
        assert!(legal_transition(
            &DeliveryTransactionState::Planned,
            &DeliveryTransactionState::PendingReviewCreated
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::PendingReviewCreated,
            &DeliveryTransactionState::CommentsCreated
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::CommentsCreated,
            &DeliveryTransactionState::CommentsReconciled
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::CommentsReconciled,
            &DeliveryTransactionState::HeadRevalidated
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::HeadRevalidated,
            &DeliveryTransactionState::Submitted
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::Submitted,
            &DeliveryTransactionState::ReceiptsPersisted
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::Planned,
            &DeliveryTransactionState::Failed
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::PendingReviewCreated,
            &DeliveryTransactionState::CleanupAttempted
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::CommentsCreated,
            &DeliveryTransactionState::CleanupAttempted
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::CommentsReconciled,
            &DeliveryTransactionState::CleanupAttempted
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::HeadRevalidated,
            &DeliveryTransactionState::CleanupAttempted
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::Submitted,
            &DeliveryTransactionState::CleanupAttempted
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::CleanupAttempted,
            &DeliveryTransactionState::CleanedUp
        ));
        assert!(legal_transition(
            &DeliveryTransactionState::CleanupAttempted,
            &DeliveryTransactionState::Failed
        ));
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

        let mut invalid_cleanup = serde_json::to_value(&failed)?;
        invalid_cleanup["cleanup"]["reason"] = "".into();
        let error = require_error(
            serde_json::from_value::<DeliveryTransaction>(invalid_cleanup),
            "empty cleanup failure reason must fail on deserialization",
        );
        assert_eq!(
            error.to_string(),
            "cleanup failure reason must be non-empty"
        );

        let mut empty_cleanup = DeliveryTransaction::new(HEAD, vec![])?;
        empty_cleanup.transition(DeliveryTransactionState::PendingReviewCreated)?;
        empty_cleanup.record_failure(DeliveryFailureStage::Submission, "submit rejected", true)?;
        let error = require_error(
            empty_cleanup.finish_cleanup(CleanupOutcome::Failed(String::new())),
            "empty cleanup failure reason must fail before mutation",
        );
        assert_eq!(
            error.to_string(),
            "cleanup failure reason must be non-empty"
        );
        assert_eq!(
            empty_cleanup.state,
            DeliveryTransactionState::CleanupAttempted
        );
        assert_eq!(empty_cleanup.cleanup, CleanupOutcome::NotAttempted);

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
            planned.record_failure(DeliveryFailureStage::Submission, "too early", true),
            "cleanup from planned state must fail atomically",
        );
        assert!(
            error
                .to_string()
                .contains("illegal delivery transaction failure transition")
        );
        assert_eq!(planned.state, DeliveryTransactionState::Planned);
        assert_eq!(planned.failure, None);

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
    fn post_submission_failure_is_terminal_and_never_deletes_submitted_review() -> Result<()> {
        for terminal in [
            DeliveryTransactionState::Submitted,
            DeliveryTransactionState::ReceiptsPersisted,
        ] {
            let mut transaction = DeliveryTransaction::new(HEAD, vec![])?;
            for next in [
                DeliveryTransactionState::PendingReviewCreated,
                DeliveryTransactionState::CommentsCreated,
                DeliveryTransactionState::CommentsReconciled,
                DeliveryTransactionState::HeadRevalidated,
                DeliveryTransactionState::Submitted,
            ] {
                transaction.transition(next.clone())?;
                if next == terminal {
                    break;
                }
            }
            if terminal == DeliveryTransactionState::ReceiptsPersisted {
                transaction.transition(DeliveryTransactionState::ReceiptsPersisted)?;
            }
            transaction.record_post_submission_failure(
                DeliveryFailureStage::ReceiptPersistence,
                "receipt write failed",
            )?;
            assert_eq!(transaction.state, DeliveryTransactionState::Failed);
            assert_eq!(transaction.cleanup, CleanupOutcome::NotAttempted);
            assert_eq!(
                transaction.failure.as_ref().map(|failure| &failure.stage),
                Some(&DeliveryFailureStage::ReceiptPersistence)
            );
            let error = require_error(
                transaction.record_post_submission_failure(
                    DeliveryFailureStage::ReceiptPersistence,
                    "second failure",
                ),
                "terminal post-submission state must reject a second failure",
            );
            assert!(
                error
                    .to_string()
                    .contains("requires a submitted transaction")
            );
        }
        Ok(())
    }

    #[test]
    fn cleanup_outcome_validation_is_fail_closed() -> Result<()> {
        assert!(validate_cleanup_outcome(&CleanupOutcome::NotAttempted).is_ok());
        assert!(validate_cleanup_outcome(&CleanupOutcome::Succeeded).is_ok());
        let error = match validate_cleanup_outcome(&CleanupOutcome::Failed(String::new())) {
            Ok(()) => return Err(anyhow::anyhow!("empty cleanup failure reason was accepted")),
            Err(error) => error,
        };
        assert_eq!(
            error.to_string(),
            "cleanup failure reason must be non-empty"
        );
        assert!(
            validate_cleanup_outcome(&CleanupOutcome::Failed("delete rejected".to_owned())).is_ok()
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
