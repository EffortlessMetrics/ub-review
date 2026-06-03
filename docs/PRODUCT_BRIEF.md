# ub-review

ub-review is an evidence-first CI review gate.

It turns one CI runner into a review cockpit: checkout once, diff once, run on-diff sensors once, start read-only model lanes immediately, run relevant proof centrally, and post one concise PR Review.

The goal is not the fastest possible review. The goal is the best useful review inside the runner budget.

## Operating principle

Whole-runner stewardship: while the runner is live, every useful resource serves the review.

CPU runs focused tests. Disk holds proof worktrees. Memory holds evidence packets. Model budget goes to reasoning over prepared evidence. Remote model calls run concurrently with local proof; provider wait does not occupy the runner's local compute lease. Time is spent producing receipts.

## Product sentence

ub-review prepares evidence, runs focused investigation lanes, proves what it
can, and reports only what changes the reviewer's decision.

## Review contract

The PR body contains only reviewer-value content:

- findings
- verification questions
- proof results
- refutations
- parked follow-ups
- specific evidence gaps that affect trust

Everything else goes to artifacts:

- lane outputs
- model status
- sensor logs
- proof stdout/stderr
- resource leases
- metrics
- raw observations

The default `[review_body]` policy enforces that split:

```toml
[review_body]
include_successful_lane_table = false
include_provider_table = "on_failure"
include_sensor_table = "on_failure"
include_execution_summary = "none"
```

## Execution contract

Models investigate. Tools produce receipts. The compiler decides what earns the
reviewer's time.

## External positioning

ub-review is proof-backed PR review. It uses the CI runner as an investigation bench: prepare evidence once, run focused model lanes, execute relevant proof centrally, and post only the result a reviewer needs.

## Product category

`ub-review` is an evidence-first CI review gate. It replaces the usual split
between generic review bot, ad hoc proof job, and manual first-pass triage with
one runner-owned review pass:

```text
published sensors -> evidence planner -> proof broker -> model lanes -> review compiler
```

The category is not "LLM review comments." The category is proof-backed review
inside CI. The runner prepares evidence once, leases local proof work while
remote model calls wait on network I/O, and compiles one review artifact that a
maintainer can act on.

## Sensor moat

The defensible layer is the sensor stack:

- `tokmd` prepares deterministic repository and diff packets.
- `ripr` exposes changed-behavior test-oracle weakness before slower mutation
  runs.
- `unsafe-review` makes unsafe/native seams reviewable through safety-contract,
  guard, reach, and witness evidence.
- `cargo-allow` keeps controlled exceptions owned and auditable.
- MergeCode can become the semantic context sensor for non-Rust code once it is
  wired into the packet path.

The orchestration layer is intentionally ordinary: route context, ask bounded
questions, schedule proof once, and compile a concise review. The sensors make
that orchestration useful because they turn a raw diff into evidence that can be
routed, checked, refuted, and receipted.

## Economic model

The budget rule is simple: the runner is the scarce resource, not model tokens.
A standard pass should spend one GitHub runner lease on evidence preparation,
model fanout, focused proof, and one grouped review. Remote model calls are
network I/O; they run concurrently with local proof and should not cause the
CPU, disk, or checkout to sit idle.

Target operating envelope:

| Deployment | Target cost posture |
|---|---|
| GitHub-hosted runner | Two useful review passes per PR under the small-runner budget. |
| Self-hosted runner | Same evidence contract with lower per-PR runner cost. |
| Trusted repo default | `opened` and `ready_for_review`, no default `synchronize` spend. |

The proof broker and evidence planner exist to keep this economics true: only
proof likely to change the review decision should consume the local lease.

## Dogfood proof

The first proof wedge is the Bun UB work. The Bun profile forces the system to
review hard native-boundary changes where generic diff chat is weakest:

- resizable ArrayBuffer active-view and backing-store mistakes;
- stale pointer/length hazards;
- detach, transfer, worker handoff, and GC lifetime risks;
- tests that execute code but fail to prove the changed behavior;
- unsafe/native contracts that need explicit review evidence.

The product story and the audit story are the same story: UB findings are sensor
receipts that demonstrate why evidence-first review is materially different from
model-only diff navigation.

## Honest risks

- The product is not complete until the proof broker, resource broker, coverage
  sensor, ready-pass context ingestion, evidence planner lane, and MergeCode
  sensor path are implemented and proven together.
- The Bun evidence is strongest when fixes merge upstream; unmerged findings are
  still useful calibration but weaker external proof.
- The sensor moat is currently Rust-deep. MergeCode integration is the path from
  Rust-first strength to broader language coverage.
- The review compiler must stay strict. Missing evidence, provider failures, and
  skipped sensors must remain explicit gaps, never positive safety claims.

## Next proof obligations

1. Ship and calibrate the remaining Bun UB findings.
2. Build the proof broker so lanes request proof and the runner executes it once.
3. Wire MergeCode as a semantic context sensor.
4. Add the evidence planner lane that selects PR-specific proof work.
5. Keep the artifact contract auditable enough that every review claim has a
   route back to diff, sensor, model lane, or proof receipt.
