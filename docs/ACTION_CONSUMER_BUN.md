# Bun consumer workflow

The Bun fork should consume `ub-review` as a normal GitHub Action, not vendor the Rust runner.

```yaml
- name: Build UB review packet
  uses: EffortlessMetrics/ub-review@da14100f862610477e27948719bf5f0d222d27e6
  with:
    preset: bun-ub
    profile: gh-runner
    runtime-profile: gh-runner
    root: .
    base: origin/${{ github.base_ref }}
    head: HEAD
    out: target/ub-review
    install-tools: 'true'
    tool-bundle: core
    posting: review
    mode: review-byok
    github-token: ${{ github.token }}
    minimax-api-key: ${{ secrets.MINIMAX }}
    minimax-provider-kind: anthropic
    model-mode: auto
    depth: standard
    provider-policy: minimax-only
    lane-width: '10'
    model-timeout-sec: '300'
    max-inline-comments: '8'
    model-concurrency: '8'
    max-model-calls: '14'
    fail-on-post-error: 'false'
    allow-heavy: 'false'
```

The Bun fork should use a full commit SHA, not `main`. The current known-good
pin is `da14100f862610477e27948719bf5f0d222d27e6`; update it only after this
repo's verifier passes and the Bun consumer workflow uploads a valid packet.

GLM is skipped for v0. The Bun v0 cutover workflow uses direct MiniMax M3 for
all model lanes. OpenCode Go remains optional for later direct provider
canary/deep modes; `ub-review` does not invoke the OpenCode agent harness.
Use `depth: quick`, `standard`, or `deep` for preset lane/model-call pressure;
keep raw lane/model budget overrides on `standard`.
For focused reruns, `lanes`, `except-lanes`, `tools`, and `except-tools` accept
comma-separated IDs and are recorded in `resolved-plan.json`.
Use `runtime-profile` for box budgets and local work limits; `profile` is kept
as the backward-compatible action input.

The default review policy keeps PR bodies and inline comments free of lane
rosters, successful provider/model status tables, execution summaries, and
generic no-finding boilerplate. Those surfaces stay in artifacts;
`github-review.json` is written only when reviewer-value content survives
compilation.
See [REVIEW_BODY_CONTRACT.md](REVIEW_BODY_CONTRACT.md) for the exact decision
memo style.

## Permissions

The workflow uses the built-in token with pull-request write scope to create one
grouped Pull Request Review.

```yaml
permissions:
  contents: read
  pull-requests: write
```

The workflow uploads artifacts from the consumer repository:

```yaml
- uses: actions/upload-artifact@v7
  if: always()
  with:
    name: ub-review-packet-${{ github.event.pull_request.number }}
    path: target/ub-review
    retention-days: 7
    if-no-files-found: warn
```

The action also exposes the core packet and posting artifact paths as outputs.
For posting diagnostics, start with `post-result-path` when the grouped review
posted and `post-error-path` when posting was skipped or failed. The full packet
still includes `review/github-review-post-payload.json`, `review/post-stdout.json`,
and `review/post-stderr.txt` for the exact request/response trail.

With `install-tools: 'true'` and `tool-bundle: core`, the hosted-runner setup
attempts `tokmd`, `cargo-allow`, `ripr`, `unsafe-review`, `ast-grep`, and
`actionlint`. On a generic hosted runner an install miss is recorded as missing
evidence; on the standard image, `ub-review doctor --require-core-tools` should
fail the image before a Bun packet starts.

## Trigger policy

For the Bun UB hunt:

```yaml
on:
  pull_request:
    types: [opened, ready_for_review]
    paths-ignore:
      - "**/*.md"
      - "docs/**"
```

Draft PR opened gets the first packet. `ready_for_review` gets the second packet. No `synchronize` spam.
