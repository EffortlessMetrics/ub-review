//! Host-only #1071 controller. The consumer receives data and the exact
//! resolver step, with neither the source checkout nor the build toolchain.
use super::{Provenance, admit, candidate_campaign, input};
use crate::{ADAPTER, ASSET, SOURCE, digest, executable, resolver_shell_from, write};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use wait_timeout::ChildExt;

const VERIFIER: &[u8] = include_bytes!("../../scripts/verify-bun-review-artifacts.py");
const WORKER: &str = r#"set -euo pipefail
input=/input
out=/evidence
export HOME=/tmp/home LC_ALL=C PATH=/usr/bin:/bin
mkdir -p "$HOME"
. /etc/os-release
printf '%s %s\n' "$ID" "$VERSION_ID" > "$out/os"
uname -m > "$out/architecture"
uname -r > "$out/kernel"
getconf GNU_LIBC_VERSION > "$out/libc"
[[ "$ID" == ubuntu && "$VERSION_ID" == "$1" && "$(uname -m)" == x86_64 ]]
for tool in cargo rustc rustup; do
  if command -v "$tool"; then exit 41; fi
done
test ! -e /checkout
printf '%s\n' no_toolchain_or_action_checkout > "$out/environment"
export PATH=/input/bin:/usr/bin:/bin
export GITHUB_ACTION_PATH=/checkout FROZEN=/input/frozen TAG="$2"
export UB_REVIEW_RELEASE_VERSION="$TAG" UB_REVIEW_INSTALL_MODE=release
export UB_REVIEW_ACTION_REPOSITORY=EffortlessMetrics/ub-review UB_REVIEW_SERVER_URL=https://archive.invalid
for control in 1 checksum 0; do
  export ADAPTER_FAIL="$control" ADAPTER_LOG="$out/transport-$control"
  export RUNNER_TEMP="/tmp/resolve-$control" GITHUB_OUTPUT="$out/resolver-$control.output"
  mkdir -p "$RUNNER_TEMP"
  : > "$GITHUB_OUTPUT"
  rc=0
  bash /input/resolver.sh > "$out/resolver-$control.stdout" 2> "$out/resolver-$control.stderr" || rc=$?
  printf '%s\n' "$rc" > "$out/resolver-$control.exit"
  test ! -e "$RUNNER_TEMP/ub-review-action-src"
  if [[ "$control" != 0 ]]; then
    test "$rc" -ne 0 && test ! -s "$GITHUB_OUTPUT" || exit 42
    if [[ "$control" == 1 ]]; then
      grep -Fq 'release binary download failed' "$out/resolver-$control.stdout"
    else
      grep -Fq 'release binary checksum mismatch' "$out/resolver-$control.stdout"
    fi
  else
    test "$rc" -eq 0
  fi
done
bin="$RUNNER_TEMP/ub-review-action-release/ub-review"
test "$(grep '^bin=' "$GITHUB_OUTPUT")" = "bin=$bin"
test "$(wc -l < "$out/transport-0")" -eq 2
test -f "$bin" && test ! -L "$bin"
sha256sum "$bin" > "$out/installed.sha256"
test "$(cut -d' ' -f1 "$out/installed.sha256")" = "$3"
printf '%s\n' strict_release_installed_no_fallback > "$out/resolver.status"
"$bin" --version > "$out/version"
test "$(cat "$out/version")" = "ub-review $4"
"$bin" --help > "$out/help"
command -v git > "$out/git"
repo=/tmp/repo
mkdir -p "$repo/src"
git -C "$repo" init -q -b main
git -C "$repo" config user.name "Portability fixture"
git -C "$repo" config user.email "portability@example.invalid"
printf '[package]\nname = "portability-fixture"\nversion = "0.0.0"\nedition = "2024"\n' > "$repo/Cargo.toml"
printf 'pub fn answer() -> u8 { 41 }\n' > "$repo/src/lib.rs"
git -C "$repo" add .
git -C "$repo" commit -qm base
printf 'pub fn answer() -> u8 { 42 }\n' > "$repo/src/lib.rs"
git -C "$repo" commit -qam head
git -C "$repo" rev-parse HEAD~1 HEAD > "$out/fixture-revisions"
export ADAPTER_FAIL=1 ADAPTER_LOG="$out/model-transport"
env -i HOME="$HOME" LC_ALL=C PATH="$PATH" ADAPTER_FAIL="$ADAPTER_FAIL" ADAPTER_LOG="$ADAPTER_LOG" \
  "$bin" run --config /input/model-off.toml --root "$repo" --base HEAD~1 --head HEAD \
  --out "$out/packet" --mode review-byok --model-mode off --posting artifact-only \
  --fail-on-gate false --run-pass opened --no-github-summary > "$out/run.stdout" 2> "$out/run.stderr"
