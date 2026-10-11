//! Local recorded-file admission for #1335. Pins come from an independently
//! retained intake. Files must remain immutable during reading; these portable
//! checks do not claim to defeat concurrent filesystem substitution races.
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, ensure};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};

use super::{RecordedReviewExport, export_recorded_review, replay_recorded_review_export};
use crate::claim_graph::{ClaimEvidenceGap, ClaimGraph, ClaimNode, ConflictRecord};
use crate::review_judgment::public_plan::{
    DeliverySubject, MaterialClass, PublicPlanInput, RecordedAnchor, RecordedMaterial,
    RecordedPublicSource,
};
use crate::review_judgment::{
    ImportedCi, JudgmentSubject, RecordedEvidence, RecordedJudgmentSource,
};
use crate::revision_identity::RevisionIdentity;
use crate::{RevisionRef, sha256_hex};

const OBJECT_LIMIT: usize = 256 * 1024;
const CONTEXT_LIMIT: usize = 1024 * 1024;

#[derive(Debug, Args)]
pub(crate) struct RecordedReviewArgs {
    #[command(subcommand)]
    command: Operation,
}

#[derive(Debug, Subcommand)]
enum Operation {
    /// Print one shadow source view; missing/invalid lead remains explicit.
    Export(Inputs),
    /// Revalidate sources and compare an independently pinned prior view.
    Replay(Replay),
    /// Print a pure source/payload/observation projection at a pinned watermark.
    AgentExport(Inputs),
    /// Independently replay a recorded agent projection; performs no publication.
    AgentReplay(Replay),
}

#[derive(Debug, Args)]
struct Inputs {
    /// Immutable transferred input directory; no Git or network lookup.
    #[arg(long)]
    evidence_root: PathBuf,
    /// Root-relative recorded-file intake JSON.
    #[arg(long)]
    intake: String,
    /// Intake SHA256 retained independently before consuming its files.
    #[arg(long)]
    intake_sha256: String,
}

