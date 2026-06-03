# Policy allowlists

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
dependency surfaces, expensive CI lanes, and `ripr` suppressions.

## Ledgers

The seed policy kit lives under `policy/`:

- `clippy-lints.toml`, `clippy-debt.toml`, and `clippy-exceptions.toml` track
  active Clippy posture, planned lint upgrades, and suppression receipts.
- `no-panic-allowlist.toml` and `no-panic-baseline.toml` track panic-family
  exceptions by semantic identity, not line number.
- `non-rust-allowlist.toml` and `non-rust-debt.toml` explain which non-Rust
  surfaces may exist and which are scheduled for conversion or review.
- `generated-allowlist.toml`, `executable-allowlist.toml`,
  `dependency-surface-allowlist.toml`, `workflow-allowlist.toml`,
  `process-allowlist.toml`, and `network-allowlist.toml` govern companion
  questions that the non-Rust ledger does not answer by itself.
- `ripr-suppressions.toml` records static mutation-exposure suppressions.
- `ci-budget.toml`, `ci-lanes.toml`, and `ci-risk-packs.toml` keep PR-time proof
  cheap by policy rather than by weakening deep verification.

## Receipt quality

Good reasons are specific and auditable, for example:

- `GitHub Actions requires workflow YAML.`
- `Fixture data consumed by integration tests.`
- `Legacy release wrapper retained for workflow compatibility; scheduled for
  Rust/xtask conversion after parity is proven.`

Bad reasons are vague labels such as `misc`, `needed`, `legacy`, or
`temporary`. Those words can appear in a receipt only when the rest of the
receipt explains ownership, coverage, review timing, and expiry.
