# UB Review

- Shared context: `independent-context`
- Profile: `gh-runner`
- Base: `HEAD~1`
- Head: `HEAD`
- Changed files: `2`
- Inline comments: `0`

## Decision

- Needs focused proof failure resolved before upstream.

## No blocking finding after checking

- Shared diff packet, changed-file list, diff flags, sensor receipts, model lane receipts, and Bun lane packet prompts.
- Inline-comment guardrails found no validated candidate comments to post.

## Confirmed findings

- None validated for inline posting.

## Summary-only findings

- None.

## Failed objections

- Strongest failed objection: a model or sensor may have found a real issue, but no blocker/high/medium candidate survived the bounded lane run, diff-line validation, and refuter path.
- Missing evidence is not a failed objection; it is listed separately below.

## Residual risk

- Artifact audit note: unsafe/native route truth, oracle strength, and unavailable evidence remain tracked in artifacts.

## Parked follow-ups

- No parked follow-up was promoted from ledger or lane evidence in this run.

## Missing or failed evidence

- Recorded proof result `head_failed`: `review/proof_receipts.json#head_failed-stale`; revision unrecorded; legacy base `origin/main`, head `old-head`.

## Model lanes

