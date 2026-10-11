use super::*;
use crate::review_judgment::public_plan::payload::{
    PreparedPayloadBinding, RecordedPreparedPayload, payload_source_digest,
    validate_prepared_payload,
};

fn mixed_review() -> Value {
    json!({"event":"COMMENT", "body":String::from_utf8_lossy(BODY_B), "comments":[{
        "path":"src/lib.rs","line":1,"side":"RIGHT","body":String::from_utf8_lossy(BODY_A)
    }]})
}

fn case(
    f: &Fixture,
    materials: &[RecordedMaterial<'_>],
    anchors: &[RecordedAnchor<'_>],
    mut review: Value,
    edit: impl FnOnce(&mut Value, &mut Value, &mut JudgmentSubject, &mut DeliverySubject, &mut &[u8]),
) -> Result<PreparedPayloadBinding> {
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
    let mut subject = f.subject.clone();
    let mut delivery_subject = f.delivery.clone();
    let mut observed_ci = f.ci.as_slice();
    edit(
        &mut plan,
        &mut review,
        &mut subject,
        &mut delivery_subject,
        &mut observed_ci,
    );
    let plan = serde_json::to_vec(&plan)?;
    let plan_digest = sha256_hex(&plan);
    let review = serde_json::to_vec(&review)?;
    let review_digest = sha256_hex(&review);
    let revision = f.identity.revision_ref();
    let payload = RecordedPreparedPayload {
        subject: &subject,
        delivery_subject: &delivery_subject,
        revision: &revision,
        judgment_digest: &answer_digest,
        public_plan_digest: &plan_digest,
        payload_digest: &review_digest,
        bytes: &review,
    };
    let payload_pin = payload_source_digest(&payload)?;
    validate_prepared_payload(
        PublicPlanInput {
            judgment_source: &source,
            current_revision: &f.identity,
            expected_source_digest: &source_digest,
            answer: Some(&answer),
            expected_answer_digest: Some(&answer_digest),
            observed_ci: Some(observed_ci),
            public_source: &public,
            expected_public_source_digest: &public_digest,
            plan: Some(&plan),
            expected_plan_digest: Some(&plan_digest),
        },
        &payload,
        &payload_pin,
    )
}

#[test]
fn binds_real_compiler_whole_body_without_rendering_or_delivery_inference() -> Result<()> {
    let mut f = Fixture::new()?;
    let args = crate::tests::test_run_args(std::path::PathBuf::from("recorded-unused"));
    let plan = crate::tests::test_plan(vec![]);
    let mut diff = crate::tests::test_diff();
    diff.head = f.delivery.head_sha.clone();
    diff.patch = PATCH.to_owned();
    let summaries = [crate::SummaryOnlyFinding {
        lane: "recorded".into(),
        severity: "high".into(),
        confidence: "high".into(),
        reason: "The unchecked index reads beyond the allocation when the input is empty.".into(),
        evidence: "exact citation".into(),
    }];
    let policy = crate::ReviewBodyPolicy::default();
    let compiled = crate::compile_review_surface(crate::ReviewCompilerInput {
        shared_context_id: "recorded",
        review_body_policy: &policy,
        run_pass: crate::RunPass::Manual,
        post_review_on: &[],
        args: &args,
        plan: &plan,
        diff: &diff,
        model_lanes: &[],
        missing_or_failed_sensor_evidence: &[],
        missing_or_failed_model_evidence: &[],
        inline_comments: &[],
        summary_only_findings: &summaries,
        observations: &[],
        proof_receipts: &[],
        suggested_issues: &[],
        final_follow_up_tasks: 0,
        reporter_distillation: None,
    })?;
    assert!(compiled.should_prepare_github_review);
    let actual_body = compiled.github_review.body.as_bytes();
    assert!(!actual_body.is_empty());
    f.body_b_digest = sha256_hex(actual_body);
    let mut materials = f.materials();
    materials[1].body = actual_body;
    let mut review = serde_json::to_value(&compiled.github_review)?;
    // The canonical recorded lead still selects both A and B; keep A as an
    // admitted withheld operational item, with only B owning the public body.
    materials[0].class = MaterialClass::Operational;
    let ci_before = f.ci.clone();
    let payload_before = serde_json::to_vec(&compiled.github_review)?;
    let binding = case(&f, &materials, &[], review.take(), |p, _, _, _, _| {
        p["items"][0]["disposition"] = json!({"kind":"withheld","reason":"operational"});
    })?;
    let value = serde_json::to_value(&binding)?;
    assert_eq!(value["whole_body_claim"], "B");
    assert_eq!(value["qualification"], "shadow_direct_slots_only");
    assert!(value.get("delivery_result").is_none());
    assert!(value.get("ci_result").is_none());
    assert_eq!(f.ci, ci_before);
    assert_eq!(serde_json::to_vec(&compiled.github_review)?, payload_before);
    Ok(())
}

#[test]
fn summary_loss_extra_content_and_changed_inline_slots_fail_even_repinned() -> Result<()> {
    let f = Fixture::new()?;
    let materials = f.materials();
    let anchors = f.anchors();
    let positive = case(&f, &materials, &anchors, mixed_review(), |_, _, _, _, _| {})?;
    assert_eq!(
        serde_json::to_value(&positive)?["inline_claims"],
        json!(["A"])
    );
    for body in [
        "",
        "changed public wording",
        "The summary-only claim remains material without an inline anchor. Extra finding.",
    ] {
        assert!(
            case(&f, &materials, &anchors, mixed_review(), |_, r, _, _, _| {
                r["body"] = json!(body)
            })
            .is_err()
        );
    }
    for field in ["body", "path", "side"] {
        assert!(
            case(&f, &materials, &anchors, mixed_review(), |_, r, _, _, _| {
                r["comments"][0][field] = json!("changed")
            })
            .is_err()
        );
    }
    assert!(
        case(&f, &materials, &anchors, mixed_review(), |_, r, _, _, _| {
            r["comments"] = json!([])
        })
        .is_err()
    );
    assert!(
        case(&f, &materials, &anchors, mixed_review(), |_, r, _, _, _| {
            let extra = r["comments"][0].clone();
            if let Some(comments) = r["comments"].as_array_mut() {
                comments.push(extra);
            }
        })
        .is_err()
    );
    Ok(())
}

#[test]
fn source_subject_pass_and_legacy_unknown_fields_cannot_gain_binding() -> Result<()> {
    let f = Fixture::new()?;
    let materials = f.materials();
    let anchors = f.anchors();
    assert!(
        case(&f, &materials, &anchors, mixed_review(), |_, _, s, _, _| {
            s.pass_id = "another-pass".into()
        })
        .is_err()
    );
    assert!(
        case(&f, &materials, &anchors, mixed_review(), |_, _, s, _, _| {
            s.repository = "other/repo".into()
        })
        .is_err()
    );
    assert!(
        case(&f, &materials, &anchors, mixed_review(), |_, _, _, d, _| {
            d.head_sha = "f".repeat(40)
        })
        .is_err()
    );
    for field in [
        "ci_result",
        "required",
        "dispatch",
        "private_audit",
        "delivery_confirmation",
        "arbitrary_unknown",
    ] {
        assert!(
            case(&f, &materials, &anchors, mixed_review(), |_, r, _, _, _| {
                r[field] = json!(true)
            })
            .is_err()
        );
        assert!(
            case(&f, &materials, &anchors, mixed_review(), |_, r, _, _, _| {
                r["comments"][0][field] = json!({"nested":true})
            })
            .is_err()
        );
    }
    assert!(
        case(&f, &materials, &anchors, mixed_review(), |_, r, _, _, _| {
            r["comments"][0]["suggestion"] = json!("edit()")
        })
        .is_err()
    );
    assert!(
        case(&f, &materials, &anchors, mixed_review(), |_, r, _, _, _| {
            r["event"] = json!("APPROVE")
        })
        .is_err()
    );
    Ok(())
}

#[test]
fn withheld_private_material_never_fills_a_public_slot_and_ci_stays_opaque() -> Result<()> {
    let f = Fixture::new()?;
    let mut materials = f.materials();
    materials[1].class = MaterialClass::PrivateAudit;
    let anchors = f.anchors();
    let withheld = |p: &mut Value| {
        p["items"][1]["disposition"] = json!({"kind":"withheld","reason":"private_audit"})
    };
    assert!(
        case(&f, &materials, &anchors, mixed_review(), |p, _, _, _, _| {
            withheld(p)
        })
        .is_err()
    );
    let binding = case(&f, &materials, &anchors, mixed_review(), |p, r, _, _, _| {
        withheld(p);
        r["body"] = json!("");
    })?;
    assert!(serde_json::to_value(binding)?["whole_body_claim"].is_null());
    assert!(
        case(
            &f,
            &materials,
            &anchors,
            mixed_review(),
            |p, r, _, _, ci| {
                withheld(p);
                r["body"] = json!("");
                *ci = b"changed opaque CI";
            }
        )
        .is_err()
    );
    assert!(case(&f, &materials, &anchors, mixed_review(), |_, _, _, _, _| {}).is_err());
    Ok(())
}

#[test]
fn multiple_summary_fragments_require_actual_compiler_provenance() -> Result<()> {
    let f = Fixture::new()?;
    let materials = f.materials();
    // Both selected summaries are legitimate #1014 dispositions, but this
    // direct-slot profile cannot attribute two fragments inside one body.
    assert!(
        case(&f, &materials, &[], mixed_review(), |p, r, _, _, _| {
            p["items"][0]["disposition"] =
                json!({"kind":"summary","body_ref":"public#A","body_digest":f.body_a_digest});
            r["body"] = json!(String::from_utf8_lossy(BODY_A));
            r["comments"] = json!([]);
        })
        .is_err()
    );
    Ok(())
}
