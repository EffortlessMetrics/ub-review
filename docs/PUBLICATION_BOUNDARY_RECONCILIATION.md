# Publication-boundary reconciliation

Issue [#957] requires current packets to account for every publication boundary
before `FinalizedOutcome` can consume delivery truth. This document describes
the bounded shadow checker in
`scripts/reconcile-publication-boundaries.py`.

The checker is deliberately **read-only and non-authoritative**. It compares
what `run` prepared, what `post` recorded, and what the run-stage
`gate_outcome` projected. It does not recompute the product outcome, change the
required check, post to GitHub, or make a prepared review count as confirmed.
[#959] owns delivery finalization and [#960] owns shadow
`FinalizedOutcome` integration.

## Contract

```text
immutable revision admission
+ github-review.json XOR github-review-skip.json
+ post-result.json XOR post-error.json when a post was attempted
+ run-stage gate_outcome.publication_result
-> bounded shadow reconciliation report
```

The report is written to:

```text
review/publication_boundary_reconciliation.json
```

with schema:

```text
ub-review.publication_boundary_reconciliation.v1
```

Its authority is always `shadow-only`.

## States

Preparation:

- `prepared`: one grouped GitHub review payload exists;
- `not_needed`: a valid skip receipt exists;
- `unverifiable`: the preparation surface is missing, malformed, or
  contradictory.

A current skip receipt has `schema_version = 1`, `status = skipped`, one known
`review_payload_status`, and `github_review_json = null`. When `cmd_post`
consumes that skip, an optional `post-result.json` with `status = skipped`
remains consistent with `not_needed`; it does not prove a public delivery.

Delivery:

- `confirmed`: a successful post receipt identifies the admitted pull-request
  head;
- `failed`: a post error exists or a success receipt names the wrong head;
- `prepared`: review output exists but no post attempt receipt exists;
- `not_needed`: the run deliberately prepared no public review;
- `unverifiable`: available receipts cannot establish a delivery result.

A successful response is compared to `pr_head_commit`, not the synthetic merge
object stored as `reviewed_commit` under `merge_result` semantics. HTTP success
without a usable response head remains `unverifiable`.

## Stable contradiction classes

The checker records bounded reason tokens including:

- `prepared_payload_projected_posted`;
- `failed_delivery_projected_posted`;
- `confirmed_delivery_not_projected_posted`;
- `skipped_review_projected_posted`;
- `post_response_head_mismatch`;
- `prepared_review_xor_violation`;
- `post_receipt_xor_violation`;
- malformed, missing, unsafe-path, and revision-mismatch classes.

An unavailable GitHub response head remains an explicit observation. The
checker does not invent confirmation from HTTP success alone.

## Bounds and mutation policy

The checker:

- reads only a fixed allowlisted artifact set;
- rejects duplicate JSON keys, non-finite numbers, unsafe paths, and symlinks;
- caps individual files, aggregate input, retained findings, and report bytes;
- records input byte counts and SHA-256 digests;
- writes the optional report atomically;
- removes a stale prior report before a requested rewrite;
- never changes run, review, delivery, scheduling, or gate authority.

## Commands

Regression corpus:

```bash
python scripts/test-publication-boundaries.py
```

Inspect a packet and atomically retain the shadow report:

```bash
python scripts/reconcile-publication-boundaries.py target/ub-review \
  --write-report
```

Exit status is `0` for coherent shadow input, `1` for a typed contradiction,
and `2` when required input or bounded processing is unavailable.

The contained candidate gate runs the regression corpus as an enforced source
check and runs packet reconciliation with `continue-on-error`. A contradictory
shadow report is evidence for #957; it does not alter legacy gate enforcement.

## Acceptance boundary

This slice proves the checker contract and exposes live contained-packet
publication state. It does **not** complete #957 by itself. Complete #957 still
requires production-generated coherent model-off, model-on, and worker packets,
full task/projection reconciliation, and every applicable publication boundary.
It also does not satisfy prepared-versus-confirmed finalization under #959 or
outcome integration under #960.

[#957]: https://github.com/EffortlessMetrics/ub-review/issues/957
[#959]: https://github.com/EffortlessMetrics/ub-review/issues/959
[#960]: https://github.com/EffortlessMetrics/ub-review/issues/960
