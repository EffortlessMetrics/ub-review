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
Thus current required context/app IDs, strictness, rulesets, merge queue and
external required checks are **unknown**. Do not infer them from job names or
an API failure's presumed cause. CLI open-PR and issue reads likewise returned
`Post "https://api.github.com/graphql": Forbidden`; REST open pulls returned
`Get "https://api.github.com/repos/EffortlessMetrics/ub-review/pulls?state=open&per_page=100": Forbidden`.
Connected GitHub search/fetch reads succeeded and supplied current owner leads.

## Candidate and local proof boundary

`templates/ci/ub-review-gate.yml` and `templates/ci/independent-baseline.yml`
are dormant full workflow candidates, outside GitHub's workflow directory.
Active workflows are byte-for-byte unchanged. The existing permissions, check
commands, tools, timeouts, artifacts and trust split remain. The existing
manual gate stays active today; the dormant replacement intentionally only
handles PR admission. Owners must decide manual recovery separately at cutover.

Both candidates subscribe to opened, reopened, ready_for_review, synchronize,
labeled, unlabeled and converted_to_draft. Heavy evidence jobs require an open,
non-draft PR whose current event label set contains the whole name `merge-ready` (case-insensitive Actions
`contains` matching).
A label already present admits synchronize/reopened/ready transitions; a draft
or absent/removed label does not. Labels are sticky across pushes: owners must
remove the label to return a repair to preparation. This does not encode
human review approval or per-SHA authorization. Unrelated label edits while
admitted also rerun evidence; keeping a single native predicate avoids another
API/controller boundary. No savings or <=20-minute claim is established.

A skipped required job can count as success in Actions. Therefore the contained
job becomes `contained candidate evidence`; a no-checkout always-run finalizer
retains the exact native `ub-review/gate` check name and accepts only
`needs.gate.result == success`. Failure, cancellation and skipped admission are
non-green. The existing independent finalizer already rejects non-success and
retains `ub-review/independent-baseline`. The coverage download/upload job is
skipped for unadmitted evidence. No custom status writer, checks permission,
controller, custody store or new platform is introduced.

`tests/merge_ready_admission.rs` exercises both event-family truth tables and
checks the actual dormant source's predicate, subscriptions, native finalizers,
unchanged execution bodies, checkout identity and permission boundaries.
These are local structural/model fixtures, **not** a GitHub expression engine,
branch-protection proof, race proof, or hosted trigger test. Event delivery can
race label removal and pushes; existing cancel-in-progress is preserved. An
in-flight green check can precede the removal event; label absence is not an
atomic merge interlock. Source-only Tower (#1334/#1335/#1338) has no CI sufficiency
authority and is neither invoked nor promoted here. CI mode and source-only
review remain separate.

## Owner handoff and cutover requirements

Coordinate release/ci through #937/#1285 (containment/protection), #1270/#1275/
#1276 (baseline/equivalence/decommission), #1288 (concurrency), #1015/#1277
(final authority/result separation), #814/#809 (stable judge), and #1464
(current review_forward authority documentation). This candidate does not
replace their owners or touch their branches. #1460 retains baseline fixtures;
#1461 owns Rust projection-reader work; #1359 owns xtask path admission.

Before any activation, owners need a successful live required-check/ruleset
export including GitHub App identities, independent exact-candidate review,
actionlint, and an explicitly authorized late hosted exercise. Verify native
check association with the PR's current head/merge object, especially the
pull_request_target baseline's protected-base run SHA, required-context matching
and fork behavior. Exercise draft/open/label/remove/repair/cancel/failure cases
and demonstrate the finalizers cannot satisfy requirements when unadmitted.
Preserve the independent baseline as authority until the existing cutover
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
