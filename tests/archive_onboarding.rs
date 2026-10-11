//! #1340: actual Action resolver and archive CLI rehearsal, never Cargo's test binary.
//! Owner: ub-review/tests. Run explicitly with frozen inputs; receipts survive failure.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

const ASSET: &str = "ub-review-x86_64-unknown-linux-gnu.tar.gz";
const SOURCE: &str = env!("CARGO_MANIFEST_DIR");

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    fs::create_dir_all(path.parent().context("file parent")?)?;
    fs::write(path, bytes)?;
    Ok(())
}
fn executable(path: &Path, body: &str) -> Result<()> {
    write(path, body)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}
fn inventory(root: &Path) -> Result<BTreeMap<String, String>> {
    fn visit(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let relative = path.strip_prefix(root)?.to_string_lossy().into_owned();
            let metadata = fs::symlink_metadata(&path)?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "unexpected symlink: {relative}"
            );
            if metadata.is_dir() {
                out.insert(
                    format!("{relative}/"),
                    format!("directory:{:o}", metadata.permissions().mode()),
                );
                visit(root, &path, out)?;
            } else {
                out.insert(
                    relative,
                    format!(
                        "{}:{:o}:{}",
                        metadata.len(),
                        metadata.permissions().mode(),
                        digest(&fs::read(path)?)
                    ),
                );
            }
        }
        Ok(())
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out)?;
    Ok(out)
}
fn resolver_shell() -> Result<String> {
    let text = fs::read_to_string(Path::new(SOURCE).join("action.yml"))?;
    let marker = "    - name: Resolve ub-review runner\n";
    ensure!(text.matches(marker).count() == 1, "resolver step changed");
    let step = text
        .split_once(marker)
        .context("resolver")?
        .1
        .split("\n    - ")
        .next()
        .context("step")?;
    let script = step.split_once("      run: |\n").context("shell block")?.1;
    script
        .lines()
        .map(|line| {
            if line.is_empty() {
                Ok(String::new())
            } else {
                Ok(line
                    .strip_prefix("        ")
                    .context("shell indentation")?
                    .into())
            }
        })
        .collect::<Result<Vec<String>>>()
        .map(|lines| lines.join("\n"))
}

