# Adoption modes: advisory → deterministic-floor → review-forward

This document explains the four ub-review gate modes and the staged path
from advisory reviewer to primary required gate. It complements
[ADOPTION_ADVISORY.md](ADOPTION_ADVISORY.md) (the minimal setup guide).

## The four modes

| Mode | `fail-on-gate` | `[gate].review_forward` | What blocks merge | Model role |
|---|---|---|---|---|
| **Advisory** | `false` | `false` (default) | Nothing (ub-review check is non-required) | Investigates + reports, never blocks |
| **Deterministic-floor** | `true` | `false` | Deterministic failures only (proof, sensors, policy) | Investigates + reports, but model output never blocks |
| **Hybrid** | `true` | `false` | Deterministic failures + tool-gate thresholds | Same as deterministic-floor, with tool findings also blocking |
| **Review-forward** | `true` | `true` | Deterministic + final reporter verdict (changes_requested / uncertain) | Reporter verdict may block under explicit opt-in |

Individual investigation lanes **never** block, post, or execute commands in
any mode. Only brokered proof, configured gate policy, and (in review-forward
mode) the final reporter verdict can affect enforcement.

## Mode 1 — Advisory (default, recommended starting point)

**Purpose:** collect calibration data, build reviewer trust, tune lanes and
prompts without blocking development.

**Setup:**
```yaml
- uses: EffortlessMetrics/ub-review@<SHA>
  with:
    fail-on-gate: 'false'
    posting: review
    model-mode: auto
    profile: gh-runner
```

**What happens:**
- ub-review runs the full same-model review team on every PR.
- Findings post as a grouped PR review (neutral COMMENT event).
- The gate check is non-required (continue-on-error: true).
- Every run produces `review/calibration.json` with headline metrics.

**Promotion criterion:** stay advisory for 10–20 PRs (or 1–2 weeks).
Classify each run using the calibration artifact:
- expected-quiet (correctly silent)
- true-positive (useful finding)
- false-positive (noisy or wrong)
- infra-excluded (runner/secret failure)
- proof-changed-conclusion (proof changed a lane verdict)
- acted-on-comment (reviewer addressed a finding)

Promote to deterministic-floor only when:
- infra-excluded rate is near zero
- false-positive rate is low
- the calibration data shows consistent signal

## Mode 2 — Deterministic-floor (safest primary gate)

**Purpose:** make ub-review the one required CI check enforcing the
deterministic evidence floor without making AI judgment the merge blocker.

**Setup:**
```yaml
- uses: EffortlessMetrics/ub-review@<SHA>
  with:
    fail-on-gate: 'true'
    mode: intelligent-ci
    posting: review
    model-mode: auto
    profile: gh-runner
```

**What happens:**
- The gate check is required (remove `continue-on-error`).
- Gate conclusion is `pass`, `fail`, or `inconclusive`:
  - `pass`: all required evidence passed.
  - `fail`: a deterministic check ran and found a defect (required proof
    failed, required sensor finding, tool-gate threshold exceeded).
  - `inconclusive`: required evidence was unavailable (tool missing, timed
    out, key absent). This is NOT clean — it means "we couldn't check."
- Model review still posts as advisory COMMENT (reporter distillation,
  inline candidates). Model output never feeds the gate verdict.
- `[[proof.required]]` entries in the config declare the must-run floor
  (e.g., `cargo check --locked`, `cargo clippy -D warnings`).

**Promotion criterion:** run deterministic-floor for a calibration window.
Verify:
- Required proof receipts are consistently produced and correct.
- `inconclusive` rate is low (tools are available and fast enough).
- No false `fail` from tool-gate misconfiguration.

Promote to review-forward only when:
- The reporter's verdict (from calibration data) has a high true-positive
  rate on changes_requested / uncertain.
- You have explicit finding classes you want to make blocking.

## Mode 3 — Hybrid (deterministic + tool-gate thresholds)

