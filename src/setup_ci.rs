//! CI setup wizard: generates the migration PR with .ub-review.toml,
//! the ub-review/gate workflow edits, the required proof floor, the
//! adaptive proof policy, and branch-protection instructions (cleanup
//! train step 16, pure code motion). Requires --accept `<job>`=`<command>`
//! for required proof materialization and never mutates branch protection
//! silently.

use crate::*;

/// One `--accept <job>=<command>` pair. The audit receipts record triggers,
/// timings, and correlation - never the runnable command - so the
/// maintainer supplies it and the generator never invents one.
#[derive(Clone, Debug)]
pub(crate) struct SetupCiAccept {
    pub(crate) job: String,
    pub(crate) command: String,
}

pub(crate) fn parse_setup_ci_accepts(raw: &[String]) -> Result<Vec<SetupCiAccept>> {
    let mut accepts = Vec::new();
    for entry in raw {
        let Some((job, command)) = entry.split_once('=') else {
            bail!(
                "--accept needs `<job>=<command>` (the audit receipts do not record the \
                 runnable command; supply it explicitly): got `{entry}`"
            );
        };
        let job = job.trim();
        let command = command.trim();
        if job.is_empty() || command.is_empty() {
            bail!("--accept `<job>=<command>` needs both halves non-empty: got `{entry}`");
        }
        accepts.push(SetupCiAccept {
            job: job.to_owned(),
            command: command.to_owned(),
        });
    }
    Ok(accepts)
}

pub(crate) fn setup_ci_required_flag_for_tier(tier: &str) -> Option<bool> {
    match tier {
        "move-to-ub-review-required" => Some(true),
        "adaptive" => Some(false),
        _ => None,
    }
}

pub(crate) fn load_ci_audit_receipt<T: serde::de::DeserializeOwned>(
    dir: &Path,
    name: &str,
    expected_schema: &str,
) -> Result<T> {
    let path = dir.join(name);
    let bytes = fs::read(&path).with_context(|| {
        format!(
            "missing audit receipt {}; run `ub-review audit-ci` first",
            path.display()
        )
    })?;
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
    let schema = value.get("schema").and_then(serde_json::Value::as_str);
    if schema != Some(expected_schema) {
        bail!(
            "{} has schema {:?}; expected {expected_schema}",
            path.display(),
            schema
        );
    }
    serde_json::from_value(value).with_context(|| format!("decode {}", path.display()))
}

/// Sanitize an audited job id into a `[[proof.required]]` id: lowercase
/// alphanumerics and dashes, collapsing every other byte to a dash.
pub(crate) fn setup_ci_proof_id(job: &str) -> String {
    let mut id = String::with_capacity(job.len());
    for ch in job.chars() {
        if ch.is_ascii_alphanumeric() {
            id.push(ch.to_ascii_lowercase());
        } else if !id.ends_with('-') {
            id.push('-');
        }
    }
    let id = id.trim_matches('-').to_owned();
    if id.is_empty() { "job".to_owned() } else { id }
}

pub(crate) fn toml_basic_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 2);
    escaped.push('"');
    for ch in value.chars() {
        match ch {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\t' => escaped.push_str("\\t"),
            '\r' => escaped.push_str("\\r"),
            _ => escaped.push(ch),
        }
    }
    escaped.push('"');
    escaped
}

