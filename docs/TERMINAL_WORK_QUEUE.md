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
plan row. Proof receipts join to that row only through an explicit shared
`request_id`; equality between a planner task ID and receipt ID is not itself a
join. A receipt may join at most one planner task in this slice. Every remaining
receipt is represented under its own receipt identity, including impact proof
that was not present in the planner catalog. Each receipt identity appears once,
either as the exact joined receipt reference on one planner task or as one
standalone receipt-backed task.

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

A planned proof with no exact receipt join becomes `not_executed`; a proof
receipt retains its producer result such as `head_passed`, `head_failed`, or
`discriminating`. Multiple distinct receipt results remain explicit rather
than being collapsed.

## Authority boundary

This is a producer and audit surface only. The legacy queue remains unchanged,
and TaskLedger is still shadow observation/replay authority. The artifact does
not schedule, deduplicate, satisfy Required consumers, change gate enforcement,
or migrate the Python reconciliation checker. Parent issue #957 owns those
later integration and compatibility decisions.

The projection is generated only when `work_queue_plan.json` exists, so
standalone receipt-writer tests and worker-only paths remain unchanged. It is
byte-deterministic for the same plan and receipts, rejects malformed or
duplicate identities, and records the SHA-256 of the immutable plan input.