struct Campaign {
    root: PathBuf,
    path: PathBuf,
    frozen: PathBuf,
    manifest: Value,
    rows: Vec<Value>,
    scenarios: BTreeMap<String, Value>,
}
impl Campaign {
    // Each command gets a cleared environment, bounded wait and file-backed logs.
    fn command(
        &mut self,
        cwd: &Path,
        program: &Path,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<Output> {
        let index = self.rows.len();
        let logs = self.root.join(format!("logs/{index}"));
        fs::create_dir_all(&logs)?;
        let stdout = fs::File::create(logs.join("stdout"))?;
        let stderr = fs::File::create(logs.join("stderr"))?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", &self.path)
            .env("HOME", &self.root)
            .env("LC_ALL", "C")
            .env("POISON_LOG", self.root.join("poison.log"))
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr);
        for (key, value) in env {
            cmd.env(key, value);
        }
        self.rows.push(json!({"program":program,"argv":args,"cwd":cwd,"state":"attempted","exit":null,"logs":format!("logs/{index}")}));
        self.receipt("running", None)?;
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.rows[index]["state"] = json!("spawn_failed");
                self.rows[index]["error"] = json!(error.to_string());
                return Err(error).with_context(|| format!("spawn {}", program.display()));
            }
        };
        let status = match child.wait_timeout(Duration::from_secs(30))? {
            Some(status) => status,
            None => {
                self.rows[index]["state"] = json!("timed_out");
                child.kill()?;
                let status = child.wait()?;
                self.rows[index]["exit"] = json!(status.code());
                self.rows[index]["reaped"] = json!(true);
                anyhow::bail!("command timed out; logs at {}", logs.display());
            }
        };
        let output = Output {
            status,
            stdout: fs::read(logs.join("stdout"))?,
            stderr: fs::read(logs.join("stderr"))?,
        };
        self.rows[index]["state"] = json!("exited");
        self.rows[index]["exit"] = json!(status.code());
        self.receipt("running", None)?;
        Ok(output)
    }
    fn proven(&mut self, name: &str) -> Result<()> {
        self.scenarios.insert(
            name.to_owned(),
            json!({"status":"proven", "through_command":self.rows.len()}),
        );
        self.receipt("running", None)
    }
    fn receipt(&self, status: &str, error: Option<String>) -> Result<()> {
        write(
            &self.root.join("receipt.json"),
            serde_json::to_vec_pretty(&json!({
                "schema":"ub-review.archive_onboarding_rehearsal.v1", "phase":"preliminary_local_archive",
                "distribution":"candidate_rehearsal", "platform":"Linux x86_64", "status":status,"error":error,
                "frozen_manifest":self.manifest, "harness_sha256":digest(include_bytes!("archive_onboarding.rs")),
                "action_sha256":digest(&fs::read(Path::new(SOURCE).join("action.yml"))?), "commands":self.rows,"scenarios":self.scenarios,
                "unproven":["final #816 archive", "public distribution", "previous supported release upgrade", "generated workflow execution", "live provider/delivery", "all #1340 leaf regression controls", "general process-tree containment"]
            }))?,
        )
    }
    fn resolve(&mut self, mode: &str, fail: &str) -> Result<Output> {
        let runtime = self.root.join("runtime");
        fs::create_dir_all(&runtime)?;
        let output = self.root.join("resolver-output");
        fs::write(&output, "")?;
        let shell = resolver_shell()?;
        let tag = self.manifest["tag"].as_str().context("tag")?.to_owned();
        let root = self.root.clone();
        let frozen = self.frozen.clone();
        self.command(
            &root,
            Path::new("/bin/bash"),
            &["--noprofile", "--norc", "-c", &shell],
            &[
                ("RUNNER_TEMP", runtime.to_str().context("runtime")?),
                ("GITHUB_OUTPUT", output.to_str().context("output")?),
                ("GITHUB_ACTION_PATH", SOURCE),
                ("UB_REVIEW_INSTALL_MODE", mode),
                ("UB_REVIEW_RELEASE_VERSION", &tag),
                ("UB_REVIEW_ACTION_REPOSITORY", "EffortlessMetrics/ub-review"),
                ("UB_REVIEW_SERVER_URL", "https://archive.invalid"),
                ("FROZEN", frozen.to_str().context("frozen")?),
                ("TAG", &tag),
                ("ADAPTER_FAIL", fail),
                (
                    "ADAPTER_LOG",
                    root.join("adapter.log").to_str().context("log")?,
                ),
                (
                    "POISON_LOG",
                    root.join("poison.log").to_str().context("log")?,
                ),
            ],
        )
    }
    fn selected(&self) -> Result<PathBuf> {
        let output = fs::read_to_string(self.root.join("resolver-output"))?;
        let bins: Vec<_> = output
            .lines()
            .filter_map(|line| line.strip_prefix("bin="))
            .collect();
        ensure!(bins.len() == 1, "exactly one resolved binary required");
        let path = PathBuf::from(bins[0]);
        self.identity(&path)?;
        Ok(path)
    }
    fn identity(&self, path: &Path) -> Result<()> {
        ensure!(
            path == self.root.join("runtime/ub-review-action-release/ub-review"),
            "PATH/workspace substitution rejected"
        );
        ensure!(
            fs::symlink_metadata(path)?.is_file(),
            "regular binary required"
        );
        ensure!(
            digest(&fs::read(path)?) == self.manifest["binary_sha256"],
            "frozen binary digest mismatch"
        );
        Ok(())
    }
    fn cli(&mut self, cwd: &Path, bin: &Path, args: &[&str]) -> Result<Output> {
        self.identity(bin)?;
        let frozen = self.frozen.clone();
        let tag = self.manifest["tag"].as_str().context("tag")?.to_owned();
        let log = self.root.join("adapter.log");
        self.command(
            cwd,
            bin,
            args,
            &[
                ("FROZEN", frozen.to_str().context("frozen")?),
                ("TAG", &tag),
                ("ADAPTER_LOG", log.to_str().context("log")?),
            ],
        )
    }
}
fn success(output: &Output) -> Result<()> {
    ensure!(
        output.status.success(),
        "command failed: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
fn refused(output: &Output) -> Result<()> {
    ensure!(!output.status.success(), "expected refusal");
    ensure!(
        String::from_utf8_lossy(&output.stderr).contains("exist"),
        "actionable existing-state diagnostic required"
    );
    Ok(())
}

// An inert transport adapter: allowlisted archive/checksum GETs and release metadata only.
// Unknown URLs and methods fail and are logged; this never executes real curl.
const ADAPTER: &str = r#"#!/bin/bash
set -eu
printf '%s\n' "$*" >> "$ADAPTER_LOG"
out=''
url=''
while [ "$#" -gt 0 ]; do
 case "$1" in
  -o) out="$2"; shift 2;;
  -X) [ "$2" = GET ] || exit 91; shift 2;;
  https://*) url="$1"; shift;;
  *) shift;;
 esac
