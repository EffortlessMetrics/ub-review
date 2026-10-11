use super::*;
use crate::claim_graph::{
    ClaimGraph, ClaimNode, ClaimState, EvidenceClass, EvidenceRef, RelevanceKind, RelevancePath,
};
use crate::review_judgment::{ImportedCi, RecordedEvidence, source_manifest_digest};
use crate::revision_identity::{CommitTree, ReviewSemantics};
use serde_json::{Value, json};

mod recorded_export;

const PATCH: &str = "diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-safe();\n+danger();\n";
const BODY_A: &[u8] = b"This unchecked index may read beyond the allocation.";
const BODY_B: &[u8] = b"The summary-only claim remains material without an inline anchor.";

struct Fixture {
    identity: RevisionIdentity,
    canonical: String,
    subject: JudgmentSubject,
    delivery: DeliverySubject,
    graph: ClaimGraph,
    evidence_digest: String,
    ci: Vec<u8>,
    ci_digest: String,
    body_a_digest: String,
    body_b_digest: String,
    line_digest: String,
    diff_digest: String,
}

impl Fixture {
    fn new() -> Result<Self> {
        let head = CommitTree::new("head", &"a".repeat(40), &"b".repeat(40))?;
        let diff_digest = RevisionIdentity::diff_digest(PATCH.as_bytes());
        let identity = RevisionIdentity::new(
            CommitTree::new("base", &"c".repeat(40), &"d".repeat(40))?,
            head.clone(),
            head,
            None,
            ReviewSemantics::CandidateHead,
            &RevisionIdentity::changed_paths_digest(["src/lib.rs"]),
            &diff_digest,
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
        for id in ["A", "B"] {
            graph.claims.push(ClaimNode {
                id: id.to_owned(),
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
        let ci = b"{\"result\":\"not_proven\",\"required_gap\":true}\n".to_vec();
        Ok(Self {
            canonical: identity.canonical_form(),
            identity,
            subject: JudgmentSubject {
                repository: "owner/repo".to_owned(),
                review_spec_digest: "e".repeat(64),
                pass_id: "fresh-pass".to_owned(),
                attempt_id: "attempt-1".to_owned(),
            },
            delivery: DeliverySubject {
                repository: "owner/repo".to_owned(),
                pull_number: 7,
                head_sha: "a".repeat(40),
            },
            graph,
            evidence_digest: sha256_hex(b"source citation"),
            ci_digest: sha256_hex(&ci),
            ci,
            body_a_digest: sha256_hex(BODY_A),
            body_b_digest: sha256_hex(BODY_B),
            line_digest: sha256_hex(b"danger();"),
            diff_digest,
        })
    }

    fn materials(&self) -> Vec<RecordedMaterial<'_>> {
        vec![
            RecordedMaterial {
                claim_id: "A",
                body_ref: "public#A",
                body_digest: &self.body_a_digest,
                class: MaterialClass::Public,
                body: BODY_A,
            },
            RecordedMaterial {
                claim_id: "B",
                body_ref: "public#B",
                body_digest: &self.body_b_digest,
                class: MaterialClass::Public,
                body: BODY_B,
            },
        ]
    }
    fn anchors(&self) -> Vec<RecordedAnchor<'_>> {
        vec![
            RecordedAnchor {
                reference: "anchor#A",
                claim_id: "A",
                path: "src/lib.rs",
                line: 1,
                line_digest: &self.line_digest,
            },
            RecordedAnchor {
                reference: "anchor#B",
                claim_id: "B",
                path: "src/lib.rs",
                line: 1,
                line_digest: &self.line_digest,
            },
        ]
    }
    fn source<'a>(&'a self, evidence: &'a [RecordedEvidence<'a>]) -> RecordedJudgmentSource<'a> {
        RecordedJudgmentSource {
            subject: &self.subject,
            revision_canonical: &self.canonical,
            graph: &self.graph,
            review_complete: true,
            evidence,
            imported_ci: Some(ImportedCi {
                digest: &self.ci_digest,
                bytes: &self.ci,
            }),
        }
    }
    fn public<'a>(
        &'a self,
        materials: &'a [RecordedMaterial<'a>],
        anchors: &'a [RecordedAnchor<'a>],
    ) -> RecordedPublicSource<'a> {
        RecordedPublicSource {
            delivery_subject: &self.delivery,
            diff_digest: &self.diff_digest,
            diff_patch: PATCH.as_bytes(),
            materials,
            anchors,
        }
    }
    fn answer(&self, source_digest: &str) -> Value {
        json!({"schema":"ub-review.recorded-judgment.v1", "subject":self.subject, "revision":self.identity.revision_ref(), "source_manifest_digest":source_digest,
            "review_result":"findings", "selections":[{"claim_id":"A", "evidence_refs":["fact#1"]}, {"claim_id":"B", "evidence_refs":["fact#1"]}]})
    }
    fn plan(&self, answer_digest: &str, public_digest: &str) -> Result<Value> {
        Ok(
            json!({"schema":SCHEMA, "judgment_digest":answer_digest, "revision":self.identity.revision_ref(), "subject":self.subject,
            "delivery_subject":self.delivery, "public_source_digest":public_digest, "quiet":false,
            "items":[{"claim_id":"A", "disposition":{"kind":"inline", "body_ref":"public#A", "anchor_ref":"anchor#A", "delivery":self.inline("A", &self.body_a_digest)?}},
                {"claim_id":"B", "disposition":{"kind":"summary", "body_ref":"public#B", "body_digest":self.body_b_digest}}]}),
        )
    }
    fn inline(&self, claim: &str, body_digest: &str) -> Result<PlannedDelivery> {
        PlannedDelivery::new(
            &self.delivery.head_sha,
            claim,
            DeliveryAction::Inline,
            DeliveryLocation::new("src/lib.rs", 1, "RIGHT"),
            None,
            body_digest,
        )
    }
}

