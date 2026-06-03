# Freshness rail

Fresh source is not enough. Test harnesses must resolve fresh binaries too.
Many agent and CI failures are stale-context failures rather than logic
failures, so freshness is a product concern for this repository.

## Checks to preserve

A future `freshness-check` should verify that:

- the branch is based on the intended target branch;
- issue bodies and implementation-ready docs are treated as the current source
  of truth;
- stale comments are research logs, not authoritative requirements;
- test harnesses resolve the just-built `ub-review` binary;
- `CARGO_TARGET_DIR` is interpreted consistently;
- no harness silently falls back to an older workspace binary;
- release docs cite current receipt paths.

## Claim boundary

Freshness checks prove that the selected harnesses and docs point at current
inputs and binaries. They do not prove behavior correctness or release
readiness; they prevent stale evidence from masquerading as current proof.
