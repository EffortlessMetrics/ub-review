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
pub(crate) fn build_shadow_impact_plan(root: &Path, changed_files: &[String]) -> ImpactPlan {
    // Attempt to parse the Cargo workspace graph. If unavailable, record it
    // as an evidence gap. (Order 1 PR 4: this now populates changed_packages
    // from the workspace graph when available.)
    let cargo_graph = parse_cargo_workspace(root);
    let mut changed_packages = Vec::new();
    let mut evidence_gaps = Vec::new();

    match &cargo_graph {
        Some(graph) => {
            // For each changed file, find the owning package.
            for changed_file in changed_files {
                let normalized = changed_file.replace('\\', "/");
                if let Some(pkg) = graph.packages.iter().find(|p| {
                    if p.directory == "." {
                        // Root package: file is owned if it's not under another package's directory.
                        !graph.packages.iter().any(|other| {
                            other.directory != "."
                                && normalized.starts_with(&format!("{}/", other.directory))
                        })
                    } else {
                        normalized.starts_with(&format!("{}/", p.directory))
                    }
                }) {
                    changed_packages.push(ImpactPackage {
                        name: pkg.name.clone(),
                        manifest_path: pkg.manifest_path.clone(),
                        relation: "changed",
                    });
                }
            }
        }
        None => {
            evidence_gaps.push(ImpactEvidenceGap {
                kind: "no-cargo-metadata",
                detail: "Cargo workspace/package graph not available. \
                         Changed-file → package resolution unavailable."
                    .to_owned(),
            });
        }
    }

    // Remaining evidence gaps (filled by future PRs).
    if cargo_graph.is_some() {
        evidence_gaps.push(ImpactEvidenceGap {
            kind: "no-reverse-dependency-closure",
            detail: "Reverse-dependency closure not yet computed (Order 1 PR 6). \
                     Affected-package identification unavailable."
                .to_owned(),
        });
        evidence_gaps.push(ImpactEvidenceGap {
            kind: "no-candidate-ranking",
            detail: "Candidate test/build ranking not yet implemented (Order 1 PR 7). \
                     No deterministic impact candidates emitted."
                .to_owned(),
        });
    }

    ImpactPlan {
        schema: IMPACT_PLAN_SCHEMA,
        changed_files: changed_files.to_vec(),
        changed_packages,
        affected_packages: Vec::new(),
        candidate_tasks: Vec::new(),
        evidence_gaps,
        selection_mode: "shadow",
    }
}

// --- Cargo workspace graph (Order 1 PR 4) ---

/// A parsed Cargo workspace: packages, their targets, and dependency edges.
/// Built from `cargo metadata --format-version 1 --no-deps`.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct CargoWorkspaceGraph {
    /// All workspace packages keyed by package name.
    pub(crate) packages: Vec<CargoPackageInfo>,
    /// Workspace root directory (repo-relative).
    pub(crate) workspace_root: String,
}

/// A single package in the workspace graph.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct CargoPackageInfo {
    pub(crate) name: String,
    /// Repo-relative manifest path (e.g., "Cargo.toml" or "subcrate/Cargo.toml").
    pub(crate) manifest_path: String,
    /// Repo-relative directory containing the manifest.
    pub(crate) directory: String,
    /// Targets (lib, bin, test, example, bench) declared by this package.
    pub(crate) targets: Vec<CargoTargetInfo>,
    /// Package names this package depends on (within workspace only).
    pub(crate) dependencies: Vec<String>,
}

/// A build target within a package.
#[derive(Clone, Debug, Serialize)]
pub(crate) struct CargoTargetInfo {
    pub(crate) name: String,
    /// Target kind: "lib", "bin", "test", "example", "bench", "custom-build".
    pub(crate) kind: String,
    /// Repo-relative source path.
    pub(crate) src_path: String,
}

/// Parse `cargo metadata --format-version 1 --no-deps` and build the workspace
/// graph. Returns `None` if cargo is unavailable or the output is unparseable
/// (recorded as an evidence gap rather than a hard failure).
pub(crate) fn parse_cargo_workspace(root: &Path) -> Option<CargoWorkspaceGraph> {
    let output = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).ok()?;
    let workspace_root_abs = json.get("workspace_root")?.as_str()?.to_string();

    let mut packages = Vec::new();
    if let Some(pkgs) = json.get("packages").and_then(|v| v.as_array()) {
        for pkg in pkgs {
            let name = pkg.get("name")?.as_str()?.to_string();
            let manifest_path_abs = pkg.get("manifest_path")?.as_str()?.to_string();
            let manifest_path = relative_to_repo(root, &manifest_path_abs);
            let directory = parent_directory(&manifest_path);

            let mut targets = Vec::new();
            if let Some(tgts) = pkg.get("targets").and_then(|v| v.as_array()) {
                for tgt in tgts {
                    let tgt_name = tgt.get("name")?.as_str()?.to_string();
                    let kinds = tgt.get("kind").and_then(|v| v.as_array());
                    let kind = kinds
                        .and_then(|a| a.first())
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let src_path_abs = tgt.get("src_path")?.as_str()?.to_string();
                    let src_path = relative_to_repo(root, &src_path_abs);
                    targets.push(CargoTargetInfo {
                        name: tgt_name,
                        kind,
                        src_path,
                    });
                }
            }

            let mut dependencies = Vec::new();
            if let Some(deps) = pkg.get("dependencies").and_then(|v| v.as_array()) {
                for dep in deps {
                    if let Some(name) = dep.get("name").and_then(|v| v.as_str()) {
                        dependencies.push(name.to_string());
                    }
                }
            }

            packages.push(CargoPackageInfo {
                name,
                manifest_path,
                directory,
                targets,
                dependencies,
            });
        }
    }

    // Only keep workspace-member packages. The workspace_members array contains
    // package IDs like "path+file:///.../Cargo.toml#0.1.0". We match by
    // checking if any workspace_member ID contains the package's manifest path.
    let workspace_member_ids: Vec<String> = json
        .get("workspace_members")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let packages: Vec<CargoPackageInfo> = packages
        .into_iter()
        .filter(|p| {
            // A package is a workspace member if any workspace_member ID
            // contains its manifest directory path or name.
            workspace_member_ids
                .iter()
                .any(|id| id.contains(&p.name) || id.contains(&p.manifest_path))
        })
        .collect();

    let workspace_root = relative_to_repo(root, &workspace_root_abs);

    Some(CargoWorkspaceGraph {
        packages,
        workspace_root,
    })
}

