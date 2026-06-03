# Review-fast Rust and review-tooling doctrine

`ub-review` exists because useful automated review needs a clear definition of
what good looks like. The target is not generic commentary, maximal CI, or tiny
cleanup. The target is **review-fast, properly evidenced** work.

Review-fast work is fast to review because the seam is coherent, the behavior or
claim is covered, verification is efficient, and the claim boundary is explicit.
A reviewer should be able to validate the result from the PR body, receipts, and
focused proof instead of reconstructing the risk from scratch.

## Tooling implication

Review tooling should push every lane toward five concrete outputs:

1. the narrow seam touched by the PR;
2. the behavior, invariant, or product/policy claim at risk;
3. the strongest evidence from the packet;
4. the targeted proof that would confirm or refute it;
5. what the available evidence does not establish.

A zero-finding lane is not approval. It still needs the concrete paths,
invariants, tests, or claims checked; the strongest failed objection; why that
objection did not hold; and residual risk for a human reviewer.

## Admission filter

Good candidate seams are testable or policy-verifiable:

- under-tested behavior with edge cases;
- fragile parser, path, string, numeric, or error logic;
- panic-prone paths inside a narrow seam;
- duplicated logic that can be centralized and tested;
- unsafe/native-boundary invariants that can be narrowed and documented;
- xtask, receipt, prompt, or policy branches that can get fixture-style tests.

Poor candidate seams are untested drive-by cleanup, purely idiomatic rewrites,
module shuffling with no claim boundary, speculative adapters, and
suppression-only policy changes.

## Proof ladder

Run the narrowest proof that validates the claim, then broaden only as needed:

1. targeted test or fixture for the changed behavior;
2. touched crate or prompt/receipt tests;
3. relevant policy, sensor, or artifact verifier if touched;
4. `cargo fmt --all -- --check` when Rust formatting may be affected;
5. broader `cargo check`, Clippy, or workspace tests only when the touched
   surface or repo policy requires them.

The final report must say what was run, what the proof establishes, and what it
does not establish.

## Guardrails

Do not add new panic paths, unchecked indexing/slicing, `unsafe`, fake commands,
fake repo rules, unrelated cleanup, convenience dependencies, or hidden policy
drift. If a policy, support tier, CI lane, receipt contract, generated baseline,
or product claim changes, update the matching source of truth.
