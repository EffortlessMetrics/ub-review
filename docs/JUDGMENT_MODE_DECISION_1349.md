# Judgment reuse and mode dependencies — decision packet #1349

Status: **proposed; Steven's decision pending**. Owner: existing #1349
architecture lane under [#945](https://github.com/EffortlessMetrics/ub-review/issues/945).
Registry handoff: [#1216](https://github.com/EffortlessMetrics/ub-review/issues/1216).
This documentation changes no executable behavior, dependency enforcement,
model permission, public authority, deployment, release or repository setting.

## Frozen source and ownership observation

Inspected 2026-10-11: local and remote main
`5a72634d4eec98d5de494598902cc6cac256b508`, tree
`284b5c58cd95de6ec8b6451accb4c7cf3d1abed9`. This confirms the parent audit;
#1349's opening `266260b4` observation is historical. Recheck live heads before
implementation. Read AGENTS.md, FILE_POLICY, REPO_STYLE, operating handoff and
current #945/#1216 bodies/comments. No repository-local SKILL.md was present;
the selected environment's /workspace/.agents directory was empty.

Connected GitHub inventory returned 29 open PRs and all their changed-file
lists. None touches ARCHITECTURE.md or this packet, and #1349 has no assignee,
comments or identified implementation PR at this observation. No competing
owner was found. CLI GraphQL and REST reads returned Forbidden; those routes
were stopped. Successful connected reads do not establish CLI write permission.

Preserve #929/#999–#1003 semantic owners, #865/#1013/#1014 final lead/compiler,
#926/#1277 result separation, #1334 admission, #1335 projection and #1338
adapter. Observed open owner lanes include #1317 delivery at `87e28f89`,
#1370 recovery tests at `54bc3a9f`, #1352/PR #1373 applicability at `2bc9478f`,
#1343/PR #1379 bytes at `25686bca`, #1347 analyzer admission at `99a60cf5`,
and #1354 README at `da7f873d`. #1344 owns byte promotion. These are ownership
observations, not acceptance receipts; this packet implements none of them.

## Actual source trace and smallest reusable boundary

| Surface at frozen main | Actual API and dependency | Reuse implication / gap |
| --- | --- | --- |
| Exact source | [revision_identity.rs](../src/revision_identity.rs): `CommitTree`, `ReviewSemantics`, `RevisionIdentity::new`, `canonical_form`, `from_canonical`, `identity_digest`, `RevisionRef::validate` | Reuse admitted base/head/reviewed/merge commit and tree, changed-path and diff digests. Candidate-head and merge-result differ. Repository/PR, review-spec/pass and attempt identities belong in a separately verified envelope; do not coerce em-ci acquisition hashes into this digest. |
| Specialist output | [main.rs](../src/main.rs): `LaneModelOutputWire` -> `LaneModelOutput`; [model_exec.rs](../src/model_exec.rs): `apply_model_output`, `validate_proof_intent`; [claim_graph.rs](../src/claim_graph.rs): `ClaimGraph`, `ClaimNode`, `EvidenceRef`; [review_topics.rs](../src/review_topics.rs): `build_active_claim_graph` | Current lane results include findings, observations, failed objections, private audit and proof intents. Admission and claim/evidence reconciliation are genuine shared dependencies. A lane summary is not a verified final judgment. Source-only adapter must never call the proof execution path. |
| Reporter | [reporter.rs](../src/reporter.rs): `LaneDigest`, `ReporterConclusion`, `ReporterVerdict`, strict parsing, `ReporterTurnResolution::public_distillation` / `gate_input`; model_exec's `run_reporter_coordination` and main's `finalize_reporter_coordination` composition | Current reporter is model-generated coordination with current-head resolution, not the planned protected `ReviewJudgment`. `gate_input` also feeds opt-in legacy review-forward gating. Reuse parsing/provenance with care; never route source-only output into that gate consumer. Private LaneDigest audit is not public input. |
| Editorial compiler | [review_compiler.rs](../src/review_compiler.rs): `ReviewCompilerInput`, `FinalCompilerInputArtifact`, `compile_review_surface` -> `CompiledReviewSurface` | Value compilation still depends on root binary types (`RunArgs`, `Plan`, `DiffContext`, receipts, findings, policy). It renders admitted lane material and conditionally admits reporter text when body is empty. It is not the #1014 sole final-lead selector. Pure helpers can be reused; formatting does not create a semantic judgment. |
| Gate truth | [gate_truth.rs](../src/gate_truth.rs): `GateTruthInput`, `build_gate_truth` -> `GateTruth` | Current prepared payload maps to `posted`; publication failure/not-proven takes precedence in legacy `gate_result`. `gate_outcome.conclusion` remains enforced compatibility authority. None of these fields is a source-only CI result or the target frozen `ci_evidence_result`. Repairs stay #1317/#1277, with #1352 applicability separate. |
| Delivery values | [delivery_transaction.rs](../src/delivery_transaction.rs): `PlannedDelivery`, `ObservedDelivery`, `DeliveryReconciliation`, `DeliveryReceipt`, `DeliveryTransaction`, state transitions | Exact-head/claim/location/body/action/thread identities and transaction receipts are reusable delivery context. Current files/state are not an append-only AgentReviewResult event store. Keep immutable public-plan selection distinct from mutable transaction bookkeeping. |
| Delivery I/O | [github_delivery.rs](../src/github_delivery.rs): `execute_pending_review_delivery`, transport fetch/send, head revalidation, pending-review submission and cleanup; main's `cmd_post` and [post_command.rs](../src/post_command.rs) post-error classification | Process/credential/network operations are outside the pure boundary. A failed/network_post tuple can follow remote acceptance and lost response. It proves neither absence nor safe retry. #1370 owns the tests and #1317 the repair; #1335 needs a versioned unknown/reconciling projection, without rewriting transport here. |

No `ReviewJudgment` or `PublicReviewPlan` declaration was found in current src.
They are target canonical contracts in #1013/#1014, not shipped APIs that a
new adapter can simply import. Existing specialist structures are not the full
#929/#999–#1003 target semantic state either. Keep the current-source reuse
inventory distinct from the future canonical contract owner.

Recommended seam: a narrow Rust CLI/value facade over verified recorded inputs,
claim/evidence state, **recorded final-lead response**, canonical validation and
public-plan compilation. No `run` or `post` orchestration entrypoint; no provider
or proof executor inside the pure constructors. Missing actual final-lead answer
returns judgment-unavailable; deterministic concatenation of specialist prose
cannot fill it. This is a future adapter shape, not a new CLI implemented here.

Prefer a facade to broad crate surgery. If binary-private type visibility is the
only blocker, consider minimal library extraction of the existing values and
pure validators with unchanged identity/wire contracts and one implementation
owner. Inventory transitive dependencies first; do not expose RunArgs transport
capabilities just to satisfy the compiler constructor. Both choices must use
the same canonical validator/compiler; a Python verdict engine or copied
judgment schema is rejected. Language-boundary mapping is not semantic authority.

## Four persisted objects and completion law

```text
immutable verified ReviewJudgment (source/spec/pass + model-answer identity)
  -> immutable PublicReviewPlan (selected claim IDs, anchors/body digests/subject)
  + append-only DeliveryAttempt and remote-observation receipts
  -> versioned AgentReviewResult (source digests + named observation watermark)
```

The last object is a projection, not a fourth verdict. No second model call,
Markdown scraping, GateDecisionSummary reinterpretation or mutation of source
objects on retry. Preserve failed/unknown attempts. Confirmed historical delivery
may later be superseded; confirmation and currentness are separate. No receipt
does not mean no remote effect. Ambiguous submission stays unknown/reconciling
until identity-bound read-only remote observation resolves it. This packet
proposes the projection requirement, not a change to existing delivery enums.

Source-only completion waits for applicable source investigations and bounded
claim reconciliation: each mandatory review programme has settled or explicit
limited/unavailable disposition, protected final-lead budget remains, and every
required-for-scope source/evidence/model input has a valid identity. Missing or
unsupported material for the claimed scope prevents clean. Findings and limited
coverage may coexist. Optional irrelevant exclusions narrow the declared scope.
Unexecuted ProofIntents remain claim-bound evidence needs, without argv, dispatch,
candidate checkout/build/test or waiting for a nonexistent proof pool.

Source-only mode **owns no CI sufficiency result**. Represent that at a versioned
envelope/mapping boundary as no applicable CI decision owned here; do not invent
a FinalizedOutcome enum or empty Required PASS. Imported deterministic evidence
is read-only identity-bound context with its original bytes/digests/result.
Standalone clean grants no tests-passed, gate, merge or release permission.
Integrated #995 Required wind-down/zero-pending and #1013/#1014/#1277 frozen-CI
guards remain intact. Equal complete admitted inputs/policy plus the same recorded
model response yield equal semantic validation; additional executed evidence
may change judgment. Different model calls are not claimed deterministic.

## Dependency classifications and exact proposed edge effects

Class names describe prerequisites, not completion inferred from parent or issue
state. An issue spanning phases has separate rows. These are inputs for #945 and
the future #1216 registry; no readiness controller/manifest is implemented here.

| Class | Exact owners / prerequisite | Applies to | Proposed effect |
| --- | --- | --- | --- |
| shared_contract | #923/#948–#951 revision admission/joins; #940/#927/#864 scope/routing; #929/#999–#1003 semantics, final input and private boundary; #969 bounded architecture knowledge; #926 result-plane value contract | Canonical validation and #1334/#1338 reuse | Preserve canonical owners and current pure/source qualification rules. No fabricated complete source from a selected blob/digest. |
| shared_contract | #865/#1013 judgment type/validator; #1014 compiler/firewall; #765/#959 delivery value identities; #1334 source/spec/pass crosswalk | #1335 pure view and #1338 facade | Reuse one semantic engine. Prepare fixture-only work without waiting for operated service or public delivery. A missing canonical contract is a named producer gap, not permission to copy it. |
| mode_qualification | #1334 pure admission; #1335 recorded projector; #1338 recorded facade; #930/#1010/#1011 budget/accounting values | Pure/source qualification | No model invocation, dispatch, post, host or gate result. This packet is not a new prerequisite for work already explicitly permitted by those issues. |
| mode_qualification | em-ci#189/#192/#195 experiment, #196 GO; admitted #221 packet; exact engine/model/data/tool/policy identity and isolated broker budget | Credential-isolated experiment | No dependency on its future deployment, #215 delivery or completed #213/#218 operated host. Model calls require the experiment's own authorization/containment; replay tests confer none. |
| mode_qualification | Proposed source-only guard, accepted #1349 amendment, qualified canonical producer/#1338 facade and #1335 projection | Earlier standalone model-on qualification | Substitute only the integrated-execution-specific guard on this physically separate path, and only after Steven's ruling and negative proof below. Not currently adopted. |
| integrated_activation | #1001 zero-pending final input + #1008 exact proof reconsideration + #1012 protected live budgets + #969 + #1277 -> #1013; #1013 + #1003 + #1277 + completed #765 -> #1014 | Integrated live judgment/public selection | **No edge removed.** Preserve #1120 live-plan, #994/#995 wind-down/reserve and #958 -> #959 -> #960 -> #1277 prerequisites; #1277 still precedes #1007/#1008/#1012/#1013/#1014/#1015. |
| operated_host | #876 hostile-head-safe trust/credential boundary as applicable; em-ci#197 qualified host, #213/#214–#217 operated service; #218 processor consumes qualified #1338, #215 transport consumes immutable plan/delivery identities | Standalone operated activation | Exact containment, workload, isolation, real broker and transport evidence separate from pure code. #217 operating GO gates later #1337 challenge; no launch dependency on #1337 or optional #1336 intent absent the feature. |
| external_authorization | Steven's explicit #1349 architecture ruling; em-ci experiment/operating data/provider/App/host authority; exact #817 release authorization and #1285 protection disposition when applicable | Specified authorized transition only | No schema test, issue closure, draft PR or code merge grants these. Host/App/provider/release/merge authority cannot be minted by the reviewer. |

Specific #945/#1216 amendment if accepted: index #1334 pure source admission,
#1335 pure projection and #1338 pure facade distinctly from their production
inputs/activation. Record #1013/#1014 as shared schema/validator/compiler owners
and integrated-live acceptance owners, with #1277 unchanged on integrated paths.
Add a separately evidence-bound standalone activation edge rather than making
#1338 wait on proof tasks its mode cannot run. No wholesale issue dependency
deletion and no closure of the integrated owners. #1336 is optional accepted
intent; #1337 remains later challenge/reconsideration, not ordinary-review launch.

#1339's bounded installed advisory release is a separate scope. Its existing
#957 -> #958 -> #959 -> #960 -> #962 acceptance and selected release owners stay
unchanged. No source-only service/full final-lead/operations-controller backlog
is added to it by this terminology. #1343/#1379 -> #1344 byte promotion and
#817 publication remain separate; #1347 and #1354 retain their lanes.

## Small dependency truth table

Allowed means eligible for the stated bounded activity under existing owner
rules, never proof of implementation completion or live authorization.

| Shared input/recorded answer | Mode guard evidence | External authority | Requested activity | Disposition |
| --- | --- | --- | --- | --- |
| Supplied fixture values; canonical seam missing | None | None needed for pure fixture preparation | #1335 projection contract work | Allowed to prepare existing-owner recorded cases; unavailable canonical producer remains explicit, no copied schema/production claim. |
| Complete admitted fixture + recorded final answer | None | None needed for replay | #1338 deterministic validation/facade | Allowed pure/source qualification; no provider call, post or CI result. |
| Complete source, no actual final answer | Any | Any | Semantic clean | Reject clean: judgment-unavailable; formatter is not final lead. |
| Qualified canonical engine | Experimental containment | Experiment explicitly authorized | Isolated model-on experiment | Eligible only within em-ci experiment policy; no public/gate authority or future-host dependency. |
| Qualified canonical engine | Source-only negatives not proven | Architecture accepted | Standalone operated activation | Blocked: ruling alone cannot prove guard/host containment. |
| Qualified canonical engine | Source-only negatives proven | Architecture ruling absent | Earlier standalone activation | Blocked: evidence alone cannot adopt the amendment. |
| Qualified canonical engine | Accepted standalone guard + qualified host | Exact standalone operation authorized | Source-only advisory operation | Candidate eligibility after all mode-specific receipts; owns no CI result, no proof execution. Not proven by this packet. |
| Qualified canonical engine | #1277 missing | Standalone ruling present | Integrated live lead/public authority | Blocked; standalone ruling does not waive integrated safeguards. |
| Integrated prerequisites complete | Integrated guard verified | Applicable live operation authorized | Integrated activation | Existing owner acceptance applies; independent exact-head checks/review remain required. |
| Any | Any | Any | #1339 cut inferred to require service launch | Reject scope creep absent a separate recorded release amendment. |

## Negative fixture specifications (design controls, not production proof)

These named vectors are a small reviewable corpus for #1216 and mode owners.
They are **specified**, not executed by a new graph/guard validator in this PR.
Passing local documentation/policy checks establishes none of the runtime
rejections below. Each future test must exercise the real consumer and retain
inputs, exact rejection, output/state effects and independent verifier receipts.

| ID | Invalid input / perturbation | Required oracle | Positive control / owner |
| --- | --- | --- | --- |
| parent-cycle | #840/#945 parent link encoded as child completion dependency back to that child | Reject parent-as-prerequisite cycle; parent membership is identity only | File-disjoint pure child eligible; #1216 |
| experiment-deploy-cycle | em-ci#195 experiment requires its own future #197/#213/#218 deployment | Reject bootstrap cycle | Isolated authorized experiment without host deployment; em-ci#189/#195, #1216 |
| shadow-post-edge | #1335 recorded/no-post projection requires completed live #215/#959 post | Reject delivery-as-shadow-precondition | Append recorded delivery attempts or absent/not-requested context; #1335/#1216 |
| empty-required-pass | Source-only constructor coerces absent CI domain into Required=[] -> PASS | Reject CI construction; zero CI writer/executor effects | Explicit no-owned-CI mapping; imported fail/not_proven unchanged; #1334/#1338 |
| engine-swap | Evaluated engine/model/tool/policy identity replaced by unevaluated engine after qualification | Reject stale qualification and invalidate affected receipts | Exact qualified identity or fresh evaluation; #1338/em-ci#189 |
| stale-clean-suppression | Earlier same-source clean pass used to suppress requested later pass or its finding | Reject pass substitution; later adverse result survives | Duplicate event idempotent within same pass, new pass admissible; #1334/#1335/#1337 |
| ambiguous-submit | Remote accepted submission; response lost; local failed/network_post or no post-success receipt | View unknown/reconciling; never remote-absence or safe-retry assertion | Independently matched remote receipt resolves outcome/currentness; #1335, implementation tests #1370/#1317 |
| missing-lead | Complete specialist prose but absent/malformed/stale recorded final-lead answer | Judgment unavailable/limited, never committee clean | Valid recorded answer with exact evidence references; #1013/#1338 |
| source-gap | Unsupported mandatory source/programme or exhausted protected lead reserve | No clean for claimed scope; no CI failure fabrication | Explicit narrower scope with irrelevant optional exclusion; #1334/#1338 |
| cross-plane | Vary provider failure, findings/clean/limited/unavailable judgment, renderer, prepared/failed/unknown/confirmed delivery | Imported deterministic bytes/digest/result invariant; fail/not_proven never hidden | Deterministic receipt/policy changes can change independent CI only in integrated mode; #1277/#1335 |
| capability-forgery | Model/output tries gate/status/check writer, proof argv/dispatch, candidate execution or privilege grant | Reject forbidden capability before side effect; no credential/process/network write | Inert claim-bound next-evidence request; #1334/#1338, operated em-ci owners |
| private-material-loss | Existing body or unsafe inline anchor erases accepted material summary claim, or exposes private audit | Explicit valid disposition/summary fallback; no invented clean or private sink | Structural claim appears once across human/machine projection; #1003/#1014/#1335 |

## Strongest counter-read, evidence gaps and finite decision

Strongest counter-read: #1277 intentionally gates **every** live model authority
because current legacy coupling is real. The compiler/reporter remain tied to
binary orchestration and opt-in gate semantics; a facade might smuggle those
capabilities into a supposedly separate service. Splitting activation before
production non-interference could create a bypass while only schemas are tested.
Current code cannot disprove this. Retain existing integrated law unless an
explicitly accepted, physically separate source-only boundary is actually proven.

Recommendation: accept the bounded reuse/phase split as a planning amendment,
prefer the narrow facade, and keep standalone activation blocked on independently
verified mode evidence and external authority. Do not require nonexistent proof
work or synthesize CI PASS. Pure #1334/#1335/#1338 work remains allowed already.

Evidence still needed before earlier standalone activation:

- #1013/#1014 owners: canonical typed judgment/validator/compiler availability,
  exact same-recorded-input semantic parity, missing-lead/material-loss/private
  controls and fresh same-head pass handling. No canonical producer is claimed here.
- #1334/#1338 and #1277 owners: actual facade/call-graph/capability negatives;
  no gate/check/status writer, candidate executor, proof dispatcher or reviewer
  privilege minting; independently recomputed imported-CI byte/digest invariance
  under provider/judgment/delivery perturbations. Inspect-only design is insufficient.
- em-ci#189/#195/#196: experimental engine/data/model policy, identity-bound
  evaluation and explicit experiment permission. No model call occurred here.
- em-ci#197/#213/#218/#215: qualified operated isolation/capacity, broker and
  authorized transport; remote ambiguity/reconciliation evidence under
  #1370/#1317 before relying on delivery observation. Fake transport is not live proof.
- Steven: a recorded architecture ruling identifying the precise accepted
  source-only alternative guard and preserved integrated edges. No issue closure
  or passing schema test substitutes for this decision.

Finite choice for Steven (both keep this issue open pending its decision record):

1. **Accept the proposed phase split** for #945/#1216 planning. Record the exact
   standalone guard amendment and required evidence above; delegate implementation
   to existing owners. This accepts a design, not live activation or completion
   of #1013/#1014/#1277. Keep integrated safeguards and #1339 scope unchanged.
2. **Reject earlier standalone activation**. Retain current live dependencies;
   keep the shared pure contract/projector/facade preparation permitted by current
   issue bodies. Record the specific inseparable contract/containment blocker.
   Do not start a second engine, broaden this packet, or wait for unrelated platform
   work without naming the missing producer/evidence.

Executable next pure slice under current owners: #1335's fixture-only projection
can inventory recorded claim IDs, public selections, delivery observations and
watermarks and exercise duplicate/fresh-pass, summary-only, stale-view and
ambiguous-submit cases through existing source values. Where the canonical
judgment contract is absent, report producer-unavailable and coordinate its pure
seam with #1013/#1014; do not mint a replacement schema. #1338 can prepare the
recorded-input facade/admission and judgment-unavailable tests alongside that
owner. Neither task needs this recommendation adopted, a provider call, public
post, proof-pool wind-down or an operated host to perform pure fixture work.

Handoff this proposed diff to #945/#1216, #1013/#1014/#1277, #1334/#1335/#1338
and em-ci#213/#218. Parent retains independent exact-head review, Ready/merge and
architecture ruling. Rollback is to withhold the proposed mode amendment and
retain this evidence plus existing integration dependencies; no runtime rollback
is needed for documentation alone.