done
[ "${ADAPTER_FAIL:-0}" != 1 ] || exit 22
asset=ub-review-x86_64-unknown-linux-gnu.tar.gz
case "$url" in
 "https://archive.invalid/EffortlessMetrics/ub-review/releases/download/$TAG/$asset") cp "$FROZEN/$asset" "$out";;
 "https://archive.invalid/EffortlessMetrics/ub-review/releases/download/$TAG/$asset.sha256")
   if [ "${ADAPTER_FAIL:-0}" = checksum ]; then printf '%064d\n' 0 > "$out"; else cp "$FROZEN/$asset.sha256" "$out"; fi;;
 https://api.github.com/repos/EffortlessMetrics/ub-review/releases/latest)
   printf '{"tag_name":"%s","assets":[{"name":"%s"},{"name":"%s.sha256"}]}\n' "$TAG" "$asset" "$asset";;
 *) exit 92;;
esac
"#;

#[test]
#[ignore = "requires independently frozen preliminary archive, manifest and retained output directory; see docs/ARCHIVE_ONBOARDING_REHEARSAL.md"]
fn frozen_archive_onboarding() -> Result<()> {
    let frozen =
        PathBuf::from(std::env::var("UB_ARCHIVE_PACKET").context("UB_ARCHIVE_PACKET required")?)
            .canonicalize()?;
    let root = PathBuf::from(
        std::env::var("UB_ARCHIVE_RECEIPTS").context("UB_ARCHIVE_RECEIPTS required")?,
    );
    ensure!(root.is_absolute(), "receipt directory must be absolute");
    fs::create_dir(&root)?;
    let manifest: Value = serde_json::from_slice(&fs::read(frozen.join("manifest.json"))?)?;
    let mut campaign = Campaign {
        path: root.join("bin"),
        root,
        frozen,
        manifest,
        rows: vec![],
        scenarios: BTreeMap::new(),
    };
    let result = rehearse(&mut campaign);
    campaign.receipt(
        if result.is_ok() {
            "bounded_pass"
        } else {
            "failed"
        },
        result.as_ref().err().map(|e| format!("{e:#}")),
    )?;
    result
}
fn rehearse(c: &mut Campaign) -> Result<()> {
    ensure!(
        c.manifest["phase"] == "preliminary_local_archive",
        "only preliminary inputs admitted in first slice"
    );
    ensure!(
        c.manifest["target"] == "x86_64-unknown-linux-gnu",
        "unsupported platform"
    );
    for key in [
        "source_commit",
        "source_tree",
        "toolchain",
        "version",
        "binary_sha256",
        "archive_sha256",
        "checksum_sha256",
    ] {
        ensure!(
            c.manifest[key].as_str().is_some_and(|s| !s.is_empty()),
            "missing manifest {key}"
        );
    }
    ensure!(
        digest(&fs::read(c.frozen.join(ASSET))?) == c.manifest["archive_sha256"],
        "archive digest mismatch"
    );
    ensure!(
        digest(&fs::read(c.frozen.join(format!("{ASSET}.sha256")))?)
            == c.manifest["checksum_sha256"],
        "checksum digest mismatch"
    );
    fs::create_dir(&c.path)?;
    for tool in [
        "awk",
        "tar",
        "gzip",
        "sha256sum",
        "uname",
        "rm",
        "mkdir",
        "chmod",
        "cp",
        "git",
        "cat",
    ] {
        symlink(format!("/usr/bin/{tool}"), c.path.join(tool))?;
    }
    executable(&c.path.join("curl"), ADAPTER)?;
    // Real compilers are absent from PATH. These tripwires make attempted fallback visible.
    for tool in ["cargo", "rustc", "rustup", "gh", "ub-review"] {
        executable(
            &c.path.join(tool),
            "#!/bin/bash\nprintf '%s\\n' \"$0 $*\" >> \"${POISON_LOG:?}\"\nexit 93\n",
        )?;
    }
    for attempt in 1..=2 {
        let failed = c.resolve("release", "1")?;
        ensure!(!failed.status.success(), "missing archive accepted");
        ensure!(
            fs::read_to_string(c.root.join("resolver-output"))?.is_empty(),
            "failed resolver emitted outputs"
        );
        c.proven(&format!("unavailable_archive_refusal_{attempt}"))?;
    }
    for attempt in 1..=2 {
        let failed = c.resolve("release", "checksum")?;
        ensure!(!failed.status.success(), "bad checksum accepted");
        ensure!(
            String::from_utf8_lossy(&failed.stdout).contains("checksum mismatch"),
            "checksum refusal not reached"
        );
        ensure!(
            fs::read_to_string(c.root.join("resolver-output"))?.is_empty(),
            "checksum rejection emitted outputs"
        );
        c.proven(&format!("checksum_mismatch_refusal_{attempt}"))?;
    }
    ensure!(
        !c.root.join("poison.log").exists(),
        "forbidden fallback attempted"
    );
    // Production auto is deliberately outside the rehearsal: successful PATH selection must be rejected.
    let auto = c.resolve("auto", "0")?;
    success(&auto)?;
    ensure!(
        c.selected().is_err(),
        "PATH substitution passed identity guard"
    );
    c.proven("PATH_selection_rejected")?;
    let workspace = c.root.join("workspace/target/release/ub-review");
    executable(&workspace, "#!/bin/bash\nexit 94\n")?;
    ensure!(
        c.identity(&workspace).is_err(),
        "workspace substitute accepted"
    );
    c.proven("workspace_selection_rejected")?;
    let positive = c.resolve("release", "0")?;
    success(&positive)?;
    let bin = c.selected()?;
    let root = c.root.clone();
    let version = c.cli(&root, &bin, &["--version"])?;
    success(&version)?;
    ensure!(
        String::from_utf8_lossy(&version.stdout).trim()
            == format!(
                "ub-review {}",
                c.manifest["version"].as_str().context("version")?
            ),
        "binary version mismatch"
    );
    c.proven("frozen_resolver_digest_and_version")?;
    // Replacing the installed bytes must be rejected before a command can execute.
    let original = fs::read(&bin)?;
    fs::write(&bin, b"substitute")?;
    ensure!(
        c.identity(&bin).is_err(),
        "installed byte substitution accepted"
    );
    fs::write(&bin, &original)?;
    c.proven("installed_byte_substitution_rejected")?;
    for mixed in [false, true] {
        repository(c, &bin, mixed)?;
    }
    ensure!(
        !c.root.join("poison.log").exists(),
        "compiler/workspace/network substitution invoked"
    );
    ensure!(
        !c.root.join("runtime/ub-review-action-src").exists(),
        "source fallback state exists"
    );
    c.proven("no_compiler_source_fallback_or_remote_mutation")?;
    Ok(())
}
fn assert_preserved_inventory(root: &Path, expected: &BTreeMap<String, String>) -> Result<()> {
    ensure!(
        inventory(root)? == *expected,
        "rollback changed unrelated bytes/inventory"
    );
    Ok(())
}

