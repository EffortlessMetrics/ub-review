use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

pub(super) const REPOSITORY: &str = "EffortlessMetrics/ub-review";
pub(super) const TAG: &str = "v0.1.0";
pub(super) const COMMIT: &str = "743ae2b5d9b9532852f702a095cf363380c932a2";
pub(super) const ARCHIVE: &str = "ub-review-x86_64-unknown-linux-gnu.tar.gz";
pub(super) const ARCHIVE_SHA256: &str =
    "87a660273e8d6f76d78b41d5bf2da1ed2928cb7987fbe114f9e1035d32b03465";
const CHECKSUM_SHA256: &str = "1d3506efb235aeeeb86f02dbe45081808d6f2995a2c44c7535c0f938beb23a97";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct Asset {
    pub id: u64,
    pub name: String,
    pub size: u64,
    pub digest: String,
    pub browser_download_url: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct Release {
    pub id: u64,
    pub tag_name: String,
    pub draft: bool,
    pub prerelease: bool,
    pub assets: Vec<Asset>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum ObjectKind {
    Tag,
    Commit,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct GitObject {
    pub sha: String,
    #[serde(rename = "type")]
    pub kind: ObjectKind,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct TagRef {
    #[serde(rename = "ref")]
    pub reference: String,
    pub object: GitObject,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct AnnotatedTag {
    pub sha: String,
    pub object: GitObject,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct ReleaseIdentity {
    pub release_id: u64,
    pub tag: String,
    pub tag_commit: String,
    pub archive: Asset,
    pub checksum: Asset,
    pub tag_chain: Vec<GitObject>,
}

pub(super) fn require_oid(oid: &str) -> Result<()> {
    ensure!(
        oid.len() == 40
            && oid
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "Git object identity must be a canonical forty-character SHA"
    );
    Ok(())
}

pub(super) fn resolve_tag(
    reference: &TagRef,
    annotations: &BTreeMap<String, AnnotatedTag>,
) -> Result<Vec<GitObject>> {
    ensure!(
        reference.reference == format!("refs/tags/{TAG}"),
        "wrong tag reference"
    );
    let mut current = reference.object.clone();
    let mut seen = BTreeSet::new();
    let mut chain = Vec::new();
    loop {
        require_oid(&current.sha)?;
        ensure!(
            chain.len() < 8 && seen.insert(current.sha.clone()),
            "tag chain is cyclic or excessive"
        );
        chain.push(current.clone());
        if current.kind == ObjectKind::Commit {
            ensure!(
                current.sha == COMMIT,
                "published tag moved from its pinned commit"
            );
            return Ok(chain);
        }
        let annotation = annotations
            .get(&current.sha)
            .context("annotated tag object is missing")?;
        ensure!(
            annotation.sha == current.sha,
            "annotated tag identity mismatch"
        );
        current = annotation.object.clone();
    }
}

fn exact_asset(release: &Release, name: &str, id: u64, size: u64, digest: &str) -> Result<Asset> {
    let mut matches = release.assets.iter().filter(|asset| asset.name == name);
    let asset = matches
        .next()
        .with_context(|| format!("missing release asset {name}"))?;
    ensure!(matches.next().is_none(), "duplicate release asset {name}");
    ensure!(asset.id == id, "asset ID mismatch for {name}");
    ensure!(asset.size == size, "asset size mismatch for {name}");
    ensure!(
        asset.digest == format!("sha256:{digest}"),
        "asset published digest mismatch for {name}"
    );
    ensure!(
        asset.browser_download_url
            == format!("https://github.com/{REPOSITORY}/releases/download/{TAG}/{name}"),
        "asset download URL mismatch for {name}"
    );
    Ok(asset.clone())
}

pub(super) fn validate_release(
    release: &Release,
    reference: &TagRef,
    annotations: &BTreeMap<String, AnnotatedTag>,
) -> Result<ReleaseIdentity> {
    ensure!(release.id == 356064141, "release ID mismatch");
    ensure!(
        release.tag_name == TAG && !release.draft && !release.prerelease,
        "unexpected release tag/state"
    );
    let chain = resolve_tag(reference, annotations)?;
    let commit = chain
        .last()
        .context("empty resolved tag chain")?
        .sha
        .clone();
    Ok(ReleaseIdentity {
        release_id: release.id,
        tag: release.tag_name.clone(),
        tag_commit: commit,
        archive: exact_asset(release, ARCHIVE, 481332264, 2_750_491, ARCHIVE_SHA256)?,
        checksum: exact_asset(
            release,
            &format!("{ARCHIVE}.sha256"),
            481332265,
            113,
            CHECKSUM_SHA256,
        )?,
        tag_chain: chain,
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    pub(in crate::release_portability) fn release() -> Release {
        Release {
            id: 356064141,
            tag_name: TAG.to_owned(),
            draft: false,
            prerelease: false,
            assets: [
                (ARCHIVE.to_owned(), 481332264, 2_750_491, ARCHIVE_SHA256),
                (format!("{ARCHIVE}.sha256"), 481332265, 113, CHECKSUM_SHA256),
            ]
            .into_iter()
            .map(|(name, id, size, digest)| Asset {
                browser_download_url: format!(
                    "https://github.com/{REPOSITORY}/releases/download/{TAG}/{name}"
                ),
                name,
                id,
                size,
                digest: format!("sha256:{digest}"),
            })
            .collect(),
        }
    }

    pub(in crate::release_portability) fn reference() -> TagRef {
        TagRef {
            reference: format!("refs/tags/{TAG}"),
            object: GitObject {
                sha: COMMIT.to_owned(),
                kind: ObjectKind::Commit,
            },
        }
    }

    #[test]
    fn typed_metadata_selects_exact_observed_assets() -> Result<()> {
        let metadata: Release = serde_json::from_slice(&serde_json::to_vec(&release())?)?;
        let identity = validate_release(&metadata, &reference(), &BTreeMap::new())?;
        ensure!(identity.archive.id == 481332264 && identity.checksum.size == 113);
        ensure!(serde_json::from_str::<Release>(r#"{"id":"wrong"}"#).is_err());
        for mutation in 0..4 {
            let mut changed = release();
            match mutation {
                0 => changed.id += 1,
                1 => changed.tag_name = "v0.1.1".to_owned(),
                2 => changed.draft = true,
                _ => changed.prerelease = true,
            }
            ensure!(
                validate_release(&changed, &reference(), &BTreeMap::new()).is_err(),
                "accepted metadata mutation {mutation}"
            );
        }
        for asset_index in 0..2 {
            for mutation in 0..7 {
                let mut changed = release();
                let asset = changed
                    .assets
                    .get_mut(asset_index)
                    .context("fixture asset")?;
                match mutation {
                    0 => asset.id += 1,
                    1 => asset.size += 1,
                    2 => asset.digest = format!("sha256:{}", "0".repeat(64)),
                    3 => asset.browser_download_url = "https://example.invalid/asset".to_owned(),
                    4 => asset.name = "wrong.tar.gz".to_owned(),
                    5 => {
                        let duplicate = asset.clone();
                        changed.assets.push(duplicate);
                    }
                    _ => {
                        changed.assets.remove(asset_index);
                    }
                }
                ensure!(
                    validate_release(&changed, &reference(), &BTreeMap::new()).is_err(),
                    "accepted asset {asset_index} mutation {mutation}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn annotated_tag_dereference_rejects_movement_missing_and_cycles() -> Result<()> {
        let sha = "a".repeat(40);
        let mut reference = reference();
        reference.object = GitObject {
            sha: sha.clone(),
            kind: ObjectKind::Tag,
        };
        let mut annotations = BTreeMap::from([(
            sha.clone(),
            AnnotatedTag {
                sha: sha.clone(),
                object: GitObject {
                    sha: COMMIT.to_owned(),
                    kind: ObjectKind::Commit,
                },
            },
        )]);
        let chain = resolve_tag(&reference, &annotations)?;
        ensure!(chain.len() == 2 && chain.last().context("resolved commit")?.sha == COMMIT);
        ensure!(resolve_tag(&reference, &BTreeMap::new()).is_err());
        annotations.get_mut(&sha).context("annotation")?.object.sha = "b".repeat(40);
        ensure!(resolve_tag(&reference, &annotations).is_err());
        annotations.get_mut(&sha).context("annotation")?.object = reference.object.clone();
        ensure!(resolve_tag(&reference, &annotations).is_err());
        annotations.get_mut(&sha).context("annotation")?.object = GitObject {
            sha: COMMIT.to_owned(),
            kind: ObjectKind::Commit,
        };
        annotations.get_mut(&sha).context("annotation")?.sha = "b".repeat(40);
        ensure!(resolve_tag(&reference, &annotations).is_err());
        Ok(())
    }

    #[test]
    fn tag_resolution_rejects_noncanonical_identity_and_excessive_depth() -> Result<()> {
        for oid in ["", "abcdef", &"A".repeat(40), &"g".repeat(40)] {
            ensure!(require_oid(oid).is_err(), "accepted malformed Git identity");
        }
        let mut wrong_reference = reference();
        wrong_reference.reference = "refs/tags/v0.1.1".to_owned();
        ensure!(resolve_tag(&wrong_reference, &BTreeMap::new()).is_err());
        let mut current = reference();
        let mut annotations = BTreeMap::new();
        for depth in 1..=8 {
            let sha = format!("{depth:040x}");
            annotations.insert(
                sha.clone(),
                AnnotatedTag {
                    sha: sha.clone(),
                    object: current.object.clone(),
                },
            );
            current.object = GitObject {
                sha,
                kind: ObjectKind::Tag,
            };
            if depth < 8 {
                ensure!(resolve_tag(&current, &annotations)?.len() == depth + 1);
            } else {
                ensure!(resolve_tag(&current, &annotations).is_err());
            }
        }
        Ok(())
    }
}
