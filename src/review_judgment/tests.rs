use super::*;
use crate::claim_graph::{ClaimNode, EvidenceRef, RelevanceKind, RelevancePath};
use crate::revision_identity::{CommitTree, ReviewSemantics};
use serde_json::{Value, json};

struct Fixture {
    identity: RevisionIdentity,
    canonical: String,
    subject: JudgmentSubject,
    graph: ClaimGraph,
    evidence_bytes: Vec<u8>,
    evidence_digest: String,
    ci: Vec<u8>,
    ci_digest: String,
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
            &RevisionIdentity::diff_digest(b"recorded diff"),
        )?;
        let evidence_bytes = b"recorded exact source citation".to_vec();
        let ci = b"{\"result\":\"fail\",\"required_gap\":true}\n".to_vec();
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
        graph.claims.push(ClaimNode {
            id: "claim-A".to_owned(),
            subject: "material recorded claim".to_owned(),
            source_lane: "recorded".to_owned(),
            state: ClaimState::Confirmed,
            supporting_evidence: vec![EvidenceRef {
                class: EvidenceClass::ExactCitation,
                reference: "source#A".to_owned(),
                detail: "exact citation".to_owned(),
            }],
            contradicting_evidence: vec![],
            relevance: RelevancePath {
                kind: RelevanceKind::ChangedLine,
                explanation: "changed source".to_owned(),
            },
            severity: "high".to_owned(),
        });
        Ok(Self {
            canonical: identity.canonical_form(),
            identity,
            subject: JudgmentSubject {
                repository: "owner/repo".to_owned(),
                review_spec_digest: "e".repeat(64),
                pass_id: "pass-2".to_owned(),
                attempt_id: "attempt-1".to_owned(),
            },
            graph,
            evidence_digest: sha256_hex(&evidence_bytes),
            evidence_bytes,
            ci_digest: sha256_hex(&ci),
            ci,
        })
    }

    fn source<'a>(&'a self, evidence: &'a [RecordedEvidence<'a>]) -> RecordedJudgmentSource<'a> {
        RecordedJudgmentSource {
            subject: &self.subject,
            revision_canonical: &self.canonical,
            graph: &self.graph,
            review_complete: true,
            evidence,
            imported_ci: Some(ImportedCi {
                bytes: &self.ci,
                digest: &self.ci_digest,
            }),
        }
    }

    fn evidence(&self) -> Vec<RecordedEvidence<'_>> {
        vec![RecordedEvidence {
            reference: "source#A",
            bytes: &self.evidence_bytes,
            digest: &self.evidence_digest,
        }]
    }

    fn answer(&self, source_digest: &str) -> Value {
        json!({"schema": SCHEMA, "subject": self.subject, "revision": self.identity.revision_ref(),
            "source_manifest_digest": source_digest, "review_result": "findings",
            "selections": [{"claim_id":"claim-A", "evidence_refs":["source#A"]}]})
    }
}

fn call(fixture: &Fixture, value: &Value) -> Result<RecordedLead> {
    let evidence = fixture.evidence();
    let source = fixture.source(&evidence);
    let answer = serde_json::to_vec(value)?;
    validate_recorded_judgment(
        &source,
        &fixture.identity,
        &source_manifest_digest(&source)?,
        Some(&answer),
        Some(&sha256_hex(&answer)),
        Some(&fixture.ci),
    )
}

fn assert_invalid(result: RecordedLead, expected: &str) {
    assert!(
        matches!(result, RecordedLead::Invalid { ref reason } if reason.contains(expected)),
        "{result:?}"
    );
}

#[test]
fn recorded_entrypoint_distinguishes_findings_clean_limited_absent_invalid() -> Result<()> {
    let mut fixture = Fixture::new()?;
    let evidence = fixture.evidence();
    let source = fixture.source(&evidence);
    let source_digest = source_manifest_digest(&source)?;
    let finding = fixture.answer(&source_digest);
    let first = call(&fixture, &finding)?;
    assert_eq!(first, call(&fixture, &finding)?);
    assert!(
        matches!(first, RecordedLead::Validated { judgment, .. } if judgment.review_result == ReviewResult::Findings && judgment.selections[0].claim_id == "claim-A")
    );
    assert_eq!(
        validate_recorded_judgment(
            &source,
            &fixture.identity,
            &source_digest,
            None,
            None,
            Some(&fixture.ci)
        )?,
        RecordedLead::Absent
    );
    let mut limited = finding.clone();
    limited["review_result"] = json!("limited");
    assert!(
        matches!(call(&fixture, &limited)?, RecordedLead::Validated { judgment, .. } if judgment.review_result == ReviewResult::Limited)
    );
    let mut clean = finding;
    clean["review_result"] = json!("clean");
    clean["selections"] = json!([]);
    assert_invalid(call(&fixture, &clean)?, "unsupported clean");
    fixture.graph.claims.clear();
    clean["source_manifest_digest"] = json!(source_manifest_digest(
        &fixture.source(&fixture.evidence())
    )?);
    assert!(
        matches!(call(&fixture, &clean)?, RecordedLead::Validated { judgment, .. } if judgment.review_result == ReviewResult::Clean)
    );
    Ok(())
}

