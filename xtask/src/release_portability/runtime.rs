use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use wait_timeout::ChildExt;

use super::archive::{ArchiveLayout, inspect_archive, verify_download, verify_version};
use super::metadata::{Asset, ReleaseIdentity};
use super::packet;
use super::process::{Capture, Runner, strings};
use super::receipts::{Authorization, Environment, Outcome, Platform, Row, write_json};

const BASE_SOURCE: &str =
    "pub fn checked_add(left: usize, right: usize) -> Option<usize> { left.checked_add(right) }\n";
const HEAD_SOURCE: &str = "pub fn checked_add(left: usize, right: usize) -> Option<usize> { left.checked_add(right) }\n\npub unsafe fn read_raw(value: *const usize) -> usize {\n    // SAFETY: the synthetic fixture intentionally exposes a reviewable pointer contract.\n    unsafe { *value }\n}\n";
const VALID_CONFIG: &str =
    "profile = \"gh-runner\"\n\n[providers]\npolicy = \"auto\"\n\n[impact]\nmode = \"shadow\"\n";

struct Container {
    id: String,
    active: bool,
}

impl Container {
    fn create(runner: &mut Runner, image: &str) -> Result<Self> {
        let created = runner.checked(
            "docker",
            &strings(&[
                "create",
                "--platform",
                "linux/amd64",
                image,
                "sleep",
                "1800",
            ]),
        )?;
        let id = created.text()?.to_owned();
        ensure!(
            id.len() == 64 && id.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "Docker returned an invalid container identity"
        );
        let container = Self { id, active: true };
        runner.checked("docker", &strings(&["start", &container.id]))?;
        Ok(container)
    }

    fn exec(&self, runner: &mut Runner, args: &[&str]) -> Result<Capture> {
        let mut command = strings(&["exec", &self.id]);
        command.extend(strings(args));
        runner.run("docker", &command)
    }

    fn checked(&self, runner: &mut Runner, args: &[&str]) -> Result<Capture> {
        self.exec(runner, args)?
            .require_success("container command")
    }

    fn copy_to(&self, runner: &mut Runner, source: &Path, target: &str) -> Result<()> {
        runner.checked(
            "docker",
            &strings(&[
                "cp",
                source.to_str().context("non-UTF-8 copy source")?,
                &format!("{}:{target}", self.id),
            ]),
        )?;
        Ok(())
    }

    fn copy_from(&self, runner: &mut Runner, source: &str, target: &Path) -> Result<()> {
        runner.checked(
            "docker",
            &strings(&[
                "cp",
                &format!("{}:{source}", self.id),
                target.to_str().context("non-UTF-8 copy destination")?,
            ]),
        )?;
        Ok(())
    }

    fn remove(&mut self, runner: &mut Runner) -> Result<()> {
        runner.checked("docker", &strings(&["rm", "--force", &self.id]))?;
        self.active = false;
        Ok(())
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        if self.active {
            let removed = (|| -> Result<bool> {
                let mut child = Command::new("docker")
                    .args(["rm", "--force", &self.id])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn()?;
                match child.wait_timeout(Duration::from_secs(15)) {
                    Ok(Some(status)) => Ok(status.success()),
                    _ => {
                        let _ = child.kill();
                        let _ = child.wait();
                        Ok(false)
                    }
                }
            })();
            if !matches!(removed, Ok(true)) {
                eprintln!(
                    "cleanup incomplete: lane-owned Docker container {}",
                    self.id
                );
            }
        }
    }
}

