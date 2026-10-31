# Agentic workflow

Agents should leave the repo more governed than they found it.

`AGENTS.md` routes delivery to
[deliver-change](../../.agents/skills/deliver-change/SKILL.md) and independent
review to [review-change](../../.agents/skills/review-change/SKILL.md). These
skills extend this procedure; they do not replace the checks or lane owner.
Manual session exercises are documented in
[AGENT_WORKFLOW_EXERCISES.md](AGENT_WORKFLOW_EXERCISES.md).

For source-only Tower review versus integrated CI evidence and the intended
late-qualification flow, use [the transition guide](TOWER_CI_TRANSITIONS.md).
It describes evidence boundaries and unknown states; it grants no activation.

Work review-fast: one coherent proof obligation per PR, with validation and
known gaps recorded before handoff or merge. Complete the objective's code,
proof, docs and contracts together; a larger coherent PR is acceptable with adequate
review and tests. Do not split those pieces merely to keep a diff small. This ladder
is an example for separately useful capabilities, not a required sequence of partial
PRs:

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

The lane lead may orchestrate subagents for bounded research, implementation,
review and repair. It owns integration, final staging, validation, merge judgment
and cleanup. Multiple alternatives may be developed cheaply; only the selected
candidate intended to land gets expensive qualification. Independent objectives
may proceed concurrently, with one writer per candidate.

For an authorized implementation goal, the same useful lead normally continues
through the CI wait, complete review/Conversation inventory, actual result inspection,
separate current-repository judgment, protected merge and landing readback. An
explicit Draft-only or read-only assignment still stops at its named boundary.
A real interruption uses an exact continuation record, not a duplicate CI request.
Read deliver-change for the complete finish path. The existing #1465 admission
cutover remains separate; this guidance does not claim live triggers changed.

Run the cheapest relevant proof first. Escalate to deep proof when the changed
surface buys signal from it. `ripr` is static mutation-exposure analysis: it
shifts weak test-oracle signal left, while runtime mutation remains the slower
backstop.
