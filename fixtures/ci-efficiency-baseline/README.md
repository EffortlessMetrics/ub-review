# Fixed Required-starvation before-state fixture (#1270)

This preparatory slice retains one #1458 run pair observed on 2026-10-14. It
adds no production behavior and does not complete #1270 or establish reuse.

- Candidate: [run 38520204127 / gate job 115707925898](https://github.com/EffortlessMetrics/ub-review/actions/runs/38520204127/job/115707925898), merge result `4fd2812df756e609ce350c27a62149af633516a3`
- Independent baseline: [run 38520201117](https://github.com/EffortlessMetrics/ub-review/actions/runs/38520201117), raw head `0ce3c632cbd19e102dc49c24a59f30f48bb5935d`, protected workflow definition `3dbcc2be9768b082c0896dc2360e950ffbd56a65`
- [Owner issue](https://github.com/EffortlessMetrics/ub-review/issues/1270), [historical closeout boundary](https://github.com/EffortlessMetrics/ub-review/issues/1270#issuecomment-5702958987), and [current preparation scope](https://github.com/EffortlessMetrics/ub-review/issues/1270#issuecomment-6175829187)

The 36 observations comprise 16 wrapper/workflow commands, 14 **nested,
shadow-reported** process-started tasks (11 sensors and 3 impact proofs), and
6 consumers. They are not 36 independent executions. Printed baseline case
arms and finalizer command lists are not extra executions. Three configured
Required rules remain separate from three unsatisfied runtime request IDs;
there is no ordinal/name mapping or claim of Required command re-execution.

The contradictory, shadow-only report has `input_unavailable=true` and retains
`timeout_ceilings_exhausted_budget_before_actual_deadline`. Reconciliation
exited 2; workflow `continue-on-error: true` yielded a successful step conclusion.
Ten reported sensor successes coexist with product gate failure on RIPR.
Coverage was terminal non-success, LCOV absent, and publication skipped.

Manifest observations join to `source-projections.json`, which records public
capture URLs, basis-labeled SHA-256/byte references, and selected
JSON fields or numbered original lines. Captures distinguish connector bytes,
decoded public bytes and the derived log-report projection. Full bytes are not
retained;
projection-file digest/size checks bind only committed projection bytes.
Producer artifact references remain reported, uninspected digests/sizes.
No archives, full raw logs, private transfer metadata or credentials are included.
The reviewed input packet digest is pinned.

All nine overlaps stay `unproven_equivalence`. Tested subjects, authorities,
target roots and coverage/test witness semantics remain distinct. Individual
proof argv, outcomes and worktree SHAs are unknown; `proof-head` is no exact SHA.
Bootstrap/review wrapper argv are also unknown: printed recipes are source
evidence. Process totals, billing, savings, critical path and inspected archive
sizes remain null. No nested observation or shared step window is summed.

Run `cargo test --locked --test ci_efficiency_baseline`. The bounded Rust loader
reuses inventory/path/privacy/digest and source-join approaches from historical
commit `c6c89c949c23ee9d20ee0f65ac40457ee0a329e3`; it does not revive that larger
historical corpus or add a second Python verifier. Fixed identities/counts and
the projection-byte lock protect **this fixture only**. Semantic negative
controls bypass the byte lock and must fail at their named substantive join.
Corrections need source review and a new lock. All fixture files must remain
below 64 KiB in total.