/// Render the generated `.ub-review.toml` additions for the accepted jobs.
/// Emits nothing but `[gate].required_check` and one `[[proof.required]]`
/// entry per accepted generated-proof job - no `[providers]`, no
/// `synchronize_mode`, no `[tools.*.gate]` thresholds (spec 0008: never
/// ship decorative policy into a consumer repo).
pub(crate) fn render_setup_ci_gate_config(
    accepts: &[SetupCiAccept],
    recommendations: &[CiRecommendation],
    inventory: &CiInventoryArtifact,
    required_check: &str,
) -> String {
    let mut text = String::from("[gate]\n");
    text.push_str(&format!(
        "required_check = {}\n",
        toml_basic_string(required_check)
    ));
    for accept in accepts {
        let recommendation = recommendations.iter().find(|entry| entry.job == accept.job);
        let required = recommendation
            .and_then(|entry| setup_ci_required_flag_for_tier(&entry.tier))
            .unwrap_or(false);
        let receipt = recommendation
            .and_then(|entry| entry.receipts.first().cloned())
            .unwrap_or_else(|| format!("ci-audit/recommendations.json#{}", accept.job));
        let timeout_sec = inventory
            .jobs
            .iter()
            .find(|job| job.job == accept.job)
            .and_then(|job| job.timeout_minutes)
            .map(|minutes| minutes.saturating_mul(60))
            .filter(|seconds| *seconds > 0)
            .unwrap_or(600);
        let reason = if required {
            format!(
                "moved to required proof from audited job `{}`; receipt {receipt}",
                accept.job
            )
        } else {
            format!(
                "right-sized to adaptive proof from audited job `{}`; receipt {receipt}",
                accept.job
            )
        };
        text.push_str(&format!(
            "\n[[proof.required]]\nid = {id}\nlanguages = [\"all\"]\ndiff_classes = [\"all\"]\ncommand = {command}\nreason = {reason}\ntimeout_sec = {timeout_sec}\nrequired = {required}\nenabled = true\n",
            id = toml_basic_string(&setup_ci_proof_id(&accept.job)),
            command = toml_basic_string(&accept.command),
            reason = toml_basic_string(&reason),
        ));
    }
    text
}

pub(crate) fn setup_ci_section_bullets(recommendations: &[CiRecommendation], tier: &str) -> String {
    let entries: Vec<&CiRecommendation> = recommendations
        .iter()
        .filter(|entry| entry.tier == tier)
        .collect();
    if entries.is_empty() {
        return "- none recommended by this audit\n".to_owned();
    }
    let mut text = String::new();
    for entry in entries {
        text.push_str(&format!(
            "- `{}` ({}) - {}. judgment: {}; receipts: {}\n",
            entry.job,
            entry.workflow,
            entry.reason,
            entry.judgment,
            entry.receipts.join(", ")
        ));
    }
    text
}

pub(crate) fn setup_ci_move_required_bullets(
    recommendations: &[CiRecommendation],
    accepts: &[SetupCiAccept],
) -> String {
    let entries: Vec<&CiRecommendation> = recommendations
        .iter()
        .filter(|entry| entry.tier == "move-to-ub-review-required")
        .collect();
    if entries.is_empty() {
        return "- none recommended by this audit\n".to_owned();
    }
    let mut text = String::new();
    for entry in entries {
        let status = match accepts.iter().find(|accept| accept.job == entry.job) {
            Some(accept) => format!("accepted; command `{}`", accept.command),
            None => "not accepted; no policy generated".to_owned(),
        };
        text.push_str(&format!(
            "- `{}` ({}) - {}. {status}. judgment: {}; receipts: {}\n",
            entry.job,
            entry.workflow,
            entry.reason,
            entry.judgment,
            entry.receipts.join(", ")
        ));
    }
    text
}

pub(crate) fn setup_ci_branch_protection_change(
    inventory: &CiInventoryArtifact,
    required_check: &str,
) -> String {
    let status_unknown = inventory.jobs.iter().any(|job| {
        job.required_check.is_none()
            || job.required_check_source == "unknown"
            || (job.required_check == Some(true) && job.required_check_context.is_none())
    });
    let required_gap = inventory.evidence_gaps.iter().any(|gap| {
        gap.contains("required-check status unknown")
            || gap.contains("required-check context not matched")
            || gap.contains("required checks unreadable")
    });
    if status_unknown || required_gap {
        let source = inventory
            .jobs
            .first()
            .map(|job| job.required_check_source.as_str())
            .unwrap_or("unknown");
        return format!(
            "- add required check: `{required_check}`\n- old required checks unknown: \
             audit-ci did not prove the full branch-protection/ruleset remove list \
             (inventory records `required_check_source: \"{source}\"`), so this plan refuses \
             to invent it. Review the repository's required checks by hand before removing \
             anything.\n"
        );
    }

    let required_jobs: Vec<&CiInventoryJob> = inventory
        .jobs
        .iter()
        .filter(|job| job.required_check == Some(true))
        .collect();
    let mut text = format!("- add required check: `{required_check}`\n");
    if required_jobs.is_empty() {
        text.push_str("- remove required checks: none reported by audit-ci receipts\n");
    } else {
        text.push_str("- remove required checks reported by audit-ci receipts:\n");
        for job in required_jobs {
            let context = job
                .required_check_context
                .as_deref()
                .unwrap_or(job.job.as_str());
            text.push_str(&format!(
                "  - `{}` from `{}` (source: {}; receipt: ci-audit/inventory.json#{})\n",
                context, job.workflow, job.required_check_source, job.job
            ));
        }
    }
    text.push_str(
        "- apply this manually after the migration PR has passed the repo's existing CI; \
         setup-ci did not mutate branch protection.\n",
    );
    text
}

