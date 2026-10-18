# Protected-source installation sequence (#1465)

This is a prepared source plan. No PR, CI request, merge or protected ref write
is performed. Current main is ac06293dd0871531c1826e4a3266c8dc537cf4f6.

Use three PRs to obtain a recoverable cutover without a special bootstrap retry
protocol. All three branches are owned continuations of release/ci #1465; #1502
and its branch remain untouched.

| PR | Prepared branch | Source installed | Actual qualification after authorization |
|---|---|---|---|
| 1 | `codex/1465-protected-helper-prep-20261018` | Exact adapter, dormant workflow fixtures, offline controls and native cargo test bridge; every active workflow byte-identical to ac06293d | Existing gate and existing seven-worker baseline |
| 2 | `codex/1465-baseline-cutover-20261018` | Baseline admission only; gate remains byte-identical to old main | Existing gate and old protected-base seven-worker baseline |
| 3 | `codex/1465-gate-cutover-20261018` | Gate admission and cheap admission-contract workflow | One explicit selected request starts the new gate and already-installed new protected baseline |

The native cargo bridge explicitly selects installed lanes: none, baseline,
then baseline+gate. Other lanes use dormant exact fixtures. It runs the same
28 offline helper/shell controls in the current required cargo test population.
Installed workflow bytes must exactly match their prepared fixtures. Fixtures
are not Actions workflows and cannot admit hosted execution.

The helper bytes must stay identical through all three PRs. Current SHA-256:
`d1ed65a761b7285c833a753f9ff7b4dbba2f2a463461aebde2304f26f0eab81c`.
PR1 must install the independently reviewed exact adapter before either cutover.
After each protected normal merge, read actual main and landed helper/workflow
blobs, compose the next PR on that actual base, and obtain fresh exact-head and
composition review. Prepared local descendants are not landed-base receipts.

## Why the smaller two-PR sequence is not selected

Helper-only followed by both workflows would cost a minimum of two gate and two
baseline populations on a first-attempt success. It breaks the missing-source
dependency, but the baseline on that combined cutover still runs the old
opened-event definition. It is bootstrap evidence, not new baseline admission
or retry evidence, and supplies no labeled protocol predecessor. If the new gate
fails, normal successor retry cannot reference a valid baseline predecessor.
Do not rerun it blindly, fabricate source/review changes or treat the old baseline
as a new protocol receipt. That two-PR alternative must stop at this exact failed
bootstrap and would need another reviewed bootstrap recovery mechanism.

Installing baseline admission before gate admission avoids that exception. PR2
uses the unchanged old gate and old protected baseline, including their existing
native failure/recovery semantics. Inspect any failed result before an explicit
legacy recovery; do not fish for green. PR3's initial explicit request has genuine
labeled predecessors in both populations, so ordinary terminal retry and
post-success withdrawal/reinstatement work without a bootstrap special case.

## Cost and native witnesses

For PR1 and PR2, prepare locally, review exact head, then open each selected PR
late and non-Draft if that entry format is authorized. No extra push or Ready is
included in the minimum. Each opening spends one old gate plus one seven-worker
baseline. PR3 may open Draft: both its candidate gate and the installed protected
baseline observe opening, push and Ready cheaply. After final source/review,
the separate explicit request spends one new gate plus one new baseline.

Thus the selected sequence's first-success minimum is **3 gate + 3 baseline
populations** (21 baseline workers), before any deliberate lifecycle test or
genuine failure recovery. No Draft/Ready double charge is assumed. Choosing Draft
then Ready for either legacy PR adds another old gate+baseline pair per such
transition; do not do that implicitly.

On PR3, retain native cheap opened/Draft/synchronize/Ready and invalid/stale-request
results before the selected positive request. Duplicate/replay tests allocate no
new proof. For a deliberate same-head lifecycle rehearsal, qualify once, withdraw,
request reinstatement, cancel that admitted attempt, then explicitly retry the
terminal result to final qualification. This budgets **two additional pairs**,
charging the cancelled pair as potentially full: total **5 gate + 5 baseline
populations**. It shares the three-generation bound and preserves all failed,
cancelled and retired receipts. If hosted duplicate delivery cannot be induced
through supported controls, retain that limitation rather than inventing an event.
No execution-time or twenty-minute acceptance follows from this budget.

The PR3 baseline is genuine protected-definition admission evidence because PR2
has already landed; it is not the old-baseline bootstrap result. Read required
contexts, exact heads, workers, artifacts and terminal outcomes before normal
protected integration. Post-landing everyday-event checks can use the next
selected objective; do not create an extra expensive acceptance PR by default.

## Authority and current stopping point

No credentials, token scopes, protection, settings, release or deployment changes
are needed for this sequence. Implementation, branch publication, local checks
and exact-head review are within the requested CI configuration work. Ordinary
protected merges, when the parent-held conditional merge authority applies and
all conditions pass, do not need an invented per-PR security approval.

The present delegation explicitly holds PR opening, hosted CI and merges. Those
actions are not executed. The actual outstanding decision is the one-time legacy
qualification budget/entry format if it has not already been authorized, and
lifting this source-planning execution hold. It is not an unspecified need for
new access. The release/ci lead retains review, qualification and integration
judgment under the existing authority. Main remains unchanged until that sequence
is separately executed and read back.
