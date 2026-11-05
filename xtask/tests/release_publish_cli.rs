//! End-to-end CLI proof for the #1344 publish-path commands: the
//! `release-packet-digest` digest derivation and the `validate-release`
//! admission decision run as real subprocesses, so the xtask command arms
//! and their trusted-fact plumbing are exercised outside the in-process
//! unit tests. No publish side effect is possible here: every case runs
//! without `--publish` or is expected to refuse.

use std::fs;
use std::io::Write as IoWrite;
use std::path::PathBuf;
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};
use flate2::{Compression, write::GzEncoder};
use xtask::release_bytes as rb;

const PACKET_SCHEMA: &str = "ub-review.authorized_release.v1";

fn run_xtask(args: &[String]) -> Result<Output> {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .output()
        .context("spawn xtask subprocess")
}

fn gzip(tar: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    encoder.write_all(tar)?;
    encoder.finish().map_err(Into::into)
}

fn header(name: &[u8], kind: u8, prefix: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 512];
    bytes[..name.len()].copy_from_slice(name);
    bytes[100..108].copy_from_slice(b"0000755\0");
    bytes[124..136].copy_from_slice(b"00000000004\0");
    bytes[148..156].fill(b' ');
    bytes[156] = kind;
    bytes[257..263].copy_from_slice(b"ustar\0");
    bytes[263..265].copy_from_slice(b"00");
    bytes[345..345 + prefix.len()].copy_from_slice(prefix);
    seal_header(&mut bytes);
    bytes
}

