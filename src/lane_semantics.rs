//! Interim semantic preservation for #999, separate from execution receipts.
//! This is a view of the existing parsed output, not the SpecialistResult contract.

use crate::*;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::PathBuf;

/// Freshness witness for inspection only. A stage is current only when its
/// entire invocation object equals `review.json.semantic_shadow_invocation`.
/// Missing or mismatched metadata is unknown/stale, including same-head reruns.
/// This does not grant semantic or public authority, or qualify an aborted run
/// which did not publish a current `review.json`.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct ShadowInvocation {
    pub(crate) id: String,
    pub(crate) base: String,
    pub(crate) head: String,
    pub(crate) shared_prefix_hash: String,
}

impl ShadowInvocation {
    pub(crate) fn new(base: &str, head: &str, shared_prefix_hash: &str) -> Self {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nonce = format!(
            "{}:{}:{}",
            Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true),
            std::process::id(),
            SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
        );
        Self {
            id: sha256_hex(nonce.as_bytes()),
            base: base.to_owned(),
            head: head.to_owned(),
            shared_prefix_hash: shared_prefix_hash.to_owned(),
        }
    }
}

struct StagedShadowFile {
    path: PathBuf,
    file: Option<File>,
    published: bool,
}

impl Drop for StagedShadowFile {
    fn drop(&mut self) {
        drop(self.file.take());
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// Replace one observational shadow artifact only after its complete contents
/// have been written and flushed to a same-directory staging file. Rename gives
/// atomic visibility on supported filesystems; this does not sync the file or
/// directory and therefore does not claim crash durability.
pub(crate) fn write_semantic_shadow_json(path: &Path, bytes: &[u8]) -> Result<()> {
    write_semantic_shadow_json_with(path, bytes, |file, bytes| file.write_all(bytes))
}

fn write_semantic_shadow_json_with(
    path: &Path,
    bytes: &[u8],
    write_bytes: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
) -> Result<()> {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let parent = path
        .parent()
        .context("semantic shadow path has no parent")?;
    let name = path
        .file_name()
        .context("semantic shadow path has no file name")?
        .to_string_lossy();
    let prior_permissions = match fs::metadata(path) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).context("inspect prior semantic shadow file"),
    };
    let (staged_path, file) = loop {
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let staged_path = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), sequence));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staged_path)
        {
            Ok(file) => break (staged_path, file),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("create semantic shadow staging file"),
        }
    };
    let mut staged = StagedShadowFile {
        path: staged_path,
        file: Some(file),
        published: false,
    };
    let staged_file = staged
        .file
        .as_mut()
        .context("semantic shadow staging file unexpectedly closed")?;
    if let Some(permissions) = prior_permissions {
        staged_file
            .set_permissions(permissions)
            .context("preserve semantic shadow file permissions")?;
    }
    write_bytes(staged_file, bytes).context("write semantic shadow staging file")?;
    staged_file
        .flush()
        .context("flush semantic shadow staging file")?;
    drop(staged.file.take());
    fs::rename(&staged.path, path).context("atomically publish semantic shadow file")?;
    staged.published = true;
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct LaneSemanticState {
    pub(crate) shared_prefix_hash: String,
    pub(crate) conclusion: String,
}

impl LaneSemanticState {
    pub(crate) fn from_output(output: &LaneModelOutput, shared_prefix_hash: &str) -> Result<Self> {
        // Select semantic fields explicitly: private audits and failed objections
        // must remain artifact-only, even when a reporter asks for prior context.
        let observations: Vec<_> = output
            .observations
            .iter()
            .filter(|observation| {
                !matches!(
                    observation.kind.as_deref(),
                    Some(
                        "empty-internal-audit"
                            | "malformed-internal-audit"
                            | "internal-audit-classification"
                    )
                ) && !matches!(
                    observation.dedupe_key.as_deref(),
                    Some("lane-output-shape-failed_objections" | "lane-output-malformed-content")
                )
            })
            .collect();
        let conclusion = serde_json::to_string(&serde_json::json!({
            "summary": output.summary,
            "inline_comments": output.inline_comments,
            "candidate_findings": output.candidate_findings,
            "summary_only_findings": output.summary_only_findings,
            "observations": observations,
            "proof_requests": output.proof_requests,
            "proof_intents": output.proof_intents,
            "degraded": output.degraded,
            "limits": "Parsed specialist evidence only; zero findings do not establish complete coverage or clean evidence. Private audit is excluded."
        }))?;
        Ok(Self {
            shared_prefix_hash: shared_prefix_hash.to_owned(),
            conclusion,
        })
    }
}

