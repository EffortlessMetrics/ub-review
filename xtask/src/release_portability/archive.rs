use std::io::{Cursor, Read};

use anyhow::{Context, Result, ensure};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::metadata::{ARCHIVE, Asset};

const MAX_EXPANDED_BYTES: u64 = 32 * 1024 * 1024;

pub(super) fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub(super) fn verify_download(asset: &Asset, bytes: &[u8]) -> Result<()> {
    ensure!(
        u64::try_from(bytes.len())? == asset.size,
        "download size differs from resolved asset {}",
        asset.name
    );
    ensure!(
        format!("sha256:{}", sha256(bytes)) == asset.digest,
        "download digest differs from resolved asset {}",
        asset.name
    );
    Ok(())
}

pub(super) fn verify_checksum(bytes: &[u8], archive: &Asset) -> Result<()> {
    let text = std::str::from_utf8(bytes).context("published checksum is not UTF-8")?;
    let expected = archive
        .digest
        .strip_prefix("sha256:")
        .context("archive digest algorithm")?;
    let fields = text.split_whitespace().collect::<Vec<_>>();
    // The immutable published checksum names its build-time dist/ path.
    // Match that exact label; never interpret it as an extraction destination.
    let published_name = format!("dist/{ARCHIVE}");
    ensure!(
        fields.as_slice() == [expected, published_name.as_str()],
        "published checksum does not identify the resolved archive"
    );
    Ok(())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(super) struct ArchiveLayout {
    pub schema: String,
    pub member_name: String,
    pub member_size: u64,
    pub executable_sha256: String,
}

pub(super) fn inspect_archive(bytes: &[u8]) -> Result<(ArchiveLayout, Vec<u8>)> {
    let mut expanded = Vec::new();
    GzDecoder::new(bytes)
        .take(MAX_EXPANDED_BYTES + 1)
        .read_to_end(&mut expanded)
        .context("decompress release archive")?;
    ensure!(
        u64::try_from(expanded.len())? <= MAX_EXPANDED_BYTES,
        "expanded archive exceeds byte budget"
    );
    let mut archive = tar::Archive::new(Cursor::new(expanded));
    let mut entries = archive.entries().context("read archive members")?;
    let mut entry = entries.next().context("archive has no executable")??;
    let raw_name = entry.path_bytes();
    ensure!(
        raw_name.as_ref() == b"ub-review" || raw_name.as_ref() == b"./ub-review",
        "archive executable is not at the exact root path"
    );
    ensure!(
        entry.header().entry_type().is_file(),
        "archive executable is a link or non-regular member"
    );
    ensure!(
        entry.link_name()?.is_none(),
        "archive executable has a link target"
    );
    let name = String::from_utf8(raw_name.into_owned()).context("archive path is not UTF-8")?;
    let size = entry.size();
    ensure!(
        size > 0 && size <= MAX_EXPANDED_BYTES,
        "archive executable size is invalid"
    );
    let mut executable = Vec::new();
    entry
        .read_to_end(&mut executable)
        .context("read archived executable")?;
    ensure!(
        u64::try_from(executable.len())? == size,
        "archive executable is truncated"
    );
    ensure!(
        entries.next().is_none(),
        "archive contains duplicate or additional members"
    );
    Ok((
        ArchiveLayout {
            schema: "ub-review.release_archive_layout.v2".to_owned(),
            member_name: name,
            member_size: size,
            executable_sha256: sha256(&executable),
        },
        executable,
    ))
}

pub(super) fn verify_version(status: bool, stdout: &[u8], stderr: &[u8]) -> Result<()> {
    ensure!(
        status && stderr.is_empty() && std::str::from_utf8(stdout)?.trim() == "ub-review 0.1.0",
        "release product/version identity mismatch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};

    fn archive(names: &[&str], kind: tar::EntryType) -> Result<Vec<u8>> {
        let mut tar = tar::Builder::new(Vec::new());
        for name in names {
            let mut header = tar::Header::new_gnu();
            // Raw header mutation allows hostile traversal fixtures which the
            // producer-side set_path API correctly refuses to manufacture.
            let path = header
                .as_mut_bytes()
                .get_mut(..100)
                .context("tar path field")?;
            path.fill(0);
            path.get_mut(..name.len())
                .context("fixture path length")?
                .copy_from_slice(name.as_bytes());
            header.set_entry_type(kind);
            header.set_size(if kind.is_file() { 3 } else { 0 });
            header.set_mode(0o755);
            if kind.is_symlink() || kind.is_hard_link() {
                header.set_link_name("outside")?;
            }
            header.set_cksum();
            let content: &[u8] = if kind.is_file() { b"bin" } else { b"" };
            tar.append(&header, content)?;
        }
        let bytes = tar.into_inner()?;
        let mut gzip = GzEncoder::new(Vec::new(), Compression::default());
        std::io::Write::write_all(&mut gzip, &bytes)?;
        Ok(gzip.finish()?)
    }

    #[test]
    fn layout_rejects_traversal_links_duplicates_and_wrong_root() -> Result<()> {
        let valid = archive(&["ub-review"], tar::EntryType::Regular)?;
        let (layout, executable) = inspect_archive(&valid)?;
        ensure!(layout.member_size == 3 && executable == b"bin");
        for names in [
            vec!["../ub-review"],
            vec!["/ub-review"],
            vec!["nested/ub-review"],
            vec!["ub-review", "ub-review"],
            vec![],
        ] {
            ensure!(inspect_archive(&archive(&names, tar::EntryType::Regular)?).is_err());
        }
        for kind in [
            tar::EntryType::Symlink,
            tar::EntryType::Link,
            tar::EntryType::Directory,
        ] {
            ensure!(inspect_archive(&archive(&["ub-review"], kind)?).is_err());
        }
        Ok(())
    }

    #[test]
    fn download_and_identity_checks_discriminate_tampered_bytes() -> Result<()> {
        let mut asset = Asset {
            id: 1,
            name: ARCHIVE.to_owned(),
            size: 3,
            digest: format!("sha256:{}", sha256(b"abc")),
            browser_download_url: String::new(),
        };
        verify_download(&asset, b"abc")?;
        ensure!(verify_download(&asset, b"abd").is_err());
        asset.size = 4;
        ensure!(verify_download(&asset, b"abc").is_err());
        let checksum = format!("{}  dist/{ARCHIVE}\n", sha256(b"abc"));
        verify_checksum(checksum.as_bytes(), &asset)?;
        ensure!(verify_checksum(checksum.replace("dist/", "").as_bytes(), &asset).is_err());
        ensure!(verify_checksum(checksum.replace("dist/", "../").as_bytes(), &asset).is_err());
        ensure!(verify_checksum(checksum.replace(ARCHIVE, "other").as_bytes(), &asset).is_err());
        verify_version(true, b"ub-review 0.1.0\n", b"")?;
        ensure!(verify_version(true, b"true (GNU coreutils) 9.4\n", b"").is_err());
        ensure!(verify_version(false, b"ub-review 0.1.0\n", b"").is_err());
        Ok(())
    }
}