#[derive(Debug, Args)]
struct Replay {
    #[command(flatten)]
    inputs: Inputs,
    /// Root-relative exact view bytes from the prior export's stdout.
    #[arg(long)]
    view: String,
    #[arg(long)]
    view_sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FilePin {
    path: String,
    sha256: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceFile {
    reference: String,
    file: FilePin,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MaterialFile {
    claim_id: String,
    body_ref: String,
    class: MaterialClass,
    file: FilePin,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Anchor {
    reference: String,
    claim_id: String,
    path: String,
    line: u32,
    line_digest: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Intake {
    schema: String,
    revision: FilePin,
    graph: FilePin,
    subject: JudgmentSubject,
    review_complete: bool,
    source_manifest_digest: String,
    evidence: Vec<EvidenceFile>,
    imported_ci: Option<FilePin>,
    delivery_subject: DeliverySubject,
    diff: FilePin,
    public_source_digest: String,
    materials: Vec<MaterialFile>,
    anchors: Vec<Anchor>,
    answer: Option<FilePin>,
    plan: Option<FilePin>,
    observations: Option<ObservationFiles>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservationFiles {
    payload: FilePin,
    payload_source_digest: String,
    stream: FilePin,
    watermark: FilePin,
    watermark_name: String,
}

/// Decode the existing graph wire without changing its static schema/mode
/// representation. This bounded subset rejects topic-bearing graphs explicitly;
/// it never drops active topics or takes over their compiler/transport owner.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GraphFile {
    schema: String,
    head_sha: String,
    revision: RevisionRef,
    claims: Vec<ClaimNode>,
    topics: Vec<serde_json::Value>,
    conflicts: Vec<ConflictRecord>,
    evidence_gaps: Vec<ClaimEvidenceGap>,
    mode: String,
}

fn json<T: serde::de::DeserializeOwned>(bytes: &[u8], label: &str) -> Result<T> {
    // Do not echo arbitrary malformed/private values into the CLI diagnostic.
    serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!("invalid {label} JSON"))
}

fn read_pin(root: &Path, pin: &FilePin, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        pin.sha256.len() == 64
            && pin
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid file SHA256 pin"
    );
    ensure!(
        !pin.path.is_empty()
            && !pin.path.contains(['\\', ':', '\0'])
            && pin
                .path
                .split('/')
                .all(|part| !part.is_empty() && part != "." && part != ".."),
        "unsafe recorded file path"
    );
    let relative = Path::new(&pin.path);
    ensure!(
        relative
            .components()
            .all(|c| matches!(c, Component::Normal(_))),
        "unsafe recorded file path"
    );
    let mut path = root.to_path_buf();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path).context("inspect recorded file component")?;
        ensure!(
            !metadata.file_type().is_symlink(),
            "recorded symlink rejected"
        );
        if components.peek().is_some() {
            ensure!(metadata.is_dir(), "recorded parent is not a directory");
        } else {
            ensure!(metadata.is_file(), "recorded input is not a regular file");
            ensure!(
                metadata.len() <= limit as u64,
                "recorded file exceeds byte limit"
            );
        }
    }
    let file = File::open(&path).context("open recorded file")?;
    ensure!(
        file.metadata()?.is_file(),
        "recorded input is not a regular file"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .context("read recorded file")?;
    ensure!(bytes.len() <= limit, "recorded file exceeds byte limit");
    ensure!(
        sha256_hex(&bytes) == pin.sha256,
        "recorded file SHA256 mismatch"
    );
    Ok(bytes)
}

pub(crate) fn run(args: RecordedReviewArgs) -> Result<()> {
    let agent = matches!(
        &args.command,
        Operation::AgentExport(_) | Operation::AgentReplay(_)
    );
    let (inputs, replay) = match &args.command {
        Operation::Export(inputs) | Operation::AgentExport(inputs) => (inputs, None),
        Operation::Replay(replay) | Operation::AgentReplay(replay) => {
            (&replay.inputs, Some(replay))
        }
    };
    let root_metadata =
        fs::symlink_metadata(&inputs.evidence_root).context("inspect evidence root")?;
    ensure!(
        root_metadata.is_dir() && !root_metadata.file_type().is_symlink(),
        "evidence root must be a directory, not a symlink"
    );
    let root = inputs
        .evidence_root
        .canonicalize()
        .context("resolve evidence root")?;
    let intake_bytes = read_pin(
        &root,
        &FilePin {
            path: inputs.intake.clone(),
            sha256: inputs.intake_sha256.clone(),
        },
        OBJECT_LIMIT,
    )?;
    let intake: Intake = json(&intake_bytes, "intake")?;
    ensure!(
        intake.schema
            == if agent {
                "ub-review.recorded-agent-file-input.v1"
            } else {
                "ub-review.recorded-file-input.v1"
            }
            && intake.observations.is_some() == agent,
        "unsupported recorded intake schema"
    );
    ensure!(
        intake.evidence.len() <= 128
            && intake.materials.len() <= 128
            && intake.anchors.len() <= 128,
        "recorded intake exceeds row limit"
    );
    let revision_bytes = read_pin(&root, &intake.revision, OBJECT_LIMIT)?;
    let canonical = std::str::from_utf8(&revision_bytes).context("revision file must be UTF-8")?;
    let revision = RevisionIdentity::from_canonical(canonical)
        .map_err(|_| anyhow::anyhow!("invalid recorded revision identity"))?;
    let graph_bytes = read_pin(&root, &intake.graph, CONTEXT_LIMIT)?;
    let wire: GraphFile = json(&graph_bytes, "claim graph")?;
    ensure!(
        wire.schema == crate::artifacts::CLAIM_GRAPH_SCHEMA && wire.mode == "active",
        "unsupported recorded graph schema/mode"
    );
    ensure!(
        wire.topics.is_empty(),
        "recorded file subset does not admit topic-bearing graphs"
    );
    ensure!(
        wire.claims.len() <= 128 && wire.conflicts.len() <= 128 && wire.evidence_gaps.len() <= 128,
        "recorded graph exceeds row limit"
    );
    let graph = ClaimGraph {
        schema: crate::artifacts::CLAIM_GRAPH_SCHEMA,
        head_sha: wire.head_sha,
        revision: Some(wire.revision),
        claims: wire.claims,
        topics: vec![],
        conflicts: wire.conflicts,
        evidence_gaps: wire.evidence_gaps,
        mode: "active",
    };
    ensure!(
        serde_json::to_value(&graph)? == json::<serde_json::Value>(&graph_bytes, "claim graph")?,
        "recorded graph has unsupported or discarded fields"
    );
    let mut budget = CONTEXT_LIMIT;
    let evidence_bytes = intake
        .evidence
        .iter()
        .map(|e| {
            let bytes = read_pin(&root, &e.file, budget)?;
            budget -= bytes.len();
            Ok(bytes)
        })
        .collect::<Result<Vec<_>>>()?;
    let ci_bytes = intake
        .imported_ci
        .as_ref()
        .map(|pin| read_pin(&root, pin, CONTEXT_LIMIT))
        .transpose()?;
    let diff = read_pin(&root, &intake.diff, CONTEXT_LIMIT)?;
    let bodies = intake
        .materials
        .iter()
        .map(|m| read_pin(&root, &m.file, 6000))
        .collect::<Result<Vec<_>>>()?;
    let answer = intake
        .answer
        .as_ref()
        .map(|pin| read_pin(&root, pin, OBJECT_LIMIT))
        .transpose()?;
    let plan = intake
        .plan
        .as_ref()
        .map(|pin| read_pin(&root, pin, OBJECT_LIMIT))
        .transpose()?;
    let evidence = intake
        .evidence
        .iter()
        .zip(&evidence_bytes)
        .map(|(entry, bytes)| RecordedEvidence {
            reference: &entry.reference,
            digest: &entry.file.sha256,
            bytes,
        })
        .collect::<Vec<_>>();
    let source =
        RecordedJudgmentSource {
            subject: &intake.subject,
            revision_canonical: canonical,
            graph: &graph,
            review_complete: intake.review_complete,
            evidence: &evidence,
            imported_ci: intake.imported_ci.as_ref().zip(ci_bytes.as_deref()).map(
                |(pin, bytes)| ImportedCi {
                    digest: &pin.sha256,
                    bytes,
                },
            ),
        };
    let materials = intake
        .materials
        .iter()
        .zip(&bodies)
        .map(|(entry, body)| RecordedMaterial {
            claim_id: &entry.claim_id,
            body_ref: &entry.body_ref,
            body_digest: &entry.file.sha256,
            class: entry.class,
            body,
        })
        .collect::<Vec<_>>();
    let anchors = intake
        .anchors
        .iter()
        .map(|a| RecordedAnchor {
            reference: &a.reference,
            claim_id: &a.claim_id,
            path: &a.path,
            line: a.line,
            line_digest: &a.line_digest,
        })
        .collect::<Vec<_>>();
    let diff_digest = RevisionIdentity::diff_digest(&diff);
    let public = RecordedPublicSource {
        delivery_subject: &intake.delivery_subject,
        diff_digest: &diff_digest,
        diff_patch: &diff,
        materials: &materials,
        anchors: &anchors,
    };
    let input = PublicPlanInput {
        judgment_source: &source,
        current_revision: &revision,
        expected_source_digest: &intake.source_manifest_digest,
        answer: answer.as_deref(),
        expected_answer_digest: intake.answer.as_ref().map(|p| p.sha256.as_str()),
        observed_ci: ci_bytes.as_deref(),
        public_source: &public,
        expected_public_source_digest: &intake.public_source_digest,
        plan: plan.as_deref(),
        expected_plan_digest: intake.plan.as_ref().map(|p| p.sha256.as_str()),
    };
    let output = if agent {
        use crate::delivery_transaction::recorded_observations::ObservationInput;
        use crate::review_judgment::public_plan::payload::RecordedPreparedPayload;
        let pins = intake
            .observations
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("observation intake absent"))?;
        let payload = read_pin(&root, &pins.payload, OBJECT_LIMIT)?;
        let stream = read_pin(&root, &pins.stream, CONTEXT_LIMIT)?;
        let watermark = read_pin(&root, &pins.watermark, 4096)?;
        let revision_ref = revision.revision_ref();
        let prepared = RecordedPreparedPayload {
            subject: &intake.subject,
            delivery_subject: &intake.delivery_subject,
            revision: &revision_ref,
            judgment_digest: input
                .expected_answer_digest
                .ok_or_else(|| anyhow::anyhow!("recorded lead absent"))?,
            public_plan_digest: input
                .expected_plan_digest
                .ok_or_else(|| anyhow::anyhow!("recorded plan absent"))?,
            payload_digest: &pins.payload.sha256,
            bytes: &payload,
        };
        let observation = ObservationInput {
            source: input,
            payload: &payload,
            payload_sha256: &pins.payload.sha256,
            stream: &stream,
            stream_sha256: &pins.stream.sha256,
            watermark: &watermark,
            watermark_sha256: &pins.watermark.sha256,
            watermark_name: &pins.watermark_name,
        };
        if let Some(replay) = replay {
            let prior = read_pin(
                &root,
                &FilePin {
                    path: replay.view.clone(),
                    sha256: replay.view_sha256.clone(),
                },
                16 * CONTEXT_LIMIT,
            )?;
            super::agent_result::replay(
                observation,
                &prepared,
                &pins.payload_source_digest,
                &prior,
                &replay.view_sha256,
            )
            .map_err(|_| anyhow::anyhow!("recorded agent replay rejected"))?;
            let mut bytes = serde_json::to_vec(&serde_json::json!({
                "schema":"ub-review.recorded-agent-file-replay.v1", "status":"replayed_recorded_sources_and_prefix",
                "view_sha256":replay.view_sha256, "qualification":"shadow_recorded_only_pending_dependency_review",
                "remote_authenticity":"unverified", "whole_review_delivery":"not_qualified"
            }))?;
            bytes.push(b'\n');
            bytes
        } else {
            super::agent_result::export(observation, &prepared, &pins.payload_source_digest)
                .map_err(|_| anyhow::anyhow!("recorded agent validation rejected"))?
        }
    } else if let Some(replay) = replay {
        let view_json = read_pin(
            &root,
            &FilePin {
                path: replay.view.clone(),
                sha256: replay.view_sha256.clone(),
            },
            4 * CONTEXT_LIMIT,
        )?;
        let recorded = RecordedReviewExport {
            view_json,
            imported_ci_bytes: ci_bytes.clone(),
        };
        replay_recorded_review_export(input, &recorded, &replay.view_sha256)
            .map_err(|_| anyhow::anyhow!("recorded export replay rejected"))?;
        #[derive(Serialize)]
        struct Receipt<'a> {
            schema: &'static str,
            status: &'static str,
            view_sha256: &'a str,
            qualification: &'static str,
            remote_effects: &'static str,
        }
        let mut bytes = serde_json::to_vec(&Receipt {
            schema: "ub-review.recorded-file-replay.v1",
            status: "replayed_recorded_sources",
            view_sha256: &replay.view_sha256,
            qualification: "shadow_recorded_only",
            remote_effects: "unknown",
        })?;
        bytes.push(b'\n');
        bytes
    } else {
        export_recorded_review(input)
            .map_err(|_| anyhow::anyhow!("recorded source validation rejected"))?
            .view_json
    };
    std::io::stdout()
        .lock()
        .write_all(&output)
        .context("write recorded response")?;
    Ok(())
}
