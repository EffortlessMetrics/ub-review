# Repository operating style

This repo is a control plane for review evidence, not just a crate. The desired
operating pattern is:

```text
docs/spec rail
→ small implementation PRs
→ policy receipts
→ CI economics
→ release proof
```

At industrialized agentic-development volume, verification is part of the
product surface. The repo should make correct work cheap, incorrect work
visible, and expensive proof targeted.

## Core doctrine

- **Rust/xtask by default.** Rust code and repo-native command surfaces are the
  construction material for durable automation.
- **Non-Rust by receipt.** Shell, Python, YAML, fixtures, schemas, generated
  outputs, and release metadata are allowed when their ownership and purpose are
  clear.
- **One PR per objective.** Do not combine MSRV/toolchain changes, lint policy,
  no-panic baseline work, file policy, CI routing, API cleanup, release docs, and
  version bumps.
- **Docs first, not docs forever.** Establish enough spec rail before building;
  then stop adding roadmap unless implementation discovers a missing spec.
- **Policy receipts over tribal memory.** If the repo needs to know a rule,
  a command, policy ledger, or receipt should eventually be able to prove it.
- **Deep verification, scoped by risk.** Default CI should be useful and cheap;
  expensive checks should be routed by risk packs, labels, changed surfaces, or
  release/nightly lanes.
- **Release readiness before release mutation.** Readiness documents and dry-run
  receipts come before version bumps, tags, or publishing.

## PR ladder discipline

Before opening planned work, inspect what is already landed. If the target work
already exists, do not duplicate it. Convert the PR into an audit, repair, or
small doc-sync PR while keeping the acceptance boundary narrow.

A healthy rollout is a ladder, for example:

```text
docs map
→ compatibility/consistency audit
→ MSRV/toolchain sync
→ rustc lint floor
→ Clippy ratchets
→ no-panic hardening
→ file policy
→ CI routing
→ API cleanup
→ release prep
→ dry-run proof
```

Each rung should be reviewable on its own.

## Standard rollout doc template

Use `docs/development/<LANE>_ROLLOUT.md` for a lane-level rail when a feature or
policy rollout needs coordination.

Required sections:

```markdown
# Current state
# Target state
# What is already landed
# What is explicitly deferred
# PR ladder
# Acceptance gates
# Bot/CI loop
# Do not combine
# Release boundary
```

Doctrine line for every rollout rail:

```text
Do not open more rail docs unless implementation discovers a missing spec.
```

## Product rail template

Use `docs/development/<RAIL>.md` for product-facing seams that connect existing
substrate into user-visible behavior.

Required sections:

```markdown
# User-visible goal
# Existing substrate
# Missing connector
# Acceptance receipts
# Known deferrals
# Release status
```

## Rust version and release policy

An MSRV increase is semver-significant. A release that raises MSRV must be at
least a minor release unless an explicit compatibility policy says otherwise.
Readiness proof should precede version bumps and tags.

Rust version surfaces that should stay consistent include:

```text
Cargo.toml
rust-toolchain.toml
clippy.toml
policy/clippy-lints.toml
.github/workflows/**
README.md
AGENTS.md / CLAUDE.md
CHANGELOG.md
docs/reference/support-matrix.md
release docs
```

## Evidence boundaries

Coverage is execution-surface evidence, not correctness proof. Tests provide
behavior proof. Policy checks provide governance proof. `ripr` provides static
mutation-exposure signal. Runtime mutation testing remains the empirical
backstop for changes static analysis cannot prove.