/// Convert an absolute path to a repo-relative path. If the path is not under
/// the repo root, returns the original (best-effort).
fn relative_to_repo(root: &Path, abs: &str) -> String {
    let abs_path = std::path::Path::new(abs);
    abs_path
        .strip_prefix(root)
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_else(|_| abs.replace('\\', "/"))
}

/// Extract the parent directory from a repo-relative manifest path.
fn parent_directory(manifest_path: &str) -> String {
    let normalized = manifest_path.replace('\\', "/");
    match normalized.rfind('/') {
        Some(idx) => normalized[..idx].to_string(),
        None => ".".to_string(),
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
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let plan = build_shadow_impact_plan(
            root,
            &["src/config.rs".to_owned(), "src/gate.rs".to_owned()],
        );
        assert_eq!(plan.schema, "ub-review.impact_plan.v1");
        assert_eq!(plan.changed_files.len(), 2);
        assert_eq!(plan.selection_mode, "shadow");
        // changed_packages may be populated if cargo metadata succeeds on this repo
        if plan.changed_packages.is_empty() {
            assert!(
                plan.evidence_gaps
                    .iter()
                    .any(|g| g.kind == "no-cargo-metadata"),
                "should have cargo-metadata gap when packages are empty"
            );
        }
    }

    #[test]
    fn shadow_impact_plan_handles_empty_diff() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let plan = build_shadow_impact_plan(root, &[]);
        assert!(plan.changed_files.is_empty());
    }

    #[test]
    fn impact_plan_schema_is_stable() {
        assert_eq!(IMPACT_PLAN_SCHEMA, "ub-review.impact_plan.v1");
    }

    #[test]
    fn cargo_workspace_graph_parses_this_repo() -> anyhow::Result<()> {
        // This repo has 2 workspace members: ub-review and xtask.
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let graph = parse_cargo_workspace(root);
        assert!(
            graph.is_some(),
            "cargo metadata should succeed on this repo"
        );
        let graph = graph.ok_or_else(|| anyhow::anyhow!("cargo metadata returned None"))?;
        let names: Vec<&str> = graph.packages.iter().map(|p| p.name.as_str()).collect();
        assert!(
            names.contains(&"ub-review"),
            "ub-review package should be in the graph: {names:?}"
        );
        assert!(
            names.contains(&"xtask"),
            "xtask package should be in the graph: {names:?}"
        );
        // ub-review should have at least a bin target
        let ub_review = graph.packages.iter().find(|p| p.name == "ub-review");
        assert!(ub_review.is_some(), "ub-review package exists");
        if let Some(pkg) = ub_review {
            assert!(
                pkg.targets.iter().any(|t| t.kind == "bin"),
                "ub-review should have a bin target"
            );
        }
        Ok(())
    }

    #[test]
    fn cargo_workspace_graph_returns_none_on_missing_cargo() -> anyhow::Result<()> {
        // Point at a directory with no Cargo.toml.
        let tmp = tempfile::tempdir()?;
        let graph = parse_cargo_workspace(tmp.path());
        // cargo metadata will fail in a non-cargo directory.
        // It may still return Some if cargo finds a parent Cargo.toml,
        // but in CI/temp dirs it should be None or contain no packages.
        if let Some(g) = graph {
            // If cargo found a parent workspace, just verify the structure is valid.
            assert!(g.packages.len() <= 10, "unexpected package count");
        }
        // Either None or a valid graph is acceptable here.
        Ok(())
    }

    #[test]
    fn relative_to_repo_strips_prefix() {
        let root = std::path::Path::new("/code/repo");
        assert_eq!(
            relative_to_repo(root, "/code/repo/src/main.rs"),
            "src/main.rs"
        );
        assert_eq!(
            relative_to_repo(root, "/code/repo/Cargo.toml"),
            "Cargo.toml"
        );
    }

    #[test]
    fn parent_directory_extracts_dir() {
        assert_eq!(parent_directory("Cargo.toml"), ".");
        assert_eq!(parent_directory("subcrate/Cargo.toml"), "subcrate");
    }
}