pub(crate) fn render_setup_ci_branch_protection_doc(
    inventory: &CiInventoryArtifact,
    recommendations: &CiRecommendationsArtifact,
    required_check: &str,
) -> String {
    let mut doc = format!(
        "# Branch protection change\n\nRepo: {} (window: {} days). Generated by `ub-review setup-ci` from `ci-audit/inventory.json` required-check receipts.\n\n",
        recommendations.repo, recommendations.window_days
    );
    doc.push_str("## Decision\n\n");
    doc.push_str(
        "Branch protection remains manual. `setup-ci` opened a migration PR only; it did not mutate repository protection rules.\n\n",
    );
    doc.push_str("## Change\n\n");
    let change = setup_ci_branch_protection_change(inventory, required_check);
    let exact_remove_list = !change.contains("old required checks unknown");
    doc.push_str(&change);
    doc.push_str("\n## Apply after\n\n");
    doc.push_str("- the migration PR passes the repository's existing required checks;\n");
    doc.push_str(
        "- the new `ub-review/gate` check has one observed red proof and one quiet-green proof;\n",
    );
    doc.push_str("- a maintainer has reviewed the required-check remove list against the repository settings UI.\n");
    doc.push_str("\n## Rollback\n\n");
    doc.push_str(
        "- Revert the migration PR. If `ub-review/gate` was added manually, remove that required check by hand.\n",
    );
    if exact_remove_list {
        doc.push_str(
            "- If old required checks were removed manually, restore the exact checks listed above.\n",
        );
    } else {
        doc.push_str(
            "- This file does not prove an old-check remove list; review repository settings by hand before removing or restoring old required checks.\n",
        );
    }
    doc
}

pub(crate) fn render_setup_ci_migration_plan(
    inventory: &CiInventoryArtifact,
    recommendations: &CiRecommendationsArtifact,
    accepts: &[SetupCiAccept],
    required_check: &str,
) -> String {
    let jobs = &recommendations.jobs;
    let mut plan = format!(
        "# CI migration plan\n\nRepo: {} (window: {} days). Rendered by `ub-review setup-ci \
         --print-pr` from the ci-audit receipts; nothing below was applied.\n\n",
        recommendations.repo, recommendations.window_days
    );
    plan.push_str("## Decision\n\n");
    if accepts.is_empty() {
        plan.push_str(&format!(
            "No jobs accepted into the generated gate policy, so there is no migration PR \
             to open. The audit covered {} job(s); pass `--accept <job>=<command>` for each \
             adaptive or move-to-ub-review-required job to fold into `{required_check}`.\n\n",
            jobs.len()
        ));
    } else {
        plan.push_str(&format!(
            "Fold {} accepted job(s) into one required check `{required_check}` as adaptive \
             or required proof; every other job keeps its current posture per the tiers below.\n\n",
            accepts.len()
        ));
    }
    plan.push_str("## Keep required\n\n");
    plan.push_str(&setup_ci_section_bullets(jobs, "keep-required"));
    plan.push_str("\n## Move into ub-review/gate\n\n");
    plan.push_str(&setup_ci_move_required_bullets(jobs, accepts));
    plan.push_str("\n## Right-size to adaptive\n\n");
    let adaptive: Vec<&CiRecommendation> = jobs
        .iter()
        .filter(|entry| entry.tier == "adaptive")
        .collect();
    if adaptive.is_empty() {
        plan.push_str("- none recommended by this audit\n");
    } else {
        for entry in &adaptive {
            let accepted = accepts.iter().find(|accept| accept.job == entry.job);
            let status = match accepted {
                Some(accept) => format!("accepted; command `{}`", accept.command),
                None => "not accepted; no policy generated".to_owned(),
            };
            plan.push_str(&format!(
                "- `{}` ({}) - {}. {status}. judgment: {}; receipts: {}\n",
                entry.job,
                entry.workflow,
                entry.reason,
                entry.judgment,
                entry.receipts.join(", ")
            ));
        }
    }
    plan.push_str("\n## Label-gated / nightly / release\n\n");
    plan.push_str(&setup_ci_section_bullets(jobs, "label-gated"));
    plan.push_str("\n## Human review required\n\n");
    plan.push_str(&setup_ci_section_bullets(jobs, "flag-for-human"));
    plan.push_str("\n## Proposed branch protection change\n\n");
    plan.push_str(&setup_ci_branch_protection_change(
        inventory,
        required_check,
    ));
    plan.push_str("\n## Rollback\n\n");
    plan.push_str(
        "- revert the migration PR; nothing else changed. Branch protection is never \
         mutated by setup-ci, so the only manual step is removing the required check if \
         it was added by hand.\n",
    );
    if !accepts.is_empty() {
        plan.push_str("\n## Generated .ub-review.toml additions\n\n```toml\n");
        plan.push_str(&render_setup_ci_gate_config(
            accepts,
            jobs,
            inventory,
            required_check,
        ));
        plan.push_str("```\n");
    }
    plan
}

