# ub-review

ub-review is an evidence-first CI review gate.

It turns one CI runner into a review cockpit: checkout once, diff once, run on-diff sensors once, start read-only model lanes immediately, run relevant proof centrally, and post one concise PR Review.

The goal is not the fastest possible review. The goal is the best useful review inside the runner budget.

## Operating principle

Whole-runner stewardship: while the runner is live, every useful resource serves the review.

CPU runs focused tests. Disk holds proof worktrees. Memory holds evidence packets. Model budget goes to reasoning over prepared evidence. Remote model calls run concurrently with local proof; provider wait does not occupy the runner's local compute lease. Time is spent producing receipts.

## Product sentence

ub-review prepares evidence, runs focused investigation lanes, proves what it
can, and reports only what changes the reviewer's decision.

## Review contract

The PR body contains only reviewer-value content:

- findings
- verification questions
- proof results
- refutations
- parked follow-ups
- specific evidence gaps that affect trust

Everything else goes to artifacts:

- lane outputs
- model status
- sensor logs
- proof stdout/stderr
- resource leases
- metrics
- raw observations

The default `[review_body]` policy enforces that split:

```toml
[review_body]
include_successful_lane_table = false
include_provider_table = "on_failure"
include_sensor_table = "on_failure"
include_execution_summary = "none"
```

## Execution contract

Models investigate. Tools produce receipts. The compiler decides what earns the
reviewer's time.

## External positioning

ub-review is proof-backed PR review. It uses the CI runner as an investigation bench: prepare evidence once, run focused model lanes, execute relevant proof centrally, and post only the result a reviewer needs.
