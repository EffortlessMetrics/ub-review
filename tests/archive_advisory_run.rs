//! #1341 preliminary archive CLI campaign. Installation stays with #1382.
#![cfg(all(target_os = "linux", target_arch = "x86_64"))]

use anyhow::{Context, Result, ensure};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

const ASSET: &str = "ub-review-x86_64-unknown-linux-gnu.tar.gz";
const CASES: &[&str] = &[
    "off",
    "required-gap",
    "provider-finding",
    "provider-outage",
    "provider-text",
    "missing-key",
    "post-success",
    "post-error",
];

fn sha(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn read(path: &Path) -> Result<Value> {
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<()> {
    fs::create_dir_all(path.parent().context("parent")?)?;
    fs::write(path, bytes)?;
    Ok(())
}
fn script(path: &Path, body: &str) -> Result<()> {
    write(path, body)?;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))?;
    Ok(())
}
fn checked_result(actual: &Value, expected: &Value) -> Result<()> {
    for (key, value) in expected.as_object().context("expected result object")? {
        ensure!(
            actual.get(key) == Some(value),
            "wrong result plane {key}: expected {value}, observed {}",
            actual[key]
        );
    }
    Ok(())
}
const RECONCILIATION_REJECTION: &str = "verify-bun-review-artifacts: review/resolved_candidates.json does not match candidates plus follow-up outputs and prior resolved candidates";
fn packet_verdict(id: &str, verified: bool, output: &str) -> Result<()> {
    if id == "provider-finding" {
        ensure!(
            !verified && output.trim() == RECONCILIATION_REJECTION,
            "frozen structured packet must retain its exact reconciliation rejection: {output}"
        );
    } else {
        ensure!(verified, "{id} packet contract failed: {output}");
    }
    Ok(())
}
fn copy_packet(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_packet(&entry.path(), &target)?;
        } else {
            ensure!(
                entry.file_type()?.is_file(),
                "regular packet files required"
            );
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
// Additive diagnostic: the ordinary verifier must still reject this exact packet.
// Continue only its documented reconciliation and prepare-only missing-post denials, retaining all other
// checks (including the remainder of that function). No production verifier edit.
const UNAFFECTED_CONTRACTS: &str = r#"
import contextlib, importlib.util, io, pathlib, sys
spec=importlib.util.spec_from_file_location('archive_contract',sys.argv[1])
v=importlib.util.module_from_spec(spec); sys.modules[spec.name]=v; spec.loader.exec_module(v)
original=v.fail; denials=[]
def retained_failure(message):
    if message in ('review/resolved_candidates.json does not match candidates plus follow-up outputs and prior resolved candidates','neither post-result.json nor post-error.json exists'):
        denials.append(message)
    else: original(message)
v.fail=retained_failure
with contextlib.redirect_stdout(io.StringIO()):
    result=v.main(['archive_contract',sys.argv[2],'--expected-repo-kind','rust'])
if result!=0 or denials!=['review/resolved_candidates.json does not match candidates plus follow-up outputs and prior resolved candidates','neither post-result.json nor post-error.json exists']: original('expected exact reconciliation and prepared-stage missing-post denials')
print('unaffected contracts checked; reconciliation and prepared-stage missing-post denials retained; full packet remains rejected')
"#;
fn unchanged(before: &[u8], after: &[u8]) -> Result<()> {
    ensure!(
        before == after,
        "delivery rewrote imported deterministic outcome"
    );
    Ok(())
}

struct Campaign {
    root: PathBuf,
    bin: PathBuf,
    manifest: Value,
    intake: Value,
    commands: Vec<Value>,
    scenarios: Value,
}
impl Campaign {
    fn receipt(&self, state: &str, error: Option<String>) -> Result<()> {
        write(
            &self.root.join("receipt.json"),
            serde_json::to_vec_pretty(&json!({
                "schema":"ub-review.archive_advisory_campaign.v1", "phase":"preliminary_local_archive",
                "status":state, "error":error, "manifest":self.manifest, "intake":self.intake,
                "harness_sha256":sha(include_bytes!("archive_advisory_run.rs")), "transport_adapter_sha256":sha(TRANSPORT.as_bytes()),
                "commands":self.commands, "scenarios":self.scenarios,
                "boundaries":{"provider":"inert synthetic curl adapter", "github":"inert synthetic curl adapter", "ambient_environment":"cleared", "live_calls":false},
                "pending":["final #816 bytes/provenance", "live provider/delivery", "Action execution", "platform matrix", "#957-#962 terminal/outcome authority", "#1352 disabled applicability", "#959 confirmed delivery", "general process-tree containment"]
            }))?,
        )
    }
    fn invoke(
        &mut self,
        program: &Path,
        cwd: &Path,
        args: &[String],
        env: &[(&str, String)],
    ) -> Result<(bool, String)> {
        if program == self.bin {
            ensure!(
                fs::symlink_metadata(program)?.is_file(),
                "selected binary must remain regular"
            );
            ensure!(
                sha(&fs::read(program)?) == self.manifest["binary_sha256"],
                "selected binary moved"
            );
        }
        let index = self.commands.len();
        let logs = self.root.join(format!("logs/{index}"));
        fs::create_dir_all(&logs)?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .env_clear()
            .env("PATH", self.root.join("bin"))
            .env("HOME", &self.root)
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(fs::File::create(logs.join("stdout"))?)
            .stderr(fs::File::create(logs.join("stderr"))?);
        for (key, value) in env {
            cmd.env(key, value);
        }
        self.commands.push(json!({"program":program,"cwd":cwd,"argv":args,"state":"attempted", "environment":env.iter().filter(|(k,_)| !k.ends_with("KEY") && *k != "GITHUB_TOKEN").map(|(k,v)| json!([k,v])).collect::<Vec<_>>(), "timeout_seconds":30}));
        self.receipt("running", None)?;
        let mut child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                self.commands[index]["state"] = json!("spawn_failed");
                return Err(error.into());
            }
        };
        let status = match child.wait_timeout(Duration::from_secs(30))? {
            Some(status) => status,
            None => {
                child.kill()?;
                child.wait()?;
                self.commands[index]["state"] = json!("timed_out_direct_child_reaped");
                anyhow::bail!("command timed out: {}", logs.display());
            }
        };
        let stdout = fs::read(logs.join("stdout"))?;
        let stderr = fs::read(logs.join("stderr"))?;
        self.commands[index]["state"] = json!("exited");
        self.commands[index]["exit"] = json!(status.code());
        self.commands[index]["stdout_sha256"] = json!(sha(&stdout));
        self.commands[index]["stderr_sha256"] = json!(sha(&stderr));
        self.receipt("running", None)?;
        Ok((
            status.success(),
            format!(
                "{}{}",
                String::from_utf8_lossy(&stdout),
                String::from_utf8_lossy(&stderr)
            ),
        ))
    }
    fn fixture_git(&mut self, cwd: &Path, args: &[String]) -> Result<String> {
        let (ok, text) = self.invoke(Path::new("/usr/bin/git"), cwd, args, &[])?;
        ensure!(ok, "fixture git {args:?} failed: {text}");
        Ok(text)
    }
    fn cli(&mut self, args: &[String], env: &[(&str, String)]) -> Result<(bool, String)> {
        let bin = self.bin.clone();
        let root = self.root.clone();
        self.invoke(&bin, &root, args, env)
    }
}

