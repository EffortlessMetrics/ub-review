# LEM budgeting

LEM makes CI cost reviewable.

```text
LEM = wall-clock job minutes × runner multiplier
```

Static estimates should use `policy/ci-budget.toml`. A future actuals collector can replace guesses with recent p50/p90/p95 measurements, but learned estimates should start advisory before becoming hard gates.

Budget intent:

- Preferred default PRs stay cheap.
- Elevated PRs are allowed when risk packs or labels justify them.
- Hard limits require explicit owner review.
