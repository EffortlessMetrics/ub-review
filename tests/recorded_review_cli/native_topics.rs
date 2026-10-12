use super::*;

fn native_topic_fixture() -> Result<Fixture> {
    let mut f = Fixture::new()?;
    let old_graph = fs::read_to_string(f.root.path().join("graph.json"))?;
    let graph = include_bytes!("../../fixtures/recorded-native-topic-graph.json");
    let graph_text = std::str::from_utf8(graph)?.trim_end();
    let evidence = include_bytes!("../../fixtures/review-experience/perl-lsp-3627.json");
    let old_evidence = f.intake["evidence"][0]["file"]["sha256"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("evidence pin absent"))?
        .to_owned();
    f.source_json = f
        .source_json
        .replace(&old_graph, graph_text)
        .replace("fact#1", "summary-only-finding")
        .replace(&old_evidence, &hash(evidence));
    f.source_json = f
        .source_json
        .replace("\"review_complete\":true", "\"review_complete\":false");
    fs::write(f.root.path().join("graph.json"), graph)?;
    fs::write(f.root.path().join("evidence.bin"), evidence)?;
    f.intake["graph"]["sha256"] = json!(hash(graph));
    f.intake["evidence"][0]["reference"] = json!("summary-only-finding");
    f.intake["evidence"][0]["file"]["sha256"] = json!(hash(evidence));
    f.intake["source_manifest_digest"] = json!(domain_hash(
        "ub-review.recorded-judgment-source.v1",
        f.source_json.as_bytes()
    ));
    f.intake["review_complete"] = json!(false);
    f.intake["answer"] = Value::Null;
    f.intake["plan"] = Value::Null;
    f.intake["materials"] = json!([]);
    let delivery = format!(
        r#"{{"repository":"owner/repo","pull_number":7,"head_sha":"{}"}}"#,
        "a".repeat(40)
    );
    let public = format!(
        r#"{{"delivery_subject":{delivery},"diff_digest":"{}","materials":[],"anchors":[]}}"#,
        domain_hash("ub-review.revision-identity.diff.v1", PATCH)
    );
    f.intake["public_source_digest"] = json!(domain_hash(
        "ub-review.recorded-public-source.v1",
        public.as_bytes()
    ));
    Ok(f)
}

#[test]
fn real_cli_admits_actual_builder_topics_without_lead_or_public_payload() -> Result<()> {
    let f = native_topic_fixture()?;
    let graph = fs::read(f.root.path().join("graph.json"))?;
    let ci = fs::read(f.root.path().join("ci.bin"))?;
    let output = f.invoke("export", &[])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let view: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(view["judgment"]["availability"], "absent");
    assert_eq!(view["public_plan"]["availability"], "lead_absent");
    assert_eq!(
        view["source_manifest_digest"],
        f.intake["source_manifest_digest"]
    );
    assert_eq!(view["imported_ci_digest"], hash(&ci));
    assert_eq!(view["remote_effects"], "unknown");
    assert!(view["public_plan_recorded_bytes"].is_null());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("repository-owned Perl-LSP"));
    fs::write(f.root.path().join("native-view.json"), &output.stdout)?;
    let replay = f.invoke(
        "replay",
        &[
            "--view",
            "native-view.json",
            "--view-sha256",
            &hash(&output.stdout),
        ],
    )?;
    assert!(replay.status.success());
    assert_eq!(fs::read(f.root.path().join("graph.json"))?, graph);
    assert_eq!(fs::read(f.root.path().join("ci.bin"))?, ci);
    Ok(())
}