// No network client is present: even an unknown URL cannot escape this adapter.
const TRANSPORT: &str = r#"#!/usr/bin/python3
import json, os, sys
a=sys.argv[1:]; method='GET'; url=''; body=None
headers=sys.stdin.read()
if 'Authorization: Bearer synthetic-fixture-value' not in headers: sys.exit(94)
for i,v in enumerate(a):
    if v=='-X': method=a[i+1]
    if v=='--data-binary': body=json.load(open(a[i+1][1:])); method='POST' if method=='GET' else method
    if v.startswith('https://'): url=v
s=json.load(open(os.environ['CAMPAIGN_TRANSPORT']))
status=200
if url=='https://provider.invalid/v1/chat/completions' and method=='POST':
    if s['provider']=='outage': status=503; result={'error':{'message':'synthetic unavailable'}}
    else: result={'choices':[{'message':{'content':s['content']}}]}
elif url=='https://github.invalid/repos/example/repo/pulls/123' and method=='GET': result={'head':{'sha':s['head']}}
elif url=='https://github.invalid/repos/example/repo/pulls/123/reviews' and method=='POST': status=201; result={'id':987,'state':'PENDING'}
elif '/reviews/987/comments' in url and url.startswith('https://github.invalid/') and method=='GET': result=[]
elif url=='https://github.invalid/repos/example/repo/pulls/123/reviews/987/events' and method=='POST':
    if s['post']=='error': status=500; result={'message':'synthetic submit error'}
    elif s['post']=='mismatched': result={'id':988,'state':'COMMENTED','body':'synthetic submission'}
    else: result={'id':987,'state':'COMMENTED','body':'synthetic submission'}