test ! -e "$out/packet/review/github-review.json"
gate_sha=$(sha256sum "$out/packet/review/gate_outcome.json")
env -i HOME="$HOME" LC_ALL=C PATH="$PATH" ADAPTER_FAIL="$ADAPTER_FAIL" ADAPTER_LOG="$ADAPTER_LOG" \
  "$bin" post --review-json "$out/packet/review/github-review.json" --out "$out/packet/review" \
  --repo example/repo --pull-number 123 > "$out/post.stdout" 2> "$out/post.stderr"
test "$gate_sha" = "$(sha256sum "$out/packet/review/gate_outcome.json")"
test ! -e "$ADAPTER_LOG"
test "$(sha256sum "$bin" | cut -d' ' -f1)" = "$3"
"#;

fn model_config() -> String {
    let mut config = String::from(
        "profile = \"gh-runner\"\n[repo]\nkind = \"rust\"\nledger = \"\"\n[review]\nenable_default_lanes = false\n[[lanes]]\nid = \"archive-lane\"\nrole = \"Portability fixture\"\nfocus = \"Inspect the return value\"\n",
    );
    for name in [
        "tokmd",
        "cargo-allow",
        "ast-grep",
        "ripr",
        "unsafe-review",
        "semgrep",
        "actionlint",
        "zizmor",
        "gitleaks",
        "cargo-fmt",
        "cargo-check",
        "cargo-test",
        "cargo-clippy",
        "cargo-doc",
        "artifact-verifier",
        "coverage",
        "osv-scanner",
        "cargo-audit",
        "cargo-deny",
        "shellcheck",
        "cppcheck",
        "test",
        "build",
        "miri",
        "cargo-mutants",
    ] {
        config.push_str(&format!(
            "\n[tools.{name}]\nenabled = false\nrequired = false\n"
        ));
    }
    config
}

fn local_cmd(output: &Path, name: &str, program: &str, args: &[&str], deadline: u64) -> Result<()> {
    let stdout = output.join(format!("{name}.stdout"));
    let stderr = output.join(format!("{name}.stderr"));
    let mut child = Command::new(program)
        .args(args)
        .env_clear()
        .env("HOME", output)
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(fs::File::create(stdout)?)
        .stderr(fs::File::create(stderr)?)
        .spawn()
        .with_context(|| format!("start {program} for {name}"))?;
    let Some(status) = child.wait_timeout(Duration::from_secs(deadline))? else {
        child.kill()?;
        child.wait()?;
        write(
            &output.join(format!("{name}.status")),
            "timeout; direct child killed and reaped\n",
        )?;
        anyhow::bail!("{name} timed out");
    };
    write(&output.join(format!("{name}.status")), status.to_string())?;
    ensure!(status.success(), "{name} failed: {status}");
    Ok(())
}

fn string(path: &Path) -> Result<&str> {
    path.to_str().context("non-UTF-8 path")
}

fn image_id(id: &str) -> bool {
    id.strip_prefix("sha256:").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    })
}

fn docker(root: &Path, name: &str, args: &[&str], deadline: u64) -> Result<()> {
    let mut command = vec!["--host=unix:///var/run/docker.sock"];
    command.extend_from_slice(args);
    let program = ["/usr/bin/docker", "/usr/local/bin/docker"]
        .into_iter()
        .find(|path| Path::new(path).is_file())
        .context("local Docker CLI unavailable")?;
    local_cmd(root, name, program, &command, deadline)
}

