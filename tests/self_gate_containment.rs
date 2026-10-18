use anyhow::{Context, Result, ensure};

const WORKFLOW: &str = include_str!("../.github/workflows/ub-review-gate.yml");
const INDEPENDENT_WORKFLOW: &str = include_str!("../.github/workflows/independent-baseline.yml");

fn job_section<'a>(workflow: &'a str, job: &str, next_job: Option<&str>) -> Result<&'a str> {
    let start_marker = format!("\n  {job}:\n");
    let start = workflow
        .find(&start_marker)
        .with_context(|| format!("workflow job `{job}` is missing"))?
        + 1;
    let tail = &workflow[start..];
    let end = if let Some(next_job) = next_job {
        let end_marker = format!("\n  {next_job}:\n");
        tail.find(&end_marker)
            .with_context(|| format!("workflow job `{next_job}` is missing"))?
    } else {
        tail.len()
    };
    Ok(&tail[..end])
}

fn without_protected_helper_token(section: &str, phase: &str) -> Result<String> {
    let token_line = "          ADMISSION_METADATA_TOKEN: ${{ github.token }}\n";
    ensure!(
        section.matches("github.token").count() == 1
            && section.matches("ADMISSION_METADATA_TOKEN").count() == 1
            && section.contains(&format!("          ADMISSION_PHASE: {phase}\n{token_line}")),
        "only the protected adapter step may receive the existing metadata token"
    );
    ensure!(
        section.contains("python3 -I \"$script\"")
            && section.contains("echo \"$ADAPTER_SHA256  $script\" | sha256sum --check --status"),
        "token-bearing helper must execute only digest-verified source with isolated imports"
    );
    if let Some(checkout) = section.find("actions/checkout") {
        let token = section.find(token_line).context("helper token missing")?;
        ensure!(
            token < checkout,
            "metadata token helper must precede candidate checkout"
        );
        ensure!(
            !section[checkout..].contains("ADMISSION_METADATA_TOKEN")
                && !section[checkout..].contains("github.token"),
            "candidate execution must not receive the helper token"
        );
    }
    Ok(section.replace(token_line, ""))
}

#[test]
fn metadata_tokens_are_scoped_to_isolated_protected_source_helpers() -> Result<()> {
    for (workflow, worker, next_worker) in [(INDEPENDENT_WORKFLOW, "evidence", Some("finalize"))] {
        ensure!(
            workflow.contains("ADAPTER_SOURCE_SHA: ${{ github.event.pull_request.base.sha }}")
                && workflow.contains("ADAPTER_REPOSITORY: EffortlessMetrics/ub-review")
                && !workflow
                    .contains("ADAPTER_SOURCE_SHA: ${{ github.event.pull_request.head.sha }}"),
            "credential-bearing helpers require canonical immutable protected-base source"
        );
        for (name, next, phase) in [
            ("admission", Some(worker), "admit"),
            (worker, next_worker, "recheck"),
            ("finalize", None, "finalize"),
        ] {
            let section = job_section(workflow, name, next)?;
            without_protected_helper_token(section, phase)?;
            if name != worker {
                ensure!(
                    !section.contains("actions/checkout"),
                    "metadata/finalizer must not checkout candidate code"
                );
            }
        }
    }
    Ok(())
}

#[test]
fn candidate_gate_job_is_model_off_artifact_only_and_without_privileged_inputs() -> Result<()> {
    ensure!(
        WORKFLOW.contains("permissions: {}"),
        "workflow defaults must grant no token permissions"
    );
    let gate = job_section(WORKFLOW, "gate", Some("coverage-upload"))?;

    for required in [
        "permissions:\n      contents: read",
        "persist-credentials: false",
        "uses: ./",
        "posting: artifact-only",
        "model-mode: off",
        "Verify ub-review artifacts",
        "actions/upload-artifact@v7",
    ] {
        ensure!(
            gate.contains(required),
            "contained candidate gate is missing `{required}`"
        );
    }

    for forbidden in [
        "actions: read",
        "pull-requests: write",
        "checks: write",
        "id-token: write",
        "github-token:",
        "github.token",
        "secrets.",
        "minimax-api-key:",
        "opencode-api-key:",
        "posting: review",
        "model-mode: auto",
        "codecov/codecov-action",
    ] {
        ensure!(
            !gate.contains(forbidden),
            "candidate-controlled gate must not contain `{forbidden}`"
        );
    }

    Ok(())
}

#[test]
fn coverage_oidc_is_isolated_in_a_non_candidate_telemetry_job() -> Result<()> {
    let coverage = job_section(WORKFLOW, "coverage-upload", None)?;

    for required in [
        "needs: gate",
        "actions: read",
        "id-token: write",
        "actions/download-artifact@v5",
        "codecov/codecov-action@v7",
        "use_oidc: true",
        "continue-on-error: true",
    ] {
        ensure!(
            coverage.contains(required),
            "coverage telemetry job is missing `{required}`"
        );
    }

    for forbidden in [
        "uses: ./",
        "actions/checkout",
        "github-token:",
        "github.token",
        "secrets.",
        "minimax-api-key:",
        "opencode-api-key:",
    ] {
        ensure!(
            !coverage.contains(forbidden),
            "coverage telemetry job must not contain `{forbidden}`"
        );
    }

    Ok(())
}

