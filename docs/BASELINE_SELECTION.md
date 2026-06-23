# Baseline: current proof selection vs existing CI

This document records what `ub-review`'s deterministic planner selects today
(baseline for Order 0 of epic #655), so the impact planner (Order 1) has a
comparison point.

## What the deterministic planner selects today

### Diff-driven candidates (no model keys needed)

The deterministic planner (`focused_test_candidates_from_diff` in
`src/proof/tasks.rs:168`) selects test candidates from the diff using:

- **Bun test files only**: paths matching `test/` or `tests/` prefix with
  suffix `.test.{ts,tsx,js,jsx,mjs,cjs}` (via `is_bun_focused_test_file`).
- For Rust repos: **zero deterministic test candidates** from source-file
  changes. The planner does not map `src/config.rs` → owning package →
  reverse-dependent test crates.

### Model-lane proof requests (needs API key)

Model lanes can emit `focused-test` and `focused-build` proof requests
(via the `proof_request` artifact). These are allowlisted, grouped,
deduplicated, and executed by the proof broker. This is the ONLY path by
which Cargo tests are selected today — entirely model-driven, not
deterministically planned.

### Required proof (repo policy)

`.ub-review.toml` declares 3 required proof commands:

| ID | Command | Languages | Diff classes |
|---|---|---|---|
| `cargo-check` | `cargo check --workspace --all-targets --locked` | rust | all |
| `cargo-doc` | `cargo doc --workspace --no-deps --locked` | rust | all |
| `policy-check` | `cargo xtask policy-check` | all | all |

These run unconditionally on every Rust-source PR (they're wildcard-scoped).

### Sensors (deterministic, trigger-based)

15 tools are configured, of which 12 are enabled. Each fires based on
trigger rules (source-changed, rust-behavior-or-tests-changed,
workflow-changed, etc.):

| Tool | Class | Default trigger | Requires lease |
|---|---|---|---|
| cargo-fmt | static | rust-behavior-or-tests-changed | no |
| cargo-check | build | rust-behavior-or-tests-changed | no |
| cargo-test | test | rust-behavior-or-tests-changed | no |
| cargo-clippy | static | rust-behavior-or-tests-changed | no |
| cargo-doc | build | rust-behavior-or-tests-changed | no |
| artifact-verifier | static | diff | no |
| tokmd | packet | diff | no |
| cargo-allow | static | rust-behavior-or-tests-changed | no |
| ripr | search | source-changed | no |
| unsafe-review | security | unsafe-or-native-risk-changed | no |
| ast-grep | search | source-changed | no |
| actionlint | workflow | workflow-changed | no |
| coverage | coverage | diff | **yes** (`allow-heavy`) |

Disabled: semgrep, osv-scanner, cargo-audit, cargo-deny, shellcheck,
cppcheck, zizmor, gitleaks (8 tools).

## What this repo's existing CI runs

The repo runs **one** required check: `ub-review/gate`. This gate internally
runs the entire tool registry above plus the required proof commands. There
is no separate CI workflow — the gate IS the CI.

For comparison, the repo's gate includes:
- All 13 enabled tools (trigger-gated)
- 3 required proof commands (always on Rust diffs)
- Model-lane investigation (10 Bun UB lanes, when API key available)
- Proof broker execution (focused tests from diff + model requests)
- Follow-up orchestrator (bounded single pass)

## What the impact planner (Order 1) should add

The gap: for a Rust source change (e.g., `src/config.rs`), the deterministic
planner today selects **zero focused tests**. It only knows to run
`cargo check --workspace` and `cargo test --workspace` (via the required
proof and the cargo-test tool). It cannot say: "config.rs is in the
ub-review package; the ub-review package has test targets; these specific
test functions exercise config parsing; run focused red/green on those."

The impact planner should add:
1. Cargo workspace/package graph parsing
2. Changed-file → owning-package resolution
3. Reverse-dependency closure (which packages depend on the changed one)
4. Test-target enumeration for affected packages
5. Candidate ranking with selection reasons
6. A shadow-mode artifact (`impact_plan.v1`) comparing selected vs unselected

## Baseline measurement (for future comparison)

When the impact planner ships in shadow mode, record per-PR:

| Metric | Current value | Target |
|---|---|---|
| Deterministic test candidates (non-Bun) | 0 | >0 for Rust source changes |
| Focused tests selected by reason | 0 | Every candidate has a reason |
| Package-graph awareness | none | workspace + reverse deps |
| Selection artifact | none | `impact_plan.v1` |

This baseline is the comparison point for the impact planner's shadow-mode
evaluation.