fn environment(container: &Container, runner: &mut Runner) -> Result<Environment> {
    // Shell is used only for its executable lookup builtin. Rust evaluates
    // every result and owns the environment assertion and receipt.
    let cargo = container.exec(runner, &["sh", "-c", "command -v cargo"])?;
    let rustc = container.exec(runner, &["sh", "-c", "command -v rustc"])?;
    ensure!(
        (cargo.success || cargo.code == Some(127) || cargo.code == Some(1))
            && (rustc.success || rustc.code == Some(127) || rustc.code == Some(1)),
        "tool-absence probe did not execute"
    );
    let source = container.exec(runner, &["test", "-e", "/work/fixture/src/main.rs"])?;
    let fallback = container.exec(runner, &["test", "-e", "/work/ub-review-action-src"])?;
    ensure!(
        matches!(source.code, Some(0 | 1)) && matches!(fallback.code, Some(0 | 1)),
        "source-absence probe did not execute"
    );
    let observation = Environment {
        cargo_present: cargo.success,
        rustc_present: rustc.success,
        source_checkout_present: source.success,
        source_fallback_present: fallback.success,
    };
    observation.validate()?;
    Ok(observation)
}

fn platform(container: &Container, runner: &mut Runner, image: &str) -> Result<Platform> {
    let os = container.checked(runner, &["cat", "/etc/os-release"])?;
    let fields: BTreeMap<_, _> = os
        .text()?
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key, value.trim_matches('"')))
        .collect();
    let image_id = runner
        .checked(
            "docker",
            &strings(&["inspect", "--format", "{{.Image}}", &container.id]),
        )?
        .text()?
        .to_owned();
    Ok(Platform {
        image: image.to_owned(),
        image_id,
        os_id: fields.get("ID").context("OS ID")?.to_string(),
        os_version: fields.get("VERSION_ID").context("OS version")?.to_string(),
        architecture: container
            .checked(runner, &["uname", "-m"])?
            .text()?
            .to_owned(),
        libc: container
            .checked(runner, &["getconf", "GNU_LIBC_VERSION"])?
            .text()?
            .to_owned(),
        kernel: container
            .checked(runner, &["uname", "-r"])?
            .text()?
            .to_owned(),
    })
}

fn negatives(
    container: &Container,
    runner: &mut Runner,
    out: &Path,
    archive: &Asset,
    archive_bytes: &[u8],
) -> Result<()> {
    let mut tampered = archive_bytes.to_vec();
    tampered.push(0);
    ensure!(
        verify_download(archive, &tampered).is_err(),
        "tampered asset accepted"
    );
    let missing = runner.run(
        "curl",
        &strings(&[
            "--fail",
            "--silent",
            "--show-error",
            "--max-time",
            "10",
            "file:///ub-review-portability-intentionally-missing",
        ]),
    )?;
    ensure!(
        !missing.success && missing.code == Some(37),
        "missing asset control did not fail as a missing file"
    );
    let executable = out.join("impostor");
    container.copy_from(runner, "/bin/true", &executable)?;
    let impostor_bytes = fs::read(&executable)?;
    let mut builder = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(u64::try_from(impostor_bytes.len())?);
    header.set_mode(0o755);
    header.set_cksum();
    builder.append_data(&mut header, "ub-review", impostor_bytes.as_slice())?;
    let tar_bytes = builder.into_inner()?;
    let mut compressed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    std::io::Write::write_all(&mut compressed, &tar_bytes)?;
    let impostor_archive = compressed.finish()?;
    let impostor_asset = Asset {
        id: 0,
        name: "negative-control".to_owned(),
        size: u64::try_from(impostor_archive.len())?,
        digest: format!("sha256:{}", super::archive::sha256(&impostor_archive)),
        browser_download_url: String::new(),
    };
    verify_download(&impostor_asset, &impostor_archive)?;
    let (_, extracted) = inspect_archive(&impostor_archive)?;
    fs::write(&executable, extracted)?;
    container.copy_to(runner, &executable, "/work/bin/impostor")?;
    container.checked(runner, &["chmod", "755", "/work/bin/impostor"])?;
    let identity = container.exec(runner, &["/work/bin/impostor", "--version"])?;
    ensure!(
        verify_version(identity.success, &identity.stdout, &identity.stderr).is_err(),
        "checksum-valid impostor accepted"
    );
    let mut controls = BTreeMap::from([
        ("missing_asset_rejected", true),
        ("tampered_asset_rejected_before_extraction", true),
        ("checksum_valid_impostor_rejected_by_identity", true),
    ]);
    for (name, members) in [
        (
            "wrong_layout_rejected_before_execution",
            vec!["nested/ub-review"],
        ),
        (
            "duplicate_archive_member_rejected_before_execution",
            vec!["ub-review", "ub-review"],
        ),
    ] {
        let mut builder = tar::Builder::new(Vec::new());
        for member in members {
            let mut header = tar::Header::new_gnu();
            header.set_size(3);
            header.set_mode(0o755);
            header.set_cksum();
            builder.append_data(&mut header, member, b"bin".as_slice())?;
        }
        let bytes = builder.into_inner()?;
        let mut compressed =
            flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        std::io::Write::write_all(&mut compressed, &bytes)?;
        ensure!(
            inspect_archive(&compressed.finish()?).is_err(),
            "{name} control accepted"
        );
        controls.insert(name, true);
    }
    write_json(&out.join("negative-controls.json"), &controls)
}