fn run_container(
    root: &Path,
    bundle: &Path,
    output: &Path,
    image: &str,
    consumer_args: [&str; 4],
) -> Result<()> {
    docker(
        root,
        "image",
        &[
            "image",
            "inspect",
            "--format",
            "{{.Id}} {{.Os}} {{.Architecture}}",
            image,
        ],
        10,
    )
    .context("required pinned local image unavailable")?;
    ensure!(
        fs::read_to_string(root.join("image.stdout"))?.trim() == format!("{image} linux amd64"),
        "local image identity or platform mismatch"
    );
    let nonce = tempfile::tempdir_in(root)?;
    let label = format!(
        "ub-review.portability={}",
        nonce.path().file_name().context("nonce")?.to_string_lossy()
    );
    let label_filter = format!("label={label}");
    let input_mount = format!("type=bind,src={},dst=/input,readonly", bundle.display());
    let output_mount = format!("type=bind,src={},dst=/evidence", output.display());
    let user = format!(
        "{}:{}",
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw()
    );
    let result = docker(
        root,
        "container",
        &[
            "run",
            "--rm",
            "--pull=never",
            "--platform=linux/amd64",
            "--network=none",
            "--read-only",
            "--cap-drop=ALL",
            "--security-opt=no-new-privileges",
            "--log-driver=none",
            "--pids-limit=128",
            "--user",
            &user,
            "--label",
            &label,
            "--tmpfs",
            "/tmp:rw,exec,nosuid,nodev,size=128m",
            "--mount",
            &input_mount,
            "--mount",
            &output_mount,
            "--entrypoint",
            "/usr/bin/env",
            image,
            "-i",
            "/bin/bash",
            "/input/worker.sh",
            consumer_args[0],
            consumer_args[1],
            consumer_args[2],
            consumer_args[3],
        ],
        90,
    );
    // List only this run's containers, including a surviving daemon-side task
    // after a killed Docker client. Never remove an unrelated container.
    docker(
        root,
        "cleanup-discover",
        &[
            "container",
            "ls",
            "-aq",
            "--no-trunc",
            "--filter",
            &label_filter,
        ],
        15,
    )?;
    let ids = fs::read_to_string(root.join("cleanup-discover.stdout"))?;
    for id in ids.lines() {
        ensure!(
            image_id(&format!("sha256:{id}")),
            "invalid cleanup container ID"
        );
        docker(root, "cleanup-remove", &["container", "rm", "-f", id], 15)?;
    }
    docker(
        root,
        "cleanup-check",
        &[
            "container",
            "ls",
            "-aq",
            "--no-trunc",
            "--filter",
            &label_filter,
        ],
        15,
    )?;
    ensure!(
        fs::read(root.join("cleanup-check.stdout"))?.is_empty(),
        "owned container remains"
    );
    result
}

