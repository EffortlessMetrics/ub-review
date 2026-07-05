# Architecture

`ub-review` is an evidence control plane for PR review.

```text
immutable source checkout
  -> shared diff/context packet
  -> fast sensors under profile limits
  -> lane-specific packets
  -> bounded MiniMax/OpenCode Go direct provider lanes
       (late-phase sensors run concurrently, join before reporter/gate)
  -> validated inline candidates
  -> one grouped Pull Request Review
  -> append-only events
  -> single-writer running summary
```

## Pipelined evidence phases (#325)

Sensor execution is two-phase. Fast sensors (static, search, packet,
workflow, security classes) complete before the shared context prefix is
rendered — they are the deterministic signal model lanes launch from. Late
sensors (test, build, coverage, lease-gated witnesses; per-tool override via
`[tools.<id>].phase`) run on a background pool concurrently with the model
wave, hiding the slow suite under lane network latency. The late pool is
joined before the reporter, tool-status/gate-outcome computation, the review
compiles, and the gate — so the gate always evaluates complete sensor
evidence, and a late sensor without a receipt at join time is missing
evidence, never clean evidence. Late receipts are routed to the reporter and
into lane continuation turns as "late deterministic evidence"
(stream-as-it-lands: direct provider lanes are single network calls, so
post-wave continuation turns are the streaming surface). `--sensor-phases
serial` restores single-phase execution.

## Mutation model

| Zone | Policy |
|---|---|
| Source checkout | immutable |
| Lane scratch | mutable by lane owner only |
| Sensor artifacts | immutable once emitted for a run |
| Event log | append-only |
| Running summary | single-writer |
| Patch worktree | mutable only with an explicit lease |
| PR review | one grouped review compiled from validated candidates |

## Posting model

`ub-review run` writes review artifacts. `ub-review post` submits
`review/github-review.json` as one Pull Request Review. Sensors and individual
lanes do not post comments directly, and the runner does not create issue
comments or status-comment chatter.


## Standard Rust evidence stack

For Rust repositories, the clean review stack separates static evidence,
runtime backstops, and retained policy:

```text
cargo-allow   = durable exception ledger
ripr          = static mutation-exposure analysis
unsafe-review = static unsafe-contract review
xtask         = orchestration / receipts / repo policy
cargo-mutants = runtime mutation backstop
Miri          = concrete UB execution backstop
Codecov       = execution-surface telemetry
```

The architecture is static first, runtime where it pays, receipts everywhere.
`ub-review` consumes those receipts and routes them to review lanes; it should
not blur tool ownership by treating unsafe-review cards as proof of soundness or
Miri/runtime receipts as retained exception policy.

## Sensor issue boundary

`ub-review` owns orchestration, routing, model fanout, posting, and fallback
behavior. It should not silently absorb sensor defects into local glue
(canonical upstream-boundary contract + routing table: `docs/specs/UB-REVIEW-SPEC-0016-sensor-upstream-boundary.md`).

When a real sensor issue blocks the Bun UB lane, file a focused upstream issue:

| Sensor area | Upstream repo |
|---|---|
| `ripr` bug or weak command/output contract | `ripr-swarm` |
| `unsafe-review` bug or weak ReviewCard/witness/comment-plan contract | `unsafe-review-swarm` |
| `tokmd` bug or weak packet/manifest/context contract | `tokmd-swarm` |

The issue should include a minimal repro, command run, expected behavior, actual
behavior, artifact excerpt, Bun UB impact, and proposed acceptance criteria.
Local workarounds are allowed only to keep `ub-review` usable, and should link
to the upstream issue.

## Agent modes

The v0 hot path is `review-byok`: BYOK MiniMax M3 calls from the Rust runner
over one shared evidence packet. `intelligent-ci` names the required-gate
product mode. Legacy `review-direct` is accepted as an alias. OpenCode Go is an
optional direct provider lane, not an agent orchestrator. GLM is skipped for v0.
`agent-investigate` and `agent-patch` are reserved for future leased
Codex/OpenCode/Pi-style workers and are not part of the default review path.
