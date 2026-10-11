use anyhow::{Result, ensure};

use super::*;

const HEAD: &str = "1212121212121212121212121212121212121212";

fn observation() -> Observation {
    Observation {
        schema: "observation".to_owned(),
        id: "claim-alpha".to_owned(),
        lane: "tests".to_owned(),
        question: "Does the parser retain the boundary?".to_owned(),
        claim: "parser boundary may be dropped".to_owned(),
        kind: "bug".to_owned(),
        status: "open".to_owned(),
        severity: "high".to_owned(),
        confidence: "medium".to_owned(),
        path: Some("src/parser.rs".to_owned()),
        line: Some(12),
        fingerprint: "parser-boundary".to_owned(),
        evidence: vec!["src/parser.rs:12 needs a boundary witness".to_owned()],
        dedupe_key: "parser-boundary".to_owned(),
        source: "tests".to_owned(),
    }
}

fn request(id: &str) -> ProofRequest {
    ProofRequest {
        schema: "proof-request".to_owned(),
        id: id.to_owned(),
        lane: "tests".to_owned(),
        requested_by: vec!["tests".to_owned()],
        command: "cargo test boundary".to_owned(),
        reason: "needs a boundary witness".to_owned(),
        cost: "small".to_owned(),
        timeout_sec: 60,
        required: false,
        status: "requested".to_owned(),
    }
}

fn receipt(id: &str, head: &str, binding: &str, result: &str, reason: &str) -> ProofReceipt {
    ProofReceipt {
        schema: "proof".to_owned(),
        id: id.to_owned(),
        kind: "focused-test".to_owned(),
        base: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        head: head.to_owned(),
        revision: None,
        test_patch_mode: "head-only".to_owned(),
        requested_by: vec!["tests".to_owned()],
        request_ids: vec![binding.to_owned()],
        commands: Vec::new(),
        result: result.to_owned(),
        reason: reason.to_owned(),
    }
}

fn compile_graph(
    observation: &Observation,
    requests: &[ProofRequest],
    receipts: &[ProofReceipt],
) -> ClaimGraph {
    build_active_claim_graph(
        HEAD,
        std::slice::from_ref(observation),
        &[],
        &[],
        requests,
        receipts,
        &PrThreadContext {
            schema: "ub-review.pr_thread_context.v1".to_owned(),
            status: "seeded".to_owned(),
            max_bytes: 65_536,
            sources: Vec::new(),
            warnings: Vec::new(),
            pull_number: None,
            title: None,
            body: None,
            body_truncated: false,
            thread_context_path: None,
            thread_context: None,
            thread_context_truncated: false,
            threads: Vec::new(),
        },
    )
}

#[test]
fn equivalent_proof_record_permutations_have_literal_links_and_identical_bytes() -> Result<()> {
    let observation = observation();
    let requests = [request("parser-boundary"), request("claim-alpha")];
    let receipts = [
        receipt(
            "receipt-z",
            HEAD,
            "parser-boundary",
            "skipped",
            "target unavailable",
        ),
        receipt(
            "receipt-a",
            HEAD,
            "claim-alpha",
            "head_failed",
            "HEAD command failed",
        ),
        receipt(
            "receipt-m",
            HEAD,
            "claim-alpha",
            "not_discriminating",
            "both sides passed",
        ),
    ];
    let mut canonical_bytes = None;
    for request_order in [[0, 1], [1, 0]] {
        for receipt_order in [
            [0, 1, 2],
            [0, 2, 1],
            [1, 0, 2],
            [1, 2, 0],
            [2, 0, 1],
            [2, 1, 0],
        ] {
            let ordered_requests = request_order.map(|index| requests[index].clone());
            let ordered_receipts = receipt_order.map(|index| receipts[index].clone());
            let graph = compile_graph(&observation, &ordered_requests, &ordered_receipts);
            ensure!(graph.topics.len() == 1 && graph.claims.len() == 1);
            let topic = &graph.topics[0];
            ensure!(
                topic.proof_requests == ["claim-alpha", "parser-boundary"],
                "request links depend on input order: {:?}",
                topic.proof_requests
            );
            ensure!(
                topic.proof_receipts == ["receipt-a", "receipt-m", "receipt-z"],
                "receipt links depend on input order: {:?}",
                topic.proof_receipts
            );
            let evidence = serde_json::to_value(&topic.evidence)?;
            ensure!(
                evidence
                    == serde_json::json!([
                        {"class": "ModelInterpretation", "reference": "observation:claim-alpha", "detail": "src/parser.rs:12 needs a boundary witness"},
                        {"class": "ProofReceipt", "reference": "review/proof_receipts.json#receipt-a", "detail": "head_failed: HEAD command failed"},
                        {"class": "ProofReceipt", "reference": "review/proof_receipts.json#receipt-m", "detail": "not_discriminating: both sides passed"},
                        {"class": "ProofReceipt", "reference": "review/proof_receipts.json#receipt-z", "detail": "skipped: target unavailable"}
                    ]),
                "evidence order/content changed: {evidence}"
            );
            ensure!(topic.status == "needs_evidence" && topic.delivery == "no-human-surface");
            ensure!(serde_json::to_value(&graph.claims[0].state)? == "NeedsEvidence");
            let bytes = serde_json::to_vec(&graph)?;
            ensure!(
                bytes
                    == serde_json::to_vec(&compile_graph(
                        &observation,
                        &ordered_requests,
                        &ordered_receipts
                    ))?,
                "repeated identical records changed output"
            );
            if let Some(expected) = &canonical_bytes {
                ensure!(
                    &bytes == expected,
                    "equivalent permutations changed serialized graph"
                );
            } else {
                canonical_bytes = Some(bytes);
            }
        }
    }
    Ok(())
}