elif url=='https://github.invalid/repos/example/repo/pulls/123/reviews/987' and method=='DELETE': status=204; result={}
else:
    print('unallowlisted transport: '+method+' '+url,file=sys.stderr); sys.exit(95)
with open(os.environ['CAMPAIGN_TRANSPORT_LOG'],'a') as f: f.write(json.dumps({'method':method,'url':url,'body':body,'status':status})+'\n')
print(json.dumps(result)); print('UB_REVIEW_HTTP_STATUS:'+str(status))
sys.exit(22 if status>=400 else 0)
"#;

fn intake(packet: &Path, installed: &Path) -> Result<(PathBuf, Value, Value)> {
    let manifest = read(&packet.join("manifest.json"))?;
    ensure!(
        manifest["phase"] == "preliminary_local_archive"
            && manifest["target"] == "x86_64-unknown-linux-gnu",
        "preliminary Linux archive required"
    );
    for (file, key) in [
        (ASSET, "archive_sha256"),
        (
            "ub-review-x86_64-unknown-linux-gnu.tar.gz.sha256",
            "checksum_sha256",
        ),
    ] {
        ensure!(
            sha(&fs::read(packet.join(file))?) == manifest[key],
            "frozen {key} mismatch"
        );
    }
    let receipt = read(&installed.join("receipt.json"))?;
    ensure!(
        receipt["schema"] == "ub-review.archive_onboarding_rehearsal.v1"
            && receipt["status"] == "bounded_pass",
        "completed #1382 strict-install receipt required"
    );
    ensure!(
        receipt["frozen_manifest"] == manifest,
        "installation used different frozen bytes"
    );
    let bin = installed.join("runtime/ub-review-action-release/ub-review");
    ensure!(
        fs::symlink_metadata(&bin)?.is_file(),
        "regular installed executable required"
    );
    ensure!(
        sha(&fs::read(&bin)?) == manifest["binary_sha256"],
        "installed executable digest mismatch"
    );
    let identities = json!({"archive_path":packet,"install_receipt_path":installed,"install_receipt_sha256":sha(&fs::read(installed.join("receipt.json"))?), "install_harness_sha256":receipt["harness_sha256"], "archive_bytes":fs::metadata(packet.join(ASSET))?.len(), "binary_path":bin,"manifest_sha256":sha(&fs::read(packet.join("manifest.json"))?), "provenance":"source/toolchain manifest labels only; not cryptographic binary provenance"});
    Ok((bin, manifest, identities))
}

fn config(required: bool) -> String {
    let mut text = String::from(
        "profile = \"gh-runner\"\n[repo]\nkind = \"rust\"\nledger = \"\"\n[review]\nenable_default_lanes = false\n[[lanes]]\nid = \"archive-lane\"\nrole = \"Controlled archive fixture\"\nfocus = \"Inspect changed return value\"\n",
    );
    for tool in [
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
        text.push_str(&format!(
            "\n[tools.{tool}]\nenabled = false\nrequired = {}\n",
            required && tool == "tokmd"
        ));
    }
    text
}
fn strings(args: &[&str]) -> Vec<String> {
    args.iter().map(|s| (*s).to_owned()).collect()
}

