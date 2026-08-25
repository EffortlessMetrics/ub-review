use anyhow::{Context, Result, ensure};

const WORKFLOW: &str = include_str!("../.github/workflows/ub-review-gate.yml");
const INDEPENDENT_WORKFLOW: &str =
    include_str!("../.github/workflows/independent-baseline.yml");

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
fn independent_baseline_uses_base_owned_orchestration_and_exact_candidate_checkout() -> Result<()> {
    ensure!(
        INDEPENDENT_WORKFLOW.starts_with("name: ub-review/independent-baseline\n"),
        "independent check must expose one stable check name"
    );
    ensure!(
        INDEPENDENT_WORKFLOW.contains("\n  pull_request_target:\n"),
        "workflow definition must come from the protected base branch"
    );
    for required in [
        "permissions:\n  contents: read",
        "repository: ${{ github.event.pull_request.head.repo.full_name }}",
        "ref: ${{ github.event.pull_request.head.sha }}",
        "persist-credentials: false",
        "actual_sha=\"$(git rev-parse HEAD)\"",
        "workflow_source: \"protected-base\"",
        "Enforce independent baseline",
        "actions/upload-artifact@v7",
    ] {
        ensure!(
            INDEPENDENT_WORKFLOW.contains(required),
            "independent baseline is missing `{required}`"
        );
    }

    for command in [
        "cargo fmt --all -- --check",
        "cargo check --workspace --all-targets --locked",
        "cargo clippy --workspace --all-targets --locked -- -D warnings",
        "cargo test --workspace --all-targets --locked",
        "cargo doc --workspace --no-deps --locked",
        "cargo xtask policy-check",
        "python scripts/verify-bun-review-artifacts.py --self-test",
    ] {
        ensure!(
            INDEPENDENT_WORKFLOW.matches(command).count() == 2,
            "trusted command must appear once for execution and once in the receipt: `{command}`"
        );
    }

    for forbidden in [
        "uses: ./",
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
            !INDEPENDENT_WORKFLOW.contains(forbidden),
            "independent baseline must not contain `{forbidden}`"
        );
    }
    Ok(())
}
