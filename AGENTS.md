# Agent operating contract

This repo optimizes for **review-fast, properly evidenced** Rust and review-tooling work.

Review-fast does not mean tiny. It means the seam is coherent, the behavior or
claim is covered by relevant evidence, verification is efficient, and the claim
boundary is explicit. Every PR should make the reviewer validate rather than
investigate.

## Work selection

- Use cheap randomness or existing tool/worklist findings to reduce overlap, but
  do not let randomness define quality.
- Prefer actionable seams from `ripr`, `unsafe-review`, sensor receipts, policy
  reports, or focused tests when those artifacts already exist.
- Do not spend long selecting a target. Accept only work that can be tested,
  characterized, or policy-verified without broad unrelated changes.
- Avoid generic cleanup, architecture theater, suppression-only policy changes,
  and rewrites with no proof path.

## Proof and claims

- Testing is part of the change.
- Behavior changes need focused regression or edge-case proof when practical.
- Behavior-preserving refactors need existing characterization coverage or new
  characterization coverage before the refactor.
- Policy/checker changes need fixture-style accept/reject proof.
- Panic-removal changes need proof of the new fallible path.
- Run the narrowest proof that validates the claim, then broaden only when the
  touched surface or repo policy requires it.
- Final reports must state what was run and what the evidence proves. Do not
  claim broad safety from narrow proof or missing receipts.

## Rust and review-tooling guardrails

Do not add new panic paths, unchecked indexing/slicing, `unsafe`, fake commands,
fake repo policies, unrelated cleanup, convenience dependencies, or hidden policy
drift. If an existing panic or unsafe boundary is inside the chosen seam, narrow
or document it when practical and keep the fix scoped.

Favor SRP-heavy internal modules, clean public seams, and constrained hexagonal
boundaries. Keep domain logic independent from IO, CLI, env, filesystem,
network, process, and wall-clock concerns where practical. Use ports/adapters
only where they remove real coupling.

## Traceability

If a PR changes a product claim, support tier, policy ledger, no-panic allowlist,
Clippy exception, non-Rust file policy, CI lane, generated baseline, or receipt
contract, update the matching source of truth. Do not hide debt: fix it, narrow
it, or receipt it.