#[test]
#[ignore = "requires retained #1340 custody inputs and a private output parent; run mode needs a pinned local image"]
fn selected_archive_portability() -> Result<()> {
    let mode = std::env::var("UB_PORTABILITY_MODE")?;
    ensure!(
        matches!(mode.as_str(), "prepare" | "run"),
        "expected prepare or run"
    );
    let distro = std::env::var("UB_PORTABILITY_OS")?;
    ensure!(
        matches!(distro.as_str(), "24.04" | "22.04"),
        "expected Ubuntu 24.04 or 22.04"
    );
    let provenance = match std::env::var("UB_CANDIDATE_PROVENANCE")?.as_str() {
        "synthetic_fixture" => Provenance::Synthetic,
        "candidate_rehearsal" => Provenance::Candidate,
        _ => anyhow::bail!("explicit provenance required"),
    };
    let root = input("UB_PORTABILITY_RECEIPTS")?;
    ensure!(
        root.is_absolute() && !root.to_string_lossy().contains(','),
        "absolute path without comma required"
    );
    fs::DirBuilder::new().mode(0o700).create(&root)?;
    let (phase, distribution) = provenance.receipt_labels();
    let bundle = root.join("input");
    let output = root.join("consumer");
    let mut campaign = candidate_campaign(root.clone(), provenance);
    campaign.frozen = bundle.join("frozen");
    let mut evidence = json!({
        "schema":"ub-review.portability.local.v1", "phase":phase, "distribution":distribution,
        "requested_ubuntu":distro, "mode":mode, "network":"none", "source_mount":false,
        "resolver_receipt":"consumer/resolver.status", "packet":"consumer/packet"
    });
    let result = (|| -> Result<()> {
        fs::create_dir(&bundle)?;
        fs::create_dir(&output)?;
        admit(
            &mut campaign,
            &input("UB_CANDIDATE_BYTES_ZIP")?,
            &input("UB_CANDIDATE_PACKET_ZIP")?,
            &input("UB_CANDIDATE_FACTS")?,
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        )?;
        let action = fs::read(Path::new(SOURCE).join("action.yml"))?;
        let resolver = resolver_shell_from(std::str::from_utf8(&action)?)?;
        let model_config = model_config();
        evidence["materials"] = json!({
            "action_sha256":digest(&action), "resolver_sha256":digest(resolver.as_bytes()),
            "verifier_sha256":digest(VERIFIER), "adapter_sha256":digest(ADAPTER.as_bytes()),
            "worker_sha256":digest(WORKER.as_bytes()), "config_sha256":digest(model_config.as_bytes()),
            "controller_sha256":digest(include_bytes!("portability.rs"))
        });
        write(&root.join("action.yml"), action)?;
        write(&root.join("verify-bun-review-artifacts.py"), VERIFIER)?;
        write(&bundle.join("resolver.sh"), resolver)?;
        write(&bundle.join("worker.sh"), WORKER)?;
        write(&bundle.join("model-off.toml"), model_config)?;
        executable(&bundle.join("bin/curl"), ADAPTER)?;
        for file in ["resolver.sh", "worker.sh", "bin/curl"] {
            local_cmd(
                &root,
                &format!("syntax-{}", file.replace('/', "-")),
                "/bin/bash",
                &["-n", string(&bundle.join(file))?],
                10,
            )?;
        }
        local_cmd(
            &root,
            "elf-subject",
            "/usr/bin/tar",
            &[
                "-xOzf",
                string(&campaign.frozen.join(ASSET))?,
                "--",
                "ub-review",
            ],
            20,
        )?;
        let subject = root.join("elf-subject.stdout");
        let bin_digest = campaign.manifest["binary_sha256"]
            .as_str()
            .context("admitted binary digest")?;
        ensure!(
            digest(&fs::read(&subject)?) == bin_digest,
            "ELF subject differs from admitted member"
        );
        local_cmd(
            &root,
            "elf",
            "/usr/bin/readelf",
            &["-hW", "-lW", "-dW", "-VW", string(&subject)?],
            20,
        )?;
        evidence["elf_report_sha256"] = json!(digest(&fs::read(root.join("elf.stdout"))?));
        if mode == "prepare" {
            return Ok(());
        }
        let image = std::env::var("UB_PORTABILITY_IMAGE_ID")?;
        ensure!(
            image_id(&image),
            "expected an immutable local sha256 image ID"
        );
        evidence["image_id"] = json!(image);
        let tag = campaign.manifest["tag"].as_str().context("admitted tag")?;
        let version = campaign.manifest["version"]
            .as_str()
            .context("admitted version")?;
        run_container(
            &root,
            &bundle,
            &output,
            &image,
            [&distro, tag, bin_digest, version],
        )?;
        ensure!(
            fs::read_to_string(output.join("resolver.status"))?.trim()
                == "strict_release_installed_no_fallback"
        );
        let packet = output.join("packet");
        local_cmd(
            &root,
            "verify-packet",
            "/usr/bin/python3",
            &[
                "-I",
                string(&root.join("verify-bun-review-artifacts.py"))?,
                string(&packet)?,
                "--expected-repo-kind",
                "rust",
            ],
            30,
        )?;
        let gate: Value =
            serde_json::from_slice(&fs::read(packet.join("review/gate_outcome.json"))?)?;
        ensure!(
            gate["terminal_status"] == "artifact-only"
                && gate["gate_result"] == "not_proven"
                && gate["analysis_result"] == "not_proven"
                && gate["publication_result"] == "not_needed",
            "unexpected model-off result planes"
        );
        Ok(())
    })();
    evidence["manifest"] = campaign.manifest;
    evidence["status"] = json!(if result.is_err() {
        "failed"
    } else if mode == "prepare" {
        "prepared_only"
    } else {
        "bounded_local_observations_pass"
    });
    evidence["error"] = json!(result.as_ref().err().map(|error| format!("{error:#}")));
    write(
        &root.join("portability-receipt.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    result
}

#[test]
fn portability_rejects_tags_and_noncanonical_image_ids() {
    assert!(image_id(&format!("sha256:{}", "a".repeat(64))));
    for invalid in ["ubuntu:24.04", "ubuntu:22.04", "sha256:0", "sha256:"] {
        assert!(!image_id(invalid));
    }
    assert!(!image_id(&format!("sha256:{}", "A".repeat(64))));
}
