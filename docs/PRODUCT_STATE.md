# Product state assessment (June 2026)

## Category

`ub-review` is an intelligent targeted PR CI gate with review judgment
built in. It replaces fixed CI + cold-start AI review + manual triage
with one runner-owned decision loop.

The core system exists. The job now is to finish the execution loop,
harden the contracts, and make adoption boring.

## Architecture boundary (invariant)

```text
models investigate
sensors produce evidence
proof broker runs commands
resource broker controls the box
compiler decides what earns attention
GitHub broker performs side effects
gate_outcome decides pass/fail
```

Models do not prove correctness. No model finding is proof.

## What is already done

Do not reopen these without a concrete failing receipt.

- Incident-only workflow settings removed; normal `model-mode: auto` and
  heavy-proof posture restored.
- `doctor` gives expected versions and exact repair commands for missing
  or stale tools.
- `ripr` tool-gate detail chain has verifier coverage.
- Focused-build fill-ledger entries carry matching proof and lease anchors.
- Receipt routes use exact `proof_receipts.json#...` and
  `resource_leases.json#...` pointers.
- Adaptor handoff reflects `fill-ledger`, `ripr` exposure gaps, and
  current `audit-ci` / `setup-ci` boundaries.
- Work queue and fill-ledger surfaces exist with verifier coverage.

## PR-by-PR status

### PR 1 — Proof-broker edge reliability — DONE

Lease validation (no lease -> no command), duplicate request dedup,
timeout, `base_patch_failed`, bounded stdout/stderr, cleanup after
partial execution. Issue-ledger #312 closed.

### PR 2 — Deterministic test-impact candidate planning — PARTIAL

Candidate planners exist (`focused_test_candidates_from_diff`,
`focused_test_candidates_from_requests`,
`focused_build_candidates_from_requests`). They are diff-file and
request driven only.

**Gap:** No Cargo package-graph or changed-module expansion. The planner
does not expand a changed module to its reverse-dependency test crates.
A grep for `cargo_metadata`, `package_graph`, `workspace_members`,
`changed_module` returns zero hits.

### PR 3 — Base+tests red/green — DONE

All six states handled: `discriminating`, `non_discriminating`,
`head_failed`, `base_patch_failed`, `timed_out`, `skipped_budget`.

### PR 4 — Targeted heavy witnesses — PARTIAL

Mutation and sanitizer are declared as heavy-witness types with skip and
parked reasons. Config and budget surfaces exist
(`[budgets].mutation`, `[budgets].sanitizer`, `requires_lease`,
`--allow-heavy`).

**Gap:** No executor route for mutation (cargo-mutants) or sanitizer
(asan/msan/tsan). They are parked, not executed. Coverage and miri have
config wiring but miri execution is a nightly external step, not an
in-broker route.

### PR 5 — Lane routing and convergence — PARTIAL

Lane definitions and width routing exist. Sufficient terminal state
works. Late receipt routing reconciles receipts back into candidates.
Per-comment and same-claim dedupe is implemented.

**Gap:** Cross-lane contradiction reconciliation is open
(issue-ledger #147 narrowed). Diff-irrelevance is guidance text, not
enforced routing. Cross-section body dedupe is doctrine but not
structurally implemented.

### PR 6 — Pure-signal compiler enforcement — DONE

`has_forbidden_pr_review_boilerplate` rejects lane rosters, tool tables,
provider status, command logs, generic caveats, successful-tool
announcements, approval filler, and machine-state summaries. Body byte
and bullet caps enforced. Refuted-only and summary-only posts governed.

### PR 7 — Provider and cache reliability — DONE

`max_concurrency` enforced. Backpressure diverts to fallback on
`ProviderBackpressure`. 429/timeout/5xx runtime fallback via
`runtime_fallback_retry_spec`. Prompt-prefix cache accounting with
fresh vs cached token receipts flowing into cost receipt.

### PR 8 — init, audit-ci, setup-ci — DONE

- `cmd_init` renders config proposal and guide.
- `cmd_audit_ci` produces inventory, recommendations, history, costs,
  correlation artifacts (read-only).
- `cmd_setup_ci` requires `--accept <job>=<command>` for required proof
  materialization, generates config + workflow + branch-protection
  instructions, opens PR via `--open-pr` with `--action-sha` pinning.
  Never silently mutates branch protection.

### PR 9 — Follow-up issue capture — DONE

Candidate validation, classification, fingerprinting for duplicate
search, broker plan with allowlist gating. Artifacts:
`review/issue_broker_plan.json`, `review/suggested_issues.md`.

### PR 10 — Economics and calibration — DONE

`review/ub-review-cost.json` (CostReceipt v1), `review/fill-ledger.json`
with verifier parity, `work_queue.json`, floor-trend artifact. Telemetry
fields: `comments_posted`, `comments_accepted`, cached-token accounting.
Issue-ledger #336-339 closed.

**Minor gap:** `floor_time`, `llm_wall_time`, `fresh_tokens` as exact
field names are absent; equivalent data exists under different names.

### PR 11 — Release and install — DONE (workflow ready, tag pending)

Release workflow: tag-triggered, builds Linux x64 archive, emits
`.sha256` checksum, publishes via `gh release`. GitHub Action download
path with checksum validation. Doctor reports install status.
Source-build fallback for non-release refs.

**Remaining action:** cut the actual v0.1.0 tag (issue-ledger #343).

### PR 12 — Fleet rollout — NOT STARTED

No fleet or swarm orchestration code in ub-review. The `*-swarm`
references in `.ripr/suppressions.toml` point to upstream tooling issue
trackers, not ub-review rollout infrastructure.

## Modularization status (June 2026)

`main.rs` reduced from 45,547 to 35,208 lines (-22.7%) via 20 module
extractions. All merged clean through the ub-review gate.

Extracted modules: `prompt_cache`, `providers`, `proof/broker`,
`lanes`, `observations`, `init`, `model_api`, `model_exec`, `validate`,
`render`, `decision_core`, `noise`, `diff_class`, `observation_build`,
`candidate`, `issue_broker`, `quality_backfill_build`, `fill_ledger`,
`follow_up_routing`, `plan_build`, `lane_packets`.

Blocked: `audit_ci.rs` (2,628 lines) and `github.rs` (~1,400 lines)
extraction blocked by ripr-swarm#1324 (runner OOM on large new-file
diffs during ripr analysis).

## Known blockers

- **ripr-swarm#1324:** runner OOM when a PR introduces a large new file
  (~2,600+ lines). ripr's analysis of the full codebase exceeds the 7 GB
  runner budget. Affects `audit_ci.rs` and `github.rs` extraction.
- **Issue-ledger #343:** no published release tag yet.
- **Issue-ledger #147:** cross-lane contradiction reconciliation narrowed
  but not closed.