#[test]
fn stale_forged_cross_claim_and_authority_fields_are_rejected() -> Result<()> {
    let fixture = Fixture::new()?;
    let valid = fixture.answer(&source_manifest_digest(
        &fixture.source(&fixture.evidence()),
    )?);
    for (pointer, replacement, reason) in [
        ("/schema", json!("unknown.v99"), "unknown judgment schema"),
        (
            "/subject/repository",
            json!("other/repo"),
            "source identity mismatch",
        ),
        (
            "/subject/pass_id",
            json!("pass-1"),
            "source identity mismatch",
        ),
        (
            "/subject/attempt_id",
            json!("attempt-0"),
            "source identity mismatch",
        ),
        (
            "/subject/review_spec_digest",
            json!("f".repeat(64)),
            "source identity mismatch",
        ),
        (
            "/revision/digest",
            json!("f".repeat(64)),
            "source identity mismatch",
        ),
        (
            "/revision/reviewed_commit",
            json!("f".repeat(40)),
            "source identity mismatch",
        ),
        (
            "/source_manifest_digest",
            json!("f".repeat(64)),
            "source identity mismatch",
        ),
        (
            "/selections/0/claim_id",
            json!("forged"),
            "unknown selected claim",
        ),
        (
            "/selections/0/evidence_refs/0",
            json!("forged#receipt"),
            "unknown or duplicate evidence",
        ),
        ("/selections/0/evidence_refs", json!([]), "no evidence"),
    ] {
        let mut mutant = valid.clone();
        *mutant
            .pointer_mut(pointer)
            .ok_or_else(|| anyhow::anyhow!("missing fixture pointer"))? = replacement;
        assert_invalid(call(&fixture, &mutant)?, reason);
    }
    for field in [
        "ci_evidence_result",
        "required",
        "policy",
        "delivery_confirmation",
        "check_exit_status",
        "argv",
        "dispatch",
        "rationale",
        "anchors",
        "suggestions",
    ] {
        for pointer in ["", "/subject", "/revision", "/selections/0"] {
            let mut mutant = valid.clone();
            mutant
                .pointer_mut(pointer)
                .and_then(Value::as_object_mut)
                .ok_or_else(|| anyhow::anyhow!("missing object"))?
                .insert(field.to_owned(), json!("forged authority"));
            assert!(
                matches!(call(&fixture, &mutant)?, RecordedLead::Invalid { .. }),
                "{field} at {pointer}"
            );
        }
    }
    // Existing and byte-verified evidence, but attached to another claim.
    let mut wrong_edge = Fixture::new()?;
    wrong_edge.graph.claims[0].supporting_evidence[0].reference = "source#B".to_owned();
    let answer = wrong_edge.answer(&source_manifest_digest(
        &wrong_edge.source(&wrong_edge.evidence()),
    )?);
    assert_invalid(call(&wrong_edge, &answer)?, "not bound to selected claim");
    Ok(())
}