fn campaign(c: &mut Campaign) -> Result<()> {
    fs::create_dir(c.root.join("bin"))?;
    for tool in ["git", "uname", "sh"] {
        symlink(format!("/usr/bin/{tool}"), c.root.join("bin").join(tool))?;
    }
    script(&c.root.join("bin/curl"), TRANSPORT)?;
    let (ok, text) = c.cli(&strings(&["--version"]), &[])?;
    ensure!(
        ok && text.trim()
            == format!(
                "ub-review {}",
                c.manifest["version"].as_str().context("version")?
            ),
        "archive version mismatch: {text}"
    );
    let repo = c.root.join("fixture repo");
    fs::create_dir(&repo)?;
    write(
        &repo.join("src/lib.rs"),
        "pub fn answer() -> usize { 41 }\n",
    )?;
    let git = Path::new("/usr/bin/git");
    for args in [
        strings(&["init"]),
        strings(&["config", "user.email", "archive@example.invalid"]),
        strings(&["config", "user.name", "Archive Fixture"]),
        strings(&["add", "."]),
        strings(&["commit", "-m", "base"]),
    ] {
        let (ok, text) = c.invoke(git, &repo, &args, &[])?;
        ensure!(ok, "fixture git: {text}");
    }
    write(
        &repo.join("src/lib.rs"),
        "pub fn answer() -> usize { 42 }\n",
    )?;
    for args in [strings(&["add", "."]), strings(&["commit", "-m", "head"])] {
        let (ok, text) = c.invoke(git, &repo, &args, &[])?;
        ensure!(ok, "fixture git: {text}");
    }
    let head = c.fixture_git(&repo, &strings(&["rev-parse", "HEAD"]))?;
    let head = head.trim().to_owned();
    let base = c.fixture_git(&repo, &strings(&["rev-parse", "HEAD~1"]))?;
    let diff = c.fixture_git(&repo, &strings(&["diff", "--binary", "HEAD~1", "HEAD"]))?;
    write(&c.root.join("input.patch"), diff.as_bytes())?;
    c.intake["input"] = json!({"base":base.trim(),"head":head,"diff_sha256":sha(diff.as_bytes()),"source_sha256":sha(&fs::read(repo.join("src/lib.rs"))?)});
    for id in CASES.iter().take(6) {
        let root = c.root.join(id);
        fs::create_dir(&root)?;
        let cfg = root.join("config.toml");
        write(&cfg, config(*id == "required-gap"))?;
        let content = if *id == "provider-text" {
            "Readable synthetic lane evidence without structured JSON".to_owned()
        } else {
            json!({"summary":"Archive fixture needs a boundary regression for the changed return value.","inline_comments":[],"summary_only_findings":[{"severity":"medium","confidence":"high","reason":"Archive fixture needs a boundary regression for the changed return value.","evidence":"src/lib.rs return changes from 41 to 42 without a regression test."}],"internal_audit":{"surfaces_checked":["PRIVATE_ARCHIVE_AUDIT_SENTINEL"],"strongest_rejected_hypothesis":"PRIVATE_ARCHIVE_AUDIT_SENTINEL","remaining_local_uncertainty":null}}).to_string()
        };
        let transport = root.join("transport.json");
        let log = root.join("transport.ndjson");
        write(
            &transport,
            serde_json::to_vec(
                &json!({"provider":if *id=="provider-outage" {"outage"} else {"success"},"post":"success","head":head,"content":content}),
            )?,
        )?;
        let out = root.join("packet");
        let mut args = strings(&["run", "--config"]);
        args.push(cfg.display().to_string());
        args.extend(strings(&["--root"]));
        args.push(repo.display().to_string());
        args.extend(strings(&["--base", "HEAD~1", "--head", "HEAD", "--out"]));
        args.push(out.display().to_string());
        args.extend(strings(&[
            "--mode",
            if id.starts_with("provider") || *id == "missing-key" {
                "review-byok"
            } else {
                "intelligent-ci"
            },
            "--fail-on-gate",
            "false",
            "--model-mode",
            if id.starts_with("provider") || *id == "missing-key" {
                "auto"
            } else {
                "off"
            },
            "--provider-policy",
            "minimax-only",
            "--minimax-provider-kind",
            "openai",
            "--lanes",
            "archive-lane",
            "--model-concurrency",
            "1",
            "--max-model-calls",
            "1",
            "--max-inline-comments",
            "0",
            "--model-timeout-sec",
            "5",
            "--run-pass",
            "opened",
            "--posting",
            if *id == "provider-finding" {
                "review"
            } else {
                "artifact-only"
            },
            "--github-repo",
            "example/repo",
            "--github-pull-number",
            "123",
            "--no-github-summary",
        ]));
        let mut env = vec![
            ("CAMPAIGN_TRANSPORT", transport.display().to_string()),
            ("CAMPAIGN_TRANSPORT_LOG", log.display().to_string()),
            (
                "UB_REVIEW_MINIMAX_API_URL",
                "https://provider.invalid/v1/chat/completions".to_owned(),
            ),
        ];
        if *id != "missing-key" {
            env.push((
                "UB_REVIEW_MINIMAX_API_KEY",
                "synthetic-fixture-value".to_owned(),
            ));
        }
        let (ok, text) = c.cli(&args, &env)?;
        ensure!(ok, "{id} run failed: {text}");
        let verifier =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("scripts/verify-bun-review-artifacts.py");
        c.intake["packet_verifier_sha256"] = json!(sha(&fs::read(&verifier)?));
        let (verified, verification) = c.invoke(
            Path::new("/usr/bin/python3"),
            &root,
            &[
                verifier.display().to_string(),
                out.display().to_string(),
                "--expected-repo-kind".to_owned(),
                "rust".to_owned(),
            ],
            &[],
        )?;
        packet_verdict(id, verified, &verification)?;
        if *id == "provider-finding" {
            ensure!(
                c.manifest["binary_sha256"]
                    == "7b0379029b108a264222e01cabf58a0f3c471ae845bb24b2cebea1f75750c8b6",
                "known reconciliation observation is pinned to selected binary bytes; refresh expectations for other bytes"
            );
            c.intake["unaffected_contract_driver_sha256"] =
                json!(sha(UNAFFECTED_CONTRACTS.as_bytes()));
            let check = |packet: &Path| {
                vec![
                    "-c".to_owned(),
                    UNAFFECTED_CONTRACTS.to_owned(),
                    verifier.display().to_string(),
                    packet.display().to_string(),
                ]
            };
            let (ok, text) = c.invoke(Path::new("/usr/bin/python3"), &root, &check(&out), &[])?;
            ensure!(ok, "unaffected structured packet contracts failed: {text}");
            let corrupt = root.join("invalid-gate-control");
            copy_packet(&out, &corrupt)?;
            let path = corrupt.join("review/gate_outcome.json");
            let mut gate = read(&path)?;
            gate["schema"] = json!("CONTROL_INVALID_SCHEMA");
            write(&path, serde_json::to_vec_pretty(&gate)?)?;
            let (ok, text) =
                c.invoke(Path::new("/usr/bin/python3"), &root, &check(&corrupt), &[])?;
            ensure!(
                !ok && text.contains("gate outcome has wrong schema: 'CONTROL_INVALID_SCHEMA'"),
                "unrelated gate corruption escaped retained denial: {text}"
            );
        }
        let gate = read(&out.join("review/gate_outcome.json"))?;
        let terminal = read(&out.join("review/terminal_state.json"))?;
        let review = read(&out.join("review/review.json"))?;
        let expected = match *id {
            "required-gap" => {
                json!({"conclusion":"inconclusive","gate_result":"not_proven","analysis_result":"not_proven","publication_result":"not_proven"})
            }
            "provider-finding" => {
                json!({"conclusion":"pass","gate_result":"pass","analysis_result":"findings","publication_result":"posted"})
            }
            "provider-text" => {
                json!({"conclusion":"pass","gate_result":"pass","analysis_result":"limited","publication_result":"not_needed"})
            }
            "off" => {
                json!({"conclusion":"pass","gate_result":"not_proven","analysis_result":"not_proven","publication_result":"not_needed"})
            }
            _ => {
                json!({"conclusion":"pass","gate_result":"not_proven","analysis_result":"not_proven","publication_result":"not_proven"})
            }
        };
        checked_result(&gate, &expected)?;
        let mut wrong = expected.clone();
        wrong["conclusion"] = json!("fail");
        ensure!(
            checked_result(&gate, &wrong).is_err(),
            "wrong-outcome control accepted"
        );
        let calls = if log.exists() {
            fs::read_to_string(&log)?
        } else {
            String::new()
        };
        if *id == "off" || *id == "required-gap" || *id == "missing-key" {
            ensure!(calls.is_empty(), "{id} invoked transport");
        } else {
            ensure!(
                calls.lines().count() == 2
                    || *id == "provider-outage" && calls.lines().count() == 1,
                "{id} unexpected provider denominator"
            );
        }
        if *id == "provider-finding" {
            let payload = fs::read_to_string(out.join("review/github-review.json"))?;
            ensure!(
                payload.contains("Archive fixture needs a boundary regression"),
                "public finding lost"
            );
            ensure!(
                !payload.contains("PRIVATE_ARCHIVE_AUDIT_SENTINEL"),
                "private audit leaked"
            );
            let audit =
                fs::read_to_string(out.join("review/model/archive-lane/internal_audit.json"))?;
            ensure!(
                audit.contains("PRIVATE_ARCHIVE_AUDIT_SENTINEL"),
                "private positive control lost"
            );
            ensure!(
                !out.join("review/post-result.json").exists(),
                "prepared payload claimed transport confirmation"
            );
            posts(c, &out, &head)?;
        }
        if *id == "provider-text" {
            ensure!(
                review["model_lanes"]
                    .as_array()
                    .context("lanes")?
                    .iter()
                    .any(|r| r["status"] == "degraded"),
                "text lane was not retained as degraded"
            );
        }
        c.scenarios[*id] = json!({"status":if verified {"observed_current_contract"} else {"blocked_packet_contract"},"expected":expected,"gate":gate,"terminal":terminal,"config_sha256":sha(&fs::read(&cfg)?),"transport_sha256":sha(&fs::read(&transport)?),"transport_log_sha256":sha(calls.as_bytes()),"packet_path":out,"gate_sha256":sha(&fs::read(out.join("review/gate_outcome.json"))?),"terminal_sha256":sha(&fs::read(out.join("review/terminal_state.json"))?),"review_sha256":sha(&fs::read(out.join("review/review.json"))?),"wrong_outcome_rejected":true,"unaffected_contracts_checked":true,"invalid_gate_control_rejected":*id=="provider-finding","packet_contract_verified":verified,"packet_verifier_output":verification});
        c.receipt("running", None)?;
    }
    ensure!(
        c.scenarios
            .as_object()
            .context("scenarios")?
            .values()
            .all(|r| r["status"] != "not_run"),
        "scenario omitted"
    );
    Ok(())
}
fn posts(c: &mut Campaign, out: &Path, head: &str) -> Result<()> {
    let before = fs::read(out.join("review/gate_outcome.json"))?;
    for (id, mode, expected_ok) in [
        ("post-success", "success", true),
        ("post-error", "error", false),
    ] {
        let root = c.root.join(id);
        fs::create_dir(&root)?;
        let transport = root.join("transport.json");
        let log = root.join("transport.ndjson");
        write(
            &transport,
            serde_json::to_vec(&json!({"head":head,"post":mode}))?,
        )?;
        let mut args = strings(&["post", "--review-json"]);
        args.push(out.join("review/github-review.json").display().to_string());
        args.extend(strings(&["--out"]));
        args.push(root.display().to_string());
        args.extend(strings(&[
            "--repo",
            "example/repo",
            "--pull-number",
            "123",
            "--github-api-url",
            "https://github.invalid",
            "--github-token",
            "synthetic-fixture-value",
            "--fail-on-post-error",
        ]));
        let (ok, text) = c.cli(
            &args,
            &[
                ("CAMPAIGN_TRANSPORT", transport.display().to_string()),
                ("CAMPAIGN_TRANSPORT_LOG", log.display().to_string()),
            ],
        )?;
        ensure!(ok == expected_ok, "{id} unexpected post exit: {text}");
        unchanged(&before, &fs::read(out.join("review/gate_outcome.json"))?)?;
        let receipt = read(&root.join(if expected_ok {
            "post-result.json"
        } else {
            "post-error.json"
        }))?;
        checked_result(
            &receipt,
            &if expected_ok {
                json!({"schema_version":1,"status":"ok","http_status":200,"comments":0})
            } else {
                json!({"schema_version":1,"status":"failed","failure_tolerated":false,"fail_on_post_error":true,"http_status":null,"error_kind":"failed","failure_stage":"unknown"})
            },
        )?;
        let pending = read(&root.join("pending-review-stdout.json"))?;
        checked_result(&pending, &json!({"id":987,"state":"PENDING"}))?;
        if expected_ok {
            checked_result(&receipt["response"], &json!({"id":987,"state":"COMMENTED"}))?;
            ensure!(
                receipt["response"]["id"] == pending["id"],
                "submitted review identity does not match actual pending response"
            );
        }
        let calls = fs::read_to_string(&log)?;
        ensure!(
            calls.contains("reviews/987/events"),
            "{id} never reached submit boundary"
        );
        ensure!(
            !calls.contains("PRIVATE_ARCHIVE_AUDIT_SENTINEL"),
            "audit reached outbound payload"
        );
        let transaction = read(&root.join("delivery-transaction.json"))?;
        if !expected_ok {
            checked_result(
                &transaction,
                &json!({"state":"cleaned_up","cleanup":{"status":"succeeded"}}),
            )?;
            let rows = calls
                .lines()
                .map(serde_json::from_str::<Value>)
                .collect::<std::result::Result<Vec<_>, _>>()?;
            ensure!(
                rows.iter().any(|r| r["method"] == "POST"
                    && r["url"]
                        .as_str()
                        .is_some_and(|u| u.ends_with("/reviews/987/events"))
                    && r["status"] == 500),
                "missing failed submit control"
            );
            ensure!(
                rows.iter().any(|r| r["method"] == "DELETE"
                    && r["url"]
                        .as_str()
                        .is_some_and(|u| u.ends_with("/reviews/987"))
                    && r["status"] == 204),
                "missing cleanup control"
            );
        }

        if expected_ok {
            let mismatch = root.join("mismatched-identity");
            fs::create_dir(&mismatch)?;
            let mismatch_config = mismatch.join("transport.json");
            let mismatch_log = mismatch.join("transport.ndjson");
            write(
                &mismatch_config,
                serde_json::to_vec(&json!({"head":head,"post":"mismatched"}))?,
            )?;
            let mut mismatch_args = args.clone();
            let out_index = mismatch_args
                .iter()
                .position(|a| a == "--out")
                .context("post output flag")?
                + 1;
            mismatch_args[out_index] = mismatch.display().to_string();
            let (accepted, text) = c.cli(
                &mismatch_args,
                &[
                    ("CAMPAIGN_TRANSPORT", mismatch_config.display().to_string()),
                    ("CAMPAIGN_TRANSPORT_LOG", mismatch_log.display().to_string()),
                ],
            )?;
            ensure!(
                accepted,
                "legacy mismatch observation changed; refresh against selected bytes: {text}"
            );
            let mismatched_receipt = read(&mismatch.join("post-result.json"))?;
            ensure!(
                checked_result(
                    &mismatched_receipt["response"],
                    &json!({"id":987,"state":"COMMENTED"})
                )
                .is_err(),
                "mismatched review identity accepted by campaign oracle"
            );
            unchanged(&before, &fs::read(out.join("review/gate_outcome.json"))?)?;
            c.intake["mismatched_identity_control"] = json!({"legacy_cli_accepted":true,"campaign_identity_oracle_rejected":true,"post_receipt":mismatched_receipt,"post_receipt_sha256":sha(&fs::read(mismatch.join("post-result.json"))?),"transport_log_sha256":sha(&fs::read(mismatch_log)?)});
        }
        c.scenarios[id] = json!({"status":"observed_current_contract","post_receipt":receipt,"pending_review":pending,"pending_review_sha256":sha(&fs::read(root.join("pending-review-stdout.json"))?),"delivery_transaction":transaction,"delivery_transaction_sha256":sha(&fs::read(root.join("delivery-transaction.json"))?),"post_receipt_sha256":sha(&fs::read(root.join(if expected_ok {"post-result.json"} else {"post-error.json"}))?),"public_payload_sha256":sha(&fs::read(out.join("review/github-review.json"))?),"deterministic_bytes_unchanged":true,"transport_log_sha256":sha(calls.as_bytes()),"expected_exit_success":expected_ok});
    }
    Ok(())
}

