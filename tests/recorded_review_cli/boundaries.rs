use super::*;

fn absent_fixture() -> Result<Fixture> {
    let mut f = Fixture::new()?;
    f.intake["answer"] = Value::Null;
    f.intake["plan"] = Value::Null;
    Ok(f)
}

fn repin_source(f: &mut Fixture) {
    f.intake["source_manifest_digest"] = json!(domain_hash(
        "ub-review.recorded-judgment-source.v1",
        f.source_json.as_bytes()
    ));
}

fn outcome(f: &Fixture, expected: Option<&str>, label: &str) -> Result<()> {
    let ci = fs::read(f.root.path().join("ci.bin"))?;
    let out = f.invoke("export", &[])?;
    if let Some(message) = expected {
        assert!(!out.status.success(), "{label} admitted");
        assert!(out.stdout.is_empty(), "{label} emitted output");
        assert!(
            String::from_utf8_lossy(&out.stderr).contains(message),
            "{label}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    } else {
        assert!(
            out.status.success(),
            "{label}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    assert_eq!(fs::read(f.root.path().join("ci.bin"))?, ci);
    Ok(())
}

#[test]
fn intake_row_boundaries_are_checked_before_later_validation() -> Result<()> {
    for field in ["evidence", "materials", "anchors"] {
        for count in [128, 129] {
            let mut f = absent_fixture()?;
            match field {
                "evidence" => {
                    let pin = f.intake["evidence"][0]["file"].clone();
                    let digest = pin["sha256"]
                        .as_str()
                        .ok_or_else(|| anyhow::anyhow!("missing digest"))?;
                    let refs: Vec<_> = (0..count)
                        .map(|i| {
                            if i == 0 {
                                "fact#1".to_owned()
                            } else {
                                format!("fact#{}", i + 1)
                            }
                        })
                        .collect();
                    f.intake["evidence"] = Value::Array(
                        refs.iter()
                            .map(|r| json!({"reference":r,"file":pin}))
                            .collect(),
                    );
                    let old = format!(r#"{{"reference":"fact#1","digest":"{digest}"}}"#);
                    let rows = refs
                        .iter()
                        .map(|r| format!(r#"{{"reference":"{r}","digest":"{digest}"}}"#))
                        .collect::<Vec<_>>()
                        .join(",");
                    assert!(f.source_json.contains(&old));
                    f.source_json = f.source_json.replacen(&old, &rows, 1);
                    repin_source(&mut f);
                }
                "materials" => {
                    let row = f.intake["materials"][0].clone();
                    f.intake["materials"] = Value::Array(vec![row; count]);
                }
                _ => {
                    f.intake["anchors"] = Value::Array(vec![
                        json!({"reference":"a","claim_id":"A","path":"src/lib.rs","line":1,"line_digest":"0".repeat(64)});
                        count
                    ]);
                }
            }
            outcome(
                &f,
                (count == 129).then_some("recorded intake exceeds row limit"),
                &format!("{field}:{count}"),
            )?;
        }
    }
    Ok(())
}

#[test]
fn graph_row_boundaries_precede_downstream_checks() -> Result<()> {
    for field in ["claims", "conflicts", "evidence_gaps"] {
        for count in [128, 129] {
            let mut f = absent_fixture()?;
            let original = fs::read_to_string(f.root.path().join("graph.json"))?;
            let changed = if field == "claims" {
                let start = original
                    .find("\"claims\":[")
                    .ok_or_else(|| anyhow::anyhow!("claims absent"))?
                    + "\"claims\":[".len();
                let end = original[start..]
                    .find("],\"topics\":")
                    .ok_or_else(|| anyhow::anyhow!("claims end absent"))?
                    + start;
                let row = &original[start..end];
                let rows = (0..count)
                    .map(|i| {
                        if i == 0 {
                            row.to_owned()
                        } else {
                            row.replacen("\"id\":\"A\"", &format!("\"id\":\"A{i}\""), 1)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                format!("{}{rows}{}", &original[..start], &original[end..])
            } else {
                let row = if field == "conflicts" {
                    r#"{"claim_ids":["A"],"winner":null,"loser":null,"description":"bounded","resolution":"Unresolved"}"#
                } else {
                    r#"{"claim_id":"A","needed_evidence_class":"proof-receipt","detail":"bounded"}"#
                };
                original.replacen(
                    &format!("\"{field}\":[]"),
                    &format!("\"{field}\":[{}]", vec![row; count].join(",")),
                    1,
                )
            };
            assert_ne!(original, changed);
            f.source_json = f.source_json.replacen(&original, &changed, 1);
            repin_source(&mut f);
            fs::write(f.root.path().join("graph.json"), changed.as_bytes())?;
            f.intake["graph"]["sha256"] = json!(hash(changed.as_bytes()));
            outcome(
                &f,
                (count == 129).then_some("recorded graph exceeds row limit"),
                &format!("{field}:{count}"),
            )?;
            assert_eq!(
                fs::read(f.root.path().join("graph.json"))?,
                changed.as_bytes()
            );
        }
    }
    Ok(())
}

#[test]
fn intake_and_graph_schema_reject_independently_pinned_changes() -> Result<()> {
    let mut f = absent_fixture()?;
    outcome(&f, None, "schema baseline")?;
    f.intake["schema"] = json!("ub-review.recorded-file-input.v0");
    outcome(
        &f,
        Some("unsupported recorded intake schema"),
        "intake schema",
    )?;
    let mut f = absent_fixture()?;
    let original = fs::read_to_string(f.root.path().join("graph.json"))?;
    let changed = original.replace("ub-review.claim_graph.v1", "ub-review.claim_graph.v0");
    assert_ne!(original, changed);
    f.source_json = f.source_json.replacen(&original, &changed, 1);
    repin_source(&mut f);
    fs::write(f.root.path().join("graph.json"), changed.as_bytes())?;
    f.intake["graph"]["sha256"] = json!(hash(changed.as_bytes()));
    outcome(
        &f,
        Some("unsupported recorded graph schema/mode"),
        "graph schema",
    )?;
    Ok(())
}
