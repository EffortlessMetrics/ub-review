//! Admission for native counts and the explicitly supported RIPR badge schemas.
//! Parse the original JSON once so duplicate authority fields cannot be erased
//! by a Value/map normalization or hidden by fallback to another format.

use std::fmt;

use serde::de::{MapAccess, Visitor, value::MapAccessDeserializer};
use serde::{Deserialize, Deserializer};

#[derive(Deserialize)]
struct GateReceipt {
    #[serde(default, deserialize_with = "present")]
    new_unsuppressed: Option<u64>,
    #[serde(default, deserialize_with = "present")]
    schema_version: Option<String>,
    #[serde(default, deserialize_with = "counts_object")]
    counts: Option<BadgeCounts>,
    #[serde(default, deserialize_with = "present")]
    preview_skipped: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct BadgeCounts {
    unsuppressed_exposure_gaps: u64,
}

pub(super) enum CountEvidence {
    Complete(u64),
    Incomplete(u64),
}

// Missing fields default to None; present nulls must not masquerade as absent
// fields. Serde's struct visitor also rejects duplicate known fields.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

fn counts_object<'de, D>(deserializer: D) -> Result<Option<BadgeCounts>, D::Error>
where
    D: Deserializer<'de>,
{
    struct CountsVisitor;

    impl<'de> Visitor<'de> for CountsVisitor {
        type Value = BadgeCounts;

        fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str("a counts object")
        }

        fn visit_map<A>(self, map: A) -> Result<Self::Value, A::Error>
        where
            A: MapAccess<'de>,
        {
            BadgeCounts::deserialize(MapAccessDeserializer::new(map))
        }
    }

    // Derived structs otherwise accept positional arrays such as [0].
    deserializer.deserialize_map(CountsVisitor).map(Some)
}

pub(super) fn parse(text: &str) -> Result<CountEvidence, String> {
    if !text.trim_start().starts_with('{') {
        return Err("gate-decision receipt must be a JSON object".to_owned());
    }
    let receipt: GateReceipt = serde_json::from_str(text).map_err(|err| err.to_string())?;
    match (
        receipt.new_unsuppressed,
        receipt.schema_version,
        receipt.counts,
        receipt.preview_skipped,
    ) {
        (Some(count), None, None, None) => Ok(CountEvidence::Complete(count)),
        (None, Some(version), Some(counts), preview_skipped) => {
            // 0.5 is the retained RIPR 0.8.0 contract. Released RIPR 0.10.0
            // declares 0.6 in output/badge/model.rs and always emits the new
            // preview_skipped field; nonempty means incomplete coverage.
            if !matches!(version.as_str(), "0.5" | "0.6") {
                return Err(format!(
                    "unsupported RIPR badge schema_version {version:?}; expected 0.5 or 0.6"
                ));
            }
            if version == "0.6" && preview_skipped.is_none() {
                return Err("RIPR badge schema_version 0.6 requires preview_skipped".to_owned());
            }
            // The known-schema count remains a lower bound when a language
            // was skipped. Preserve an observed violation, but never use that
            // lower bound to prove that the complete count is within policy.
            if preview_skipped.is_some_and(|languages| !languages.is_empty()) {
                Ok(CountEvidence::Incomplete(counts.unsuppressed_exposure_gaps))
            } else {
                Ok(CountEvidence::Complete(counts.unsuppressed_exposure_gaps))
            }
        }
        _ => Err(
            "gate-decision receipt must contain either native new_unsuppressed or a versioned \
             RIPR badge with schema_version and counts, never mixed authority fields"
                .to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests;
