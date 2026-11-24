use super::{Campaign, Provenance, Result, digest, fs, rehearse, write};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::io::Read;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use xtask::release_bytes::{
    ASSET, CHECKSUM_ASSET, MAX_ARCHIVE_BYTES, MAX_PACKET_BYTES, Producer, parse_packet,
    verify_asset_bytes, verify_candidate_source,
};

const PACKET: &str = "authorized-release-packet.json";
const RELEASE_NOTES: &str = "release-notes.md";
const MAX_ZIP_OVERHEAD: usize = 1024 * 1024;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Artifact {
    id: u64,
    name: String,
    run_id: u64,
    run_attempt: u64,
    retained_until: u64,
    length: u64,
    sha256: String,
}

/// This input comes from retained custody outside the downloaded ZIPs.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CandidateFacts {
    producer: Producer,
    package_version: String,
    bytes_artifact: Artifact,
    packet_artifact: Artifact,
    raw_packet_sha256: String,
}

fn read_bounded(path: &Path, limit: usize) -> Result<Vec<u8>> {
    ensure!(
        fs::symlink_metadata(path)?.is_file(),
        "not a regular input: {}",
        path.display()
    );
    let file = fs::File::open(path)?;
    ensure!(
        file.metadata()?.len() <= limit as u64,
        "input exceeds size limit: {}",
        path.display()
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "input exceeds size limit: {}",
        path.display()
    );
    Ok(bytes)
}

fn capture(zip: &Path, captured: &Path, artifact: &Artifact, limit: usize) -> Result<()> {
    let bytes = read_bounded(zip, limit)?;
    ensure!(
        bytes.len() as u64 == artifact.length,
        "{} ZIP length mismatch",
        artifact.name
    );
    ensure!(
        digest(&bytes) == artifact.sha256,
        "{} ZIP digest mismatch",
        artifact.name
    );
    write(captured, bytes)
}

fn unzip(zip: &Path, member: Option<&str>, limit: usize) -> Result<Vec<u8>> {
    let mut command = Command::new("/usr/bin/timeout");
    command
        .args(["-k", "1", "10", "/usr/bin/unzip"])
        .arg(if member.is_some() { "-p" } else { "-Z1" })
        .arg(zip)
        .args(member)
        .env_clear()
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command.spawn().context("start bounded ZIP reader")?;
    let mut bytes = Vec::new();
    let read = child
        .stdout
        .take()
        .context("ZIP reader pipe missing")
        .and_then(|stdout| Ok(stdout.take(limit as u64 + 1).read_to_end(&mut bytes)?));
    // The pipe is closed even on overflow. Keep the timeout supervisor alive
    // until it has reaped its reader, including on the failure path.
    let status = child.wait()?;
    read?;
    ensure!(
        bytes.len() <= limit,
        "ZIP member or listing exceeds size limit"
    );
    ensure!(
        status.success(),
        "ZIP reader failed for {member:?}: {status}"
    );
    Ok(bytes)
}

fn check_zip_inventory(zip: &Path, expected: &[&str]) -> Result<()> {
    let listing = unzip(zip, None, 1024)?;
    let names = std::str::from_utf8(&listing)?.lines().collect::<Vec<_>>();
    ensure!(
        names.len() == expected.len() && expected.iter().all(|name| names.contains(name)),
        "unexpected ZIP members: {names:?}"
    );
    Ok(())
}

