# Agent brief

This repo is Rust-first and policy-ledger governed.

Start with [the native workflow](docs/development/AGENTIC_WORKFLOW.md). For a
change to carry through review, qualification and authorized merge, read
[deliver-change](.agents/skills/deliver-change/SKILL.md). For focused review
or reproduction, read [review-change](.agents/skills/review-change/SKILL.md).
Read the relevant skill on continuation too; it supplies procedure, while the
current conversation and native issue/PR state supply scope and authority.

## Zero trust development and fallibilism

Current user intent governs the desired outcome and authorized scope; evidence
governs factual claims. Any premise can be wrong, including one supplied by the
user, an issue, a specification, a prior review or this guidance. Check the
load-bearing premise against the relevant source, contract and observed behavior;
repair or supersede a mistaken premise with its evidence and reason in the native
issue/PR. Do not silently replace the user's goal or invent new authority.

Reuse applicable evidence with its original source, scope and limits. Refresh it
when relevant inputs change, evidence contradicts it, or a required boundary
needs a current observation; do not restart every check on every continuation.
Prefer the same useful session or a fork for successive review angles. Change
the question, oracle, threat model or counterexample; use a separate session
when the additional perspective earns its context and coordination cost. The
owning lead may make the distinct readiness judgment under existing protections.

Choose source-only review or authorized executable reproduction before checkout,
build or test. Source-only mode must not execute candidate code; the review skill
defines the boundary and the separate route for executable evidence.

- Use Rust and durable `cargo`/`xtask`-style automation for repo checks where practical.
- Treat `policy/allow.toml` as the default source-tree exception ledger.
- Do not add non-Rust files, panic-family calls, lint suppressions, generated files, scripts, workflows, or other controlled surfaces anonymously.
- Do not use bare `#[allow]`; use `#[expect(..., reason = "...")]` and add an owned receipt when the suppression is durable.
- Work review-fast: one coherent proof obligation per PR, with exact validation and known gaps.
- Run the cheapest relevant proof first, then deeper proof where it buys signal.
- Select the candidate intended to land before expensive CI; use the actual installed request path and preserve required checks during the late-qualification cutover.
- Treat `ripr` as static mutation-exposure analysis: earlier and cheaper mutation-like weak-oracle signal, with runtime mutation as the slower backstop.
- Treat `unsafe-review` as unsafe/native reviewability: safety contract, guard, reach, and witness route.
- Use `xtask` for repo-local orchestration and aggregation; do not reimplement `cargo-allow`, `ripr`, or `unsafe-review` in it.
- CI cost discipline exists so the project can afford more verification, not less.

For an implementation goal authorized through integration, the owning lead stays
with the selected candidate through ordinary CI waiting, complete review and
result inspection, final current-repository judgment, protected merge and landing
readback. Draft publication and green CI are checkpoints, not default endpoints.
A separate judgment does not require a separate agent or another routine human
approval. Explicit review-only, Draft-only and release boundaries still apply.
