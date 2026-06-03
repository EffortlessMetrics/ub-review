# Clippy Policy

Strict Clippy is the repository road surface. The baseline targets panic-free production and tests, AST/parser/string/indexing safety, silent-failure prevention, suppression governance, useful readability lints, and planned MSRV-aligned lint flips.

## Workspace policy

- Tests are not a panic playground.
- Do not add Clippy test carveouts such as `allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-panic-in-tests`, `allow-indexing-slicing-in-tests`, or `allow-dbg-in-tests`.
- Prefer fallible test setup, fixture plumbing, parsing, IO, indexing, and helpers.
- Blanket category suppressions are not allowed.

## Suppressions

Use semantic `expect` receipts:

```rust
#[expect(clippy::some_lint, reason = "policy:clippy-0001")]
```

Do not use silent allows:

```rust
#[allow(clippy::some_lint)]
```

Exception receipts live in `policy/clippy-exceptions.toml`; debt and planned flips live in `policy/clippy-debt.toml`.