/// Standard base64 (RFC 4648, with padding) for the GitHub contents API.
pub(crate) fn base64_standard(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        encoded.push(ALPHABET[(triple >> 18) as usize & 0x3f] as char);
        encoded.push(ALPHABET[(triple >> 12) as usize & 0x3f] as char);
        encoded.push(if chunk.len() > 1 {
            ALPHABET[(triple >> 6) as usize & 0x3f] as char
        } else {
            '='
        });
        encoded.push(if chunk.len() > 2 {
            ALPHABET[triple as usize & 0x3f] as char
        } else {
            '='
        });
    }
    encoded
}

/// Render the generated consumer gate workflow, pinned to the given
/// ub-review commit SHA. Mirrors this repository's own gate workflow shape
/// (job name = the required check name) at the zero-key tier: model-mode
/// off, no heavy witnesses, tool-bundle core. Model keys are a documented
/// edit, never a generated secret reference.
pub(crate) fn render_setup_ci_gate_workflow(action_sha: &str, required_check: &str) -> String {
    format!(
        r#"name: {required_check}

# Generated by `ub-review setup-ci`. The gate runs the proofs declared in
# .ub-review.toml and reports one required check. Model lanes are off until
# the repo opts in (model-mode + a provider key input).
on:
  pull_request:
    types: [opened, reopened, ready_for_review, synchronize]

permissions:
  contents: read
  pull-requests: write
  checks: write

concurrency:
  group: ub-review-gate-${{{{ github.event.pull_request.number || github.ref }}}}
  cancel-in-progress: true

jobs:
  gate:
    name: {required_check}
    runs-on: ubuntu-latest
    timeout-minutes: 60
    steps:
      - uses: actions/checkout@v5
        with:
          fetch-depth: 0
          persist-credentials: false

      - name: ub-review gate
        uses: EffortlessMetrics/ub-review@{action_sha}
        with:
          mode: intelligent-ci
          fail-on-gate: auto
          root: .
          base: origin/${{{{ github.base_ref }}}}
          head: HEAD
          out: target/ub-review
          install-tools: 'true'
          tool-bundle: core
          posting: artifact-only
          model-mode: 'off'
          github-token: ${{{{ github.token }}}}

      - name: Upload ub-review artifacts
        if: always()
        uses: actions/upload-artifact@v7
        with:
          name: ub-review-${{{{ github.event.pull_request.number || github.run_id }}}}
          path: target/ub-review
          if-no-files-found: warn
          retention-days: 7
"#
    )
}

pub(crate) struct SetupCiGeneratedFile {
    pub(crate) path: &'static str,
    pub(crate) content: String,
    pub(crate) message: &'static str,
}

pub(crate) fn setup_ci_generated_files(
    plan: &str,
    generated_config: &str,
    branch_protection_doc: &str,
    action_sha: &str,
    required_check: &str,
) -> Vec<SetupCiGeneratedFile> {
    vec![
        SetupCiGeneratedFile {
            path: ".ub-review.toml",
            content: generated_config.to_owned(),
            message: "Add the ub-review gate policy from the CI audit",
        },
        SetupCiGeneratedFile {
            path: ".github/workflows/ub-review-gate.yml",
            content: render_setup_ci_gate_workflow(action_sha, required_check),
            message: "Add the ub-review gate workflow",
        },
        SetupCiGeneratedFile {
            path: "docs/ci/ub-review-migration.md",
            content: plan.to_owned(),
            message: "Record the CI migration plan and its audit receipts",
        },
        SetupCiGeneratedFile {
            path: "docs/ci/branch-protection-change.md",
            content: branch_protection_doc.to_owned(),
            message: "Record the manual branch protection change",
        },
    ]
}

