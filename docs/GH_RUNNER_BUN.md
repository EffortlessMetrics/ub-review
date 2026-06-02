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

The action is no-token and read-only by default. It writes a packet under
`target/ub-review`, appends `running-summary.md` to the job summary, and lets the
calling workflow upload the artifact.

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
  events.ndjson
  running-summary.md
```

Start with `running-summary.md`, then inspect the lane packets under `lanes/`.

## CI budget rule

The GitHub runner should prepare evidence once. It should not host long-running
LLM orchestration. LLMs and reviewers consume the packet after CI uploads it.
