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

## Temporary independent containment baseline

`.github/workflows/independent-baseline.yml` is a temporary self-hosting
containment check. It uses `pull_request_target` so GitHub loads the workflow
and fixed command list from the protected base branch, then checks out the
exact pull-request head SHA only for deterministic build and test execution.
The candidate executes with a read-only token posture, no repository secrets,
no OIDC permission, no persisted checkout credentials, and no shared build
cache.

This check proves that a pull-request head cannot replace the deciding command
list or substitute its own `gate_outcome.json`. It does **not** prove that
candidate tests, manifests, build scripts, policy code, or verifier code are
trusted. The released stable-coordinator and hostile-head-safe job split in
#876/#814 own that stronger boundary.

Merging the workflow does not change branch protection. The check remains
advisory until a separate maintainer-authorized operation records the exact
external rule and rollback.

Do **not** promote `ub-review/independent-baseline` through a legacy required
status-check name alone. A pull-request workflow can mint the same job/check
name, so the name is not an independent authority identity. Any temporary
promotion must use GitHub's required-workflow ruleset bound to the
protected-default-branch `.github/workflows/independent-baseline.yml` (or an
equivalent identity the candidate cannot mint). Replacing `ub-review/gate` or
retiring either workflow remains a later, separately authorized operation.
