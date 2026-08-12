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

When the admitted body exceeds the byte or bullet budget, the compiler applies
deterministic evidence/materiality ordering and writes
`review/output_degradation.json`. That receipt binds the exact head, original
and final sizes, retained topic identities, dropped-topic reasons, selected
fallback mode, and configured limits. The complete admitted inputs remain in
the review artifacts; body-size pressure alone is never a code failure.

Claims compile across lanes and sections. One semantic claim receives one final
disposition and appears once. A successfully posted inline comment is not
repeated in the summary; failed inline delivery must render the actual concise
finding rather than internal planning metadata.

## Inline Comment Rule

An inline comment is a margin note on one source line, so it carries two extra
limits beyond the PR body rule:

- **No lane identity.** `review/github-review.json` keeps the `[lane]` prefix as
  artifact provenance; the text posted to GitHub is the reviewer-facing sentence
  only. The strip happens once, in `github_review_post_comment_body`, alongside
  the suggestion-fence rendering.
- **Length.** A candidate whose reviewer-facing text exceeds
  `INLINE_COMMENT_MAX_REVIEWER_CHARS` is not a line comment. It is demoted to a
  summary-only finding that keeps its own text and names the anchor it lost —
  never dropped.

Demotion is not deletion. When an inline candidate fails the inline guard, the
refuter, or a candidate-only lane rule, the surviving summary-only finding keeps
the model's own comment text, prefixed with the `path:line` anchor it claimed so
the finding stays line-level and actionable. The machine diagnostic naming the
demotion reason lives in the finding's `evidence` field, artifact-side, and is
never rendered into the PR body.

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
review head and at least one `request_ids` value exactly matches the topic's
structural claim ID, an exact observation ID or dedupe key compiled into that
topic, or an explicitly linked proof-request ID. Lane ownership alone is routing
metadata, not proof linkage. Identity matching is exact; prefixes and substrings
do not establish linkage. A failed receipt for another claim, or a receipt for a
question already answered by newer evidence, remains artifact-only.

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
