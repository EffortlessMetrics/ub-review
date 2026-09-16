# Terminal work-queue projection

`work_queue.json` is the legacy planner-time queue. It is written before sensor
and proof execution and therefore preserves intent rather than terminal truth.
Issue #1305 adds explicit names for the two different contracts without changing
execution or compatibility consumers:

```text
work_queue_plan.json
work_events_plan.ndjson
  immutable byte-identical copies of the planner queue and events

work_queue_terminal.json
work_events_terminal.ndjson
  deterministic projection after late sensors join and final proof receipts
  are published
```

## Terminal task identity

The terminal projection never infers that two source-shaped requests are the
same execution. A planner task retains its own identity and embeds its exact
plan row. Proof receipts join to that row through an explicit shared
`request_id` when available. Current focused-proof producers also use the
planner task ID as the receipt ID, so exact task/receipt identity is retained as
a separately reported compatibility join when request identities do not line
up. Fuzzy text, command, source, lane, or path similarity never creates a join.

A receipt may join at most one planner task in this slice. If its request
identity points to one task while its exact receipt identity names another, the
projection fails closed instead of choosing a winner. Every remaining receipt
is represented under its own receipt identity, including impact proof that was
not present in the planner catalog. Each receipt identity appears once, either
as the exact joined receipt reference on one planner task or as one standalone
receipt-backed task. The terminal reason records `join=request_identity`,
`join=task_identity`, or `join=request_and_task_identity` for auditability.

Every planner task must retain the versioned `ub-review.work_queue_task.v1`
schema. A malformed task schema, duplicate task identity, duplicate receipt
identity, empty identity, or ambiguous join aborts projection rather than
silently changing the queue.

Sensor tasks retain their plan status separately and project the terminal
status receipt as one of:

```text
ok
failed
timed_out
missing
missing_receipt
skipped
```

`skipped` may originate either from the immutable plan or from a terminal sensor
receipt on a planned dry-run path; the plan and terminal statuses remain
separate fields.

A planned proof with no exact receipt join becomes `not_executed`; a proof
receipt retains its producer result such as `head_passed`, `head_failed`, or
`discriminating`. Multiple distinct receipt results remain explicit rather
than being collapsed.

## Fail-closed publication

A new projection first removes prior canonical and staging outputs. It builds
and validates the complete queue and event stream in memory, stages both files,
publishes the event stream, and publishes `work_queue_terminal.json` last as
the commit marker. If validation, staging, or either rename fails, the producer
removes any partial canonical output. A failed rerun therefore cannot leave an
older terminal queue claiming to describe the newly written proof receipts.

Consumers must require `work_queue_terminal.json`; an event file without that
commit marker is incomplete publication, not terminal truth. Repeating the same
valid inputs produces byte-identical canonical output and no staging files.

## Authority boundary

This is a producer and audit surface only. The legacy queue remains unchanged,
and TaskLedger is still shadow observation/replay authority. The artifact does
not schedule, deduplicate, satisfy Required consumers, change gate enforcement,
or migrate the Python reconciliation checker. Parent issue #957 owns those
later integration and compatibility decisions.

The projection is generated only when `work_queue_plan.json` exists, so
standalone receipt-writer tests and worker-only paths remain unchanged. It is
byte-deterministic for the same plan and receipts and records the SHA-256 of the
immutable plan input.
