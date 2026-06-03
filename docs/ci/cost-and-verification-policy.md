# CI Cost and Verification Policy

CI is part of this repository's architecture, not plumbing. A workflow job is a
control-plane item: it must have an explicit lane, purpose, cost posture,
trigger rule, proof obligation, and duplicate-work note.

The center of gravity is:

> We are not reducing CI because we want less verification. We are reducing wasted CI so we can afford more verification where it matters.

The target is more proof per CI minute, not fewer checks.

## Doctrine

Agentic development increases verification demand. Broad validation without
routing can become economically unsustainable, especially when the same PR pays
for every platform, package shape, provider path, and deep adequacy lens by
default. This repository treats verification as an economics problem:

- generation is getting cheaper;
- verification is the bottleneck;
- Rust makes verification unusually efficient;
- `ripr` is reserved for cheap oracle-gap signal before runtime-heavy adequacy
  checks;
- LEM makes cost visible;
- risk packs tie expensive lanes to actual repository risk;
- receipts make proof auditable.

## Required questions for each lane

Every lane in `policy/ci-lanes.toml` should answer:

1. What failure mode does this catch?
2. Why does it run on ordinary PRs, or why is it deferred?
3. What cheaper signal was considered first?
4. What does it cost in Linux-equivalent minutes?
5. Is it blocking, advisory, nightly, release-only, or label-gated?
6. What does it duplicate, if anything?
7. Which artifact or receipt proves that it ran?

## Rollout posture

Budget enforcement follows measurement:

```text
visibility -> advisory -> actuals -> learned estimates -> warnings -> hard ceiling -> selective enforcement
```

The current policy files are advisory. They are intended to make CI intent
diffable and reviewable before turning cost estimates into hard gates.
