//! Init command inspection and guide rendering (cleanup train step 18,
//! pure code motion). Scans the repo for build systems, CI workflows,
//! source files, and package scripts, then renders the init guide with
//! a config proposal and model-assist handoff.

use crate::*;

#[derive(Debug)]
pub(crate) struct InitGuideInspection {
    pub(crate) root: PathBuf,
    pub(crate) build_systems: Vec<String>,
    pub(crate) workflows: Vec<CiWorkflowScan>,
    pub(crate) package_scripts: Vec<InitPackageScript>,
    pub(crate) audit_ci: Option<InitAuditCiInspection>,
    pub(crate) rust_source_count: usize,
    pub(crate) rust_test_count: usize,
    pub(crate) docs_or_specs_count: usize,
    pub(crate) unsafe_native_found: bool,
    pub(crate) cargo_allow_path: Option<String>,
}

#[derive(Debug)]
pub(crate) struct InitAuditCiInspection {
    pub(crate) dir: PathBuf,
    pub(crate) inventory: Option<CiInventoryArtifact>,
    pub(crate) recommendations: Option<CiRecommendationsArtifact>,
    pub(crate) audit_report: Option<PathBuf>,
    pub(crate) evidence_gaps: Vec<String>,
}

#[derive(Debug)]
pub(crate) struct InitPackageScript {
    pub(crate) name: String,
    pub(crate) command: String,
    pub(crate) definition: String,
}

pub(crate) fn cmd_init(args: InitArgs) -> Result<()> {
    if args.path.exists() && !args.force {
        bail!(
            "{} already exists; pass --force to overwrite",
            args.path.display()
        );
    }
    if !args.no_guide {
        if init_destination_key(&args.path)? == init_destination_key(&args.guide_out)? {
            bail!(
                "--path and --guide-out must name different files ({})",
                args.path.display()
            );
        }
        if args.guide_out.exists() && !args.force {
            bail!(
                "{} already exists; pass --force to overwrite",
                args.guide_out.display()
            );
        }
        if !args.root.is_dir() {
            bail!(
                "{} is not a directory; pass --root <repo> or --no-guide",
                args.root.display()
            );
        }
    }
    let config = Config {
        profile: args.profile.key().to_owned(),
        ..Config::default()
    };
    let guide = if args.no_guide {
        None
    } else {
        Some(render_init_guide(&args, &config)?)
    };
    fs::write(&args.path, toml::to_string_pretty(&config)?)?;
    println!("wrote {}", args.path.display());
    if let Some(guide) = guide {
        fs::write(&args.guide_out, guide)?;
        println!("wrote {}", args.guide_out.display());
    }
    Ok(())
}

pub(crate) fn init_destination_key(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .context("resolve current directory for init output paths")?
            .join(path)
    };
    Ok(init_normalize_path_lexically(&absolute))
}

pub(crate) fn init_normalize_path_lexically(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push(component.as_os_str());
                }
            }
            Component::Normal(part) => normalized.push(part),
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(component.as_os_str()),
        }
    }
    normalized
}

