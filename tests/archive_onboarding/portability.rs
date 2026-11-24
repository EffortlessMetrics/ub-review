//! Host-only #1071 controller. The consumer receives data and the exact
//! resolver step, with neither the source checkout nor the build toolchain.
use super::{Provenance, admit, candidate_campaign, input};
use crate::{ADAPTER, ASSET, digest, executable, resolver_shell_from, write};
use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use std::fs;
use std::os::unix::fs::DirBuilderExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const VERIFIER: &[u8] = include_bytes!("../../scripts/verify-bun-review-artifacts.py");
// The host never extracts using container-provided paths or metadata. It
// validates the complete archive first, then writes only ordinary files.
const ADMIT_EVIDENCE: &str = r#"import pathlib, re, sys, tarfile
archive, output = map(pathlib.Path, sys.argv[1:])
limit = 32 * 1024 * 1024
if archive.stat().st_size > limit:
    raise ValueError("evidence stream exceeds size limit")
with tarfile.open(archive, "r:") as bundle:
    entries = {}
    files = 0
    size = 0
    for member in bundle:
        name = member.name.removeprefix("./")
        if (member.pax_headers or member.sparse is not None
                or member.offset_data != member.offset + 512
                or member.type not in (tarfile.REGTYPE, tarfile.AREGTYPE, tarfile.DIRTYPE)):
            raise ValueError("unsupported evidence member")
        parts = () if name in (".", "") else tuple(name.split("/"))
        if (len(parts) > 12 or any(not re.fullmatch(r"[A-Za-z0-9_.-]{1,160}", p)
                                   or p in (".", "..") for p in parts)
                or (not parts and (member.name not in (".", "./") or not member.isdir()))
                or parts in entries):
            raise ValueError("invalid evidence path")
        if member.isdir():
            if member.size:
                raise ValueError("directory has content")
        else:
            files += 1
            size += member.size
            if member.size < 0 or member.size > 4 * 1024 * 1024 or size > 24 * 1024 * 1024:
                raise ValueError("evidence file size exceeds limit")
        if len(entries) >= 2048 or files > 1024:
            raise ValueError("evidence entry count exceeds limit")
        entries[parts] = member
    if not files:
        raise ValueError("no evidence files")
    for parts in entries:
        for i in range(1, len(parts)):
            if parts[:i] in entries and not entries[parts[:i]].isdir():
                raise ValueError("evidence file is a parent path")
    output.mkdir(mode=0o700)
    for parts, member in entries.items():
        destination = output.joinpath(*parts)
        if member.isdir():
            destination.mkdir(mode=0o700, parents=True, exist_ok=True)
        else:
            destination.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            with bundle.extractfile(member) as source, destination.open("xb") as target:
                data = source.read(member.size + 1)
                if len(data) != member.size:
                    raise ValueError("incomplete evidence file")
                target.write(data)
"#;

const TRANSFER_WORKER: &str = r#"rc=0
/bin/bash /input/worker.sh "$@" >&2 || rc=$?
tar --format=ustar -cf - -C /evidence . || exit 44
exit "$rc"
"#;

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
    let cap_kib = match name {
        "elf-subject" => "65536",
        "container" => "32768",
        "admit-evidence" => "4096",
        "fixture" => "8192",
        _ => "2048",
    };
    let status = Command::new("/usr/bin/timeout")
        .args([
            "--kill-after=2s",
            &format!("{deadline}s"),
            "/bin/bash",
            "-c",
            r#"ulimit -f "$1" || exit 125; shift; exec "$@""#,
            "bounded-command",
            cap_kib,
            program,
        ])
        .args(args)
        .env_clear()
        .env("HOME", output)
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .stdout(fs::File::create(stdout)?)
        .stderr(fs::File::create(stderr)?)
        .status()
        .with_context(|| format!("start bounded {program} for {name}"))?;
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

