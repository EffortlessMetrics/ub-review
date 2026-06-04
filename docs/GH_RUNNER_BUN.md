# Bun UB preset on GitHub-hosted runners

The first production use of `ub-review` is the Bun UB hunt.

The Bun fork should consume this repository as a GitHub Action:

```yaml
- uses: EffortlessMetrics/ub-review@main
  with:
    preset: bun-ub
    profile: gh-runner
    minimax-provider-kind: anthropic
    model-timeout-sec: '300'
```

Use `@main` until the first Bun fork verification succeeds. After tagging `v0`,
pin the Bun workflow to `EffortlessMetrics/ub-review@v0`.

The action builds the packet without sensor secrets. In `posting: review` mode,
the Bun workflow gives it the scoped `github.token` so it can submit one grouped
Pull Request Review. MiniMax M3 lanes use `secrets.MINIMAX`; GLM is skipped for
v0. The calling workflow still owns uploading `target/ub-review` as the durable
artifact.

The `core` hosted-runner tool bundle attempts `tokmd`, `cargo-allow`, `ripr`,
`unsafe-review`, `ast-grep`, and `actionlint`. Missing tools on a generic
hosted runner are evidence gaps in the packet. Missing tools on the standard
ub-review image are image drift and should fail `ub-review doctor
--require-core-tools`.

## Copy-ready workflow

See:

```text
examples/bun/.github/workflows/ub-review-packet.yml
```

Recommended triggers:

```yaml
pull_request:
  types: [opened, ready_for_review]
  paths-ignore:
    - "**/*.md"
    - "docs/**"
```

Draft PRs should run the packet. Ready-for-review should run it again. Do not use
`synchronize` by default; it creates CI spam while the PR is still moving.

## Expected artifact

```text
target/ub-review/
  input/
  sensors/
  lanes/
  review/
    shared_context.md
    review.json
    review.md
    github-review.json
    post-result.json
    post-error.json
  events.ndjson
  running-summary.md
```

Start with `running-summary.md`, then inspect `review/review.md`, the lane
packets under `lanes/`, and `review/post-result.json` or
`review/post-error.json` for the grouped review posting trail.

For the first Bun fork verification, download the artifact and run:

```bash
python scripts/verify-bun-review-artifacts.py target/ub-review \
  --min-ok-model-lanes 10 \
  --require-no-model-evidence-failures
```

## CI budget rule

The GitHub runner should prepare evidence once and avoid duplicated local
discovery. Bounded direct MiniMax/OpenCode Go calls may fan out over the shared
packet, but the action should not shell out to Codex, OpenCode, Droid, or other
agent harnesses as the hot-path orchestrator.
