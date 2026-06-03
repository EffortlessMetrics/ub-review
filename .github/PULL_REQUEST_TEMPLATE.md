## Production delta

## Evidence/support delta

## Acceptance criteria

## Review map

| File | Change | Why |
|---|---|---|

## Policy checks

- [ ] No new panic-family calls unless narrowly justified with an explanation.
- [ ] No new non-Rust repository machinery unless Rust is not the right surface.
- [ ] No new generated, dependency, process, network, workflow-shell, executable, or local-context surfaces without a receipt.
- [ ] No broad lint suppressions or vague allowlist-style reasons.
- [ ] CI-relevant docs and checks were updated.

## Commands run

```bash
cargo generate-lockfile
cargo fmt --all --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
```

## Non-goals
