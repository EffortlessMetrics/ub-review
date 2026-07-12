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
gate_outcome decides pass/fail/inconclusive
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
- Proof requests are terminalized before final artifacts; receipted requests
  retain their result, duplicates are marked `deduplicated`, and unreceipted
  requests become explicit `deferred` dispositions (PR #747).
- Human-facing review output uses evidence-first structural claim identity and
  preserves distinct claims with shared vocabulary (PRs #749/#750).
- Internal planner language is withheld to artifacts rather than failing a
  valid code gate at the final compiler boundary (PR #751).
- The artifact verifier now enforces watchdog cross-field coherence (PR #753),
  and direct runs fail closed for every non-pass gate conclusion (PR #754).
  Both slices have successful artifact verification but remain unmergeable while
  the required gate reports the upstream RIPR exposure blocker.

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

**Gap:** The Cargo graph always emits its artifact. Shadow, default, and invalid
modes keep candidates artifact-only; explicit active mode may feed the ranked
catalog to model proof planning, while Rust policy and the broker retain
execution authority. The graph still uses `cargo metadata --no-deps`, package
names, and direct manifest dependency names. Before it can determine required
execution it needs package-ID/resolve-graph edges, bounded approved command
templates, and receipt-backed broker execution.

The planner emits `review/proof_intents.json` with an answer-shaped question,
expected-result, proof-kind, value, and stable claim identity for each legacy
request. The deterministic broker now ranks executable candidates by required
floor, discriminating-test value, shared request coverage, and estimated cost;
it reruns that selection after test receipts before considering builds. The
final `review/proof_portfolio.json` records selected, receipt-satisfied,
superseded, declined, and safe-wind-down dispositions without exposing the
request queue to the reviewer.

The portfolio selector now consumes the observed box capacity, profile lease,
remaining hard-deadline window, existing leases, and terminal receipts. Its
portfolio artifact records those inputs and distinguishes box-capacity
declines from safe deadline wind-down.

Model lanes may now submit answer-shaped intents without a command field; Rust
validates their claim, question, expected answer, typed proof kind, and safe
repository target before routing them into the planner artifacts. Legacy
command-shaped requests remain accepted for compatibility.

**Gap:** The executor still consumes the legacy request/task adapters for
execution. A future seam must resolve typed intent targets to approved task
templates before model-native intents can select execution directly.

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
and structural cross-section claim identity is now implemented (PRs #749/#750).
Transactional inline delivery remains in draft PR #748. The upstream RIPR
CLI-subprocess analyzer contract is now merged in RIPR #1455, with the
structured warning consumer and 0.10.1 release-prep work merged in #1457 and
#1456.

### Proof request execution and terminalization — PARTIAL

The focused-test and focused-build broker executes approved requests with
leases, bounded receipts, and follow-up routing. PR #747 closes the final
artifact request queue so no `requested` status reaches the reporter. The
remaining proof-depth gap is production sanitizer/mutation execution; issue
#681 preserves the sanitizer route while its current consumer diff is held on
the upstream RIPR semantic-probe boundary. The preserved watchdog branch also
has local receipt provenance hardening (`8391ed0`), but it is not published
while the same RIPR blocker remains unresolved.

The current-head topic slice is now active at the final compiler boundary:
`review/claim_graph.json` carries structural topics, exact head identity,
current versus stale thread references, proof request/receipt links, and
delivery state. Each topic also records the current thread disposition
(`novel`, `already_covered`, `corroborated_with_new_evidence`,
`refuted_by_new_evidence`, `accepted_tradeoff`, or
`fixed_on_current_head`, or `superseded_by_head_change`). Current-head inline
candidates already covered by an anchored
thread are withheld from a second comment; stale threads do not suppress new
delivery. Transactional replies and confirmed GitHub delivery receipts remain
the separate #748 seam. That PR already carries the pending-review ->
comment-list -> submit lifecycle, cleanup receipts, and focused mismatch
coverage; its hosted gate still awaits a published RIPR 0.10.1 artifact, so
this branch does not duplicate that posting implementation.

### PR 6 — Pure-signal compiler enforcement — DONE

`has_forbidden_pr_review_boilerplate` rejects lane rosters, tool tables,
provider status, command logs, generic caveats, successful-tool
announcements, approval filler, and machine-state summaries. Body byte
and bullet caps enforced. Refuted-only and summary-only posts governed. Body
quality is separate from finding quality: when noisy PR prose is suppressed,
independently validated inline findings remain eligible for concise inline-only
delivery.

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

The release-aware `enable` path is merged and generates release downloads with
cached source builds only when no installable release is resolvable. Generated
workflows use MiniMax primary with optional OpenCode fallback. The release happy
path needs no action SHA; an explicit validated SHA is required only to permit
the cached source-build fallback.

**Remaining action:** cut the actual v0.1.0 tag with maintainer authorization
and execute the release smoke plan recorded in issue #716.

### PR 12 — Fleet rollout — IN PROGRESS (external adoption blocked)

No fleet or swarm orchestration code in ub-review. The `*-swarm`
references in `.ripr/suppressions.toml` point to upstream tooling issue
trackers, not ub-review rollout infrastructure.

The single-gate adoption surface is now executable but has not been applied to
an external repository. A read-only release workflow dispatch on 2026-07-12
completed the build, checksum, and archive-upload stages on `main` (run
29182345044); no tag or release was created, and the Linux asset was not
executed on Windows. The actual v0.1.0 tag remains an explicitly authorized
release action. Current release-installed pilot drafts are Bun #34046 and
perl-lsp-swarm #4015; both remain unable to prove the release path until
ub-review publishes v0.1.0.

Read-only GitHub inspection found the current adoption blockers:

- `EffortlessSteven/bun` has a successful packet-only UB Review workflow, but
  it is advisory and the default branch is not protected.
- `EffortlessMetrics/perl-lsp` has a successful advisory workflow with
  `continue-on-error`, and the default branch is not protected.
- `audit-ci` plus `setup-ci --print-pr` produced fail-closed migration plans
  for both repositories. The Perl-lsp retry included seven days of GitHub
  history; Bun remained inventory-only. Every inspected job is
  `flag-for-human`; no job was eligible for automatic acceptance, so no
  migration plan invented a proof command or old-check removal list.

External adoption therefore requires, in order: an authorized stable release
pin, a reviewed migration PR in each selected repository, a green current-head
gate run, and repository-owner action to require only `ub-review/gate`. Until
those receipts exist, ub-review is a proven self-gate and an advisory external
consumer, not a fleet-wide sole gate.

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
- **RIPR semantic-probe contract:** #748 (transactional inline delivery),
  #681 (production sanitizer witness), #745 (terminal watchdog), and the
  published follow-ups #753/#754 have
  green functional proof but remain unpublished or unmergeable while RIPR
  reports unresolved CLI/subprocess or semantic-probe exposure gaps. No
  aliases or threshold relaxation are permitted; current upstream tracking is
  ripr-swarm#1528 and the related semantic-probe fixes.
- **Issue-ledger #147:** closed (PRs #459/#460). Cross-lane conflict detection
  and suppression shipped; deeper evidence-precedence adjudication is the open
  next step (epic #655 milestone 4).
  but not closed.