pub(crate) fn render_init_guide(args: &InitArgs, config: &Config) -> Result<String> {
    let inspection = inspect_init_guide_repo(&args.root)?;
    let mut text = String::new();
    text.push_str("# ub-review init guide\n\n");
    text.push_str("## Decision\n\n");
    text.push_str(&format!(
        "- Starter config: `{}` using profile `{}`.\n",
        args.path.display(),
        config.profile
    ));
    text.push_str(
        "- Keep setup advisory until one red proof and one quiet-green run are verified.\n",
    );
    text.push_str("- `audit-ci` and `setup-ci` own CI migration; `init` only writes the starter config and this handoff.\n\n");

    text.push_str("## Repo inspection\n\n");
    text.push_str(&format!("- Root: `{}`.\n", inspection.root.display()));
    if inspection.build_systems.is_empty() {
        text.push_str("- Build systems: no recognized manifest found; add required proof commands manually.\n");
    } else {
        text.push_str(&format!(
            "- Build systems: {}.\n",
            inspection.build_systems.join("; ")
        ));
    }
    text.push_str(&format!(
        "- Rust source scan: {} `.rs` files, {} test-bearing files, unsafe/native markers {}.\n",
        inspection.rust_source_count,
        inspection.rust_test_count,
        if inspection.unsafe_native_found {
            "found"
        } else {
            "not found"
        }
    ));
    if inspection.workflows.is_empty() {
        text.push_str("- Existing workflows: none under `.github/workflows`.\n");
    } else {
        text.push_str("- Existing workflows:\n");
        for workflow in &inspection.workflows {
            let triggers = init_join_or_none(&workflow.triggers);
            let jobs = workflow.yaml_jobs.len();
            let cancel = if workflow.cancel_in_progress {
                ", cancels superseded runs"
            } else {
                ""
            };
            text.push_str(&format!(
                "  - `{}`: {jobs} jobs, triggers {triggers}{cancel}.\n",
                workflow.path
            ));
        }
    }

    text.push_str("\n## Sensor receipts\n\n");
    text.push_str("- `tokmd`: generate the context packet before model lanes.\n");
    if inspection.workflows.is_empty() {
        text.push_str("- `actionlint`: not selected until workflows exist or change.\n");
    } else {
        text.push_str("- `actionlint`: selected when workflow files change.\n");
    }
    if inspection.rust_source_count == 0 {
        text.push_str(
            "- `ripr`: no Rust source detected; add only if Rust proof enters the repo.\n",
        );
    } else {
        text.push_str("- `ripr`: selected for Rust behavior or test changes; persist finding details in artifacts.\n");
    }
    if inspection.unsafe_native_found {
        text.push_str("- `unsafe-review`: unsafe/native markers found; keep receipts structured and route posting through ub-review.\n");
    } else {
        text.push_str("- `unsafe-review`: no unsafe/native markers found in the bounded scan; enable when unsafe/native risk changes.\n");
    }
    match inspection.cargo_allow_path.as_deref() {
        Some(path) => text.push_str(&format!(
            "- `cargo-allow`: candidate policy ledger at `{path}`; verify dialect before making it required.\n"
        )),
        None => text.push_str(
            "- `cargo-allow`: no `policy/allow.toml` ledger detected; add only with an owned policy receipt.\n",
        ),
    }

    render_init_audit_ci_summary(&mut text, &inspection);
    render_init_model_assist_handoff(&mut text, &inspection);
    render_init_config_proposal(&mut text, &inspection);

    text.push_str("\n## Recommended path\n\n");
    text.push_str(&format!(
        "1. Run `ub-review doctor --config {} --root {} --require-core-tools` and fix missing tools or provider keys before trusting the standard gate image.\n",
        args.path.display(),
        args.root.display()
    ));
    text.push_str(&format!(
        "2. Run `ub-review audit-ci --root {} --out target/ub-review` for read-only CI receipts.\n",
        args.root.display()
    ));
    text.push_str(
        "3. Run `ub-review setup-ci --print-pr --out target/ub-review` to inspect the migration without writes.\n",
    );
    text.push_str(
        "4. Add explicit `--accept <job>=<command>` values only for audited `adaptive` or `move-to-ub-review-required` jobs a maintainer can run and explain.\n",
    );
    text.push_str(
        "5. Run `ub-review setup-ci --open-pr --out target/ub-review --action-sha <40-hex-sha>` only after reviewing the rendered PR.\n",
    );

    text.push_str("\n## Open decisions\n\n");
    text.push_str("- Model key: configure `MINIMAX_API_KEY` for the action or map runner env to `UB_REVIEW_MINIMAX_API_KEY`; keep secret values out of files.\n");
    text.push_str("- Required floor: decide which existing CI jobs become required proof versus adaptive, risk-pack, nightly, or human-reviewed.\n");
    text.push_str("- Accepted tiers: `move-to-ub-review-required` materializes as required proof; `adaptive` materializes as non-required proof; `keep-required`, `flag-for-human`, risk-pack, nightly, release, deploy, provenance, and compliance jobs remain manual unless a later audited receipt changes the tier.\n");
    text.push_str("- Branch protection: update required checks manually after the advisory PR proves one red run and one quiet-green run.\n");
    text.push_str("- Follow-up capture: route real out-of-scope work into issue candidates with evidence, plan, and acceptance criteria.\n");

    Ok(text)
}

