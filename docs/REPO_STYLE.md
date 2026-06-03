# Rust repo operating style

This repo is a repo operating-system lane: it makes Rust repos easier for
agents, reviewers, and verification tooling to work in. It is not a Bun UB
implementation lane and not an `ub-review` engine lane unless explicitly
reassigned.

The target repo behaves like an evidence machine:

- strict Rust defaults;
- explicit owned exceptions;
- cheap PR proof by default;
- deeper proof routed by risk and claim;
- visible CI cost;
- release claims backed by receipts;
- agents working review-fast PR by review-fast PR;
- the repo itself as source of truth.

## Canonical tool layering

`cargo-allow` owns source-tree exceptions: unsafe, panic-family calls, lint
suppressions, generated files, scripts, workflows, non-Rust files, and other
syntax-visible exceptions.

`ripr` is static mutation-exposure analysis. It gives mutation-like weak-oracle
signal earlier and cheaper than runtime mutation.

`unsafe-review` owns unsafe/native reviewability: safety contract, guard, reach,
and witness route.

`xtask` orchestrates repo-local receipts, policy checks, CI planning, and release
control. It should wrap or aggregate specialized tools rather than reimplement
`cargo-allow`, `ripr`, or `unsafe-review`.

`cargo-mutants`, Miri, and Codecov are runtime and execution-surface backstops.
They should be scoped by risk and claim.

## Maturity ladder

Small crate:

- strict `Cargo.toml` lint defaults;
- `AGENTS.md` with review-fast expectations;
- `policy/allow.toml` only when exceptions exist.

Serious repo:

- `policy/allow.toml` as the default exception ledger;
- PR template receipt section;
- CI budget and lane policy;
- docs for Clippy, panic-family calls, and non-Rust surfaces.

High-volume repo:

- `xtask` parse/check commands for policy files;
- inventory/propose commands for exceptions;
- risk packs that select deeper proof;
- CI summary output that distinguishes pass, fail, advisory, and skipped by
  policy.

Industrialized swarm repo:

- learned CI budgets;
- branch protection on a summary check after it exists;
- scheduled/runtime backstops for mutation, Miri, coverage, fuzzing, release,
  and platform claims;
- subagent roles for policy dedupe, CI economics, `cargo-allow`, `ripr`,
  `unsafe-review`, docs, `xtask`, and cleanup review.

## Review-fast rule

Every PR should land one coherent layer: repo style page, exception-ledger
policy, `ripr` doctrine, `unsafe-review` policy, CI economics, review-fast agent
contract, coverage claim boundaries, source-of-truth docs, or tests/fixtures for
existing tooling.

If two PRs encode the same doctrine, keep the better layer and close or
supersede the weaker one.
