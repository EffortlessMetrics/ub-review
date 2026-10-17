---
name: deliver-change
description: Carry a repository change through implementation, review, selected-candidate qualification, authorized merge and reconciliation; use again when continuing delivery work.
---

# Deliver a change

Read `AGENTS.md`, `docs/development/AGENTIC_WORKFLOW.md`, `CLAUDE.md` and the
relevant policy receipts. Reuse their commands and boundaries. This skill is
procedure only: obtain the live assignment, ownership, permissions and stopping
condition from the conversation and current native issue/PR state.

## Establish and replace scope

Inspect the current branch, clean/dirty state, exact base/head, issue/PR body,
recent discussion, equivalent candidates and direct dependencies before editing. Use your own candidate
branch or worktree; preserve other owners' commits and uncommitted work.

When a new assignment explicitly replaces an old one, rewrite the active goal,
deliverables and stopping condition around the replacement. Mark prior work as
superseded in the task handoff; keep its receipts as history. Cancel obsolete
task-owned queued work where cancellation is authorized and redirect delegated work before it continues. Do not keep old
deliverables active simply because they appeared earlier. Steering that adds a
constraint or asks for status preserves the goal unless it explicitly replaces
it. Ask only when that distinction changes a concrete next action and cannot be
resolved from the instruction.

## Continue through checkpoints

Implementation, a passing focused test, a recovery commit, draft publication,
review feedback and a pending hosted check are checkpoints. At each checkpoint,
compare completed work with the current requested outcome. Perform remaining
authorized work: finish relevant validation, repair demonstrated failures,
publish an understandable candidate when requested, reconcile review concerns,
inspect qualification and complete the authorized merge and reconciliation.
Use an exact-revision handoff for an actual interruption or explicit stopping
boundary, not as a default endpoint for an implementation goal. A status question merits a brief answer
followed by continued work. A blocker holds only its dependent step; continue
useful independent work. If nothing authorized remains, record the blocker,
owner, resume condition and next step and stop or wait. Do not invent busywork.

Run the cheapest relevant proof first, then the native baseline in `CLAUDE.md`
and staged `cargo xtask precommit --staged` where applicable. Preserve failures,
missing tools, skips and policy warnings. Never change checks, thresholds,
credentials, deployment behavior or access boundaries to manufacture success.
After source changes, invalidate affected prior checks and reviews and rerun
against the final candidate. Scope relevant base drift before integration.

## Select before qualification

Compare alternative candidates with focused development proof and review before
PR publication while opening even a Draft still triggers hosted qualification.
After the explicit-request cutover, alternative Draft PRs may remain cheap.
Select the candidate intended to land before expensive pre-merge qualification;
unselected alternatives do not each get a full CI run. Selection is per objective,
not a repository-wide max-one queue. Independent objectives may proceed in parallel.
A lead may delegate bounded implementation and review while retaining integration
ownership; one writer mutates each candidate at a time.

Read the actual workflows and the transition guide before publishing or requesting
CI. The late explicit-request path remains subject to #1465 cutover; this skill does
not activate it, rename a label or weaken the installed gate. Where ordinary PR
publication still triggers CI, publish the selected converged candidate rather
than using broad hosted runs to debug each alternative. Do not claim Draft itself
prevents execution, or invent a request command that does not exist.

The normal target is complete qualification execution within 20 minutes, with
queue time separate. Never reduce proof, hide setup or change a timeout to assert
that target. Reconcile any existing run before a new request. A changed candidate
cannot inherit a retained request label or a sibling PR's green status.

## Publish evidence in native state

Use the existing PR template, draft PR body, substantive review threads, check
runs and their artifact links. Keep the body centered on the current change:
problem/result, scope, exact base/head/tree, commands and terminal outcomes,
policy receipts, known gaps and remaining integration conditions. Update it
when scope changes; label historical receipts as superseded. Prefer durable run,
job, review and artifact links; a local path needs retained content/digest and
must not be represented as accessible hosted evidence. Keep status chatter out
of review output. Do not add a second ledger, coordinator or status service.

Read back the published branch/head and PR state. Distinguish local validation,
independent review, hosted required checks and publication confirmation. A
successful write request alone is not a readback receipt. Read formal review
state through authoritative native reads. A failed, denied or incomplete
read leaves the unread state unknown; a public page with no visible reviews
does not prove formal reviews are absent. Record the exact failed surface and
use only the state actually returned. Preserve access denials; do not change
credentials or permissions to turn unknown into a clearance claim.
Inspect all review surfaces: submitted reviews, inline threads including outdated
unresolved threads, and ordinary Conversation comments. Disposition every material
finding against the current candidate: repair and test, refute with source evidence,
or record a justified separate follow-up without leaving a known merge blocker.
Reply before resolving a thread; read back both operations. A resolved flag, stale
clean review, provider quota skip, truncated read or zero visible threads is not a
substantive clean review. Do not discard a valid later finding after an earlier pass.

Draft, Ready, merge and release are distinct transitions: obey live authorization and
repository protections for each; do not infer one from another. A green
aggregate or local baseline does not waive a failing required check.

## Finish the selected candidate

Retain the same useful owning lead through a normal in-window CI wait. Use a
supported wait/event facility or bounded checks; do not spawn a tight polling
child, automatically discard context or require another session merely because
CI is running. A real interruption preserves the exact head/run/attempt, evidence,
findings, owner and next action; resume that transaction without duplicate effects.

After CI, inspect actual terminal jobs, commands, selected populations, artifacts
and the required result on its admitted revision. Skipped, zero-subject, partial,
failed or wrong-revision evidence is not green. An independent baseline pass does
not replace `ub-review/gate`. Classify red evidence and repair its cause or route
a precise upstream issue; do not rerun an unchanged failure to fish for green.

Make the final judgment against the live head, substantive findings, applicable
proof, current integration state and product objective. Main advancing alone does
not require a full restack; refresh affected evidence for actual semantic changes.
If the task delegates ordinary integration and conditions pass, perform the normal
protected expected-head merge and read GitHub back to verify the landing. Do not
stop at green to seek an unrequired approval. No auto-merge or merge queue is added.

Reconcile the landed result with the controlling issue and residual acceptance.
Do not repeat the full CI suite after every merge; incident, periodic main-health
and release proof have their own owners. Keep explicit Draft-only/review-only holds,
required-check failures, unavailable access and reserved release/settings/credential
authority intact. A genuine blocker names its evidence, owner and resume condition.
Keep live task status and permissions in the native PR, never in this reusable skill.