#[test]
fn stale_unrelated_and_substring_subject_records_leave_missing_proof_explicit() -> Result<()> {
    let mut observation = observation();
    observation.evidence.clear();
    let receipts = [
        receipt(
            "stale",
            "3434343434343434343434343434343434343434",
            "claim-alpha",
            "discriminating",
            "old head",
        ),
        receipt(
            "same-lane-other-claim",
            HEAD,
            "another-claim",
            "discriminating",
            "different subject",
        ),
        receipt(
            "substring",
            HEAD,
            "prefix-claim-alpha-suffix",
            "discriminating",
            "substring is not linkage",
        ),
    ];
    let requests = [
        request("another-claim"),
        request("prefix-parser-boundary-suffix"),
    ];
    for receipts in [receipts.to_vec(), receipts.into_iter().rev().collect()] {
        let graph = compile_graph(&observation, &requests, &receipts);
        ensure!(graph.topics.len() == 1 && graph.claims.len() == 1);
        let topic = &graph.topics[0];
        ensure!(topic.proof_requests.is_empty() && topic.proof_receipts.is_empty());
        ensure!(topic.evidence.is_empty() && graph.claims[0].supporting_evidence.is_empty());
        ensure!(topic.head_sha == HEAD && topic.status == "needs_evidence");
        ensure!(graph.evidence_gaps.len() == 1);
        ensure!(graph.evidence_gaps[0].claim_id == topic.claim_id);
        ensure!(graph.evidence_gaps[0].needed_evidence_class == "proof-receipt");
        ensure!(
            graph.evidence_gaps[0].detail
                == "Claim `parser boundary may be dropped` has no deterministic evidence"
        );
    }
    Ok(())
}

#[test]
fn same_receipt_id_with_distinct_details_is_retained_in_stable_order() -> Result<()> {
    let observation = observation();
    let receipts = [
        receipt(
            "receipt-collision",
            HEAD,
            "claim-alpha",
            "skipped",
            "z unavailable",
        ),
        receipt(
            "receipt-collision",
            HEAD,
            "claim-alpha",
            "head_failed",
            "failed",
        ),
        receipt(
            "receipt-collision",
            HEAD,
            "claim-alpha",
            "skipped",
            "a unavailable",
        ),
    ];
    let forward = compile_graph(&observation, &[], &receipts);
    let reverse = compile_graph(
        &observation,
        &[],
        &receipts.into_iter().rev().collect::<Vec<_>>(),
    );
    ensure!(forward.topics.len() == 1);
    ensure!(forward.topics[0].proof_receipts == ["receipt-collision"]);
    let details = forward.topics[0]
        .evidence
        .iter()
        .map(|evidence| evidence.detail.as_str())
        .collect::<Vec<_>>();
    ensure!(
        details
            == [
                "src/parser.rs:12 needs a boundary witness",
                "head_failed: failed",
                "skipped: a unavailable",
                "skipped: z unavailable"
            ],
        "colliding receipt details were lost or reordered: {details:?}"
    );
    ensure!(serde_json::to_vec(&forward)? == serde_json::to_vec(&reverse)?);
    ensure!(forward.topics[0].status == "needs_evidence");
    Ok(())
}
