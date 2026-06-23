//! Impact plan v1: Cargo workspace graph, changed-package ownership,
//! reverse-dependency closure, and deterministic test/build candidate
//! selection with reasons.
//!
//! **Shadow mode**: this module computes and emits the impact plan artifact
//! but does NOT change proof execution. The existing planner continues to
//! select candidates from diff filenames and model requests. The shadow
//! artifact lets us compare what the impact planner WOULD select against
//! what currently runs.
//!
//! Order 1 of the evidence-control-plane epic (#655).

use serde::Serialize;
use std::path::Path;

use crate::artifacts::IMPACT_PLAN_SCHEMA;

/// The complete impact plan for a single run. Written to
/// `review/impact_plan.json` as a shadow artifact.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ImpactPlan {
    pub(crate) schema: &'static str,
    /// Repository-relative changed files from the diff.
    pub(crate) changed_files: Vec<String>,
    /// Packages identified as owning one or more changed files.
    /// Empty until Cargo metadata parsing is implemented (Order 1 PR 4).
    pub(crate) changed_packages: Vec<ImpactPackage>,
    /// Packages identified as reverse-dependency-affected.
    /// Empty until reverse-dependency closure is implemented (Order 1 PR 6).
    pub(crate) affected_packages: Vec<ImpactPackage>,
    /// Candidate tests/builds the impact planner WOULD select.
    /// Empty until candidate ranking is implemented (Order 1 PR 7).
    pub(crate) candidate_tasks: Vec<ImpactCandidateTask>,
    /// Evidence gaps: what the impact planner could not determine.
    pub(crate) evidence_gaps: Vec<ImpactEvidenceGap>,
    /// Whether the impact plan is computed from real Cargo metadata or
    /// is a placeholder. "shadow" until promoted to active selection.
    pub(crate) selection_mode: &'static str,
}

/// A package in the workspace graph.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ImpactPackage {
    pub(crate) name: String,
    /// Repository-relative manifest path (e.g., "Cargo.toml" or "subcrate/Cargo.toml").
    pub(crate) manifest_path: String,
    /// Whether this package was changed directly or affected via reverse dependency.
    pub(crate) relation: &'static str,
}

/// A candidate proof task the impact planner would select.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ImpactCandidateTask {
    /// The test target or build command.
    pub(crate) target: String,
    /// Why this candidate is relevant to the diff.
    pub(crate) reason: String,
    /// Owning package of the changed file that triggered this candidate.
    pub(crate) owning_package: String,
    /// Package whose test target this is (may differ from owning_package).
    pub(crate) test_package: String,
    /// Estimated cost: "low" | "medium" | "high".
    pub(crate) estimated_cost: &'static str,
    /// Expected decision value: "high" | "medium" | "low".
    pub(crate) expected_value: &'static str,
}

/// An evidence gap: something the impact planner could not determine.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct ImpactEvidenceGap {
    pub(crate) kind: &'static str,
    pub(crate) detail: String,
}

/// Build the initial shadow impact plan. This is a placeholder that records
/// what we know (changed files) and what we don't (everything else). As
/// Order 1 PRs land, this function gains Cargo metadata parsing, package
/// resolution, reverse-dependency closure, and candidate ranking.
pub(crate) fn build_shadow_impact_plan(changed_files: &[String]) -> ImpactPlan {
    ImpactPlan {
        schema: IMPACT_PLAN_SCHEMA,
        changed_files: changed_files.to_vec(),
        changed_packages: Vec::new(),
        affected_packages: Vec::new(),
        candidate_tasks: Vec::new(),
        evidence_gaps: vec![
            ImpactEvidenceGap {
                kind: "no-cargo-metadata",
                detail: "Cargo workspace/package graph not yet parsed (Order 1 PR 4). \
                         Changed-file → package resolution unavailable."
                    .to_owned(),
            },
            ImpactEvidenceGap {
                kind: "no-reverse-dependency-closure",
                detail: "Reverse-dependency closure not yet computed (Order 1 PR 6). \
                         Affected-package identification unavailable."
                    .to_owned(),
            },
            ImpactEvidenceGap {
                kind: "no-candidate-ranking",
                detail: "Candidate test/build ranking not yet implemented (Order 1 PR 7). \
                         No deterministic impact candidates emitted."
                    .to_owned(),
            },
        ],
        selection_mode: "shadow",
    }
}

/// Write the impact plan as a shadow artifact.
pub(crate) fn write_impact_plan(out: &Path, plan: &ImpactPlan) -> anyhow::Result<()> {
    let path = out.join("review").join("impact_plan.json");
    let parent = path.parent();
    if let Some(dir) = parent {
        std::fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(plan)?;
    std::fs::write(&path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shadow_impact_plan_records_changed_files_and_gaps() {
        let plan = build_shadow_impact_plan(&["src/config.rs".to_owned(), "src/gate.rs".to_owned()]);
        assert_eq!(plan.schema, "ub-review.impact_plan.v1");
        assert_eq!(plan.changed_files.len(), 2);
        assert!(plan.changed_packages.is_empty());
        assert!(plan.affected_packages.is_empty());
        assert!(plan.candidate_tasks.is_empty());
        assert_eq!(plan.evidence_gaps.len(), 3);
        assert_eq!(plan.selection_mode, "shadow");
        assert_eq!(plan.evidence_gaps[0].kind, "no-cargo-metadata");
    }

    #[test]
    fn shadow_impact_plan_handles_empty_diff() {
        let plan = build_shadow_impact_plan(&[]);
        assert!(plan.changed_files.is_empty());
        assert_eq!(plan.evidence_gaps.len(), 3);
    }

    #[test]
    fn impact_plan_schema_is_stable() {
        assert_eq!(IMPACT_PLAN_SCHEMA, "ub-review.impact_plan.v1");
    }
}
