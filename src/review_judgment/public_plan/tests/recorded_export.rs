use super::*;
use crate::review_judgment::public_plan::recorded_export::{
    RecordedReviewExport, export_recorded_review, replay_recorded_review_export,
};

fn with_input<T>(
    f: &Fixture,
    lead_present: bool,
    plan_present: bool,
    edit_lead: impl FnOnce(&mut Value),
    edit_plan: impl FnOnce(&mut Value),
    run: impl FnOnce(PublicPlanInput<'_>) -> Result<T>,
) -> Result<T> {
    let evidence = [RecordedEvidence {
        reference: "fact#1",
        digest: &f.evidence_digest,
        bytes: b"source citation",
    }];
    let source = f.source(&evidence);
    let source_digest = source_manifest_digest(&source)?;
    let materials = f.materials();
    let anchors = f.anchors();
    let public = f.public(&materials, &anchors);
    let public_digest = public_source_digest(&public)?;
    let mut answer = f.answer(&source_digest);
    edit_lead(&mut answer);
    let mut answer = serde_json::to_vec(&answer)?;
    // Noncanonical whitespace belongs to the immutable recorded object too.
    answer.push(b'\n');
    let answer_digest = sha256_hex(&answer);
    let mut plan = f.plan(&answer_digest, &public_digest)?;
    edit_plan(&mut plan);
    let mut plan = serde_json::to_vec(&plan)?;
    plan.push(b'\n');
    let plan_digest = sha256_hex(&plan);
    run(PublicPlanInput {
        judgment_source: &source,
        current_revision: &f.identity,
        expected_source_digest: &source_digest,
        answer: lead_present.then_some(answer.as_slice()),
        expected_answer_digest: lead_present.then_some(answer_digest.as_str()),
        observed_ci: Some(&f.ci),
        public_source: &public,
        expected_public_source_digest: &public_digest,
        plan: plan_present.then_some(plan.as_slice()),
        expected_plan_digest: plan_present.then_some(plan_digest.as_str()),
    })
}

#[test]
fn export_preserves_source_bytes_and_preparation_is_remotely_unknown() -> Result<()> {
    let mut f = Fixture::new()?;
    f.ci.extend([0, 255, b'\n']);
    f.ci_digest = sha256_hex(&f.ci);
    with_input(
        &f,
        true,
        true,
        |_| {},
        |_| {},
        |input| {
            let expected_answer = input.answer.map(<[u8]>::to_vec);
            let expected_plan = input.plan.map(<[u8]>::to_vec);
            let export = export_recorded_review(input)?;
            let view: Value = serde_json::from_slice(&export.view_json)?;
            let answer: Vec<u8> =
                serde_json::from_value(view["judgment"]["recorded_bytes"].clone())?;
            let plan: Vec<u8> = serde_json::from_value(view["public_plan_recorded_bytes"].clone())?;
            assert_eq!(Some(answer.clone()), expected_answer);
            assert_eq!(Some(plan.clone()), expected_plan);
            assert_eq!(sha256_hex(&answer), view["judgment"]["answer_digest"]);
            assert_eq!(sha256_hex(&plan), view["public_plan"]["plan_digest"]);
            assert_eq!(export.imported_ci_bytes.as_deref(), Some(f.ci.as_slice()));
            assert_eq!(
                sha256_hex(export.imported_ci_bytes.as_deref().unwrap_or_default()),
                view["imported_ci_digest"]
            );
            assert_eq!(view["judgment"]["judgment"]["review_result"], "findings");
            assert_eq!(
                view["public_plan"]["plan"]["items"][1]["disposition"]["kind"],
                "summary"
            );
            assert_eq!(view["delivery_projection"], "not_admitted");
            assert!(view["observation_watermark"].is_null());
            assert_eq!(view["remote_effects"], "unknown");
            assert!(!String::from_utf8_lossy(&export.view_json).contains("summary-only claim"));
            assert_eq!(
                view["current_revision"],
                serde_json::to_value(f.identity.revision_ref())?
            );
            Ok(())
        },
    )?;
    let export = with_input(&f, true, true, |_| {}, |_| {}, export_recorded_review)?;
    with_input(
        &f,
        true,
        true,
        |_| {},
        |_| {},
        |input| replay_recorded_review_export(input, &export, &sha256_hex(&export.view_json)),
    )
}

#[test]
fn replay_rejects_repinned_tampering_and_ci_whitespace_changes() -> Result<()> {
    let f = Fixture::new()?;
    let export = with_input(&f, true, true, |_| {}, |_| {}, export_recorded_review)?;
    for key in [
        "remote_effects",
        "observation_watermark",
        "source_manifest_digest",
        "source_currentness",
    ] {
        let mut view: Value = serde_json::from_slice(&export.view_json)?;
        view[key] = json!("forged");
        let mut bytes = serde_json::to_vec(&view)?;
        bytes.push(b'\n');
        let changed = RecordedReviewExport {
            view_json: bytes,
            imported_ci_bytes: export.imported_ci_bytes.clone(),
        };
        let error = with_input(
            &f,
            true,
            true,
            |_| {},
            |_| {},
            |input| replay_recorded_review_export(input, &changed, &sha256_hex(&changed.view_json)),
        );
        assert!(error.is_err(), "repinned altered {key} accepted");
    }
    let mut ci = export.imported_ci_bytes.clone().unwrap_or_default();
    ci.push(b' ');
    let changed = RecordedReviewExport {
        view_json: export.view_json.clone(),
        imported_ci_bytes: Some(ci),
    };
    let error = with_input(
        &f,
        true,
        true,
        |_| {},
        |_| {},
        |input| replay_recorded_review_export(input, &changed, &sha256_hex(&changed.view_json)),
    );
    assert!(error.is_err());
    with_input(
        &f,
        true,
        true,
        |_| {},
        |_| {},
        |input| {
            assert!(replay_recorded_review_export(input, &export, &"0".repeat(64)).is_err());
            Ok(())
        },
    )
}

#[test]
fn export_missingness_never_becomes_clean_or_remote_absence() -> Result<()> {
    let f = Fixture::new()?;
    for (present, invalid, expected, expected_plan) in [
        (false, false, "absent", "lead_absent"),
        (true, true, "invalid", "lead_invalid"),
        (true, false, "validated", "plan_absent"),
    ] {
        let export = with_input(
            &f,
            present,
            false,
            |lead| {
                if invalid {
                    lead["ci_result"] = json!("pass");
                }
            },
            |_| {},
            export_recorded_review,
        )?;
        let view: Value = serde_json::from_slice(&export.view_json)?;
        assert_eq!(view["judgment"]["availability"], expected);
        assert_eq!(view["public_plan"]["availability"], expected_plan);
        assert!(view["public_plan_recorded_bytes"].is_null());
        assert_eq!(view["remote_effects"], "unknown");
        assert_eq!(view["source_currentness"], "matches_admitted_revision");
        if expected != "validated" {
            assert!(view["judgment"]["judgment"].is_null());
            assert!(view["judgment"]["recorded_bytes"].is_null());
        }
    }
    Ok(())
}

#[test]
fn export_entrypoint_rejects_altered_source_objects_and_authority() -> Result<()> {
    let f = Fixture::new()?;
    for field in ["repository", "pass_id", "attempt_id"] {
        assert!(
            with_input(
                &f,
                true,
                true,
                |lead| lead["subject"][field] = json!("other"),
                |_| {},
                export_recorded_review
            )
            .is_err()
        );
    }
    for field in [
        "ci_result",
        "dispatch",
        "delivery_receipts",
        "observation_watermark",
    ] {
        assert!(
            with_input(
                &f,
                true,
                true,
                |_| {},
                |plan| plan[field] = json!("forged"),
                export_recorded_review
            )
            .is_err()
        );
    }
    assert!(
        with_input(
            &f,
            true,
            true,
            |_| {},
            |plan| {
                plan["items"].as_array_mut().map(|items| items.pop());
            },
            export_recorded_review
        )
        .is_err()
    );
    with_input(
        &f,
        true,
        true,
        |_| {},
        |_| {},
        |mut input| {
            input.observed_ci = Some(b"changed CI");
            assert!(export_recorded_review(input).is_err());
            Ok(())
        },
    )?;
    with_input(
        &f,
        true,
        true,
        |_| {},
        |_| {},
        |mut input| {
            input.expected_public_source_digest = "forged";
            assert!(export_recorded_review(input).is_err());
            Ok(())
        },
    )?;
    let moved =
        RevisionIdentity::from_canonical(&f.canonical.replace(&"a".repeat(40), &"f".repeat(40)))?;
    with_input(
        &f,
        true,
        true,
        |_| {},
        |_| {},
        |input| {
            assert!(
                export_recorded_review(PublicPlanInput {
                    current_revision: &moved,
                    ..input
                })
                .is_err()
            );
            Ok(())
        },
    )?;
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
    ] {
        assert!(
            with_input(
                &f,
                true,
                true,
                |_| {},
                |plan| {
                    plan["items"][0]["disposition"]["delivery"][field] = json!({"nested":true});
                },
                export_recorded_review
            )
            .is_err()
        );
    }
    Ok(())
}

