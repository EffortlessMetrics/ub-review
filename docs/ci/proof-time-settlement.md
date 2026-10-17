# Focused proof time: reservation and terminal settlement

The existing sequential proof brokers admit each task against its maximum
command timeout. Required work is admitted before optional impact work by the
Required-first composition in #1484. Neither a selected task nor a reservation
is evidence that the command passed.

A resource lease retains its original `timeout_sec`. After a focused task returns
confirmed terminal command receipts and successful cleanup/receipt construction,
the runner records `settled_elapsed_ms` separately. It is the larger of the
monotonic task interval (including task setup and cleanup) and the sum of command
process durations. The latter is a conservative floor, including for injected
runner results in tests. A settled failure or timeout remains failure or timeout.

Terminal command status must agree with its result: `passed` requires exit 0
without a timeout, `failed` requires any nonzero exit (including 101) without a
timeout, and `timed_out` requires the timeout flag. Contradictory receipts retain
the full reservation. Settlement also requires a granted lease and an exactly
matching revision reference: digest, reviewed commit and candidate/merge semantics.
A missing reference on either side cannot match a present one. Stale or
non-granted receipts keep their reservation and cannot refund another candidate.
A red/green task counts both command durations, including
an expected failing base: 21 seconds plus 22 seconds gives a 43-second floor,
even when an injected task clock reports less.

Later admission charges the rounded-up elapsed seconds, at least one second per
completed task. Actual overruns are never capped at the earlier reservation.
Arithmetic saturates rather than overflowing. Historical maximum reservations
may sum above the allocation after completed tasks return unused time; this is
not simultaneous over-allocation. Each new task must still fit its full ceiling
alongside prior consumption, and task/file-count limits are not refunded.

Missing settlement, including old serialized leases, retains the full timeout.
Skipped/unavailable commands, failed base preparation and unconfirmed completion
cannot settle. Command cleanup, receipt or ledger errors propagate before settlement;
they do not create free work. Existing absent/exhausted lease behavior remains
conservative. The field is execution accounting, not proof satisfaction or a new
execution authority; packet consumers must retain its original revision/context.

This is a bounded legacy-broker correction. It does not complete #992's separate
queue/resource-wait accounting, migrate scheduler authority, or reselect tasks
already excluded from an upfront portfolio batch. It permits cross-phase reuse
of returned time while preserving the existing Required-first ordering. It does
not establish a complete <=20-minute CI portfolio or alter workflow triggers.

Focused checks:

```sh
cargo test --locked --bin ub-review proof::budget::tests
cargo test --locked --bin ub-review seeded_required_admission_preserves_time_slots_and_failure_truth
cargo test --locked --bin ub-review model_request_phase_preserves_command_outcomes_and_unavailable_receipt
cargo test --locked --bin ub-review unbuildable_base_plus_tests_patch_reports_base_patch_failed
```

Retain measured durations and admission ceilings separately in exact-candidate
handoffs. Passing an independent baseline or refunding a completed task does not
satisfy another Required request without its own applicable receipt.

The red/green loop also pins next-task admission independently of task-count
limits. With two60-second commands per task and180seconds total, a first
60,000ms task permits the next120-second reservation. A60,002ms task rounds
to61seconds and refuses that next reservation; skipped work remains unsettled.
These are injected duration controls over the real runner and worktree path,
not a wall-time benchmark.
