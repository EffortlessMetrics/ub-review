# Tower adoption checklist v1

Source snapshot: 2026-10-15 UTC, `main`
`3dbcc2be9768b082c0896dc2360e950ffbd56a65`. This checklist prepares evaluation;
it activates nothing and grants no merge, model-call, posting, release, or
required-check authority. [Product state](PRODUCT_STATE.md) and
[adoption modes](ADOPTION_MODES.md) retain their capability/promotion contracts.
Recheck source and owner state before using this snapshot.

## Available capabilities and candidate limits

“Tower prototype” here means the bounded recorded source-review path associated
with #1334/#1335/#1338, not a shipped Tower service or a new CLI mode.
“Full CI” means integrated deterministic evidence and enforcement, whose
acceptance remains separate from source-review quality.

| Surface inspected | Exists | Does not establish |
| --- | --- | --- |
| Main: [CLI](../src/cli.rs), [gate](../src/gate.rs), [tool outcomes](../src/tools.rs), [revision identity](../src/revision_identity.rs) | `run` prepares artifacts; separate `post` delivers; model-off/artifact-only/advisory controls, revision-bound receipts, configured proof/tool gates and legacy gate enforcement exist. | Complete Required selection/scheduling, authoritative finalized CI outcome, or replacement of existing CI. |
| `test/tower-1398-authority-controls` at `dc77a28b96578b8ba2dd0f727210e34bde942109` | Recorded source export/replay and agent-result composition; authority-forgery controls in `tests/recorded_review_cli/authority.rs`. | Live model investigation, source acquisition, remote authenticity, accepted intent, or whole-review delivery. The agent export explicitly says `shadow_recorded_only_pending_dependency_review`, `not_admitted`, `unverified`, `not_qualified`. |
| `review/1398-authority-audit` at `0c2badf275a8c36a1cc0fef4d7820bf2e38a8562` | Later authority-control repair relinks prepared-payload and observation pins before testing rejection. | A passing rejection at a stale unrelated pin is not evidence that the intended authority boundary rejected the forgery. Requalify the selected candidate with valid positive controls. |
| `integration/review-repairs-20261015` at `c030f6b10b6fca3df84c16e512c09092384d9dc8` | Candidate changes to planner receipts, summary diagnostics and model-evidence provenance. | Merged, reviewed or deployed state; its proof does not transfer automatically to Tower. |
| `candidate/merge-ready-admission-20261015` at `0528c88244b28b20642218fc25c763cc01517ec0` | Dormant workflow templates, admission tests and candidate documentation; source-only Tower has no CI sufficiency authority there. | Active admission or permission to move templates into `.github/workflows`, label PRs, or alter protection. |

These identities were read through Git remote refs and inspected as source,
not adopted into this branch. Commit authorship is not live ownership. Current
PR ownership, holds, reviews and checks remain unknown here: authenticated
`gh pr list` returned `Post "https://api.github.com/graphql": Forbidden`.
The coordinating owner must supply authoritative native state before any
integration/activation decision. Preserve original branches and explicit holds;
do not infer clearance from this source inventory.

## Prototype evidence checklist

- [ ] Select one immutable engine/candidate SHA and tree, harness/adapter version,
  schema, review spec/pass/attempt, repository, base/head/reviewed identities,
  diff/path digests, policy and input packet digests. Keep candidate-head and
  synthetic merge identities distinct.
- [ ] Admit complete declared source/evidence and an actual recorded final-lead
  answer through the candidate's canonical validator/compiler. Missing answer
  means judgment unavailable; missing mandatory coverage prevents clean.
  Source-only findings can coexist with explicit limited coverage.
- [ ] Retain original answer/plan bytes, prepared-payload binding and named
  observation prefix. Reproduce export/replay equality, valid positive controls
  and correctly resealed negative controls for forged authority, bad pins,
  malformed/oversized input and stale observations. Record exact commands,
  candidate/tree and outcomes; source inspection alone is not a test pass.
- [ ] Keep imported CI bytes/digests read-only and separately identified.
  A clean source review confers no tests-passed, Required PASS, dispatch, gate,
  merge or release result. A recorded delivery observation is not authenticated
  remote delivery. Unknown effect remains unknown pending bound observation.
- [ ] Use the same canonical contracts from any supported harness. Record each
  adapter's mapping and unsupported fields. No Codex/OpenCode/Pi-specific
  orchestrator, custom controller, future proof pool or operated service is a
  prerequisite for recorded qualification. Live model or delivery pilots need
  their own authorized isolation/evidence; replay grants neither.

## Full CI evidence checklist

- [ ] Begin with the existing [model-off, artifact-only, non-required
  evaluation](ADOPTION_ADVISORY.md); retain independent required CI. Pin engine,
  Action, tools, config and exact workload. Inspect effective config, actual
  executables, status/timeout/exit receipts and retained artifact digests.
- [ ] Declare repository-owned Required obligations and compare their actual
  receipts with existing CI over the same admitted revision. Exercise failure,
  unavailable evidence, cancellation, timeout, stale-head and missing-packet
  cases. Provisioning/access failure is an execution/evidence blocker, not a
  code-test failure. A summary, zero exit, or `continue-on-error` is insufficient.
- [ ] Require the coherent authority packet (#957–#962), output containment,
  Required-first plan/scheduler, result-plane non-interference (#1277), shadow
  comparison (#1275) and explicit enforcement proof (#1015) described in
  [adoption modes](ADOPTION_MODES.md) before co-required promotion. Preserve
  legacy disagreement; source-only replay cannot discharge these obligations.
- [ ] Before sole-gate use, retain hostile-head/credential isolation, terminal
  watchdog, independent receipt verification, exact-byte install/upgrade proof,
  external calibration and tested rollback/break-glass evidence under #658/#811.
  Obtain the owner's explicit protection decision; this checklist authorizes
  no activation, CI retirement or release.

## Unavailable sensors and measurements

On this executor, `tokmd`, `cargo-allow`, `ripr`, `unsafe-review`, `ast-grep` and
`actionlint` were absent from PATH; no installation was attempted. Rust/Cargo
1.95.0 and an existing target cache are available. This is an executor
observation, not an inventory of the eventual Tower host. Use `doctor` and
`resolved-tools.json` / `tool-status.json` on the actual host; standard-image
requirements remain those in [RUNNER_IMAGE.md](RUNNER_IMAGE.md).

Missing, disabled, failed, stale, timed-out and unmeasured evidence stays
explicit. RIPR measures static mutation exposure; unsafe-review measures
native reviewability; coverage measures execution surface. None alone proves
correctness. Heavy witnesses remain off without explicit policy/authorization.
Recorded replay measures no provider reliability/cost, live latency, real
delivery usefulness, hostile-head safety or full-CI sufficiency. Sensor defects
retain their [upstream owners](SENSOR_ROUTING.md).

## Rollout and rollback

1. Keep this versioned checklist and exact evidence packet with the selected
   candidate; coordinate with existing semantic, adapter, delivery, CI and
   release owners. Refresh unknown native owner/hold state before proceeding.
2. Qualify recorded prototype inputs first. Separately evaluate a contained CI
   packet; separately authorize any live model/posting pilot. Expand only the
   plane whose acceptance evidence is complete. No custom rollout controller
   is required; an unmerged controller branch is not an adoption prerequisite.
3. Before expansion, retain the previous verified immutable pin, config, schema
   and adapter mapping plus rollback evidence. On regression, restore those or
   stop the non-required pilot; preserve original packets and delivery history.
   Never relabel old receipts, retry ambiguous remote effects blindly, move
   release tags or weaken required CI. Protection changes remain owner actions.
