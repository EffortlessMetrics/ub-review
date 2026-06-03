# Requirements

## Product

- Evidence-first PR packet builder.
- Box-aware profile selection.
- Fast sensors first; heavy tools behind leases.
- Lane-specific evidence routing.
- Bounded MiniMax model lanes over shared context, with optional OpenCode Go direct provider canary.
- One grouped Pull Request Review with validated inline comments.
- Append-only events and single-writer running summary.
- External UB ledger support.
- Source exception ledger receipts through `cargo-allow` when adopting the Rust
  repo template.
- No rubber-stamp review language.
- No fake verification from missing evidence.
- Sensor defects are filed upstream in the matching `*-swarm` repo instead of
  silently forked into local `ub-review` behavior.

## Non-goals

- Many issue comments or one comment per lane.
- Replacement for Droid Action.
- Blocking SAST gate.
- Proof of memory safety, security, or test adequacy.
- `cockpitctl` integration in this first scaffold.
