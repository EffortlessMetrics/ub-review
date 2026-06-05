# Bun UB preset on GitHub-hosted runners

The first production use of `ub-review` is the Bun UB hunt.
Use [BUN_UB_HUNT.md](BUN_UB_HUNT.md) for the hunt invariant, proof rules, and
packet handoff.

The Bun fork should consume this repository as a GitHub Action:

```yaml
- uses: EffortlessMetrics/ub-review@804d198b5a15a0df94bb4f43750dba71165916cd
  with:
    preset: bun-ub
    profile: gh-runner
    minimax-provider-kind: anthropic
    model-timeout-sec: '300'
```

Keep the Bun workflow pinned to a verified commit SHA. The current known-good
pin is `804d198b5a15a0df94bb4f43750dba71165916cd`, validated by
`EffortlessSteven/bun#49` with a successful `UB evidence packet / gh-runner`
run, uploaded packet artifact, `tokmd` receipts, terminal state `sufficient`,
artifact-only post skip, and verifier pass. Do not float the Bun gate on `main`.

The action builds the packet without sensor secrets. In `posting: review` mode,
the Bun workflow gives it the scoped `github.token` so it can submit one grouped
Pull Request Review. MiniMax M3 lanes use `secrets.MINIMAX_API_KEY`; GLM is
skipped for v0. `secrets.OPENCODE` is reserved for optional direct provider
canary/deep modes. The action maps them into `UB_REVIEW_MINIMAX_API_KEY` and
`UB_REVIEW_OPENCODE_API_KEY`; `ub-review doctor` reports only present/missing
status for those env vars. The calling workflow still owns uploading
`target/ub-review` as the durable artifact.
`FACTORY_API_KEY` is not an action input for this preset. Keep it out of the Bun
workflow unless a later provider path adds an explicit input. Raw Factory key
assignments are verifier failures; GitHub secret placeholders are allowed.

The `core` hosted-runner tool bundle attempts `tokmd` `1.12.0`,
`cargo-allow`, `ripr`, `unsafe-review`, `ast-grep`, and `actionlint`. Missing
tools on a generic hosted runner are evidence gaps in the packet. Missing tools
on the standard ub-review image are image drift and should fail `ub-review
doctor --require-core-tools`.

## Copy-ready workflow

Use the current template:

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
    model_stages.json
    review.json
    review.md
    candidates.json
    follow_up_results.json
    follow_up_outputs.json
    resolved_candidates.json
    final_compiler_input.json
    github-review.json        # only when review content is posted
    github-review-skip.json   # when artifact-only output is correct
    post-result.json
    post-error.json
  events.ndjson
  model_stages.ndjson
  resolved_candidates.ndjson
  running-summary.md
```

Start with `running-summary.md`, then inspect `review/review.md`, the lane
packets under `lanes/`, and `review/post-result.json` or
`review/post-error.json` for the grouped review posting trail.
Use `review/resolved_candidates.json` when checking whether follow-up evidence
actually changed candidate disposition.

After changing the pin, download the Bun packet artifact and run:

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
