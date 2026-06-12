# ub-review review ledger

Repo-specific review context injected into lane packets via `[repo].ledger`.
Bounded by the shared-context budget; every entry is a measured risk class or
calibration from this repository's own review corpus
(docs/specs/UB-REVIEW-SPEC-0011-lane-doctrine.md), not generic advice.

## Lint posture (calibration boundary)

This repository sets `unsafe_code = forbid` and denies `unwrap_used`,
`expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro`. UB, lifetime,
aliasing, and allocation-failure threat models do not apply to safe code
here; objections framed in them are preset leakage, not findings. Frame in
contracts: schema strings, count parity, reason kinds, policy receipts.

## Mirror pairs (highest-risk surface)

Both 2026-06-06 contract drifts lived here; check that both sides moved
together when either side changes:

```text
src/main.rs render_follow_up_question_prompt
  <-> scripts/verify-bun-review-artifacts.py follow_up_question_prompt
src/main.rs routed_proof_receipt_excerpt
  <-> scripts/verify-bun-review-artifacts.py routed_proof_receipt_excerpt
src/main.rs is_*_noise rules + is_pr_body_artifact_only_observation
  <-> verifier twins (phrase parity pinned by
      self_test_noise_rule_phrase_parity_with_rust)
src/main.rs build_orchestrator_plan / build_final_orchestrator_plan
  <-> verifier expected_orchestrator_plan / expected_final_orchestrator_plan
src/main.rs FinalCompilerInputArtifact (v2 filter contract)
  <-> verifier require_final_compiler_input
src/main.rs follow_up_resolved_away_candidate_ids + surface matchers
  <-> verifier mirrors (pinned by self-tests)
schema strings ub-review.<name>.vN in Rust
  <-> exact strings the verifier pins
```

When a mirror side moves, the deterministic check that settles parity is the
verifier (self-test or full-tree); flag the pair, do not adjudicate parity by
argument.

## Gate semantics invariants

- Reason kinds: required-proof, tool-gate, required-sensor,
  required-tool-timeout (timed-out required sensor with timeout_sec and
  next_action), blocking-finding, policy; internal is declared-but-unemitted.
  The `[gate.blocking]` opt-ins surface as blocking-finding, never as
  required-proof/tool-gate kinds.
- `review-direct` is a legacy alias of review-byok and never enforces.
- Model and provider failures never redden the gate; missing evidence is
  recorded as missing evidence, never as clean evidence.
- Posting policy is `[gate].post_review_on` alone; `synchronize_mode` is
  inert (#306).

## Known noise classes (do not emit)

- Bun-preset calibrations on this repo: Box::from allocation-failure,
  aliasing-UB on safe code, miri/unsafe-review requests on non-unsafe diffs.
- Meta-chatter: "broad meta-class scan found nothing", restating the diff,
  per-lane repeats of one orthogonal fact (a flaky sensor is one mention
  with a receipt).
- Confident refutations of mechanically checkable objections: route to a
  proof request or verification question instead (the 2026-06-06 corpus's
  worst failure was a wrong refutation of a true mirror objection).
