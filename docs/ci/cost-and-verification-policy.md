# CI cost and verification policy

CI is part of the product's load-bearing architecture. The goal is not cheap CI
for its own sake; the goal is more proof per CI minute.

We are not reducing CI because we want less verification. We are reducing wasted
CI so we can afford more verification where it matters.

We optimize for proof per Linux-equivalent minute (LEM).

## Doctrine

- Ordinary pull requests should get cheap, meaningful verification.
- Expensive validation is routed, not skipped.
- Labels authorize extra spend when a PR needs that proof now.
- Main, nightly, release, and explicit-label lanes preserve deeper validation.
- CI artifacts should make the selected checks and actual spend auditable.
