# Agentic workflow

Agents should leave the repo more governed than they found it.

Work review-fast: one coherent proof obligation per PR, with validation and
known gaps recorded before handoff or merge. Prefer this ladder over a mega PR:

1. docs / doctrine;
2. consolidated TOML ledger;
3. inventory command;
4. propose command;
5. checker;
6. report;
7. advisory CI;
8. blocking allowlist;
9. strict mode later.

Use Rust and `cargo`/`xtask`-style automation for durable checks. Do not add
non-Rust files, panic-family calls, Clippy suppressions, workflows, network
surfaces, process spawning, or expensive CI lanes anonymously. Add a structured
policy receipt first or in the same PR.

Subagents inspect and propose. The lane owner owns final staging, validation,
merge choice, and cleanup.

Run the cheapest relevant proof first. Escalate to deep proof when the changed
surface buys signal from it. `ripr` is static mutation-exposure analysis: it
shifts weak test-oracle signal left, while runtime mutation remains the slower
backstop.