pub(crate) fn render_init_audit_ci_summary(text: &mut String, inspection: &InitGuideInspection) {
    let Some(audit) = inspection.audit_ci.as_ref() else {
        return;
    };

    text.push_str("\n## Audit-ci receipt summary\n\n");
    text.push_str(&format!(
        "- Existing audit-ci receipts: `{}`.\n",
        init_display_repo_path(&inspection.root, &audit.dir)
    ));
    match audit.inventory.as_ref() {
        Some(inventory) => text.push_str(&format!(
            "- Inventory: {} jobs for `{}` over {} days.\n",
            inventory.jobs.len(),
            init_markdown_inline_code(&inventory.repo),
            inventory.window_days
        )),
        None => text.push_str("- Inventory: unavailable; rerun `ub-review audit-ci --out target/ub-review` before setup-ci materialization.\n"),
    }
    if let Some(recommendations) = audit.recommendations.as_ref() {
        text.push_str(&format!(
            "- Recommendations: {} jobs for `{}` over {} days.\n",
            recommendations.jobs.len(),
            init_markdown_inline_code(&recommendations.repo),
            recommendations.window_days
        ));
        for (tier, label) in CI_AUDIT_REPORT_TIER_SECTIONS {
            let entries: Vec<&CiRecommendation> = recommendations
                .jobs
                .iter()
                .filter(|entry| entry.tier == *tier)
                .collect();
            if entries.is_empty() {
                continue;
            }
            text.push_str(&format!("- {label} (`{tier}`):\n"));
            for entry in entries.iter().take(8) {
                text.push_str(&format!(
                    "  - `{}` from `{}` - {}. receipts: {}\n",
                    init_markdown_inline_code(&entry.job),
                    init_markdown_inline_code(&entry.workflow),
                    init_markdown_plain(&entry.reason),
                    init_join_markdown_code(&entry.receipts)
                ));
            }
            if entries.len() > 8 {
                text.push_str(&format!(
                    "  - ... {} more jobs in this tier; inspect `ci-audit/recommendations.json`.\n",
                    entries.len() - 8
                ));
            }
        }
        for gap in recommendations.evidence_gaps.iter().take(4) {
            text.push_str(&format!(
                "- Recommendation evidence gap: {}.\n",
                init_markdown_plain(gap)
            ));
        }
        let accept_candidates = init_setup_ci_accept_candidates(recommendations);
        if !accept_candidates.is_empty() {
            text.push_str(
                "- Acceptable setup-ci candidates (commands still maintainer-supplied):\n",
            );
            for entry in accept_candidates.iter().take(8) {
                text.push_str(&format!(
                    "  - `{}` (`{}`): add `{}` after running the command locally; receipts: {}\n",
                    init_markdown_inline_code(&entry.job),
                    init_markdown_inline_code(&entry.tier),
                    init_setup_ci_accept_placeholder(&entry.job),
                    init_join_markdown_code(&entry.receipts)
                ));
            }
            if accept_candidates.len() > 8 {
                text.push_str(&format!(
                    "  - ... {} more acceptable jobs; inspect `ci-audit/recommendations.json`.\n",
                    accept_candidates.len() - 8
                ));
            }
            text.push_str(
                "  - Leave `keep-required`, `flag-for-human`, risk-pack, nightly, release, deploy, provenance, and compliance jobs manual unless a later audit changes their tier.\n",
            );
        }
    } else {
        text.push_str("- Recommendations: unavailable; rerun `ub-review audit-ci --out target/ub-review` before setup-ci materialization.\n");
    }
    if let Some(inventory) = audit.inventory.as_ref() {
        for gap in inventory.evidence_gaps.iter().take(4) {
            text.push_str(&format!(
                "- Inventory evidence gap: {}.\n",
                init_markdown_plain(gap)
            ));
        }
    }
    for gap in &audit.evidence_gaps {
        text.push_str(&format!(
            "- Audit receipt evidence gap: {}.\n",
            init_markdown_plain(gap)
        ));
    }
    text.push_str(
        "- Setup boundary: audit receipts do not record runnable commands; use explicit `setup-ci --accept <job>=<command>` only for audited `adaptive` or `move-to-ub-review-required` jobs.\n",
    );
}

pub(crate) fn init_setup_ci_accept_candidates(
    recommendations: &CiRecommendationsArtifact,
) -> Vec<&CiRecommendation> {
    let mut entries = Vec::new();
    for (tier, _) in CI_AUDIT_REPORT_TIER_SECTIONS {
        if setup_ci_required_flag_for_tier(tier).is_none() {
            continue;
        }
        entries.extend(
            recommendations
                .jobs
                .iter()
                .filter(|entry| entry.tier == *tier),
        );
    }
    entries
}

