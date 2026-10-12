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
