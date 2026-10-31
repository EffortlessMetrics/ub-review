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
const UNSUPPORTED_KIND_REASON: &str = "unsupported proof kind; supported kinds: focused-test, focused-build, base-plus-tests. Choose one only if it answers the proof question; otherwise leave the proof unresolved.";

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
            .entry(request_key(
                &request.command,
                &request.cost,
                request.timeout_sec,
                request.required,
            ))
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
            _ => Err(UNSUPPORTED_KIND_REASON.to_owned()),
        };

        let target = match target_result {
            Ok(target) => target,
            Err(reason) => {
                resolved.status = if reason == UNSUPPORTED_KIND_REASON {
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
                resolved.resolution_reason = UNSUPPORTED_KIND_REASON.to_owned();
                resolved_intents.push(resolved);
                continue;
            }
        };
        let required = intent.estimated_value == "high";
        let key = request_key(&target.command, cost, requested_timeout, required);
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
                required,
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

fn request_key(command: &str, cost: &str, timeout_sec: u64, required: bool) -> String {
    format!(
        "{cost}:{timeout_sec}:{required}:{}",
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
        0 => Err(format!(
            "no approved focused test target matches `{label}`; choose a declared integration test using `cargo-test:<package>::<test>`"
        )),
        count => Err(format!(
            "ambiguous focused test target `{label}` matched {count} approved targets; choose one declared integration test using `cargo-test:<package>::<test>`"
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
        1 => {
            let candidate = candidates.remove(0);
            if let Some(target) = &candidate.target {
                return Err(format!(
                    "named focused build target `{label}` of kind `{}` is unsupported by the proof broker; choose `cargo-package:{}` only if package-level checking answers the proof question; otherwise leave the proof unresolved",
                    target.kind, candidate.package
                ));
            }
            Ok(candidate)
        }
        0 => Err(format!(
            "no approved focused build target matches `{label}`; choose a declared package using `cargo-package:<package>`"
        )),
        count => Err(format!(
            "ambiguous focused build target `{label}` matched {count} approved targets; choose a declared package using `cargo-package:<package>`"
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
        command.push_str(&format!(" {flag}"));
        if target.kind != "lib" {
            command.push_str(&format!(" {}", target.name));
        }
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

    fn graph() -> CargoWorkspaceGraph {
        CargoWorkspaceGraph {
            workspace_root: ".".to_owned(),
            packages: vec![CargoPackageInfo {
                name: "ub-review".to_owned(),
                manifest_path: "Cargo.toml".to_owned(),
                directory: ".".to_owned(),
                targets: vec![
                    CargoTargetInfo {
                        name: "ub-review".to_owned(),
                        kind: "bin".to_owned(),
                        src_path: "src/main.rs".to_owned(),
                    },
                    CargoTargetInfo {
                        name: "cli".to_owned(),
                        kind: "test".to_owned(),
                        src_path: "tests/cli.rs".to_owned(),
                    },
                    CargoTargetInfo {
                        name: "cli_contract".to_owned(),
                        kind: "test".to_owned(),
                        src_path: "tests/cli_contract.rs".to_owned(),
                    },
                ],
                dependencies: Vec::new(),
            }],
        }
    }

    fn diff(changed_files: &[&str]) -> DiffContext {
        DiffContext {
            base: "base".to_owned(),
            head: "head".to_owned(),
            patch: String::new(),
            changed_files: changed_files
                .iter()
                .map(|file| (*file).to_owned())
                .collect(),
            diff_class: DiffClass::SourceGeneral,
            flags: DiffFlags::default(),
        }
    }

    fn budget() -> ProofBudget {
        ProofBudget {
            max_focused_test_files: 3,
            max_focused_tests: 3,
            per_command_timeout_sec: 300,
            max_total_seconds: 600,
        }
    }

    fn expected_error<T: std::fmt::Debug>(
        result: std::result::Result<T, String>,
    ) -> Result<String> {
        match result {
            Ok(value) => anyhow::bail!("expected resolver error, got {value:?}"),
            Err(error) => Ok(error),
        }
    }

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
        let result = resolve_model_proof_intents(
            root,
            &diff(&["src/main.rs"]),
            &[intent(ProofKind::FocusedTest, "cli")],
            &[],
            budget(),
        )?;
        assert_eq!(result.proof_requests.len(), 1);
        assert_eq!(
            result.proof_requests[0].command,
            "cargo test --locked --package ub-review --test cli"
        );
        assert_eq!(result.intents[0].status, "resolved");
        assert_eq!(result.intents[0].resolved_request_ids.len(), 1);
        assert_eq!(
            result.intents[0].resolved_request_ids[0],
            result.proof_requests[0].id
        );
        assert_eq!(result.proof_requests[0].timeout_sec, 300);
        assert!(
            result.intents[0]
                .resolution_reason
                .contains("approved `focused-test` target")
        );
        Ok(())
    }

    #[test]
    fn build_and_base_plus_tests_use_only_approved_templates() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let intents = [
            intent(ProofKind::BasePlusTests, "ub-review::cli"),
            intent(ProofKind::FocusedBuild, "ub-review"),
        ];
        let result =
            resolve_model_proof_intents(root, &diff(&["src/main.rs"]), &intents, &[], budget())?;
        assert_eq!(result.proof_requests.len(), 2);
        assert_eq!(result.proof_requests[0].cost, "focused-test");
        assert_eq!(
            result.proof_requests[0].command,
            "cargo test --locked --package ub-review --test cli"
        );
        assert_eq!(result.proof_requests[1].cost, "focused-build");
        assert_eq!(
            result.proof_requests[1].command,
            "cargo check --locked --package ub-review"
        );
        assert_eq!(result.intents[0].status, "resolved");
        assert_eq!(result.intents[1].status, "resolved");
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
        let mut oversized = intent(ProofKind::FocusedTest, "cli");
        oversized.timeout_sec = Some(301);
        let malicious_targets = [
            ("../../bin", "rejected_target"),
            ("--package", "rejected_target"),
            ("cargo;rm", "rejected_target"),
            ("cargo|rm", "rejected_target"),
            ("cargo>out", "rejected_target"),
            ("$(cargo)", "rejected_target"),
            ("cargo-target:ub-review", "unsupported_target"),
        ];
        for (target, expected_status) in malicious_targets {
            let result = resolve_model_proof_intents(
                root,
                &diff(&[]),
                &[intent(ProofKind::FocusedTest, target)],
                &[],
                budget(),
            )?;
            assert!(result.proof_requests.is_empty(), "target {target} ran");
            assert_eq!(result.intents[0].status, expected_status);
            if expected_status == "rejected_target" {
                assert_eq!(
                    result.intents[0].resolution_reason,
                    "target is not a repository-owned semantic label"
                );
            } else {
                assert!(
                    result.intents[0]
                        .resolution_reason
                        .contains("no approved focused test target")
                );
            }
        }
        let result = resolve_model_proof_intents(
            root,
            &diff(&[]),
            &[
                intent(ProofKind::FocusedTest, "cargo-package:ub-review"),
                intent(ProofKind::MutationWitness, "cli"),
                intent(ProofKind::FocusedTest, "../../bin"),
                oversized,
            ],
            &[],
            budget(),
        )?;
        assert_eq!(result.proof_requests.len(), 0);
        assert_eq!(result.intents[0].status, "ambiguous_target");
        assert!(
            result.intents[0]
                .resolution_reason
                .contains("ambiguous focused test target")
        );
        assert_eq!(result.intents[1].status, "unsupported_kind");
        assert_eq!(
            result.intents[1].resolution_reason,
            "unsupported proof kind; supported kinds: focused-test, focused-build, base-plus-tests. Choose one only if it answers the proof question; otherwise leave the proof unresolved."
        );
        assert_eq!(result.intents[2].status, "rejected_target");
        assert_eq!(result.intents[3].status, "invalid_timeout");
        assert!(
            result.intents[3]
                .resolution_reason
                .contains("outside the approved per-command budget")
        );
        Ok(())
    }

    #[test]
    fn equivalent_intents_and_existing_requests_share_one_request_identity() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let existing = ProofRequest {
            schema: crate::artifacts::PROOF_REQUEST_SCHEMA.to_owned(),
            id: "legacy-request".to_owned(),
            lane: "legacy".to_owned(),
            requested_by: vec!["legacy".to_owned()],
            command: "cargo test --locked --package ub-review --test cli".to_owned(),
            reason: "legacy equivalent".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: true,
            status: "requested".to_owned(),
        };
        let mut second = intent(ProofKind::FocusedTest, "cli");
        second.id = "intent-second".to_owned();
        let result = resolve_model_proof_intents(
            root,
            &diff(&[]),
            &[intent(ProofKind::FocusedTest, "cli"), second],
            &[existing],
            budget(),
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
        assert_eq!(
            result.intents[1].resolved_request_ids,
            vec!["legacy-request"]
        );
        assert!(result.intents.iter().all(|item| {
            item.resolution_reason
                .contains("equivalent approved request")
        }));
        Ok(())
    }

    #[test]
    fn legacy_request_with_different_requiredness_keeps_distinct_identity() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let existing = ProofRequest {
            schema: crate::artifacts::PROOF_REQUEST_SCHEMA.to_owned(),
            id: "legacy-optional-request".to_owned(),
            lane: "legacy".to_owned(),
            requested_by: vec!["legacy".to_owned()],
            command: "cargo test --locked --package ub-review --test cli".to_owned(),
            reason: "legacy optional equivalent".to_owned(),
            cost: "focused-test".to_owned(),
            timeout_sec: 300,
            required: false,
            status: "requested".to_owned(),
        };
        let result = resolve_model_proof_intents(
            root,
            &diff(&[]),
            &[intent(ProofKind::FocusedTest, "cli")],
            &[existing],
            budget(),
        )?;
        assert_eq!(result.proof_requests.len(), 1);
        assert!(result.proof_requests[0].required);
        assert_ne!(result.proof_requests[0].id, "legacy-optional-request");
        assert_eq!(result.intents[0].status, "resolved");
        Ok(())
    }

    #[test]
    fn semantic_target_grammar_and_target_selectors_are_exact() -> Result<()> {
        assert!(safe_semantic_target("cli"));
        assert!(safe_semantic_target("cargo-test:cli"));
        assert!(safe_semantic_target("ub-review::cli"));
        assert!(!safe_semantic_target(""));
        assert!(!safe_semantic_target("/absolute"));
        assert!(!safe_semantic_target("--package"));
        assert!(!safe_semantic_target("../cli"));
        assert!(!safe_semantic_target("C:/repo"));
        assert!(!safe_semantic_target("cargo|test"));
        assert!(!safe_semantic_target("cargo test"));
        assert_eq!(
            split_target_label("ub-review::cli"),
            Some(("ub-review", "cli"))
        );
        assert_eq!(
            split_target_label("ub-review/cli"),
            Some(("ub-review", "cli"))
        );
        assert_eq!(split_target_label("cli"), None);

        let workspace = graph();
        let test_cases = [
            ("cli", "cargo test --locked --package ub-review --test cli"),
            (
                "cargo-test:cli",
                "cargo test --locked --package ub-review --test cli",
            ),
            (
                "test:cli",
                "cargo test --locked --package ub-review --test cli",
            ),
            (
                "ub-review::cli",
                "cargo test --locked --package ub-review --test cli",
            ),
            (
                "ub-review/cli",
                "cargo test --locked --package ub-review --test cli",
            ),
        ];
        for (label, command) in test_cases {
            let target =
                resolve_test_target(&workspace, &diff(&[]), label).map_err(anyhow::Error::msg)?;
            assert_eq!(target.command, command);
            assert_eq!(target.package, "ub-review");
            assert_eq!(target.target_name(), "cli");
        }
        let source_path = resolve_test_target(&workspace, &diff(&[]), "tests/cli.rs")
            .map_err(anyhow::Error::msg)?;
        assert_eq!(source_path.target_name(), "cli");
        assert_eq!(source_path.command, test_cases[0].1);
        assert_eq!(
            expected_error(resolve_test_target(
                &workspace,
                &diff(&[]),
                "no-such-target"
            ))?,
            "no approved focused test target matches `no-such-target`; choose a declared integration test using `cargo-test:<package>::<test>`"
        );
        assert_eq!(
            expected_error(resolve_test_target(
                &workspace,
                &diff(&[]),
                "cargo-package:ub-review"
            ))?,
            "ambiguous focused test target `cargo-package:ub-review` matched 2 approved targets; choose one declared integration test using `cargo-test:<package>::<test>`"
        );
        Ok(())
    }

    #[test]
    fn library_build_template_preserves_named_target_refusal() -> Result<()> {
        let mut workspace = graph();
        let library = CargoTargetInfo {
            name: "library".to_owned(),
            kind: "lib".to_owned(),
            src_path: "src/lib.rs".to_owned(),
        };
        workspace.packages[0].targets.push(library.clone());
        let command = build_command(&workspace.packages[0], Some(&library));
        assert_eq!(command, "cargo check --locked --package ub-review --lib");
        let spec = focused_build_command_spec(&command)
            .ok_or_else(|| anyhow::anyhow!("library template refused by broker"))?;
        assert_eq!(
            spec.argv,
            [
                "cargo",
                "check",
                "--locked",
                "--package",
                "ub-review",
                "--lib"
            ]
        );
        assert!(
            focused_build_command_spec("cargo check --locked --package ub-review --lib library")
                .is_none()
        );

        // Valid Cargo syntax does not authorize a named model intent.
        for separator in ["::", "/"] {
            let label = format!("cargo-target:ub-review{separator}library");
            assert_eq!(
                expected_error(resolve_build_target(&workspace, &diff(&[]), &label))?,
                format!(
                    "named focused build target `{label}` of kind `lib` is unsupported by the proof broker; choose `cargo-package:ub-review` only if package-level checking answers the proof question; otherwise leave the proof unresolved"
                )
            );
        }
        let package = resolve_build_target(&workspace, &diff(&[]), "cargo-package:ub-review")
            .map_err(anyhow::Error::msg)?;
        assert!(package.target.is_none());
        assert_eq!(package.command, "cargo check --locked --package ub-review");
        assert!(focused_build_command_spec(&package.command).is_some());

        for (kind, flag) in [
            ("bin", "--bin"),
            ("test", "--test"),
            ("example", "--example"),
            ("bench", "--bench"),
        ] {
            let target = CargoTargetInfo {
                name: "named".to_owned(),
                kind: kind.to_owned(),
                src_path: "fixture.rs".to_owned(),
            };
            assert_eq!(
                build_command(&workspace.packages[0], Some(&target)),
                format!("cargo check --locked --package ub-review {flag} named")
            );
        }
        Ok(())
    }

    #[test]
    fn build_resolution_and_command_templates_are_exact() -> Result<()> {
        let workspace = graph();
        let workspace_target = resolve_build_target(&workspace, &diff(&[]), "workspace")
            .map_err(anyhow::Error::msg)?;
        assert_eq!(workspace_target.command, "cargo check --locked --workspace");
        assert!(workspace_target.target.is_none());

        let package_target = resolve_build_target(&workspace, &diff(&[]), "package:ub-review")
            .map_err(anyhow::Error::msg)?;
        assert_eq!(
            package_target.command,
            "cargo check --locked --package ub-review"
        );
        assert!(package_target.target.is_none());

        assert_eq!(
            expected_error(resolve_build_target(
                &workspace,
                &diff(&[]),
                "cargo-target:ub-review/ub-review"
            ))?,
            "named focused build target `cargo-target:ub-review/ub-review` of kind `bin` is unsupported by the proof broker; choose `cargo-package:ub-review` only if package-level checking answers the proof question; otherwise leave the proof unresolved"
        );

        let package = &workspace.packages[0];
        let lib = CargoTargetInfo {
            name: "library".to_owned(),
            kind: "lib".to_owned(),
            src_path: "src/lib.rs".to_owned(),
        };
        assert_eq!(
            build_command(package, Some(&lib)),
            "cargo check --locked --package ub-review --lib"
        );
        let example = CargoTargetInfo {
            name: "demo".to_owned(),
            kind: "example".to_owned(),
            src_path: "examples/demo.rs".to_owned(),
        };
        assert_eq!(
            build_command(package, Some(&example)),
            "cargo check --locked --package ub-review --example demo"
        );
        let unknown = CargoTargetInfo {
            name: "custom".to_owned(),
            kind: "custom-build".to_owned(),
            src_path: "build.rs".to_owned(),
        };
        assert_eq!(
            build_command(package, Some(&unknown)),
            "cargo check --locked --package ub-review"
        );
        assert_eq!(
            expected_error(resolve_build_target(
                &workspace,
                &diff(&[]),
                "no-such-package"
            ))?,
            "no approved focused build target matches `no-such-package`; choose a declared package using `cargo-package:<package>`"
        );
        assert_eq!(
            expected_error(resolve_build_target(
                &workspace,
                &diff(&[]),
                "cargo-target:ub-review/no-such-target"
            ))?,
            "no approved focused build target matches `cargo-target:ub-review/no-such-target`; choose a declared package using `cargo-package:<package>`"
        );
        assert_eq!(
            expected_error(resolve_test_target(
                &workspace,
                &diff(&[]),
                "no-such-test-target"
            ))?,
            "no approved focused test target matches `no-such-test-target`; choose a declared integration test using `cargo-test:<package>::<test>`"
        );
        assert!(
            expected_error(resolve_test_target(
                &workspace,
                &diff(&[]),
                "cargo-package:ub-review"
            ))?
            .contains("ambiguous focused test target")
        );
        let direct_test_error = resolve_test_target(&workspace, &diff(&[]), "no-such-test-target")
            .err()
            .ok_or_else(|| anyhow::anyhow!("missing focused-test rejection"))?;
        assert_eq!(
            direct_test_error,
            "no approved focused test target matches `no-such-test-target`; choose a declared integration test using `cargo-test:<package>::<test>`"
        );
        let direct_build_error = resolve_build_target(&workspace, &diff(&[]), "no-such-package")
            .err()
            .ok_or_else(|| anyhow::anyhow!("missing focused-build rejection"))?;
        assert_eq!(
            direct_build_error,
            "no approved focused build target matches `no-such-package`; choose a declared package using `cargo-package:<package>`"
        );
        assert_eq!(
            request_key(
                "cargo test --locked --package ub-review --test cli",
                "focused-test",
                300,
                true,
            ),
            "focused-test:300:true:cargo test --locked --package ub-review --test cli"
        );
        Ok(())
    }

    #[test]
    fn execution_requirements_are_part_of_request_identity() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut defaulted = intent(ProofKind::FocusedTest, "cli");
        defaulted.timeout_sec = Some(300);
        defaulted.estimated_value = "medium".to_owned();
        let mut longer = defaulted.clone();
        longer.id = "intent-longer".to_owned();
        longer.timeout_sec = Some(600);
        let mut required = defaulted.clone();
        required.id = "intent-required".to_owned();
        required.estimated_value = "high".to_owned();

        let mut test_budget = budget();
        test_budget.per_command_timeout_sec = 900;
        let result = resolve_model_proof_intents(
            root,
            &diff(&[]),
            &[defaulted, longer, required],
            &[],
            test_budget,
        )?;
        assert_eq!(result.proof_requests.len(), 3);
        assert_eq!(
            result
                .proof_requests
                .iter()
                .map(|request| (request.timeout_sec, request.required))
                .collect::<Vec<_>>(),
            vec![(300, false), (600, false), (300, true)]
        );
        assert!(result.intents.iter().all(|item| item.status == "resolved"));
        assert_eq!(
            result
                .intents
                .iter()
                .map(|item| item.resolved_request_ids[0].clone())
                .collect::<Vec<_>>()
                .len(),
            3
        );
        Ok(())
    }

    #[test]
    fn resolver_rejects_each_terminal_condition_with_exact_reason() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let cases = [
            (
                ProofKind::FocusedTest,
                "no-such-target",
                "unsupported_target",
            ),
            (
                ProofKind::FocusedTest,
                "cargo-package:ub-review",
                "ambiguous_target",
            ),
            (ProofKind::MutationWitness, "cli", "unsupported_kind"),
        ];
        for (kind, target, status) in cases {
            let result = resolve_model_proof_intents(
                root,
                &diff(&[]),
                &[intent(kind, target)],
                &[],
                budget(),
            )?;
            assert_eq!(result.proof_requests.len(), 0);
            assert_eq!(result.intents[0].status, status);
            assert!(!result.intents[0].resolution_reason.is_empty());
            assert!(result.intents[0].resolved_request_ids.is_empty());
        }

        let mut zero = intent(ProofKind::FocusedTest, "cli");
        zero.timeout_sec = Some(0);
        let result = resolve_model_proof_intents(root, &diff(&[]), &[zero], &[], budget())?;
        assert_eq!(result.intents[0].status, "invalid_timeout");
        assert_eq!(result.intents[0].resolved_request_ids, Vec::<String>::new());

        let unavailable = resolve_model_proof_intents(
            Path::new("C:\\path\\that\\does\\not\\exist"),
            &diff(&[]),
            &[intent(ProofKind::FocusedTest, "cli")],
            &[],
            budget(),
        )?;
        assert_eq!(unavailable.proof_requests.len(), 0);
        assert_eq!(unavailable.intents[0].status, "unavailable_metadata");
        assert_eq!(
            unavailable.intents[0].resolution_reason,
            "Cargo metadata was unavailable, so no executable target was approved"
        );

        let mut defaulted = intent(ProofKind::FocusedBuild, "workspace");
        defaulted.requested_by.clear();
        defaulted.timeout_sec = None;
        let defaulted_result =
            resolve_model_proof_intents(root, &diff(&[]), &[defaulted], &[], budget())?;
        assert_eq!(defaulted_result.proof_requests.len(), 1);
        assert_eq!(defaulted_result.proof_requests[0].lane, "proof-planner");
        assert_eq!(defaulted_result.proof_requests[0].timeout_sec, 300);
        assert_eq!(defaulted_result.intents[0].status, "resolved");
        Ok(())
    }

    #[test]
    fn recovery_guidance_preserves_literal_rejection_and_resolution_controls() -> Result<()> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for kind in [
            ProofKind::SanitizerWitness,
            ProofKind::MutationWitness,
            ProofKind::MiriWitness,
            ProofKind::SourceRouteProbe,
            ProofKind::ExternalCheck,
        ] {
            let mut request = intent(kind, "cli");
            request
                .resolved_request_ids
                .push("stale-request".to_owned());
            let result = resolve_model_proof_intents(root, &diff(&[]), &[request], &[], budget())?;
            assert!(result.proof_requests.is_empty());
            assert_eq!(result.intents[0].status, "unsupported_kind");
            assert!(result.intents[0].resolved_request_ids.is_empty());
            assert_eq!(
                result.intents[0].resolution_reason,
                "unsupported proof kind; supported kinds: focused-test, focused-build, base-plus-tests. Choose one only if it answers the proof question; otherwise leave the proof unresolved."
            );
        }
        for (target, status) in [
            ("absent-package::absent-test", "unsupported_target"),
            ("module::unit_test_function", "unsupported_target"),
            ("cargo-package:ub-review", "ambiguous_target"),
            ("--test", "rejected_target"),
            ("../../tests/cli.rs", "rejected_target"),
            ("cli;cargo", "rejected_target"),
            ("$(cargo)", "rejected_target"),
        ] {
            let result = resolve_model_proof_intents(
                root,
                &diff(&[]),
                &[intent(ProofKind::FocusedTest, target)],
                &[],
                budget(),
            )?;
            assert!(result.proof_requests.is_empty(), "{target}");
            assert_eq!(result.intents[0].status, status, "{target}");
            assert!(
                result.intents[0].resolved_request_ids.is_empty(),
                "{target}"
            );
            if status != "rejected_target" {
                assert!(
                    result.intents[0]
                        .resolution_reason
                        .contains("declared integration test using `cargo-test:<package>::<test>`")
                );
            }
        }
        for timeout in [0, 301, 600] {
            let mut request = intent(ProofKind::FocusedTest, "cargo-test:ub-review::cli");
            request.timeout_sec = Some(timeout);
            let result = resolve_model_proof_intents(root, &diff(&[]), &[request], &[], budget())?;
            assert!(result.proof_requests.is_empty());
            assert_eq!(result.intents[0].status, "invalid_timeout");
            assert!(result.intents[0].resolved_request_ids.is_empty());
        }
        for (kind, target, command) in [
            (
                ProofKind::FocusedTest,
                "cargo-test:ub-review::cli",
                "cargo test --locked --package ub-review --test cli",
            ),
            (
                ProofKind::BasePlusTests,
                "cargo-test:ub-review::cli",
                "cargo test --locked --package ub-review --test cli",
            ),
            (
                ProofKind::FocusedBuild,
                "cargo-package:ub-review",
                "cargo check --locked --package ub-review",
            ),
            (
                ProofKind::FocusedBuild,
                "workspace",
                "cargo check --locked --workspace",
            ),
        ] {
            let result = resolve_model_proof_intents(
                root,
                &diff(&[]),
                &[intent(kind, target)],
                &[],
                budget(),
            )?;
            assert_eq!(result.intents[0].status, "resolved");
            assert_eq!(result.proof_requests.len(), 1);
            assert_eq!(result.proof_requests[0].command, command);
            assert_eq!(
                result.intents[0].resolved_request_ids,
                [result.proof_requests[0].id.clone()]
            );
            assert!(
                focused_cargo_test_command_spec(command).is_some()
                    || focused_build_command_spec(command).is_some()
            );
        }
        Ok(())
    }

    #[test]
    fn build_rejections_explain_declared_package_recovery() -> Result<()> {
        let mut workspace = graph();
        let guidance = "; choose a declared package using `cargo-package:<package>`";
        for target in ["absent-package", "cargo-target:ub-review/absent-target"] {
            assert_eq!(
                expected_error(resolve_build_target(&workspace, &diff(&[]), target))?,
                format!("no approved focused build target matches `{target}`{guidance}")
            );
        }
        let mut nested = workspace.packages[0].clone();
        nested.name = "nested".to_owned();
        nested.directory = "nested".to_owned();
        workspace.packages.push(nested);
        assert_eq!(
            expected_error(resolve_build_target(
                &workspace,
                &diff(&["nested/src/lib.rs"]),
                "nested/src/lib.rs"
            ))?,
            format!(
                "ambiguous focused build target `nested/src/lib.rs` matched 2 approved targets{guidance}"
            )
        );
        let selected = resolve_build_target(&workspace, &diff(&[]), "cargo-package:nested")
            .map_err(anyhow::Error::msg)?;
        assert_eq!(selected.command, "cargo check --locked --package nested");
        Ok(())
    }

    fn named_target_fixture() -> Result<tempfile::TempDir> {
        let temp = tempfile::tempdir()?;
        std::fs::write(
            temp.path().join("Cargo.toml"),
            r#"[package]
name = "proof-fixture"
version = "0.0.0"
edition = "2024"
build = "build.rs"
[workspace]
[lib]
name = "library"
path = "library.rs"
[[bin]]
name = "app"
path = "app.rs"
[[test]]
name = "integration"
path = "integration.rs"
[[example]]
name = "demo"
path = "demo.rs"
test = true
[[bench]]
name = "speed"
path = "speed.rs"
"#,
        )?;
        for path in [
            "library.rs",
            "build.rs",
            "app.rs",
            "integration.rs",
            "demo.rs",
            "speed.rs",
        ] {
            std::fs::write(temp.path().join(path), "fn main() {}\n")?;
        }
        Ok(temp)
    }

    #[test]
    fn named_build_intents_are_declined_before_scheduling() -> Result<()> {
        let temp = named_target_fixture()?;
        for (kind, target, command) in [
            (
                "bin",
                "app",
                "cargo check --locked --package proof-fixture --bin app",
            ),
            (
                "test",
                "integration",
                "cargo check --locked --package proof-fixture --test integration",
            ),
            (
                "example",
                "demo",
                "cargo check --locked --package proof-fixture --example demo",
            ),
            (
                "bench",
                "speed",
                "cargo check --locked --package proof-fixture --bench speed",
            ),
        ] {
            for separator in ["::", "/"] {
                let label = format!("cargo-target:proof-fixture{separator}{target}");
                let mut input = intent(ProofKind::FocusedBuild, &label);
                input.resolved_request_ids.push("stale-request".to_owned());
                let result =
                    resolve_model_proof_intents(temp.path(), &diff(&[]), &[input], &[], budget())?;
                assert_eq!(result.intents[0].status, "unsupported_target", "{label}");
                assert_eq!(
                    result.intents[0].resolution_reason,
                    format!(
                        "named focused build target `{label}` of kind `{kind}` is unsupported by the proof broker; choose `cargo-package:proof-fixture` only if package-level checking answers the proof question; otherwise leave the proof unresolved"
                    )
                );
                assert!(result.intents[0].resolved_request_ids.is_empty());
                assert!(result.proof_requests.is_empty());
                assert!(focused_build_candidates_from_requests(&result.proof_requests).is_empty());
                assert!(focused_test_candidates_from_requests(&result.proof_requests).is_empty());
                let tasks = build_proof_planner_output(
                    &diff(&[]),
                    &Profile::default(),
                    &result.proof_requests,
                )?
                .proof_tasks;
                assert!(tasks.is_empty());
                assert!(focused_build_command_spec(command).is_none());
                let broker_status = proof_request_status(command, "focused-build");
                assert_eq!(broker_status, "unsupported");
                println!(
                    "named_target_receipt {}",
                    serde_json::json!({
                        "intent": result.intents[0], "generated_requests": result.proof_requests,
                        "declined_command": command, "broker_status": broker_status, "planner_tasks": tasks,
                    })
                );
            }
        }
        Ok(())
    }

    #[test]
    fn remaining_named_build_kinds_are_declined_before_scheduling() -> Result<()> {
        let temp = named_target_fixture()?;
        for (kind, target) in [("lib", "library"), ("custom-build", "build-script-build")] {
            for separator in ["::", "/"] {
                let label = format!("cargo-target:proof-fixture{separator}{target}");
                let mut input = intent(ProofKind::FocusedBuild, &label);
                input.resolved_request_ids.push("stale-request".to_owned());
                let result =
                    resolve_model_proof_intents(temp.path(), &diff(&[]), &[input], &[], budget())?;
                assert_eq!(result.intents[0].status, "unsupported_target", "{label}");
                assert_eq!(
                    result.intents[0].resolution_reason,
                    format!(
                        "named focused build target `{label}` of kind `{kind}` is unsupported by the proof broker; choose `cargo-package:proof-fixture` only if package-level checking answers the proof question; otherwise leave the proof unresolved"
                    )
                );
                assert!(result.intents[0].resolved_request_ids.is_empty());
                assert!(result.proof_requests.is_empty());
                assert!(focused_build_candidates_from_requests(&result.proof_requests).is_empty());
                assert!(focused_test_candidates_from_requests(&result.proof_requests).is_empty());
                assert!(
                    build_proof_planner_output(
                        &diff(&[]),
                        &Profile::default(),
                        &result.proof_requests,
                    )?
                    .proof_tasks
                    .is_empty()
                );
            }
        }
        assert!(
            focused_build_command_spec(
                "cargo check --locked --package proof-fixture --lib library"
            )
            .is_none()
        );
        assert!(
            focused_build_command_spec("cargo check --locked --package proof-fixture").is_some()
        );
        let Some(mut graph) = parse_cargo_workspace(temp.path()) else {
            anyhow::bail!("named-target fixture metadata is unavailable");
        };
        graph.packages[0].targets[0].kind = "future-target-kind".to_owned();
        let target_name = &graph.packages[0].targets[0].name;
        for separator in ["::", "/"] {
            let label = format!("cargo-target:proof-fixture{separator}{target_name}");
            assert_eq!(
                resolve_build_target(&graph, &diff(&[]), &label).err(),
                Some(format!(
                    "named focused build target `{label}` of kind `future-target-kind` is unsupported by the proof broker; choose `cargo-package:proof-fixture` only if package-level checking answers the proof question; otherwise leave the proof unresolved"
                ))
            );
        }
        Ok(())
    }

    #[test]
    fn named_target_supported_intents_keep_exact_requests() -> Result<()> {
        let temp = named_target_fixture()?;
        for (kind, label, cost, command) in [
            (
                ProofKind::FocusedTest,
                "cargo-test:proof-fixture::integration",
                "focused-test",
                "cargo test --locked --package proof-fixture --test integration",
            ),
            (
                ProofKind::BasePlusTests,
                "cargo-test:proof-fixture::integration",
                "focused-test",
                "cargo test --locked --package proof-fixture --test integration",
            ),
            (
                ProofKind::FocusedBuild,
                "cargo-package:proof-fixture",
                "focused-build",
                "cargo check --locked --package proof-fixture",
            ),
            (
                ProofKind::FocusedBuild,
                "workspace",
                "focused-build",
                "cargo check --locked --workspace",
            ),
        ] {
            let result = resolve_model_proof_intents(
                temp.path(),
                &diff(&[]),
                &[intent(kind, label)],
                &[],
                budget(),
            )?;
            assert_eq!(result.intents[0].status, "resolved");
            assert_eq!(result.proof_requests.len(), 1);
            let request = &result.proof_requests[0];
            assert_eq!(request.command, command);
            assert_eq!(request.cost, cost);
            assert_eq!(request.timeout_sec, 300);
            assert!(request.required);
            assert_eq!(
                result.intents[0].resolved_request_ids,
                std::slice::from_ref(&request.id)
            );
            let test_tasks = focused_test_candidates_from_requests(&result.proof_requests);
            let build_tasks = focused_build_candidates_from_requests(&result.proof_requests);
            assert_eq!(
                (test_tasks.len(), build_tasks.len()),
                if cost == "focused-test" {
                    (1, 0)
                } else {
                    (0, 1)
                }
            );
            let spec = if cost == "focused-test" {
                focused_cargo_test_command_spec(command)
            } else {
                focused_build_command_spec(command)
            };
            assert_eq!(
                spec.map(|spec| spec.argv.join(" ")).as_deref(),
                Some(command)
            );
            let broker_status = proof_request_status(command, cost);
            assert_eq!(broker_status, "requested");
            let tasks = build_proof_planner_output(
                &diff(&[]),
                &Profile::default(),
                &result.proof_requests,
            )?
            .proof_tasks;
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].request_ids, std::slice::from_ref(&request.id));
            println!(
                "named_target_receipt {}",
                serde_json::json!({
                    "intent": result.intents[0], "generated_requests": result.proof_requests,
                        "test_tasks": test_tasks.len(), "build_tasks": build_tasks.len(), "broker_status": broker_status, "planner_tasks": tasks,
                })
            );
        }
        Ok(())
    }

    #[test]
    fn named_target_literal_admission_and_malicious_controls() -> Result<()> {
        for (command, test, build) in [
            (
                "cargo test --locked --package proof-fixture --test integration",
                true,
                false,
            ),
            (
                "cargo test --locked --package proof-fixture --bin app named_probe",
                true,
                false,
            ),
            (
                "cargo test --locked --package proof-fixture --example demo named_probe",
                true,
                false,
            ),
            (
                "cargo test --locked --package proof-fixture --bin app",
                false,
                false,
            ),
            (
                "cargo test --locked --package proof-fixture --example demo",
                false,
                false,
            ),
            (
                "cargo test --locked --package proof-fixture --bench speed named_probe",
                false,
                false,
            ),
            (
                "cargo check --locked --package proof-fixture --bins",
                false,
                true,
            ),
            (
                "cargo check --locked --package proof-fixture --tests",
                false,
                true,
            ),
            (
                "cargo check --locked --package proof-fixture --examples",
                false,
                true,
            ),
            (
                "cargo check --locked --package proof-fixture --benches",
                false,
                true,
            ),
            (
                "cargo check --locked --package proof-fixture --bin app",
                false,
                false,
            ),
            (
                "cargo check --locked --package proof-fixture --test integration",
                false,
                false,
            ),
            (
                "cargo check --locked --package proof-fixture --example demo",
                false,
                false,
            ),
            (
                "cargo check --locked --package proof-fixture --bench speed",
                false,
                false,
            ),
            (
                "cargo check --locked --package proof-fixture --bins extra",
                false,
                false,
            ),
            (
                "cargo test --locked --package proof-fixture --test integration -- --format json",
                false,
                false,
            ),
            (
                "cargo test --locked --package proof-fixture --test integration && cargo build",
                false,
                false,
            ),
        ] {
            assert_eq!(
                focused_cargo_test_command_spec(command).is_some(),
                test,
                "{command}"
            );
            assert_eq!(
                focused_build_command_spec(command).is_some(),
                build,
                "{command}"
            );
            assert_eq!(
                proof_request_allowed_v0(command, "focused-test"),
                test,
                "{command}"
            );
            assert_eq!(
                proof_request_allowed_v0(command, "focused-build"),
                build,
                "{command}"
            );
            println!(
                "named_target_admission_receipt {}",
                serde_json::json!({"command": command, "focused_test": proof_request_status(command, "focused-test"), "focused_build": proof_request_status(command, "focused-build")})
            );
        }
        let temp = named_target_fixture()?;
        for label in [
            "cargo-target:proof-fixture::app --release",
            "cargo-target:proof-fixture::app;cargo",
            "cargo-target:proof-fixture::../app",
            "cargo-target:proof-fixture::$(cargo)",
        ] {
            let result = resolve_model_proof_intents(
                temp.path(),
                &diff(&[]),
                &[intent(ProofKind::FocusedBuild, label)],
                &[],
                budget(),
            )?;
            assert_eq!(result.intents[0].status, "rejected_target", "{label}");
            assert!(result.proof_requests.is_empty());
            assert!(result.intents[0].resolved_request_ids.is_empty());
        }
        Ok(())
    }
}
