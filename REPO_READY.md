# Repo-ready package

This tree is intended to be pushed to `EffortlessMetrics/ub-review`.

The Bun fork should consume it as a GitHub Action, for example:

```yaml
- uses: EffortlessMetrics/ub-review@217f123e688e42ddfce98eec5795b88bf457dd34
  with:
    preset: bun-ub
    profile: gh-runner
    base: origin/${{ github.base_ref }}
    head: HEAD
    out: target/ub-review
    minimax-provider-kind: anthropic
    model-timeout-sec: '300'
```

Use a verified commit SHA for the Bun gate. The current known-good pin is
`217f123e688e42ddfce98eec5795b88bf457dd34`, validated by
`EffortlessSteven/bun#45` with a successful UB evidence packet run, uploaded
packet artifact, `tokmd` receipts, and zero inline comments.

Sensor packet generation is no-token by default. Review posting uses the scoped
`github.token` with pull-request write permission, and MiniMax model lanes use
the configured model secret. The calling workflow still uploads
`target/ub-review` as the durable artifact.
