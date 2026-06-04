# GitHub runner notes

The first adoption target is the Bun UB hunt.

The Bun fork should call:

```yaml
uses: EffortlessMetrics/ub-review@0b938918eb20d38d383dba4d588b0a97bc4591f4
```

Sensor packet generation does not require secrets. Posting one grouped Pull
Request Review needs `pull-requests: write` and the scoped `github.token`;
MiniMax lanes need the configured model secret. The workflow uploads
`target/ub-review` as an artifact either way.