fn seal_header(bytes: &mut [u8]) {
    bytes[148..156].fill(b' ');
    let sum: u64 = bytes[..512].iter().map(|b| u64::from(*b)).sum();
    bytes[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
}

fn tar_bytes(name: &[u8], kind: u8, prefix: &[u8]) -> Vec<u8> {
    let mut bytes = header(name, kind, prefix);
    bytes.extend(b"ELF!");
    bytes.resize(2048, 0);
    bytes
}

struct Fixture {
    dir: PathBuf,
    packet_json: Vec<u8>,
    archive: Vec<u8>,
    facts_args: Vec<String>,
}

impl Fixture {
    /// Authorized fixture mirroring the #1638 golden shape: packet, archive
    /// and checksum agree, and the trusted-fact arguments match the packet.
    fn create(slug: &str) -> Result<Self> {
        let dir = std::env::temp_dir().join(format!(
            "ub-review-release-publish-cli-{slug}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)?
                .as_nanos()
        ));
        fs::create_dir_all(&dir)?;
        let archive = gzip(&tar_bytes(b"ub-review", b'0', b""))?;
        let checksum = format!("{}  dist/{}\n", rb::sha256(&archive), rb::ASSET).into_bytes();
        let packet = rb::Packet {
            schema: PACKET_SCHEMA.into(),
            repository: "EffortlessMetrics/ub-review".into(),
            head_sha: "a".repeat(40),
            tree_sha: "b".repeat(40),
            workflow: ".github/workflows/release-binary.yml".into(),
            producer_ref: "refs/heads/prepared".into(),
            run_id: 100,
            run_attempt: 2,
            artifact_id: 200,
            artifact_name: "ub-review-linux-x64-release".into(),
            proposed_tag: "v0.1.3".into(),
            peeled_commit: "a".repeat(40),
            asset: rb::ASSET.into(),
            checksum_asset: rb::CHECKSUM_ASSET.into(),
            archive_length: archive.len() as u64,
            checksum_length: checksum.len() as u64,
            archive_sha256: rb::sha256(&archive),
            checksum_sha256: rb::sha256(&checksum),
            binary_version: "0.1.3".into(),
            package_version: "0.1.3".into(),
            target: "x86_64-unknown-linux-gnu".into(),
            toolchain: "1.95.0".into(),
            retained_until: 500,
            authorization_record: "release-authority/817/record-7".into(),
            verifier: "trusted-release-verifier/v1".into(),
        };
        let packet_json = serde_json::to_vec(&packet)?;
        fs::write(dir.join("authorized-release-packet.json"), &packet_json)?;
        fs::write(dir.join(rb::ASSET), &archive)?;
        fs::write(dir.join(rb::CHECKSUM_ASSET), &checksum)?;
        fs::write(dir.join("release-notes.md"), "# ub-review v0.1.3\n")?;
        let digest = rb::packet_digest(&packet)
            .map_err(|reason| anyhow::anyhow!("fixture packet digest rejected: {reason:?}"))?;
        let facts_args = vec![
            "--staging".to_owned(),
            dir.display().to_string(),
            "--repository".to_owned(),
            packet.repository.clone(),
            "--workflow".to_owned(),
            packet.workflow.clone(),
            "--head-sha".to_owned(),
            packet.head_sha.clone(),
            "--tree-sha".to_owned(),
            packet.tree_sha.clone(),
            "--producer-ref".to_owned(),
            packet.producer_ref.clone(),
            "--run-id".to_owned(),
            "100".to_owned(),
            "--run-attempt".to_owned(),
            "2".to_owned(),
            "--artifact-id".to_owned(),
            "200".to_owned(),
            "--artifact-name".to_owned(),
            packet.artifact_name.clone(),
            "--retained-until".to_owned(),
            "500".to_owned(),
            "--proposed-tag".to_owned(),
            packet.proposed_tag.clone(),
            "--package-version".to_owned(),
            packet.package_version.clone(),
            "--authorization-record".to_owned(),
            packet.authorization_record.clone(),
            "--authorization-verifier".to_owned(),
            packet.verifier.clone(),
            "--packet-sha256".to_owned(),
            digest.clone(),
            "--authorization-expires-at".to_owned(),
            "600".to_owned(),
            "--now".to_owned(),
            "100".to_owned(),
        ];
        Ok(Self {
            dir,
            packet_json,
            archive,
            facts_args,
        })
    }

    fn cleanup(&self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn packet_digest_command_matches_the_lib_derived_authorization_digest() -> Result<()> {
    let fixture = Fixture::create("digest")?;
    let packet_path = fixture.dir.join("authorized-release-packet.json");
    let output = run_xtask(&[
        "release-packet-digest".to_owned(),
        "--packet".to_owned(),
        packet_path.display().to_string(),
    ])?;
    if !output.status.success() {
        bail!(
            "release-packet-digest failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let expected = rb::packet_digest(
        &rb::parse_packet(&fixture.packet_json)
            .map_err(|reason| anyhow::anyhow!("fixture packet rejected: {reason:?}"))?,
    )
    .map_err(|reason| anyhow::anyhow!("digest: {reason:?}"))?;
    anyhow::ensure!(
        stdout.contains(&format!("\"packet_sha256\":\"{expected}\"")),
        "digest receipt must name the lib-derived digest {expected}, got: {stdout}"
    );
    fixture.cleanup();
    Ok(())
}

#[test]
fn validate_release_subprocess_admits_and_then_refuses_tampered_facts() -> Result<()> {
    let fixture = Fixture::create("cli")?;
    // Dry-run admission (no --publish): decision receipt with zero side
    // effects and the byte digests of the staged files.
    let mut admit_args = vec!["validate-release".to_owned()];
    admit_args.extend(fixture.facts_args.iter().cloned());
    let output = run_xtask(&admit_args)?;
    if !output.status.success() {
        bail!(
            "validate-release refused an authorized fixture: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    anyhow::ensure!(
        stdout.contains("\"decision\":\"publish\"") && stdout.contains("\"side_effects\":[]"),
        "dry-run receipt must publish-decide with zero side effects, got: {stdout}"
    );
    anyhow::ensure!(
        stdout.contains(&format!(
            "\"archive_sha256\":\"{}\"",
            rb::sha256(&fixture.archive)
        )),
        "receipt must name the staged archive digest, got: {stdout}"
    );
    // A wrong trusted fact refuses the same staged bytes via the CLI.
    let head_index = admit_args
        .iter()
        .position(|arg| arg == "--head-sha")
        .context("--head-sha flag present")?;
    let mut wrong = admit_args.clone();
    wrong[head_index + 1] = "c".repeat(40);
    let refused = run_xtask(&wrong)?;
    anyhow::ensure!(
        !refused.status.success(),
        "wrong head sha must fail the CLI run"
    );
    let refused_out = String::from_utf8_lossy(&refused.stdout);
    anyhow::ensure!(
        refused_out.contains("\"decision\":\"refuse\"")
            && refused_out.contains(r#"Identity(\"head_sha\")"#),
        "refusal receipt must name the rejection, got: {refused_out}"
    );
    fixture.cleanup();
    Ok(())
}
