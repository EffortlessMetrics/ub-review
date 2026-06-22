# UB-REVIEW-SPEC-0015 — follow-up orchestrator lifecycle

Status: authored 2026-06-22 (Wave 6+, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Related: [SPEC-0004](UB-REVIEW-SPEC-0004-artifact-contract.md) (promotes
orchestrator/follow-up artifacts to stable, verifier-required tier),
[SPEC-0012](UB-REVIEW-SPEC-0012-proof-broker.md) (broker entry points: the
follow-up broker is one of the four), [SPEC-0011](UB-REVIEW-SPEC-0011-lane-doctrine.md).
Maturity: production — the orchestrator runs on every `ub-review run` with
`--mode auto`; the follow-up model pass and resolution reconciliation are
test-pinned. This spec owns the lifecycle that SPEC-0004 only lists by
artifact filename.

## Purpose

Name the contract for the follow-up orchestrator: the component that turns
unresolved candidates and observations into bounded model-lane questions,
routes proof evidence back into them, and reconciles the final dispositions
(confirmed / refuted / dropped / parked) that the review compiler consumes.
SPEC-0004 promotes `orchestrator_plan.v1`, `follow_up_question_packet.v1`,
and the follow-up results/outputs/evidence trio to **stable, verifier-required**
artifacts, but the lifecycle producing them was code-only.

## Architecture

```text
candidates + observations (unresolved)
        │
        ▼
follow_up_task_for_group / follow_up_task_for_observation_group
        │  (builds a FollowUpQuestionTask per evidence need)
        ▼
follow_up_question_packet.v1  ──▶  model lane
        │                              │
        │                              ▼
        │                    follow_up_output.v1 (one per candidate/observation)
        │                              │
        ▼                              ▼
resolved_candidate_records ◀── follow_up_result.v1
        │  (reconciles: prior + current outputs)
        ▼
resolved_candidates.v1 ──▶ review compiler (excludes refuted/dropped)
```

## Follow-up question lifecycle

1. **Evidence need detection**: `candidate_evidence_need`
   (`follow_up_routing.rs:73`) and `observation_evidence_need` (`:94`)
   classify what evidence each unresolved candidate/observation needs.

2. **Task construction**: `follow_up_task_for_group` (`:129`) and
   `follow_up_task_for_observation_group` (`:159`) build a
   `FollowUpQuestionTask` per group, tagged with a stage
   (primary / secondary / tertiary).

3. **Question packet**: each task is written as a
   `follow_up_question_packet.v1` artifact
   (`follow_up_packet_artifact_path`, `proof_planner_lane.rs:314`),
   consumed by the model lane.

4. **Model pass**: `run_follow_up_model_pass` (`proof_planner_lane.rs:129`)
   drives the model lane, producing `follow_up_output.v1` records
   (`follow_up_output_record`, `:388`) — one per candidate/observation the
   pass touched.

5. **Result aggregation**: `follow_up_result` (`:474`) and
   `follow_up_result_artifacts` (`:349`) write the `follow_up_result.v1`
   artifact linking outputs back to tasks.

## Stage semantics

`follow_up_stage_reason` (`follow_up_routing.rs:207`) distinguishes two stages:

| Stage | Meaning |
|---|---|
| `tertiary` | Routed evidence or prior disposition is available; the model should refine, refute, drop, or park — not restate the concern. |
| _(default)_ | No routed proof receipt is available; ask for the smallest remaining evidence or proof request. |

## Routed evidence

The orchestrator routes proof-receipt, resource-lease, and tool-gate evidence
back into follow-up tasks via:

- `proof_receipt_routed_evidence` (`:354`) — extracts the evidence from a
  `ProofReceipt`.
- `resource_lease_routed_evidence` (`:366`) — from a `ResourceLease`.
- `tool_gate_outcome_routed_evidence` (`:388`) — from a `ToolGateOutcomeEntry`.
- `routed_receipt_excerpts_for_task` (`:329`) — bounded stdout/stderr excerpts
  (`ROUTED_RECEIPT_STDERR_TAIL_BYTES = 1200`,
  `ROUTED_RECEIPT_STDOUT_TAIL_BYTES = 600`).

Routed status is derived per evidence type:
`routed_status_for_proof_receipt` (`:419`),
`routed_status_for_tool_gate_outcome` (`:410`).

## Resolution reconciliation

`resolved_candidate_records` (`issue_broker.rs:438`) reconciles each
candidate's final disposition from its linked follow-up outputs plus any
prior-resolved-candidates (from the previous run's
`review/prior_resolved_candidates.json`, auto-discovered by `action.yml`).

### Resolution states

`resolve_candidate_from_follow_ups` produces one of:

| `resolved_status` | Meaning | Review-compiler effect |
|---|---|---|
| `confirmed` | Follow-up evidence supports the candidate's original status. | Kept in the review. |
| `refuted` | Follow-up evidence contradicts the candidate. | **Excluded** via `follow_up_resolved_away_candidate_ids` (`:470`). |
| `dropped` | Follow-up demoted the candidate as not worth surfacing. | **Excluded** via the same set. |
| `parked-follow-up` | Follow-up ran but could not resolve; deferred. | **Kept** — parked items render in the parked section. |

> `follow_up_resolved_away_candidate_ids` deliberately excludes
> `parked-follow-up` — parked items keep their surface. Only `refuted` and
> `dropped` are "resolved away" (excluded from the final review). Pinned by
> `follow_up_resolved_away_excludes_refuted_and_dropped_but_keeps_parked`
> (`main.rs:22792`).

### Resolution source

The `resolution_source` field records where the disposition came from:

- `"current-run"` — from this run's follow-up outputs.
- `"prior-resolved-candidates"` — inherited from the previous run's
  `resolved_candidates.json` (auto-discovered via the action's
  `prior-resolved-candidates` input).

When source is `prior-resolved-candidates`, the artifact
`review/prior_resolved_candidates.json` is added to the receipt's
`source_artifacts`.

## Artifact schemas

All schema constants in `src/artifacts.rs:27-47`:

| Artifact | Schema | Stable tier |
|---|---|---|
| Orchestrator plan | `ub-review.orchestrator_plan.v1` | stable, verifier-required |
| Orchestrator evidence group | `ub-review.orchestrator_evidence_group.v1` | stable |
| Orchestrator observation group | `ub-review.orchestrator_observation_group.v1` | stable |
| Orchestrator routed evidence | `ub-review.orchestrator_routed_evidence.v1` | stable |
| Follow-up question packet | `ub-review.follow_up_question_packet.v1` | stable, verifier-required |
| Follow-up question | `ub-review.follow_up_question.v1` | stable |
| Follow-up output | `ub-review.follow_up_output.v1` | stable |
| Follow-up result | `ub-review.follow_up_result.v1` | stable |
| Follow-up evidence | `ub-review.follow_up_evidence.v1` | stable |
| Resolved candidates | `ub-review.resolved_candidate.v1` | stable |

## Verification

The lifecycle is test-pinned by:

- `resolved_candidate_records_capture_follow_up_dispositions` (`main.rs:22140`)
  — pins the reconciliation logic.
- `follow_up_resolved_away_excludes_refuted_and_dropped_but_keeps_parked`
  (`main.rs:22792`) — pins the resolved-away semantics.
- The follow-up broker integration tests (SPEC-0012 §Broker entry points).

## Non-claims

The orchestrator does **not** claim:

- **That a `confirmed` candidate is correct.** Confirmation means the
  follow-up evidence was consistent with the original status, not that the
  original status was right.
- **That `parked-follow-up` will eventually resolve.** Parking may persist
  indefinitely if the evidence need cannot be met.
- **Proof.** Follow-up questions and their outputs are model-derived evidence
  for the review compiler, not deterministic proof (which is the proof
  broker's domain, SPEC-0012).
