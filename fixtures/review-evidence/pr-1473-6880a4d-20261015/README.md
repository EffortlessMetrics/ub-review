# Independent renderer/artifact review evidence — 2026-10-15

Evidence owner: independent reviewer delegated by Rook; implementation and follow-through remain with the existing assembly owner. This is an evidence-only handoff, not a repair or integration candidate. Data/Markdown are owned technical fixtures under the existing `non-rust-fixtures` policy category (`ub-review/tests`). No executable, cache, credential, private file-service reference, or workflow is included.

## Frozen subjects

| Subject | Head | Tree | Base |
|---|---|---|---|
| #1473 | `6880a4deeccc93d2e795ebd85da85b197cb3c32d` | `f56e29d47f43628c87ad86150dfa54ad6647b815` | `565e2a8e77a132351580d3f44bae43b6de0b2a70` |
| #1470 | `00e314080637bacb50ebf49332af9388b3a534b6` | `4f85bd233b3097c48500a58bc02fb73c2ee0a370` | `17a6528f68601e6aaed5d6b88053f44eb57fa9b6` |

Current AGENTS.md, native workflow, review-change skill, source and substantive review threads were inspected before judgment. Both heads remained unchanged and both PRs were Draft at the publication preflight.

## Reproduced findings

1. `src/review_text.rs:257` / `src/render.rs:141`: current worker `failed` and `sanitizer_ub_detected` receipts yield a clean artifact decision and `None recorded` under missing or failed evidence. `canonical-worker/` retains actual production-worker receipts with matching 40-hex heads and candidate-head revision stamps, plus compiled bodies. Only the subprocess runner is controlled: nightly preflight succeeds and the proof command exits 1. This establishes failure loss without claiming actual UB or running Miri/ASAN.
2. The same artifact selectors and `src/render.rs:342` consume stale `head_failed`. A stale failure alone or alongside a current `head_passed` causes `Needs focused proof failure resolved before upstream` and public text `This is a current failure, not a red/green witness.` Current `head_failed` is the paired positive control. See `representative/head_failed-{current,stale,mixed}`.
3. `src/render.rs:65`: current proof-only `head_failed` produces both the failure decision and `## No blocking finding after checking`. See the current-failure artifact above.

Artifact-decision acceptance is withheld on these frozen heads. Narrow repair: share failure classification for artifact truth (`head_failed`, `failed`, `sanitizer_ub_detected`), select current-head receipts before decision/evidence selection for both audiences, and make the heading consistent with that decision. Retain stale raw rows as audit evidence. Extend #1470 tests with worker vocabulary, current/stale/mixed heads and heading consistency; retain intentional public-suppression and configured-gate semantics.

## Executed scope and controls

Each frozen subject ran 105 cases through the production worker-result classifier, production receipt writer, JSON round-trip, review compiler and both renderers: 8 kinds × 4 statuses × 3 head arrangements, plus 3 result controls × 3 arrangements. Outputs were identical; `matrix-summary.json` retains original full-matrix hashes and representative rows. Raw representative fixtures and bodies are byte copies. Timed-out evidence correctly becomes incomplete, and a discriminating proof remains publishable. Canonical worker → compiler execution additionally confirms failure loss for focused-test, Miri and sanitizer receipts.

Existing child `proof_only_` tests passed 4/4 despite the findings, and passed 4/4 again after exact restoration. The pristine parent artifact-decision control passed 1/1 in a fresh target directory. No production mutant was needed because unchanged production already supplies counterexamples. Temporary source edits were test harnesses only; both detached review checkouts were restored clean.

The initial parent and child matrix compiles used one reviewer-owned target sequentially; each log explicitly records compilation from its intended checkout. A later attempted parent control reused the child's cached binary and was excluded. The retained `parent-pristine-control.log` is its fresh-target replacement. No author's target was used.

## Reproduction commands

Linux x86_64; Rust 1.95.0, Cargo 1.95.0; two build jobs. Work in separate detached reviewer checkouts. Set `CARGO_HOME`/`RUSTUP_HOME` to your installed toolchain and use distinct `CARGO_TARGET_DIR` values for each checkout/experiment.

1. Append `counterexample-module.rs` to `src/decision_core.rs` at either frozen head. Its output root is `/workspace/review-evidence/parent-matrix`; change only that root to `child-matrix` for the dependent subject. Run:
   `cargo test --bin ub-review --locked independent_worker_and_head_matrix -- --nocapture`
2. At the child head, insert `canonical-worker-test.rs` inside `worker_proof_tests` immediately before `worker_preflight_and_proof_are_distinct_revision_bound_tasks`. Run:
   `cargo test --bin ub-review --locked independent_canonical_worker_failure_into_compiler -- --nocapture`
3. Restore submitted source, then run on the child:
   `cargo test --bin ub-review --locked proof_only_ -- --nocapture`
4. In a fresh parent target directory, run:
   `cargo test --bin ub-review --locked artifact_decision_accounts_for_proof_only_failure_and_missing_evidence -- --nocapture`

Harness paths are local output directories, not dependencies or private artifact references. The three `*-tree.txt` files identify the original test-only experiment trees. Fixture heads `HEAD` / `old-head` are deliberate identities in the matrix; canonical worker fixtures additionally cover real commit IDs and revision stamps.

## Limits

This is local renderer/artifact evidence. It does not establish a configured CI gate bypass, full-run stale-artifact import/reuse, successful publication, delivered review, full-suite qualification or promotion authority. Narrow public worker-result suppression can be intentional; an empty public body alone is not the false-pass finding. `skipped` also rendered clean at the classifier boundary, but no worker spawn-error finding is claimed. The implementation owner owns repairs and fresh exact-head validation/review. No thread was resolved and no Ready/merge transition is authorized by this receipt.
