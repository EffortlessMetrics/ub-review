# GitHub runner notes

The first adoption target is the Bun UB hunt.

The Bun fork should call:

```yaml
uses: EffortlessMetrics/ub-review@7b969e53b58d7b2a32db9006f1f2f43916fc2134
```

Sensor packet generation does not require secrets. Posting one grouped Pull
Request Review needs `pull-requests: write` and the scoped `github.token`;
MiniMax lanes need the configured model secret. The workflow uploads
`target/ub-review` as an artifact either way.
