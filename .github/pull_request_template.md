## Summary

-

## Policy receipts

If this PR adds or changes any controlled surface, link the TOML receipt here:

- non-Rust files:
- panic-family calls:
- Clippy suppressions:
- generated/executable files:
- workflow/process/network/dependency surfaces:
- expensive CI lanes or risk packs:
- `ripr` suppressions:

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
- CI cost discipline exists so we can afford more verification, not less.