pub(crate) fn valid_setup_ci_action_sha(args: &SetupCiArgs) -> Result<Option<&str>> {
    match args.action_sha.as_deref().map(str::trim) {
        Some(sha) if sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()) => Ok(Some(sha)),
        Some(_) => bail!(
            "--action-sha must be the full 40-hex ub-review commit to pin in the generated workflow"
        ),
        None => Ok(None),
    }
}

pub(crate) fn write_setup_ci_preview_files(
    dir: &Path,
    files: &[SetupCiGeneratedFile],
) -> Result<Vec<String>> {
    let preview_dir = dir.join("preview");
    let mut written = Vec::new();
    for file in files {
        let path = preview_dir.join(file.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        }
        fs::write(&path, &file.content).with_context(|| format!("write {}", path.display()))?;
        written.push(file.path.to_owned());
    }
    Ok(written)
}

/// The receipt `--open-pr` writes (ub-review.setup_pr_result.v1).
#[derive(Debug, Serialize)]
pub(crate) struct SetupPrResult {
    pub(crate) schema: String,
    pub(crate) repo: String,
    pub(crate) base: String,
    pub(crate) branch: String,
    pub(crate) pr_url: String,
    pub(crate) files: Vec<String>,
    pub(crate) action_sha: String,
}

pub(crate) struct SetupCiOpenContext<'a> {
    pub(crate) token: &'a str,
    pub(crate) out_dir: &'a Path,
}

