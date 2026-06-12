# GitHub runner setup

`ub-review` is intended to be used as a normal GitHub Action from consuming
repositories.

```yaml
- uses: EffortlessMetrics/ub-review@804d198b5a15a0df94bb4f43750dba71165916cd
  with:
    preset: bun-ub
    profile: gh-runner
    minimax-provider-kind: anthropic
    model-timeout-sec: '300'
```

Use a full commit SHA for the Bun gate. The current known-good pin is
`804d198b5a15a0df94bb4f43750dba71165916cd`, validated by
`EffortlessSteven/bun#49` with terminal state `sufficient` and an artifact-only
post skip. Tagged action refs use `install-mode: auto` to try the Linux x64
release asset `ub-review-x86_64-unknown-linux-gnu.tar.gz` first, download and
verify its `.sha256` receipt, then fall back to a source build when the asset or
receipt is not available. Pushing a `v*` tag in this repository builds and
publishes that archive plus its `.sha256` receipt.

For Bun review posting, the consuming workflow should set:

```yaml
permissions:
  contents: read
  pull-requests: write
```

Sensor packet generation does not require secrets. Posting uses the scoped
`github.token`. Default model review uses direct MiniMax M3 through
`secrets.MINIMAX_API_KEY`. The Bun v0 workflow uses
`provider-policy: minimax-only`. OpenCode Go remains optional for later direct
provider canary/deep modes through `secrets.OPENCODE`; the action does not shell
out to OpenCode as an agent harness. GLM is skipped for v0. Missing model keys
are recorded as missing review evidence.
`FACTORY_API_KEY` can remain an organization secret, but the current action does
not consume it. Do not pass it through the Bun v0 workflow; the verifier guards
the name so raw values fail artifact checks.

The consuming workflow is responsible for:

1. checking out the target repository;
2. fetching the base ref if needed;
3. calling `EffortlessMetrics/ub-review@<ref>`;
4. allowing the action to submit one grouped Pull Request Review when configured;
5. uploading `target/ub-review` as an artifact.

To force a path during rollout, set `install-mode: source` for source bootstrap,
`install-mode: release` for release-download-first fallback, or
`install-mode: path` with `binary-path` for a preinstalled executable. Until
release binaries are published, the fallback source build remains the expected
runner path. Cache Cargo registry/git and `CARGO_TARGET_DIR` in consuming
workflows if runtime matters.

## Self-smoke model verification

The repo-local `Action Smoke` workflow always runs the token-free local action
smoke. To verify live MiniMax provider behavior in `EffortlessMetrics/ub-review`
without labeling a PR, run the workflow manually on a trusted ref with
`run_model_smoke: true`.
That job uses the repository secret `MINIMAX_API_KEY`, defaults manual dispatch
to the standard 10-lane MiniMax pass, verifies the MiniMax preflight and expected
successful lane count, uploads `target/ub-review-model-smoke`, and keeps posting
errors tolerated so artifacts remain available for inspection.

Adding the `ub-review-model-smoke` label to a PR still runs a cheap one-lane
provider smoke by default. Without that label, the PR job is intentionally
skipped even when `MINIMAX_API_KEY` is configured. Manual dispatch exposes
`model_smoke_lane_width`, `model_smoke_max_model_calls`,
`model_smoke_model_concurrency`, and `model_smoke_expected_ok_lanes` when a
wider or deeper live check is needed.
