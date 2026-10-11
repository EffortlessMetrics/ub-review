//! Versioned, offline admission for exact prepared release bytes.
//!
//! A future trusted adapter must obtain producer/artifact facts and a successful
//! unused-tag lookup independently, and authenticate a release authority record
//! outside candidate-controlled files. It must verify that record's issuer,
//! scope, expiry, packet digest and verifier identity before constructing
//! `Authority::Trusted`, including independent evidence for binary/package
//! version and target/toolchain provenance. This validator binds those claims
//! to authorized bytes; it does not infer them by executing the binary.
//! API errors are `Observation::Failed`, never absence.
//! Publish only the owned slices in `VerifiedPublication`; reopening paths or
//! rebuilding binaries discards this proof. At the side-effect boundary the
//! publisher must call `ensure_current` with its trusted current time before
//! consuming these slices. Tag absence is a point-in-time observation, not a
//! reservation: the next integration slice must recheck tag collisions directly
//! before side effects; `ensure_current` checks only artifact/authority expiry.
//! This module performs no side effects.
use flate2::bufread::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Cursor, Read};

pub const SCHEMA: &str = "ub-review.authorized_release.v1";
pub const ASSET: &str = "ub-review-x86_64-unknown-linux-gnu.tar.gz";
pub const CHECKSUM_ASSET: &str = "ub-review-x86_64-unknown-linux-gnu.tar.gz.sha256";
pub const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_PACKET_BYTES: usize = 16 * 1024;
const MAX_TAR_BYTES: usize = MAX_ARCHIVE_BYTES + 4096;

