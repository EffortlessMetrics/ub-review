# Architecture

`ub-review` is a revision-bound evidence control plane for PR review and
targeted CI. It coordinates deterministic evidence, bounded model
investigation, approved proof, editorial compilation, GitHub delivery, and a
machine-verifiable gate result over one admitted code state.

The product is not a generic review bot with extra tools, and it is not a fixed
CI matrix with an LLM summary attached. Its destination is one transaction in
which review and CI share evidence while retaining separate decision authority.

## Product invariant

```text
exact admitted revision
  -> repository contract + diff/world model
  -> one shared gate-and-review plan
  -> Required deterministic spine starts immediately
  -> relevant sensors, model programmes, and proof tasks share one scheduler
  <-> proof receipts update only the claims they test
  -> one final review judgment
  -> one concise grouped Pull Request Review
  + one eventual deterministic PASS / FAIL / NOT_PROVEN FinalizedOutcome
  -> retained revision/task/model/claim/delivery/outcome receipts
```

The final line uses the target semantics and serialization defined in the
[CI outcome vocabulary](PRODUCT_STATE.md#ci-outcome-vocabulary), not the
current additive `gate-result` vocabulary.

The authority rules are stable even where the implementation is still
migrating:

```text
models investigate
sensors observe
approved executors produce proof receipts
one task/resource authority controls execution
claim reconciliation decides what the evidence supports
final judgment decides what the review says
delivery receipts prove what GitHub received
finalized outcome decides CI sufficiency
a stable coordinator publishes the terminal check
```

Models do not prove correctness. A model finding is a claim until deterministic
evidence, repository facts, or accepted human authority supports it. Missing
Required evidence is never clean evidence.

## Current implementation boundary

The target architecture above is not yet the full live authority graph. Current
`main` is in a shadow-authority migration:

```text
RevisionIdentity
  -> shared diff/context and repository facts
  -> existing fast/late sensor pools
  -> existing proof broker, workers, leases, and budgets
  -> bounded model lanes and receipt-linked reconsideration
  -> claim graph and editorial compiler
  -> prepared GitHub review transaction
  -> post/reconciliation receipts
  -> gate_outcome

TaskLedger observes fast/late sensor execution in shadow through #1263/#955
proof and worker execution remain outside the ledger until #956
cross-projection reconciliation remains #957
FinalizedOutcome remains a shadow-first train
legacy gate_outcome.conclusion remains the enforced field
```

The immutable revision contract is already admitted and joined across core
current-run artifacts. The pure TaskLedger, execution accounting, ledger
artifact verifier, retained contradiction corpus, and sensor lifecycle adapter
also exist. The ledger now observes fast/late sensor execution, but it does not
yet observe proof/worker paths, reconcile all legacy projections, schedule
work, or deduplicate cross-source tasks. Until #956 -> #957 closes, the existing
pools, brokers, leases, and budgets remain execution authority.

The same distinction applies to gate truth. Additive `analysis_result`,
`publication_result`, and `gate_result` fields can state that a run is limited
or not proven even when the compatibility `conclusion` says pass. They are
run-stage diagnostics today, not yet post-confirmed delivery truth or production
enforcement authority. In particular, a prepared payload currently becomes
`publication_result = posted` before `post` runs, and post receipts do not
recompute `gate_outcome`.

Revision binding also has one concrete delivery gap: under `merge_result`
semantics, `revision.reviewed_commit` is the synthetic merge object, while the
GitHub pull API exposes the candidate-head object used for review delivery.
The current adapter compares those different objects. Candidate-head delivery
is retained; merge-result delivery authority remains a target contract.

[PRODUCT_STATE.md](PRODUCT_STATE.md) is the canonical earned-state matrix.
[Issue #945] owns implementation order.

## Authority map

| Decision | Current authority | Destination |
| --- | --- | --- |
| Reviewed code state | Admitted `RevisionIdentity` for core current-run joins; delivery exact-head authority is candidate-head only | The same identity contract preserves both reviewed and PR-head objects across every task, claim, receipt, delivery, reuse, learning, and stable/candidate comparison |
| What work exists | Multiple configured, impact, sensor, model, follow-up, and worker planners | One frozen `SharedRunPlan` compiled from repository, CI, diff, risk, and budget contracts |
| What runs next | Existing sensor pools, proof broker, worker paths, leases, and local budgets | One event-driven TaskLedger scheduler with canonical task/cache/deadline/resource authority |
| Whether a model claim is proven | Approved proof receipts and evidence precedence, where joined | Exact claim-to-proof-effect joins with no unrelated-claim spillover |
| What the reviewer sees | Editorial compiler plus transactional GitHub delivery | One typed final-lead judgment compiled to a bounded senior review |
| Whether GitHub received it | Post/reconciliation receipts exist separately from `gate_outcome` | Exact-revision confirmed delivery folded into FinalizedOutcome |
| Whether CI is sufficient | Legacy `gate_outcome.conclusion` | Finalized `ci_evidence_result` over current-revision Required terminal receipts |
| What future runs learn | Telemetry and calibration-v0 observations | Rebuildable, trust-classed learning with counterfactual evidence and explicit promotion rules |
| Who owns the required check | Candidate self-gate plus separate containment/baseline surfaces | Released stable coordinator with candidate shadow, rollback, break-glass, and terminal publication |

No projection becomes authority merely because it has a schema. Authority is
earned when production decisions consume it and verification rejects stale,
forged, missing, mixed, and contradictory inputs.

## Proof-in-the-loop review

The distinctive execution loop is not “run tools, then ask a model to summarize
them.” It is:

```text
material claim
  -> specialist reconstructs implementation, consumers, and failure boundary
  -> specialist emits a semantic proof intent
  -> Rust resolves the intent to an approved task identity and argv template
  -> the task competes in the evidence portfolio under resource/deadline policy
  -> the executor emits a revision-bound receipt
  -> only receipt-linked claims receive bounded reconsideration
  -> final judgment sees the resulting evidence state
```

This gives the reviewer a controlled lab without giving the model shell
authority. It also lets deterministic work start before every model call is
finished, which is required for the eventual critical-path advantage over
separate CI and cold-start AI review.

Current production has semantic intent resolution, approved focused tasks,
current-head receipt routing, red/green proof, and bounded reconsideration
substrate. It does not yet have one live scheduler, durable lane-completion
streaming for every path, or one authoritative final lead. Those are later
migration steps, not reasons to weaken the contract.

## One required check, many receipted tasks

“One required check” describes the repository-facing authority surface, not the
shape of execution. The coordinator may run local tasks, isolated jobs, or
future distributed workers, but each unit must have:

- canonical revision, execution, tool, and policy identity;
- explicit consumers (`Required`, `Detective`, or `Advisory` at the target
  architecture);
- bounded resource and deadline accounting;
- a terminal disposition and release state;
- retained stdout/stderr/result receipts where applicable;
- deterministic subsumption and reuse rules;
- a verifier-visible link to the outcome it can satisfy.

The terminal check summarizes those receipts. It must not erase the independent
proof units, turn optional work into Required work, or treat a coordinator exit
code as evidence sufficiency.

## Evidence phases and critical path

The current runner separates fast and late sensors. Fast sensors build the
initial deterministic context; late sensors can overlap model latency and join
before final compilation. That is a useful implementation technique, but
`fast` and `late` are scheduling metadata rather than separate lifecycle or
truth models.

The destination scheduler orders all work by authority and marginal value:

1. start the minimal Required spine immediately;
2. preserve finalization and retry reserve;
3. spend remaining capacity on the work most likely to change a material
   review or gate decision;
4. reuse equivalent current-revision work rather than rerunning it;
5. stop optional work safely when its marginal value no longer clears its
   remaining cost or deadline risk;
6. terminalize every admitted or declined task explicitly.

Until TaskLedger migration is complete, current pools must keep their existing
behavior and shadow adapters must expose disagreements rather than silently
normalize them.

## Review compilation and public surface

Sensors, specialist lanes, proof tasks, and workers never post independently.
They write artifacts. The compiler reconciles their claims, suppresses
adjudicated losers and internal machinery, preserves material summary-only
findings, validates inline anchors and suggestions, and prepares one grouped
review.

`ub-review run` prepares artifacts and a GitHub transaction. `ub-review post`
performs the side effect, revalidates the expected delivery subject, reconciles
comments, and writes success or failure receipts. Candidate-head delivery has
retained transaction proof. Merge-result delivery does not yet have the right
subject split: the adapter compares the synthetic reviewed commit with the
GitHub PR head, so that path cannot claim confirmed exact-head delivery.

The current `gate_outcome` is also compiled before `post`: a prepared payload is
reported as `publication_result = posted`, and `post-error.json` does not cause
it to be recomputed. **Prepared output is not delivered output**, and a finding
trapped only in artifacts cannot support a clean public-review result, are
therefore FinalizedOutcome rules owned by #959/#960—not current guarantees.

The public surface normally contains only:

- concrete findings tied to code, behavior, proof, or support boundaries;
- concise verification questions where material evidence remains missing;
- proof results that actually change the decision;
- a bounded decision and local residual risk.

Lane rosters, provider status, successful-tool inventories, planner narration,
raw command logs, and generic caveats belong in private artifacts.

## Mutation and trust zones

| Zone | Mutation / trust policy |
| --- | --- |
| Admitted source snapshot | Immutable for the run; identity is commit/tree/diff bound |
| Trusted base and candidate input | Explicitly distinguished; candidate-controlled configuration cannot silently acquire trusted authority |
| Sensor artifacts | Append/write once per task identity, then immutable |
| Model lane scratch | Private to the lane; never direct proof or posting authority |
| Task/event ledger | Append-only events; deterministic replay derives state |
| Proof worktree | Mutable only through an approved task and bounded lease |
| Running projections | Derived from canonical events/receipts; never independent authority |
| Review transaction | Prepared once from validated claims; side effects only through the GitHub broker |
| Delivery receipts | Append-only evidence of attempted and confirmed remote state; not yet folded back into `gate_outcome` |
| Stable terminal check | Published only by the trusted stable coordinator after exact-head verification |

Candidate-head dogfood remains model-off and artifact-only, with a separate
base-owned deterministic baseline. This containment is evidence about one
boundary; it is not yet full hostile-head-safe execution or stable-coordinator
proof.

## Provider and programme boundary

Providers are execution backends, not the architecture. MiniMax and OpenCode
are current direct-provider options; future approved backends may differ by
stage, latency, cache semantics, context, and cost. The control plane must retain
identical revision, task, model-stage, claim, proof, and outcome observability
across them.

Likewise, a fixed wall of named review lanes is transitional. The destination
is repository-native programme selection from explicit architecture, ownership,
support, mirror, CI, proof, and risk contracts. A fast router may refine a
deterministically approved programme set, but it may not invent authority,
hide Required work, or make an unreceipted omission look learned.

## Sensor ownership boundary

`ub-review` owns admission, orchestration, routing, proof brokerage, claim
reconciliation, review compilation, posting, fallback, and gate/outcome
contracts. It should not silently absorb analyzer defects into permanent local
glue.

For Rust/native review, the normal evidence stack keeps tool ownership clear:

```text
cargo-allow   = durable exception policy
ripr          = static test-oracle / mutation-exposure evidence
unsafe-review = static unsafe-contract evidence
xtask         = repository orchestration and retained policy
cargo-mutants = runtime mutation backstop
Miri          = concrete UB execution backstop
Codecov       = execution-surface telemetry
```

A sensor can supply evidence or expose an instrument gap. It cannot prove
soundness merely by running successfully. Upstream command/schema defects should
be fixed at the producing tool, with only bounded compatibility adapters in
`ub-review`.

## Non-claims

The architecture does not claim:

- code correctness or UB-freedom;
- that a model verdict is proof;
- that every sensor or model should run on every PR;
- that the current self-gate is ready to be an external sole required check;
- that v0 telemetry is trusted learning;
- that separate model personas create independent authority;
- that one successful receipt erases missing Required evidence;
- that a prepared review was posted;
- that merge-result review delivery is currently confirmable against the GitHub PR head;
- that run-stage `publication_result = posted` proves GitHub delivery;
- that the stable coordinator, rollback, or break-glass path is complete.

The intended destination remains stronger than the current implementation: one
architecture-aware senior reviewer and one trustworthy CI evidence gate, sharing
one revision-bound evidence transaction and earning every public and machine
claim from retained receipts.

[Issue #945]: https://github.com/EffortlessMetrics/ub-review/issues/945
