# Branch protection

Branch protection requires exactly one check: the `ub-review/gate` workflow
(#602 — updated from the seed-contract name `PR Gate Success`, which never
existed as a GitHub check). This is the single meta-gate that runs the
deterministic tool registry plus the `[[proof.required]]` tasks and produces
the `gate_outcome.v1` verdict.

```text
ub-review/gate
```

Do not require individual matrix leaves such as macOS, Windows, coverage,
mutation, `ripr`, Docker, GPU, or feature-matrix jobs. Optional and expensive
jobs can be skipped by policy, and skipped optional jobs should not strand a
required check. Until `PR Gate Success` exists, keep the existing GitHub checks
as the source of truth and treat this document as the target contract.

The summary check should distinguish:

- passed;
- failed;
- skipped by policy;
- advisory failed.

A skipped optional lane is not a pass. It is a policy decision recorded by the
summary.
