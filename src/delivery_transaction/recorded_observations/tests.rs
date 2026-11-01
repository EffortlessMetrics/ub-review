use super::*;
use crate::claim_graph::{
    ClaimGraph, ClaimNode, ClaimState, EvidenceClass, EvidenceRef, RelevanceKind, RelevancePath,
};
use crate::review_judgment::public_plan::{
    MaterialClass, RecordedAnchor, RecordedMaterial, RecordedPublicSource, public_source_digest,
};
use crate::review_judgment::{
    ImportedCi, RecordedEvidence, RecordedJudgmentSource, source_manifest_digest,
};
use crate::revision_identity::{CommitTree, ReviewSemantics, RevisionIdentity};
use serde_json::{Value, json};

const PATCH: &[u8] = b"diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-safe();\n+danger();\n";
const INLINE: &str = "The unchecked index can exceed the allocation.";
const SUMMARY: &str = "This summary remains material without an inline anchor.";

struct Fixture {
    identity: RevisionIdentity,
    subject: JudgmentSubject,
    delivery: DeliverySubject,
    graph: ClaimGraph,
    ci: Vec<u8>,
}
impl Fixture {
    fn new() -> Result<Self> {
        let head = CommitTree::new("head", &"a".repeat(40), &"b".repeat(40))?;
        let identity = RevisionIdentity::new(
            CommitTree::new("base", &"c".repeat(40), &"d".repeat(40))?,
            head.clone(),
            head,
            None,
            ReviewSemantics::CandidateHead,
            &RevisionIdentity::changed_paths_digest(["src/lib.rs"]),
            &RevisionIdentity::diff_digest(PATCH),
        )?;
        let mut graph = crate::build_active_claim_graph(
            &"a".repeat(40),
            &[],
            &[],
            &[],
            &[],
            &[],
            &crate::absent_pr_thread_context(0),
        );
        graph.revision = Some(identity.revision_ref());
        for claim in ["A", "B"] {
            graph.claims.push(ClaimNode {
                id: claim.to_owned(),
                subject: "admitted claim".to_owned(),
                source_lane: "recorded".to_owned(),
                state: ClaimState::Confirmed,
                supporting_evidence: vec![EvidenceRef {
                    class: EvidenceClass::ExactCitation,
                    reference: "fact#1".to_owned(),
                    detail: "citation".to_owned(),
                }],
                contradicting_evidence: vec![],
                relevance: RelevancePath {
                    kind: RelevanceKind::ChangedLine,
                    explanation: "changed source".to_owned(),
                },
                severity: "high".to_owned(),
            });
        }
        Ok(Self {
            identity,
            subject: JudgmentSubject {
                repository: "owner/repo".to_owned(),
                review_spec_digest: "e".repeat(64),
                pass_id: "pass-1".to_owned(),
                attempt_id: "lead-1".to_owned(),
            },
            delivery: DeliverySubject {
                repository: "owner/repo".to_owned(),
                pull_number: 7,
                head_sha: "a".repeat(40),
            },
            graph,
            ci: vec![0, 255, b' ', b'\n'],
        })
    }
    fn planned(&self) -> Result<PlannedDelivery> {
        PlannedDelivery::new(
            &self.delivery.head_sha,
            "A",
            DeliveryAction::Inline,
            DeliveryLocation::new("src/lib.rs", 1, "RIGHT"),
            None,
            sha256_hex(INLINE.as_bytes()),
        )
    }
    fn with_source<T>(&self, run: impl FnOnce(PublicPlanInput<'_>) -> Result<T>) -> Result<T> {
        let evidence_hash = sha256_hex(b"citation");
        let evidence = [RecordedEvidence {
            reference: "fact#1",
            digest: &evidence_hash,
            bytes: b"citation",
        }];
        let canonical = self.identity.canonical_form();
        let ci_hash = sha256_hex(&self.ci);
        let source = RecordedJudgmentSource {
            subject: &self.subject,
            revision_canonical: &canonical,
            graph: &self.graph,
            review_complete: true,
            evidence: &evidence,
            imported_ci: Some(ImportedCi {
                digest: &ci_hash,
                bytes: &self.ci,
            }),
        };
        let source_hash = source_manifest_digest(&source)?;
        let inline_hash = sha256_hex(INLINE.as_bytes());
        let summary_hash = sha256_hex(SUMMARY.as_bytes());
        let line_hash = sha256_hex(b"danger();");
        let diff_hash = RevisionIdentity::diff_digest(PATCH);
        let materials = [
            RecordedMaterial {
                claim_id: "A",
                body_ref: "body#A",
                body_digest: &inline_hash,
                class: MaterialClass::Public,
                body: INLINE.as_bytes(),
            },
            RecordedMaterial {
                claim_id: "B",
                body_ref: "body#B",
                body_digest: &summary_hash,
                class: MaterialClass::Public,
                body: SUMMARY.as_bytes(),
            },
        ];
        let anchors = [RecordedAnchor {
            reference: "anchor#A",
            claim_id: "A",
            path: "src/lib.rs",
            line: 1,
            line_digest: &line_hash,
        }];
        let public = RecordedPublicSource {
            delivery_subject: &self.delivery,
            diff_digest: &diff_hash,
            diff_patch: PATCH,
            materials: &materials,
            anchors: &anchors,
        };
        let public_hash = public_source_digest(&public)?;
        let mut answer = serde_json::to_vec(
            &json!({"schema":"ub-review.recorded-judgment.v1","subject":self.subject,"revision":self.identity.revision_ref(),"source_manifest_digest":source_hash,
            "review_result":"findings","selections":[{"claim_id":"A","evidence_refs":["fact#1"]},{"claim_id":"B","evidence_refs":["fact#1"]}]}),
        )?;
        answer.push(b'\n');
        let answer_hash = sha256_hex(&answer);
        let mut plan = serde_json::to_vec(
            &json!({"schema":"ub-review.recorded-public-plan.v1","judgment_digest":answer_hash,"revision":self.identity.revision_ref(),"subject":self.subject,"delivery_subject":self.delivery,
            "public_source_digest":public_hash,"quiet":false,"items":[{"claim_id":"A","disposition":{"kind":"inline","body_ref":"body#A","anchor_ref":"anchor#A","delivery":self.planned()?}},
            {"claim_id":"B","disposition":{"kind":"summary","body_ref":"body#B","body_digest":summary_hash}}]}),
        )?;
        plan.push(b'\n');
        let plan_hash = sha256_hex(&plan);
        run(PublicPlanInput {
            judgment_source: &source,
            current_revision: &self.identity,
            expected_source_digest: &source_hash,
            answer: Some(&answer),
            expected_answer_digest: Some(&answer_hash),
            observed_ci: Some(&self.ci),
            public_source: &public,
            expected_public_source_digest: &public_hash,
            plan: Some(&plan),
            expected_plan_digest: Some(&plan_hash),
        })
    }
    fn payload(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(
            &json!({"event":"COMMENT","body":"Summary correspondence is deliberately not admitted.","comments":[{"path":"src/lib.rs","line":1,"side":"RIGHT","body":INLINE}]}),
        )?)
    }
    fn stream(&self, payload: &[u8]) -> Result<ObservationStream> {
        self.with_source(|s| {
            Ok(ObservationStream {
                schema: STREAM_SCHEMA.to_owned(),
                subject: ObservationSubject {
                    judgment_subject: self.subject.clone(),
                    delivery_subject: self.delivery.clone(),
                    revision: self.identity.revision_ref(),
                    source_manifest_digest: s.expected_source_digest.to_owned(),
                    public_source_digest: s.expected_public_source_digest.to_owned(),
                    judgment_digest: s.expected_answer_digest.unwrap_or_default().to_owned(),
                    plan_digest: s.expected_plan_digest.unwrap_or_default().to_owned(),
                    payload_sha256: sha256_hex(payload),
                },
                events: vec![],
            })
        })
    }
    fn ambiguous(&self) -> Result<RecordedObservation> {
        let mut transaction =
            DeliveryTransaction::new(&self.delivery.head_sha, vec![self.planned()?])?;
        for state in [
            DeliveryTransactionState::PendingReviewCreated,
            DeliveryTransactionState::CommentsCreated,
            DeliveryTransactionState::CommentsReconciled,
            DeliveryTransactionState::HeadRevalidated,
        ] {
            transaction.transition(state)?;
        }
        transaction.record_failure(
            DeliveryFailureStage::Submission,
            "lost response PRIVATE_SENTINEL",
            true,
        )?;
        transaction.finish_cleanup(CleanupOutcome::Failed("already submitted".to_owned()))?;
        let mut bytes = serde_json::to_vec(&transaction)?;
        bytes.push(b'\n');
        Ok(RecordedObservation::Ambiguous {
            stage: DeliveryFailureStage::Submission,
            local_receipt: ReceiptBytes {
                sha256: sha256_hex(&bytes),
                bytes,
            },
        })
    }
    fn confirmation(&self, review: &str, comment: &str) -> Result<RecordedObservation> {
        let planned = self.planned()?;
        let observed = vec![ObservedDelivery::new(comment, planned.clone())?];
        let receipt = reconcile_deliveries(&self.delivery.head_sha, review, &[planned], &observed)?;
        let mut bytes = serde_json::to_vec(&receipt)?;
        bytes.push(b'\n');
        Ok(RecordedObservation::ConfirmedInline {
            observed,
            reconciliation: ReceiptBytes {
                sha256: sha256_hex(&bytes),
                bytes,
            },
        })
    }
}
fn append(
    stream: &mut ObservationStream,
    attempt: &str,
    observation: RecordedObservation,
) -> Result<()> {
    let mut chain = genesis(&stream.subject)?;
    for event in &stream.events {
        chain = extend_chain(&chain, event);
    }
    stream.events.push(serde_json::to_vec(&ObservationEvent {
        sequence: stream.events.len() + 1,
        previous_sha256: chain,
        attempt_id: attempt.to_owned(),
        observation,
    })?);
    Ok(())
}
fn watermark(stream: &ObservationStream, count: usize, name: &str) -> Result<Vec<u8>> {
    let mut chain = genesis(&stream.subject)?;
    let mut head = stream.subject.delivery_subject.head_sha.clone();
    for bytes in stream.events.iter().take(count) {
        let event: Value = serde_json::from_slice(bytes)?;
        if event["observation"]["kind"] == "superseded" {
            head = event["observation"]["observed_head_sha"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("fixture head missing"))?
                .to_owned();
        }
        chain = extend_chain(&chain, bytes);
    }
    Ok(serde_json::to_vec(&ObservationWatermark {
        schema: WATERMARK_SCHEMA.to_owned(),
        name: name.to_owned(),
        count,
        chain_sha256: chain,
        observed_head_sha: head,
    })?)
}
fn invoke(
    f: &Fixture,
    stream: &ObservationStream,
    payload: &[u8],
    wm: &[u8],
    name: &str,
) -> Result<RecordedObservationExport> {
    let stream = serde_json::to_vec(stream)?;
    invoke_bytes(f, &stream, payload, wm, name)
}
fn invoke_bytes(
    f: &Fixture,
    stream: &[u8],
    payload: &[u8],
    wm: &[u8],
    name: &str,
) -> Result<RecordedObservationExport> {
    f.with_source(|source| {
        export_recorded_observations(ObservationInput {
            source,
            payload,
            payload_sha256: &sha256_hex(payload),
            stream,
            stream_sha256: &sha256_hex(stream),
            watermark: wm,
            watermark_sha256: &sha256_hex(wm),
            watermark_name: name,
        })
    })
}

