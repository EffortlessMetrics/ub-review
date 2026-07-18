# ADR 0002: Single required gate and the CI audit wizard

## Status

Accepted

## Context

Historically, `ub-review` ran required proof in `intelligent-ci` mode and wrote
`review/terminal_state.json` (`ub-review.terminal_state.v1`) without turning that
state into a CI verdict. That pre-verdict limitation is the context for this
ADR; the gate outcome artifact, action exit-code contract, and `fail-on-gate`
input are now implemented as described below. A failed required proof produces
a receipt-backed terminal verdict rather than an advisory-only signal.

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
`true` for `intelligent-ci`, `false` for `review-byok`) and a
`gate-outcome-path` output path. Posting failures keep the separate
`fail-on-post-error` input;
posting trouble is not gate trouble.

`gate_outcome.json` minimum fields:

```json
{
  "schema": "ub-review.gate_outcome.v1",
  "conclusion": "pass | fail | inconclusive",
  "terminal_status": "sufficient",
  "reasons": [
    {
      "kind": "required-proof | tool-gate | required-sensor | required-tool-timeout | sensor-finding | reporter-verdict | blocking-finding | policy | internal",
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

`gate_outcome.json` decides "is this compiled review a pass or fail?" A
separate, inert substrate — the current-head watchdog
(`review/gate_watchdog.json`, schema `ub-review.gate_watchdog.v1`, issue #745,
child of #658) — decides "did the current head actually reach an honest
terminal state, or is a green verdict stale, cancelled, or missing?" It is a
pure classifier over frozen observations that never reads live provider state,
publishes a check, or mutates branch protection (consistent with the invariant
that the gate never depends on live CI state), and the same fail-closed rule
holds: only a `terminal` state carries a conclusion, so missing or malformed
head evidence can never serialize `pass`. The watchdog is substrate for the
future stable coordinator, not enforcement, and stays dormant until that
coordinator consumes it. See SPEC-0003 "Current-head watchdog (#745)."

### 2. CI audit wizard

A new read-only subcommand, `ub-review audit-ci`, and a later PR-emitting
wizard mode build the adoption path. The framing is **right-sizing**, not
downgrading: less fixed CI, more useful proof. The deterministic/judgment
split follows repo doctrine:

```text
inventory  (deterministic): parse .github/workflows/*.yml — jobs, triggers,
            matrices, timeouts, permissions, secrets; read branch
            protection/rulesets for which checks are actually required
history    (deterministic): GitHub run history — duration percentiles,
            failure/cancellation/flake rates, rerun→pass patterns,
            runner-minutes/month, and the independent merge-decision signal
            (did this job ever fail when all cheaper jobs passed?)
judgment   (bounded model lane, optional): classify jobs into tiers over the
            deterministic receipts only — the model must not invent facts
audit-ci   read-only report artifacts, no mutation of any repo file
setup-ci   one migration PR (or --print-pr): .ub-review.toml + minimal
            workflow edits + the exact branch-protection change spelled out,
            every recommendation citing its receipt
```

Recommendation tiers:

```text
keep-required                cheap, deterministic, high-signal, foundational
move-to-ub-review-required   still runs every PR, but inside ub-review/gate
                             as [[proof.required]]
adaptive                     run when diff class / paths warrant it
label-gated                  heavy witnesses (Miri, ASAN, mutation) — risk packs
nightly-release              valuable but not per-PR
advisory                     useful signal, never branch-protection material
flag-for-human               security, secrets, compliance, signing, deploy,
                             provenance, or unclear ownership — never
                             auto-right-sized
```

Survivorship rule: "this job caught nothing in 90 days" is not sufficient
evidence to right-size. The report must state both what a job is positioned to
catch (from its triggers and paths) and what it has caught (from history). The
judgment lane weighs both; conservative defaults; the maintainer reviews the PR.

The migration PR cannot change branch protection (admin API, not repo files);
it carries the exact required-checks change in
`docs/ci/branch-protection-change.md` and the PR body. `setup-ci
--apply-branch-protection` is not implemented in the current CLI and is not
part of the adoption path; a future admin-only command may apply the same
documented change, but only as a separate explicit invocation. Full artifact
and PR contracts: [../CI_AUDIT_WIZARD.md](../CI_AUDIT_WIZARD.md).

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
6. setup-ci migration PR mode, calibrated on the receipts from steps 3-5
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
