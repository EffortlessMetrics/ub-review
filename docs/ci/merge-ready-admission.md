# Dormant native merge-ready admission candidate

Owner: release/ci. Prepared against main `3dbcc2be9768b082c0896dc2360e950ffbd56a65`
on 2026-10-15. Nothing in this candidate activates CI, changes required checks,
retires proof, or closes the existing CI authority work.

## Observed inventory

| Workflow | Current trigger | Native job/check names | Authority / side effects |
| --- | --- | --- | --- |
| ub-review-gate.yml | PR opened, reopened, ready_for_review, synchronize; manual | ub-review/gate; coverage telemetry | Candidate synthetic merge checkout, source-built Action, intelligent-ci/model-off/artifact-only; gate contents:read. Separate coverage job has actions:read, contents:read, id-token:write. |
| independent-baseline.yml | pull_request_target opened, reopened, ready_for_review, synchronize | independent evidence / fmt, check, clippy, test, doc, policy, verifier; ub-review/independent-baseline | Protected-base definition, exact raw candidate SHA, read-only deterministic matrix, no-checkout finalizer, receipt artifact. |
| quality-bootstrap-tests.yml | PR path filter over two quality workflows and two bootstrap scripts | test (workflow quality/bootstrap-contract) | Offline Python fixtures, contents:read. Default PR event types, no explicit ready_for_review subscription. |
| action-smoke.yml | manual | Local action smoke | Optional model smoke, PR-write permission; separate from this admission proposal. |
| quality-backfill.yml | daily 08:17 UTC; manual | Quality telemetry backfill | Read-only GitHub collection, bounded artifact publication. |
| release-binary.yml | v* tag push; manual | Build release asset; Publish release asset | Publish job only on tag push; contents:write there. |

The checked-in `policy/ci-lanes.toml` summary_check is `ub-review/gate` and
explicitly remains a seed, not a live protection export. #937 describes an
older only-gate protection posture; it is not proof of current settings.
The live protection read failed exactly:
`Get "https://api.github.com/repos/EffortlessMetrics/ub-review/branches/main/protection": Forbidden`.
The parent subsequently reported a live branch summary requiring
`ub-review/gate` from GitHub App **15368**, and an empty ruleset list. This is
parent-supplied current evidence; the earlier local read failure is retained
above. An empty ruleset list does not establish the full classic protection
policy. The independent baseline is **not** in that reported required set;
#1285 owns that separate protection gap. No protection mutation is proposed.
CLI open-PR and issue reads likewise returned
`Post "https://api.github.com/graphql": Forbidden`; REST open pulls returned
`Get "https://api.github.com/repos/EffortlessMetrics/ub-review/pulls?state=open&per_page=100": Forbidden`.
Connected GitHub search/fetch reads succeeded and supplied current owner leads.

## Candidate and local proof boundary

