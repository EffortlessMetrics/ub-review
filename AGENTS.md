# Repository Agent Instructions

This repository follows a policy-driven Rust style: Rust-first implementation, panic-free production and tests, strict Clippy, semantic TOML exception receipts, LEM-aware CI, and small agent-safe PRs.

- Use separate PRs unless a stacked sequence is explicitly requested.
- Do not make mega-PRs or silent policy changes.
- Prefer Rust and `xtask` for automation and policy checks where practical.
- Do not introduce `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `dbg!`, unchecked indexing, or silent `#[allow(...)]` suppressions without a policy receipt.
- Suppress Clippy with `#[expect(..., reason = "policy:<id>")]`, not bare `#[allow(...)]`.
- Run relevant local checks before finishing and document any environment limitation.
