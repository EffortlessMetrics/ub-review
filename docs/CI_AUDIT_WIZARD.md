# CI right-sizing contract: audit-ci and setup-ci

This document locks the artifact contracts for CI right-sizing. The decision
record is
[adr/0002-single-gate-and-ci-audit-wizard.md](adr/0002-single-gate-and-ci-audit-wizard.md).

The framing is **right-sizing**, not downgrading. The doctrine is: less fixed
CI, more useful proof. Recommendations move jobs between placements:

```text
mandatory → adaptive
mandatory → recommended
mandatory → nightly / release-only
mandatory → risk-pack
mandatory → inside ub-review/gate as required proof
```

Two commands, one wedge:

```text
audit-ci    read-only report: inventory, history, receipts, recommendations.
            No workflow edits. No branch-protection changes.

setup-ci    migration-PR generator: writes .ub-review.toml, proposes workflow
            edits, prints the exact branch-protection instruction.
            --print-pr renders the PR contents without opening one.
            Never mutates branch protection itself.
```

`audit-ci` is the adoption wedge and ships first. `setup-ci` ships only after
the gate verdict surface (roadmap #23) and required-proof/tool policy
(roadmap #24) exist, because the migration PR depends on `ub-review/gate`
being able to fail correctly. A later `setup-ci --apply-branch-protection`
may apply the required-checks change with an explicitly granted admin token;
it is a separate command invocation, never a default.

## audit-ci artifacts

`ub-review audit-ci --out target/ub-review` writes under `<out>/ci-audit/`:

```text
<out>/ci-audit/
  inventory.json         parsed workflow jobs, triggers, matrices, secrets,
                         permissions, path filters, required-check status
  history.json           run/check history per job over the stated window
  costs.json             runtime percentiles, runner-minutes, matrix expansion
  correlation.json       co-failure structure between jobs
  recommendations.json   tiered recommendations with receipts
  runner-cancellations.json
                         cancellation classification receipts
  audit-report.md        the human-readable report
```

No file outside the output directory is created or modified. `audit-ci` makes
read-only GitHub API calls only (`actions/workflows`, `actions/runs`, checks,
branch protection / rulesets when token scope allows) and degrades to
inventory-only when no token is available. Missing evidence is reported as
missing evidence.

### inventory.json (`ub-review.ci_inventory.v1`)

Per job:

```json
{
  "workflow": ".github/workflows/ci.yml",
  "job": "rust",
  "name": "Rust 1.95 / 2024",
  "triggers": ["pull_request", "push:main"],
  "path_filters": [],
  "matrix_size": 1,
  "timeout_minutes": 20,
  "permissions": {"contents": "read"},
  "uses_secrets": [],
  "required_check": true,
  "required_check_source": "branch-protection | ruleset | unknown"
}
```

### history.json (`ub-review.ci_history.v1`) and costs.json (`ub-review.ci_costs.v1`)

Per job, over a stated window (default 90 days):

```json
{
  "job": "rust",
  "window_days": 90,
  "runs": 412,
  "failure_rate": 0.06,
  "cancellation_rate": 0.01,
  "flake_rate": 0.01,
  "rerun_then_pass": 4,
  "evidence_gaps": ["branch-protection unreadable: token scope"]
}
```

```json
{
  "job": "rust",
  "duration_p50_sec": 312,
  "duration_p90_sec": 480,
  "duration_p99_sec": 660,
  "runner_minutes_per_month": 340,
  "matrix_expansion": 1
}
```

### correlation.json (`ub-review.ci_correlation.v1`)

The killer metric is **independent merge-decision signal**: did this job ever
fail on a PR when all cheaper prerequisite jobs passed? A job that only fails
after `cargo test` already failed is useful diagnosis but weak branch-protection
material.

```json
{
  "job": "e2e-matrix-windows",
  "independent_failures": 0,
  "co_failing_jobs": ["test"],
  "cheaper_jobs_compared": ["fmt", "check", "test"],
  "window_days": 90
}
```

### recommendations.json (`ub-review.ci_recommendations.v1`)

Per job:

```json
{
  "job": "e2e-matrix-windows",
  "tier": "adaptive",
  "positioned_to_catch": "Windows-specific runtime regressions on release paths",
  "has_caught": "0 independent failures in 412 runs / 90 days",
  "receipts": ["ci-audit/correlation.json#e2e-matrix-windows", "ci-audit/costs.json#e2e-matrix-windows"],
  "proposed_policy": "[[proof.required]] with diff_classes = [\"source-general\"] and required = false",
  "confidence": "high | medium | low",
  "judgment": "deterministic | model-assisted"
}
```

Tiers:

```text
keep-required                cheap, deterministic, high-signal, foundational;
                             stays a standalone required check until the gate
                             absorbs it
move-to-ub-review-required   must still run on every PR, but inside
                             ub-review/gate as [[proof.required]]
adaptive                     run when diff class / paths warrant it
label-gated                  expensive witness lane behind a risk-pack label
nightly-release              valuable but not per-PR; nightly or release lane
advisory                     useful signal, never branch-protection material
flag-for-human               security, secrets, compliance, release signing,
                             deploy, provenance, or unclear ownership
```

Rules:

- security-sensitive jobs (CodeQL, secret scanning, provenance, signing,
  deploy gates, permission checks) are always `flag-for-human`; no automatic
  right-sizing, ever, unless the repo opts into an explicit written policy;
- the model lane classifies **only over deterministic receipts**; it must not
  invent facts, and every model-assisted tier carries
  `judgment: model-assisted`;
- survivorship: a right-size recommendation requires both a weak `has_caught`
  record and a `positioned_to_catch` statement plausibly covered by a
  `keep-required`/`move-to-ub-review-required` job or an adaptive trigger;
  absence of failures alone caps at `confidence: low`;
- every recommendation carries at least one receipt pointer; no recommendation
  without a receipt;
- with `model-mode: off`, all recommendations are deterministic and
  conservative: nothing right-sizes below `adaptive`, and ambiguity resolves
  to `flag-for-human`.

### runner-cancellations.json (`ub-review.ci_runner_cancellations.v1`)

This artifact keeps hosted-runner cancellation diagnosis out of the gate
verdict and out of PR prose. Each entry is artifact-only unless it changes the
current merge decision:

```json
{
  "classification": "runner_eviction_suspected",
  "workflow": ".github/workflows/ub-review-gate.yml",
  "workflow_name": "ub-review gate",
  "job": "ub-review/gate",
  "runs": 16,
  "cancellations": 16,
  "cancellation_rate": 1.0,
  "audit_cancel_events": 0,
  "runner_shutdown_signal": true,
  "github_hosted": true,
  "runner_labels": ["ubuntu-latest"],
  "suggested_action": "rerun on self-hosted or cx profile",
  "receipts": ["ci-audit/history.json#ub-review/gate"],
  "evidence": ["runs-on matches GitHub-hosted runner labels"],
  "evidence_gaps": []
}
```

`audit_cancel_events` is supplied with `--audit-cancel-events <count>` after a
separate read-only audit-log check. If that count is not supplied, audit-ci
records the gap and avoids claiming audit-log proof.

## setup-ci migration PR contract

`setup-ci` emits one PR (or prints it with `--print-pr`). Repo-file changes
only:

```text
adds      .ub-review.toml                       required proof + tool gates +
                                                adaptive triggers
adds      .github/workflows/ub-review-gate.yml
adds      docs/ci/ub-review-migration.md        the migration plan
adds      docs/ci/branch-protection-change.md   exact required-checks change
edits     .github/workflows/*.yml               right-sized jobs become
                                                non-required / label-gated /
                                                nightly where file-based
writes    <out>/ci-audit/migration-plan.md      run artifact copy of the plan
```

PR body structure (the PR is a product demo — every bullet cites a receipt:
duration, independent failures, diff class, risk class, policy reason; no
vibes):

```md
## Decision

Move this repo toward `ub-review/gate` as the single PR gate.

## Keep required

## Move into ub-review/gate

## Right-size to adaptive

## Label-gated / nightly / release

## Human review required

## Proposed branch protection change

## Rollback
```

Constraints:

- one PR, no force-push, no branch-protection mutation by the PR;
- `docs/ci/branch-protection-change.md` states the exact change
  (remove required: `<old checks>`; add required: `ub-review/gate`);
- no broad workflow rewrites in the first migration PR — minimal edits that
  implement the accepted recommendations only;
- the rollback section names the exact revert (reverting the PR restores
  prior CI behavior completely);
- the migration PR must pass the repo's existing CI as it stands before any
  branch-protection change is applied — the old gate approves its successor.

## Guardrails (v1)

```text
No automatic branch protection mutation.
No automatic security-check right-sizing.
No recommendation without receipt.
No replacing CI before gate_outcome can fail correctly.
No broad workflow rewrites in the first PR.
```
