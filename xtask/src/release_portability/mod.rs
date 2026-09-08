//! Immutable v0.1.0 portability proof; never publishes or builds the product.

mod archive;
mod metadata;
mod packet;
mod process;
mod receipts;
mod resolver;
mod runtime;

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use serde::de::DeserializeOwned;

use metadata::{AnnotatedTag, ObjectKind, Release, TagRef};
use process::{Runner, strings};
use receipts::{Authorization, write_json};

struct Options {
    mode: String,
    out: PathBuf,
    local: bool,
    step_outcome: Option<String>,
}

impl Options {
    fn parse(mut args: impl Iterator<Item = String>) -> Result<Self> {
        let mode = args
            .next()
            .context("release-portability requires run, resolver-before, or resolver-after")?;
        ensure!(
            matches!(mode.as_str(), "run" | "resolver-before" | "resolver-after"),
            "unknown release-portability mode"
        );
        let mut out = PathBuf::from("target/release-portability");
        let mut local = false;
        let mut step_outcome = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--out" => out = args.next().context("--out requires a path")?.into(),
                "--local-explicit" => local = true,
                "--step-outcome" => {
                    step_outcome = Some(
                        args.next()
                            .context("--step-outcome requires the Action outcome")?,
                    )
                }
                other => bail!("unexpected portability option {other}"),
            }
        }
        ensure!(!out.as_os_str().is_empty(), "output path is empty");
        ensure!(
            mode == "run" || !local,
            "resolver control requires a workflow dispatch"
        );
        ensure!(
            (mode == "resolver-after") == step_outcome.is_some(),
            "only resolver-after requires --step-outcome"
        );
        Ok(Self {
            mode,
            out,
            local,
            step_outcome,
        })
    }
}

