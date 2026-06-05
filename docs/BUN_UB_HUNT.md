# Bun UB Hunt Handoff

This is the operating handoff for using `ub-review` on the Bun fork. It keeps
the Bun-specific doctrine out of chat and points implementers to the evidence
they need before upstreaming a fix.

## Purpose

The Bun gate is an evidence-first PR gate for review-fast UB fixes. It should:

- run on `pull_request.opened` and `pull_request.ready_for_review`;
- use the pinned `bun-ub` profile from [GH_RUNNER_BUN.md](GH_RUNNER_BUN.md);
- upload the complete `target/ub-review` packet;
- post only decision-changing review text;
- leave Droid and other bots auxiliary.

The current known-good action pin remains:

```text
EffortlessMetrics/ub-review@804d198b5a15a0df94bb4f43750dba71165916cd
```

Move that pin only after the local verifier passes and a Bun consumer workflow
uploads a valid packet.

## Hunt Invariant

Hunt by invariant, not surface name:

```text
Rust/native code must not retain or later materialize JS-owned bytes after JS
can resize, detach, alias, mutate, race, or reenter.
```

Common surfaces include ArrayBuffer and typed-array paths, FFI buffers, native
copy helpers, file/database/network codecs, worker transfer paths, and any
native API that stores a pointer/length pair derived from JS-owned memory.

## Review-Fast PR Shape

A Bun UB PR should give the gate and reviewer a narrow proof obligation:

- one coherent seam;
- explicit claim boundary;
- changed-route map;
- sibling paths classified as fixed, unaffected, or parked;
- focused test or witness route;
- red/green receipt when the bug is observable;
- parked follow-ups with exact missing evidence.

Avoid broad cleanup. Do not mix unrelated UB families in one PR.

## Proof Rules

Use the cheapest proof that decides the claim:

| Claim type | Required evidence |
|---|---|
| Observable wrong behavior | focused HEAD proof plus base+tests red/green |
| Non-observable UB | mutation, Miri, type/model proof, or another explicit witness route |
| Crash class | ASAN, debug trap, native symptom, or focused crash test |
| Source inspection only | not sufficient |

Coverage is execution-surface telemetry. It can show whether code ran; it does
not prove the fix is correct or UB-free.

## Tool Roles

Use each tool for its own evidence layer:

| Tool | Role |
|---|---|
| `tokmd` | PR packet, changed-route cockpit, bounded context |
| `cargo-allow` | owned source-tree exceptions |
| `ripr` | static mutation-exposure / weak-oracle signal |
| `unsafe-review` | unsafe/native safety contract and witness route |
| `ast-grep` | structural sibling scans |
| `actionlint` | workflow diffs |
| Codecov / coverage | execution-surface telemetry |
| cargo-mutants / Miri / ASAN | scoped runtime backstops |

Missing configured tools are evidence gaps. On the standard image, missing core
tools are image drift and should fail `ub-review doctor --require-core-tools`.

## Packet Reading Order

Start with:

```text
target/ub-review/running-summary.md
target/ub-review/review/review.md
target/ub-review/review/proof_receipts.json
target/ub-review/lanes/tests.md
target/ub-review/lanes/ub.md
target/ub-review/lanes/source-route.md
target/ub-review/input/diff.patch
```

Then inspect the matching sensor directory when a claim depends on a tool:

```text
target/ub-review/sensors/tokmd/
target/ub-review/sensors/ripr/
target/ub-review/sensors/unsafe-review/
target/ub-review/sensors/coverage/
```

## Reviewer Surface

The PR review body is scarce. Allowed sections are decision, findings,
verification questions, proof results, refutations, parked follow-ups, and
specific evidence gaps.

Do not post:

- lane rosters;
- provider tables;
- setup logs;
- command logs;
- generic residual risk;
- no-finding boilerplate;
- missing-tool chatter that cannot change this PR's decision.

Full receipts stay in artifacts.

## Feedback Loop

When a tool blocks or misleads the Bun lane, file a grounded issue in the
matching repo instead of hiding the defect in `ub-review` glue. Include:

- command;
- repo/ref;
- artifact path and excerpt;
- expected behavior;
- actual behavior;
- Bun UB impact;
- acceptance criteria.

Use [SENSOR_ROUTING.md](SENSOR_ROUTING.md) for the current filing map.

## Related Docs

- [ACTION_CONSUMER_BUN.md](ACTION_CONSUMER_BUN.md) for the consumer workflow.
- [GH_RUNNER_BUN.md](GH_RUNNER_BUN.md) for the known-good pin and artifact check.
- [RUNNER_IMAGE.md](RUNNER_IMAGE.md) for required tools and doctor behavior.
- [REVIEW_BODY_CONTRACT.md](REVIEW_BODY_CONTRACT.md) for reviewer-facing text.
- [calibration/bun-ub-review-ledger.md](calibration/bun-ub-review-ledger.md)
  for real run calibration notes.
