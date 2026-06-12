# Runner cancellation classification

Cancelled runs are classified as infrastructure evidence unless a receipt
shows a code or required-evidence failure. Do not treat a hosted-runner
cancellation as proof that the PR is bad.

`audit-ci` writes `ci-audit/runner-cancellations.json` from deterministic
inputs:

```text
cancelled_superseded        workflow uses cancel-in-progress; inspect the newer
                            run for the same PR/head
runner_eviction_suspected   audit_cancel_events=0, GitHub-hosted labels, and a
                            runner shutdown signal were all observed
unavailable_repeated        repeated hosted-runner cancellations, but the audit
                            or shutdown evidence is incomplete
unknown                     cancellation exists, but the cause is not proven
```

The audit-log count is explicit input:

```bash
ub-review audit-ci --audit-cancel-events 0
```

Use that only after a read-only audit-log check for cancellation events. When
the count is not supplied, the artifact records an evidence gap and avoids
claiming audit-log proof.

Runner-profile doctrine:

```text
GitHub-hosted    adoption default; quick, normal, and advisory runs
cx/self-hosted   merge-critical stacks, long proof, heavy fills, and repeated
                 hosted-runner cancellation diagnosis
```

Red gates should cite code or required-evidence receipts. Cancelled or
repeatedly unavailable gates should cite the runner-cancellation receipt and
rerun on a self-hosted or `cx*` profile before drawing a code conclusion.
