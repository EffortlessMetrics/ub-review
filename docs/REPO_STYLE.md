# Rust repo operating style

This repo is a repo operating-system lane: it makes Rust repos easier for
agents, reviewers, and verification tooling to work in. It is not a Bun UB
implementation lane and not an `ub-review` engine lane unless explicitly
reassigned.


## Evidence-machine doctrine

This repo is operated as an evidence machine. Rust and `xtask` are the default
construction material. Non-Rust files, unsafe, panic paths, lint suppressions,
generated files, workflow behavior, process/network access, expensive CI lanes,
and release claims must be owned and receipted.

Static evidence runs first:

- `cargo-allow` for source exceptions;
- `ripr` for static mutation-exposure;
- `unsafe-review` for unsafe-contract review;
- rustc and Clippy for code-shape policy.

Runtime evidence runs where it pays:

- focused tests on PRs;
- targeted mutation for risk PRs;
- broader mutation, Miri, fuzz, and coverage on nightly and release lanes.

CI is designed for proof per Linux-equivalent minute. Default PRs are cheap,
deterministic, and high-signal. Deep validation is preserved, but routed by risk
pack, label, main, nightly, or release.

Agents work one review-fast PR at a time. Review-fast does not mean tiny; it
means coherent seam, nearby proof, efficient verification, and honest claim
boundary. Do not broaden scope to satisfy CI. Do not add invisible exceptions.

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

- `cargo xtask policy-check` for parse-only policy receipt validation;
- `cargo xtask policy-inventory` for receipt and CI policy counts;
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

## Seed policy ledgers

`policy/allow.toml` is the default durable source-exception ledger. Keep it as
the first home for syntax-visible exceptions: unsafe, panic-family calls, lint
suppressions, generated files, scripts, workflows, and non-Rust files.

Use companion ledgers only when they add review semantics:

- `policy/clippy-lints.toml` records active and targeted lint policy.
- `policy/clippy-debt.toml` records retained lint-policy debt when a repo cannot
  turn a lint on yet.
- `policy/ripr-suppressions.toml` records reviewed static mutation-exposure
  suppressions.
- `policy/unsafe-review-suppressions.toml` records reviewed unsafe-contract
  suppressions.

Do not seed empty debt or suppression ledgers. Create them when there is a real
exception to own or enough volume that splitting the ledger improves review.

## Review-fast rule

Every PR should land one coherent layer: repo style page, exception-ledger
policy, `ripr` doctrine, `unsafe-review` policy, CI economics, review-fast agent
contract, coverage claim boundaries, source-of-truth docs, or tests/fixtures for
existing tooling.

If two PRs encode the same doctrine, keep the better layer and close or
supersede the weaker one.

See [REPO_OPERATING_HANDOFF.md](REPO_OPERATING_HANDOFF.md) for the current
cross-repo adoption package and lane handoff notes.
