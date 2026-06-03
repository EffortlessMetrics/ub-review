# CI Cost and Verification Policy

This repository treats verification as architecture, not an afterthought. CI is part of the product surface because high-volume agentic development raises verification demand.

## Doctrine

We are not reducing CI because we want less verification. We are reducing wasted CI so we can afford more verification where it matters.

We optimize for proof per Linux-equivalent minute (LEM).

## Policy

- Ordinary pull requests run cheap, meaningful proof by default: format, compile, lint, unit/oracle tests, policy checks, and advisory mutation-exposure signal where available.
- Expensive lanes are routed, not skipped. Labels, changed files, risk packs, and release cadence decide when deeper validation runs.
- Every lane must have a named failure mode, expected artifact, skip rule, and estimated LEM cost.
- CI budget overrides are explicit spend decisions and must leave reviewable receipts.

## Default PR posture

The default PR budget favors deterministic Linux checks and Rust-native verification. Broad matrices, Docker-heavy lanes, model validation, hardware checks, coverage, and runtime mutation testing are reserved for labels, main/nightly, or release readiness.