fn prepared() -> RecordedObservation {
    RecordedObservation::Prepared { retry_of: None }
}
fn lifecycle(f: &Fixture, payload: &[u8]) -> Result<ObservationStream> {
    let mut stream = f.stream(payload)?;
    append(&mut stream, "delivery-1", prepared())?;
    append(&mut stream, "delivery-1", f.ambiguous()?)?;
    append(&mut stream, "delivery-1", f.confirmation("987", "123")?)?;
    append(
        &mut stream,
        "delivery-1",
        RecordedObservation::Superseded {
            observed_head_sha: "f".repeat(40),
        },
    )?;
    Ok(stream)
}
fn view(export: &RecordedObservationExport) -> Result<Value> {
    Ok(serde_json::from_slice(&export.observation_json)?)
}

#[test]
fn prefix_preserves_sources_and_local_failure_never_proves_remote_absence() -> Result<()> {
    let f = Fixture::new()?;
    let payload = f.payload()?;
    let stream = lifecycle(&f, &payload)?;
    let original = f.with_source(export_recorded_review)?;
    for count in 1..=4 {
        let name = format!("cut-{count}");
        let wm = watermark(&stream, count, &name)?;
        let exported = invoke(&f, &stream, &payload, &wm, &name)?;
        let mut prefix = stream.clone();
        prefix.events.truncate(count);
        assert_eq!(exported, invoke(&f, &prefix, &payload, &wm, &name)?);
        let v = view(&exported)?;
        assert_eq!(exported.source, original);
        assert_eq!(
            exported.source.imported_ci_bytes.as_deref(),
            Some(f.ci.as_slice())
        );
        assert_eq!(v["watermark"]["count"], count);
        assert_eq!(v["whole_review_delivery"], "not_qualified");
        assert_eq!(v["summary_payload_correspondence"], "unverified");
        assert_eq!(
            v["attempts"][0]["remote"]["outcome"],
            if count < 3 {
                "unknown"
            } else {
                "confirmed_inline"
            }
        );
        assert_eq!(
            v["source_currentness"],
            if count == 4 {
                "historical_at_recorded_watermark"
            } else {
                "at_recorded_source_head"
            }
        );
        assert!(!String::from_utf8_lossy(&exported.observation_json).contains("PRIVATE_SENTINEL"));
        if count >= 2 {
            assert_eq!(v["attempts"][0]["local_failure"]["stage"], "submission");
        }
        if count >= 3 {
            assert_eq!(
                v["attempts"][0]["remote"]["reconciliation"]["receipts"][0]["confirmed_head_sha"],
                "a".repeat(40)
            );
        }
        let bytes = serde_json::to_vec(&stream)?;
        f.with_source(|source| {
            replay_recorded_observations(
                ObservationInput {
                    source,
                    payload: &payload,
                    payload_sha256: &sha256_hex(&payload),
                    stream: &bytes,
                    stream_sha256: &sha256_hex(&bytes),
                    watermark: &wm,
                    watermark_sha256: &sha256_hex(&wm),
                    watermark_name: &name,
                },
                &exported,
                &sha256_hex(&exported.observation_json),
            )
        })?;
    }
    Ok(())
}

