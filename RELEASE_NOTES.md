# ub-review initial scaffold

Initial standalone repository package for `EffortlessMetrics/ub-review`.

Includes:

- root `action.yml` composite action
- Rust 2024 / Rust 1.95 CLI
- `bun-ub` preset
- `gh-runner`, `cx23`, `cx33`, `cx43`, `auto`, and `custom` profiles
- no-token GitHub runner path
- best-effort `tokmd`, `ripr`, `unsafe-review`, and `ast-grep` sensor setup
- Bun consumer workflow example
- no PR posting and no provider credentials

Before tagging a release, generate and commit `Cargo.lock`, then run the CI gate.
