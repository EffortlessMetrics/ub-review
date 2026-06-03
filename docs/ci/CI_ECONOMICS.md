# CI economics

`ub-review` is built as an evidence machine: every lane should state the claim it
supports, preserve a durable receipt, and avoid implying more than it proves.
At industrialized-agent throughput, verification cost dominates marginal
development economics. The repo therefore prefers cheap per-PR static and
scoped signals, with heavier empirical checks reserved for targeted, nightly, or
release lanes. This does not weaken verification; it stages verification by cost
and signal.

## Default posture

- Keep pull-request CI cheap, deterministic, and high-signal.
- Prefer one shared evidence packet over independent jobs that rediscover the
  same repository state.
- Keep heavy witnesses behind labels, manual dispatch, schedules, release
  readiness, or explicit profile policy.
- Preserve machine-readable receipts for evidence lanes so reviewers can audit
  what ran and what did not run.
- Make claim boundaries explicit in docs, PRs, and artifacts.

## Claim boundaries

A green CI run means the configured checks completed for the selected scope. It
is not a general proof of correctness, release readiness, parser adequacy,
security posture, fuzz robustness, mutation adequacy, or policy completeness.
Each check must say which claim it supports and which claims remain outside the
receipt.

## Lane staging

The expected staging is:

```text
cheap static and scoped checks per PR
→ targeted heavy checks when risk or labels justify them
→ release-readiness receipts before version bumps or publish actions
```

This mirrors the action's runtime shape: one runner builds the packet and cheap
sensor outputs once, then bounded model lanes reason over that shared evidence.
The same economics should guide future repo checks.
