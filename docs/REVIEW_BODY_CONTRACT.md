# Review Body Contract

The PR review is a decision memo. No word earns a place unless it changes what
the reviewer should do next.

Default hard limits:

- at most 6 KB of PR body text;
- at most 12 top-level bullets.

## Runner Rule

Use the box intelligently while it is live:

- build shared evidence once;
- run model investigation as network I/O;
- run local proof, sensors, and focused tests concurrently;
- dedupe repeated proof requests;
- write full receipts to artifacts.

The runner can spend CPU, disk, memory, network, model budget, and wall time.
The PR body spends reviewer attention.

Claims compile across lanes and sections. One semantic claim receives one final
disposition and appears once. A successfully posted inline comment is not
repeated in the summary; failed inline delivery must render the actual concise
finding rather than internal planning metadata.

## PR Body Rule

Allowed content:

- decision;
- confirmed findings;
- material unresolved questions whose missing evidence changes the decision;
- proof results;
- refutations;
- parked follow-ups;
- specific evidence gaps.

Everything else stays in artifacts:

- lane rosters;
- provider and sensor status;
- shared context hashes;
- cache manifests;
- runtime profile details;
- terminal state;
- command logs;
- raw observations;
- candidate queues, lane conflicts, and duplicate markers;
- unexecuted proof requests and inline-comment plans;
- approval filler;
- successful-tool announcements;
- generic residual risk.

Missing-proof receipts are public only when their `head` matches the current
review head and their `request_ids` contains the exact surviving observation
identity (`id`, `dedupe_key`, or an observation id). A failed receipt from
another lane, or a receipt for a question already answered by newer evidence,
remains artifact-only.

## Outcomes

Needs attention:

```md
## Decision

- Needs one route check before upstream.

## Verification questions

- Confirm `FileHandle.write` reaches the patched scalar-write path.
```

Sufficient with proof:

```md
## Test proof

- Focused red/green proof discriminates the patch: HEAD passed and base+tests failed.
```

Evidence gap:

```md
## Evidence gaps

- The focused proof timed out before it could prove the changed path.
```

Artifact-only:

```text
No PR post.
```

## Summary-Only Suppressor Policy

When reviewer-value content survives compilation but the rendered PR body is
classified as no-value boilerplate, `[review_body].summary_only_body` decides
what happens:

- `suppress` (consumer default): withhold the PR post; the skip receipt names
  this policy value and the summary-only/substantive finding counts;
- `post_substantive`: post when at least one summary-only finding is
  substantive — severity medium+ or confidence medium-high+, excluding pure
  lane-status notes;
- `post_all`: post whenever any summary-only finding exists.

Unknown values are policy parse errors and become receipted gate reasons. The
structural walls, body-size limit, bullet budget, and internal-machinery ban
hold under every value.

## Banned In PR Commentary

- no-finding boilerplate;
- model lane or provider status;
- sensor status dumps;
- shared context or cache metadata;
- terminal state summaries;
- "human should still review" disclaimers;
- generic residual-risk language.
- inline-candidate, duplicate-candidate, and cross-lane planning metadata;
- duplicate summary copies of findings already posted inline.
