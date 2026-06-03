# No-panic policy

`ub-review` follows a panic-free policy for production and tests. Tests are not
a panic playground: setup, fixture plumbing, parsing, IO, indexing, and helper
code should be fallible and explicit.

## Rule

Panic-family calls are denied by default:

- `unwrap`;
- `expect`;
- `panic!`;
- `todo!`;
- `unimplemented!`;
- unchecked indexing or slicing when a fallible alternative is practical.

## Receipts

Temporary exceptions must be represented by semantic TOML receipts in
`policy/no-panic-allowlist.toml`. Receipts identify the exception by path,
family, and selector. Line and column are only advisory `last_seen` hints so
normal refactors do not create allowlist churn.

Each receipt needs an owner, classification, explanation, and expiry date.