#[test]
#[ignore = "requires selected preliminary archive and completed #1382 install receipts; see docs/ARCHIVE_ADVISORY_RUN.md"]
fn frozen_archive_advisory_campaign() -> Result<()> {
    let packet =
        PathBuf::from(std::env::var("UB_ARCHIVE_PACKET").context("UB_ARCHIVE_PACKET required")?)
            .canonicalize()?;
    let installed = PathBuf::from(
        std::env::var("UB_ADVISORY_INSTALL_RECEIPTS")
            .context("UB_ADVISORY_INSTALL_RECEIPTS required")?,
    )
    .canonicalize()?;
    let root = PathBuf::from(
        std::env::var("UB_ADVISORY_RECEIPTS").context("UB_ADVISORY_RECEIPTS required")?,
    );
    ensure!(
        root.is_absolute(),
        "new absolute receipt directory required"
    );
    fs::create_dir(&root)?;
    let (bin, manifest, intake) = intake(&packet, &installed)?;
    let scenarios = CASES
        .iter()
        .map(|id| ((*id).to_owned(), json!({"status":"not_run"})))
        .collect::<serde_json::Map<String, Value>>();
    let mut c = Campaign {
        root,
        bin,
        manifest,
        intake,
        commands: vec![],
        scenarios: Value::Object(scenarios),
    };
    let result = campaign(&mut c);
    c.receipt(
        if result.is_ok() {
            "bounded_observations_with_blockers"
        } else {
            "failed"
        },
        result.as_ref().err().map(|e| format!("{e:#}")),
    )?;
    result
}

