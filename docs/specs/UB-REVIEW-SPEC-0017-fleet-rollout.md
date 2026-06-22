# UB-REVIEW-SPEC-0017 — fleet / multi-repo rollout

Status: authored 2026-06-22 (Wave 6+, docs-only).
Umbrella: [UB-REVIEW-SPEC-0001](UB-REVIEW-SPEC-0001-release-surface.md).
Related: [SPEC-0002](UB-REVIEW-SPEC-0002-review-byok.md) (BYOK tiers),
[SPEC-0013](UB-REVIEW-SPEC-0013-profile-input-schema.md) (profile schema),
[SPEC-0008](UB-REVIEW-SPEC-0008-setup-ci.md) (migration PR mode).
Maturity: forward-looking — no fleet orchestration code exists in `ub-review`
(PRODUCT_STATE PR 12: NOT STARTED). This spec frames the onboarding contract
so the eventual rollout (ROADMAP item 27, ADR-0002 steps 5-6) has a target.

## Purpose

Frame the fleet / multi-repo rollout posture: how a new repo onboards, how
profiles propagate, how the single-gate model extends across repos, and what
the no-model tier provides. PRODUCT_STATE PR 12 is not started; ADR-0002
rollout steps 5-6 name rolling out to the owner's other Rust repos. This spec
makes the onboarding contract explicit.

## Onboarding contract

ROADMAP item 27 acceptance criteria:

- A new repo onboards with **~10 lines of TOML** and zero model keys.
- `model-mode: off` is a fully supported tier with a useful gate.
- Each onboarded repo keeps **one required check only** (`ub-review/gate`).

### The ~10-line onboarding shape

A minimal consumer config (the no-model tier):

```toml
review_profile = "rust-test-proof"
profile = "gh-runner"

[gate]
required_check = "ub-review/gate"
```

The repo adds the composite action to its CI (one workflow file) and the
`.ub-review.toml` above. No API keys, no model spend — the gate runs
deterministic proof + advisory sensors only.

### The no-model tier

`model-mode: off` (SPEC-0002) means:

- No provider HTTP calls — MiniMax/OpenCode keys are absent (missing evidence,
  not a failure).
- The gate runs: `cargo check`/`test`/`clippy`/`doc` (required proof), `ripr`
  + `unsafe-review` + `cargo-allow` (advisory sensors by trigger), and the
  policy-check gate.
- Review posting is advisory (artifact-only) unless a model key is provided.

This tier makes adoption cost near zero and makes model spend an **upgrade
decision** rather than an entry requirement (ADR-0002).

## Profile propagation

Profiles propagate via the `review_profile` + `profile` keys (SPEC-0013):

- **`review_profile`** names the review posture (lane set, body policy, tool
  triggers). The initial fleet profiles are:
  - `bun-ub-v0` — the Bun UB hunt (production, pinned).
  - `rust-test-proof` — generic Rust (ROADMAP item 27; planned).
  - Future: `js-native-boundary`, `github-action-security` (ROADMAP line 843-846).
- **`profile`** names the runtime resource envelope (`gh-runner`,
  `gh-runner-standard`, `gh-runner-full`, `cx23`/`cx33`/`cx43`).

A repo composes its posture by choosing one of each; repo-local `[[lanes]]`,
`[[proof.required]]`, and `[tools.*]` overrides layer on top (SPEC-0011,
SPEC-0013 merge precedence).

## The single-gate model across repos

Each repo keeps exactly one required check: `ub-review/gate`. The gate's
verdict surface (SPEC-0003) is repo-agnostic — the same `gate_outcome.v1`
schema, the same reason kinds, the same `fail-on-gate` resolution. Fleet
rollout does **not** introduce per-repo gate variants.

Branch protection per repo: require `ub-review/gate` (the setup-ci migration
PR mode, SPEC-0008, emits the branch-protection-change doc; the repo owner
applies it). `setup-ci --apply-branch-protection` is not implemented (ADR-0002)
— branch protection is a human-gate action.

## Fleet monitoring posture

No fleet-level orchestration code exists in `ub-review`. The owner monitors
multiple repos' gates via:

- Per-repo `ub-review/gate` check status (GitHub branch protection).
- Per-repo `quality-receipt.json` / `quality-trend.json` artifacts
  (advisory; written by the `quality-backfill` / `quality-github-*` commands).
- Per-repo `cost_receipt.json` (LEM spend; advisory, not enforced — see
  SPEC-0013's `learned-budgets` gap).

A future fleet dashboard (not in scope for `ub-review` itself) would aggregate
these artifacts across repos.

## Rollout sequencing (ADR-0002 steps 3-6)

1. **Single-repo dogfood** (done — this repo, ROADMAP item 26).
2. **rust-test-proof profile** (ROADMAP item 27) — author
   `profiles/rust-test-proof.toml`, onboard one of the owner's other Rust
   repos (tokmd / ripr / unsafe-review / cargo-allow).
3. **setup-ci migration PR mode** (ROADMAP item 28, SPEC-0008) — calibrate on
   the receipts from steps 3-5.
4. **Fleet** (this spec) — extend to the remaining owner repos, then external
   consumers.

## Non-claims

This spec does **not** claim:

- **That fleet orchestration code exists.** It does not (PRODUCT_STATE PR 12:
  NOT STARTED). This spec frames the target.
- **That a repo can onboard without reading the docs.** The ~10-line TOML is
  the config shape; onboarding also requires the workflow file and branch
  protection setup (SPEC-0008).
- **Cross-repo evidence aggregation.** Each repo's evidence is independent;
  no fleet-level proof or review synthesis exists.

## Related

- ROADMAP items 26-28 (single-gate dogfood, rust-test-proof profile, setup-ci
  migration).
- ADR-0002 (single-gate + CI audit wizard; rollout steps 5-6).
- PRODUCT_STATE PR 12 (fleet rollout — NOT STARTED).
