# Repository agent instructions

## Review-fast Rust work

Optimize for review-fast, properly evidenced PRs. Review-fast does not mean tiny; it means the touched seam is coherent, the behavior is covered, verification is efficient, and the claim boundary is explicit.

Do not optimize for the fewest changed lines. Optimize for the narrowest complete change that includes code, tests or characterization proof, relevant receipts/docs, and exact proof commands. Every PR should make the reviewer validate instead of investigate.

## Work selection

Use cheap randomness to reduce overlap between independent agents, but do not let randomness define quality. Prefer preloaded or repo-provided worklists when available:

- `ripr` findings;
- `unsafe-review` findings;
- `xtask` or policy reports;
- no-panic, lint, file-policy, support-tier, or receipt reports.

Randomly choose among actionable findings instead of always taking the first or top finding. If no useful worklist exists, start from a random tracked Rust, test, xtask, policy, or agent-doc file and look for the first coherent testable seam.

Do not spend long on selection. If a seam cannot be tested, characterized, or policy-verified without broad work, choose another seam. Tool findings are starting points, not definitions of done; do not suppress findings just to make reports green.

## Test-quality rubric

Testing is part of the change.

- Behavior fixes: add a focused regression or edge-case test that fails before the fix when practical.
- Behavior-preserving refactors: rely on existing characterization coverage or add focused characterization coverage before changing code.
- Error handling: cover the success path and at least one failure path when practical, while preserving useful error context.
- Panic removal: prove the new fallible path and include the former panic or edge input when practical.
- Parser, path, string, numeric, and boundary logic: cover the edge case that makes the seam risky.
- Policy, `xtask`, and checker changes: add fixture-style accept/reject tests.
- Efficiency refactors: preserve behavior with tests; add measurement only when the repo already has cheap benchmark or receipt patterns.
- Snapshots: do not rely on snapshot churn alone unless the snapshot is the contract.

## Proof ladder

Run the narrowest proof that validates the claim, then broaden only when the touched surface or repo policy requires it.

1. Targeted test for changed behavior.
2. Touched crate tests or checks.
3. Relevant `xtask`, policy, receipt, or sensor check if touched.
4. `cargo fmt --all --check` when Rust formatting may be affected.
5. `cargo check` or `cargo clippy` for the touched crate or workspace as policy requires.
6. Broader workspace checks only when dependency direction, public surface, or repo policy requires them.

Final reports must state what was run, what the proof establishes, and what it does not establish.

## Rust guardrails

Do not add new panic paths or hidden risk. Avoid adding:

- `unwrap`;
- `expect`;
- `panic!`;
- `todo!`;
- `unimplemented!`;
- `unreachable!`;
- unchecked indexing;
- unchecked string slicing;
- `unsafe`.

No new panic paths does not require fixing all historical panic debt. If the chosen seam contains existing panic debt, narrow or remove it when practical and prove the fallible path.

Prefer `Result`/`Option`, `?`, `ok_or_else`, `map_err` with source preservation, `get`, checked or saturating arithmetic, fallible helpers, typed errors where useful, and small named functions/types over clever chains.

If touching existing unsafe code, keep it minimal and document the invariant with a real `SAFETY:` rationale.

## Architecture style

Favor SRP-heavy internal modules with clean public seams. Split by responsibility, not by vague `utils` buckets.

Keep domain logic independent from CLI, IO, filesystem, environment variables, process spawning, network, and wall-clock time where practical. Use ports/adapters where they remove real coupling, not as ceremony.

Prefer internal submodules over unnecessary public crates. Create public crate seams only when the boundary is stable and genuinely reusable.

## Traceability and receipts

If a PR changes a product claim, support tier, policy ledger, no-panic allowlist, Clippy exception, non-Rust file policy, CI lane, generated baseline, or receipt contract, update the matching source of truth.

Retained exceptions should include owner, reason, scope, and review or expiry date when the repo uses that style. Do not hide debt: fix it, narrow it, or receipt it.

## PR evidence expectations

Every PR should explain:

- selected seam;
- behavior claim and claim boundary;
- tests added or strengthened;
- exact proof commands run;
- what the proof establishes;
- what it does not establish;
- follow-ups intentionally left out.

See `docs/agent/review-fast-prs.md` and `.github/PULL_REQUEST_TEMPLATE.md` for the canonical review-fast workflow.
