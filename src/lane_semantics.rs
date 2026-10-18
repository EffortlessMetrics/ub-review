//! Interim semantic preservation for #999, separate from execution receipts.
//! This is a view of the existing parsed output, not the SpecialistResult contract.

use crate::*;

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
) -> Result<()> {
    let dir = review_dir.join("semantic-shadow");
    fs::create_dir_all(&dir)?;
    let lanes: Vec<_> = receipts.iter().filter(|r| !r.thread_id.is_empty()).map(|r| {
        serde_json::json!({"lane":r.lane,"status":r.status,"conclusion":r.semantic_conclusion()})
    }).collect();
    fs::write(
        dir.join("primary.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema":"ub-review.semantic-shadow.v1", "authority":false,
            "evaluation":"parsed primary state only", "lanes":lanes,
        }))?,
    )?;
    Ok(())
}
