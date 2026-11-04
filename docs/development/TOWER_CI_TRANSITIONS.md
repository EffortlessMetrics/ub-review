# Tower review and full CI: agent transition guide

Use this guide with [the native workflow](AGENTIC_WORKFLOW.md) and the delivery
or review skill. It describes evidence and handoff boundaries; it does not
activate a mode, amend architecture, grant execution/publication authority, or
change required checks. Live assignments and holds stay in conversation and
native issues/PRs.

## Select the claimed result before starting

Here **Tower review** means the adjacent source-only advisory review path
owned by [em-ci#218](https://github.com/EffortlessMetrics/em-ci/issues/218)
and [#1334](https://github.com/EffortlessMetrics/ub-review/issues/1334),
[#1335](https://github.com/EffortlessMetrics/ub-review/issues/1335), and
[#1338](https://github.com/EffortlessMetrics/ub-review/issues/1338).
It is a scope description, not a new CLI flag or assertion that an operated
service is qualified. **Full CI** means the integrated deterministic proof and
review path under [#945](https://github.com/EffortlessMetrics/ub-review/issues/945).
Source-only advisory review, the installed advisory release, and integrated
CI have separate acceptance and authorization.

| Question | Tower source-only review | Full CI / integrated review |
| --- | --- | --- |
| Subject | Admitted source, spec/pass and exact evidence identities | Admitted revision plus actual execution identity, configuration and policy |
| Work | Source investigation, recorded evidence, bounded claim reconciliation | Applicable deterministic obligations and approved targeted proof; model investigation only under its own policy |
| Proof request | Inert, claim-bound next-evidence need; no executable argv or dispatch | Resolve through approved task/budget/resource admission; retain actual terminal receipt |
| Completion | Applicable source programmes settled or explicitly limited/unavailable; valid inputs and actual final-lead answer required for semantic clean | Required evidence settled under the current integrated contract; retain missing/failed/stale outcomes and protected lead/wind-down safeguards |
| CI result | No CI sufficiency decision owned here; imported CI stays read-only with original identity, bytes/digest and result | Current enforced compatibility result remains distinct from the target FinalizedOutcome contract |
| Publication | Immutable public selection plus separately observed delivery; prepared is not confirmed | Review publication, CI sufficiency and instrument coverage remain separate results |

A clean Tower review does not mean tests passed or authorize merge/release.
Unsupported material necessary for the declared review scope prevents clean.
Do not encode absence of CI ownership as an empty Required set yielding PASS.
A formatter or concatenated specialist output cannot replace a missing final
lead. A fresh requested same-head review is a new pass; an earlier clean pass
cannot suppress its findings.

## Move evidence across modes without promoting its authority

1. Refresh main, instructions, the exact base/head/tree, native issue/PR state,
   substantive threads, overlapping owners and current holds. Select a disjoint
   candidate or contribute independent evidence to the existing owner. Keep
   other worktrees, security research and active resources intact.
2. Declare mode, repository/PR, source/spec/pass/attempt identities, intended
   decision and allowed capabilities. Separate candidate head, reviewed commit
   and synthetic merge result; a symbolic ref is insufficient. Admit complete
   inputs through the canonical owners; do not invent a second judgment engine.
3. For Tower-to-CI, import review as advisory context, then independently admit
   execution and applicable deterministic obligations. Never reuse source clean
   as an executed proof receipt. For CI-to-Tower, verify the imported receipt's
   source/execution identity and digest, preserving failure, not-proven and scope.
   Stale evidence remains historical. More executed evidence may change judgment.
4. Only in admitted executable mode, before local Cargo, measure live CPU, RAM,
   commit headroom and target-disk space; identify active resource ownership
   and reserve task-owned resources.
   Choose concurrency from capacity and the whole invocation, including outer
   builds and descendants. No blanket serial rule. Reuse only verified immutable
   artifacts with compatible source/toolchain/lock/config identity; isolate
   source experiments and mutable targets from sibling work.
5. For executable proof, run cheapest relevant real proof and discriminating
   controls first. Record exact commands, effective executable/tool versions,
   configuration, actual outcomes and limitations. A control that never compiled or reached the
   intended consumer proves no rejection. Restore pristine inputs and rerun
   the positive control. Use the native CLAUDE.md baseline and staged precommit
   as applicable; do not substitute a green proxy for required qualification.
6. Challenge the exact candidate from relevant different angles and reconcile
   material findings. Prefer the same useful session or a fork under the review
   skill; a separate judgment does not require a fresh actor. Bind carried-forward
   evidence to its original subject and explain applicability. A changed source,
   policy, execution identity or model answer invalidates the affected proof;
   it does not erase unrelated historical proof.
7. Before publication, inspect workflow triggers and side effects. Before Ready
   or merge, inspect the current PR's checks, their available contexts and App
   identities, reviews, integration state and applicable holds. Use ordinary PR
   operations and let GitHub enforce its protections; routine delivery does not
   require reading branch administration settings. A genuinely required but
   unavailable validator or review read blocks the dependent decision, not an
   otherwise independent authorized publication. Do not bypass a required check
   or protection. At handoff name the
   next owner, remaining evidence and resume condition. Read back every
   authorized publication against the expected SHA.

## Decide the next transition from current evidence

Ready-for-review is a native review state, not an independent CI admission
decision or proof that qualification ran. In the current workflows it is also
a trigger, so marking Ready can launch broad CI. Check the actual event and
side effects before acting; do not infer permission to run CI from permission
to request review. Apply the same check to draft creation and repair pushes.

| Observation or request | Supported next action | Claim that remains unestablished |
| --- | --- | --- |
| Local checks and source review complete; expensive qualification held | Retain the reviewed branch and receipts; name the owner/resume condition for any publication that would trigger held work | Draft or Ready does not make the event cheap or authorize qualification |
| Ready requested and its current CI side effects authorized | Refresh head/base, reviews and live workflows; perform the authorized transition once, then read back state and check identities | Ready itself does not establish an executed or passing check |
| Wrapper/check green, `conclusion = pass`, `gate_result = not_proven` | Retain both results and the raw `not_proven_reasons`; investigate the missing evidence under existing admission policy | Compatibility success does not establish evidence sufficiency |
| Wrapper green but current receipt missing, unreadable or for another revision | Record unavailable or historical evidence with its original identity; obtain the current receipt | Green alone cannot establish current evidence or clean review |
| Prepared review or run-stage `publication_result = posted` | Read the separate post receipt and remote PR surface against the selected payload/head | Preparation does not confirm delivery |

For current evidence, read `review/gate_outcome.json`: `revision`, `conclusion`,
`analysis_result`, `publication_result`, `gate_result` and
`not_proven_reasons`. The Action exposes the corresponding hyphenated result
outputs and `gate-outcome-path`. [The output reader](../../action.yml) reports
`not_proven` for missing/unreadable evidence; its successful execution is not
proof of a pass. [Gate enforcement](../../src/gate.rs) still consumes the
legacy `conclusion`. Neither `gate_result = pass` alone nor an empty reasons
list replaces applicable Required receipts, current native checks or reviews.
Do not mechanically rename these compatibility fields to the proposed
FinalizedOutcome contract; see [product state](../PRODUCT_STATE.md).

These are consumer and handoff rules over existing surfaces. They do not add a
Ready interceptor, late-CI command or operated Tower service. #1465 owns dormant
native admission; #987 owns Required scheduler migration; #1015 owns enforcement
migration. Keep those implementations and their qualification with their owners.

## Selected lifecycle and the source-to-installed boundary

The selected sequence is implementation, checks and substantive review in the warm
owning session, conditional landing authorization for exact H, native enqueue,
qualification of actual G, protected landing M and readback. Draft, Ready,
ordinary updates, labels, reviews and comments allocate no Actions workflow in
the target; a cheap wrapper or skipped job does not satisfy that condition.
There is no routine main replay or second approval after a successful G. Current
source and settings must establish each claim; selection does not prove activation
or the normally <=20-minute complete execution target.

At main `3dbcc2be9768b082c0896dc2360e950ffbd56a65` (2026-10-15 observation),
[gate](../../.github/workflows/ub-review-gate.yml) runs on opened, reopened,
Ready and synchronize events; [independent baseline](../../.github/workflows/independent-baseline.yml)
uses the corresponding protected-base events and seven evidence populations.
Neither trigger excludes drafts. Opening a draft can start broad proof during
development; pushing a repair to an existing PR can do the same and cancel
predecessor work. Refresh these files before every publication decision.

[#1465](https://github.com/EffortlessMetrics/ub-review/issues/1465) owns the
existing admission work. Its former label-driven, hosted-observation design is
historical preparation; it is not the selected steady-state front door. Keep the
existing helper and evidence while joining the following actual gaps:

- `ub-review/gate` requires Actions App 15368. A different App's same-name H check
  cannot satisfy that binding. Qualify the existing trusted publisher's phase-
  explicit H admission/G qualification aggregate and coordinated protection.
- `revision_admission.rs` now admits an explicit `merge_group` subject with exact
  B/G commits, actual B-to-G diff, a clean G checkout and no PR head. Canonical
  artifact validation preserves that identity and PR delivery refuses it. Actual-G
  reads and diff construction disable replacement objects and isolate inherited
  Git repository/index/configuration variables. A fresh private index from G
  checks working bytes without inheriting assume-unchanged, skip-worktree or sparse
  flags, and leaves the caller index unchanged; canonical readers require exact LF
  normalization, including when a forged digest is recomputed. This
  object proof does not itself establish native queue membership or authority.
- The independent baseline obtains its fixed command authority from protected
  `pull_request_target` source. Moving its event to `merge_group` alone would make
  that definition candidate-owned. Prepare and qualify an immutable protected
  reusable definition before the caller changes, retaining all seven populations.
  The additive callable path accepts exact event-bound B/G and records its pinned
  definition separately from caller G. Its fresh no-checkout finalizer reads the
  exact native run/attempt and reusable reference with an Actions-read token.
  The native run-level reference alone does not establish job-source custody: the
  publisher must also bind the reviewed caller and its complete producer closure.
- Preserve the complete contained model-off gate and independent baseline. Their
  apparent overlap is not receipt equivalence and does not justify proof removal.
- Native acceptance must show zero development allocations, stale-H refusal,
  complete actual-G proof, a failed required obligation blocking landing, duplicate
  request reconciliation and M custody. Rollback restores full PR qualification
  before queue-only enforcement is removed.

The staged final catalog has one native `merge_group: checks_requested` caller,
`queue-qualification.yml`, and no ordinary PR/push/review/comment/label roots.
It joins the complete contained model-off gate, the immutable seven-population
baseline at `bf077a0178c3fe159927a6ea39ee274050a92e10`, and the bootstrap contract.
The contained gate definition is also pinned to that reviewed preparation SHA:
G code cannot move the telemetry OIDC permission into candidate execution before
the publisher checks source generation. Its checkout and analysis still use G.
The internal Actions `ub-review/gate` finalizer requires every family to succeed;
missing, skipped, cancelled and failed results cannot pass. `xtask policy-check`
rejects all reintroduced ordinary roots, including cheap or skipped wrappers.
The literal preparation SHA is published on `ci/1465-merge-group-source-20261104`;
its reviewed callable definition must remain reachable. An arbitrary mutable
branch/tag or a caller-supplied SHA is not an independent definition.

This is coordinated activation source, not installed enforcement. Before landing
it, qualify the existing Hermes publisher's `CI Lifecycle Qualification` context
with its actual accepted App ID on H and G, complete native run/attempt/job and
producer-closure binding, and queue-only enforcement. The currently Actions-bound
`ub-review/gate` cannot be fulfilled on H by another App copying that name. Leaving
an obsolete H Actions requirement in place blocks intake; replacing it without
full G qualification and queue-only enforcement permits underqualified landing.
Reconcile effective rulesets, classic protection and any settings-app source as
one controlled cutover. Preserve the old full protected route until that cutover
is executable, and restore it before disabling queue-only enforcement on rollback.
No required proof is removed by the new aggregate.
The release-tag
workflow and daily quality telemetry retain their named, separately measured work.
Do not copy a second admission implementation, activate dormant templates, broaden
permissions or substitute a source-only result for current enforcement.
If an active user hold keeps expensive qualification out of development and
current PR creation would trigger it, retain the reviewed branch/evidence and
report the blocked PR step to the coordinating owner. Draft status alone is
not admission control. Ordinary branch push safety also requires inspecting
all push/tag workflows; do not assume it from these two files.

## Evidence required at handoff

Retain evidence in the existing native PR/issue and linked artifacts, not a new
status registry. Each claim needs subject base/head/tree (and reviewed/merge
identity when applicable), mode, policy/configuration/tool identity, command or
observation, timestamp, terminal result, raw receipt/run/artifact link or retained
content plus digest, scope and owner. Local paths alone are not hosted evidence.

| Claimed transition | Minimum evidence and discriminating controls |
| --- | --- |
| Recorded/pure source qualification | Same complete admitted inputs and recorded model answer give the same semantic validation; missing/stale lead and required source gaps prevent clean; duplicate event is idempotent while fresh pass remains admissible |
| Standalone operated Tower eligibility | Explicit architecture decision plus qualified engine/host/broker/transport and applicable operation authority; real consumer rejects proof dispatch, candidate execution, gate/check/status writes and privilege minting before side effects; provider/judgment/delivery perturbations leave imported deterministic bytes/digest/result unchanged |
| Integrated CI sufficiency | Exact applicable Required obligations and terminal receipts; success plus failure/stale/forged/partial/cancellation controls as relevant; imported review/provider/publication changes cannot manufacture deterministic sufficiency; preserve current review_forward policy exception pending its owners' migration |
| Confirmed publication | Identity-bound remote readback/receipt, immutable selected body/anchors and attempt identity; accepted-submit/lost-response control remains unknown/reconciling until observation resolves it; absence of a receipt never proves absence of remote effect or safe retry |
| SDLC admission/cutover | Hosted exact-head/base/context/App association, fresh-intent/withdrawal/draft/closure/retarget/review and delayed-event cases; failed/skipped worker and cancellation before/during finalization controls; active-run preservation, terminal accounting, measured whole-portfolio duration and independent review; exact authorization and rollback receipt before cutover |

The fixture/control descriptions above are expectations, not tests executed by
this document. Keep source-only negative proofs separate from hosted native
plumbing and from real product workload qualification.

## Preserve unknown states and existing owners

- [#1349's decision packet](../JUDGMENT_MODE_DECISION_1349.md) remains proposed
  until its explicit ruling and mode-specific proof exist. Pure recorded work
  does not need to wait for an operated host; it cannot authorize activation.
- [Product state](../PRODUCT_STATE.md) distinguishes implemented, wired,
  retained-run proof, trusted authority and external calibration. None implies
  the next. Current compatibility gate-result and gate-conclusion are not a
  mechanical mapping to target PASS/FAIL/NOT_PROVEN. #1015 owns enforcement
  migration; #1277 owns result-plane non-interference. #1464 separately owns
  documentation/tests for the current opt-in review_forward exception.
- #1465's 2026-10-15 sandbox jobs failed to start due to the reported account
  scheduling/billing condition. Provisioning failure is no admission/finalizer
  test evidence. Hosted behavior, full protection association and timing remain
  unproven until retained execution demonstrates them. Refresh the issue's
  receipts rather than treating this dated observation as permanent status.
- Failed or incomplete authenticated reads leave reviews, protection details or
  delivery unknown. Public absence, a green aggregate, issue closure or local
  fixtures cannot clear those gaps. Preserve the failed surface and exact
  response without inferring its cause or changing access to evade it.
- #945/#1216 retain sequence/registry ownership; #1334/#1335/#1338 retain pure
  source-only contracts; #1013/#1014 retain canonical judgment/compiler;
  em-ci retains operated Tower ownership. This guide grants no activation,
  deployment, release, production App, credential or settings change.