fn supported_packet(container: &Container, runner: &mut Runner, out: &Path) -> Result<()> {
    let fixture = out.join("fixture-input");
    fs::create_dir(&fixture)?;
    fs::create_dir(fixture.join("src"))?;
    fs::write(
        fixture.join("Cargo.toml"),
        "[package]\nname = \"release-portability-fixture\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )?;
    fs::write(fixture.join("src/lib.rs"), BASE_SOURCE)?;
    fs::write(fixture.join(".ub-review.toml"), VALID_CONFIG)?;
    container.copy_to(runner, &fixture, "/work/fixture")?;
    for args in [
        vec!["git", "-C", "/work/fixture", "init", "-q", "-b", "main"],
        vec![
            "git",
            "-C",
            "/work/fixture",
            "config",
            "user.name",
            "UB Review Portability Proof",
        ],
        vec![
            "git",
            "-C",
            "/work/fixture",
            "config",
            "user.email",
            "proof@invalid.example",
        ],
    ] {
        container.checked(runner, &args)?;
    }
    container.checked(
        runner,
        &[
            "/work/bin/ub-review",
            "init",
            "--path",
            "/work/fixture/init-generated.ub-review.toml",
            "--no-guide",
            "--profile",
            "gh-runner",
            "--force",
        ],
    )?;
    container.copy_from(
        runner,
        "/work/fixture/init-generated.ub-review.toml",
        &out.join("init-generated.ub-review.toml"),
    )?;
    container.checked(runner, &["git", "-C", "/work/fixture", "add", "."])?;
    container.checked(
        runner,
        &[
            "git",
            "-C",
            "/work/fixture",
            "commit",
            "-q",
            "-m",
            "fixture base",
        ],
    )?;
    fs::write(fixture.join("src/lib.rs"), HEAD_SOURCE)?;
    container.copy_to(
        runner,
        &fixture.join("src/lib.rs"),
        "/work/fixture/src/lib.rs",
    )?;
    container.checked(runner, &["git", "-C", "/work/fixture", "add", "src/lib.rs"])?;
    container.checked(
        runner,
        &[
            "git",
            "-C",
            "/work/fixture",
            "commit",
            "-q",
            "-m",
            "fixture head",
        ],
    )?;
    for (config, packet_name) in [
        ("init-generated.ub-review.toml", "packet-init-generated"),
        (".ub-review.toml", "packet"),
    ] {
        let config_path = format!("/work/fixture/{config}");
        container.checked(
            runner,
            &[
                "/work/bin/ub-review",
                "doctor",
                "--root",
                "/work/fixture",
                "--config",
                &config_path,
                "--profile",
                "gh-runner",
                "--base",
                "HEAD~1",
            ],
        )?;
        let packet_path = format!("/work/fixture/{packet_name}");
        container.checked(
            runner,
            &[
                "/work/bin/ub-review",
                "run",
                "--root",
                "/work/fixture",
                "--base",
                "HEAD~1",
                "--head",
                "HEAD",
                "--config",
                &config_path,
                "--out",
                &packet_path,
                "--profile",
                "gh-runner",
                "--dry-run",
                "--posting",
                "artifact-only",
                "--run-pass",
                "manual",
                "--model-mode",
                "off",
                "--fail-on-gate",
                "false",
                "--no-github-summary",
            ],
        )?;
        container.copy_from(runner, &packet_path, &out.join(packet_name))?;
    }
    packet::validate_init_defect(&fs::read(
        out.join("packet-init-generated/review/gate_outcome.json"),
    )?)?;
    let inventory = packet::inventory(&out.join("packet"))?;
    write_json(&out.join("packet-inventory.json"), &inventory)?;
    Ok(())
}

pub(super) struct VerifiedRelease<'a> {
    pub release: &'a ReleaseIdentity,
    pub layout: &'a ArchiveLayout,
    pub archive_bytes: &'a [u8],
    pub executable: &'a Path,
}