pub(crate) fn render_init_model_assist_handoff(
    text: &mut String,
    inspection: &InitGuideInspection,
) {
    let Some(audit) = inspection.audit_ci.as_ref() else {
        return;
    };

    text.push_str("\n## Model-assisted config proposal input\n\n");
    text.push_str(&format!(
        "- Bounded deterministic inputs: `{}/inventory.json`, `{}/recommendations.json`, and `{}/audit-report.md` when present and readable.\n",
        init_display_repo_path(&inspection.root, &audit.dir),
        init_display_repo_path(&inspection.root, &audit.dir),
        init_display_repo_path(&inspection.root, &audit.dir)
    ));
    if let Some(path) = audit.audit_report.as_ref() {
        text.push_str(&format!(
            "- Human audit report: `{}` pairs tier summaries with backticked recommendation receipt pointers.\n",
            init_display_repo_path(&inspection.root, path)
        ));
    } else {
        text.push_str(
            "- Human audit report: unavailable; rerun `ub-review audit-ci --out target/ub-review` before asking a model or external agent to propose setup-ci accepts.\n",
        );
    }
    text.push_str(
        "- Use recommendation receipts as pointers to supporting audit artifacts; do not infer from workflow names alone.\n",
    );
    match audit.recommendations.as_ref() {
        Some(recommendations) => {
            let accept_candidates = init_setup_ci_accept_candidates(recommendations);
            if accept_candidates.is_empty() {
                text.push_str("- Setup-ci accepts: none proposed by deterministic receipts; ask for more audit proof before materializing commands.\n");
            } else {
                text.push_str(&format!(
                    "- Setup-ci accepts: {} audited `adaptive` or `move-to-ub-review-required` jobs may be proposed only with maintainer-supplied commands.\n",
                    accept_candidates.len()
                ));
            }
            text.push_str(
                "- Manual boundary: keep-required, flag-for-human, risk-pack, nightly, release, deploy, provenance, and compliance jobs stay manual unless later receipts retier them.\n",
            );
            if !recommendations.evidence_gaps.is_empty() {
                text.push_str(
                    "- Evidence gaps: convert unresolved recommendation gaps into verification questions, not config changes.\n",
                );
            }
        }
        None => text.push_str(
            "- Recommendations are unavailable; rerun `ub-review audit-ci --out target/ub-review` before asking a model or external agent to propose setup-ci accepts.\n",
        ),
    }
    if !audit.evidence_gaps.is_empty() {
        text.push_str(
            "- Receipt gaps: treat missing or unreadable audit receipts as blockers for materialization.\n",
        );
    }
    text.push_str(
        "- Proposal boundary: a model or external agent may draft rationale and open questions from these receipts, but must not invent commands, treat model judgment as proof, enable posting/blocking, or mutate branch protection.\n",
    );
}

