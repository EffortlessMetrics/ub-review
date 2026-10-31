use std::fs;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

use anyhow::{Result, bail};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use wait_timeout::ChildExt;

const PATCH: &[u8] = b"diff --git a/src/lib.rs b/src/lib.rs\n--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1 +1 @@\n-safe();\n+danger();\n";

fn hash(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn domain_hash(domain: &str, bytes: &[u8]) -> String {
    let mut input = domain.as_bytes().to_vec();
    input.push(0);
    input.extend(bytes);
    hash(&input)
}

struct Fixture {
    root: TempDir,
    intake: Value,
    source_json: String,
}

impl Fixture {
    fn new() -> Result<Self> {
        Self::with_patch(PATCH)
    }

    fn with_patch(patch: &[u8]) -> Result<Self> {
        let root = tempfile::tempdir()?;
        let write = |name: &str, bytes: &[u8]| -> Result<Value> {
            fs::write(root.path().join(name), bytes)?;
            Ok(json!({"path":name,"sha256":hash(bytes)}))
        };
        let head = "a".repeat(40);
        let diff_digest = domain_hash("ub-review.revision-identity.diff.v1", patch);
        let paths_digest = domain_hash(
            "ub-review.revision-identity.changed-paths.v1",
            b"src/lib.rs\0",
        );
        let canonical = format!(
            "ub-review.revision-identity.v1\nsemantics=candidate_head\nbase={} {}\nhead={head} {}\nreviewed={head} {}\nmerge=-\nchanged_paths={paths_digest}\ndiff={diff_digest}\n",
            "c".repeat(40),
            "d".repeat(40),
            "b".repeat(40),
            "b".repeat(40)
        );
        let revision_digest = domain_hash(
            "ub-review.revision-identity.digest.v1",
            canonical.as_bytes(),
        );
        let revision = format!(
            r#"{{"digest":"{revision_digest}","semantics":"candidate_head","reviewed_commit":"{head}"}}"#
        );
        let subject = format!(
            r#"{{"repository":"owner/repo","review_spec_digest":"{}","pass_id":"pass-1","attempt_id":"attempt-1"}}"#,
            "e".repeat(64)
        );
        // Freeze canonical producer field order independently of the CLI decoder.
        let graph = format!(
            r#"{{"schema":"ub-review.claim_graph.v1","head_sha":"{head}","revision":{revision},"claims":[{{"id":"A","subject":"admitted claim","source_lane":"recorded","state":"Confirmed","supporting_evidence":[{{"class":"ExactCitation","reference":"fact#1","detail":"citation"}}],"contradicting_evidence":[],"relevance":{{"kind":"ChangedLine","explanation":"changed source"}},"severity":"high"}}],"topics":[],"conflicts":[],"evidence_gaps":[],"mode":"active"}}"#
        );
        let evidence = b"source citation";
        let evidence_digest = hash(evidence);
        let ci = b"{\"result\":\"not_proven\"}\n\0\xff";
        let ci_digest = hash(ci);
        let source = format!(
            r#"{{"subject":{subject},"revision_canonical":{},"graph":{graph},"review_complete":true,"evidence":[{{"reference":"fact#1","digest":"{evidence_digest}"}}],"imported_ci":{{"digest":"{ci_digest}"}}}}"#,
            serde_json::to_string(&canonical)?
        );
        let source_digest = domain_hash("ub-review.recorded-judgment-source.v1", source.as_bytes());
        let body = b"This summary claim remains material without an inline anchor.";
        let body_digest = hash(body);
        let delivery =
            format!(r#"{{"repository":"owner/repo","pull_number":7,"head_sha":"{head}"}}"#);
        let public = format!(
            r#"{{"delivery_subject":{delivery},"diff_digest":"{diff_digest}","materials":[{{"claim_id":"A","body_ref":"public#A","body_digest":"{body_digest}","class":"public"}}],"anchors":[]}}"#
        );
        let public_digest = domain_hash("ub-review.recorded-public-source.v1", public.as_bytes());
        let answer = format!(
            r#"{{"schema":"ub-review.recorded-judgment.v1","subject":{subject},"revision":{revision},"source_manifest_digest":"{source_digest}","review_result":"findings","selections":[{{"claim_id":"A","evidence_refs":["fact#1"]}}]}}"#
        );
        let answer_digest = hash(answer.as_bytes());
        let plan = format!(
            r#"{{"schema":"ub-review.recorded-public-plan.v1","judgment_digest":"{answer_digest}","revision":{revision},"subject":{subject},"delivery_subject":{delivery},"public_source_digest":"{public_digest}","quiet":false,"items":[{{"claim_id":"A","disposition":{{"kind":"summary","body_ref":"public#A","body_digest":"{body_digest}"}}}}]}}"#
        );
        let intake = json!({
            "schema":"ub-review.recorded-file-input.v1",
            "revision":write("revision.txt", canonical.as_bytes())?,
            "graph":write("graph.json", graph.as_bytes())?,
            "subject":serde_json::from_str::<Value>(&subject)?,
            "review_complete":true,"source_manifest_digest":source_digest,
            "evidence":[{"reference":"fact#1","file":write("evidence.bin",evidence)?}],
            "imported_ci":write("ci.bin",ci)?,
            "delivery_subject":serde_json::from_str::<Value>(&delivery)?,
            "diff":write("diff.patch",patch)?,"public_source_digest":public_digest,
            "materials":[{"claim_id":"A","body_ref":"public#A","class":"public","file":write("body.txt",body)?}],
            "anchors":[],"answer":write("answer.json",answer.as_bytes())?,"plan":write("plan.json",plan.as_bytes())?
        });
        Ok(Self {
            root,
            intake,
            source_json: source,
        })
    }

    fn invoke(&self, operation: &str, extras: &[&str]) -> Result<Output> {
        let bytes = serde_json::to_vec(&self.intake)?;
        fs::write(self.root.path().join("intake.json"), &bytes)?;
        let pin = hash(&bytes);
        self.invoke_pin(operation, &pin, extras)
    }

    fn invoke_pin(&self, operation: &str, pin: &str, extras: &[&str]) -> Result<Output> {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ub-review"));
        command
            .args(["recorded-review", operation, "--evidence-root"])
            .arg(self.root.path())
            .args(["--intake", "intake.json", "--intake-sha256", pin])
            .args(extras)
            .current_dir(self.root.path())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = command.spawn()?;
        if child.wait_timeout(Duration::from_secs(20))?.is_none() {
            child.kill()?;
            child.wait()?;
            bail!("recorded CLI timed out");
        }
        Ok(child.wait_with_output()?)
    }
}

fn rejected(output: Output, message: &str) {
    assert!(
        !output.status.success(),
        "unexpected success: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    assert!(
        output.stdout.is_empty(),
        "failed admission emitted a partial view"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains(message),
        "expected {message}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[path = "recorded_review_cli/agent_result.rs"]
mod agent_result;
#[path = "recorded_review_cli/native_topics.rs"]
mod native_topics;

#[path = "recorded_review_cli/diff_identity.rs"]
mod diff_identity;

#[test]
fn real_command_exports_exact_sources_and_replays_without_remote_confirmation() -> Result<()> {
    let f = Fixture::new()?;
    let original_ci = fs::read(f.root.path().join("ci.bin"))?;
    // A hostile production config is irrelevant: this command reads no config.
    fs::write(
        f.root.path().join(".ub-review.toml"),
        b"models = invalid production config",
    )?;
    let output = f.invoke("export", &[])?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let view: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(view["judgment"]["judgment"]["review_result"], "findings");
    assert_eq!(
        view["public_plan"]["plan"]["items"][0]["disposition"]["kind"],
        "summary"
    );
    assert_eq!(view["delivery_projection"], "not_admitted");
    assert_eq!(view["remote_effects"], "unknown");
    assert!(view["observation_watermark"].is_null());
    let answer: Vec<u8> = serde_json::from_value(view["judgment"]["recorded_bytes"].clone())?;
    assert_eq!(answer, fs::read(f.root.path().join("answer.json"))?);
    let plan: Vec<u8> = serde_json::from_value(view["public_plan_recorded_bytes"].clone())?;
    assert_eq!(plan, fs::read(f.root.path().join("plan.json"))?);
    assert_eq!(hash(&original_ci), view["imported_ci_digest"]);
    assert_eq!(fs::read(f.root.path().join("ci.bin"))?, original_ci);
    let pin = hash(&output.stdout);
    fs::write(f.root.path().join("view.json"), &output.stdout)?;
    let replay = f.invoke("replay", &["--view", "view.json", "--view-sha256", &pin])?;
    assert!(
        replay.status.success(),
        "{}",
        String::from_utf8_lossy(&replay.stderr)
    );
    let receipt: Value = serde_json::from_slice(&replay.stdout)?;
    assert_eq!(receipt["status"], "replayed_recorded_sources");
    assert_eq!(receipt["view_sha256"], pin);
    assert_eq!(receipt["remote_effects"], "unknown");
    Ok(())
}

#[test]
fn intake_and_file_pins_malformed_missing_and_oversize_inputs_fail_closed() -> Result<()> {
    let mut f = Fixture::new()?;
    let success = f.invoke("export", &[])?;
    assert!(success.status.success());
    rejected(
        f.invoke_pin("export", &"0".repeat(64), &[])?,
        "SHA256 mismatch",
    );
    let malformed = b"{ private malformed intake";
    fs::write(f.root.path().join("intake.json"), malformed)?;
    let output = f.invoke_pin("export", &hash(malformed), &[])?;
    assert!(!String::from_utf8_lossy(&output.stderr).contains("private malformed"));
    rejected(output, "invalid intake JSON");
    f.intake["dispatch"] = json!({"argv":["forbidden"]});
    rejected(f.invoke("export", &[])?, "invalid intake JSON");
    f.intake.as_object_mut().map(|m| m.remove("dispatch"));
    fs::write(f.root.path().join("evidence.bin"), b"changed")?;
    rejected(f.invoke("export", &[])?, "SHA256 mismatch");
    fs::remove_file(f.root.path().join("evidence.bin"))?;
    rejected(f.invoke("export", &[])?, "inspect recorded file");
    fs::write(
        f.root.path().join("evidence.bin"),
        vec![b'x'; 1024 * 1024 + 1],
    )?;
    rejected(f.invoke("export", &[])?, "exceeds byte limit");
    let giant = vec![b'x'; 256 * 1024 + 1];
    fs::write(f.root.path().join("intake.json"), &giant)?;
    rejected(
        f.invoke_pin("export", &hash(&giant), &[])?,
        "exceeds byte limit",
    );
    Ok(())
}

#[test]
fn repinned_stale_and_altered_views_sources_and_ci_are_rejected() -> Result<()> {
    let mut f = Fixture::new()?;
    let output = f.invoke("export", &[])?;
    assert!(output.status.success());
    let mut view: Value = serde_json::from_slice(&output.stdout)?;
    view["remote_effects"] = json!("confirmed");
    let changed = serde_json::to_vec(&view)?;
    fs::write(f.root.path().join("view.json"), &changed)?;
    rejected(
        f.invoke(
            "replay",
            &["--view", "view.json", "--view-sha256", &hash(&changed)],
        )?,
        "replay rejected",
    );
    fs::write(f.root.path().join("view.json"), &output.stdout)?;
    rejected(
        f.invoke(
            "replay",
            &["--view", "view.json", "--view-sha256", &"0".repeat(64)],
        )?,
        "SHA256 mismatch",
    );
    f.intake["subject"]["pass_id"] = json!("fresh-pass");
    rejected(f.invoke("export", &[])?, "source validation rejected");
    f.intake["subject"]["pass_id"] = json!("pass-1");
    let mut ci = fs::read(f.root.path().join("ci.bin"))?;
    ci.push(b' ');
    fs::write(f.root.path().join("ci.bin"), &ci)?;
    f.intake["imported_ci"]["sha256"] = json!(hash(&ci));
    rejected(f.invoke("export", &[])?, "source validation rejected");
    Ok(())
}

#[test]
fn independently_repinned_moved_revision_still_rejects_old_graph() -> Result<()> {
    let mut f = Fixture::new()?;
    let path = f.root.path().join("revision.txt");
    let original = fs::read_to_string(&path)?;
    let moved = original.replace(&"a".repeat(40), &"f".repeat(40));
    fs::write(&path, &moved)?;
    f.intake["revision"]["sha256"] = json!(hash(moved.as_bytes()));
    let source = f.source_json.replace(
        &serde_json::to_string(&original)?,
        &serde_json::to_string(&moved)?,
    );
    f.intake["source_manifest_digest"] = json!(domain_hash(
        "ub-review.recorded-judgment-source.v1",
        source.as_bytes()
    ));
    rejected(f.invoke("export", &[])?, "source validation rejected");
    Ok(())
}

#[test]
fn absent_lead_is_distinct_from_declared_missing_or_invalid_lead() -> Result<()> {
    let mut f = Fixture::new()?;
    f.intake["answer"] = Value::Null;
    f.intake["plan"] = Value::Null;
    let output = f.invoke("export", &[])?;
    assert!(output.status.success());
    let view: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(view["judgment"]["availability"], "absent");
    let bytes = b"private malformed lead";
    fs::write(f.root.path().join("answer.json"), bytes)?;
    f.intake["answer"] = json!({"path":"answer.json","sha256":hash(bytes)});
    let output = f.invoke("export", &[])?;
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stdout).contains("private malformed"));
    let view: Value = serde_json::from_slice(&output.stdout)?;
    assert_eq!(view["judgment"]["availability"], "invalid");
    fs::remove_file(f.root.path().join("answer.json"))?;
    rejected(f.invoke("export", &[])?, "inspect recorded file");
    Ok(())
}

#[test]
fn unsafe_paths_directories_parent_files_and_unsupported_graphs_are_rejected() -> Result<()> {
    let mut f = Fixture::new()?;
    for path in [
        "../evidence.bin",
        "/tmp/evidence.bin",
        "dir/../evidence.bin",
        "./evidence.bin",
        "dir\\evidence.bin",
    ] {
        f.intake["evidence"][0]["file"]["path"] = json!(path);
        rejected(f.invoke("export", &[])?, "unsafe recorded file path");
    }
    f.intake["evidence"][0]["file"]["path"] = json!("evidence.bin/child");
    rejected(f.invoke("export", &[])?, "parent is not a directory");
    f.intake["evidence"][0]["file"]["path"] = json!("directory");
    fs::create_dir(f.root.path().join("directory"))?;
    rejected(f.invoke("export", &[])?, "not a regular file");
    f.intake["evidence"][0]["file"]["path"] = json!("evidence.bin");
    let graph_path = f.root.path().join("graph.json");
    let mut graph: Value = serde_json::from_slice(&fs::read(&graph_path)?)?;
    graph["topics"] = json!([{"claim_id":"A"}]);
    let bytes = serde_json::to_vec(&graph)?;
    fs::write(&graph_path, &bytes)?;
    f.intake["graph"]["sha256"] = json!(hash(&bytes));
    rejected(f.invoke("export", &[])?, "invalid claim graph JSON");
    graph["topics"] = json!([]);
    graph["revision"]["dispatch"] = json!("forged");
    let bytes = serde_json::to_vec(&graph)?;
    fs::write(&graph_path, &bytes)?;
    f.intake["graph"]["sha256"] = json!(hash(&bytes));
    rejected(f.invoke("export", &[])?, "unsupported or discarded fields");
    Ok(())
}

#[cfg(unix)]
#[test]
fn unix_symlinks_and_socket_inputs_fail_before_opening() -> Result<()> {
    use std::os::unix::fs::symlink;
    use std::os::unix::net::UnixListener;
    let mut f = Fixture::new()?;
    symlink(
        f.root.path().join("evidence.bin"),
        f.root.path().join("link"),
    )?;
    f.intake["evidence"][0]["file"]["path"] = json!("link");
    rejected(f.invoke("export", &[])?, "symlink rejected");
    let socket_path = f.root.path().join("socket");
    let _socket = UnixListener::bind(&socket_path)?;
    f.intake["evidence"][0]["file"]["path"] = json!("socket");
    rejected(f.invoke("export", &[])?, "not a regular file");
    symlink(f.root.path(), f.root.path().join("parent-link"))?;
    f.intake["evidence"][0]["file"]["path"] = json!("parent-link/evidence.bin");
    rejected(f.invoke("export", &[])?, "symlink rejected");
    Ok(())
}
