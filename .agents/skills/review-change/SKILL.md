---
name: review-change
description: Independently review an exact candidate revision and reproduce or experiment in an isolated checkout without changing the author's branch.
---

# Review a change

Read `AGENTS.md` and `docs/development/AGENTIC_WORKFLOW.md`. Obtain the current
assignment, base/head, changed paths, intended contract and raw receipts from
native PR state and the conversation. Inspect substantive threads before
claiming technical clearance. Author self-review is not independent review.
Failed or incomplete authenticated review reads leave formal review state
unknown; absence on a public page is not an authoritative empty review result.
Carry that uncertainty into the handoff without evading the failed access
boundary or treating it as a clean review.

## Use an isolated lab

Reviewers may run focused reproduction, negative controls and temporary source
experiments within the assigned review boundary. Use a separate detached
worktree at the exact candidate SHA, for example
`git worktree add --detach <review-dir> <candidate-sha>`, or an isolated clone
when worktrees are unavailable. Verify its HEAD before execution. Never mutate
the author's checkout, index, branch, receipts or another reviewer's lab.
Respect checkout trust, secrets, network, resource and heavy-witness policy;
isolation grants no new execution or publication authority.

Use a separate Cargo target directory for source experiments so cached author
or sibling binaries cannot masquerade as candidate proof. Record pristine tree
identity, command/configuration, observed outcome and any experiment diff. A
mutant must compile and exercise the real relevant path before its rejection
counts as discrimination evidence. Restore the pristine source and rerun the
focused positive control. Experimental changes stay in the lab unless the
owner explicitly accepts a scoped fix. Clean up only your own worktree when
the evidence is retained and no consumer needs it.

## Return bounded findings and evidence

Report material findings with exact path/location, concrete trigger, observed
failure, implication and bounded fix/proof route. Report a disproved concern
with the reproduction evidence. Bind clearance to exact base/head/tree and
the checks actually exercised, with missing proof explicit. Do not substitute
an older review, newer approval or unrelated green checks for resolution of a
substantive concern. A changed head needs a relevant fresh review.

Return results to the lane owner. Publish substantive native review feedback
only when authorized; do not add per-lane status comments or a parallel review
ledger. The owner retains final staging, validation, publication, integration
and cleanup decisions under the native procedure. Keep current assignments,
approval decisions and live task status outside this skill.
