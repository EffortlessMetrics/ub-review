# Policy Allowlists

Strict rules are deny-by-default and allow-by-receipt.

Allowlists are TOML control-plane files under `policy/`. Receipts should include stable identifiers, owners, classifications, rationale, coverage, expiry where applicable, and semantic selectors instead of fragile line-only locations.

Current policy surfaces include Clippy exceptions, Clippy debt, panic-family exceptions, non-Rust surfaces, generated files, process use, network use, unsafe islands, CI lanes, CI budgets, CI risk packs, and `ripr` suppressions.
