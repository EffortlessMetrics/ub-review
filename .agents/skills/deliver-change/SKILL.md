---
name: deliver-change
description: Carry a repository change through implementation, validation, draft publication and an exact-revision handoff; use again when continuing delivery work.
---

# Deliver a change

Read `AGENTS.md`, `docs/development/AGENTIC_WORKFLOW.md`, `CLAUDE.md` and the
relevant policy receipts. Reuse their commands and boundaries. This skill is
procedure only: obtain the live assignment, ownership, permissions and stopping
condition from the conversation and current native issue/PR state.

## Establish and replace scope

Inspect the current branch, clean/dirty state, exact base/head, issue/PR body,
recent discussion and overlapping owners before editing. Use your own candidate
branch or worktree; preserve other owners' commits and uncommitted work.

When a new assignment explicitly replaces an old one, rewrite the active goal,
deliverables and stopping condition around the replacement. Mark prior work as
superseded in the task handoff; keep its receipts as history. Cancel obsolete
queued work and redirect delegated work before it continues. Do not keep old
deliverables active simply because they appeared earlier. Steering that adds a
constraint or asks for status preserves the goal unless it explicitly replaces
it. Ask only when that distinction changes a concrete next action and cannot be
resolved from the instruction.

## Continue through checkpoints

Implementation, a passing focused test, a recovery commit, draft publication,
review feedback and a pending hosted check are checkpoints. At each checkpoint,
compare completed work with the current requested outcome. Perform remaining
authorized work: finish relevant validation, repair demonstrated failures,
publish an understandable draft when requested, reconcile review concerns and
prepare the exact-revision handoff. A status question merits a brief answer
followed by continued work. A blocker holds only its dependent step; continue
useful independent work. If nothing authorized remains, record the blocker,
owner, resume condition and next step and stop or wait. Do not invent busywork.

Run the cheapest relevant proof first, then the native baseline in `CLAUDE.md`
and staged `cargo xtask precommit --staged` where applicable. Preserve failures,
missing tools, skips and policy warnings. Never change checks, thresholds,
credentials, deployment behavior or access boundaries to manufacture success.
After source changes, invalidate affected prior checks and reviews and rerun
against the final candidate. Scope relevant base drift before integration.

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
successful write request alone is not a readback receipt. Inspect substantive
review comments and resolve each against the current head with proof. Draft,
Ready, merge and release are distinct transitions: obey live authorization and
repository protections for each; do not infer one from another. A green
aggregate or local baseline does not waive a failing required check.

Finish with exact candidate evidence, what is established, each remaining
integration condition and who owns the next step. Keep live task status and
permissions in the conversation/native PR, never in this reusable skill.