fn admit(
    c: &mut Campaign,
    bytes_zip: &Path,
    packet_zip: &Path,
    facts_path: &Path,
    now: u64,
) -> Result<()> {
    let raw_facts = read_bounded(facts_path, MAX_PACKET_BYTES)?;
    let facts: CandidateFacts =
        serde_json::from_slice(&raw_facts).context("candidate custody facts")?;
    let bytes = &facts.bytes_artifact;
    let packet_artifact = &facts.packet_artifact;
    ensure!(
        facts.producer.repository == "EffortlessMetrics/ub-review"
            && facts.producer.workflow == ".github/workflows/release-binary.yml"
            && bytes.id == facts.producer.artifact_id
            && bytes.name == facts.producer.artifact_name
            && bytes.name == "ub-review-linux-x64-release"
            && bytes.retained_until == facts.producer.retained_until
            && packet_artifact.id != 0
            && packet_artifact.id != bytes.id
            && packet_artifact.name == "ub-review-linux-x64-release-packet"
            && packet_artifact.retained_until > now,
        "candidate artifact identity or retention mismatch"
    );

    for artifact in [bytes, packet_artifact] {
        ensure!(
            artifact.run_id == facts.producer.run_id
                && artifact.run_attempt == facts.producer.run_attempt,
            "artifact run or attempt mismatch: {}",
            artifact.name
        );
    }

    // The reader sees only private copies of the independently digest-checked
    // wrappers. It streams fixed members without extracting them to disk.
    let captured = tempfile::tempdir_in(&c.root)?;
    let retained_packet = captured.path().join("packet.zip");
    capture(
        packet_zip,
        &retained_packet,
        packet_artifact,
        MAX_PACKET_BYTES + MAX_ZIP_OVERHEAD,
    )?;
    check_zip_inventory(&retained_packet, &[PACKET])?;
    let raw_packet = unzip(&retained_packet, Some(PACKET), MAX_PACKET_BYTES)?;
    let packet_sha256 = digest(&raw_packet);
    ensure!(
        packet_sha256 == facts.raw_packet_sha256,
        "raw producer packet digest mismatch"
    );
    let packet =
        parse_packet(&raw_packet).map_err(|error| anyhow::anyhow!("producer packet: {error:?}"))?;
    verify_candidate_source(&packet, &facts.producer, now)
        .map_err(|error| anyhow::anyhow!("candidate source: {error:?}"))?;
    ensure!(
        packet.package_version == facts.package_version,
        "independent package version mismatch"
    );
    ensure!(
        packet.retained_until <= packet_artifact.retained_until,
        "packet artifact retention does not cover candidate"
    );

    let retained_bytes = captured.path().join("candidate.zip");
    capture(
        bytes_zip,
        &retained_bytes,
        bytes,
        MAX_ARCHIVE_BYTES + MAX_ZIP_OVERHEAD,
    )?;
    check_zip_inventory(&retained_bytes, &[ASSET, CHECKSUM_ASSET, RELEASE_NOTES])?;
    let archive = unzip(&retained_bytes, Some(ASSET), MAX_ARCHIVE_BYTES)?;
    let checksum = unzip(&retained_bytes, Some(CHECKSUM_ASSET), 512)?;
    let binary_sha256 = verify_asset_bytes(&packet, &archive, &checksum)
        .map_err(|error| anyhow::anyhow!("candidate bytes: {error:?}"))?;

    fs::DirBuilder::new().mode(0o700).create(&c.frozen)?;
    write(&c.frozen.join(ASSET), archive)?;
    write(&c.frozen.join(CHECKSUM_ASSET), checksum)?;
    c.manifest = json!({
        "source_commit": packet.head_sha,
        "source_tree": packet.tree_sha,
        "toolchain": packet.toolchain,
        "version": packet.package_version,
        "tag": packet.proposed_tag,
        "target": packet.target,
        "archive_sha256": packet.archive_sha256,
        "archive_length": packet.archive_length,
        "checksum_sha256": packet.checksum_sha256,
        "checksum_length": packet.checksum_length,
        "binary_sha256": binary_sha256,
        "raw_packet_sha256": packet_sha256,
        "retained_until": packet.retained_until,
        "custody_facts_sha256": digest(&raw_facts),
        "producer": facts.producer,
        "bytes_artifact": bytes,
        "packet_artifact": packet_artifact,
    });
    Ok(())
}

fn candidate_campaign(root: PathBuf, provenance: Provenance) -> Campaign {
    Campaign {
        path: root.join("bin"),
        frozen: root.join("frozen"),
        root,
        provenance,
        manifest: json!({}),
        rows: vec![],
        scenarios: BTreeMap::new(),
    }
}

#[test]
#[ignore = "requires independently retained candidate ZIPs, custody facts and receipt directory"]
fn selected_candidate_archive_onboarding() -> Result<()> {
    let mode = std::env::var("UB_CANDIDATE_PROVENANCE").context(
        "UB_CANDIDATE_PROVENANCE must explicitly select candidate_rehearsal or synthetic_fixture",
    )?;
    let provenance = match mode.as_str() {
        "candidate_rehearsal" => Provenance::Candidate,
        "synthetic_fixture" => Provenance::Synthetic,
        _ => anyhow::bail!(
            "invalid UB_CANDIDATE_PROVENANCE: expected candidate_rehearsal or synthetic_fixture"
        ),
    };
    let root = input("UB_ARCHIVE_RECEIPTS")?;
    ensure!(root.is_absolute(), "receipt directory must be absolute");
    fs::DirBuilder::new().mode(0o700).create(&root)?;
    let mut campaign = candidate_campaign(root, provenance);
    let result = (|| {
        admit(
            &mut campaign,
            &input("UB_CANDIDATE_BYTES_ZIP")?,
            &input("UB_CANDIDATE_PACKET_ZIP")?,
            &input("UB_CANDIDATE_FACTS")?,
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        )?;
        rehearse(&mut campaign)
    })();
    campaign.receipt(
        if result.is_ok() {
            "bounded_pass"
        } else {
            "failed"
        },
        result
            .as_ref()
            .err()
            .map(|error: &anyhow::Error| format!("{error:#}")),
    )?;
    result
}