#[test]
fn byte_tamper_source_staleness_and_ci_missingness_fail_before_lead() -> Result<()> {
    let fixture = Fixture::new()?;
    let evidence = fixture.evidence();
    let mut source = fixture.source(&evidence);
    let source_digest = source_manifest_digest(&source)?;
    let valid = fixture.answer(&source_digest);
    let answer = serde_json::to_vec(&valid)?;
    let digest = sha256_hex(&answer);
    assert_invalid(
        validate_recorded_judgment(
            &source,
            &fixture.identity,
            &source_digest,
            Some(b"{}"),
            Some(&digest),
            Some(&fixture.ci),
        )?,
        "answer bytes mismatch",
    );
    assert!(
        validate_recorded_judgment(
            &source,
            &fixture.identity,
            &source_digest,
            None,
            None,
            Some(b"{\"result\":\"pass\"}")
        )
        .is_err()
    );
    // Same parsed CI object, different bytes: whitespace still matters.
    let mut changed_ci = fixture.ci.clone();
    changed_ci.push(b' ');
    assert!(
        validate_recorded_judgment(
            &source,
            &fixture.identity,
            &source_digest,
            None,
            None,
            Some(&changed_ci)
        )
        .is_err()
    );
    assert!(
        validate_recorded_judgment(&source, &fixture.identity, &source_digest, None, None, None)
            .is_err()
    );
    source.review_complete = false;
    assert!(
        validate_recorded_judgment(
            &source,
            &fixture.identity,
            &source_digest,
            None,
            None,
            Some(&fixture.ci)
        )
        .is_err()
    );
    let mut stale = Fixture::new()?;
    stale
        .graph
        .revision
        .as_mut()
        .ok_or_else(|| anyhow::anyhow!("missing revision"))?
        .digest = "f".repeat(64);
    let stale_evidence = stale.evidence();
    let stale_source = stale.source(&stale_evidence);
    assert!(
        validate_recorded_judgment(
            &stale_source,
            &stale.identity,
            &source_manifest_digest(&stale_source)?,
            None,
            None,
            Some(&stale.ci)
        )
        .is_err()
    );
    let bad_evidence = [RecordedEvidence {
        reference: "source#A",
        digest: &fixture.evidence_digest,
        bytes: b"forged citation",
    }];
    let bad_source = fixture.source(&bad_evidence);
    assert!(
        validate_recorded_judgment(
            &bad_source,
            &fixture.identity,
            &source_manifest_digest(&bad_source)?,
            None,
            None,
            Some(&fixture.ci)
        )
        .is_err()
    );
    let mut no_ci = fixture.source(&evidence);
    no_ci.imported_ci = None;
    let no_ci_digest = source_manifest_digest(&no_ci)?;
    assert_eq!(
        validate_recorded_judgment(&no_ci, &fixture.identity, &no_ci_digest, None, None, None)?,
        RecordedLead::Absent
    );
    assert!(
        validate_recorded_judgment(
            &no_ci,
            &fixture.identity,
            &no_ci_digest,
            None,
            None,
            Some(&fixture.ci)
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn resealed_sources_still_require_current_identity_and_unambiguous_references() -> Result<()> {
    let mut fixture = Fixture::new()?;
    fixture.graph.claims.push(fixture.graph.claims[0].clone());
    let evidence = fixture.evidence();
    let source = fixture.source(&evidence);
    let error = validate_recorded_judgment(
        &source,
        &fixture.identity,
        &source_manifest_digest(&source)?,
        None,
        None,
        Some(&fixture.ci),
    )
    .err();
    assert!(error.is_some_and(|error| error.to_string().contains("ambiguous source claim")));
    fixture.graph.claims.pop();
    let duplicate_evidence = [
        RecordedEvidence {
            reference: "source#A",
            digest: &fixture.evidence_digest,
            bytes: &fixture.evidence_bytes,
        },
        RecordedEvidence {
            reference: "source#A",
            digest: &fixture.evidence_digest,
            bytes: &fixture.evidence_bytes,
        },
    ];
    let source = fixture.source(&duplicate_evidence);
    let error = validate_recorded_judgment(
        &source,
        &fixture.identity,
        &source_manifest_digest(&source)?,
        None,
        None,
        Some(&fixture.ci),
    )
    .err();
    assert!(error.is_some_and(|error| error.to_string().contains("ambiguous evidence reference")));
    let head = CommitTree::new("head", &"f".repeat(40), &"b".repeat(40))?;
    let moved = RevisionIdentity::new(
        CommitTree::new("base", &"c".repeat(40), &"d".repeat(40))?,
        head.clone(),
        head,
        None,
        ReviewSemantics::CandidateHead,
        &RevisionIdentity::changed_paths_digest(["src/lib.rs"]),
        &RevisionIdentity::diff_digest(b"recorded diff"),
    )?;
    let evidence = fixture.evidence();
    let source = fixture.source(&evidence);
    let error = validate_recorded_judgment(
        &source,
        &moved,
        &source_manifest_digest(&source)?,
        None,
        None,
        Some(&fixture.ci),
    )
    .err();
    assert!(error.is_some_and(|error| error.to_string().contains("stale source revision")));
    // Pinned malformed bytes must be Invalid even if their digest is accurate.
    let bad = b"not JSON";
    assert!(matches!(
        validate_recorded_judgment(
            &source,
            &fixture.identity,
            &source_manifest_digest(&source)?,
            Some(bad),
            Some(&sha256_hex(bad)),
            Some(&fixture.ci)
        )?,
        RecordedLead::Invalid { .. }
    ));
    // An incomplete scope can retain a material finding, but cannot claim clean.
    fixture.graph.claims.clear();
    let evidence = fixture.evidence();
    let mut source = fixture.source(&evidence);
    source.review_complete = false;
    let digest = source_manifest_digest(&source)?;
    let clean = serde_json::to_vec(&json!({"schema":SCHEMA, "subject":fixture.subject,
        "revision":fixture.identity.revision_ref(), "source_manifest_digest":digest,
        "review_result":"clean", "selections":[]}))?;
    assert_invalid(
        validate_recorded_judgment(
            &source,
            &fixture.identity,
            &digest,
            Some(&clean),
            Some(&sha256_hex(&clean)),
            Some(&fixture.ci),
        )?,
        "unsupported clean",
    );
    Ok(())
}