fn repository(c: &mut Campaign, bin: &Path, mixed: bool) -> Result<()> {
    let repo = c.root.join(if mixed {
        "mixed repo ; literal"
    } else {
        "rust workspace ; literal"
    });
    fs::create_dir(&repo)?;
    write(
        &repo.join("Cargo.toml"),
        "[workspace]\nmembers=[\"core\"]\nresolver=\"3\"\n",
    )?;
    write(
        &repo.join("core/Cargo.toml"),
        "[package]\nname=\"fixture\"\nversion=\"0.0.0\"\nedition=\"2024\"\n",
    )?;
    write(
        &repo.join("core/src/lib.rs"),
        if mixed {
            "unsafe extern \"C\" { fn native(); }\n"
        } else {
            "pub fn answer() -> u32 { 42 }\n"
        },
    )?;
    if mixed {
        write(
            &repo.join("package.json"),
            "{\"scripts\":{\"test\":\"node --test\"}}\n",
        )?;
        write(
            &repo.join("native/boundary.c"),
            "int native(void) { return 0; }\n",
        )?;
    }
    write(&repo.join("unrelated.txt"), "preserve exact bytes\n")?;
    write(
        &repo.join(".github/workflows/existing.yml"),
        "name: existing\non: [pull_request]\njobs:\n  check:\n    runs-on: ubuntu-latest\n    steps:\n      - run: echo existing\n",
    )?;
    for args in [
        vec!["init"],
        vec!["config", "user.name", "Inert Fixture"],
        vec!["config", "user.email", "fixture@example.invalid"],
        vec![
            "remote",
            "add",
            "origin",
            "https://github.com/example/inert",
        ],
        vec!["add", "."],
        vec!["-c", "core.hooksPath=/dev/null", "commit", "-m", "fixture"],
    ] {
        success(&c.command(&repo, Path::new("/usr/bin/git"), &args, &[])?)?;
    }
    let shape = if mixed { "mixed" } else { "rust" };
    let before = inventory(&repo)?;
    write(
        &c.root.join(if mixed {
            "mixed-before.json"
        } else {
            "rust-before.json"
        }),
        serde_json::to_vec_pretty(&before)?,
    )?;
    // Mutually exclusive first-run routes. Manual init is rehearsed and rolled back first.
    success(&c.cli(&repo, bin, &["init", "--no-guide"])?)?;
    let initialized = inventory(&repo)?;
    for _ in 0..2 {
        refused(&c.cli(&repo, bin, &["init", "--no-guide"])?)?;
        refused(&c.cli(&repo, bin, &["enable", "--mode", "advisory", "--inspect"])?)?;
        ensure!(
            inventory(&repo)? == initialized,
            "conflict altered original state"
        );
    }
    c.proven(&format!("{shape}_manual_init_and_twice_conflict_refusal"))?;
    fs::remove_file(repo.join(".ub-review.toml"))?;
    ensure!(
        inventory(&repo)? == before,
        "manual rollback changed unrelated state"
    );
    success(&c.cli(&repo, bin, &["enable", "--mode", "advisory", "--inspect"])?)?;
    let config = fs::read_to_string(repo.join(".ub-review.toml"))?;
    let parsed: toml::Value = toml::from_str(&config)?;
    ensure!(
        parsed["tools"]["ripr"]["enabled"].as_bool() == Some(true),
        "Rust workspace not inspected"
    );
    ensure!(
        parsed["tools"]["unsafe-review"]["enabled"].as_bool() == Some(mixed),
        "native risk shape not inspected"
    );
    let workflow = fs::read(repo.join(".github/workflows/ub-review.yml"))?;
    // Independently parse YAML with declared system Python/PyYAML. No substring-only oracle.
    let yaml_check = "import sys,yaml; d=yaml.safe_load(open(sys.argv[1])); s=[s for j in d['jobs'].values() for s in j['steps'] if str(s.get('uses','')).startswith('EffortlessMetrics/ub-review@')]; assert len(s)==1; w=s[0]['with']; assert w['install-mode']=='release'; assert w['review-mode']=='advisory'; assert 'pull_request_target' not in d and 'pull_request_target' not in d.get(True,{}); print(yaml.__version__)";
    success(&c.command(
        &repo,
        Path::new("/usr/bin/python3"),
        &["-c", yaml_check, ".github/workflows/ub-review.yml"],
        &[],
    )?)?;
    ensure!(
        !String::from_utf8_lossy(&workflow)
            .split_whitespace()
            .any(|word| word == "bun" || word.starts_with("bun:")),
        "maintainer Bun default leaked"
    );
    write(
        &c.root.join(if mixed {
            "mixed-generated.yml"
        } else {
            "rust-generated.yml"
        }),
        &workflow,
    )?;
    c.proven(&format!("{shape}_parsed_advisory_generation"))?;
    let enabled = inventory(&repo)?;
    for _ in 0..2 {
        refused(&c.cli(&repo, bin, &["enable", "--mode", "advisory", "--inspect"])?)?;
        ensure!(inventory(&repo)? == enabled, "replay changed files");
    }
    success(&c.cli(&repo, bin, &["audit-ci", "--out", "owned audit ; literal"])?)?;
    success(&c.cli(
        &repo,
        bin,
        &["setup-ci", "--out", "owned audit ; literal", "--print-pr"],
    )?)?;
    c.proven(&format!(
        "{shape}_twice_enable_replay_refusal_and_audit_preview"
    ))?;
    let audited = inventory(&repo)?;
    for (path, value) in &before {
        ensure!(
            audited.get(path) == Some(value),
            "unrelated bytes changed: {path}"
        );
    }
    let owned: Vec<_> = audited
        .keys()
        .filter(|p| !before.contains_key(*p))
        .cloned()
        .collect();
    ensure!(
        owned.iter().all(|p| p == ".ub-review.toml"
            || p == ".github/workflows/ub-review.yml"
            || p.starts_with("owned audit ; literal/")),
        "unexpected generated paths: {owned:?}"
    );
    // Add a file after setup: rollback must preserve this as well as the original inventory.
    write(
        &repo.join("later-unrelated.txt"),
        "post-setup unrelated state\n",
    )?;
    let mut expected = before;
    expected.insert(
        "later-unrelated.txt".into(),
        inventory(&repo)?
            .get("later-unrelated.txt")
            .context("late sentinel")?
            .clone(),
    );
    for path in owned.iter().filter(|p| !p.ends_with('/')) {
        fs::remove_file(repo.join(path))?;
    }
    let mut dirs: Vec<_> = owned.iter().filter(|p| p.ends_with('/')).collect();
    dirs.sort_by_key(|p| std::cmp::Reverse(p.len()));
    for path in dirs {
        fs::remove_dir(repo.join(path))?;
    }
    assert_preserved_inventory(&repo, &expected)?;
    write(
        &c.root.join(if mixed {
            "mixed-after.json"
        } else {
            "rust-after.json"
        }),
        serde_json::to_vec_pretty(&expected)?,
    )?;
    c.proven(&format!("{shape}_harness_owned_rollback_exact_inventory"))?;
    Ok(())
}

