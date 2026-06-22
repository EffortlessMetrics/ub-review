# UB-REVIEW-SPEC-0012 — proof broker / resource lease surface

Status: authored 2026-06-22 (Wave 4, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Related: [SPEC-0003](UB-REVIEW-SPEC-0003-intelligent-ci-gate.md) (gate consumes
proof receipts), [SPEC-0004](UB-REVIEW-SPEC-0004-artifact-contract.md)
(artifact rows promoted to first-class here), [ADR 0001](../adr/0001-whole-runner-stewardship.md).
Maturity: production — the broker runs focused red/green and focused-build proof
on every `ub-review/gate` run under the runtime profile's budget; edge cases are
test-pinned (issue #312 closed).

## Purpose

Name the contract for the proof broker: the component that turns
`[[proof.required]]` policy and model-lane proof requests into bounded, leased,
allowlisted command executions, and writes the receipts the gate and review
compiler consume. SPEC-0001 names "the proof broker runs commands" as a
first-class architectural role; this spec owns that role's behavior so it does
not accrete without a contract (issue #312 fixed real edges without one).

## User question

```text
When ub-review runs my [[proof.required]] command, what guarantees do I get?
```

You get: an allowlisted command (no shell, no manual cost), a leased execution
under the runtime profile's budget, a receipt that records pass / fail /
timeout / setup-failure distinctly, and a route that names exactly which model
lanes or gate policy consumed it. You do not get: concurrency, arbitrary
commands, or a claim that a passed receipt proves correctness.

## Architectural role

The broker sits between proof *requests* (from `[[proof.required]]` policy and
model lanes) and proof *receipts* (consumed by the gate and review compiler).
It never reads model output, never posts comments, and never decides the gate
verdict — it produces deterministic evidence that other components judge.

```text
[[proof.required]] policy ─┐
model-lane proof requests ─┴─▶ planner ─▶ broker ─▶ receipts ─▶ gate / compiler
                                  │           │
                                  │           └─ allowlist + lease + budget
                                  └─ groups duplicates, classifies status
```

## Broker entry points

Four entry points (`src/proof/broker.rs`), run in this order within a single
`ub-review run`:

| Function | When | Loop label |
|---|---|---|
| `run_initial_diff_proof_broker_v0` (`broker.rs:15`) | First pass — focused tests discovered directly from the diff (changed `.test.*` files). | `initial-diff-broker` |
| `run_seeded_proof_stream_v0` (`broker.rs:41`) | Auto-mode orchestrator — wraps the initial pass and, if seeded requests remain unreceipted, calls the request broker. | (orchestrator) |
| `run_request_proof_broker_v0` (`broker.rs:112`) | Model-request pass — drains unreceipted tasks from model-lane / policy proof requests. | `model-request-broker` |
| `run_follow_up_proof_broker_v0` (`broker.rs:168`) | Follow-up pass — runs after the orchestrator's follow-up phase produces new proof requests. | `follow-up-broker` |

Each later pass receives the prior pass's `proof_receipts` and `resource_leases`
so consumed budget is subtracted; exhaustion is sticky across passes.

## Lease lifecycle

A **resource lease** (`ResourceLease`, `src/proof/mod.rs:97-115`,
schema `ub-review.resource_lease.v1`) is a per-task accounting record — **not**
a concurrency primitive. The broker is a sequential loop over tasks.

| Phase | Mechanism |
|---|---|
| Requested | Each focused test/build task implies one lease, constructed at execution time (`focused_test_resource_lease` / `focused_build_resource_lease`, `broker.rs:331-394`). |
| Granted | Constructed with `status = "granted"` when budget allows (`red_green.rs:105-111`); the command may then run. |
| Tracked | An in-memory `BTreeSet<String>` of executed files and a running `estimated_seconds` (`red_green.rs:40-41`); prior leases fold into remaining budget (`remaining_focused_proof_budget`, `budget.rs:60-91`). |
| Released | Implicit — a lease is terminal once emitted with a terminal status; it is never mutated. The base+tests worktree is cleaned by `cleanup_base_plus_tests_worktree` (`worktree.rs:117`). |

**`consumer` field**: the lease names the task it authorizes
(`consumer = task.id`). The command guard (`run_proof_command_receipt`,
`command.rs:65-80`) refuses to run unless `lease.status == "granted"` AND
`lease.consumer == receipt_id` — a command cannot borrow another task's lease.

**Status vocabulary** (string-typed):

| Status | Meaning | Budget effect |
|---|---|---|
| `granted` | Leased; command may run. | Subtracts from remaining budget. |
| `absent` | Profile opts out of the whole proof class (`limits.tests == 0` / `limits.builds == 0`). | **Blocks** further budget (`focused_proof_lease_blocks_budget`, `budget.rs:97`). |
| `exhausted` | Runtime time/count budget used up. | **Blocks** further budget. |
| `skipped_profile` | `args.dry_run` set; broker did not execute. | Ignored (non-blocking). |

> **`requires_lease` is a different concept.** It is a `ToolPolicy` field on
> *sensors/tools* (e.g. actionlint, coverage), controlling whether a sensor is
> dropped without `--allow-heavy` (`plan_build.rs:448`). It is unrelated to
> `ResourceLease` and must not be conflated with proof-lease granting.

## Command allowlist (issue #312)

`[[proof.required]].command` strings flow through `build_proof_request` →
`proof_request_status` (`planner.rs:485-522`, `:581-608`) before they can be
brokered.

**Shell-token rejection**: every entry point calls
`has_shell_control_token(command)` (`validate.rs:414-418`), rejecting any
command containing `& | ; \` > < $`. This is the literal fix for #312. Called
from `proof_request_allowed_v0` (`planner.rs:592`) and both focused command
specs (`command_parse.rs:27, 229`).

**No shell**: commands are split with `command.split_whitespace()` into an argv
(`command_parse.rs:31-33, 232-235`) and executed directly — no `sh -c`.

**Allowlist** — only three command families are brokerable, gated by `cost`:

| `cost` | Brokerable commands |
|---|---|
| `focused-test` | Bun: `bun test <file>`, `bun bd test <file>`, `USE_SYSTEM_BUN=1 bun test <file>` (file must be repo-relative under `test/` or `tests/`, ending `.test.{ts,tsx,js,jsx,mjs,cjs}`). Cargo: `cargo test --locked [--test <target> \| <filter>]` with passthrough limited to `--exact \| --nocapture \| --show-output \| --ignored \| --include-ignored \| --test-threads <u16>`. |
| `focused-build` | `cargo {build\|check\|doc} --locked <args>`, plus the exact invocations `cargo xtask policy-check` and `cargo run --locked -p xtask -- check-pr`. |
| `manual` | **Never brokerable** — `proof_request_allowed_v0` returns false. |

**Classification** (`proof_request_status`, `planner.rs:581-590`):
- `"invalid"` — empty/whitespace command.
- `"requested"` — allowlisted; may be grouped and tasked.
- `"unsupported"` — anything else (manual cost, shell tokens, unrecognized).

Only `"requested"` groups produce tasks.

## Budget enforcement

Two budget structs from the runtime profile's `[budgets]` (`proof_budget`,
`proof_lease_budget`, `budget.rs:7-58`):

```text
ProofBudget {
    max_focused_test_files,   # distinct file cardinality cap
    max_focused_tests,        # total focused-test receipt count cap
    per_command_timeout_sec,  # clamped per task
    max_total_seconds,        # wall-clock cap across the whole run
}
ProofLeaseBudget { cpu, memory_mb, disk_mb, network, scratch }
```

> **There is no concurrency.** Admission is bounded by count
> (`max_focused_tests`), file cardinality (`max_focused_test_files`), and
> wall-clock (`max_total_seconds`). The broker loop is sequential
> (`for task in tasks`). "Concurrent leases" is not a concept.

**Per-task admission** (`focused_proof_budget_allows_next`, `tasks.rs:323-339`):
count < max, file-set < max_files (or file already in set), and
`estimated + next_timeout * command_count <= max_total_seconds`.

**Validation that rejects malformed budgets when focused proof is enabled**
(`budget.rs:14-26, 37-56`): zero `per_command_timeout_sec` or
`max_total_seconds` while `max_focused_tests > 0` bails; zero
`cpu`/`memory_mb`/`disk_mb` while `limits.tests > 0` and
`max_focused_tests > 0` bails.

## Edge-case semantics

The distinct outcomes a proof receipt can record, and the test that pins each:

| `result` | Meaning | Lease `status` | Pinned by |
|---|---|---|---|
| `head_passed` | HeadOnly / build command passed. | `granted` | `proof_broker_v0_executes_allowlisted_focused_build_request` (`build.rs:191`) |
| `head_failed` | Head command failed (red/green short-circuits; base+tests not run). | `granted` | `proof_broker_v0_skips_base_plus_tests_when_head_fails` (`main.rs:12138`) |
| `discriminating` | HEAD passed, base+tests failed — the red/green signal. | `granted` | `proof_broker_v0_runs_budgeted_focused_red_green_targets_and_writes_receipts` (`main.rs:11398`) |
| `non_discriminating` | Both passed — no signal. | `granted` | `proof_broker_v0_marks_base_plus_tests_pass_as_non_discriminating` (`main.rs:12078`) |
| `base_patch_failed` | **Setup failure** — HEAD passed but the base+tests worktree could not be prepared. Routes as missing-evidence. | `granted` | `proof_broker_v0_records_base_patch_failed_as_missing_proof` (`main.rs:12190`) |
| `timed_out` | Command exceeded its timeout. | `granted` | `proof_command_receipt_records_timeout_and_artifact_paths` (`command.rs:405`) |
| `skipped_budget` | Runtime time/count budget exhausted. | `exhausted` | `proof_broker_v0_exhausts_focused_tests_after_runtime_budget` (`main.rs:11846`) |
| `skipped_profile` | Profile opts out (`limits == 0`) or dry-run. | `absent` / `skipped_profile` | `proof_broker_v0_does_not_execute_when_focused_test_budget_is_zero` (`main.rs:12396`) |

> **`base_patch_failed` is a setup failure, not a test failure.** It fires when
> `prepare_base_plus_tests` returns `Err` after HEAD already passed — the broker
> could not construct the comparison side. It is the only red/green result that
> emits a synthetic `skipped` base-plus-tests command receipt without running
> anything (`red_green.rs:248-257`).

## Exit-code contract

The broker does **not** interpret exit codes by numeric value. The runner
returns a `CommandStatus { exit_code: Option<i32>, timed_out: bool, success:
bool, reason, duration_ms }`, classified in `run_proof_command_receipt`
(`command.rs:93-122`):

| Condition | command `status` |
|---|---|
| `timed_out == true` | `timed_out` (exit code preserved) |
| `success == true` | `passed` |
| otherwise | `failed` (exit code recorded verbatim for forensics only) |
| runner infrastructure error (spawn failed) | `skipped`, `exit_code = None` |

> **Exit code 2 has no special meaning.** Classification keys off `success` /
> `timed_out`, not numeric codes. The `exit_code` field is forensic.

## Artifact schemas

All schema constants in `src/artifacts.rs:48-68`. Written by
`write_proof_planner_artifacts` (`planner.rs:102-132`),
`write_proof_request_artifacts` (`planner.rs:168-310`),
`write_receipt_route_artifacts` (`planner.rs:409-431`).

| Artifact | Schema | Path | Key fields |
|---|---|---|---|
| Resource lease | `ub-review.resource_lease.v1` | `review/resource_leases.json` (+ `.ndjson`) | `id, kind ∈ {focused-test, focused-build}, consumer, status, cpu/memory_mb/disk_mb/timeout_sec, network, scratch, worktree?, command?` |
| Proof planner input | `ub-review.proof_planner_input.v1` | `review/proof_planner_input.json` | `diff_class, changed_files, proof_requests[], runtime_budget, box_shape` |
| Proof planner output | `ub-review.proof_planner_output.v1` | `review/proof_planner_output.json` | `lane = "proof-planner", proof_tasks[], skip[]` |
| Proof task | `ub-review.proof_task.v1` | `proof_tasks.ndjson` (root) + planner output | `id, kind, command, head_command, base_plus_tests_command?, purpose, consumers[], value, cost, timeout_sec, lease, test_file, mode, requested_by[], request_ids[]` |
| Proof receipt | `ub-review.proof_receipt.v1` | `review/proof_receipts.json` (+ `.ndjson`) | `id, kind, base, head, test_patch_mode, requested_by[], request_ids[], commands[], result, reason` |
| Proof request | `ub-review.proof_request.v1` | `review/proof_requests.json` + `proof_requests/<id>.json` | `id, lane, requested_by[], command, reason, cost, timeout_sec, required, status` |
| Proof request group | `ub-review.proof_request_group.v1` | `review/proof_request_groups.json` | `id, command, cost, timeout_sec, required, status, requested_by[], request_ids[], duplicate_count` |
| Receipt route | `ub-review.receipt_route.v1` | `review/receipt_routes.json` (+ `.ndjson`) | `id, receipt_id, phase ∈ {initial-diff-receipt, model-request-receipt, follow-up-receipt}, receipt_kind, result, status, consumers[], lease_ids[], source_artifacts[]` |

Receipt-route phases pin which broker pass produced each receipt
(`receipt_routes_capture_initial_model_and_follow_up_consumers`,
`planner.rs:1397`).

## Verification

The contract above is test-pinned by the proof module's test suite. The load-bearing tests:

- **Lease guard**: `proof_command_receipt_refuses_non_granted_lease_without_running`
  (`command.rs:454`), `proof_command_receipt_refuses_lease_for_different_consumer`
  (`command.rs:503`).
- **Allowlist**: `proof_request_status_enforces_v0_focused_allowlist`
  (`planner.rs:1003`), `proof_request_status_rejects_manual_cost_and_shell_tokens`
  (`planner.rs:1234`), `focused_build_command_spec_accepts_only_cargo_build_family_or_exact_policy_check`
  (`tasks.rs:724`), `focused_cargo_test_command_spec_pins_focus_and_passthrough_allowlist`
  (`tasks.rs:811`).
- **Budget**: `proof_budget_comes_from_runtime_profile_budgets` (`budget.rs:134`),
  `remaining_focused_proof_budget_subtracts_granted_focused_leases` (`budget.rs:367`),
  `remaining_focused_proof_budget_zeroes_after_exhausted_focused_lease` (`budget.rs:384`),
  `remaining_focused_proof_budget_zeroes_after_absent_focused_lease` (`budget.rs:397`),
  `invalid_enabled_proof_budget_is_rejected` (`budget.rs:158`).
- **Edge cases**: see the table above (each row cites its pinning test).
- **End-to-end**: `run_executes_focused_proof_and_writes_receipts` (integration,
  `tests/cli.rs`).

## Non-claims

The proof broker does **not** claim:

- Code correctness, soundness, or UB-freedom — a `passed` receipt means the
  command exited successfully, nothing more.
- That a red/green `discriminating` result proves a test catches a real bug —
  it proves the test *changed* behavior, not that the change is meaningful.
- Concurrency or parallelism — execution is sequential.
- Arbitrary command execution — only the three allowlisted families run.

Missing evidence (sensor absent, lease absent, budget exhausted, setup failure)
is recorded as missing evidence, never as clean evidence — consistent with the
umbrella (SPEC-0001).
