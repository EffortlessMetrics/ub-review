# Publication-boundary reconciliation

Issue [#957] requires current packets to account for every publication boundary
before `FinalizedOutcome` can consume delivery truth. This document describes
the bounded shadow checker entered through
`scripts/reconcile-publication-boundaries.py`.

The checker is deliberately **read-only and non-authoritative**. It compares
what `run` prepared, the exact payload `post` attempted, the terminal delivery
transaction and post receipts, and the run-stage `gate_outcome` projection. It
does not recompute the product outcome, change the required check, post to
GitHub, or make a prepared review count as confirmed. [#959] owns delivery
finalization and [#960] owns shadow `FinalizedOutcome` integration.

## Contract

```text
validated canonical revision admission
+ terminal_state review/payload projection and truth-table invariants
+ github-review.json XOR github-review-skip.json
+ exact github-review-post-payload.json when success is claimed
+ terminal delivery-transaction.json when success is claimed
+ post-result.json XOR post-error.json when a post was attempted
+ run-stage gate_outcome.publication_result
-> bounded shadow reconciliation report
```

The checker parses the normalized `ub-review.revision-identity.v1` canonical
form, recomputes its domain-separated SHA-256 digest, validates candidate-head
versus merge-result semantics, and requires the exposed semantics, reviewed
commit, and pull-request head to match that canonical identity.

The report is written atomically to:

```text
review/publication_boundary_reconciliation.json
```

with schema `ub-review.publication_boundary_reconciliation.v1` and authority
`shadow-only`.

## Preparation and delivery states

Preparation is one of:

- `prepared`: one valid grouped GitHub review exists and terminal state agrees;
- `not_needed`: one complete skip receipt exists and terminal state agrees;
- `unverifiable`: the preparation surface is missing, malformed,
  contradictory, or cannot be joined to terminal state.

A current skip receipt must contain the complete producer shape. When
`cmd_post` consumes that skip, `post-result.json` is accepted only when it is
the identical complete skip receipt. A truncated, stale, or unrelated skip
result cannot establish `not_needed`.

Delivery is one of:

- `confirmed`: the retained post payload exactly equals the public payload
  derived from the prepared review; every success-receipt field is valid;
  terminal stdout/stderr files exist; the response state is `COMMENTED`; the
  response identifies the admitted pull-request head; and the terminal
  delivery transaction accounts for every prepared inline delivery;
- `failed`: a valid bounded post error exists or an otherwise valid success
  receipt names the wrong head;
- `prepared`: review output exists but no post-attempt receipt exists;
- `not_needed`: the run deliberately prepared no public review and any skip
  result exactly matches the validated skip artifact;
- `unverifiable`: the available artifacts cannot establish one of the states
  above.

Exact payload identity includes the event, full body, every inline anchor, and
the public comment body after the same lane-prefix/evidence trimming and
suggestion rendering used by posting. Cardinality and byte length remain useful
receipt checks but never substitute for identity.

Every prepared inline item must be represented exactly once by the terminal
transaction, with the exact pull-request head and public-body digest. A
terminal transaction that contains only a subset of the prepared comments is
`unverifiable` with `delivery_transaction_incomplete`. The live producer can
legitimately create a transaction over only `remaining_inline` during a retry;
this shadow checker therefore fails such a packet closed until [#959] can join
omitted items to independently validated prior-confirmation evidence. It does
not infer that earlier delivery succeeded.

A successful response is compared to `pr_head_commit`, not the synthetic merge
object stored as `reviewed_commit` under `merge_result` semantics. HTTP success
without a usable response head remains `unverifiable`.

`post-error.json` is definitive only for the bounded producer combinations:

```text
missing_token / preflight
invalid_repo / preflight
missing_pull_number / preflight
invalid_review_payload / payload_validation
post_http_error / network_post
post_failed / network_post
failed / unknown
```

Unknown pairs are malformed evidence, not inferred failure truth.

## Stable contradiction classes

The checker retains bounded reason tokens including:

- prepared, failed, skipped, confirmed, and unverifiable projection mismatch;
- response-head, exact-payload, and success-metadata mismatch;
- missing, wrong-head, nonterminal, failed, cleanup-mismatched, payload-mismatched,
  and incomplete delivery transactions;
- missing terminal files and invalid response state;
- invalid post-error classification and skip-result mismatch;
- impossible terminal-state combinations and terminal projection mismatch;
- canonical revision mismatch;
- preparation/post XOR violations;
- malformed, missing, unsafe-path, symlink, and budget failures.

An unavailable GitHub response head remains an explicit observation. The
checker never invents confirmation from HTTP success, equal payload lengths, or
matching item counts.

## Bounds and temporary implementation

The checker reads only a fixed allowlisted artifact set; rejects duplicate JSON
keys, non-finite numbers, unsafe paths, and symlinks; caps per-file and aggregate
input, retained findings, and report bytes; records source byte counts and
SHA-256 digests; and never changes product authority.

The temporary Python family is:

```text
scripts/reconcile-publication-boundaries.py             entrypoint and final fail-closed guards
scripts/reconcile-publication-boundaries-core.py        reviewed bounded reconciliation core
scripts/test-publication-boundaries.py                  primary regression corpus
scripts/test-publication-transaction-completeness.py    complete prepared-to-terminal negative
```

These files are owned by `release/ci` under the existing
`policy/allow.toml` script receipt. The family must move into a durable
Rust/verifier boundary or be removed before #960 can make `FinalizedOutcome`
production-authoritative. The receipt's review and expiry dates grant no
authority by themselves.

## Commands

Regression corpus: 37 cases across the two suites.

```bash
python scripts/test-publication-boundaries.py
python scripts/test-publication-transaction-completeness.py
python -m py_compile \
  scripts/reconcile-publication-boundaries.py \
  scripts/reconcile-publication-boundaries-core.py \
  scripts/test-publication-boundaries.py \
  scripts/test-publication-transaction-completeness.py
```

Inspect a packet and atomically retain the shadow report:

```bash
python scripts/reconcile-publication-boundaries.py target/ub-review \
  --write-report
```

Exit status is `0` for coherent shadow input, `1` for a typed contradiction or
otherwise incomplete but readable relationship, and `2` when required input or
bounded processing is unavailable.

The contained candidate gate enforces both regression suites and runs packet
reconciliation with `continue-on-error`. A contradictory shadow report is
evidence for #957; it does not alter legacy gate enforcement.

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