pub(crate) fn render_init_config_proposal(text: &mut String, inspection: &InitGuideInspection) {
    text.push_str("\n## File-driven config proposal\n\n");
    text.push_str(
        "Use this section as a handoff for Codex, Claude, or the maintainer. It is not branch protection and it should not be pasted into policy until the command has run in this repo.\n\n",
    );

    text.push_str("### Required proof candidates\n\n");
    if init_inspection_has_cargo(inspection) {
        text.push_str("- `cargo-fmt`: `cargo fmt --all --check` - Rust formatting floor.\n");
        text.push_str("- `cargo-check`: `cargo check --workspace --all-targets --locked` - fast compile floor.\n");
        text.push_str("- `cargo-test`: `cargo test --workspace --locked` - repo test floor; split or narrow only when audit receipts show the full suite is too costly.\n");
        text.push_str("- `cargo-clippy`: `cargo clippy --workspace --all-targets --locked -- -D warnings` - lint floor when the repo already treats clippy as merge-relevant.\n");
        text.push_str("- `cargo-doc`: `cargo doc --workspace --no-deps --locked` - documentation/API surface floor for public Rust crates.\n");
        if inspection.docs_or_specs_count > 0 {
            text.push_str("- Repo policy verifier: keep any existing `cargo xtask policy-check` or artifact-verifier command if audit-ci receipts prove it runs and fails independently.\n");
        }
    } else {
        text.push_str("- No root Cargo manifest detected; derive required proof from `audit-ci` receipts and maintainer-supplied `--accept <job>=<command>` values.\n");
    }
    if !inspection.package_scripts.is_empty() {
        text.push_str("- JavaScript/TypeScript package scripts detected in `package.json` (candidate commands only after a maintainer runs them locally):\n");
        for script in &inspection.package_scripts {
            text.push_str(&format!(
                "  - `{}`: `{}` (script: `{}`).\n",
                init_markdown_inline_code(&script.name),
                init_markdown_inline_code(&script.command),
                init_markdown_inline_code(&script.definition)
            ));
        }
    }
    if inspection.workflows.is_empty() {
        text.push_str("- `actionlint`: wait until workflow files exist or change.\n");
    } else {
        text.push_str("- `actionlint`: run for workflow changes after doctor confirms the binary is installed.\n");
    }
    text.push_str("- Do not materialize any candidate with `setup-ci --accept` until a maintainer has run it locally or audit receipts prove it is runnable and merge-relevant; only audited `adaptive` and `move-to-ub-review-required` recommendations can become generated proof.\n");

    text.push_str("\n### Repo-specific lane proposal\n\n");
    if inspection.rust_test_count > 0 {
        text.push_str("- `tests`: review oracle strength and request focused red/green proof for changed tests or behavior.\n");
    } else {
        text.push_str(
            "- `tests`: keep narrow; enable when test files or behavior claims change.\n",
        );
    }
    if inspection.rust_source_count > 0 {
        text.push_str(
            "- `source-route`: trace changed Rust routes, public callers, and sibling paths.\n",
        );
    }
    if inspection.unsafe_native_found {
        text.push_str("- `ub`: route unsafe/native changes through unsafe-review receipts; do not treat missing receipts as safety evidence.\n");
    }
    if !inspection.workflows.is_empty() {
        text.push_str("- `gate-semantics`: route workflow and gate-policy changes here so red/green/quiet behavior stays honest.\n");
    }
    if inspection.docs_or_specs_count > 0 {
        text.push_str("- `spec-honesty`: route docs/spec claims here so implemented behavior is not overstated.\n");
    }
    text.push_str(
        "- Keep diff-irrelevant lanes artifact-only; do not run every lane on every PR.\n",
    );
}

pub(crate) fn init_inspection_has_cargo(inspection: &InitGuideInspection) -> bool {
    inspection
        .build_systems
        .iter()
        .any(|system| system.starts_with("Rust "))
}

pub(crate) fn inspect_init_guide_repo(root: &Path) -> Result<InitGuideInspection> {
    let root = root.to_path_buf();
    let workflows = scan_local_workflows(&root)?;
    let rust_files = collect_init_repo_files(&root, is_rust_source_file, 512);
    let rust_source_count = rust_files.len();
    let rust_test_count = rust_files
        .iter()
        .filter(|path| init_rust_file_has_tests(path))
        .count();
    let unsafe_native_found = rust_files
        .iter()
        .take(256)
        .any(|path| init_rust_file_has_unsafe_native(path));
    let docs_or_specs_count = collect_init_repo_files(&root, is_init_docs_or_spec_file, 256).len();
    let mut build_systems = Vec::new();
    if root.join("Cargo.toml").is_file() {
        build_systems.push(init_cargo_manifest_summary(&root));
    }
    if root.join("package.json").is_file() {
        build_systems.push("JavaScript/TypeScript (`package.json`)".to_owned());
    }
    let package_scripts = init_package_json_scripts(&root);
    let audit_ci = inspect_init_audit_ci_receipts(&root);
    if root.join("pyproject.toml").is_file() {
        build_systems.push("Python (`pyproject.toml`)".to_owned());
    } else if root.join("requirements.txt").is_file() {
        build_systems.push("Python (`requirements.txt`)".to_owned());
    }
    if root.join("go.mod").is_file() {
        build_systems.push("Go (`go.mod`)".to_owned());
    }
    let cargo_allow_path = root
        .join("policy")
        .join("allow.toml")
        .is_file()
        .then(|| "policy/allow.toml".to_owned());
    Ok(InitGuideInspection {
        root,
        build_systems,
        workflows,
        package_scripts,
        audit_ci,
        rust_source_count,
        rust_test_count,
        docs_or_specs_count,
        unsafe_native_found,
        cargo_allow_path,
    })
}

