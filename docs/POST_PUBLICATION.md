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
1 MiB; malformed, oversized or historical decision inputs without a supported
`code_gate_result` fail before posting. The command owns the gate and payload
for the attempt; concurrent replacement, including an ABA payload change, is
outside this bounded seam's qualification.

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
| All planned comments already delivered and currently reconciled | posted | confirmed | not_attempted |
| Missing, stale or malformed confirmation | not_proven | unknown | unknown or attempted |
| Receipt persistence failure | failed | failed | unknown |
| Authorized skip with no public value needed | not_needed | not_needed | not_attempted |

Confirmation requires the current in-memory success result, valid receipt
preconditions, successful HTTP status, matching repository/PR/payload path,
and a head equal to the frozen validated RevisionRef.reviewed_commit. Grouped
reviews require a positive GitHub review ID and COMMENTED/commented state.
The submitted review ID must match the container created by that transaction.
Every newly delivered or currently reconciled comment has a positive numeric
GitHub ID; a valid final reply ID cannot cover an invalid earlier reply.
The existing transaction attaches current in-memory head confirmation after
native reconciliation and head checks; a grouped REST response without
`commit_id` uses that confirmation. An explicit response commit and any attached
native head confirmation must independently match the frozen head.
Direct replies and already-delivered retries require complete, positive
planned/confirmed comment counts for the same frozen head and PR. A new reply
requires its positive GitHub comment ID; an already-delivered retry records
`not_attempted` for the current post. The finalizer does not read historical
transaction receipts to obtain this proof.

The payload must remain byte identical. A historical gate without the
independent code projection cannot confirm publication. This preserves the
existing revision semantics and the posting transaction's head checks.

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
failure, idempotence and standalone/no-value skip behavior. Existing native
fake-transport tests also execute grouped submission, direct replies and a
reconciled retry through finalization. No real GitHub delivery is claimed. The separate full outcome, artifact-authentication
and operated static-worker qualification remain unproven.

The existing Action publication-result output reads the finalized artifact;
tolerated post failure remains visible even when the advisory job succeeds.
The separate #957 shadow publication report still recognizes a narrower set
of response forms and does not qualify native lowercase grouped responses,
direct replies or reconciled retries. Its integration remains with the #957
owners; this seam does not establish one globally coherent packet.
Rollback removes the post finalization hooks, while retaining pre-post
unconfirmed truth; restoring the old prepared-equals-posted claim is invalid.
