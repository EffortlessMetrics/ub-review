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
schema and one of the producer kinds `sensor`, `focused-test`, or
`focused-build`. A proof receipt kind such as `focused-head` is not a planner
kind. Every proof receipt must use `PROOF_RECEIPT_SCHEMA`, including receipts
not joined to any planner task. An unsupported schema or planner kind,
duplicate task or receipt identity, empty identity, or ambiguous join aborts
projection rather than silently changing the queue.

Every present row in `proof_tasks.ndjson` must use `ub-review.proof_task.v1`
before its request identities can participate in a receipt join. Missing,
non-string, foreign, padded, or future schema values reject the entire terminal
projection, including when an otherwise successful receipt has the exact task
identity. The diagnostic identifies the physical catalog line. A wholly absent
catalog retains the existing missing-evidence and exact task-identity
compatibility behavior; this guard does not infer missing request identities or
change Required satisfaction. Rejection leaves no committed terminal generation.

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

## Sensor receipt path admission

A selected sensor reads only its exact producer path:
`sensors/<sensor-id>/ub-review-sensor-status.json`. Absolute, parent-relative,
and unrelated packet-local paths are rejected before receipt content is read.
Empty or dot-component identities and identities containing a slash, backslash,
colon, or NUL cannot supply that path. Existing symlink components below the
output root are rejected, including the receipt itself and both parent levels.

The output root remains a trusted, single-writer directory. These checks reject
existing redirects; they do not provide isolation against a process concurrently
replacing filesystem entries. Intentionally skipped sensors still read no
receipt. An absent canonical receipt still projects as `missing_receipt`, not
success. A path rejection leaves no committed terminal generation and does not
modify planner bytes or the external receipt.

## Fail-closed publication

Before publishing any new planner artifact, the producer removes both canonical
terminal files and both terminal staging files. Cleanup failure aborts plan
publication before replacing either the legacy planner files or their explicit
plan copies. Terminal artifacts remain absent until current receipts regenerate
them; a new plan can never coexist with the previous plan's terminal marker.

The four planner artifacts are then staged in their destination directory and
published as one recoverable set. The producer snapshots the prior bytes and,
after every replacement is ready, removes the previous `work_queue_plan.json`
commit marker before changing any canonical sibling. It publishes the legacy
queue and events followed by explicit plan events, then publishes
`work_queue_plan.json` last because its presence enables terminal receipt
projection.

On a handled replacement failure, rollback attempts every non-marker sibling
rather than stopping at the first restoration error, and staging cleanup
attempts every staging path rather than stopping at the first obstruction. The
prior plan marker is restored only after every non-marker sibling is restored.
An incomplete sibling restoration withholds the marker; a failed marker restore
also attempts to remove any incomplete marker. The caller receives the original
publication error together with rollback and cleanup diagnostics, including any
failure to withhold the marker. A staging-cleanup failure alone can leave a
coherent restored planner set, but still returns an error and names the residue.
It is not reported as complete recovery or successful publication of new work.

A complete rollback restores the prior planner set; when no prior set existed,
it restores absence. Persistent filesystem obstructions must be resolved before
retrying. Interruption during canonical replacement, before the final marker is
published, leaves the plan marker absent rather than advertising a mixed set.
The terminal generation remains invalidated after failed planner publication;
rollback never implicitly revives a terminal projection for the restored plan.
These are single-writer, process-level recovery guarantees, not concurrent-reader
isolation or storage durability. The publisher does not synchronize files or
directories to stable storage and does not claim power-loss atomicity.

Proof receipt replacement separately invalidates the terminal queue commit
marker before writing new receipt bytes. Terminal projection then removes all
prior canonical and staging outputs, builds and validates the complete queue
and event stream in memory, stages both files, publishes the event stream, and
publishes `work_queue_terminal.json` last as the commit marker. If validation,
staging, or either rename fails, the producer removes partial canonical output.
A failed rerun therefore cannot leave an older terminal queue claiming to
describe newly written proof receipts.

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