pub(crate) fn inspect_init_audit_ci_receipts(root: &Path) -> Option<InitAuditCiInspection> {
    let dir = root.join("target").join("ub-review").join("ci-audit");
    let inventory_path = dir.join("inventory.json");
    let recommendations_path = dir.join("recommendations.json");
    let audit_report_path = dir.join("audit-report.md");
    if !inventory_path.exists() && !recommendations_path.exists() && !audit_report_path.exists() {
        return None;
    }

    let mut evidence_gaps = Vec::new();
    let inventory = if inventory_path.is_file() {
        match load_ci_audit_receipt(&dir, "inventory.json", CI_INVENTORY_SCHEMA) {
            Ok(receipt) => Some(receipt),
            Err(error) => {
                evidence_gaps.push(format!(
                    "`{}` unreadable: {}",
                    init_display_repo_path(root, &inventory_path),
                    init_markdown_plain(&error.to_string())
                ));
                None
            }
        }
    } else {
        evidence_gaps.push(format!(
            "`{}` missing; rerun `ub-review audit-ci --out target/ub-review`",
            init_display_repo_path(root, &inventory_path)
        ));
        None
    };
    let recommendations = if recommendations_path.is_file() {
        match load_ci_audit_receipt(&dir, "recommendations.json", CI_RECOMMENDATIONS_SCHEMA) {
            Ok(receipt) => Some(receipt),
            Err(error) => {
                evidence_gaps.push(format!(
                    "`{}` unreadable: {}",
                    init_display_repo_path(root, &recommendations_path),
                    init_markdown_plain(&error.to_string())
                ));
                None
            }
        }
    } else {
        evidence_gaps.push(format!(
            "`{}` missing; rerun `ub-review audit-ci --out target/ub-review`",
            init_display_repo_path(root, &recommendations_path)
        ));
        None
    };
    let audit_report = if audit_report_path.is_file() {
        match fs::read_to_string(&audit_report_path) {
            Ok(_) => Some(audit_report_path.clone()),
            Err(error) => {
                evidence_gaps.push(format!(
                    "`{}` unreadable: {}",
                    init_display_repo_path(root, &audit_report_path),
                    init_markdown_plain(&error.to_string())
                ));
                None
            }
        }
    } else {
        evidence_gaps.push(format!(
            "`{}` missing; rerun `ub-review audit-ci --out target/ub-review`",
            init_display_repo_path(root, &audit_report_path)
        ));
        None
    };

    Some(InitAuditCiInspection {
        dir,
        inventory,
        recommendations,
        audit_report,
        evidence_gaps,
    })
}

pub(crate) fn collect_init_repo_files(
    root: &Path,
    predicate: fn(&Path) -> bool,
    limit: usize,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .collect();
        paths.sort();
        for path in paths {
            if init_skip_repo_dir(&path) {
                continue;
            }
            let Ok(metadata) = fs::symlink_metadata(&path) else {
                continue;
            };
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() && predicate(&path) {
                files.push(path);
                if files.len() >= limit {
                    return files;
                }
            }
        }
    }
    files
}

pub(crate) fn init_skip_repo_dir(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    matches!(
        name,
        ".git" | "target" | ".ub-review" | "node_modules" | ".venv" | "dist" | "build"
    )
}

pub(crate) fn is_rust_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("rs"))
}

pub(crate) fn is_init_docs_or_spec_file(path: &Path) -> bool {
    let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    if !matches!(ext.to_ascii_lowercase().as_str(), "md" | "adoc" | "rst") {
        return false;
    }
    path.components().any(|component| {
        component.as_os_str().to_str().is_some_and(|part| {
            part.eq_ignore_ascii_case("docs") || part.eq_ignore_ascii_case("specs")
        })
    }) || path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("README.md"))
}

pub(crate) fn init_rust_file_has_tests(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_str()
            .is_some_and(|part| part == "tests")
    }) || fs::read_to_string(path)
        .map(|text| text.contains("#[test]") || text.contains("#[tokio::test]"))
        .unwrap_or(false)
}

pub(crate) fn init_rust_file_has_unsafe_native(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|text| {
            text.contains("unsafe")
                || text.contains("extern \"C\"")
                || text.contains("extern \"system\"")
                || text.contains("no_mangle")
        })
        .unwrap_or(false)
}

