//! Deterministic resolution of model-native proof intents.
//!
//! Models name a repository-owned test or package. They do not provide a
//! command. This module is the boundary that turns one exact semantic label
//! into one approved internal request, or records a terminal disposition
//! before the request can enter the broker queue.

use std::collections::BTreeMap;
use std::path::Path;

use anyhow::Result;

use crate::*;

const DEFAULT_TARGET_PREFIX: &str = "cargo-test:";

#[derive(Clone, Debug)]
struct ApprovedTarget {
    package: String,
    target: Option<CargoTargetInfo>,
    command: String,
}

#[derive(Clone, Debug)]
pub(crate) struct ProofIntentResolution {
    pub(crate) intents: Vec<ProofIntent>,
    pub(crate) proof_requests: Vec<ProofRequest>,
}

/// Resolve model intents and return only requests whose command was produced
/// by an approved Cargo template. Invalid, unsupported, ambiguous, or
/// unavailable intents remain in the returned artifact with a terminal status.
pub(crate) fn resolve_model_proof_intents(
    root: &Path,
    diff: &DiffContext,
    intents: &[ProofIntent],
    legacy_requests: &[ProofRequest],
    budget: ProofBudget,
) -> Result<ProofIntentResolution> {
    let graph = parse_cargo_workspace(root);
    let mut requests = Vec::new();
    let mut known_requests = BTreeMap::<String, String>::new();

    for request in legacy_requests {
        known_requests
            .entry(request_key(&request.command, &request.cost))
            .or_insert_with(|| request.id.clone());
    }

    let mut resolved_intents = Vec::with_capacity(intents.len());
    for intent in intents {
        let mut resolved = intent.clone();
        resolved.resolved_request_ids.clear();

        let requested_timeout = intent.timeout_sec.unwrap_or(budget.per_command_timeout_sec);
        if requested_timeout == 0 || requested_timeout > budget.per_command_timeout_sec {
            resolved.status = "invalid_timeout".to_owned();
            resolved.resolution_reason = format!(
                "requested timeout {requested_timeout}s is outside the approved per-command budget of {}s",
                budget.per_command_timeout_sec
            );
            resolved_intents.push(resolved);
            continue;
        }

        if !safe_semantic_target(&intent.target) {
            resolved.status = "rejected_target".to_owned();
            resolved.resolution_reason =
                "target is not a repository-owned semantic label".to_owned();
            resolved_intents.push(resolved);
            continue;
        }

        let Some(graph) = graph.as_ref() else {
            resolved.status = "unavailable_metadata".to_owned();
            resolved.resolution_reason =
                "Cargo metadata was unavailable, so no executable target was approved".to_owned();
            resolved_intents.push(resolved);
            continue;
        };

        let target_result = match intent.proof_kind {
            ProofKind::FocusedTest | ProofKind::BasePlusTests => {
                resolve_test_target(graph, diff, &intent.target)
            }
            ProofKind::FocusedBuild => resolve_build_target(graph, diff, &intent.target),
            _ => Err("unsupported proof kind".to_owned()),
        };

        let target = match target_result {
            Ok(target) => target,
            Err(reason) => {
                resolved.status = if reason == "unsupported proof kind" {
                    "unsupported_kind".to_owned()
                } else if reason.starts_with("ambiguous") {
                    "ambiguous_target".to_owned()
                } else {
                    "unsupported_target".to_owned()
                };
                resolved.resolution_reason = reason;
                resolved_intents.push(resolved);
                continue;
            }
        };

        let cost = match intent.proof_kind {
            ProofKind::FocusedTest | ProofKind::BasePlusTests => "focused-test",
            ProofKind::FocusedBuild => "focused-build",
            _ => {
                resolved.status = "unsupported_kind".to_owned();
                resolved.resolution_reason = "unsupported proof kind".to_owned();
                resolved_intents.push(resolved);
                continue;
            }
        };
        let key = request_key(&target.command, cost);
        let request_id = if let Some(existing) = known_requests.get(&key) {
            resolved.status = "deduplicated".to_owned();
            resolved.resolution_reason = format!(
                "equivalent approved request already owns target `{}`",
                target.command
            );
            existing.clone()
        } else {
            let digest = sha256_hex(format!("{cost}\n{key}").as_bytes());
            let id = format!("proof-intent-{}", &digest[..16]);
            let request = ProofRequest {
                schema: crate::artifacts::PROOF_REQUEST_SCHEMA.to_owned(),
                id: id.clone(),
                lane: intent
                    .requested_by
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "proof-planner".to_owned()),
                requested_by: intent.requested_by.clone(),
                command: target.command.clone(),
                reason: format!(
                    "{} Expected answer: {}",
                    intent.question, intent.expected_answer_shape
                ),
                cost: cost.to_owned(),
                timeout_sec: requested_timeout,
                required: intent.estimated_value == "high",
                status: "requested".to_owned(),
            };
            known_requests.insert(key, id.clone());
            requests.push(request);
            resolved.status = "resolved".to_owned();
            resolved.resolution_reason = format!(
                "resolved to one approved `{}` target `{}`",
                intent.proof_kind.key(),
                target.command
            );
            id
        };
        resolved.resolved_request_ids.push(request_id);
        resolved_intents.push(resolved);
    }

    Ok(ProofIntentResolution {
        intents: resolved_intents,
        proof_requests: requests,
    })
}

