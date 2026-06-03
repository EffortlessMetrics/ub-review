# `ripr` Policy

`ripr` gives mutation-testing-lite value at static-analysis prices.

It does not run mutants or report killed/survived outcomes. It statically asks whether the behavior changed in this diff appears exposed to a meaningful test discriminator.

## Role in CI

- Advisory on ordinary production Rust diffs unless a release policy promotes it.
- Blocking only when the repository explicitly requires static mutation-exposure proof for a risk pack.
- Suppressions must be represented in `policy/ripr-suppressions.toml` with owner, reason, classification, and expiry.

## Relationship to mutation testing

`ripr` does not replace runtime mutation testing. It shifts mutation-shaped feedback left into PR-time static analysis, while runtime mutation testing remains a deeper, more expensive lane for scheduled or label-triggered validation.
