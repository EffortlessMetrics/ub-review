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
the full reservation. A red/green task counts both command durations, including
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
