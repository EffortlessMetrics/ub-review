# CI Lane Map

The CI lane map is intentionally policy-backed rather than convention-backed.
The registry is `policy/ci-lanes.toml`; this document explains the ordinary
routing shape.

```text
PR opened/updated
  -> rust_fast_gate       (default, blocking)
  -> action_smoke         (default, blocking)
  -> workflow_policy      (planned, advisory first)
  -> ripr_advisory        (planned, label/advisory first)
  -> release_pr_build     (path/label gated)
  -> model_smoke          (label/manual gated)
  -> release_publish      (tag only)
```

## Ordinary PR lane

The ordinary lane is narrow autofocus: local Rust proof and local action
contract proof. It should not use large model calls, macOS, Windows, Docker,
GPU, full coverage, or mutation testing by default.

## Deep lanes

Deep lanes are still valuable. They should move to label-gated PRs, `main`,
nightly, release, or campaign workflows rather than being deleted.
