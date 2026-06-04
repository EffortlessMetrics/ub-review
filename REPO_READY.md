# Repo-ready package

This tree is intended to be pushed to `EffortlessMetrics/ub-review`.

The Bun fork should consume it as a GitHub Action, for example:

```yaml
- uses: EffortlessMetrics/ub-review@0b938918eb20d38d383dba4d588b0a97bc4591f4
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
`0b938918eb20d38d383dba4d588b0a97bc4591f4`, validated by
`EffortlessSteven/bun#44` with a successful UB evidence packet run and uploaded
packet artifact.

Sensor packet generation is no-token by default. Review posting uses the scoped
`github.token` with pull-request write permission, and MiniMax model lanes use
the configured model secret. The calling workflow still uploads
`target/ub-review` as the durable artifact.
