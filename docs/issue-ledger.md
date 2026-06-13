# Issue ledger

Triage of every open issue against receipts in the tree (main @ 26bd360,
2026-06-07). Discipline: an issue is `closed` only with a concrete receipt
(merged PR, named test, artifact); `narrowed` issues carry the exact
remaining delta as a comment on the issue; everything else is `keep` with a
one-sentence next action. Umbrella issues are not worked as written - they
are converted into one-promise-one-verifier slices. Re-triage whenever a
bucket-2 item merges.

Tally this pass: 4 closed, 8 narrowed, 15 kept.

## Closed this pass (receipts on the issues)

```text
#66   invalid_json lane drops      degradation path + 4 named tests; degraded
                                   lanes count attempted, never missing
#73   findings vs evidence gaps    gate_outcome blocking/advisory split (#340
                                   verifier-covered); pr_decision_sentence
                                   gates on findings only
#74   no boilerplate, lead with    render_pull_request_review_body; absent
      Decision                     sections test-pinned
#218  tokmd 1.12 contract          PR #220; STANDARD_IMAGE_TOKMD_VERSION
                                   1.12.0; --preset bun-ub commands pinned
```

## 1. Stale / solved - remaining after the closure pass

```text
#77   narrowed  terminal sufficient state shipped; remaining: cross-pass
                convergence (pass N+1 vs pass N resolutions) + materiality
                threshold
#306  DONE      synchronize_mode removed from config contract; legacy configs
                receive a deprecation PolicyError receipt
#310  narrowed  retry (#315), [providers].policy D2 (#351),
                per-provider max_concurrency, and 429/timeout/5xx
                wave shedding (`model_error_triggers_provider_backpressure`)
                shipped; future provider config choices remain separate
```

## 2. Gate trust and evidence detail (highest-priority implementation bucket)

Rule held throughout: configured behavior must execute; executed behavior
must leave receipts; missing/degraded evidence must be distinguishable from
skipped-by-policy.

```text
#347  closed  sensors/ripr/exposure-gaps.json ships per-finding gap
            detail with verifier reconciliation against badge counts
            (train step 4)
#312  keep  proof-broker edges; split into 4 slices: lease `absent` branch
            (two-line fix + test), base_patch_failed lane surfacing,
            manual-cost allowlist test, shell-token e2e
#313  keep  coverage exit-101 trend issue by design; 2-week auto-close
            horizon, capture llvm-cov stderr on recurrence
#317  keep  xtask run_capture buffers unbounded stdout (450 MB ripr.md);
            head+tail budget + stdout_truncated marker
#318  closed  foreign-dialect policy/allow.toml skips with linked reason;
              CLI artifact test pins resolved/tool-status/gate parity
#319  closed  tokmd version preflight fails before preset-bearing commands
              and names installed vs pinned version in the sensor receipt
#396  closed  unsafe-review exit 1 is completed policy-finding evidence;
              exit 2/tool errors remain sensor failures
#320  keep  xtask missing tools = success:true skipped; add distinct
            `missing` status mirroring the main binary (pair with #321)
#321  keep  missing-tool receipts lack install guidance; one shared hint
            table for xtask precommit + doctor (pair with #320)
```

Note #316 is already closed: #335 production-evaluated the ripr threshold
(run 27077206713) and it has blocked three real PRs (#342, #346, #351).

## 3. Economics / telemetry (how the gate learns)

Strict sequence: #336 -> #337 -> #338 -> #339. Artifacts only by default;
no invented quality score.

```text
#336  keep      ub-review-cost.json; inputs all exist (metrics, cache
                events, provider receipts, ci-budget.toml)
#337  keep      suggested-fill ledger; proof-planner rationale is the source
#338  keep      floor-time trend; pure aggregation over #336 receipts
#339  keep      quality telemetry (posted vs accepted, fills with signal);
                per-run receipt first, nightly backfill second
#325  narrowed  ask (c) shipped (proof broker on-demand focused proof);
                remaining: (a) lanes off fast precontext, (b) streaming
                late deterministic outputs into running lanes
```

## 4. Adoption

```text
#343  keep      zero releases exist; cut v0.1.0 with the Linux x64 archive
                + checksums; SPEC-0010 specifies the surface; first
                external consumer is already live
#327  narrowed  audit-ci/setup-ci ship the gate-generation half (#299,
                #354, #355); remaining: LLM-guided config proposal +
                file-driven ub-review-init.md mode
```

## 5. Older architecture umbrellas (converted, not worked as written)

```text
#75   narrowed  red/green half shipped (proof broker allowlisted execution,
                discriminating classification); remaining: scoped
                sanitizer/mutation issue behind allow-heavy
#76   narrowed  cache/planner/[[lanes]]/doctrine shipped; remaining two
                slices: diff-class lane routing, PR-thread seed packet
#147  narrowed  refuter covers part of contradiction; remaining: cross-lane
                conflict surfacing (lane-gating half folds into #76)
#178  done      file:line dedup, same-claim-different-line dedup,
                and value-ranked body/inline cap shipped in the compiler
```