fn input(name: &str) -> Result<PathBuf> {
    Ok(PathBuf::from(
        std::env::var_os(name).with_context(|| format!("{name} required"))?,
    ))
}

mod controls {
    use super::*;
    use crate::{executable, install_and_check_version, prepare_resolver};
    use xtask::release_bytes::{Packet, SCHEMA};

    const VERSION: &str = "0.1.2";
    const EXECUTABLE: &str = "#!/bin/bash\n# a\nprintf 'ub-review 0.1.2\\n'\n";
    const SUBSTITUTE: &str = "#!/bin/bash\n# b\nprintf 'ub-review 0.1.2\\n'\n";
    const NOW: u64 = 100;

    struct Fixture {
        temp: tempfile::TempDir,
        bytes: PathBuf,
        packet: PathBuf,
        facts: PathBuf,
    }

    impl Fixture {
        fn new() -> Result<Self> {
            let temp = tempfile::tempdir()?;
            let root = temp.path();
            executable(&root.join("ub-review"), EXECUTABLE)?;
            Self::run(root, "/usr/bin/tar", &["-czf", ASSET, "ub-review"])?;
            let archive = fs::read(root.join(ASSET))?;
            let archive_sha256 = digest(&archive);
            let checksum = format!("{archive_sha256}  dist/{ASSET}\n");
            write(&root.join(CHECKSUM_ASSET), &checksum)?;
            write(&root.join(RELEASE_NOTES), "Synthetic fixture")?;
            Self::run(
                root,
                "/usr/bin/zip",
                &[
                    "-q",
                    "-0",
                    "bytes.zip",
                    ASSET,
                    CHECKSUM_ASSET,
                    RELEASE_NOTES,
                ],
            )?;
            let packet = Packet {
                schema: SCHEMA.into(),
                repository: "EffortlessMetrics/ub-review".into(),
                head_sha: "a".repeat(40),
                tree_sha: "b".repeat(40),
                workflow: ".github/workflows/release-binary.yml".into(),
                producer_ref: "refs/heads/main".into(),
                run_id: 28,
                run_attempt: 2,
                artifact_id: 41,
                artifact_name: "ub-review-linux-x64-release".into(),
                proposed_tag: format!("v{VERSION}"),
                peeled_commit: "a".repeat(40),
                asset: ASSET.into(),
                checksum_asset: CHECKSUM_ASSET.into(),
                archive_length: archive.len() as u64,
                checksum_length: checksum.len() as u64,
                archive_sha256,
                checksum_sha256: digest(checksum.as_bytes()),
                binary_version: VERSION.into(),
                package_version: VERSION.into(),
                target: "x86_64-unknown-linux-gnu".into(),
                toolchain: "1.95.0".into(),
                retained_until: NOW + 10,
                authorization_record: "synthetic-fixture".into(),
                verifier: "synthetic-fixture".into(),
            };
            let producer = Producer {
                repository: "EffortlessMetrics/ub-review".into(),
                head_sha: "a".repeat(40),
                tree_sha: "b".repeat(40),
                workflow: ".github/workflows/release-binary.yml".into(),
                producer_ref: "refs/heads/main".into(),
                run_id: 28,
                run_attempt: 2,
                artifact_id: 41,
                artifact_name: "ub-review-linux-x64-release".into(),
                retained_until: NOW + 20,
            };
            let raw_packet = serde_json::to_vec(&packet)?;
            write(&root.join(PACKET), &raw_packet)?;
            Self::run(root, "/usr/bin/zip", &["-q", "-0", "packet.zip", PACKET])?;
            let fixture = Self {
                bytes: root.join("bytes.zip"),
                packet: root.join("packet.zip"),
                facts: root.join("independent.json"),
                temp,
            };
            let facts = CandidateFacts {
                producer,
                package_version: VERSION.into(),
                bytes_artifact: Self::artifact(&fixture.bytes, 41, "ub-review-linux-x64-release")?,
                packet_artifact: Self::artifact(
                    &fixture.packet,
                    42,
                    "ub-review-linux-x64-release-packet",
                )?,
                raw_packet_sha256: digest(&raw_packet),
            };
            write(&fixture.facts, serde_json::to_vec(&facts)?)?;
            Ok(fixture)
        }

