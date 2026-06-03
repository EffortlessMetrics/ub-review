# Agent brief

This repo is Rust-first and policy-ledger governed.

- Use Rust and durable `cargo`/`xtask`-style automation for repo checks where practical.
- Do not add non-Rust files anonymously; add or update a policy receipt with owner, reason, review date, and expiry when applicable.
- Do not add panic-family calls unless they are deliberately covered by the no-panic policy.
- Do not use bare `#[allow]`; use `#[expect(..., reason = "...")]` and add a debt or exception receipt when the suppression is durable.
- Prefer small single-responsibility PRs over mega PRs.
- Run the cheapest relevant proof first, then deeper proof where it buys signal.
- Use labels or explicit policy receipts for expensive CI lanes.
- Treat `ripr` as static mutation-exposure analysis: earlier and cheaper mutation signal, with runtime mutation as the slower backstop.
- CI cost discipline exists so the project can afford more verification, not less.
