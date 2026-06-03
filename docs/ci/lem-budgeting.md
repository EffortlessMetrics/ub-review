# LEM Budgeting

Linux-equivalent minutes (LEM) are the repository's unit for CI cost planning. A lane's LEM estimate is the expected elapsed Linux runner cost after applying cache and parallelism assumptions.

## Bands

- `tiny`: documentation, metadata, and policy-only changes.
- `default`: normal Rust implementation PRs.
- `expanded`: PRs with risk labels or broad surface changes.
- `override`: explicit budget override for release, migration, or incident response.

## Rules

1. Prefer cheaper signals before expensive ones.
2. Route expensive proof by changed surface, label, and risk pack.
3. Preserve deep validation on main, nightly, release, and explicit labels.
4. Emit `target/ci/ci-plan.json` before running routed lanes.
5. Record actual lane cost in `target/ci/ci-actuals-<lane>.json` when CI provides timing data.
