//! Owner: ub-review/tests; proposal consumer: #1067, comparison-only #813.
//! Synthetic native-shape rows, not copied analyzer output or release evidence.
use anyhow::{Context, Result, bail, ensure};
use serde::de::{MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub const CANDIDATE_SOURCE: &str = "a734bc1acc0f6ef53a3101c55a5aa9ac6fc2418e";
const GAPS: &[&str] = &["weakly_exposed", "reachable_unrevealed", "no_static_path"];
const UNKNOWNS: &[&str] = &["infection_unknown", "propagation_unknown", "static_unknown"];

#[derive(Clone, Copy, Debug)]
pub enum Admission {
    LegacyUnspecified0100,
    CandidateA734,
}

#[derive(Debug, PartialEq)]
pub struct Inventory {
    pub raw_rows: usize,
    pub raw_gaps: u64,
    pub raw_unknowns: u64,
    pub current_gaps: u64,
    pub current_unknowns: u64,
    pub current_ids: BTreeSet<String>,
    pub retained_noncurrent_ids: BTreeSet<String>,
}

// Unlike Value's default JSON decoder, reject duplicate keys before binding.
struct UniqueJson(Value);
impl<'de> Deserialize<'de> for UniqueJson {
    fn deserialize<D: Deserializer<'de>>(decoder: D) -> std::result::Result<Self, D::Error> {
        struct UniqueVisitor;
        impl<'de> Visitor<'de> for UniqueVisitor {
            type Value = UniqueJson;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("JSON without duplicate keys")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                let mut object = serde_json::Map::new();
                while let Some((key, UniqueJson(value))) = map.next_entry::<String, UniqueJson>()? {
                    if object.insert(key.clone(), value).is_some() {
                        return Err(serde::de::Error::custom(format!(
                            "duplicate JSON key: {key}"
                        )));
                    }
                }
                Ok(UniqueJson(Value::Object(object)))
            }
            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut seq: S,
            ) -> std::result::Result<Self::Value, S::Error> {
                let mut items = Vec::new();
                while let Some(UniqueJson(value)) = seq.next_element()? {
                    items.push(value);
                }
                Ok(UniqueJson(Value::Array(items)))
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                value: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_i64<E: serde::de::Error>(
                self,
                value: i64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_u64<E: serde::de::Error>(
                self,
                value: u64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_f64<E: serde::de::Error>(
                self,
                value: f64,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_str<E: serde::de::Error>(
                self,
                value: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(json!(value)))
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(UniqueJson(Value::Null))
            }
        }
        decoder.deserialize_any(UniqueVisitor)
    }
}

pub fn decode(bytes: &[u8]) -> Result<Value> {
    Ok(serde_json::from_slice::<UniqueJson>(bytes)?.0)
}
fn count(value: &Value, key: &str) -> Result<u64> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .with_context(|| format!("missing/invalid counter {key}"))
}

