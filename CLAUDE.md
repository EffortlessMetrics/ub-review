# Claude Operating Notes

Follow `AGENTS.md`. The short contract is:

- One PR, one objective.
- Inspect the current repo before starting; if the target already exists, make an
  audit/repair/doc-sync PR instead of duplicating work.
- Rust/xtask by default; non-Rust by owned receipt.
- Docs/spec rails before code, then stop documenting and build.
- Deep verification, cheap by default, risk-routed when expensive.
- `ripr` is static mutation-exposure analysis that shifts mutation signal left;
  runtime mutation remains the slower backstop.
- Release readiness proof comes before version bumps or tags.
