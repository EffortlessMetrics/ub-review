# Work queue and packet policy

`ub-review` is a CI gate that starts evidence, model, and proof work together.
The queue decides what must finish before the first packet, what may arrive
late, and what affects the gate without becoming PR-body text.

## Task Classes

Every task has one packet policy:

| Policy | Meaning |
|---|---|
| `must-run` | Required by repo policy; affects gate status. |
| `include-if-ready` | Included in the initial model packet only if ready before the packet deadline. |
| `late-follow-up` | Routed to named lanes when the receipt lands. |
| `adaptive` | Added by the proof planner or orchestrator when the PR needs it. |
| `artifact-only` | Written for audit, not reviewer-facing by default. |
| `gate-only` | Affects status; reaches the PR body only when it changes trust. |

Late is not missing. If a task misses the packet deadline, it becomes pending
queue state and may still produce a receipt for follow-up routing.

## Required Fields

No local task runs without:

```json
{
  "id": "base-tests-red-green-ffi",
  "kind": "focused-test",
  "source": "proof-planner",
  "priority": "high",
  "packet_policy": "late-follow-up",
  "deadline_sec": 300,
  "consumers": ["tests-oracle", "opposition", "compiler"],
  "gate_policy": "trust-affecting",
  "dedupe_key": "bun-ffi-to-buffer-red-green",
  "initial_packet_status": "pending_initial_packet",
  "lease": {
    "cpu": 2,
    "memory_mb": 2048,
    "disk_mb": 1024,
    "network": false,
    "timeout_sec": 300
  },
  "receipt_path": "review/proof_receipts.json"
}
```

The proof broker executes commands. Lanes request proof; they do not shell out.

`initial_packet_status` records what the first model packet can know:

| Status | Meaning |
|---|---|
| `ready_for_initial_packet` | The task is planned, initial-packet eligible, and its receipt exists when `work_queue.json` is written. |
| `pending_initial_packet` | The task is planned and should appear as pending initial context or late follow-up work. |
| `not_initial_packet` | The task is skipped or artifact-only for the first packet. |

`work_events.ndjson` mirrors this field for every planned task. The artifact
verifier cross-checks queue tasks against events and receipt presence.

## Packet Timing

Default trusted-repo shape:

```text
T+0s       checkout, diff, line map, PR thread ingest, queue plan
T+0-60s    fast sensors and must-run static checks
T+60s      close the initial packet with completed receipts and pending queue
T+60s+     start cached model lanes
T+60s-30m  continue proof, route late receipts, run follow-ups, compile final state
```

The initial packet should include pending work, so lanes know which concerns may
be answered later instead of treating unfinished proof as permanent missing
evidence.

## Routing

Receipts are routed only where they can change output:

| Receipt | Consumers |
|---|---|
| base+tests red/green | `tests-oracle`, `opposition`, `compiler` |
| coverage changed-line receipt | `tests-oracle`, `source-route`, `compiler` |
| unsafe-review receipt | `ub-memory-lifetime`, `security`, `compiler` |
| ripr receipt | `tests-oracle`, `proof-planner`, `compiler` |

Follow-up prompts are narrow:

```text
You raised concern X.
Receipt Y is now available.
Classify X as confirmed, refuted, parked, or still open.
Return only changed conclusions.
```

## Artifacts

The full audit trail belongs in artifacts. Existing packet paths use the repo's
current hyphenated tool-artifact names:

```text
work_queue.json
work_events.ndjson
resolved-tools.json
tool-status.json
proof_tasks.ndjson
proof_receipts.ndjson
receipt_routes.ndjson
resource_leases.ndjson
review/proof_plan.md
review/proof_receipts.json
review/receipt_routes.json
review/resource_leases.json
review/model_stages.json
review/resolved_candidates.json
review/final_compiler_input.json
model_stages.ndjson
resolved_candidates.ndjson
review/cache_manifest.json
review/metrics.json
events.ndjson
```

Sensor queue tasks are generated from the tool registry. `tool-status.json`
must mirror the stable tool metadata from `resolved-tools.json`, including
timeout, artifact budget, lease flag, gate policy, and artifact paths. The
artifact verifier rejects drift between those files because the queue cannot be
audited if status receipts describe a different tool plan.

The PR body gets only decision-relevant findings, questions, proof results,
refutations, and trust-affecting evidence gaps. It does not list the queue,
tool table, lane roster, command logs, or generic residual risk.
