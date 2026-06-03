# Clippy policy

Clippy is a repository policy mechanism, not only a suggestion engine. The baseline posture is strict in both production code and tests.

## Defaults

The repository denies common panic and debugging lints in `Cargo.toml`. Future strictness should be added incrementally and only after either remediating current debt or recording reviewed temporary exceptions.

Target posture:

- Deny `dbg_macro`, `todo`, `unimplemented`, `panic`, `unreachable`, `unwrap_used`, and `expect_used`.
- Forbid unsafe Rust unless a first-class unsafe exception regime is adopted.
- Avoid test carveouts for unwrap, expect, panic, indexing, or slicing.
- Prefer `#[expect(..., reason = "policy:<id>: ...")]` over `#[allow(...)]`.

## Exception receipts

A lint suppression should have a matching entry in `policy/clippy-exceptions.toml` with:

- `id`
- `path`
- `lint`
- `owner`
- `classification`
- `reason`
- `selector`
- `review_after`
- `expires`

Bare suppressions should be treated as migration debt and replaced as the checker matures.
