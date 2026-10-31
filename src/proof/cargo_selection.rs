//! Declaration provenance for exact, already-approved Cargo test selections.
//! Presence is not assertion authority. Unsupported selectors and unavailable
//! locked metadata remain unresolved; unit-test filters keep their executor.
//!
//! This adapter reads structured argv from the existing allowlisted resolver;
//! it never executes text quoted in a review. Full native metadata (including
//! the resolve graph) supplies package identity and integration-test declarations.
//! Workspace-only metadata cannot establish dependency-package absence.
//! Version/glob selectors, workspace/default-member unions, manifest overrides,
//! and feature/platform activation are deliberately unresolved here. Their
//! original argv is retained, and none becomes an executed failure.
//!
//! A declared target only permits the existing executor to run. It does not
//! establish that an assertion or filter ran, nor turn compiler diagnostics into
//! assertion authority. Nonzero executions retain the existing command-failure
//! semantics. This is the #1005 provenance boundary, not a general Cargo parser
//! or the #1306 terminal consumer integration. Metadata uses the existing process
//! timeout primitive; the 4 MiB retention cap is applied after completion and
//! does not replace the separate #1281 peak-output streaming work.

use super::command::{proof_command_paths, skipped_proof_command_receipt_for_id};
use crate::task_ledger::TaskNonExecutionDisposition;
use crate::*;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

const METADATA_READ_CAP: u64 = 4 * 1024 * 1024;
const METADATA_TIMEOUT_SEC: u64 = 5;

