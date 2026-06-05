# Test evidence lanes

`ub-review` treats tests, coverage, mutation, Miri, and static review as separate
evidence lanes with different costs and claims. The default PR path should buy
fast signal first, then route deeper proof only when risk or claim boundaries
justify the spend.

## Static-first PR evidence

Default PR evidence should prefer deterministic, cheap checks:

- format and compile checks;
- Clippy over changed Rust surfaces;
- `cargo-allow` source-exception checks when exception surfaces change;
- `ripr` static mutation-exposure analysis for changed Rust behavior;
- `unsafe-review` cards when unsafe, native, FFI, raw-pointer, or layout seams
  change.

`ripr` is mutation-shaped weak-oracle signal shifted left. It does not kill or
survive runtime mutants, and it does not replace runtime mutation where execution
proof is worth the cost.

## Runtime backstops

Runtime lanes should remain available, but they should be selected by risk pack,
label, main, nightly, or release posture:

- focused tests for the changed seam on ordinary PRs;
- targeted mutation for high-risk oracle gaps;
- Miri for concrete UB execution witnesses;
- coverage for execution-surface telemetry;
- fuzzing or workload replay for parser, boundary, or input-heavy claims.

## Claim boundaries

A PR should state what its evidence proves and what it does not prove. A skipped
optional lane is not a pass; it is a policy decision that belongs in the PR gate
summary or review notes.
