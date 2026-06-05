# Policy exception ledger

`ub-review` uses receipt-backed policy ledgers instead of anonymous exceptions.
The platform rule is:

```text
Global rule by default.
Local exception by structured receipt.
```

A receipt should explain where the exception is, who owns it, why it exists,
what covers it, when it should be reviewed, and when it expires if temporary.
This applies to panic-family calls, Clippy suppressions, non-Rust files,
generated files, executable files, workflows, process spawning, network access,
dependency surfaces, expensive CI lanes, `ripr` suppressions, and
`unsafe-review` suppressions.

## Ledgers

The seed policy kit lives under `policy/`:

- `allow.toml` is the default source-tree exception ledger. It is the place for
  syntax-visible exceptions such as panic-family calls, lint suppressions,
  non-Rust files, generated files, executable files, scripts, workflows,
  process spawning, network access, and dependency surfaces.
- `ci-budget.toml`, `ci-lanes.toml`, and `ci-risk-packs.toml` keep PR-time proof
  cheap by policy rather than by weakening deep verification.

Do not add separate per-surface ledgers by default. Split `allow.toml` only when
the repo has enough real exception volume that a narrower ledger improves
reviewability.

`policy/clippy-lints.toml` may record active and targeted lint policy because it
is not an exception ledger. `policy/clippy-debt.toml`,
`policy/ripr-suppressions.toml`, and
`policy/unsafe-review-suppressions.toml` should be created only when there is
real retained debt or a reviewed suppression to own.

## Receipt quality

Good reasons are specific and auditable, for example:

- `GitHub Actions requires workflow YAML.`
- `Fixture data consumed by integration tests.`
- `Legacy release wrapper retained for workflow compatibility; scheduled for
  Rust/xtask conversion after parity is proven.`

Bad reasons are vague labels such as `misc`, `needed`, `legacy`, or
`temporary`. Those words can appear in a receipt only when the rest of the
receipt explains ownership, coverage, review timing, and expiry.
