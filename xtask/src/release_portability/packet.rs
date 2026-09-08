use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::archive::sha256;

#[derive(Debug, Deserialize)]
struct Reason {
    kind: String,
    id: String,
}

#[derive(Debug, Deserialize)]
struct Gate {
    schema: String,
    conclusion: String,
    reasons: Vec<Reason>,
}

#[derive(Debug, Deserialize, PartialEq)]
struct Terminal {
    schema: String,
    status: String,
    #[serde(flatten)]
    other: BTreeMap<String, Value>,
}

#[derive(Deserialize)]
struct Review {
    terminal_state: Terminal,
}

#[derive(Debug, Serialize)]
pub(super) struct Artifact {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Debug, Serialize)]
pub(super) struct Inventory {
    schema: &'static str,
    artifact_count: usize,
    artifacts: Vec<Artifact>,
    gate_conclusion: String,
    terminal_status: String,
    all_json_parsed: bool,
    all_ndjson_parsed: bool,
}

fn read(path: &Path) -> Result<Vec<u8>> {
    let metadata =
        fs::symlink_metadata(path).with_context(|| format!("inspect {}", path.display()))?;
    ensure!(
        metadata.is_file() && metadata.len() <= 8 * 1024 * 1024,
        "packet member is not a bounded regular file"
    );
    fs::read(path).with_context(|| format!("read {}", path.display()))
}

pub(super) fn validate_init_defect(bytes: &[u8]) -> Result<()> {
    let gate: Gate = serde_json::from_slice(bytes).context("parse historical initializer gate")?;
    ensure!(
        gate.schema == "ub-review.gate_outcome.v1" && gate.conclusion == "fail",
        "initializer no longer records the historical failure"
    );
    let reasons: BTreeSet<_> = gate
        .reasons
        .iter()
        .map(|reason| (reason.kind.as_str(), reason.id.as_str()))
        .collect();
    ensure!(
        gate.reasons.len() == 2
            && reasons == BTreeSet::from([("policy", "providers"), ("policy", "impact.mode")]),
        "initializer gate does not contain exactly its two known historical policy failures"
    );
    Ok(())
}

pub(super) fn inventory(root: &Path) -> Result<Inventory> {
    for required in [
        "running-summary.md",
        "review/review.json",
        "review/terminal_state.json",
        "review/gate_outcome.json",
    ] {
        ensure!(
            !read(&root.join(required))?.is_empty(),
            "required packet artifact {required} is empty"
        );
    }
    let mut pending = vec![root.to_path_buf()];
    let mut paths = Vec::new();
    let mut entry_count = 0_usize;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            entry_count = entry_count
                .checked_add(1)
                .context("packet inventory count overflow")?;
            ensure!(entry_count <= 2048, "packet inventory exceeds entry budget");
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else {
                ensure!(kind.is_file(), "packet contains a linked or special member");
                paths.push(entry.path());
            }
        }
    }
    paths.sort();
    let mut artifacts = Vec::new();
    let mut total = 0_u64;
    for path in paths {
        let bytes = read(&path)?;
        let size = u64::try_from(bytes.len())?;
        total = total
            .checked_add(size)
            .context("packet inventory byte overflow")?;
        ensure!(
            total <= 64 * 1024 * 1024,
            "packet inventory exceeds total byte budget"
        );
        match path.extension().and_then(|extension| extension.to_str()) {
            Some("json") => {
                let _: Value = serde_json::from_slice(&bytes)
                    .with_context(|| format!("parse {}", path.display()))?;
            }
            Some("ndjson") => {
                for (number, line) in std::str::from_utf8(&bytes)?.lines().enumerate() {
                    if !line.trim().is_empty() {
                        let _: Value = serde_json::from_str(line).with_context(|| {
                            format!("parse {} line {}", path.display(), number + 1)
                        })?;
                    }
                }
            }
            _ => {}
        }
        artifacts.push(Artifact {
            path: path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/"),
            size,
            sha256: sha256(&bytes),
        });
    }
    let gate: Gate = serde_json::from_slice(&read(&root.join("review/gate_outcome.json"))?)?;
    let terminal: Terminal =
        serde_json::from_slice(&read(&root.join("review/terminal_state.json"))?)?;
    let review: Review = serde_json::from_slice(&read(&root.join("review/review.json"))?)?;
    ensure!(
        gate.schema == "ub-review.gate_outcome.v1" && gate.conclusion == "pass",
        "explicit-config model-off gate did not pass"
    );
    ensure!(
        terminal.schema == "ub-review.terminal_state.v1" && terminal.status == "artifact-only",
        "unexpected model-off terminal state"
    );
    ensure!(
        review.terminal_state == terminal,
        "review terminal state disagrees with terminal artifact"
    );
    Ok(Inventory {
        schema: "ub-review.release_packet_inventory.v2",
        artifact_count: artifacts.len(),
        artifacts,
        gate_conclusion: gate.conclusion,
        terminal_status: terminal.status,
        all_json_parsed: true,
        all_ndjson_parsed: true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn historical_initializer_defect_requires_exact_failure_set() -> Result<()> {
        let bytes = br#"{"schema":"ub-review.gate_outcome.v1","conclusion":"fail","reasons":[{"kind":"policy","id":"providers"},{"kind":"policy","id":"impact.mode"}]}"#;
        validate_init_defect(bytes)?;
        let text = std::str::from_utf8(bytes)?;
        ensure!(validate_init_defect(text.replace("impact.mode", "unrelated").as_bytes()).is_err());
        ensure!(validate_init_defect(text.replace("\"fail\"", "\"pass\"").as_bytes()).is_err());
        Ok(())
    }

    #[test]
    fn packet_inventory_rejects_malformed_json_and_conflicting_terminal() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let root = temp.path();
        fs::create_dir(root.join("review"))?;
        fs::write(root.join("running-summary.md"), "fixture")?;
        fs::write(
            root.join("review/gate_outcome.json"),
            r#"{"schema":"ub-review.gate_outcome.v1","conclusion":"pass","reasons":[]}"#,
        )?;
        let terminal =
            r#"{"schema":"ub-review.terminal_state.v1","status":"artifact-only","detail":"same"}"#;
        fs::write(root.join("review/terminal_state.json"), terminal)?;
        fs::write(
            root.join("review/review.json"),
            format!("{{\"terminal_state\":{terminal}}}"),
        )?;
        ensure!(inventory(root)?.artifact_count == 4);
        fs::write(root.join("malformed.ndjson"), "{\n")?;
        ensure!(inventory(root).is_err());
        fs::remove_file(root.join("malformed.ndjson"))?;
        fs::write(
            root.join("review/terminal_state.json"),
            terminal.replace("same", "different"),
        )?;
        ensure!(inventory(root).is_err());
        Ok(())
    }
}
