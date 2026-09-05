# Product state

_Last reconciled against `main` at `55e4fba` on 2026-08-28._

This is the canonical capability-state document. [Issue #945] owns execution
order; parent issues own capability contracts; retained receipts and runtime
verification outrank this file. Update this document after an authority-bearing
merge, not after a type, schema, fixture, or issue plan lands.

## Current position

Transactional review delivery, current-head reconciliation, substantial
sensor/proof/review compilation, and additive result separation exist.
Immutable revision identity is admitted and verifier-joined across the core
current-run artifacts on `main`. The pure TaskLedger, execution-accounting
lifecycle, ledger artifact verifier, retained contradiction corpus, and the
first production shadow adapter also exist. Fast and late sensor execution now
emits revision-bound TaskLedger lifecycles through #1263/#955. TaskLedger still
does not schedule that work, observe proof/worker execution, or reconcile every
legacy projection.

One live scheduler/resource authority, a repository-native architecture
contract, a Required-first CI spine, one shared run plan, authoritative final
judgment, finalized outcome enforcement, trusted learning, a hostile-head-safe
stable coordinator, and externally calibrated sole-gate operation remain
incomplete. The shortest accurate statement is:

> Transactional delivery and substantial review/proof substrate exist.
> Revision authority is materially advanced; task authority is in shadow.
> Scheduler, final-judgment, finalized-outcome, learning, and stable-gate
> authority remain incomplete in the precise ways tracked by #923–#945.

## What makes UB Review different

UB Review is not distinct because it calls models, and its product boundary is
not limited to undefined-behaviour review. Its distinguishing design is one
revision-bound evidence transaction that can produce both a senior PR review
and a deterministic CI evidence decision:

```text
exact admitted revision
  -> repository and diff evidence
  -> one bounded gate-and-review plan
  -> deterministic sensors/proof + model investigation
  <-> approved proof tasks and revision-bound receipts
  -> claim/evidence reconciliation
  -> concise senior review
  + eventual PASS / FAIL / NOT_PROVEN FinalizedOutcome
  -> retained audit artifacts
```

The important hinges are:

- **Models investigate; receipts decide.** A model can form a hypothesis or ask
  a material question. It cannot prove correctness, authorize an arbitrary
  command, or make missing Required evidence clean.
- **The reviewer has a lab.** The intended loop starts approved focused proof
  while investigation continues, then lets the resulting receipt change only
  the exact claim it tests.
- **Review and CI share evidence without sharing a verdict.** Review quality,
  publication success, instrument coverage, and CI sufficiency are separate
  results compiled from the same revision-bound run.
- **One required check does not mean one giant job.** The destination is one
  stable coordinator over independently receipted tasks, caches, deadlines,
  and side effects—not a serial workflow or a model acting as judge.
- **Private machinery, public judgment.** Full planner, lane, sensor, cost, and
  failure detail remains auditable in artifacts. The PR surface spends
  attention only on material findings, verification questions, and the
  smallest complete decision.
- **Repository-native evidence is the destination.** Explicit architecture,
  ownership, support, mirror, CI, and proof contracts should determine which
  review programmes apply. A fixed panel of generic reviewers is transitional
  implementation, not the product model.

Some of this loop is live today; some remains the destination. The matrix below
keeps those states separate.

## CI outcome vocabulary

The target CI authority is `FinalizedOutcome.ci_evidence_result`. Its semantic
and serialized contract is:

| Semantic result | Serialized value | Earned only when |
| --- | --- | --- |
| **PASS** | `pass` | Every applicable Required obligation has a current-revision satisfying terminal receipt, and no deterministic receipt establishes a policy-blocking violation. |
| **FAIL** | `fail` | A current-revision deterministic receipt establishes a policy-blocking violation. A model or Advisory finding never suffices. |
| **NOT_PROVEN** | `not_proven` | Any applicable Required obligation is missing, unavailable, stale, malformed, or the terminal inputs cannot establish PASS or FAIL. |

Current compatibility surfaces do not yet serialize that authority directly:

| Current surface | Values | Current authority |
| --- | --- | --- |
| `gate-result` | `pass`, `finding`, `not_proven` | Truthful additive report. `finding` is not an automatic alias for target `fail`. |
| `gate-conclusion` / `gate_outcome.conclusion` | `pass`, `fail`, `inconclusive` | Legacy enforced compatibility result until #1015. |

Do not mechanically translate `finding` to `fail` or `inconclusive` to
`not_proven`. FinalizedOutcome recomputes the target result from typed
current-revision terminal inputs. #1015 changes enforcement authority only
after Required-first execution and live scheduler proof.

## Capability-state vocabulary

| State | Meaning |
| --- | --- |
| **Implemented** | Types, reducers, schemas, or local behavior exist. |
| **Wired in production** | A normal run emits or consumes the behavior. |
| **Proven by a retained run** | A replay or hosted exact-head packet preserves the behavior and its receipts. |
| **Trusted authority** | Production decisions depend on it and verification rejects stale, forged, missing, or contradictory authority. |
| **Externally calibrated** | Representative external repositories show sustained usefulness, reliability, and acceptable economics. |

A later state is never inferred from an earlier one. Issue closure, schema
presence, and a green self-test are not substitutes for production authority.

## Capability matrix

| Surface | Highest earned state | What exists now | Missing authority / next contract | Evidence and roadmap |
| --- | --- | --- | --- | --- |
| Reviewer-facing compilation | **Proven by a retained run** | Evidence-backed item admission, value-ordered degradation, concise line notes, summary preservation, click-to-apply suggestion guards, and exact GitHub payload goldens are merged and retained. | Sustained maintainer value is not externally calibrated; one authoritative architecture-aware final lead remains future work. | [#829 commit], [#834], [#846], [#849], [#850], [#851], [#853], [#840], [#865] |
| Candidate-head delivery and fixed-head silence | **Proven by a retained run** | Candidate-head pending-review transactions, exact comment reconciliation, current-head revalidation, idempotent replies/fallbacks, integrated Perl replay, and replay deduplication are retained. | Synthetic merge-result delivery is not proven: the current adapter reads `revision.reviewed_commit` (the synthetic merge object) as the expected delivery head, while GitHub exposes the pull-request head SHA. Carry both identities or select the PR-head delivery subject before claiming merge-result delivery. Cross-push memory/reanchoring and stable-coordinator ownership also remain open. | [delivery transaction], [reply delivery], [#835], [#880], [#867], [#923], [#814] |
| Proof requests and execution adapters | **Proven by a retained run** | Semantic model intents resolve to approved focused tasks; current-head receipt replanning, Rust impact tests, base-plus-tests red/green selection, candidate cataloguing, budgets, leases, and focused proof receipts exist. | Executable paths still enter through multiple legacy adapters. They must all be observed and reconciled in TaskLedger before canonical identity, deduplication, or live scheduling. | [#836], [#837], [#852], [#854], [#916], [#956], [#957], [#860] |
| Immutable revision identity | **Wired in production** | Pure identity, ordinary Git admission, exact candidate-head/merge-result semantics, propagation through core proof/claim/cost/gate artifacts, verifier joins, and trusted-base diff admission are merged. Symbolic refs remain compatibility labels. | Authority is not complete across delivery: merge-result `reviewed_commit` is a synthetic merge object, not the GitHub PR-head object used for posting revalidation. Keep #923 open until delivery and every future reuse/stable surface preserve both identities and reject the wrong subject. | [#1245], [#1246], [#1248], [#1250], [#1251], [#1252], [#1255], [#923] |
| Task and resource authority | **Wired in production** | Pure task lifecycle/accounting, deterministic ledger artifacts/verifier, the sanitized #915/#916/#921 contradiction corpus, and normal-run fast/late sensor lifecycle shadowing are merged. | TaskLedger is observation/replay authority only. Existing pools, brokers, workers, leases, and budgets still control execution; #956 must observe proof/workers and #957 must reconcile every projection before #861 later owns live scheduling. | [#1253], [#1256], [#1259], [#1262], [#1263], [#955], [#956], [#957], [#861] |
| Required CI spine and evidence portfolio | **Wired in production** | Configured proof, impact proof, portfolio selection, receipt catalogues, required-tool semantics, and additive coverage accounting run in production. | Required/Detective/Advisory semantics, one repository-owned minimal spine, one authority-ordered portfolio, and one shared frozen run plan are not yet the live source of truth. | [#855], [#852], [#916], [#941], [#928], [#942] |
| Model routing, reconsideration, and final lead | **Wired in production** | Bounded specialist lanes, semantic proof intents, artifact-only private audits, and receipt-linked reconsideration substrate run in production. | Material-change-driven programme selection, proof intake while other lanes are still running, one ModelStageLedger, protected final-call reserve, and typed final-lead authority remain open. | [#836], [#912], [#914], [#864], [#859], [#930], [#865], [#929] |
| Gate result and enforcement | **Wired in production** | Analysis, run-stage publication projection, sensor/model coverage, and current `gate-result` values are reported separately from the legacy conclusion; insufficient evidence can be represented as `not_proven`. A prepared payload currently serializes `publication_result = posted` before GitHub delivery runs. | Post success/failure receipts remain separate and do not recompute `gate_outcome`; #959/#960 must finalize prepared versus confirmed/failed delivery before #1015 can switch enforcement to target `ci_evidence_result`. `gate-check` still enforces the legacy conclusion. | [#855], [#926], [#958], [#959], [#960], [#1015] |
| Learning and calibration | **Implemented** | Cost, token, timing, finding, delivery, and calibration-v0 telemetry can retain observations. | Those observations are not trusted learning authority, adaptive omission evidence, or proof that later runs improve. Canonical learning, economics, counterfactuals, and external calibration remain post-useful-product work. | [#840], [#868], [#932], [#933], [#939] |
| Candidate-head containment | **Proven by a retained run** | Candidate-head dogfood is model-off/artifact-only; a base-owned deterministic baseline, committed source lockfile, and trusted-base diff admission are retained. | Full hostile-head environment/tool isolation and the untrusted-evidence to trusted-review/posting handoff remain open. | [#1240], [#1241], [#1249], [#1255], [#876] |
| Release and install distribution | **Wired in production** | Archive/binary identity validation and the v0.1.2 release path exist. | Clean no-host-Cargo portability, stable analyzer bundle authority, upgrade/rollback acceptance, and external install proof remain open. | [#903], [#906], [#1242], [#815] |
| Stable coordinator and terminal check | **Implemented** | Pure watchdog classification and stable/candidate authority contracts exist; candidate self-gate authority is separately contained. | A released stable coordinator, identical-input candidate shadow, exact-head terminal publication, rollback, and break-glass are not wired as required authority. | [#745], [#814], [#658] |
| Onboarding commands | **Wired in production** | `init`, `enable --inspect`, `audit-ci`, and `setup-ci` inspect and generate adoption proposals without silently changing branch protection. | Fresh-repo release-installed adoption, upgrade/rollback, and architecture/CI-spine proposal acceptance remain open. | [#845], [#944] |
| External pilot and promotion substrate | **Implemented** | Advisory pilot contracts, disposition/economics surfaces, and explicit promotion authority nodes exist. | Bun/Perl pilot execution, comparative calibration, measured advantage over the existing review process, and repository-owner sole-gate authorization remain open. | [#806], [#807], [#808], [#840], [#658] |

## Live authority map

The current run has several useful planes, but they do not yet form one live
control plane:

```text
RevisionIdentity
  = bounded current-run identity and verifier join authority;
    merge-result delivery binding is not yet authoritative

legacy sensor pools + proof broker + worker paths + leases/budgets
  = current execution and resource authority

TaskLedger
  = shadow observation/replay authority only

review compiler + delivery transaction
  = candidate-head public-output preparation and side-effect path;
    post receipts remain separate from gate truth

gate_outcome.conclusion
  = current enforcement field

analysis_result / publication_result / gate_result
  = run-stage additive diagnostics, not yet delivery-confirmed or enforcement authority

model outputs
  = hypotheses, proposed findings, and semantic proof intents; never proof

calibration.v0 and cost telemetry
  = observations; never trusted learning or omission authority
```

Do not describe the resource broker as controlling the box, TaskLedger as the
scheduler, `gate_result` as the enforced verdict, or calibration-v0 as learned
policy. Those are destination claims, not current behavior.

## Active implementation front

The containment, committed-lockfile, and RevisionIdentity front named in #963
has landed and must not remain an agent instruction. The live serial front is:

```text
#956      observe configured/impact/model/follow-up proof and workers
  -> #957      reconcile queue/portfolio/lease/gate projections
  -> #958      pure FinalizedOutcome reducer
  -> #959      prepared-versus-confirmed delivery finalization
  -> #960      shadow FinalizedOutcome integration and verification
  -> #962      complete Horizon A revision/task/delivery/outcome packet proof
```

Fast/late sensor shadowing completed in [#1263]/[#955]. The complete
Horizon A packet proof remains [#962]. Issue #945 remains the execution-order
authority. File-disjoint documentation,
read-only research, and frozen fixtures may proceed in parallel; implementation
must not jump the serial authority front.

## Do not start yet

While Horizon A remains incomplete, do not begin production implementation of:

- the live scheduler/resource migration (#861);
- trusted learning, adaptive omission, or counterfactual selection
  (#868/#932/#933);
- production heavy-witness expansion unrelated to the active authority front;
- provider-native fork/multi-agent orchestration ([#934] and [#938]);
- the CI advisor ([#936]);
- stable-coordinator or sole-required-check promotion (#814/#658).

This is sequencing, not product retreat. The destination remains the integrated,
architecture-aware reviewer and trustworthy sole gate in #840/#945.

## Evidence discipline for status changes

Promote a row only when the strongest relevant receipt exists:

1. **Implemented:** merged code plus focused success/failure/serialization tests.
2. **Wired:** a normal run emits and consumes the new surface.
3. **Retained:** an exact-head or replay packet is committed or durably linked.
4. **Authority:** stale, forged, missing, mixed, and contradictory inputs fail at
   the production decision boundary.
5. **Calibrated:** external runs are reconciled for value, misses, noise,
   reliability, latency, cost, and maintainer disposition.

When evidence conflicts, retain the contradiction and lower the claim. Do not
rewrite a historical packet to make the current architecture look coherent.

[Issue #945]: https://github.com/EffortlessMetrics/ub-review/issues/945
[#840]: https://github.com/EffortlessMetrics/ub-review/issues/840
[#923]: https://github.com/EffortlessMetrics/ub-review/issues/923
[#926]: https://github.com/EffortlessMetrics/ub-review/issues/926
[#928]: https://github.com/EffortlessMetrics/ub-review/issues/928
[#929]: https://github.com/EffortlessMetrics/ub-review/issues/929
[#930]: https://github.com/EffortlessMetrics/ub-review/issues/930
[#932]: https://github.com/EffortlessMetrics/ub-review/issues/932
[#933]: https://github.com/EffortlessMetrics/ub-review/issues/933
[#934]: https://github.com/EffortlessMetrics/ub-review/issues/934
[#936]: https://github.com/EffortlessMetrics/ub-review/issues/936
[#938]: https://github.com/EffortlessMetrics/ub-review/issues/938
[#939]: https://github.com/EffortlessMetrics/ub-review/issues/939
[#941]: https://github.com/EffortlessMetrics/ub-review/issues/941
[#942]: https://github.com/EffortlessMetrics/ub-review/issues/942
[#944]: https://github.com/EffortlessMetrics/ub-review/issues/944
[#955]: https://github.com/EffortlessMetrics/ub-review/issues/955
[#956]: https://github.com/EffortlessMetrics/ub-review/issues/956
[#957]: https://github.com/EffortlessMetrics/ub-review/issues/957
[#958]: https://github.com/EffortlessMetrics/ub-review/issues/958
[#959]: https://github.com/EffortlessMetrics/ub-review/issues/959
[#960]: https://github.com/EffortlessMetrics/ub-review/issues/960
[#962]: https://github.com/EffortlessMetrics/ub-review/issues/962
[#1015]: https://github.com/EffortlessMetrics/ub-review/issues/1015
[#745]: https://github.com/EffortlessMetrics/ub-review/issues/745
[#806]: https://github.com/EffortlessMetrics/ub-review/issues/806
[#807]: https://github.com/EffortlessMetrics/ub-review/issues/807
[#808]: https://github.com/EffortlessMetrics/ub-review/issues/808
[#814]: https://github.com/EffortlessMetrics/ub-review/issues/814
[#815]: https://github.com/EffortlessMetrics/ub-review/issues/815
[#861]: https://github.com/EffortlessMetrics/ub-review/issues/861
[#864]: https://github.com/EffortlessMetrics/ub-review/issues/864
[#865]: https://github.com/EffortlessMetrics/ub-review/issues/865
[#867]: https://github.com/EffortlessMetrics/ub-review/issues/867
[#868]: https://github.com/EffortlessMetrics/ub-review/issues/868
[#876]: https://github.com/EffortlessMetrics/ub-review/issues/876
[#658]: https://github.com/EffortlessMetrics/ub-review/issues/658
[#859]: https://github.com/EffortlessMetrics/ub-review/issues/859
[#860]: https://github.com/EffortlessMetrics/ub-review/issues/860
[#829 commit]: https://github.com/EffortlessMetrics/ub-review/commit/469288586d6ef01e50a2968f8826c89f1c1009f0
[#834]: https://github.com/EffortlessMetrics/ub-review/commit/7da3d68d82ff5d930a2968a4b109301ab274382e
[#835]: https://github.com/EffortlessMetrics/ub-review/commit/87b7023b85bb2ab05b5b2ff2ee820d03fc442745
[#836]: https://github.com/EffortlessMetrics/ub-review/commit/1510d29d5394fa9ca936e3c390a85ac93dec5b13
[#837]: https://github.com/EffortlessMetrics/ub-review/commit/7b112d9cebe989e1e9d4b693dfe5c743dc4a22df
[#845]: https://github.com/EffortlessMetrics/ub-review/commit/e53f5820db1b4d031c91bb840031b4904c7ea3cc
[#846]: https://github.com/EffortlessMetrics/ub-review/commit/d92c6f9c8790cb7a56c6b3ccbb22e974a425b20b
[#849]: https://github.com/EffortlessMetrics/ub-review/commit/875d54468ca5b0649421f78469c06ccedbee2770
[#850]: https://github.com/EffortlessMetrics/ub-review/commit/5a9bf3d640361a08be8eedb41ffddc55578d3337
[#851]: https://github.com/EffortlessMetrics/ub-review/commit/ec5d9e2c484246c7373d25a330a1577417eee607
[#852]: https://github.com/EffortlessMetrics/ub-review/commit/fa9fccd109b024cdeb051edca39cef4646a4229d
[#853]: https://github.com/EffortlessMetrics/ub-review/commit/7098c05256197169fd958a02581259dcd7417395
[#854]: https://github.com/EffortlessMetrics/ub-review/commit/56f83d72f1724b7324ff7bece4223b287fb94986
[#855]: https://github.com/EffortlessMetrics/ub-review/commit/5edcc8c107b3bbef508c35e7164c831cbfcb7fa4
[#880]: https://github.com/EffortlessMetrics/ub-review/commit/96e1b1dbb2d6508cc8ee35dcc5402684b722687b
[#903]: https://github.com/EffortlessMetrics/ub-review/commit/4a8eb7ee0c19e88084a2fd4f8af9d7f58098849e
[#906]: https://github.com/EffortlessMetrics/ub-review/commit/798db6d8f628701bce343e5d9ae28bff03d367e1
[#912]: https://github.com/EffortlessMetrics/ub-review/commit/d4dc4830bcfb94ee8fb1c0099c08c69f00dc6cbf
[#914]: https://github.com/EffortlessMetrics/ub-review/commit/f6c7a1c2d84817b54df2e3380066af11c1bf540b
[#916]: https://github.com/EffortlessMetrics/ub-review/commit/c11677ca917da00066759fc07d8c7da66465ae81
[#1240]: https://github.com/EffortlessMetrics/ub-review/commit/d6aa30cdafc2cf45e8b66f305e64a43d959a18c7
[#1241]: https://github.com/EffortlessMetrics/ub-review/commit/0227d8a4300e23e1f5c77e04bf164668a32924e4
[#1242]: https://github.com/EffortlessMetrics/ub-review/commit/3e51ec8dda19baf4d377c8543868cf2c9eb76d85
[#1245]: https://github.com/EffortlessMetrics/ub-review/commit/932b887f2b111423a3bc7ad846213ffa5feba1b7
[#1246]: https://github.com/EffortlessMetrics/ub-review/commit/39a27a65459b525cf8735c6f43fc8b32502ade1e
[#1248]: https://github.com/EffortlessMetrics/ub-review/commit/bcaa102ab0424c5ad376670a476a145d9b360643
[#1249]: https://github.com/EffortlessMetrics/ub-review/commit/84477696e4213a81b0f5e78c048fb25f05bb50c9
[#1250]: https://github.com/EffortlessMetrics/ub-review/commit/a85fd3bc8d18d6f8d90ca6b3c5211fa5cc7998c1
[#1251]: https://github.com/EffortlessMetrics/ub-review/commit/8f6d817cd5e6b6937ff747dac5a0770431e2be8e
[#1252]: https://github.com/EffortlessMetrics/ub-review/commit/89560a9060afa033d7470d00dec4d136cf896459
[#1253]: https://github.com/EffortlessMetrics/ub-review/commit/afcccb472a9cf1f361aea9f0c6484160556224a8
[#1255]: https://github.com/EffortlessMetrics/ub-review/commit/28f4f4bfd8344dc00baa24f213c69279f593418d
[#1256]: https://github.com/EffortlessMetrics/ub-review/commit/fac7983d49ed94d3d17899fdf6cc007e33a23630
[#1259]: https://github.com/EffortlessMetrics/ub-review/commit/e24f7edb3502f4308b45f690c171de863299c329
[#1262]: https://github.com/EffortlessMetrics/ub-review/commit/f02f4a85449ec8b8faf3dafb67071ccd544afae0
[#1263]: https://github.com/EffortlessMetrics/ub-review/commit/55e4fbab879b3a83c31f35cdc9f7e5cd99f0a6c8
[delivery transaction]: https://github.com/EffortlessMetrics/ub-review/commit/55221c1b9832a43f30b36470a0266fced516653d
[reply delivery]: https://github.com/EffortlessMetrics/ub-review/commit/65289d6489adacf4e5ba89e8518906f8d720f682