#[test]
fn independent_baseline_isolates_candidate_evidence_from_trusted_finalization() -> Result<()> {
    ensure!(
        INDEPENDENT_WORKFLOW.starts_with("name: ub-review/independent-baseline\n"),
        "independent check must expose one stable check name"
    );
    ensure!(
        INDEPENDENT_WORKFLOW.contains("\n  pull_request_target:\n"),
        "workflow definition must come from the protected base branch"
    );
    ensure!(
        !INDEPENDENT_WORKFLOW.contains("\n  pull_request:\n"),
        "candidate-owned pull_request orchestration must not share this workflow"
    );

    let evidence = job_section(INDEPENDENT_WORKFLOW, "evidence", Some("finalize"))?;
    let finalize = job_section(INDEPENDENT_WORKFLOW, "finalize", None)?;
    let evidence = without_protected_helper_token(evidence, "recheck")?;
    let finalize = without_protected_helper_token(finalize, "finalize")?;

    for required in [
        "name: independent evidence / ${{ matrix.check }}",
        "fail-fast: false",
        "check: [fmt, check, clippy, test, doc, policy, verifier]",
        "uses: actions/checkout@fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09",
        "repository: ${{ github.event.pull_request.head.repo.full_name }}",
        "ref: ${{ github.event.pull_request.head.sha }}",
        "persist-credentials: false",
        "actual_sha=\"$(git rev-parse HEAD)\"",
        "- name: Install fixed Rust toolchain\n        if: ${{ matrix.check != 'verifier' }}\n        uses: dtolnay/rust-toolchain@6c977a6ca4077a0ceb28ffbe03f59d46e9ac8772",
        "- name: Install fixed Python toolchain\n        if: ${{ matrix.check == 'verifier' }}\n        uses: actions/setup-python@ece7cb06caefa5fff74198d8649806c4678c61a1",
        "python-version: \"3.12\"",
        "export CARGO_TARGET_DIR=\"$RUNNER_TEMP/ub-review-independent-${CHECK}-target\"",
        "Run one fixed deterministic check",
    ] {
        ensure!(
            evidence.contains(required),
            "independent evidence matrix is missing `{required}`"
        );
    }

    for forbidden in [
        "receipt.json",
        "actions/upload-artifact",
        "needs.evidence.result",
        "Enforce independent baseline",
        "github-token:",
        "github.token",
        "secrets.",
        "pull-requests: write",
        "checks: write",
        "actions: write",
        "id-token: write",
        "Swatinem/rust-cache",
        "actions/cache",
        "gate_outcome.json",
        "ub-review gate-check",
    ] {
        ensure!(
            !evidence.contains(forbidden),
            "candidate evidence job must not contain `{forbidden}`"
        );
    }

    for required in [
        "'ub-review/independent-baseline'",
        "needs: [admission, evidence]",
        "if: ${{ always() }}",
        "EVIDENCE_RESULT: ${{ needs.evidence.result }}",
        "TRUSTED_WORKFLOW_SHA: ${{ github.sha }}",
        "Write trusted exact-head baseline receipt",
        "--arg workflow_sha \"$TRUSTED_WORKFLOW_SHA\"",
        "evidence_topology: \"isolated-matrix-jobs\"",
        "uses: actions/upload-artifact@043fb46d1a93c77aae656e7c1c64a875d1fc6a0a",
        "path: ${{ runner.temp }}/ub-review-independent-baseline",
        "Enforce independent baseline",
    ] {
        ensure!(
            finalize.contains(required),
            "trusted finalizer is missing `{required}`"
        );
    }

    for forbidden in [
        "actions/checkout",
        "repository: ${{ github.event.pull_request.head.repo.full_name }}",
        "git rev-parse",
        "uses: ./",
        "github-token:",
        "github.token",
        "secrets.",
        "pull-requests: write",
        "checks: write",
        "actions: write",
        "id-token: write",
    ] {
        ensure!(
            !finalize.contains(forbidden),
            "trusted finalizer must not contain `{forbidden}`"
        );
    }

    for command in [
        "cargo fmt --all -- --check",
        "cargo check --workspace --all-targets --locked",
        "cargo clippy --workspace --all-targets --locked -- -D warnings",
        "cargo test --workspace --all-targets --locked",
        "cargo doc --workspace --no-deps --locked",
        "cargo run --locked --package xtask -- policy-check",
        "python scripts/verify-bun-review-artifacts.py --self-test",
    ] {
        ensure!(
            INDEPENDENT_WORKFLOW.matches(command).count() == 2,
            "fixed command must appear once in execution and once in the trusted receipt: `{command}`"
        );
    }

    for forbidden in [
        "actions/checkout@v5",
        "dtolnay/rust-toolchain@master",
        "actions/setup-python@v6",
        "actions/upload-artifact@v7",
        "cargo xtask policy-check",
    ] {
        ensure!(
            !INDEPENDENT_WORKFLOW.contains(forbidden),
            "independent workflow must not retain mutable or unlocked reference `{forbidden}`"
        );
    }

    Ok(())
}
