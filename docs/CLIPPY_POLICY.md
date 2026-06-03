# Clippy policy

The Rust style is platform-wide: strict by default, repo-specific additions are
allowed, and repo-specific weakenings must be tracked as debt.

## Baseline

The workspace denies panic-family and placeholder lints in `Cargo.toml`. Tests
are not a panic playground: do not add test-specific Clippy carveouts for
`unwrap`, `expect`, `panic`, `dbg`, or indexing/slicing.

## Suppressions

Use `#[expect(..., reason = "...")]`, not bare `#[allow(...)]`.

`expect` is intentional because it fails when the lint stops firing. Bare
`allow` silently survives after the code changes and lets agents sand off
verification guardrails. Durable suppressions should also have a matching
receipt in `policy/allow.toml`.

## Planned upgrades

When the MSRV changes, update `Cargo.toml` and this policy instead of relying
on chat history. Planned lint entries should include the lint name, target
level, activation MSRV, and the reason the lint buys signal.