pub(crate) fn init_cargo_manifest_summary(root: &Path) -> String {
    let manifest = root.join("Cargo.toml");
    let Ok(text) = fs::read_to_string(&manifest) else {
        return "Rust (`Cargo.toml`)".to_owned();
    };
    let package_name = init_cargo_package_name(&text);
    let Ok(value) = text.parse::<toml::Value>() else {
        return package_name
            .map(|name| format!("Rust package `{name}` (`Cargo.toml`)"))
            .unwrap_or_else(|| "Rust (`Cargo.toml`)".to_owned());
    };
    if let Some(workspace) = value.get("workspace").and_then(toml::Value::as_table) {
        let members = workspace
            .get("members")
            .and_then(toml::Value::as_array)
            .map_or(0, Vec::len);
        if members == 0 {
            return "Rust workspace (`Cargo.toml`)".to_owned();
        }
        return format!("Rust workspace (`Cargo.toml`, {members} members)");
    }
    if let Some(name) = value
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .map(str::to_owned)
        .or(package_name)
    {
        return format!("Rust package `{name}` (`Cargo.toml`)");
    }
    "Rust (`Cargo.toml`)".to_owned()
}

pub(crate) fn init_package_json_scripts(root: &Path) -> Vec<InitPackageScript> {
    let path = root.join("package.json");
    let Ok(text) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(serde_json::Value::as_object) else {
        return Vec::new();
    };
    let package_manager = init_package_manager(root);
    let mut candidates = Vec::new();
    for (name, definition) in scripts {
        let Some(definition) = definition.as_str() else {
            continue;
        };
        let definition = definition.trim();
        if definition.is_empty()
            || !init_package_script_name_is_safe(name)
            || !init_package_script_is_proof_candidate(name)
        {
            continue;
        }
        candidates.push(InitPackageScript {
            name: name.to_owned(),
            command: format!("{package_manager} run {name}"),
            definition: definition.to_owned(),
        });
        if candidates.len() >= 8 {
            break;
        }
    }
    candidates
}

pub(crate) fn init_package_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else {
        "npm"
    }
}

pub(crate) fn init_package_script_name_is_safe(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 80
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ':' | '_' | '-' | '.' | '/' | '@'))
}

pub(crate) fn init_package_script_is_proof_candidate(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.starts_with("pre") || lower.starts_with("post") {
        return false;
    }
    if [
        "dev",
        "serve",
        "start",
        "watch",
        "deploy",
        "publish",
        "release",
        "docker",
        "container",
        "image",
        "push",
        "upload",
    ]
    .iter()
    .any(|term| lower.contains(term))
    {
        return false;
    }
    [
        "test", "lint", "type", "check", "build", "fmt", "format", "doc",
    ]
    .iter()
    .any(|term| lower.contains(term))
}

pub(crate) fn init_markdown_inline_code(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '`' => '\'',
            '\r' | '\n' => ' ',
            _ => ch,
        })
        .collect::<String>()
        .trim()
        .to_owned()
}

pub(crate) fn init_markdown_plain(value: &str) -> String {
    value
        .chars()
        .map(|ch| match ch {
            '\r' | '\n' | '\t' => ' ',
            _ => ch,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

pub(crate) fn init_join_markdown_code(values: &[String]) -> String {
    if values.is_empty() {
        return "`ci-audit/recommendations.json`".to_owned();
    }
    values
        .iter()
        .map(|value| format!("`{}`", init_markdown_inline_code(value)))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn init_setup_ci_accept_placeholder(job: &str) -> String {
    format!(
        "--accept {}=\"<maintainer command>\"",
        init_markdown_inline_code(job)
    )
}

pub(crate) fn init_display_repo_path(root: &Path, path: &Path) -> String {
    let display = path.strip_prefix(root).unwrap_or(path);
    display.to_string_lossy().replace('\\', "/")
}

pub(crate) fn init_cargo_package_name(text: &str) -> Option<String> {
    let mut in_package = false;
    for line in text.lines() {
        let trimmed = line.split('#').next().unwrap_or("").trim();
        if trimmed.starts_with('[') {
            in_package = trimmed == "[package]";
            continue;
        }
        if !in_package {
            continue;
        }
        let Some((key, value)) = trimmed.split_once('=') else {
            continue;
        };
        if key.trim() != "name" {
            continue;
        }
        let name = value
            .trim()
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_owned();
        if !name.is_empty() {
            return Some(name);
        }
    }
    None
}

pub(crate) fn init_join_or_none(values: &[String]) -> String {
    if values.is_empty() {
        "none detected".to_owned()
    } else {
        values.join(", ")
    }
}
