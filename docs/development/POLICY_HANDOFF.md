# Policy handoff package

This package is for moving the learned policy shape into other Rust repos and
feeding it back into agent lanes without copying unimplemented claims.

## Portable repo defaults

- Start with an `AGENTS.md` brief that says the repo is Rust-first, not
  Rust-only, and that exceptions need structured receipts.
- Add a PR template section for policy receipts and cheapest relevant proof.
- Seed `policy/allow.toml` as the default source-tree exception ledger.
- Add separate suppression ledgers only when real volume makes them easier to
  review than the consolidated ledger.
- Seed CI budget/routing ledgers when the repo needs risk-routed verification.
- Prefer `#[expect(..., reason = "...")]` over bare `#[allow(...)]` for durable
  Clippy suppressions, with a matching debt or exception receipt.
- Keep panic allowlists semantic: path, family, and selector. Line and column
  metadata is advisory only.

## Implementation order

1. Add doctrine and seed ledgers.
2. Add a parse-only policy check for the ledgers, normally through `xtask`.
3. Add inventory commands that report current exceptions without failing.
4. Add propose commands that generate candidate receipts.
5. Add advisory CI and summary output.
6. Turn high-signal categories into blocking gates after the repo has receipts.

## Lane feedback

- Do not open several PRs that all add competing style doctrines. Pick one
  umbrella doctrine PR, then stack narrow implementation PRs behind it.
- Do not claim branch protection, policy gates, or CI summaries exist until the
  workflow or checker exists in the repo.
- Do not generate every possible policy file for every repo. Start with the
  maturity tier the repo actually needs.
- Use exact receipt names and file paths in generated summaries so humans can
  merge, split, or reject them without re-deriving the policy.
- Treat rate-limited review bots as missing external evidence, not as approval.
