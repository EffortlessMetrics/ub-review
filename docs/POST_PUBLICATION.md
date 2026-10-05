# Prepared output and confirmed publication

This is the bounded post-run seam owned by #959 under #926, using the existing
#765 posting transaction. It does not migrate gate enforcement or implement
the complete FinalizedOutcome contract owned by #960.

`run` writes `publication_result: not_proven` when a grouped review is prepared.
Preparation is never proof of delivery. It also retains `code_gate_result`,
computed from the same existing receipt-derived decision before publication
uncertainty. The legacy `conclusion`, code findings, proof accounting and
enforcement policy keep their existing meanings.

When `post` finds a sibling `gate_outcome.json`, it freezes the decision source
and prepared payload hash, then invalidates any old publication confirmation
before a new attempt. A standalone `post` without that artifact keeps its
existing receipt-only behavior. Gate and payload inputs are each bounded to
1 MiB; malformed or oversized decision inputs fail before posting.

After persisting the current attempt's existing success/error/skip receipt,
`post` finalizes only publication fields and their dependent reported
`gate_result`/publication reasons. It uses the frozen `code_gate_result` without
recomputing code evidence. Legacy `conclusion`, analysis, deterministic reasons,
proof accounting and all non-publication uncertainty remain unchanged.

| Evidence | publication_result | delivery_result | delivery_attempt |
| --- | --- | --- | --- |
| Prepared only | not_proven | prepared | not_attempted |
| Missing token / preflight failure | failed | failed | blocked |
| Network posting failure | failed | failed | attempted |
| Valid current-revision success | posted | confirmed | attempted |
| Missing, stale or malformed confirmation | not_proven | unknown | unknown or attempted |
| Receipt persistence failure | failed | failed | unknown |
| Authorized skip with no public value needed | not_needed | not_needed | not_attempted |

Confirmation requires the current in-memory success result, valid receipt
preconditions, a positive GitHub review ID, COMMENTED state, successful HTTP
status, matching repository/PR/payload path, and response commit equal to the
frozen validated RevisionRef.reviewed_commit. The payload must remain byte
identical. A historical gate without the independent code projection cannot
confirm publication. This preserves the existing revision semantics; it does
not relax the posting transaction's head checks.

The finalizer never scans prior post-result/error files for confirmation. It
records digests of the current receipt's compact serde JSON and the frozen raw
review bytes, plus a bounded machine-readable reason. It never copies arbitrary
transport error text into the gate. Repeating finalization with the same frozen
source/result is byte-idempotent. Changed decision sources are rejected and
changed payloads cannot confirm delivery.

No model call, command execution, network request, credential discovery or
posting retry occurs in finalization. Synthetic Rust fixtures cover success,
missing-token preflight through the actual post command, network failure,
prepared-only output, stale/malformed responses, changed sources, persistence
failure, idempotence and standalone/no-value skip behavior. No real delivery is
claimed by those fixtures. The separate full outcome, artifact-authentication
and operated static-worker qualification remain unproven.

The existing Action publication-result output reads the finalized artifact;
tolerated post failure remains visible even when the advisory job succeeds.
Rollback removes the post finalization hooks, while retaining pre-post
unconfirmed truth; restoring the old prepared-equals-posted claim is invalid.
