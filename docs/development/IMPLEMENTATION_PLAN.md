# Implementation plan

This repository should evolve toward a governed verification system in small,
reviewable increments:

1. Keep strict Rust/Clippy rails green.
2. Add policy receipt checks in repo-native tooling.
3. Promote CI planning into an `xtask` control plane.
4. Record CI plans and actuals as artifacts.
5. Route deeper validation through labels, main, nightly, and release rails.
6. Keep `ripr` advisory signal available for mutation-exposure risk.

Avoid broad roadmap-only PRs unless implementation uncovers a missing spec that
blocks the next scoped change.
