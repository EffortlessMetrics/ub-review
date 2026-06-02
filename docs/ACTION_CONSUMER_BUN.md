# Bun consumer workflow

The Bun fork should consume `ub-review` as a normal GitHub Action, not vendor the Rust runner.

```yaml
- name: Build UB review packet
  uses: EffortlessMetrics/ub-review@main
  with:
    preset: bun-ub
    profile: gh-runner
    root: .
    base: origin/${{ github.base_ref }}
    head: HEAD
    out: target/ub-review
    install-tools: 'true'
    tool-bundle: core
    posting: review
    mode: review-direct
    github-token: ${{ github.token }}
    minimax-api-key: ${{ secrets.MINIMAX }}
    minimax-provider-kind: anthropic
    opencode-api-key: ${{ secrets.OPENCODE }}
    model-mode: auto
    provider-policy: minimax-primary
    lane-width: '10'
    model-timeout-sec: '300'
    max-inline-comments: '8'
    model-concurrency: '8'
    max-model-calls: '14'
    fail-on-post-error: 'false'
    allow-heavy: 'false'
```

GLM is skipped for v0. OpenCode Go is optional and used only as a direct
provider canary lane; `ub-review` does not invoke the OpenCode agent harness.
Use `provider-policy: minimax-only` to force all model lanes through direct
MiniMax M3.

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
