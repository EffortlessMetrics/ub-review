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

## PR Body Rule

Allowed content:

- decision;
- confirmed findings;
- verification questions;
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
- generic residual risk.

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

## Banned In PR Commentary

- no-finding boilerplate;
- model lane or provider status;
- sensor status dumps;
- shared context or cache metadata;
- terminal state summaries;
- "human should still review" disclaimers;
- generic residual-risk language.