#[test]
fn real_cli_rejects_repinned_topic_identity_evidence_and_authority_tampering() -> Result<()> {
    for shape in [
        "head",
        "claim",
        "duplicate",
        "evidence_class",
        "evidence_ref",
        "evidence_detail",
        "unknown",
        "nested_unknown",
        "rows",
        "type",
    ] {
        let mut f = native_topic_fixture()?;
        let original = fs::read_to_string(f.root.path().join("graph.json"))?;
        let topic_start = original
            .find("\"topics\":[")
            .ok_or_else(|| anyhow::anyhow!("topics absent"))?;
        let prefix = &original[..topic_start];
        let topic = &original[topic_start..];
        // Surgical fixture edits keep the native producer's serialization order,
        // so independently repinned source manifests reach the structural guard.
        let changed_topic = match shape {
            "head" => topic.replace(
                &format!("\"head_sha\":\"{}\"", "a".repeat(40)),
                &format!("\"head_sha\":\"{}\"", "f".repeat(40)),
            ),
            "claim" => {
                let value: Value = serde_json::from_str(&original)?;
                let claim = value["topics"][0]["claim_id"]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("claim absent"))?;
                topic.replace(
                    &format!("\"claim_id\":\"{claim}\""),
                    "\"claim_id\":\"wrong-claim\"",
                )
            }
            "duplicate" => {
                let begin = "\"topics\":[".len();
                let end = topic
                    .find("],\"conflicts\":")
                    .ok_or_else(|| anyhow::anyhow!("topic end absent"))?;
                format!(
                    "{}{},{}{}",
                    &topic[..begin],
                    &topic[begin..end],
                    &topic[begin..end],
                    &topic[end..]
                )
            }
            "evidence_class" => topic.replace(
                "\"class\":\"ModelInterpretation\"",
                "\"class\":\"ProofReceipt\"",
            ),
            "evidence_ref" => topic.replace(
                "\"reference\":\"summary-only-finding\"",
                "\"reference\":\"forged#reference\"",
            ),
            "evidence_detail" => topic.replace(
                "repository-owned Perl-LSP review fixture",
                "PRIVATE_CLAIM_SENTINEL",
            ),
            "unknown" => topic.replacen(
                "\"topics\":[{",
                "\"topics\":[{\"delivery_confirmation\":\"PRIVATE_CI_SENTINEL\",",
                1,
            ),
            "nested_unknown" => topic.replace(
                "\"class\":\"ModelInterpretation\"",
                "\"dispatch\":\"PRIVATE_CI_SENTINEL\",\"class\":\"ModelInterpretation\"",
            ),
            "rows" => topic.replace(
                "\"proof_requests\":[]",
                &format!(
                    "\"proof_requests\":{}",
                    serde_json::to_string(&vec!["id"; 129])?
                ),
            ),
            _ => topic.replace("\"proof_receipts\":[]", "\"proof_receipts\":true"),
        };
        let changed = format!("{prefix}{changed_topic}");
        assert_ne!(changed, original, "{shape} did not edit native fixture");
        f.source_json = f
            .source_json
            .replace(original.trim_end(), changed.trim_end());
        f.intake["source_manifest_digest"] = json!(domain_hash(
            "ub-review.recorded-judgment-source.v1",
            f.source_json.as_bytes()
        ));
        fs::write(f.root.path().join("graph.json"), changed.as_bytes())?;
        f.intake["graph"]["sha256"] = json!(hash(changed.as_bytes()));
        let output = f.invoke("export", &[])?;
        assert!(!output.status.success(), "{shape} admitted");
        assert!(output.stdout.is_empty(), "{shape} emitted partial result");
        assert!(!String::from_utf8_lossy(&output.stderr).contains("PRIVATE_"));
    }
    Ok(())
}

fn repin_native_graph(f: &mut Fixture, original: &str, changed: &str) -> Result<()> {
    f.source_json = f
        .source_json
        .replace(original.trim_end(), changed.trim_end());
    f.intake["source_manifest_digest"] = json!(domain_hash(
        "ub-review.recorded-judgment-source.v1",
        f.source_json.as_bytes()
    ));
    fs::write(f.root.path().join("graph.json"), changed.as_bytes())?;
    f.intake["graph"]["sha256"] = json!(hash(changed.as_bytes()));
    Ok(())
}
fn native_segment<'a>(graph: &'a str, start: &str, end: &str) -> Result<&'a str> {
    let from = graph
        .find(start)
        .ok_or_else(|| anyhow::anyhow!("native segment start absent"))?
        + start.len();
    let to = graph[from..]
        .find(end)
        .ok_or_else(|| anyhow::anyhow!("native segment end absent"))?
        + from;
    Ok(&graph[from..to])
}