        fn run(cwd: &Path, program: &str, args: &[&str]) -> Result<()> {
            let status = Command::new("/usr/bin/timeout")
                .args(["-k", "1", "10", program])
                .args(args)
                .current_dir(cwd)
                .env_clear()
                .env("PATH", "/usr/bin:/bin")
                .status()?;
            ensure!(
                status.success(),
                "fixture utility {program} failed: {status}"
            );
            Ok(())
        }

        fn artifact(path: &Path, id: u64, name: &str) -> Result<Artifact> {
            let bytes = fs::read(path)?;
            Ok(Artifact {
                id,
                name: name.into(),
                run_id: 28,
                run_attempt: 2,
                length: bytes.len() as u64,
                sha256: digest(&bytes),
                retained_until: NOW + 20,
            })
        }

        fn facts(&self) -> Result<CandidateFacts> {
            Ok(serde_json::from_slice(&fs::read(&self.facts)?)?)
        }

        fn update_packet(&self, raw: &[u8], facts: &mut CandidateFacts) -> Result<()> {
            let root = self.temp.path();
            write(&root.join(PACKET), raw)?;
            fs::remove_file(&self.packet)?;
            Self::run(root, "/usr/bin/zip", &["-q", "-0", "packet.zip", PACKET])?;
            facts.packet_artifact =
                Self::artifact(&self.packet, 42, "ub-review-linux-x64-release-packet")?;
            Ok(())
        }

        fn campaign(&self) -> Result<Campaign> {
            let root = tempfile::tempdir_in(self.temp.path())?.keep();
            Ok(candidate_campaign(root, Provenance::Synthetic))
        }

        fn admit(&self, c: &mut Campaign) -> Result<()> {
            admit(c, &self.bytes, &self.packet, &self.facts, NOW)
        }

        fn refusals_have_no_resolver_activity(&self, expected: &str) -> Result<()> {
            let mut campaign = self.campaign()?;
            let error = self
                .admit(&mut campaign)
                .err()
                .context("invalid candidate was admitted")?;
            ensure!(
                format!("{error:#}").contains(expected),
                "unexpected refusal: {error:#}"
            );
            ensure!(
                campaign.rows.is_empty()
                    && campaign.scenarios.is_empty()
                    && !campaign.frozen.exists()
                    && !campaign.root.join("adapter.log").exists(),
                "invalid candidate reached the resolver"
            );
            Ok(())
        }
    }