fn request_key(command: &str, cost: &str) -> String {
    format!(
        "{cost}:{}",
        normalize_proof_command(command)
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    )
}

fn safe_semantic_target(target: &str) -> bool {
    !target.is_empty()
        && !target.starts_with('/')
        && !target.starts_with('-')
        && !target.contains("..")
        && !target.contains('\\')
        && !target.contains(":/")
        && !has_shell_control_token(target)
        && !target.chars().any(char::is_whitespace)
        && target.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '-' | '.' | ':' | '/')
        })
}

fn resolve_test_target(
    graph: &CargoWorkspaceGraph,
    diff: &DiffContext,
    label: &str,
) -> std::result::Result<ApprovedTarget, String> {
    let label = label.strip_prefix(DEFAULT_TARGET_PREFIX).unwrap_or(label);
    let package_filter = label
        .strip_prefix("cargo-package:")
        .or_else(|| label.strip_prefix("package:"));
    let target_label = label.strip_prefix("test:").unwrap_or(label);

    let mut candidates = graph
        .packages
        .iter()
        .flat_map(|package| {
            package
                .targets
                .iter()
                .filter(|target| target.kind == "test")
                .map(move |target| (package, target))
        })
        .filter(|(package, target)| {
            if let Some(package_filter) = package_filter {
                return package.name == package_filter;
            }
            if target_label == target.src_path {
                return true;
            }
            if target_label == target.name {
                return true;
            }
            if let Some((package_name, test_name)) = split_target_label(target_label) {
                return package.name == package_name && target.name == test_name;
            }
            package_owns_changed_path(package, diff, target_label)
        })
        .map(|(package, target)| ApprovedTarget {
            package: package.name.clone(),
            target: Some(target.clone()),
            command: format!(
                "cargo test --locked --package {} --test {}",
                package.name, target.name
            ),
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        (left.package.as_str(), left.target_name())
            .cmp(&(right.package.as_str(), right.target_name()))
    });
    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => Err(format!("no approved focused test target matches `{label}`")),
        count => Err(format!(
            "ambiguous focused test target `{label}` matched {count} approved targets"
        )),
    }
}

fn resolve_build_target(
    graph: &CargoWorkspaceGraph,
    diff: &DiffContext,
    label: &str,
) -> std::result::Result<ApprovedTarget, String> {
    if label == "workspace" || label == "cargo-workspace" {
        return Ok(ApprovedTarget {
            package: graph.workspace_root.clone(),
            target: None,
            command: "cargo check --locked --workspace".to_owned(),
        });
    }
    let target_label = label
        .strip_prefix("cargo-package:")
        .or_else(|| label.strip_prefix("package:"))
        .unwrap_or(label);
    let target_selector = label.strip_prefix("cargo-target:");
    let mut candidates = graph
        .packages
        .iter()
        .filter(|package| {
            if let Some(selector) = target_selector {
                return split_target_label(selector)
                    .is_some_and(|(package_name, _)| package_name == package.name);
            }
            package.name == target_label || package_owns_changed_path(package, diff, label)
        })
        .flat_map(|package| {
            if let Some(selector) = target_selector {
                let (_, target_name) = split_target_label(selector)?;
                let target = package
                    .targets
                    .iter()
                    .find(|target| target.name == target_name)?;
                return Some(ApprovedTarget {
                    package: package.name.clone(),
                    target: Some(target.clone()),
                    command: build_command(package, Some(target)),
                });
            }
            Some(ApprovedTarget {
                package: package.name.clone(),
                target: None,
                command: build_command(package, None),
            })
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        (left.package.as_str(), left.target_name())
            .cmp(&(right.package.as_str(), right.target_name()))
    });
    match candidates.len() {
        1 => Ok(candidates.remove(0)),
        0 => Err(format!(
            "no approved focused build target matches `{label}`"
        )),
        count => Err(format!(
            "ambiguous focused build target `{label}` matched {count} approved targets"
        )),
    }
}