#[test]
fn real_cli_graph_row_boundary_rejects_before_claim_or_evidence_processing() -> Result<()> {
    for field in ["claims", "topics", "conflicts", "evidence_gaps"] {
        for count in [128, 129] {
            let mut f = native_topic_fixture()?;
            let original = fs::read_to_string(f.root.path().join("graph.json"))?;
            let value: Value = serde_json::from_str(&original)?;
            let id = value["claims"][0]["id"]
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("claim absent"))?;
            let claim = native_segment(&original, "\"claims\":[", "],\"topics\":")?;
            let topic = native_segment(&original, "\"topics\":[", "],\"conflicts\":")?;
            let mut changed = original.clone();
            if field == "claims" || field == "topics" {
                let claim_count = if field == "topics" { 128 } else { count };
                let claims = (0..claim_count)
                    .map(|i| {
                        if i == 0 {
                            claim.to_owned()
                        } else {
                            claim.replace(
                                &format!("\"id\":\"{id}\""),
                                &format!("\"id\":\"bounded-claim-{i}\""),
                            )
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                changed = changed.replacen(
                    &format!("\"claims\":[{claim}]"),
                    &format!("\"claims\":[{claims}]"),
                    1,
                );
                if field == "topics" {
                    // The 129th topic is deliberately a duplicate. Resource-limit
                    // rejection must precede the distinct identity rejection.
                    let topics = (0..count)
                        .map(|i| {
                            if i == 0 || i == 128 {
                                topic.to_owned()
                            } else {
                                topic.replace(
                                    &format!("\"claim_id\":\"{id}\""),
                                    &format!("\"claim_id\":\"bounded-claim-{i}\""),
                                )
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(",");
                    changed = changed.replacen(
                        &format!("\"topics\":[{topic}]"),
                        &format!("\"topics\":[{topics}]"),
                        1,
                    );
                }
            } else {
                let row = if field == "conflicts" {
                    format!(
                        r#"{{"claim_ids":["{id}"],"winner":null,"loser":null,"description":"bounded recorded context","resolution":"Unresolved"}}"#
                    )
                } else {
                    format!(
                        r#"{{"claim_id":"{id}","needed_evidence_class":"proof-receipt","detail":"bounded recorded context"}}"#
                    )
                };
                changed = changed.replace(
                    &format!("\"{field}\":[]"),
                    &format!("\"{field}\":[{}]", vec![row; count].join(",")),
                );
            }
            repin_native_graph(&mut f, &original, &changed)?;
            let ci = fs::read(f.root.path().join("ci.bin"))?;
            let output = f.invoke("export", &[])?;
            if count == 128 {
                assert!(
                    output.status.success(),
                    "{field} boundary: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            } else {
                assert!(!output.status.success(), "{field} oversize admitted");
                assert!(output.stdout.is_empty());
                assert!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains("recorded graph exceeds row limit"),
                    "{field}: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            assert_eq!(
                fs::read(f.root.path().join("graph.json"))?,
                changed.as_bytes()
            );
            assert_eq!(fs::read(f.root.path().join("ci.bin"))?, ci);
        }
    }
    Ok(())
}

#[test]
fn real_cli_nested_topic_reference_row_boundaries_preserve_bytes() -> Result<()> {
    for field in [
        "evidence",
        "existing_threads",
        "stale_threads",
        "proof_requests",
        "proof_receipts",
    ] {
        for count in [128, 129] {
            let mut f = native_topic_fixture()?;
            let original = fs::read_to_string(f.root.path().join("graph.json"))?;
            let topic_start = original
                .find("\"topics\":[")
                .ok_or_else(|| anyhow::anyhow!("topics absent"))?;
            let (prefix, topic) = original.split_at(topic_start);
            let changed_topic = if field == "evidence" {
                let edge = native_segment(topic, "\"evidence\":[", "],\"existing_threads\":")?;
                topic.replace(
                    &format!("\"evidence\":[{edge}]"),
                    &format!("\"evidence\":[{}]", vec![edge; count].join(",")),
                )
            } else {
                // Proof/thread strings are opaque context, not authenticated IDs.
                let ids = (0..count)
                    .map(|i| format!("bounded-context-{i}"))
                    .collect::<Vec<_>>();
                topic.replace(
                    &format!("\"{field}\":[]"),
                    &format!("\"{field}\":{}", serde_json::to_string(&ids)?),
                )
            };
            let changed = format!("{prefix}{changed_topic}");
            repin_native_graph(&mut f, &original, &changed)?;
            let ci = fs::read(f.root.path().join("ci.bin"))?;
            let output = f.invoke("export", &[])?;
            if count == 128 {
                assert!(
                    output.status.success(),
                    "{field} boundary: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            } else {
                assert!(!output.status.success(), "{field} oversize admitted");
                assert!(output.stdout.is_empty());
                assert!(
                    String::from_utf8_lossy(&output.stderr)
                        .contains("recorded topic exceeds row limit")
                );
            }
            assert_eq!(
                fs::read(f.root.path().join("graph.json"))?,
                changed.as_bytes()
            );
            assert_eq!(fs::read(f.root.path().join("ci.bin"))?, ci);
        }
    }
    Ok(())
}