pub(super) fn run_row(
    runner: &mut Runner,
    out: &Path,
    image: &str,
    verified: VerifiedRelease<'_>,
    authorization: &Authorization,
) -> Result<Row> {
    let VerifiedRelease {
        release,
        layout,
        archive_bytes,
        executable,
    } = verified;
    fs::create_dir(out)?;
    let mut container = Container::create(runner, image)?;
    let result = (|| {
        let platform = platform(&container, runner, image)?;
        environment(&container, runner)?;
        container.checked(runner, &["mkdir", "-p", "/work/bin"])?;
        container.copy_to(runner, executable, "/work/bin/ub-review")?;
        container.checked(runner, &["chmod", "755", "/work/bin/ub-review"])?;
        let version = container.exec(runner, &["/work/bin/ub-review", "--version"])?;
        let mut checks: BTreeMap<String, String> = [
            "download",
            "published_checksum",
            "archive_layout",
            "environment",
        ]
        .into_iter()
        .map(|name| (name.to_owned(), "pass".to_owned()))
        .collect();
        let outcome = if image == "ubuntu:22.04" {
            ensure!(
                !version.success && String::from_utf8_lossy(&version.stderr).contains("GLIBC_2.39"),
                "unsupported baseline did not fail at the expected GLIBC_2.39 loader boundary"
            );
            checks.insert("loader".to_owned(), "GLIBC_2.39_rejection".to_owned());
            Outcome::UnsupportedExpected
        } else {
            verify_version(version.success, &version.stdout, &version.stderr)?;
            let help = container.checked(runner, &["/work/bin/ub-review", "--help"])?;
            ensure!(
                help.text()?.contains("Build box-aware evidence packets"),
                "help does not identify UB Review"
            );
            container.checked(runner, &["apt-get", "update", "-qq"])?;
            container.checked(
                runner,
                &[
                    "apt-get",
                    "install",
                    "-y",
                    "-qq",
                    "--no-install-recommends",
                    "ca-certificates",
                    "git",
                ],
            )?;
            negatives(&container, runner, out, &release.archive, archive_bytes)?;
            supported_packet(&container, runner, out)?;
            for name in [
                "binary_identity",
                "help",
                "doctor",
                "model_off_packet",
                "negative_controls",
            ] {
                checks.insert(name.to_owned(), "pass".to_owned());
            }
            checks.insert(
                "init_known_defect".to_owned(),
                "expected_policy_failure".to_owned(),
            );
            Outcome::SupportedPass
        };
        let environment = environment(&container, runner)?;
        let row = Row {
            schema: "ub-review.release_portability_receipt.v2".to_owned(),
            release: release.clone(),
            authorization: authorization.clone(),
            archive: layout.clone(),
            platform,
            environment,
            outcome,
            checks,
        };
        write_json(&out.join("receipt.json"), &row)?;
        Ok(row)
    })();
    let cleanup = container.remove(runner);
    let row = result?;
    cleanup?;
    Ok(row)
}
