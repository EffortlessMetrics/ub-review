# GitHub runner notes

The first adoption target is the Bun UB hunt.

The Bun fork should call:

```yaml
uses: EffortlessMetrics/ub-review@main
```

No secrets are required. The workflow needs only read permissions and uploads `target/ub-review` as an artifact.