fn git_text(root: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(root)
        .output()
        .context("inspect proof source")?;
    ensure!(output.status.success(), "source Git inspection failed");
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

fn execution(root: &Path, local: bool) -> Result<Authorization> {
    let source = git_text(root, &["rev-parse", "HEAD"])?;
    ensure!(
        git_text(root, &["status", "--porcelain"])?.is_empty(),
        "portability proof requires a clean exact source checkout"
    );
    let execution = if local {
        ensure!(
            env::var("GITHUB_ACTIONS").ok().as_deref() != Some("true"),
            "workflow proof cannot substitute local execution"
        );
        Authorization {
            event: "local_explicit".to_owned(),
            actor: env::var("USER")
                .or_else(|_| env::var("USERNAME"))
                .context("local proof actor")?,
            run_id: format!("local-{}", std::process::id()),
            workflow_sha: source.clone(),
            source_sha: source,
        }
    } else {
        ensure!(
            env::var("GITHUB_EVENT_NAME")?.as_str() == "workflow_dispatch",
            "expensive portability proof requires workflow_dispatch"
        );
        ensure!(
            env::var("GITHUB_REPOSITORY")? == metadata::REPOSITORY,
            "wrong portability workflow repository"
        );
        let declared_source = env::var("GITHUB_SHA")?;
        ensure!(
            source == declared_source,
            "checked-out source differs from dispatched commit"
        );
        Authorization {
            event: "workflow_dispatch".to_owned(),
            actor: env::var("GITHUB_ACTOR")?,
            run_id: env::var("GITHUB_RUN_ID")?,
            workflow_sha: env::var("GITHUB_WORKFLOW_SHA")
                .context("dispatched workflow source identity")?,
            source_sha: source,
        }
    };
    execution.validate()?;
    Ok(execution)
}

fn api<T: DeserializeOwned>(runner: &mut Runner, endpoint: &str) -> Result<T> {
    let output = runner.checked("gh", &strings(&["api", "--method", "GET", endpoint]))?;
    serde_json::from_slice(&output.stdout)
        .with_context(|| format!("parse public GitHub metadata for {endpoint}"))
}

fn resolve_release(runner: &mut Runner, out: &Path) -> Result<metadata::ReleaseIdentity> {
    let repository = metadata::REPOSITORY;
    let release: Release = api(
        runner,
        &format!("repos/{repository}/releases/tags/{}", metadata::TAG),
    )?;
    let reference: TagRef = api(
        runner,
        &format!("repos/{repository}/git/ref/tags/{}", metadata::TAG),
    )?;
    write_json(&out.join("github-release.json"), &release)?;
    write_json(&out.join("github-tag-reference.json"), &reference)?;
    let mut annotations = BTreeMap::<String, AnnotatedTag>::new();
    let mut object = reference.object.clone();
    while object.kind == ObjectKind::Tag {
        metadata::require_oid(&object.sha)?;
        ensure!(
            annotations.len() < 8 && !annotations.contains_key(&object.sha),
            "tag metadata chain is cyclic or excessive"
        );
        let annotation: AnnotatedTag = api(
            runner,
            &format!("repos/{repository}/git/tags/{}", object.sha),
        )?;
        object = annotation.object.clone();
        let prior = annotations.insert(annotation.sha.clone(), annotation);
        ensure!(prior.is_none(), "duplicate observed tag object");
    }
    write_json(&out.join("github-tag-annotations.json"), &annotations)?;
    let identity = metadata::validate_release(&release, &reference, &annotations)?;
    write_json(&out.join("resolved-release.json"), &identity)?;
    Ok(identity)
}

fn download(runner: &mut Runner, out: &Path, asset: &metadata::Asset) -> Result<Vec<u8>> {
    let path = out.join(&asset.name);
    runner.checked(
        "curl",
        &strings(&[
            "--fail",
            "--location",
            "--silent",
            "--show-error",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--max-time",
            "120",
            "--max-filesize",
            &asset.size.to_string(),
            "--output",
            path.to_str().context("download output path")?,
            &asset.browser_download_url,
        ]),
    )?;
    let metadata = fs::metadata(&path)?;
    ensure!(
        metadata.is_file() && metadata.len() == asset.size,
        "downloaded asset size mismatch"
    );
    let bytes = fs::read(path)?;
    archive::verify_download(asset, &bytes)?;
    Ok(bytes)
}

fn prove(out: &Path, execution: &Authorization) -> Result<()> {
    let mut runner = Runner::new(out)?;
    write_json(&out.join("authorization.json"), execution)?;
    let release = resolve_release(&mut runner, out)?;
    let archive = download(&mut runner, out, &release.archive)?;
    let checksum = download(&mut runner, out, &release.checksum)?;
    archive::verify_checksum(&checksum, &release.archive)?;
    let (layout, executable) = archive::inspect_archive(&archive)?;
    write_json(&out.join("archive-layout.json"), &layout)?;
    let executable_path = out.join("ub-review");
    fs::write(&executable_path, executable)?;
    let mut rows = Vec::new();
    for (name, image) in [
        ("ubuntu-24.04", "ubuntu:24.04"),
        ("ubuntu-22.04", "ubuntu:22.04"),
    ] {
        rows.push(runtime::run_row(
            &mut runner,
            &out.join(name),
            image,
            runtime::VerifiedRelease {
                release: &release,
                layout: &layout,
                archive_bytes: &archive,
                executable: &executable_path,
            },
            execution,
        )?);
    }
    let matrix = receipts::reconcile(rows, &release, execution)?;
    write_json(&out.join("matrix.json"), &matrix)?;
    fs::write(
        out.join("decision.md"),
        "# Immutable v0.1.0 portability execution\n\nThe exact asset in resolved-release.json ran on Ubuntu 24.04 x86_64/glibc 2.39. Ubuntu 22.04 x86_64/glibc 2.35 rejected it at the GLIBC_2.39 loader boundary. Other distributions and architectures remain unproven. The historical initializer policy defect remains separate from the explicit-config passing model-off packet. See matrix.json and command logs for this run's source and execution.\n",
    )?;
    Ok(())
}

pub(crate) fn run(root: &Path, args: impl Iterator<Item = String>) -> Result<()> {
    let options = Options::parse(args)?;
    let execution = execution(root, options.local)?;
    let out = if options.out.is_absolute() {
        options.out
    } else {
        root.join(options.out)
    };
    if options.mode == "resolver-after" {
        ensure!(
            out.is_dir(),
            "resolver baseline output directory is missing"
        );
    } else {
        ensure!(
            !out.exists(),
            "proof output must be new; preserve or remove the prior lane output explicitly"
        );
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::create_dir(&out)?;
    }
    match options.mode.as_str() {
        "run" => prove(&out, &execution),
        "resolver-before" => resolver::before(&out, execution),
        "resolver-after" => resolver::after(
            &out,
            execution,
            options
                .step_outcome
                .as_deref()
                .context("resolver outcome")?,
        ),
        _ => bail!("unsupported portability mode"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn portability_options_require_explicit_resolver_outcome() -> Result<()> {
        ensure!(Options::parse(strings(&["resolver-after"]).into_iter()).is_err());
        ensure!(
            Options::parse(strings(&["resolver-before", "--local-explicit"]).into_iter()).is_err()
        );
        ensure!(
            Options::parse(strings(&["run", "--step-outcome", "failure"]).into_iter()).is_err()
        );
        let parsed = Options::parse(
            strings(&["run", "--out", "target/new-proof", "--local-explicit"]).into_iter(),
        )?;
        ensure!(parsed.local && parsed.out == Path::new("target/new-proof"));
        Ok(())
    }
}