// A test oracle for this one frozen source contract, never a production parser.
// Caller must independently bind artifact/version/source custody before use.
pub fn verify(raw: &Value, badge: &Value, admission: Admission, source: &str) -> Result<Inventory> {
    ensure!(
        raw["schema_version"] == "0.2" && raw["tool"] == "ripr" && raw["mode"] == "ready",
        "raw admission"
    );
    let schema = match admission {
        Admission::LegacyUnspecified0100 => {
            ensure!(source == "registry:0.10.0", "legacy source admission");
            "0.6"
        }
        Admission::CandidateA734 => {
            ensure!(source == CANDIDATE_SOURCE, "candidate source admission");
            "0.8"
        }
    };
    ensure!(
        badge["schema_version"] == schema
            && badge["kind"] == "ripr"
            && badge["scope"] == "diff"
            && badge["basis"] == "finding_exposure",
        "badge admission"
    );
    let rows = raw["findings"].as_array().context("findings array")?;
    ensure!(
        count(&raw["summary"], "findings")? == rows.len() as u64,
        "raw denominator"
    );
    ensure!(
        count(&raw["summary"], "probes")? == rows.len() as u64,
        "probe denominator"
    );
    ensure!(
        count(&badge["counts"], "analyzed_findings")? == rows.len() as u64,
        "native denominator"
    );
    if matches!(admission, Admission::CandidateA734) {
        ensure!(
            raw["analysis_complete"] == true
                && badge["analysis_complete"] == true
                && raw["downstream_consumable"] == true,
            "incomplete evidence"
        );
        ensure!(
            raw["run_limitations"].as_array().is_some_and(Vec::is_empty),
            "run limitations"
        );
        let outcome = &badge["analysis_outcome"];
        ensure!(
            raw["analysis_outcome"]["outcome"] == *outcome,
            "outcome disagreement"
        );
        ensure!(outcome["schema_version"] == "0.1", "typed schema admission");
        ensure!(
            matches!(
                outcome["kind"].as_str(),
                Some("complete_with_findings" | "no_behavioral_candidates")
            ),
            "typed outcome incomplete"
        );
        ensure!(
            (outcome["kind"] == "no_behavioral_candidates") == rows.is_empty(),
            "typed outcome kind/denominator disagreement"
        );
        // Deliberately bounded to the complete, limitation-free Rust packet.
        // eol_only_churn and other disclosures need separate owner fixtures.
        ensure!(
            outcome["limitations"].as_array().is_some_and(Vec::is_empty),
            "unsupported bounded disclosure"
        );
        ensure!(
            count(&outcome["counts"], "finding_count")? == rows.len() as u64
                && count(&outcome["counts"], "probe_count")? == rows.len() as u64,
            "typed denominator"
        );
    }
    let mut result = Inventory {
        raw_rows: rows.len(),
        raw_gaps: 0,
        raw_unknowns: 0,
        current_gaps: 0,
        current_unknowns: 0,
        current_ids: BTreeSet::new(),
        retained_noncurrent_ids: BTreeSet::new(),
    };
    let mut ids = BTreeSet::new();
    let mut classes = BTreeMap::<&str, u64>::new();
    for row in rows {
        let id = row["id"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .context("finding id")?;
        ensure!(ids.insert(id), "duplicate finding id");
        let class = row["classification"].as_str().context("classification")?;
        ensure!(
            GAPS.contains(&class) || UNKNOWNS.contains(&class) || class == "exposed",
            "unsupported classification"
        );
        *classes.entry(class).or_default() += 1;
        let current = match row.get("source_currentness") {
            Some(Value::String(value)) if value == "candidate_current" => true,
            Some(Value::String(value))
                if matches!(value.as_str(), "base_deleted" | "moved_or_renamed") =>
            {
                false
            }
            None if matches!(admission, Admission::LegacyUnspecified0100) => true,
            _ => bail!("unresolved currentness for {id}"),
        };
        if current {
            result.current_ids.insert(id.to_owned());
        } else {
            result.retained_noncurrent_ids.insert(id.to_owned());
        }
        if GAPS.contains(&class) {
            result.raw_gaps += 1;
            result.current_gaps += u64::from(current);
        }
        if UNKNOWNS.contains(&class) {
            result.raw_unknowns += 1;
            result.current_unknowns += u64::from(current);
        }
    }
    for class in GAPS.iter().chain(UNKNOWNS).chain([&"exposed"]) {
        ensure!(
            count(&raw["summary"], class)? == classes.get(class).copied().unwrap_or_default(),
            "raw classification denominator {class}"
        );
    }
    ensure!(
        result.current_gaps
            == count(&badge["counts"], "unsuppressed_exposure_gaps")?
                .checked_add(count(&badge["counts"], "suppressed_exposure_gaps")?)
                .context("counter overflow")?,
        "current gap disagreement"
    );
    ensure!(
        result.current_unknowns == count(&badge["counts"], "unknowns")?,
        "current unknown disagreement"
    );
    Ok(result)
}

pub fn fixture(candidate: bool) -> (Value, Value) {
    let rows: Vec<Value> = (0..46).map(|index| {
        let class = if index < 33 { UNKNOWNS[index % 3] } else if index < 35 { GAPS[index % 3] } else { "exposed" };
        let currentness = match index { 0 | 1 => "base_deleted", 2 => "moved_or_renamed", _ => "candidate_current" };
        let mut row = json!({"id":format!("synthetic-{index}"), "classification":class, "source_currentness":currentness, "probe":{"file":"src/lib.rs","line":index+1,"expression":"subject"}});
        if !candidate && let Some(object) = row.as_object_mut() { object.remove("source_currentness"); }
        row
    }).collect();
    let mut summary = json!({"findings":46,"probes":46});
    for class in GAPS.iter().chain(UNKNOWNS).chain([&"exposed"]) {
        summary[*class] = json!(
            rows.iter()
                .filter(|row| row["classification"] == *class)
                .count()
        );
    }
    let outcome = json!({"schema_version":"0.1","kind":"complete_with_findings","limitations":[],"counts":{"finding_count":46,"probe_count":46}});
    let mut raw = json!({"schema_version":"0.2","tool":"ripr","mode":"ready","summary":summary,"findings":rows,"analysis_complete":true,"downstream_consumable":true,"run_limitations":[],"analysis_outcome":{"outcome":outcome}});
    let mut badge = json!({"schema_version":if candidate {"0.8"} else {"0.6"},"kind":"ripr","scope":"diff","basis":"finding_exposure","analysis_complete":true,"analysis_outcome":outcome,"counts":{"analyzed_findings":46,"unsuppressed_exposure_gaps":2,"suppressed_exposure_gaps":0,"unknowns":if candidate {30} else {33}}});
    if !candidate {
        if let Some(object) = raw.as_object_mut() {
            for key in [
                "analysis_complete",
                "downstream_consumable",
                "run_limitations",
                "analysis_outcome",
            ] {
                object.remove(key);
            }
        }
        if let Some(object) = badge.as_object_mut() {
            for key in ["analysis_complete", "analysis_outcome"] {
                object.remove(key);
            }
        }
    }
    (raw, badge)
}
