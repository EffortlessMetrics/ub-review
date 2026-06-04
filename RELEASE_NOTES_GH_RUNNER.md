# GitHub runner notes

The first adoption target is the Bun UB hunt.

The Bun fork should call:

```yaml
uses: EffortlessMetrics/ub-review@da14100f862610477e27948719bf5f0d222d27e6
```

Sensor packet generation does not require secrets. Posting one grouped Pull
Request Review needs `pull-requests: write` and the scoped `github.token`;
MiniMax lanes need the configured model secret. The workflow uploads
`target/ub-review` as an artifact either way.
