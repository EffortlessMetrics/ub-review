# Contributing

`ub-review` uses a policy-first Rust repository style. The goal is to make the correct behavior the paved road and make exceptions explicit, owned, reviewed, and temporary.

## Pull request expectations

Every non-trivial PR should explain:

- Purpose and user impact.
- CI / LEM impact.
- Workflows touched, if any.
- Branch-protection impact, if any.
- Failure mode caught or proof obligation improved.
- Cheaper signal considered for expensive checks.
- Rollback path.
- Commands run.

## Rust expectations

- Prefer fallible APIs and `Result` returns over panic-family operations.
- Do not treat tests as a panic dumping ground; use fallible fixture setup in tests.
- Do not add bare lint suppressions. If suppression is unavoidable, add a policy receipt and a reason-bearing `#[expect(...)]` where possible.
- Keep implementation Rust-first. New non-Rust surfaces need an entry in `policy/non-rust-allowlist.toml`.

## CI expectations

- Do not add unregistered CI jobs. Add or update `policy/ci-lane-whitelist.toml` with the lane owner, proof obligation, trigger policy, and LEM estimate.
- Do not make expensive checks default for ordinary PRs without a proof obligation and budget justification.
- Keep optional/deep jobs advisory or risk-routed until calibrated.
