# Clippy policy

This repository treats strict Clippy as part of the review surface, not as a
style preference. The default posture is panic-free production code and tests,
explicit suppression receipts, and shared lint expectations that can be audited
across Rust repositories.

## Baseline

The workspace denies panic-family and debugging lints in `Cargo.toml` and keeps
additional governance in `policy/clippy-lints.toml`:

- no `unwrap`, `expect`, `panic`, `todo`, `unimplemented`, or `dbg!` in
  production code or tests;
- no test carveouts for fixture setup, parsing, IO, indexing, or helpers;
- no blanket `allow` categories;
- suppressions must be narrow and must point at a policy receipt.

## Suppression format

Use `#[expect(..., reason = "policy:<id>")]` for a local, reviewable exception:

```rust
#[expect(clippy::some_lint, reason = "policy:clippy-0001")]
fn intentionally_exceptional() {}
```

Do not use silent allowances:

```rust
#[allow(clippy::some_lint)]
fn silently_exceptional() {}
```

Exception metadata belongs in `policy/clippy-exceptions.toml`; planned cleanup
belongs in `policy/clippy-debt.toml`.