fn action_bytes(root: &Path, repo: &Path, commit: &str, tree: &str, blob: &str) -> Result<Vec<u8>> {
    for oid in [commit, tree, blob] {
        ensure!(
            oid.len() == 40
                && oid
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "expected a full lowercase Git SHA-1 object ID"
        );
    }
    let repo = string(repo)?;
    let git = "/usr/bin/git";
    local_cmd(
        root,
        "action-revision",
        git,
        &[
            "-C",
            repo,
            "rev-parse",
            "--verify",
            &format!("{commit}^{{commit}}"),
        ],
        10,
    )?;
    ensure!(
        fs::read_to_string(root.join("action-revision.stdout"))?.trim() == commit,
        "selected Action commit mismatch"
    );
    local_cmd(
        root,
        "action-tree",
        git,
        &[
            "-C",
            repo,
            "rev-parse",
            "--verify",
            &format!("{commit}^{{tree}}"),
        ],
        10,
    )?;
    ensure!(
        fs::read_to_string(root.join("action-tree.stdout"))?.trim() == tree,
        "selected Action tree mismatch"
    );
    local_cmd(
        root,
        "action-entry",
        git,
        &["-C", repo, "ls-tree", tree, "--", "action.yml"],
        10,
    )?;
    ensure!(
        fs::read_to_string(root.join("action-entry.stdout"))?.trim_end()
            == format!("100644 blob {blob}\taction.yml"),
        "selected Action blob mismatch"
    );
    local_cmd(
        root,
        "action-blob",
        git,
        &["-C", repo, "cat-file", "blob", blob],
        10,
    )?;
    let bytes = root.join("action-blob.stdout");
    local_cmd(
        root,
        "action-rehash",
        git,
        &["-C", repo, "hash-object", "--no-filters", string(&bytes)?],
        10,
    )?;
    ensure!(
        fs::read_to_string(root.join("action-rehash.stdout"))?.trim() == blob,
        "retained Action bytes do not match the selected blob"
    );
    Ok(fs::read(bytes)?)
}

fn admit_evidence(root: &Path, archive: &Path, output: &Path) -> Result<()> {
    local_cmd(
        root,
        "admit-evidence",
        "/usr/bin/python3",
        &[
            "-I",
            "-c",
            ADMIT_EVIDENCE,
            string(archive)?,
            string(output)?,
        ],
        20,
    )
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
    let user = format!(
        "{}:{}",
        rustix::process::getuid().as_raw(),
        rustix::process::getgid().as_raw()
    );
    write(
        &root.join("cleanup.state"),
        "uncertain: create not yet reconciled\n",
    )?;
    let created = docker(
        root,
        "container-create",
        &[
            "create",
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
            "--tmpfs",
            "/evidence:rw,noexec,nosuid,nodev,size=32m,nr_inodes=2048,mode=1777",
            "--entrypoint",
            "/usr/bin/env",
            image,
            "-i",
            "/bin/bash",
            "-c",
            TRANSFER_WORKER,
            "transfer-worker",
            consumer_args[0],
            consumer_args[1],
            consumer_args[2],
            consumer_args[3],
        ],
        15,
    );
    let result = created.and_then(|()| {
        let id = fs::read_to_string(root.join("container-create.stdout"))?;
        let id = id.trim();
        ensure!(
            image_id(&format!("sha256:{id}")),
            "invalid created container ID"
        );
        write(&root.join("container.id"), id)?;
        docker(root, "container", &["start", "--attach", id], 90)?;
        docker(
            root,
            "consumer-exit",
            &[
                "container",
                "inspect",
                "--format",
                "{{.State.Status}} {{.State.ExitCode}}",
                id,
            ],
            10,
        )?;
        ensure!(
            fs::read_to_string(root.join("consumer-exit.stdout"))?.trim() == "exited 0",
            "consumer did not exit successfully"
        );
        Ok(())
    });
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
    let known_id = fs::read_to_string(root.join("container.id")).ok();
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
    if let Some(id) = known_id {
        docker(
            root,
            "cleanup-id-check",
            &[
                "container",
                "ls",
                "-aq",
                "--no-trunc",
                "--filter",
                &format!("id={id}"),
            ],
            15,
        )?;
        ensure!(
            fs::read(root.join("cleanup-id-check.stdout"))?.is_empty(),
            "known container remains"
        );
        write(
            &root.join("cleanup.state"),
            "confirmed: owned container absent\n",
        )?;
    }
    // A failed consumer can still yield an admissible archive of partial evidence.
    let stream = root.join("container.stdout");
    if stream.is_file() && fs::metadata(&stream)?.len() > 0 {
        let admitted = admit_evidence(root, &stream, output);
        result?;
        admitted
    } else {
        result?;
        anyhow::bail!("consumer returned no evidence stream")
    }
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
        admit(
            &mut campaign,
            &input("UB_CANDIDATE_BYTES_ZIP")?,
            &input("UB_CANDIDATE_PACKET_ZIP")?,
            &input("UB_CANDIDATE_FACTS")?,
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        )?;
        let repo = input("UB_PORTABILITY_ACTION_REPO")?;
        ensure!(
            repo.is_absolute(),
            "Action object repository path must be absolute"
        );
        let commit = std::env::var("UB_PORTABILITY_ACTION_COMMIT")?;
        let tree = std::env::var("UB_PORTABILITY_ACTION_TREE")?;
        let blob = std::env::var("UB_PORTABILITY_ACTION_BLOB")?;
        evidence["selected_action"] =
            json!({"commit":commit,"tree":tree,"blob":blob,"verified":false});
        let action = action_bytes(&root, &repo, &commit, &tree, &blob)?;
        evidence["selected_action"]["verified"] = json!(true);
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
    if root.join("cleanup.state").is_file() {
        evidence["cleanup"] = json!(fs::read_to_string(root.join("cleanup.state"))?.trim());
    }

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

#[test]
fn portability_action_uses_selected_git_object_and_refuses_wrong_blob() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let repo = root.join("repo");
    let logs = root.join("logs");
    fs::create_dir(&repo)?;
    fs::create_dir(&logs)?;
    let path = string(&repo)?;
    let git = |name, args: &[&str]| -> Result<String> {
        let mut command = vec!["-C", path];
        command.extend_from_slice(args);
        local_cmd(&logs, name, "/usr/bin/git", &command, 10)?;
        Ok(fs::read_to_string(logs.join(format!("{name}.stdout")))?
            .trim()
            .to_owned())
    };
    git("init", &["init", "-q"])?;
    write(&repo.join("action.yml"), "name: retained\n")?;
    git("add", &["add", "action.yml"])?;
    git(
        "commit",
        &[
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.invalid",
            "commit",
            "-qm",
            "selected Action",
        ],
    )?;
    let commit = git("head", &["rev-parse", "HEAD"])?;
    let tree = git("tree", &["rev-parse", "HEAD^{tree}"])?;
    let blob = git("blob", &["rev-parse", "HEAD:action.yml"])?;
    write(&repo.join("action.yml"), "name: mutable checkout\n")?;
    assert_eq!(
        action_bytes(&logs, &repo, &commit, &tree, &blob)?,
        b"name: retained\n"
    );
    let error = action_bytes(&logs, &repo, &commit, &tree, &"0".repeat(40))
        .err()
        .context("incorrect blob admitted")?;
    assert!(error.to_string().contains("selected Action blob mismatch"));
    Ok(())
}

