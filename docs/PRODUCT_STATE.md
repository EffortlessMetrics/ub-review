# Product state assessment (July 2026)

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
`focused_test_candidates_from_requests`, and
`focused_build_candidates_from_requests`). The Cargo workspace graph now
identifies changed-package ownership, direct reverse-dependency candidates,
declared targets, and ranked test candidates.

**Gap:** The Cargo graph remains a shadow/advisory plan: it uses
`cargo metadata --no-deps`, package names, and direct manifest dependency
names, and does not yet change command execution. Before activation it needs
package-ID/resolve-graph edges, bounded approved command templates, and
receipt-backed broker execution.

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

**Status:** Cross-lane contradiction detection and suppression are DONE
(issue-ledger #147 closed by PRs #459/#460: surface-aware lane gating +
explicit cross-lane conflict receipts). Conflicted findings are suppressed
and replaced with a verification question. Deeper evidence-precedence
adjudication (resolving which side wins using proof receipts) is the open
next step (tracked in epic #655, milestone 4). Diff-irrelevance is
guidance text, not enforced routing. Cross-section body dedupe is doctrine
but not structurally implemented.

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

**Remaining action:** merge the release-aware `enable` path, refresh the
release packet with the exact candidate SHA and receipts, then cut the actual
v0.1.0 tag with maintainer authorization (issue #716).

### PR 12 — Fleet rollout — NOT STARTED

No fleet or swarm orchestration code in ub-review. The `*-swarm`
references in `.ripr/suppressions.toml` point to upstream tooling issue
trackers, not ub-review rollout infrastructure.

## Modularization status (June 2026)

`main.rs` reduced from 45,547 to 21,764 lines (-52.2%) via the cleanup train
(one pure-code-motion extraction per PR, reached step 59). All merged clean
through the ub-review gate.

49 top-level modules now live under `src/*.rs` (plus the `proof/` subtree of
10 files and the `sensors/` subtree of 5 files). Extracted dispatchers include
`cmd_init` (`init.rs`), `cmd_gate_check` (`gate.rs`), and the
`cmd_quality_github_*` pair (`quality_github.rs`). Remaining seams worth
extracting when next touched:

- the inline `mod tests` in `main.rs` (~19.6k lines, 337 tests) — co-locate
  tests with the modules they exercise;
- the CI subsystem (`cmd_audit_ci`, `cmd_setup_ci`, ~25 `ci_*` helpers, YAML
  parse, GitHub REST, gate-workflow templating) — a ~2,300-line island
  unrelated to the review pipeline;
- the `pub(crate) use x::*` glob re-exports — replacing them with explicit
  imports would create real module boundaries (currently modules are
  physically split but logically one flat namespace).

## Known blockers

- **ripr-swarm#1324:** runner OOM when a PR introduces a large new file
  (~2,600+ lines); ripr's analysis of the full codebase exceeds the 7 GB
  runner budget. The self-gate is back at a strict zero ceiling; any future
  exception must be narrowly receipted and evidenced.
- **Issue #716:** no published `v0.1.0` GitHub Release yet. The release packet
  must name the exact candidate SHA, pre-tag receipts, archive/checksum names,
  downstream smoke plan, and rollback before maintainer authorization.
- **Issue-ledger #147:** closed (PRs #459/#460). Cross-lane conflict detection
  and suppression shipped; deeper evidence-precedence adjudication is the open
  next step (epic #655 milestone 4).
  but not closed.
