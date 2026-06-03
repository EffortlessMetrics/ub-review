# File policy

The repo is Rust-first, not Rust-only. Non-Rust surfaces can exist when they are
receipted and fit a real platform need: GitHub workflow YAML, documentation,
fixtures, generated artifacts, release compatibility wrappers, external tool
rules, or other ecosystem surfaces Rust cannot replace cleanly.

`policy/allow.toml` is the default source-tree exception ledger. It answers
whether a controlled file or call site may exist and why. Companion ledgers
should be added only when the repo has enough policy volume to justify them.
Examples of narrower questions are:

- may this file execute?
- may this workflow use secrets?
- may this script publish or mutate release state?
- may this code contact the network?
- may this generated file be edited by hand?

When adding a new file category, add the narrowest receipt possible and include
owner, reason, review date, and expiry for temporary exceptions.
