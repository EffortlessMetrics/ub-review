# Model-off CI efficiency path

_Status at source commit `55e4fbab879b3a83c31f35cdc9f7e5cd99f0a6c8`._

Issue [#1268] owns this programme. Issue [#945] remains the complete product
roadmap and merge-front authority. Current source, exact-head receipts, and
newer issue amendments outrank this document when they disagree.

## Decision

UB Review should earn useful deterministic CI savings before the complete
model reviewer, learning system, and sole-gate infrastructure are finished.
That does not create a second product or a smaller competing architecture.
The model-off path is the deterministic core of the same revision-bound
evidence transaction:

```text
exact revision
  -> repository-owned Required obligations
  -> one canonical identity per approved execution
  -> one Required-first SharedRunPlan
  -> one TaskLedger and scheduler
  -> current-revision receipts
  -> one truthful PASS / FAIL / NOT_PROVEN result
```

The architecture-aware reviewer later augments the same plan with repository
knowledge, selected model programmes, proof requests, reconsideration, one
final lead, and confirmed delivery. It does not replace or reinterpret the
deterministic core.

The following identities remain singular throughout the programme:

- one immutable `RevisionIdentity` for the reviewed run;
- one canonical execution identity for equivalent approved work;
- one `SharedRunPlan`, with an explicit model-off form and later model-on
  augmentation;
- one append-only `TaskLedger` for execution lifecycle and resource truth;
- one `FinalizedOutcome`, with deterministic CI, review, delivery, and run
  results kept separate.

## Current authority boundary

At the source commit named above:

- the existing sensor pools, proof broker, worker paths, leases, and budgets
  still control live execution;
- `TaskLedger` observes and replays execution in shadow;
- `gate_outcome.conclusion` remains the compatibility enforcement field;
- additive truth can report `not_proven`, but the normal required check can
  still pass over that disagreement;
- `build_gate_truth` still lets publication failure force the compatibility
  `gate_result` to `not_proven`; result-plane non-interference is therefore a
  required future contract, not current behavior;
- candidate self-dogfood remains candidate code, not a released stable judge;
- existing downstream CI remains authoritative.

Do not describe a planned or shadow contract as live authority. Do not retire
an existing check because UB Review can run a command with a similar string.

## Ordered path

### A. Make one run coherent

The authority merge front is [#1266]/[#956] -> [#957] -> [#958] -> [#959] ->
[#960] -> [#962].

This sequence must establish one coherent account of the exact revision, every
proposed and executed task, terminal receipts or failures, resource release,
prepared versus confirmed delivery, and the truthful outcome. Horizon A does
not claim scheduling or cost optimization.

### B. Bound evidence and retain the before-state

- [#1269] owns the output-containment programme.
- [#1279] defines the shared streaming bounded-capture primitive.
- [#1280] routes primary and nested/detail sensor processes through it.
- [#1281] routes proof and worker processes through it.
- [#1282] enforces expanded per-file, file-count, manifest, and total packet
  budgets before upload.
- [#1283] retains hostile-output and real RIPR integration proof, then closes
  [#1269].
- [#1284] separates small validated cross-job handoffs from the complete audit
  packet.
- [#1270] retains source-linked runner, process, duplicate-candidate, artifact,
  and outcome measurements without counting unproven command similarity as
  avoided work.

These issues may prepare fixtures file-disjoint from the implementation merge
front. Production process-runner edits must wait until they no longer collide
with the active authority work.

### C. Give equivalent work one identity and one receipt

The current identity train is [#902] -> [#896] -> [#900] -> [#899] -> [#964]
-> [#965] -> [#966].

[#902] is the bounded RIPR discriminator acceptance strategy. It is not proof
policy or an execution identity. [#896] then lands the pure identity value,
[#900] defines typed relation/subsumption semantics, and [#899] attaches those
contracts to planner/task artifacts before source-wide shadow adoption and
consumer reuse.

The earlier [#898] / PR [#901] attempt did not merge, and
`src/proof/identity.rs` is absent from current `main`. Those artifacts are
historical implementation and review evidence, not landed authority.

Equivalent approved requests from the gate, a sensor, deterministic proof, or
review may eventually attach to one queued, running, or completed
current-revision task. Distinct revisions, proof modes, red/green sides,
packages, targets, filters, features, working roots, environments, and tool
contracts remain distinct unless an explicit typed subsumption rule proves
otherwise.

The first economic receipt is one hosted model-off packet in which several
consumers request the same work, one process executes, and each consumer traces
to the same terminal receipt.

### D. Compile the deterministic SharedRunPlan core

The shared model-off portfolio and plan path is:

- [#970] defines the local Required spine.
- [#973] and [#974] provide mechanical package, target, test, consumer, and
  bounded reverse-dependency impact.
- [#976], [#977], and [#978] provide shared consumer semantics, catalog, and
  Required-first ranking.
- [#1271] retains the model-off Required-first portfolio receipt.
- [#1272] compiles the deterministic core of `SharedRunPlan`.
- [#1273] makes Required work immediately ready and protects deterministic
  receipt, outcome, verification, packet, and cleanup reserves.
- [#1274] proves the frozen model-off plan before scheduler migration.

Remote-platform, repository-contract, and model inputs have explicit source
states. Their absence may be `not_applicable`, an evidence gap, or a reason for
conservative broad fallback. It is never silently interpreted as complete
coverage.

[#1274] unlocks the pure and Required-only scheduler path. The complete
architecture/programme plan remains [#1120] and augments the same core before
model tasks enter live scheduler authority.

### E. Move deterministic execution under one scheduler

Scheduler migration proceeds through [#986], [#987], [#988], [#989], [#990],
and [#991].

The migration is staged so each authority move has a shadow comparison and an
explicit rollback. The complete state is not earned until every executable
path enters one scheduler, every terminal path releases resources, equivalent
work does not execute twice, and no independent timeout or budget authority can
starve Required work.

### F. Fix actual-time and Cargo economics

Actual-time and deadline work proceeds through [#992], [#993], [#994], and
[#995]. Cache-domain and same-run Cargo reuse proceed through [#996] and
[#997].

A timeout is a safety ceiling. A task that completes quickly returns unused
reservation immediately. Proof equivalence remains separate from cache
compatibility: `cargo check`, Clippy, docs, and tests may safely reuse compatible
compilation state without becoming interchangeable evidence.

### G. Prove savings before changing authority

[#1275] compares the model-off runner with existing CI over a preselected corpus
of at least twenty exact PR revisions across UB Review and one materially
different Rust repository. Existing CI remains authoritative during the pilot.
At least ten valid paired exact revisions are required per repository, selected
before candidate results are known.

The hard authority bar is zero unexplained instances of:

```text
Required evidence missing under PASS
stale evidence satisfying the current revision
unknown or unledgered physical execution
duplicate canonical execution
Required starvation
infrastructure failure mislabeled as a code failure
nonterminal admitted work at finalization
incorrect cross-mode or red/green satisfaction
silent packet-budget loss
```

The runner-work bar is at least 25% median reduction in Linux-equivalent runner
minutes over the selected replaceable workload. Report each repository
separately and the combined corpus; a combined improvement cannot conceal a
repository regression.

The deterministic-decision latency bar is also evaluated separately for each
repository:

- baseline: existing-CI time from the comparable run start/admission boundary to
  its deterministic decision timestamp;
- candidate: UB Review time over the same exact revision and boundary;
- estimator: nearest-rank p95 over the valid paired revisions;
- minimum sample: ten valid pairs with known start and deterministic-decision
  timestamps per repository;
- maximum candidate p95:
  `baseline p95 + min(120 seconds, max(30 seconds, 10% of baseline p95))`.

Fewer than ten valid pairs, an unknown decision timestamp, or incomparable
timing boundaries yields `mixed_requires_more_evidence`, not pass. Repository
results may not be pooled to conceal a failure.

The report must preserve sample sizes, runner rounding, unknowns, cold/warm
Cargo state, exact executions avoided, and compressed versus expanded artifact
sizes. A mixed or no-go result remains mixed or no-go and opens only the
concrete failed invariant or economic seams exposed by the corpus.

### H. Freeze result-plane non-interference, enforce truth, then retire work

[#1277] is a predecessor, not a post-hoc audit. After [#960], it must freeze the
`ci_evidence_result` inputs and digest before review or delivery are joined,
then prove that model judgment, provider state, publication, delivery, and run
classification cannot rewrite deterministic CI truth. Live model proof,
reconsideration, stage budgets, final lead, and public-review authority remain
blocked until this contract is terminal.

[#1015] then changes `gate-check` and Action exit authority to the frozen
`FinalizedOutcome.ci_evidence_result`: `pass` succeeds, deterministic `fail` is
a code/policy failure, and `not_proven` is non-green evidence unavailability.

[#1276] retires only jobs proven receipt-equivalent through a reversible
shadow-decommission transaction, one independent obligation at a time.

Workflow changes and branch-protection changes are separate authority steps.
Remote platform, special-hardware, security, or service-integration checks are
not replaced by a local approximation. Checks protecting UB Review's own gate,
policy, action, or verifier remain independently protected until a released
stable coordinator judges candidate code.

## Complete reviewer path remains live

The model-off cut does not remove the work needed for a senior reviewer with a
controlled lab:

- [#967] -> [#968] establishes explicit repository-source ingestion.
- [#973] -> [#974] establishes mechanical package, target, test, consumer, and
  bounded reverse-dependency scope.
- [#968] + [#974] -> [#969] combines repository contracts with that bounded
  mechanical scope into cycle-free architecture knowledge.
- [#969] -> [#975] adds material architecture, trust, mirror, and support edges.
- [#971] and [#972] add remote/existing-CI mapping where applicable.
- [#979] proves the complete enriched portfolio.
- [#980], [#981], and [#982] define and select registered review
  programmes.
- [#983], [#984], and [#985] augment the same `SharedRunPlan` with model
  work, reserves, and one shared prompt-prefix plan.
- [#1120] proves the complete enriched reviewer-and-gate plan.
- [#1277] must then be terminal before any live C4/C5 model authority.
- C4/C5 may start approved proof during analysis, reconsider exact claims, and
  run one final lead only through the existing plan, scheduler, TaskLedger, and
  non-interference guard.

Review and CI share evidence without sharing verdict authority. A clean model
judgment cannot turn missing Required evidence into PASS. A model finding cannot
fabricate deterministic FAIL. Delivery failure changes delivery/run truth, not
the code-evidence result.

## Merge-front discipline

One authority-bearing implementation writer remains canonical. Parallel work is
limited to read-only investigation, frozen fixture preparation, file-disjoint
documentation, independent challenge, and baseline measurement that does not
create a competing truth model.

For every implementation PR:

1. refresh current `main`, dependencies, source, fixtures, verifier, workflows,
   docs, and downstream pins;
2. state one exact behavior and explicit non-goals;
3. prove success, failure, stale/forged, terminal, replay, and rollback behavior
   appropriate to the seam;
4. run focused proof before the complete repository verification chain;
5. inspect exact-head hosted artifacts and current review threads;
6. merge only the reviewed exact head; then reconcile issues, branches,
   worktrees, receipts, and the next legal frontier.

Green CI is necessary, not sufficient. Plans, types, issue closure, and shadow
artifacts do not prove production authority.

## Promotion boundary

This path earns a useful, truthful model-off CI runner. It does not by itself
earn the sole required check. Hostile-head isolation, released stable versus
candidate authority, terminal GitHub check publication, rollback, break-glass,
external calibration, and explicit branch-protection authorization remain under
[#658] and its stable-coordinator programme. The current temporary decision to
require the independent baseline is separately owned by [#1285].

[#658]: https://github.com/EffortlessMetrics/ub-review/issues/658
[#896]: https://github.com/EffortlessMetrics/ub-review/issues/896
[#898]: https://github.com/EffortlessMetrics/ub-review/issues/898
[#899]: https://github.com/EffortlessMetrics/ub-review/issues/899
[#900]: https://github.com/EffortlessMetrics/ub-review/issues/900
[#901]: https://github.com/EffortlessMetrics/ub-review/pull/901
[#902]: https://github.com/EffortlessMetrics/ub-review/issues/902
[#945]: https://github.com/EffortlessMetrics/ub-review/issues/945
[#956]: https://github.com/EffortlessMetrics/ub-review/issues/956
[#957]: https://github.com/EffortlessMetrics/ub-review/issues/957
[#958]: https://github.com/EffortlessMetrics/ub-review/issues/958
[#959]: https://github.com/EffortlessMetrics/ub-review/issues/959
[#960]: https://github.com/EffortlessMetrics/ub-review/issues/960
[#962]: https://github.com/EffortlessMetrics/ub-review/issues/962
[#964]: https://github.com/EffortlessMetrics/ub-review/issues/964
[#965]: https://github.com/EffortlessMetrics/ub-review/issues/965
[#966]: https://github.com/EffortlessMetrics/ub-review/issues/966
[#967]: https://github.com/EffortlessMetrics/ub-review/issues/967
[#968]: https://github.com/EffortlessMetrics/ub-review/issues/968
[#969]: https://github.com/EffortlessMetrics/ub-review/issues/969
[#970]: https://github.com/EffortlessMetrics/ub-review/issues/970
[#971]: https://github.com/EffortlessMetrics/ub-review/issues/971
[#972]: https://github.com/EffortlessMetrics/ub-review/issues/972
[#973]: https://github.com/EffortlessMetrics/ub-review/issues/973
[#974]: https://github.com/EffortlessMetrics/ub-review/issues/974
[#975]: https://github.com/EffortlessMetrics/ub-review/issues/975
[#976]: https://github.com/EffortlessMetrics/ub-review/issues/976
[#977]: https://github.com/EffortlessMetrics/ub-review/issues/977
[#978]: https://github.com/EffortlessMetrics/ub-review/issues/978
[#979]: https://github.com/EffortlessMetrics/ub-review/issues/979
[#980]: https://github.com/EffortlessMetrics/ub-review/issues/980
[#981]: https://github.com/EffortlessMetrics/ub-review/issues/981
[#982]: https://github.com/EffortlessMetrics/ub-review/issues/982
[#983]: https://github.com/EffortlessMetrics/ub-review/issues/983
[#984]: https://github.com/EffortlessMetrics/ub-review/issues/984
[#985]: https://github.com/EffortlessMetrics/ub-review/issues/985
[#986]: https://github.com/EffortlessMetrics/ub-review/issues/986
[#987]: https://github.com/EffortlessMetrics/ub-review/issues/987
[#988]: https://github.com/EffortlessMetrics/ub-review/issues/988
[#989]: https://github.com/EffortlessMetrics/ub-review/issues/989
[#990]: https://github.com/EffortlessMetrics/ub-review/issues/990
[#991]: https://github.com/EffortlessMetrics/ub-review/issues/991
[#992]: https://github.com/EffortlessMetrics/ub-review/issues/992
[#993]: https://github.com/EffortlessMetrics/ub-review/issues/993
[#994]: https://github.com/EffortlessMetrics/ub-review/issues/994
[#995]: https://github.com/EffortlessMetrics/ub-review/issues/995
[#996]: https://github.com/EffortlessMetrics/ub-review/issues/996
[#997]: https://github.com/EffortlessMetrics/ub-review/issues/997
[#1015]: https://github.com/EffortlessMetrics/ub-review/issues/1015
[#1120]: https://github.com/EffortlessMetrics/ub-review/issues/1120
[#1266]: https://github.com/EffortlessMetrics/ub-review/pull/1266
[#1268]: https://github.com/EffortlessMetrics/ub-review/issues/1268
[#1269]: https://github.com/EffortlessMetrics/ub-review/issues/1269
[#1270]: https://github.com/EffortlessMetrics/ub-review/issues/1270
[#1271]: https://github.com/EffortlessMetrics/ub-review/issues/1271
[#1272]: https://github.com/EffortlessMetrics/ub-review/issues/1272
[#1273]: https://github.com/EffortlessMetrics/ub-review/issues/1273
[#1274]: https://github.com/EffortlessMetrics/ub-review/issues/1274
[#1275]: https://github.com/EffortlessMetrics/ub-review/issues/1275
[#1276]: https://github.com/EffortlessMetrics/ub-review/issues/1276
[#1277]: https://github.com/EffortlessMetrics/ub-review/issues/1277
[#1279]: https://github.com/EffortlessMetrics/ub-review/issues/1279
[#1280]: https://github.com/EffortlessMetrics/ub-review/issues/1280
[#1281]: https://github.com/EffortlessMetrics/ub-review/issues/1281
[#1282]: https://github.com/EffortlessMetrics/ub-review/issues/1282
[#1283]: https://github.com/EffortlessMetrics/ub-review/issues/1283
[#1284]: https://github.com/EffortlessMetrics/ub-review/issues/1284
[#1285]: https://github.com/EffortlessMetrics/ub-review/issues/1285