`templates/ci/ub-review-gate.yml` and `templates/ci/independent-baseline.yml`
are dormant full workflow candidates, outside GitHub's workflow directory.
Active workflows are byte-for-byte unchanged. Existing heavy proof commands,
tools, timeouts, artifacts and the two proof populations remain. The source-only
Tower (#1334/#1335/#1338) path has no CI sufficiency authority and is not invoked.

Heavy qualification requires a **fresh labeled event** whose event label is
`merge-ready`, an open non-draft event snapshot, and current label presence.
Actions compares label names case-insensitively. Synchronize, reopening, Ready,
retarget/edit, closure, draft conversion, removal, and unrelated label events
never admit heavy qualification just because a label remains attached. Remove
and reapply `merge-ready` when a repaired head is ready for a new qualification.
The old sticky-label candidate and its positive synchronize fixtures are
superseded. Manual workflow_dispatch remains in the gate template for artifact
recovery; it can execute the original manual proof path, but cannot green the
protected check because it has no fresh PR label edge/current PR binding.

Each template now has a cheap no-checkout admission job before heavy allocation,
and repeats the same live check in its always-run native finalizer after proof.
The fixed `gh api repos/$GITHUB_REPOSITORY/pulls/$EXPECTED_PR` read must succeed.
The live PR must be open, non-draft, have the exact head SHA/repository, base
SHA/ref/repository, event snapshot updated_at and current merge-ready label.
Missing/failed/malformed reads reject. Snapshot movement rejects even if H is
unchanged, including retarget or a remove/reapply with a different updated_at.
This conservative timestamp binding can also reject after comments or other
activity; it does not assert that timestamps are unique edge IDs.

Permission delta for **parent review before activation**: new cheap admission
jobs and the gate no-checkout finalizer request only `pull-requests: read`;
independent finalize adds that permission to its existing `contents: read`.
Heavy candidate/evidence jobs and coverage permissions remain unchanged. Tokens
are only passed to native reads in the no-checkout jobs, never to candidate
files. No check writer, write scope, controller, custody project or new platform
is introduced. Parent must verify those authorized reads work for same-repo and
fork candidates with the proposed permissions.

Skipped protected-name jobs can count as success. Both protected-name finalizers
are therefore unconditional `always()` jobs, and accept only successful proof
plus the repeated live admission check. Failure, cancellation, skipped evidence,
manual recovery and non-edge observations are non-green. The independent receipt
still describes evidence status, not live admission authority; the final job
result enforces both. Cheap revocation/observation runs may fail without
allocating heavy work; unrelated label edits can conservatively invalidate a
previous check and require another fresh edge. They never reuse a prior green
result or manufacture success by skipping the protected name.

Concurrency separates qualification edges from observations. Qualification uses
one per-PR group with `cancel-in-progress: false`, preserving the active run;
non-edge events and manual recovery use a unique observe-run-id group and cannot
cancel qualification. Native newest-pending replacement is only preparation for
#1288, not its complete terminal accounting, duplicate, stale publication or
lifecycle proof. Older qualification can finish historical artifacts but must
pass current-state validation to green. Parent coordinates #1288 ownership.

`tests/merge_ready_admission.rs` pins the actual candidate predicates and heavy
job dependencies, preserved proof populations, manual recovery, concurrency,
and unskipped native finalizers. It executes the **actual extracted Bash/jq**
admission/finalizer bodies with a local gh fixture and no network/credentials:
changed H/repository, draft, closed, unchanged-H retarget/base movement,
remove/reapply, late and unrelated events, failed/malformed API reads and
non-success evidence. It also proves a new reapply snapshot can qualify.
These fixtures do not emulate Actions scheduling, app identity or hosted races.

Remaining native limits: live-read-to-job-completion is not atomic with merge,
and PR updated_at has finite timestamp precision. Remove/reapply or other
movement inside the same timestamp quantum can be indistinguishable from the
old edge; no per-edge nonce/current-run API proof is claimed. Branch protection
must still enforce the actual current required head/check. Hosted exercises and
owner review must assess these limits before cutover. No savings, <=20-minute,
race-free label custody or complete #1288 claim is established.

## Owner handoff and cutover requirements

Coordinate release/ci through #937/#1285 (containment/protection), #1270/#1275/
#1276 (baseline/equivalence/decommission), #1288 (concurrency), #1015/#1277
(final authority/result separation), #814/#809 (stable judge), and #1464
(current review_forward authority documentation). This candidate does not
replace their owners or touch their branches. #1460 retains baseline fixtures;
#1461 owns Rust projection-reader work; #1359 owns xtask path admission.

Before any activation, owners need the complete live classic-policy/check-identity evidence beyond the
reported gate/App15368 summary, parent acceptance of the dormant read-permission
delta, independent exact-candidate review, actionlint and an explicitly
authorized late hosted exercise. Verify native
check association with the PR's current head/merge object, especially the
pull_request_target baseline's protected-base run SHA, required-context matching
and fork behavior. Exercise draft/open/label/remove/repair/cancel/failure cases
and demonstrate the finalizers cannot satisfy requirements when unadmitted.
Preserve the independent baseline execution; #1285 owns making its protection
posture match the intended authority before the existing cutover
programme's obligations are satisfied. This is admission preparation, not
proof-equivalent retirement or sole-gate promotion.

An owner can compare each template directly with its active counterpart. Rollback
of a future authorized cutover restores the exact pre-cutover workflow blobs;
the dormant candidate itself can be reverted without affecting CI. Re-read main
and owners before integrating. No PR is opened here: current active opened
hooks would launch broad early CI even for a draft PR. Ordinary task-branch
push has no matching push hook (only v* release tags do); repository hooks are
sample files only and core.hooksPath is unset.

Alternatives considered: job skipping alone risks green admission; custom check
publication adds permissions and custody; deleting/retiring baseline loses its
independent constraint; moving candidate execution to pull_request_target would
change trust and revision semantics. Retaining the two native event families
and making only admission/finalizer changes is the bounded candidate.
