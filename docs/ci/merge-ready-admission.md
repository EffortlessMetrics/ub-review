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
tools, timeouts, artifacts and the two proof populations remain. Source-only
Tower (#1334/#1335/#1338) has no CI sufficiency authority and is not invoked.

Heavy qualification requires a fresh labeled event for merge-ready and a live
open, non-draft PR with that label. Actions compares label names without case.
Sticky synchronize/reopen/Ready, retarget, withdrawal, closure, review and
unrelated metadata events do not allocate heavy proof. Gate workflow_dispatch
remains artifact recovery, under a non-required observation result. It cannot
green the protected PR result without a fresh PR edge and material binding.

A no-checkout admission job uses fixed native gh reads of the live PR and
GitHub's native reviewDecision before allocation. Successful proof is rechecked
in the finalizer against live head SHA/repository, base SHA/ref/repository,
open/non-draft status, current merge-ready label and native reviewDecision.
CHANGES_REQUESTED blocks; APPROVED/REVIEW_REQUIRED/null defer remaining review
requirements to native protection. This does not invent approval policy or
reimplement reviewer aggregation. Gate additionally listens to submitted and
dismissed pull_request_review events; current blocking review changes invalidate
its result without re-running heavy proof. Independent baseline checks the same
live decision when qualifying, without adding a review-event trust source.
Existing actual findings/proof failures still reject the qualification result.

Harmless comments, title/body edits, unrelated labels, and nonblocking review
activity do not invalidate existing proof or allocate replacements. updated_at
is not authority. The admission output explicitly routes irrelevant events to
a native job name **admission observation**, distinct from ub-review/gate and
ub-review/independent-baseline. The protected names are emitted only for
qualification or relevant invalidation (or a failed/unknown admission read).
Both finalizer jobs always execute; no protected-name skipped success is used.
Observation jobs may succeed without proof, but their names cannot satisfy the
required native contexts. Observation baseline runs also skip receipt upload.

Live subject mismatch routes a delayed event for a superseded head/base to the
observation name. A late withdrawal after the label has been reapplied likewise
observes current restored readiness. A current withdrawal, draft, closure,
head/base transition or blocking review routes to the protected name and fails
without allocating proof. An unchanged-H retarget is a material transition.
Ready/reopen/synchronize events require a new explicit edge to qualify. A
withdrawal while finalization observes absent readiness rejects; if readiness
is restored for identical material inputs, prior in-flight proof remains usable.
No unique label-edge custody or history-based invalidation is claimed.

Native cancellation is checked independently of predecessor evidence. Heavy
jobs use !cancelled(). Finalizers capture cancelled() before the always-run
verification step, reject that capture even after evidence success, and have a
separate cancelled()-conditioned exit-1 step after verification for cancellation
during finalization. If the platform cancels the job itself its conclusion is
cancelled, not success. Local fixtures exercise the captured-cancellation and
rejection shell paths; hosted native scheduling/cancellation remains unproven.

Qualification keeps its per-PR cancel-in-progress:false group. Non-edge
observations have unique observe-run-id groups and cannot cancel active proof.
This prepares #1288 active-run preservation; native pending replacement is not
its complete terminal accounting or duplicate-run qualification.

Permission delta for parent review before activation: admission jobs and the
gate finalizer propose only pull-requests:read; independent finalize adds that
scope to existing contents:read. Heavy and coverage permissions stay unchanged.
No-checkout jobs alone receive the read token. The additional native
reviewDecision read uses the same scope. No check writer, write scope,
controller, custody service, new platform or Perl-specific validator is added.

Local fixtures pin source predicates, dependency/output routing, native names,
permissions, manual recovery and cancellation conditions. They execute actual
extracted Bash/jq with a local gh fixture: material movement, harmless timestamp/
comment/label changes, delayed events, review decisions, withdrawal/draft/closed,
unchanged-H retarget, failed reads, evidence failure and own-workflow cancellation.
They demonstrate harmless events cannot replace protected proof via the
observation-name expression. They do not emulate GitHub expression evaluation,
job naming/required-context association, review-event trust or cancellation.

The final live-read-to-native-completion/merge window remains non-atomic. There
is no timestamp interlock, controller or custody promise. Hosted checks must
verify native job-name expressions, current-head/base association, fork read
permissions, harmless-event retention, real revocation/review findings and
cancellation before cutover. No savings, <=20-minute or race-free claim is made.

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
