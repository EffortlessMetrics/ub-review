use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use super::archive::ArchiveLayout;
use super::metadata::ReleaseIdentity;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Authorization {
    pub event: String,
    pub actor: String,
    pub run_id: String,
    pub workflow_sha: String,
    pub source_sha: String,
}

impl Authorization {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.event.as_str(), "workflow_dispatch" | "local_explicit"),
            "portability execution is not explicitly authorized"
        );
        ensure!(
            !self.actor.trim().is_empty() && !self.run_id.trim().is_empty(),
            "execution has no actor/run identity"
        );
        super::metadata::require_oid(&self.source_sha)?;
        super::metadata::require_oid(&self.workflow_sha)?;
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Environment {
    pub cargo_present: bool,
    pub rustc_present: bool,
    pub source_checkout_present: bool,
    pub source_fallback_present: bool,
}

impl Environment {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.cargo_present
                && !self.rustc_present
                && !self.source_checkout_present
                && !self.source_fallback_present,
            "proof environment contains Rust tooling or source fallback"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Platform {
    pub image: String,
    pub image_id: String,
    pub os_id: String,
    pub os_version: String,
    pub architecture: String,
    pub libc: String,
    pub kernel: String,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum Outcome {
    SupportedPass,
    UnsupportedExpected,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Row {
    pub schema: String,
    pub release: ReleaseIdentity,
    #[serde(rename = "authorization")]
    pub execution: Authorization,
    pub archive: ArchiveLayout,
    pub platform: Platform,
    pub environment: Environment,
    pub outcome: Outcome,
    pub checks: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub(super) struct Matrix {
    schema: &'static str,
    release: ReleaseIdentity,
    #[serde(rename = "authorization")]
    execution: Authorization,
    rows: Vec<Row>,
    decision: BTreeMap<&'static str, &'static str>,
}

pub(super) fn reconcile(
    mut rows: Vec<Row>,
    expected_release: &ReleaseIdentity,
    expected_authorization: &Authorization,
) -> Result<Matrix> {
    expected_authorization.validate()?;
    ensure!(rows.len() == 2, "matrix requires exactly two runtime rows");
    rows.sort_by(|left, right| left.platform.image.cmp(&right.platform.image));
    let first = rows.first().context("matrix first row")?;
    let release = first.release.clone();
    let execution = first.execution.clone();
    ensure!(
        &release == expected_release && &execution == expected_authorization,
        "runtime rows do not bind the independently resolved release and execution authorization"
    );
    let layout = first.archive.clone();
    let mut seen = std::collections::BTreeSet::new();
    for row in &rows {
        ensure!(
            row.schema == "ub-review.release_portability_receipt.v2",
            "wrong row schema"
        );
        ensure!(
            row.release == release && row.execution == execution && row.archive == layout,
            "matrix rows disagree about the release, source, or archive"
        );
        row.environment.validate()?;
        ensure!(
            row.platform.os_id == "ubuntu" && row.platform.architecture == "x86_64",
            "unexpected runtime platform"
        );
        ensure!(
            !row.platform.image_id.is_empty() && !row.platform.kernel.is_empty(),
            "missing runtime observations"
        );
        ensure!(
            seen.insert(row.platform.image.clone()),
            "duplicate runtime image"
        );
        let expected = match row.platform.image.as_str() {
            "ubuntu:24.04" => ("24.04", "glibc 2.39", Outcome::SupportedPass),
            "ubuntu:22.04" => ("22.04", "glibc 2.35", Outcome::UnsupportedExpected),
            _ => anyhow::bail!("unrecognized runtime matrix image"),
        };
        ensure!(
            row.platform.os_version == expected.0
                && row.platform.libc == expected.1
                && row.outcome == expected.2,
            "runtime observation contradicts the declared compatibility boundary"
        );
        for name in [
            "download",
            "published_checksum",
            "archive_layout",
            "environment",
        ] {
            ensure!(
                row.checks.get(name).map(String::as_str) == Some("pass"),
                "missing successful {name} proof"
            );
        }
        let required = if row.outcome == Outcome::SupportedPass {
            vec![
                ("binary_identity", "pass"),
                ("help", "pass"),
                ("doctor", "pass"),
                ("init_known_defect", "expected_policy_failure"),
                ("model_off_packet", "pass"),
                ("negative_controls", "pass"),
            ]
        } else {
            vec![("loader", "GLIBC_2.39_rejection")]
        };
        for (name, expected) in required {
            ensure!(
                row.checks.get(name).map(String::as_str) == Some(expected),
                "missing successful {name} proof"
            );
        }
    }
    Ok(Matrix {
        schema: "ub-review.release_portability_matrix.v2",
        release,
        execution,
        rows,
        decision: BTreeMap::from([
            (
                "ubuntu_24_04_x86_64_glibc_2_39",
                "supported_by_this_exact_asset_execution",
            ),
            (
                "ubuntu_22_04_x86_64_glibc_2_35",
                "explicitly_unsupported_GLIBC_2.39",
            ),
            ("other_platforms", "not_proven"),
            (
                "v0.1.0_init",
                "historical_empty_policy_defaults_fixed_separately_by_845",
            ),
        ]),
    })
}

pub(super) fn json_bytes(value: &impl Serialize) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(super) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    fs::write(path, json_bytes(value)?).with_context(|| format!("write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(image: &str) -> Result<Row> {
        let supported = image == "ubuntu:24.04";
        let release = crate::release_portability::metadata::validate_release(
            &crate::release_portability::metadata::tests::release(),
            &crate::release_portability::metadata::tests::reference(),
            &BTreeMap::new(),
        )?;
        Ok(Row {
            schema: "ub-review.release_portability_receipt.v2".to_owned(),
            release,
            execution: Authorization {
                event: "workflow_dispatch".to_owned(),
                actor: "fixture".to_owned(),
                run_id: "123".to_owned(),
                workflow_sha: "a".repeat(40),
                source_sha: "b".repeat(40),
            },
            archive: ArchiveLayout {
                schema: "ub-review.release_archive_layout.v2".to_owned(),
                member_name: "ub-review".to_owned(),
                member_size: 3,
                executable_sha256: "c".repeat(64),
            },
            platform: Platform {
                image: image.to_owned(),
                image_id: "sha256:fixture".to_owned(),
                os_id: "ubuntu".to_owned(),
                os_version: if supported { "24.04" } else { "22.04" }.to_owned(),
                architecture: "x86_64".to_owned(),
                libc: if supported {
                    "glibc 2.39"
                } else {
                    "glibc 2.35"
                }
                .to_owned(),
                kernel: "fixture".to_owned(),
            },
            environment: Environment {
                cargo_present: false,
                rustc_present: false,
                source_checkout_present: false,
                source_fallback_present: false,
            },
            outcome: if supported {
                Outcome::SupportedPass
            } else {
                Outcome::UnsupportedExpected
            },
            checks: [
                ("download", "pass"),
                ("published_checksum", "pass"),
                ("archive_layout", "pass"),
                ("environment", "pass"),
                ("binary_identity", "pass"),
                ("help", "pass"),
                ("doctor", "pass"),
                ("init_known_defect", "expected_policy_failure"),
                ("model_off_packet", "pass"),
                ("negative_controls", "pass"),
                ("loader", "GLIBC_2.39_rejection"),
            ]
            .into_iter()
            .map(|(a, b)| (a.to_owned(), b.to_owned()))
            .collect(),
        })
    }

    #[test]
    fn matrix_requires_both_exact_platforms_and_consistent_identity() -> Result<()> {
        let positive = row("ubuntu:24.04")?;
        let negative = row("ubuntu:22.04")?;
        let check = |rows| reconcile(rows, &positive.release, &positive.execution);
        let bytes = json_bytes(&check(vec![positive.clone(), negative.clone()])?)?;
        let serialized: serde_json::Value = serde_json::from_slice(&bytes)?;
        ensure!(
            serialized.get("authorization") == Some(&serde_json::to_value(&positive.execution)?)
        );
        ensure!(serialized.get("execution").is_none());
        ensure!(bytes == json_bytes(&check(vec![negative.clone(), positive.clone()])?)?);
        ensure!(check(vec![positive.clone()]).is_err());
        ensure!(check(vec![positive.clone(), positive.clone()]).is_err());
        for mutation in 0..5 {
            let mut changed = negative.clone();
            match mutation {
                0 => changed.release.archive.id += 1,
                1 => changed.platform.libc = "glibc 2.39".to_owned(),
                2 => changed.environment.cargo_present = true,
                3 => changed.outcome = Outcome::SupportedPass,
                _ => {
                    changed.checks.remove("loader");
                }
            }
            ensure!(check(vec![positive.clone(), changed]).is_err());
        }
        let mut forged_positive = positive.clone();
        let mut forged_negative = negative;
        forged_positive.release.archive.id += 1;
        forged_negative.release.archive.id += 1;
        ensure!(check(vec![forged_positive, forged_negative]).is_err());
        Ok(())
    }
}
