# UB-REVIEW-SPEC-0011 - lane doctrine: what makes a good review per lane type

Status: authored 2026-06-06 (release surface spec wave follow-on; docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
This spec is measured, not aspirational: every maxim, good example, and
failure mode below comes from a mined corpus of the gate's own reviews on
this repository's PRs #305-#335 (2026-06-06), with ground truth - we know
which findings were applied, which exposed latent bugs, which were noise,
and which defect the lanes missed entirely.

## Purpose

The lane system is layered:

```text
1. lane doctrine        what a good review looks like per lane type (this doc)
2. builtin lane library lanes implementing the doctrine
3. selection            repos pick lanes by id (config / CLI selectors)
4. init guidance        ub-review init inspects the repo and recommends
5. custom lanes         [[lanes]] in .ub-review.toml, written to this doctrine
```

Selection and init guidance are only as good as the doctrine they point at,
and a custom lane without doctrine is vibes with a model bill.

## The corpus and its verdict

Fifteen PRs, one day, gate reviewing its own repository: 5 substantive
reviews, 7 proof-only one-liners, 2 total silences. Ground truth:

- every applied finding except one style nit came from the
  tests-red-green lane;
- the two PRs that touched the Rust<->Python verifier mirror - the repo's
  highest-risk surface - received total silence; the one mirror drift that
  day was caught by the deterministic verifier (run 27073001145), never by
  a lane;
- worse: on PR #309 the opposition lane raised exactly the right objection
  ("duplicating the matchers in Python and Rust is an architecture smell")
  and the refutation step dismissed it citing an unverified claim
  ("the self-test pins byte-level equivalence"); the drift that falsified
  that refutation landed within hours;
- Bun-preset calibration leaked into a forbid(unsafe_code) repository:
  Box::from allocation-failure refutations restated three times in one
  review, aliasing-UB hypotheses on safe code;
- the same coverage-sensor failure was restated five times across bodies;
  the identical decision line ("Needs reviewer attention before upstream:
  findings remain") opened every substantive review carrying zero severity
  signal.

## Maxims (corpus-extracted, all five falsifiable)

```text
1. State the missing test with its exact oracle. Both applied #315
   findings named the scenario AND the assertions ("calls does not exceed
   max_model_calls, loop terminates, receipt is terminal"). Findings
   without an oracle were ignored.

2. Cite line + repo precedent, not severity. The applied style nit
   pointed at the literal and the existing repo style; every "could be
   UB/lifetime risk" claim was dead on arrival in a forbid-unsafe,
   deny-unwrap repo.

3. Refutations need receipts as much as findings do. A refutation that
   dismisses a real defect class on an unverified claim is worse than no
   refutation - it spends the objection.

4. Calibration is preset-scoped. Check the target repo's lint posture
   before importing a threat model; surfacing (or even self-refuting) a
   foreign preset's known false premises is pure token cost.

5. Say each thing once, and make silence deliberate. Repetition is a
   dedupe failure; silence should key on touched surfaces (mirror files,
   gate policy), never on whether lanes happened to emit text.
```

## Lane-type doctrine

Template per type: the user question the lane answers, the evidence a good
finding cites, a real good example, the real failure mode, default
`receives` wiring, and what the lane must never do. All lanes inherit the
global never-do set: no approvals, no lane rosters, no severity without a
receipt, no restating the diff, no generic caveats.

### tests-red-green (proof-oracle review)

- User question: do the tests distinguish old behavior from new, and prove
  the PR claim?
- Good finding cites: the uncovered scenario, the exact missing oracle
  (assertions), and which existing test is closest.
- Corpus good example: "Add a wave-loop test where primary fails retryably
  and max_model_calls is set so the queued retry cannot run. Assert: calls
  does not exceed max_model_calls, loop terminates, and the lane's receipt
  is terminal" (#315 - applied; pinning it exposed a latent
  evidence-accounting bug).
- Failure mode: hypothesis without an object ("reviewer should look for
  missing/weak unit tests" - #305, self-refuted).
- Receives: tokmd, ripr, diff, shared-context.
- Never: request proof the budget cannot run without flagging the budget
  (the lane asked for tests on #315 the proof broker then skipped by
  budget; the gap was honest but the ask should name its cost).

### source-route (route truth review)

- User question: do the changed routes, references, and callers all agree
  with the PR's claim?
- Good finding cites: the specific stale reference, include site, or
  sibling path, with the grep that proves it.
- Corpus good example: the stale `profiles/ub-review-self` reference sweep
  on #307; the raw-string style nit with repo precedent (applied inline).
- Failure mode: restating the diff as observation ("9 include_str! sites
  changed").
- Receives: tokmd, ast-grep, diff.
- Never: style opinions without an existing repo precedent to cite.

### correctness (contract-edge review)

- User question: does the changed behavior hold at its contract edges?
- Good finding cites: the exact edge (input shape, state, ordering) and
  the contract line it strains.
- Corpus good example: "a candidate with path Some(\"\") and empty comment
  path would match unexpectedly" (#309); "malformed badge receipts with a
  counts block but different schema silently pass threshold" (#335).
- Failure mode: imported threat models - UB/lifetime/allocation hypotheses
  in a repo whose lints forbid the constructs (maxim 4). This lane is net
  positive only when it frames in contracts, not memory.
- Receives: tokmd, ripr, ast-grep, diff.
- Never: severity language ("critical", "dangerous") without a receipt.

### architecture (boundary review)

- User question: is this the smallest complete fix at the right boundary?
- Good finding cites: the boundary that should have moved, the named
  alternative, and what breaks if scope grows.
- Corpus verdict: lowest-value lane of the day - one affirmation with no
  action and the corpus's one catastrophic wrong refutation. Doctrine:
  affirmations are artifact-only; this lane speaks in the PR body only
  when it can name a different boundary and the cost of the current one.
- Receives: tokmd, ast-grep.
- Never: "no architecture-level action requested" as a posted finding.

### opposition (strongest-objection review)

- User question: what is the strongest substantiated case against this PR?
- Good finding cites: the objection plus the receipt that substantiates it
  - or routes the objection to a deterministic check instead of
  adjudicating it by argument.
- Corpus verdict: generated the right objections, then destroyed value in
  refutation (#309 mirror smell, refuted on an unverified claim, falsified
  within hours). Doctrine: when an objection is mechanically checkable
  (parity, schema equality, count coherence), the lane's output is a proof
  request or a verification question - never a confident refutation. The
  refutation step is the failure mode, not the hypothesis step.
- Receives: tokmd, ripr, ast-grep, shared-context.
- Never: refute an objection with a claim no receipt supports (maxim 3).

### contract-mirror (cross-language parity review) - NEW, corpus-mandated

- User question: did this change move one side of a mirrored contract
  (Rust renderer <-> Python verifier expectation, schema string <-> pinned
  string, count <-> count) without the other?
- Good finding cites: both sides of the mirror with file paths, and which
  side moved.
- Corpus mandate: both mirror-touching PRs (#322, #334) got total silence;
  the drift was caught only by CI. A lane whose packet routes the paired
  files (src/main.rs render/serialize sites + verify-bun-review-artifacts.py
  require_* mirrors) closes the day's largest observed gap.
- Receives: diff, ast-grep, shared-context (with mirror-pair map from the
  repo ledger).
- Never: assert parity holds - only flag that a mirror side moved and name
  the deterministic check that settles it.

### gate-semantics (verdict-surface review) - NEW

- User question: does this change alter what can redden the gate, what
  stays advisory, or what posts - and does the spec/doc text still match?
- Good finding cites: the reason kind / policy key / pass-policy branch
  affected and the SPEC-0003 clause it must agree with.
- Receives: diff, tokmd, shared-context (gate policy excerpt).
- Never: treat model/provider failure paths as gate-relevant.

### spec-honesty (claims-vs-code review) - NEW

- User question: does any doc/spec claim in this diff present intent as
  implemented behavior?
- Good finding cites: the claim line and the code (or its absence) that
  contradicts it. The spec wave's fact-check fleet found 37 of these in
  one day; ten spec PRs then sailed through the gate with one-line
  reviews - no lane reads docs against code.
- Receives: diff, shared-context.
- Never: style or voice notes; only claim-truth.

### security / workflow-ci / policy-receipts (existing specialist types)

Same template; their doctrine follows the lanes already shipped (security:
secret-name guards, injection surfaces, with the verifier's
SECRET_VALUE_NAMES as precedent; workflow-ci: actionlint receipts +
permission/pin changes; policy-receipts: allow.toml/expect discipline,
receipt expiry). Each cites receipts, never severity adjectives.

## Structural doctrine (the body skeleton)

- The decision line must carry signal: "needs reviewer attention" on every
  review is boilerplate; the line should name the strongest item class
  present (blocker / verification question / proof failure) or not exist.
- Proof-only one-line reviews on docs-only passes are ceremony; the
  artifact carries proof results. Prefer the skip receipt.
- Deliberate silence is a posting decision keyed on touched surfaces, not
  an accident of lane emptiness: a diff touching mirror files or gate
  policy that produces no findings deserves an explicit
  "high-risk surface, no objection sustained" item, not silence.
- One claim, one appearance: the compiler dedupes across confirmed /
  parked / refuted sections, and cross-body repetition of orthogonal
  facts (a flaky sensor) is one mention with a receipt.

## Custom lane template

```toml
[[lanes]]
id = "contract-mirror"
role = "Cross-language mirror parity review"
focus = """
For every changed file that has a mirrored counterpart (Rust renderer or
serializer <-> Python verifier expectation), check whether both sides of
the contract moved together. Cite both file paths and which side moved.
If parity is mechanically checkable, emit a proof request or verification
question naming the check; never assert parity by argument.
"""
# receives defaults to the common sensor trio; diff_classes defaults to all
```

A custom lane earns its place when its focus names: the surface it owns,
the evidence a finding must cite, and what it must never do. If the focus
cannot say what a bad finding from this lane looks like, the lane is not
ready.

## Validation commands

```bash
cargo xtask policy-check
python scripts/verify-bun-review-artifacts.py --self-test
# calibration loop: re-mine the gate's reviews after any lane change and
# re-score against the maxims (corpus method: classify every finding as
# applied / useful / noise with ground truth from the PR history)
```

## Implementation PR slices

```text
1. DONE — wire [[lanes]] custom-lane consumption: merge_repo_lanes_into
   (src/lanes.rs:36) + repo_lane_plans (src/plan_build.rs:375), test-pinned
   by repo_lanes_merge_with_defaults_replacement_and_diff_class_gating.
2. DONE — declare this repo's lanes per doctrine: contract-mirror,
   gate-semantics, spec-honesty (.ub-review.toml [[lanes]]); [repo].ledger
   mirror-pair map lives in docs/REVIEW_LEDGER.md.
3. OPEN — compiler structural fixes the corpus demands: decision-line signal,
   cross-section dedupe, deliberate-silence item for high-risk surfaces
   (grep for these concepts returns no implementation).
4. PARTIAL — refutation discipline: append_cross_lane_conflict_observations
   (src/observation_build.rs:128) + summary_finding_has_cross_lane_conflict
   (src/noise.rs:580) cover part of this; cross-lane conflict surfacing
   remains narrowed per issue #147, not closed.
5. OPEN — preset-scoped calibration: Bun false-premise rules do not run on
   non-Bun repos.
6. OPEN — lane library + selection by id + init recommendations.
```

## Release note claim

```text
ub-review lanes are doctrine-governed: each lane type documents what a
good finding cites, what its failure mode is, and what it must never do -
measured against the gate's own review corpus, not asserted.
```
