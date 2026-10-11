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
elif url=='https://github.invalid/repos/example/repo/pulls/123/reviews' and method=='POST': result={'id':987,'state':'PENDING'}
elif '/reviews/987/comments' in url and url.startswith('https://github.invalid/') and method=='GET': result=[]
elif url=='https://github.invalid/repos/example/repo/pulls/123/reviews/987/events' and method=='POST':
    if s['post']=='error': status=500; result={'message':'synthetic submit error'}
    else: result={'id':988,'state':'COMMENTED','body':'synthetic submission'}
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
    let (_, head) = c.invoke(git, &repo, &strings(&["rev-parse", "HEAD"]), &[])?;
    let head = head.trim().to_owned();
    let (_, base) = c.invoke(git, &repo, &strings(&["rev-parse", "HEAD~1"]), &[])?;
    let (_, diff) = c.invoke(
        git,
        &repo,
        &strings(&["diff", "--binary", "HEAD~1", "HEAD"]),
        &[],
    )?;
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
        if !verified {
            ensure!(*id == "provider-finding" && verification.contains("review/resolved_candidates.json does not match candidates plus follow-up outputs and prior resolved candidates"), "{id} unexpected packet contract failure: {verification}");
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
        c.scenarios[*id] = json!({"status":if verified {"observed_current_contract"} else {"blocked_packet_contract"},"expected":expected,"gate":gate,"terminal":terminal,"config_sha256":sha(&fs::read(&cfg)?),"transport_sha256":sha(&fs::read(&transport)?),"transport_log_sha256":sha(calls.as_bytes()),"packet_path":out,"gate_sha256":sha(&fs::read(out.join("review/gate_outcome.json"))?),"terminal_sha256":sha(&fs::read(out.join("review/terminal_state.json"))?),"review_sha256":sha(&fs::read(out.join("review/review.json"))?),"wrong_outcome_rejected":true,"packet_contract_verified":verified,"packet_verifier_output":verification});
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

        c.scenarios[id] = json!({"status":"observed_current_contract","post_receipt":receipt,"delivery_transaction":transaction,"delivery_transaction_sha256":sha(&fs::read(root.join("delivery-transaction.json"))?),"post_receipt_sha256":sha(&fs::read(root.join(if expected_ok {"post-result.json"} else {"post-error.json"}))?),"public_payload_sha256":sha(&fs::read(out.join("review/github-review.json"))?),"deterministic_bytes_unchanged":true,"transport_log_sha256":sha(calls.as_bytes()),"expected_exit_success":expected_ok});
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
