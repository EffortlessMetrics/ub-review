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
validated canonical revision admission
+ terminal_state review/payload projection
+ github-review.json XOR github-review-skip.json
+ post-result.json XOR post-error.json when a post was attempted
+ run-stage gate_outcome.publication_result
-> bounded shadow reconciliation report
```

The revision admission is not trusted from matching exposed strings alone. The
checker parses the normalized `ub-review.revision-identity.v1` canonical form,
recomputes its domain-separated SHA-256 digest, validates candidate-head versus
merge-result semantics, and requires the exposed semantics, reviewed commit,
and pull-request head to match the canonical identity.

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

- `prepared`: one valid grouped GitHub review payload exists and terminal state
  reports `review_payload_status = prepared`;
- `not_needed`: one valid skip receipt exists and its payload/terminal values
  agree with `terminal_state.json`;
- `unverifiable`: the preparation surface is missing, malformed,
  contradictory, or cannot be joined to terminal state.

A current skip receipt has `schema_version = 1`, `status = skipped`, one known
`review_payload_status`, one valid `terminal_state`, and
`github_review_json = null`. When `cmd_post` consumes that skip, an optional
`post-result.json` with `status = skipped` remains consistent with
`not_needed`; it does not prove a public delivery.

Delivery:

- `confirmed`: every required success-receipt field is valid and the response
  identifies the admitted pull-request head;
- `failed`: a valid post error exists or a valid success receipt names the
  wrong head;
- `prepared`: review output exists but no post-attempt receipt exists;
- `not_needed`: the run deliberately prepared no public review;
- `unverifiable`: malformed receipts, failed success preconditions, missing
  response-head identity, or contradictory receipt surfaces cannot establish a
  delivery result.

A successful response is compared to `pr_head_commit`, not the synthetic merge
object stored as `reviewed_commit` under `merge_result` semantics. HTTP success
without a usable response head remains `unverifiable`. A skip receipt combined
with a valid post error is retained as failed delivery and a typed
contradiction; it cannot remain coherent `not_needed` state.

## Stable contradiction classes

The checker records bounded reason tokens including:

- `prepared_payload_projected_posted`;
- `failed_delivery_projected_posted`;
- `confirmed_delivery_not_projected_posted`;
- `skipped_review_projected_posted`;
- `unverifiable_delivery_projected_posted`;
- `post_response_head_mismatch`;
- `terminal_payload_mismatch` and `terminal_status_mismatch`;
- `revision_digest_mismatch` and exposed/canonical revision mismatches;
- `prepared_review_xor_violation` and `post_receipt_xor_violation`;
- malformed, missing, unsafe-path, and bounded-input classes.

An unavailable GitHub response head remains an explicit observation. The
checker does not invent confirmation from HTTP success alone.

## Bounds, ownership, and mutation policy

The checker:

- reads only a fixed allowlisted artifact set;
- rejects duplicate JSON keys, non-finite numbers, unsafe paths, and symlinks;
- caps individual files, aggregate input, retained findings, and report bytes;
- records input byte counts and SHA-256 digests;
- writes the optional report atomically;
- removes a stale prior report before a requested rewrite;
- never changes run, review, delivery, scheduling, or gate authority.

The two Python files are temporary, owned `release/ci` shadow checks under the
existing `policy/allow.toml` receipt. They must move into a durable Rust/verifier
boundary or be removed before #960 can make `FinalizedOutcome`
production-authoritative. The policy receipt retains the review and expiry
horizon; expiry never grants authority.

## Commands

Regression corpus (24 current cases):

```bash
python scripts/test-publication-boundaries.py
python -m py_compile \
  scripts/reconcile-publication-boundaries.py \
  scripts/test-publication-boundaries.py
```

Inspect a packet and atomically retain the shadow report:

```bash
python scripts/reconcile-publication-boundaries.py target/ub-review \
  --write-report
```

Exit status is `0` for coherent shadow input, `1` for a typed contradiction or
otherwise unverifiable relationship, and `2` when required input or bounded
processing is unavailable.

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
