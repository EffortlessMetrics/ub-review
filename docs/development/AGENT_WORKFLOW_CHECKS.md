# Agent workflow decision checks

At the introduction base, `AGENTS.md` and the native workflow covered policy,
cheap-first validation and lane ownership, but supplied no skill routing,
checkpoint continuation, replacement-scope procedure, isolated reviewer lab or
agent PR evidence procedure. The owning delivery/review skills fill those gaps.
They preserve `CLAUDE.md` commands and the existing native workflow; no product
coordinator, GitHub authority or deployment behavior changes.

Run the small deterministic regression set with the ordinary workspace tests,
or narrowly:

```text
cargo test --locked -p xtask agent_workflow
cargo xtask agent-workflow-scenarios
cargo xtask agent-workflow-check <answers.json>
```

For a behavioral run, give a fresh independent agent the scenarios output and
access to this exact checkout. Start discovery from `AGENTS.md`; do not give it
the expected answer arrays in the Rust checker. Request its JSON answer array
without performing the hypothetical actions. Save the unedited answer outside
the source tree, then run the checker. Retain candidate base/head/tree, agent
identity, prompt and answer bytes/digests, checker command/outcome and any
failures. Re-run when the guidance changes. The five cases cover workflow
discovery, continuing after a status/recovery checkpoint, replacing an explicit
old assignment, isolated review experiments and native exact-head evidence.

The oracle checks reported next-action decisions, cited workflow, unique and
complete cases and answer shape. Deterministic negative controls reject omitted
critical actions and harmful substitutions in each case. The discovery check
also resolves the actual skill routes from `AGENTS.md`. These checks do not
execute an agent, authenticate that it read a skill, assess free-text reasoning
or establish real delivery/review behavior. Actual implementation still needs
the native exact-head checks and independent review. A model run is bounded
decision evidence only; each case requires its essential actions and permits
named relevant follow-ups. Native guidance rereads, historical retention and
owner handoff are valid common follow-ups. The evidence case requires explicit
integration-condition recording or owner handoff, since the delivery skill's
handoff includes those conditions. This is a bounded action oracle rather than
a general natural-language evaluator.

This harness runs without credentials or model calls in baseline tests. Model
scenario runs are explicit exercises within the current permission/resource
boundary, not a new CI lane. Keep live assignments, permissions, status and
scenario receipts in native task/PR state or retained local artifacts, outside
reusable skills.
