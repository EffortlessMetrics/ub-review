---
name: review-change
description: Challenge an exact candidate through differentiated review; select source-only inspection or authorized isolated reproduction before execution.
---

# Review a change

Read `AGENTS.md` and `docs/development/AGENTIC_WORKFLOW.md`. Obtain the current
assignment, base/head, changed paths, intended contract and raw receipts from
native PR state and the conversation. Treat the stated problem and proposed fix
as revisable claims: check the load-bearing premise against the relevant spec,
source and actual consumer. Record corrections and their reasons without changing
the authorized goal. Inspect substantive threads before claiming technical
clearance.
Failed or incomplete authenticated review reads leave formal review state
unknown; absence on a public page is not an authoritative empty review result.
Carry that uncertainty into the handoff without evading the failed access
boundary or treating it as a clean review.

## Select mode before checkout, build or test

Choose the evidence needed and the allowed capabilities before acquiring or
executing candidate material:

- **Source-only:** inspect admitted source as data and imported receipts under
  [the Tower boundary](../../../docs/development/TOWER_CI_TRANSITIONS.md). Do not
  execute candidate code, hooks, filters, build scripts, tests, local actions or
  project configuration, or dispatch proof from this mode. A source-only clean
  result is not executable proof or CI qualification. If execution is needed,
  retain a bounded proof request for the authorized execution owner.
- **Executable reproduction:** use the already-authorized isolated coding/proof
  environment and its trust, secret, network and resource bounds. Admit the exact
  candidate and smallest useful experiment, then use the lab below. Isolation
  alone does not grant authority. An explicit source-only assignment remains
  source-only unless its execution boundary is separately authorized.

## Reuse context and vary the challenge

Prefer continuing the useful owning session or forking its context for successive
review angles. Self-review can supply a distinct judgment when the method can
falsify the candidate; merely repeating the construction rationale cannot. Ask a
different question, inspect a different consumer or use a different oracle,
counterexample or threat model. The same lead may judge readiness. Use another
session when fresh context is likely to find something worth its additional cost,
or when an explicit review requirement calls for it; agent identity alone is not
independence of evidence.

Retain compatible prior evidence with its original identity and limits. Refresh
only affected claims when source, assumptions or integration context change, plus
any observations required to be current. A prior clean pass does not bar a useful
new angle or invalidate a later finding; stop when further passes add no useful
evidence, not because a fixed review count or actor count has been reached.

## Use an isolated lab for executable reproduction

In executable mode, reviewers may run focused reproduction, negative controls
and temporary source experiments within the assigned boundary. Use a separate
detached worktree at the exact candidate SHA, for example
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
substantive concern. A changed head needs review of the affected claims, while
unaffected evidence keeps its original identity; it does not require replaying
every prior check. For a behavior change, seek a control that fails on the old or
plausibly wrong behavior and passes on the candidate. Record a concrete limit
when that discriminator cannot be executed. Decide its recurring test tier by
signal and cost separately from establishing the initial red/green evidence.

Return results to the lane owner. Publish substantive native review feedback
only when authorized; do not add per-lane status comments or a parallel review
ledger. The owner retains final staging, validation, publication, integration
and cleanup decisions under the native procedure. Keep current assignments,
approval decisions and live task status outside this skill.