#[test]
fn retries_are_distinct_and_late_confirmation_remains_historical() -> Result<()> {
    let f = Fixture::new()?;
    let payload = f.payload()?;
    let mut s = f.stream(&payload)?;
    append(&mut s, "delivery-1", prepared())?;
    append(&mut s, "delivery-1", f.ambiguous()?)?;
    append(
        &mut s,
        "delivery-2",
        RecordedObservation::Prepared {
            retry_of: Some("delivery-1".to_owned()),
        },
    )?;
    append(
        &mut s,
        "delivery-2",
        RecordedObservation::Superseded {
            observed_head_sha: "f".repeat(40),
        },
    )?;
    append(&mut s, "delivery-2", f.confirmation("987", "123")?)?;
    let v = view(&invoke(
        &f,
        &s,
        &payload,
        &watermark(&s, 5, "latest")?,
        "latest",
    )?)?;
    assert_eq!(v["attempts"].as_array().map(Vec::len), Some(2));
    assert_eq!(v["attempts"][0]["remote"]["outcome"], "unknown");
    assert_eq!(v["attempts"][1]["remote"]["outcome"], "confirmed_inline");
    assert_eq!(v["attempts"][1]["retry_of"], "delivery-1");
    assert_eq!(v["source_currentness"], "historical_at_recorded_watermark");
    assert_eq!(
        v["attempts"][1]["remote"]["reconciliation"]["exact_head_sha"],
        "a".repeat(40)
    );
    Ok(())
}

