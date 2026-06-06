# CI audit wizard contract

This document locks the artifact contracts for `ub-review audit-ci` (read-only)
and the later PR-emitting wizard mode. The decision record is
[adr/0002-single-gate-and-ci-audit-wizard.md](adr/0002-single-gate-and-ci-audit-wizard.md).

## audit-ci report

`ub-review audit-ci --out target/ub-review-audit` writes:

```text
target/ub-review-audit/
  inventory.json        parsed workflow jobs, triggers, matrices, secrets
  evidence.json         run-history evidence per job
  recommendations.json  tiered recommendations with receipts
  audit-report.md       the human-readable report
```

No file outside the output directory is created or modified. `audit-ci` makes
read-only GitHub API calls only (`actions/workflows`, `actions/runs`, branch
protection when token scope allows) and degrades to inventory-only when no
token is available. Missing evidence is reported as missing evidence.

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
  "uses_secrets": [],
  "required_check": true,
  "required_check_source": "branch-protection | unknown"
}
```

### evidence.json (`ub-review.ci_evidence.v1`)

Per job, over a stated window (default 90 days):

```json
{
  "job": "rust",
  "window_days": 90,
  "runs": 412,
  "duration_p50_sec": 312,
  "duration_p95_sec": 540,
  "pr_failure_rate": 0.06,
  "flake_rate": 0.01,
  "independent_failures": 0,
  "co_failing_jobs": ["test"],
  "runner_minutes_per_month": 340,
  "evidence_gaps": ["branch-protection unreadable: token scope"]
}
```

`independent_failures` counts PR runs where this job failed and no
`keep-required` candidate failed alongside it. It is the core downgrade
signal — and it is not sufficient alone (see survivorship rule below).

### recommendations.json (`ub-review.ci_recommendations.v1`)

Per job:

```json
{
  "job": "e2e-matrix-windows",
  "tier": "adaptive",
  "positioned_to_catch": "Windows-specific runtime regressions on release paths",
  "has_caught": "0 independent failures in 412 runs / 90 days",
  "receipts": ["evidence.json#e2e-matrix-windows"],
  "proposed_policy": "[[proof.required]] with diff_classes = [\"source-general\"] and required = false",
  "confidence": "high | medium | low",
  "judgment": "deterministic | model-assisted"
}
```

Tiers: `keep-required`, `adaptive`, `label-gated`, `flag-for-human`.

Rules:

- security-relevant jobs (CodeQL, secret scanning, provenance, signing) are
  always `flag-for-human`; the wizard never downgrades them;
- survivorship: a downgrade recommendation requires both a weak `has_caught`
  record and a `positioned_to_catch` statement that is plausibly covered by a
  `keep-required` job or an adaptive trigger; absence of failures alone is
  `confidence: low`;
- every recommendation carries at least one receipt pointer;
- `judgment: model-assisted` marks tiers chosen or adjusted by the bounded
  model lane; with `model-mode: off`, all recommendations are deterministic
  and conservative (no downgrades below `adaptive`, more `flag-for-human`).

## Wizard PR contract

The wizard mode (later) emits one PR:

```text
adds      .ub-review.toml          required proof + tool gates + adaptive triggers
edits     .github/workflows/*.yml  downgraded jobs gated behind labels/paths or removed
adds      .github/workflows/ub-review-gate.yml
states    the exact branch-protection change (required checks list) in the PR body
```

PR body structure:

```md
## What changes

## Kept required (with receipts)

## Downgraded to adaptive (with receipts)

## Label-gated heavy lanes (with receipts)

## Flagged for human decision

## Branch protection change to apply

## Rollback
```

Constraints:

- one PR, no force-push, no branch-protection mutation by the wizard itself;
- every kept/downgraded/gated line cites its evidence receipt;
- the rollback section names the exact revert (the PR revert restores prior CI
  behavior completely);
- the wizard PR must pass the repo's existing CI as it stands before the
  branch-protection change is applied — the old gate approves its successor.