/// Every mutated plan is independently repinned: failures must be semantic
/// identity/completeness controls, not just stale plan byte hashes.
fn invoke(
    f: &Fixture,
    materials: &[RecordedMaterial<'_>],
    anchors: &[RecordedAnchor<'_>],
    mutate: impl FnOnce(&mut Value),
) -> Result<RecordedPublicPlan> {
    let evidence = [RecordedEvidence {
        reference: "fact#1",
        digest: &f.evidence_digest,
        bytes: b"source citation",
    }];
    let source = f.source(&evidence);
    let source_digest = source_manifest_digest(&source)?;
    let public = f.public(materials, anchors);
    let public_digest = public_source_digest(&public)?;
    let answer = serde_json::to_vec(&f.answer(&source_digest))?;
    let answer_digest = sha256_hex(&answer);
    let mut plan = f.plan(&answer_digest, &public_digest)?;
    mutate(&mut plan);
    let plan = serde_json::to_vec(&plan)?;
    validate_recorded_public_plan(PublicPlanInput {
        judgment_source: &source,
        current_revision: &f.identity,
        expected_source_digest: &source_digest,
        answer: Some(&answer),
        expected_answer_digest: Some(&answer_digest),
        observed_ci: Some(&f.ci),
        public_source: &public,
        expected_public_source_digest: &public_digest,
        plan: Some(&plan),
        expected_plan_digest: Some(&sha256_hex(&plan)),
    })
}

fn rejection(result: Result<RecordedPublicPlan>, reason: &str) {
    assert!(
        result
            .as_ref()
            .err()
            .is_some_and(|e| e.to_string().contains(reason)),
        "expected {reason}, got {result:?}"
    );
}

#[test]
fn summary_survives_inline_content_and_replay_is_exact() -> Result<()> {
    let f = Fixture::new()?;
    let materials = f.materials();
    let anchors = f.anchors();
    let first = invoke(&f, &materials, &anchors, |_| {})?;
    assert_eq!(first, invoke(&f, &materials, &anchors, |_| {})?);
    let mut without_summary_anchor = f.anchors();
    without_summary_anchor.pop();
    assert!(matches!(
        invoke(&f, &materials, &without_summary_anchor, |_| {})?,
        RecordedPublicPlan::Validated { .. }
    ));
    let RecordedPublicPlan::Validated { plan, plan_digest } = first else {
        anyhow::bail!("valid fixture was unavailable")
    };
    assert_eq!(plan.items.len(), 2);
    assert!(matches!(
        plan.items[1].disposition,
        PublicDisposition::Summary { .. }
    ));
    // Digests/refs only: raw public bodies, source audit and imported CI are not
    // copied into this future agent-facing source object.
    let bytes = serde_json::to_vec(&plan)?;
    // The immutable pin names the ORIGINAL recorded JSON bytes. Reserializing
    // the parsed struct changes field order and must not silently reseal it.
    assert_eq!(
        plan_digest,
        "0e175b97ebc061a8ebf54398d13b03e658f40dc80c0afd3722867306995a706d"
    );
    let text = std::str::from_utf8(&bytes)?;
    assert!(!text.contains("unchecked index") && !text.contains("required_gap"));
    rejection(
        invoke(&f, &materials, &anchors, |p| {
            p["items"].as_array_mut().map(|a| a.pop());
        }),
        "lead-selected claim lost",
    );
    rejection(
        invoke(&f, &materials, &anchors, |p| {
            p["quiet"] = json!(true);
            p["items"] = json!([]);
        }),
        "unsupported deliberate silence",
    );
    Ok(())
}

#[test]
fn duplicate_locations_forged_subjects_and_reference_tampering_fail() -> Result<()> {
    let f = Fixture::new()?;
    let materials = f.materials();
    let anchors = f.anchors();
    for (path, replacement, reason) in [
        (
            "/judgment_digest",
            json!("f".repeat(64)),
            "identity mismatch",
        ),
        (
            "/revision/digest",
            json!("f".repeat(64)),
            "identity mismatch",
        ),
        (
            "/revision/reviewed_commit",
            json!("f".repeat(40)),
            "identity mismatch",
        ),
        (
            "/subject/pass_id",
            json!("old-clean-pass"),
            "identity mismatch",
        ),
        (
            "/subject/review_spec_digest",
            json!("f".repeat(64)),
            "identity mismatch",
        ),
        (
            "/delivery_subject/repository",
            json!("other/repo"),
            "identity mismatch",
        ),
        (
            "/delivery_subject/pull_number",
            json!(8),
            "identity mismatch",
        ),
        (
            "/delivery_subject/head_sha",
            json!("f".repeat(40)),
            "identity mismatch",
        ),
        (
            "/items/0/disposition/anchor_ref",
            json!("anchor#B"),
            "unknown claim anchor",
        ),
        (
            "/items/0/disposition/delivery/exact_head_sha",
            json!("f".repeat(40)),
            "inline delivery subject mismatch",
        ),
        (
            "/items/1/disposition/body_ref",
            json!("private#unknown"),
            "summary body identity",
        ),
        (
            "/items/1/disposition/body_digest",
            json!("f".repeat(64)),
            "summary body identity",
        ),
    ] {
        rejection(
            invoke(&f, &materials, &anchors, |p| {
                *p.pointer_mut(path).unwrap_or(&mut Value::Null) = replacement;
            }),
            reason,
        );
    }
    rejection(
        invoke(&f, &materials, &anchors, |p| {
            p["items"][1] = p["items"][0].clone();
        }),
        "duplicate or unselected public claim",
    );
    let second_inline = f.inline("B", &f.body_b_digest)?;
    rejection(
        invoke(&f, &materials, &anchors, |p| {
            p["items"][1]["disposition"] = json!({"kind":"inline", "body_ref":"public#B", "anchor_ref":"anchor#B", "delivery":second_inline});
        }),
        "duplicate public location",
    );
    for field in [
        "ci_evidence_result",
        "private_audit",
        "provider",
        "argv",
        "delivery_confirmation",
    ] {
        rejection(
            invoke(&f, &materials, &anchors, |p| {
                p[field] = json!("forged");
            }),
            "unknown field",
        );
        rejection(
            invoke(&f, &materials, &anchors, |p| {
                p["revision"][field] = json!("forged");
            }),
            "unknown plan revision field",
        );
        rejection(
            invoke(&f, &materials, &anchors, |p| {
                p["items"][0]["disposition"][field] = json!("forged");
            }),
            "unknown field",
        );
    }
    Ok(())
}

#[test]
fn nested_delivery_firewall_preserves_exact_positive_bytes() -> Result<()> {
    let f = Fixture::new()?;
    let materials = f.materials();
    let anchors = f.anchors();
    let ci_before = f.ci.clone();
    let ci_digest_before = sha256_hex(&ci_before);
    let validated = invoke(&f, &materials, &anchors, |_| {})?;
    let RecordedPublicPlan::Validated { plan, plan_digest } = validated else {
        anyhow::bail!("expected valid recorded plan");
    };
    assert_eq!(
        plan_digest,
        "0e175b97ebc061a8ebf54398d13b03e658f40dc80c0afd3722867306995a706d"
    );
    let expected = f.inline("A", &f.body_a_digest)?;
    let bytes = serde_json::to_vec(&expected)?;
    let roundtrip: PlannedDelivery = serde_json::from_slice(&bytes)?;
    assert_eq!(roundtrip, expected);
    assert_eq!(serde_json::to_vec(&roundtrip)?, bytes);
    assert!(
        matches!(&plan.items[0].disposition, PublicDisposition::Inline { delivery, .. } if *delivery == expected)
    );
    assert_eq!(f.ci, ci_before);
    assert_eq!(sha256_hex(&f.ci), ci_digest_before);
    for field in [
        "ci_evidence_result",
        "required",
        "policy",
        "dispatch",
        "argv",
        "delivery_confirmation",
        "check_exit_status",
        "private_audit",
        "arbitrary_unknown",
        "nested_unknown",
    ] {
        for payload in [json!("forged"), json!({"nested": {"authority": true}})] {
            rejection(
                invoke(&f, &materials, &anchors, |p| {
                    p["items"][0]["disposition"]["delivery"][field] = payload;
                }),
                "unknown plan delivery field",
            );
        }
    }
    for field in ["source_thread_id", "action", "expected_body_digest", "line"] {
        assert!(
            invoke(&f, &materials, &anchors, |p| {
                p["items"][0]["disposition"]["delivery"][field] =
                    json!({"arbitrary": {"unknown": true}});
            })
            .is_err(),
            "nested object accepted in {field}"
        );
    }
    Ok(())
}

#[test]
fn private_and_operational_material_require_qualified_withholding() -> Result<()> {
    let f = Fixture::new()?;
    let anchors = f.anchors();
    let mut materials = f.materials();
    materials[1].class = MaterialClass::PrivateAudit;
    rejection(
        invoke(&f, &materials, &anchors, |_| {}),
        "private material in public plan",
    );
    let withheld = invoke(&f, &materials, &anchors, |p| {
        p["items"][1]["disposition"] = json!({"kind":"withheld", "reason":"private_audit"});
    })?;
    assert!(matches!(withheld, RecordedPublicPlan::Validated { .. }));
    materials[1].class = MaterialClass::Operational;
    rejection(
        invoke(&f, &materials, &anchors, |p| {
            p["items"][1]["disposition"] = json!({"kind":"withheld", "reason":"private_audit"});
        }),
        "unqualified withholding",
    );
    assert!(matches!(
        invoke(&f, &materials, &anchors, |p| {
            p["items"][1]["disposition"] = json!({"kind":"withheld", "reason":"operational"});
        })?,
        RecordedPublicPlan::Validated { .. }
    ));
    materials[1].class = MaterialClass::Public;
    rejection(
        invoke(&f, &materials, &anchors, |p| {
            p["items"][1]["disposition"] = json!({"kind":"withheld", "reason":"private_audit"});
        }),
        "unqualified withholding",
    );
    let private_body = b"OPENCODE = never-publish-this-key";
    let private_digest = sha256_hex(private_body);
    materials[1].body = private_body;
    materials[1].body_digest = &private_digest;
    rejection(invoke(&f, &materials, &anchors, |_| {}), "needs redaction");
    Ok(())
}

#[test]
fn independent_body_anchor_and_input_size_controls_fail_closed() -> Result<()> {
    let f = Fixture::new()?;
    let mut materials = f.materials();
    let mut anchors = f.anchors();
    materials[0].body = b"tampered source body";
    rejection(
        invoke(&f, &materials, &anchors, |_| {}),
        "body bytes mismatch",
    );
    materials = f.materials();
    anchors[0].line_digest = "forged";
    rejection(
        invoke(&f, &materials, &anchors, |_| {}),
        "anchor text digest mismatch",
    );
    anchors = f.anchors();
    anchors[0].line = 20;
    rejection(invoke(&f, &materials, &anchors, |_| {}), "anchor absent");
    let huge = vec![b'x'; MAX_BODY_BYTES + 1];
    let digest = sha256_hex(&huge);
    materials[0].body = &huge;
    materials[0].body_digest = &digest;
    rejection(invoke(&f, &materials, &f.anchors(), |_| {}), "oversize");
    let huge_metadata = vec![b'a'; MAX_SOURCE_BYTES + 1];
    assert!(bounded_json(&String::from_utf8(huge_metadata)?).is_err());
    Ok(())
}

#[test]
fn quiet_clean_absent_invalid_lead_and_ci_missingness_are_explicit() -> Result<()> {
    let mut f = Fixture::new()?;
    f.graph.claims.clear();
    let evidence = [RecordedEvidence {
        reference: "fact#1",
        digest: &f.evidence_digest,
        bytes: b"source citation",
    }];
    let source = f.source(&evidence);
    let digest = source_manifest_digest(&source)?;
    let public = f.public(&[], &[]);
    let public_digest = public_source_digest(&public)?;
    let mut answer = f.answer(&digest);
    answer["review_result"] = json!("clean");
    answer["selections"] = json!([]);
    let answer = serde_json::to_vec(&answer)?;
    let answer_digest = sha256_hex(&answer);
    let mut plan = f.plan(&answer_digest, &public_digest)?;
    plan["quiet"] = json!(true);
    plan["items"] = json!([]);
    let plan = serde_json::to_vec(&plan)?;
    let plan_digest = sha256_hex(&plan);
    let input = || PublicPlanInput {
        judgment_source: &source,
        current_revision: &f.identity,
        expected_source_digest: &digest,
        answer: Some(&answer),
        expected_answer_digest: Some(&answer_digest),
        observed_ci: Some(&f.ci),
        public_source: &public,
        expected_public_source_digest: &public_digest,
        plan: Some(&plan),
        expected_plan_digest: Some(&plan_digest),
    };
    assert!(
        matches!(validate_recorded_public_plan(input())?, RecordedPublicPlan::Validated { plan, .. } if plan.quiet && plan.items.is_empty())
    );
    let mut missing = input();
    missing.plan = None;
    missing.expected_plan_digest = None;
    assert_eq!(
        validate_recorded_public_plan(missing)?,
        RecordedPublicPlan::PlanAbsent
    );
    let mut absent = input();
    absent.plan = None;
    absent.expected_plan_digest = None;
    absent.answer = None;
    absent.expected_answer_digest = None;
    assert_eq!(
        validate_recorded_public_plan(absent)?,
        RecordedPublicPlan::LeadAbsent
    );
    let mut fabricated = input();
    fabricated.answer = None;
    fabricated.expected_answer_digest = None;
    rejection(
        validate_recorded_public_plan(fabricated),
        "plan without recorded lead",
    );
    let mut invalid = input();
    invalid.answer = Some(b"not JSON");
    invalid.plan = None;
    invalid.expected_plan_digest = None;
    assert_eq!(
        validate_recorded_public_plan(invalid)?,
        RecordedPublicPlan::LeadInvalid
    );
    let mut changed_ci = f.ci.clone();
    changed_ci.push(b' ');
    let mut corrupted = input();
    corrupted.observed_ci = Some(&changed_ci);
    rejection(
        validate_recorded_public_plan(corrupted),
        "imported CI bytes changed",
    );
    let mut corrupted = input();
    corrupted.observed_ci = None;
    rejection(
        validate_recorded_public_plan(corrupted),
        "CI observation missing",
    );
    let oversized = vec![b' '; MAX_OBJECT_BYTES + 1];
    let mut large = input();
    large.plan = Some(&oversized);
    rejection(
        validate_recorded_public_plan(large),
        "object exceeds byte limit",
    );
    Ok(())
}
