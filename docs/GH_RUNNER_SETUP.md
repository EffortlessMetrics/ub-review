# GitHub runner setup

`ub-review` is intended to be used as a normal GitHub Action from consuming
repositories.

```yaml
- uses: EffortlessMetrics/ub-review@v0
  with:
    preset: bun-ub
    profile: gh-runner
```

For Bun review posting, the consuming workflow should set:

```yaml
permissions:
  contents: read
  pull-requests: write
```

Sensor packet generation does not require secrets. Posting uses the scoped
`github.token`. Default model review uses direct MiniMax M3 through
`secrets.MINIMAX`. OpenCode Go is optional and used only as a direct provider
lane through `secrets.OPENCODE`; the action does not shell out to OpenCode as an
agent harness. GLM is skipped for v0. Missing model keys are recorded as missing
review evidence.

The consuming workflow is responsible for:

1. checking out the target repository;
2. fetching the base ref if needed;
3. calling `EffortlessMetrics/ub-review@<ref>`;
4. allowing the action to submit one grouped Pull Request Review when configured;
5. uploading `target/ub-review` as an artifact.

Until release binaries are published, the action bootstraps from source on the
runner. Cache Cargo registry/git and `CARGO_TARGET_DIR` in consuming workflows if
runtime matters.

## Self-smoke model verification

The repo-local `Action Smoke` workflow always runs the token-free local action
smoke. To verify live MiniMax provider behavior in `EffortlessMetrics/ub-review`
without labeling a PR, run the workflow manually on a trusted ref with
`run_model_smoke: true`.
That job uses the repository secret `MINIMAX_API_KEY`, caps the run to one
model call, uploads `target/ub-review-model-smoke`, and keeps posting errors
tolerated so artifacts remain available for inspection.
