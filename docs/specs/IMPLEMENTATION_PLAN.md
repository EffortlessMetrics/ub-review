# Release surface implementation plan

Status: authored 2026-06-06, closing the release surface spec wave (PR 8).
This document routes every piece of open release work through the specs
(UB-REVIEW-SPEC-0001 through 0010). It adds no implementation; each slice is
one PR-sized unit with its governing spec, its issue, and its proof
obligation. Work not listed here and not an emergency should not start until
it is routed here.

## Routing table

Ordered by leverage; one slice per PR.

```text
slice                                          spec    issue   proof obligation
------------------------------------------------------------------------------
1  DONE #335: ripr receipt chain — sensor      0005    #316    proven: run 27077206713 shows
   emits gate-decision.json from badge-json            (+ripr- evaluated: true; production
   stdout, parser reads                                swarm   blocks on PR #342 / #346;
   counts.unsuppressed_exposure_gaps,                  #1038)  follow-up: receipt depth
   doctor pins ripr 0.8.0 /                                    stops at counts (#347 - CLOSED)
   unsafe-review 0.3.3, unevaluated-
   required-gate alarm in running summary
2  end-to-end loop-closure verifier test:      0004    #314    negative self-test proves a
   leaked refuted surface fails                        leaked refuted candidate
   require_final_compiler_input                        surface reds the verifier
3  [providers] config parsing + per-provider   0006    #310    DONE: config keys read
   max_concurrency + 429 backpressure                          with CLI precedence;
   (prompt_cache/model/env/role config                         a rate-limited provider
   choices remain separate)                                    sheds load instead of
                                                               failing waves
4  xtask precommit: missing tools exit         0005    #320    missing-tool receipt
   distinctly, receipts say how to install,            #321    distinguishable from
   bounded stdout embedding                            #317    relevance skip; ripr.md
                                                       bounded on a loud diff
5  proof-broker edges: lease absent status,    0003    #312    each edge pinned by a test;
   base_patch_failed surfaced to requesting            base_patch_failed visible to
   lanes, manual-cost allowlist test,                  the lane that asked
   shell-token end-to-end rejection test
6  DONE: tokmd below-pin failure reason        0005    #319    sensor receipt names the
   surfaced before --preset commands run               version gap, not a bare failure
7  DONE: cargo-allow foreign-dialect          0005    #318    resolved-tools, sensor status,
   ledger skips with linked reason                    tool-status, and tool-gate
                                                      preserve the same reason
8  DONE: synchronize_mode deleted with        0003    #306    setting it yields a
   PolicyError deprecation                             deprecation receipt; post_review_on
                                                       remains the posting policy
9  setup-ci v1 per the 0008 contract           0008    item28  --print-pr emits the
                                                       migration PR content; never
                                                       mutates branch protection
10 rust-test-proof profile + first             0003    item27  a second repo onboards with
   multi-repo onboarding                               ~10 lines of TOML, zero model
                                                       keys, one required check
11 audit-ci v1: model-assisted judgment,       0007    -       judgment field earns values
   permissions/secrets extraction,                     beyond "deterministic";
   branch-protection query                             required_check_source resolved
12 coverage flake trend: close or diagnose     0005    #313    second occurrence captures
                                                       llvm-cov stderr, or issue closed
```

Tool-repo dependencies (filed upstream, tracked there, never absorbed
locally): ripr-swarm #1035-#1038, unsafe-review-swarm #1516-#1518,
cargo-allow #1467-#1470, tokmd-swarm #219-#221. Slice 1 consumes
ripr-swarm #1038 (compact gate receipt) if it lands; otherwise the parser
adapts to the receipt ripr ships today.

## Open product decisions

Decisions surfaced by the spec drafting and fact-check passes. Each blocks
or shapes a slice; none blocks the wave.

```text
D1  RESOLVED 2026-06-06: the design center is arbitrary provider/role
    routing declared in repo config - the MiniMax+OpenCode pair is one
    instance, not the product shape. Defaults stay conservative; the
    [providers] surface carries the variation per repo.       shapes slice 3
D2  RESOLVED 2026-06-06: config wins, CLI overrides.          shapes slice 3
D3  keep or remove the `auto` provider-policy alias (today identical to
    minimax-primary)                                          shapes slice 3
D4  opencode-model default: stay minimax-m3 (like-for-like fallback) or
    move to an OpenCode-native model?                         shapes slice 3
D5  gate_outcome.json under verifier coverage in addition to gate-check,
    or keep the deliberate enforcement split?                 shapes slice 2
D6  schema strings for the schema-less stable artifacts (plan.json,
    proof_requests.json, follow_up_* arrays, observation summaries), or
    declare existence+field checks their contract tier?       shapes 0004 rev
D7  promote follow-up question packets to stable (prompt material is
    byte-checked already) or keep internal?                   shapes 0004 rev
D8  opencode-go-canary: surface missing_key at assignment time instead of
    run time?                                                 shapes slice 3
D9  unknown tool-bundle values: hard-fail instead of silently defaulting
    to core?                                                  joins slice 4
```

## Sequencing

Slices 1-2 first: they close the two gaps where the gate's own posture is
weaker than its policy text (a configured threshold that never evaluates,
and an enforcement loop pinned only by halves). Slice 3 next, after D1-D4.
Slices 4-8 are independent and small; interleave freely. Slices 9-11 are
the product expansion train (ADR 0002 items 27/28) and follow the same
spec-first discipline: contract in the spec, then code. PR #232 (crate
split) stays parked - modules are organization, crates are contracts, and
the current contract is the gate.

## Wave receipts

Specs 0001-0010 merged via PRs #323, #324, #326, #328, #329, #331, #332,
each docs-only through the live gate. Authoring process: six-area code
inventory, subagent drafts, adversarial fact-check fleet (37 corrections
applied pre-merge across nine specs), main-lane review. The correction
classes worth remembering: enforcement-mechanism overclaims, reason-kind
taxonomy drift, alias semantics (review-direct never enforces), and
declared-but-unwired config keys presented as behavior.
