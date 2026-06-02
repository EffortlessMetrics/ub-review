# Repo-ready package

This tree is intended to be pushed to `EffortlessMetrics/ub-review`.

The Bun fork should consume it as a GitHub Action, for example:

```yaml
- uses: EffortlessMetrics/ub-review@main
  with:
    preset: bun-ub
    profile: gh-runner
    base: origin/${{ github.base_ref }}
    head: HEAD
    out: target/ub-review
    minimax-provider-kind: anthropic
    model-timeout-sec: '300'
```

Use `@main` for the first Bun fork verification. After that run posts a useful
review and uploads the expected packet, tag `v0` and switch Bun to
`EffortlessMetrics/ub-review@v0`.

The action is advisory and no-token by default: it needs repository read access,
prepares `target/ub-review`, appends the running summary to the job summary, and
leaves artifact upload to the calling workflow.