pub(crate) fn setup_ci_api_post(
    context: &SetupCiOpenContext<'_>,
    method: &str,
    url: &str,
    payload: &serde_json::Value,
    receipt_name: &str,
) -> Result<serde_json::Value> {
    let payload_path = context.out_dir.join(receipt_name);
    fs::write(&payload_path, serde_json::to_vec_pretty(payload)?)
        .with_context(|| format!("write {}", payload_path.display()))?;
    let output = run_curl_json_send(
        Path::new("."),
        method,
        url,
        &format!("Authorization: Bearer {}", context.token),
        &payload_path,
        &[
            "Accept: application/vnd.github+json",
            "Content-Type: application/json",
            "X-GitHub-Api-Version: 2022-11-28",
        ],
        60,
    )
    .with_context(|| format!("{method} {url}"))?;
    if !output.status.success() {
        bail!(
            "{method} {url} failed with http status {:?}: {}",
            output.http_status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(serde_json::from_slice(&output.stdout).unwrap_or(serde_json::Value::Null))
}

/// Open the migration PR: one new branch from the default branch, four new
/// files (config, gate workflow, migration plan doc, branch-protection doc), one PR whose body is
/// the plan. Refuses to edit a repo that already carries a .ub-review.toml
/// (file edits are a later slice); never touches branch protection.
pub(crate) fn execute_setup_ci_open_pr(
    args: &SetupCiArgs,
    plan: &str,
    generated_config: &str,
    branch_protection_doc: &str,
    required_check: &str,
) -> Result<SetupPrResult> {
    let token = args
        .github_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("--open-pr needs a GitHub token (GITHUB_TOKEN)"))?;
    let repo = args
        .repo
        .as_deref()
        .filter(|value| is_valid_repo_slug(value))
        .ok_or_else(|| anyhow::anyhow!("--open-pr needs a valid --repo owner/name slug"))?;
    let action_sha = valid_setup_ci_action_sha(args)?.ok_or_else(|| {
        anyhow::anyhow!(
            "--open-pr needs --action-sha, the full 40-hex ub-review commit to pin \
             in the generated workflow; the generator refuses to invent a pin"
        )
    })?;
    let api_url = args.github_api_url.trim_end_matches('/');
    let out_dir = args.out.join("ci-audit");
    let context = SetupCiOpenContext {
        token,
        out_dir: &out_dir,
    };

    let repo_value = run_github_api_get(Path::new("."), &format!("{api_url}/repos/{repo}"), token)
        .with_context(|| "read repository metadata")?;
    let base = repo_value
        .get("default_branch")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("repository metadata has no default_branch"))?
        .to_owned();
    let base_ref = run_github_api_get(
        Path::new("."),
        &format!("{api_url}/repos/{repo}/git/ref/heads/{base}"),
        token,
    )
    .with_context(|| "read default branch ref")?;
    let base_sha = base_ref
        .get("object")
        .and_then(|object| object.get("sha"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("default branch ref has no object.sha"))?
        .to_owned();
    let tree = run_github_api_get(
        Path::new("."),
        &format!("{api_url}/repos/{repo}/git/trees/{base_sha}"),
        token,
    )
    .with_context(|| "read default branch tree")?;
    let has_config = tree
        .get("tree")
        .and_then(serde_json::Value::as_array)
        .is_some_and(|entries| {
            entries.iter().any(|entry| {
                entry.get("path").and_then(serde_json::Value::as_str) == Some(".ub-review.toml")
            })
        });
    if has_config {
        bail!(
            "{repo} already has a .ub-review.toml; this slice only creates new files. \
             Apply the printed additions by hand or wait for the config-edit slice."
        );
    }

    setup_ci_api_post(
        &context,
        "POST",
        &format!("{api_url}/repos/{repo}/git/refs"),
        &serde_json::json!({
            "ref": format!("refs/heads/{}", args.branch),
            "sha": base_sha,
        }),
        "setup-pr-branch-payload.json",
    )
    .with_context(|| format!("create branch {} (does it already exist?)", args.branch))?;

    let files = setup_ci_generated_files(
        plan,
        generated_config,
        branch_protection_doc,
        action_sha,
        required_check,
    );
    let mut file_paths = Vec::new();
    for (index, file) in files.iter().enumerate() {
        setup_ci_api_post(
            &context,
            "PUT",
            &format!("{api_url}/repos/{repo}/contents/{}", file.path),
            &serde_json::json!({
                "message": file.message,
                "content": base64_standard(file.content.as_bytes()),
                "branch": args.branch,
            }),
            &format!("setup-pr-file-payload-{index}.json"),
        )
        .with_context(|| format!("create {}", file.path))?;
        file_paths.push(file.path.to_owned());
    }

    let pr = setup_ci_api_post(
        &context,
        "POST",
        &format!("{api_url}/repos/{repo}/pulls"),
        &serde_json::json!({
            "title": "Adopt ub-review/gate from the CI audit",
            "head": args.branch,
            "base": base,
            "body": plan,
        }),
        "setup-pr-pull-payload.json",
    )
    .with_context(|| "open the migration PR")?;
    let pr_url = pr
        .get("html_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(SetupPrResult {
        schema: SETUP_PR_RESULT_SCHEMA.to_owned(),
        repo: repo.to_owned(),
        base,
        branch: args.branch.clone(),
        pr_url,
        files: file_paths,
        action_sha: action_sha.to_owned(),
    })
}

pub(crate) fn remove_stale_setup_ci_terminal_receipt(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("remove stale {}", path.display())),
    }
}