#[derive(Debug, Serialize)]
#[serde(tag = "disposition", rename_all = "snake_case")]
enum CargoSelectionProvenance {
    OutsideScope,
    PackagePresent {
        package_id: String,
    },
    TargetPresent {
        package_id: String,
        target: String,
        source: String,
    },
    TargetAbsent {
        package_id: String,
        target: String,
    },
    Unresolved {
        reason: String,
    },
}
impl CargoSelectionProvenance {
    fn rejection(&self) -> Option<(TaskNonExecutionDisposition, String)> {
        match self {
            Self::OutsideScope | Self::PackagePresent { .. } | Self::TargetPresent { .. } => None,
            Self::TargetAbsent { package_id, target } => Some((
                TaskNonExecutionDisposition::Unsupported,
                format!(
                    "locked Cargo metadata resolved package `{package_id}` without integration-test target `{target}`; proof did not execute"
                ),
            )),
            Self::Unresolved { reason } => Some((
                TaskNonExecutionDisposition::Refused,
                format!("Cargo selection unresolved: {reason}; proof did not execute"),
            )),
        }
    }
}
#[derive(Debug)]
struct ExactSelection {
    package: String,
    test: Option<String>,
}
enum SelectionRequest {
    OutsideScope,
    Exact(ExactSelection),
    Unresolved(&'static str),
}

fn selection_request(argv: &[String]) -> SelectionRequest {
    if argv.first().map(String::as_str) != Some("cargo")
        || argv.get(1).map(String::as_str) != Some("test")
    {
        return SelectionRequest::OutsideScope;
    }
    let (mut packages, mut tests) = (Vec::new(), Vec::new());
    let (mut locked, mut context_sensitive, mut mixed) = (false, false, false);
    let mut unknown_option = false;
    let mut index = 2;
    while let Some(arg) = argv.get(index) {
        if arg == "--" {
            break;
        }
        match arg.as_str() {
            "--locked" | "--frozen" => locked = true,
            "-p" | "--package" | "--test" => {
                let Some(value) = argv.get(index + 1) else {
                    return SelectionRequest::Unresolved("selector value is missing");
                };
                if arg == "--test" {
                    tests.push(value.clone());
                } else {
                    packages.push(value.clone());
                }
                index += 1;
            }
            "--features" | "--target" => {
                context_sensitive = true;
                index += 1;
            }
            "--all-features" | "--no-default-features" => context_sensitive = true,
            "--workspace" => context_sensitive = true,
            "--all-targets" | "--tests" | "--lib" | "--bins" | "--examples" | "--doc" => {
                mixed = true
            }
            "--bin" | "--example" => {
                mixed = true;
                index += 1;
            }
            _ if arg.starts_with("--package=") => {
                packages.push(arg["--package=".len()..].to_owned())
            }
            _ if arg.starts_with("--test=") => tests.push(arg["--test=".len()..].to_owned()),
            _ if arg.starts_with("--features=") || arg.starts_with("--target=") => {
                context_sensitive = true
            }
            _ if arg.starts_with("--bin=") || arg.starts_with("--example=") => mixed = true,
            "--offline" => {}
            _ if arg.starts_with('-') => unknown_option = true,
            _ => {}
        }
        index += 1;
    }
    if tests.is_empty() && packages.is_empty() {
        return SelectionRequest::OutsideScope;
    }
    if !locked
        || unknown_option
        || (!tests.is_empty() && mixed)
        || packages.len() != 1
        || tests.len() > 1
    {
        return SelectionRequest::Unresolved(
            "requires one locked package and integration-test selector without mixed selection",
        );
    }
    let literal = |value: &str| {
        !value.is_empty()
            && value
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    };
    if !literal(&packages[0]) || tests.first().is_some_and(|test| !literal(test)) {
        return SelectionRequest::Unresolved(
            "glob, version, or nonliteral selector is outside this exact-selection adapter",
        );
    }
    if context_sensitive {
        return SelectionRequest::Unresolved(
            "feature/platform-sensitive selection is preserved but not resolved by this default-context adapter",
        );
    }
    SelectionRequest::Exact(ExactSelection {
        package: packages.remove(0),
        test: tests.pop(),
    })
}
#[derive(Deserialize)]
struct Metadata {
    packages: Vec<MetadataPackage>,
    resolve: Option<MetadataResolve>,
}
#[derive(Deserialize)]
struct MetadataPackage {
    id: String,
    name: String,
    targets: Vec<MetadataTarget>,
}
#[derive(Deserialize)]
struct MetadataTarget {
    name: String,
    kind: Vec<String>,
    src_path: String,
    #[serde(default, rename = "required-features")]
    required_features: Vec<String>,
}
#[derive(Deserialize)]
struct MetadataResolve {
    nodes: Vec<MetadataNode>,
}
#[derive(Deserialize)]
struct MetadataNode {
    id: String,
}

fn check_metadata(selection: &ExactSelection, bytes: &[u8]) -> CargoSelectionProvenance {
    let Ok(metadata) = serde_json::from_slice::<Metadata>(bytes) else {
        return CargoSelectionProvenance::Unresolved {
            reason: "metadata is malformed or incomplete".to_owned(),
        };
    };
    let Some(resolve) = metadata.resolve else {
        return CargoSelectionProvenance::Unresolved {
            reason: "dependency resolution is absent (workspace-only metadata is insufficient)"
                .to_owned(),
        };
    };
    let packages = metadata
        .packages
        .iter()
        .filter(|package| package.name == selection.package)
        .collect::<Vec<_>>();
    if packages.len() != 1 {
        return CargoSelectionProvenance::Unresolved {
            reason: format!(
                "package selector matched {} packages in this locked snapshot; no global nonexistence claim is made",
                packages.len()
            ),
        };
    }
    let package = packages[0];
    if !resolve.nodes.iter().any(|node| node.id == package.id) {
        return CargoSelectionProvenance::Unresolved {
            reason: "selected package has no dependency-resolution identity".to_owned(),
        };
    }
    let Some(test) = selection.test.as_ref() else {
        // A unique package does not validate its library targets or test-name
        // filters. Keep the existing approved executor without granting any
        // new assertion-specific authority.
        return CargoSelectionProvenance::PackagePresent {
            package_id: package.id.clone(),
        };
    };
    if let Some(target) = package
        .targets
        .iter()
        .find(|target| target.name == *test && target.kind.iter().any(|kind| kind == "test"))
    {
        if !target.required_features.is_empty() {
            return CargoSelectionProvenance::Unresolved { reason: "target exists but required-feature activation is not established by this adapter".to_owned() };
        }
        return CargoSelectionProvenance::TargetPresent {
            package_id: package.id.clone(),
            target: target.name.clone(),
            source: target.src_path.clone(),
        };
    }
    CargoSelectionProvenance::TargetAbsent {
        package_id: package.id.clone(),
        target: test.clone(),
    }
}

#[derive(Debug)]
pub(crate) struct SelectionCheckedCommand {
    pub(crate) command: ProofCommandReceipt,
    pub(crate) unresolved: bool,
}

pub(crate) fn run_selection_checked_command<F>(
    invocation: ProofCommandInvocation<'_, '_>,
    runner: &mut F,
) -> Result<SelectionCheckedCommand>
where
    F: FnMut(
        &Path,
        &[String],
        &BTreeMap<String, String>,
        u64,
        &Path,
        &Path,
        &mut dyn FnMut(CommandProcessObservation),
    ) -> Result<CommandStatus>,
{
    if invocation.task_ledger.is_some() != invocation.task.is_some() {
        anyhow::bail!("selection guard requires both task and ledger metadata");
    }
    if invocation.lease.status != "granted"
        || invocation.lease.consumer != invocation.receipt_id
        || matches!(
            selection_request(&invocation.spec.argv),
            SelectionRequest::OutsideScope
        )
    {
        return run_proof_command_receipt(invocation, runner).map(|command| {
            SelectionCheckedCommand {
                command,
                unresolved: false,
            }
        });
    }
    let started = Instant::now();
    let mut completion_unconfirmed = false;
    let preparation = (|| -> Result<CargoSelectionProvenance> {
        let paths = proof_command_paths(
            invocation.out,
            invocation.receipt_id,
            &format!("{}-cargo-metadata", invocation.side),
        )?;
        let provenance = match selection_request(&invocation.spec.argv) {
            SelectionRequest::OutsideScope => CargoSelectionProvenance::OutsideScope,
            SelectionRequest::Unresolved(reason) => CargoSelectionProvenance::Unresolved {
                reason: reason.to_owned(),
            },
            SelectionRequest::Exact(_) if invocation.timeout_sec == 0 => {
                CargoSelectionProvenance::Unresolved {
                    reason: "metadata preparation has no time budget".to_owned(),
                }
            }
            SelectionRequest::Exact(selection) => {
                let argv = [
                    "cargo",
                    "metadata",
                    "--locked",
                    "--offline",
                    "--format-version",
                    "1",
                ]
                .map(str::to_owned);
                let result = run_command_to_files_with_spawn_observer(
                    invocation.command_root,
                    &argv,
                    &invocation.spec.env,
                    invocation.timeout_sec.min(METADATA_TIMEOUT_SEC),
                    &paths.stdout_path,
                    &paths.stderr_path,
                    &mut |observation| {
                        if matches!(
                            observation,
                            CommandProcessObservation::CompletionUnconfirmed
                        ) {
                            completion_unconfirmed = true;
                        }
                    },
                );
                if completion_unconfirmed {
                    return Err(anyhow::anyhow!(
                        "metadata child completion remains unconfirmed; preserve command root for recovery"
                    ));
                }
                if fs::metadata(&paths.stderr_path)?.len() > METADATA_READ_CAP {
                    fs::write(
                        &paths.stderr_path,
                        b"metadata diagnostics exceeded the retention cap\n",
                    )?;
                }
                let stdout_overflow = fs::metadata(&paths.stdout_path)?.len() > METADATA_READ_CAP;
                if stdout_overflow {
                    fs::write(
                        &paths.stdout_path,
                        b"metadata exceeded the read cap; selection unresolved\n",
                    )?;
                }
                match result {
                    Ok(status) if status.success && !status.timed_out => {
                        if stdout_overflow {
                            fs::write(
                                &paths.stdout_path,
                                b"metadata exceeded the read cap; selection unresolved\n",
                            )?;
                            CargoSelectionProvenance::Unresolved {
                                reason: "metadata exceeded the 4 MiB read budget".to_owned(),
                            }
                        } else {
                            check_metadata(&selection, &fs::read(&paths.stdout_path)?)
                        }
                    }
                    _ => CargoSelectionProvenance::Unresolved {
                        reason: "full locked offline metadata was unavailable".to_owned(),
                    },
                }
            }
        };
        fs::write(
            paths.stdout_path.with_file_name("selection.json"),
            serde_json::to_vec_pretty(&provenance)?,
        )?;
        Ok(provenance)
    })();
    let provenance = match preparation {
        Ok(provenance) => provenance,
        Err(error) => {
            if !completion_unconfirmed
                && let Some(cleanup) = invocation.cleanup
                && let Err(cleanup_error) = cleanup()
            {
                return Err(error.context(format!(
                    "cleanup after metadata preparation failure: {cleanup_error:#}"
                )));
            }
            return Err(error);
        }
    };
    let remaining = Duration::from_secs(invocation.timeout_sec)
        .saturating_sub(started.elapsed())
        .as_secs();
    let rejection = provenance.rejection().or_else(|| {
        (remaining == 0).then(|| {
            (
                TaskNonExecutionDisposition::Refused,
                "Cargo metadata preparation consumed the command deadline; proof did not execute"
                    .to_owned(),
            )
        })
    });
    if let Some((disposition, reason)) = rejection {
        if let Some(cleanup) = invocation.cleanup {
            cleanup().context("cleanup unresolved Cargo selection")?;
        }
        if let (Some(ledger), Some(task)) = (invocation.task_ledger, invocation.task) {
            ledger.decline_command(task, disposition, &reason)?;
        }
        let command = skipped_proof_command_receipt_for_id(
            invocation.out,
            invocation.receipt_id,
            invocation.side,
            invocation.spec,
            "skipped",
            reason,
        )?;
        return Ok(SelectionCheckedCommand {
            command,
            unresolved: true,
        });
    }
    let mut bounded_runner =
        |root: &Path,
         argv: &[String],
         env: &BTreeMap<String, String>,
         timeout: u64,
         stdout: &Path,
         stderr: &Path,
         observer: &mut dyn FnMut(CommandProcessObservation)| {
            runner(
                root,
                argv,
                env,
                timeout.min(remaining),
                stdout,
                stderr,
                observer,
            )
        };
    run_proof_command_receipt(invocation, &mut bounded_runner).map(|command| {
        SelectionCheckedCommand {
            command,
            unresolved: false,
        }
    })
}

#[cfg(test)]
pub(crate) mod tests;
