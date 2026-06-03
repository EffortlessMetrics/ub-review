# Agent Operating Contract

This repository is a Rust-native evidence/control-plane project. Treat each
change as part of the repo's verification surface, not only as application code.

## Default operating style

- Start from the current checked-out branch and inspect the existing repo state
  before planning work.
- One PR per objective. Never combine MSRV/toolchain changes, lint activation,
  baseline resets, CI routing, API cleanup, and release/version bumps.
- If planned work already exists, do not duplicate it. Convert the change into
  an audit, repair, or doc-sync PR while preserving the original acceptance
  boundary.
- Prefer Rust and repo-native tooling. Non-Rust files are allowed when they are
  fixtures, docs, workflow/config surfaces, generated artifacts, or other
  explicitly owned receipts.
- Docs/spec rails come before implementation, but do not open more rail docs
  once the rail is good enough unless implementation discovers a missing spec.
- Keep verification deep but scoped by risk. Make the cheap default path useful
  and route expensive proof only when risk requires it.

## Review and CI behavior

- Inspect CI logs before editing for a CI failure.
- Fix the first real failure only; avoid speculative cleanup in the same PR.
- Do not claim green until the relevant checks have actually completed green.
- Treat policy receipts, evidence packets, and release-readiness docs as product
  surfaces.
- Preserve the doctrine that `ripr` shifts mutation signal left; do not frame it
  as replacing runtime mutation testing.

## Required self-review shape

Use this checklist before marking work ready:

```markdown
## Self-review

- Scope matches PR title:
- Files touched are expected:
- No unrelated cleanup:
- Policy changes are intentional:
- No Clippy test carveouts added:
- No bare `#[allow(clippy::...)]` added:
- No-panic baseline handling is scoped:
- Non-Rust allowlist changes are narrow:
- CI economics: lanes are risk-pack appropriate:
- ripr mutation-exposure framing preserved:
- Local validation:
- CI status:
- Bot comments addressed:
- Follow-ups:
```