#[test]
fn deletion_reordering_and_previous_link_tampering_are_rejected() -> Result<()> {
    let f = Fixture::new()?;
    let payload = f.payload()?;
    let s = lifecycle(&f, &payload)?;
    let mut deleted = s.clone();
    deleted.events.remove(1);
    let mut reordered = s.clone();
    reordered.events.swap(1, 2);
    let mut bad_link = s.clone();
    let mut e: Value = serde_json::from_slice(&bad_link.events[0])?;
    e["previous_sha256"] = json!("0".repeat(64));
    bad_link.events[0] = serde_json::to_vec(&e)?;
    for bad in [deleted, reordered, bad_link] {
        let wm = watermark(&bad, bad.events.len(), "changed")?;
        assert!(invoke(&f, &bad, &payload, &wm, "changed").is_err());
    }
    let mut truncated = s.clone();
    truncated.events.pop();
    assert!(
        invoke(
            &f,
            &truncated,
            &payload,
            &watermark(&s, 4, "latest")?,
            "latest"
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn canonical_subject_and_payload_bindings_survive_repinned_adversaries() -> Result<()> {
    let f = Fixture::new()?;
    let payload = f.payload()?;
    let s = lifecycle(&f, &payload)?;
    let wm = watermark(&s, 4, "latest")?;
    for field in [
        "repository",
        "pull_number",
        "head_sha",
        "pass_id",
        "attempt_id",
        "review_spec_digest",
        "source_manifest_digest",
        "public_source_digest",
        "judgment_digest",
        "plan_digest",
        "payload_sha256",
    ] {
        let mut bad = s.clone();
        let mut v = serde_json::to_value(&bad.subject)?;
        match field {
            "repository" | "pull_number" | "head_sha" => {
                v["delivery_subject"][field] = if field == "pull_number" {
                    json!(8)
                } else {
                    json!("wrong")
                }
            }
            "pass_id" | "attempt_id" | "review_spec_digest" => {
                v["judgment_subject"][field] = json!("wrong")
            }
            _ => v[field] = json!("0".repeat(64)),
        }
        bad.subject = serde_json::from_value(v)?;
        assert!(
            invoke(&f, &bad, &payload, &wm, "latest").is_err(),
            "{field}"
        );
    }
    let mut fresh = Fixture::new()?;
    fresh.subject.pass_id = "pass-2".to_owned();
    assert!(invoke(&fresh, &s, &payload, &wm, "latest").is_err());
    for field in ["body", "path", "line", "side"] {
        let mut p: Value = serde_json::from_slice(&payload)?;
        p["comments"][0][field] = if field == "line" {
            json!(2)
        } else {
            json!("wrong")
        };
        let p = serde_json::to_vec(&p)?;
        let mut bad = f.stream(&p)?;
        append(&mut bad, "delivery-1", prepared())?;
        assert!(
            invoke(&f, &bad, &p, &watermark(&bad, 1, "latest")?, "latest").is_err(),
            "{field}"
        );
    }
    Ok(())
}

#[test]
fn forged_stale_watermarks_and_repinned_output_are_rejected() -> Result<()> {
    let f = Fixture::new()?;
    let payload = f.payload()?;
    let s = lifecycle(&f, &payload)?;
    let wm = watermark(&s, 4, "latest")?;
    assert!(invoke(&f, &s, &payload, &watermark(&s, 1, "old")?, "latest").is_err());
    for field in ["count", "chain_sha256", "observed_head_sha"] {
        let mut w: Value = serde_json::from_slice(&wm)?;
        w[field] = if field == "count" {
            json!(3)
        } else {
            json!("0".repeat(64))
        };
        assert!(
            invoke(&f, &s, &payload, &serde_json::to_vec(&w)?, "latest").is_err(),
            "{field}"
        );
    }
    let bytes = serde_json::to_vec(&s)?;
    assert!(
        f.with_source(|source| export_recorded_observations(ObservationInput {
            source,
            payload: &payload,
            payload_sha256: &sha256_hex(&payload),
            stream: &bytes,
            stream_sha256: &sha256_hex(&bytes),
            watermark: &wm,
            watermark_sha256: &"0".repeat(64),
            watermark_name: "latest"
        }))
        .is_err()
    );
    let mut exported = invoke(&f, &s, &payload, &wm, "latest")?;
    let mut v = view(&exported)?;
    v["whole_review_delivery"] = json!("confirmed");
    exported.observation_json = serde_json::to_vec(&v)?;
    assert!(
        f.with_source(|source| replay_recorded_observations(
            ObservationInput {
                source,
                payload: &payload,
                payload_sha256: &sha256_hex(&payload),
                stream: &bytes,
                stream_sha256: &sha256_hex(&bytes),
                watermark: &wm,
                watermark_sha256: &sha256_hex(&wm),
                watermark_name: "latest"
            },
            &exported,
            &sha256_hex(&exported.observation_json)
        ))
        .is_err()
    );
    Ok(())
}

#[test]
fn receipt_relationships_conflicting_events_and_physical_ids_are_rejected() -> Result<()> {
    let f = Fixture::new()?;
    let payload = f.payload()?;
    for shape in [
        "wrong_body",
        "wrong_comment",
        "partial",
        "wrong_head",
        "forged_receipt",
    ] {
        let mut s = f.stream(&payload)?;
        append(&mut s, "delivery-1", prepared())?;
        let mut confirmation = f.confirmation("987", "123")?;
        if let RecordedObservation::ConfirmedInline {
            observed,
            reconciliation,
        } = &mut confirmation
        {
            match shape {
                "wrong_body" => {
                    observed[0] = ObservedDelivery::new(
                        "123",
                        PlannedDelivery::new(
                            &f.delivery.head_sha,
                            "A",
                            DeliveryAction::Inline,
                            DeliveryLocation::new("src/lib.rs", 1, "RIGHT"),
                            None,
                            "0".repeat(64),
                        )?,
                    )?
                }
                "wrong_comment" => observed[0] = ObservedDelivery::new("456", f.planned()?)?,
                "partial" => observed.clear(),
                "wrong_head" => {
                    observed[0] = ObservedDelivery::new(
                        "123",
                        PlannedDelivery::new(
                            "f".repeat(40),
                            "A",
                            DeliveryAction::Inline,
                            DeliveryLocation::new("src/lib.rs", 1, "RIGHT"),
                            None,
                            sha256_hex(INLINE.as_bytes()),
                        )?,
                    )?
                }
                _ => {
                    let mut r: Value = serde_json::from_slice(&reconciliation.bytes)?;
                    r["receipts"][0]["path"] = json!("other.rs");
                    reconciliation.bytes = serde_json::to_vec(&r)?;
                    reconciliation.sha256 = sha256_hex(&reconciliation.bytes);
                }
            }
        }
        append(&mut s, "delivery-1", confirmation)?;
        assert!(
            invoke(&f, &s, &payload, &watermark(&s, 2, "latest")?, "latest").is_err(),
            "{shape}"
        );
    }
    for (review, comment) in [("0987", "123"), ("987", "00123"), ("+987", "123")] {
        let mut s = f.stream(&payload)?;
        append(&mut s, "delivery-1", prepared())?;
        append(&mut s, "delivery-1", f.confirmation(review, comment)?)?;
        assert!(
            invoke(&f, &s, &payload, &watermark(&s, 2, "latest")?, "latest").is_err(),
            "noncanonical physical id {review}/{comment}"
        );
    }
    for duplicate in [prepared(), f.ambiguous()?, f.confirmation("987", "123")?] {
        let mut s = f.stream(&payload)?;
        append(&mut s, "delivery-1", prepared())?;
        append(&mut s, "delivery-1", f.ambiguous()?)?;
        append(&mut s, "delivery-1", f.confirmation("987", "123")?)?;
        append(&mut s, "delivery-1", duplicate)?;
        assert!(invoke(&f, &s, &payload, &watermark(&s, 4, "latest")?, "latest").is_err());
    }
    let mut s = f.stream(&payload)?;
    append(&mut s, "delivery-1", prepared())?;
    append(&mut s, "delivery-1", f.ambiguous()?)?;
    append(
        &mut s,
        "delivery-2",
        RecordedObservation::Prepared {
            retry_of: Some("delivery-1".to_owned()),
        },
    )?;
    append(&mut s, "delivery-1", f.confirmation("987", "123")?)?;
    append(&mut s, "delivery-2", f.confirmation("988", "123")?)?;
    assert!(invoke(&f, &s, &payload, &watermark(&s, 5, "latest")?, "latest").is_err());
    Ok(())
}

#[test]
fn unknown_fields_and_size_limits_do_not_enter_recorded_authority() -> Result<()> {
    let f = Fixture::new()?;
    let payload = f.payload()?;
    let mut s = f.stream(&payload)?;
    append(&mut s, "delivery-1", prepared())?;
    let wm = watermark(&s, 1, "latest")?;
    let mut root = serde_json::to_value(&s)?;
    root["authority"] = json!("posted");
    assert!(invoke_bytes(&f, &serde_json::to_vec(&root)?, &payload, &wm, "latest").is_err());
    let mut p: Value = serde_json::from_slice(&payload)?;
    p["dispatch"] = json!(true);
    let p = serde_json::to_vec(&p)?;
    assert!(invoke(&f, &s, &p, &wm, "latest").is_err());
    let mut w: Value = serde_json::from_slice(&wm)?;
    w["authority"] = json!(true);
    assert!(invoke(&f, &s, &payload, &serde_json::to_vec(&w)?, "latest").is_err());
    let mut e: Value = serde_json::from_slice(&s.events[0])?;
    e["authority"] = json!(true);
    s.events[0] = serde_json::to_vec(&e)?;
    assert!(invoke(&f, &s, &payload, &watermark(&s, 1, "latest")?, "latest").is_err());
    let mut s = f.stream(&payload)?;
    append(&mut s, "delivery-1", prepared())?;
    append(&mut s, "delivery-1", f.confirmation("987", "123")?)?;
    let mut e: Value = serde_json::from_slice(&s.events[1])?;
    e["observation"]["observed"][0]["delivery"]["authority"] = json!(true);
    s.events[1] = serde_json::to_vec(&e)?;
    assert!(invoke(&f, &s, &payload, &watermark(&s, 2, "latest")?, "latest").is_err());
    assert!(invoke_bytes(&f, &vec![b' '; BYTE_LIMIT + 1], &payload, &wm, "latest").is_err());
    Ok(())
}

#[test]
fn local_failure_receipts_and_nested_confirmation_fields_are_bound() -> Result<()> {
    let f = Fixture::new()?;
    let payload = f.payload()?;
    for shape in [
        "stage",
        "planned_body",
        "transaction_unknown",
        "receipt_unknown",
        "receipt_pin",
    ] {
        let mut s = f.stream(&payload)?;
        append(&mut s, "delivery-1", prepared())?;
        let mut observation = if shape.starts_with("receipt") {
            f.confirmation("987", "123")?
        } else {
            f.ambiguous()?
        };
        match &mut observation {
            RecordedObservation::Ambiguous {
                stage,
                local_receipt,
            } => {
                if shape == "stage" {
                    *stage = DeliveryFailureStage::ReceiptPersistence;
                } else {
                    let mut transaction: Value = serde_json::from_slice(&local_receipt.bytes)?;
                    if shape == "planned_body" {
                        transaction["planned"][0]["expected_body_digest"] = json!("0".repeat(64));
                    } else {
                        transaction["failure"]["authority"] = json!(true);
                    }
                    local_receipt.bytes = serde_json::to_vec(&transaction)?;
                    local_receipt.sha256 = sha256_hex(&local_receipt.bytes);
                }
            }
            RecordedObservation::ConfirmedInline { reconciliation, .. } => {
                if shape == "receipt_pin" {
                    reconciliation.sha256 = "0".repeat(64);
                } else {
                    let mut receipt: Value = serde_json::from_slice(&reconciliation.bytes)?;
                    receipt["receipts"][0]["authority"] = json!(true);
                    reconciliation.bytes = serde_json::to_vec(&receipt)?;
                    reconciliation.sha256 = sha256_hex(&reconciliation.bytes);
                }
            }
            _ => return Err(anyhow::anyhow!("fixture observation missing")),
        }
        append(&mut s, "delivery-1", observation)?;
        assert!(
            invoke(&f, &s, &payload, &watermark(&s, 2, "latest")?, "latest").is_err(),
            "{shape}"
        );
    }
    Ok(())
}

#[test]
fn recorded_payload_body_and_comment_byte_boundaries() -> Result<()> {
    for size in [6000, 6001] {
        let body = "x".repeat(size);
        let empty: Vec<PlannedDelivery> = vec![];
        let payload = serde_json::to_vec(&json!({"event":"COMMENT","body":body,"comments":[]}))?;
        let result = payload_matches_inline(&payload, &empty);
        if size == 6000 {
            result?;
        } else {
            assert!(
                result
                    .err()
                    .is_some_and(|e| e.to_string().contains("unsupported recorded payload"))
            );
        }
        let f = Fixture::new()?;
        let planned = PlannedDelivery::new(
            &f.delivery.head_sha,
            "A",
            DeliveryAction::Inline,
            DeliveryLocation::new("src/lib.rs", 1, "RIGHT"),
            None,
            sha256_hex(body.as_bytes()),
        )?;
        let payload = serde_json::to_vec(
            &json!({"event":"COMMENT","body":"","comments":[{"path":"src/lib.rs","line":1,"side":"RIGHT","body":body}]}),
        )?;
        let result = payload_matches_inline(&payload, &[planned]);
        if size == 6000 {
            result?;
        } else {
            assert!(
                result
                    .err()
                    .is_some_and(|e| e.to_string().contains("payload body limit"))
            );
        }
    }
    Ok(())
}

#[test]
fn recorded_stream_byte_boundary_accepts_valid_json_and_rejects_overflow() -> Result<()> {
    let f = Fixture::new()?;
    let payload = f.payload()?;
    let mut s = f.stream(&payload)?;
    append(&mut s, "delivery-1", prepared())?;
    let wm = watermark(&s, 1, "latest")?;
    let mut bytes = serde_json::to_vec(&s)?;
    assert!(bytes.len() < BYTE_LIMIT);
    bytes.resize(BYTE_LIMIT, b' ');
    invoke_bytes(&f, &bytes, &payload, &wm, "latest")?;
    bytes.push(b' ');
    let error = invoke_bytes(&f, &bytes, &payload, &wm, "latest")
        .err()
        .ok_or_else(|| anyhow::anyhow!("oversized valid stream admitted"))?;
    assert!(error.to_string().contains("recorded byte limit"));
    Ok(())
}
