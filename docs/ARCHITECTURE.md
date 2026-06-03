# Architecture

`ub-review` is an evidence control plane for PR review.

```text
immutable source checkout
  -> shared diff/context packet
  -> fast sensors under profile limits
  -> lane-specific packets
  -> bounded MiniMax/OpenCode Go direct provider lanes
  -> validated inline candidates
  -> one grouped Pull Request Review
  -> append-only events
  -> single-writer running summary
```

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

## Repo-style quality stack

For Rust repos, `ub-review` treats `ripr` as the static mutation-exposure layer in the quality stack, not merely as a loose advisory note:

```text
cargo-allow = durable source-exception ledger
ripr = static mutation-exposure / repair-routing layer
xtask = repo control plane and receipt aggregator
cargo-mutants = runtime backstop
Codecov = execution-surface receipt
```

`ripr` should answer whether changed behavior appears exposed to a meaningful oracle in the current tests. Runtime mutation remains the targeted/nightly/release confirmation path. See `docs/ci/ripr.md` for the claim boundary, packet shape, and repair loop.

## Sensor issue boundary

`ub-review` owns orchestration, routing, model fanout, posting, and fallback
behavior. It should not silently absorb sensor defects into local glue.

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

The v0 hot path is `review-direct`: direct BYOK MiniMax M3 calls from the Rust
runner over one shared evidence packet. OpenCode Go is an optional direct
provider lane, not an agent orchestrator. GLM is skipped for v0.
`agent-investigate` and `agent-patch` are reserved for future leased
Codex/OpenCode/Pi-style workers and are not part of the default review path.
