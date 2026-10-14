//! Test-only proposal: currentness/schema admission; no production migration.
#[path = "common/currentness_1067.rs"]
mod contract;
use anyhow::Result;
use contract::{Admission, CANDIDATE_SOURCE, decode, fixture, verify};
use serde_json::{Value, json};

#[test]
fn currentness_1067_raw33_native30_retains_every_row() -> Result<()> {
    let (raw, badge) = fixture(true);
    let before = raw.clone();
    let result = verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE)?;
    assert_eq!(result.raw_rows, 46);
    assert_eq!((result.raw_unknowns, result.current_unknowns), (33, 30));
    assert_eq!((result.raw_gaps, result.current_gaps), (2, 2));
    assert_eq!(result.current_ids.len(), 43);
    assert_eq!(result.retained_noncurrent_ids.len(), 3);
    assert_eq!(raw, before);
    Ok(())
}

#[test]
fn currentness_1067_legacy_requires_explicit_admission() -> Result<()> {
    let (raw, badge) = fixture(false);
    let result = verify(
        &raw,
        &badge,
        Admission::LegacyUnspecified0100,
        "registry:0.10.0",
    )?;
    assert_eq!(result.current_unknowns, 33);
    assert!(verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE).is_err());
    assert!(
        verify(
            &raw,
            &badge,
            Admission::LegacyUnspecified0100,
            CANDIDATE_SOURCE
        )
        .is_err()
    );
    Ok(())
}

#[test]
fn currentness_1067_invalid_values_reject_even_non_gap_rows() {
    for index in [0, 34, 45] {
        for invalid in [
            Value::Null,
            json!(false),
            json!(0),
            json!({}),
            json!("unresolved_subject"),
            json!("unsupported"),
            json!("legacy_unspecified"),
        ] {
            let (mut raw, badge) = fixture(true);
            raw["findings"][index]["source_currentness"] = invalid;
            assert!(
                verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE).is_err(),
                "row {index}"
            );
        }
        let (mut raw, badge) = fixture(true);
        if let Some(object) = raw["findings"][index].as_object_mut() {
            object.remove("source_currentness");
        }
        assert!(verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE).is_err());
    }
}

#[test]
fn currentness_1067_deleted_and_moved_gaps_are_audit_only() -> Result<()> {
    let (mut raw, mut badge) = fixture(true);
    raw["findings"][33]["source_currentness"] = json!("base_deleted");
    raw["findings"][34]["source_currentness"] = json!("moved_or_renamed");
    badge["counts"]["unsuppressed_exposure_gaps"] = json!(0);
    let result = verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE)?;
    assert_eq!((result.raw_gaps, result.current_gaps), (2, 0));
    assert_eq!(result.retained_noncurrent_ids.len(), 5);
    Ok(())
}

#[test]
fn currentness_1067_schema_counter_and_completeness_controls() -> Result<()> {
    // Each case is separately applied to a fresh coherent pair.
    let cases = [
        (false, "/schema_version", json!("0.3")),
        (true, "/schema_version", json!("0.6")),
        (true, "/schema_version", json!("0.9")),
        (true, "/kind", json!("other")),
        (true, "/scope", json!("full")),
        (true, "/basis", json!("other")),
        (false, "/summary/findings", json!(45)),
        (false, "/summary/probes", json!(45)),
        (false, "/summary/static_unknown", json!(0)),
        (true, "/counts/analyzed_findings", json!(45)),
        (true, "/counts/unknowns", json!(33)),
        (true, "/counts/unknowns", json!(0)),
        (true, "/counts/unknowns", Value::Null),
        (true, "/counts/unknowns", json!(-1)),
        (true, "/counts/unknowns", json!(false)),
        (true, "/counts/unknowns", json!("30")),
        (true, "/counts/unsuppressed_exposure_gaps", json!(0)),
        (true, "/counts/suppressed_exposure_gaps", json!(1)),
        (false, "/downstream_consumable", json!(false)),
        (false, "/analysis_complete", json!(false)),
        (true, "/analysis_complete", json!(false)),
        (false, "/run_limitations", json!(["output_truncated"])),
        (true, "/analysis_outcome/schema_version", json!("0.2")),
        (true, "/analysis_outcome/kind", json!("partial")),
        (true, "/analysis_outcome/counts/finding_count", json!(45)),
        (false, "/findings/45/classification", json!("future_class")),
    ];
    for (is_badge, pointer, invalid) in cases {
        let (mut raw, mut badge) = fixture(true);
        let target = if is_badge { &mut badge } else { &mut raw };
        let slot = target
            .pointer_mut(pointer)
            .ok_or_else(|| anyhow::anyhow!("fixture missing {pointer}"))?;
        *slot = invalid;
        assert!(
            verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE).is_err(),
            "accepted {pointer}"
        );
    }
    // A mutually agreeing unsupported typed schema/state must still reject.
    for (field, invalid) in [
        ("schema_version", json!("0.2")),
        ("kind", json!("partial")),
        ("limitations", json!(["eol_only_churn"])),
    ] {
        let (mut raw, mut badge) = fixture(true);
        badge["analysis_outcome"][field] = invalid.clone();
        raw["analysis_outcome"]["outcome"][field] = invalid;
        assert!(
            verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE).is_err(),
            "accepted agreed {field}"
        );
    }
    let (raw, badge) = fixture(true);
    assert!(verify(&raw, &badge, Admission::CandidateA734, "another-source").is_err());
    Ok(())
}

