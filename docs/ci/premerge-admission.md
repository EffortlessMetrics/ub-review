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
recorded as its sole native assignee. Multiple assignees refuse before admission, even when each has write authority and a valid request. A fresh native collaborator-permission read must
also confirm the same user still has effective write/admin permission; old comment
association or assignment alone is not authority after queue wait. Selection remains per objective, not a global
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
adapter-policy SHA-256, specific native review IDs and their complete selected-evidence
generation. The complete native review inventory separately controls current
blocking decisions; unrelated comments/approvals do not mutate selected evidence. Unknown/duplicate fields, nonfinite JSON, wrong actor/source/owner,
missing reviews, review changes and current native blocking reviews refuse.
COMMENTED owner-published delegated-review reports retain their actual identity;
they are not formal approvals. The adapter verifies reference/currentness, not
the semantic quality or independence of prose. The lead must obtain substantive
independent review, and existing native approval/protection policy still applies.
No self-authored success flag substitutes for either obligation. Arbitrary prose
is not automatically classified as a material finding: use native blocking review
or withdraw/update the selected evidence, and retain the lead's final findings
judgment. Unknown or unavailable review metadata is never clean.

Keep exactly one owner-selected request label for the current subject. Unrelated
actors' malformed labels are not selection evidence. A withdrawn/reapplied
request is retired, even if source readiness is later restored. A new source,
policy or materially refreshed review needs a newly prepared request. Never
remove/re-add the same label or rerun a workflow to recover a lost response.

## Admission, duplicates and currentness

Both workflows retain their native PR event families, including the baseline's
protected-base definition. Draft/open/reopen/Ready, ordinary edits, unrelated
labels and diagnostic dispatch are non-required observations. Synchronize,
retarget, draft/close, withdrawal, blocking reviews and changes to selected
review evidence only invalidate; they
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
winner remains the winner. Later duplicates/replayed attempts of that transaction
are non-required observations returning the original run/attempt-1 link, preserving
its truthful result. No automatic retry erases a failed attempt.

Schema `ub-review.premerge.v2` carries `retry: null` for an initial transaction.
A recovery/reinstatement request uses a strict retry object containing a cause and consecutive generation
(1 through 3), the canonical prior request SHA-256, and both native prior winner
run IDs keyed by gate/baseline. There is no nonce or reason field that can evade
deduplication. Prepare it with the same command plus `--retry-of PRIOR_REQUEST_SHA256
--gate-run PRIOR_GATE_WINNER --baseline-run PRIOR_BASELINE_WINNER`. This only
prepares bytes; admission rechecks all provenance. The same assigned owner,
objective, head/base/merge/tree, policy and selected reviews must remain unchanged.
Both native original attempt-1 winners must be completed, with conclusions limited
to success/failure/cancelled/timed_out and at least one non-success. Complete
anchored histories establish the actual earliest winners in both populations.
The new owner comment and label edge must follow both terminal rows' native
updated_at timestamps; unknown times, queued/running attempts, successful-only
transactions, reruns, missing lanes, skipped generations and changed subjects
refuse. A terminal retry uses cause `terminal`. If both winners succeeded, cause `withdrawal` instead requires a verified native withdrawal of the predecessor after both terminal updates, then a fresh owner comment and application after that retirement; use `--retry-cause withdrawal`. For other invalidations, explicitly withdraw the selected label after settlement first. Successful proof is not silently reusable after retirement. Cause selection is determined by native outcomes (terminal for any failure, withdrawal for both success), so changing the cause cannot create a second valid transaction for the same predecessor/generation. Reinstatement deduplicates and shares the same three-generation bound. updated_at is a conservative native last-update bound, not a fabricated
completion timestamp. Every ancestor is checked, and the fourth retry refuses.

For a retry, withdraw the previous selected label and apply the newly prepared
one. Withdrawal truthfully invalidates the previous context. The fresh explicit
retry admits **both** populations again, including a previously successful lane;
that user-requested spend avoids silently reusing a now-invalidated required
context. Original run/attempt receipts remain. A duplicate/lost-response request
for the same retry generation still elects its original winner regardless of byte
formatting or startup order. This is an explicit bounded recovery transaction,
not a workflow rerun or a change to source/reviews merely to get another attempt.

Deleted rows/anchor, missing current run, inconsistent pages, unknown request
bytes or incomplete history fail closed. The bounded prefix is at most1200 runs;
exhaustion requires an owner-reviewed protocol/anchor migration, not a silent
floor advance that could resurrect old requests. The anchors are existing native
facts, not new credentials or a parallel state store. Native run-history behavior
and pagination consistency still require hosted witnesses.