/// Explicit new schema: the existing release_candidate.v1 producer is unchanged.
/// Declaration-order serialization supplies the canonical semantic digest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    pub schema: String,
    pub repository: String,
    pub head_sha: String,
    pub tree_sha: String,
    pub workflow: String,
    pub producer_ref: String,
    pub run_id: u64,
    pub run_attempt: u64,
    pub artifact_id: u64,
    pub artifact_name: String,
    pub proposed_tag: String,
    pub peeled_commit: String,
    pub asset: String,
    pub checksum_asset: String,
    pub archive_length: u64,
    pub checksum_length: u64,
    pub archive_sha256: String,
    pub checksum_sha256: String,
    pub binary_version: String,
    pub package_version: String,
    pub target: String,
    pub toolchain: String,
    pub retained_until: u64,
    pub authorization_record: String,
    pub verifier: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Producer {
    pub repository: String,
    pub head_sha: String,
    pub tree_sha: String,
    pub workflow: String,
    pub producer_ref: String,
    pub run_id: u64,
    pub run_attempt: u64,
    pub artifact_id: u64,
    pub artifact_name: String,
    pub retained_until: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnusedTag {
    pub repository: String,
    pub tag: String,
    /// Authorized expected peel for the proposed tag, which does not exist yet.
    /// Success represents a successful API observation proving tag absence.
    pub peeled_commit: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Observation<T> {
    Failed,
    Success(T),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Authorization {
    pub record: String,
    pub packet_sha256: String,
    pub verifier: String,
    pub expires_at: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Authority {
    Untrusted,
    Trusted(Authorization),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Rejection {
    PacketSize,
    SchemaFields,
    UnsupportedSchema,
    MalformedIdentity,
    Identity(&'static str),
    AvailabilityUnknown,
    TagAbsenceUnknown,
    Expired,
    UntrustedAuthority,
    AuthorizationMismatch,
    ArchiveLength,
    ArchiveDigest,
    ChecksumLength,
    ChecksumDigest,
    ChecksumContents,
    ArchiveEncoding,
    ArchiveLayout,
    ArchiveBounds,
}

/// Fields and captured bytes are private and cannot be replaced after admission.
#[derive(Debug, PartialEq, Eq)]
pub struct VerifiedPublication {
    packet: Packet,
    packet_sha256: String,
    archive: Box<[u8]>,
    checksum: Box<[u8]>,
    validated_authority: Authorization,
}
impl VerifiedPublication {
    pub fn packet(&self) -> &Packet {
        &self.packet
    }
    pub fn packet_sha256(&self) -> &str {
        &self.packet_sha256
    }
    pub fn archive(&self) -> &[u8] {
        &self.archive
    }
    pub fn checksum(&self) -> &[u8] {
        &self.checksum
    }
    pub fn authorization(&self) -> &Authorization {
        &self.validated_authority
    }
    pub fn valid_until(&self) -> u64 {
        self.packet
            .retained_until
            .min(self.validated_authority.expires_at)
    }
    pub fn ensure_current(&self, now: u64) -> Result<(), Rejection> {
        if now >= self.valid_until() {
            Err(Rejection::Expired)
        } else {
            Ok(())
        }
    }
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
pub fn packet_digest(packet: &Packet) -> Result<String, Rejection> {
    serde_json::to_vec(packet)
        .map(|bytes| sha256(&bytes))
        .map_err(|_| Rejection::SchemaFields)
}
pub fn parse_packet(bytes: &[u8]) -> Result<Packet, Rejection> {
    if bytes.len() > MAX_PACKET_BYTES {
        return Err(Rejection::PacketSize);
    }
    let packet: Packet = serde_json::from_slice(bytes).map_err(|_| Rejection::SchemaFields)?;
    if packet.schema != SCHEMA {
        return Err(Rejection::UnsupportedSchema);
    }
    Ok(packet)
}
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn safe_token(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

pub fn validate(
    packet_bytes: &[u8],
    archive: &[u8],
    checksum: &[u8],
    producer: &Observation<Producer>,
    tag: &Observation<UnusedTag>,
    authority: &Authority,
    now: u64,
) -> Result<VerifiedPublication, Rejection> {
    let packet = parse_packet(packet_bytes)?;
    if !hex(&packet.head_sha, 40)
        || !hex(&packet.tree_sha, 40)
        || !hex(&packet.peeled_commit, 40)
        || !hex(&packet.archive_sha256, 64)
        || !hex(&packet.checksum_sha256, 64)
        || packet.run_id == 0
        || packet.run_attempt == 0
        || packet.artifact_id == 0
        || packet.repository.split('/').count() != 2
        || [
            &packet.repository,
            &packet.workflow,
            &packet.producer_ref,
            &packet.artifact_name,
            &packet.authorization_record,
            &packet.verifier,
        ]
        .iter()
        .any(|value| !safe_token(value))
    {
        return Err(Rejection::MalformedIdentity);
    }
    if packet.asset != ASSET {
        return Err(Rejection::Identity("asset"));
    }
    if packet.checksum_asset != CHECKSUM_ASSET {
        return Err(Rejection::Identity("checksum_asset"));
    }
    if packet.target != "x86_64-unknown-linux-gnu" {
        return Err(Rejection::Identity("target"));
    }
    if packet.toolchain != "1.95.0" {
        return Err(Rejection::Identity("toolchain"));
    }
    if packet.binary_version != packet.package_version || !valid_version(&packet.package_version) {
        return Err(Rejection::Identity("version"));
    }
    if packet.proposed_tag != format!("v{}", packet.package_version) {
        return Err(Rejection::Identity("tag"));
    }
    if packet.peeled_commit != packet.head_sha {
        return Err(Rejection::Identity("peeled_commit"));
    }
    if !packet.producer_ref.starts_with("refs/heads/")
        && !packet.producer_ref.starts_with("refs/tags/")
    {
        return Err(Rejection::Identity("producer_ref"));
    }
    if packet.producer_ref.starts_with("refs/tags/")
        && packet.producer_ref != format!("refs/tags/{}", packet.proposed_tag)
    {
        return Err(Rejection::Identity("producer_ref"));
    }
    let Observation::Success(source) = producer else {
        return Err(Rejection::AvailabilityUnknown);
    };
    macro_rules! bind {
        ($field:ident) => {
            if packet.$field != source.$field {
                return Err(Rejection::Identity(stringify!($field)));
            }
        };
    }
    bind!(repository);
    bind!(head_sha);
    bind!(tree_sha);
    bind!(workflow);
    bind!(producer_ref);
    bind!(run_id);
    bind!(run_attempt);
    bind!(artifact_id);
    bind!(artifact_name);
    bind!(retained_until);
    if packet.retained_until <= now {
        return Err(Rejection::Expired);
    }
    let Observation::Success(unused) = tag else {
        return Err(Rejection::TagAbsenceUnknown);
    };
    if unused.repository != packet.repository
        || unused.tag != packet.proposed_tag
        || unused.peeled_commit != packet.peeled_commit
    {
        return Err(Rejection::Identity("unused_tag"));
    }
    let Authority::Trusted(auth) = authority else {
        return Err(Rejection::UntrustedAuthority);
    };
    if auth.expires_at <= now {
        return Err(Rejection::Expired);
    }
    let digest = packet_digest(&packet)?;
    if auth.record != packet.authorization_record
        || auth.verifier != packet.verifier
        || !hex(&auth.packet_sha256, 64)
        || auth.packet_sha256 != digest
    {
        return Err(Rejection::AuthorizationMismatch);
    }
    if archive.len() > MAX_ARCHIVE_BYTES || archive.len() as u64 != packet.archive_length {
        return Err(Rejection::ArchiveLength);
    }
    if sha256(archive) != packet.archive_sha256 {
        return Err(Rejection::ArchiveDigest);
    }
    if checksum.len() > 512 || checksum.len() as u64 != packet.checksum_length {
        return Err(Rejection::ChecksumLength);
    }
    if sha256(checksum) != packet.checksum_sha256 {
        return Err(Rejection::ChecksumDigest);
    }
    let expected = format!("{}  {}\n", packet.archive_sha256, packet.asset);
    // The unchanged v1 producer writes sha256sum's fixed dist/ prefix.
    let producer_checksum = format!("{}  dist/{}\n", packet.archive_sha256, packet.asset);
    if checksum != expected.as_bytes() && checksum != producer_checksum.as_bytes() {
        return Err(Rejection::ChecksumContents);
    }
    inventory(archive)?;
    Ok(VerifiedPublication {
        packet,
        packet_sha256: digest,
        archive: archive.into(),
        checksum: checksum.into(),
        validated_authority: auth.clone(),
    })
}
fn valid_version(value: &str) -> bool {
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.bytes().all(|b| b.is_ascii_digit())
                && (part.len() == 1 || !part.starts_with('0'))
        })
}

/// Fixed release archive layout: one nonempty regular `ub-review` at archive root.
/// Reject extensions, directories, links, duplicates, traversal and concatenation.
fn inventory(archive: &[u8]) -> Result<(), Rejection> {
    let mut decoder = GzDecoder::new(Cursor::new(archive));
    let mut tar = Vec::new();
    decoder
        .by_ref()
        .take((MAX_TAR_BYTES + 1) as u64)
        .read_to_end(&mut tar)
        .map_err(|_| Rejection::ArchiveEncoding)?;
    if tar.len() > MAX_TAR_BYTES {
        return Err(Rejection::ArchiveBounds);
    }
    if decoder.into_inner().position() as usize != archive.len() {
        return Err(Rejection::ArchiveEncoding);
    }
    if tar.len() < 1536 || tar.len() % 512 != 0 {
        return Err(Rejection::ArchiveLayout);
    }
    let header = &tar[..512];
    if cstring(&header[..100]) != Some(b"ub-review".as_slice())
        || cstring(&header[345..500]) != Some(b"".as_slice())
        || cstring(&header[157..257]) != Some(b"".as_slice())
        || !matches!(header[156], 0 | b'0')
        || !matches!(&header[257..265], b"ustar\x0000" | b"ustar  \0")
    {
        return Err(Rejection::ArchiveLayout);
    }
    let recorded = octal(&header[148..156]).ok_or(Rejection::ArchiveEncoding)?;
    let actual: u64 = header
        .iter()
        .enumerate()
        .map(|(i, b)| {
            if (148..156).contains(&i) {
                32
            } else {
                u64::from(*b)
            }
        })
        .sum();
    if actual != recorded {
        return Err(Rejection::ArchiveEncoding);
    }
    if octal(&header[100..108]) != Some(0o755) {
        return Err(Rejection::ArchiveLayout);
    }
    let size = octal(&header[124..136]).ok_or(Rejection::ArchiveEncoding)?;
    if size == 0 || size > MAX_ARCHIVE_BYTES as u64 {
        return Err(Rejection::ArchiveBounds);
    }
    let end = 512 + (size as usize).div_ceil(512) * 512;
    if end + 1024 > tar.len()
        || tar[end..].iter().any(|byte| *byte != 0)
        || tar[512 + size as usize..end].iter().any(|byte| *byte != 0)
    {
        return Err(Rejection::ArchiveLayout);
    }
    Ok(())
}
fn cstring(bytes: &[u8]) -> Option<&[u8]> {
    let end = bytes
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(bytes.len());
    if bytes[end..].iter().any(|byte| *byte != 0) {
        return None;
    }
    Some(&bytes[..end])
}
fn octal(bytes: &[u8]) -> Option<u64> {
    let value = std::str::from_utf8(bytes).ok()?.trim_matches(['\0', ' ']);
    if value.is_empty() || !value.bytes().all(|b| (b'0'..=b'7').contains(&b)) {
        return None;
    }
    u64::from_str_radix(value, 8).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::{Compression, write::GzEncoder};
    use std::io::Write;

    struct Fixture {
        packet: Packet,
        archive: Vec<u8>,
        checksum: Vec<u8>,
        producer: Observation<Producer>,
        tag: Observation<UnusedTag>,
        authority: Authority,
    }
    fn gzip(tar: &[u8]) -> std::io::Result<Vec<u8>> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(tar)?;
        encoder.finish()
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
    fn tar(name: &[u8], kind: u8, prefix: &[u8]) -> Vec<u8> {
        let mut bytes = header(name, kind, prefix);
        bytes.extend(b"ELF!");
        bytes.resize(2048, 0);
        bytes
    }
    impl Fixture {
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let archive = gzip(&tar(b"ub-review", b'0', b""))?;
            let checksum = format!("{}  dist/{ASSET}\n", sha256(&archive)).into_bytes();
            let packet = Packet {
                schema: SCHEMA.into(),
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
                asset: ASSET.into(),
                checksum_asset: CHECKSUM_ASSET.into(),
                archive_length: archive.len() as u64,
                checksum_length: checksum.len() as u64,
                archive_sha256: sha256(&archive),
                checksum_sha256: sha256(&checksum),
                binary_version: "0.1.3".into(),
                package_version: "0.1.3".into(),
                target: "x86_64-unknown-linux-gnu".into(),
                toolchain: "1.95.0".into(),
                retained_until: 500,
                authorization_record: "release-authority/817/record-7".into(),
                verifier: "trusted-release-verifier/v1".into(),
            };
            let producer = Observation::Success(Producer {
                repository: packet.repository.clone(),
                head_sha: packet.head_sha.clone(),
                tree_sha: packet.tree_sha.clone(),
                workflow: packet.workflow.clone(),
                producer_ref: packet.producer_ref.clone(),
                run_id: packet.run_id,
                run_attempt: packet.run_attempt,
                artifact_id: packet.artifact_id,
                artifact_name: packet.artifact_name.clone(),
                retained_until: packet.retained_until,
            });
            let tag = Observation::Success(UnusedTag {
                repository: packet.repository.clone(),
                tag: packet.proposed_tag.clone(),
                peeled_commit: packet.peeled_commit.clone(),
            });
            let mut fixture = Self {
                packet,
                archive,
                checksum,
                producer,
                tag,
                authority: Authority::Untrusted,
            };
            fixture.authorize();
            Ok(fixture)
        }
        fn authorize(&mut self) {
            self.authority = match packet_digest(&self.packet) {
                Ok(digest) => Authority::Trusted(Authorization {
                    record: self.packet.authorization_record.clone(),
                    packet_sha256: digest,
                    verifier: self.packet.verifier.clone(),
                    expires_at: 600,
                }),
                Err(_) => Authority::Untrusted,
            };
        }
        fn replace_archive(&mut self, archive: Vec<u8>) {
            self.archive = archive;
            self.checksum = format!("{}  {ASSET}\n", sha256(&self.archive)).into_bytes();
            self.packet.archive_length = self.archive.len() as u64;
            self.packet.archive_sha256 = sha256(&self.archive);
            self.packet.checksum_length = self.checksum.len() as u64;
            self.packet.checksum_sha256 = sha256(&self.checksum);
            self.authorize();
        }
        fn admit(&self) -> Result<VerifiedPublication, Rejection> {
            let bytes = serde_json::to_vec(&self.packet).map_err(|_| Rejection::SchemaFields)?;
            validate(
                &bytes,
                &self.archive,
                &self.checksum,
                &self.producer,
                &self.tag,
                &self.authority,
                100,
            )
        }
    }
    #[derive(Default)]
    struct RecordingPublisher {
        calls: Vec<String>,
    }
    impl RecordingPublisher {
        fn consume(
            &mut self,
            input: Result<VerifiedPublication, Rejection>,
        ) -> Result<(), Rejection> {
            let verified = input?;
            verified.ensure_current(100)?;
            self.calls.push(sha256(verified.archive()));
            Ok(())
        }
    }

    #[test]
    fn canonical_digest_has_a_frozen_independent_golden_vector()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut f = Fixture::new()?;
        f.packet.archive_length = 123;
        f.packet.checksum_length = 456;
        f.packet.archive_sha256 = "c".repeat(64);
        f.packet.checksum_sha256 = "d".repeat(64);
        // Independently calculated with Python hashlib over an explicitly
        // ordered JSON object, separators=(',', ':'), no whitespace/newline.
        assert_eq!(
            packet_digest(&f.packet),
            Ok("248425e2ec28491a9257dc2dfad55a9ea4e709b84879601f7b949f93545736b4".into())
        );
        Ok(())
    }

    #[test]
    fn expiry_is_rechecked_at_publication_boundary() -> Result<(), Box<dyn std::error::Error>> {
        let mut f = Fixture::new()?;
        if let Authority::Trusted(auth) = &mut f.authority {
            auth.expires_at = 150;
        }
        let verified = f.admit().map_err(|_| "admit")?;
        assert_eq!(verified.valid_until(), 150);
        assert_eq!(verified.authorization().verifier, f.packet.verifier);
        assert_eq!(verified.ensure_current(149), Ok(()));
        let mut publisher = RecordingPublisher::default();
        let at_boundary = verified.ensure_current(150).map(|()| verified);
        assert_eq!(publisher.consume(at_boundary), Err(Rejection::Expired));
        assert!(publisher.calls.is_empty());
        Ok(())
    }

    #[test]
    fn actual_gnu_producer_format_is_accepted() -> Result<(), Box<dyn std::error::Error>> {
        // Actual GNU tar fixture captured with the unchanged producer command:
        // tar -czf ASSET -C dist/TARGET_DIR ub-review (mode 0755, four bytes).
        // Archive contents are never executed; these bytes remain offline.
        let real_archive: &[u8] = &[
            31, 139, 8, 0, 0, 0, 0, 0, 0, 3, 237, 206, 49, 10, 194, 64, 20, 132, 225, 119, 148,
            245, 0, 194, 123, 49, 111, 247, 4, 73, 229, 37, 54, 176, 4, 27, 139, 152, 152, 235,
            171, 32, 146, 74, 177, 8, 33, 240, 127, 205, 20, 51, 197, 76, 221, 113, 40, 247, 75,
            153, 101, 61, 250, 148, 220, 95, 105, 201, 117, 153, 111, 181, 152, 87, 177, 138, 234,
            181, 159, 68, 205, 98, 50, 9, 186, 226, 167, 143, 233, 54, 230, 33, 4, 201, 125, 185,
            142, 95, 118, 191, 250, 157, 106, 206, 237, 97, 235, 15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 128, 255, 61, 0, 196, 47, 128, 40, 0, 40, 0, 0,
        ];
        let mut real = Fixture::new()?;
        real.replace_archive(real_archive.to_vec());
        assert!(real.admit().is_ok());
        // Mirror GNU tar's ustar-space version and 20-block record padding.
        let mut bytes = tar(b"ub-review", b'0', b"");
        bytes[257..263].copy_from_slice(b"ustar ");
        bytes[263..265].copy_from_slice(b" \0");
        bytes.resize(10240, 0);
        seal_header(&mut bytes);
        let mut f = Fixture::new()?;
        f.replace_archive(gzip(&bytes)?);
        // Unchanged workflow's sha256sum command writes the fixed dist/ prefix.
        f.checksum = format!("{}  dist/{ASSET}\n", sha256(&f.archive)).into_bytes();
        f.packet.checksum_length = f.checksum.len() as u64;
        f.packet.checksum_sha256 = sha256(&f.checksum);
        f.authorize();
        assert!(f.admit().is_ok());
        bytes[263..265].copy_from_slice(b"99");
        seal_header(&mut bytes);
        f.replace_archive(gzip(&bytes)?);
        rejected(f.admit(), Rejection::ArchiveLayout);
        bytes[263..265].copy_from_slice(b"00");
        seal_header(&mut bytes);
        f.replace_archive(gzip(&bytes)?);
        rejected(f.admit(), Rejection::ArchiveLayout);
        Ok(())
    }
    fn rejected(result: Result<VerifiedPublication, Rejection>, expected: Rejection) {
        assert_eq!(result.as_ref().err(), Some(&expected));
        let mut publisher = RecordingPublisher::default();
        assert_eq!(publisher.consume(result), Err(expected));
        assert!(publisher.calls.is_empty());
    }

    #[test]
    fn acceptance_captures_private_bytes_and_deterministic_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut fixture = Fixture::new()?;
        let first = fixture.admit();
        let second = fixture.admit();
        assert_eq!(first, second);
        if let Ok(verified) = first {
            assert_eq!(verified.archive(), fixture.archive);
            assert_eq!(verified.checksum(), fixture.checksum);
            let old_archive = verified.archive().to_vec();
            let old_checksum = verified.checksum().to_vec();
            fixture.archive[0] ^= 1;
            fixture.checksum[0] ^= 1;
            assert_eq!(verified.archive(), old_archive);
            assert_eq!(verified.checksum(), old_checksum);
            assert_eq!(verified.packet(), &fixture.packet);
            assert_eq!(
                verified.packet_sha256(),
                packet_digest(&fixture.packet).map_err(|_| "digest")?
            );
            let mut publisher = RecordingPublisher::default();
            assert_eq!(publisher.consume(Ok(verified)), Ok(()));
            assert_eq!(publisher.calls, vec![sha256(&old_archive)]);
            rejected(fixture.admit(), Rejection::ArchiveDigest);
        } else {
            return Err("matching authorized packet rejected".into());
        }
        Ok(())
    }
    #[test]
    fn every_required_field_is_strict_and_input_order_is_semantic()
    -> Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new()?;
        let value = serde_json::to_value(&fixture.packet)?;
        let Some(fields) = value.as_object() else {
            return Err("not object".into());
        };
        for name in fields.keys() {
            let mut missing = fields.clone();
            missing.remove(name);
            rejected(
                validate(
                    &serde_json::to_vec(&missing)?,
                    &fixture.archive,
                    &fixture.checksum,
                    &fixture.producer,
                    &fixture.tag,
                    &fixture.authority,
                    100,
                ),
                Rejection::SchemaFields,
            );
            let original = serde_json::to_string(&fixture.packet)?;
            let duplicate = format!("{{\"{name}\":{},{}", fields[name], &original[1..]);
            rejected(
                validate(
                    duplicate.as_bytes(),
                    &fixture.archive,
                    &fixture.checksum,
                    &fixture.producer,
                    &fixture.tag,
                    &fixture.authority,
                    100,
                ),
                Rejection::SchemaFields,
            );
        }
        let mut unknown = fields.clone();
        unknown.insert("authorize_myself".into(), serde_json::json!(true));
        assert_eq!(
            parse_packet(&serde_json::to_vec(&unknown)?),
            Err(Rejection::SchemaFields)
        );
        let reverse = format!(
            "{{{}}}",
            fields
                .iter()
                .rev()
                .map(|(k, v)| format!("\"{k}\":{v}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let reordered = parse_packet(reverse.as_bytes()).map_err(|_| "parse")?;
        assert_eq!(reordered, fixture.packet);
        assert_eq!(packet_digest(&reordered), packet_digest(&fixture.packet));
        let mut unsupported = fixture.packet.clone();
        unsupported.schema = "ub-review.release_candidate.v1".into();
        assert_eq!(
            parse_packet(&serde_json::to_vec(&unsupported)?),
            Err(Rejection::UnsupportedSchema)
        );
        let mut malformed = fields.clone();
        malformed.insert("run_attempt".into(), serde_json::json!("2"));
        assert_eq!(
            parse_packet(&serde_json::to_vec(&malformed)?),
            Err(Rejection::SchemaFields)
        );
        assert_eq!(
            parse_packet(&vec![b' '; MAX_PACKET_BYTES + 1]),
            Err(Rejection::PacketSize)
        );
        Ok(())
    }
    #[test]
    fn individual_identity_mutations_have_stable_reasons() -> Result<(), Box<dyn std::error::Error>>
    {
        type Mutation = fn(&mut Packet);
        let cases: &[(Mutation, Rejection)] = &[
            (
                |p| p.repository = "other/repo".into(),
                Rejection::Identity("repository"),
            ),
            (
                |p| p.head_sha = "c".repeat(40),
                Rejection::Identity("peeled_commit"),
            ),
            (
                |p| p.tree_sha = "c".repeat(40),
                Rejection::Identity("tree_sha"),
            ),
            (
                |p| p.workflow = ".github/workflows/other.yml".into(),
                Rejection::Identity("workflow"),
            ),
            (
                |p| p.producer_ref = "refs/heads/other".into(),
                Rejection::Identity("producer_ref"),
            ),
            (|p| p.run_id += 1, Rejection::Identity("run_id")),
            (|p| p.run_attempt += 1, Rejection::Identity("run_attempt")),
            (|p| p.artifact_id += 1, Rejection::Identity("artifact_id")),
            (
                |p| p.artifact_name = "other".into(),
                Rejection::Identity("artifact_name"),
            ),
            (
                |p| p.proposed_tag = "v0.1.4".into(),
                Rejection::Identity("tag"),
            ),
            (
                |p| p.peeled_commit = "c".repeat(40),
                Rejection::Identity("peeled_commit"),
            ),
            (
                |p| p.asset = "../asset".into(),
                Rejection::Identity("asset"),
            ),
            (
                |p| p.checksum_asset = "other".into(),
                Rejection::Identity("checksum_asset"),
            ),
            (
                |p| p.binary_version = "0.1.4".into(),
                Rejection::Identity("version"),
            ),
            (
                |p| p.package_version = "0.1.4".into(),
                Rejection::Identity("version"),
            ),
            (
                |p| p.target = "aarch64-unknown-linux-gnu".into(),
                Rejection::Identity("target"),
            ),
            (
                |p| p.toolchain = "1.94.0".into(),
                Rejection::Identity("toolchain"),
            ),
            (
                |p| p.retained_until += 1,
                Rejection::Identity("retained_until"),
            ),
            (
                |p| p.authorization_record = "other-record".into(),
                Rejection::AuthorizationMismatch,
            ),
            (
                |p| p.verifier = "other-verifier".into(),
                Rejection::AuthorizationMismatch,
            ),
            (|p| p.archive_length += 1, Rejection::ArchiveLength),
            (|p| p.checksum_length += 1, Rejection::ChecksumLength),
            (
                |p| p.archive_sha256 = "c".repeat(64),
                Rejection::ArchiveDigest,
            ),
            (
                |p| p.checksum_sha256 = "c".repeat(64),
                Rejection::ChecksumDigest,
            ),
        ];
        for (mutate, expected) in cases {
            let mut fixture = Fixture::new()?;
            mutate(&mut fixture.packet);
            // Trusted independent authorization for byte mutations isolates the
            // digest/length proof rather than failing earlier at packet binding.
            if matches!(
                expected,
                Rejection::ArchiveLength
                    | Rejection::ChecksumLength
                    | Rejection::ArchiveDigest
                    | Rejection::ChecksumDigest
            ) {
                fixture.authorize();
            }
            rejected(fixture.admit(), expected.clone());
        }
        Ok(())
    }
    #[test]
    fn unsuccessful_observations_and_self_authorization_never_publish()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut f = Fixture::new()?;
        f.packet.run_attempt += 1;
        if let Observation::Success(source) = &mut f.producer {
            source.run_attempt += 1;
        }
        // Legitimate later-attempt source evidence cannot reuse older approval.
        rejected(f.admit(), Rejection::AuthorizationMismatch);
        let mut f = Fixture::new()?;
        f.producer = Observation::Failed;
        rejected(f.admit(), Rejection::AvailabilityUnknown);
        let mut f = Fixture::new()?;
        f.tag = Observation::Failed;
        rejected(f.admit(), Rejection::TagAbsenceUnknown);
        let mut f = Fixture::new()?;
        f.authority = Authority::Untrusted;
        rejected(f.admit(), Rejection::UntrustedAuthority);
        let mut f = Fixture::new()?;
        if let Authority::Trusted(auth) = &mut f.authority {
            auth.packet_sha256 = "0".repeat(64);
        }
        rejected(f.admit(), Rejection::AuthorizationMismatch);
        let mut f = Fixture::new()?;
        if let Authority::Trusted(auth) = &mut f.authority {
            auth.expires_at = 100;
        }
        rejected(f.admit(), Rejection::Expired);
        let mut f = Fixture::new()?;
        f.packet.retained_until = 100;
        if let Observation::Success(producer) = &mut f.producer {
            producer.retained_until = 100;
        }
        rejected(f.admit(), Rejection::Expired);
        let mut f = Fixture::new()?;
        if let Observation::Success(tag) = &mut f.tag {
            tag.tag = "v0.1.4".into();
        }
        rejected(f.admit(), Rejection::Identity("unused_tag"));
        Ok(())
    }
    #[test]
    fn archive_negative_corpus_is_bounded_without_execution()
    -> Result<(), Box<dyn std::error::Error>> {
        for (name, kind, prefix) in [
            (b"../ub-review".as_slice(), b'0', b"".as_slice()),
            (b"/ub-review", b'0', b""),
            (b"ub-review", b'2', b""),
            (b"ub-review", b'1', b""),
            (b"ub-review", b'5', b""),
            (b"ub-review", b'0', b"root"),
            (b"ub-review", b'x', b""),
        ] {
            let mut f = Fixture::new()?;
            f.replace_archive(gzip(&tar(name, kind, prefix))?);
            rejected(f.admit(), Rejection::ArchiveLayout);
        }
        let mut duplicate = tar(b"ub-review", b'0', b"");
        duplicate.truncate(1024);
        duplicate.extend(tar(b"ub-review", b'0', b""));
        let mut f = Fixture::new()?;
        f.replace_archive(gzip(&duplicate)?);
        rejected(f.admit(), Rejection::ArchiveLayout);
        let mut link = tar(b"ub-review", b'0', b"");
        link[157] = b'x';
        seal_header(&mut link);
        let mut f = Fixture::new()?;
        f.replace_archive(gzip(&link)?);
        rejected(f.admit(), Rejection::ArchiveLayout);
        let mut bad_mode = tar(b"ub-review", b'0', b"");
        bad_mode[105] = b'4';
        seal_header(&mut bad_mode);
        let mut f = Fixture::new()?;
        f.replace_archive(gzip(&bad_mode)?);
        rejected(f.admit(), Rejection::ArchiveLayout);
        let mut bad_header = tar(b"ub-review", b'0', b"");
        bad_header[136] ^= 1;
        let mut f = Fixture::new()?;
        f.replace_archive(gzip(&bad_header)?);
        rejected(f.admit(), Rejection::ArchiveEncoding);
        let mut f = Fixture::new()?;
        let mut extra = f.archive.clone();
        extra.extend(&f.archive);
        f.replace_archive(extra);
        rejected(f.admit(), Rejection::ArchiveEncoding);
        let mut f = Fixture::new()?;
        let mut truncated = f.archive.clone();
        truncated.pop();
        f.replace_archive(truncated);
        rejected(f.admit(), Rejection::ArchiveEncoding);
        let mut f = Fixture::new()?;
        f.replace_archive(gzip(&vec![0; MAX_TAR_BYTES + 1])?);
        rejected(f.admit(), Rejection::ArchiveBounds);
        Ok(())
    }
    #[test]
    fn checksum_layout_and_malformed_identity_reject_independently()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut f = Fixture::new()?;
        f.checksum = format!("{}  ../{ASSET}\n", sha256(&f.archive)).into_bytes();
        f.packet.checksum_length = f.checksum.len() as u64;
        f.packet.checksum_sha256 = sha256(&f.checksum);
        f.authorize();
        rejected(f.admit(), Rejection::ChecksumContents);
        let mut f = Fixture::new()?;
        f.packet.head_sha = "A".repeat(40);
        rejected(f.admit(), Rejection::MalformedIdentity);
        let mut f = Fixture::new()?;
        f.packet.producer_ref = "refs/heads/../escape".into();
        rejected(f.admit(), Rejection::MalformedIdentity);
        Ok(())
    }
}