#[test]
fn currentness_1067_agreed_typed_counts_require_retained_row_denominator() -> Result<()> {
    for (finding_count, probe_count) in [
        (0, 46),
        (45, 46),
        (47, 46),
        (46, 0),
        (46, 45),
        (46, 47),
        (0, 0),
        (45, 45),
        (47, 47),
    ] {
        let (mut raw, mut badge) = fixture(true);
        // Retained rows, raw summary and native counts stay at 46; only the
        // mutually agreeing typed outcome counts carry a false denominator.
        badge["analysis_outcome"]["counts"] =
            json!({"finding_count": finding_count, "probe_count": probe_count});
        raw["analysis_outcome"]["outcome"] = badge["analysis_outcome"].clone();
        let error = verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE)
            .err()
            .ok_or_else(|| {
                anyhow::anyhow!("accepted typed counts ({finding_count}, {probe_count})")
            })?;
        assert_eq!(error.to_string(), "typed denominator");
    }
    Ok(())
}

#[test]
fn currentness_1067_duplicate_ids_and_keys_reject() {
    let (mut raw, badge) = fixture(true);
    raw["findings"][45]["id"] = raw["findings"][0]["id"].clone();
    assert!(verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE).is_err());
    for bytes in [
        br#"{"schema_version":"0.6","schema_version":"0.8"}"#.as_slice(),
        br#"{"counts":{"unknowns":0,"unknowns":30}}"#.as_slice(),
        br#"{"findings":[{"id":"a","id":"b"}]}"#.as_slice(),
    ] {
        assert!(decode(bytes).is_err());
    }
}

#[test]
fn currentness_1067_id_churn_is_literal_not_an_alias() -> Result<()> {
    let (raw, badge) = fixture(true);
    let old = verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE)?;
    let mut moved_line = raw.clone();
    moved_line["findings"][3]["probe"]["line"] = json!(900);
    assert_eq!(
        old.current_ids,
        verify(
            &moved_line,
            &badge,
            Admission::CandidateA734,
            CANDIDATE_SOURCE
        )?
        .current_ids
    );
    moved_line["findings"][3]["id"] = json!("new-semantic-subject");
    moved_line["findings"][3]["probe"]["expression"] = json!("changed semantic subject");
    let new = verify(
        &moved_line,
        &badge,
        Admission::CandidateA734,
        CANDIDATE_SOURCE,
    )?;
    assert_eq!(
        old.current_ids
            .difference(&new.current_ids)
            .collect::<Vec<_>>(),
        vec!["synthetic-3"]
    );
    assert_eq!(
        new.current_ids
            .difference(&old.current_ids)
            .collect::<Vec<_>>(),
        vec!["new-semantic-subject"]
    );
    // Coordinates and expression correspondence never collapse literal IDs.
    Ok(())
}

#[test]
fn currentness_1067_unique_decoder_preserves_fixture() -> Result<()> {
    let (raw, badge) = fixture(true);
    let decoded = decode(&serde_json::to_vec(&raw)?)?;
    assert_eq!(raw, decoded);
    verify(
        &decoded,
        &decode(&serde_json::to_vec(&badge)?)?,
        Admission::CandidateA734,
        CANDIDATE_SOURCE,
    )?;
    Ok(())
}

#[test]
fn currentness_1067_missing_counters_are_unavailable_not_zero() {
    for (is_badge, object_pointer, field) in [
        (true, "/counts", "unknowns"),
        (true, "/counts", "suppressed_exposure_gaps"),
        (false, "/summary", "static_unknown"),
        (false, "/summary", "findings"),
    ] {
        let (mut raw, mut badge) = fixture(true);
        let target = if is_badge { &mut badge } else { &mut raw };
        if let Some(object) = target
            .pointer_mut(object_pointer)
            .and_then(Value::as_object_mut)
        {
            object.remove(field);
        }
        assert!(
            verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE).is_err(),
            "missing {field}"
        );
    }
}

#[test]
fn currentness_1067_suppression_partition_does_not_change_currentness() -> Result<()> {
    let (raw, mut badge) = fixture(true);
    badge["counts"]["unsuppressed_exposure_gaps"] = json!(1);
    badge["counts"]["suppressed_exposure_gaps"] = json!(1);
    let result = verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE)?;
    assert_eq!(result.current_gaps, 2);
    assert_eq!(result.current_unknowns, 30);
    // Aggregate reconciliation grants no per-ID suppression provenance.
    Ok(())
}

#[test]
fn currentness_1067_explicit_empty_complete_packet_is_distinct_from_unresolved() -> Result<()> {
    let (mut raw, mut badge) = fixture(true);
    raw["findings"] = json!([]);
    for key in [
        "findings",
        "probes",
        "weakly_exposed",
        "reachable_unrevealed",
        "no_static_path",
        "infection_unknown",
        "propagation_unknown",
        "static_unknown",
        "exposed",
    ] {
        raw["summary"][key] = json!(0);
    }
    for key in [
        "analyzed_findings",
        "unsuppressed_exposure_gaps",
        "suppressed_exposure_gaps",
        "unknowns",
    ] {
        badge["counts"][key] = json!(0);
    }
    badge["analysis_outcome"]["kind"] = json!("no_behavioral_candidates");
    badge["analysis_outcome"]["counts"] = json!({"finding_count":0,"probe_count":0});
    raw["analysis_outcome"]["outcome"] = badge["analysis_outcome"].clone();
    let empty = verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE)?;
    assert_eq!((empty.raw_rows, empty.current_unknowns), (0, 0));
    // Matching wrappers do not make a contradictory outcome kind admissible.
    badge["analysis_outcome"]["kind"] = json!("complete_with_findings");
    raw["analysis_outcome"]["outcome"] = badge["analysis_outcome"].clone();
    assert!(verify(&raw, &badge, Admission::CandidateA734, CANDIDATE_SOURCE).is_err());
    Ok(())
}
