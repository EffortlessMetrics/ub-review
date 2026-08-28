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
src/follow_up_routing.rs render_follow_up_question_prompt
  <-> scripts/verify-bun-review-artifacts.py follow_up_question_prompt
src/follow_up_routing.rs routed_proof_receipt_excerpt
  <-> scripts/verify-bun-review-artifacts.py routed_proof_receipt_excerpt
src/noise.rs + src/decision_core.rs is_*_noise rules
  + is_pr_body_artifact_only_observation
  <-> verifier twins (phrase parity pinned by
      self_test_noise_rule_phrase_parity_with_rust)
src/candidate.rs build_orchestrator_plan / build_final_orchestrator_plan
  <-> verifier expected_orchestrator_plan / expected_final_orchestrator_plan
src/review_compiler.rs FinalCompilerInputArtifact (v2 filter contract)
  <-> verifier require_final_compiler_input
src/compiler_reconciliation.rs CompilerReconciliationReceipt (v1 surface
  accounting contract)
  <-> scripts/verify-bun-review-artifacts.py require_compiler_reconciliation
src/issue_broker.rs follow_up_resolved_away_candidate_ids + surface matchers
  <-> verifier mirrors (pinned by self-tests)
src/work_queue.rs work_queue_task_from_sensor (#325 late-phase pending rule)
  <-> verifier require_sensor_work_queue_task_schema (pinned by
      self_test_late_phase_sensor_work_queue_task_stays_pending)
src/task_ledger_artifact.rs strict v1 replay, digest, and snapshot contract
  <-> verifier require_task_ledger_artifacts (cross-language golden pinned by
      self_test_task_ledger_contract)
src/sensor_task_ledger.rs production sensor shadow adapter and artifact writer
  <-> src/sensors/mod.rs fast/late execution boundaries and status receipts
      (focused sensor lifecycle tests plus dry-run CLI artifact inventory)
schema strings ub-review.<name>.vN in Rust
  <-> exact strings the verifier pins
```

When a mirror side moves, the deterministic check that settles parity is the
verifier (self-test or full-tree); flag the pair, do not adjudicate parity by
argument.

## Retained authority incidents

`fixtures/authority-incidents/manifest.json` indexes the byte-identical minimal
artifact corpus from exact-head hosted PRs #915, #916, and #921. The generic
loader in `tests/authority_incidents.rs` owns path confinement, file inventory,
SHA-256, size-budget, secret/private-payload, and evidence-pointer checks. #957
must consume the manifest rather than hard-code incident directories. These are
historical contradiction inputs, not current schema authority or passing
goldens; see `docs/AUTHORITY_INCIDENT_FIXTURES.md`.

## Gate semantics invariants

- Reason kinds: required-proof, tool-gate, required-sensor,
  required-tool-timeout (timed-out required sensor with timeout_sec and
  next_action), blocking-finding, policy; internal is declared-but-unemitted.
  The `[gate.blocking]` opt-ins surface as blocking-finding, never as
  required-proof/tool-gate kinds.
- `review-direct` is a legacy alias of review-byok and never enforces.
- Model and provider failures never redden the gate; missing evidence is
  recorded as missing evidence, never as clean evidence.
- Posting policy is `[gate].post_review_on` alone; legacy
  `synchronize_mode` is stripped with a deprecation `PolicyError` (#306).

## Known noise classes (do not emit)

- Bun-preset calibrations on this repo: Box::from allocation-failure,
  aliasing-UB on safe code, miri/unsafe-review requests on non-unsafe diffs.
- Meta-chatter: "broad meta-class scan found nothing", restating the diff,
  per-lane repeats of one orthogonal fact (a flaky sensor is one mention
  with a receipt).
- Confident refutations of mechanically checkable objections: route to a
  proof request or verification question instead (the 2026-06-06 corpus's
  worst failure was a wrong refutation of a true mirror objection).
