# Review-fast agentic Rust PRs

`ub-review` work should compound: independent agents should avoid overlap, choose testable seams, and leave reviewers with bounded evidence instead of archaeology.

The house style is:

```text
cheap entropy for distribution + coherent seam + real proof + explicit claim boundary
```

## Review-fast doctrine

A review-fast PR is not necessarily tiny. It is fast to review because:

- the seam is coherent;
- the behavior claim is explicit;
- tests or characterization proof cover the claim;
- verification is complete for the claim boundary and not wider than necessary;
- unrelated cleanup and policy drift are absent.

Avoid shallow cleanup that leaves reviewers reconstructing risk. A focused PR with tests and a clear non-goal list is often faster to review than a smaller untested diff.

## Work selection

Use cheap randomness to reduce overlap between agent swarms. Do not spend long searching.

Prefer repo-provided or preloaded worklists when available:

- `ripr` for weak test-oracle seams, missing discriminator tests, and edge coverage gaps;
- `unsafe-review` for unsafe, panic, native-boundary, fallibility, and invariant surfaces;
- policy, receipt, no-panic, lint, file-policy, and support-tier reports.

Choose randomly among actionable findings rather than always taking the top finding. If no worklist exists, start from a random tracked Rust, test, `xtask`, policy, or agent-doc file.

The finding or random file is only the starting point. Accept the work only when the seam can be tested, characterized, or policy-verified without broad unrelated work.

## Test-quality rubric

Use the test shape that matches the work type:

| Work type | Expected proof |
|---|---|
| Behavior fix | Focused regression or edge-case test that fails before the fix when practical. |
| Behavior-preserving refactor | Existing characterization coverage or new focused characterization coverage before refactoring. |
| Error handling | Success path plus at least one failure path when practical; preserve useful error source/context. |
| Panic removal | Proof of the new fallible path, including the former panic or edge input when practical. |
| Parser/path/string/numeric logic | Edge coverage for the risky discriminator or boundary. |
| Policy/`xtask`/checker change | Fixture-style accept/reject tests. |
| Efficiency refactor | Behavior-preserving tests first; measurements only when cheap benchmark or receipt patterns already exist. |
| Snapshot update | Snapshot churn only when the snapshot is the actual contract. |

For behavior changes, a red-green-refactor loop is ideal when practical. For behavior-preserving refactors, do not force fake red tests; use characterization coverage instead.

## Proof ladder

Run the narrowest proof that validates the claim, then broaden only as needed:

1. Targeted test for changed behavior.
2. Touched crate tests/checks.
3. Relevant sensor, `xtask`, policy, or receipt check if touched.
4. `cargo fmt --all --check` when Rust formatting may be affected.
5. `cargo check` or `cargo clippy` for the touched crate/workspace as policy requires.
6. Broader workspace checks only when the surface, dependency direction, or repo policy requires them.

Verification completeness and verification efficiency are both review-speed tools. Do not stop at “I ran fmt” for behavior changes, and do not burn full-workspace proof for a tiny local seam unless policy or dependency surface requires it.

## Rust guardrails

Maintain no new panic debt. Do not add `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!`, unchecked indexing, unchecked string slicing, or new `unsafe`.

If the chosen seam contains existing panic debt, narrow or remove it when practical and prove the new fallible path. Do not turn a scoped PR into a repo-wide panic cleanup campaign.

When unsafe remains, keep it minimal and document the invariant with a concrete `SAFETY:` rationale.

## Architecture style

Favor SRP-heavy internal modules with clean public seams. Keep domain logic separate from CLI, IO, filesystem, environment variables, process spawning, network, and wall-clock time where practical.

Use ports/adapters when they remove real coupling. Do not add abstraction layers as architecture theater, and do not create public crate seams until the boundary is stable and genuinely reusable.

## Traceability

If the PR changes a product claim, support tier, policy ledger, no-panic allowlist, Clippy exception, non-Rust file policy, CI lane, generated baseline, or receipt contract, update the matching source of truth.

Retained exceptions should be explicit about owner, reason, scope, and review/expiry date when the repo uses that style. Fix debt, narrow it, or receipt it; do not hide it.

## Final report and PR body

Every review-fast PR should state:

- selected seam;
- behavior claim and whether behavior changed, stayed preserved, or only tests/receipts changed;
- tests added or strengthened;
- exact commands run;
- what those commands prove;
- what they do not prove;
- risk controls;
- follow-ups intentionally left out.

The reviewer should be able to validate the claim boundary from the PR body and proof commands without rediscovering the agent's reasoning path.
