# UB-REVIEW-SPEC-0001 — Release surface and product boundary

Status: authored 2026-06-06 (release surface spec wave, docs-only).
This umbrella spec names the product surfaces, fixes the boundary language,
sets the claims a release may make, and indexes the use-case specs. It does
not change behavior; every maturity label below cites the code or issue that
proves it.

## Product definition

```text
ub-review is the intelligent targeted CI gate.

It runs repo-mandated checks, prepares evidence, coordinates model lanes,
runs targeted proof, and emits one gate decision plus useful review feedback.
```

It is not an AI review bot, a CI wrapper, a tool dump, or a model swarm. The
release question it must answer:

```text
How does a repo adopt this safely,
what artifacts are stable,
what can block a PR,
what is advisory,
what can users build against,
and what claims must never be made?
```

## The boundary

```text
ub-review orchestrates.
Sensors instrument.
The proof broker runs commands.
The compiler posts.
The gate decides.
```

Corollaries the specs hold everywhere:

- instruments (`tokmd`, `ripr`, `unsafe-review`, `cargo-allow`, `ast-grep`,
  `actionlint`, coverage) emit artifacts; ub-review decides how artifacts
  affect review and gate behavior; sensor defects are fixed upstream, never
  silently absorbed;
- models investigate; they never prove, and model output never feeds the gate
  verdict directly (`proof_request_is_gate_required` requires the
  `intelligent-ci-policy` lane — a model lane marking `required = true`
  cannot block);
- only the proof broker runs local commands; only the compiler posts; missing
  evidence is recorded as missing evidence, never as clean evidence.

## The nine surfaces