fn build_command(package: &CargoPackageInfo, target: Option<&CargoTargetInfo>) -> String {
    let mut command = format!("cargo check --locked --package {}", package.name);
    if let Some(target) = target {
        let flag = match target.kind.as_str() {
            "lib" => "--lib",
            "bin" => "--bin",
            "test" => "--test",
            "example" => "--example",
            "bench" => "--bench",
            _ => return command,
        };
        command.push_str(&format!(" {flag} {}", target.name));
    }
    command
}

fn split_target_label(label: &str) -> Option<(&str, &str)> {
    label
        .split_once("::")
        .or_else(|| label.split_once('/'))
        .filter(|(package, target)| !package.is_empty() && !target.is_empty())
}

fn package_owns_changed_path(package: &CargoPackageInfo, diff: &DiffContext, label: &str) -> bool {
    let label = normalize_repo_path(label);
    diff.changed_files.iter().any(|file| {
        let file = normalize_repo_path(file);
        file == label
            && (package.directory == "." || file.starts_with(&format!("{}/", package.directory)))
    })
}

impl ApprovedTarget {
    fn target_name(&self) -> &str {
        self.target
            .as_ref()
            .map(|target| target.name.as_str())
            .unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intent(kind: ProofKind, target: &str) -> ProofIntent {
        ProofIntent {
            id: format!("intent-{target}"),
            claim_id: "claim-proof".to_owned(),
            question: "does the approved proof answer the claim?".to_owned(),
            expected_answer_shape: "terminal receipt".to_owned(),
            proof_kind: kind,
            target: target.to_owned(),
            estimated_value: "high".to_owned(),
            requested_by: vec!["tests-oracle".to_owned()],
            status: "requested".to_owned(),
            timeout_sec: None,
            resolved_request_ids: Vec::new(),
            resolution_reason: String::new(),
        }
    }

    #[test]
    fn resolves_one_cargo_test_label_to_one_approved_request() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let diff = DiffContext {
            base: "base".to_owned(),
            head: "head".to_owned(),
            patch: String::new(),
            changed_files: vec!["src/main.rs".to_owned()],
            diff_class: DiffClass::SourceGeneral,
            flags: DiffFlags::default(),
        };
        let result = resolve_model_proof_intents(
            root,
            &diff,
            &[intent(ProofKind::FocusedTest, "cli")],
            &[],
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 3,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
        )?;
        assert_eq!(result.proof_requests.len(), 1);
        assert_eq!(
            result.proof_requests[0].command,
            "cargo test --locked --package ub-review --test cli"
        );
        assert_eq!(result.intents[0].status, "resolved");
        assert_eq!(result.intents[0].resolved_request_ids.len(), 1);
        Ok(())
    }