**Purpose:** block on deterministic evidence AND specific tool-gate threshold
exceedances (e.g., ripr new-unsuppressed exposure gaps).

**Setup:**
```toml
# policy/ub-review.toml
[tools.ripr.gate]
scope = "on-diff"
max_new_unsuppressed = 0

[gate.blocking]
tool_gate_missing_evidence = true  # block when a required tool-gate can't evaluate
```

This is a variant of deterministic-floor — same `fail-on-gate: 'true'`, but
with `[tools.<id>.gate]` thresholds configured. The gate `fail`s when a tool
finding is evaluated and exceeds the threshold. Model output still never feeds
the gate.

## Mode 4 — Review-forward (opt-in model-derived blocking)

**Purpose:** the final reporter verdict may affect the gate, but only under
explicit repo opt-in and only for the final coordinated verdict.

**Setup:**
```toml
# policy/ub-review.toml
[gate]
review_forward = true
```

**What happens:**
- All deterministic-floor behavior is preserved.
- Additionally, the reporter's structured verdict is read from
  `review/threads/reporter/turn-000.json`.
- If the verdict is `changes_requested` or `uncertain`, a `reporter-verdict`
  gate reason is added → the gate `fail`s.
- If the verdict is `clear` or `none`, no gate effect.
- Individual lanes never block — only the final reporter verdict.

**Safety guardrails:**
- `review_forward` defaults to `false`. It must be explicitly set.
- The reporter verdict is probabilistic. It should be calibrated before
  enabling.
- Start with narrow finding classes (proof-backed claims, spec mismatches)
  before broadening.

**Recommended initial blockable classes:**
- PR claims test coverage but proof shows non-discriminating test.
- PR body/spec claims behavior unsupported by code or receipts.
- Changed unsafe/native/FFI surface lacks required safety evidence.

**Do NOT initially block on:**
- Architecture taste.
- Style suggestions.
- Broad "needs more tests" without proof backing.
- Unverified model suspicion.

## Staged promotion checklist

For each repo adopting ub-review as a primary gate:

```
Stage 0: Advisory
  - Pin to a merged-main SHA with all features (post-#713).
  - Run for 10–20 PRs with calibration.json collected.
  - Classify: expected-quiet, true-positive, false-positive, infra-excluded.
  - Goal: false-positive rate < 10%, infra-excluded rate < 5%.

Stage 1: Deterministic-floor
  - Set fail-on-gate: true, mode: intelligent-ci.
  - Configure [[proof.required]] for the repo's must-run checks.
  - Make ub-review/gate a required branch-protection check.
  - Model review stays advisory (reporter posts, never blocks).
  - Goal: zero false fail, low inconclusive rate.

Stage 2: Review-forward (optional, only if calibration supports it)
  - Set [gate].review_forward = true.
  - Start with narrow blockable finding classes.
  - Monitor true-positive vs false-positive on the reporter verdict.
  - Expand blockable classes only with evidence.
  - Goal: reporter verdict adds signal without blocking good PRs.
```

## Metrics to track (from calibration.json)

```
runs
expected-quiet %
true-positive count
false-positive count
infra-excluded count
proof-changed-conclusion count
reporter-questions count
lane-continuations count
acted-on-comment count
```

The two headline metrics:
```
proof_changed_conclusion_count   — does the system buy useful evidence?
useful_comments_acted_on_count   — does the review save reviewer time?
```

Use `cargo xtask calibration-report <dir>` to aggregate across runs.

## Related

- [ADOPTION_ADVISORY.md](ADOPTION_ADVISORY.md) — minimal advisory setup (2 files + 1 secret).
- [RUNTIME_PROFILES.md](RUNTIME_PROFILES.md) — runner-size profiles.
- [POLICY_ALLOWLISTS.md](POLICY_ALLOWLISTS.md) — tool-gate thresholds.
- #678 — the cohort-orchestrator epic.