impl ModelLaneReceipt {
    pub(crate) fn semantic_conclusion(&self) -> String {
        if self.shared_prefix_hash.is_empty() {
            return "Missing semantic evidence: shared context identity is absent.".to_owned();
        }
        match &self.semantic_state {
            Some(state) if state.shared_prefix_hash != self.shared_prefix_hash =>
                "Missing semantic evidence: stale shared context; no clean conclusion is available.".to_owned(),
            Some(state) if !state.conclusion.trim().is_empty() => state.conclusion.clone(),
            _ => "Missing semantic evidence: no substantive lane result; operational completion is not a clean conclusion.".to_owned(),
        }
    }
}

/// Inspection sidecar only. Its consumer is a reviewer or explicit shadow
/// evaluation; the live model/public/gate paths never read this artifact.
pub(crate) fn write_primary_semantic_shadow(
    review_dir: &Path,
    receipts: &[ModelLaneReceipt],
    invocation: Option<&ShadowInvocation>,
) -> Result<()> {
    let dir = review_dir.join("semantic-shadow");
    fs::create_dir_all(&dir)?;
    let lanes: Vec<_> = receipts.iter().filter(|r| !r.thread_id.is_empty()).map(|r| {
        serde_json::json!({"lane":r.lane,"status":r.status,"conclusion":r.semantic_conclusion()})
    }).collect();
    write_semantic_shadow_json(
        &dir.join("primary.json"),
        &serde_json::to_vec_pretty(&serde_json::json!({
            "schema":"ub-review.semantic-shadow.v1", "authority":false,
            "invocation": invocation,
            "evaluation":"parsed primary state only", "lanes":lanes,
        }))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod atomic_shadow_write_tests {
    use super::*;
    use anyhow::ensure;

    #[test]
    fn partial_stage_write_failure_preserves_the_prior_shadow_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let destination = temp.path().join("reporter-001.json");
        let prior = br#"{"schema":"ub-review.semantic-shadow.v1","authority":false,"invocation":{"id":"prior"}}"#;
        write_semantic_shadow_json(&destination, prior)?;

        let attempted = vec![b'x'; 4096];
        let error = write_semantic_shadow_json_with(&destination, &attempted, |file, bytes| {
            file.write_all(&bytes[..1024])?;
            Err(io::Error::other("injected short-write failure"))
        })
        .err()
        .context("partial stage write unexpectedly succeeded")?;

        ensure!(
            format!("{error:#}").contains("injected short-write failure"),
            "unexpected write failure: {error:#}"
        );
        let retained = fs::read(&destination)?;
        ensure!(
            retained == prior,
            "prior shadow bytes changed after failed stage write"
        );
        let parsed: serde_json::Value = serde_json::from_slice(&retained)?;
        ensure!(
            parsed["invocation"]["id"] == "prior",
            "prior valid invocation was not retained"
        );
        let entries = fs::read_dir(temp.path())?.collect::<io::Result<Vec<_>>>()?;
        ensure!(entries.len() == 1, "failed stage file was not cleaned up");
        Ok(())
    }

    #[test]
    fn successful_stage_write_replaces_the_shadow_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let destination = temp.path().join("reporter-001.json");
        let prior = br#"{"invocation":{"id":"prior"}}"#;
        let current = br#"{"invocation":{"id":"current"}}"#;
        write_semantic_shadow_json(&destination, prior)?;
        write_semantic_shadow_json(&destination, current)?;

        ensure!(
            fs::read(&destination)? == current,
            "new shadow bytes were not published"
        );
        let entries = fs::read_dir(temp.path())?.collect::<io::Result<Vec<_>>>()?;
        ensure!(
            entries.len() == 1,
            "staging file remained after publication"
        );
        Ok(())
    }
}
