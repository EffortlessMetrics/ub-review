# Learned budgets

The CI cost-discipline model is documented across three surfaces:

- **`policy/ci-budget.toml`** — the LEM (Linux-equivalent-minute) bands and
  rate. Default `preferred_default_lem = 25`, `default_limit = 35`,
  `elevated_limit = 75`, `hard_limit = 125`, `linux_minute_rate_usd = 0.008`.
- **`docs/ci/lem-budgeting.md`** — the normative cost vocabulary: the planning
  loop (`changed files + labels + cargo graph + historical timing` →
  `ci-plan.json` → selected lanes → CI actuals → learned estimates → budget
  warnings/guardrails`) and the five budget bands.
- **`policy/ci-lanes.toml`** and **`policy/ci-risk-packs.toml`** — the lane
  and risk-pack contracts that labels select into. (Both are currently seed
  contracts; see `docs/ci/cost-and-verification-policy.md` for the maturity
  model and the gap between documented and enforced posture.)

## What is not yet built

The planning loop's **learned-estimates feedback** leg — feeding actual CI
LEM spend back from `ci-costs.v1` receipts into per-lane timing estimates
that tighten future budget warnings — is documented in `lem-budgeting.md`
but **not enforced by any workflow or xtask step**. No machinery computes
LEM or blocks on budget today; the only enforced cost ceiling is GitHub's
`timeout-minutes` and `.ub-review.toml` `target_minutes`/`hard_timeout_minutes`.

This is the gap tracked in the repo tracker (UB-18, issue #601): the
cost-discipline doctrine is real and documented, but the label-gated
enforcement and learned-budget feedback are future work. When that work
lands, this document should record the learned per-lane estimates and the
guardrail thresholds they produce.
