//! Release workflow publication boundary contract (#1344).
//!
//! Mechanical guards over `.github/workflows/release-binary.yml`: candidate
//! preparation only uploads frozen artifacts, and the production publish path
//! never rebuilds the archive, never overwrites or uploads onto existing
//! releases, and never executes candidate code with publishing credentials.
//! The publish decision itself lives in `xtask::release_publish` and is
//! proven by its recording-publisher tests; this file pins the workflow side
//! of the same boundary. Syntax is enforced separately by actionlint.

use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

fn workflow_text() -> Result<String> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join(".github")
        .join("workflows")
        .join("release-binary.yml");
    fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))
}

/// Source of one top-level job: from its 2-space-indented key to the next
/// 2-space-indented sibling key (job bodies are at least four spaces deep).
fn job_section<'a>(text: &'a str, job: &str) -> Result<&'a str> {
    let marker = format!("\n  {job}:\n");
    let start = text
        .find(&marker)
        .with_context(|| format!("workflow is missing job `{job}`"))?;
    let rest = &text[start + marker.len()..];
    let end = rest
        .match_indices('\n')
        .find_map(|(idx, _)| {
            let line = &rest[idx + 1..];
            let top_level = line.starts_with("  ") && !line.starts_with("   ");
            top_level.then_some(idx)
        })
        .unwrap_or(rest.len());
    Ok(&rest[..end])
}

fn assert_absent(haystack: &str, needles: &[&str], scope: &str) -> Result<()> {
    for needle in needles {
        if haystack.contains(needle) {
            bail!("{scope} must not contain `{needle}`");
        }
    }
    Ok(())
}

#[test]
fn candidate_preparation_only_uploads_frozen_artifacts() -> Result<()> {
    // Acceptance #1344 item 1: ordinary candidate preparation creates only
    // workflow artifacts, not tags or releases.
    let text = workflow_text()?;
    let package = job_section(&text, "package")?;
    assert_absent(
        package,
        &["contents: write", "gh release", "create-release"],
        "package job",
    )?;
    if !package.contains("actions/upload-artifact") {
        bail!("package job must upload workflow artifacts");
    }
    if package.matches("retention-days: 14").count() != 2 {
        bail!(
            "package job must retain both frozen artifacts for the bounded 14-day window, found {}",
            package.matches("retention-days: 14").count()
        );
    }
    if !package.contains("ub-review.authorized_release.v1") {
        bail!("package job must emit the #1343 authorized-candidate packet schema");
    }
    if !package.contains("artifact-id") {
        bail!("package job must pin the immutable uploaded artifact id into the packet");
    }
    // A dispatch that carries authorization inputs must not re-package.
    if !package.contains("inputs['candidate-run-id'] == ''") {
        bail!("package job must skip publish dispatches that carry authorization inputs");
    }
    Ok(())
}

#[test]
fn validity_window_is_stamped_before_the_bytes_upload() -> Result<()> {
    // Review finding F3: the packet's retained_until claim must be stamped
    // strictly BEFORE the bytes artifact upload. The publish job derives the
    // admission horizon from that artifact's GitHub-side expires_at (upload
    // time + retention); stamping after the upload makes claim > horizon by
    // construction, so admission would reject every legitimate packet.
    let text = workflow_text()?;
    let package = job_section(&text, "package")?;
    let stamp = package.find("id: window").with_context(
        || "package job must stamp the validity window in a dedicated `window` step",
    )?;
    let upload = package
        .find("actions/upload-artifact")
        .with_context(|| "package job must upload artifacts")?;
    if stamp > upload {
        bail!(
            "validity-window stamp (offset {stamp}) must precede the first artifact upload (offset {upload})"
        );
    }
    if !package.contains("steps.window.outputs.retained_until") {
        bail!("packet step must consume the pre-upload window stamp, not a freshly computed time");
    }
    if package.matches("date -u -d '+14 days'").count() != 1 {
        bail!(
            "the claim must be stamped exactly once (found {} stamps), before the upload",
            package.matches("date -u -d '+14 days'").count()
        );
    }
    Ok(())
}

#[test]
fn exact_length_hex_checks_use_regex_quantifiers() -> Result<()> {
    // Review findings F1/F2: bash case globs of N char-classes match exactly
    // N characters, so they rejected every legitimate 64-hex packet digest
    // and 40-hex head sha. The workflow must validate these with quantified
    // regexes instead.
    let text = workflow_text()?;
    for pattern in [
        "grep -qE \"^[0-9a-f]{$2}$\"",
        "'^[0-9a-f]{40}$'",
        "is_hex \"$PACKET_SHA256\" 64",
    ] {
        if !text.contains(pattern) {
            bail!("workflow must validate hex identity with the quantified regex {pattern}");
        }
    }
    assert_absent(
        &text,
        &["[0-9a-f][0-9a-f][0-9a-f]"],
        "release workflow (case-glob hex classes match only their own length)",
    )?;
    Ok(())
}

#[test]
fn publish_path_never_rebuilds_or_overwrites() -> Result<()> {
    // Acceptance #1344 items 6 and 7: the publish path never rebuilds a fresh
    // archive, never substitutes a newest artifact, and never clobbers.
    let text = workflow_text()?;
    assert_absent(
        &text,
        &[
            "--clobber",
            "gh release upload",
            "gh release delete",
            "gh release edit",
            "gh release",
        ],
        "release workflow",
    )?;
    let publish = job_section(&text, "publish")?;
    assert_absent(
        publish,
        &[
            "cargo build",
            "tar -czf",
            "sha256sum",
            "needs:",
            "--bin ub-review",
        ],
        "publish job",
    )?;
    if !publish.contains("github.event_name == 'workflow_dispatch'") {
        bail!("publish job must run only on the explicit authorization dispatch");
    }
    if !publish.contains("persist-credentials: false") {
        bail!(
            "publish job must not persist git credentials into the candidate-adjacent environment"
        );
    }
    Ok(())
}

#[test]
fn publish_decision_is_the_rust_validator_with_trusted_facts() -> Result<()> {
    // Acceptance #1344 item 8: candidate-controlled code/input cannot supply
    // publishing authority. The only decision point is the reviewed dispatch-
    // ref xtask command, which receives every trusted fact explicitly.
    let text = workflow_text()?;
    let publish = job_section(&text, "publish")?;
    if !publish.contains("validate-release") || !publish.contains("--publish") {
        bail!("publish job must run the `validate-release` decision command with --publish");
    }
    for flag in [
        "--packet-sha256",
        "--authorization-record",
        "--authorization-verifier",
        "--authorization-expires-at",
        "--producer-ref",
        "--tree-sha",
        "--head-sha",
        "--retained-until",
        "--package-version",
        "--now",
    ] {
        if !publish.contains(flag) {
            bail!(
                "publish job must resolve the trusted fact {flag} independently of the candidate"
            );
        }
    }
    // The frozen candidate is addressed by immutable ids inside the exact
    // authorized run, never by a mutable name or the newest artifact.
    for required in [
        "inputs.candidate-run-id",
        "inputs.candidate-run-attempt",
        "inputs.candidate-artifact-id",
        "inputs.packet-artifact-id",
    ] {
        if !publish.contains(required) {
            bail!("publish job must bind the frozen candidate through {required}");
        }
    }
    Ok(())
}
