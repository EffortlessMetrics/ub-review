# Native explicit pre-merge admission (#1465)

This is the proposed bounded **ub-review** route, not a deployed central em-ci
interface or activation receipt. Until the configuration lands and native
acceptance passes, main still starts broad CI on Draft/open/push/Ready. Do not
open the configuration Draft without the separate old-base validation decision.

Owner: release/ci. Base: ac06293dd0871531c1826e4a3266c8dc537cf4f6.
The existing #1502@59e52e3d6aa42eb3e07c3e368c47575f085337b9 supplies the
observation-name, separate metadata/evidence/finalization and own-cancellation
patterns. Its label-only admission and permission delta are **not** installed.
The active worker populations are preserved: contained candidate Action and
all seven protected-base deterministic workers. The gate still has candidate
workflow authority; this change does not make that lane hostile-head-independent.

## Request and return

The existing accountable lead selects the candidate for its existing objective,
keeps local proof and independent exact-head review in the native PR, and is
recorded as its native assignee. Selection remains per objective, not a global
max-one queue. Native assignment, native comment identity and the actor's actual
ability to label this repository supply ownership evidence; JSON does not grant
request, review, merge or App authority.

After review/repair, use the installed adapter to **prepare bytes only**:

```
python3 scripts/premerge-admission.py prepare --pr PR --objective ISSUE \
  --owner-id NATIVE_USER_ID --review-id NATIVE_REVIEW_ID
```

It returns a compact JSON `body` and a `label`. Post the body verbatim as a native
PR Conversation comment using the existing external authorized caller, create
that exact label if absent, then apply it to the selected PR. This is a separate
explicit qualification action. Nothing here posts or labels automatically.
GITHUB_TOKEN-generated labels do not trigger this route; no new token is created.
App request effects remain conservatively refused, not declared solved.

The label is `ci:` plus the full SHA-256 of the UTF-8 comment bytes encoded as
43 unpadded base64url characters (46 characters total). The event therefore binds
all request bytes; a comment ID or an editable readiness Boolean is insufficient.
A different byte encoding can have another label, but canonical requests still
resolve to the same native transaction. The old `merge-ready` label is inert;
there is no silent alias or central trigger rename.

The strict request object binds repository/PR, existing objective, assigned owner
ID, full head repository/SHA, main base SHA/ref, actual synthetic merge SHA/tree,
adapter-policy SHA-256, specific native review IDs and the complete native review
generation. Unknown/duplicate fields, nonfinite JSON, wrong actor/source/owner,
missing reviews, review changes and current native blocking reviews refuse.
COMMENTED owner-published delegated-review reports retain their actual identity;
they are not formal approvals. The adapter verifies reference/currentness, not
the semantic quality or independence of prose. The lead must obtain substantive
independent review, and existing native approval/protection policy still applies.
No self-authored success flag substitutes for either obligation.

Keep exactly one owner-selected request label for the current subject. Unrelated
actors' malformed labels are not selection evidence. A withdrawn/reapplied
request is retired, even if source readiness is later restored. A new source,
policy or materially refreshed review needs a newly prepared request. Never
remove/re-add a label or rerun a workflow to recover a lost response.

## Admission, duplicates and currentness

Both workflows retain their native PR event families, including the baseline's
protected-base definition. Draft/open/reopen/Ready, ordinary edits, unrelated
labels and diagnostic dispatch are non-required observations. Synchronize,
retarget, draft/close, withdrawal and gate review events only invalidate; they
never start heavy work or publish required success. Harmless events have separate
run-ID concurrency groups and cannot cancel or replace qualification. Requests
use per-PR, per-workflow groups with cancel-in-progress false. A native pending
slot can still be replaced; that is an unavailable/cancelled transaction, not
permission to start another copy.

Deduplication does **not** depend on start order or concurrency alone. The adapter
requires the complete consecutive native workflow-run prefix from retained
anchors baseline39015965202/#678 and gate39015965204/#1858. Native workflow,
repository, actor, run-number and PR/head/base association are checked against
request/display identities. Identical canonical requests elect the earliest
native run number, even if a later run starts first. A queued, failed or cancelled
winner remains the winner. Later duplicates/replayed attempts are non-required
observations returning the original run/attempt-1 link, preserving its truthful
result. No automatic retry erases a failed attempt.