#[test]
fn campaign_oracle_rejects_wrong_result_and_delivery_rewrite() -> Result<()> {
    let expected = json!({"conclusion":"inconclusive","gate_result":"not_proven"});
    checked_result(&expected, &expected)?;
    ensure!(
        checked_result(
            &json!({"conclusion":"pass","gate_result":"pass"}),
            &expected
        )
        .is_err(),
        "wrong outcome survived"
    );
    unchanged(
        b"retained deterministic receipt",
        b"retained deterministic receipt",
    )?;
    ensure!(
        unchanged(b"retained deterministic receipt", b"rewritten by delivery").is_err(),
        "delivery override survived"
    );
    Ok(())
}

#[test]
fn fixture_identity_and_diff_fail_closed_on_real_git_errors() -> Result<()> {
    let root = tempfile::tempdir()?;
    let cwd = root.path().join("not-a-repository");
    fs::create_dir(&cwd)?;
    let mut c = Campaign {
        root: root.path().to_owned(),
        bin: root.path().join("unused-binary"),
        manifest: json!({}),
        intake: json!({}),
        commands: vec![],
        scenarios: json!({}),
    };
    for args in [
        strings(&["rev-parse", "HEAD"]),
        strings(&["rev-parse", "HEAD~1"]),
        strings(&["diff", "--binary", "HEAD~1", "HEAD"]),
    ] {
        let result = c.fixture_git(&cwd, &args);
        let error = match result {
            Ok(text) => anyhow::bail!("failed Git output accepted as identity/patch: {text}"),
            Err(error) => error,
        };
        ensure!(
            error.to_string().contains("fixture git") && error.to_string().contains("failed:"),
            "wrong failure route: {error}"
        );
        ensure!(
            c.intake.get("input").is_none() && !root.path().join("input.patch").exists(),
            "failed git accepted input evidence"
        );
    }
    ensure!(
        c.commands.len() == 3
            && c.commands
                .iter()
                .all(|r| r["state"] == "exited" && r["exit"] != 0),
        "failure command receipts omitted"
    );
    Ok(())
}

#[test]
fn known_packet_denial_cannot_accept_success_or_unrelated_failures() -> Result<()> {
    packet_verdict("provider-finding", false, RECONCILIATION_REJECTION)?;
    ensure!(
        packet_verdict("provider-finding", true, "").is_err(),
        "changed expected success accepted"
    );
    ensure!(
        packet_verdict("provider-finding", false, "gate outcome has wrong schema").is_err(),
        "unrelated rejection accepted"
    );
    ensure!(
        packet_verdict(
            "provider-finding",
            false,
            &format!("{RECONCILIATION_REJECTION}\nextra error")
        )
        .is_err(),
        "extra rejection masked"
    );
    ensure!(
        packet_verdict("off", false, RECONCILIATION_REJECTION).is_err(),
        "known gap widened to other modes"
    );
    Ok(())
}
