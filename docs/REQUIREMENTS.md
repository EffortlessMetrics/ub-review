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
- No rubber-stamp review language.
- No fake verification from missing evidence.
- Sensor defects are filed upstream in the matching `*-swarm` repo instead of
  silently forked into local `ub-review` behavior (canonical statement + routing
  table: `docs/specs/UB-REVIEW-SPEC-0016-sensor-upstream-boundary.md`).

## Non-goals

- Many issue comments or one comment per lane.
- Replacement for Droid Action.
- A traditional blocking SAST gate that reds on lint/static-analysis rules
  alone. (`ub-review/gate` *is* a blocking required check per ADR-0002, but
  it blocks on deterministic proof receipts and explicitly-marked blocking
  findings — a different posture from a SAST-only gate. See ADR-0002 for the
  single-gate decision that supersedes the literal reading of this non-goal.)
- Proof of memory safety, security, or test adequacy.
- `cockpitctl` integration in this first scaffold.