Deleted rows/anchor, missing current run, inconsistent pages, unknown request
bytes or incomplete history fail closed. The bounded prefix is at most1200 runs;
exhaustion requires an owner-reviewed protocol/anchor migration, not a silent
floor advance that could resurrect old requests. The anchors are existing native
facts, not new credentials or a parallel state store. Native run-history behavior
and pagination consistency still require hosted witnesses.

Every worker revalidates source, owner, live request edge, reviews, integration
parents/tree and election immediately before credential-free candidate checkout.
The gate additionally verifies its actual checkout SHA. Fresh no-checkout
finalizers repeat validation, reject failed/skipped/cancelled/missing evidence
and own-workflow cancellation, and preserve artifact upload failures. Gate
finalization also requires one nonempty, unexpired digest-bearing gate packet
bound to the native run/head. The baseline keeps its trusted receipt/upload and
seven-worker aggregate. No protected-name skipped success is used.

Only the elected eligible request can emit the existing required context.
Duplicates/harmless events have observation names. Material invalidations emit
failure, never success; a delayed invalidation can conservatively block newer
proof. Sequential reads and native check completion are not an atomic merge lock.
The owning lead still refreshes repository state and findings before its separate
normal protected merge judgment. This service does not merge.

## Source, credentials and bounds

Metadata uses only public HTTPS GitHub reads, with no token, credential fallback,
redirect, retry or candidate checkout. Private repositories and unavailable public
metadata refuse. Public rate limits can make qualification unavailable; no
permission expansion is hidden as recovery. Responses are capped at2MB, ordinary
lists at5x100 rows, history at12x100 rows, requests at40 calls, each call at10s,
and metadata execution at120s wall time. Bounds refuse rather than truncate clean.

The baseline's no-checkout bootstrap downloads the adapter only at the native
main-base SHA in the canonical repository. The gate's bootstrap uses the exact
candidate head repository/SHA, preserving that lane's existing candidate workflow
and Action authority; its metadata/finalizer jobs receive no token permissions.
Both verify the workflow-pinned full digest before execution, with no default-tip,
source fallback or credential recovery. Missing source or a hash mismatch refuses.
The gate bootstrap is not independent hostile-head admission. During the initial
configuration PR it can qualify the reviewed candidate without first installing
its adapter on main; the unchanged old-base baseline still runs its seven workers
on opening. Subsequent adapter-policy changes require a coordinated version
migration: a candidate/base policy mismatch remains unavailable, never a fallback. Heavy permissions, thresholds, proof timeouts and
release/deployment routes are preserved. The explicit label uses the supported
`manual` full-pass policy while retaining the true native labeled event metadata;
`auto` would classify it as PullRequestOther and could omit first-pass work.

## Local verification and integration hold

The cheap `ci/admission-contract` workflow runs offline controls on changes to the
configuration/adapter and does not compile or run product proof. It fetches only
the immutable preservation baseline.

Run `python3 scripts/test-premerge-admission.py` for the actual extracted YAML
shells with public HTTP fixtures beneath the production reader. Positive controls
exercise admission, worker preflight and finalization; negatives assert no worker
allocation and no successful required context. Complete worker preservation is
compared with ac06293d. Native containment tests and actionlint remain applicable.
These tests do not execute product qualification or establish hosted acceptance.

Before activation, obtain independent exact-head source review and real native
positive/negative witnesses: App15368/context/current-head association, actual
merge/raw-head checkout and parents/tree, public metadata/artifact availability,
queued duplicate/lost-response settlement, pending replacement, withdrawal,
blocking review and cancellation before/during finalization. Verify required
checks without changing protection. Retain exact-head failed/skipped/partial
results. No <=20-minute claim follows from metadata tests or unchanged timeouts.

Opening a Draft against the unchanged base necessarily starts its old seven-lane
pull_request_target baseline. Present that exact one-time old-base qualification
for a validation decision only after source/review are ready. The candidate gate
then needs its distinct explicit eligible native request; opening alone cannot
produce required gate success under this candidate. Keep Draft to avoid another
old-base Ready admission; otherwise retain the
branch/evidence without opening it. There is no skip directive or protection bypass.
Rollback after an authorized cutover restores the exact prior workflow and source
revision, preserving request/run history and failed attempts; rollback itself is
not authorized or performed by this document.
