# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

`ub-review` is a Rust CLI (and GitHub Action, see `action.yml`) that builds deterministic evidence packets for UB-focused PR review: it plans evidence from a PR diff, runs cheap static sensors (`tokmd`, `cargo-allow`, `ripr`, `unsafe-review`, `ast-grep`, `actionlint`), fans out bounded BYOK model lanes (MiniMax M3 by default), validates inline comment candidates, and submits one grouped GitHub PR review. First production preset: `bun-ub` (the Bun UB hunt).

## Commands

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets --locked
cargo test --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo doc --workspace --no-deps --locked
cargo xtask policy-check          # validate policy/allow.toml receipts (runs in CI)
cargo xtask precommit             # fmt/check/clippy-on-diff + sensor receipts into an out dir
```

- Full pre-PR gate: `scripts/check-pr.sh` (or the `justfile` recipes: `just fmt`, `just lint`, `just test`, `just policy`).
- Single test: `cargo test --workspace --locked <test_name>`; integration tests live in `tests/cli.rs` (`cargo test --test cli <name>`), unit tests in the `mod tests` at the bottom of `src/main.rs`.
- Local smoke run of the binary: `scripts/smoke-local.sh` (doctor → init → plan → dry-run).
- Always use `--locked`; CI pins Rust 1.95, edition 2024 (`rust-toolchain.toml`).

## Hard lint policy (build fails otherwise)

`unsafe_code = forbid`. Clippy denies `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro` in both crates — write fallible code paths (`anyhow::Result`, `Context`) instead. Never use bare `#[allow]`; use `#[expect(..., reason = "...")]` and, for durable suppressions, add an owned receipt to `policy/allow.toml` (owner, reason, created/review_after/expiry). The same ledger governs non-Rust files, scripts, workflows, and generated files — do not add controlled surfaces anonymously. See `AGENTS.md`, `docs/NO_PANIC_POLICY.md`, `docs/FILE_POLICY.md`.

## Code layout

Two-member workspace:

- **`src/main.rs`** — currently the entire `ub-review` binary in one ~31k-line file. This is tech debt, not a design goal: modularizing it into submodules is wanted. When touching it, prefer extracting coherent seams into `src/` submodules over growing the monolith. Landmarks until then: subcommands `init`, `doctor`, `cache`, `plan`, `run`, `summary`, `post` dispatch from `main()` to `cmd_*` functions (~line 3300+); inline unit tests start at `mod tests` near line 20800.
- **`xtask/`** — repo-local policy orchestration (`policy-check`, `policy-inventory`, `precommit`). It aggregates external tools (`cargo-allow`, `ripr`, `unsafe-review`); do not reimplement them inside it.

Configuration data, not code, drives behavior:

- `profiles/` — review profiles (e.g. `bun-ub-v0.toml` defines the six lanes: ub, source-route, tests, arch, opposition, security).
- `runtime/` — box budget profiles (`gh-runner`, `cx23`, `cx33`, `cx43`), separate from review profiles.
- `configs/` — example/consumer repo configs (`.ub-review.toml` shape).
- `policy/` — exception ledger (`allow.toml`) plus CI budget/lane/risk-pack policy.
- `scripts/verify-bun-review-artifacts.py` — packet-contract verifier for downloaded Bun run artifacts (has `--self-test`, run in CI).

## Architecture invariants

The pipeline is: diff → evidence plan → sensors → lane packets → model lanes → proof receipts → validated inline candidates → **one grouped PR review**. Key rules from `docs/ARCHITECTURE.md`:

- Mutation zones: source checkout is immutable; sensor artifacts immutable once emitted; `events.ndjson` is append-only; `running-summary.md` is single-writer.
- `run` only writes artifacts; `post` is the separate command that submits `review/github-review.json` as one PR review. Never post per-lane comments, issue comments, or status chatter.
- Missing sensors/model keys are recorded as **missing evidence, never as clean evidence**.
- Lane identity and model identity are separate; packet prefixes use lane names only.
- Heavy witnesses (builds, tests, Miri, ASAN, mutation) are off by default and gated behind explicit workflow policy (`allow-heavy`).
- Sensor defects belong upstream (`ripr-swarm`, `unsafe-review-swarm`, `tokmd-swarm`), not silently absorbed into local glue; local workarounds must link the upstream issue.

## Working style

From `AGENTS.md`: one coherent proof obligation per PR ("review-fast"), with exact validation steps and known gaps stated. Run the cheapest relevant proof first. Do not broaden scope to satisfy CI. The Bun consumer workflow pins this action by full commit SHA — update the pin only after the verifier passes and the Bun consumer workflow succeeds (see README "Copy/paste Bun setup").
