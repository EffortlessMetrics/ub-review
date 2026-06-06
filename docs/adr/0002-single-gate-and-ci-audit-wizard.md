# ADR 0002: Single required gate and the CI audit wizard

## Status

Accepted

## Context

`ub-review` already runs required proof in `intelligent-ci` mode, routes required
sensor gaps to the `failed-to-review` terminal state, and writes
`review/terminal_state.json` (`ub-review.terminal_state.v1`). What it does not do
is turn that state into a CI verdict: there is no gate outcome artifact, no
action exit-code contract, and no `fail-on-gate` input. Today a failed required
proof produces evidence, not a red check. The product is an advisory reviewer
wearing a gate's name.

The adoption problem is the mirror image. Repositories already have CI: workflow
files that encode years of accreted caution — jobs added after incidents, matrix
entries nobody prunes, required checks that have not independently changed a
merge decision in months. Asking a repo to add yet another check competes with
that noise. Showing a repo, with receipts, which of its mandatory jobs earn
their runner minutes is a different conversation.

The operating goal for this lane: `ub-review/gate` becomes the standard and only
required PR check across the owner's repositories — efficient enough,
configurable enough, and good enough out of the box to deserve that position.

## Decision

Two product surfaces, one contract.

### 1. Gate verdict surface

`ub-review run` writes `review/gate_outcome.json`
(schema `ub-review.gate_outcome.v1`), derived deterministically from the
terminal state, required-proof receipts, and tool gate outcomes. No model output
feeds the verdict directly; models influence the gate only through validated
candidates and proof receipts that survive compilation.

Conclusion mapping from `ub-review.terminal_state.v1`:

```text
terminal status              gate conclusion
---------------------------  -------------------------------------------
sufficient                   pass
artifact-only                pass
needs-reviewer-attention     pass        (review feedback, not a blocker)
needs-reviewer-attention     fail        (only when a blocking condition below holds)
failed-to-review             fail
```

The gate goes red only for:

- required baseline proof failure (`[[proof.required]]` matched and failed);
- required tool threshold failure (`[tools.*.gate]` exceeded);
- required sensor missing, skipped, failed, or timed out when its trigger matched;
- a blocking finding or blocking verification question (explicitly marked
  `blocking = true` by repo policy, never by model output alone);
- an internal failure that makes the review untrustworthy.

The gate never goes red for: non-required missing evidence, successful tool
status, model/provider fallback or failure, lane rosters, artifact-only
observations, or generic caveats. Missing model keys degrade the review; they do
not fail the gate.

Exit-code contract: `ub-review run --mode intelligent-ci` exits non-zero when
the gate conclusion is `fail`. The action exposes `fail-on-gate` (default
`true` for `intelligent-ci`, `false` for `review-byok`) and a `gate-outcome`
output path. Posting failures keep the separate `fail-on-post-error` input;
posting trouble is not gate trouble.

`gate_outcome.json` minimum fields:

```json
{
  "schema": "ub-review.gate_outcome.v1",
  "conclusion": "pass | fail",
  "terminal_status": "sufficient",
  "reasons": [
    {
      "kind": "required-proof | tool-gate | required-sensor | blocking-finding | internal",
      "id": "cargo-check",
      "detail": "exit 101 after 41s",
      "receipt": "review/proof_receipts.json#cargo-check"
    }
  ],
  "required_proof": {"matched": 3, "passed": 3, "failed": 0, "skipped": 0},
  "tool_gates": {"evaluated": 2, "passed": 2, "failed": 0},
  "evidence_gaps_blocking": 0,
  "evidence_gaps_advisory": 2
}
```

Every `fail` reason carries a receipt pointer. A red gate with no receipt is a
bug in the gate, not a finding.

### 2. CI audit wizard

A new read-only subcommand, `ub-review audit-ci`, and a later PR-emitting
wizard mode build the adoption path. The deterministic/judgment split follows
repo doctrine:

```text
inventory  (deterministic): parse .github/workflows/*.yml — jobs, triggers,
            matrices, timeouts, secrets; read branch protection for which
            checks are actually required
evidence   (deterministic): GitHub run history — per-job duration percentiles,
            PR failure rate, flake rate (fail→rerun→pass), failure correlation
            (did this job ever fail independently?), runner-minutes/month
judgment   (bounded model lane, optional): classify each job into a tier
output     (audit-ci): one report artifact, no mutation
output     (wizard):   one PR with .ub-review.toml + workflow edits + the
            branch-protection change spelled out, every recommendation citing
            its receipt
```

Recommendation tiers:

```text
keep-required    cheap, deterministic, high-signal (fmt/check/clippy/focused tests)
adaptive         expensive, diff-class-dependent — routed through the proof
                 planner and [[proof.required]] / required_if triggers
label-gated      heavy witnesses (Miri, ASAN, mutation) — risk packs
flag-for-human   security-relevant jobs (CodeQL, secret scanning, provenance)
                 are never auto-downgraded, only annotated
```

Survivorship rule: "this job caught nothing in 90 days" is not sufficient
evidence to downgrade. The report must state both what a job is positioned to
catch (from its triggers and paths) and what it has caught (from history). The
judgment lane weighs both; conservative defaults; the maintainer reviews the PR.

The wizard PR cannot change branch protection (admin API, not repo files); it
carries the exact required-checks change in its body, or a follow-up step
applies it with explicit permission.

### 3. Out-of-the-box posture

For the single-gate ambition to be honest, `model-mode: off` must be a
supported product tier, not a degraded run: deterministic sensors + required
proof + gate verdict + artifact-only summary, zero model keys, zero tokens.
Model lanes are additive judgment, never load-bearing for the verdict.

## Rollout

```text
1. gate_outcome.json + exit-code contract + fail-on-gate     (this repo)
2. repo-configured gate policy: [tools.*.gate] thresholds,
   blocking markers, required_if triggers
3. audit-ci read-only report, run against this repo first
4. dogfood: ub-review/gate becomes the only required check on
   EffortlessMetrics/ub-review; ci.yml jobs fold into the gate
5. generic rust-test-proof profile; roll out to the owner's other
   Rust repos (tokmd, ripr, unsafe-review, cargo-allow lanes)
6. wizard PR mode, calibrated on the receipts from steps 3-5
```

Each step is one review-fast PR with verifier coverage for any new artifact
contract.

## Consequences

- A required proof failure becomes a red check, which makes `ub-review`
  actual CI rather than an advisory reviewer.
- The verdict path is deterministic and receipt-backed end to end, so a red
  gate is always explainable from artifacts.
- The audit wizard converts the evidence-machine doctrine
  (`policy/ci-budget.toml`, `ci-lanes.toml`, `ci-risk-packs.toml`) from
  self-governance into the onboarding product motion.
- Dogfooding as the only required check forces the gate to be trustworthy
  before any external rollout claim.
- The no-model tier keeps adoption cost near zero and makes model spend an
  upgrade decision instead of an entry requirement.