| # | Surface | Spec | User question | Maturity |
|---|---------|------|---------------|----------|
| 1 | `review-byok` | 0002 | Useful AI PR review with my own key and minimal setup? | production (Bun pin live) |
| 2 | `intelligent-ci` gate | 0003 | Can this replace my required PR CI gate? | production on this repo (sole required check; red/green proven, roadmap item 26) |
| 3 | Artifact contract | 0004 | What files can I build automation against? | production, contract enforced by the artifact verifier |
| 4 | Sensor/tool integration | 0005 | How do the six sensors and coverage feed the gate? | production — registry, receipts, and `[tools.*.gate]` thresholds production-proven (#335: evaluated + two real blocks, PR #342/#346); per-finding gap detail in `sensors/ripr/exposure-gaps.json` (#347 closed) |
| 5 | Provider/cache/fallback | 0006 | How do I configure model providers safely and cheaply? | partial — CLI-flag surface production (preflight + runtime fallback, prompt caching on by default); `[providers].policy`, per-provider `max_concurrency`, and 429/timeout/5xx wave shedding executed; prompt-cache config remains open |
| 6 | `audit-ci` | 0007 | Which CI should stay required, become adaptive, or move? | v0 — deterministic judgment only; local permissions/secrets receipts implemented; branch-protection/ruleset required-check receipts populate when readable |
| 7 | `setup-ci` | 0008 | Can ub-review open the migration PR? | partial — `--print-pr` renders the plan from audit receipts, including exact required-check remove instructions only when audit-ci proved them; `--open-pr` opens the new-files-only migration PR (config + pinned workflow + plan doc; refuses repos with an existing config). Existing-CI edits and branch-protection mutation remain contract intent |
| 8 | `bun-ub` preset | 0009 | How does the Bun UB hunt use this gate? | production at the pinned SHA; consumer contract live |
| 9 | Release binary / Action install | 0010 | How do I install this without rebuilding the world? | partial - source-build fallback and doctor pins are production; release binary fast path is implemented but awaits the first published archive (#343) |

## Claims

A release may claim:

```text
ub-review provides a repo-configured intelligent CI gate.
ub-review emits stable gate, proof, tool, resource, and review artifacts.
ub-review can run BYOK review lanes over prepared evidence.
ub-review can fold required CI checks into one gate.
ub-review can audit existing CI and recommend right-sizing.
```

A release must never claim:

```text
proves code correct
proves UB-free
replaces all security tooling
runs every possible test
auto-downgrades CI safely
model findings are proof
```

## Gate semantics every spec inherits

Red means a real gate reason, with a receipt pointer:

```text
required proof failure
required tool threshold failure
required evidence gap (intelligent-ci mode only)
blocking finding (repo policy opt-in)
policy parse error on a policy the repo wrote
```

Never red:

```text
model/provider failures (including fallback use)
optional missing evidence
successful tool status
artifact-only observations
lane metadata
```

Fail-closed: the gate check recognizes exactly the string `pass`; missing,
null, or case-drifted conclusions are failures (src/main.rs `gate-check`).
`fail-on-gate` resolves `auto` → true only for `intelligent-ci`; `review-byok`
stays non-blocking by default (src/cli.rs FailOnGate).

## Spec template

Every use-case spec answers, in this order:

```text
Purpose
User question
Lifecycle moment
Consumer
Inputs
Output artifact / user surface
Required fields
Advisory vs blocking behavior
Fail-closed behavior
Trust boundary / non-claims
Validation commands
Implementation PR slices
Release note claim
```

And explicitly:

```text
What can a user rely on?
What can break the gate?
What is only advisory?
What is visible in the PR?
What is artifact-only?
What does success look like in ten minutes?
```

## Known reserved/inert surfaces (specs must not paper over these)

- `[providers]` config section: partly wired. `policy` selects provider
  routing when CLI/env policy is `auto`, and provider `max_concurrency` caps
  model-lane waves. Descriptive keys inside it (`env`, `model`, `role`,
  `models`, `prompt_cache`, `fallback_for`) remain documentation of intent,
  not wired behavior.
- Legacy `[gate].synchronize_mode`: removed from the config contract because
  it never controlled posting (#306). Configs that still set it receive a
  deprecation `PolicyError`; posting on quiet passes is governed solely by
  `[gate].post_review_on`.
- `[tools.ripr.gate] max_new_unsuppressed`: production-enforcing since #335
  (#316 closed) — the sensor persists `sensors/ripr/gate-decision.json` and
  the threshold has blocked two real PRs (#342, #346). Remaining honesty
  per-finding gap detail ships next to the badge receipt
  (`sensors/ripr/exposure-gaps.json`, verifier-checked; #347 closed).
- `setup-ci`: `--print-pr` is implemented (slice 1 - migration plan from
  audit receipts, fail-closed, round-trip-checked generated config). The
  PR generator, repo-file generation, and `--apply-branch-protection` stay
  contract-first spec only.

## Spec index and authoring sequence

```text
UB-REVIEW-SPEC-0001  release surface and product boundary   (this doc)
UB-REVIEW-SPEC-0002  review-byok surface
UB-REVIEW-SPEC-0003  intelligent-ci gate surface
UB-REVIEW-SPEC-0004  artifact contract surface
UB-REVIEW-SPEC-0005  sensor and tool integration surface
UB-REVIEW-SPEC-0006  provider, cache, and fallback surface
UB-REVIEW-SPEC-0007  audit-ci surface
UB-REVIEW-SPEC-0008  setup-ci surface
UB-REVIEW-SPEC-0009  bun-ub preset surface
UB-REVIEW-SPEC-0010  release binary / Action install surface
IMPLEMENTATION_PLAN  routes all open release work through these specs
```

PR sequence: 0001 alone; 0002+0003; 0004+0005; 0006; 0007+0008; 0009; 0010;
then one implementation plan routing all open release work (including issues
#306, #310, #311 remainder, #312, #314, #316–#321) through these specs. No
implementation lands inside the spec wave.

## Validation commands (for this wave)

```bash
cargo xtask policy-check          # docs are governed surfaces (allow.toml glob)
python scripts/verify-bun-review-artifacts.py --self-test
cargo test --bin ub-review --locked   # doc-referenced contracts stay pinned
```
