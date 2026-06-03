## Summary

-

## Policy receipts

If this PR adds or changes any controlled surface, link the TOML receipt here:

- source-tree exceptions in `policy/allow.toml`:
- expensive CI lanes or risk packs:
- `ripr` or `unsafe-review` suppressions, only if needed:

## Verification

Cheapest relevant proof first, deeper proof where it buys signal.

- [ ] Rust fast gate
- [ ] policy ledger parse/check, if available
- [ ] selected risk-pack checks, if any
- [ ] deep lanes only when label/reason requires them

## CI budget

- Planned band: pennies / default / elevated / high / over ceiling
- Reason for elevated or higher band:

## Agent notes

- Prefer small SRP PRs.
- Do not use bare `#[allow]`; use `#[expect(..., reason = "...")]` with a receipt.
- `ripr` is static mutation-exposure analysis; runtime mutation remains the slower backstop.
- `unsafe-review` owns unsafe/native reviewability; do not duplicate it in repo scripts.
- `xtask` should orchestrate repo-local receipts and checks, not replace specialized tools.
- CI cost discipline exists so we can afford more verification, not less.