#[test]
fn portability_evidence_admits_plain_files_and_refuses_unsupported_members() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let fixture = r#"import io, sys, tarfile
with tarfile.open(sys.argv[1], "w", format=tarfile.USTAR_FORMAT) as t:
    dir = tarfile.TarInfo("./")
    dir.type = tarfile.DIRTYPE
    t.addfile(dir)
    f = tarfile.TarInfo("./packet/status")
    f.size = 2
    t.addfile(f, io.BytesIO(b"ok"))
    if sys.argv[2] == "link":
        link = tarfile.TarInfo("./ordinary-link")
        link.type = tarfile.SYMTYPE
        link.linkname = "packet/status"
        t.addfile(link)
    if sys.argv[2] == "count":
        for i in range(1024):
            t.addfile(tarfile.TarInfo(f"./file-{i}"))
    if sys.argv[2] == "size":
        large = tarfile.TarInfo("./large")
        large.size = 4*1024*1024+1
        t.addfile(large, io.BytesIO(bytes(large.size)))
"#;
    for control in ["plain", "link", "count", "size"] {
        let archive = root.join("fixture.tar");
        local_cmd(
            root,
            "fixture",
            "/usr/bin/python3",
            &["-I", "-c", fixture, string(&archive)?, control],
            10,
        )?;
        let admitted = root.join(control);
        let result = admit_evidence(root, &archive, &admitted);
        if control == "plain" {
            result?;
            assert_eq!(fs::read(admitted.join("packet/status"))?, b"ok");
        } else {
            assert!(result.is_err() && !admitted.exists());
        }
    }
    Ok(())
}

#[test]
fn portability_host_command_caps_output_and_time() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let result = local_cmd(
        root,
        "overflow",
        "/usr/bin/dd",
        &["if=/dev/zero", "bs=1048576", "count=3"],
        10,
    );
    assert!(result.is_err());
    assert!(fs::metadata(root.join("overflow.stdout"))?.len() <= 2 * 1024 * 1024);
    let result = local_cmd(root, "deadline", "/usr/bin/sleep", &["15"], 1);
    assert!(result.is_err());
    assert!(fs::read_to_string(root.join("deadline.status"))?.contains("124"));
    Ok(())
}
