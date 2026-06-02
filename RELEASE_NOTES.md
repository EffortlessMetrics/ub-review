# ub-review v0 readiness notes

`ub-review` has moved past the initial artifact-only scaffold. The current
`main` branch is the pre-`v0` Bun UB review-direct line.

Current supported shape:

- root `action.yml` composite action
- Rust 2024 / Rust 1.95 CLI
- `bun-ub` preset
- `gh-runner`, `cx23`, `cx33`, `cx43`, `auto`, and `custom` profiles
- best-effort `tokmd`, `ripr`, `unsafe-review`, and `ast-grep` sensor setup
- direct MiniMax M3 review lanes with GLM skipped for v0
- optional OpenCode Go direct provider canary lane
- grouped Pull Request Review posting with bounded inline comments
- full packet artifacts, including `review/post-result.json` or `review/post-error.json`
- Bun consumer workflow example using `EffortlessMetrics/ub-review@main`

Before tagging `v0`, prove the same commit on:

1. the locked CI gate;
2. the local action smoke workflow;
3. a live MiniMax model smoke run with repository secrets;
4. a real Bun fork draft PR that posts one grouped review and uploads a complete packet.

After those checks pass, tag `v0` and switch the Bun workflow from `@main` to
`@v0`.
