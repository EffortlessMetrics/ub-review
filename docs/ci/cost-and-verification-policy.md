# CI cost and verification policy

CI cost is architecture. At high PR volume, runner minutes, disk, local analyzer
fanout, mutation time, and reviewer attention can cost more than model tokens.
The target is not shallow CI; it is deeper verification that is cheaper by
default because proof is scoped by risk.

## Operating goals

- Run useful default checks on normal PRs.
- Keep expensive proof targeted to high-risk surfaces, labels, release lanes, or
  scheduled lanes.
- Emit receipts when checks run, skip, downgrade, or route to a different lane.
- Keep branch-protection signals aggregated and understandable.
- Separate evidence claims so that coverage, tests, policy, `ripr`, and mutation
  do not overstate what they prove.

## Budget vocabulary

Use Linux-equivalent minutes (LEM) for cross-runner budgeting. A future CI
policy ledger should encode runner multipliers and budget bands such as:

```toml
preferred_default_lem = 25
default_limit_lem = 35
elevated_limit_lem = 75
hard_limit_lem = 125
```

## Lane shape

Default PR lanes should favor:

```text
fmt
check
clippy
focused tests
policy checks
ripr static mutation-exposure analysis
```

Risk PR lanes may add targeted mutation for high-risk surfaces. Nightly lanes
may run broader mutation matrices. Release lanes should collect readiness proof
that is clean enough to ship.

## Claim boundaries

Codecov and other coverage tools say which lines or branches executed. They do
not prove correctness, mutation adequacy, or release readiness.

Coverage receipts should carry a claim boundary like:

```json
{
  "claim_boundary": [
    "execution_surface_only",
    "not_correctness_proof",
    "not_mutation_adequacy",
    "not_release_readiness"
  ]
}
```

`ripr` and runtime mutation belong in the same evidence family, but not the same
cost tier: `ripr` shifts mutation signal left, while mutation remains the slower
runtime backstop.