#[test]
fn production_resolver_extraction_retains_release_failure_boundary() -> Result<()> {
    let shell = resolver_shell()?;
    ensure!(
        shell.contains("if [[ \"$mode\" == \"release\" ]]"),
        "release boundary absent"
    );
    ensure!(
        shell.contains("version_output="),
        "version admission absent"
    );
    Ok(())
}

#[test]
fn command_receipts_retain_spawn_failure_and_nonzero_exit() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path().to_path_buf();
    let mut c = Campaign {
        path: root.join("bin"),
        frozen: root.join("frozen"),
        root: root.clone(),
        manifest: json!({}),
        rows: vec![],
        scenarios: BTreeMap::new(),
    };
    ensure!(
        c.command(&root, &root.join("absent"), &[], &[]).is_err(),
        "spawn failure accepted"
    );
    ensure!(c.rows[0]["state"] == "spawn_failed", "spawn failure lost");
    let output = c.command(&root, Path::new("/bin/false"), &[], &[])?;
    ensure!(!output.status.success(), "nonzero exit lost");
    ensure!(
        c.rows[1]["state"] == "exited" && c.rows[1]["exit"] == 1,
        "nonzero receipt lost"
    );
    c.receipt("failed", Some("control".into()))?;
    let parsed: Value = serde_json::from_slice(&fs::read(root.join("receipt.json"))?)?;
    ensure!(
        parsed["commands"].as_array().context("commands")?.len() == 2,
        "attempt omitted"
    );
    Ok(())
}

