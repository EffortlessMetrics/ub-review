# Agent instructions

This repository is a governed verification system, not just source plus CI.

- Prefer small, reviewable PRs over mega-PRs.
- Keep Rust-first implementation and policy checks repo-native where practical.
- Preserve panic-free production and tests; do not add test carveouts.
- Use structured TOML receipts for exceptions instead of silent suppressions.
- Treat expensive CI labels as spend authorization.
- Address bot comments and CI failures before moving to the next scoped change.
