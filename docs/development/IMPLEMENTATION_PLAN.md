# Implementation Plan

This repository should ratchet toward the governed Rust baseline in scoped PRs:

1. Land policy docs and TOML control-plane files.
2. Add `xtask` commands for lint, panic-family, file-policy, CI planning, and policy-report checks.
3. Wire cheap default CI lanes to the `xtask` checks.
4. Add `ripr` advisory artifacts and suppressions.
5. Add learned CI actuals and LEM-aware lane routing.
6. Promote scheduled, main, release, and label-triggered deep validation.

Do not bundle all enforcement into one mega-PR. Each step should be reviewable, reversible, and backed by acceptance checks.