Every worker revalidates source, owner, live request edge, reviews, integration
parents/tree and election immediately before credential-free candidate checkout.
The gate additionally verifies its actual checkout SHA and ordered parents, and
diffs against the immutable admitted base SHA rather than ambient origin/main. Fresh no-checkout
finalizers repeat validation, reject failed/skipped/cancelled/missing evidence
and own-workflow cancellation, and preserve artifact upload failures. Gate
finalization also requires one nonempty, unexpired digest-bearing gate packet
bound to the native run/head. The baseline keeps its trusted receipt/upload and
seven-worker aggregate. No protected-name skipped success is used.

Only the elected eligible request can emit the existing required context.
Duplicates/harmless events have observation names. Material invalidations emit
failure, never success; a delayed invalidation can conservatively block newer
proof. Sequential reads and native check completion are not an atomic merge lock. Both populations inspect
complete native review records and label-edge timestamps: a post-request blocking
review remains a retirement even after a later approval/dismissal. Unknown native
timestamps or dismissal identities refuse. Gate event history additionally sees
selected report body edit/revert; the baseline has no equivalent historical edit
witness, and relies on current selected bytes plus explicit withdrawal/native
blocking signals. That historical limitation is not declared solved.
The owning lead still refreshes repository state and findings before its separate
normal protected merge judgment. This service does not merge.

## Source, credentials and bounds

The adapter runs only from digest-verified canonical protected-base bytes, in
isolated Python mode (`python3 -I`). Metadata and finalizer jobs never check out
candidate code. Worker preflight runs before candidate checkout, and no
credential-bearing adapter runs later in that worker job. The existing ephemeral
job token is scoped to those helper steps only; it is not a workflow/job-wide
environment value, command argument, output, artifact or product input.

All reads remain anonymous except the fixed canonical HTTPS
`/collaborators/{validated-live-owner-login}/permission` GET. Only that request
uses the existing job token's implicit Metadata-read permission. Other API reads
and source downloads receive no Authorization header. No persisted credential,
new token, permission block, App setting or protection change is introduced.
Redirects, retries, alternate endpoints and credential recovery are refused.
Private repositories remain unsupported; missing/denied permission metadata
refuses, never falls back to assignment or an old association. The earlier
anonymous permission probe is not retried. See GitHub's [token metadata scope](https://docs.github.com/actions/deployment/security-hardening-your-deployments/configuring-openid-connect-in-hashicorp-vault)
and [collaborator API contract](https://github.com/github/rest-api-description/blob/main/descriptions/api.github.com/api.github.com.json).
Documentation supports this source design; actual hosted token acceptance remains
unproven and requires the later authorized native acceptance, without scope expansion.

Responses are capped at2MB, ordinary
lists at5x100 rows, history at12x100 rows, requests at40 calls, each call at10s,
and metadata execution at120s wall time. Bounds refuse rather than truncate clean.

Both lanes' no-checkout bootstrap downloads the adapter only at the native
main-base SHA in the canonical repository, verifies the workflow-pinned full
digest, then executes it in isolated mode. No candidate-head source, default-tip,
source fallback or credential recovery is accepted. The gate remains a candidate
workflow/Action lane; this does not create hostile-head-independent gate authority.
Missing protected source or a digest/version mismatch refuses. **The initial
configuration candidate can no longer bootstrap its own adapter from its head:**
old main lacks that protected source. Ordinary PR opening cannot qualify this
protocol. A separately reviewed/authorized installation and native acceptance
sequence must establish protected source before an explicit request can work.
No bootstrap, PR opening or protected branch mutation is performed here.
Subsequent adapter-policy changes also require a coordinated version migration;
a candidate/base mismatch stays unavailable, never a source fallback. Heavy permissions, thresholds, proof timeouts and
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
results. No <=20-minute claim follows from metadata tests or unchanged timeouts. Native
worker success still has the installed product's compatibility meaning; this
adapter does not repair or waive the separately tracked gate_result=not_proven,
model-off, Required accounting or shadow-result limitations (#1015/#1277). Inspect
the actual packet in native acceptance rather than promoting a wrapper green to
unproven semantic sufficiency.

Opening a Draft against the unchanged base necessarily starts its old seven-lane
pull_request_target baseline. Present that exact one-time old-base qualification
for a validation decision only after source/review are ready. The candidate gate
additionally requires the protected-base adapter installation and a distinct
explicit eligible native request; opening alone cannot produce required gate
success under this candidate, and candidate-source fallback is forbidden. Admission retains non-draft
eligibility. Consequently a Draft-first bootstrap also needs a later Ready
transition, which the old base would independently charge another seven-worker
baseline. A validation decision must account for both old-base admissions, or
explicitly choose one late non-draft opening (one old-base baseline) instead of
Draft-first publication. Neither alternative is performed or silently authorized
here; retaining the branch avoids both. Keep the branch/evidence while either transition is on hold. There is no skip directive or protection bypass.
Rollback after an authorized cutover restores the exact prior workflow and source
revision, preserving request/run history and failed attempts; rollback itself is
not authorized or performed by this document.
