# UB-REVIEW-SPEC-0014 — heavy witness (mutation / sanitizer / Miri)

Status: authored 2026-06-22 (Wave 6+, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Related: [SPEC-0005](UB-REVIEW-SPEC-0005-sensor-tool-integration.md)
(`[tools.*]` registry rows), [SPEC-0012](UB-REVIEW-SPEC-0012-proof-broker.md)
(proof broker — the executor heavy witnesses would route through),
[SPEC-0013](UB-REVIEW-SPEC-0013-profile-input-schema.md) (`[budgets]` fields).
Maturity: partial — config and budget surfaces exist; mutation and sanitizer
are **declared but parked** (no executor route). Miri is partially routed
(diff-gated skip, but no leased execution path).

## Purpose

Name the contract for heavy witnesses — the runtime backstop evidence class
that supplements cheap deterministic proof with expensive execution-based
signals. PRODUCT_STATE PR 4 declares mutation (`cargo-mutants`) and sanitizer
(ASAN/MSAN/TSAN) as `heavy-witness` tools with config surfaces but **no
executor route**; issue #75 tracks the execution-side work. This spec owns the
parking contract so the parking→execution transition does not drift silently
when the executor route eventually lands.

## Witness classes

| Witness | Cost class | Config field | Executor route |
|---|---|---|---|
| Mutation (`cargo-mutants`) | heavy-witness | `[budgets].mutation` | **parked** (no executor; config leases but never executes) |
| Sanitizer (ASAN/MSAN/TSAN) | heavy-witness | `[budgets].sanitizer` | **parked** (no executor) |
| Miri | heavy-witness | (diff-gated) | **partial** (planned as a proof task when `unsafe_or_native_risk`; no leased execution path) |
| Coverage | execution-surface telemetry | `[tools.coverage]` | LIVE (leased sensor, `requires_lease = true`) |

> Coverage is **not** a heavy witness — it is explicitly execution-surface
*telemetry*, not proof (see `docs/ci/coverage.md`). It shares the lease
mechanism but is a different evidence class.

## Parking contract

Heavy witnesses are governed by the **proof planner's skip table**
(`src/proof/planner.rs:134-164` `proof_planner_skips`), which emits a
`ProofPlannerSkip` entry per witness class. The skip reason distinguishes two
states:

| State | Trigger | Reason string | Test pin |
|---|---|---|---|
| **skip** (not leased) | `profile.budgets.mutation == false` / `sanitizer == false` | "...skipped because this runtime profile does not lease [mutation/sanitizer] proof. Use a risk-pack/manual-heavy profile to run it." | `proof_planner_records_heavy_witness_skips_without_leases` (`planner.rs:947`) |
| **parked** (leased, no executor) | `profile.budgets.mutation == true` / `sanitizer == true` | "...leased by this runtime profile, but ub-review has no [mutation/sanitizer] executor route yet; parked as manual-heavy evidence until executor routing lands." | `proof_planner_parks_leased_heavy_witnesses_until_executors_route_them` (`planner.rs:975`) |

**Key distinction**: a profile that enables `[budgets].mutation = true` does
NOT cause mutation to run — it causes the planner to *park* the witness (record
it as leased-but-unexecuted) rather than *skip* it (record it as not-leased).
Both states produce a `ProofPlannerSkip` artifact entry; the difference is the
reason string and the budget reservation.

### Miri's partial routing

Miri is gated by the diff flag `unsafe_or_native_risk` (`planner.rs:136-139`):

- If **no unsafe/native risk** detected → skip with "cheaper focused proof is
  preferred when available."
- If **unsafe/native risk detected** → Miri is planned as a proof task (not
  skipped), but there is **no leased execution path** through the broker. The
  broker's allowlist (SPEC-0012) does not include a Miri command family, so the
  task would be classified `"unsupported"` by `proof_request_status`.

This means Miri is currently a planned-but-unbrokerable witness — it appears in
the planner output but cannot execute through the proof broker today.

## Budget fields

From SPEC-0013, the `[budgets]` fields that control heavy witnesses:

| Field | Type | Default | Effect |
|---|---|---|---|
| `mutation` | `bool` | `false` | `true` → park (lease) mutation; `false` → skip |
| `sanitizer` | `bool` | `false` | `true` → park (lease) sanitizer; `false` → skip |

These are consumed at `planner.rs:146-160`. Setting them to `true` does not
enable execution — it only changes the skip reason from "skip" to "parked".

## Receipt schema

Heavy-witness skip/park entries appear in `proof_planner_output.json`
(`ub-review.proof_planner_output.v1`) under the `skip[]` array, each with:

```
{ kind: "mutation" | "sanitizer" | "miri", reason: <string> }
```

The reason string is the load-bearing signal: consumers (and the review
compiler) distinguish "not leased" from "leased but no executor" by parsing it.
No separate heavy-witness receipt schema exists today.

## The parking → execution transition

To unpark a heavy witness (make it actually run), two things must land:

1. **A brokerable command family** in the proof allowlist (SPEC-0012 §Command
   allowlist). Today only `focused-test` and `focused-build` are brokerable;
   `manual` is never executed. Mutation/sanitizer would need a new cost class
   (e.g. `heavy-witness`) with an allowlisted command set
   (`cargo-mutants`, `cargo build --target ... -Z sanitizer=address`, etc.).
2. **A leased execution path** through the broker. The broker's lease lifecycle
   (SPEC-0012) would need to grant and track heavy-witness leases under the
   runtime profile's budget — currently the budget fields exist but the broker
   has no code path that reads them for execution decisions.

Until both land, `[budgets].mutation = true` / `sanitizer = true` is a
documentation-of-intent knob that changes the skip reason but executes nothing.

## Non-claims

Heavy witnesses do **not** claim:

- **Soundness or UB-freedom.** A parked or even executed mutation/sanitizer
  witness does not prove memory safety — it provides a runtime backstop signal
  at best.
- **Completeness.** Mutation testing covers only the mutants `cargo-mutants`
  generates; sanitizer runs cover only the code paths the test suite exercises.
- **Equivalence to the deterministic proof lanes.** Heavy witnesses are slower,
  less deterministic, and higher-cost than the focused red/green proof the
  broker runs by default.

Missing heavy-witness evidence (skip or park) is recorded as missing evidence,
never as clean evidence — consistent with the umbrella (SPEC-0001).

## Verification

The parking contract is test-pinned:

- `proof_planner_records_heavy_witness_skips_without_leases` (`planner.rs:947`)
  — pins the skip state when budgets are false.
- `proof_planner_parks_leased_heavy_witnesses_until_executors_route_them`
  (`planner.rs:975`) — pins the parked state when budgets are true.

## Related

- Issue #75 — the execution-side work (wire the executor route).
- PRODUCT_STATE PR 4 — "Targeted heavy witnesses — PARTIAL."
- `docs/ci/coverage.md` — coverage's distinct "telemetry not proof" posture.
- `docs/UNSAFE_REVIEW_POLICY.md` — the static unsafe-contract review pillar
  (advisory by default; Miri/sanitizer are the runtime backstops).