pub(crate) fn cmd_setup_ci(args: SetupCiArgs) -> Result<()> {
    if !args.print_pr && !args.open_pr {
        bail!(
            "setup-ci does nothing implicitly: pass --print-pr to render the migration \
             PR contents from a prior audit-ci run, or --open-pr to open it."
        );
    }
    let dir = args.out.join("ci-audit");
    let inventory: CiInventoryArtifact =
        load_ci_audit_receipt(&dir, "inventory.json", CI_INVENTORY_SCHEMA)?;
    let recommendations: CiRecommendationsArtifact =
        load_ci_audit_receipt(&dir, "recommendations.json", CI_RECOMMENDATIONS_SCHEMA)?;
    for (name, expected_schema) in [
        ("history.json", CI_HISTORY_SCHEMA),
        ("costs.json", CI_COSTS_SCHEMA),
        ("correlation.json", CI_CORRELATION_SCHEMA),
    ] {
        let _: serde_json::Value = load_ci_audit_receipt(&dir, name, expected_schema)?;
    }
    if dir.join("runner-cancellations.json").exists() {
        let _: serde_json::Value = load_ci_audit_receipt(
            &dir,
            "runner-cancellations.json",
            CI_RUNNER_CANCELLATIONS_SCHEMA,
        )?;
    }
    let accepts = parse_setup_ci_accepts(&args.accept)?;
    for accept in &accepts {
        let Some(recommendation) = recommendations
            .jobs
            .iter()
            .find(|entry| entry.job == accept.job)
        else {
            bail!(
                "--accept `{}` does not match any job in ci-audit/recommendations.json",
                accept.job
            );
        };
        match recommendation.tier.as_str() {
            tier if setup_ci_required_flag_for_tier(tier).is_some() => {}
            "flag-for-human" => bail!(
                "--accept `{}` refused: flag-for-human recommendations never become \
                 generated edits; a human reviews that job directly",
                accept.job
            ),
            tier => bail!(
                "--accept `{}` refused: tier `{tier}` proposes no generated edit; only \
                 adaptive or move-to-ub-review-required jobs are acceptable",
                accept.job
            ),
        }
    }
    let required_check = Config::load_or_default(&args.config, None)
        .map(|config| config.gate.required_check)
        .unwrap_or_else(|_| "ub-review/gate".to_owned());
    let plan =
        render_setup_ci_migration_plan(&inventory, &recommendations, &accepts, &required_check);
    let branch_protection_doc =
        render_setup_ci_branch_protection_doc(&inventory, &recommendations, &required_check);
    let generated =
        render_setup_ci_gate_config(&accepts, &recommendations.jobs, &inventory, &required_check);
    if !accepts.is_empty() {
        // The round-trip oracle, enforced at runtime too: a generated config
        // the loader strips keys from is a generator failure, abort.
        let reloaded = Config::from_toml_with_policy_receipts(&generated)
            .with_context(|| "generated config failed to parse; generator failure")?;
        if !reloaded.policy_errors.is_empty() {
            bail!(
                "generator failure: generated config reloads with policy receipts: {:?}",
                reloaded.policy_errors
            );
        }
    }
    fs::create_dir_all(&dir).with_context(|| format!("create {}", dir.display()))?;
    let plan_path = dir.join("migration-plan.md");
    fs::write(&plan_path, &plan).with_context(|| format!("write {}", plan_path.display()))?;
    print!("{plan}");
    eprintln!("wrote {}", plan_path.display());
    if args.print_pr && !accepts.is_empty() {
        if let Some(action_sha) = valid_setup_ci_action_sha(&args)? {
            let files = setup_ci_generated_files(
                &plan,
                &generated,
                &branch_protection_doc,
                action_sha,
                &required_check,
            );
            let written = write_setup_ci_preview_files(&dir, &files)?;
            eprintln!(
                "wrote {} setup-ci preview file(s) under {}",
                written.len(),
                dir.join("preview").display()
            );
        } else {
            let preview_dir = dir.join("preview");
            if preview_dir.exists() {
                fs::remove_dir_all(&preview_dir)
                    .with_context(|| format!("remove stale {}", preview_dir.display()))?;
            }
            eprintln!(
                "skipped setup-ci preview files; pass --action-sha <40-hex-sha> to render the pinned workflow preview"
            );
        }
    }
    if args.open_pr {
        if accepts.is_empty() {
            bail!(
                "--open-pr with no accepted jobs has no migration PR to open; the plan \
                 above explains the tiers. Pass --accept <job>=<command> for the \
                 adaptive jobs to fold in."
            );
        }
        let result_path = dir.join("setup-pr-result.json");
        let error_path = dir.join("setup-pr-error.json");
        remove_stale_setup_ci_terminal_receipt(&result_path)?;
        remove_stale_setup_ci_terminal_receipt(&error_path)?;
        match execute_setup_ci_open_pr(
            &args,
            &plan,
            &generated,
            &branch_protection_doc,
            &required_check,
        ) {
            Ok(result) => {
                fs::write(&result_path, serde_json::to_vec_pretty(&result)?)
                    .with_context(|| format!("write {}", result_path.display()))?;
                println!("opened {}", result.pr_url);
                eprintln!("wrote {}", result_path.display());
            }
            Err(err) => {
                fs::write(
                    &error_path,
                    serde_json::to_vec_pretty(&serde_json::json!({
                        "schema": SETUP_PR_ERROR_SCHEMA,
                        "status": "failed",
                        "reason": format!("{err:#}"),
                    }))?,
                )
                .with_context(|| format!("write {}", error_path.display()))?;
                return Err(err);
            }
        }
    }
    Ok(())
}
