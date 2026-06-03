# CI Inventory

This inventory maps the current workflow surface to `policy/ci-lanes.toml`.
Every workflow job should have one lane registry entry.

| Workflow | Job | Lane | Default PR | Blocking | Purpose |
| --- | --- | --- | ---: | ---: | --- |
| `.github/workflows/ci.yml` | `rust` | `rust_fast_gate` | yes | yes | Format, check, test, Clippy, and docs on Linux. |
| `.github/workflows/action-smoke.yml` | `smoke` | `action_smoke` | yes | yes | Exercise this repository as a local GitHub Action with model mode disabled and verify artifacts. |
| `.github/workflows/action-smoke.yml` | `model-smoke` | `model_smoke` | no | no | Label/manual MiniMax model smoke for provider integration and evidence quality. |
| `.github/workflows/release-binary.yml` | `build-linux-x64` | `release_pr_build` | no | no | Path-gated PR release binary and packaging dry run. |
| `.github/workflows/release-binary.yml` | `publish` | `release_publish` | no | yes on tags | Tagged release build and GitHub release asset publication. |

## Planned lanes

| Lane | Purpose |
| --- | --- |
| `ripr_advisory` | Static oracle-gap detection for production Rust diffs. |
| `workflow_policy` | Lint workflows against the lane registry and budget policy. |

## Known duplicate work

- `release_pr_build` duplicates part of `rust_fast_gate` by rebuilding the
  binary, but it adds release-mode packaging receipts.
- `model_smoke` duplicates the action invocation shape of `action_smoke`, but it
  buys provider/model integration proof and remains label/manual gated.
