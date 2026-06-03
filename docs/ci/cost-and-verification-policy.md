# CI cost and verification policy

CI should be efficient, not weak. The repository should run the right proof at the right time: cheap ordinary PR checks, explicit risk expansion, and deep backstops on labels, main, nightly, release, or manual dispatch.

## LEM

The standard CI cost unit is LEM:

```text
LEM = wall-clock job minutes × runner multiplier
```

Use `policy/ci-budget.toml` for static budget bands and runner multipliers. Hard learned-budget enforcement should wait until CI actuals are collected and reviewed.

## Lane proof obligations

Every lane should answer:

- What does this prove?
- What failure mode does it catch?
- Who owns it?
- How much does it cost?
- When should it run?
- Is it blocking or advisory?
- What duplicates it?
- When should it be reviewed or expired?

Record those answers in `policy/ci-lane-whitelist.toml`.

## Merge gate

Branch protection should require a single aggregate `ci/merge-gate` status. Optional platform, coverage, model, mutation, Docker, or advisory checks should feed the aggregate gate only when selected by policy.
