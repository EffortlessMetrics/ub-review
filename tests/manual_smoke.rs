//! Real workflow-script and CLI admission controls for manual smoke (#1603).
#![cfg(unix)]

use std::fs;

use anyhow::{Context, Result};

#[path = "common/cli_run.rs"]
mod common;
use common::*;

#[test]
fn manual_smoke_revision_selection_reaches_real_cli() -> Result<()> {
    let _guard = cli_subprocess_test_lock()?;
    let workflow = include_str!("../.github/workflows/action-smoke.yml");
    let scripts = workflow
        .split("          # smoke-revision-begin\n")
        .skip(1)
        .map(|part| -> Result<String> {
            let (script, _) = part
                .split_once("          # smoke-revision-end")
                .context("missing smoke revision end marker")?;
            script
                .lines()
                .map(|line| {
                    line.strip_prefix("          ")
                        .map(|line| format!("{line}\n"))
                        .context("unexpected workflow script indentation")
                })
                .collect()
        })
        .collect::<Result<Vec<_>>>()?;
    assert_eq!(scripts.len(), 2, "both manual jobs need revision selection");
    assert_eq!(
        scripts[0], scripts[1],
        "manual jobs must select identically"
    );
    let script = format!("export GITHUB_OUTPUT=\"$1\"\n{}", scripts[0]);
    let (_, jobs) = workflow.split_once("\njobs:\n").context("missing jobs")?;
    let (off, on) = jobs
        .split_once("\n  model-smoke:\n")
        .context("missing model smoke job")?;
    for job in [off, on] {
        for input in [
            "install-mode: source",
            "base: ${{ steps.revision.outputs.base }}",
            "head: ${{ steps.revision.outputs.head }}",
            "pr-head-sha: ${{ steps.revision.outputs.pr-head-sha }}",
        ] {
            assert_eq!(job.matches(input).count(), 1, "missing input: {input}");
        }
        let selection = job.find("id: revision").context("missing selection")?;
        let action = job.find("uses: ./").context("missing local Action")?;
        assert!(selection < action, "selection must precede the Action");
    }
    assert!(on.contains("if: inputs.run_model_smoke == true"));
    assert!(off.contains("model-mode: off"));

    for history in ["linear", "merge", "root", "octopus"] {
        let temp = tempfile::tempdir()?;
        let repo = temp.path().join("repo");
        init_minimal_repo(&repo)?;
        if matches!(history, "merge" | "octopus") {
            run(
                &repo,
                "bash",
                &[
                    "-euc",
                    "feature=$(git rev-parse HEAD)\n\
                     git checkout -q -b smoke-base HEAD^\n\
                     printf 'base advance\\n' > main.txt\n\
                     git add main.txt\n\
                     git commit -qm 'base advance'\n\
                     git merge -q --no-ff -m 'smoke merge' \"$feature\"",
                ],
            )?;
        }
        if history == "root" {
            run(&repo, "git", &["checkout", "-q", "--detach", "HEAD^"])?;
        } else if history == "octopus" {
            run(
                &repo,
                "bash",
                &[
                    "-euc",
                    "tip=$(git commit-tree 'HEAD^{tree}' -p HEAD -p HEAD^1 -p HEAD^2 -m octopus)\n\
                     git checkout -q --detach \"$tip\"",
                ],
            )?;
        }
        let output = temp.path().join("github-output");
        let resolver_args = [
            "--noprofile",
            "--norc",
            "-c",
            script.as_str(),
            "smoke-revision",
            path_str(&output)?,
        ];
        if matches!(history, "root" | "octopus") {
            let diagnostic = run_expect_failure(&repo, "bash", &resolver_args)?;
            assert!(diagnostic.contains("requires a commit with one or two parents"));
            assert!(!output.exists(), "unsupported history emitted inputs");
            continue;
        }
        run(&repo, "bash", &resolver_args)?;

        // Read objects independently of the workflow's output construction.
        let objects = temp.path().join("objects");
        let head_side = if history == "merge" { "HEAD^2" } else { "HEAD" };
        run(
            &repo,
            "bash",
            &[
                "-euc",
                "git rev-parse HEAD^1 HEAD 'HEAD^{tree}' 'HEAD^1^{tree}' \"$1\" \"$1^{tree}\" > \"$2\"",
                "smoke-objects",
                head_side,
                path_str(&objects)?,
            ],
        )?;
        let object_text = fs::read_to_string(&objects)?;
        let ids = object_text.lines().collect::<Vec<_>>();
        assert_eq!(ids.len(), 6);
        assert_eq!(
            fs::read_to_string(&output)?,
            format!("base={}\nhead={}\npr-head-sha={}\n", ids[0], ids[1], ids[4])
        );
        let config = temp.path().join("review.toml");
        write_file(
            &config,
            "profile = \"gh-runner\"\n[review]\nenable_default_lanes = false\n",
        )?;

        // The missing-metadata merge case reproduces the old workflow refusal.
        // Correct metadata admits the same objects; forged metadata still fails.
        for (case, pr_head) in [
            ("correct", Some(ids[4])),
            ("missing", None),
            ("wrong", Some(ids[0])),
        ] {
            let out = temp.path().join(format!("packet-{case}"));
            let mut args = vec!["-i".to_owned(), format!("PATH={}", std::env::var("PATH")?)];
            if let Ok(value) = std::env::var("LLVM_PROFILE_FILE") {
                args.push(format!("LLVM_PROFILE_FILE={value}"));
            }
            args.extend(
                [
                    env!("CARGO_BIN_EXE_ub-review"),
                    "run",
                    "--root",
                    path_str(&repo)?,
                    "--config",
                    path_str(&config)?,
                    "--base",
                    ids[0],
                    "--head",
                    ids[1],
                    "--out",
                    path_str(&out)?,
                    "--dry-run",
                    "--model-mode",
                    "off",
                    "--posting",
                    "artifact-only",
                    "--no-github-summary",
                    "--fail-on-gate",
                    "false",
                ]
                .into_iter()
                .map(str::to_owned),
            );
            if let Some(pr_head) = pr_head {
                args.extend(["--pr-head-sha".to_owned(), pr_head.to_owned()]);
            }
            let args = args.iter().map(String::as_str).collect::<Vec<_>>();
            let accepted = case == "correct" || (history == "linear" && case == "missing");
            if !accepted {
                let diagnostic = run_expect_failure(&repo, "env", &args)?;
                assert!(diagnostic.contains("pull-request head"), "{diagnostic}");
                assert!(!out.exists(), "rejected metadata created a packet");
                continue;
            }
            run(&repo, "env", &args)?;
            let admission: serde_json::Value =
                serde_json::from_slice(&fs::read(out.join("input/revision-admission.json"))?)?;
            let semantics = if history == "merge" {
                "merge_result"
            } else {
                "candidate_head"
            };
            assert_eq!(admission["schema"], "ub-review.revision_admission.v1");
            assert_eq!(admission["semantics"], semantics);
            assert_eq!(admission["reviewed_commit_oid"], ids[1]);
            assert_eq!(admission["pr_head_commit"], ids[4]);
            let canonical = admission["identity_canonical"]
                .as_str()
                .context("missing canonical identity")?;
            for pair in [
                format!("base={} {}", ids[0], ids[3]),
                format!("head={} {}", ids[4], ids[5]),
                format!("reviewed={} {}", ids[1], ids[2]),
            ] {
                assert!(canonical.lines().any(|line| line == pair), "{pair}");
            }
            let expected_merge = if history == "merge" {
                format!("merge={} {}", ids[1], ids[2])
            } else {
                "merge=-".to_owned()
            };
            assert!(canonical.lines().any(|line| line == expected_merge));
        }
    }
    Ok(())
}
