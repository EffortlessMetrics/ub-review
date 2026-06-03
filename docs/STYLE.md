# Repository style

`ub-review` is a strict, Rust-first, policy-encoded repository where CI is cheap by default but deep on demand. Every exception should have a receipt, every CI lane should have a proof obligation, and contributors should be able to infer repository rules from checked-in files rather than reviewer folklore.

## Core principles

1. **Policy is part of the codebase.** Durable policy belongs in `docs/`; machine-readable ledgers belong in `policy/`.
2. **Rust is the default implementation surface.** Non-Rust code is allowed only when justified by function, ownership, and review cadence.
3. **Panic-free is the default.** Production code and tests should avoid unreceipted panic-family behavior.
4. **Suppressions are receipts.** Use reason-bearing `#[expect(...)]` and matching TOML exceptions instead of bare `#[allow(...)]` when practical.
5. **CI is efficient, not weak.** Default PR checks should be cheap and deterministic; risky changes should route deeper proof.
6. **Evidence beats opinion.** CI budget and enforcement should move from static estimates to actuals after telemetry exists.

## Paved-road verification

The default local proof set is:

```bash
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

Additional policy checks should be added through repo-owned commands as they mature, preferably under `cargo xtask`.