#[test]
fn quiet_clean_limited_and_fresh_same_head_pass_remain_distinct() -> Result<()> {
    let mut f = Fixture::new()?;
    for claim in &mut f.graph.claims {
        claim.state = ClaimState::Refuted;
    }
    let clean = with_input(
        &f,
        true,
        true,
        |lead| {
            lead["review_result"] = json!("clean");
            lead["selections"] = json!([]);
        },
        |plan| {
            plan["quiet"] = json!(true);
            plan["items"] = json!([]);
        },
        export_recorded_review,
    )?;
    let view: Value = serde_json::from_slice(&clean.view_json)?;
    assert_eq!(view["judgment"]["judgment"]["review_result"], "clean");
    assert_eq!(view["public_plan"]["plan"]["quiet"], true);
    assert_eq!(view["remote_effects"], "unknown");
    let mut f = Fixture::new()?;
    let limited = with_input(
        &f,
        true,
        true,
        |lead| lead["review_result"] = json!("limited"),
        |_| {},
        export_recorded_review,
    )?;
    let view: Value = serde_json::from_slice(&limited.view_json)?;
    assert_eq!(view["judgment"]["judgment"]["review_result"], "limited");
    assert_eq!(
        view["public_plan"]["plan"]["items"]
            .as_array()
            .map(Vec::len),
        Some(2)
    );
    f.subject.pass_id = "fresh-pass-2".into();
    assert!(
        with_input(
            &f,
            true,
            true,
            |lead| lead["review_result"] = json!("limited"),
            |_| {},
            |input| {
                replay_recorded_review_export(input, &limited, &sha256_hex(&limited.view_json))
            }
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn absent_ci_stays_absent_without_a_gate_result() -> Result<()> {
    let f = Fixture::new()?;
    with_input(
        &f,
        false,
        false,
        |_| {},
        |_| {},
        |input| {
            let original = input.judgment_source;
            let source = RecordedJudgmentSource {
                subject: original.subject,
                revision_canonical: original.revision_canonical,
                graph: original.graph,
                review_complete: original.review_complete,
                evidence: original.evidence,
                imported_ci: None,
            };
            let digest = source_manifest_digest(&source)?;
            let export = export_recorded_review(PublicPlanInput {
                judgment_source: &source,
                expected_source_digest: &digest,
                observed_ci: None,
                ..input
            })?;
            let view: Value = serde_json::from_slice(&export.view_json)?;
            assert!(export.imported_ci_bytes.is_none());
            assert!(view["imported_ci_digest"].is_null());
            assert_eq!(view["judgment"]["availability"], "absent");
            assert!(view.get("ci_result").is_none());
            assert_eq!(view["remote_effects"], "unknown");
            Ok(())
        },
    )
}

#[test]
fn replay_recorded_ci_bytes_respect_one_mib_limit() -> Result<()> {
    let mut f = Fixture::new()?;
    f.ci = vec![b'x'; 1024 * 1024];
    f.ci_digest = sha256_hex(&f.ci);
    let export = with_input(&f, true, true, |_| {}, |_| {}, export_recorded_review)?;
    with_input(
        &f,
        true,
        true,
        |_| {},
        |_| {},
        |input| replay_recorded_review_export(input, &export, &sha256_hex(&export.view_json)),
    )?;
    let mut oversized = RecordedReviewExport {
        view_json: export.view_json.clone(),
        imported_ci_bytes: export.imported_ci_bytes.clone(),
    };
    oversized
        .imported_ci_bytes
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("CI absent"))?
        .push(b'x');
    let error = with_input(
        &f,
        true,
        true,
        |_| {},
        |_| {},
        |input| replay_recorded_review_export(input, &oversized, &sha256_hex(&oversized.view_json)),
    )
    .err()
    .ok_or_else(|| anyhow::anyhow!("oversized CI admitted"))?;
    assert!(
        error
            .to_string()
            .contains("recorded export exceeds byte limit")
    );
    assert_eq!(export.imported_ci_bytes.as_deref(), Some(f.ci.as_slice()));
    Ok(())
}