#[test]
fn inventories_detect_directory_mode_and_identity_rejects_substitutes() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    let dir = root.join("directory");
    fs::create_dir(&dir)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755))?;
    let before = inventory(root)?;
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    ensure!(
        inventory(root)? != before,
        "directory mode mutation invisible"
    );
    let binary = root.join("runtime/ub-review-action-release/ub-review");
    write(&binary, b"frozen bytes")?;
    let c = Campaign {
        root: root.to_path_buf(),
        path: root.join("bin"),
        frozen: root.join("frozen"),
        manifest: json!({"binary_sha256":digest(b"frozen bytes")}),
        rows: vec![],
        scenarios: BTreeMap::new(),
    };
    c.identity(&binary)?;
    let workspace = root.join("target/release/ub-review");
    write(&workspace, b"frozen bytes")?;
    ensure!(
        c.identity(&workspace).is_err(),
        "same-byte workspace substitution accepted"
    );
    write(&binary, b"different bytes")?;
    ensure!(c.identity(&binary).is_err(), "digest substitution accepted");
    fs::remove_file(&binary)?;
    symlink(&workspace, &binary)?;
    ensure!(
        c.identity(&binary).is_err(),
        "symlink substitution accepted"
    );
    Ok(())
}

#[test]
fn pre_cleanup_snapshot_rejects_later_sentinel_corruption() -> Result<()> {
    let temp = tempfile::tempdir()?;
    let original = temp.path().join("original.txt");
    let later = temp.path().join("later-unrelated.txt");
    write(&original, b"original bytes")?;
    write(&later, b"post-setup unrelated state\n")?;
    fs::set_permissions(&later, fs::Permissions::from_mode(0o644))?;
    let expected = inventory(temp.path())?;
    assert_preserved_inventory(temp.path(), &expected)?;
    write(&later, b"corrupted during cleanup\n")?;
    ensure!(
        assert_preserved_inventory(temp.path(), &expected).is_err(),
        "sentinel byte corruption accepted"
    );
    write(&later, b"post-setup unrelated state\n")?;
    assert_preserved_inventory(temp.path(), &expected)?;
    fs::set_permissions(&later, fs::Permissions::from_mode(0o600))?;
    ensure!(
        assert_preserved_inventory(temp.path(), &expected).is_err(),
        "sentinel mode corruption accepted"
    );
    fs::set_permissions(&later, fs::Permissions::from_mode(0o644))?;
    assert_preserved_inventory(temp.path(), &expected)?;
    fs::remove_file(&later)?;
    ensure!(
        assert_preserved_inventory(temp.path(), &expected).is_err(),
        "sentinel deletion accepted"
    );
    write(&later, b"post-setup unrelated state\n")?;
    fs::set_permissions(&later, fs::Permissions::from_mode(0o644))?;
    write(&original, b"changed original bytes")?;
    ensure!(
        assert_preserved_inventory(temp.path(), &expected).is_err(),
        "original file corruption accepted"
    );
    Ok(())
}
