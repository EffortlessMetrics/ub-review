# UB-REVIEW-SPEC-0004 - artifact contract surface

Status: authored 2026-06-06 (release surface spec wave, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Maturity: production - the contract is enforced by the artifact verifier,
`scripts/verify-bun-review-artifacts.py`. The required
`[tools.artifact-verifier]` tool (`required = true` in .ub-review.toml)
runs the verifier's `--self-test` inside the gate, regression-pinning the
enforcement tool; the full-tree contract check runs as a dedicated step of
the required ub-review/gate workflow on this repo
(.github/workflows/ub-review-gate.yml). The verifier IS the executable
spec: where this document and the script disagree, the script wins and this
document has a bug.

## Purpose

Lock which files under the run output directory automation may build
against, the schema versioning rules those files follow, the mirror, parity,
and XOR invariants the verifier enforces fail-closed, and which files are
internal decomposition that may change shape without notice. One run of
`ub-review run` writes one immutable artifact tree (docs/ARCHITECTURE.md
mutation zones: sensor artifacts immutable once emitted, `events.ndjson`
append-only, `running-summary.md` single-writer); this spec is the contract
for reading that tree.

## User question

```text
What files can I build automation against?
```

Build against the stable set below. Everything in it is either required to
exist by `require_common_tree` in the verifier, pinned to an exact
`ub-review.<name>.vN` schema string, or both. Everything outside it is
internal: it exists today, it may not exist tomorrow, and no parity rule
defends it on your behalf (the post-* receipts are the one exception:
`require_post_receipt` fail-closed-checks their status/validity fields).

## Lifecycle moment

Every run pass. `run` writes the whole tree (plan-phase artifacts at the
root, review-phase artifacts under `review/`); `post` is a separate command
that reads `review/github-review.json` and writes its own receipts
(`post-result.json`, `post-error.json`) without touching run artifacts
(src/main.rs `cmd_post`). The full-tree verifier executes as a dedicated
step of the required gate workflow, so a contract break on this repository
is caught on the PR that introduces it, before any consumer downloads the
packet.

## Consumer

```text
downstream automation     downloads the out dir as a workflow artifact and
                          reads stable files (the Bun hunt verifier flow)
the artifact verifier     scripts/verify-bun-review-artifacts.py, the
                          contract enforcement itself
gate-check                reads review/gate_outcome.json (also verifier-
                          covered since #340, require_gate_outcome)
action outputs            path mapping only: gate-outcome-path,
                          review-json-path, metrics-json-path,
                          github-review-path, post-* paths (action.yml)
```

## Inputs

The out directory (`--out`, default `target/ub-review`; action input `out`).
Verifier invocation: positional out dir plus `--expected-review-profile`,
`--expected-repo-kind`, `--min-ok-model-lanes`, and `--self-test` (no out
dir; runs the script's own fixture checks).

## Output artifact / user surface

### The stable set

Root-level (plan phase and cross-stream):

```text
plan.json                      evidence plan
resolved-profile.json          ub-review.resolved_profile.v1; carries gate
                               config that must equal resolved-plan.json's
resolved-plan.json             ub-review.resolved_plan.v1; selectors,
                               effective_model_lanes, run_pass
resolved-tools.json            ub-review.resolved_tools.v1   (mirrored)
tool-status.json               ub-review.tool_status.v1      (mirrored)
tool-gate-outcomes.json        ub-review.tool_gate_outcomes.v1 (mirrored);
                               entries are ub-review.tool_gate_outcome.v1
work_queue.json                ub-review.work_queue.v1; tasks are
                               ub-review.work_queue_task.v1
work_events.ndjson             ub-review.work_event.v1 lines
events.ndjson                  append-only run timeline; ts/kind/payload
                               per line, eight required kinds (below)
running-summary.md             single-writer; five required headings (below)
input/changed-files.txt        one changed path per line
input/diff.patch               the diff the review is anchored to
input/diff-context.json        structured diff context
lanes/<sanitized-lane>.md      one packet per effective model lane, exact
                               set match, [<lane>] prefix required
sensors/<id>/ub-review-sensor-status.json
                               for all six sensors: tokmd, cargo-allow,
                               ripr, unsafe-review, ast-grep, actionlint;
                               status ok|missing|skipped|failed|timed_out,
                               reason field mandatory
```

Root-level NDJSON streams (each is line-for-line parity with a `review/`
JSON array; see parity rules):

```text
candidates.ndjson              follow_up_questions.ndjson
resolved_candidates.ndjson     follow_up_results.ndjson
model_stages.ndjson            follow_up_outputs.ndjson
witnesses.ndjson               proof_requests.ndjson
proof_tasks.ndjson             proof_receipts.ndjson
receipt_routes.ndjson          tool_gate_outcomes.ndjson
resource_leases.ndjson
```

`review/` (the compiled review surface):

```text
gate_outcome.json              ub-review.gate_outcome.v1 (spec 0003 owns the
                               field contract; enforced by gate-check, not
                               by the verifier - see fail-closed section)
metrics.json                   integer schema_version 1; run/streams/
                               scheduler_roles/loops/phases/models; the
                               count-parity anchor for the whole tree
scheduler.json                 ub-review.scheduler.v1; exact mirror of
                               metrics.run
review.json                    compiled review: mode, posting, run_pass,
                               review_profile, shared_context_id (64-hex),
                               model_lanes, inline_comments,
                               summary_only_findings, and embedded copies of
                               terminal_state, pr_thread_context,
                               proof_requests, proof_receipts,
                               resource_leases that must equal the
                               standalone artifacts
review.md                      seven required headings (below)
terminal_state.json            ub-review.terminal_state.v1; status is one of
                               needs-reviewer-attention | sufficient |
                               artifact-only | failed-to-review
pr_thread_context.json         ub-review.pr_thread_context.v1; status
                               seeded | absent | unavailable
github-review.json             XOR github-review-skip.json (skip statuses:
                               skipped_empty_smoke |
                               skipped_artifact_only_body |
                               skipped_pass_policy)
provider-preflight-status.json provider/endpoint/status/cache_usage receipts
shared_context.md              the shared model context
shared_context_cache_block.md  byte-equal mirror of shared_context.md
shared_context_hash.txt        the hash every cache artifact must repeat
cache_manifest.json            ub-review.cache_manifest.v1
cache_events.ndjson            ub-review.cache_event.v1 lines; must include
                               kind shared_context_prepared
observations.json              the canonical observation array
unique_observations.json       deduplicated summary
merged_observations.json       lane-scoped merged summary
dropped_observations.json      suppressed-boilerplate summary
candidates.json                ub-review.candidate.v1 records
resolved_candidates.json       ub-review.resolved_candidate.v1 records
orchestrator_plan.json         ub-review.orchestrator_plan.v1
final_orchestrator_plan.json   ub-review.orchestrator_plan.v1
follow_up_results.json         follow-up lane results
follow_up_outputs.json         follow-up model outputs
follow_up_evidence.json        evidence routed into the final compile
model_stages.json              ub-review.model_stage.v1 records
final_compiler_input.json      ub-review.final_compiler_input.v2; the v2
                               example of a schema bump (PR #309)
witnesses.json                 ub-review.witness.v1 records
witness_registry.json          ub-review.witness_registry.v1
proof_requests.json            proof requests array
proof_request_groups.json      ub-review.proof_request_group.v1
proof_planner_input.json       ub-review.proof_planner_input.v1
proof_planner_output.json      ub-review.proof_planner_output.v1; tasks are
                               ub-review.proof_task.v1
proof_receipts.json            ub-review.proof_receipt.v1 records
receipt_routes.json            ub-review.receipt_routes.v1; route entries
                               ub-review.receipt_route.v1, phase
                               initial-diff-receipt | model-request-receipt
                               | follow-up-receipt
resource_leases.json           ub-review.resource_lease.v1 records
resolved-tools.json            mirror copy (must equal root)
tool-status.json               mirror copy (must equal root)
tool-gate-outcomes.json        mirror copy (must equal root)
proof_plan.md                  existence required; prose not contracted
resource_plan.md               existence required; prose not contracted
```

Per-record directories (existence tied to their array; see XOR rules):
`candidates/<sanitized-id>.json`, `proof_requests/<sanitized-id>.json`.

### Internal / debug (do not build against)

```text
box-state.json                          plan-phase diff boxing state; no
                                        schema pin, no verifier coverage
github-review-post-payload.json,        cmd_post working receipts; their
post-result.json, post-error.json,      paths are exposed as action outputs
post-stdout.json, post-stderr.txt       and they carry no schema string,
                                        but require_post_receipt
                                        fail-closed-checks their status/
                                        validity fields and requires one of
                                        post-result.json/post-error.json to
                                        exist on posting passes
observations/<lane>.ndjson              per-lane decomposition of
                                        review/observations.json; verified
                                        for consistency, but the canonical
                                        surface is the review/ array
questions/<lane>/<question>.json        per-question observation
                                        decomposition (same status)
questions/orchestrator-follow-up/*.json follow-up question packets
                                        (ub-review.follow_up_question_packet
                                        .v1); verifier-reconstructed and
                                        byte-checked, but they are model
                                        prompt material, not an automation
                                        surface
input/pr.md, input/claims.md            prompt construction inputs
review/proof_plan.md,                   markdown prose beyond "exists" is
review/resource_plan.md                 uncontracted
ci-audit/*                              audit-ci output; contract pending
                                        UB-REVIEW-SPEC-0007
```

"Internal" does not mean unverified - several of these are exact-checked by
the verifier for internal consistency. It means: the canonical surface is
elsewhere, and the decomposition (file layout, naming, prompt text) may
change in any release that also updates the verifier.

## Required fields

### Schema versioning rules

Every schema-bearing stable JSON artifact carries a literal schema string
`ub-review.<name>.vN` and the verifier pins the exact string - a schema
mismatch is a hard fail, including case or version drift. `N` bumps on any
breaking shape change; the live example is
`ub-review.final_compiler_input.v2` (PR #309), which added
`follow_up_resolved_candidate_ids` and changed the meaning of
`inline_comments`/`summary_only_findings` to exclude candidates the
follow-up pass resolved as refuted or dropped
(scripts/verify-bun-review-artifacts.py `require_final_compiler_input`;
src/main.rs). Consumers must match schema strings exactly and treat an
unknown version as unreadable, the same way the verifier does.

Deliberate exceptions:

- `plan.json`, `input/diff-context.json`,
  `review/provider-preflight-status.json`, and the
  `review/github-review.json` / `github-review-skip.json` payloads carry no
  schema string at all; they are existence- and field-checked only.
- Bare-array artifacts (`review/proof_requests.json`,
  `follow_up_results.json`, `observations.json`, and the like) pin schema
  strings per record, not on the file.
- `review/metrics.json` uses an integer `schema_version: 1`, not a string.
- `events.ndjson` lines have no schema field; each line must be an object
  with non-empty string `ts`, non-empty string `kind`, and a `payload` key,
  and the run must contain all eight kinds: `run_started`,
  `evidence_stream_started`, `evidence_stream_completed`,
  `model_stream_started`, `model_stream_completed`, `proof_stream_started`,
  `proof_stream_completed`, `run_finished` (verifier `require_events`).
- Markdown artifacts contract by required headings, not schema.
  `running-summary.md` must contain `## Missing evidence`,
  `## Provider preflights`, `## Model lane status`, `## Lane packets`,
  `## Review efficiency`, and a `Follow-up results:` efficiency line
  (verifier `require_summary`). `review/review.md` must contain
  `## Decision`, `## Confirmed findings`, `## Summary-only findings`,
  `## Failed objections`, `## Residual risk`, `## Parked follow-ups`,
  `## Missing or failed evidence` (verifier `require_review`).

There are no JSON Schema files; the pinned strings plus the verifier's field
checks are the registry.

### Root/review mirror rules (exact equality)

The tool registry trio is written twice - once at the root after the plan
phase, once under `review/` so the review packet is self-contained
(src/main.rs `write_tool_status_artifacts`,
`write_tool_gate_outcome_artifacts`). Both copies are required and the
verifier demands exact equality of the parsed JSON values
(`require_tool_registry_artifacts`,
`require_tool_gate_outcome_artifacts`):

```text
resolved-tools.json        == review/resolved-tools.json
tool-status.json           == review/tool-status.json
tool-gate-outcomes.json    == review/tool-gate-outcomes.json
```

Further exact mirrors:

```text
review/shared_context_cache_block.md  == review/shared_context.md
cache_manifest.shared_context_hash    == shared_context_hash.txt contents;
                                         every manifest lane and every cache
                                         event repeats the same hash
review.json.terminal_state            == review/terminal_state.json
review.json.pr_thread_context         == review/pr_thread_context.json
review.json.proof_requests            == review/proof_requests.json
review.json.proof_receipts            == review/proof_receipts.json
review.json.resource_leases           == review/resource_leases.json
review/scheduler.json                 == metrics.run (streams,
                                         scheduler_roles, loops, overlaps,
                                         phases all compared field-exact)
resolved-profile.json.gate            == resolved-plan.json.gate
```

### NDJSON parity pairs

Each root NDJSON stream must match its JSON array line-for-line: same line
count, and line `i` parsed must equal array element `i`. Pairs:

```text
candidates.ndjson           <-> review/candidates.json
resolved_candidates.ndjson  <-> review/resolved_candidates.json
model_stages.ndjson         <-> review/model_stages.json
witnesses.ndjson            <-> review/witnesses.json
proof_requests.ndjson       <-> review/proof_requests.json
proof_tasks.ndjson          <-> review/proof_planner_output.json tasks
proof_receipts.ndjson       <-> review/proof_receipts.json
receipt_routes.ndjson       <-> review/receipt_routes.json
tool_gate_outcomes.ndjson   <-> tool-gate-outcomes.json outcomes
resource_leases.ndjson      <-> review/resource_leases.json
follow_up_results.ndjson    <-> review/follow_up_results.json
follow_up_outputs.ndjson    <-> review/follow_up_outputs.json
follow_up_questions.ndjson  <-> orchestrator plan follow_up_tasks
```

Per-lane `observations/<lane>.ndjson` entries must match
`review/observations.json` (the canonical array; there is no root
`observations.ndjson`).

### Strict count parity (metrics is the anchor)

`review/metrics.json` counts must equal the actual array lengths - the
verifier compares, it does not trust (`require_metrics`):

```text
metrics.observations            == len(review/observations.json)
metrics.proof_requests          == len(review/proof_requests.json)
metrics.proof_receipts          == len(review/proof_receipts.json)
metrics.resource_leases         == len(review/resource_leases.json)
metrics.inline_comments         == len(review.json.inline_comments)
metrics.summary_only_findings   == len(review.json.summary_only_findings)
metrics.lane_packets            == len(effective_model_lanes)
metrics.final_follow_up_tasks   == len(final_orchestrator_plan
                                       .follow_up_tasks)
                                == terminal_state.final_follow_up_tasks
metrics.terminal_state          == terminal_state.status
```

On a skipped review payload: `metrics.review_payload_status` must be one of
the skip statuses, and `github_review_body_bytes` and
`github_review_comments` must both be exactly 0.

### XOR and set-equality rules

- `review/github-review.json` XOR `review/github-review-skip.json`: exactly
  one exists, never both, never neither (`require_common_tree`).
- `lanes/*.md` must be exactly the set derived from
  `resolved-plan.json.selectors.effective_model_lanes` - an extra packet
  file fails as hard as a missing one, and each packet must contain its
  `[<lane>]` prefix.
- `candidates/` must exist when `review/candidates.json` is non-empty, and
  then its files must be exactly `<sanitized-id>.json` per candidate and
  each must equal the array record; an empty leftover directory is
  tolerated when the array is empty (`require_candidate_artifacts`).
- `proof_requests/` follows the same rule against
  `review/proof_requests.json` (`require_proof_request_files`).
- `questions/orchestrator-follow-up/` exists iff the orchestrator plan has
  follow-up tasks, with exact file-set and content match (next section).

### Hardcoded fields (current single-implementation reality)

The verifier pins these to literal values; they are honest documentation of
the only implementation that exists, not configurable knobs:

```text
metrics.run.scheduler_profile                  "default-three-stream-v0"
metrics.run.concurrency_model                  "profiled-stream-scheduler-v0"
metrics.run.local_proof_wall_excludes_model_wait  true
scheduler phases must include                  (evidence, sensors-and-packet)
                                               (proof, initial-diff-broker)
                                               (compiler, final)
cache_manifest.explicit_cache_provider         "minimax"
cache_manifest.explicit_cache_endpoint         "anthropic-messages"
cache_manifest.cache_lifetime                  "provider-ephemeral"
                                               (Rust-hardcoded only, not
                                               verifier-pinned)
cache_manifest.cache_block_path                "review/shared_context_cache_block.md"
cache_manifest.hash_path                       "review/shared_context_hash.txt"
cache_manifest.events_path                     "review/cache_events.ndjson"
```

(src/main.rs cache manifest construction; verifier `require_cache_artifacts`,
`require_run_loop_metrics`, `require_scheduler_artifact`.) The cache
provider/endpoint hardcoding mirrors `model_cache_mode` being implemented
only for MiniMax over anthropic-messages; the provider surface that would
generalize it is spec 0006 territory (#310 remainder).

## Advisory vs blocking behavior

Artifacts never gate by themselves. The contract becomes blocking through
two distinct enforcement points:

- The required `[tools.artifact-verifier]` tool (`required = true`,
  .ub-review.toml) runs the verifier's `--self-test` inside the gate,
  regression-pinning the enforcement tool. The full-tree contract check
  runs as a dedicated step of the required ub-review/gate workflow on this
  repo (ub-review-gate.yml), so a contract break fails that required check
  as a CI step failure - not as a required-sensor failure under spec 0003,
  and with no sensor receipt. Consumer repos get artifact-contract blocking
  only by adding an equivalent verifier step to their own required
  workflow - otherwise the contract is advisory and enforced only upstream,
  on this repo's own PRs.
- `review/gate_outcome.json` is enforced by `ub-review gate-check`
  (src/main.rs `cmd_gate_check`), not by the verifier - it is the one
  stable artifact outside `require_common_tree`. Spec 0003 owns that
  contract; this spec only locks its location (written unconditionally to
  `review/gate_outcome.json` on every review compile, path not
  configurable) and schema string.

Everything in the internal tier is advisory by definition: no parity rule,
schema pin, or gate consequence protects a consumer reading it.

## Fail-closed behavior

The verifier fails closed and fails loud: every check calls `fail()`, which
prints the violation and exits non-zero on the first breach. There is no
warning tier and no partial pass. Specifically:

- Missing required file: fail. Extra file in an exact-set directory
  (`lanes/`, `candidates/`, `proof_requests/`,
  `questions/orchestrator-follow-up/`): fail.
- Any mirror above compares with `!=` on parsed JSON or raw text - exact
  equality, not subset or fuzzy match. This includes the follow-up packet
  prompt mirror: the verifier independently reconstructs each expected
  packet, including rendering the full multi-line `prompt` string from the
  orchestrator plan task (`expected_follow_up_question_packet`,
  `follow_up_question_prompt` in the verifier, twinned with
  `follow_up_question_packet`, `render_follow_up_question_prompt` in
  src/main.rs), and requires the artifact to equal the reconstruction. A
  one-character prompt drift between the Rust renderer and the Python
  reconstruction fails the gate - by design, that twin rendering is the
  proof the packet format did not silently move.
- Schema strings, enum values (terminal state, pr-thread status, sensor
  status, posting, skip statuses), and the hardcoded fields are matched
  exactly; unknown values fail rather than pass through.
- `--self-test` runs the script's own fixture suite, including
  false-pass regression checks (for example
  `self_test_tool_gate_outcome_false_pass_fails`), and runs in CI so the
  enforcement tool itself is regression-pinned.

`gate-check` has its own fail-closed contract (exact string `pass`, exact
schema, missing/null/case-drift all fail) - inherited from spec 0003.

## Trust boundary / non-claims

```text
The verifier is the executable spec; this document is its commentary.
A file outside the stable set is not a contract, even if it looks stable.
Exact equality is the default; anything weaker is named here explicitly.
```

Honest current-state limits a consumer must know:

- `gate_outcome.json` is enforced twice since #340: gate-check turns it
  into the verdict, and the verifier audits it on every full-tree run
  (`require_gate_outcome`: schema, conclusion-iff-reasons, receipt-pointer
  resolution, count coherence, terminal_status mirror).
- The gate config block that `resolved-profile.json`/`resolved-plan.json`
  must carry includes `synchronize_mode` as a required non-empty string
  (verifier `require_gate_config`) even though no functional code consumes
  the field (#306). The artifact contract currently pins an inert knob;
  resolving #306 must update the verifier and this spec together.
- `tool-gate-outcomes.json` entries route receipts through
  `sensors/<tool>/gate-decision.json`, which the ripr sensor produces in
  production since #335 (#316 closed): verbatim badge-json stdout, threshold
  on `counts.unsuppressed_exposure_gaps`, two real blocks (PR #342, #346).
  Known depth limit: the receipt carries counts only, so a tool-gate red is
  not diagnosable to specific findings from the artifact tree (#347).
- Proof receipt and resource lease edge statuses (lease `absent`,
  `base_patch_failed` routing, manual-cost allowlist path) have named test
  gaps (#312); treat rare status values in `proof_receipts.json` /
  `resource_leases.json` as stable in shape but under-exercised in
  production.
- Sensor status receipts are required and shape-pinned, but the quality of
  their `reason` strings has known defects upstream (cargo-allow
  foreign-dialect failures #318, tokmd version-pin rejections #319). The
  xtask precommit receipt surface (#317, #320, #321) is a different
  artifact tree entirely and is out of scope here (sensor integration is
  spec 0005).
- `ci-audit/*` artifacts exist with v1 schema strings but their contract is
  deliberately deferred to spec 0007; do not build against them from this
  spec.
- Never claim the artifact tree proves code correct or UB-free (umbrella
  0001); the tree records what ran and what it saw, including missing
  evidence as missing evidence.

## Artifact maturity

Maturity is a promise about shape stability, not about enforcement -
several experimental artifacts below are already verifier-required.
Changing a row's maturity tier is a spec PR. A deprecation gets one
minor-version overlap during which both the old and the new artifact are
written. Nothing in this table may be removed or reshaped without updating
the verifier and this table in the same PR - the same rule
`final_compiler_input.v2` followed (PR #309).

Tiers: stable - verifier- or gate-check-enforced contract a consumer may
build on. experimental - schema'd and (mostly) enforced but young; shape
may still move via a verifier+spec PR (the issue-capture and broker
artifacts, the coverage sidecar, ci-audit/*). internal - everything else
under the out dir; no contract, canonical surface elsewhere.

Verifier status values: required - checked on every full-tree run, by the
named function. conditional - checked when present, presence rule named.
gate-check - enforced by `ub-review gate-check` (src/main.rs
`cmd_gate_check`), not the verifier. none (tests only) - pinned only by the
named Rust test in src/main.rs. The schema column abbreviates
`ub-review.<name>.vN` to `<name>.vN`; the pinned literal always carries the
`ub-review.` prefix.

| artifact | maturity | schema | consumer | verifier status |
|---|---|---|---|---|
| plan.json | stable | none (existence only) | downstream automation | required (require_common_tree) |
| resolved-profile.json | stable | resolved_profile.v1 | downstream automation | required (require_profile_artifacts, require_gate_config) |
| resolved-plan.json | stable | resolved_plan.v1 | downstream automation; verifier (lane-set source) | required (require_profile_artifacts; lane set in require_common_tree) |
| resolved-tools.json + review/ mirror | stable | resolved_tools.v1 | downstream automation | required (require_tool_registry_artifacts; exact mirror equality) |
| tool-status.json + review/ mirror | stable | tool_status.v1 | downstream automation | required (require_tool_registry_artifacts) |
| tool-gate-outcomes.json + review/ mirror | stable | tool_gate_outcomes.v1; entries tool_gate_outcome.v1 | downstream automation; gate-check cross-check | required (require_tool_gate_outcome_artifacts) |
| work_queue.json | stable | work_queue.v1; tasks work_queue_task.v1 | downstream automation | required (require_work_queue_artifacts) |
| work_events.ndjson | stable | work_event.v1 lines | downstream automation | required (require_work_queue_artifacts) |
| events.ndjson | stable | none (ts/kind/payload; eight required kinds) | downstream automation | required (require_events) |
| running-summary.md | stable | five required headings | humans (GitHub step summary) | required (require_summary) |
| input/changed-files.txt, input/diff.patch, input/diff-context.json | stable | none | downstream automation | required (require_common_tree) |
| lanes/<sanitized-lane>.md | stable | `[<lane>]` prefix; exact set vs effective_model_lanes | humans; downstream automation | required (require_common_tree, set equality) |
| sensors/<id>/ub-review-sensor-status.json (all six sensors) | stable | status enum + mandatory reason | downstream automation | required (require_common_tree, require_sensor_receipts) |
| root NDJSON streams (candidates, resolved_candidates, model_stages, witnesses, proof_requests, proof_tasks, proof_receipts, receipt_routes, tool_gate_outcomes, resource_leases, follow_up_results, follow_up_outputs, follow_up_questions) | stable | per-stream vN lines | downstream automation | required (per-stream require_* functions, line parity with review/ arrays) |
| review/gate_outcome.json | stable | gate_outcome.v1 (spec 0003 owns fields) | gate-check | required (require_gate_outcome, #340) + gate-check (cmd_gate_check) |
| review/metrics.json | stable | integer schema_version 1 | downstream automation; verifier count anchor | required (require_metrics) |
| review/scheduler.json | stable | scheduler.v1 | downstream automation | required (require_scheduler_artifact, mirror of metrics.run) |
| review/review.json | stable | none on file; embedded mirrors contracted | downstream automation (action output review-json-path) | required (require_review) |
| review/review.md | stable | seven required headings | humans | required (require_review) |
| review/terminal_state.json | stable | terminal_state.v1 | downstream automation; gate-check cross-check | required (require_review; status mirror in require_gate_outcome) |
| review/pr_thread_context.json | stable | pr_thread_context.v1 | downstream automation | required (require_review) |
| review/github-review.json XOR github-review-skip.json | stable | none (field-checked; skip statuses pinned) | `ub-review post` (cmd_post); downstream automation | required (require_common_tree XOR; require_review; require_pr_review_body_policy) |
| review/provider-preflight-status.json | stable | none | downstream automation | required (require_common_tree; receipt fields via require_model_receipts) |
| review/shared_context.md + shared_context_cache_block.md + shared_context_hash.txt + cache_manifest.json + cache_events.ndjson | stable | cache_manifest.v1, cache_event.v1; byte-equal mirror + repeated hash | downstream automation; verifier (mirror proof) | required (require_cache_artifacts) |
| review/observations.json + unique/merged/dropped_observations.json | stable | per-record fields, grouped records | downstream automation | required (require_metrics, require_observation_schema, require_observation_summary_artifacts) |
| review/candidates.json + resolved_candidates.json | stable | candidate.v1, resolved_candidate.v1 | downstream automation | required (require_candidate_artifacts, require_resolved_candidate_artifacts) |
| review/orchestrator_plan.json + final_orchestrator_plan.json | stable | orchestrator_plan.v1 | downstream automation | required (require_orchestrator_plan, expected_final_orchestrator_plan) |
| review/follow_up_results.json + follow_up_outputs.json + follow_up_evidence.json | stable | per-record fields | downstream automation | required (require_follow_up_results/_outputs/_evidence + schema checks) |
| review/model_stages.json | stable | model_stage.v1 records | downstream automation | required (require_model_stage_artifacts) |
| review/final_compiler_input.json | stable | final_compiler_input.v2 | downstream automation | required (require_final_compiler_input) |
| review/witnesses.json + witness_registry.json | stable | witness.v1, witness_registry.v1 | downstream automation | required (require_witness_artifacts, require_witness_registry) |
| review/proof_requests.json + proof_request_groups.json + proof_planner_input.json + proof_planner_output.json + proof_receipts.json | stable | proof_request_group.v1, proof_planner_input.v1, proof_planner_output.v1, proof_task.v1, proof_receipt.v1 | downstream automation | required (require_proof_request_groups, require_proof_planner_artifacts, schema checks) |
| review/receipt_routes.json + resource_leases.json | stable | receipt_routes.v1/receipt_route.v1, resource_lease.v1 | downstream automation | required (require_receipt_route_artifacts, require_resource_lease_artifacts) |
| review/proof_plan.md, review/resource_plan.md | stable (existence only) | none; prose uncontracted | humans | required (require_common_tree) |
| candidates/<sanitized-id>.json | stable | candidate.v1 copies, exact set | downstream automation | conditional (require_candidate_artifacts; dir required iff array non-empty) |
| proof_requests/<sanitized-id>.json | stable | per-record copies, exact set | downstream automation | conditional (require_proof_request_files; dir required iff array non-empty) |
| review/issue_candidates.json + issue_candidates.ndjson (root twin) | experimental | issue_candidate.v1 records | humans; the broker | required (require_issue_capture_artifacts; full tree since #345) |
| review/issue_actions.json + issue_actions.ndjson (root twin) | experimental | issue_action.v1 records; run-side vocabulary excludes opened/failed_to_open | humans; the broker | required (require_issue_capture_artifacts; one action per candidate) |
| review/suggested_issues.md | experimental | none (rendered issue drafts) | humans (PR body links here since #346) | required (require_issue_capture_artifacts, existence) |
| review/issue_broker_plan.json + issue_broker_plan.ndjson (root twin) | experimental | issue_broker_plan.v1 records | the broker (run decides and renders; post reads the plan) | conditional (require_issue_broker_artifacts; written only when [issues] mode=open-high-confidence, #348) |
| review/issue_broker_results.json + issue_broker_results.ndjson (root twin) | experimental | issue_broker_result.v1 records | humans; downstream automation (the broker's receipts) | conditional (require_issue_broker_artifacts; post-side, checked when present; results without a plan fail) |
| sensors/coverage/status.json (+ coverage-summary.json, changed-lines.json, upload.json, lcov.info) | experimental | coverage_status.v1, coverage_summary.v1 | gate-check (coverage tool gate); downstream automation | conditional (require_coverage_status_artifact; runs when tool-status carries the coverage tool) |
| ci-audit/inventory.json, history.json, costs.json, correlation.json, recommendations.json | experimental | ci_inventory.v1, ci_history.v1, ci_costs.v1, ci_correlation.v1, ci_recommendations.v1 | setup-ci (spec 0008, planned); humans; contract deferred to spec 0007 | none (tests only - ci_audit_artifacts_carry_schema_fields_and_receipts, src/main.rs) |
| ci-audit/audit-report.md | experimental | none (tier-ordered report) | humans | none (tests only - ci_audit_report_lines_carry_receipts_without_boilerplate, src/main.rs) |
| post-result.json / post-error.json | internal | none | downstream automation via action outputs post-result-path / post-error-path | conditional (require_post_receipt; one must exist on posting passes, status/validity fields fail-closed) |
| everything else under the out dir (box-state.json, post payload/stdout/stderr receipts, observations/<lane>.ndjson, questions/**, input/pr.md, input/claims.md) | internal | none | none contracted | none (some internally exact-checked; no row here defends them) |

### Deprecated

None today. When a row is deprecated it moves to this subsection naming the
replacement, both artifacts are written for one minor-version overlap, and
the verifier keeps checking both until the removal PR deletes the old row,
the old writer, and the old check together.

## Validation commands

```bash
python scripts/verify-bun-review-artifacts.py --self-test
python scripts/verify-bun-review-artifacts.py target/ub-review \
  --expected-review-profile ub-review-self --expected-repo-kind ub-review
cargo test --bin ub-review --locked    # artifact writers and the Rust side
                                       # of twin-rendered packets are pinned
                                       # in the inline tests
ub-review gate-check \
  --gate-outcome target/ub-review/review/gate_outcome.json \
  --fail-on-gate auto --mode intelligent-ci   # the gate_outcome leg
```

## Implementation PR slices

This spec is docs-only. Open contract-surface work it routes:

```text
#316   DONE (#335): sensors/ripr/gate-decision.json produced in production;
       the tool-gate receipt route no longer dangles
#347   deepen the gate-decision receipt past counts: per-finding
       exposure-gap detail so a tool-gate red is diagnosable from artifacts
#306   wire or delete [gate].synchronize_mode; either way, update
       require_gate_config and this spec in the same PR
#312   close the proof-broker edge-status test gaps so rare receipt/lease
       statuses are exercised, not just shaped
0007   give ci-audit/* its own contract spec before anyone builds on it
0006   provider/cache surface that would un-hardcode the cache manifest
       provider fields (#310 remainder)
```

Rule for all future artifact changes: a shape change ships in the same PR as
its verifier change, and a schema bump (`vN+1`) whenever the change is
breaking - exactly how `final_compiler_input.v2` landed (PR #309).

## Release note claim

```text
ub-review emits stable gate, proof, tool, resource, and review artifacts.
```

Concretely claimable: every stable artifact is existence-required,
schema-pinned, or both by a fail-closed verifier that runs inside this
repository's own required gate and self-tests in CI. Not claimable: that
internal-tier files are stable, that `ci-audit/*` has a contract yet, or
that the verifier covers `gate_outcome.json` (gate-check does).

## The six reliance questions

What can a user rely on?
The stable set's existence on every run; exact `ub-review.<name>.vN` schema
strings; root/review mirror equality for the tool registry trio; NDJSON
line-for-line parity with the JSON arrays; metrics counts equal to array
lengths; the github-review XOR skip rule; lane packet set equality;
`final_compiler_input.v2` excluding follow-up-refuted/dropped candidates;
required headings in running-summary.md and review.md; the eight required
events.ndjson kinds; append-only events and immutable emitted artifacts.

What can break the gate?
On this repository: any verifier failure, because the full-tree verifier
runs as a dedicated step of the required ub-review/gate workflow (a CI
step failure of that required check) - a missing file, a drifted schema
string, a count mismatch, a broken mirror (including the follow-up packet
prompt mirror), or both/neither on the XOR pair. Plus, separately, a
`gate_outcome.json` that gate-check cannot read as an exact `pass`.

What is only advisory?
The whole internal tier: box-state.json, post-* receipts (shape-wise; note
require_post_receipt does fail-closed-check their status/validity fields),
per-lane observation NDJSON and per-question JSON decomposition, follow-up
question packets as a surface, input/pr.md and input/claims.md, proof_plan.md and
resource_plan.md prose, and ci-audit/* pending spec 0007. Also the entire
contract on consumer repos that do not add an equivalent verifier step to
their own required workflow.

What is visible in the PR?
Almost none of this. running-summary.md is appended to the GitHub step
summary (action.yml), and the grouped review posts from
github-review.json on posting passes. Everything else is artifact-only by
design (docs/REVIEW_BODY_CONTRACT.md).

What is artifact-only?
The tree itself: plan, tool, sensor, cache, observation, candidate,
follow-up, witness, proof, lease, scheduler, and metrics artifacts, plus
gate_outcome.json and the skip receipt on quiet passes. The PR thread never
carries status tables; the artifacts carry everything.

What does success look like in ten minutes?
Run `ub-review run` against any PR of this repo, then point the verifier at
the out dir with this repo's expected profile flags. It exits 0 with a
one-line verified summary, or names the first violated invariant. Then break the contract on purpose -
delete `review/tool-status.json` or edit one byte of
`review/shared_context_cache_block.md` - and rerun: the verifier fails
naming exactly that mirror. On a PR of this repo, that same failure fails
the dedicated verifier step of the required gate workflow and the check
goes red. The contract is not this document; it is the script that just
refused your byte.