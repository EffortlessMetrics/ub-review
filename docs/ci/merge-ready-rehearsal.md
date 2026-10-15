# Rerunnable native admission rehearsal (#1465)

Owner: release/ci; continuation remains with Steven's release-readiness campaign.
Source subject: `0528c88244b28b20642218fc25c763cc01517ec0`, tree
`cf93d160ba3530da4234c04942672043276d32da`, main/base
`3dbcc2be9768b082c0896dc2360e950ffbd56a65`. This document supplements the
[source contract](merge-ready-admission.md), without activating its templates.
Markdown is covered by `policy/allow.toml` receipt
`non-rust-docs-and-templates` (ub-review/docs).

## Recorded outcome and resume boundary

Read back on 2026-10-15: [issue #1465 and owner discussion](https://github.com/EffortlessMetrics/ub-review/issues/1465),
including [the desktop result](https://github.com/EffortlessMetrics/ub-review/issues/1465#issuecomment-6183154565).
The source branch still resolves to the subject above. #1288 retains concurrency
and terminal accounting; #1285 retains baseline protection; the remaining
cutover owners in the source contract keep their scopes. No competing candidate
writer is assigned by this document.

Private sandbox: `EffortlessSteven/ub-review-sdlc-rehearsal-1465-20261015`.
[Preserved report](https://github.com/EffortlessSteven/ub-review-sdlc-rehearsal-1465-20261015/blob/3144225e44530579d979dc8e4ecb02b0d83e22c3/RESULTS.md)
and [controls](https://github.com/EffortlessSteven/ub-review-sdlc-rehearsal-1465-20261015/blob/3144225e44530579d979dc8e4ecb02b0d83e22c3/README.md)
were read through the connected GitHub reader; access is required.
Installation/base: `127464c9ea6638f77d1b48312501abb51ff80819`;
draft PR #1 head: `80ab06e9f103f0ef0cd713be1bc60394d70b915f`.

| Smoke | Recorded result | What it establishes |
| --- | --- | --- |
| [gate 38671966499](https://github.com/EffortlessSteven/ub-review-sdlc-rehearsal-1465-20261015/actions/runs/38671966499) | failure before admission/finalizer steps; evidence skipped | Scheduler refusal, protected fallback name resolution |
| [baseline 38671967954](https://github.com/EffortlessSteven/ub-review-sdlc-rehearsal-1465-20261015/actions/runs/38671967954) | same refusal; matrix placeholder skipped | Same limit; no expanded worker execution |

The recorded annotation says recent account payments failed or spending limit
needs increasing. This is provisioning nonexecution, not a failed behavioral
test. The report records both check suites at the smoke head and App **15368**;
it does not establish internal `github.sha`, required-policy matching, or
successful observation naming. Raw API evidence is retained in that sandbox's
`evidence/smoke-evidence.json`; this continuation also read that preserved export
(observed at 02:38:42 UTC), confirming zero admission/finalizer steps, skipped
workers, the billing annotations, head and App identity. It is not a fresh
scheduler export. No hosted behavior passed.

## Run protocol and expected matrix

Run only after the owner confirms scheduling is available in this same private
facility. Do not change billing, credentials, protections or permissions, or
create an alternate account/repository/runner route. Refresh owners, sandbox
source, PR head/base and active runs first. Preserve the original failed runs.
Rerun one original failed workflow as a **smoke**, recording its attempt and
original event snapshot; the live PR must still match that snapshot. If it has
drifted, use a fresh draft fixture at the preserved installation instead.

Smoke acceptance: actual admission and finalizer steps run, no evidence worker
allocates, and the protected finalizer **fails** for draft/unadmitted state.
Expected rejection is a passed scenario, not green qualification. Repeat the
smoke for the other event family only after the first executes as expected.
Stop again on provisioning refusal. This assignment prepares the subsequent
matrix; it does not launch it or authorize production cutover.

For later authorized continuation, use isolated fixture PRs/reset state between
rows; wait for each relevant event to be observable. Record run ID/attempt,
workflow installation blob, event action, exact event/live head and base
SHA/ref/repository, job names/results, worker starts, check-suite/check-run SHA
and App, and receipt identity. Capture before/after checks on the same PR head
to test preservation. Seven baseline worker identities must remain distinct.

`P` below means the native finalizer `ub-review/gate` or
`ub-review/independent-baseline`; it does not imply that branch policy requires
both. `O` means `admission observation`, which must never satisfy P. Except the
manual recovery and review rows, exercise both native event families.

| Scenario / rerunnable stimulus | Evidence allocation | Expected native outcome |
| --- | --- | --- |
| Draft open; push while draft; then Ready without a fresh edge | none | P fails; Ready alone cannot qualify |
| Open/reopen/synchronize with sticky merge-ready | none | P fails; remove/reapply label only when explicitly qualifying |
| Open non-draft, current head/base, fresh merge-ready edge; no control markers | one gate worker / seven baseline controls | P succeeds only with successful workers and unchanged material readiness at final read |
| Fresh edge with `rehearsal-wait`; edit title/body (retain marker), add/remove unrelated label while waiting | existing workers continue; metadata starts none | Metadata O succeeds; prior P is preserved; original proof may succeed |
| Comment on PR | none from this event | No subscribed issue-comment run; do not mistake a local commented fixture for hosted subscription |
| Remove merge-ready while waiting, convert to draft, or close | no replacement workers | Current invalidation P fails; in-flight finalizer rejects absent readiness |
| Restore identical readiness before old final read; delay old withdrawal event | new fresh edge may separately qualify | Late withdrawal O; old in-flight proof may succeed on identical restored inputs; no unique-edge custody claim |
| Push new head during wait or retarget unchanged head | no heavy allocation from transition | Current synchronize/retarget P fails; old finalizer rejects mismatch; delayed superseded-subject events O |
| Advance base branch SHA while proof waits | no subscribed base-push run | Existing in-flight finalizer rejects live base mismatch; base push alone emits no new P |
| Independent reviewer requests changes | no heavy review-triggered work | Gate review event P fails on native CHANGES_REQUESTED; baseline has no review-event subscription, but qualification/finalization rejects blocking decision |
| Nonblocking review / dismiss blocking review | no heavy review-triggered work | Gate O for current nonblocking native decision; dismissal alone grants no qualification; native protection still owns approval requirements |
| Fresh edge with `rehearsal-fail` / `rehearsal-skip` | deterministic fail / deliberately skipped | P fails for failed/skipped predecessor; no green aggregate from missing proof |
| Cancel while workers wait | no replacement from cancellation | Workers/finalizer never produce successful P; native cancellation may prevent steps entirely |
| `rehearsal-finalize-wait`, cancel after workers succeed before cancellation capture | workers already succeeded | Captured cancellation rejects P; success of predecessor is insufficient |
| Cancel during verification after capture | workers already succeeded | P failure/cancelled, never success; separately exercise late cancellation guard, not just pre-capture wait |
| Multiple fresh edges queued; metadata during active run; unrelated PR | bounded qualification groups; unique observation groups | Active proof not cancelled by observation; inspect pending replacement; do not claim #1288 terminal-accounting completion |
| Gate manual workflow_dispatch | artifact recovery permitted | O only, cannot green P; baseline has no dispatch subscription |
| Material live read unavailable or malformed | no admitted proof | Admission fails, fallback P fails; harmless metadata must still O without API reads |
| Fork PR and native approval freshness under existing policy | only explicit admitted edge | Read scope and subject association must be observed; if reviewer/policy/facility absent, blocked rather than passed |

Sandbox body controls are preserved at the linked README: `rehearsal-wait`
(90s), `rehearsal-fail` (exit 17), `rehearsal-skip`, and
`rehearsal-finalize-wait` (90s before capture). Keep control markers through
unrelated body edits. There is no dedicated after-capture timing control;
if that window cannot be observed safely, leave that cancellation row unproven.
The sandbox has no installed branch protections and the sole author cannot
self-approve/request changes. Required-context matching and approval freshness
need an independent reviewer and an existing suitable authorized policy surface;
do not install policy to manufacture a pass. API outage injection likewise needs
an explicitly scoped fixture; never modify real access to produce it.

## Local source evidence and limits

Preferred warm rerun at the exact candidate, from an isolated checkout:

```bash
cargo test --locked --offline --test merge_ready_admission -- --nocapture
```

This continuation's command could not start: `cargo: command not found`.
No Rust build/test or fresh actionlint result is claimed. The issue's earlier
3/3 Rust and independent review receipts remain historical exact-source proof.
A temporary local Python driver instead extracted the actual Bash bodies from
both unchanged templates and used Bash/jq plus recording local gh responses:
**72 assertions passed** (36 per workflow), including fresh-edge success,
sticky rejection, metadata/API-outage preservation, material mismatch,
revocation, native review decisions, predecessor failure/skips and cancellation
shell paths. This is shell execution, not GitHub expression/scheduling emulation.
No harness is added to the repository; the durable Rust fixtures remain canonical.
Local result JSON SHA256:
`7bf42c3d6651df231e0a375ce7e573be8383b8c2b999791427ec3a1f35d85eef`.
The existing cached xtask binary's policy-check passed (six policy files,
16 receipts), retaining ten inherited overdue-review warnings. Staged precommit
could not complete: `run cargo metadata --format-version=1 --no-deps: No such
file or directory (os error 2)`. This cached-tool result is not a rebuilt
exact-candidate Rust baseline. `git diff --check` passes.
Active workflow diff against the base is empty. No source repair is justified
by the scheduler refusal. Only this documentation is added.

## Stop and rollback

On an unexpected behavioral result, stop matrix advancement, retain exact
run/check/artifact and source identities, and return the smallest counterexample
to release/ci. Leave failed checks intact; do not weaken conditions or rename
protected checks to obtain green. Cancellation outcomes require native evidence.

For this dormant documentation branch, revert its documentation commit; active
CI is untouched. For sandbox continuation, stop new fixture events, terminalize
only task-owned runs if necessary with an explicit cancellation receipt, and
retain the failed attempts and private evidence. Do not delete the sandbox as
cleanup. A future separately authorized production cutover must first retain
exact pre-cutover workflow blobs and use owner-reviewed restoration of those
blobs, with protection and legacy enforcement preserved. Hosted plumbing,
product proof populations, fork trust, non-atomic final read/completion races,
and portfolio timing remain separate acceptance obligations.
