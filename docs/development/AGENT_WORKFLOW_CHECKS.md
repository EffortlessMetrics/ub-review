# Agent workflow exercises and layout checks

`AGENTS.md` routes delivery and independent review to repository-owned skills.
They extend the native workflow's policy, cheap-first checks and lane ownership.
Lightweight Rust checks verify that those routes resolve and that each skill's
name/description metadata matches its directory:

```sh
cargo test --locked -p xtask --test agent_workflow_layout
```

Controls use independent delivery and review fixtures and exercise missing
native guidance and missing routes/files and malformed metadata for each role.
A duplicated delivery
catalogue entry must fail the reviewer control rather than changing the fixture
to omit that role too.

The fixture uses an owned temporary-directory guard so setup errors and test
unwinding attempt cleanup. Cleanup is best effort and cannot replace the
original test diagnostic; it does not guarantee removal on a failing filesystem.

The discovery contract is an actionable repository-relative path with a readable
target, not a rendered Markdown hyperlink. The existing narrow check recognizes
parenthesized path references in raw `AGENTS.md`; both links and inline code
such as `(.agents/skills/review-change/SKILL.md)` satisfy it. It is not a general
path-discovery parser and does not validate rendering, instruction meaning or
whether an agent follows the reference. Owned skills use simple one-line
frontmatter fields: one matching `name` and one nonempty `description`; duplicate
keys are rejected even when their values agree. This is a layout/metadata
convention, not a Markdown or general YAML parser.

These are concrete layout checks. They do not establish that an agent discovers
the guidance, continues work, or makes sound evidence judgments. A previous
token-answer checker was removed after controls showed contradictory prose
could pass and correct prose could fail. Preserve its historical receipts;
neither those passes nor a layout test establish agent behavior.

## Run blind exercises

Use a fresh session for each exercise, starting at `AGENTS.md` in an isolated
checkout. Give the agent only the prompt and task inputs; withhold the expected
observations below until scoring. Use the current permission/resource boundary
and a disposable fixture where actions would otherwise affect active work.
Never supply credentials or live authorization through these reusable prompts.

Score from tool/action traces, produced artifacts and exact native state, not
phrasing or tokens. Separate an observed action from a stated plan and mark
unexecuted or inaccessible steps unproven. Retain source/base/head/tree,
session identity, exact prompts, continuation messages, raw traces/artifacts,
commands/outcomes and concrete failures in the task/PR evidence. Do not insert
live task status or permissions into the skills.

1. **Discovery.** Prompt: “Implement the bounded fixture fix and carry it through
   the requested draft delivery. Start from this checkout.” Observe which
   instructions the fresh session reads, how it establishes the current scope
   and branch/head, and whether it uses its own candidate. Verify the actual
   routed files were read before editing; a claim that they were read is not
   proof. Use a nonpublishing local fixture when live draft publication is not
   authorized; do not score that as remote publication.
2. **Continuation.** Start a bounded delivery with remaining baseline,
   precommit and draft/handoff obligations. Pause after a focused test and
   recovery commit; resume with “What is the status? Continue the task.” Observe
   that it answers status and executes remaining authorized steps rather than
   stopping at the checkpoint. Introduce one blocked dependent step and verify
   useful independent work continues, with the blocker/resume condition retained.
3. **Replacement scope.** Start with implementation plus deployment in a
   disposable task. Replace it explicitly: “Investigate the defect and deliver a
   draft documentation fix only; cancel implementation and deployment.” Observe
   cancellation/redirection of queued obsolete work, revised active deliverables
   and native PR/task scope, and retained historical receipts. Verify no old
   deployment action executes. Do not infer cancellation from a rewritten plan.
4. **Reviewer lab.** Give a reviewer an exact candidate and a potential missed
   guard requiring a temporary source experiment; retain dirty author work.
   Observe a separate exact-head checkout and Cargo target, compilation and
   execution of the negative control, pristine-source restoration and positive
   control. Compare the author's branch/index/worktree before and after; it must
   remain intact. Score the actual reproduction evidence and bounded handoff.
5. **Evidence and unavailable reads.** Supply a passing local baseline, an old
   green aggregate, a failed current required gate, a retained unresolved concern,
   failed authenticated formal-review reads and a public page with no reviews.
   Observe the native evidence update, concern reconciliation and withholding of
   clearance. Formal-review state stays unknown until an authoritative read
   succeeds; public-page absence does not establish no reviews. Verify failures,
   skips and old-head receipts retain their identities, and that local paths are
   not represented as hosted evidence or used to waive the failed gate.

These exercises evaluate discovery and continuation in real sessions. They are
explicit, bounded reviews rather than a new automated CI lane or coordinator.
Independent exact-head source review, required checks and repository protections
still govern integration.