    #[test]
    fn synthetic_candidate_installs_only_the_admitted_executable_bytes() -> Result<()> {
        let fixture = Fixture::new()?;
        let mut campaign = fixture.campaign()?;
        fixture.admit(&mut campaign)?;
        ensure!(campaign.manifest["binary_sha256"] == digest(EXECUTABLE.as_bytes()));
        prepare_resolver(&campaign)?;
        let bin = install_and_check_version(&mut campaign)?;
        let root = campaign.root.clone();
        let logs = fs::read_to_string(root.join("adapter.log"))?;
        ensure!(logs.lines().count() == 2 && !root.join("poison.log").exists());

        ensure!(SUBSTITUTE.len() == EXECUTABLE.len());
        executable(&bin, SUBSTITUTE)?;
        // Independent control establishes that the substitute reports the same
        // version. The real campaign caller must still refuse before a command.
        let output = Command::new(&bin).arg("--version").env_clear().output()?;
        ensure!(output.status.success() && output.stdout == b"ub-review 0.1.2\n");
        let before = campaign.rows.len();
        let error = campaign
            .cli(&root, &bin, &["--version"])
            .err()
            .context("same-version substitute was run")?;
        ensure!(error.to_string().contains("frozen binary digest mismatch"));
        ensure!(campaign.rows.len() == before);

        let receipt: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join("receipt.json"))?)?;
        ensure!(receipt["phase"] == "synthetic_candidate_intake");
        ensure!(receipt["distribution"] == "synthetic_fixture");
        Ok(())
    }

    #[test]
    fn synthetic_candidate_rejects_unbound_wrapper_and_inner_bytes() -> Result<()> {
        let fixture = Fixture::new()?;
        let path = &fixture.bytes;
        let bytes = fs::read(path)?;
        let mut changed = bytes.clone();
        changed[0] ^= 1;
        write(path, &changed)?;
        fixture.refusals_have_no_resolver_activity("ZIP digest mismatch")?;
        write(path, &bytes)?;

        let root = fixture.temp.path();
        let archive = fs::read(root.join(ASSET))?;
        let mut altered_archive = archive.clone();
        altered_archive[0] ^= 1;
        write(&root.join(ASSET), altered_archive)?;
        let refresh_bytes_wrapper = || -> Result<CandidateFacts> {
            fs::remove_file(path)?;
            Fixture::run(
                root,
                "/usr/bin/zip",
                &[
                    "-q",
                    "-0",
                    "bytes.zip",
                    ASSET,
                    CHECKSUM_ASSET,
                    RELEASE_NOTES,
                ],
            )?;
            let mut facts = fixture.facts()?;
            facts.bytes_artifact = Fixture::artifact(path, 41, "ub-review-linux-x64-release")?;
            write(&fixture.facts, serde_json::to_vec(&facts)?)?;
            Ok(facts)
        };
        refresh_bytes_wrapper()?;
        fixture.refusals_have_no_resolver_activity("ArchiveDigest")?;
        write(&root.join(ASSET), archive)?;

        let mut checksum = fs::read(root.join(CHECKSUM_ASSET))?;
        checksum[0] = if checksum[0] == b'0' { b'1' } else { b'0' };
        write(&root.join(CHECKSUM_ASSET), &checksum)?;
        fs::remove_file(path)?;
        Fixture::run(
            root,
            "/usr/bin/zip",
            &[
                "-q",
                "-0",
                "bytes.zip",
                ASSET,
                CHECKSUM_ASSET,
                RELEASE_NOTES,
            ],
        )?;
        let mut facts = fixture.facts()?;
        facts.bytes_artifact = Fixture::artifact(path, 41, "ub-review-linux-x64-release")?;
        write(&fixture.facts, serde_json::to_vec(&facts)?)?;
        fixture.refusals_have_no_resolver_activity("ChecksumDigest")?;

        let mut packet: Packet = serde_json::from_slice(&fs::read(root.join(PACKET))?)?;
        packet.checksum_sha256 = digest(&checksum);
        let raw = serde_json::to_vec(&packet)?;
        fixture.update_packet(&raw, &mut facts)?;
        facts.raw_packet_sha256 = digest(&raw);
        write(&fixture.facts, serde_json::to_vec(&facts)?)?;
        fixture.refusals_have_no_resolver_activity("ChecksumContents")
    }

    #[test]
    fn synthetic_candidate_receipt_requires_unambiguous_bytes_and_independent_identity()
    -> Result<()> {
        let fixture = Fixture::new()?;
        let original = fs::read(&fixture.facts)?;
        let mut facts = fixture.facts()?;
        facts.packet_artifact.run_attempt += 1;
        write(&fixture.facts, serde_json::to_vec(&facts)?)?;
        fixture.refusals_have_no_resolver_activity("artifact run or attempt mismatch")?;

        facts.producer.run_attempt += 1;
        facts.bytes_artifact.run_attempt += 1;
        write(&fixture.facts, serde_json::to_vec(&facts)?)?;
        fixture.refusals_have_no_resolver_activity("run_attempt")?;

        write(&fixture.facts, &original)?;
        let mut facts = fixture.facts()?;
        facts.packet_artifact.retained_until = NOW;
        write(&fixture.facts, serde_json::to_vec(&facts)?)?;
        fixture.refusals_have_no_resolver_activity("artifact identity or retention")?;

        write(&fixture.facts, &original)?;
        let mut facts = fixture.facts()?;
        let original_packet = fs::read(fixture.temp.path().join(PACKET))?;
        let mut repeated = b"{\"schema\":\"ub-review.authorized_release.v1\",".to_vec();
        repeated.extend_from_slice(&original_packet[1..]);
        fixture.update_packet(&repeated, &mut facts)?;
        write(&fixture.facts, serde_json::to_vec(&facts)?)?;
        fixture.refusals_have_no_resolver_activity("raw producer packet digest mismatch")?;
        facts.raw_packet_sha256 = digest(&repeated);
        write(&fixture.facts, serde_json::to_vec(&facts)?)?;
        fixture.refusals_have_no_resolver_activity("SchemaFields")
    }
}

#[path = "portability.rs"]
mod portability;
