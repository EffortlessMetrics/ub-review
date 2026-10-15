use super::*;

fn write(f: &Fixture, name: &str, bytes: &[u8]) -> Result<Value> {
    fs::write(f.root.path().join(name), bytes)?;
    Ok(json!({"path":name,"sha256":hash(bytes)}))
}
fn decode_bytes(value: &Value) -> Result<Value> {
    Ok(serde_json::from_slice(&serde_json::from_value::<Vec<u8>>(
        value.clone(),
    )?)?)
}
fn link(previous: &str, event: &[u8]) -> String {
    let mut bytes = b"ub-review.recorded-delivery-event.v1\0".to_vec();
    bytes.extend(previous.as_bytes());
    bytes.extend((event.len() as u64).to_be_bytes());
    bytes.extend(event);
    hash(&bytes)
}

struct History {
    f: Fixture,
    subject_json: String,
    payload_metadata_json: String,
    subject: Value,
    events: Vec<Vec<u8>>,
}
impl History {
    fn new() -> Result<Self> {
        let mut f = Fixture::new()?;
        let graph = fs::read_to_string(f.root.path().join("graph.json"))?;
        let start = graph
            .find("\"claims\":[")
            .ok_or_else(|| anyhow::anyhow!("claims absent"))?
            + 10;
        let end = graph
            .find("],\"topics\"")
            .ok_or_else(|| anyhow::anyhow!("topics absent"))?;
        let claim = &graph[start..end];
        let two_graph = format!(
            "{},{},{}",
            &graph[..end],
            claim.replace("\"id\":\"A\"", "\"id\":\"B\""),
            &graph[end..]
        );
        // Remove the extra separator before the original closing array.
        let two_graph = two_graph.replace(",],\"topics\"", "],\"topics\"");
        f.source_json = f.source_json.replace(&graph, &two_graph);
        f.intake["graph"] = write(&f, "graph.json", two_graph.as_bytes())?;
        f.intake["source_manifest_digest"] = json!(domain_hash(
            "ub-review.recorded-judgment-source.v1",
            f.source_json.as_bytes()
        ));
        let summary = fs::read(f.root.path().join("body.txt"))?;
        let inline = b"The unchecked index can exceed the allocation.";
        let inline_pin = write(&f, "inline.txt", inline)?;
        let summary_pin = f.intake["materials"][0]["file"].clone();
        f.intake["materials"] = json!([
            {"claim_id":"A","body_ref":"public#A","class":"public","file":inline_pin},
            {"claim_id":"B","body_ref":"public#B","class":"public","file":summary_pin}
        ]);
        let line_hash = hash(b"danger();");
        f.intake["anchors"] = json!([{"reference":"anchor#A","claim_id":"A","path":"src/lib.rs","line":1,"line_digest":line_hash}]);
        // Canonical source manifests use existing producer field order, not Value order.
        let delivery_canonical = format!(
            r#"{{"repository":"owner/repo","pull_number":7,"head_sha":"{}"}}"#,
            "a".repeat(40)
        );
        let public = format!(
            r#"{{"delivery_subject":{delivery_canonical},"diff_digest":"{}","materials":[{{"claim_id":"A","body_ref":"public#A","body_digest":"{}","class":"public"}},{{"claim_id":"B","body_ref":"public#B","body_digest":"{}","class":"public"}}],"anchors":[{{"reference":"anchor#A","claim_id":"A","path":"src/lib.rs","line":1,"line_digest":"{line_hash}"}}]}}"#,
            domain_hash("ub-review.revision-identity.diff.v1", PATCH),
            hash(inline),
            hash(&summary)
        );
        f.intake["public_source_digest"] = json!(domain_hash(
            "ub-review.recorded-public-source.v1",
            public.as_bytes()
        ));
        let mut answer: Value =
            serde_json::from_slice(&fs::read(f.root.path().join("answer.json"))?)?;
        answer["source_manifest_digest"] = f.intake["source_manifest_digest"].clone();
        answer["selections"] = json!([{"claim_id":"A","evidence_refs":["fact#1"]},{"claim_id":"B","evidence_refs":["fact#1"]}]);
        f.intake["answer"] = write(&f, "answer.json", &serde_json::to_vec(&answer)?)?;
        let planned = json!({"exact_head_sha":"a".repeat(40),"claim_id":"A","action":"inline","path":"src/lib.rs","line":1,"side":"RIGHT","source_thread_id":null,"expected_body_digest":hash(inline)});
        let mut plan: Value = serde_json::from_slice(&fs::read(f.root.path().join("plan.json"))?)?;
        plan["judgment_digest"] = f.intake["answer"]["sha256"].clone();
        plan["public_source_digest"] = f.intake["public_source_digest"].clone();
        plan["items"] = json!([
            {"claim_id":"A","disposition":{"kind":"inline","body_ref":"public#A","anchor_ref":"anchor#A","delivery":planned}},
            {"claim_id":"B","disposition":{"kind":"summary","body_ref":"public#B","body_digest":hash(&summary)}}
        ]);
        f.intake["plan"] = write(&f, "plan.json", &serde_json::to_vec(&plan)?)?;
        let payload = serde_json::to_vec(
            &json!({"event":"COMMENT","body":String::from_utf8_lossy(&summary),"comments":[{"path":"src/lib.rs","line":1,"side":"RIGHT","body":String::from_utf8_lossy(inline)}]}),
        )?;
        let payload_pin = write(&f, "payload.json", &payload)?;
        // Metadata canonical serialization follows the actual existing type field order.
        let subject_canonical = format!(
            r#"{{"repository":"owner/repo","review_spec_digest":"{}","pass_id":"pass-1","attempt_id":"attempt-1"}}"#,
            "e".repeat(64)
        );
        let revision_canonical = format!(
            r#"{{"digest":"{}","semantics":"candidate_head","reviewed_commit":"{}"}}"#,
            plan["revision"]["digest"].as_str().unwrap_or_default(),
            "a".repeat(40)
        );
        let metadata = format!(
            r#"{{"subject":{subject_canonical},"delivery_subject":{delivery_canonical},"revision":{revision_canonical},"judgment_digest":"{}","public_plan_digest":"{}","payload_digest":"{}"}}"#,
            f.intake["answer"]["sha256"].as_str().unwrap_or_default(),
            f.intake["plan"]["sha256"].as_str().unwrap_or_default(),
            hash(&payload)
        );
        let observation_subject = format!(
            r#"{{"judgment_subject":{subject_canonical},"delivery_subject":{delivery_canonical},"revision":{revision_canonical},"source_manifest_digest":"{}","public_source_digest":"{}","judgment_digest":"{}","plan_digest":"{}","payload_sha256":"{}"}}"#,
            f.intake["source_manifest_digest"]
                .as_str()
                .unwrap_or_default(),
            f.intake["public_source_digest"]
                .as_str()
                .unwrap_or_default(),
            f.intake["answer"]["sha256"].as_str().unwrap_or_default(),
            f.intake["plan"]["sha256"].as_str().unwrap_or_default(),
            hash(&payload)
        );
        f.intake["schema"] = json!("ub-review.recorded-agent-file-input.v1");
        f.intake["observations"] = json!({"payload":payload_pin,"payload_source_digest":domain_hash("ub-review.recorded-prepared-payload-source.v1",metadata.as_bytes()),"stream":null,"watermark":null,"watermark_name":"latest"});
        let transaction = serde_json::to_vec(
            &json!({"schema":"ub-review.delivery_transaction.v1","exact_head_sha":"a".repeat(40),"planned":[planned],"state":"failed","failure":{"stage":"submission","reason":"PRIVATE_SENTINEL lost response"},"cleanup":{"status":"failed","reason":"unknown remote"}}),
        )?;
        let mut receipt = planned.clone();
        receipt["schema"] = json!("ub-review.delivery_receipt.v1");
        receipt["review_id"] = json!("987");
        receipt["comment_id"] = json!("123");
        receipt["confirmed_head_sha"] = json!("a".repeat(40));
        let reconciliation = serde_json::to_vec(
            &json!({"schema":"ub-review.delivery_reconciliation.v1","exact_head_sha":"a".repeat(40),"review_id":"987","planned_count":1,"observed_count":1,"receipts":[receipt]}),
        )?;
        let observations = [
            json!({"kind":"prepared","retry_of":null}),
            json!({"kind":"ambiguous","stage":"submission","local_receipt":{"sha256":hash(&transaction),"bytes":transaction}}),
            json!({"kind":"confirmed_inline","observed":[{"comment_id":"123","delivery":planned}],"reconciliation":{"sha256":hash(&reconciliation),"bytes":reconciliation}}),
            json!({"kind":"superseded","observed_head_sha":"f".repeat(40)}),
        ];
        let mut chain = domain_hash(
            "ub-review.recorded-delivery-genesis.v1",
            observation_subject.as_bytes(),
        );
        let mut events = vec![];
        for (i, observation) in observations.into_iter().enumerate() {
            let event = serde_json::to_vec(
                &json!({"sequence":i+1,"previous_sha256":chain,"attempt_id":"delivery-1","observation":observation}),
            )?;
            chain = link(&chain, &event);
            events.push(event);
        }
        Ok(Self {
            f,
            subject: serde_json::from_str(&observation_subject)?,
            subject_json: observation_subject,
            payload_metadata_json: metadata,
            events,
        })
    }
    fn repin_payload_and_chain(&mut self) -> Result<()> {
        let old = self.subject["payload_sha256"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        let new = self.f.intake["observations"]["payload"]["sha256"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        self.subject_json = self.subject_json.replace(&old, &new);
        self.subject["payload_sha256"] = json!(new);
        self.payload_metadata_json = self.payload_metadata_json.replace(&old, &new);
        self.f.intake["observations"]["payload_source_digest"] = json!(domain_hash(
            "ub-review.recorded-prepared-payload-source.v1",
            self.payload_metadata_json.as_bytes()
        ));
        self.rechain()
    }
    fn rechain(&mut self) -> Result<()> {
        let mut chain = domain_hash(
            "ub-review.recorded-delivery-genesis.v1",
            self.subject_json.as_bytes(),
        );
        for (i, bytes) in self.events.iter_mut().enumerate() {
            let mut event: Value = serde_json::from_slice(bytes)?;
            event["sequence"] = json!(i + 1);
            event["previous_sha256"] = json!(chain);
            *bytes = serde_json::to_vec(&event)?;
            chain = link(&chain, bytes);
        }
        Ok(())
    }
    fn admit(&mut self, count: usize) -> Result<()> {
        let stream = serde_json::to_vec(
            &json!({"schema":"ub-review.recorded-delivery-observations.v1","subject":self.subject,"events":self.events}),
        )?;
        let mut chain = domain_hash(
            "ub-review.recorded-delivery-genesis.v1",
            self.subject_json.as_bytes(),
        );
        for event in self.events.iter().take(count) {
            chain = link(&chain, event);
        }
        let watermark = serde_json::to_vec(
            &json!({"schema":"ub-review.recorded-delivery-watermark.v1","name":"latest","count":count,"chain_sha256":chain,"observed_head_sha":if count==4 {"f".repeat(40)}else{"a".repeat(40)}}),
        )?;
        self.f.intake["observations"]["stream"] = write(&self.f, "stream.json", &stream)?;
        self.f.intake["observations"]["watermark"] = write(&self.f, "watermark.json", &watermark)?;
        Ok(())
    }
}

#[test]
fn real_cli_replays_each_lifecycle_prefix_without_erasing_sources_or_failure() -> Result<()> {
    let mut h = History::new()?;
    let originals = ["answer.json", "plan.json", "ci.bin"]
        .map(|p| fs::read(h.f.root.path().join(p)))
        .into_iter()
        .collect::<std::io::Result<Vec<_>>>()?;
    for count in 1..=4 {
        h.admit(count)?;
        let out = h.f.invoke("agent-export", &[])?;
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let result: Value = serde_json::from_slice(&out.stdout)?;
        let source = decode_bytes(&result["source_view_json"])?;
        let observations = decode_bytes(&result["observation_view_json"])?;
        assert_eq!(source["judgment"]["judgment"]["review_result"], "findings");
        assert_eq!(result["prepared_payload_binding"]["whole_body_claim"], "B");
        assert_eq!(result["whole_review_delivery"], "not_qualified");
        assert_eq!(result["remote_authenticity"], "unverified");
        assert_eq!(observations["summary_payload_correspondence"], "unverified");
        assert_eq!(
            observations["attempts"][0]["remote"]["outcome"],
            if count < 3 {
                "unknown"
            } else {
                "confirmed_inline"
            }
        );
        assert_eq!(
            observations["attempts"][0]["local_failure"].is_null(),
            count < 2
        );
        assert_eq!(
            observations["source_currentness"],
            if count == 4 {
                "historical_at_recorded_watermark"
            } else {
                "at_recorded_source_head"
            }
        );
        assert!(!String::from_utf8_lossy(&out.stdout).contains("PRIVATE_SENTINEL"));
        assert_eq!(
            serde_json::from_value::<Vec<u8>>(source["judgment"]["recorded_bytes"].clone())?,
            originals[0]
        );
        assert_eq!(
            serde_json::from_value::<Vec<u8>>(source["public_plan_recorded_bytes"].clone())?,
            originals[1]
        );
        fs::write(h.f.root.path().join("agent-view.json"), &out.stdout)?;
        let pin = hash(&out.stdout);
        let replay = h.f.invoke(
            "agent-replay",
            &["--view", "agent-view.json", "--view-sha256", &pin],
        )?;
        assert!(
            replay.status.success(),
            "{}",
            String::from_utf8_lossy(&replay.stderr)
        );
    }
    for (i, name) in ["answer.json", "plan.json", "ci.bin"]
        .into_iter()
        .enumerate()
    {
        assert_eq!(fs::read(h.f.root.path().join(name))?, originals[i]);
    }
    Ok(())
}

#[test]
fn real_cli_rejects_reordered_deleted_altered_events_and_forged_prefixes() -> Result<()> {
    for shape in [
        "altered",
        "deleted",
        "reordered",
        "name",
        "watermark",
        "stream_pin",
        "payload_pin",
        "pass",
        "head",
        "body_loss",
        "numeric_alias",
        "payload_source_pin",
    ] {
        let mut h = History::new()?;
        if shape == "altered" {
            let mut e: Value = serde_json::from_slice(&h.events[1])?;
            e["attempt_id"] = json!("other");
            h.events[1] = serde_json::to_vec(&e)?;
        }
        if shape == "deleted" {
            h.events.remove(1);
        }
        if shape == "reordered" {
            h.events.swap(1, 2);
        }
        if shape == "pass" {
            h.subject["judgment_subject"]["pass_id"] = json!("old-pass");
        }
        if shape == "head" {
            h.subject["delivery_subject"]["head_sha"] = json!("f".repeat(40));
        }
        h.admit(h.events.len())?;
        match shape {
            "name" => h.f.intake["observations"]["watermark_name"] = json!("other"),
            "stream_pin" => h.f.intake["observations"]["stream"]["sha256"] = json!("0".repeat(64)),
            "payload_pin" => {
                h.f.intake["observations"]["payload"]["sha256"] = json!("0".repeat(64))
            }
            "watermark" => {
                let mut w: Value =
                    serde_json::from_slice(&fs::read(h.f.root.path().join("watermark.json"))?)?;
                w["count"] = json!(1);
                h.f.intake["observations"]["watermark"] =
                    write(&h.f, "watermark.json", &serde_json::to_vec(&w)?)?;
            }
            "body_loss" => {
                let mut p: Value =
                    serde_json::from_slice(&fs::read(h.f.root.path().join("payload.json"))?)?;
                p["body"] = json!("");
                h.f.intake["observations"]["payload"] =
                    write(&h.f, "payload.json", &serde_json::to_vec(&p)?)?;
                h.repin_payload_and_chain()?;
                h.admit(4)?;
            }
            "payload_source_pin" => {
                h.f.intake["observations"]["payload_source_digest"] = json!("0".repeat(64))
            }
            "numeric_alias" => {
                let mut e: Value = serde_json::from_slice(&h.events[2])?;
                e["observation"]["observed"][0]["comment_id"] = json!("0123");
                let receipt_bytes: Vec<u8> =
                    serde_json::from_value(e["observation"]["reconciliation"]["bytes"].clone())?;
                let mut receipt: Value = serde_json::from_slice(&receipt_bytes)?;
                receipt["receipts"][0]["comment_id"] = json!("0123");
                let bytes = serde_json::to_vec(&receipt)?;
                e["observation"]["reconciliation"] = json!({"sha256":hash(&bytes),"bytes":bytes});
                h.events[2] = serde_json::to_vec(&e)?;
                h.rechain()?;
                h.admit(4)?;
            }
            _ => {}
        }
        let out = h.f.invoke("agent-export", &[])?;
        assert!(!out.status.success(), "{shape} accepted");
        assert!(out.stdout.is_empty(), "{shape} partial output");
    }
    Ok(())
}

#[test]
fn real_cli_replay_rejects_repinned_projection_authority_and_erased_history() -> Result<()> {
    let mut h = History::new()?;
    h.admit(4)?;
    let original = h.f.invoke("agent-export", &[])?;
    assert!(original.status.success());
    for shape in [
        "whole_review",
        "qualification",
        "erased_failure",
        "currentness",
        "source_bytes",
    ] {
        let mut result: Value = serde_json::from_slice(&original.stdout)?;
        match shape {
            "whole_review" => result["whole_review_delivery"] = json!("confirmed"),
            "qualification" => result["qualification"] = json!("production"),
            "source_bytes" => {
                let mut source = decode_bytes(&result["source_view_json"])?;
                source["judgment"]["availability"] = json!("absent");
                result["source_view_json"] = json!(serde_json::to_vec(&source)?);
            }
            _ => {
                let mut observations = decode_bytes(&result["observation_view_json"])?;
                if shape == "erased_failure" {
                    observations["attempts"][0]["local_failure"] = Value::Null;
                } else {
                    observations["source_currentness"] = json!("at_recorded_source_head");
                }
                result["observation_view_json"] = json!(serde_json::to_vec(&observations)?);
            }
        }
        let bytes = serde_json::to_vec(&result)?;
        fs::write(h.f.root.path().join("tampered-agent.json"), &bytes)?;
        rejected(
            h.f.invoke(
                "agent-replay",
                &[
                    "--view",
                    "tampered-agent.json",
                    "--view-sha256",
                    &hash(&bytes),
                ],
            )?,
            "recorded agent replay rejected",
        );
    }
    Ok(())
}

#[path = "authority.rs"]
mod authority;
