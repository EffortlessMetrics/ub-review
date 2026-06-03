# CI and policy rollout plan

Roll policy enforcement in small PRs:

1. Document the style contract.
2. Inventory existing lint, panic, non-Rust, and CI surfaces.
3. Add static budget and lane whitelist policy.
4. Add advisory checkers.
5. Add PR planning artifacts.
6. Clean up cache and cancellation behavior.
7. Add a merge-gate aggregator.
8. Document branch-protection migration.
9. Tighten Rust, no-panic, file-policy, ripr, and CI-actuals enforcement after calibration.

Advisory visibility should come before blocking enforcement.