    #[test]
    fn build_and_base_plus_tests_use_only_approved_templates() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let diff = DiffContext {
            base: "base".to_owned(),
            head: "head".to_owned(),
            patch: String::new(),
            changed_files: vec!["src/main.rs".to_owned()],
            diff_class: DiffClass::SourceGeneral,
            flags: DiffFlags::default(),
        };
        let intents = [
            intent(ProofKind::BasePlusTests, "ub-review::cli"),
            intent(ProofKind::FocusedBuild, "ub-review"),
        ];
        let result = resolve_model_proof_intents(
            root,
            &diff,
            &intents,
            &[],
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 3,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
        )?;
        assert_eq!(result.proof_requests.len(), 2);
        assert!(
            result
                .proof_requests
                .iter()
                .any(|request| request.cost == "focused-test")
        );
        assert!(
            result
                .proof_requests
                .iter()
                .any(|request| request.cost == "focused-build")
        );
        assert!(
            result
                .proof_requests
                .iter()
                .all(|request| request.command.starts_with("cargo "))
        );
        let test_tasks = focused_test_candidates_from_requests(&result.proof_requests);
        let build_tasks = focused_build_candidates_from_requests(&result.proof_requests);
        assert_eq!(test_tasks.len(), 1);
        assert_eq!(test_tasks[0].mode, FocusedProofMode::RedGreen);
        assert_eq!(build_tasks.len(), 1);
        Ok(())
    }

    #[test]
    fn terminalizes_ambiguous_unsupported_and_malicious_intents() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let diff = DiffContext {
            base: "base".to_owned(),
            head: "head".to_owned(),
            patch: String::new(),
            changed_files: vec![],
            diff_class: DiffClass::SourceGeneral,
            flags: DiffFlags::default(),
        };
        let mut oversized = intent(ProofKind::FocusedTest, "cli");
        oversized.timeout_sec = Some(301);
        let malicious_targets = [
            "../../bin",
            "--package",
            "cargo;rm",
            "cargo|rm",
            "cargo>out",
            "$(cargo)",
            "cargo-target:ub-review",
        ];
        for target in malicious_targets {
            let result = resolve_model_proof_intents(
                root,
                &diff,
                &[intent(ProofKind::FocusedTest, target)],
                &[],
                ProofBudget {
                    max_focused_test_files: 3,
                    max_focused_tests: 3,
                    per_command_timeout_sec: 300,
                    max_total_seconds: 600,
                },
            )?;
            assert!(result.proof_requests.is_empty(), "target {target} ran");
            assert!(!matches!(
                result.intents[0].status.as_str(),
                "resolved" | "deduplicated"
            ));
        }
        let result = resolve_model_proof_intents(
            root,
            &diff,
            &[
                intent(ProofKind::FocusedTest, "cargo-package:ub-review"),
                intent(ProofKind::MutationWitness, "cli"),
                intent(ProofKind::FocusedTest, "../../bin"),
                oversized,
            ],
            &[],
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 3,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
        )?;
        assert_eq!(result.proof_requests.len(), 0);
        assert_eq!(result.intents[0].status, "ambiguous_target");
        assert_eq!(result.intents[1].status, "unsupported_kind");
        assert_eq!(result.intents[2].status, "rejected_target");
        assert_eq!(result.intents[3].status, "invalid_timeout");
        Ok(())
    }

    #[test]
    fn equivalent_intents_and_existing_requests_share_one_request_identity() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let diff = DiffContext {
            base: "base".to_owned(),
            head: "head".to_owned(),
            patch: String::new(),
            changed_files: vec![],
            diff_class: DiffClass::SourceGeneral,
            flags: DiffFlags::default(),
        };
        let existing = ProofRequest {
            schema: crate::artifacts::PROOF_REQUEST_SCHEMA.to_owned(),
            id: "legacy-request".to_owned(),
            lane: "legacy".to_owned(),
            requested_by: vec!["legacy".to_owned()],
            command: "cargo test --locked --package ub-review --test cli".to_owned(),
            reason: "legacy equivalent".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "requested".to_owned(),
        };
        let mut second = intent(ProofKind::FocusedTest, "cli");
        second.id = "intent-second".to_owned();
        let result = resolve_model_proof_intents(
            root,
            &diff,
            &[intent(ProofKind::FocusedTest, "cli"), second],
            &[existing],
            ProofBudget {
                max_focused_test_files: 3,
                max_focused_tests: 3,
                per_command_timeout_sec: 300,
                max_total_seconds: 600,
            },
        )?;
        assert!(result.proof_requests.is_empty());
        assert!(
            result
                .intents
                .iter()
                .all(|item| item.status == "deduplicated")
        );
        assert_eq!(
            result.intents[0].resolved_request_ids,
            vec!["legacy-request"]
        );
        Ok(())
    }
}
