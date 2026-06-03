# unsafe-review CI integration

`unsafe-review` is a static unsafe-contract review signal. CI should treat it as
review evidence first and as a blocking policy only when the repository has
explicitly opted in.

## Pull requests

Recommended PR command surface:

```bash
cargo xtask unsafe-review-pr
```

The PR job should upload:

```text
target/unsafe-review/cards.json
target/unsafe-review/pr-summary.md
target/unsafe-review/github-summary.md
target/unsafe-review/cards.sarif
target/unsafe-review/comment-plan.json
target/unsafe-review/witness-plan.md
target/unsafe-review/lsp.json
target/unsafe-review/receipt-audit.json
```

Default PR status is advisory. Missing cards or failed execution should be
reported as missing review evidence.

## Risk PRs

If a PR changes high-risk unsafe seams, require a witness route before merge.
Examples include FFI boundaries, raw pointer dereference, layout/representation
assumptions, aliasing-sensitive conversions, allocator ownership transfer, and
lifetime extension.

A witness route may be a targeted test, Miri run, sanitizer run, fuzz/workload
receipt, or another documented execution path. It is evidence for review, not a
proof of soundness.

## Nightly and release

Nightly jobs should run concrete unsafe witnesses where feasible:

```bash
cargo xtask unsafe-review-repo
cargo +nightly miri test
cargo mutants
```

Release jobs should fail only on explicit repository policy, such as unexplained
new unsafe-review gaps on public or load-bearing crates.

## ub-review orchestration

`ub-review` routes unsafe-review output to the UB, tests, architecture,
opposition, and security lanes. Those lanes use the cards and witness plan to
ask whether changed unsafe code is reviewable, while runtime backstops continue
to live in focused test, Miri, sanitizer, mutation, and coverage jobs.
