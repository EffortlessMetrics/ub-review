# Agent instructions

This repository is policy-first, Rust-first, panic-averse, CI-cost-aware, and evidence-driven.

## Workflow

- Inspect the current repository state before changing files.
- Keep changes small and reviewable; avoid mega-PR rollouts.
- Prefer Rust code, Cargo workflows, TOML policy files, GitHub Actions YAML, and Markdown docs.
- Put durable policy in `docs/` and machine-readable ledgers in `policy/` instead of relying on tribal knowledge.
- If a target policy or checker already exists, improve it rather than duplicating it.
- Do not add random shell, Python, TypeScript, generated, or other non-Rust surfaces unless they are explicitly justified and covered by `policy/non-rust-allowlist.toml`.
- Run the narrowest useful checks locally and report the exact commands.

## Rust style

- Panic-family behavior is not the paved road: avoid `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!`, unchecked indexing/slicing, and byte-index string slicing in production code and tests.
- Tests should return `Result` for fallible setup instead of using panics as convenience control flow.
- `unsafe` is forbidden unless the repository adopts a documented exception regime.
- Bare `#[allow(...)]` suppressions are discouraged. Prefer `#[expect(..., reason = "policy:<id>: ...")]` with a matching TOML exception receipt.

## CI style

- CI should run the right proof at the right time: cheap default PR checks, risk-routed expansion, and deeper validation on labels, main, nightly, release, or manual dispatch.
- Every CI lane needs an owner, proof obligation, trigger policy, approximate LEM cost, and review/expiry cadence.
- Branch protection should require the aggregate `ci/merge-gate` status rather than optional leaf jobs.
- PR caches should restore only; canonical cache saves belong on the default branch.
